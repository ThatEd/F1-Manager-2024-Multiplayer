'use strict';
/*
 * lobby_boot_test.js - does a multiplayer entry point actually reach the UI?
 *
 * Why this file exists
 * --------------------
 * "No multiplayer button on either side" was reported against two different builds and **no suite covered
 * it**. Every other test here tests a module: the observer's event mapping, the press module's control
 * discovery, the proto client. None executed `lobby.js`, which is the file that renders the entry point. So
 * the one thing a player sees was the one thing with no test, and a regression there stays invisible until
 * somebody launches the game and looks.
 *
 * What it checks, and why each is worth a test
 * -------------------------------------------
 *  1. every AMD module `lobby.js` requires resolves. One bad id makes the whole `require([...])` never call
 *     back - a total, silent failure of the integration.
 *  2. the `MainMenu_rightBox` anchor still exists. The patch prepends into it; a restructured menu breaks the
 *     patch without breaking anything else.
 *  3. the fallback button exists and is positioned. This is diagnostic: if the patch fails, a corner button
 *     still appears, so "no button at all" means lobby.js never ran - a pak or load-order problem - rather
 *     than that the anchor moved.
 *  4. each page references its coop scripts, and Frontend loads lobby.js before require.js.
 *  5. observe.js declines instead of throwing when there is no engine, because a throw at load time aborts
 *     the remaining script tags.
 *
 * Skips with a named reason when the decompiled export is absent.
 */

const path = require('path');
const fs = require('fs');

const { load, readModule, exportAvailable, CONTENT_ROOT } = require('./export_loader.js');

if (!exportAvailable()) {
  console.log('SKIP: lobby_boot_test needs the game UI export (F1M_UI_EXPORT), not found at ' + CONTENT_ROOT);
  process.exit(0);
}

const ROOT = path.join(__dirname, '..', '..');
const PAK = path.join(ROOT, 'pak', 'f1manager24', 'Content', 'UIGameface');
const COOP = path.join(PAK, 'js', 'coop');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

console.log('lobby_boot: does the multiplayer entry point actually reach the UI?\n');

const lobbySrc = fs.readFileSync(path.join(COOP, 'lobby.js'), 'utf8');

// ---------------------------------------------------------------------------------------------
// 1. Every module lobby.js requires must resolve
// ---------------------------------------------------------------------------------------------

console.log('the modules lobby.js needs');

const modList = (() => {
  const m = lobbySrc.match(/var MODULES = \[([\s\S]*?)\];/);
  return m ? [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]) : [];
})();

ok(modList.length >= 10, 'lobby.js declares its module list', `${modList.length} ids`);

{
  let l = null;
  let bootErr = null;
  try {
    l = load('project/apps/FrontendApp', { quiet: true });
  } catch (e) {
    bootErr = e.message;
  }
  ok(l !== null, 'the front-end app boots in the harness', bootErr);

  if (l) {
    const missing = [];
    for (const id of modList) {
      try {
        const ns = readModule(l, id);
        if (ns === undefined || ns === null) missing.push(id);
      } catch (e) {
        missing.push(id + ' (' + e.message + ')');
      }
    }
    ok(missing.length === 0, 'every required module resolves', missing.slice(0, 5).join(' | '));
    try { l.dispose(); } catch (e) { /* nothing to release in the harness */ }
  }
}

// ---------------------------------------------------------------------------------------------
// 2. The anchor the menu patch prepends into
// ---------------------------------------------------------------------------------------------

console.log('\nthe menu anchor');

{
  /* Read the component the patch will actually run against: the export's MainMenu.js. The pak deliberately
     does not carry a copy (it is the game's own UI), so the export is the only correct source. An earlier
     version of this test fell back to serialising the module's export *keys* when the pak copy was absent,
     which contains no JSX and therefore reported the anchor missing - a false negative that would have sent
     someone editing a menu that was fine. */
  const exportMenu = path.join(CONTENT_ROOT, 'UIGameface', 'js', 'project', 'modules', 'frontend',
    'components', 'MainMenu.js');
  let menuSrc = null;
  try {
    menuSrc = fs.readFileSync(exportMenu, 'utf8');
  } catch (e) {
    menuSrc = null;
  }
  ok(menuSrc !== null, 'MainMenu.js can be read from the export', exportMenu);

  if (menuSrc !== null) {
    const count = (menuSrc.match(/MainMenu_rightBox/g) || []).length;
    ok(count > 0, 'the patch anchor MainMenu_rightBox still exists',
      'the patch would find no anchor and fall back to a corner button');
  }
}

// ---------------------------------------------------------------------------------------------
// 3. The fallback, and what its presence licenses us to conclude
// ---------------------------------------------------------------------------------------------

console.log('\nthe fallback path');

ok(/function fallback\(/.test(lobbySrc), 'lobby.js defines a fallback button');
ok(/coop-fallback/.test(lobbySrc), 'the fallback is identifiable (id coop-fallback)');
ok(/position:fixed/.test(lobbySrc), 'the fallback is positioned, not layout-dependent');

// ---------------------------------------------------------------------------------------------
// 4. Page wiring
// ---------------------------------------------------------------------------------------------

console.log('\npage wiring');

for (const page of ['Frontend.html', 'TeamManagement.html', 'RaceWeekend.html', 'MinimapContent.html']) {
  const html = fs.readFileSync(path.join(PAK, page), 'utf8');
  const needed = page === 'Frontend.html'
    ? ['coop/press.js', 'coop/observe.js', 'coop/bridge.js', 'coop/lobby.js', 'require.js']
    : ['coop/press.js', 'coop/observe.js', 'coop/bridge.js', 'require.js'];
  const idx = needed.map((f) => html.indexOf(f));
  ok(idx.every((i) => i >= 0), `${page} references its coop scripts`,
    needed.filter((f, i) => idx[i] < 0).join(', ') + ' missing');
  if (page === 'Frontend.html' && idx.every((i) => i >= 0)) {
    ok(idx[3] < idx[4], 'Frontend loads lobby.js before require.js');
  }
}

// ---------------------------------------------------------------------------------------------
// 5. Load safety: a throw at load time aborts the remaining script tags
// ---------------------------------------------------------------------------------------------

console.log('\nload safety');

{
  const observeSrc = fs.readFileSync(path.join(COOP, 'observe.js'), 'utf8');
  ok(/root\.CoopObserve/.test(observeSrc), 'observe.js installs itself on window');
  ok(/typeof engine\.trigger !== 'function'/.test(observeSrc),
    'observe.js declines rather than throwing when there is no engine');
}

// ---------------------------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------------------------

const failed = results.filter((r) => !r.ok);
console.log(`\nlobby_boot: ${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log('failures:');
  for (const f of failed) console.log('  - ' + f.name);
  /* `process.exit` rather than falling through: booting the front-end app leaves the game's own timers and
     preact's rerender queue scheduled inside the vm context, so the event loop never drains and node sits
     there after the summary has printed. The exit code is the whole result, so leaving deliberately is
     correct - and it is why this suite hung when the runner spawned it. */
  process.exit(1);
}

/* An explicit exit(0) for the same reason: the harness holding timers is not a failure, and a suite that
   never returns is indistinguishable from a deadlock to whatever spawns it. */
process.exit(0);
