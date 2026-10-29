# Real client: pressing the UI instead of firing events

The bridge used to do one thing: `engine.trigger(name)`. That is the same call the game's buttons make,
so it worked â€” but it skipped everything the buttons do *around* the call, which meant it could succeed
where a player could not, and fail where a player would not.

This note covers the change that makes the in-game client press the actual control, and the one case
where that press cannot be verified outside the game.

## What changed

| file | change |
|---|---|
| `pak/.../js/coop/press.js` | **new.** Finds the real control and presses it. No engine, no network, DOM only. |
| `pak/.../js/coop/bridge.js` | `continue`, `load`, `advance` and `nextmonth` now route through a press when the module is present; the direct trigger remains as a fallback. |
| `pak/.../UIGameface/*.html` | loads `press.js` before `bridge.js`, on all four pages. |
| `tools/deploy_pak.py` | `press.js` added to the syntax check and to `CRITICAL` (the files whose staleness is checked after deploy). |
| `tools/headless/press_test.js` | **new.** 14 assertions loading the shipped module against the real UI. |

## Why pressing is better than firing the name

Three concrete reasons, each verified in the export or in the headless harness:

1. **The Advance button is a state machine, not one event.** `ContinueButtonContainer.onSelect` switches
   on `state.advanceType` and emits different things â€” `AdvanceToNextEvent`, a navigation to the Inbox,
   a navigation to a sponsor screen. Firing `AdvanceToNextEvent` by name asserts a state the view may
   not be in. That is why `advancen` existed: hammering the event N times until one landed. Pressing the
   control lets the view decide.

2. **`LoadGame` is not legal before a slot is selected.** `LoadSaveScreen.js:128` fires it from
   `onSelectLoadData`, which only runs after a row is chosen and the confirm panel is up. Firing the
   event by name loads a slot the UI never agreed to. `press.js` walks the real sequence: menu â†’
   `DescriptionButton_root` row â†’ confirm.

3. **A press proves the action was available.** If the control is not on screen, or is rendered
   `disabled`/`blocked`, `press.js` returns a miss and the bridge reports an error instead of claiming
   an event nobody could have emitted.

## The two things that make a synthetic press work

Both were measured, not assumed:

- **`mousedown` before `click`.** `ButtonBehaviourT.onClick` returns early unless `this.state.down` is
  true (`ButtonBehaviourT.js:119-120`), and only `onMouseDown` sets it (`:131`). A bare `click` sends
  nothing at all â€” every button on the main menu was inert until the mousedown was added.
- **A gap between them (~35 ms).** preact defers its render flush through a promise (`preact.js:6`), so
  the click can be processed before `state.down` lands. At 0 ms the press fails; at 30 ms it works.

## What is verified, and how

`node tools/headless/press_test.js` loads the **exact file that goes into the pak** into the headless
render host and drives real components:

```
25 passed, 0 failed          (stable across 3 consecutive runs)
  ok  module installs window.__coopPress
  ok  uses a non-zero mousedown->click gap
  ok  finds the menu controls
  ok  pressing continue emitted ContinueGame
  ok  the sequence emitted LoadGame
  ok  LoadGame carried a slot name
  ok  a disabled advance button is refused
  ok  load menu control found (text raw)
  ok  race weekend exposes an advance control at boot
  ok  SessionOverview advance emitted SessionOverviewClosed
  ok  SessionResults advance emitted AdvanceToNextStage
  ok  the race-start confirm appeared on PreSession
  ok  PreSession advance emitted PreSessionClosed (the pre-session confirm)
```

Pak build, via the existing deploy tool (no game install touched):

```
python tools/deploy_pak.py --no-deploy --host-only
1. syntax                   ok
2. menu integration tests   29 checks, 0 failed
3. build                    built CoopBridge_P.pak (85213 bytes)
4. verify pak contents      ok, the JS inside the pak is byte-identical to the source
```

## Verified: the race-weekend advance press

This is the part that matters for co-op, and it *is* verifiable headless because the race-weekend
handlers emit unconditionally â€” no `isCoherentPlayer` guard:

| screen | press result |
|---|---|
| `SessionOverview` | `SessionOverviewClosed` + `RaceViewChanged` (`SessionOverview.js:147-152`) |
| `SessionResults` | `AdvanceToNextStage` (`SessionResults.js:17-19`) |
| `PreSession` | **`PreSessionClosed`** â€” the race-start confirm (`PresessionView.js:119, :242-252`) |

So the client can now walk a race weekend by pressing, not by naming events. `AdvanceToNextStage` and
`PreSessionClosed` were previously only reachable by firing them blind.

### Getting the race-start confirm to appear at all

This took four separate gates, each found by a throw or a wrong-looking screen, not by reading a schema.
`seedDefaults()` in `press.js` clears them, and the reason it is a list of specific keys rather than a
"fill everything" sweep is that a broad fill broke the TeamManagement dashboard instead (its own event
sorter throws on a hand-built event â€” `CalendarUtils.js:777`).

| gate | what it hides | where |
|---|---|---|
| `RaceSim.ERSData[i].maxEnergyStored` null | ERS panels subtract it and throw, aborting the render and thus any press | `RaceWeekendUtil.js:187-199` |
| `Parts.missingPartsCar1/2` | renders a `[CONTINUE]` car-components button instead of the race-start confirm | `PresessionView.js:51-54, :126-128` |
| `Parts.Types.<t>.installedOnCar<n>Item` null | part lookup returns null, `isPowertrainFunctional_DS` false, same wrong button | `RaceWeekendUtil.js` |
| `Parts.powertrainPartViolatesPenaltyCar1/2` | confirm renders, but pressing opens a penalty dialog and emits nothing | `PresessionView.js:243-251` |

That last one is the subtle one: the press reported success while emitting nothing, because the handler
legitimately routed to a dialog. It only showed up by comparing emitted events against the expected one.

## What is NOT verified: the career advance

The career `ContinueButtonContainer` is the one press that cannot be confirmed outside the game.

`ContinueButtonContainer.onSelect` branches on the engine mode **before** anything else:

```js
if (Engine.isCoherentPlayer()) { DS.setValue(['MainHud'], 'advancing', true); return; }
...
switch (this.state.advanceType) { ... Engine.sendEvent('AdvanceToNextEvent') ... }
```

- In the **headless harness**, `isCoherentPlayer()` is `true` (that is the preview path the whole harness
  runs in). Pressing Advance sets `MainHud.advancing` to true and returns **without emitting anything** â€”
  measured: `advancing` goes `null â†’ true`, no event sent.
- In the **shipped game** it is `false`, so the same press runs the switch and can emit
  `AdvanceToNextEvent`.

The press plumbing is identical and proven elsewhere, so this is a property of that one view. Confirming
it needs the game running. Do not treat "career advance works" as established until it has been seen live.

A related finding from the same file: the career advance button carries `disabled` when
`getAdvanceTypeScreen()` equals the screen you are already on (`ContinueButtonContainer.js:73-75`), and
when a tutorial objective is active (`:76-78`). In a fresh career it is legitimately disabled until the
tutorial gate clears â€” a press reporting a miss there is correct, not broken.

## Deploying

The pak in the host install is currently **stale** (80,149 bytes installed, 77,114 built), which the
tool reports and refuses to call success:

```
3. installed paks
  FAIL host pak is NOT the one just built (installed 80149 bytes, built 85213 bytes)
```

That is the exact failure documented in `UI-MENU-INTEGRATION.md` â€” a stale pak once wasted several
cycles because the UI change was in the build and in none of the running game. Deploying writes into the
game install, so it is left to the operator:

```
python tools/deploy_pak.py            # build, verify, deploy to host + second machine
python tools/deploy_pak.py --check    # verify only, deploy nothing
```

The game must be closed: the pak is held open by the running process.

## Next steps this unblocks

1. **Deploy, then verify the career advance in-game.** That press is the one that cannot be proven
   headless, and `AdvanceToNextEvent` is what moves a career forward.
2. **Check whether `PreSessionClosed` actually starts the sim.** This is the open question from
   `UI-COMMANDS.md` §3 and the reason all of this exists. The press now fires that event reliably
   (verified above); what is still unproven is what the engine does on receipt. That needs the game.
3. **Extend `SPECS` for pit and strategy controls.** `RaceWeekendScreens.PitOptions` and the strategy
   screens render `Button_root`/`SimpleButton_root` controls; these are where a client's own car
   decisions live, and they are presses rather than events.
4. **Feed the press path from the co-op link.** `press.js` exposes `window.__coopPress`, so `lobby.js`
   can drive the UI directly rather than only through the loopback command server.