'use strict';
/*
 * pak_paths_test.js - can the game actually find the files in our pak?
 *
 * Why this file exists
 * --------------------
 * "No multiplayer button" was caused by the pak storing `Frontend.html` instead of
 * `F1Manager24/Content/UIGameface/Frontend.html`. Everything else was right: the pak loaded, was
 * byte-identical on both machines, reported the correct version (V11), the correct mount point
 * (`../../../`) and no compression, and it contained exactly the intended files.
 *
 * Every existing check asked "was the pak built from the right source files" and none asked "will the game
 * resolve those entries". With mount point `../../../` - three up from `Content/Paks` is the game root - the
 * game looks for `<game root>/<entry>`, so a bare entry addresses a file that does not exist and the pak
 * silently overrides nothing.
 *
 * This test reads the built pak's own entry list and enforces the prefix. It needs only a built pak, not the
 * game and not the export, so it runs everywhere.
 */

const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const ROOT = path.join(__dirname, '..', '..');
const PAK = path.join(ROOT, 'pak', 'CoopBridge_P.pak');
const REPAK = path.join(ROOT, 'tools', 'bin', 'repak.exe');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

console.log('pak_paths: will the game resolve the entries in our pak?\n');

if (!fs.existsSync(PAK)) {
  console.log('SKIP: pak_paths_test needs a built pak - run `python tools/deploy_pak.py --no-deploy`');
  console.log('  (expected at ' + PAK + ')');
  process.exit(0);
}

if (!fs.existsSync(REPAK)) {
  console.log('SKIP: pak_paths_test needs repak at tools/bin/repak.exe');
  process.exit(0);
}

const PREFIX = 'F1Manager24/Content/UIGameface/';

// ---------------------------------------------------------------------------------------------
// 1. Info: the mount point decides what the entries must look like
// ---------------------------------------------------------------------------------------------

let info = '';
try {
  info = execFileSync(REPAK, ['info', PAK], { encoding: 'utf8' });
} catch (e) {
  info = '';
}

const mount = (info.match(/mount point:\s*(.+)/) || [])[1];
ok(!!mount, 'the pak reports a mount point', info.slice(0, 120));
if (mount) {
  /* The relationship is the whole point: entries are resolved relative to this. `../../../` counts up from
     `Content/Paks` - Paks -> Content -> F1Manager24 -> the game root - so entries must be game-root-relative
     and therefore start with `F1Manager24/`. Measured against the working mod pak in the install, which uses
     exactly this mount point with full-path entries. */
  ok(mount.trim() === '../../../',
    'mount point is `../../../` (game root)', 'got ' + JSON.stringify(mount.trim()));
}

const version = (info.match(/version:\s*(\S+)/) || [])[1];
ok(version === 'V11', 'pak version is V11, matching the working mod paks', 'got ' + version);

// ---------------------------------------------------------------------------------------------
// 2. Every entry carries the full path
// ---------------------------------------------------------------------------------------------

let listing = '';
try {
  listing = execFileSync(REPAK, ['list', PAK], { encoding: 'utf8' });
} catch (e) {
  listing = '';
}
const entries = listing
  .replace(/\r/g, '\n')
  .split('\n')
  .map((l) => l.trim().replace(/\\/g, '/'))
  .filter((l) => l && !l.startsWith('Packed'));

ok(entries.length > 0, 'the pak lists entries', 'none - repak could not read it');

if (entries.length) {
  const bad = entries.filter((e) => !e.startsWith(PREFIX));
  ok(bad.length === 0,
    'every entry is under ' + PREFIX,
    `${bad.length} of ${entries.length} missing the prefix, e.g. ${bad[0]} - with mount point ../../../ ` +
    'these resolve to the game root and override nothing');

  /* The specific files that matter. A pak with the right prefix but no Frontend.html would override nothing
     useful; naming them makes the failure legible. */
  for (const want of ['Frontend.html', 'js/coop/lobby.js', 'js/coop/bridge.js']) {
    ok(entries.includes(PREFIX + want), `pak contains ${PREFIX}${want}`);
  }

  // Nothing extra. A scratch file in the source tree shipped into a build once already.
  const allowed = new Set([
    PREFIX + 'Frontend.html',
    PREFIX + 'MinimapContent.html',
    PREFIX + 'RaceWeekend.html',
    PREFIX + 'TeamManagement.html',
    PREFIX + 'js/coop/bridge.js',
    PREFIX + 'js/coop/lobby.js',
    PREFIX + 'js/coop/observe.js',
    PREFIX + 'js/coop/press.js',
  ]);
  const extra = entries.filter((e) => !allowed.has(e));
  ok(extra.length === 0, 'no unexpected files in the pak', extra.join(', '));
}

// ---------------------------------------------------------------------------------------------
// 3. The same rules apply to any pak a user builds, so state the invariant
// ---------------------------------------------------------------------------------------------

/* If the prefix requirement ever changes - a different mount point, a different game layout - this comment
   plus the two assertions above are the place to change it. The failure mode is silent by construction:
   nothing errors, the pak simply addresses files that are not there. */

const failed = results.filter((r) => !r.ok);
console.log(`\npak_paths: ${results.length - failed.length}/${results.length} passed`);
if (failed.length) {
  console.log('failures:');
  for (const f of failed) console.log('  - ' + f.name);
  process.exit(1);
}
process.exit(0);
