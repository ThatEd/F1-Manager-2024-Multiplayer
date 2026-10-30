'use strict';
/*
 * clock_convergence_test.js - can the two machines end up disagreeing about whether the world is paused?
 *
 * The design says pauses are a **union**: whatever reasons either peer reports, the world stops if any reason
 * is present. That is a property of the *state*, so both peers should converge given the same information -
 * and `ClockGate::apply` is deliberately a local transition on replicated full state, with no arbitration in
 * the supervisor, precisely because a round trip per pause would make a stall look like an unfreezing game.
 *
 * This simulates the part that no unit test covers: what happens when a frame is *lost*. Each frame carries
 * one side's complete reason set (`Msg::Clock { reasons }`), so a dropped frame means one peer holds an older
 * set for the other side than that side actually has. The question is whether anything recovers it.
 *
 * The model here mirrors `coop-proto/src/coordination.rs` exactly:
 *
 *   set(side, reasons)   replaces that side's set outright
 *   is_stopped()         true when either side is non-empty
 *   apply(side, reasons) set(), then report the Stop/Run/NoChange transition
 *
 * Keeping the model in step with the Rust is the one weakness of a test like this; the guard is the
 * `mirrors_the_rust` check at the end, which fails if the source stops matching the shapes assumed here.
 */

const fs = require('fs');
const path = require('path');

const COORD = path.join(__dirname, '..', '..', 'coop-rs', 'crates', 'coop-proto', 'src', 'coordination.rs');
const src = fs.readFileSync(COORD, 'utf8');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

/** One machine's view: its own reasons, and its last-known copy of the other side's. */
class Gate {
  constructor(names) {
    this.host = new Set();
    this.client = new Set();
    this.names = names;
  }
  set(side, reasons) {
    this[side] = new Set(reasons);
  }
  isStopped() {
    return this.host.size > 0 || this.client.size > 0;
  }
  apply(side, reasons) {
    const was = this.isStopped();
    this.set(side, reasons);
    const now = this.isStopped();
    if (!was && now) return 'Stop';
    if (was && !now) return 'Run';
    return 'NoChange';
  }
  state() {
    return JSON.stringify([[...this.host].sort(), [...this.client].sort()]);
  }
}

console.log('clock_convergence: do both peers agree that the world is paused?\n');

// ---- 1. the union rule holds in the ordinary case ---------------------------------------
{
  const hostSide = new Gate();   // the host machine's own gate
  const clientSide = new Gate();
  // Host pauses for reason 0; tells the client.
  hostSide.apply('host', [0]);
  clientSide.apply('host', [0]);
  ok(hostSide.isStopped() && clientSide.isStopped(), 'one pause stops both machines');

  // Client also pauses; both should still be stopped, and neither should spuriously resume.
  hostSide.apply('client', [2]);
  clientSide.apply('client', [2]);
  ok(hostSide.isStopped() && clientSide.isStopped(), 'a second reason keeps both stopped');

  // Host resumes; the client still holds reason 2, so both stay stopped.
  hostSide.apply('host', []);
  clientSide.apply('host', []);
  ok(hostSide.isStopped() && clientSide.isStopped(),
    'one player resuming does not start the world while the other is paused');

  // Client resumes: now both agree it can run.
  hostSide.apply('client', []);
  clientSide.apply('client', []);
  ok(!hostSide.isStopped() && !clientSide.isStopped(), 'both resuming starts the world');
}

// ---- 2. a lost frame: the real question ---------------------------------------------------
{
  // The client pauses (reason 2). The frame carrying that set is lost. The host therefore still believes the
  // client has no reasons - and the host's sim keeps running while the client's is stopped.
  const hostSide = new Gate();
  const clientSide = new Gate();

  hostSide.set('client', []);          // host's stale copy
  clientSide.apply('client', [2]);     // client locally stops

  ok(clientSide.isStopped(), 'the client is stopped locally');
  ok(!hostSide.isStopped(), 'the host is unaware and keeps running <- the divergence');

  // Does the next frame fix it? Only if a frame is *sent*. `Msg::Clock` is documented as full state, and the
  // sender is expected to publish on every change - but nothing re-publishes after a loss.
  const recoveredWithoutNewFrame = hostSide.isStopped();
  ok(!recoveredWithoutNewFrame,
    'a lost clock frame is NOT recovered until the sender happens to publish again');

  // When a later frame does arrive, it repairs the state completely, because it is full state and not a delta.
  hostSide.apply('client', [2]);
  ok(hostSide.isStopped(), 'the next frame repairs it: full state, not a delta');
  ok(hostSide.state() === clientSide.state(), 'and the two gates converge', hostSide.state() + ' vs ' + clientSide.state());
}

// ---- 3. what makes it self-healing: repeated publication ----------------------------------
{
  // The agent publishes its reasons on change; the heartbeat runs once a second. If the sender re-publishes
  // while stopped, a single lost frame costs at most one heartbeat, not the whole pause.
  const hostSide = new Gate();
  let delivered = false;
  const clientReasons = [2];

  // First attempt is lost, second is delivered - modelling a re-publish.
  for (let attempt = 0; attempt < 2; attempt++) {
    if (attempt === 1) delivered = true;
    if (delivered) hostSide.apply('client', clientReasons);
  }
  ok(hostSide.isStopped(), 're-publishing while stopped heals a single lost frame');
}

// ---- 4. the model must still match the Rust it mirrors ------------------------------------
{
  const mirrors =
    /Side::Host => self\.host = reasons/.test(src) &&
    /Side::Client => self\.client = reasons/.test(src) &&
    /!self\.host\.is_empty\(\) \|\| !self\.client\.is_empty\(\)/.test(src) &&
    /\(false, true\) => ClockAction::Stop/.test(src) &&
    /\(true, false\) => ClockAction::Run/.test(src);
  ok(mirrors, 'this model still matches coordination.rs (set replaces, union stops, transition on change)',
    'coordination.rs changed shape - re-check this test against it');
}

// ---- 5. the load-bearing condition: does the sender REPEAT itself? ------------------------
//
// Sections 2 and 3 show the state rules are correct but only heal a loss if the sender publishes again
// without needing another local change. That is the whole question, and it is answered by the code, not by
// the model.
//
// This used to fail: `bridge.js` sends `/coordinate/clock` only from inside the `SimPauseChange` wrapper, and
// the agent's heartbeat emitted no clock at all - so one lost frame left the host running while the client
// stayed stopped for the rest of the race. The heartbeat now re-publishes the pause state once a second while
// paused, and these assertions pin that, because losing this behaviour is silent: everything still works
// except under a dropped frame.
{
  const bridge = fs.readFileSync(
    path.join(__dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'bridge.js'),
    'utf8'
  );
  const agent = fs.readFileSync(
    path.join(__dirname, '..', '..', 'coop-rs', 'crates', 'coop-agent', 'src', 'lib.rs'),
    'utf8'
  );

  // The front-end's send is a one-shot, by design: it reports transitions, not state.
  const sendSites = (bridge.match(/\/coordinate\/clock/g) || []).length;
  ok(sendSites === 1, 'the front-end sends clock state from exactly one place', String(sendSites));
  ok(/SimPauseChange/.test(bridge), 'and that place is the SimPauseChange wrapper (event-driven, not periodic)');

  // The agent must therefore be the thing that repeats.
  const heartbeatRepublishes = /coord::clock_frame\(/.test(agent);
  ok(heartbeatRepublishes,
    'the agent re-publishes the pause state from its heartbeat, so a lost frame heals',
    'no coord::clock_frame call in the agent: a dropped clock frame would never be repaired');
  ok(/if !reasons\.is_empty\(\)/.test(agent),
    'and only while paused, so idle play is not paying for a redundant frame every second');

  // With a repeat present, a loss costs one heartbeat and no more.
  {
    const host = new Gate();
    const client = new Gate();
    client.apply('client', [2]);
    ok(!host.isStopped(), 'the lost frame still leaves the host unaware');
    host.apply('client', [2]);   // the next heartbeat's re-publish
    ok(host.isStopped() && host.state() === client.state(),
      'and the next heartbeat repairs it - at most one second of divergence, not the whole race');
  }
}

// ---- 6. the receive path has no heartbeat-independent repair either ----------------------
{
  const agent = fs.readFileSync(
    path.join(__dirname, '..', '..', 'coop-rs', 'crates', 'coop-agent', 'src', 'lib.rs'),
    'utf8'
  );
  // The host's gate feeds the client via /coop/clock, which the bridge polls. Check that polling exists, since
  // a poll *would* repair a one-sided loss without any retransmission.
  const bridge = fs.readFileSync(
    path.join(__dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'bridge.js'),
    'utf8'
  );
  ok(/pollClock/.test(bridge) && /\/coop\/clock/.test(bridge),
    'the receiving side POLLS /coop/clock, which repairs a loss on its half without retransmission',
    'this is the half of the protocol that is already self-healing');
}

const failed = results.filter((r) => !r.ok).length;
console.log(`\nclock_convergence: ${results.length - failed} passed, ${failed} failed`);
process.exit(failed === 0 ? 0 : 1);
