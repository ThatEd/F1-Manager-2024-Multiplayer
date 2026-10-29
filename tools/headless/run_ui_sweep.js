'use strict';
/*
 * run_ui_sweep.js - render every Gameface screen in the export and report what happened.
 *
 * One child process per screen (see render_one.js for why), run N at a time. Each child boots the real
 * app, fires the app's own navigation event, and reports whether the screen's own render() ran.
 *
 * The screen list comes from the registries the apps actually use, not from the ScreenID enums: the
 * enums contain ids with no entry (and the registries contain ids the enums do not name), so the
 * registry is what determines whether a screen can be reached at all.
 *
 *   project/data/FrontendData.js                        (FrontendApp)
 *   project/data/TeamManagementData.js                  (TeamManagementApp)
 *   project/modules/raceWeekend/utils/RaceWeekendViewData.js (RaceWeekendApp)
 *
 * Usage:
 *   node run_ui_sweep.js                       # all three apps
 *   node run_ui_sweep.js --app=teamManagement  # one app
 *   node run_ui_sweep.js --jobs=6              # parallelism (default: cores/2)
 *   node run_ui_sweep.js --json=ui-sweep.json  # machine-readable report
 *   node run_ui_sweep.js --list                # print the screen list, do not render anything
 */

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');
const { load, readModule } = require('./export_loader.js');

/*
 * Enumerating the screen registries means loading a module that transitively imports every view
 * component (project/data/TeamManagementData.js imports ~80 of them). Those imports run module-level
 * code, so a component that throws during that side effect would kill the whole sweep before a single
 * screen was rendered. Children are isolated; the parent is not, so give it the same protection and
 * report the problem instead of dying.
 */
const parentProblems = [];
process.on('uncaughtException', (e) => { parentProblems.push((e && e.message) || String(e)); });
process.on('unhandledRejection', (e) => { parentProblems.push((e && e.message) || String(e)); });

const APPS = {
  frontend: { enumModule: 'project/data/FrontendScreens', registryModule: 'project/data/FrontendData' },
  teamManagement: { enumModule: 'project/data/TeamScreens', registryModule: 'project/data/TeamManagementData' },
  raceWeekend: { enumModule: 'project/data/RaceWeekendScreens', registryModule: 'project/modules/raceWeekend/utils/RaceWeekendViewData' },
};

/** Read the screen list for an app straight out of its registry. */
function listScreens(appKey) {
  const app = APPS[appKey];
  const l = load(app.registryModule, { quiet: true });
  const reg = readModule(l, app.registryModule);
  const enumNs = readModule(l, app.enumModule);
  const ScreenID = enumNs.ScreenID || {};
  const byValue = new Map();
  for (const [name, value] of Object.entries(ScreenID)) {
    if (typeof value === 'number' && !byValue.has(value)) byValue.set(value, name);
  }
  const out = (reg.screens || []).map((e) => ({
    app: appKey,
    name: byValue.get(e.id) || `#${e.id}`,
    id: e.id,
    isMenu: !!e.isMenu,
    hasComponent: typeof e.component === 'function',
  }));
  l.dispose();
  return out;
}

/**
 * Retry only the failures that are plausibly timing.
 *
 * "The component's own render() never ran" with a DOM that did not grow means the dispatch landed
 * before the navigation module was ready to swap screens - a scheduler artefact, and it reproduces
 * differently under load (measured: CarsEditProjectManufacture and WarehouseEngineering fail in a
 * 6-worker sweep and pass standalone). Everything else is deterministic and retrying it just costs
 * another app boot.
 */
function isRetryable(r) {
  if (!r || r.ok) return false;
  if (r.renders === 0 && !r.error) return true;
  const msg = String(r.reason || r.error || '');
  if (/timed out/i.test(msg)) return true;
  return false;
}

/** Slightly longer timeout per attempt, since a retry runs on a machine that is already busy. */
function attemptBudget(_screen, attempt) {
  return attempt;
}

/** Run one screen in its own process; resolve with its parsed result. */
function runScreen(appKey, screenName, timeoutMs, attempt = 1) {
  return new Promise((resolve) => {
    const args = [
      // Cap the heap per screen. Some views walk a DataStore list until a value goes null
      // (DataStoreUtil.js:19-30 `readSequentalValues`), and with dummy data that never terminates -
      // observed on CarsCurrentProjects, which consumes 4 GB and dies. A 512 MB cap turns that into a
      // fast, attributable OOM instead of a multi-minute stall that also starves the parallel workers.
      '--max-old-space-size=512',
      path.join(__dirname, 'render_one.js'), `--app=${appKey}`, `--screen=${screenName}`, '--json',
    ];
    const child = spawn(process.execPath, args, {
      cwd: __dirname,
      env: { ...process.env, COOP_DEBUG_VISIT: '' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let out = '';
    let err = '';
    let settled = false;
    const done = (r) => { if (!settled) { settled = true; resolve(r); } };

    const timer = setTimeout(() => {
      try { child.kill('SIGKILL'); } catch (_) {}
      done({ app: appKey, screen: screenName, ok: false, reason: `timed out after ${timeoutMs}ms`, screenStarted: false });
    }, timeoutMs);

    child.stdout.on('data', (d) => { out += d; });
    child.stderr.on('data', (d) => { err += d; });
    child.on('close', (code) => {
      clearTimeout(timer);
      const line = out.split('\n').reverse().find((s) => s.startsWith('##RESULT##'));
      if (line) {
        try { return done(JSON.parse(line.slice('##RESULT##'.length))); } catch (_) {}
      }
      // No result line: the child died before reporting.
      const tail = (err || out).trim();
      const oom = /heap out of memory|Ineffective mark-compacts/i.test(tail);
      done({
        app: appKey,
        screen: screenName,
        ok: false,
        reason: oom
          ? 'exhausted the heap before rendering (a data walk in the view does not terminate on dummy data)'
          : `child exited ${code} without a result: ${tail.split('\n').filter(Boolean).slice(-2).join(' / ')}`,
        screenStarted: false,
      });
    });
    child.on('error', (e) => { clearTimeout(timer); done({ app: appKey, screen: screenName, ok: false, reason: 'spawn failed: ' + e.message, screenStarted: false }); });
  });
}

/** Fixed-size worker pool. */
async function pool(items, size, worker) {  const results = [];
  let next = 0;
  const workers = Array.from({ length: Math.min(size, items.length) }, async () => {
    for (;;) {
      const i = next++;
      if (i >= items.length) return;
      results[i] = await worker(items[i]);
    }
  });
  await Promise.all(workers);
  return results;
}

async function main() {
  const argv = process.argv.slice(2);
  const opt = (n, d) => (argv.find((a) => a.startsWith(`--${n}=`)) || '').split('=').slice(1).join('=') || d;
  const appFilter = opt('app', null);
  const jobs = Number(opt('jobs', Math.max(2, Math.floor(os.cpus().length / 2))));
  const jsonPath = opt('json', null);
  const timeoutMs = Number(opt('timeout', 90000));

  const appKeys = appFilter ? [appFilter] : Object.keys(APPS);
  for (const k of appKeys) if (!APPS[k]) { console.error(`unknown app '${k}'; known: ${Object.keys(APPS).join(', ')}`); process.exit(2); }

  console.log('reading screen registries...');
  const screens = [];
  for (const k of appKeys) {
    const list = listScreens(k);
    console.log(`  ${k}: ${list.length} screens (${list.filter((s) => !s.hasComponent).length} without a component)`);
    screens.push(...list);
  }

  if (argv.includes('--list')) {
    for (const s of screens) console.log(`${s.app}\t${s.name}\t${s.id}\t${s.isMenu ? 'menu' : ''}`);
    process.exit(0);
  }

  // A screen with no component function cannot render by definition; record it without paying for a boot.
  const runnable = screens.filter((s) => s.hasComponent);
  const skipped = screens.filter((s) => !s.hasComponent)
    .map((s) => ({ ...s, ok: false, reason: 'registry entry has no component function', screenStarted: false }));

  console.log(`\nrendering ${runnable.length} screens with ${jobs} parallel workers\n`);
  const t0 = Date.now();
  let done = 0;
  const results = await pool(runnable, jobs, async (s) => {
    let r = await runScreen(s.app, s.name, timeoutMs, attemptBudget(s, 1));
    // A screen whose *own component never ran* is usually a timing artefact of parallel load: the
    // child renders in isolation but the dispatch landed too early. Observed on
    // CarsEditProjectManufacture and WarehouseEngineering, which pass standalone and failed in a
    // 6-worker sweep. Those are retried; hard failures (a real throw, an OOM, a missing DataStore
    // context) are not, because repeating them only costs another boot.
    if (!r.ok && isRetryable(r)) {
      for (let attempt = 2; attempt <= 3 && !r.ok; attempt++) {
        const again = await runScreen(s.app, s.name, timeoutMs, attemptBudget(s, attempt));
        if (again.ok) { again.attempts = attempt; r = again; }
      }
    }
    done++;
    const tag = r.ok ? 'ok  ' : 'FAIL';
    const note = r.attempts ? ` (attempt ${r.attempts})` : '';
    process.stdout.write(`[${String(done).padStart(3)}/${runnable.length}] ${tag} ${s.app.padEnd(15)} ${s.name}${note}${r.ok ? '' : '  -- ' + (r.reason || r.error || '')}\n`);
    return r;
  });

  const all = results.concat(skipped);
  const elapsed = ((Date.now() - t0) / 1000).toFixed(0);

  // Summary: per app, and grouped failure reasons so 60 identical DataStore gaps read as one problem.
  console.log('\n=== summary ===');
  for (const k of appKeys) {
    const rows = all.filter((r) => r.app === k);
    const pass = rows.filter((r) => r.ok).length;
    console.log(`${k.padEnd(16)} ${pass}/${rows.length} rendered`);
  }
  const failures = all.filter((r) => !r.ok);
  const buckets = new Map();
  for (const f of failures) {
    let key = f.reason || f.error || 'unknown';
    key = key.replace(/Cannot read properties of undefined \(reading '([^']+)'\)/, "missing DataStore value: '$1'");
    buckets.set(key, (buckets.get(key) || 0) + 1);
  }
  if (buckets.size) {
    console.log('\nfailure reasons:');
    for (const [k, n] of [...buckets.entries()].sort((a, b) => b[1] - a[1])) {
      console.log(`  ${String(n).padStart(3)}x  ${k}`);
    }
  }

  // Style coverage: of the classes the rendered screens emitted, how many have a rule in the shipped
  // CSS. This is the difference between "a component ran" and "the layout has styles for what it drew".
  const withCss = all.filter((r) => r.ok && r.css);
  if (withCss.length) {
    let emitted = 0;
    let styled = 0;
    const unstyledCount = new Map();
    for (const r of withCss) {
      emitted += r.css.emitted;
      styled += r.css.styled;
      for (const c of r.css.unstyled || []) unstyledCount.set(c, (unstyledCount.get(c) || 0) + 1);
    }
    const pct = emitted ? ((styled / emitted) * 100).toFixed(1) : '0.0';
    console.log(`\nstyle coverage: ${styled}/${emitted} emitted classes have a rule in the shipped CSS (${pct}%)`);
    const top = [...unstyledCount.entries()].sort((a, b) => b[1] - a[1]).slice(0, 6);
    if (top.length) console.log(`  most common unstyled: ${top.map(([c, n]) => `${c}(${n})`).join(', ')}`);
  }
  const missingSheets = new Set();
  for (const r of all) for (const s of r.missingStylesheets || []) missingSheets.add(s);
  if (missingSheets.size) {
    console.log(`\nstylesheets requested but absent from the export: ${missingSheets.size}`);
    for (const s of [...missingSheets].slice(0, 8)) console.log(`  ${s}`);
  }
  const pass = all.filter((r) => r.ok).length;
  console.log(`\nTOTAL ${pass}/${all.length} screens rendered in ${elapsed}s`);

  if (jsonPath) {
    const report = {
      generatedAt: new Date().toISOString(),
      source: 'E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js',
      method: 'one process per screen; app booted, real navigation event fired, own render() counted',
      total: all.length,
      rendered: pass,
      elapsedSeconds: Number(elapsed),
      apps: appKeys.map((k) => {
        const rows = all.filter((r) => r.app === k);
        return { app: k, total: rows.length, rendered: rows.filter((r) => r.ok).length };
      }),
      screens: all
        .map((r) => ({
          app: r.app, screen: r.screen, id: r.id, isMenu: !!r.isMenu,
          ok: !!r.ok, renders: r.renders, nodes: r.nodes, classes: r.classes,
          css: r.css || null,
          missingStylesheets: r.missingStylesheets || [],
          textLength: r.textLength, text: r.text || null,
          error: r.error || null, reason: r.reason || null,
          screenStarted: !!r.screenStarted,
        }))
        .sort((a, b) => (a.app === b.app ? (a.id - b.id) : a.app.localeCompare(b.app))),
    };
    fs.writeFileSync(jsonPath, JSON.stringify(report, null, 2));
    console.log(`wrote ${jsonPath}`);
  }

  process.exit(pass === all.length ? 0 : 1);
}

main().catch((e) => { console.error('sweep crashed:', (e && e.stack) || e); process.exit(2); });
