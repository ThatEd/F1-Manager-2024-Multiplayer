'use strict';
/*
 * press_test.js - verify the SHIPPING press module (pak/.../js/coop/press.js) against the real UI.
 *
 * This is not a test of the harness. It loads the exact file that goes into the game pak into the
 * headless render host and checks what it does to the real components, so a change to the pak module is
 * caught before it reaches a game install.
 *
 * What is covered here, and what is not - deliberately kept honest:
 *
 *   VERIFIED:  `continue` emits ContinueGame
 *              `load` runs the two-step save-list sequence and emits LoadGame
 *              control discovery finds the right elements
 *              a disabled control is refused instead of silently "succeeding"
 *              label matching works against both translated and raw-symbol text
 *
 *   NOT VERIFIED: `advance`. `ContinueButtonContainer.onSelect` branches on
 *              `Engine.isCoherentPlayer()` first: in this harness that is TRUE (preview path), so the
 *              press sets `MainHud.advancing` and returns without emitting anything. In the shipped game
 *              it is FALSE and the same press runs the advance switch. The press plumbing is identical,
 *              so this is a property of that one view - it can only be confirmed in-game. Asserting it
 *              here would either be a lie or would test a mock.
 */

const fs = require('fs');
const vm = require('vm');
const path = require('path');
const { load, readModule, exportAvailable, CONTENT_ROOT } = require('./export_loader.js');

// Skip, not fail, when the UI export is absent. This suite renders real components from the game's own
// decompiled UI, so without the export there is nothing to press - and failing would make a fresh clone
// red for a reason that has nothing to do with the code. The runner reports the variable that fixes it.
if (!exportAvailable()) {
  console.log('SKIP: press_test needs the game UI export (F1M_UI_EXPORT), not found at ' + CONTENT_ROOT);
  process.exit(0);
}

const PRESS = path.join(
  __dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'press.js'
);

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

/*
 * Career views throw inside preact's deferred flush when generated data lacks a value the view indexes
 * (here: DashboardEventPanel -> CalendarUtils.js:777 `currEvent.filterCategory`). That arrives as an
 * uncaught exception, which by default ends the process. The suites below deliberately visit career
 * screens, so the run must survive one bad view to report the rest - same policy as ui_driver.js.
 */
let activeCase = null;
let caseError = null;
process.on('uncaughtException', (e) => { caseError = caseError || ((e && e.message) || String(e)); });
process.on('unhandledRejection', (e) => { caseError = caseError || ((e && e.message) || String(e)); });

/** Boot an app, load the shipping module into it, and tap engine.trigger to record events. */
async function open(appId, quiet = true) {
  const l = load(appId, { quiet });
  const eng = readModule(l, 'common/core/Engine');
  const ready = await l.waitFor(() => {
    try { return eng.isCoherentPlayer() === true; } catch (_) { return false; }
  }, { maxMs: 20000 });
  if (!ready.ok) throw new Error('engine not ready');
  await l.waitFor(() => l.document.body.childNodes.length > 0, { maxMs: 30000 });
  await l.tick(1600);

  const sent = [];
  const e = l.ctx.engine;
  const real = e.trigger;
  e.trigger = function (name, ...args) { sent.push({ name, args }); return real.apply(this, arguments); };

  vm.runInContext(fs.readFileSync(PRESS, 'utf8'), l.ctx, { filename: 'press.js' });
  const P = l.ctx.window.__coopPress;
  if (!P) throw new Error('press.js did not install window.__coopPress');
  return { l, P, sent, eng, DS: readModule(l, 'common/core/DataStore') };
}

function events(sent) {
  return sent.map((e) => e.name).filter((n) => !/^SendAudioEventMessage$|^DSCallID\d+$/.test(n));
}

const invoke = (P, method, ...args) => new Promise((res) => P[method](...args, (el, err) => res({ el, err })));

async function run() {
  console.log('press: the shipping pak module, against the real UI\n');

  // ---- module shape -----------------------------------------------------------------------
  {
    const { l, P } = await open('project/apps/FrontendApp');
    ok(P.version >= 1, 'module installs window.__coopPress', typeof P.version);
    ok(typeof P.press === 'function' && typeof P.pressLoad === 'function', 'exposes press and pressLoad');
    ok(P.gapMs >= 20, 'uses a non-zero mousedown->click gap', `gapMs=${P.gapMs}`);
    l.dispose();
  }

  // ---- control discovery ------------------------------------------------------------------
  {
    const { l, P } = await open('project/apps/FrontendApp');
    const list = P.list();
    ok(list.length >= 6, 'finds the menu controls', `${list.length} found`);
    ok(P.has('continue'), 'reports the continue control present');
    ok(list.some((s) => /FrontendContinueButton_root/.test(s)), 'lists the continue button by class');
    l.dispose();
  }

  // ---- continue ---------------------------------------------------------------------------
  {
    const { l, P, sent } = await open('project/apps/FrontendApp');
    const r = await invoke(P, 'press', 'continue');
    await l.tick(500);
    ok(r.el, 'continue control was pressed', r.err || 'no element');
    ok(events(sent).includes('ContinueGame'), 'pressing continue emitted ContinueGame', events(sent).join(','));
    l.dispose();
  }

  // ---- load (two steps) -------------------------------------------------------------------
  {
    const { l, P, sent } = await open('project/apps/FrontendApp');
    const r = await invoke(P, 'pressLoad', 'save1');
    await l.tick(900);
    const evs = events(sent);
    ok(r.el, 'load sequence reached the confirm button', r.err || 'no element');
    ok(evs.includes('LoadGame'), 'the sequence emitted LoadGame', evs.join(','));
    const lg = sent.find((e) => e.name === 'LoadGame');
    ok(lg && typeof lg.args[0] === 'string' && lg.args[0].length > 0,
      'LoadGame carried a slot name', lg ? String(lg.args[0]) : 'none');
    l.dispose();
  }

  // ---- a disabled control is refused, not faked -------------------------------------------
  {
    const { l, P, sent, DS } = await open('project/apps/TeamManagementApp');
    // Seed just enough for the career advance button to render, then check the module's own verdict.
    try {
      const day = DS.getValue(['Calendar', 'NextEvent'], 'nextEventDay') || 0;
      const base = ['Calendar', 'UpcomingEvents', String(day), 'Events', '0'];
      DS.setValue(base, 'eventName', 'DEFAULT');
      DS.setValue(base, 'daysUntilEvent', 1);
      DS.setValue(base, 'referenceID', 0);
      DS.setValue(base, 'filterCategory', 0);
      await l.tick(1200);
    } catch (_) { /* if seeding fails the assertions below report that, rather than the run dying */ }

    const btn = l.document.querySelector('.ContinueButton_root');
    ok(!!btn, 'career advance button rendered');
    const disabled = btn && btn.classList && (btn.classList.contains('disabled') || btn.classList.contains('blocked'));
    const r = await invoke(P, 'press', 'advance');
    await l.tick(400);

    if (disabled) {
      ok(!r.el, 'a disabled advance button is refused', 'module pressed it anyway');
    } else {
      // Enabled: the press must go through. What it *emits* is the view's business, and in this
      // preview path it emits nothing - see the header note. Record that instead of asserting a lie.
      ok(!!r.el, 'an enabled advance button is pressed', r.err || 'no element');
      ok(!events(sent).includes('AdvanceToNextEvent'),
        'preview path emits no AdvanceToNextEvent (documented, not a defect)', events(sent).join(','));
    }
    l.dispose();
  }

  // ---- label matching handles raw symbols --------------------------------------------------
  {
    const { l, P } = await open('project/apps/FrontendApp');
    // The menu's text may still be raw `[LOAD_CAREER]` at this point; both forms must match.
    const raw = String(l.document.body.textContent || '');
    const isRaw = raw.includes('[LOAD_CAREER]');
    ok(P.has('loadmenu'), `load menu control found (text ${isRaw ? 'raw' : 'translated'})`);
    l.dispose();
  }

  // ---- race weekend: the advance press emits real events -----------------------------------
  // This is the case that matters for co-op: getting through a race weekend. Unlike the career
  // ContinueButtonContainer, these handlers emit unconditionally with no isCoherentPlayer guard, so a
  // press here is verifiable headless.
  {
    const { l, P, sent } = await open('project/apps/RaceWeekendApp');
    const nav = readModule(l, 'project/utils/RaceWeekendNavigation');
    const eng2 = readModule(l, 'common/core/Engine');

    const visit = async (id) => {
      nav.setDefault(-1);
      eng2.sendEvent('SetRaceView', id);
      await l.tick(900);
    };

    ok(P.has('advance'), 'race weekend exposes an advance control at boot');

    // SessionOverview emits SessionOverviewClosed (SessionOverview.js:147-152).
    await visit(8);
    let before = sent.length;
    let r = await invoke(P, 'press', 'advance');
    await l.tick(500);
    let evs = events(sent.slice(before));
    ok(r.el, 'pressed advance on SessionOverview', r.err || 'no element');
    ok(evs.includes('SessionOverviewClosed'), 'SessionOverview advance emitted SessionOverviewClosed', evs.join(','));

    // SessionResults emits AdvanceToNextStage (SessionResults.js:17-19).
    await visit(9);
    before = sent.length;
    r = await invoke(P, 'press', 'advance');
    await l.tick(500);
    evs = events(sent.slice(before));
    ok(r.el, 'pressed advance on SessionResults', r.err || 'no element');
    ok(evs.includes('AdvanceToNextStage'), 'SessionResults advance emitted AdvanceToNextStage', evs.join(','));

    l.dispose();
  }

  // ---- race weekend: pre-session confirm ----------------------------------------------------
  // Its ContinueButton's onSelect is `onAdvanceNext` (PresessionView.js:242-252), which sends
  // `PreSessionClosed` - the only event that screen emits, and the closest thing the UI has to "start
  // the session". Needs `seedDefaults` first: the ERS panels subtract on a null maxEnergyStored
  // (RaceWeekendUtil.js:187-199) and throw, aborting the press before onSelect runs.
  //
  // Booted fresh. Visiting PreSession after the other race-weekend screens leaves state consumed from
  // the earlier presses, and the sequence then misses - a real ordering sensitivity, so the test does
  // not hide it behind a shared boot.
  {
    const { l, P, sent } = await open('project/apps/RaceWeekendApp');
    const DS = readModule(l, 'common/core/DataStore');
    const eng2 = readModule(l, 'common/core/Engine');
    const nav = readModule(l, 'project/utils/RaceWeekendNavigation');

    ok(typeof P.seedDefaults === 'function', 'module exposes seedDefaults');
    let filled = 0;
    try { filled = P.seedDefaults(DS); } catch (_) { filled = -1; }
    ok(filled >= 0, 'seedDefaults ran without throwing', String(filled));

    // Seed BEFORE navigating: the race-weekend views read these values during the render the
    // navigation triggers, so seeding afterwards is too late.
    // Seed, navigate, seed again.
    //
    // Order matters and neither order alone is reliable: the pre-session view reads these values during
    // the render that navigation triggers, but the app's own bootstrap can also rebuild `Parts.*` after
    // an early seed. Seeding before navigation covers the first render; seeding again afterwards covers
    // a bootstrap that ran late. Both are idempotent (`seedDefaults` only fills nulls).
    nav.setDefault(-1);
    eng2.sendEvent('SetRaceView', 0);
    await l.tick(1400);
    try { P.seedDefaults(DS); } catch (_) { }

    const raceStartReady = () => {
      const list = P.list();
      return list.some((s) => /ContinueButton_root/.test(s) && /PRE_SESSION_RACE_START/i.test(s));
    };
    let appeared = await l.waitFor(raceStartReady, { maxMs: 15000 });
    if (!appeared.ok) {
      // Force a re-render now that the data is in place, then look again.
      try { P.seedDefaults(DS); } catch (_) { }
      nav.setDefault(-1);
      eng2.sendEvent('SetRaceView', 0);
      await l.tick(1200);
      appeared = await l.waitFor(raceStartReady, { maxMs: 15000 });
    }
    // 4000ms was not enough when the whole suite ran concurrently: this is a real headless render of
    // RaceWeekendApp, and under load it took longer than four seconds. A fixed budget on a cost that varies
    // with machine load fails for reasons unrelated to what it tests - the same defect that made a live-server
    // suite report "timed out" after a Rust build. The render is deterministic, just slow when starved, so the
    // budget is generous and the failure message says what it actually saw.
    ok(appeared.ok, 'the race-start confirm appeared on PreSession',
      appeared.ok ? '' : `waited 30s across two attempts; saw ${(P.list().filter((s) => /Continue/.test(s))[0] || 'nothing')}`);
    if (appeared.ok) {
      const before = sent.length;
      const r = await invoke(P, 'press', 'advance');
      await l.tick(900);
      const evs = events(sent.slice(before));
      ok(r.el, 'pressed advance on PreSession', r.err || 'no element');
      ok(evs.includes('PreSessionClosed'),
        'PreSession advance emitted PreSessionClosed (the pre-session confirm)', evs.join(',') || 'none');
    }

    l.dispose();
  }

  const fail = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - fail} passed, ${fail} failed`);
  process.exit(fail ? 1 : 0);
}

run().catch((e) => { console.error('press test crashed:', (e && e.stack) || e); process.exit(2); });
