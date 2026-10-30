'use strict';
/*
 * run_tests.js - offline tests for the headless client.
 *
 * Two layers, on purpose:
 *   1. unit   - the pure logic (legality + command grammar), no network
 *   2. live   - spawn the real tools/coop_server.py on a throwaway port and talk to it
 *
 * Layer 2 matters. A hand-rolled mock of the server would only test my mental model of it; the
 * point of this harness is to prove it speaks the server's actual contract, so the server is
 * started for real.
 *
 * Usage: node run_tests.js [--skip-live]
 */

const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');
const http = require('http');
const os = require('os');

const { HeadlessClient, planCommand, allowedFor } = require('./coop_client.js');

const HERE = __dirname;
const ROOT = path.resolve(HERE, '..', '..');
const SERVER_PY = path.join(ROOT, 'tools', 'coop_server.py');
const PORT = 46231;

let pass = 0, fail = 0;
const failures = [];

function ok(cond, name, detail) {
  if (cond) { pass++; console.log(`  ok    ${name}`); }
  else { fail++; failures.push(name + (detail ? ` -- ${detail}` : '')); console.log(`  FAIL  ${name}${detail ? ' -- ' + detail : ''}`); }
}

function eq(a, b, name) {
  ok(a === b, name, `expected ${JSON.stringify(b)}, got ${JSON.stringify(a)}`);
}

// ---------------------------------------------------------------------------------------------
// Layer 1: pure logic
// ---------------------------------------------------------------------------------------------
function testLegality() {
  console.log('\n[unit] command/page legality (mirrors bridge.allowedFor)');
  ok(allowedFor('load save2', 'Frontend'), 'Load allowed from Frontend');
  ok(!allowedFor('load save2', 'RaceWeekend'), 'Load REFUSED from RaceWeekend');
  ok(!allowedFor('load save2', 'TeamManagement'), 'Load REFUSED from TeamManagement');
  ok(allowedFor('Continue', 'Frontend'), 'Continue allowed from Frontend');
  ok(!allowedFor('Continue', 'RaceWeekend'), 'Continue REFUSED from RaceWeekend');
  ok(!allowedFor('continuegame', 'TeamManagement'), 'continuegame REFUSED from TeamManagement');
  ok(allowedFor('Advance', 'RaceWeekend'), 'Advance allowed from RaceWeekend');
  ok(allowedFor('Advance', 'TeamManagement'), 'Advance allowed from TeamManagement');
  ok(allowedFor('NextMonth', 'Frontend'), 'NextMonth not page-restricted');
  ok(allowedFor('menu', 'RaceWeekend'), 'QuitGame not page-restricted');
  ok(allowedFor('raw StageProceed', 'RaceWeekend'), 'raw not page-restricted');
  // case/whitespace robustness - the server hands us whatever the operator typed
  ok(!allowedFor('  LOAD   save3', 'RaceWeekend'), 'legality is case- and space-insensitive');
  ok(allowedFor('  advance 2  ', 'RaceWeekend'), 'advance with args still allowed');
}

function testGrammar() {
  console.log('\n[unit] command -> event grammar (mirrors bridge.dispatch)');
  eq(planCommand('Continue').event, 'ContinueGame', 'Continue -> ContinueGame');
  eq(planCommand('continuegame').event, 'ContinueGame', 'continuegame -> ContinueGame');
  eq(planCommand('Load save2').event, 'LoadGame', 'Load -> LoadGame');
  eq(planCommand('Load save2').args[0], 'save2', 'Load takes the slot as arg');
  eq(planCommand('load').args[0], 'autoSave', 'Load with no slot defaults to autoSave');
  eq(planCommand('Advance').event, 'AdvanceToNextEvent', 'Advance -> AdvanceToNextEvent');
  eq(planCommand('NextMonth').event, 'NextMonthSelected', 'NextMonth -> NextMonthSelected');
  eq(planCommand('menu').event, 'QuitGame', 'menu -> QuitGame');
  eq(planCommand('Raw StageProceed').event, 'StageProceed', 'Raw passes the event name through');
  eq(planCommand('Raw PreSessionClosed 1 2').args.length, 2, 'Raw passes trailing args');
  ok(planCommand('Raw').error, 'Raw with no name is an error');
  ok(planCommand('bogus').error, 'unknown command is an error');
  eq(planCommand('   '), null, 'blank line is ignored');
}

// ---------------------------------------------------------------------------------------------
// Layer 2: live server
// ---------------------------------------------------------------------------------------------
function get(urlPath) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: '127.0.0.1', port: PORT, path: urlPath, method: 'GET', timeout: 5000 },
      (res) => {
        let b = '';
        res.on('data', (d) => { b += d; });
        res.on('end', () => resolve({ status: res.statusCode, body: b }));
      });
    req.on('timeout', () => req.destroy(new Error('timeout')));
    req.on('error', reject);
    req.end();
  });
}

async function waitFor(fn, ms, label) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    try { if (await fn()) return true; } catch (_) {}
    await new Promise((r) => setTimeout(r, 120));
  }
  throw new Error('timed out waiting for ' + label);
}

function startServer(port) {
  // The server hardcodes PORT; patch it at runtime via an env var if supported, else use the file
  // as-is. coop_server.py defines PORT = 45999 at module scope, so we run it with a tiny wrapper
  // that overrides the constant before main() runs.
  const wrapper = path.join(os.tmpdir(), 'coop_srv_test_%d.py'.replace('%d', process.pid));
  fs.writeFileSync(wrapper, [
    'import runpy, sys, os',
    `sys.argv = [${JSON.stringify(SERVER_PY)}]`,
    'import importlib.util',
    `spec = importlib.util.spec_from_file_location("coop_server", ${JSON.stringify(SERVER_PY)})`,
    'mod = importlib.util.module_from_spec(spec)',
    'spec.loader.exec_module(mod)',
    `mod.PORT = ${port}`,
    'mod.main()',
  ].join('\n'), 'utf8');

  const p = spawn('python', [wrapper], { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
  p.stdout.setEncoding('utf8');
  p.stderr.setEncoding('utf8');
  p.on('error', (e) => console.log('  server spawn error:', e.message));
  // Keep the last of what the server said. Without it, a failed start is indistinguishable from a slow
  // one: both just say "timed out", and the reason (a Python error, a busy port) is thrown away.
  const said = [];
  const keep = (s) => { said.push(s); if (said.length > 40) said.shift(); };
  p.stdout.on('data', keep);
  p.stderr.on('data', keep);
  return { proc: p, wrapper, said };
}

async function testLive() {
  console.log('\n[live] real tools/coop_server.py on port ' + PORT);
  if (!fs.existsSync(SERVER_PY)) {
    ok(false, 'coop_server.py found', SERVER_PY);
    return;
  }

  const { proc, wrapper, said } = startServer(PORT);
  try {
    // 12s was not enough when the rest of the runner had just built the Rust workspace; the server is a
    // cold Python start, and this suite then failed for a reason unrelated to what it tests. A stable-cost
    // suite should not have a load-sensitive deadline, so poll until the process dies or the budget (which
    // can be raised on a slow machine) runs out - and report what the server actually said, because
    // "timed out" alone cannot distinguish "slow" from "crashed with a traceback".
    const budget = Number(process.env.COOP_TEST_STARTUP_MS || 60000);
    try {
      await waitFor(async () => {
        if (proc.exitCode !== null) throw new Error('server exited with ' + proc.exitCode);
        return (await get('/status')).status === 200;
      }, budget, 'server up');
    } catch (e) {
      console.log('  server output was:\n    ' + (said.join('').trim().split('\n').join('\n    ') || '(nothing)'));
      throw e;
    }
    ok(true, 'server started and answers /status');

    // A client in none-mode at Frontend: Continue must be accepted.
    const c = new HeadlessClient({ server: '127.0.0.1', port: PORT, mode: 'none', page: 'Frontend', pollMs: 80, verbose: false });
    const run = c.start();
    await waitFor(async () => (await get('/status')).body.includes('headless'), 8000, 'client registered');
    ok(true, 'headless client registered in /status');

    let st = JSON.parse((await get('/status')).body);
    let cid = Object.keys(st.clients)[0];
    eq(st.clients[cid].page, 'Frontend', 'server recorded page=Frontend');

    // Queue a Continue and watch it be delivered + acked.
    await get('/cmd?text=' + encodeURIComponent('Continue'));
    await waitFor(async () => {
      const s = JSON.parse((await get('/status')).body);
      const hist = JSON.stringify(s.history);
      return hist.includes('Continue') && s.clients[cid] && s.clients[cid].acks > 0;
    }, 8000, 'Continue ack');
    ok(true, 'Continue delivered and acked');

    st = JSON.parse((await get('/status')).body);
    ok((st.clients[cid].lastok || '').includes('Continue'),
      'lastok mentions Continue', (st.clients[cid].lastok || '').slice(0, 120));
    ok((st.clients[cid].lastok || '').includes('would trigger ContinueGame'),
      'headless reported it WOULD trigger, not that it did', (st.clients[cid].lastok || '').slice(0, 160));

    // Now the safety case: the server itself refuses to deliver Load to a client in a career.
    // (Server-side guard: take_for() adds the client to job['skipped'] and records 'skip'. The
    // client therefore never sees the command - which is the correct, defence-in-depth behaviour:
    // the server refuses FIRST, and the bridge's own allowedFor() is the second line.)
    c.page = 'RaceWeekend';
    const sentBefore = c.sent;
    await get('/cmd?text=' + encodeURIComponent('Load save2'));
    await waitFor(async () => {
      const s = JSON.parse((await get('/status')).body);
      const h = JSON.stringify(s.history || []);
      return h.includes('"kind": "skip"') || h.includes('"kind":"skip"');
    }, 8000, 'server-side Load refusal');
    ok(true, 'server refused to DELIVER Load to a client in a career');

    st = JSON.parse((await get('/status')).body);
    const skippedNote = (st.history || []).filter((h) => h.kind === 'skip' && /Load/i.test(h.cmd || ''))[0];
    ok(!!skippedNote, 'skip recorded with a reason', JSON.stringify(skippedNote || {}).slice(0, 140));
    ok(skippedNote && skippedNote.page === 'RaceWeekend', 'skip recorded the client page');
    ok(skippedNote && /front-end/i.test(skippedNote.why || ''), 'skip reason names the front-end rule');

    // The client must never have received it, so it must not have "sent" anything for it.
    ok(c.sent === sentBefore,
      'client never received the refused Load (server-side refusal is complete)',
      `sent ${sentBefore} -> ${c.sent}`);

    // Now test the CLIENT-side guard directly, by handing it the command the server would refuse.
    // This is the second line of defence and must work even if the server let it through.
    const refusedBefore = c.refused;
    await c.handle('Load save2');
    ok(c.refused > refusedBefore, 'client-side guard also refuses Load in a career');
    ok((c._lastRefusal || '').includes('refused on RaceWeekend'),
      'client refusal names the page',
      (c._lastRefusal || '').slice(0, 120));

    // Advance must still work from inside a career.
    await get('/cmd?text=' + encodeURIComponent('Advance'));
    await waitFor(async () => {
      const s = JSON.parse((await get('/status')).body);
      return JSON.stringify(s.history).includes('"cmd": "Advance"') || JSON.stringify(s.history).includes('Advance');
    }, 8000, 'Advance delivered');
    st = JSON.parse((await get('/status')).body);
    ok(JSON.stringify(st.history).includes('Advance'), 'Advance queued and delivered');
    ok(!(st.clients[cid].lasterr || '').includes('Advance'),
      'Advance was NOT refused inside a career', (st.clients[cid].lasterr || '').slice(0, 120));

    c.stop();
    await new Promise((r) => setTimeout(r, 150));
  } finally {
    try { proc.kill(); } catch (_) {}
    try { fs.unlinkSync(wrapper); } catch (_) {}
  }
}

// ---------------------------------------------------------------------------------------------
async function main() {
  const skipLive = process.argv.includes('--skip-live');
  console.log('headless client tests');

  testLegality();
  testGrammar();
  if (!skipLive) { try { await testLive(); } catch (e) { ok(false, 'live suite', e.message); } }
  else console.log('\n[live] skipped');

  console.log(`\n${pass} passed, ${fail} failed`);
  if (fail) { console.log('failures:'); failures.forEach((f) => console.log('  - ' + f)); process.exit(1); }
}

main().catch((e) => { console.error(e); process.exit(1); });
