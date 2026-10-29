'use strict';
/*
 * render_one.js - render exactly one Gameface screen, in its own process.
 *
 * Why one process per screen. The first version of this harness booted the app once and navigated
 * through all 86 TeamManagement screens in a single process. That is closer to what a player does,
 * but it is measurably wrong for this purpose:
 *
 *   - Preact's rerender queue is module-local (preact.js:139-150) and the non-preview branch
 *     *rethrows* after logging (preact.js:159-174). A component that throws is popped, but everything
 *     queued behind it is stranded, so one bad view makes the next 80 report "render() never ran".
 *   - Every module, enum and DataStore context survives navigation, and 86 mounted screens of
 *     accumulated listeners, rAF loops and contexts ran the process out of memory (observed:
 *     "Ineffective mark-compacts near heap limit" after ~30 screens).
 *
 * A fresh process per screen makes every result independent, and makes a crash attributable. The cost
 * is wall-clock time: the app boot takes ~0.5-7 s because the dummy-data generators build the whole
 * world. `run_ui_sweep.js` parallelises that across CPU cores.
 *
 * Usage (normally driven by run_ui_sweep.js):
 *   node render_one.js --app=frontend --screen=MainMenu
 *   node render_one.js --app=frontend --screen=MainMenu --json
 */

const { load, readModule } = require('./export_loader.js');

const APPS = {
  frontend: {
    entry: 'project/apps/FrontendApp',
    event: 'SetFrontendScreenID',
    byName: true,
    enumModule: 'project/data/FrontendScreens',
    registryModule: 'project/data/FrontendData',
    navModule: 'project/utils/FrontendNavigation',
  },
  teamManagement: {
    entry: 'project/apps/TeamManagementApp',
    event: 'SetManagementScreenID',
    byName: true,
    enumModule: 'project/data/TeamScreens',
    registryModule: 'project/data/TeamManagementData',
    navModule: 'project/utils/TeamManagementNavigation',
  },
  raceWeekend: {
    entry: 'project/apps/RaceWeekendApp',
    event: 'SetRaceView',
    byName: false,
    enumModule: 'project/data/RaceWeekendScreens',
    registryModule: 'project/modules/raceWeekend/utils/RaceWeekendViewData',
    navModule: 'project/utils/RaceWeekendNavigation',
  },
};

function classesOf(root) {
  const s = new Set();
  (function walk(n) {
    if (!n) return;
    if (n.classList) for (const c of n.classList._set) s.add(c);
    for (const c of n.childNodes || []) walk(c);
  })(root);
  return s;
}

function countNodes(root) {
  let n = 0;
  (function walk(x) { if (!x) return; if (x.nodeType === 1) n++; for (const c of x.childNodes || []) walk(c); })(root);
  return n;
}

function textOf(root) {
  return String(root.textContent || '').replace(/\s+/g, ' ').trim();
}

/** Attribute an async failure to this screen instead of letting it kill the run. */
const crash = { error: null };
process.on('uncaughtException', (e) => { crash.error = crash.error || ((e && e.message) || String(e)); });
process.on('unhandledRejection', (e) => { crash.error = crash.error || ((e && e.message) || String(e)); });

/**
 * Count this screen's own render() calls, and capture a throw from inside render.
 *
 * Class names cannot answer "did this screen render?": the exported ScreenID and the component's root
 * class diverge - StartupBrightness renders StartupFlow_root, CreateATeamLogo renders EditTeamLogo_root,
 * ScenarioMode renders ScenarioScreen_root. Counting render() is exact, and the module resolves the
 * component out of the registry at render time (FrontendModule.js:46, TeamManagementModule.js:69,
 * RaceWeekendModule.js:42), so patching the prototype is seen by the real render path.
 *
 * Capturing the throw here matters because preact defers its flush (preact.js:6) and rethrows
 * (preact.js:167-173), so without this the only evidence is "render ran, produced nothing" - which
 * reads identically to a view that legitimately has no output. Recorded before rethrowing so the
 * counter and the error agree.
 */
function instrument(entry) {
  const counter = { renders: 0, throwAt: null, threw: null };
  if (!entry || typeof entry.component !== 'function') return null;
  const comp = entry.component;
  const wrap = (fn) => function (...args) {
    counter.renders++;
    counter.throwAt = counter.renders;   // which render call threw
    try {
      return fn.apply(this, args);
    } catch (e) {
      counter.threw = (e && e.message) || String(e);
      throw e;
    }
  };
  if (comp.prototype && typeof comp.prototype.render === 'function') {
    comp.prototype.render = wrap(comp.prototype.render);
  } else if (entry.__origComponent === undefined) {
    entry.__origComponent = comp;
    entry.component = wrap(comp);
  }
  return counter;
}

/**
 * Extra navigation props a screen requires, or null.
 *
 * Several screens read a context out of `props` that navigation supplies, and are only ever reached from
 * a caller that passes it. Reached bare, the prop is `undefined` and the view throws deep inside
 * `DataStoreJSImpl.contextToID` (`context.length` of undefined), which the sweep used to report as a
 * broken screen when it is really a harness gap.
 *
 * Two shapes, both read out of the export rather than guessed:
 *   - `{ context: [...] }`  — TeamManagement screens. `CalendarUtils.js:627-628` enters DriverDetails
 *     with `{ context: ['Staff', id], showTab }`; `ContractNegotiation.js:102` does the same for
 *     StaffDetails.
 *   - `{ context: [...] }`  — RaceWeekend warehouse screens. Their registry entry has no parent screen
 *     to derive a part from, so the context is built here from the generated part list.
 */
function navPropsFor(screenName, l, appKey) {
  const DS = (() => { try { return readModule(l, 'common/core/DataStore'); } catch (_) { return null; } })();

  if (screenName === 'DriverDetails' || screenName === 'StaffDetails') {
    let refID = 1;
    try {
      const teamID = DS && DS.getValue(['Teams'], 'playerTeamID');
      if (teamID != null) refID = teamID;
    } catch (_) { /* keep 1 */ }
    return { context: ['Staff', String(refID)] };
  }

  // Warehouse detail screens read `props.context` for a part name
  // (WarehouseAerodynamicPart.js:13 `DS.getValue(this.props.context, 'name')`).
  if (appKey === 'raceWeekend' && /^Warehouse/.test(screenName)) {
    // `['Parts','Types',<type>,'Items','0']` is the item context the Cars screens use for installed
    // parts; the generated data has type 0 with an item at id 0 (press.js seeds the same key).
    return { context: ['Parts', 'Types', '0', 'Items', '0'] };
  }

  return null;
}

async function main() {
  const argv = process.argv.slice(2);
  const opt = (name) => (argv.find((a) => a.startsWith(`--${name}=`)) || '').split('=').slice(1).join('=');
  const appKey = opt('app');
  const screenName = opt('screen');
  const asJson = argv.includes('--json');

  const app = APPS[appKey];
  if (!app) fail(`unknown app '${appKey}'`, asJson);
  if (!screenName) fail('missing --screen', asJson);

  const t0 = Date.now();
  const l = load(app.entry, { quiet: true });
  const eng = readModule(l, 'common/core/Engine');

  const coherent = await l.waitFor(() => {
    try { return eng.isCoherentPlayer() === true; } catch (_) { return false; }
  }, { maxMs: 8000 });
  if (!coherent.ok) fail(`engine never reached isCoherentPlayer() (${coherent.ms}ms)`, asJson);

  const mounted = await l.waitFor(() => l.document.body.childNodes.length > 0, { maxMs: 12000 });
  if (!mounted.ok) fail(`app mounted nothing (${mounted.ms}ms) errors=${l.errors.slice(0, 3).join(' | ')}`, asJson);
  // Let mount effects and the first render flush settle.
  await l.tick(300);

  // Resolve the screen target from the registry so the id and the enum name agree.
  const reg = readModule(l, app.registryModule);
  const enumNs = readModule(l, app.enumModule);
  const ScreenID = enumNs.ScreenID || {};
  const byValue = new Map();
  for (const [name, value] of Object.entries(ScreenID)) if (typeof value === 'number' && !byValue.has(value)) byValue.set(value, name);

  const entry = (reg.screens || []).find((e) => (byValue.get(e.id) || `#${e.id}`) === screenName);
  if (!entry) fail(`screen '${screenName}' is not in ${app.registryModule}`, asJson);

  const counter = instrument(entry);

  // Every nav module early-returns when the target is already current:
  //   FrontendNavigation.js:15-17, TeamManagementNavigation.js:32-35, RaceWeekendNavigation.js:12-13.
  // The app boots already showing the first registry entry, so without care the boot screen is never
  // measured as a transition - and even when it is, preact reuses the mounted instance and skips a
  // re-render with identical props (observed: MainMenu rendered 231 nodes but reported 0 render calls).
  //
  // Navigating to a *different* screen first forces the target to be a fresh mount, which is what a
  // player pressing a menu item actually gets. The pivot's own errors are discarded: it is plumbing.
  const nav = readModule(l, app.navModule);
  if (nav && typeof nav.setDefault === 'function') {
    try { nav.setDefault(-1); } catch (_) {}
  }
  const pivot = (reg.screens || []).find((e) => e.id !== entry.id);
  if (pivot) {
    try {
      eng.sendEvent(app.event, app.byName ? (byValue.get(pivot.id) || `#${pivot.id}`) : pivot.id);
      await l.tick(150);
    } catch (_) { /* a bad pivot must not stop the measurement */ }
    // The pivot may legitimately throw (many screens need DataStore contexts the export lacks).
    // Reset so the error recorded below belongs to the screen under test.
    crash.error = null;
    if (nav && typeof nav.setDefault === 'function') {
      try { nav.setDefault(-1); } catch (_) {}
    }
    // Let the pivot finish unwinding. Without this the target's dispatch can land while the pivot is
    // still settling, and the target's render is optimised away - observed on Staff (0 renders, 6
    // nodes) while an extra half second of settle made the same screen render 378 nodes.
    //
    // The bound is generous on purpose: under parallel load a 500 ms quiet window was not enough and
    // five screens reported "0 renders, 6 nodes" in a sweep while rendering normally on their own.
    let quiet = 0;
    let prev = -1;
    for (let i = 0; i < 120; i++) {
      const n = countNodes(l.document.body);
      if (n === prev) { if (++quiet >= 12) break; } else { quiet = 0; }
      prev = n;
      await l.tick(25);
    }
  }

  crash.error = null;
  const before = classesOf(l.document.body);
  const nodesBefore = countNodes(l.document.body);

  /*
   * Some screens are only ever entered with navigation extra-props, and reading them bare is a harness
   * fidelity gap rather than a broken view.
   *
   * `DriverDetails` and `StaffDetails` read `props.context` and hand it to `DS.getValue`. Entered with no
   * props, that is `undefined`, and the throw surfaces deep inside `DataStoreJSImpl.contextToID`
   * (`context.length` of undefined) - which the sweep was reporting as a screen failure. In the game
   * these screens are only reached from their caller, which passes the context:
   *   CalendarUtils.js:627-628  set(DriverDetails, { context: driverContext, showTab })
   *   ContractNegotiation.js:102 set(StaffDetails, { context: this.props.context })
   *
   * So when the app's navigation module exposes `set`, route through it with the props the real caller
   * would supply; otherwise fall back to the plain screen event.
   */
  const extraProps = navPropsFor(screenName, l, appKey);
  let dispatched = false;
  if (extraProps && nav && typeof nav.set === 'function' && typeof entry.id === 'number') {
    try { nav.set(entry.id, extraProps); dispatched = true; } catch (_) { dispatched = false; }
  }
  if (!dispatched) {
    try {
      eng.sendEvent(app.event, app.byName ? screenName : entry.id);
    } catch (e) {
      fail(`dispatch threw: ${e && e.message}`, asJson);
    }
  }

  // Wait for the render, don't sleep a fixed amount. Some screens mount in one flush; others run an
  // animation or a data walk first (Staff took ~600 ms, and measured 0 renders at a flat 300 ms).
  // Stop early once the counter has moved and the tree has stopped growing for a short quiet period.
  let lastNodes = -1;
  let stableFor = 0;
  for (let i = 0; i < 300; i++) {
    await l.tick(25);
    const n = countNodes(l.document.body);
    const rendered = counter ? counter.renders > 0 : n > nodesBefore;
    if (rendered && n === lastNodes) {
      if (++stableFor >= 16) break;         // ~400 ms with no growth
    } else {
      stableFor = 0;
    }
    lastNodes = n;
    if (crash.error) break;
  }

  const classes = classesOf(l.document.body);
  // Class coverage against the export's real stylesheets: what fraction of the classes this screen
  // emitted actually have a rule in the shipped CSS. This is the part that makes "rendered" mean more
  // than "a component ran" - a view emitting classes with no CSS is structurally rendered but visually
  // unstyled, and there is no way to tell those apart without the selector set.
  let cssStats = null;
  let locStats = null;
  try {
    const idx = require('./css_index.js');
    const cssIdx = idx.build();
    const used = [...classes];
    const styled = used.filter((c) => cssIdx.classes.has(c));
    cssStats = { emitted: used.length, styled: styled.length, unstyled: used.filter((c) => !cssIdx.classes.has(c)).slice(0, 12) };

    // Split raw [SYMBOL]s into "export has no such key" and "view did not translate it".
    const bodyText = String(l.document.body.textContent || '');
    const rawSymbols = new Set();
    for (const m of bodyText.matchAll(/\[([A-Z0-9_]+)\]/g)) rawSymbols.add(m[1]);
    if (rawSymbols.size) {
      const keys = idx.locKeys();
      const all = [...rawSymbols];
      locStats = {
        rawSymbols: all.length,
        notInExport: all.filter((s) => !keys.has(s)).slice(0, 10),
        inExportButUntranslated: all.filter((s) => keys.has(s)).slice(0, 10),
      };
    }
  } catch (_) { /* keep nulls: report stays usable without the index */ }

  const result = {
    app: appKey,
    screen: screenName,
    id: entry.id,
    isMenu: !!entry.isMenu,
    renders: counter ? counter.renders : -1,
    renderThrew: counter ? counter.threw : null,
    nodes: countNodes(l.document.body),
    classes: classes.size,
    addedClasses: [...classes].filter((c) => !before.has(c)),
    css: cssStats,
    loc: locStats,
    missingStylesheets: l.missingStylesheets ? l.missingStylesheets() : [],
    textLength: textOf(l.document.body).length,
    text: textOf(l.document.body).slice(0, 240),
    error: crash.error || (counter && counter.threw) || null,
    bootMs: Date.now() - t0,
    loadErrors: l.errors.slice(0, 5),
  };
  // `screenStarted=true` distinguishes "the sweep reached this screen and it failed" from
  // "the process died before reporting". run_ui_sweep.js needs that difference.
  result.screenStarted = true;
  // Success = this screen's own component ran render() and produced DOM. Note `error` is not part of
  // the test: a view can render correctly and still throw later (an animation callback, a listener on
  // a context that does not exist). That case is reported, not counted as a failed render - see the
  // `error` field, which stays populated.
  result.ok = result.renders > 0 && result.nodes > 6;
  l.dispose();

  if (asJson) process.stdout.write('##RESULT##' + JSON.stringify(result) + '\n');
  else console.log(JSON.stringify(result, null, 2));
  process.exit(result.ok ? 0 : 1);
}

function fail(reason, asJson) {
  const r = { ok: false, reason, screenStarted: false };
  if (asJson) process.stdout.write('##RESULT##' + JSON.stringify(r) + '\n');
  else console.error(reason);
  process.exit(1);
}

main().catch((e) => { fail('crashed: ' + ((e && e.message) || e), process.argv.includes('--json')); });
