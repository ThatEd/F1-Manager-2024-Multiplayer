/*
 * The coordination observer: turns what a player just did in the game into a coordination frame.
 *
 * Why this file exists
 * --------------------
 * `coop-proto` defines `Clock`, `Contract` and `Vote`; `coop-supervisor::arbitrate` enforces them; and
 * the agent now has a sender. All of that was inert because nothing in either game produced a frame.
 * This is the producer, and it sits in the web view because that is the only place that knows what the
 * player pressed.
 *
 * How it observes, and why not by patching AMD modules
 * ----------------------------------------------------
 * `Engine.sendEvent` is the game's scripting API for its own UI: every user action funnels through
 * `engine.trigger(name, ...args)` (see UI-COMMANDS.md). That is one function to wrap.
 *
 * Patching the module export instead would be a race this file cannot win: it is loaded before
 * `require.js` (see the pak's HTML), and `require.js` reads `data-main` during its own execution and then
 * loads the app. So by the time any AMD module exists this script has already run - there is nothing to
 * patch yet. Wrapping the concrete `engine` host object also survives the two call shapes the export
 * contains (`sendEvent(...)` and the TypeScript-compiled `(0, Engine_1.sendEvent)(...)`), because both end
 * at the same host function.
 *
 * Interception on `engine.trigger` is additive, never a replacement: the original is always called
 * first, synchronously, with the identical argument list. If this file has a bug, or the agent is not
 * running, normal play is unaffected - the worst case is that no coordination frame is produced, which
 * is exactly the state the mod shipped in before this file existed.
 *
 * Event shapes, all read from the export rather than guessed
 * ----------------------------------------------------------
 *   SimPauseChange(value, pauseLayer)      js/project/modules/raceWeekend/RaceWeekendEvents.js:66
 *   SetDilemmaResponse(mailId, response)   js/project/modules/inbox/InboxDilemmaButton.js:53
 *   OnSubmitContractOffer(terms, staffID)  .../staff/contractNegotiation/ContractProposalAndResultsPanel.js:187
 *   OnConfirmStaffContract(staffID)        .../ContractProposalAndResultsPanel.js:216
 *   OnCancelStaffContract(staffID)         .../ContractProposalAndResultsPanel.js:117
 *   OnFailedStaffContract(staffID)         .../ContractProposalAndResultsPanel.js:140 *
 * Anything not in that list is observed and ignored. That is deliberate: an unrecognised event tells us
 * nothing about coordination, and inventing a meaning for it is how a mod starts disagreeing with the
 * game.
 */
(function (root) {
    'use strict';

    // ---------------------------------------------------------------------------------------------
    // 1. Pause layers -> pause reasons
    // ---------------------------------------------------------------------------------------------

    /*
     * `PauseLayer` (js/project/data/GameTypes.js:1565-1576) is the *UI's* enum and is numbered
     * differently from `PauseReason` in coop-proto/coordination.rs. Mapping by number would be a silent
     * off-by-one: layer 0 is UserTimeControl while reason 0 is UserPause. So the mapping is explicit and
     * by name, and any layer not listed maps to nothing rather than to a guess.
     *
     * Two layers are deliberately dropped:
     *   QuickSim (1) - a local fast-forward. It stops *this* player's world for a moment by choice; it is
     *   not a pause that the other player must observe, and propagating it would fight with the other
     *   player's own time control.
     *   Replay (5)   - a local camera feature with no effect on the world's clock.
     *
     * Everything else stops the world for the player, so it must stop it for both (the union rule in
     * `ClockGate`). Reusing the protocol's names where they exist keeps the two ends saying the same
     * word; EventPause/RedFlagUI/EditStrategyUI/PitOptionsUI/RaceViewUI/CircuitInfoUI/EndOfSession have
     * no exact `PauseReason` and take the closest one that is defined - `PauseReason::UserPause` - rather
     * than a new wire value.
     */
    var LAYER_TO_REASON = {
        0: 'UserTimeControl', // UserTimeControl
        2: 'UserPause',       // UserPause
        3: 'Notifications',   // Notifications
        4: 'Tutorial',        // Tutorial
        6: 'UserPause',       // RedFlagUI
        7: 'UserPause',       // EditStrategyUI
        8: 'UserPause',       // PitOptionsUI
        9: 'UserPause',       // RaceViewUI
        10: 'UserPause',      // CircuitInfoUI
        11: 'UserPause'       // EndOfSession
    };

    /* `PauseReason` numbering, from coordination.rs `as_u8`. Named here because the wire carries bytes:
       the UI must send the protocol's number, not the layer's. */
    var REASON_ID = {
        UserPause: 0,
        UserTimeControl: 1,
        Notifications: 2,
        Tutorial: 3,
        Resync: 4
    };

    /*
     * The pause state as the *agent* tracks it.
     *
     * A set, not a flag, because two layers can be active at once (a tutorial opens over a settled
     * circuit-info panel) and whichever clears first must not start the world. The other end does the
     * union across players; this end does the union across layers.
     */
    function createClock() {
        var active = {};
        return {
            /** Applies one `SimPauseChange` and returns the reasons now holding this player. */
            apply: function (value, layer) {
                var reason = LAYER_TO_REASON[layer];
                if (!reason) return reasons();
                if (value) {
                    active[layer] = reason;
                } else {
                    delete active[layer];
                }
                return reasons();
            },
            /** Every reason currently holding this player, de-duplicated and sorted. */
            reasons: reasons,
            /** True when nothing is holding this player's world. */
            clear: function () {
                active = {};
                return reasons();
            },
            /** The raw layers, for the status line. */
            layers: function () {
                var out = [];
                for (var k in active) if (Object.prototype.hasOwnProperty.call(active, k)) out.push(Number(k));
                return out.sort(function (a, b) { return a - b; });
            }
        };

        function reasons() {
            var seen = {};
            for (var k in active) {
                if (!Object.prototype.hasOwnProperty.call(active, k)) continue;
                seen[REASON_ID[active[k]]] = active[k];
            }
            var ids = Object.keys(seen).map(Number).sort(function (a, b) { return a - b; });
            return ids.map(function (id) { return seen[id]; });
        }
    }

    /**
     * The wire numbers for a reason-name list, e.g. `['UserPause','Notifications'] -> '0,2'`.
     * Sorted and de-duplicated so the payload is stable, which makes a repeated frame cheap and a log
     * diff meaningful.
     */
    function reasonIds(names) {
        var ids = [];
        for (var i = 0; i < names.length; i++) {
            var id = REASON_ID[names[i]];
            if (id === undefined) continue;
            if (ids.indexOf(id) < 0) ids.push(id);
        }
        return ids.sort(function (a, b) { return a - b; });
    }

    // ---------------------------------------------------------------------------------------------
    // 2. What to report for an observed event
    // ---------------------------------------------------------------------------------------------

    /*
     * Turns one observed `engine.trigger` call into a coordination request, or null.
     *
     * `ctx` carries what cannot be read from the arguments:
     *   ctx.day      the current calendar day, or null when unknown
     * Returns `{ kind, query }` where `query` is the query string for `/coop/coordinate/<kind>`.
     *
     * Kept pure so every rule below is testable without a game, a socket or a `engine` object.
     */
    function classify(name, args, ctx) {
        ctx = ctx || {};
        args = args || [];

        if (name === 'SimPauseChange') {
            // The clock is handled by the caller, which owns the layer set; it needs the whole set, not
            // one layer's change. Signalled with a marker rather than duplicating the set logic here.
            return { kind: 'clock', pause: { value: !!args[0], layer: Number(args[1]) } };
        }

        if (name === 'SetDilemmaResponse') {
            /*
             * A regulation vote is a two-choice dilemma mail (Volta: "these will always feature two
             * choices, and you must vote for one or the other"). The arguments are the mail id and the
             * chosen response index, which is exactly what `Msg::Vote` wants: the mail is the proposal
             * and the index is the option.
             *
             * Not every dilemma is a regulation vote, so the same frame is sent for all of them. Sending
             * a vote for a pit-options dilemma is harmless - the supervisor tallies per proposal id and
             * the ids do not collide with a proposal's - and distinguishing them would need to read mail
             * state the event does not carry.
             */
            var mail = Number(args[0]);
            var option = Number(args[1]);
            if (!isFinite(mail) || mail < 0 || mail > 65535) return null;
            if (!isFinite(option) || option < 0 || option > 254) return null;
            return { kind: 'vote', query: 'proposal=' + mail + '&option=' + option };
        }

        if (name === 'OnSubmitContractOffer') {
            /*
             * `OnSubmitContractOffer(terms, staffID)`: the player has put an offer to this person.
             *
             * This is a *claim*, not a signing. `OnConfirmStaffContract` is the signing, and the day rule
             * is checked again there - the export shows both events, and the negotiated offer can fail,
             * which is why `OnFailedStaffContract` releases the claim.
             *
             * No day is available: `OnSubmitContractOffer` carries only the terms and the staff id. The
             * day rule therefore cannot be enforced at claim time (which is fine - a claim only reserves
             * the person) and is enforced at confirm, where the agent supplies the day it was told.
             */
            return contract('claim', args[1], ctx);
        }

        if (name === 'OnConfirmStaffContract') {
            return contract('confirm', args[0], ctx);
        }

        if (name === 'OnCancelStaffContract' || name === 'OnFailedStaffContract') {
            return contract('release', args[0], ctx);
        }

        return null;
    }

    /*
     * A contract frame needs `day` because the far side compares it against the calendar. When the day is
     * unknown the frame is *not* sent: a signing reported with day 0 would either be refused (day must be
     * >= 1) or, worse if that check were ever relaxed, silently bypass the one rule it exists to enforce.
     * Returning null makes the caller log "no day", which is an honest gap rather than a wrong claim.
     */
    function contract(kind, staffIdArg, ctx) {
        var staff = Number(staffIdArg);
        var team = ctx.team;
        var day = ctx.day;
        if (!isFinite(staff) || staff < 0) return null;
        if (team === undefined || team === null || !isFinite(team)) return null;
        if (day === undefined || day === null || !isFinite(day) || day < 1) return null;
        return {
            kind: 'contract',
            query: 'person=' + staff + '&team=' + team + '&day=' + day + '&kind=' + kind
        };
    }

    /**
     * Wraps `engine.trigger` so every UI action is seen, without changing what the game does.
     *
     * `send` receives `{ kind, query }` (or `{ kind: 'clock', reasons }`). It is called after the original
     * trigger has already run, and any exception it throws is swallowed: a coordination report must never
     * be able to break the screen that produced it.
     */
    function observe(engine, send, options) {
        if (!engine || typeof engine.trigger !== 'function') return null;
        if (engine.trigger.__coopObserved) return engine.__coopObserver || null;

        var opts = options || {};
        var clock = createClock();
        var original = engine.trigger;
        var seen = 0;
        var last = '';

        function observed(name) {
            var args = Array.prototype.slice.call(arguments, 1);
            // The original first, always, and with the same `this`. If it throws, the game's own error
            // handling sees the throw: swallowing it here would hide real game bugs.
            var result = original.apply(this || engine, arguments);
            try {
                seen++;
                var ctx = typeof opts.context === 'function' ? opts.context() : {};
                var plan = classify(name, args, ctx);
                if (plan && plan.pause) {
                    var reasons = clock.apply(plan.pause.value, plan.pause.layer);
                    var ids = reasonIds(reasons);
                    // An empty set is meaningful (this player is no longer holding the world), but only
                    // after a layer was actually applied: an unmapped layer returns the unchanged set and
                    // must not be reported as "unpaused".
                    if (LAYER_TO_REASON[plan.pause.layer] === undefined) return result;
                    last = 'clock ' + ids.join(',');
                    send({ kind: 'clock', reasons: ids });
                } else if (plan) {
                    last = plan.kind + ' ' + plan.query;
                    send(plan);
                }
            } catch (e) {
                // Deliberately silent on the game's behalf; the bridge's own report line records the
                // failure, so it is visible without a console message per frame.
                try {
                    if (typeof opts.onError === 'function') opts.onError(e);
                } catch (e2) { /* a failing reporter must not fail the event */ }
            }
            return result;
        }

        // Named so a stack trace through a screen still points at the game's call site, and marked so a
        // second load cannot wrap the wrapper.
        observed.__coopObserved = true;
        engine.trigger = observed;
        var api = {
            clock: clock,
            /** How many UI events have been routed through here. */
            count: function () { return seen; },
            /** The last coordination line, for the status endpoint and the bridge's log. */
            last: function () { return last; },
            /** Puts the original back; used by tests and by the re-arm path. */
            restore: function () {
                engine.trigger = original;
                return true;
            }
        };
        engine.__coopObserver = api;
        return api;
    }

    var api = {
        observe: observe,
        classify: classify,
        createClock: createClock,
        reasonIds: reasonIds,
        REASON_ID: REASON_ID,
        LAYER_TO_REASON: LAYER_TO_REASON
    };

    if (typeof module === 'object' && module && module.exports) {
        module.exports = api;
    }
    root.CoopObserve = api;
})(typeof window !== 'undefined' ? window : this);
