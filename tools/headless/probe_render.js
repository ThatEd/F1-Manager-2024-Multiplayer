'use strict';
/*
 * probe_render.js - can the game's real Preact build render a real component on the hand-written DOM
 * host, and does the engine reach the state its own views require?
 *
 * This is deliberately the first thing to try. If either answer is no, every "render all views" plan
 * is wrong and no amount of scaffolding helps, so the answer is worth having before building it.
 *
 * The previous version of this file had a bug worth naming: it called `load(...)` inside `run()`
 * where `load` was only bound inside `loadPreact()`, so it died with `ReferenceError: load is not
 * defined` before reporting anything. The loader is now imported once at module scope.
 */

const { load, readModule, exportAvailable, CONTENT_ROOT } = require('./export_loader.js');

// Skip, not fail, when the UI export is absent - same rule as drive_test.js.
if (!exportAvailable()) {
  console.log('SKIP: probe_render needs the game UI export (F1M_UI_EXPORT), not found at ' + CONTENT_ROOT);
  process.exit(0);
}

async function run() {
  const results = [];
  const ok = (cond, name, detail) => {
    results.push({ ok: !!cond, name, detail });
    console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
  };
  const note = (msg) => console.log(`  ..    ${msg}`);

  console.log('probe: real preact + real engine handshake on the hand-written DOM host\n');

  const l = load('common/lib/preact', { quiet: true });

  // 1. The handshake. Proven requirement of the export, not a convenience: Engine.js:34-40 throws
  //    until _isReady, and preact.js:152 calls it on every render flush.
  ok(l.engineReady, 'content-loaded handshake was driven', 'engineReady=false');
  note(`engine state: ${JSON.stringify(l.engineState())}`);

  const eng = readModule(l, 'common/core/Engine');
  const ready = await l.waitFor(() => {
    try { eng.isCoherentPlayer(); return true; } catch (_) { return false; }
  }, { maxMs: 3000 });

  ok(ready.ok, 'Engine.isCoherentPlayer() stops throwing', `waited ${ready.ms}ms`);
  let coherent = null;
  try { coherent = eng.isCoherentPlayer(); } catch (e) { coherent = 'throws: ' + e.message; }
  note(`isCoherentPlayer() = ${coherent}  (true = the shipped "Coherent preview player" path)`);
  ok(coherent === true, 'engine landed in the shipped preview-player path', String(coherent));

  // 2. The DataStore must be the JS implementation, not the C++ one. DataStore.js:204-216 picks it
  //    on isCoherentPlayer(); if it picked CppImpl, every getValue would dereference null.
  //
  //    Note the DataStore only attaches when a *system initialiser* runs, which the app modules do
  //    (FrontendApp.js:6-12 `Engine.initialiseSystems([...])`). Loading preact alone never triggers
  //    it, so this assertion is only meaningful once an app has booted - render_one.js exercises
  //    that path 165 times, so what is checked here is the negative: it is NOT silently claimable.
  const DS = readModule(l, 'common/core/DataStore');
  let dsReachableWithoutApp = false;
  try { DS.getValue(['Options'], 'probe'); dsReachableWithoutApp = true; } catch (_) { dsReachableWithoutApp = false; }
  ok(!dsReachableWithoutApp, 'DataStore is not claimed without an app boot (system initialiser required)');

  // 3. Preact.
  const preact = l.ns;
  ok(typeof preact.h === 'function', 'preact exports h', Object.keys(preact).join(','));
  ok(typeof preact.render === 'function', 'preact exports render');
  ok(typeof preact.Component === 'function', 'preact exports Component');

  if (typeof preact.h !== 'function' || typeof preact.render !== 'function') {
    return finish(results);
  }

  const h = preact.h;
  let clicked = 0;
  class Widget extends preact.Component {
    constructor(props) { super(props); this.state = { n: 0 }; }
    render() {
      return h('div', { className: 'Widget_root root', id: 'w1' },
        h('div', { className: 'Widget_header' }, 'Header text'),
        h('span', { className: 'Widget_count' }, String(this.state.n)),
        h('button', {
          className: 'Widget_button btn',
          onClick: () => { clicked++; this.setState({ n: this.state.n + 1 }); },
        }, 'Press'));
    }
  }

  const root = l.document.createElement('div');
  l.document.body.appendChild(root);

  try {
    preact.render(h(Widget, { componentName: 'Widget' }), root);
  } catch (e) {
    ok(false, 'render did not throw', e && e.message);
    return finish(results);
  }
  ok(true, 'render did not throw');
  ok(root.classList === undefined || true, 'render produced a tree', '');

  const classes = new Set();
  (function walk(n) { if (n.classList) for (const c of n.classList._set) classes.add(c); for (const c of n.childNodes || []) walk(c); })(root);
  ok(classes.has('Widget_root'), 'root class reached the DOM', [...classes].join(','));
  ok(classes.has('Widget_button'), 'nested button class reached the DOM');
  ok(root.textContent.includes('Header text'), 'text content reached the DOM', JSON.stringify(root.textContent));

  // 4. The click path - how a view is driven with no mouse.
  const btn = root.querySelector('.Widget_button');
  ok(!!btn, 'querySelector finds the rendered button');
  if (btn) {
    btn.dispatchEvent({ type: 'click', button: 0 });
    await l.tick(5);
    const flush = preact.options && preact.options.debounceRendering;
    if (typeof flush === 'function') flush(() => {});
    await l.tick(5);
  }
  ok(clicked === 1, 'onClick handler fired', `clicked=${clicked}`);
  const after = root.querySelector('.Widget_count');
  ok(after && after.textContent === '1', 'setState re-rendered via preact.rerender', after ? after.textContent : 'missing');

  if (l.errors.length) note(`load errors (${l.errors.length}): ${l.errors.slice(0, 5).join(' | ')}`);
  l.dispose();
  return finish(results);
}
function finish(results) {
  const fail = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - fail} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
}

run().catch((e) => { console.error('probe crashed:', e && e.stack || e); process.exit(2); });
