// F1 Manager 24 co-op bridge.
//
// Loaded by our pak override of Content/UIGameface/*.html. It has exactly one job: let an
// external process fire the same Gameface events the game's own buttons fire, so two clients can
// press Continue, advance the calendar and enter a race weekend together.
//
// Every button in this game is one of these calls:
//   MainMenu.js              onSelectContinue -> Engine.sendEvent('ContinueGame')
//   LoadSaveScreen.js:128    onSelectLoadData -> Engine.sendEvent('LoadGame', slotName)
//   TimeProgressionPanel.js  onAdvance        -> Engine.sendEvent('AdvanceToNextEvent')
//   CalendarContents.js:278                    -> Engine.sendEvent('NextMonthSelected')
// and Engine.sendEvent is `engine.trigger(name, ...args)`. A remote trigger of the same name is
// byte-for-byte the call the button makes: no input injection, no code patching, no hooking an
// address that moves every session, and no debugger that can wedge the process.
//
// Transport is a plain async GET to a loopback command server the operator runs. This is not a
// new capability for the UI: the shipped localisation loader uses XMLHttpRequest the same way.
//
// Facts established against a live game, each of which shaped this file:
//   * Synchronous XHR is blocked in Cohtml, so every request here is async.
//   * ContinueGame and LoadGame destroy the document that fired them, and Cohtml drops any
//     request still in flight during that teardown. So the report of "I ran it" is therefore
//     sent BEFORE the event, not after: waiting until after would always lose it.
//   * That means a synchronous ack is impossible for those two. The proof they ran is the page
//     change itself, which the next document reports on its first heartbeat.
//
// Failure policy: every network error is swallowed and retried with backoff. A missing command
// server must never affect the UI, so nothing here throws out of the poll loop.

(function () {
    'use strict';

    // Hot reload. Every change so far needed a game restart on both machines, which is the most
    // expensive thing in this project. The bridge re-fetches its own source when a revision file
    // changes and re-evaluates it, so behaviour can be changed by writing a file and waiting.
    //
    // A reload is a full re-run of this IIFE, so nothing may depend on state surviving it: `state`
    // is rebuilt and the poll loop restarts. The timer itself must survive, so it lives on window.
    var SELF = '/bridge.js';
    var REV_FILE = '/bridge.rev';
    var RELOAD_MS = 3000;

    function reloadSource() {
        var xr = new XMLHttpRequest();
        try {
            xr.open('GET', 'http://' + (HOST || CANDIDATES[0]) + ':' + PORT + SELF + '?t=' + Date.now(), true);
            xr.timeout = 5000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4) return;
                if (xr.status !== 200 || !xr.responseText) return;
                try {
                    // Compiled before it runs: a syntax error in a live edit must not leave the
                    // page with no bridge at all.
                    (new Function(xr.responseText))();
                } catch (e) {
                    console.error('[coop] reload rejected: ' + e);
                }
            };
            xr.send();
        } catch (e) { /* not fatal */ }
    }
    if (!window.__coopReload) {
        window.__coopReload = { last: '', misses: 0, stopped: false };
        // Backoff, for the same reason as lobby.js: this ran an unconditional setInterval every 3 s for
        // the life of every page. With no command server that is a failed connect every 3 s forever,
        // during ordinary single-player play. Slow down on misses and stop after a few.
        var RELOAD_GIVE_UP = 4;
        var reloadTick = function () {
            if (window.__coopReload.stopped) return;
            var xr = new XMLHttpRequest();
            var settled = false;
            var miss = function () {
                if (settled) return;
                settled = true;
                window.__coopReload.misses++;
                if (window.__coopReload.misses >= RELOAD_GIVE_UP) {
                    window.__coopReload.stopped = true;
                    console.log('[coop] reload poll stopped (no server); normal play unaffected');
                }
            };
            try {
                xr.open('GET', 'http://' + (window.__coopHost || '127.0.0.1') + ':' + (window.__coopPort || 45999) + REV_FILE + '?t=' + Date.now(), true);
                xr.timeout = 4000;
                xr.onreadystatechange = function () {
                    if (xr.readyState !== 4) return;
                    if (xr.status !== 200) { miss(); return; }
                    settled = true;
                    window.__coopReload.misses = 0;
                    var rev = String(xr.responseText || '').trim();
                    if (rev && rev !== window.__coopReload.last) {
                        window.__coopReload.last = rev;
                        reloadSource();
                    }
                };
                xr.onerror = miss;
                xr.ontimeout = miss;
                xr.send();
            } catch (e) { miss(); }
            var delay = window.__coopReload.misses === 0
                ? RELOAD_MS
                : RELOAD_MS * Math.pow(2, Math.min(window.__coopReload.misses, 3));
            setTimeout(reloadTick, delay);
        };
        setTimeout(reloadTick, RELOAD_MS);
    }

    var HOST = null;                 // resolved on the first successful poll
    var PORT = 45999;

    var POLL_IDLE = 250;      // ms between polls when nothing is happening
    var POLL_BUSY = 120;      // ms between polls right after a command

    // How many full probe sweeps to attempt before assuming there is no host on this machine. With
    // CANDIDATES being a handful of loopback addresses, 6 sweeps is roughly a minute of trying - long
    // enough that a server started during launch is still found, short enough that ordinary play is not
    // paying for an absent one indefinitely.
    var PROBE_GIVE_UP = 6;

    // The inbox poll is a mailbox, not a stream: 6 s is fast enough that a refused signing is explained
    // while the player is still looking at the screen, and slow enough to be invisible. It stops after the
    // same number of consecutive misses as a probe sweep, and `__coopRearm` starts it again - see
    // NORMAL-PLAY.md for why an unconditional repeating timer is not acceptable here.
    var INBOX_IDLE = 6000;
    var INBOX_GIVE_UP = 6;

    // The clock poll is faster than the inbox because it is the one loop that must feel immediate: a pause
    // the other player will notice as lag. 400 ms is well inside human perception for "we both stopped" and
    // costs one tiny JSON GET. It reads the *agent*, not the supervisor, so it is loopback only.
    var CLOCK_POLL = 400;
    var CLOCK_GIVE_UP = 6;

    // Which machine runs the command server. Both clients use the same pak, so it cannot be a single
    // hardcoded address: on the host, loopback is right, and on the other machine loopback has
    // nothing listening. The bridge probes in order and keeps the first that answers, so one pak
    // works on either side and a moved machine only costs a probe until it finds the host.
    var CANDIDATES = [
        '127.0.0.1',                 // same machine as the server (the host)
        '192.168.1.101',             // the host's LAN address, from the other machine
    ];

    // Which document this is. Frontend.html is the menu UI; RaceWeekend.html and friends are the
    // in-game UIs. The page name tells the lobby which screen a client is on without reading game
    // memory, and a change of page name is how the lobby confirms a destructive command landed.
    var PAGE = (function () {
        try {
            var p = String(window.location.pathname || '');
            var m = p.match(/([A-Za-z]+)\.html/);
            if (m) return m[1];
            return p || 'unknown';
        } catch (e) { return 'unknown'; }
    })();

    var state = {
        stage: 'loading',
        sent: 0,
        failures: 0,
        backoff: 1,
        polling: false,
        seenEngine: false,
        lastHadWork: false
    };

    function url(path, query) {
        return 'http://' + (HOST || CANDIDATES[0]) + ':' + PORT + path + (query ? '?' + query : '');
    }

    function urlFor(host, path, query) {
        return 'http://' + host + ':' + PORT + path + (query ? '?' + query : '');
    }

    function enc(s) {
        return encodeURIComponent(String(s));
    }

    function engineReady() {
        return typeof engine !== 'undefined' && engine && typeof engine.trigger === 'function';
    }

    // Pressing the real control, where one exists. `press.js` is loaded before this file by the pak's
    // HTML; if it is missing (old pak, or a page that does not include it) every caller falls back to
    // the direct trigger, so the bridge never depends on it being present.
    //
    // Why prefer a press: it is the same path a player takes, so the view decides what to emit. The
    // Advance button in particular is a state machine whose event depends on DataStore state, and
    // LoadGame cannot legally be fired before a slot is selected - see press.js.
    function presser() {
        return (typeof window !== 'undefined' && window.__coopPress) ? window.__coopPress : null;
    }

    function send(path, query) {
        var xr = new XMLHttpRequest();
        try {
            xr.open('GET', url(path, query), true);
            xr.timeout = 5000;
            xr.send();
        } catch (e) { /* the UI must not care whether the operator is running */ }
    }

    // Poll: identify this document, and take the next command if there is one.
    function poll() {
        var q = 'client=' + enc(HOST || '?') +
            '&page=' + enc(PAGE) +
            '&stage=' + enc(state.stage) +
            '&sent=' + state.sent +
            '&pkg=coop-bridge' +
            '&v=6';

        // No host resolved yet: probe the candidates in order and keep the first that answers.
        if (!HOST) {
            HOST = window.__coopHost || null;
        }
        if (!HOST) {
            probeHosts(0);
            return;
        }

        var xr = new XMLHttpRequest();
        var done = false;

        function schedule() {
            if (done) return;
            done = true;
            setTimeout(poll, state.lastHadWork ? POLL_BUSY : POLL_IDLE);
        }

        try {
            xr.open('GET', url('/hello', q), true);
            xr.timeout = 5000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4) return;
                state.lastHadWork = false;
                if (xr.status === 200) {
                    state.failures = 0;
                    state.backoff = 1;
                    if (xr.responseText) {
                        state.lastHadWork = true;
                        // Run outside the XHR callback: dispatching inside it would let a command
                        // that tears the document down re-enter the destroyed view's handlers.
                        var next = xr.responseText;
                        setTimeout(function () { dispatch(next); }, 0);
                    }
                    schedule();
                } else {
                    // Not fatal and not unusual: the view is between documents.
                    state.failures++;
                    state.backoff = Math.min(state.backoff + 1, 20);
                    setTimeout(poll, 400 * state.backoff);
                }
            };
            xr.send();
        } catch (e) {
            state.failures++;
            setTimeout(poll, 500);
        }
    }

    function probeHosts(i) {
        if (i >= CANDIDATES.length) {
            HOST = null;
                // Give up after a few full sweeps instead of retrying every 2 s forever.
                //
                // This mod is installed on both machines and has to be invisible when nobody is hosting.
                // Measured before this change: with no command server, a failed probe sweep every 2 s on
                // every UI document, for as long as the game runs. A player who never opens the co-op menu
                // was paying for it constantly.
                //
                // Nothing is lost: opening the co-op menu (or any navigation that re-runs boot) calls
                // probeHosts again, so starting the server later still works.
                state.offlineSweeps = (state.offlineSweeps || 0) + 1;
                if (state.offlineSweeps >= PROBE_GIVE_UP) {
                    state.probing = false;
                    console.log('[coop] host probe stopped after ' + state.offlineSweeps +
                        ' sweeps (no server); normal play unaffected');
                    return;
                }
                setTimeout(poll, 2000);
                return;
            }
        var candidate = CANDIDATES[i];
        var xr = new XMLHttpRequest();
        var settled = false;
        function next() {
            if (settled) return;
            settled = true;
            probeHosts(i + 1);
        }
        try {
            xr.open('GET', urlFor(candidate, '/hello',
                'client=' + enc('probe') + '&page=' + enc(PAGE) + '&stage=probing' +
                '&pkg=coop-bridge&v=6'), true);
            xr.timeout = 3000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4) return;
                if (xr.status === 200) {
                    settled = true;
                    HOST = candidate;
                    window.__coopHost = candidate;
                    state.stage = 'engine-ready';
                    report('probe', true, 'attached to ' + candidate);
                    setTimeout(poll, 0);
                } else {
                    next();
                }
            };
            xr.send();
        } catch (e) {
            next();
        }
    }

    // Which commands are legal on which document.
    //
    // This is not a guess: firing LoadGame from the in-game views (RaceWeekend.html, TeamManagement.html)
    // tore the front-end down and never rebuilt it, leaving a black screen with the process still
    // alive. The game's own load path only ever runs from Frontend.html - LoadSaveScreen lives in the
    // front-end app - so that is the only document allowed to issue it.
    //
    // ContinueGame is in the same category: MainMenu.js is a front-end component.
    var LOAD_FROM = ['Frontend'];
    var MENU_FROM = ['Frontend'];

    function allowedFor(cmd, page) {
        if (cmd === 'load' || cmd === 'loadgame') return LOAD_FROM.indexOf(page) >= 0;
        if (cmd === 'continue' || cmd === 'continuegame') return MENU_FROM.indexOf(page) >= 0;
        return true;
    }

    // The same rule again, applied to the *event name* instead of the command word.
    //
    // `allowedFor` above matches on the command a person typed, and `Raw` reaches `engine.trigger` with an
    // arbitrary name that it resolves afterwards - so `Raw LoadGame autosave` from RaceWeekend.html passed
    // the command check (cmd was 'raw', which is allowed everywhere) and fired the one event documented to
    // black-screen the process. A guard on the spelling of a command is not a guard on what gets dispatched.
    //
    // Derived from LOAD_FROM/MENU_FROM rather than restated, so the two cannot drift apart. Both halves are
    // needed: the command check refuses early with the clearer message that names the page, and this one
    // catches every other route including Raw, so a command added later cannot reopen the hole by
    // forgetting to register it.
    var FRONTEND_ONLY_EVENTS = { LoadGame: LOAD_FROM, ContinueGame: MENU_FROM };

    function eventAllowedOn(eventName, page) {
        var pages = FRONTEND_ONLY_EVENTS[eventName];
        if (!pages) return true;
        return pages.indexOf(page) >= 0;
    }

    // Command grammar is tiny and whitespace-split, so a shell, a .bat file or the Rust lobby can
    // all produce it. Event names pass through unchanged: this is a thin remote for
    // engine.trigger, deliberately not a reimplementation of the game's commands.
    //
    //   Continue               ContinueGame               main-menu Continue      (Frontend only)
    //   Load [slot]            LoadGame <slot>            autosave, save1..29     (Frontend only)
    //   Advance                AdvanceToNextEvent         the in-career Advance button
    //   NextMonth              NextMonthSelected          the calendar month button
    //   Raw <event> [arg...]   any Gameface event by name
    //   Stage <name>           report-only
    function dispatch(text) {
        var line = String(text).replace(/[\r\n]+/g, ' ').trim();
        if (!line) return;
        var parts = line.split(/\s+/);
        var cmd = parts[0].toLowerCase();

        if (!engineReady()) {
            report(line, false, 'engine.trigger unavailable on ' + PAGE);
            return;
        }

        if (cmd === 'stage') {
            state.stage = parts[1] || state.stage;
            report(line, true, 'stage ' + state.stage);
            return;
        }

        // `Team <id>` and `Day <n>` tell the observer what no event carries. They are report-only: they
        // change what this mod believes about the session, never what the game does.
        if (cmd === 'team') {
            var t = parseInt(parts[1], 10);
            if (!isFinite(t) || t < 0 || t > 255) {
                report(line, false, 'team must be 0..255');
                return;
            }
            consent.team = t;
            report(line, true, 'team ' + t);
            return;
        }
        if (cmd === 'day') {
            var d = parseInt(parts[1], 10);
            if (!isFinite(d) || d < 1) {
                report(line, false, 'day must be 1 or later');
                return;
            }
            consent.day = d;
            report(line, true, 'day ' + d);
            return;
        }
        // Reports the observer's own view: how many events it has routed, and the last frame it built.
        // Without this a missing coordination frame is indistinguishable from a missing event.
        if (cmd === 'coopstate') {
            var ob = engine && engine.__coopObserver;
            report(line, true, 'PAGE ' + PAGE +
                ' team=' + consent.team + ' day=' + consent.day +
                ' seen=' + (ob ? ob.count() : 'no-observer') +
                ' last=' + (ob ? ob.last() : ''));
            return;
        }

        // Reads the live DOM and reports it. This is how the *actual* pre-session screen is
        // identified instead of guessed: the game's session start is not a Gameface event, so the
        // next step has to be chosen from what is really on screen.
        if (cmd === 'dom') {
            var out = [];
            try {
                var all = document.querySelectorAll('*');
                for (var i = 0; i < all.length && out.length < 60; i++) {
                    var el = all[i];
                    var cls = el.className;
                    if (typeof cls !== 'string' || !cls) continue;
                    if (/root$|_root|Panel|Menu|Header|Button|Layer/.test(cls)) {
                        var txt = (el.textContent || '').replace(/\s+/g, ' ').trim();
                        out.push(cls + ' | ' + txt.substring(0, 70));
                    }
                }
            } catch (e) {
                out.push('ERR ' + e);
            }
            report(line, true, 'PAGE ' + PAGE + ' :: ' + out.join(' ;; '));
            return;
        }

        if (!allowedFor(cmd, PAGE)) {
            // Refuse rather than try: this exact call is what black-screened a live client.
            report(line, false, 'refused on ' + PAGE + ' (needs ' +
                (cmd === 'advance' ? 'any page' : 'Frontend') + ')');
            return;
        }

        var eventName = null;
        var eventArgs = [];

        if (cmd === 'continue' || cmd === 'continuegame') {
            eventName = 'ContinueGame';
        } else if (cmd === 'load' || cmd === 'loadgame') {
            eventName = 'LoadGame';
            eventArgs = [parts[1] || 'autoSave'];
        } else if (cmd === 'advance') {
            // The Advance button is a state machine, not one event: ContinueButtonContainer decides
            // per press (needs-a-response -> inbox, sponsor targets -> its own screen, race weekend
            // -> AdvanceToNextEvent). On the race-weekend screens the press that actually starts the
            // session is AdvanceToNextEvent, sometimes preceded by other UI steps, so a single
            // Advance from the bridge can be swallowed with no error. `advancen` sends it N times,
            // spaced, which is what "advance until the sim steps" needs in practice.
            var times = 1;
            if (parts[1]) {
                var parsed = parseInt(parts[1], 10);
                if (parsed > 0 && parsed <= 20) times = parsed;
            }
            for (var i = 0; i < times; i++) {
                (function (n) {
                    setTimeout(function () {
                        try { engine.trigger('AdvanceToNextEvent'); } catch (e) { /* view gone */ }
                    }, n * 1200);
                })(i);
            }
            eventName = 'AdvanceToNextEvent';
            state.sent += times - 1;
        } else if (cmd === 'nextmonth') {
            eventName = 'NextMonthSelected';
        } else if (cmd === 'raw') {
            if (!parts[1]) {
                report(line, false, 'Raw needs an event name');
                return;
            }
            eventName = parts[1];
            eventArgs = parts.slice(2);
        } else if (cmd === 'menu' || cmd === 'mainmenu') {
            eventName = 'QuitGame';
        } else {
            report(line, false, 'unknown command');
            return;
        }

        // Report BEFORE firing. ContinueGame and LoadGame replace this document, and Cohtml drops
        // in-flight requests during that teardown; a report sent afterwards never arrives.
        //
        // The event-name guard has to run here, before anything is dispatched, because this is the first
        // point where a command has resolved to the event it will actually send. `Raw` chooses that name
        // below the command check, so checking only the command leaves it unguarded.
        if (!eventAllowedOn(eventName, PAGE)) {
            report(line, false, 'refused on ' + PAGE + ': ' + eventName +
                ' is only legal from ' + FRONTEND_ONLY_EVENTS[eventName].join('/') +
                ' (firing it here tears down this document)');
            return;
        }

        state.sent++;

        // Prefer the real control. A press proves the action was available on this screen: if the
        // button is not there the press finds nothing and the report says so, instead of claiming an
        // event nobody could have emitted. Falls through to the direct trigger when press.js is
        // unavailable or has no mapping for the command.
        var P = presser();
        var viaPress = null;
        if (P) {
            if (cmd === 'continue' || cmd === 'continuegame') viaPress = 'continue';
            else if (cmd === 'load' || cmd === 'loadgame') viaPress = 'load';
            else if (cmd === 'advance') viaPress = 'advance';
            else if (cmd === 'nextmonth') viaPress = 'nextmonth';
        }

        // Fill the few values generated data leaves null before pressing. In the shipped game the real
        // career database supplies them, so this is a no-op there; it matters on the race-weekend pages,
        // whose ERS panels throw on a null `maxEnergyStored` and abort the press before it can emit
        // anything (RaceWeekendUtil.js:187-199). Guarded: a missing DataStore must not stop the press.
        if (P && viaPress && typeof require === 'function') {
            try {
                require(['common/core/DataStore'], function (DS) {
                    try { P.seedDefaults(DS); } catch (e) { }
                });
            } catch (e) { /* seeding is optional */ }
        }

        if (P && viaPress === 'load') {
            report(line, true, PAGE + ' -> press load ' + eventArgs[0]);
            P.pressLoad(eventArgs[0], function (el, err) {
                if (!el) {
                    // The press proved the sequence is not available here. Do not fall back to a blind
                    // trigger: that is the behaviour this replaced.
                    send('/report', 'page=' + enc(PAGE) +
                        '&text=' + enc('ERR ' + line + ' :: no load path (' + err + ')'));
                }
            });
            return;
        }

        if (P && viaPress) {
            report(line, true, PAGE + ' -> press ' + viaPress);
            P.press(viaPress, function (el, err) {
                if (el) return;
                send('/report', 'page=' + enc(PAGE) +
                    '&text=' + enc('ERR ' + line + ' :: no ' + viaPress + ' control (' + err + ')'));
            });
            return;
        }

        report(line, true, PAGE + ' -> ' + eventName +
            (eventArgs.length ? ' ' + eventArgs.join(' ') : ''));

        try {
            engine.trigger.apply(engine, [eventName].concat(eventArgs));
        } catch (e) {
            // On the success path this document is already going away, so there is no way to
            // report a throw. The operator sees the absence of a page change instead.
            return;
        }
    }

    function report(line, ok, detail) {
        var payload = (ok ? 'OK ' : 'ERR ') + line + ' :: ' + detail;
        send('/report', 'page=' + enc(PAGE) + '&text=' + enc(payload));
    }

    // What the observer cannot read from an event argument: the player's team and the calendar day.
    //
    // Neither is carried by any event, so the front-end is told. `Stage` was already report-only; this is
    // the same idea with the two values the coordination rules need. Kept as plain state so the observe
    // callback stays synchronous - a DataStore read during a UI event risks re-entering a view that is
    // mid-update.
    var consent = { team: null, day: null };

    // Launch-from-invite.
    //
    // When a friend accepts an invite while the game is closed, Steam launches it with
    // `+connect_lobby <id>` (documented). Nothing in the game reads that argument, so without this the launch
    // would succeed and then sit on the main menu, which looks like the invite was ignored.
    //
    // Asked once per document, and only acts when an id is present. The agent parses its own command line, so
    // the front-end does not have to guess at argument shapes.
    function connectFromInvite() {
        if (window.__coopInviteChecked) return;
        window.__coopInviteChecked = true;
        try {
            var xr = new XMLHttpRequest();
            xr.open('GET', url('/coop/connectlobby'), true);
            xr.timeout = 5000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4 || xr.status !== 200) return;
                var body;
                try { body = JSON.parse(xr.responseText || '{}'); } catch (e) { return; }
                if (!body || !body.lobby) return;
                report('invite', true, 'launched from an invite for lobby ' + body.lobby);
                try {
                    var j = new XMLHttpRequest();
                    j.open('GET', url('/coop/lobby/join', 'lobby=' + enc(body.lobby)), true);
                    j.timeout = 5000;
                    j.send();
                } catch (e) { /* reported by the lobby status line */ }
            };
            xr.send();
        } catch (e) { /* no agent: nothing to join through */ }
    }

    // The combined clock state: this player's own reasons unioned with the other player's, computed by the
    // agent so the rule lives in one place (`clocksync.rs`, which delegates to the tested `ClockGate`).
    //
    // `seq` is the whole protocol. Acting on `stopped` every poll would re-pause a paused world forever and
    // the pause would never end; acting on a *new* `seq` acts exactly once per transition. This is the same
    // reasoning as `ClockAction::NoChange`, one layer up.
    var clock = { seq: 0, side: null, misses: 0, running: false };

    function pollClock() {
        // Same give-up discipline as the inbox. A 400 ms loop is the most expensive one in the mod, so it
        // must not survive the agent going away - see NORMAL-PLAY.md.
        if (clock.misses >= CLOCK_GIVE_UP) {
            clock.running = false;
            return;
        }
        clock.running = true;
        if (!HOST || !engineReady()) {
            clock.misses++;
            setTimeout(pollClock, CLOCK_POLL * 4);
            return;
        }
        var xr = new XMLHttpRequest();
        var done = false;
        function again(delay) {
            if (done) return;
            done = true;
            setTimeout(pollClock, delay);
        }
        try {
            xr.open('GET', url('/coop/clock'), true);
            xr.timeout = 5000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4) return;
                if (xr.status !== 200) {
                    clock.misses++;
                    again(CLOCK_POLL * 3);
                    return;
                }
                clock.misses = 0;
                var body;
                try { body = JSON.parse(xr.responseText || '{}'); } catch (e) { again(CLOCK_POLL); return; }
                clock.side = body.side || clock.side;
                // First sight of the state is not a change to act on. The agent starts at seq 0 precisely so
                // that "nothing has happened yet" cannot be mistaken for "the game should pause".
                if (typeof body.seq === 'number' && body.seq !== clock.seq) {
                    var previous = clock.seq;
                    clock.seq = body.seq;
                    if (previous !== 0 || body.stopped) {
                        applyClock(body);
                    }
                }
                again(CLOCK_POLL);
            };
            xr.send();
        } catch (e) {
            again(CLOCK_POLL * 3);
        }
    }

    // Applies the world's pause state through the game's own event, which is the only thing that can pause a
    // sim (UI-COMMANDS.md: `SimPauseChange(value, pauseLayer)`).
    //
    // The layer is `RaceViewUI` (9) rather than `UserPause` (2): it is *our* layer, so unpausing restores only
    // what this mod held and cannot cancel a pause the player set themselves. Getting this wrong would mean
    // one machine could force the other out of its own menu.
    //
    // Guarded on being in a race at all: `SimPauseChange` on the front-end or team-management screens has no
    // sim to pause, and firing it there is the kind of stray trigger that produces a black screen (see
    // LOBBY-GATE.md for the `LoadGame` precedent).
    function applyClock(body) {
        if (PAGE !== 'RaceWeekend') {
            report('clock', true, 'seq ' + body.seq + ' stopped=' + body.stopped + ' ignored on ' + PAGE);
            return;
        }
        var value = !!body.stopped;
        try {
            engine.trigger('SimPauseChange', value, 9);
            report('clock', true, 'seq ' + body.seq + ' -> SimPauseChange(' + value + ', RaceViewUI) ' +
                'reasons=[' + (body.reasons || []).join(',') + ']');
        } catch (e) {
            report('clock', false, String(e));
        }
    }

    // Starts the coordination observer.
    //
    // observe.js is loaded before this file by the pak's HTML, but a page that does not include it (an
    // older pak) must still work, so the absence is tolerated rather than crashing the bridge.
    function startObserver() {
        if (window.__coopObserverStarted) return;
        var O = window.CoopObserve;
        if (!O || typeof O.observe !== 'function') return;
        window.__coopObserverStarted = true;
        try {
            var api = O.observe(engine, function (plan) {
                if (plan.kind === 'clock') {
                    // An empty set is meaningful: it means this player is no longer holding the world.
                    send('/coordinate/clock', 'reasons=' + enc(plan.reasons.join(',')));
                    return;
                }
                if (plan.query) send('/coordinate/' + plan.kind, plan.query);
            }, {
                context: function () { return { team: consent.team, day: consent.day }; },
                onError: function (e) { report('observe', false, String(e && e.message || e)); }
            });
            if (api) report('observe', true, 'watching engine.trigger');
        } catch (e) {
            report('observe', false, String(e));
        }
    }

    // The supervisor's answer to a refused claim or a resolved vote, shown so a losing player is told
    // why instead of watching a signing do nothing. Polled slowly: it is a mailbox, not a stream.
    //
    // This has its own give-up counter for the same reason the main poll has one. An unconditional
    // repeating timer here is precisely the regression NORMAL-PLAY.md measured: with no agent present the
    // game paid ~100 loopback attempts per 10 idle minutes for a feature nobody was using. It stops after
    // INBOX_GIVE_UP consecutive misses and `__coopRearm` starts it again.
    var inbox = { failures: 0, running: false };

    function pollInbox() {
        if (inbox.failures >= INBOX_GIVE_UP) {
            inbox.running = false;
            return;
        }
        inbox.running = true;
        // Nothing to ask. Counted as a miss rather than skipped: without a resolved host this path would
        // otherwise reschedule every INBOX_IDLE forever, which is the same unconditional repeating timer in
        // a different disguise - and the reason NORMAL-PLAY.md exists.
        if (!HOST || !engineReady()) {
            inbox.failures++;
            setTimeout(pollInbox, INBOX_IDLE * Math.min(inbox.failures + 1, 5));
            return;
        }
        var xr = new XMLHttpRequest();
        var done = false;
        function again(delay) {
            if (done) return;
            done = true;
            setTimeout(pollInbox, delay);
        }
        try {
            xr.open('GET', url('/coop/inbox'), true);
            xr.timeout = 5000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4) return;
                if (xr.status !== 200) {
                    inbox.failures++;
                    again(INBOX_IDLE * Math.min(inbox.failures + 1, 5));
                    return;
                }
                inbox.failures = 0;
                var text = xr.responseText || '';
                if (text !== window.__coopInboxLast) {
                    window.__coopInboxLast = text;
                    try {
                        var body = JSON.parse(text);
                        if (body.lines && body.lines.length) {
                            report('inbox', true, body.lines[body.lines.length - 1]);
                        }
                    } catch (e) { /* a malformed body must not stop the poll */ }
                }
                again(INBOX_IDLE);
            };
            xr.send();
        } catch (e) {
            inbox.failures++;
            again(INBOX_IDLE);
        }
    }

    // Screen tracking by DOM. Each UI page renders its own root; the front-end's main menu is the
    // only screen with MainMenu_root.
    function watchScreens() {
        try {
            if (document.querySelector('.MainMenu_root')) {
                if (state.stage !== 'mainmenu') state.stage = 'mainmenu';
            } else if (PAGE !== 'Frontend') {
                if (state.stage !== 'ingame') state.stage = 'ingame';
            } else if (state.stage === 'loading' || state.stage === 'engine-ready') {
                state.stage = 'frontend';
            }
        } catch (e) { /* DOM not ready */ }
        setTimeout(watchScreens, 500);
    }

    function start() {
        if (!engineReady()) {
            setTimeout(start, 250);
            return;
        }
        // Publish the resolved host/port on window so the reload timer keeps talking to the right
        // machine after a re-evaluation.
        window.__coopPort = PORT;

        if (!state.seenEngine) {
            state.seenEngine = true;
            state.stage = 'engine-ready';
            send('/pak-ok', 'file=' + enc(PAGE) + '.html&bridge=coop');
        }
        watchScreens();
        startObserver();

        // Re-arm hook.
        //
        // The probe gives up after PROBE_GIVE_UP sweeps so an idle game is not paying for an absent
        // server. That must not make the feature unreachable: anything that means "the player now wants
        // co-op" calls this, which resets the counters and starts polling again. The lobby's own screen
        // calls it on mount, so opening the multiplayer menu always works even after a long single-player
        // session.
        window.__coopRearm = function () {
            state.offlineSweeps = 0;
            state.failures = 0;
            state.backoff = 1;
            state.probing = false;
            HOST = window.__coopHost || HOST || null;
            try { poll(); } catch (e) { /* poll is defensive already */ }
            // A re-arm means the player now wants co-op, so the inbox poll gets its budget back too. It is
            // otherwise left stopped after INBOX_GIVE_UP misses, which is what keeps an offline game quiet.
            inbox.failures = 0;
            if (!inbox.running) {
                try { pollInbox(); } catch (e) { /* same policy as poll */ }
            }
            clock.misses = 0;
            if (!clock.running) {
                try { pollClock(); } catch (e) { /* same policy as poll */ }
            }
            return true;
        };

        // One poll loop per document. A reload re-evaluates this whole IIFE, so starting a second
        // loop would race the first over the same timer and could leave neither running - which is
        // exactly what happened: the host went silent for eight minutes after a live edit.
        if (!window.__coopPolling) {
            window.__coopPolling = true;
            setTimeout(poll, 0);
            pollInbox();
            pollClock();
            connectFromInvite();
        }
    }

    start();
})();
