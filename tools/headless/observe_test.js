'use strict';
/*
 * observe_test.js - verify the SHIPPING coordination observer (pak/.../js/coop/observe.js).
 *
 * The observer is the producer that was missing: the protocol, the supervisor's arbitration and the
 * agent's sender all existed, and every one of the four coordination rules was inert because nothing in
 * either game produced a frame. This suite checks the mapping from what a player does to the frame that
 * goes out, and - just as important - that observing cannot change what the game does.
 *
 *   VERIFIED:  SimPauseChange becomes a reason set, not one flag, and the union across layers is right
 *              every mapped PauseLayer reaches a real PauseReason number
 *              QuickSim and Replay are not propagated
 *              SetDilemmaResponse becomes a vote with the sentinel range respected
 *              the four contract events become claim/confirm/release
 *              a contract is refused rather than sent with an unknown day
 *              the original engine.trigger always runs, first, with identical arguments
 *              a throw inside the observer never reaches the screen that fired the event
 *              wrapping twice does not double-send
 *
 *   NOT VERIFIED: whether the engine actually pauses when the far side sends a clock frame. That is the
 *              agent's write path and can only be confirmed in-game.
 *
 *   ASSUMED (and stated as such): that the game's own call sites use these exact event names. Every name
 *              asserted here was read from the export; see the file header of observe.js for the
 *              file:line of each.
 */

const path = require('path');
const fs = require('fs');
const OBSERVE_PATH = path.join(
  __dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'observe.js'
);
const BRIDGE = path.join(
  __dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'bridge.js'
);
const C = require(OBSERVE_PATH);

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

/* A stand-in for the game's `engine` host object. Records what the game was asked to do, so a test can
   prove the original call happened and happened first. */
function fakeEngine() {
  const calls = [];
  const engine = {
    trigger: function (name) {
      calls.push({ name: name, args: Array.prototype.slice.call(arguments, 1), when: calls.length });
      return 'game-result';
    }
  };
  return { engine, calls };
}

function run(name, fn) {
  try {
    fn();
  } catch (e) {
    ok(false, name + ' (threw: ' + e.message + ')');
  }
}

console.log('observe_test: the coordination observer produces frames from real UI events\n');

// ---------------------------------------------------------------------------------------------
// The pause layer mapping
// ---------------------------------------------------------------------------------------------

console.log('pause layers -> pause reasons');

run('mapping', () => {
  /* PauseLayer numbering is from GameTypes.js:1565-1576 and is NOT the protocol's numbering: layer 0 is
     UserTimeControl while PauseReason 0 is UserPause. This asserts the two are translated by name. */
  const layers = Object.keys(C.LAYER_TO_REASON).map(Number).sort((a, b) => a - b);
  ok(layers.join(',') === '0,2,3,4,6,7,8,9,10,11', 'the mapped layers are the world-stopping ones', layers.join(','));
  ok(C.LAYER_TO_REASON[1] === undefined, 'QuickSim (1) is local fast-forward and is not propagated');
  ok(C.LAYER_TO_REASON[5] === undefined, 'Replay (5) does not stop the world and is not propagated');

  /* Every mapped layer must name a reason the protocol actually has, or the far side would receive a
     byte it cannot name and the clock could never be cleared. */
  const names = Object.keys(C.REASON_ID);
  let bad = [];
  for (const layer of layers) {
    if (names.indexOf(C.LAYER_TO_REASON[layer]) < 0) bad.push(layer + '->' + C.LAYER_TO_REASON[layer]);
  }
  ok(bad.length === 0, 'every mapped layer names a real PauseReason', bad.join(', '));

  // The translated numbers are the protocol's, not the layer's. Layer 0 must become reason 1.
  ok(C.REASON_ID[C.LAYER_TO_REASON[0]] === C.REASON_ID.UserTimeControl && C.REASON_ID.UserTimeControl === 1,
    'layer 0 (UserTimeControl) translates to protocol reason 1, not 0',
    'got ' + C.REASON_ID[C.LAYER_TO_REASON[0]]);
});

// ---------------------------------------------------------------------------------------------
// The clock is a set
// ---------------------------------------------------------------------------------------------

console.log('\nthe clock is a union of layers, not a flag');

run('clock', () => {
  const clock = C.createClock();
  let r = clock.apply(true, 9); // RaceViewUI
  ok(r.length === 1 && r[0] === 'UserPause', 'one layer holds the clock', JSON.stringify(r));

  // A second, different layer joins: the set must grow, not replace.
  r = clock.apply(true, 3); // Notifications
  ok(r.length === 2, 'a second layer is unioned rather than replacing the first', JSON.stringify(r));
  ok(C.reasonIds(r).join(',') === '0,2', 'the union serialises to two sorted reason ids', C.reasonIds(r).join(','));

  /* The player closes the notification while still in the race view. The world must stay stopped: this is
     the exact failure the protocol's "full state, not a delta" rule exists to prevent. */
  r = clock.apply(false, 3);
  ok(r.length === 1 && r[0] === 'UserPause', 'clearing one layer leaves the other holding', JSON.stringify(r));

  // Both cleared: the player has released the world.
  r = clock.apply(false, 9);
  ok(r.length === 0, 'clearing every layer releases the clock', JSON.stringify(r));

  // An unmapped layer must not be mistaken for "unpaused".
  const before = clock.layers().join(',');
  r = clock.apply(true, 1); // QuickSim
  ok(r.length === 0 && clock.layers().join(',') === before, 'an unmapped layer changes nothing at all');
});

run('clock dedupe', () => {
  /* Two layers that map to the same reason must produce one id, or the far side sees a duplicate and the
     payload churns for no reason. */
  const clock = C.createClock();
  clock.apply(true, 2); // UserPause
  const r = clock.apply(true, 9); // also UserPause
  ok(C.reasonIds(r).join(',') === '0', 'layers sharing a reason collapse to one id', C.reasonIds(r).join(','));
});

// ---------------------------------------------------------------------------------------------
// Votes
// ---------------------------------------------------------------------------------------------

console.log('\nSetDilemmaResponse -> a vote');

run('vote', () => {
  const v = C.classify('SetDilemmaResponse', [12, 1], {});
  ok(v && v.kind === 'vote', 'a dilemma response is classified as a vote');
  ok(v.query === 'proposal=12&option=1', 'the mail id is the proposal and the index the option', v.query);

  /* 0xFF is the supervisor's resolved-tie sentinel (encode_vote_result). A real vote must never carry it,
     or both peers would read the vote as "tied". */
  ok(C.classify('SetDilemmaResponse', [12, 255], {}) === null, 'option 0xFF is refused, not sent');
  ok(C.classify('SetDilemmaResponse', [12, 256], {}) === null, 'an option outside the wire range is refused');
  ok(C.classify('SetDilemmaResponse', [-1, 0], {}) === null, 'a negative mail id is refused');
  ok(C.classify('SetDilemmaResponse', ['abc', 0], {}) === null, 'a non-numeric mail id is refused');
});

// ---------------------------------------------------------------------------------------------
// Contracts
// ---------------------------------------------------------------------------------------------

console.log('\ncontract events -> claim / confirm / release');

run('contracts', () => {
  const ctx = { team: 3, day: 11 };
  const claim = C.classify('OnSubmitContractOffer', [{}, 42], ctx);
  ok(claim && claim.kind === 'contract' && claim.query === 'person=42&team=3&day=11&kind=claim',
    'OnSubmitContractOffer is a claim for the staff id in the second argument', claim && claim.query);

  /* The staff id is argument 1, not 0: the first argument is the terms object. Getting this backwards
     would claim a person named "0" for every offer. */
  ok(claim.query.indexOf('person=42') === 0, 'the staff id comes from args[1] of OnSubmitContractOffer');

  const confirm = C.classify('OnConfirmStaffContract', [42], ctx);
  ok(confirm && confirm.query === 'person=42&team=3&day=11&kind=confirm',
    'OnConfirmStaffContract is the signing', confirm && confirm.query);

  ok(C.classify('OnCancelStaffContract', [42], ctx).query.indexOf('kind=release') > 0,
    'cancelling frees the person');
  ok(C.classify('OnFailedStaffContract', [42], ctx).query.indexOf('kind=release') > 0,
    'a failed negotiation frees the person too');

  /* Cancelling inserts arguments in a different order internally; this proves the staff id is read from
     args[0] there and that a contract can never be emitted with a person of 0 from a missing argument. */
  ok(C.classify('OnCancelStaffContract', [], ctx) === null, 'a contract event with no staff id is refused');
});

run('contracts need a day', () => {
  /* The day rule is the whole point of the contract message: "two people cannot do the same instant
     contract for someone the same day". A frame with an unknown day would either be refused at the far
     end or, if that check were relaxed, bypass the rule. Refusing locally is the only safe option. */
  ok(C.classify('OnConfirmStaffContract', [42], { team: 3 }) === null, 'no day: refused rather than sent');
  ok(C.classify('OnConfirmStaffContract', [42], { team: 3, day: 0 }) === null, 'day 0 is not a calendar day');
  ok(C.classify('OnConfirmStaffContract', [42], { team: 3, day: -2 }) === null, 'a negative day is refused');
  ok(C.classify('OnConfirmStaffContract', [42], { day: 5 }) === null, 'no team: refused rather than sent');
});

// ---------------------------------------------------------------------------------------------
// Unrelated events
// ---------------------------------------------------------------------------------------------

console.log('\nunrelated events are ignored');

run('ignore', () => {
  for (const name of ['ContinueGame', 'LoadGame', 'AdvanceToNextEvent', 'MailRead', 'DriverCommandPaceChange',
    'SetPlaybackRate', 'PreSessionClosed', 'QuitGame']) {
    ok(C.classify(name, [1, 2, 3], { team: 1, day: 2 }) === null, name + ' is not a coordination event');
  }
});

// ---------------------------------------------------------------------------------------------
// Interception must be additive
// ---------------------------------------------------------------------------------------------

console.log('\nobservation cannot change what the game does');

run('pass-through', () => {
  const { engine, calls } = fakeEngine();
  const seen = [];
  const api = C.observe(engine, (p) => seen.push(p), { context: () => ({ team: 1, day: 4 }) });

  const ret = engine.trigger('DriverCommandPaceChange', 3, 'Push');
  ok(ret === 'game-result', 'the original return value is passed through unchanged', ret);
  ok(calls.length === 1 && calls[0].name === 'DriverCommandPaceChange', 'the game still received the event');
  ok(calls[0].args.join(',') === '3,Push', 'with identical arguments', calls[0].args.join(','));
  ok(seen.length === 0, 'an unrelated event produces no coordination frame');

  engine.trigger('OnConfirmStaffContract', 7);
  ok(seen.length === 1 && seen[0].kind === 'contract', 'a contract event does produce one');
  ok(api.count() === 2, 'every event is counted, not only the interesting ones', api.count());
});

run('original runs first', () => {
  /* Order matters: the observer reads state the engine may update. More importantly, if the original were
     called after the observer, an observer that threw could prevent the game from acting at all. */
  const order = [];
  const engine = { trigger: function (n) { order.push('game:' + n); } };
  C.observe(engine, () => order.push('coop'), {});
  engine.trigger('SimPauseChange', true, 9);
  ok(order.join('|') === 'game:SimPauseChange|coop', 'the game acts before the observer reports', order.join('|'));
});

run('failure isolation', () => {
  /* A coordination report is not worth breaking a screen for. The throw is swallowed *after* the game has
     acted, so the player sees nothing and the game continues. The context is supplied so the event is
     actually classified as sendable - otherwise the refusal happens before `send` and nothing throws. */
  const { engine, calls } = fakeEngine();
  const errors = [];
  C.observe(engine, () => { throw new Error('agent unreachable'); }, {
    context: () => ({ team: 2, day: 6 }),
    onError: (e) => errors.push(e.message)
  });

  let escaped = null;
  try {
    engine.trigger('OnConfirmStaffContract', 9);
  } catch (e) {
    escaped = e;
  }
  ok(escaped === null, 'a failed report does not throw into the game', escaped && escaped.message);
  ok(calls.length === 1, 'the game still received the event');
  ok(errors.length === 1 && errors[0] === 'agent unreachable', 'the failure is reported, not hidden', errors.join(','));
});

run('failure isolation with no error handler', () => {
  const { engine } = fakeEngine();
  C.observe(engine, () => { throw new Error('boom'); }, { context: () => ({ team: 2, day: 6 }) });
  let escaped = null;
  try { engine.trigger('OnConfirmStaffContract', 1); } catch (e) { escaped = e; }
  ok(escaped === null, 'still no throw when the caller supplied no onError');
});

run('idempotent wrap', () => {
  /* The bridge may be re-armed when the co-op screen remounts. Wrapping the wrapper would send every
     frame twice, which for a contract claim means the second one is refused as "already claimed". */
  const { engine, calls } = fakeEngine();
  const seen = [];
  const a = C.observe(engine, (p) => seen.push(p), { context: () => ({ team: 1, day: 2 }) });
  const b = C.observe(engine, (p) => seen.push(p), { context: () => ({ team: 1, day: 2 }) });
  ok(b === a, 'a second observe returns the first observer rather than wrapping again');
  engine.trigger('OnConfirmStaffContract', 5);
  ok(seen.length === 1, 'the event is reported exactly once', String(seen.length));
  ok(calls.length === 1, 'and the game acted exactly once', String(calls.length));
});

run('restore', () => {
  const { engine } = fakeEngine();
  const api = C.observe(engine, () => {}, {});
  ok(typeof engine.trigger === 'function' && engine.trigger.__coopObserved, 'the wrapper is installed');
  api.restore();
  ok(!engine.trigger.__coopObserved, 'restore puts the original back');
});

run('no engine', () => {
  /* The bridge loads before `engine` exists on some pages. observe must decline rather than throw, so the
     bridge can call it unconditionally once the engine is up. */
  ok(C.observe(null, () => {}, {}) === null, 'no engine: returns null');
  ok(C.observe({}, () => {}, {}) === null, 'an engine with no trigger: returns null');
});

// ---------------------------------------------------------------------------------------------
// The clock decision: act once per transition, through the game's own event
// ---------------------------------------------------------------------------------------------

console.log('\nthe clock is applied once per transition, on the race screen only');

/*
 * A faithful model of `applyClock`/`pollClock` from bridge.js. Kept as a model rather than loading the
 * bridge because the bridge is an IIFE that starts network loops on load; the *decision* is what matters
 * here and it is three lines of logic. The layer number and the guard are asserted against the real file
 * below, so the model cannot drift silently.
 */
function clockDecision(body, page) {
  if (page !== 'RaceWeekend') return null;
  return { value: !!body.stopped, layer: 9 };
}

run('clock decision', () => {
  ok(clockDecision({ stopped: true }, 'RaceWeekend').value === true,
    'the other player pausing asks the game to pause');
  ok(clockDecision({ stopped: false }, 'RaceWeekend').value === false,
    'the last reason clearing asks the game to resume');
  ok(clockDecision({ stopped: true }, 'Frontend') === null,
    'nothing is fired on the front-end: there is no sim to pause');
  ok(clockDecision({ stopped: true }, 'TeamManagement') === null,
    'nothing is fired in team management');
  ok(clockDecision({ stopped: true }, 'RaceWeekend').layer === 9,
    'the pause layer is RaceViewUI (9), not UserPause (2)');

  /* Layer 9 rather than 2 is load-bearing: our own layer means unpausing restores only what the mod held.
     Using UserPause would let one machine cancel a pause the other player set themselves. `PauseReason`
     and `PauseLayer` are numbered differently, so this is checked against the export's numbering. */
  ok(C.LAYER_TO_REASON[9] !== undefined, 'layer 9 is a known layer');
});

run('the transition rule', () => {
  /* Acting on `stopped` every poll would re-pause a paused world forever and the pause would never end.
     bridge.js acts only when `seq` changes; this pins the rule it relies on. */
  const seen = [];
  const poll = (body, state) => {
    if (typeof body.seq === 'number' && body.seq !== state.seq) {
      const previous = state.seq;
      state.seq = body.seq;
      if (previous !== 0 || body.stopped) seen.push(body.stopped);
    }
  };
  const state = { seq: 0 };
  poll({ seq: 0, stopped: false }, state);
  ok(seen.length === 0, 'the initial state is not acted on: seq 0 means nothing happened yet');
  poll({ seq: 1, stopped: true }, state);
  poll({ seq: 1, stopped: true }, state);
  poll({ seq: 1, stopped: true }, state);
  ok(seen.length === 1, 'a repeated identical state is acted on exactly once', seen.join(','));
  poll({ seq: 2, stopped: false }, state);
  ok(seen.length === 2 && seen[1] === false, 'the release is acted on', seen.join(','));
});

run('the model matches the shipping bridge', () => {
  /* The decision above is a model, so it is only useful if the real file agrees. These assert the three
     things that would break coordination silently if they changed: the layer number, the page guard, and
     the give-up. Reading the shipping file is the point - a test of a copy proves nothing about what ships. */
  const src = fs.readFileSync(BRIDGE, 'utf8');
  ok(/engine\.trigger\('SimPauseChange', value, 9\)/.test(src),
    'the bridge pauses through SimPauseChange with layer 9',
    'the call site changed - update the model or the bridge');
  ok(/if \(PAGE !== 'RaceWeekend'\)/.test(src),
    'the bridge refuses to pause outside the race weekend');
  ok(/clock\.misses >= CLOCK_GIVE_UP/.test(src),
    'the clock poll gives up when the agent is absent');
  ok(/pollClock\(\)/.test(src), 'the clock poll is actually started');
});

// ---------------------------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------------------------

const failed = results.filter((r) => !r.ok);
console.log(`\nobserve_test: ${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log('failures:');
  for (const f of failed) console.log('  - ' + f.name);
  process.exit(1);
}
