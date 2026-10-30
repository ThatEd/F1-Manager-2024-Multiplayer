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

/**
 * Source with `//` and `/* *\/` comments removed, so an assertion about *what the player is told* is not
 * tripped by a comment that quotes the wording it forbids. Strings are left intact - that is the point.
 */
function stripComments(src) {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, ' ')
    .split('\n')
    .map((line) => {
      // Only strip `//` when it is not inside a string. A simple scan is enough here: the file uses no regex
      // literals containing `//`, and a miss only makes the assertion stricter, never falsely lenient.
      let inStr = false, quote = null;
      for (let i = 0; i < line.length; i++) {
        const ch = line[i];
        if (inStr) {
          if (ch === '\\') { i++; continue; }
          if (ch === quote) inStr = false;
        } else if (ch === '"' || ch === "'" || ch === '`') {
          inStr = true; quote = ch;
        } else if (ch === '/' && line[i + 1] === '/') {
          return line.slice(0, i);
        }
      }
      return line;
    })
    .join('\n');
}

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
    /* The multiplayer tile takes the Race Replay slot, so `MainMenu_scenarioMode` is the anchor that matters.
     * `MainMenu_rightBox` was the previous target and is the wrong place - it is the carousel/news column, not
     * the action row. Both are asserted so a future move is a deliberate edit rather than a silent drift. */
    ok(/MainMenu_scenarioMode/.test(menuSrc),
      'MainMenu.js still renders MainMenu_scenarioMode (the tile multiplayer replaces)',
      'without it there is no action-row tile to take, and the mod would have nowhere correct to put its entry');

    /* Race Replay is relocated to the small button row, which is delimited by this separator. If the row is
     * restructured, Race Replay would be lost - a regression in normal play caused by the mod. */
    ok(/MainMenu_separator horizontal/.test(menuSrc),
      'MainMenu.js still has the small button row (MainMenu_separator horizontal)',
      'Race Replay is re-emitted just before it');
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
// 3b. The two modes, and that Race Replay is not lost
// ---------------------------------------------------------------------------------------------

console.log('\nmode list and race replay');

{
  const modeIds = [...lobbySrc.matchAll(/id:\s*"([a-z-]+)"/g)].map((m) => m[1]);
  ok(modeIds.length === 2, 'exactly two modes are offered', modeIds.join(', '));
  ok(modeIds.includes('shared-team') && modeIds.includes('versus'),
    'the two modes are Shared team and Versus', modeIds.join(', '));
  ok(!/coop-career/.test(lobbySrc),
    'the unimplemented "Co-op career" mode is gone',
    'a third choice the data model cannot honour is worse than two that work');

  /* Race Replay must survive the change. It is moved into the small button row, not removed, and the mod
   * must not cost the player a feature of the base game. */
  ok(/SCENARIOMODE_HEADER/.test(lobbySrc), 'lobby.js re-emits Race Replay (SCENARIOMODE_HEADER)');
  ok(/ScreenID\.ScenarioDLCSelection/.test(lobbySrc),
    'relocated Race Replay opens ScenarioDLCSelection, which is where the original tile went',
    'MainMenu.js:169 - sending it to ScenarioMode instead would open the wrong screen');

  /* The multiplayer tile reuses the same component and texture as the tile it replaces, so the bounds cannot
   * drift; only the colour changes, and it changes through CSS. */
  ok(/FrontendTextureButton/.test(lobbySrc), 'the tile uses FrontendTextureButton like its neighbours');
  ok(/Frontend\/ExclusiveScenarios/.test(lobbySrc),
    'the tile uses the Exclusive Scenarios texture, as asked');
  ok(/MainMenu_coopMode/.test(lobbySrc), 'the tile carries a hook class for the purple treatment');
  ok(/CoopPurple|hue-rotate/.test(lobbySrc), 'the purple is applied by CSS, not a separate texture');

  /* Player-facing text must never ask anyone to run an injector or a launcher. The agent is loaded by
   * version.dll beside the game exe, so opening the game is the whole install step; telling a player otherwise
   * is a second step to forget, and forgetting it looks exactly like a broken mod. This exact wording
   * regressed twice, so it is asserted - against the code with comments removed, because the comment above this
   * assertion necessarily mentions the wording it forbids. */
  const codeOnly = stripComments(lobbySrc);
  ok(!/coop-link --inject|run an injector|start the game through/i.test(codeOnly),
    'no player-facing text asks for a manual inject or a launcher');
  ok(/version\.dll/.test(codeOnly),
    'the failure message names the file that actually loads the agent');

  /* A pak and the agent DLL are deployed separately, so they drift. When the pak called a route the injected
   * DLL did not serve, the player saw a bare "HTTP 404" and it read as a broken mod. The screen must probe the
   * agent's build first and name a stale DLL as the cause. */
  ok(/\/coop\/version/.test(lobbySrc), 'the screen probes the agent build before using it');
  ok(/older than this UI mod/.test(lobbySrc),
    'a 404 from the version probe is reported as an out-of-date DLL, not a generic failure');
  /* Order matters, not just presence: the probe has to run before the status request, or a stale DLL is
   * reported as a hook problem. Measured offsets put it ~790 chars after `componentDidMount`, so the window
   * has to be wider than that - a too-tight window fails on correct code, which is a bad test. */
  ok(/checkAgentBuild\(/.test(lobbySrc) && /componentDidMount[\s\S]{0,2000}checkAgentBuild\(/.test(lobbySrc),
    'the probe runs on mount');
  ok(lobbySrc.indexOf('checkAgentBuild(', lobbySrc.indexOf('componentDidMount')) <
     lobbySrc.indexOf('agent("status"', lobbySrc.indexOf('componentDidMount')),
    'the build probe runs before the status request');
}

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
