'use strict';
/*
 * bridge_guard_test.js - the page guard must cover the *event*, not just the command word.
 *
 * Firing `LoadGame` or `ContinueGame` from an in-game document tears the front-end down and never rebuilds
 * it: a black screen with the process still alive. `bridge.js` therefore refuses those commands anywhere but
 * `Frontend.html`, via `allowedFor`.
 *
 * That check matches on the command a person typed - and the `Raw <event> [args]` command exists to dispatch
 * an arbitrary Gameface event by name. It resolves that name to `eventName` *after* the command check runs, so
 *
 *     Raw LoadGame autosave      -- sent from RaceWeekend.html
 *
 * passed `allowedFor` (cmd is 'raw', which is legal everywhere) and reached `engine.trigger` with the one
 * event known to black-screen the process. The guard was on the spelling of a command, not on what was
 * dispatched, so the documented protection did not actually cover the obvious bypass.
 *
 * These tests extract the real functions from `bridge.js` and call them, rather than restating the rule: a
 * copy would keep passing after the source regressed, which is the failure this is meant to catch.
 */

const fs = require('fs');
const path = require('path');

const BRIDGE = path.join(__dirname, '..', '..', 'pak', 'f1manager24', 'Content', 'UIGameface', 'js', 'coop', 'bridge.js');
const src = fs.readFileSync(BRIDGE, 'utf8');

const results = [];
function ok(cond, name, detail) {
  results.push({ ok: !!cond, name });
  console.log(`  ${cond ? 'ok  ' : 'FAIL'}  ${name}${cond ? '' : '  -- ' + (detail === undefined ? '' : detail)}`);
}

/**
 * Evaluate a top-level `function name(...) { ... }` from the bridge source in its own scope.
 *
 * The guard functions close over module-level constants (`LOAD_FROM`, `MENU_FROM`,
 * `FRONTEND_ONLY_EVENTS`) and nothing else, so those are lifted alongside them. If the source changes shape
 * and a name is missing, this throws rather than silently testing nothing.
 */
function lift(names) {
  const consts = ['LOAD_FROM', 'MENU_FROM', 'FRONTEND_ONLY_EVENTS']
    .map((n) => {
      const m = src.match(new RegExp('^\\s*var ' + n + '\\s*=\\s*([^;]+);', 'm'));
      if (!m) throw new Error('bridge.js no longer defines ' + n + ' - the guard moved, update this test');
      return `var ${n} = ${m[1]};`;
    })
    .join('\n');

  const fns = names
    .map((n) => {
      // Non-greedy to the first line that closes the function at column 4.
      const m = src.match(new RegExp('(function ' + n + '\\([^)]*\\) \\{[\\s\\S]*?\\n    \\})'));
      if (!m) throw new Error('bridge.js no longer defines ' + n + '() - the guard moved, update this test');
      return m[1];
    })
    .join('\n');

  // eslint-disable-next-line no-new-func
  return new Function(`${consts}\n${fns}\nreturn { ${names.join(', ')} };`)();
}

const g = lift(['allowedFor', 'eventAllowedOn']);

// ---- the rule itself ---------------------------------------------------------------------------
ok(g.allowedFor('loadgame', 'Frontend') === true, 'LoadGame is permitted on the front end');
ok(g.allowedFor('loadgame', 'RaceWeekend') === false, 'LoadGame is refused on RaceWeekend');
ok(g.allowedFor('continuegame', 'TeamManagement') === false, 'ContinueGame is refused on TeamManagement');

// ---- the bypass: the event name, whatever command produced it -----------------------------------
// This is the case that used to get through. `raw` is legal on every page, so only the event-level guard
// can stop it.
ok(g.allowedFor('raw', 'RaceWeekend') === true,
  'Raw is a legal command everywhere (which is why the command check alone is not enough)');
ok(g.eventAllowedOn('LoadGame', 'RaceWeekend') === false,
  'Raw LoadGame is refused on RaceWeekend even though the raw command is legal',
  'the event-name guard must catch what the command guard cannot');
ok(g.eventAllowedOn('LoadGame', 'TeamManagement') === false,
  'Raw LoadGame is refused on TeamManagement - the page that actually black-screened a live client');
ok(g.eventAllowedOn('ContinueGame', 'RaceWeekend') === false,
  'Raw ContinueGame is refused on RaceWeekend');
ok(g.eventAllowedOn('LoadGame', 'Frontend') === true,
  'Raw LoadGame is still permitted on the front end, where the real load path lives');

// ---- and it must not over-refuse ----------------------------------------------------------------
// A guard that blocks everything is not a guard; the mod has to keep working where it is legitimate.
for (const ev of ['AdvanceToNextEvent', 'NextMonthSelected', 'SimPauseChange', 'QuitGame']) {
  ok(g.eventAllowedOn(ev, 'RaceWeekend') === true, `${ev} still fires from RaceWeekend`);
}
ok(g.eventAllowedOn('AdvanceToNextEvent', 'TeamManagement') === true,
  'AdvanceToNextEvent still fires from TeamManagement');

// ---- the guard must be wired at the dispatch point ----------------------------------------------
// The functions above can be correct and still never be called. This is the failure mode that matters:
// `allowedFor` existed and was called, yet the hole was open because nothing checked the resolved name.
const dispatchStart = src.indexOf('function dispatch(text)');
ok(dispatchStart > 0, 'dispatch() is still present in bridge.js');
// Slice to the end of the function, not a fixed length: a window that is too short returns -1 for the
// trigger and makes the ordering comparison vacuous.
const dispatchEnd = src.indexOf('\n    function report(', dispatchStart);
const dispatchBody = src.slice(dispatchStart, dispatchEnd > dispatchStart ? dispatchEnd : src.length);
ok(dispatchBody.includes('eventAllowedOn('),
  'dispatch() must call eventAllowedOn before triggering',
  'an unwired guard leaves the Raw bypass open');

const guardAt = dispatchBody.indexOf('eventAllowedOn(');
const fireAt = dispatchBody.indexOf('engine.trigger.apply');
ok(fireAt > 0, 'dispatch() still triggers through engine.trigger.apply',
  'if the trigger moved, this ordering check proves nothing');
ok(guardAt > 0 && guardAt < fireAt,
  'the event guard must run before the trigger, not after it',
  `guard at ${guardAt}, trigger at ${fireAt}`);

// The press path returns early, so it must sit *after* the guard or it becomes a second way to bypass it.
const pressAt = dispatchBody.indexOf('if (P && viaPress');
ok(pressAt > 0 && guardAt < pressAt,
  'the guard must also precede the control-press path, which returns before reaching the trigger',
  `guard at ${guardAt}, press at ${pressAt}`);

// ---- the pages named in the message must be the pages actually allowed -------------------------
// A refusal that names the wrong page sends the operator somewhere the command still fails.
const loadPages = (src.match(/var LOAD_FROM = (\[[^\]]*\])/) || [])[1];
ok(loadPages === "['Frontend']",
  'LoadGame is allowed only on Frontend, matching where the game itself issues it',
  String(loadPages));

// ---- `Advance <n>` must send exactly n events, not n+1 ----------------------------------------
// The loop sends extras and then the tail of dispatch() sends one more through the preferred route. The
// loop used to run `i < times` starting at 0, so `Advance 3` fired four AdvanceToNextEvent. On the calendar
// that is not harmless: an extra advance moves the career past an event, skipping a race weekend for both
// players in a shared session.
const advanceBlock = (dispatchBody.match(/cmd === 'advance'\)[\s\S]*?eventName = 'AdvanceToNextEvent';/) || [''])[0];
ok(advanceBlock.length > 0, 'the advance branch is still recognisable', 'update this test if it was renamed');
ok(/i < times - 1/.test(advanceBlock),
  'the loop must send times-1, because the tail sends the last one',
  'with `i < times` the total is n+1');
ok(/\(n \+ 1\) \* 1200/.test(advanceBlock),
  'the spaced extras must not start at delay 0, or they collide with the tail trigger',
  'delay 0 races the final dispatch');

const failed = results.filter((r) => !r.ok).length;
// The runner parses `N/M passed` from the summary line, so keep that exact shape: an extra word between the
// counts and "passed" makes a passing suite look empty, which is how this suite first reported FAIL while
// printing its assertions green.
console.log(`\n  ${results.length - failed}/${results.length} passed`);
if (failed) {
  console.log(`  ${failed} failed`);
  process.exit(1);
}
