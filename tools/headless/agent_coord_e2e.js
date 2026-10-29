'use strict';
/*
 * agent_coord_e2e.js - prove the coordination path over real sockets, end to end.
 *
 * The Rust unit tests cover each piece: the observer's mapping (JS), the frame grammar (coord.rs), the
 * arbitration (supervisor) and the delivery (new supervisor socket tests). What none of them cover is the
 * join between them: does the agent's HTTP endpoint turn a front-end request into a frame that reaches the
 * supervisor, and does the supervisor's answer come back to the other player?
 *
 * This drives a real `coop-supervisor` process and speaks its wire protocol directly as two agents. It is
 * the honest substitute for a game launch, which is what the coordination layer has never had.
 *
 *   VERIFIED:  the supervisor starts and accepts two agents
 *              a clock frame from the host's agent arrives at the client's agent
 *              a contract claim that loses is refused and the loser is told
 *              both votes are counted and the resolved outcome is delivered
 *   NOT VERIFIED: the agent's HTTP endpoint itself (that needs the DLL, which needs the game). The frame
 *              grammar it uses is covered by coord.rs's own tests, including the exact query strings the
 *              JS observer builds.
 */

const net = require('net');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');

const SUPERVISOR = process.env.COOP_SUPERVISOR_EXE ||
  path.join('E:', 'TEMP', 'coop-target', 'release', 'coop-supervisor.exe');

const MAGIC = Buffer.from('F1CP', 'ascii');
const VERSION = 2; // bumped when Msg::Agent gained the side field
const TAG_HELLO = 1, TAG_AGENT = 6, TAG_BYE = 5, TAG_CLOCK = 11, TAG_CONTRACT = 12, TAG_VOTE = 13;
const SIDE_HOST = 0, SIDE_CLIENT = 1;

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function fnv1a32(buf) {
  // Math.imul, not `h * 0x01000193`: h can be near 2^32 and the product then exceeds Number's exact
  // integer range (2^53), so plain multiplication silently rounds and the checksum is wrong for most
  // payloads. That is a real bug this test hit: the supervisor rejected every frame as "corrupt or torn".
  let h = 0x811c9dc5;
  for (const b of buf) {
    h ^= b;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
}

function frame(payload) {
  const head = Buffer.alloc(14);
  MAGIC.copy(head, 0);
  head.writeUInt16LE(VERSION, 4);
  head.writeUInt32LE(payload.length, 6);
  head.writeUInt32LE(fnv1a32(payload), 10);
  return Buffer.concat([head, payload]);
}

const hello = (side) => frame(Buffer.concat([Buffer.from([TAG_HELLO, side]), Buffer.alloc(4)]));

function agent(pid, side) {
  // tag(1) + pid(4) + rsm(8) + side(1)
  const b = Buffer.alloc(14);
  b.writeUInt8(TAG_AGENT, 0);
  b.writeUInt32LE(pid, 1);
  b.writeBigUInt64LE(0x1000n, 5);
  b.writeUInt8(side, 13);
  return frame(b);
}

function clockFrame(reasons, tick) {
  // Field order is tick first, then the length-prefixed reason list (coop-proto/src/lib.rs:577-586).
  const b = Buffer.alloc(1 + 4 + 1 + reasons.length);
  b.writeUInt8(TAG_CLOCK, 0);
  b.writeUInt32LE(tick, 1);
  b.writeUInt8(reasons.length, 5);
  reasons.forEach((r, i) => b.writeUInt8(r, 6 + i));
  return frame(b);
}

function contractFrame(person, team, day, kind) {
  const b = Buffer.alloc(1 + 4 + 1 + 4 + 1);
  let o = 0;
  b.writeUInt8(TAG_CONTRACT, o); o += 1;
  b.writeInt32LE(person, o); o += 4;
  b.writeUInt8(team, o); o += 1;
  b.writeInt32LE(day, o); o += 4;
  b.writeUInt8(kind, o);
  return frame(b);
}

function voteFrame(proposal, option) {
  const b = Buffer.alloc(1 + 2 + 1);
  b.writeUInt8(TAG_VOTE, 0);
  b.writeUInt16LE(proposal, 1);
  b.writeUInt8(option, 3);
  return frame(b);
}

/** Splits a stream into decoded frames, so a read never has to guess at TCP boundaries. */
function reader(sock) {
  let buf = Buffer.alloc(0);
  const queue = [];
  const waiters = [];
  sock.on('data', (d) => {
    buf = Buffer.concat([buf, d]);
    for (;;) {
      if (buf.length < 14) break;
      const len = buf.readUInt32LE(6);
      if (buf.length < 14 + len) break;
      const payload = buf.subarray(14, 14 + len);
      buf = buf.subarray(14 + len);
      const item = { tag: payload[0], body: payload.subarray(1) };
      if (waiters.length) waiters.shift()(item);
      else queue.push(item);
    }
  });
  return {
    next(ms = 3000) {
      if (queue.length) return Promise.resolve(queue.shift());
      return new Promise((resolve) => {
        // The waiter must be removed on timeout. Leaving it in the array meant a later frame was handed to
        // an already-resolved promise and silently lost - which read exactly like the supervisor failing to
        // deliver, and cost a debugging detour.
        const waiter = (v) => { clearTimeout(t); resolve(v); };
        const t = setTimeout(() => {
          const i = waiters.indexOf(waiter);
          if (i >= 0) waiters.splice(i, 1);
          resolve(null);
        }, ms);
        waiters.push(waiter);
      });
    },
    drain() { queue.length = 0; }
  };
}

/** Connects an agent and says who it is, as the real agent does on every heartbeat. */
function connectAgent(port, side) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(port, '127.0.0.1', () => {
      sock.write(agent(4242 + side, side));
      sock.write(agent(4242 + side, side)); // a heartbeat repeats; the registry must replace, not append
      resolve({ sock, r: reader(sock) });
    });
    sock.on('error', reject);
  });
}

/** Connects a *game* peer: announces with Hello, which is what coop-link does inside the game. */
function connectGamePeer(port, side) {
  return new Promise((resolve, reject) => {
    const sock = net.connect(port, '127.0.0.1', () => {
      sock.write(hello(side));
      resolve({ sock, r: reader(sock) });
    });
    sock.on('error', reject);
  });
}

async function main() {
  console.log('agent_coord_e2e: does a frame from one player reach the other?\n');

  const port = 46000 + Math.floor(Math.random() * 900);
  // Run the supervisor in a scratch directory. It writes `coop_rsm.txt` into its working directory whenever
  // an agent announces a located RaceSimManager, and a test should not leave that behind in the repo.
  const cwd = fs.mkdtempSync(path.join(os.tmpdir(), 'coop-e2e-'));
  const sup = spawn(SUPERVISOR, ['--port', String(port), '--bind', '127.0.0.1'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    cwd
  });
  const supLog = [];
  sup.stdout.on('data', (d) => supLog.push(String(d)));
  sup.stderr.on('data', (d) => supLog.push(String(d)));

  let exited = false;
  sup.on('exit', () => { exited = true; });

  try {
    // Wait for the listener rather than sleeping a guessed interval.
    let up = false;
    for (let i = 0; i < 60 && !up; i++) {
      if (exited) break;
      up = await new Promise((res) => {
        const s = net.connect(port, '127.0.0.1', () => { s.destroy(); res(true); });
        s.on('error', () => res(false));
      });
      if (!up) await sleep(100);
    }
    ok(up, 'supervisor started and is listening', supLog.join('').slice(0, 300));
    if (!up) throw new Error('supervisor never listened');

    const host = await connectAgent(port, SIDE_HOST);
    await sleep(150);
    const client = await connectAgent(port, SIDE_CLIENT);
    await sleep(200);
    // Deliberately NO Hello on these sockets. This mirrors reality: the peer slot belongs to the game DLL
    // (coop-link) on its own connection, and the agent's connection carries only Msg::Agent - which is why
    // the agent's identity had to be added to that message in the first place. Sending Hello here would
    // model a socket that is both an agent and a relay peer, which the real deployment never creates.
    await sleep(150);

    // ---- 1. a clock reaches the other agent -------------------------------------------------
    host.r.drain(); client.r.drain();
    host.sock.write(clockFrame([0, 2], 7));
    let got = await client.r.next();
    ok(got && got.tag === TAG_CLOCK, 'the client agent received the clock frame',
      got ? `tag ${got.tag}` : 'nothing arrived');
    if (got && got.tag === TAG_CLOCK) {
      // tick at 0..4, reason count at 4, reasons from 5.
      const tick = got.body.readUInt32LE(0);
      const n = got.body[4];
      const reasons = Array.from(got.body.subarray(5, 5 + n));
      ok(reasons.join(',') === '0,2', 'the pause reasons arrived intact', reasons.join(','));
      ok(tick === 7, 'the tick arrived intact', String(tick));
    }
    // A clock is for the other player only; the sender must not get its own pause echoed back.
    ok((await host.r.next(300)) === null, 'the sender does not receive its own clock back');

    // ---- 2. a losing claim is refused and the loser is told ---------------------------------
    host.r.drain(); client.r.drain();
    host.sock.write(contractFrame(9, 1, 4, 0));
    got = await client.r.next();
    ok(got && got.tag === TAG_CONTRACT, 'the claim was relayed to the other agent', got ? `tag ${got.tag}` : 'none');

    client.sock.write(contractFrame(9, 2, 4, 0));
    got = await client.r.next();
    ok(got && got.tag === TAG_BYE, 'the refused player was told why (Bye)', got ? `tag ${got.tag}` : 'none');

    // ---- 3. both votes count, and the outcome goes to both ---------------------------------
    host.r.drain(); client.r.drain();
    host.sock.write(voteFrame(12, 1));
    got = await client.r.next();
    ok(got && got.tag === TAG_VOTE, 'the first vote was relayed', got ? `tag ${got.tag}` : 'none');

    client.sock.write(voteFrame(12, 1));
    const a = client.r.next(2000);
    const b = host.r.next(2000);
    const [toClient, toHost] = await Promise.all([a, b]);
    ok(toClient && toClient.tag === TAG_VOTE, 'the resolved outcome reached the client', toClient ? `tag ${toClient.tag}` : 'none');
    ok(toHost && toHost.tag === TAG_VOTE, 'the resolved outcome reached the host', toHost ? `tag ${toHost.tag}` : 'none');
    if (toHost && toHost.tag === TAG_VOTE) {
      ok(toHost.body[2] === 1, 'the winner is the agreed option', String(toHost.body[2]));
    }

    const log = supLog.join('');
    ok(/refused/.test(log), 'the supervisor logged the refusal', log.split('\n').filter((l) => /refus/.test(l)).join(' | ').slice(0, 200));
    ok(/vote .*resolved|vote recorded/.test(log), 'the supervisor logged the vote tally');

    // ---- 4. the other direction: game peers still relay, and agents are not in that fan-out -------
    //
    // The two kinds of socket must stay separate. Replicated car state is for the game DLL; coordination
    // is for the agent. Writing state into an agent's stream (or coordination into the game's) produces a
    // message the other end silently ignores, which is indistinguishable from desync.
    const gameHost = await connectGamePeer(port, SIDE_HOST);
    await sleep(120);
    const gameClient = await connectGamePeer(port, SIDE_CLIENT);
    await sleep(200);

    host.r.drain(); client.r.drain();
    gameHost.sock.write(frame(Buffer.from([TAG_BYE]))); // a stand-in for any replicated state frame
    const toGame = await gameClient.r.next(2000);
    ok(toGame && toGame.tag === TAG_BYE, 'a game peer frame reaches the other game peer', toGame ? `tag ${toGame.tag}` : 'none');
    ok((await host.r.next(400)) === null, 'a game peer frame is NOT written into the host agent');
    ok((await client.r.next(400)) === null, 'a game peer frame is NOT written into the client agent');

    gameHost.sock.destroy();
    gameClient.sock.destroy();

    host.sock.destroy();
    client.sock.destroy();
  } finally {
    sup.kill();
    await sleep(200);
    try { fs.rmSync(cwd, { recursive: true, force: true }); } catch (e) { /* best effort */ }
  }

  const failed = results.filter((r) => !r.ok);
  console.log(`\nagent_coord_e2e: ${results.length - failed.length}/${results.length} passed`);
  if (failed.length) {
    console.log('failures:');
    for (const f of failed) console.log('  - ' + f.name);
    process.exit(1);
  }
}

main().catch((e) => {
  console.error('agent_coord_e2e crashed:', e.message);
  process.exit(1);
});
