'use strict';
/*
 * css_index.js - index the export's real stylesheets.
 *
 * The export ships 1,002 .css files (3.8 MB) at Content/UIGameface/css, which is what the game loads:
 * `CSSUtil.loadCSS` (common/util/CSSUtil.js:6-32) appends `<link href="css/<name>.css">`. Three things
 * need that data and none of them are guessable:
 *
 *  1. **Custom properties.** ~20 call sites - every graph (DonutChart.js:34, PitGraph.js:146,
 *     TrackGraph.js:22, FuelGraph.js:18, ...), LayoutUtil.js:16 and Repeater.js:167 - read CSS
 *     variables via `getComputedStyle(document.body).getPropertyValue('--x')`. Returning '' for all of
 *     them silently changes behaviour, so `:root` / `html` / `body` custom properties are indexed here
 *     and served by the DOM host.
 *  2. **Class coverage.** Every class a view emits can be checked against the real selector set. That
 *     turns "161 screens rendered" into "161 screens rendered, each with N% of its classes styled" -
 *     a much stronger claim about the *layout* actually existing.
 *  3. **Missing files.** A view can request a stylesheet that is not in the export (TeamManagement.css,
 *     RaceWeekend.css and many per-component files are absent). Reporting which is honest; pretending
 *     the link loaded hides a real gap.
 *
 * Parsing is deliberately shallow: selector class names, and custom properties declared on root-ish
 * selectors. It is not a cascade, and it does not compute layout - see the README's limits section.
 */

const fs = require('fs');
const path = require('path');

const CONTENT_ROOT = String.raw`E:\FModel\Output\Exports\F1Manager24\Content`;
const CSS_ROOT = path.join(CONTENT_ROOT, 'UIGameface', 'css');
const EXPORT_LOC_ROOT = path.join(CONTENT_ROOT, 'Localization', 'Volta');
const CACHE = path.join(__dirname, '.css-index.json');

/** Strip comments and at-rule blocks that would confuse a naive brace scan. */
function stripNoise(css) {
  return css
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/@(?:media|supports|layer|container)[^{]*\{/g, '');
}

/**
 * Collect custom properties declared on `:root`, `html` or `body`.
 *
 * Only the root-ish selectors matter: `getComputedStyle(document.body)` inherits from `:root`, so a
 * variable a graph reads must be declared there (or on body). Values declared on `.someClass` are not
 * inherited into body and are therefore not indexed.
 */
function collectRootVars(css, into) {
  const src = stripNoise(css);
  const re = /(?:^|[},])\s*(:root|html|body)\s*\{([^}]*)\}/gi;
  let m;
  while ((m = re.exec(src)) !== null) {
    const body = m[2];
    const propRe = /(--[A-Za-z0-9_-]+)\s*:\s*([^;]+);?/g;
    let p;
    while ((p = propRe.exec(body)) !== null) {
      into.set(p[1], p[2].trim());
    }
  }
}

/** Every class name appearing in a selector position. */
function collectClasses(css, into) {
  const src = stripNoise(css);
  // Drop declaration bodies, keeping selectors: a '.name' inside a value (url(...)) must not count.
  const selectorOnly = src.replace(/\{[^{}]*\}/g, '{}');
  const re = /\.(-?[_A-Za-z][A-Za-z0-9_-]*)/g;
  let m;
  while ((m = re.exec(selectorOnly)) !== null) into.add(m[1]);
}

/**
 * Build (or load) the index.
 *
 * Cached to `.css-index.json` because the scan is ~1,000 files / 3.8 MB and takes tens of seconds -
 * paying that per screen process would double the sweep's runtime for no benefit.
 */
function build({ force = false, quiet = true } = {}) {
  if (!force && fs.existsSync(CACHE)) {
    try {
      const c = JSON.parse(fs.readFileSync(CACHE, 'utf8'));
      return { classes: new Set(c.classes), vars: new Map(c.vars), files: c.files, bytes: c.bytes, cached: true };
    } catch (_) { /* rebuild */ }
  }
  const classes = new Set();
  const vars = new Map();
  let files = 0;
  let bytes = 0;
  (function walk(dir) {
    let entries;
    try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch (_) { return; }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) { walk(p); continue; }
      if (!e.name.endsWith('.css')) continue;
      let text;
      try { text = fs.readFileSync(p, 'utf8'); } catch (_) { continue; }
      files++;
      bytes += text.length;
      collectClasses(text, classes);
      collectRootVars(text, vars);
    }
  })(CSS_ROOT);

  const out = { classes: [...classes].sort(), vars: [...vars], files, bytes };
  try { fs.writeFileSync(CACHE, JSON.stringify(out)); } catch (_) { /* read-only fs is fine */ }
  if (!quiet) console.log(`indexed ${files} css files, ${bytes} chars, ${classes.size} classes, ${vars.size} custom properties`);
  return { classes, vars, files, bytes, cached: false };
}

/** Read one stylesheet the way the game's loader would, or null if it is not in the export. */
function readStylesheet(cssRelPath) {
  const rel = String(cssRelPath).replace(/^\.?\//, '').replace(/^css\//, '');
  const file = path.join(CSS_ROOT, rel);
  try {
    if (fs.existsSync(file) && fs.statSync(file).isFile()) return fs.readFileSync(file, 'utf8');
  } catch (_) { /* fall through */ }
  return null;
}

/** `:root` custom properties as a plain object, for the DOM host's getComputedStyle. */
function rootVarsObject() {
  const { vars } = build();
  const o = {};
  for (const [k, v] of vars) o[k] = v;
  return o;
}

/**
 * Every localisation key present in the export.
 *
 * Used to separate two things that look identical in rendered text: a screen showing a raw
 * `[SYMBOL]` because the key is **absent from Volta.json**, versus one where the key **exists** and
 * the view simply is not routing that string through `Localisation.translate`. The first is a gap in
 * the export, the second is a view detail. Guessing between them would be wrong either way.
 */
let locKeyCache = null;
function locKeys() {
  if (locKeyCache) return locKeyCache;
  const set = new Set();
  try {
    const flat = JSON.parse(fs.readFileSync(path.join(EXPORT_LOC_ROOT, 'en', 'Volta.json'), 'utf8'));
    for (const ns of Object.keys(flat)) {
      const entries = flat[ns];
      if (!entries || typeof entries !== 'object') continue;
      for (const k of Object.keys(entries)) set.add(k);
    }
  } catch (_) { /* leave empty: every key then reports as absent, which is honest */ }
  locKeyCache = set;
  return set;
}

module.exports = { build, readStylesheet, rootVarsObject, locKeys, CSS_ROOT, CACHE };

if (require.main === module) {
  const r = build({ force: process.argv.includes('--force'), quiet: false });
  console.log(`classes: ${r.classes.size}, custom properties: ${r.vars.size}, cached: ${r.cached}`);
}
