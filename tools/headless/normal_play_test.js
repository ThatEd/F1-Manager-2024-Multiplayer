'use strict';
/*
 * normal_play_test.js - prove the mod is invisible when no co-op session is running.
 *
 * The requirement is specific: the same pak is installed on both machines, so it must behave exactly like
 * a vanilla install for a player who never touches multiplayer. This suite runs the shipped coop scripts
 * inside a fake browser with NO command server reachable and counts what they actually do:
 *
 *   - how many HTTP attempts they make over simulated time
 *   - whether those attempts ever stop
 *   - whether they touch the DOM
 *   - whether they throw
 *   - whether they can be re-armed (so "stopped" never means "broken")
 *
 * The scripts are evaluated as real files, not re-implementations, so a regression in the pak fails
 * here. A stubbed XMLHttpRequest records every attempt instead of making it, which is what makes the
 * measurement exact and offline.
 *
 * Time is simulated: `setTimeout`/`setInterval` are queued and drained in order, so "10 minutes of idle
 * play" costs milliseconds and gives an exact attempt count.
 */

const fs = require('fs');
const path = require('path');
const vm = require('vm');

const COOP = path.join(__dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name, detail });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}
function note(msg) { console.log(`  ..    ${msg}`); }

/**
 * A fake browser with a virtual clock.
 *
 * `advance(ms)` drains timers in deadline order, so ordering matches reality closely enough for counting
 * attempts. XHR always fails, which is the state a player's machine is in when nobody is hosting.
 */
function makeEnv(opts = {}) {
  const timers = [];
  let seq = 0;
  const clock = { now: 0 };
  const xhrLog = [];
  const domMutations = [];
  const logs = [];
  const errors = [];

  function schedule(fn, ms, repeat) {
    seq += 1;
    timers.push({ id: seq, at: clock.now + Math.max(0, ms | 0), fn, repeat: !!repeat, every: Math.max(1, ms | 0) });
    return seq;
  }

  const sandbox = {
    console: {
      log: (...a) => { logs.push(a.join(' ')); },
      warn: (...a) => { logs.push(a.join(' ')); },
      error: (...a) => { logs.push(a.join(' ')); },
    },
    setTimeout: (fn, ms) => schedule(fn, ms, false),
    setInterval: (fn, ms) => schedule(fn, ms, true),
    clearTimeout: (id) => { const i = timers.findIndex((t) => t.id === id); if (i >= 0) timers.splice(i, 1); },
    clearInterval: (id) => { const i = timers.findIndex((t) => t.id === id); if (i >= 0) timers.splice(i, 1); },
    Date: { now: () => clock.now },
    JSON, Math, String, Number, Boolean, Object, Array, Error, Promise, RegExp, Symbol, Map, Set,
  };

  // Every request fails, as it does with no server listening.
  sandbox.XMLHttpRequest = class FakeXHR {
    constructor() { this.readyState = 0; this.status = 0; this.responseText = ''; this.timeout = 0; }
    open(_m, url) { this._url = String(url); xhrLog.push({ at: clock.now, url: this._url }); }
    setRequestHeader() {}
    send() {
      // A real failed connect settles asynchronously; resolve on the next timer drain.
      if (opts.neverFails) {
        this.readyState = 4; this.status = 200; this.responseText = (opts.body || '');
        if (typeof this.onreadystatechange === 'function') this.onreadystatechange();
        return;
      }
      schedule(() => {
        this.readyState = 4;
        this.status = 0;
        if (typeof this.onerror === 'function') this.onerror();
        if (typeof this.onreadystatechange === 'function') this.onreadystatechange();
        if (typeof this.ontimeout === 'function') this.ontimeout();
      }, 1, false);
    }
    abort() {}
  };

  function el(tag) {
    return {
      tagName: String(tag).toUpperCase(),
      nodeType: 1,
      style: {}, attributes: {},
      childNodes: [], _className: '',
      get className() { return this._className; },
      set className(v) { this._className = String(v); },
      setAttribute(k, v) { this.attributes[k] = String(v); },
      getAttribute(k) { return this.attributes[k] === undefined ? null : this.attributes[k]; },
      appendChild(c) { this.childNodes.push(c); domMutations.push('appendChild(' + (c && c.tagName) + ')'); return c; },
      removeChild() {}, addEventListener() {}, removeEventListener() {},
      querySelector() { return null; }, querySelectorAll() { return []; },
      classList: { add() {}, remove() {}, contains() { return false; } },
      textContent: '',
    };
  }

  const doc = {
    body: el('body'),
    head: el('head'),
    documentElement: el('html'),
    readyState: 'complete',
    createElement: el,
    getElementById() { return null; },
    querySelector() { return null; },
    querySelectorAll() { return []; },
    addEventListener() {},
  };

  sandbox.window = sandbox;
  sandbox.self = sandbox;
  sandbox.globalThis = sandbox;
  sandbox.document = doc;
  sandbox.location = { pathname: '/Frontend.html', href: 'http://localhost/Frontend.html' };
  sandbox.navigator = { userAgent: 'normal-play-test' };
  sandbox.localStorage = { _m: new Map(), getItem(k) { return this._m.has(k) ? this._m.get(k) : null; }, setItem(k, v) { this._m.set(k, String(v)); }, removeItem(k) { this._m.delete(k); } };

  const ctx = vm.createContext(sandbox);

  /** Drain timers up to `ms` of virtual time. */
  function advance(ms) {
    const until = clock.now + ms;
    for (let guard = 0; guard < 200000; guard++) {
      timers.sort((a, b) => a.at - b.at || a.id - b.id);
      const t = timers[0];
      if (!t || t.at > until) break;
      timers.shift();
      clock.now = t.at;
      try { t.fn(); } catch (e) { errors.push((e && e.message) || String(e)); }
      if (t.repeat) { t.at = clock.now + t.every; timers.push(t); }
    }
    clock.now = until;
  }

  return { ctx, sandbox, doc, advance, xhrLog, domMutations, logs, errors, timers, opts };
}

function runFile(env, file) {
  const src = fs.readFileSync(path.join(COOP, file), 'utf8');
  vm.runInContext(src, env.ctx, { filename: file });
}

function attempts(env, pattern) {
  return env.xhrLog.filter((r) => (pattern ? pattern.test(r.url) : true)).length;
}

async function main() {
  console.log('normal play: is the mod invisible with no co-op session?\n');

  // ---- 1. bridge.js alone, no server -------------------------------------------------------
  {
    console.log('bridge.js, no command server reachable');
    const env = makeEnv();
    // A minimal engine so the bridge's engineReady() gate passes and it gets to polling.
    env.sandbox.engine = { trigger() {}, call() { return Promise.resolve(null); } };
    runFile(env, 'bridge.js');

    env.advance(10 * 60 * 1000); // ten virtual minutes
    const total = attempts(env);
    note(`HTTP attempts in 10 virtual minutes: ${total}`);
    note(`last attempt at t=${env.xhrLog.length ? env.xhrLog[env.xhrLog.length - 1].at : 0}ms`);

    ok(env.errors.length === 0, 'bridge threw nothing while offline', env.errors.slice(0, 2).join(' | '));
    ok(total > 0, 'bridge does try to find a host at all', `${total}`);
    // The point of the change: attempts must stop, not continue forever.
    const tailStart = 8 * 60 * 1000;
    const tail = env.xhrLog.filter((r) => r.at >= tailStart).length;
    ok(tail === 0, 'bridge stops polling once the host is clearly absent', `${tail} attempts in the last 2 minutes`);
    ok(total < 100, 'bridge does not hammer the loopback while idle', `${total} attempts in 10 minutes`);
    // The inbox poll was added later and must obey the same budget. Asserted by URL, because the
    // whole-bridge request count above would still pass if the inbox loop were the only thing left
    // running - which is exactly the shape of the regression that got through once.
    const inboxAttempts = env.xhrLog.filter((r) => /\/coop\/inbox/.test(r.url));
    ok(inboxAttempts.length === 0,
      'the inbox poll sends nothing at all while no host can be found', `${inboxAttempts.length} requests`);
    const clockAttempts = env.xhrLog.filter((r) => /\/coop\/clock/.test(r.url));
    ok(clockAttempts.length === 0,
      'the 400 ms clock poll sends nothing while no host can be found', `${clockAttempts.length} requests`);
    // A request counter cannot see a chain that reschedules without ever calling open() (HOST is null
    // offline, so the inbox never gets that far). What a player actually pays for is a timer left armed.
    //
    // Distinguishing signal: the inbox is the only loop with a long period (INBOX_IDLE, and it backs off
    // to 5x that). Everything that legitimately survives idle play is fast - watchScreens runs at 500 ms.
    // So a timer pending more than 2 s past the drained window can only be the inbox chain. Measured:
    // 0 with the give-up guard, 1 without it.
    const until = 10 * 60 * 1000;
    const longArmed = env.timers.filter((t) => t.at > until + 2000).length;
    ok(longArmed === 0, 'the inbox chain is not left armed after the give-up',
      `${longArmed} timers pending more than 2 s past the window (1 means the loop never gave up)`);

    /* The clock poll is the fastest loop in the mod (400 ms), so it is the one that would be most visible if
       it never gave up. Its own pending-chain timer is sub-2 s, so the assertion above cannot see it.

       The bound is 1, not 2, and it has to be exact: with the guard in place only `watchScreens` survives
       (500 ms, the game's own screen detection). With the clock guard removed the count is 2. A bound of 2
       passes in both cases, which is how this gap was found - the assertion looked right and proved nothing.
       Measured both ways before choosing the number. */
    const armed = env.timers.length;
    ok(armed <= 1, 'no polling chain survives the give-up', `${armed} timers still armed (2 = the clock loop never gave up)`);
    ok(env.domMutations.length === 0, 'bridge does not touch the DOM', String(env.domMutations.length));

    // ---- re-arm: stopping must not break co-op ------------------------------------------
    ok(typeof env.sandbox.__coopRearm === 'function', 'bridge exposes a re-arm hook');
    const before = attempts(env);
    const rearmed = env.sandbox.__coopRearm();
    env.advance(30000);
    const after = attempts(env);
    ok(rearmed === true, 're-arm returns success');
    ok(after > before, 're-arm makes the bridge probe again', `${after - before} new attempts`);
  }
  console.log();

  // ---- 2. lobby.js alone, no server -------------------------------------------------------
  {
    console.log('lobby.js, no command server reachable');
    const env = makeEnv();
    // lobby.js needs a preact-ish context only once modules load; loadModules is async and will fail
    // offline, which is exactly the path being tested.
    runFile(env, 'lobby.js');
    env.advance(10 * 60 * 1000);

    const rev = attempts(env, /lobby\.rev/);
    note(`lobby.rev polls in 10 virtual minutes: ${rev}`);
    ok(env.errors.length === 0, 'lobby threw nothing while offline', env.errors.slice(0, 2).join(' | '));
    const tailStart = 8 * 60 * 1000;
    const tail = env.xhrLog.filter((r) => r.at >= tailStart && /lobby\.rev/.test(r.url)).length;
    ok(tail === 0, 'lobby stops live-reload polling once offline is established', `${tail} in the last 2 minutes`);
    ok(rev < 60, 'lobby does not poll every 3 s forever', `${rev} in 10 minutes (was 200)`);
    // A fallback box is expected if modules failed, but nothing should be added repeatedly.
    const appended = env.domMutations.filter((m) => /DIV/.test(m)).length;
    ok(appended <= 1, 'lobby adds at most one fallback element', `${appended}`);
  }
  console.log();

  // ---- 3. with a server present, polling stays fast ---------------------------------------
  {
    console.log('bridge.js with a reachable server, idle (empty responses)');
    // `neverFails` with an EMPTY body: the server is up but has no command for us, which is the normal
    // state of an idle session. The bridge then uses POLL_IDLE (250 ms). A non-empty body would make it
    // use POLL_BUSY (120 ms), which is correct but is the "work pending" case, not this one.
    const env = makeEnv({ neverFails: true, body: '' });
    env.sandbox.engine = { trigger() {}, call() { return Promise.resolve(null); } };
    runFile(env, 'bridge.js');
    env.advance(60000);
    const total = attempts(env, /\/hello/);
    note(`/hello requests in 60 virtual seconds while healthy and idle: ${total}`);
    // 250 ms idle cadence -> about 240/min. Generous bounds: lively, but not a busy loop.
    ok(total >= 100, 'healthy polling stays responsive', `${total}/min`);
    ok(total <= 320, 'healthy idle polling is not run away', `${total}/min`);
  }
  console.log();

  // ---- 3b. busy cadence is intentional and bounded ----------------------------------------
  {
    console.log('bridge.js with a command waiting on every poll');
    const env = makeEnv({ neverFails: true, body: 'Stage idle' });
    env.sandbox.engine = { trigger() {}, call() { return Promise.resolve(null); } };
    runFile(env, 'bridge.js');
    env.advance(30000);
    const total = attempts(env, /\/hello/);
    note(`/hello requests in 30 virtual seconds with work pending: ${total}`);
    ok(total >= 100, 'busy polling is faster than idle', `${total}/30s`);
    ok(total <= 300, 'busy polling is bounded by POLL_BUSY', `${total}/30s`);
  }
  console.log();

  // ---- 4. no timers left behind ------------------------------------------------------------
  {
    console.log('after co-op give-up');
    const env = makeEnv();
    env.sandbox.engine = { trigger() {}, call() { return Promise.resolve(null); } };
    runFile(env, 'bridge.js');
    env.advance(10 * 60 * 1000);
    // Repeating timers are what burn CPU forever. Non-repeating ones that have been drained are gone.
    const repeating = env.timers.filter((t) => t.repeat).length;
    ok(repeating === 0, 'no setInterval survives into idle play', `${repeating} repeating timers`);
  }

  const fail = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - fail} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
}

main().catch((e) => { console.error('normal play test crashed:', (e && e.stack) || e); process.exit(2); });
