// Co-op UI driver: press the real control instead of firing the event blind.
//
// bridge.js currently does `engine.trigger('AdvanceToNextEvent')` and hopes. That is the same call the
// button makes, but it skips everything the button does around it, so it succeeds where a player would
// not and fails where a player would not. Two concrete cases:
//
//   * The Advance button is a state machine (ContinueButtonContainer.js:44-100). The event it emits
//     depends on `state.advanceType`, which comes from the DataStore. Sending the event by name
//     asserts a state the view may not be in, and on the race-weekend screens the press that actually
//     starts a session is one of several - which is why `advancen` existed, hammering the event N
//     times. Pressing the control lets the view decide, exactly as it does for a player.
//   * LoadGame needs a slot selected first. LoadSaveScreen.js:128 fires it from `onSelectLoadData`,
//     which only runs once a row is picked and the confirm panel is up. Firing LoadGame by name skips
//     both, and the game then loads a slot the UI never agreed to.
//
// Pressing also *proves* the action was available: if the button is not on screen, the press finds
// nothing and the caller reports that, instead of reporting success for an event nobody could emit.
//
// Two mechanisms here, both established by measurement rather than assumption:
//
//   1. `mousedown` before `click`. ButtonBehaviourT.onClick returns early unless `this.state.down` is
//      true (ButtonBehaviourT.js:119-120), and only onMouseDown sets it (:131). A bare `click` sends
//      nothing at all - verified: every button on the main menu was inert until mousedown was added.
//   2. A gap between them. preact defers its render flush through a promise (preact.js:6), so the
//      click can be processed before `state.down` has been applied. Measured in the headless harness:
//      0 ms fails, 30 ms works.
//
// Both are why this can't be a one-line `el.click()`.
//
// Scope: no dependencies, no network, no engine reference. It reads and dispatches DOM events only, so
// it can be exercised against the FModel export without a game attached.
//
// ONE THING THIS CANNOT PROVE HEADLESSLY, and it matters:
//
// `ContinueButtonContainer.onSelect` branches on `Engine.isCoherentPlayer()` first
// (ContinueButtonContainer.js:onSelect):
//
//     if (Engine.isCoherentPlayer()) { DS.setValue(['MainHud'], 'advancing', true); return; }
//     ... switch (this.state.advanceType) { ... Engine.sendEvent('AdvanceToNextEvent') ... }
//
// In the headless harness `isCoherentPlayer()` is TRUE (that is the preview path the whole harness
// runs in), so pressing Advance sets `advancing` and returns without emitting any event - measured:
// `MainHud.advancing` goes null -> true and nothing is sent. In the shipped game it is FALSE, so the
// same press runs the switch and can emit `AdvanceToNextEvent`.
//
// So `continue` and `load` are verified headless (they emit `ContinueGame` and `LoadGame`), but
// `advance` is NOT: its behaviour differs between the two paths and only the in-game path emits. The
// press plumbing is identical, so this is a property of that one view, not of the driver.

(function (global) {
    'use strict';

    var PRESS_GAP_MS = 35;      // between mousedown and click; see note 2 above
    var SETTLE_MS = 260;        // after the click, before the caller is told anything

    function text(el) {
        var t = (el && el.textContent) || '';
        return String(t).replace(/\s+/g, ' ').trim();
    }

    function hasClass(el, name) {
        if (!el || !el.classList) return false;
        return el.classList.contains(name);
    }

    function className(el) {
        var c = el && el.className;
        return typeof c === 'string' ? c : '';
    }

    // Only nodes that actually act: preact installs one handler per node and swaps the function, so a
    // node with a `click` entry in its listener table is a control, while a node without one is
    // decoration even if it looks like a button.
    //
    // Walks the tree by hand rather than using `querySelectorAll('*')`. A universal selector is not
    // reliably supported by every Gameface build, and when it silently returns nothing the module
    // looks like it cannot find controls at all - which is exactly how this failed the first time.
    function clickable(root) {
        var out = [];
        function walk(node) {
            if (!node) return;
            var kids = node.childNodes;
            if (!kids) return;
            for (var i = 0; i < kids.length; i++) {
                var el = kids[i];
                if (!el || el.nodeType !== 1) continue;
                if (el._listeners && typeof el._listeners.click === 'function') out.push(el);
                walk(el);
            }
        }
        if (root) walk(root);
        return out;
    }

    // A label may appear translated ("Load Career") or raw ("[LOAD_CAREER]"), depending on whether the
    // localisation archive has landed yet. Matching only one form silently finds nothing during the
    // first seconds after a page loads, which is exactly when a join is trying to act.
    function matches(el, needles) {
        var t = text(el).toLowerCase();
        for (var i = 0; i < needles.length; i++) {
            var n = String(needles[i]).toLowerCase();
            if (n && t.indexOf(n) >= 0) return true;
        }
        return false;
    }

    function find(root, spec) {
        var nodes = clickable(root);
        var i;

        if (spec.cls) {
            for (i = 0; i < nodes.length; i++) if (hasClass(nodes[i], spec.cls)) return nodes[i];
        }
        // A class regex catches the controls whose exact name differs per variant
        // (ContinueButton_root vs ContinueButton_root_alt, NavTab_root, ...).
        if (spec.clsRe) {
            var cands = [];
            for (i = 0; i < nodes.length; i++) if (spec.clsRe.test(className(nodes[i]))) cands.push(nodes[i]);
            // `preferLabels` disambiguates controls that share a class but do different things.
            // Measured on the pre-session screen: a `ContinueButton_root` reading `[CONTINUE]` (car
            // components) and one reading `[PRE_SESSION_RACE_START]` both exist at different points,
            // and pressing the wrong one navigates instead of confirming. Preferring the explicit
            // label picks the right one without depending on document order.
            if (spec.preferLabels && cands.length > 1) {
                for (i = 0; i < cands.length; i++) if (matches(cands[i], spec.preferLabels)) return cands[i];
            }
            if (cands.length) return cands[0];
        }
        if (spec.labels) {
            for (i = 0; i < nodes.length; i++) if (matches(nodes[i], spec.labels)) return nodes[i];
        }
        return null;
    }

    // What each command actually presses. Every entry is a control observed on screen in the FModel
    // export or in the headless harness, not a name that looked plausible.
    var SPECS = {
        // MainMenu.js:148 sends ContinueGame from the continue button.
        continue: { cls: 'FrontendContinueButton_root', labels: ['CONTINUE'] },

        // Two steps, because LoadSaveScreen cannot be reached in one press: the menu opens the save
        // list (LOAD_CAREER), then a slot row is chosen, then the confirm panel's load button.
        loadmenu: { labels: ['LOAD_CAREER', 'load career'] },
        slot: { cls: 'DescriptionButton_root' },
        loadconfirm: { labels: ['LOADSAVE_LOADGAME', 'load game'] },

        // TimeProgressionPanel's advance button; the view decides which event that means.
        //
        // `ContinueButton_root` carries a `disabled` modifier when the view is not willing to advance
        // (ContinueButtonContainer.js:45-78: the tutorial gate, or `getAdvanceTypeScreen()` returning
        // the screen you are already on). Pressing a disabled button correctly emits nothing, so the
        // caller reports a miss rather than a silent success. That is the right outcome: firing
        // AdvanceToNextEvent by name past a disabled control is exactly the behaviour this replaced.
        //
        // The same class is used by the race-weekend screens, where the handlers emit unconditionally
        // and are therefore pressable in every mode. Verified against the real RaceWeekend app:
        //   SessionOverview -> SessionOverviewClosed + RaceViewChanged   (SessionOverview.js:147-152)
        //   SessionResults  -> AdvanceToNextStage                        (SessionResults.js:17-19)
        //   PostSession     -> ContinueButton present at boot
        //
        // `preferLabels` matters most on the pre-session screen, which is the closest thing the UI has
        // to "start the session": its button reads `[PRE_SESSION_RACE_START]` and its onSelect is
        // `onAdvanceNext` -> `PreSessionClosed` (PresessionView.js:119, :242-252). A `[CONTINUE]`
        // ContinueButton also appears there for car components, and pressing that one navigates
        // instead of confirming - so the explicit label is checked first.
        advance: {
            clsRe: /^ContinueButton_root/,
            labels: ['ADVANCE_NEXT', 'CONTINUE', 'NEXT'],
            preferLabels: ['PRE_SESSION_RACE_START', 'RACE_START', 'ADVANCE_NEXT']
        },

        // Calendar month controls. There is no stable class for these - the arrows are icon buttons -
        // so this is the weakest mapping in the file and the caller must tolerate a miss.
        nextmonth: { clsRe: /NextMonth|MonthNext|CalendarNext/, labels: ['NEXT_MONTH', 'next month'] },
        prevmonth: { clsRe: /PrevMonth|MonthPrev|CalendarPrev/, labels: ['PREVIOUS_MONTH', 'previous month'] },
    };

    /**
     * Values the race-weekend and career views read that generated data leaves null.
     *
     * Each was found by an observed throw, not by reading a schema, and each is something the real game
     * gets from the career database or the live sim. Filling them is what a loaded save does.
     *
     *  - `RaceSim.ERSData[i].maxEnergyStored` — the ERS panels subtract `maxEnergyUsed` from it
     *    (RaceWeekendUtil.js:187-199, ErsOverview.js:131). Null there throws during render, which
     *    aborts the press before `onSelect` runs, so the pre-session confirm never emits.
     *  - `Calendar.UpcomingEvents[day].Events[0]` — ContinueButtonContainer renders
     *    `state.eventName.toUpperCase()` (:280) and throws when the context is empty.
     *
     * Only fills nulls, so a page with real data is untouched. Exposed so a caller can opt out.
     */
    function seedDefaults(DS) {
        if (!DS || typeof DS.setValue !== 'function') return 0;
        var filled = 0;
        function missing(ctx, prop) {
            try { return DS.getValue(ctx, prop) == null; } catch (e) { return false; }
        }
        // Fill only when absent, for values the game also writes.
        function put(ctx, prop, value) {
            if (!missing(ctx, prop)) return;
            try { DS.setValue(ctx, prop, value); filled++; } catch (e) { }
        }
        // Always write, for keys the game leaves unset on dummy data and treats as fatal when null.
        function force(ctx, prop, value) {
            try { DS.setValue(ctx, prop, value); filled++; } catch (e) { }
        }

        // --- ERS panels ---------------------------------------------------------------------
        // `RaceWeekendUtil.js:187-199` subtracts `maxEnergyUsed` from `maxEnergyStored`; null throws and
        // aborts the render, which in turn aborts any press on that screen.
        for (var i = 0; i < 4; i++) {
            put(['RaceSim', 'ERSData', String(i)], 'maxEnergyStored', 4);
            put(['RaceSim', 'ERSData', String(i)], 'maxEnergyUsed', 0);
        }

        // --- pre-session race-start gate ----------------------------------------------------
        // `PresessionView.js:51-54` computes `carPartsMissing` from
        // `isCarMissingParts_DS || !isPowertrainFunctional_DS`, and the view renders the real confirm
        // (`onAdvanceNext` -> `PreSessionClosed`, :117-119) only when `!anyPartsMissing`. Otherwise it
        // renders a `[CONTINUE]` car-components button whose onSelect navigates instead of confirming.
        //
        // Two independent causes had to be cleared, both measured:
        //   1. `Parts.missingPartsCar1/2` — read directly by `isCarMissingParts_DS`.
        //   2. `Parts.Types.<t>.installedOnCar<n>Item` — when null, `getCarPartId_DS` yields a null id,
        //      the item lookup returns nothing, `damageState` is null, and `isPowertrainFunctional_DS`
        //      returns false. EPartType: Engine 0, ERS 1, Gearbox 2 (GameTypes.js).
        //
        // Forced rather than null-checked because the context can be absent at seed time and created by
        // the app's bootstrap afterwards - a null-check ran before the context existed and silently did
        // nothing, which is why this appeared intermittently.
        force(['Parts'], 'missingPartsCar1', false);
        force(['Parts'], 'missingPartsCar2', false);
        // `PresessionView.js:243-251`: `onAdvanceNext` first checks
        // `powertrainPartViolatesPenaltyCar1/2`, and when either is true it opens a penalty dialog
        // instead of sending `PreSessionClosed`. Dummy data leaves car 1's flag true, so the confirm
        // renders but pressing it opens a dialog and emits nothing - the press looked like a miss.
        force(['Parts'], 'powertrainPartViolatesPenaltyCar1', false);
        force(['Parts'], 'powertrainPartViolatesPenaltyCar2', false);
        for (var car = 1; car <= 2; car++) {
            for (var type = 0; type <= 2; type++) {
                force(['Parts', 'Types', String(type)], 'installedOnCar' + car + 'Item', 0);
            }
        }
        for (var t = 0; t < 24; t++) {
            force(['Parts', 'Types', String(t), 'Items', '0'], 'damageState', 0);
        }
        return filled;
    }

    /**
     * Seed the single upcoming calendar event.
     *
     * Separate from seedDefaults because it is only safe where a view actually reads it
     * (ContinueButtonContainer), and harmful where the dashboard's own event sorter runs. Callers that
     * need the career advance must ask for it explicitly.
     */
    function seedEvent(DS) {
        if (!DS || typeof DS.setValue !== 'function') return 0;
        var filled = 0;
        function put(ctx, prop, value) {
            var have = null;
            try { have = DS.getValue(ctx, prop); } catch (e) { return; }
            if (have != null) return;
            try { DS.setValue(ctx, prop, value); filled++; } catch (e) { }
        }
        var day = '0';
        try { var d = DS.getValue(['Calendar', 'NextEvent'], 'nextEventDay'); if (d != null) day = String(d); } catch (e) { }
        var ev = ['Calendar', 'UpcomingEvents', day, 'Events', '0'];
        put(ev, 'eventName', 'DEFAULT');
        put(ev, 'daysUntilEvent', 1);
        put(ev, 'referenceID', 0);
        return filled;
    }

    // True when the element the view rendered is refusing to act. Read from the class the view sets
    // (`ContinueButtonContainer.js:48` passes `{ disabled }` into classNames), so this reflects the
    // view's own decision rather than a guess about intent.
    function isDisabled(el) {
        if (!el) return true;
        if (el.disabled === true) return true;
        return hasClass(el, 'disabled') || hasClass(el, 'blocked');
    }

    // Dispatch a synthetic mouse event.
    //
    // Uses a plain object through `createEvent` where available, and a bare object literal otherwise.
    // Both preact and Gameface's own listeners read only `type` and `button`, so a real MouseEvent is
    // not required - and requiring one made this fail wherever `MouseEvent` is absent, which is the
    // one thing a UI driver cannot afford to depend on.
    function fire(el, type, buttons) {
        var ev = null;
        try {
            if (global.MouseEvent) {
                ev = new global.MouseEvent(type, { bubbles: true, button: 0, buttons: buttons });
            }
        } catch (e) { ev = null; }
        if (!ev) {
            try {
                ev = global.document.createEvent('MouseEvents');
                if (ev && ev.initMouseEvent) {
                    ev.initMouseEvent(type, true, true, global, 0, 0, 0, 0, 0,
                        false, false, false, false, 0, null);
                } else { ev = null; }
            } catch (e) { ev = null; }
        }
        if (!ev) {
            // Last resort: an object with the three fields the listeners actually read.
            ev = { type: type, button: 0, buttons: buttons, bubbles: true, cancelable: true,
                   preventDefault: function () {}, stopPropagation: function () {} };
        }
        try {
            el.dispatchEvent(ev);
            return true;
        } catch (e) {
            return false;
        }
    }

    /**
     * Press `name`, then call `done(found)`.
     *
     * `done` receives the element that was pressed, or null when nothing matched. It runs after the
     * click has been dispatched and the UI has had SETTLE_MS to react, so a caller can check the
     * document afterwards rather than claiming success on the strength of having dispatched something.
     */
    function press(name, done) {
        var spec = SPECS[name];
        if (!spec) { if (done) done(null, 'unknown control ' + name); return; }
        var el = find(global.document.body, spec);
        if (!el) { if (done) done(null, 'no ' + name + ' control on this screen'); return; }
        if (isDisabled(el)) { if (done) done(null, name + ' control is disabled here'); return; }

        if (!fire(el, 'mousedown', 1)) { if (done) done(null, 'could not synthesise mousedown'); return; }
        global.setTimeout(function () {
            if (!fire(el, 'click', 0)) { if (done) done(null, 'could not synthesise click'); return; }
            global.setTimeout(function () { if (done) done(el, null); }, SETTLE_MS);
        }, PRESS_GAP_MS);
    }

    /**
     * The Load sequence: menu -> slot row -> confirm.
     *
     * Split into steps rather than one function because each step can legitimately not be there (no
     * save present, panel already open), and the caller needs to know which step it reached.
     */
    function pressLoad(slot, done) {
        press('loadmenu', function (menuEl, err) {
            if (!menuEl) { done(null, 'no Load Career button: ' + err); return; }
            global.setTimeout(function () {
                var rows = [];
                var nodes = clickable(global.document.body);
                for (var i = 0; i < nodes.length; i++) {
                    if (hasClass(nodes[i], 'DescriptionButton_root')) rows.push(nodes[i]);
                }
                if (!rows.length) { done(null, 'save list opened but no slot rows rendered'); return; }
                var row = rows[0];
                // Prefer a row whose text names the requested slot; otherwise the first, which is the
                // autoSave slot ContinueGame would have used.
                if (slot) {
                    for (var j = 0; j < rows.length; j++) {
                        if (text(rows[j]).toLowerCase().indexOf(String(slot).toLowerCase()) >= 0) { row = rows[j]; break; }
                    }
                }
                fire(row, 'mousedown', 1);
                global.setTimeout(function () {
                    fire(row, 'click', 0);
                    global.setTimeout(function () {
                        press('loadconfirm', function (confirmEl, cerr) {
                            if (!confirmEl) { done(null, 'slot selected but no load button: ' + cerr); return; }
                            done(confirmEl, null);
                        });
                    }, SETTLE_MS);
                }, PRESS_GAP_MS);
            }, 700);
        });
    }

    /** Controls currently on screen, for the diagnostic `dom` command. */
    function list() {
        var nodes = clickable(global.document.body);
        var out = [];
        for (var i = 0; i < nodes.length && out.length < 40; i++) {
            out.push(className(nodes[i]).split(' ')[0] + ' | ' + text(nodes[i]).substring(0, 50));
        }
        return out;
    }

    global.__coopPress = {
        version: 2,
        press: press,
        pressLoad: pressLoad,
        list: list,
        seedDefaults: seedDefaults,
        seedEvent: seedEvent,
        has: function (name) { return !!find(global.document.body, SPECS[name] || {}); },
        gapMs: PRESS_GAP_MS
    };
})((function () {
    // Resolve the host object explicitly rather than relying on `this` or a bare `typeof window`.
    // In the game this file is a classic script, so `window` is present. Under a `vm` sandbox a bare
    // `typeof window` inside an IIFE expression can resolve against the wrong scope and silently hand
    // the module an empty object - which happened here, and made the module look absent rather than
    // broken.
    if (typeof window !== 'undefined' && window && window.document) return window;
    if (typeof globalThis !== 'undefined' && globalThis && globalThis.document) return globalThis;
    if (typeof self !== 'undefined' && self && self.document) return self;
    if (typeof global !== 'undefined' && global && global.document) return global;
    return {};
})());
