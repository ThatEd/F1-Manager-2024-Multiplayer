'use strict';
/*
 * export_loader.js - boots the game's own RequireJS app tree headlessly.
 *
 * The export is a RequireJS 2.3.6 tree (`js/config/RequireConfig.js` sets `baseUrl: 'js/'`). Modules
 * are AMD: `define(["require","exports",...deps], function(require,exports,...){...})`.
 *
 * Rather than fake that with a hand-written `define`, the game's own `require.js` runs inside a `vm`
 * context whose globals are the DOM host. That way the loader semantics - relative ids, `exports`,
 * circular handling, the `_1` suffix the TypeScript emit uses - are the game's, not mine. Anything
 * that loads under this loader loads under the game.
 *
 * Three engine facts drive the design. All three are read out of the export, not guessed.
 *
 *  1. cohtml.js:132-143 - `engine.SendMessage` resolves a pending `engine.call` ONLY when
 *     `engine._mocks[name]` exists. With no native engine there is no mock, so the deferred entry is
 *     deleted and the promise never settles. The native side instead calls `engine._Reject(id)`
 *     (cohtml.js:235-241, message "No handler registered"). That is reproduced below, because "no
 *     handler registered" is precisely the state a headless host is in.
 *
 *  2. Engine.js:121-132 - once `IsRunningInEngine` rejects, `_isCoherentPlayer = true` and
 *     `_isReady = true`. That is the *Coherent preview player* path, and the game ships it complete:
 *     DataStore -> JSImpl (DataStore.js:207), System -> SystemJSImpl (System.js:148), Localisation ->
 *     LocalisationProviderJSImpl (Localisation.js:9), and every app gate `if (Engine.isCoherentPlayer())`
 *     runs its `generateFromSchemaFragment` block (FrontendApp.js:15, RaceWeekendApp.js:37).
 *     So this is not an invented UI mode; it is the mode Frontier already debugged.
 *
 *  3. cohtml.js:276-283 - `hasOnLoad` is taken from the module call form. In a `vm` there is no
 *     CommonJS `module`, so it takes the browser branch and waits for DOMContentLoaded, which our
 *     host never fires. Driving `_OnContentLoaded` is therefore required: it is what triggers
 *     'Ready' (cohtml.js:270-275), which resolves `engine.whenReady` (cohtml.js:196-198).
 *
 * Network is disabled: every `require` is answered from disk, so a module cannot stall on a fetch.
 */

const fs = require('fs');
const path = require('path');
const vm = require('vm');
const { createHost } = require('./dom.js');

/*
 * Where the decompiled UI lives.
 *
 * Configurable because a hard-coded path makes the harness unusable by anyone else - and the whole point of
 * publishing this bundle is that the next person can run the tests. `F1M_UI_EXPORT` points at the directory
 * containing `UIGameface/` (i.e. `.../F1Manager24/Content`); `F1M_EXPORT` is accepted as a synonym.
 *
 * The default is this machine's FModel output so the owner's own runs need no setup. Anyone else sets one
 * variable, or the suites that need the export skip themselves with a named reason (see run_all_tests.py).
 */
const CONTENT_ROOT =
  process.env.F1M_UI_EXPORT ||
  process.env.F1M_EXPORT ||
  String.raw`E:\FModel\Output\Exports\F1Manager24\Content`;
const UI_ROOT = path.join(CONTENT_ROOT, 'UIGameface');
const JS = path.join(UI_ROOT, 'js');
const EXPORT_LOC_ROOT = path.join(CONTENT_ROOT, 'Localization', 'Volta');

/** True when the export is actually present, so a caller can skip instead of throwing. */
function exportAvailable() {
  try {
    return fs.existsSync(path.join(JS, 'config', 'RequireConfig.js'));
  } catch (e) {
    return false;
  }
}

/*
 * Map a pak-relative localisation request onto the FModel export.
 *
 * At runtime LocalisationProviderJSImpl.js:16-29 asks for `./localisation/<pack>/<language>/<pack>.archive`
 * and reads it as UTF-16LE JSON with a `{Children:[{Key,Translation:{Text}}]}` shape
 * (parseJSONLocFile, :122-138). The export does not contain that archive: FModel wrote the same data
 * as Content/Localization/Volta/<language>/Volta.json, which is UTF-8 with a flat
 * `{namespace: {KEY: "text"}}` shape and 88,079 entries.
 *
 * Converting to the archive shape means the game's own parser consumes it and nothing about the lookup
 * path is special-cased. Getting the counting wrong here is what made an earlier version silently
 * report no localisation: `parseJSONLocFile` steps two code units at a time from index 2, so a UTF-16LE
 * string must start with an even number of leading units before the `{`.
 */
const locCache = new Map();

function buildLocArchive(lang) {
  if (locCache.has(lang)) return locCache.get(lang);
  const file = path.join(EXPORT_LOC_ROOT, lang, 'Volta.json');
  let out = null;
  try {
    if (fs.existsSync(file)) {
      const flat = JSON.parse(fs.readFileSync(file, 'utf8'));
      const children = [];
      for (const ns of Object.keys(flat)) {
        const entries = flat[ns];
        if (!entries || typeof entries !== 'object') continue;
        for (const key of Object.keys(entries)) {
          children.push({ Key: key, Translation: { Text: entries[key] } });
        }
      }
      const json = JSON.stringify({ Children: children });
      // UTF-16LE with a two-unit lead so `charCodeAt(2) === '{'` (0x7B), which is what
      // parseJSONLocFile assumes. Built with Buffer, not a character loop: the payload is 7.8M chars
      // and a `for` loop concatenating onto a string took 2.8 s per boot - measurably the difference
      // between a 6 s and a 65 s command round trip. Buffer.from(json,'utf16le') is the same bytes in
      // 22 ms.
      out = '\uFEFFx' + Buffer.from(json, 'utf16le').toString('utf16le');
    }
  } catch (_) { out = null; }
  locCache.set(lang, out);
  return out;
}

function resolveExportAsset(rel) {
  // Only build the (7.8 MB) archive when a loader actually asks for a localisation file. Suites that
  // never touch localisation - the protocol unit tests, most single-screen renders - then pay nothing.
  if (!/[\\/]localisation[\\/]/i.test(rel)) return null;
  const m = /localisation\/?\/?([^/]+)\/([^/]+)\//i.exec(rel);
  if (m) {
    const lang = m[2] === 'en' ? 'en' : m[2];
    return buildLocArchive(lang);
  }
  return null;
}

function load(entryModule, opts = {}) {
  // The export's real stylesheets drive two things: `:root` custom properties (read by graphs and
  // layout helpers through getComputedStyle) and the class-coverage check. Loading the index is
  // optional so the loader still works if the CSS was not exported.
  let cssIndex = null;
  if (opts.css !== false) {
    try { cssIndex = require('./css_index.js'); } catch (_) { cssIndex = null; }
  }
  const host = createHost(cssIndex ? { rootVars: cssIndex.rootVarsObject() } : {});
  const loaded = new Map();       // id -> module namespace
  const loadErrors = [];
  const warnings = [];
  const missingStylesheets = new Set();

  // CSSUtil.loadCSS (common/util/CSSUtil.js:6-32) appends `<link rel=stylesheet href=css/<name>.css>`
  // and resolves on `link.onload`. Two things to reproduce faithfully:
  //   - the sheet's real text must be attached, because the sweep reports class coverage against it;
  //   - a sheet that is NOT in the export must fail rather than "load". Some are absent
  //     (css/project/TeamManagement.css, css/project/RaceWeekend.css, many per-component files), and
  //     firing onload for a 404 is how a harness talks itself into believing the layout loaded.
  const headAppend = host.document.head.appendChild.bind(host.document.head);
  host.document.head.appendChild = (child) => {
    const r = headAppend(child);
    if (child && String(child.nodeName).toLowerCase() === 'link') {
      // CSSUtil.loadCSS assigns `link.href = url` as a property (:24), not setAttribute, so read the
      // property first. Reading only the attribute returned '' and skipped the onload below, which
      // made `await loadCSS('project/Shared')` hang and the app mount nothing.
      const href = String(child.href || child.getAttribute('href') || '');
      const text = cssIndex && href ? cssIndex.readStylesheet(href) : null;
      if (href) {
        if (text == null) missingStylesheets.add(href);
        else child.sheetText = text;
      }
      setTimeout(() => {
        try {
          if (typeof child.onload === 'function') child.onload();
        } catch (_) {}
      }, 0);
    }
    return r;
  };

  const ctx = vm.createContext({
    window: host.window,
    document: host.document,
    navigator: host.window.navigator,
    location: host.window.location,
    performance: host.window.performance,
    console: opts.quiet ? { log() {}, warn() {}, error() {}, info() {}, debug() {} } : console,
    setTimeout: host.window.setTimeout,
    clearTimeout: host.window.clearTimeout,
    setInterval: host.window.setInterval,
    clearInterval: host.window.clearInterval,
    requestAnimationFrame: host.window.requestAnimationFrame,
    cancelAnimationFrame: host.window.cancelAnimationFrame,
    URL,
    URLSearchParams,
    JSON,
    Math,
    Date,
    Promise,
    Error,
    Object,
    Array,
    String,
    Number,
    Boolean,
    RegExp,
    Symbol,
    Map,
    Set,
    WeakMap,
    Float32Array,
    Uint8Array,
    Int32Array,
  });
  ctx.globalThis = ctx;
  ctx.self = ctx.window;

  // common/components/PortalContainer.js:13 constructs a MutationObserver to re-home portal children.
  // With no real mutation engine the honest headless behaviour is to never fire: the container still
  // renders and its children still mount under it, they just are not relocated on DOM changes.
  if (typeof ctx.MutationObserver === 'undefined') {
    ctx.MutationObserver = class MutationObserver {
      constructor(cb) { this._cb = cb; this._observed = []; }
      observe(target, opts) { this._observed.push({ target, opts }); }
      disconnect() { this._observed.length = 0; }
      takeRecords() { return []; }
    };
  }

  // The export calls some window methods as bare globals rather than through `window.`
  // (e.g. project/modules/fanfareModule/FanfareDialogsModule.js:21 calls `addEventListener(...)`).
  // In a browser those resolve on the global object; in this context that means the sandbox itself.
  for (const name of [
    'addEventListener', 'removeEventListener', 'dispatchEvent',
    'requestAnimationFrame', 'cancelAnimationFrame',
    'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval',
    'getComputedStyle', 'matchMedia',
  ]) {
    if (ctx[name] === undefined && typeof host.window[name] === 'function') {
      ctx[name] = host.window[name].bind(host.window);
    }
  }

  // XMLHttpRequest is used by project/debug/LocalisationProviderJSImpl.js:17 (loc archives) and
  // project/modules/editors/EditorCommon.js:17 (textures). Both treat an empty body as "file
  // missing" and carry on, so answering from disk keeps them honest without inventing content.
  ctx.XMLHttpRequest = class HeadlessXHR {
    constructor() {
      this.readyState = 0; this.status = 0; this.responseText = ''; this.response = '';
      this.responseType = '';
      this._url = ''; this._headers = {};
      this.onreadystatechange = null; this.onload = null; this.onerror = null;
    }
    open(_method, url) { this._url = String(url); this.readyState = 1; }
    setRequestHeader(k, v) { this._headers[String(k).toLowerCase()] = v; }
    getAllResponseHeaders() { return ''; }
    // LocalisationProviderJSImpl.js:24 sets a utf-16 content type before reading a loc archive and
    // calls overrideMimeType. A throw here is not local: it escapes the app's `await` chain
    // (TeamManagementApp.js) and preact.render is never reached, which reads as "nothing rendered".
    overrideMimeType(_m) { return undefined; }
    abort() {}
    send() {
      let text = '';
      try {
        const rel = decodeURIComponent(String(this._url))
          .replace(/^file:\/\/\/?/i, '')
          .replace(/^\.?\//, '')
          .split(/[?#]/)[0];
        const file = path.resolve(UI_ROOT, rel);
        if (file.startsWith(UI_ROOT) && fs.existsSync(file) && fs.statSync(file).isFile()) {
          text = fs.readFileSync(file, 'utf8');
        } else {
          const alt = resolveExportAsset(rel);
          if (alt) text = alt;
        }
      } catch (_) { /* leave empty: treated as a missing file */ }
      this.responseText = text;
      this.response = text;
      this.status = text ? 200 : 404;
      this.readyState = 4;
      try { if (typeof this.onreadystatechange === 'function') this.onreadystatechange(); } catch (e) { warnings.push('xhr onreadystatechange: ' + e.message); }
      try { if (typeof this.onload === 'function') this.onload(); } catch (e) { warnings.push('xhr onload: ' + e.message); }
    }
  };

  // cohtml.js first: it is Coherent's runtime shim and defines the global `engine` that Engine.js
  // touches at load time (`engine.whenReady.then(...)`). With no native engine attached the shim
  // builds the working stub the game uses in a browser preview.
  vm.runInContext(fs.readFileSync(path.join(JS, 'common', 'lib', 'cohtml.js'), 'utf8'), ctx, { filename: 'cohtml.js' });
  if (typeof ctx.engine !== 'object' || !ctx.engine) {
    loadErrors.push('cohtml.js did not define the global engine shim');
  }

  // Reproduce "no handler registered". Without this every `engine.call` awaits forever.
  vm.runInContext(`
    (function () {
      if (!engine || engine.__coopPatched) return;
      engine.__coopPatched = true;
      var original = engine.SendMessage;
      engine.SendMessage = function (name, id) {
        if (engine._mocks[name] === undefined) { engine._Reject(id); return; }
        return original.apply(engine, arguments);
      };
    })();
  `, ctx);

  // The game's loader, verbatim.
  vm.runInContext(fs.readFileSync(path.join(JS, 'common', 'lib', 'require.js'), 'utf8'), ctx, { filename: 'require.js' });

  // Configure it the way RequireConfig.js does, and answer module reads from disk.
  vm.runInContext('var require = { baseUrl: "js/", paths: {} };', ctx);
  vm.runInContext(`
    requirejs.config({ baseUrl: 'js/', paths: {}, nodeRequire: undefined });
    requirejs.onError = function (err) { (window.__requireError || (window.__requireError = [])).push(String(err && err.message || err)); };
  `, ctx);

  ctx.window.__readModule = (id) => {
    const clean = String(id).replace(/^js\//, '').replace(/\.js$/, '');
    const file = path.join(JS, clean + '.js');
    if (!fs.existsSync(file)) {
      loadErrors.push('missing module: ' + clean);
      return null;
    }
    return fs.readFileSync(file, 'utf8');
  };

  // A synchronous AMD shim layered on top of requirejs for our own use: modules are needed now, not
  // after an async callback. requirejs's own define is still what parses each file.
  const defineFn = ctx.define;
  const registry = new Map();
  // `define` is called as define(deps, factory) or define(name, deps, factory). The two-argument form
  // is the common one here; getting the arity wrong silently yields `factory === undefined` for every
  // module, which then fails with "Cannot read properties of undefined (reading 'apply')".
  ctx.define = function (a, b, c) {
    let name = null;
    let deps;
    let factory;
    if (typeof a === 'string') { name = a; deps = b; factory = c; }
    else { deps = a; factory = b; }
    if (!Array.isArray(deps) || typeof factory !== 'function') return;
    const rec = { deps, factory };
    if (name) registry.set(name, rec);
    else {
      if (!registry.has('__anon__')) registry.set('__anon__', []);
      registry.get('__anon__').push(rec);
    }
  };
  ctx.define.amd = defineFn && defineFn.amd;

  function resolveId(from, dep) {
    if (!dep.startsWith('.')) return dep;
    const base = from.split('/').slice(0, -1);
    for (const seg of dep.split('/')) {
      if (seg === '.' || seg === '') continue;
      if (seg === '..') base.pop();
      else base.push(seg);
    }
    return base.join('/');
  }

  function instantiate(id, stack) {
    if (loaded.has(id)) return loaded.get(id);
    if (stack.includes(id)) {
      // Circular: hand back a partial namespace, as AMD does.
      const partial = {};
      loaded.set(id, partial);
      return partial;
    }
    const src = ctx.window.__readModule(id);
    if (src == null) {
      const empty = {};
      loaded.set(id, empty);
      return empty;
    }
    const exportsObj = {};
    loaded.set(id, exportsObj);
    registry.clear();
    try {
      vm.runInContext(`(function(){${src}\n})()`, ctx, { filename: id + '.js' });
    } catch (e) {
      // A module throwing at load time is a real finding; record it and keep what was exported.
      loadErrors.push(`${id}: ${e && e.message}`);
      return exportsObj;
    }
    let rec = registry.get(id);
    if (!rec) {
      const anon = registry.get('__anon__');
      rec = anon && anon[0];
    }
    if (!rec) return exportsObj;

    if (process.env.COOP_DEBUG_LOAD === '1') {
      console.error(`[load] ${id} deps=${JSON.stringify(rec.deps)} factory=${typeof rec.factory}`);
    }

    // The AMD loader gives each factory a `require` supporting both `require('id')` and
    // `require(['id'], resolve, reject)`. 24 modules use the array form to lazily pull debug-only
    // code (DataStore.js:207, Localisation.js:9, FrontendApp.js:16, ...). Without it they never
    // initialise and the corresponding system never reports complete.
    const reqFn = (deps, cb, errCb) => {
      if (Array.isArray(deps)) {
        try {
          const mods = deps.map((d) => instantiate(resolveId(id, d), stack.concat(id)));
          if (typeof cb === 'function') cb(...mods);
        } catch (e) {
          if (typeof errCb === 'function') errCb(e);
          else loadErrors.push(`${id} (async require): ${e && e.message}`);
        }
        return;
      }
      return instantiate(resolveId(id, deps), stack.concat(id));
    };

    const args = rec.deps.map((d) => {
      if (d === 'require') return reqFn;
      if (d === 'exports') return exportsObj;
      return instantiate(resolveId(id, d), stack.concat(id));
    });
    try {
      const ret = rec.factory.apply(exportsObj, args);
      if (ret !== undefined) {
        loaded.set(id, ret);
        return ret;
      }
    } catch (e) {
      loadErrors.push(`${id} (factory): ${e && e.message}`);
    }
    return exportsObj;
  }

  const ns = instantiate(entryModule.replace(/^js\//, '').replace(/\.js$/, ''), []);

  // Trigger the content-loaded half of the handshake (see header note 3).
  let engineReady = false;
  try {
    vm.runInContext(`(function(){
      try {
        if (engine && engine._BindingsReady === false && typeof engine.BindingsReady === 'function') {
          engine.BindingsReady(2, 0, 3, 0);
        }
        if (engine && engine._ContentLoaded === false && typeof engine._OnContentLoaded === 'function') {
          engine._OnContentLoaded();
        }
      } catch (e) {}
      return engine && engine._ContentLoaded === true;
    })()`, ctx);
    engineReady = true;
  } catch (_) { /* left false; the caller reports it */ }

  const tick = (ms) => new Promise((r) => setTimeout(r, ms));

  /**
   * Wait for a predicate, in real time. The boot chain is timers, not microtasks:
   *   'Ready' -> IsRunningInEngine rejects on a 16 ms rAF (cohtml.js:239)
   *           -> Player resolves on a 6 ms timeout (Player.js:14)
   *           -> CSS link onload
   *           -> dummy data -> preact.render
   * so a microtask flush is provably not enough and this waits on the clock.
   */
  async function waitFor(pred, { maxMs = 5000, stepMs = 20 } = {}) {
    const start = Date.now();
    for (;;) {
      let ok = false;
      try { ok = !!pred(); } catch (_) { ok = false; }
      if (ok) return { ok: true, ms: Date.now() - start };
      if (Date.now() - start >= maxMs) return { ok: false, ms: Date.now() - start };
      await tick(stepMs);
    }
  }

  /** Resolve every system, then stop. Reports the engine state instead of asserting it. */
  function engineState() {
    try {
      const eng = instantiate('common/core/Engine', []);
      let coherent = 'throws';
      try { coherent = eng.isCoherentPlayer(); } catch (_) { coherent = 'throws'; }
      return { isCoherentPlayer: coherent, hasSendEvent: typeof eng.sendEvent === 'function' };
    } catch (e) {
      return { isCoherentPlayer: 'unavailable', error: e && e.message };
    }
  }

  return {
    engineReady,
    ns,
    ctx,
    host,
    window: host.window,
    document: host.document,
    instantiate: (id) => instantiate(id, []),
    tick,
    waitFor,
    engineState,
    errors: loadErrors,
    warnings,
    // Which stylesheets a view asked for but the export does not contain. Reported rather than
    // silently treated as loaded - see the head.appendChild comment above.
    missingStylesheets: () => [...missingStylesheets],
    css: cssIndex ? { classCount: () => cssIndex.build().classes.size, index: cssIndex } : null,
    uiRoot: UI_ROOT,
    dispose: () => host.dispose(),
  };
}

/** Read a module namespace by AMD id, e.g. 'project/modules/frontend/components/MainMenu'. */
function readModule(l, id) {
  return l.instantiate(String(id).replace(/^js\//, '').replace(/\.js$/, ''));
}

module.exports = { load, readModule, JS, UI_ROOT, CONTENT_ROOT, exportAvailable };
