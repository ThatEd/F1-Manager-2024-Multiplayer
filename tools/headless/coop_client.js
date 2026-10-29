'use strict';
/*
 * coop_client.js - a headless client for the F1 Manager 2024 co-op command protocol.
 *
 * Purpose: drive a second F1 Manager client from outside the game, over the SAME protocol the
 * in-game pak bridge speaks, so the in-game bridge and this harness are interchangeable as far as
 * tools/coop_server.py is concerned.
 *
 * Why this can exist
 * ------------------
 * The pak bridge needs Coherent/Cohtml, which means a real game process per client. The server
 * never talks to Cohtml: it hands out a command string on `GET /hello` and reads an ack string.
 * Those are plain HTTP. So a client can be a Node process that speaks them.
 *
 * What it is NOT
 * --------------
 * This harness does not own a RaceSim, does not render, and cannot invent game state. It is a
 * protocol endpoint: it receives commands, decides whether it is allowed to run them, and reports
 * what happened. In `none` mode it never claims the game did anything.
 */

const http = require('http');
const fs = require('fs');
const path = require('path');

const DEFAULTS = {
  server: '127.0.0.1',
  port: 45999,
  mode: 'none',        // none | replay | ui
  page: 'Frontend',
  stage: 'menu',
  pollMs: 250,
  verbose: true,
  // IMPORTANT: coop_server.py identifies a client by its SOURCE IP (client_address[0]), not by any
  // id we send. Two clients on one machine therefore collide into a single registry entry and
  // silently steal each other's commands. Binding to a second loopback address (127.0.0.2) makes
  // the harness a distinct client on the same box. All of 127.0.0.0/8 is loopback on Windows.
  localAddress: null,
};

// ---------------------------------------------------------------------------------------------
// The page/command legality table, mirrored from pak/.../bridge.js `allowedFor`.
//
// This is the one safety-critical piece of logic in the harness: firing LoadGame or ContinueGame
// from inside a career tears the front-end down and leaves a black screen (LOBBY-GATE.md). The
// bridge refuses locally and the server refuses again; a headless client must refuse identically,
// or it becomes the one client that can black-screen itself.
// ---------------------------------------------------------------------------------------------
const LOAD_FROM = ['Frontend'];
const MENU_FROM = ['Frontend'];

function allowedFor(cmd, page) {
  // Trim BEFORE splitting, exactly as bridge.dispatch does. Splitting an untrimmed string makes
  // parts[0] === '' for "  Load save2", which silently defeats this guard - and this guard is the
  // one thing standing between a client and the black screen.
  const line = String(cmd).replace(/[\r\n]+/g, ' ').trim();
  const c = line.split(/\s+/)[0].toLowerCase();
  if (c === 'load' || c === 'loadgame') return LOAD_FROM.indexOf(page) >= 0;
  if (c === 'continue' || c === 'continuegame') return MENU_FROM.indexOf(page) >= 0;
  return true;
}

// ---------------------------------------------------------------------------------------------
// Command -> event interpolation, mirrored from bridge.js dispatch().
// Deliberately small: the bridge is the reference implementation; this copies its grammar.
// ---------------------------------------------------------------------------------------------
function planCommand(text) {
  const line = String(text).replace(/[\r\n]+/g, ' ').trim();
  if (!line) return null;
  const parts = line.split(/\s+/);
  const cmd = parts[0].toLowerCase();

  if (cmd === 'continue' || cmd === 'continuegame') return { event: 'ContinueGame', args: [] };
  if (cmd === 'load' || cmd === 'loadgame') return { event: 'LoadGame', args: [parts[1] || 'autoSave'] };
  if (cmd === 'advance') return { event: 'AdvanceToNextEvent', args: [] };
  if (cmd === 'nextmonth') return { event: 'NextMonthSelected', args: [] };
  if (cmd === 'menu' || cmd === 'mainmenu') return { event: 'QuitGame', args: [] };
  if (cmd === 'raw') {
    if (!parts[1]) return { error: 'raw needs an event name' };
    return { event: parts[1], args: parts.slice(2) };
  }
  return { error: 'unknown command' };
}

function enc(s) { return encodeURIComponent(String(s)); }

function request(opts, urlPath, timeoutMs) {
  return new Promise((resolve, reject) => {
    const reqOpts = {
      host: opts.server, port: opts.port, path: urlPath, method: 'GET',
      timeout: timeoutMs || 8000,
    };
    if (opts.localAddress) reqOpts.localAddress = opts.localAddress;
    const req = http.request(reqOpts, (res) => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', (d) => { body += d; });
      res.on('end', () => resolve({ status: res.statusCode, body }));
    });
    req.on('timeout', () => req.destroy(new Error('timeout')));
    req.on('error', reject);
    req.end();
  });
}

class HeadlessClient {
  constructor(opts) {
    this.o = Object.assign({}, DEFAULTS, opts || {});
    this.page = this.o.page;
    this.stage = this.o.stage;
    this.running = false;
    this.ticks = 0;
    this.sent = 0;
    this.applied = 0;
    this.refused = 0;
    this.errors = 0;
    this.lastAck = '';
    this._pendingAck = '';
    // Kept separately from _pendingAck because the poll consumes that field. An assertion read after an
    // await could otherwise find it cleared by a background tick and see a refusal that did happen as
    // missing. This one is only ever written.
    this._lastRefusal = '';
    this._stop = false;
    this._replayQueue = [];
    this.logLines = [];
  }

  log(...a) {
    const line = `[${new Date().toISOString().slice(11, 23)}] ${a.join(' ')}`;
    this.logLines.push(line);
    if (this.o.verbose) console.log(line);
  }

  async report(text, ok) {
    try {
      await request(this.o, `/report?page=${enc(this.page)}&text=${enc((ok ? 'OK ' : 'ERR ') + text)}`);
    } catch (_) { /* the server may be gone; a headless client must not die for it */ }
  }

  async tick() {
    this.ticks++;
    const q = [
      'page=' + enc(this.page),
      'stage=' + enc(this.stage),
      'sent=' + this.sent,
      'pkg=headless',
      'v=1',
      'ack=' + enc(this._pendingAck),
    ].join('&');
    this._pendingAck = '';

    let res;
    try {
      res = await request(this.o, `/hello?${q}`);
    } catch (e) {
      this.log('poll failed:', e.message);
      return;
    }
    if (res.status !== 200) { this.log('poll http', res.status); return; }
    const text = (res.body || '').trim();
    if (text) {
      this.log('recv:', text);
      await this.handle(text);
    }
  }

  async handle(text) {
    const plan = planCommand(text);
    if (!plan) return;

    if (plan.error) {
      this.refused++;
      this._pendingAck = `ERR ${text} :: ${plan.error}`;
      this._lastRefusal = this._pendingAck;
      await this.report(`${text} :: ${plan.error}`, false);
      return;
    }
    if (!allowedFor(text, this.page)) {
      this.refused++;
      const why = `refused on ${this.page} (load/continue need Frontend)`;
      this._pendingAck = `ERR ${text} :: ${why}`;
      this._lastRefusal = this._pendingAck;
      this.log('REFUSED', text, '-', why);
      await this.report(`${text} :: ${why}`, false);
      return;
    }

    this.sent++;
    const desc = `${plan.event}${plan.args.length ? ' ' + plan.args.join(' ') : ''}`;

    if (this.o.mode === 'none') {
      // Honest mode: there is no engine, so nothing is claimed.
      const note = `would trigger ${desc} (no engine attached)`;
      this._pendingAck = `OK ${text} :: ${note}`;
      this.log(note);
      await this.report(`${text} :: ${note}`, true);
      return;
    }

    if (this.o.mode === 'ui') {
      // Actually operate the rendered UI. `via` is part of the ack so a caller can tell a real press
      // ('ui') from a fallback send ('synthetic') without reading logs - the distinction is the whole
      // reason this mode exists instead of just calling engine.sendEvent.
      if (!this._driver) {
        const { UiDriver } = require('./ui_driver.js');
        this._driver = new UiDriver({ quiet: true, page: this.page });
      }
      const r = await this._driver.apply(plan);
      if (r.via === 'failed') {
        this.errors++;
        this._pendingAck = `ERR ${text} :: ${r.detail}`;
        this._lastRefusal = this._pendingAck;
        this.log('UI FAILED', plan.event, '-', r.detail);
        await this.report(`${text} :: ${r.detail}`, false);
      } else {
        this.applied++;
        this._pendingAck = `OK ${text} :: ${r.detail} via=${r.via}`;
        this.log('ui', plan.event, '->', r.via, '-', r.detail);
        await this.report(`${text} :: ${r.detail} via=${r.via}`, true);
      }
      return;
    }

    const outcome = this._replayQueue.length ? this._replayQueue.shift() : null;
    if (outcome && outcome.outcome) {
      this.applied++;
      this._pendingAck = `OK ${text} :: replayed ${outcome.outcome}`;
      this.log('replayed', plan.event, '->', outcome.outcome);
      await this.report(`${text} :: replayed ${outcome.outcome}`, true);
    } else {
      this.errors++;
      this._pendingAck = `ERR ${text} :: no recorded outcome`;
      await this.report(`${text} :: no recorded outcome`, false);
    }
  }
  loadReplay(file) {
    const raw = fs.readFileSync(file, 'utf8');
    this._replayQueue = raw.split('\n').filter(Boolean).map((l) => {
      try { return JSON.parse(l); } catch (_) { return null; }
    }).filter(Boolean);
    this.log('loaded', this._replayQueue.length, 'records from', path.basename(file));
  }

  async start() {
    this.running = true;
    this.log(`headless client -> http://${this.o.server}:${this.o.port}  mode=${this.o.mode} page=${this.page}`
      + (this.o.localAddress ? `  localAddress=${this.o.localAddress}` : ''));
    if (this.o.mode === 'replay' && this.o.replay) this.loadReplay(this.o.replay);
    while (this.running && !this._stop) {
      await this.tick();
      await new Promise((r) => setTimeout(r, this.o.pollMs));
    }
  }

  stop() { this._stop = true; this.running = false; }

  summary() {
    return {
      mode: this.o.mode, page: this.page, ticks: this.ticks, sent: this.sent,
      applied: this.applied, refused: this.refused, errors: this.errors,
    };
  }
}

function parseArgs(argv) {
  const o = {};
  for (const a of argv) {
    const m = /^--([a-zA-Z0-9_-]+)(?:=(.*))?$/.exec(a);
    if (!m) continue;
    const k = m[1].replace(/-/g, '_');
    let v = m[2];
    if (v === undefined) v = true;
    if (v === 'true') v = true; else if (v === 'false') v = false;
    o[k] = v;
  }
  return o;
}

async function main() {
  const a = parseArgs(process.argv.slice(2));

  if (a.help) {
    console.log(`headless client for F1 Manager 2024 co-op

  --server=HOST     coop_server host (default 127.0.0.1)
  --port=N          coop_server port (default 45999)
  --mode=MODE       none | replay | ui  (default none)
  --replay=FILE     JSONL recording, required for --mode=replay
  --page=PAGE       Frontend | RaceWeekend | TeamManagement  (default Frontend)
  --stage=STAGE     reported stage string (default menu)
  --poll=MS         poll interval (default 250)
  --local-address=IP  bind the outgoing socket to this local address. Use 127.0.0.2 when a
                      real in-game bridge is also on 127.0.0.1, otherwise the server keys both
                      clients on the same source IP and they overwrite each other.
  --quiet           less output

Modes:
  none     speak the protocol honestly; never claim the game did anything
  replay   consume a JSONL recording and report the recorded outcomes

There is deliberately no mode that fabricates success. A test that invents its own pass result
is not a test.`);
    return;
  }

  if (a.mode && !['none', 'replay', 'ui'].includes(a.mode)) {
    console.error(`unknown --mode=${a.mode}`);
    process.exit(2);
  }
  if (a.mode === 'replay' && !a.replay) {
    console.error('--mode=replay requires --replay=FILE');
    process.exit(2);
  }

  const c = new HeadlessClient({
    server: a.server || DEFAULTS.server,
    port: a.port ? parseInt(a.port, 10) : DEFAULTS.port,
    mode: a.mode || DEFAULTS.mode,
    replay: a.replay || null,
    page: a.page || DEFAULTS.page,
    stage: a.stage || DEFAULTS.stage,
    pollMs: a.poll ? parseInt(a.poll, 10) : DEFAULTS.pollMs,
    localAddress: a.local_address || DEFAULTS.localAddress,
    verbose: !a.quiet,
  });

  const shutdown = () => {
    c.log('stopping; summary', JSON.stringify(c.summary()));
    c.stop();
    process.exit(0);
  };
  process.on('SIGINT', shutdown);
  process.on('SIGTERM', shutdown);

  await c.start();
}

if (require.main === module) {
  main().catch((e) => { console.error('fatal:', (e && e.stack) || e); process.exit(1); });
}

module.exports = { HeadlessClient, planCommand, allowedFor, parseArgs };
