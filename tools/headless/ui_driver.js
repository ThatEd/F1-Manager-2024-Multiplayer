'use strict';
/*
 * ui_driver.js - apply a co-op command by actually operating the rendered UI.
 *
 * The other two modes are honest but inert: `--mode=none` reports "would trigger X (no engine
 * attached)" and `--mode=replay` replays a recording. Neither changes any state. This module does:
 * it boots the real app for a page, finds the control a player would press, and presses it.
 *
 * Why pressing beats calling `engine.sendEvent` directly: the UI does more than send the event. For
 * `Continue` the press path sends `ContinueGame` (MainMenu.js:148) - but the button also exists only
 * when `hasSaveGame` is true, so a press proves the app agreed there was a save to continue. For
 * `Load` the press requires selecting a slot first, which proves the slot list was populated. Calling
 * the event directly would skip all of that and claim success the view never reached.
 *
 * Every operation returns `via`:
 *   'ui'        a real press produced the event (evidence: the engine recorded it)
 *   'synthetic' no control matched, so engine.sendEvent was called directly - the event fired, but
 *               the UI never agreed it was a legal action
 *   'failed'    nothing was sent
 *
 * The press primitive is the one proven in drive_test.js: `mousedown` then `click`, because
 * ButtonBehaviourT.onClick returns early unless state.down was set by onMouseDown
 * (ButtonBehaviourT.js:119-131).
 */

const { load, readModule } = require('./export_loader.js');

const APPS = {
  frontend: { entry: 'project/apps/FrontendApp', nav: 'project/utils/FrontendNavigation', screenEvent: 'SetFrontendScreenID', enumModule: 'project/data/FrontendScreens' },
  teamManagement: { entry: 'project/apps/TeamManagementApp', nav: 'project/utils/TeamManagementNavigation', screenEvent: 'SetManagementScreenID', enumModule: 'project/data/TeamScreens' },
  raceWeekend: { entry: 'project/apps/RaceWeekendApp', nav: 'project/utils/RaceWeekendNavigation', screenEvent: 'SetRaceView', enumModule: 'project/data/RaceWeekendScreens' },
};

/** Nodes carrying a preact click handler - the elements that actually act. */
function clickable(root) {
  const out = [];
  (function walk(n) {
    if (!n || n.nodeType !== 1) return;
    const L = n._listeners || {};
    if (typeof L.click === 'function') out.push(n);
    for (const c of n.childNodes || []) walk(c);
  })(root);
  return out;
}

function textOf(node) {
  return String((node && node.textContent) || '').replace(/\s+/g, ' ').trim();
}

/**
 * Match a control by label, tolerating both the translated text and the raw symbol.
 *
 * Text is the only stable handle for many controls, but which form it is in depends on whether the
 * localisation archive has loaded yet. Measured on a fresh boot: the menu reads `[LOAD_CAREER]` before
 * loc loads and `Load Career` after, so matching only the English string silently found nothing.
 * Callers pass the symbol they saw in the export (`LOAD_CAREER`) and the word a player reads
 * (`load career`); either form matches.
 */
function matchesLabel(node, needles) {
  const t = textOf(node).toLowerCase();
  return needles.some((n) => n && t.includes(String(n).toLowerCase()));
}

function classHas(node, name) {
  return !!(node && node.classList && node.classList.contains(name));
}

/**
 * Press an element the way a pointer does.
 *
 * `mousedown` is not optional: onClick bails out when `state.down` is false
 * (ButtonBehaviourT.js:119-120) and only onMouseDown sets it (:131). A bare click dispatched at the
 * harness earlier produced zero events for every menu button.
 */
async function press(host, el, settleMs = 350) {
  el.dispatchEvent({ type: 'mousedown', button: 0 });
  await host.tick(30);
  el.dispatchEvent({ type: 'click', button: 0 });
  await host.tick(settleMs);
}

class UiDriver {
  constructor(opts = {}) {
    this.opts = opts;
    this.host = null;          // the export_loader handle for the current page
    this.app = null;           // 'frontend' | 'teamManagement' | 'raceWeekend'
    this.sent = [];            // engine events observed while driving
    this.notes = [];
  }

  log(msg) {
    this.notes.push(msg);
    if (!this.opts.quiet) console.log(`[ui] ${msg}`);
  }

  /** Boot (or reuse) the app for `page`. Answers whether the page is ready to be operated. */
  async open(page, { forceFresh = false } = {}) {
    const key = String(page || 'Frontend').toLowerCase();
    const appKey = key.startsWith('race') ? 'raceWeekend'
      : key.startsWith('team') ? 'teamManagement'
        : 'frontend';
    if (this.app === appKey && this.host && !forceFresh) return { ok: true, reused: true };

    this.close();
    const app = APPS[appKey];
    this.host = load(app.entry, { quiet: true });
    this.app = appKey;

    const eng = readModule(this.host, 'common/core/Engine');
    const ready = await this.host.waitFor(() => {
      try { return eng.isCoherentPlayer() === true; } catch (_) { return false; }
    }, { maxMs: 8000 });
    if (!ready.ok) return { ok: false, error: `engine not ready (${ready.ms}ms)` };

    // Some screens need generated data before they render; the app's own bootstrap builds it, but
    // TeamManagement/RaceWeekend populate more slowly than Frontend.
    const mounted = await this.host.waitFor(() => this.host.document.body.childNodes.length > 0, { maxMs: 12000 });
    if (!mounted.ok) return { ok: false, error: `app mounted nothing (${mounted.ms}ms)` };
    await this.host.tick(appKey === 'frontend' ? 900 : 1500);

    // Tap engine.trigger so every event the UI emits is recorded. This is the evidence used to decide
    // 'ui' vs 'synthetic' - the harness never assumes a press worked, it reads what came out.
    const e = this.host.ctx.engine;
    const real = e.trigger;
    const owner = this;
    e.trigger = function (name, ...args) {
      owner.sent.push({ name, args });
      return real.apply(this, arguments);
    };

    this.seedForCareer(appKey);

    // Wait for the localisation archive to land before looking for controls by label. The app asks
    // for it during bootstrap (FrontendApp.js:17 `loadLocFiles('Volta')`); until it resolves, strings
    // are raw `[SYMBOLS]` and any text-based targeting silently misses.
    try {
      const Loc = readModule(this.host, 'common/core/Localisation');
      await this.host.waitFor(() => {
        try { return Loc.translate('[CONTINUE]') === 'Continue'; } catch (_) { return false; }
      }, { maxMs: 4000 });
    } catch (_) { /* loc is optional; symbol-form matching still works */ }

    this.log(`opened ${appKey}`);
    return { ok: true };
  }

  /**
   * Seed values the career/race-weekend views read that `generateFromSchemaFragment` does not fill.
   *
   * Every entry here was found by a throw, not by reading the schema, and each is a value the real
   * game gets from the career database or the live sim. Filling them is the same thing a loaded save
   * does; without it the view throws inside preact's flush and the control cannot be pressed at all.
   *
   * Deliberately narrow: only fill a value that is currently null, so a page that already has real
   * data is never overwritten.
   */
  seedForCareer(appKey) {
    if (appKey === 'frontend') return;
    try {
      var DS = readModule(this.host, 'common/core/DataStore');
      if (!DS) return;
      var missing = function (ctx, prop) {
        try { return DS.getValue(ctx, prop) == null; } catch (e) { return true; }
      };
      var put = function (ctx, prop, value) {
        if (missing(ctx, prop)) {
            try { DS.setValue(ctx, prop, value); } catch (e) { /* context absent; skip */ }
        }
      };

      // ContinueButtonContainer binds Calendar.UpcomingEvents[day].Events[0] and then renders
      // `state.eventName.toUpperCase()`. Generated data leaves the context empty, so the listener
      // sets eventName = undefined and render throws (ContinueButtonContainer.js:280).
      var nextDay = 0;
      try { nextDay = DS.getValue(['Calendar', 'NextEvent'], 'nextEventDay'); } catch (e) { nextDay = 0; }
      var day = String(nextDay == null ? 0 : nextDay);
      var ev = ['Calendar', 'UpcomingEvents', day, 'Events', '0'];
      put(ev, 'eventName', 'DEFAULT');
      put(ev, 'daysUntilEvent', 1);
      put(ev, 'referenceID', 0);
      put(ev, 'filterCategory', 0);
      put(['Calendar', 'NextEvent'], 'eventName', 'DEFAULT');
      put(['MainHud'], 'advancing', false);

      // The ERS panels read RaceSim.ERSData[i].maxEnergyStored (RaceWeekendUtil.js:187-199) and
      // subtract, so a null there throws in the race-weekend driver panel and the pre-session
      // Continue press dies before it can emit PreSessionClosed.
      for (var i = 0; i < 4; i++) {
        put(['RaceSim', 'ERSData', String(i)], 'maxEnergyStored', 4);
        put(['RaceSim', 'ERSData', String(i)], 'maxEnergyUsed', 0);
      }
      this.log('seeded career/race-weekend data');
    } catch (e) { /* best-effort: a failure shows up as a reported miss, never as a silent pass */ }
  }

  close() {
    if (this.host) { try { this.host.dispose(); } catch (_) {} }
    this.host = null;
    this.app = null;
  }

  /** Events recorded since `since`, excluding audio and DataStore plumbing. */
  since(since) {
    return this.sent.slice(since).filter((e) => !/^SendAudioEventMessage$|^DSCallID\d+$/.test(e.name));
  }

  /**
   * Navigate by the app's own navigation, supplying the extra props a screen requires.
   *
   * Some screens are only reachable with context (`DriverDetails` needs `{ context: ['Staff', id] }`,
   * per CalendarUtils.js:627-628; entering it bare makes the view call `DS.getValue(undefined, ...)` and
   * throw inside `DataStoreJSImpl.contextToID`). Using the navigation module's `set` - which is what the
   * game's own callers use - reaches them correctly instead of failing.
   */
  async goto(screenName, extraProps = null) {
    if (!this.host) return false;
    const app = APPS[this.app];
    const nav = readModule(this.host, app.nav);
    if (nav && typeof nav.setDefault === 'function') {
      try { nav.setDefault(-1); } catch (_) {}
    }
    const eng = readModule(this.host, 'common/core/Engine');
    const before = this.host.document.body.childNodes.length;

    let dispatched = false;
    if (extraProps && nav && typeof nav.set === 'function') {
      // `set` takes a numeric id; the screen event takes a name. Resolve the id from the enum.
      const enumNs = readModule(this.host, app.enumModule || '');
      const id = enumNs && enumNs.ScreenID ? enumNs.ScreenID[screenName] : undefined;
      if (typeof id === 'number') {
        try { nav.set(id, extraProps); dispatched = true; } catch (_) { dispatched = false; }
      }
    }
    if (!dispatched) {
      try { eng.sendEvent(app.screenEvent, screenName); } catch (_) { return false; }
    }
    for (let i = 0; i < 40; i++) {
      await this.host.tick(25);
      if (this.host.document.body.childNodes.length !== before) break;
    }
    return true;
  }

  /** Press the first clickable whose text matches any of `needles` (case-insensitive). */
  async pressByText(needles, settleMs) {
    const list = Array.isArray(needles) ? needles : [needles];
    const el = clickable(this.host.document.body).find((n) => matchesLabel(n, list));
    if (!el) return null;
    await press(this.host, el, settleMs);
    return el;
  }

  /** Press the first clickable whose class list contains `cls`. */
  async pressByClass(cls, settleMs) {
    const el = clickable(this.host.document.body).find((n) => classHas(n, cls));
    if (!el) return null;
    await press(this.host, el, settleMs);
    return el;
  }

  /**
   * Apply one parsed command ({event, args}, from coop_client's planCommand).
   *
   * Returns { via, detail }. `via:'ui'` only when the expected engine event was actually observed.
   *
   * Wrapped in crash containment on purpose. Several career views index generated data that the
   * export's schemas do not fully populate - `ContinueButtonContainer` needs
   * `Calendar.UpcomingEvents.<day>.Events.<i>` and `DashboardEventPanel` needs `filterCategory` on each
   * event (CalendarUtils.js:777). Those throw *inside* preact's deferred flush, which would otherwise
   * take down the whole client process on an unrelated command. A crash is reported as the reason the
   * operation failed, and the process survives to serve the next command.
   */
  async apply(plan) {
    const bound = { error: null };
    const onCrash = (e) => { bound.error = bound.error || ((e && e.message) || String(e)); };
    process.on('uncaughtException', onCrash);
    process.on('unhandledRejection', onCrash);
    try {
      const r = await this._apply(plan);
      if (r.via !== 'ui' && bound.error) {
        return { via: r.via, detail: `${r.detail} (also threw: ${bound.error})` };
      }
      return r;
    } catch (e) {
      return { via: 'failed', detail: `threw while driving: ${(e && e.message) || e}` };
    } finally {
      process.removeListener('uncaughtException', onCrash);
      process.removeListener('unhandledRejection', onCrash);
    }
  }

  async _apply(plan) {
    const ev = plan.event;
    const arg = (plan.args && plan.args[0]) || '';

    // ---- Continue: press the continue button on the front-end -------------------------------
    if (ev === 'ContinueGame') {
      const opened = await this.open('Frontend');
      if (!opened.ok) return { via: 'failed', detail: opened.error };
      const since = this.sent.length;
      const el = await this.pressByClass('FrontendContinueButton_root', 600);
      const got = this.since(since).find((e) => e.name === 'ContinueGame');
      if (got) return { via: 'ui', detail: 'pressed FrontendContinueButton' };
      if (!el) return this.synthetic(ev, since, 'no continue button on screen (no save present?)');
      return { via: 'failed', detail: 'pressed the button but ContinueGame was not emitted' };
    }

    // ---- LoadGame <slot>: open the save list, pick the slot, let the view send the event ------
    if (ev === 'LoadGame') {
      // Fresh boot, always. The previous command may have been ContinueGame, which navigates the app
      // away from the menu - reusing that host meant the save list was never on screen and the press
      // fell back to synthetic. Measured: 11 s boot is cheaper than a wrong answer.
      const opened = await this.open('Frontend', { forceFresh: true });
      if (!opened.ok) return { via: 'failed', detail: opened.error };

      // Get to the save list. "Load Career" opens it; accept either the translated label or the raw
      // symbol, since which one is on screen depends on localisation timing.
      let onLoad = await this.pressByText(['load career', 'LOAD_CAREER'], 900);
      if (!onLoad) {
        await this.goto('Load');
      }
      await this.host.tick(900);

      const since = this.sent.length;
      // Slot rows are `DescriptionButton_root` elements (verified by walking the rendered save list:
      // 31 clickables, the slot rows among them). Prefer one whose text names the requested slot,
      // else the first row - which is autoSave, the slot `ContinueGame` uses too.
      const rows = clickable(this.host.document.body).filter((n) => classHas(n, 'DescriptionButton_root'));
      let pressed = null;
      if (arg) pressed = rows.find((n) => textOf(n).toLowerCase().includes(arg.toLowerCase())) || null;
      if (!pressed) pressed = rows[0] || null;
      if (pressed) await press(this.host, pressed, 700);

      // Selecting a row raises the info/confirmation panel; press its load button. Accept the
      // translated label and the raw symbol, since the panel populates before loc settles.
      await this.pressByText(['load game', 'LOADSAVE_LOADGAME'], 800);

      const got = this.since(since).find((e) => e.name === 'LoadGame');
      if (got) return { via: 'ui', detail: `LoadGame(${JSON.stringify(got.args[0])}) from the save list` };
      return this.synthetic(ev, since,
        pressed ? 'selected a slot but the view did not emit LoadGame' : 'no slot row on the save list');
    }

    // ---- AdvanceToNextEvent: press the continue/advance button in the career UI --------------
    if (ev === 'AdvanceToNextEvent') {
      const opened = await this.open(this.opts.page || 'TeamManagement');
      if (!opened.ok) return { via: 'failed', detail: opened.error };
      const since = this.sent.length;
      const el = await this.pressByClass('ContinueButton_root', 800);
      const got = this.since(since).find((e) => e.name === 'AdvanceToNextEvent');
      if (got) return { via: 'ui', detail: 'pressed ContinueButton' };
      return this.synthetic(ev, since,
        el ? 'pressed the advance button but AdvanceToNextEvent was not emitted' : 'no advance button on screen');
    }

    // ---- NextMonthSelected / PreviousMonthSelected: press the calendar arrows -----------------
    if (ev === 'NextMonthSelected' || ev === 'PreviousMonthSelected') {
      const opened = await this.open('TeamManagement');
      if (!opened.ok) return { via: 'failed', detail: opened.error };
      const since = this.sent.length;
      // The arrows are icon buttons with no stable text; the reliable route is the calendar screen
      // and its month controls, so navigate there first.
      await this.goto('Calendar');
      await this.host.tick(600);
      const dir = ev === 'NextMonthSelected' ? ['next', '›', '>'] : ['prev', '‹', '<'];
      let pressed = null;
      for (const needle of dir) { pressed = await this.pressByText(needle, 400); if (pressed) break; }
      const got = this.since(since).find((e) => e.name === ev);
      if (got) return { via: 'ui', detail: `pressed the ${ev === 'NextMonthSelected' ? 'next' : 'previous'} month control` };
      return this.synthetic(ev, since, pressed ? 'pressed a control but the event did not fire' : 'month control not identified');
    }

    // ---- Anything else: no UI mapping is known, so send it and say so -------------------------
    return this.synthetic(ev, this.sent.length, `no UI mapping for ${ev}`);
  }

  /**
   * Fall back to engine.sendEvent.
   *
   * This is deliberately labelled: a direct send proves the *event plumbing* works and nothing more.
   * It does not prove a player could have performed the action, which is the whole point of driving
   * the UI. Reporting it as a normal success would be the same dishonesty as `--mode=none` pretending.
   */
  synthetic(event, since, why) {
    if (!this.host) return { via: 'failed', detail: `${why}; no page open` };
    const before = this.since(since).length;
    const eng = readModule(this.host, 'common/core/Engine');
    try {
      eng.sendEvent(event);
    } catch (e) {
      return { via: 'failed', detail: `${why}; sendEvent threw: ${e && e.message}` };
    }
    const after = this.since(since);
    if (after.length > before) {
      return { via: 'synthetic', detail: `${why}; sent directly, UI did not agree it was actionable` };
    }
    return { via: 'failed', detail: `${why}; sendEvent emitted nothing observable` };
  }

  /** Everything the driver saw, for a report. */
  snapshot() {
    return { app: this.app, events: this.sent.map((e) => e.name), notes: this.notes };
  }
}

module.exports = { UiDriver, clickable, press, textOf };
