# UI-COMMANDS.md — F1 Manager 2024 Gameface command surface

Read-only map of the Coherent Gameface UI export of **F1 Manager 2024** (UE 5.1,
Frontier Developments, Steam appid 2591280), produced to establish which named events the
UI can raise and, specifically, whether any of them starts a race session.

Evidence examined (all read-only):

```
E:\FModel\Output\Exports\F1Manager24\Content\UIGameface\js\   1,448 .js files   (the UI)
E:\Steam\steamapps\common\F1 Manager 2024\F1Manager24\Binaries\Win64\
    F1Manager24.exe                 553,445,376 bytes   (string + binding evidence)
    Mappings.usmap                  1,698,349 bytes     (41,679 UE name pool entries)
```

### Game identity (pinned)

| field | value |
|---|---|
| Steam appid | `2591280` |
| Steam build id | `17356935` |
| file version | `1.11.0.39891` |
| SHA-256 | `7ffe404f37a813c326cd0635001e388715e84fe945093dc802e388897b2050f6` |
| size | 553,445,376 bytes |
| PE timestamp | `0x67AE45F7` (2025-02-13 19:20:23 UTC) |
| machine / format | x64, PE32+, 19 sections |
| CompanyName | Frontier Developments plc. |

**How to read this document.** Every statement is separated into one of three kinds:

| tag | meaning |
|---|---|
| *(no tag)* | **Proven from the export text.** The UI code at the cited `file:line` does exactly this. |
| **binary** | **Proven from the shipped `F1Manager24.exe`.** The named string exists at the cited file offset. |
| **inferred** | A conclusion drawn from UI-side and/or binary string evidence, without reading the handler code. |
| **unknown** | Neither the export nor the binary strings can answer it. |

The export proves what the *UI sends*. The binary proves which names the game has compiled in.
Neither proves what a handler *does* — §3.5 and §5 keep that distinction explicit.

---

## 1. Mechanism

### 1.1 The chain from a click to an engine call

A mouse click or gamepad/keyboard `Select` reaches `ButtonBehaviourT.performSelect`:

- [`js/common/components/ButtonBehaviourT.js:70`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L70) — `performSelect()` definition.
- Entry points, all funnelling into it:
  - [`ButtonBehaviourT.js:66`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L66) — keyboard/gamepad `handleInput` → `return this.performSelect();`
  - [`ButtonBehaviourT.js:123`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L123) — mouse `onClick`
  - [`ButtonBehaviourT.js:138`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L138) — mouse `onMouseDown` when `selectOnPress` is set

Inside `performSelect`, the `onSelect` prop is invoked through a thin wrapper:

- [`ButtonBehaviourT.js:86`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L86) — `else if (onSelect) {`
- [`ButtonBehaviourT.js:95`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L95) — `this.onSelect();`
- [`ButtonBehaviourT.js:102-103`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/ButtonBehaviourT.js#L102) — `onSelect() { this.props.onSelect?.(this.props.userData); }`

Every concrete button class used by the game derives from `ButtonBehaviourT`:

- [`js/project/components/ContinueButton.js:7`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButton.js#L7) — `class _ContinueButton extends ButtonBehaviourT.Component`
- [`js/project/components/SimpleButton.js:12`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/SimpleButton.js#L12) — `class _SimpleButton extends ButtonBehaviourT.Component`

The `onSelect` handler supplied by the screen then calls the event helper:

- [`js/common/core/Engine.js:54-56`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L54):

```js
function sendEvent(eventName, ...args) {
    console.log('Engine.sendEvent() ' + eventName + ', ' + args);
    engine.trigger(eventName, ...args);
}
```

So the full chain is:

```
click / Select
  → ButtonBehaviourT.performSelect          ButtonBehaviourT.js:70
  → this.props.onSelect(userData)           ButtonBehaviourT.js:103
  → screen's own handler (e.g. onSelect)
  → Engine.sendEvent(name, ...args)         Engine.js:54
  → engine.trigger(name, ...args)           Engine.js:56
```

`engine.trigger` is the Coherent/Gameface bridge into the native game. Nothing in the export
defines it; it comes from `js/common/lib/cohtml.js` (Coherent's runtime shim).

### 1.2 Is `engine.trigger` the single choke point?

**For UI-originated *events*, yes — with one sibling and three narrow exceptions.**

`js/common/core/Engine.js` contains every in-game call into the engine, on two lines:

| line | content |
|---|---|
| [`Engine.js:51`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L51) | `engine.trigger(name, ...args);` — body of `triggerListener(name, ...args)` (`Engine.js:50`) |
| [`Engine.js:56`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L56) | `engine.trigger(eventName, ...args);` — body of `sendEvent` |

A full scan of the export finds **exactly 5** `engine.trigger(` call sites, and all 5 are in files
under `common/`:

| file:line | context | notes |
|---|---|---|
| [`common/core/Engine.js:51`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L51) | `triggerListener` | sibling of `sendEvent`, same destination |
| [`common/core/Engine.js:56`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L56) | `sendEvent` | the main path |
| [`common/lib/cohtml.js:249`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/lib/cohtml.js#L249) | Coherent runtime internals | `engine.trigger('Error', message)` |
| [`common/lib/cohtml.js:267`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/lib/cohtml.js#L267) | Coherent runtime internals | `engine.trigger('Ready')` |
| [`common/lib/cohtml.js:273`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/lib/cohtml.js#L273) | Coherent runtime internals | `engine.trigger('Ready')` |

**Consequence for the bridge.** Wrapping `engine.trigger` intercepts *every* UI→engine event,
including `triggerListener` calls (which is how all audio events travel) and Coherent's own
`Ready`/`Error`. A wrapper therefore needs no knowledge of `sendEvent` at all. It also cannot see
game→UI traffic: that arrives via `engine.on(...)`, i.e. `Engine.addListener` at
[`Engine.js:42-43`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L42).

### 1.3 Places that bypass `engine.trigger`

Three distinct bypass surfaces exist. None of them is an event.

**(a) `engine.call` — function call, not an event.** Two sites only.

- [`Engine.js:59-64`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L59):

```js
function callBoundFunction(functionName, ...args) {
    if (!_isCoherentPlayer) {
        return engine.call(functionName, ...args);
    }
    return new Promise((resolve) => { resolve(null); });
}
```

- [`Engine.js:61`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L61) — `return engine.call(functionName, ...args);`
- [`common/core/Player.js:17`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Player.js#L17) — the other raw `engine.call`, `engine.call('GetGameOwner')`

`callBoundFunction` is the *only* route from JS into a native function by name, and it is used for
infrastructure, not gameplay. All 20 call sites:

`GetCompositeImageIDForBlurredBackground`, `RegisterCompositeRect`, `DeregisterCompositeRect`,
`UpdateCompositeRectSortOrder`, `UpdateCompositeRectColour`, `UpdateCompositeRectCoords`
([`BlurredBackground.js:28-78`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/components/BlurredBackground.js#L28)),
`GetDataStore` ([`DataStore.js:231`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/DataStore.js#L231)),
`LaunchOSK` ([`Input.js:36`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Input.js#L36)),
`CreateGlobalControlContext` / `CreatePlayerControlContext` / `DestroyControlContext`
([`Input.js:243-313`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Input.js#L243)),
`GetLocalisation` ([`Localisation.js:75`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Localisation.js#L75)),
`SetGameOwnerChangedCallback` ([`Player.js:25`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Player.js#L25)),
`GetProfiler` ([`Profile.js:129`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Profile.js#L129)),
`GetSystemInformation` ([`System.js:161`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/System.js#L161)),
`IsRunningInEngine` ([`Engine.js:123`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L123)).

**Notably there is no `callBoundFunction` anywhere in `project/modules/raceWeekend/`.**
No session start, no advance, no simulation runs through it.

**(b) Direct `DS.setValue` — DataStore write, not an event.** 220 call sites in the export.
These write the same DataStore the native game reads/writes, so a write can change game-visible
state without any event.

- Definition: [`DataStore.js:166-169`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/DataStore.js#L166)

```js
function setValue(context, propertyName, value) {
    Assert.isCoherentPlayerAssert();
    _impl.SetValue(context, propertyName, value);
}
```

- The assert ([`Assert.js:40-45`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Assert.js#L40)) is `if (Engine.isCoherentPlayer()) return true; ... throw 'Assertion failure. Is not running in Coherent Player'` (`:44`).
- **Therefore in the shipped game (where `isCoherentPlayer()` is false, §1.4) *every*
  `DS.setValue` throws `Assertion failure. Is not running in Coherent Player`.** Every
  `if (Engine.isCoherentPlayer()) { DS.setValue(...) }` block is a **preview-harness-only** path,
  not a shipped-game path. This matters enormously for §3.

Direct `DS.setValue` on gameplay state that is *not* guarded (i.e. genuinely runs in the game) is
rare. Non-debug, non-`isCoherentPlayer`-guarded writes found:

| file:line | write |
|---|---|
| [`project/modules/scouting/compare/ComparisonUtil.js:93-96`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/scouting/compare/ComparisonUtil.js#L93) | `DS.setValue(viewHistoryContext..., 'staffID', ...)` — UI view history only |
| [`project/modules/teamManagement/TeamManagementModule.js:237-255`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TeamManagementModule.js#L237) | debug keyboard shortcuts (`KeyQ`/`KeyW`/`KeyA`/`KeyS`) — inside `onKeyDown` |
| [`project/modules/raceWeekend/RaceWeekendModule.js:85-103`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L85) | debug keyboard shortcuts (`Shift+1..8`) setting `Weekend.currentStage` |
| [`project/utils/AccentColorUtil.js:39`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/utils/AccentColorUtil.js#L39) | `UI.themeIndex` — cosmetic |

The `RaceWeekendModule` debug keys are the one genuinely interesting unguarded write: they mutate
`['Weekend'].currentStage` directly. But they are bound behind
`if (Engine.isCoherentPlayer()) addEventListener(...)` at
[`RaceWeekendModule.js:30-31`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L30), so they are also dead in the shipped game.

**(c) `Engine.triggerListener` — same destination, different wrapper.** Used by the audio system
only ([`Audio.js:56`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Audio.js#L56)) and by the JS DataStore mock
([`DataStoreJSImpl.js`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/debug/DataStoreJSImpl.js)). A wrapper on
`engine.trigger` catches these too.

### 1.4 A trap: `isCoherentPlayer()` is **not** "running in the game"

The name is misleading and the polarity matters. In
[`Engine.js:121-133`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Engine.js#L121):

```js
engine.whenReady.then(() => {
    callBoundFunction(SystemConfig.getIsRunningInEngineFunctionName()).then((IsRunningInEngine) => {
        _isCoherentPlayer = false;      // Engine.js:125
        ...
    }).catch((reason) => {
        _isCoherentPlayer = true;       // Engine.js:129
        ...
    });
});
```

`getIsRunningInEngineFunctionName()` returns `'IsRunningInEngine'`
([`SystemConfig.js:13-20`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/config/SystemConfig.js#L13)).
So:

- `callBoundFunction('IsRunningInEngine')` **resolves** (running in the game) → `_isCoherentPlayer = false`.
- it **rejects** (not running in the game, i.e. Coherent's standalone preview player) → `_isCoherentPlayer = true`.

Corroborating evidence that `true` means "preview harness":

- [`DataStore.js:206-214`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/DataStore.js#L206) swaps in the **JS mock** DataStore (`common/debug/DataStoreJSImpl`) when `isCoherentPlayer()` is true, and the real `CppImpl` otherwise.
- [`RaceWeekendApp.js:37-40`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/apps/RaceWeekendApp.js#L37) loads `DebugFlagsManager`, `DebugPitStopManager`, `DebugPreSessionManager` only when it is true.
- [`RaceWeekendModule.js:58-59`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L58) renders `DebugRaceWeekendViewManager` only when it is true.

**Reading rule for the rest of this document:** in the shipped game `isCoherentPlayer()` is
`false`, so every `if (Engine.isCoherentPlayer()) {...}` block is dead code and every `else`
branch is the live one. Several UI functions are therefore *wrong* to read at face value — most
importantly `ContinueButtonContainer.onSelect` (see §3.3).

---

## 2. Complete event table

### 2.1 Count, and correcting the brief's expectation of ~103

The enumeration command in the brief is:

```powershell
Get-ChildItem '...\UIGameface\js' -Recurse -File -Filter *.js |
  Select-String -Pattern "sendEvent\('([A-Za-z0-9_]+)'" -AllMatches | ...
```

That returns **103** distinct strings — confirmed. But it **misses 251 of the game's real events**,
because the TypeScript compiler emits most calls in the indirect form

```js
(0, Engine_1.sendEvent)('LoadGame', slotName);
```

`sendEvent('` does not appear in that text. In this export **381 of 517** `Engine.sendEvent` call
sites use the compiled form, and 14 use double quotes instead of single.

A correct enumeration must also (a) allow the `)` before `(` and (b) exclude the *audio* helper,
which is a different function with the same name. Final numbers:

| measurement | count |
|---|---|
| `Engine.sendEvent(<literal>)` call sites | **517** |
| **distinct engine event names** | **352** |
| `Audio.sendEvent(...)` call sites (not engine events) | 37 |
| distinct audio strings | 9 (`begin_simulating`, `hold`, `select`, `show`, `toggleOn`, `valueUp`, `valueDown`, `show_Toast`, `hide_Toast`) |

The 103-name list from the brief is a **subset**: 101 of those 103 are engine events
(the other 2, `show_Toast` and `hide_Toast`, are `Audio.sendEvent` calls in
[`NotificationsService.js:28,38`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/utils/NotificationsService.js#L28)).
**252 engine events are missing from the 103-name list**, and they include exactly the
race-weekend machinery this research was asked about: `AdvanceToNextStage`,
`SessionOverviewClosed`, `PreSessionClosed`, `SubmitActiveSessionSetup`, `SimulateSingleSession`,
`SimulateRemainingEvent`, `SimulateToRaceOrSprint`, `ShowSimulationResults`,
`RaceWeekendModuleReady`, `OnStageBackwards`, `SkipCinematics`, `SetPlaybackRate`, `SimPauseChange`.

Verification command actually used (reproducible):

```powershell
$p='E:\FModel\Output\Exports\F1Manager24\Content\UIGameface\js'
Get-ChildItem $p -Recurse -File -Filter *.js |
  Select-String -Pattern 'sendEvent\)?\(\s*["'']([A-Za-z0-9_]+)["'']' -AllMatches |
  ForEach-Object { foreach($m in $_.Matches){ $m.Groups[1].Value } } | Sort-Object -Unique
```

(Dot-sourced into Python, classifying each hit as `Engine` / `Audio` by the receiver token on the
same line, to drop the 9 audio strings.)

### 2.2 Dynamic event names that the literal scan cannot enumerate

Four sites build the name at runtime. These are **not** in the 352 and cannot be listed exhaustively
from the export:

| file:line | construction | consequence |
|---|---|---|
| [`project/modules/options/OptionsScreenAdjustments.js:72`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/options/OptionsScreenAdjustments.js#L72) | `'OptionsChange' + key[0].toUpperCase() + key.substring(1)` | the `OptionsChange*` family is partly open-ended |
| [`project/modules/raceWeekend/DriverPanel.js:411`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/DriverPanel.js#L411) | `'Commands' + DriverPanelMenu[index] + 'TabSelected'` | one family per driver-panel tab |
| [`project/modules/fanfareModule/components/fanfares/*.js`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/fanfareModule/components/fanfares/FanfareSponsorPlanReview.js#L43) | `` `${EFanfareType[...]}AnimationFinished` `` and `` `...Continue` `` | one pair per `EFanfareType` value |
| `project/components/CASFooter.js:84,89` | `this.props.resetEvent` / `randomiseEvent` | caller-supplied |

`EFanfareType` has 14 values ([`GameTypes.js`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/GameTypes.js), search `var EFanfareType`):
`ManufacturingCompleted`, `DesignCompleted`, `ResearchCompleted`, `StaffHired`,
`ConstructionCompleted`, `UpgradeCompleted`, `RefurbishCompleted`, `MidSeasonBoardReview`,
`EndOfSeasonOverview`, `StaffRatingChanged`, `FastestPitStop`, `NewSeasonTarget`,
`EngagementPeriodCompleted`, `COUNT`. So e.g. `DesignCompletedAnimationFinished`,
`DesignCompletedContinue`, `ManufacturingCompletedAnimationFinished`, … exist at runtime.

### 2.3 The table

352 rows, sorted by event name. `file:line` is the first raising site; `(+N more sites)` means
the same name is raised from N further places (full multi-site list is in §2.4 for the
progression-relevant ones). Component is the enclosing `class`, handler the enclosing method.
**Screen** is the `.html` document whose app bundle contains the file, derived from the import
graph rooted at `Frontend.html` / `RaceWeekend.html` / `TeamManagement.html`.

| event | file:line | component | handler | screen | in-game effect |
|---|---|---|---|---|---|
| `AddCustomStaff` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:361` | _CreateATeamPersonBrowser | onAddStaffSelected | Frontend | Create-a-team browser: add a custom staff member. |
| `AddPlannedEngagementActivity` | `project/modules/finance/Obligations.js:236` | _SponsorshipPlan | addActivity | TeamManagement (Finance) | Sponsorship plan: add a planned engagement activity. |
| `AdvanceToNextEvent` | `project/components/ContinueButtonContainer.js:155` (+4 more sites) | _ContinueButtonContainer | onSelect | shared component | Team-management Advance button; asks the engine to move the career clock on to the next calendar event. Fired once per real press (see section 3). |
| `AdvanceToNextStage` | `project/modules/raceWeekend/DriverPracticeGoals.js:60` (+6 more sites) | _DriverPracticeGoals | onContinue | RaceWeekend | Race-weekend "Continue/NEXT" inside a session flow; asks the engine to step the weekend to its next stage. |
| `AnalyticsClosed` | `project/modules/raceWeekend/AnalyticsView.js:19` | AnalyticsView | onCloseAnalytics | RaceWeekend | Analytics view: player closed it. |
| `AutofillEngagementActivities` | `project/modules/finance/Obligations.js:175` | _SponsorshipPlan | onSelectAutofill | TeamManagement (Finance) | Sponsorship plan: auto-fill the engagement activities. |
| `BackToFrontEnd` | `project/modules/frontend/components/ScenarioMode/PostRace/ScenarioPostSessionScreen.js:100` | ScenarioPostSessionScreen | backToMainMenuSelected | Frontend (scenario) | Scenario post-session screen: return to the main menu. |
| `BackToScenarioSelection` | `project/modules/frontend/components/ScenarioMode/PostRace/ScenarioPostSessionScreen.js:94` | ScenarioPostSessionScreen | newChallengeSelected | Frontend (scenario) | Scenario post-session screen: go back to challenge selection. |
| `BrandingRandomise` | `project/modules/frontend/components/TeamBrandingContent.js:152` | _TeamBrandingContent | onRandomiseBranding | Frontend | Team branding: randomise the branding. |
| `BrandingReset` | `project/modules/frontend/components/TeamBrandingContent.js:157` | _TeamBrandingContent | onResetBranding | Frontend | Team branding: reset the branding. |
| `CASDeleteSave` | `project/modules/frontend/components/CreateASeason/CASSaveLoadScreen.js:160` | _CASSaveLoadScreen | onConfirmChoice | Frontend | Create-a-season: confirm deleting a save slot (arg: slotName). |
| `CASLoadSave` | `project/modules/frontend/components/CreateASeason/CASSaveLoadScreen.js:155` | _CASSaveLoadScreen | onConfirmChoice | Frontend | Create-a-season: confirm loading a save slot (arg: slotName). |
| `CASNewSave` | `project/modules/frontend/components/CreateASeason/CASSaveLoadScreen.js:169` (+1 more sites) | _CASSaveLoadScreen | onConfirmChoice | Frontend | Create-a-season: confirm writing a new save slot (arg: slotName). |
| `CASOverwriteSave` | `project/modules/frontend/components/CreateASeason/CASSaveLoadScreen.js:166` | _CASSaveLoadScreen | onConfirmChoice | Frontend | Create-a-season: confirm overwriting a save slot (arg: slotName). |
| `CASRaceCalendar_SetRace` | `project/modules/frontend/components/CreateASeason/CASRaceCalendar.js:169` (+3 more sites) | _CASRaceCalendar | onActionButtonSelected | Frontend | Create-a-season: set the race for a calendar slot (args: slot, race). |
| `CASRaceCalendar_SetRaceAsSprint` | `project/modules/frontend/components/CreateASeason/CASRaceCalendar.js:141` | _CASRaceCalendar | getPromptActionButtons | Frontend | Create-a-season: mark a calendar slot as a sprint. |
| `CASTeamLineups_AllTeamsRandomise` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:168` | _CASTeamLineUp | onModalRandomiseConfirmed | Frontend | Create-a-season: randomise every team lineup. |
| `CASTeamLineups_AllTeamsReset` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:162` | _CASTeamLineUp | onModalResetConfirmed | Frontend | Create-a-season: reset every team lineup. |
| `CASTeamLineups_SingleTeamReset` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:159` | _CASTeamLineUp | onModalResetConfirmed | Frontend | Create-a-season: reset one team lineup. |
| `CalendarDaySelected` | `project/modules/calendar/CalendarContents.js:74` (+1 more sites) | _CalendarContents | - | TeamManagement (Calendar) | Calendar: report the currently selected day (sent on unmount). |
| `CancelActiveSessionSetup` | `project/modules/raceWeekend/PitOptionsView.js:242` | _PitOptionsView | onClose | RaceWeekend | Pit options closed with unsaved changes: discard the active session setup (arg: carID). |
| `CancelEditCustomStaffMember` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:375` | _CreateATeamPersonBrowser | onCancelEditingStaff | Frontend | Create-a-team browser: cancel editing. |
| `CancelPitStopRequest` | `project/modules/raceWeekend/PitOptionsView.js:203` (+1 more sites) | _PitOptionsView | onConfirmRacePit | RaceWeekend | Pit options closed with changes: cancel the pending pit-stop request. |
| `CarBuildDevUpdateCarPerformanceValueType` | `project/modules/cars/carBuildAndPartsDevelopment/carBuild/CarBuildPartDetails.js:171` (+3 more sites) | _CarBuildPartDetails | cycleCarPerformanceValueType | TeamManagement (Cars) | Cars: cycle the performance-value display type. |
| `CarBuildPartDetails_HidePanel` | `project/modules/cars/carBuildAndPartsDevelopment/carBuild/CarBuildPartDetails.js:176` | _CarBuildPartDetails | cycleHidePanel | TeamManagement (Cars) | Cars: hide the part-details panel. |
| `CarConditionSelected` | `project/modules/raceWeekend/RaceWeekendEvents.js:132` | - | DriverCommandCarConditionSelected | RaceWeekend | Race view: player selected a car to inspect its condition. |
| `CarDriverLoadoutUpdated` | `project/modules/raceWeekend/strategy/PreSessionMenu.js:223` | PreSessionMenu | onSwapDriver | RaceWeekend | Pre-session: player swapped which driver is in which car (arg: car, staffID). |
| `CarTrackSelected` | `project/modules/cars/carBuildAndPartsDevelopment/carComparison/CarBuildCompareTrackButton.js:63` | CarBuildCompareTrackButton | onSelectTrack | TeamManagement (Cars) | Cars compare: choose the reference track. |
| `ChangeCarDevDesignExpertiseGainDifficulty` | `project/modules/frontend/components/CareerSettings.js:73` | CareerSettingsDataWrapper | getGuidanceInfo | Frontend | Career settings: set the car-development design-expertise gain difficulty. |
| `ChangeFirstTimeGuidance` | `project/modules/frontend/components/CareerSettings.js:72` (+1 more sites) | CareerSettingsDataWrapper | getGuidanceInfo | Frontend | Career settings: toggle first-time guidance. |
| `ChangeRaceSimAIDifficulty` | `project/modules/frontend/components/CareerSettings.js:74` | CareerSettingsDataWrapper | getGuidanceInfo | Frontend | Career settings: set the race-sim AI difficulty. |
| `ChangeTeam` | `project/modules/frontend/components/TeamSelectMenu.js:177` | _TeamSelectMenu | onToggleTeam | Frontend | Team-select menu: player toggled which team to take over. |
| `ChooseTeamToggleAudio` | `project/modules/frontend/components/TeamSelectMenu.js:215` (+1 more sites) | _TeamSelectMenu | onAudioToggle | Frontend | Team-select menu: audio toggle (cosmetic only). |
| `ClearCarStatHighlights` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/CarStatComparisonBar.js:103` (+5 more sites) | _CarStatComparisonBar | onFocus | TeamManagement (Cars) | Cars: clear the stat comparison highlights. |
| `ClearSecondarySponsorSlot` | `project/modules/preSeason/PreSeasonPackageSelection.js:126` | PreSeasonPackageSelection | onRemoveOptionalIncentive | TeamManagement (Pre-season) | Pre-season sponsorship: clear an optional incentive slot. |
| `CloseFanfare` | `project/modules/fanfareModule/FanfareDialogsModule.js:92` (+1 more sites) | FanfareDialogsModule | closeFanfare | both apps (fanfare) | Player dismissed a fanfare dialog. |
| `ClosePitOptions` | `project/modules/raceWeekend/PitOptionsView.js:240` | _PitOptionsView | onClose | RaceWeekend | Pit-options overlay closed. |
| `CloseReplay` | `project/modules/raceWeekend/replay/ReplayView.js:96` | _ReplayView | toggleUI | RaceWeekend | Replay view: player toggled the UI closed. |
| `ColourConfirmed` | `project/modules/editors/EditTeamLogo.js:619` (+3 more sites) | _EditTeamLogo | onAddRecentColour | TeamManagement (editors) | Logo/colour editor: player confirmed a colour swatch (adds it to recents). |
| `Commands` | `project/modules/raceWeekend/DriverPanel.js:411` | _DriverPanel | onTabSelectionChange | RaceWeekend | Driver panel: a tab was selected; the name is built as `Commands<Tab>TabSelected` (arg: playerCar). |
| `CommsResponse` | `project/modules/raceWeekend/driverPanel/teamComms/TeamComm.js:165` (+1 more sites) | _TeamComm | onSelectAction | RaceWeekend | Player answered a team-comms dilemma. |
| `ConditionChangeParameters` | `project/modules/sponsorTargets/SponsorTargetOptions.js:257` (+7 more sites) | _SponsorTargetOptions | onChange | TeamManagement (Sponsors) | Sponsor target options: change the condition parameters. |
| `ConditionConfirm` | `project/modules/sponsorTargets/SponsorTargetOptions.js:311` | _SponsorTargetOptions | onConfirm | TeamManagement (Sponsors) | Sponsor target options: confirm. |
| `ConditionRevert` | `project/modules/sponsorTargets/SponsorTargetOptions.js:242` | _SponsorTargetOptions | onCancel | TeamManagement (Sponsors) | Sponsor target options: revert. |
| `ConditionToggleGuarantee` | `project/modules/sponsorTargets/SponsorTargetEditButton.js:51` | _SponsorTargetEditButton | onToggle | TeamManagement (Sponsors) | Sponsor target options: toggle the guarantee. |
| `ConfirmCustomStaffMember` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:368` | _CreateATeamPersonBrowser | onEditingStaffConfirm | Frontend | Create-a-team browser: confirm the edited staff member. |
| `ConfirmEngagementActivities` | `project/modules/finance/Obligations.js:46` | _TeamManagementSponsorshipPlan | onClosedAndContinued | TeamManagement (Finance) | Sponsorship plan: confirm and continue. |
| `ConfirmSponsorPackages` | `project/modules/preSeason/PreSeasonSponsorshipSelection.js:150` | _PreSeasonSponsorshipSelection | onClosedAndContinued | TeamManagement (Pre-season) | Pre-season sponsorship: confirm the chosen packages and continue. |
| `ConfirmSponsorRaceBonuses` | `project/modules/sponsorTargets/SponsorTargetsPreRaceDriver.js:187` | _SponsorTargetsPreRaceDriver | onContinue | TeamManagement (Sponsors) | Pre-race sponsor targets: confirm the chosen bonuses (fires immediately before AdvanceToNextEvent). |
| `ConfirmTeamChange` | `project/modules/frontend/components/TeamSelectMenu.js:248` | _TeamSelectMenu | onFadeToBlackEnd | Frontend | Team-select fade-out finished; commit the player's new team choice. |
| `ConstructorStandingsClosed` | `project/modules/raceWeekend/ConstructorStandingsView.js:19` | ConstructorStandingsView | onCloseConstructorStandings | RaceWeekend | Constructor standings view: player closed it. |
| `ContinueGame` | `project/modules/frontend/components/MainMenu.js:148` | _MainMenu | onSelectContinue | Frontend | Player pressed main-menu Continue; loads the career named by `Save.Saves.continueSlotName`. No argument is passed. |
| `CreateASeasonLoadAutoSave` | `project/modules/frontend/components/CreateASeason/CASBaseScreen.js:130` | _CASBaseScreen | onAutoSavePopUpConfirm | Frontend | Create-a-season: load the CAS autosave. |
| `CreateASeasonRemoveAutoSave` | `project/modules/frontend/components/CreateASeason/CASBaseScreen.js:103` (+2 more sites) | _CASBaseScreen | goBack | Frontend | Create-a-season: discard the CAS autosave. |
| `CustomSeasonBuildingSetDefault` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:253` | _CASTeamLineUp | onFacilitiesChanged | Frontend | Create-a-season: default buildings. |
| `CustomSeasonCarSetDefault` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:264` | _CASTeamLineUp | onCarPerformanceChanged | Frontend | Create-a-season: set car performance to the default band. |
| `CustomSeasonCashSetDefault` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:243` | _CASTeamLineUp | onBalanceChanged | Frontend | Create-a-season: set the cash band to default. |
| `CustomSeasonChangeBuildingBandLevel` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:256` | _CASTeamLineUp | onFacilitiesChanged | Frontend | Create-a-season: change the building band level. |
| `CustomSeasonChangeCarBandLevel` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:267` | _CASTeamLineUp | onCarPerformanceChanged | Frontend | Create-a-season: change the car performance band. |
| `CustomSeasonChangeCashBandLevel` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:246` | _CASTeamLineUp | onBalanceChanged | Frontend | Create-a-season: change the cash band level. |
| `CustomSeasonChangeRegulation` | `project/modules/frontend/components/CreateASeason/CASRegulations.js:215` (+9 more sites) | _CreateASeasonRegulations | onCostCapChanged | Frontend | Create-a-season: change a regulation. |
| `CustomTeamAddStaff` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:309` (+3 more sites) | _CreateATeamPersonBrowser | onStaffSelect | Frontend | Create-a-team: add a staff member to a slot. |
| `CustomTeamChangeBuildingBandLevel` | `project/modules/frontend/components/CreateATeamSetUp.js:66` | _CreateATeamSetUp | onFacilitiesChanged | Frontend | Create-a-team: change the building band level. |
| `CustomTeamChangeBuildingLevel` | `project/modules/frontend/components/CreateATeamFacilities.js:106` | _CreateATeamFacilities | onLevelChange | Frontend | Create-a-team: change a facility level. |
| `CustomTeamChangeCarBandLevel` | `project/modules/frontend/components/CreateATeamSetUp.js:69` | _CreateATeamSetUp | onCarPerformanceChanged | Frontend | Create-a-team: change the car performance band. |
| `CustomTeamChangeCashBandLevel` | `project/modules/frontend/components/CreateATeamSetUp.js:63` | _CreateATeamSetUp | onBalanceChanged | Frontend | Create-a-team: change the starting cash band. |
| `CustomTeamChangePowertrain` | `project/modules/frontend/components/CreateATeamPowertrain.js:23` | CreateATeamPowertrain | onSupplierSelected | Frontend | Create-a-team: choose the powertrain supplier. |
| `CustomTeamChangeTeamCountry` | `project/modules/frontend/components/CreateATeamProfile.js:159` (+1 more sites) | CreateATeamProfile | onSelectCountry | Frontend | Create-a-team: choose the team country. |
| `CustomTeamChangeTeamOrigin` | `project/modules/frontend/components/CreateATeamProfile.js:173` | CreateATeamProfile | onPresetSelected | Frontend | Create-a-team: choose a team origin preset. |
| `CustomTeamRandomiseStaffSlots` | `project/modules/frontend/components/CreateATeamPeopleOverview.js:111` | CreateATeamPeopleOverview | onRandomiseConfirm | Frontend | Create-a-team: randomise the staff slots. |
| `CustomTeamRemoveStaff` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:316` (+1 more sites) | _CreateATeamPersonBrowser | onStaffSelect | Frontend | Create-a-team: remove a staff member from a slot. |
| `CustomTeamResetStaffSlots` | `project/modules/frontend/components/CreateATeamPeopleOverview.js:103` | CreateATeamPeopleOverview | onReset | Frontend | Create-a-team: reset the staff slots. |
| `CustomTeamSwapDrivers` | `project/modules/frontend/components/CreateATeamPeopleOverview.js:99` (+1 more sites) | CreateATeamPeopleOverview | onSwapDrivers | Frontend | Create-a-team: swap the two drivers. |
| `CustomTeamSwapEngineers` | `project/modules/frontend/components/CreateASeason/CASTeamLineUp.js:215` | _CASTeamLineUp | onSwapEngineers | Frontend | Create-a-team: swap the two engineers. |
| `DLCOpenStore` | `project/modules/editors/EditorUpsell.js:30` (+4 more sites) | EditorUpsell | onSelect | TeamManagement (editors) | Player opened the platform store page for a DLC. |
| `DebugInputPlayer_onLoopReplay` | `common/debug/DebugInputPlayer.js:370` | DebugInputPlayer | onReplayEnd | engine harness | Debug input player: loop-replay ended. |
| `DebugInputPlayer_onToggleReplaying` | `common/debug/DebugInputPlayer.js:338` | DebugInputPlayer | togglePlayRecording | engine harness | Debug input player: replay toggled. |
| `DeleteCustomStaffMember` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:336` | _CreateATeamPersonBrowser | onStaffSelectSubButton | Frontend | Create-a-team browser: delete a custom staff member. |
| `DesignStageChanged` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/newDesign/CarBuildNewDesign.js:50` (+2 more sites) | _CarBuildNewDesign | - | TeamManagement (Cars) | Design screen stage changed (UI bookkeeping). |
| `DriverCommandAllowTeamMatePast` | `project/modules/raceWeekend/RaceWeekendEvents.js:46` | - | DriverCommandAllowTeamMatePast | RaceWeekend | Set the allow-team-mate-past instruction (arg: carID, bool). |
| `DriverCommandAvoidKerbsChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:22` | - | DriverCommandAvoidKerbsChange | RaceWeekend | Set the avoid-kerbs instruction (arg: carID, bool). |
| `DriverCommandDefenceStrategyChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:50` | - | DriverCommandDefendStrategy | RaceWeekend | Set the defence strategy (arg: carID, value). |
| `DriverCommandDelayPitByOneLap` | `project/modules/raceWeekend/RaceWeekendEvents.js:38` | - | DriverCommandDelayPitByOneLap | RaceWeekend | Set the delay-pit-by-one-lap instruction (arg: carID, bool). |
| `DriverCommandDelayPitByOneLapIfCarInfrontPits` | `project/modules/raceWeekend/RaceWeekendEvents.js:34` | - | DriverCommandDelayPitByOneLapIfCarInfrontPits | RaceWeekend | Set the conditional delay-pit instruction (arg: carID, bool). |
| `DriverCommandDriveInClearAirChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:18` | - | DriverCommandDriveInClearAirChange | RaceWeekend | Set the drive-in-clear-air instruction (arg: carID, bool). |
| `DriverCommandERSOvertakeAssistChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:58` | - | DriverCommandERSOvertakeAssist | RaceWeekend | Set the ERS overtake-assist instruction (arg: carID, value). |
| `DriverCommandERSStrategyChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:14` | - | DriverCommandERSStrategyChange | RaceWeekend | Set ERS strategy (arg: carID, strategy). |
| `DriverCommandLiftAndCoastChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:10` | - | DriverCommandLiftAndCoastChange | RaceWeekend | Set lift-and-coast instruction (arg: carID, setting). |
| `DriverCommandOvertakeStrategyChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:54` | - | DriverCommandOvertakeStrategy | RaceWeekend | Set the overtake strategy (arg: carID, value). |
| `DriverCommandPaceChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:6` | - | DriverCommandPaceChange | RaceWeekend | Set a car's pace instruction (arg: carID, pace). |
| `DriverCommandPitOnSafetyCar` | `project/modules/raceWeekend/RaceWeekendEvents.js:42` | - | DriverCommandPitOnSafetyCar | RaceWeekend | Set the pit-on-safety-car instruction (arg: carID, bool). |
| `DriverCommandRetireCar` | `project/modules/raceWeekend/RaceWeekendEvents.js:30` | - | DriverCommandRetireCar | RaceWeekend | Retire a car from the session (arg: carID). |
| `DriverCommandShowPreview` | `project/modules/raceWeekend/RaceWeekendEvents.js:125` | - | DriverCommandShowPreview | RaceWeekend | Toggle the driver-command preview (arg: show). |
| `DriverExperienceClosed` | `project/modules/raceWeekend/DriverExperienceView.js:66` (+1 more sites) | _DriverExperienceView | onContinue | RaceWeekend | Driver-experience view: player pressed Continue. |
| `DriverPanelSelect` | `project/modules/raceWeekend/RaceWeekendEvents.js:121` | - | DriverPanelSelect | RaceWeekend | Player selected a driver panel. |
| `DriverPreparationChangeRevealed` | `project/modules/raceWeekend/DriverConfidenceBar.js:230` | _DriverPreparationBar | onAnimateBorderEnd | RaceWeekend | Driver confidence bar: the preparation-change reveal animation ended. |
| `DriverScreenTabChanged` | `project/modules/drivers/Drivers.js:179` | _Drivers | onChangeTab | TeamManagement (Drivers) | Drivers screen tab changed. |
| `DriverStandingsClosed` | `project/modules/raceWeekend/DriverStandingsView.js:19` | DriverStandingsView | onCloseDriverStandings | RaceWeekend | Driver standings view: player closed it. |
| `DriverTacticsMenuOpened` | `project/modules/raceWeekend/driverPanel/driverFluff/DriverFluffOverview.js:63` | _DriverFluffOverview | onSelectMenu | RaceWeekend | Player opened the driver-tactics menu. |
| `EditCustomStaffMember` | `project/modules/frontend/components/CreateATeamPersonBrowser.js:330` | _CreateATeamPersonBrowser | onStaffSelectSubButton | Frontend | Create-a-team browser: edit a custom staff member. |
| `EditStaffAchievements` | `project/components/CASEditStaffMemberModal.js:273` | _CASEditStaffMemberModal | onAchievementIndexChanged | shared component | Custom staff editor: change achievements. |
| `EditStaffAge` | `project/components/CASEditStaffMemberModal.js:231` | _CASEditStaffMemberModal | onAgeChanged | shared component | Custom staff editor: change age. |
| `EditStaffAggression` | `project/components/CASEditStaffMemberModal.js:251` | _CASEditStaffMemberModal | onAggressionChanged | shared component | Custom staff editor: change aggression. |
| `EditStaffCountryID` | `project/components/CASEditStaffMemberModal.js:240` | _CASEditStaffMemberModal | onCountryChanged | shared component | Custom staff editor: change country. |
| `EditStaffExperience` | `project/components/CASEditStaffMemberModal.js:268` | _CASEditStaffMemberModal | onExperienceIndexChanged | shared component | Custom staff editor: change experience. |
| `EditStaffFirstName` | `project/components/CASEditStaffMemberModal.js:199` | _CASEditStaffMemberModal | onFirstNameCommit | shared component | Custom staff editor: commit the first name. |
| `EditStaffGender` | `project/components/CASEditStaffMemberModal.js:226` | _CASEditStaffMemberModal | onGenderChanged | shared component | Custom staff editor: change gender. |
| `EditStaffImprovability` | `project/components/CASEditStaffMemberModal.js:247` | _CASEditStaffMemberModal | onImprovabilityChanged | shared component | Custom staff editor: change improvability. |
| `EditStaffLastName` | `project/components/CASEditStaffMemberModal.js:206` | _CASEditStaffMemberModal | onLastNameCommit | shared component | Custom staff editor: commit the last name. |
| `EditStaffMarketAppetite` | `project/components/CASEditStaffMemberModal.js:278` | _CASEditStaffMemberModal | onMarketAppetiteChanged | shared component | Custom staff editor: change market appetite. |
| `EditStaffPerformanceStat` | `project/components/CASEditStaffMemberModal.js:263` | _CASEditStaffMemberModal | onPerformanceValueChanged | shared component | Custom staff editor: change a performance stat. |
| `EditStaffRandomise` | `project/components/CASEditStaffMemberModal.js:215` | _CASEditStaffMemberModal | onRandomizeStats | shared component | Custom staff editor: randomise the stats. |
| `EditStaffRandomiseStaffPotrait` | `project/components/CASEditStaffMemberModal.js:212` | _CASEditStaffMemberModal | onRandomizePressed | shared component | Custom staff editor: randomise the portrait. |
| `EditStaffResetToDefaults` | `project/components/CASEditStaffMemberModal.js:220` | _CASEditStaffMemberModal | onResetToDefaults | shared component | Custom staff editor: reset to defaults. |
| `EditStaffRetirmentAge` | `project/components/CASEditStaffMemberModal.js:243` | _CASEditStaffMemberModal | onRetirementAgeChanged | shared component | Custom staff editor: change retirement age. |
| `EndTutorial` | `project/modules/options/GameplaySettings.js:102` | _GameplaySettings | endTutorial | Options (both apps) | Options: end the tutorial. |
| `EngineChoiceContinue` | `project/modules/cars/engineChoice/PreSeasonEngineChoice.js:34` (+1 more sites) | PreSeasonEngineChoice | doConfirmSelection | TeamManagement (Cars) | Pre-season engine choice: confirm the selection. |
| `EngineChoiceUpdated` | `project/modules/cars/engineChoice/PreSeasonEngineChoice.js:30` | PreSeasonEngineChoice | onSupplierSelected | TeamManagement (Cars) | Pre-season engine choice: a supplier was selected. |
| `EnterTeamChangeFlow` | `project/modules/fanfareModule/components/fanfares/FanfareNewSeasonTarget.js:81` (+1 more sites) | _FanfareNewSeasonObjectives | onSelectSwitchTeams | both apps (fanfare) | Player asked to change team (game-over screen, or new-season objectives fanfare). |
| `ExitCredits` | `project/apps/CreditsApp.js:308` | _CreditsContent | onCreditsEnd | Frontend | Credits finished or were skipped; leave the credits app. |
| `FrontendCinematicsPlay` | `project/modules/frontend/components/MainMenu.js:185` (+1 more sites) | _MainMenu | gotoNewGame | Frontend | New-career flow: the front-end intro cinematics should start playing. |
| `FrontendNewsletterSignup` | `project/modules/frontend/components/MainMenu.js:217` | _MainMenu | onConfirmDialogNewsletter | Frontend | Player answered the newsletter sign-up dialog. |
| `FrontendScreenChanged` | `project/utils/FrontendNavigation.js:27` | - | setInternal | shared util | Front-end navigation changed page; reports the new screen id to the engine (telemetry/state sync). |
| `FuelLoadChange` | `project/modules/raceWeekend/strategy/CarSetupFuel.js:121` | CarSetupFuelPracticeRace | onChange | RaceWeekend | Strategy screen: fuel load stepper changed. |
| `GameOverRetryLast5` | `project/modules/loadSave/GameOverScreen.js:111` | _GameOverScreen | onRetryLast5Confirm | Frontend + RaceWeekend | Game over: player chose to retry the last 5 races. |
| `GameOverRetrySeason` | `project/modules/loadSave/GameOverScreen.js:115` | _GameOverScreen | onRestartSeasonConfirm | Frontend + RaceWeekend | Game over: player chose to restart the season. |
| `GenerateRaceBonusOptions` | `project/modules/sponsorTargets/SponsorTargetsPreRaceDriver.js:167` | _SponsorTargetsPreRaceDriver | updateBonusOptionData | TeamManagement (Sponsors) | Pre-race sponsor targets: ask the engine to roll the bonus options. |
| `HQFacilitySelected` | `project/modules/hq/HQFacilityList.js:103` | _HQFacilityList | onSelectFacility | TeamManagement (HQ) | HQ: player selected a facility. |
| `HelpScreenRequestedByPlayer` | `project/modules/tutorial/HelperModule.js:40` | HelperModule | onRequestHelpPanel | both apps | Player opened a help panel (arg: helpScreenId). |
| `HideRedFlagDialog` | `project/modules/raceWeekend/RaceView.js:409` | _RaceView | onHideRedFlagDialog | RaceWeekend | Race view: player dismissed the red-flag dialog. |
| `LiveryEditor_Closed` | `project/modules/editors/LiveryEditor.js:359` | LiveryEditor | onHeaderBack | TeamManagement (editors) | Livery editor closed. |
| `LiveryEditor_FinishEditingDecalSlot` | `project/modules/editors/LiveryDecalEditor.js:332` (+3 more sites) | _LiveryDecalEditor | onEditReset | TeamManagement (editors) | Stop editing a decal slot. |
| `LiveryEditor_LoadCarLivery` | `project/modules/editors/LiveryEditor.js:400` | LiveryEditor | onSelectPreset | TeamManagement (editors) | Load a livery from a slot. |
| `LiveryEditor_NewCarLivery` | `project/modules/editors/LiveryEditor.js:415` | LiveryEditor | onNew | TeamManagement (editors) | Start a blank livery. |
| `LiveryEditor_OnMouseDrag` | `project/modules/editors/LiveryEditor.js:284` (+1 more sites) | LiveryEditor | onMouseMove | TeamManagement (editors) | Mouse drag inside the livery editor (live preview only). |
| `LiveryEditor_Opened` | `project/modules/editors/LiveryEditor.js:86` | LiveryEditor | - | TeamManagement (editors) | Livery editor opened (UI lifecycle signal). |
| `LiveryEditor_PreviewCarLivery` | `project/modules/editors/LiveryEditor.js:405` | LiveryEditor | onPreviewPreset | TeamManagement (editors) | Preview a stored livery. |
| `LiveryEditor_PreviewLiveryBodyPattern` | `project/modules/editors/LiveryEditor.js:330` | LiveryEditor | onPatternSelect | TeamManagement (editors) | Preview a body pattern. |
| `LiveryEditor_PreviewLiveryWheelPattern` | `project/modules/editors/LiveryEditor.js:349` | LiveryEditor | onPatternSelect | TeamManagement (editors) | Preview a wheel pattern. |
| `LiveryEditor_ResetDecalSlot` | `project/modules/editors/LiveryDecalEditor.js:331` (+1 more sites) | _LiveryDecalEditor | onEditReset | TeamManagement (editors) | Reset a decal slot to default. |
| `LiveryEditor_RevertCarLivery` | `project/modules/editors/LiveryEditor.js:426` | LiveryEditor | onDialogDiscardChanges | TeamManagement (editors) | Revert the car livery to the stored version. |
| `LiveryEditor_RevertPreview` | `project/modules/editors/LiveryEditor.js:363` | LiveryEditor | onHeaderBack | TeamManagement (editors) | Leave preview mode. |
| `LiveryEditor_SaveCarLivery` | `project/modules/editors/LiveryEditor.js:419` | LiveryEditor | onSaveLiveryToSlot | TeamManagement (editors) | Save the livery into a livery slot. |
| `LiveryEditor_SetBodyPaintColor` | `project/modules/editors/LiveryBodyPaintEditor.js:206` | _BodyPaintLayerEditor | onColorChange | TeamManagement (editors) | Set the body-paint colour. |
| `LiveryEditor_SetBodyPaintStyle` | `project/modules/editors/LiveryBodyPaintEditor.js:223` | _BodyPaintLayerEditor | onAddRecentColour | TeamManagement (editors) | Set the body-paint style. |
| `LiveryEditor_SetDecalSlotGenericColor` | `project/modules/editors/LiveryDecalEditor.js:394` | _LiveryDecalEditor | onGenericColorChange | TeamManagement (editors) | Set a generic decal colour. |
| `LiveryEditor_SetDecalSlotIcon` | `project/modules/editors/LiveryDecalEditor.js:337` (+1 more sites) | _LiveryDecalEditor | onEditClear | TeamManagement (editors) | Set a decal slot icon. |
| `LiveryEditor_SetDecalSlotTeamLogo` | `project/modules/editors/LiveryDecalEditor.js:386` | _LiveryDecalEditor | onSelectIcon | TeamManagement (editors) | Set a decal slot to the team logo. |
| `LiveryEditor_SetDecalSlotTransform` | `project/modules/editors/LiveryDecalEditor.js:403` | _LiveryDecalEditor | onTransformChange | TeamManagement (editors) | Set a decal transform (position/scale/rotation). |
| `LiveryEditor_SetDecalSponsorLogoType` | `project/modules/editors/LiveryDecalEditor.js:399` | _LiveryDecalEditor | onChangeSponsorLogoType | TeamManagement (editors) | Change the sponsor-logo type on a decal slot. |
| `LiveryEditor_SetDriverHelmetColour` | `project/modules/editors/LiveryBodyPaintEditor.js:203` | _BodyPaintLayerEditor | onColorChange | TeamManagement (editors) | Set the driver helmet colour. |
| `LiveryEditor_SetDriverHelmetStyle` | `project/modules/editors/LiveryBodyPaintEditor.js:220` | _BodyPaintLayerEditor | onAddRecentColour | TeamManagement (editors) | Set the driver helmet style. |
| `LiveryEditor_SetLiveryBodyPattern` | `project/modules/editors/LiveryEditor.js:317` | LiveryEditor | onPatternSelect | TeamManagement (editors) | Set the body pattern. |
| `LiveryEditor_SetLiveryWheelPattern` | `project/modules/editors/LiveryEditor.js:336` | LiveryEditor | onPatternSelect | TeamManagement (editors) | Set the wheel pattern. |
| `LiveryEditor_StartEditingDecalSlot` | `project/modules/editors/LiveryDecalEditor.js:304` | _LiveryDecalEditor | onRootSelectDecalSlot | TeamManagement (editors) | Begin editing a decal slot. |
| `LiveryEditor_applyChanges` | `project/modules/editors/LiveryEditor.js:421` (+1 more sites) | LiveryEditor | onSaveLiveryToSlot | TeamManagement (editors) | Apply the edited livery to the car. |
| `LoadGame` | `project/modules/loadSave/LoadSaveScreen.js:128` | _LoadSaveScreen | onSelectLoadData | Frontend + RaceWeekend | Player confirmed loading a save slot; argument is the slot name (`autoSave`, `save1`..`save29`). |
| `LogoEditor_AddLayer` | `project/modules/editors/EditTeamLogo.js:557` (+1 more sites) | _EditTeamLogo | addNewLayer | TeamManagement (editors) | Add a layer to the logo (arg: index, icon path). |
| `LogoEditor_ApplyChanges` | `project/modules/editors/EditTeamLogo.js:349` | _EditTeamLogo | onSaveDialogApplyChanges | TeamManagement (editors) | Apply the edited logo. |
| `LogoEditor_Clear` | `project/modules/editors/EditTeamLogo.js:708` | _EditTeamLogo | onMenuNew | TeamManagement (editors) | Start a blank logo. |
| `LogoEditor_Closed` | `project/modules/editors/EditTeamLogo.js:333` | _EditTeamLogo | closeLogoEditor | TeamManagement (editors) | Logo editor closed. |
| `LogoEditor_DeleteLayer` | `project/modules/editors/EditTeamLogo.js:589` | _EditTeamLogo | deleteLayer | TeamManagement (editors) | Delete a logo layer. |
| `LogoEditor_LoadLogo` | `project/modules/editors/EditTeamLogo.js:719` | _EditTeamLogo | onSelectPreset | TeamManagement (editors) | Load a logo from a slot. |
| `LogoEditor_Opened` | `project/modules/editors/EditTeamLogo.js:75` | _EditTeamLogo | - | TeamManagement (editors) | Logo editor opened (UI lifecycle signal). |
| `LogoEditor_RevertChanges` | `project/modules/editors/EditTeamLogo.js:695` | _EditTeamLogo | revertLogo | TeamManagement (editors) | Revert the logo to the stored version. |
| `LogoEditor_SaveLogo` | `project/modules/editors/EditTeamLogo.js:723` | _EditTeamLogo | onSaveLogoToSlot | TeamManagement (editors) | Save the logo into a slot. |
| `LogoEditor_SetColor` | `project/modules/editors/EditTeamLogo.js:571` (+1 more sites) | _EditTeamLogo | duplicateLayer | TeamManagement (editors) | Set a logo layer colour. |
| `LogoEditor_SetIcon` | `project/modules/editors/EditTeamLogo.js:631` (+1 more sites) | _EditTeamLogo | onSelectIcon | TeamManagement (editors) | Set a logo layer icon (arg: index, icon path). |
| `LogoEditor_SetPosition` | `project/components/Logo.js:260` (+4 more sites) | _CreateALogo | onMouseMove | shared component | Set a logo layer position. |
| `LogoEditor_SetRotation` | `project/components/Logo.js:297` (+5 more sites) | _CreateALogo | rotateIcon | shared component | Rotate a logo layer. |
| `LogoEditor_SetScale` | `project/components/Logo.js:203` (+8 more sites) | _CreateALogo | onToggleFlipH | shared component | Set a logo layer scale. |
| `LogoEditor_StartEditing` | `project/modules/editors/EditTeamLogo.js:703` | _EditTeamLogo | startEditing | TeamManagement (editors) | Begin editing a layer. |
| `LogoEditor_SwapLayers` | `project/modules/editors/EditTeamLogo.js:604` | _EditTeamLogo | moveLayer | TeamManagement (editors) | Reorder logo layers (arg: index, new index). |
| `MailFlag` | `project/modules/inbox/Inbox.js:593` | _Inbox | onSelectFlag | TeamManagement (Inbox) | Inbox: toggle a mail item flag. |
| `MailOpen` | `project/modules/inbox/Inbox.js:542` | _Inbox | onSelectMailItem | TeamManagement (Inbox) | Inbox: player opened a mail item. |
| `MailRead` | `project/modules/inbox/Inbox.js:541` | _Inbox | onSelectMailItem | TeamManagement (Inbox) | Inbox: mark a mail item read. |
| `ManagementScreenChanged` | `project/utils/TeamManagementNavigation.js:41` | - | set | shared util | Team-management navigation changed screen; reports the screen id. |
| `ManufactureStageChanged` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/manufacture/CarBuildManufacture.js:123` | _CarBuildManufacture | - | TeamManagement (Cars) | Manufacture screen stage changed (UI bookkeeping). |
| `MarkAllAsRead` | `project/modules/inbox/Inbox.js:528` | _Inbox | onSelectMarkAllAsRead | TeamManagement (Inbox) | Inbox: mark every mail read. |
| `NewCareer` | `project/modules/frontend/components/CareerSettings.js:162` (+2 more sites) | CareerSettingsSkin | onFadeToBlackEnd | Frontend | Create-a-career fade-out finished; the engine should begin a brand-new career from the current CAS/team-selection data. |
| `NewDesignSetPreset` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/CarBuildSelectPresetButton.js:56` (+2 more sites) | _CarBuildSelectPresetButton | onSelectPresetButton | TeamManagement (Cars) | Cars design: apply a preset. |
| `NewSave` | `project/modules/loadSave/LoadSaveScreen.js:122` | _LoadSaveScreen | onSelectNewSave | Frontend + RaceWeekend | Player asked to write the current career into a new save slot. |
| `NextMonthSelected` | `project/modules/calendar/CalendarContents.js:278` | _CalendarContents | onMonthUp | TeamManagement (Calendar) | Calendar: player pressed the month-forward arrow. |
| `NotificationEnded` | `project/modules/raceWeekend/notifications/NotificationsContainer.js:94` | _NotificationsContainer | clearNotification | RaceWeekend | A race notification finished playing. |
| `ObligationChanged` | `project/modules/finance/ObligationsOptions.js:166` | _ObligationsOptions | onConfirm | TeamManagement (Finance) | Sponsorship obligations: confirm a parameter change. |
| `ObligationDriverIndexChanged` | `project/modules/finance/ObligationsOptions.js:140` (+1 more sites) | _ObligationsOptions | onDriverIndexChange | TeamManagement (Finance) | Sponsorship obligations: change which driver an obligation applies to. |
| `ObligationValueChanged` | `project/modules/finance/ObligationsOptions.js:145` (+1 more sites) | _ObligationsOptions | onValueChange | TeamManagement (Finance) | Sponsorship obligations: change an obligation value. |
| `OnBookmarkToggle` | `project/modules/scouting/ScoutingSearch.js:573` (+1 more sites) | _ScoutingSearch | onBookmark | TeamManagement (Scouting) | Scouting: toggle a bookmark on a staff member. |
| `OnCancelStaffContract` | `project/modules/staff/contractNegotiation/ContractNegotiation.js:117` (+3 more sites) | _ContractNegotiation | onConfirmDialog | TeamManagement (Staff) | Contract negotiation: confirm cancelling the contract. |
| `OnCloseGridPenaltyDialog` | `project/modules/raceWeekend/RaceWeekendEvents.js:117` | - | OnCloseGridPenaltyDialog | RaceWeekend | Player closed the grid-penalty dialog. |
| `OnConfirmStaffContract` | `project/modules/staff/contractNegotiation/ContractProposalAndResultsPanel.js:216` | _ContractProposalAndResultsPanel | onProposalAnimationsFinished | TeamManagement (Staff) | Contract negotiation: the proposal animation finished; confirm the signing. |
| `OnFailedStaffContract` | `project/modules/staff/contractNegotiation/ContractProposalAndResultsPanel.js:140` | _ContractProposalAndResultsPanel | onContinue | TeamManagement (Staff) | Contract negotiation failed; continue past the result panel. |
| `OnRaceMomentDialogClosed` | `project/modules/raceWeekend/RaceView.js:295` | _RaceView | onChallengeDialogPressed | RaceWeekend | Race view: player dismissed the race-moment/challenge dialog. |
| `OnReady` | `project/apps/LegalApp.js:38` | _Legal | - | Frontend | Staff/contract operation (`OnReady`). |
| `OnRenewStaffContract` | `project/modules/staff/contractNegotiation/ContractProposalAndResultsPanel.js:106` | _ContractProposalAndResultsPanel | onConfirmDialog | TeamManagement (Staff) | Contract negotiation: confirm a renewal. |
| `OnRevertPitCrewDevelopmentPlanChanges` | `project/modules/staff/pitCrew/TMPitCrewDevelopment.js:158` (+1 more sites) | _TMPitCrewDevelopment | onConfirmUndo | TeamManagement (Staff) | Pit-crew development: revert unsaved plan changes. |
| `OnSetPitCrewTrainingFocus` | `project/modules/staff/pitCrew/TMPitCrewDevelopment.js:135` | _TMPitCrewDevelopment | onChangeFocus | TeamManagement (Staff) | Pit-crew development: change the training focus. |
| `OnSetPitCrewTrainingPlan` | `project/modules/staff/pitCrew/TMPitCrewDevelopmentEntryPanelSection.js:75` (+1 more sites) | TMPitCrewDevelopmentEntryPanelSection | onChangeStepperTraining | TeamManagement (Staff) | Pit-crew development: change the training plan stepper. |
| `OnSetPitCrewTrainingPreset` | `project/modules/staff/pitCrew/TMPitCrewDevelopment.js:127` | _TMPitCrewDevelopment | onChangePreset | TeamManagement (Staff) | Pit-crew development: choose a training preset. |
| `OnSetupStaffContract` | `project/modules/staff/contractNegotiation/ContractProposalAndResultsPanel.js:104` (+1 more sites) | _ContractProposalAndResultsPanel | onConfirmDialog | TeamManagement (Staff) | Contract negotiation: set up the contract offer. |
| `OnStaffComparison` | `project/modules/scouting/compare/ComparisonUtil.js:6` (+3 more sites) | - | setComparisonSlotOne | TeamManagement (Scouting) | Scouting: set a comparison slot. |
| `OnStageBackwards` | `project/modules/raceWeekend/PostRaceSummaryViewPitStops.js:265` (+2 more sites) | _PostRaceSummaryViewPitStops | secondsFormatter | RaceWeekend | Player pressed Back inside a session flow; asks the engine to step the weekend stage backwards. |
| `OnSubmitContractOffer` | `project/modules/staff/contractNegotiation/ContractProposalAndResultsPanel.js:187` | _ContractProposalAndResultsPanel | onOfferContract | TeamManagement (Staff) | Contract negotiation: submit the offer. |
| `OnSubmitDetailedScout` | `project/modules/staff/staffDetails/StaffDetailsScoutingPanel.js:200` | _StaffDetailsScoutingPanel | onDetailedScout | TeamManagement (Staff) | Scouting: run a detailed scout. |
| `OnSubmitPitCrewDevelopmentPlan` | `project/modules/staff/pitCrew/TMPitCrewDevelopment.js:153` | _TMPitCrewDevelopment | onConfirmSubmit | TeamManagement (Staff) | Pit-crew development: submit the plan. |
| `OnSubmitQuickScout` | `project/modules/staff/staffDetails/StaffDetailsScoutingPanel.js:195` | _StaffDetailsScoutingPanel | onStandardScout | TeamManagement (Staff) | Scouting: run a quick scout. |
| `OnTerminateStaffContract` | `project/modules/staff/staffDetails/StaffDetailsContractPanel.js:257` | _ContractPanel | onSelectBreakContract | TeamManagement (Staff) | Contract panel: player chose to break the contract. |
| `OnUpdateContractTerms` | `project/modules/staff/contractNegotiation/ContractProposalAndResultsPanel.js:198` | _ContractProposalAndResultsPanel | onUpdateContract | TeamManagement (Staff) | Contract negotiation: update the terms. |
| `OnUpdatePitCrewDevelopmentPlan` | `project/modules/staff/pitCrew/TMPitCrewDevelopment.js:43` | _TMPitCrewDevelopment | - | TeamManagement (Staff) | Pit-crew development: update the plan. |
| `OnUpdateSelectedDriverFocus` | `project/modules/staff/staffDetails/StaffDetailsDevelopmentPanel.js:142` | _DevelopmentPanel | confirmDriverDevelopmentChanges | TeamManagement (Staff) | Staff development panel: commit the driver development changes. |
| `OnViewStaff` | `project/modules/scouting/ScoutingSearch.js:547` (+2 more sites) | _ScoutingSearch | finishComparisonSelection | TeamManagement (Scouting) | Scouting: view a staff member / finish comparison selection. |
| `OptionsChange` | `project/modules/options/OptionsScreenAdjustments.js:72` | OptionsScreenAdjustments | sliderJSX | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeCarDevDesignExpertiseGainDifficulty` | `project/modules/options/GameplaySettings.js:88` | _GameplaySettings | onCarDevDesignExpertiseGain | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeCarEngineVolume` | `project/modules/options/AudioSettings.js:70` | _AudioSettings | onCarEngine | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeGamepadVibrationEnabled` | `project/modules/options/ControlSettings.js:49` | _ControlSettings | onGamepadVibrationEnabled | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeGraphicsAPI` | `project/modules/options/DisplaySettings.js:77` | _DisplaySettings | onChangeGraphicsAPI | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeLockCursorFullscreen` | `project/modules/options/ControlSettings.js:43` (+1 more sites) | _ControlSettings | onChangeLockCursorFullscreen | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeLockCursorWindowed` | `project/modules/options/ControlSettings.js:37` (+1 more sites) | _ControlSettings | onChangeLockCursorWindowed | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeMasterVolume` | `project/modules/options/AudioSettings.js:56` | _AudioSettings | onChangeMaster | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeMinimumFontSize` | `project/modules/options/AccessibilitySettings.js:101` | _AccessibilitySettings | onChangeMinimumFontSize | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeMusicVolume` | `project/modules/options/AudioSettings.js:63` | _AudioSettings | onChangeMusic | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeMuteLicensedMusic` | `project/modules/options/AudioSettings.js:105` | _AudioSettings | onMuteLicensedMusic | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeOtherCommentaryVolume` | `project/modules/options/AudioSettings.js:84` | _AudioSettings | onChangeOtherCommentary | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangePauseForDilemmas` | `project/modules/options/GameplaySettings.js:57` (+1 more sites) | _GameplaySettings | onPauseForDilemmasOn | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeRaceCommentaryVolume` | `project/modules/options/AudioSettings.js:77` | _AudioSettings | onChangeRaceCommentary | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeRaceSimAIDifficulty` | `project/modules/options/GameplaySettings.js:95` | _GameplaySettings | onRaceSimAIChange | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeResolution` | `project/modules/options/DisplaySettings.js:91` | _DisplaySettings | onChangeResolution | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeResolutionScale` | `project/modules/options/GraphicsSettings.js:146` | _GraphicsSettings | onChangeResolutionScale | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeSlowForImportantComms` | `project/modules/options/GameplaySettings.js:71` (+1 more sites) | _GameplaySettings | onSlowImportantOn | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeSoundEffectsVolume` | `project/modules/options/AudioSettings.js:91` | _AudioSettings | onChangeSoundEffects | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeTeamCommsVolume` | `project/modules/options/AudioSettings.js:112` | _AudioSettings | onChangeTeamComms | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeUISoundEffectsVolume` | `project/modules/options/AudioSettings.js:98` | _AudioSettings | onChangeUISoundEffects | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeUpscalingMode` | `project/modules/options/GraphicsSettings.js:116` | _GraphicsSettings | onChangeUpscalingMode | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeUpscalingQualityPlugin` | `project/modules/options/GraphicsSettings.js:136` | _GraphicsSettings | onChangeUpscalingQualityPlugin | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeUpscalingQualityUnreal` | `project/modules/options/GraphicsSettings.js:126` | _GraphicsSettings | onChangeUpscalingQualityUnreal | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeVSync` | `project/modules/options/DisplaySettings.js:97` | _DisplaySettings | onTogglevSync | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeWidescreenRaceView` | `project/modules/options/DisplaySettings.js:104` | _DisplaySettings | onToggleWidescreenRaceView | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsChangeWindowMode` | `project/modules/options/DisplaySettings.js:84` | _DisplaySettings | onChangeWindowMode | Options (both apps) | Options: change a setting (name encodes the setting; arg: new value). |
| `OptionsClosed` | `project/modules/options/Settings.js:132` (+1 more sites) | _Settings | onGoBack | Options (both apps) | Options screen closed without saving. |
| `OptionsConfirmChanges` | `project/modules/options/Settings.js:140` (+2 more sites) | _Settings | onGoBack | Options (both apps) | Player confirmed the options changes. |
| `OptionsEnableMonoAudio` | `project/modules/options/AccessibilitySettings.js:84` | _AccessibilitySettings | onEnableMonoAudio | Options (both apps) | Options > Accessibility: toggle mono audio. |
| `OptionsEnableSubtitles` | `project/modules/options/AccessibilitySettings.js:77` | _AccessibilitySettings | onEnableSubtitles | Options (both apps) | Options > Accessibility: toggle subtitles. |
| `OptionsMuteBackgroundNoises` | `project/modules/options/AccessibilitySettings.js:67` | _AccessibilitySettings | onMuteBackground | Options (both apps) | Options > Accessibility: mute background noises. |
| `OptionsOpened` | `project/modules/options/Settings.js:40` | _Settings | - | Options (both apps) | Options screen opened. |
| `OptionsResetToDefault` | `project/modules/options/Settings.js:122` | _Settings | onConfirmResetToDefault | Options (both apps) | Player confirmed resetting options to default. |
| `OptionsRevertChanges` | `project/modules/options/Settings.js:168` | _Settings | onRevertChanges | Options (both apps) | Player reverted the options changes. |
| `OptionsUIColorChange` | `project/modules/options/AccessibilitySettings.js:91` | _AccessibilitySettings | onChangeUIColor | Options (both apps) | Options: change the UI accent colour. |
| `PartsCFDFocused` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/newDesign/CarBuildDesignOverview.js:197` (+1 more sites) | _CarBuildDesignOverview | onFocusCFD | TeamManagement (Cars) | Cars: the CFD figure was focused. |
| `PartsDesignInspectChange` | `project/modules/cars/CarPartItem.js:105` | _CarPartTypeItem | onFocus | TeamManagement (Cars) | Cars: design inspection selection changed. |
| `PartsManufactureDesignInspectChange` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/manufacture/CarBuildManufacturePanel.js:142` | _CarBuildManufacturePanel | sendDesignInspectEvent | TeamManagement (Cars) | Cars: manufacture design inspection changed. |
| `PartsManufactureDesignSelection` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/manufacture/CarBuildManufacturePanel.js:129` (+1 more sites) | _CarBuildManufacturePanel | onSelectedDesignChanged | TeamManagement (Cars) | Cars: a manufacture design was selected. |
| `PartsManufacturePartInspectChange` | `project/modules/cars/CarPartItem.js:101` | _CarPartTypeItem | onFocus | TeamManagement (Cars) | Cars: manufacture part inspection changed. |
| `PartsProjectSelected` | `project/modules/cars/currentProjects/CarsCurrentProjectInfo.js:31` (+1 more sites) | _CarsCurrentProjectInfo | - | TeamManagement (Cars) | Cars: a development project was selected. |
| `PartsProjectsCancelProject` | `project/modules/cars/currentProjects/CarsCurrentProjects.js:257` | _CarsCurrentProjects | onDeleteProject | TeamManagement (Cars) | Cars: cancel a development project. |
| `PartsStatFocused` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/CarStatComparisonBar.js:105` (+1 more sites) | _CarStatComparisonBar | onFocus | TeamManagement (Cars) | Cars: a part stat was focused. |
| `PartsWindtunnelFocused` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/newDesign/CarBuildDesignOverview.js:215` (+1 more sites) | _CarBuildDesignOverview | onFocusWindtunnel | TeamManagement (Cars) | Cars: the wind-tunnel figure was focused. |
| `PitStopCallInCar` | `project/modules/raceWeekend/RaceWeekendEvents.js:93` (+1 more sites) | - | PitStopCallInCar | RaceWeekend | Race: call a car into the pits (arg: carID, pitLaneReason). |
| `PitStopCancelCallInCar` | `project/modules/raceWeekend/RaceWeekendEvents.js:97` (+1 more sites) | - | PitStopCancelCallInCar | RaceWeekend | Race: cancel calling a car into the pits. |
| `PitStopDoNotCallInCar` | `project/modules/raceWeekend/driverPanel/pit/PitOverview.js:167` | _PitPracticeQualifyingOverview | cancelCallInDialog | RaceWeekend | Pit overview dialog: player chose not to call the car in. |
| `PitStopReleaseRequested` | `project/modules/raceWeekend/RaceWeekendEvents.js:82` | - | DriverCommandSendOutCar | RaceWeekend | Release a car from its pit box (arg: carID). |
| `PitStopRequested` | `project/modules/raceWeekend/PitOptionsView.js:199` | _PitOptionsView | onConfirmRacePit | RaceWeekend | Race: player boxed a car this lap (args: carID, has tyres, tyreID, has components, has penalty). |
| `PlayOrSimClosed` | `project/modules/raceWeekend/simulatingSessions/SessionOptions.js:123` | SessionOptions | onConfirmGoTo | RaceWeekend | Session-options dialog confirmed: "Manage / Go to session". |
| `PlayerFirstNameChange` | `project/modules/frontend/components/CareerSettings.js:80` (+2 more sites) | CareerSettingsDataWrapper | getGuidanceInfo | Frontend | Career settings: commit the player first name. |
| `PlayerLastNameChange` | `project/modules/frontend/components/CareerSettings.js:89` (+2 more sites) | CareerSettingsDataWrapper | getGuidanceInfo | Frontend | Career settings: commit the player last name. |
| `PostRacePitStopSummaryClosed` | `project/modules/raceWeekend/PostRaceSummaryViewPitStops.js:95` | _PostRaceSummaryViewPitStops | onContinue | RaceWeekend | Post-race pit-stop summary: player pressed Continue. |
| `PostRaceSummaryClosed` | `project/modules/raceWeekend/PostRaceSummaryView.js:146` (+1 more sites) | _PostRaceSummaryView | onContinue | RaceWeekend | Post-race summary continue. |
| `PreSessionClosed` | `project/modules/raceWeekend/PresessionView.js:141` (+1 more sites) | _PreSessionView | onPenaltyDialogClosed | RaceWeekend | Pre-session strategy screen: player pressed the session start button (P1/P2/P3/Q/Race). |
| `PreviousMonthSelected` | `project/modules/calendar/CalendarContents.js:288` | _CalendarContents | onMonthDown | TeamManagement (Calendar) | Calendar: player pressed the month-back arrow. |
| `QualifyingSetAdditionalLapsOfFuel` | `project/modules/raceWeekend/strategy/CarSetupFuel.js:173` | CarSetupFuelQualifying | onadditionalLapsOfFuelChange | RaceWeekend | Qualifying run plan: set additional fuel laps. |
| `QualifyingSetFlyingLapCount` | `project/modules/raceWeekend/strategy/CarSetupFuel.js:158` | CarSetupFuelQualifying | onFlyingLapsChange | RaceWeekend | Qualifying run plan: set the number of flying laps. |
| `QualifyingSetIncludeCooldownLaps` | `project/modules/raceWeekend/strategy/CarSetupFuel.js:166` | CarSetupFuelQualifying | onCooldownLapsChange | RaceWeekend | Qualifying run plan: toggle cooldown laps. |
| `QualifyingSetReplacePart` | `project/modules/raceWeekend/RaceWeekendEvents.js:113` | - | QualifyingSetReplacePart | RaceWeekend | Qualifying: player chose to replace a part before the session (arg: carID, partType, toggled). |
| `QuitGame` | `project/modules/frontend/components/MainMenu.js:222` (+1 more sites) | _MainMenu | onConfirmDialogQuit | Frontend | Player confirmed Exit / Quit; the engine tears the UI down and leaves to the front-end (or quits). |
| `RaceDataViewOpened` | `project/modules/raceWeekend/RaceView.js:425` | _RaceView | setViewMode | RaceWeekend | Race view: player switched to the data view (sim pauses). |
| `RaceStrategyViewOpened` | `project/modules/raceWeekend/RaceView.js:432` | _RaceView | setViewMode | RaceWeekend | Race view: player switched to the strategy view (sim pauses). |
| `RaceViewChanged` | `project/utils/RaceWeekendNavigation.js:65` | - | doNavigation | shared util | Race-weekend navigation changed screen; reports the screen id to the engine. |
| `RaceWeekendModuleReady` | `project/modules/raceWeekend/RaceWeekendModule.js:29` | _RaceWeekendModule | - | RaceWeekend | RaceWeekend app mounted; tells the engine the race-weekend UI is live and ready. |
| `RandomiseCalendarTemplate` | `project/modules/circuitList/RaceSchedulePopup.js:42` | RaceSchedulePopup | onContinue | TeamManagement (Circuit list) | Create-a-season: randomise the race-calendar template. |
| `RemovePlannedEngagementActivity` | `project/modules/finance/Obligations.js:193` (+1 more sites) | _SponsorshipPlan | showActivityChoices | TeamManagement (Finance) | Sponsorship plan: remove a planned engagement activity. |
| `ResearchStageChanged` | `project/modules/cars/carBuildAndPartsDevelopment/PartDevelopment/research/CarBuildResearch.js:82` | _CarBuildResearch | - | TeamManagement (Cars) | Research screen stage changed (UI bookkeeping). |
| `ResetEngagementActivities` | `project/modules/finance/Obligations.js:99` (+1 more sites) | _SponsorshipPlan | _currentPeriod | TeamManagement (Finance) | Sponsorship plan: reset the engagement activities. |
| `RestartScenario` | `project/modules/raceWeekend/RaceWeekendEvents.js:74` | - | RestartScenario | RaceWeekend | Options dialog: player confirmed restarting the scenario from the beginning. |
| `RetryAuth` | `project/modules/frontend/components/MainMenu.js:178` | _MainMenu | onSelectSwitchAuthError | Frontend | Player pressed Retry on the Switch/auth error dialog. |
| `RetryScenario` | `project/modules/frontend/components/ScenarioMode/PostRace/ScenarioPostSessionScreen.js:97` | ScenarioPostSessionScreen | retryChallengedSelected | Frontend (scenario) | Scenario post-session screen: player asked to retry the challenge. |
| `RevertToBestSetup` | `project/modules/raceWeekend/strategy/CarSetupParts.js:56` (+1 more sites) | _CarSetupParts | onRevertConfirm | RaceWeekend | Player confirmed reverting the car setup to the best known setup. |
| `SelectSecondarySponsorPackage` | `project/modules/preSeason/PreSeasonPackageSelection.js:119` | PreSeasonPackageSelection | onSelectOptionalIncentive | TeamManagement (Pre-season) | Pre-season sponsorship: select a secondary sponsor. |
| `SelectSponsorRaceBonus` | `project/modules/sponsorTargets/SponsorTargetsPreRaceDriver.js:219` | _SponsorTargetsPreRaceDriver | onOptionSelected | TeamManagement (Sponsors) | Pre-race sponsor targets: player picked one of the three bonus options (args: driverID, raceID, index). |
| `SelectTitleSponsorPackage` | `project/modules/preSeason/PreSeasonTitleSponsorSelection.js:92` | PreSeasonTitleSponsorSelection | onFocusTitleSponsor | TeamManagement (Pre-season) | Pre-season sponsorship: select the title sponsor. |
| `SelectedTyreChanged` | `project/modules/raceWeekend/pitOptions/PitOptionsTyres.js:159` (+1 more sites) | _PitOptionsTyres | tyreID | RaceWeekend | Player changed the selected tyre compound (pit options or car setup). |
| `SessionOverviewClosed` | `project/modules/raceWeekend/SessionOverview/SessionOverview.js:149` | _SessionOverview | onAdvanceNext | RaceWeekend | Race-weekend session overview: player pressed Continue; engine should enter the session (see section 3). |
| `SetCameraTarget` | `project/modules/raceWeekend/RaceWeekendEvents.js:26` | - | SetCameraTarget | RaceWeekend | Race view: focus the camera on a car id. |
| `SetCameraType` | `project/modules/raceWeekend/RaceWeekendEvents.js:62` | - | SetCameraType | RaceWeekend | Race view: switch camera (Sky vs track). |
| `SetDilemmaResponse` | `project/modules/inbox/InboxDilemmaButton.js:53` (+1 more sites) | _InboxDilemmaButton | onSelectConfirm | TeamManagement (Inbox) | Inbox dilemma: player confirmed the chosen response. |
| `SetPlaybackRate` | `project/modules/raceWeekend/RaceWeekendEvents.js:70` | - | SetPlaybackRate | RaceWeekend | Race view time controls: set the sim playback multiplier (1/2/4/8/16). |
| `SetPracticeSessionSetup` | `project/modules/raceWeekend/RaceWeekendEvents.js:105` | - | SetPracticeSessionSetup | RaceWeekend | Request that the current car setup be used for the practice session (arg: carID, setup). |
| `SetPreviewCarSetup` | `project/modules/raceWeekend/RaceWeekendEvents.js:109` | - | SetPreviewCarSetup | RaceWeekend | Request a preview-only car setup change (arg: carID, setup). |
| `ShowReplay` | `project/modules/raceWeekend/notifications/ReplayNotification.js:152` | _ReplayNotification | - | RaceWeekend | Player accepted the replay notification; opens the replay for a lap. |
| `ShowSimulationResults` | `project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js:68` | SimulatingSessions | onContinue | RaceWeekend | Simulating sessions: continue to the results screen. |
| `SimPauseChange` | `project/modules/raceWeekend/RaceWeekendEvents.js:66` | - | SimPauseChange | RaceWeekend | Race view: pause or unpause the sim for a named UI layer. |
| `SimulateRemainingEvent` | `project/modules/raceWeekend/simulatingSessions/SessionOptions.js:142` | SessionOptions | onConfirmSimEvent | RaceWeekend | Session options: quick-sim every remaining session of this event. |
| `SimulateSingleSession` | `project/modules/raceWeekend/simulatingSessions/SessionOptions.js:132` | SessionOptions | onConfirmQuickSim | RaceWeekend | Session options: quick-sim just the current session. |
| `SimulateToRaceOrSprint` | `project/modules/raceWeekend/simulatingSessions/SessionOptions.js:152` | SessionOptions | onConfirmSimToRace | RaceWeekend | Session options: quick-sim forward only as far as the race/sprint. |
| `SkipCarReleaseCinematics` | `project/modules/raceWeekend/RaceHighlightsCinematicsView.js:130` | _RaceHighlightsCinematicsView | onSkipCinematics | RaceWeekend | Player pressed Skip on the car-release cinematic. |
| `SkipCinematics` | `project/modules/frontend/components/FrontendCinematicsView.js:58` (+6 more sites) | _FrontendCinematicsView | onSkipCinematics | Frontend | Player pressed Skip during an intro/outro/reaction cinematic. |
| `SkipTutorial` | `project/modules/tutorial/TutorialsModule.js:59` | TutorialsModule | onSelectSkip | both apps | Tutorial overlay: player skipped the tutorial. |
| `StaffDevelopmentFocusToggled` | `project/modules/staff/staffDetails/StaffDetailsDevelopmentPanel.js:122` | _DevelopmentPanel | toggleStaffTrainingButton | TeamManagement (Staff) | Staff development: toggle a training focus. |
| `StaffDevelopmentTabOpened` | `project/modules/staff/staffDetails/StaffDetailsDevelopmentPanel.js:30` | _DevelopmentPanel | - | TeamManagement (Staff) | Staff development: a tab was opened. |
| `StaffEngineerTeamChange` | `project/modules/staff/engineerTeam/StaffEngineerTeamContent.js:247` (+1 more sites) | _StaffEngineerTeamContent | onConfirmChangeSize | TeamManagement (Staff) | Staff engineer screen: confirm changing the engineer. |
| `StaffPerformanceStatDecrease` | `project/modules/staff/StaffPerformanceStepper.js:49` (+1 more sites) | _PerformanceStepper | onStepperValueChanged | TeamManagement (Staff) | Staff performance stepper decreased. |
| `StaffPerformanceStatIncrease` | `project/modules/staff/StaffPerformanceStepper.js:45` | _PerformanceStepper | onStepperValueChanged | TeamManagement (Staff) | Staff performance stepper increased. |
| `StaffPitCrewEmphasis` | `project/modules/staff/pitCrew/StaffPitCrew.js:132` (+4 more sites) | _StaffPitCrew | onSetEmphasisBalanced | TeamManagement (Staff) | Pit crew: set the training emphasis. |
| `StaffRaceEngineerSwapDrivers` | `project/modules/staff/driverDetails/DriverSetupPanel.js:88` (+1 more sites) | _DriversSetupPanel | onSelectSwap | TeamManagement (Staff) | Staff: swap the race-engineer assignment between drivers. |
| `StartBuildingRefurbish` | `project/modules/hq/HQActionsPanel.js:138` (+1 more sites) | _HQActionsPanel | onSelectRefurbish | TeamManagement (HQ) | HQ: player confirmed starting a building refurbishment. |
| `StartBuildingUpgrade` | `project/modules/hq/HQActionsPanel.js:134` (+3 more sites) | _HQActionsPanel | onSelectBuild | TeamManagement (HQ) | HQ: player confirmed starting a building upgrade. |
| `StartScenario` | `project/modules/frontend/components/ScenarioMode/ScenarioChallengeSelection.js:103` (+1 more sites) | ScenarioChallengeSelection | onScenarioSelected | Frontend (scenario) | Player picked a scenario challenge and team; begins the scenario (args: scenarioID, scenarioTeamID). |
| `StopAutomaticMode` | `project/modules/raceWeekend/RaceWeekendEvents.js:86` | - | DriverCommandStopAutomaticMode | RaceWeekend | Player took manual control of a car; leave automatic mode. |
| `StrategyAddNewStrategy` | `project/modules/raceWeekend/strategyView/DriverStrategyList.js:142` | _DriverStrategyList | onSelectNewStrategy | RaceWeekend | Race strategy editor operation (`StrategyAddNewStrategy`). |
| `StrategyCancelEditStrategy` | `project/modules/raceWeekend/strategyView/StrategyViewEditor.js:52` | _StrategyViewEditor | headerLabel | RaceWeekend | Race strategy editor operation (`StrategyCancelEditStrategy`). |
| `StrategyDeleteStrategy` | `project/modules/raceWeekend/strategyView/DriverStrategyList.js:127` | _DriverStrategyList | doDeleteStrategy | RaceWeekend | Race strategy editor operation (`StrategyDeleteStrategy`). |
| `StrategySetActiveStrategy` | `project/modules/raceWeekend/strategyView/DriverStrategyList.js:147` | _DriverStrategyList | onSelectStrategy | RaceWeekend | Race strategy editor operation (`StrategySetActiveStrategy`). |
| `StrategyStartEditStrategy` | `project/modules/raceWeekend/strategyView/DriverStrategyItem.js:36` | _DriverStrategyItem | onSelectEdit | RaceWeekend | Race strategy editor operation (`StrategyStartEditStrategy`). |
| `StrategyStintEditDelete` | `project/modules/raceWeekend/strategyView/StintStrategyItem.js:139` | StintStrategyItem | onClickBin | RaceWeekend | Race strategy editor operation (`StrategyStintEditDelete`). |
| `StrategyStintEditNewItem` | `project/modules/raceWeekend/strategyView/StrategyViewEditor.js:138` | _StrategyViewEditor | onSelectNewItem | RaceWeekend | Race strategy editor operation (`StrategyStintEditNewItem`). |
| `StrategyStintEditStartLap` | `project/modules/raceWeekend/strategyView/StintStrategyItem.js:131` | StintStrategyItem | onLapChange | RaceWeekend | Race strategy editor operation (`StrategyStintEditStartLap`). |
| `StrategyStintEditTyreType` | `project/modules/raceWeekend/strategyView/StintStrategyItem.js:123` | StintStrategyItem | onTyreChange | RaceWeekend | Race strategy editor operation (`StrategyStintEditTyreType`). |
| `StrategyStintEditTyreWearStrategy` | `project/modules/raceWeekend/strategyView/StintStrategyItem.js:89` | StintStrategyItem | onChangeTyreWearStrat | RaceWeekend | Race strategy editor operation (`StrategyStintEditTyreWearStrategy`). |
| `StrategyUpdateStrategy` | `project/modules/raceWeekend/strategyView/StrategyViewEditor.js:157` | _StrategyViewEditor | updateStrategy | RaceWeekend | Race strategy editor operation (`StrategyUpdateStrategy`). |
| `SubmitActiveSessionSetup` | `project/modules/raceWeekend/PitOptionsView.js:208` | _PitOptionsView | onSelectSubmitActiveSessionSetup | RaceWeekend | Pit options confirmed outside a race stage: submit the practice/qualifying run plan for a car (arg: carID). |
| `TeamBrandingScreenClosed` | `project/modules/board/TeamBranding.js:27` (+1 more sites) | TeamBranding | onBack | TeamManagement (Board) | Team branding screen: player went back. |
| `TeamColorChange` | `project/modules/frontend/components/TeamBrandingContent.js:143` | _TeamBrandingContent | updateColor | Frontend | Team branding: commit the team colour. |
| `TeamNameChange` | `project/modules/frontend/components/CreateATeamProfile.js:166` (+1 more sites) | CreateATeamProfile | onTeamNameCommit | Frontend | Create-a-team: commit the team name. |
| `TimelineDayAnimateComplete` | `project/modules/teamManagement/TimeProgressionPanel.js:140` | TimeProgressionPanel | onDayChangeAnimateComplete | TeamManagement | The day-change animation on the advance timeline finished; engine may advance again. |
| `TitleSponsorSelected` | `project/modules/preSeason/PreSeasonTitleSponsorSelection.js:86` | PreSeasonTitleSponsorSelection | onSelectTitleSponsor | TeamManagement (Pre-season) | Pre-season sponsorship: a title sponsor was selected. |
| `ToggleCustomTeam` | `project/modules/frontend/components/MainMenu.js:188` (+1 more sites) | _MainMenu | gotoCATNewGame | Frontend | Player chose "Create a Team" from the main menu; switches CAS into custom-team mode. |
| `TutorialStepComplete` | `project/modules/tutorial/TutorialsModule.js:51` | TutorialsModule | onSelectNext | both apps | Tutorial overlay: player pressed Next on a step. |
| `UpdateCarSetup` | `project/modules/raceWeekend/RaceWeekendEvents.js:101` | - | UpdateCarSetup | RaceWeekend | Commit a car setup change for a car (arg: carID, setup). |
| `UpdateRaceCalendarModifier` | `project/modules/circuitList/RaceSchedulePopup.js:57` | RaceSchedulePopup | onRaceScheduleModifierChanged | TeamManagement (Circuit list) | Create-a-season: change a race-calendar modifier. |
| `WarehouseAerodynamicInspectedItem` | `project/modules/cars/warehouse/aerodynamicParts/CarsWarehouseAerodynamicContent.js:118` | _CarsWarehouseAerodynamicPartContent | sendDesignInspectEvent | TeamManagement (Cars) | Warehouse: inspect an aerodynamic part. |
| `WarehouseAerodynamicInstallOnCarConfirmed` | `project/modules/cars/carBuildAndPartsDevelopment/carBuild/CarBuildPartButton.js:254` (+1 more sites) | _CarBuildPartButton | onSelectReplacePart | TeamManagement (Cars) | Warehouse: confirm installing an aerodynamic part on the car. |
| `WarehouseAerodynamicSelectedAddNewDesign` | `project/modules/cars/warehouse/aerodynamicParts/CarsWarehouseAerodynamicContent.js:128` | _CarsWarehouseAerodynamicPartContent | onConfirmDesignSelection | TeamManagement (Cars) | Warehouse: choose to add a new aerodynamic design. |
| `WarehouseMenuTypeSelection` | `project/modules/cars/warehouse/CarsWarehouse.js:68` (+2 more sites) | _CarsWarehouse | navigateToPartPage | TeamManagement (Cars) | Warehouse: navigate to a part page. |
| `WarehousePowertrainInspectedItem` | `project/modules/cars/warehouse/powertrainParts/CarsWarehousePowertrainContentView.js:121` | _CarsWarehousePowertrainPartContent | sendDesignInspectEvent | TeamManagement (Cars) | Warehouse: inspect a powertrain part. |
| `WarehousePowertrainInstallOnCarConfirmed` | `project/modules/cars/warehouse/powertrainParts/CarsWarehousePowertrainTooltipPanel.js:94` | _CarWarehousePowertrainTip | installOnCar | TeamManagement (Cars) | Warehouse: confirm installing a powertrain part on the car. |
| `WarehousePowertrainOnNewPartConfirmed` | `project/modules/cars/warehouse/powertrainParts/CarsWarehousePowertrainSelectionMenuView.js:214` | _CarsWarehouseSelectionMenu | createPart | TeamManagement (Cars) | Warehouse: confirm creating a new powertrain part. |
| `WarehousePowertrainSelectedAddNewDesign` | `project/modules/cars/warehouse/powertrainParts/CarsWarehousePowertrainContentView.js:130` | _CarsWarehousePowertrainPartContent | onConfirmDesignSelection | TeamManagement (Cars) | Warehouse: choose to add a new powertrain design. |
| `acceptEULA` | `project/modules/frontend/components/EULADialog.js:42` | EULADialog | onAccept | Frontend | Player accepted the EULA (sent 600 ms after the click). |
| `latestScenarioSeen` | `project/modules/frontend/components/ScenarioMode/NewScenarioDialog.js:80` (+1 more sites) | NewScenarioDialog | onContinue | Frontend (scenario) | Player dismissed the latest-scenario promo dialog. |
| `onCLose` | `project/apps/LegalApp.js:103` | _Legal | onAnimPlayBack | Frontend | Legal/health-tips screen: close animation finished. |
| `onReady` | `project/modules/frontend/FrontendModule.js:25` (+1 more sites) | _FrontendModule | - | Frontend | App bootstrap: the UI document has finished initialising (sent once per app). |
| `onSelect` | `project/apps/LegalApp.js:80` | _Legal | onSelect | Frontend | Legal/health-tips screen: player selected (arg: healthTipVisible). |
| `patchNotesSeen` | `project/modules/frontend/components/PatchUpdateDialog.js:30` | PatchUpdateDialog | onContinue | Frontend | Player dismissed the patch-notes / update dialog; marks the notes as read. |
| `viewPatchNotes` | `project/modules/frontend/components/MainMenu.js:242` (+1 more sites) | _MainMenu | onSelectPatchNotesOpen | Frontend | Player opened the patch-notes viewer from the main-menu carousel. |

### 2.4 Events raised from more than one place (progression-relevant only)

| event | all sites |
|---|---|
| `AdvanceToNextEvent` | [`ContinueButtonContainer.js:155`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L155) (two-press confirm), [`ContinueButtonContainer.js:176`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L176) (`TeamManagementContinue`), [`ContinueButtonContainer.js:192`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L192) (`ToRaceWeekend`/`ToRace`), [`SponsorTargetsPreRaceDriver.js:188`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/sponsorTargets/SponsorTargetsPreRaceDriver.js#L188), [`TimeProgressionPanel.js:152`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TimeProgressionPanel.js#L152) (the STOP button) |
| `AdvanceToNextStage` | [`DriverPracticeGoals.js:60`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/DriverPracticeGoals.js#L60), [`FinancesView.js:44`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/FinancesView.js#L44), [`PostPracticeConfidenceView.js:61`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostPracticeConfidenceView.js#L61), [`PostPracticeSummaryView.js:81`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostPracticeSummaryView.js#L81), [`PostQualifyingSummaryView.js:135`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostQualifyingSummaryView.js#L135), [`PostQualifyingSummaryView.js:139`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostQualifyingSummaryView.js#L139), [`SessionResults.js:18`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/sessionResults/SessionResults.js#L18) |
| `PreSessionClosed` | [`PresessionView.js:141`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L141) (after penalty dialog OK), [`PresessionView.js:250`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L250) (direct continue) |
| `OnStageBackwards` | [`PresessionView.js:237`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L237), [`SessionOptions.js:68`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SessionOptions.js#L68), [`PostRaceSummaryViewPitStops.js:265`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryViewPitStops.js#L265) |
| `SkipCinematics` | `project/modules/frontend/components/FrontendCinematicsView.js:58`, `project/modules/raceWeekend/IntroView.js:20`, `project/modules/raceWeekend/RaceHighlightsCinematicsView.js:133`, `project/modules/raceWeekend/RaceIntroCinematicsView.js:74`, `project/modules/raceWeekend/RaceOutroCinematicsView.js:74`, `project/modules/raceWeekend/RaceReactionsCinematicsView.js:74`, `project/modules/raceWeekend/SummaryView.js:19` |
| `PostRaceSummaryClosed` | [`PostRaceSummaryView.js:146`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryView.js#L146), [`PostRaceSummaryView.js:150`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryView.js#L150) |

---

## 3. Progression commands

This is the section the task asked to concentrate on. It closes with the answer to "which event
starts a race session", which is **none of them** (proven), plus precisely what the confirm button
does instead (proven) and what that implies for the bridge (inferred).

### 3.1 `ContinueGame` — loads `Save.Saves.continueSlotName`, and passes no argument

**Raising site:** [`project/modules/frontend/components/MainMenu.js:147-149`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L147)

```js
onSelectContinue = () => {
    (0, Engine_1.sendEvent)('ContinueGame');
};
```

**`ContinueGame` takes no arguments.** It is the only raising site in the export. The Continue
button is [`MainMenu.js:66`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L66),
and the save context it *displays* is passed as a prop, not as an event argument:

```js
preact.h(FrontendContinueButton_1.FrontendContinueButton, { disabled: !areTracksDownloaded,
    saveContext: currentSaveCtx, isAutoSave: autoSave, onSelect: this.onSelectContinue }),
```

**Where the slot name comes from.** `MainMenu` never sends it; it only *shows* it.

- The DataStore property is declared in [`project/data/Saves.js:59`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/Saves.js#L59):

```js
preact.h(DataStoreSchema_1.Prop, { id: 'continueSlotName', string: true, generator_value: 'autosave' }),
```

- `MainMenu` listens to it and turns it into a slot context purely for display:
  - [`MainMenu.js:33`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L33) — `addPropertyListener(this.SAVES_CONTEXT, 'continueSlotName', this.onContinueSlotNameChange)`
  - [`MainMenu.js:118-127`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L118) — turns the name into `currentSaveCtx = ['Save','Saves',<name>]`, skipping it if `corruptSave`
  - [`MainMenu.js:239`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L239) — initial read
- `currentSaveCtx` then feeds only `FrontendContinueButton` (`MainMenu.js:66`) and the "has a save?" booleans (`MainMenu.js:151`, `:160`).

So: **the native game owns `continueSlotName`.** The UI's only contribution is the default
literal `'autosave'` at [`Saves.js:59`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/Saves.js#L59). The engine writes the real value (which slot
Continue will resume) and then performs the load itself. **inferred**, but strongly: the UI sends
no slot with the event, so the engine cannot be getting the slot from the event.

**Note a casing discrepancy, unproven relevance:** the default is `'autosave'`
([`Saves.js:59`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/Saves.js#L59)), the per-slot contexts are `'autoSave'` / `'save1'..'save29'`
([`Saves.js:13`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/Saves.js#L13)). The GUI-driven value is very likely
`'autoSave'` (matching the contexts), so patching `continueSlotName` to a chosen save is plausible
but **inferred and untested**.

**Practical consequence for co-op:** `ContinueGame` is *not* a way to choose a save. `LoadGame <slot>`
is. `ContinueGame` means "resume whatever the engine considers the continue slot".

### 3.2 `LoadGame` — one argument, the slot name

**Two raising paths, both in `LoadSaveScreen`.**

Fast path (not in-game), [`LoadSaveScreen.js:125-130`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L125):

```js
onSelectLoadData = () => {
    if (this.props.inGame != undefined && !this.props.inGame) {
        const slotName = DS.getValueAsString(this.state.selectedSlotCtx, 'slotName');
        (0, Engine_1.sendEvent)('LoadGame', slotName);
        this.setState({ fadeToBlack: true });
    }
    else { ... confirmation dialog ... }
```

Confirm path (shared with the in-game flow), [`LoadSaveScreen.js:147-149`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L147):

```js
onConfirmChoice = () => {
    let slotName = DS.getValueAsString(this.state.selectedSlotCtx, 'slotName');
    (0, Engine_1.sendEvent)(this.getConfirmSelectionEvent(), slotName);
```

`getConfirmSelectionEvent()` maps the confirmation state to an event name
([`LoadSaveScreen.js:163-176`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L163)):

| `ConfirmationState` | event | line |
|---|---|---|
| `LoadGame` | `LoadGame` | [`:169`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L169) |
| `CorruptData` | `RestoreData` | `:171` |
| `DeleteData` | `DeleteGame` | `:173` |
| `OverwriteData` | `SaveGame` | `:175` |

**Where the argument is read from:** `DS.getValueAsString(this.state.selectedSlotCtx, 'slotName')`,
i.e. the DataStore property `slotName` on the slot context the player selected. Those contexts and
their `slotName` values are declared in [`Saves.js:12-24`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/Saves.js#L12):

```js
for (let i = 0; i < 30; i++) {
    const contextID = i == 0 ? 'autoSave' : ('save' + i);
```

so the key space is exactly `autoSave`, `save1` … `save29` — 30 names.

**`RestoreData`, `DeleteGame` and `SaveGame` are event names that the literal scan does *not*
list as strings**, because they are returned from `getConfirmSelectionEvent`, not written inline at
the `sendEvent` call. Treat them as real events. (They are absent from the 352 table for that
reason — see §5.)

**Confirmed-by-the-project:** [`LOAD-SAVE-CALL.md`](E:/Downloads/coop-anything-main/local/f1-manager-2024/LOAD-SAVE-CALL.md) already documents this call
and the 30-name slot space; the `file:line` above (128) matches this re-read exactly.

### 3.3 `AdvanceToNextEvent` — and a serious caveat about reading `ContinueButtonContainer`

`AdvanceToNextEvent` is raised from five places (§2.4). The one that matters is the shared Advance
button, `ContinueButtonContainer`, mounted by `TeamManagementModule`
([`TeamManagementModule.js:107`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TeamManagementModule.js#L107)), `Inbox`
([`Inbox.js:173`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/inbox/Inbox.js#L173)) and indirectly others.

**Read the guard first.** [`ContinueButtonContainer.js:149-153`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L149):

```js
onSelect = () => {
    if (Engine.isCoherentPlayer()) {
        DS.setValue(['MainHud'], 'advancing', true);
        return;
    }
```

In the shipped game `isCoherentPlayer()` is **false** (§1.4), so this block is dead and the *rest*
of `onSelect` is what actually runs. (In the preview harness the button only sets a flag and the
harness advances the day itself — `TimeProgressionPanel.js:102-103` schedules `DebugNextDay`, which
does `DS.setValue(['Calendar'], 'currentDay', ...)` at `:143-145`.)

**The two-press behaviour.** The live path is:

```js
if (this.state.isAdvancing) {                 // ContinueButtonContainer.js:154
    sendEvent('AdvanceToNextEvent');          // :155
    return;                                   // :156
}
switch (this.state.advanceType) { ... }       // :158
```

`isAdvancing` is DataStore-backed, not local: it mirrors `['MainHud'].advancing`
([`ContinueButtonContainer.js:29`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L29)
`addPropertyListener(['MainHud'], 'advancing', this.onAdvancingChange)`), and the render swaps the
button to the STOP face while it is true
([`ContinueButtonContainer.js:47`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L47)
`const advanceType = isAdvancing ? AdvanceType.TeamManagementStop : state.advanceType`).

So the real sequence for "Continue" is:

1. Press 1 → `advanceType === TeamManagementContinue` → `this.setState({ isAdvancing: true })` then
   `sendEvent('AdvanceToNextEvent')` ([`ContinueButtonContainer.js:174-177`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L174)).
   The engine begins advancing and sets `MainHud.advancing = true`.
2. While advancing, the panel [`TimeProgressionPanel`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TimeProgressionPanel.js#L74) shows the **STOP** button.
3. Press 2 on STOP → `onStopAdvancing` → `sendEvent('AdvanceToNextEvent')`
   ([`TimeProgressionPanel.js:146-153`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TimeProgressionPanel.js#L146)). The engine interprets a second
   `AdvanceToNextEvent` while already advancing as "stop".

**This is why a single scripted `AdvanceToNextEvent` can look swallowed:** the *same event name*
means start-then-stop. Firing it twice in quick succession can start and immediately stop the
advance. This is a real hazard for the bridge's existing `Advance <n>` loop, which spaces repeats
by 1200 ms ([`bridge.js`](E:/Downloads/coop-anything-main/local/f1-manager-2024/pak/f1manager24/Content/UIGameface/js/coop/bridge.js) `advance` branch).

Also note: `isAdvancing` is reset by the engine writing `MainHud.advancing = false`, which
`TimeProgressionPanel` observes at [`TimeProgressionPanel.js:18`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TimeProgressionPanel.js#L18).
`advanceType === ToPerformanceTargets` also forces it false
([`ContinueButtonContainer.js:117-119`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L117)).

### 3.3.1 Every `AdvanceType` value: label and what the button actually does

`AdvanceType` enum: [`AdvanceTypesData.js:7-49`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/AdvanceTypesData.js#L7)
(43 values, `Continue = 0` … `ToRaceWeekend = 42`). Labels from `getButtonData`
([`AdvanceTypesData.js:51`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/AdvanceTypesData.js#L51) onward).

Classification is against `ContinueButtonContainer.onSelect` + `getAdvanceTypeScreen`. There are
three behaviours: **sends** `AdvanceToNextEvent`; **navigates** to another TeamManagement screen;
**does nothing** (no case matches, `getAdvanceTypeScreen()` returns `undefined`).

| # | AdvanceType | label locstring | behaviour |
|---|---|---|---|
| 0 | `Continue` | `[CONTINUE]` (default arm) | **does nothing** — falls to `default:`, `getAdvanceTypeScreen()` returns `undefined` ([`:207-213`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L207)) |
| 1 | `ToRequiredResponse` | `[CANNOT_ADVANCE_REQUIRES_RESPONSE]` / `[CANNOT_ADVANCE_GOTO_MAIL]` | **navigates** → `ScreenID.Inbox` ([`:159-166`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L159)) |
| 2 | `ToImportantEmail` | `[ADVANCE_IMPORTANT]` / `[CANNOT_ADVANCE_GOTO_MAIL]` | **navigates** → `ScreenID.Inbox` ([`:167-173`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L167)) |
| 3 | `TeamManagementContinue` | `[CONTINUE]` | **sends** `AdvanceToNextEvent` ([`:174-178`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L174)) |
| 4 | `TeamManagementStop` | `[STOP]` | **sends** `AdvanceToNextEvent` — via `TimeProgressionPanel.onStopAdvancing` ([`TimeProgressionPanel.js:152`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TimeProgressionPanel.js#L152)); also fires via the `isAdvancing` early-return |
| 5 | `ToRaceBonusOptions` | `[EVENTS_TO_RACE_TARGETS]` | **navigates** → `ScreenID.SponsorTargetsPreRaceDriver` ([`:184-188`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L184)) |
| 6 | `ToDriverScout` | `[ADVANCE_BLOCKED]` + driver sublabel | **navigates** → `ScoutingSearchDrivers` |
| 7 | `ToDesignerScout` | `[ADVANCE_BLOCKED]` / `[CANNOT_ADVANCE_NO_CHIEF_DESIGNER]` | **navigates** → `ScoutingSearchChiefDesigners` |
| 8 | `ToAerodynamicsScout` | `[ADVANCE_BLOCKED]` / `[CANNOT_ADVANCE_NO_AERODYNAMICS]` | **navigates** → `ScoutingSearchHOA` |
| 9 | `ToEngineerScout` | `[ADVANCE_BLOCKED]` / `[CANNOT_ADVANCE_NO_RACE_ENGINEER]` | **navigates** → `ScoutingSearchRaceEngineers` |
| 10 | `ToMechanicScout` | `[ADVANCE_BLOCKED]` / `[CANNOT_ADVANCE_NO_SPORTING_DIRECTOR]` | **navigates** → `ScoutingSearchChiefMechanic` |
| 11 | `ToCarBuild` | `[ADVANCE_BLOCKED]` / `[PRE_SESSION_MISSING_CAR_PART]` | **navigates** → `CarBuild1` or `CarBuild2` ([`:238-247`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L238)) |
| 12 | `ToPerformanceTargets` | `[ADVANCE_IMPORTANT]` / `[CANNOT_ADVANCE_PERFORMANCE_TARGETS]` | **does nothing** — empty `case`, falls through to `:207` default; `getAdvanceTypeScreen()` has an empty block ([`:248-250`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L248)) |
| 13 | `ToPreSeasonFlow` | `[ADVANCE_TO_NEW_SEASON]` / `[ADVANCE_TO_NEW_SEASON_DESCR]` | **navigates** → `WelcomeToNewSeason` |
| 14 | `ToPitCrewDevelopment` | `[TM_PITCREW_TRAIN_SCHEDULE]` / `[ADVANCE_TO_PITCREW_TRAINING_DESC]` | **navigates** → `TMPitCrewDevelopment` ([`:179-183`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L179)) |
| 15 | `ToSponsorPackageSelection` | `[PRE_SEASON_SPONSOR_SELECTION]` / `[SPONSORSHIP_SELECT_SPONSORS_SUBLABEL]` | **navigates** → `SponsorshipSelection` ([`:195-198`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L195)) |
| 16 | `ToEngagementActivitiesSelection` | `[EVENT_SPONSORSHIP_SPONSOR_ACTIVITY_SCHEDULE]` / `[SPONSORSHIP_SELECT_ENGAGEMENT_ACTIVITIES_SUBLABEL]` | **navigates** → `SponsorshipPlan` ([`:200-206`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L200)) |
| 17 | `ToPracticeSessionPrep` | `[TO_NEXT_EVENT_SESSION_PREP_TITLE]` / `[TO_NEXT_EVENT_SESSION_PREP_SUBTITLE]` | **does nothing** — no case, no screen |
| 18 | `ToPracticeSession1` | `[TO_NEXT_EVENT_TO_P1]` | **does nothing** — no case, no screen |
| 19 | `ToPracticeSession2` | `[TO_NEXT_EVENT_TO_P2]` | **does nothing** |
| 20 | `ToPracticeSession3` | `[TO_NEXT_EVENT_TO_P3]` | **does nothing** |
| 21 | `ToQualifying` | `[PRE_SESSION_QUALIFYING_START]` / `[TO_NEXT_EVENT_QUALIFYING]` | **does nothing** |
| 22 | `ToRace` | `[PRE_SESSION_RACE_START]` / `[TO_NEXT_EVENT_RACESTART]` | **sends** `AdvanceToNextEvent` ([`:189-193`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L189)) |
| 23 | `ToNextDesignScreen` | `[CONTINUE]` | **does nothing** |
| 24 | `ConfirmTraining` | `[CONFIRM]` | **does nothing** |
| 25 | `ConfirmDesign` | `[CONFIRM]` | **does nothing** |
| 26 | `ConfirmResearch` | `[CONFIRM]` | **does nothing** |
| 27 | `ConfirmManufacture` | `[CONFIRM]` | **does nothing** |
| 28 | `ProposeContract` | `[CONTRACT_NEGOTIATION_OFFER_CONTRACT]` | **does nothing** |
| 29 | `ConfirmContract` | `[SCOUTING_HIRE]` | **does nothing** |
| 30 | `ConfirmRenewal` | `[RENEW_CONTRACT]` | **does nothing** |
| 31 | `NoPatienceNegotiation` | `[ATTITUDE_TYPE_0]` | **does nothing** |
| 32 | `Blocked` | `[TM_PITCREW_CHANGES_NONE]` | **does nothing** |
| 33 | `ChangeTeamBlocked` | `[TEAMPRINCIPAL_CHOOSE_ANOTHER_TEAM]` | **does nothing** |
| 34 | `BlockedSponsorPlan` | `[ADVANCE_BLOCKED]` / `[SPONSORSHIP_PLAN_TARGET_NOT_MET]` | **does nothing** |
| 35 | `ConfirmSponsorPlan` | `[CONFIRM]` / `[SPONSORSHIP_BEGIN_SPONSOR_PLAN]` | **does nothing** |
| 36 | `BlockedPreSeasonSponsors` | `[ADVANCE_BLOCKED]` / `[SPONSORSHIP_PRE_SEASON_BLOCKED]` | **does nothing** |
| 37 | `BlockedPreSeasonSponsorsPackage` | `[ADVANCE_BLOCKED]` / `[SPONSORSHIP_PRE_SEASON_PACKAGE_BLOCKED]` | **does nothing** |
| 38 | `ConfirmPreSeasonSponsors` | `[CONFIRM]` | **does nothing** |
| 39 | `PreRaceTargetsBlocked` | `[ADVANCE_BLOCKED]` / `[SPONSOR_TARGETS_RACE_DRIVER_BLOCKED]` | **does nothing** (its screen handles `onContinue` itself) |
| 40 | `PreRaceTargetsDriver2` | `[CUSTOM_TEAM_NEXT_STEP]` / `[SPONSOR_TARGETS_RACE_DRIVER_2_CONTINUE]` | **does nothing** here; handled in `SponsorTargetsPreRaceDriver.onContinue` |
| 41 | `PreRaceTargetsToRaceWeekend` | `[CONFIRM]` / `[TO_NEXT_EVENT_SESSION_PREP_TITLE]` | **does nothing** here; `SponsorTargetsPreRaceDriver` sends `ConfirmSponsorRaceBonuses` **and** `AdvanceToNextEvent` ([`SponsorTargetsPreRaceDriver.js:186-189`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/sponsorTargets/SponsorTargetsPreRaceDriver.js#L186)) |
| 42 | `ToRaceWeekend` | `[TO_NEXT_EVENT_RACE]` | **sends** `AdvanceToNextEvent` ([`:189-193`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L189)) |

**The important shape of this table:** only 3 of 43 `AdvanceType` values send
`AdvanceToNextEvent` from the shared button (`TeamManagementContinue`, `ToRaceWeekend`, `ToRace`),
plus the `isAdvancing` early-return and the STOP button. Everything else either navigates or is
inert. The actual `advanceType` is not chosen by the UI — it is read from the DataStore property
`['Calendar','NextEvent'].advanceType`
([`ContinueButtonContainer.js:28`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/components/ContinueButtonContainer.js#L28)), i.e. **the engine decides what the Advance
button will do next.** **inferred:** a scripted `AdvanceToNextEvent` therefore does whatever the
engine currently thinks "next" is, and a `LoadGame`/`ContinueGame` that lands on a race weekend sets
`advanceType` to `ToRaceWeekend`/`ToRace` (the "send" set), which is consistent with the field
observation that `AdvanceToNextEvent` from `RaceWeekend` looks like a no-op: the Advance button is a
*TeamManagement* component, and it is not mounted in `RaceWeekend.html`.

That last point is worth stating plainly: **`ContinueButtonContainer` lives in
`project/components/` and is mounted by `TeamManagementModule`, `Inbox` and friends — not by
`RaceWeekendModule`.** Scanning `RaceWeekendModule.js` shows exactly one `sendEvent`, at `:29`
(`RaceWeekendModuleReady`), and no `ContinueButtonContainer` import. Firing `AdvanceToNextEvent`
from `RaceWeekend.html` is firing an event whose only handlers live in a bundle that is not loaded
there. It is **unknown** whether the engine still acts on it.

### 3.4 `NextMonthSelected` / `PreviousMonthSelected`

[`project/modules/calendar/CalendarContents.js:276-295`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/calendar/CalendarContents.js#L276):

```js
onMonthUp = (isRepeatPress, e) => {
    if (this.shouldCalendarMonthGoUp() && (e === undefined || (0, MouseButton_1.isLeftClick)(e.button))) {
        Engine.sendEvent('NextMonthSelected');      // :278
        ...
onMonthDown = (isRepeatPress, e) => {
    if (this.shouldCalendarMonthGoDown() && (e === undefined || (0, MouseButton_1.isLeftClick)(e.button))) {
        Engine.sendEvent('PreviousMonthSelected');  // :288
```

- No arguments.
- Gated by `shouldCalendarMonthGoUp/Down` (`:296` onward), which clamps against
  `TIMESTEPPER_LIMIT` years around `this.state.currentSeason`.
- These are **calendar browsing**, not career progression: they move which month the calendar
  *shows*. **inferred** — the engine handler presumably re-renders the calendar's visible range.
  Nothing in the UI pairs them with a career-clock change.
- One further wrinkle: both handlers call `this.changeCohMonth()` **only** `if (Engine.isCoherentPlayer())`
  (`:280-282`, `:290-292`). In the shipped game the UI does not adjust its own month state, which
  is consistent with the engine being authoritative and pushing the new range back.

### 3.5 Starting a race session — the answer

**There is no Gameface event that starts a race session.** This is a proven statement about the
export, not a failure to find one:

1. The two events named in the brief's hypothesis are not session starts.
   `AdvanceToNextEvent` is a TeamManagement calendar-advance (§3.3) and its only raising
   components are not loaded in `RaceWeekend.html`.
2. The whole `raceWeekend` tree (346 files) contains **85** distinct engine events. The only ones
   that could plausibly begin a session are `PreSessionClosed`, `SessionOverviewClosed`,
   `PlayOrSimClosed`, `SimulateSingleSession`, `SimulateRemainingEvent`,
   `SimulateToRaceOrSprint`, and `AdvanceToNextStage`. Each is triggered by a specific button; none
   is a generic "start". Their handlers, verbatim from the export:
   - `PreSessionClosed` — [`PresessionView.js:242-252`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L242):
     ```js
     onAdvanceNext = () => {
         const powertrainPartViolatesPenaltyCar1 = DS.getValue(['Parts'], 'powertrainPartViolatesPenaltyCar1');
         const powertrainPartViolatesPenaltyCar2 = DS.getValue(['Parts'], 'powertrainPartViolatesPenaltyCar2');
         const showPenaltyWarning = (powertrainPartViolatesPenaltyCar1 || powertrainPartViolatesPenaltyCar2) && !this.state.isScenarioMode;
         if (showPenaltyWarning) { this.setState({ showPenaltyWarning }); }
         else { (0, Engine_1.sendEvent)('PreSessionClosed'); }
     };
     ```
   - `SessionOverviewClosed` — [`SessionOverview.js:147-153`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/SessionOverview/SessionOverview.js#L147):
     ```js
     onAdvanceNext = () => {
         RaceWeekendApp_1.persistentScreenData.resetAllStatesForId(...SessionOverview);
         Engine.sendEvent('SessionOverviewClosed');
         if (Engine.isCoherentPlayer()) { RaceWeekendNav.set(RaceWeekendScreens_1.ScreenID.PreSession); }
     };
     ```
     Note the UI navigation to `PreSession` is **inside the `isCoherentPlayer()` guard**, so in the
     shipped game the UI does not navigate itself — the engine drives the screen change (via
     `SetRaceView`, §3.6).
   - `PlayOrSimClosed` — [`SessionOptions.js:121-129`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SessionOptions.js#L121): the "Manage / Go to session" confirm; sends only the event plus `onResetTabState()`. It does **not** navigate.
   - `SimulateSingleSession` / `SimulateRemainingEvent` / `SimulateToRaceOrSprint` —
     [`SessionOptions.js:130-159`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SessionOptions.js#L130). Each sends one event **and then** navigates to
     `ScreenID.SimulatingSessions`, passing `simulateSessions` as screen config. The sim itself is
     **not** driven from JS.
   - `AdvanceToNextStage` — [`SessionResults.js:17-19`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/sessionResults/SessionResults.js#L17) and six post-session views. This advances
     *within* the weekend after a session has finished; it cannot be the thing that starts one.

3. **The confirming negative.** `SimulatingSessions` — the screen that is on-screen while a session
   simulates — has no start call in it. Its whole lifecycle is a throbber that watches a DataStore
   property:

   - [`SimulatingSessions.js:19`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js#L19):
     `DS.getPropertyListener(['Weekend'], 'isSimulating', value => this.updateThrobberState(!!value, ...))`
   - [`SimulatingSessions.js:20`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js#L20): a 3 s floor so the spinner is visible.
   - [`SimulatingSessions.js:57-62`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js#L57): when `isSimulating` goes false and the floor has elapsed, advance the animation.
   - [`SimulatingSessions.js:67-71`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js#L67): the Continue button, once visible, sends `ShowSimulationResults`.

   The UI **waits for the engine to flip `Weekend.isSimulating`**. It never starts anything. This is
   the single most direct proof that the session sim is engine-owned.

**So what does the confirm button actually do instead?** It sends an event and then waits. Using the
pre-session confirm ([`PresessionView.js:117-125`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L117), `onSelect: this.onAdvanceNext`):

1. `onAdvanceNext` reads two DataStore properties (`Parts.powertrainPartViolatesPenaltyCar1/2`) to
   decide whether to show a penalty dialog.
2. If clear, it sends exactly one event, `PreSessionClosed`, and does nothing else — no
   `engine.call`, no `DS.setValue`, no navigation.
3. **inferred (from the UI's own behaviour):** the engine's handler for `PreSessionClosed` is what
   transitions `RaceSim.sessionState` from `Inactive` to `Active`/`WaitingForLightsOut` and begins
   ticking `RaceSim.raceTime`. The UI cannot be doing it, because (a) it sends nothing else and
   (b) every `DS.setValue` in the game throws in the shipped build (§1.3b).
4. The UI then observes the result. `StageManager` holds a no-op listener on race time —
   [`StageManager.js:74`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/utils/StageManager.js#L74):
   `this.createListener(ContextUtil_1.RACE_SIM_CONTEXT, 'raceTime', () => { });`
   — and `isPreSession()` is defined as `return this._raceTime == 0;`
   ([`StageManager.js:141-143`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/utils/StageManager.js#L141)).
   **That matches the reported symptom exactly:** the sim object exists and is readable, `simtime`
   stays 0, and the UI considers itself "pre-session" — because the engine never acted.

Contrast with the one *simulation* path that does visibly work: quick-sim. There the engine is
handed the work explicitly by `SimulateSingleSession` with the stage list in the screen config, and
the UI then polls `Weekend.isSimulating`. **inferred:** this asymmetry — quick-sim names what to
simulate, "start session" names nothing — suggests the engine's `PreSessionClosed` handler may
require state the UI cannot supply (a validated setup, a chosen tyre set, a session-entered flag),
which is exactly the sort of thing a scripted event cannot fabricate.

### 3.5.1 Binary check: the engine's own binding table, and the commands the UI never sends

The exe was searched for every one of the 352 UI event names (§2.3). **324 of them appear in the
binary as exact ASCII strings.** They are not scattered: they clump into **12 dense clusters**, and
inside those clusters sit the UI's own screen-id constants (`Inbox`, `Headquarters`, `CarsDesign`,
`MainHud`), which are *not* events. **binary** That is the signature of a name→handler binding
table, not coincidence.

**The method validates itself.** §2.2 predicted the `Commands*TabSelected` family could not be
enumerated from the UI because [`DriverPanel.js:411`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/DriverPanel.js#L411)
builds it as `'Commands' + DriverPanelMenu[index] + 'TabSelected'`. The binary contains exactly
`CommandsDriverTabSelected` and `CommandsPitTabSelected` (**binary**, offsets `0x5fac000` and
`0x5fac020`) — the two `DriverPanelMenu` values that are actually used
(`Overview`/`Driver`/`Pit`/`CarSetup` at [`DriverPanel.js`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/DriverPanel.js)). So the clusters are the
binding table, and the cluster scan is trustworthy.

**What that table contains that the UI does not.** Filtering the 12 clusters for identifiers that
are neither a UI event nor a known UE name leaves **53** candidates. Most are further
`OptionsChange*`/CAS/Livery variants. These are the ones that matter:

| name in binary | offset | in UI export? | significance |
|---|---|---|---|
| **`StageProceed`** | `0x5fa7c98` | **absent from the entire export** | sits immediately beside `SessionOverviewClosed` (`0x5fa7ca8`), `AdvanceToNextStage` (`0x5fa7cc0`) and `PreSessionClosed` (`0x5fa7cd8`) — the session-flow block |
| **`TravelToRaceWeekend`** | `0x5fcac28` | **absent** | between `GPPrep` (`0x5fcac20`) and `OnSubmitQuickScout`; the only "travel to race weekend" name anywhere |
| **`ReturnToMenu`** | `0x5fc9cb0` | **absent** | beside `AdvanceToNextEvent` (`0x5fc9c98`) and `ToggleAutoNextDay` (`0x5fc9c80`) |
| `ToggleAutoNextDay` | `0x5fc9c80` | **absent** | beside `AdvanceToNextEvent`; pairs with the UI's `['MainHud'].advancing` (§3.3) |
| `SimulatingSessionsClosingConditionReport` | `0x5fa7e08` | **absent** | in the same block as `ShowSimulationResults` (`0x5fa7dd0`) |
| `SetQuickSim` | `0x5fa8030` | **absent** | beside `SetPlaybackRate` (`0x5fa8040`), `SetCameraType`, `ShowReplay` |

All of these strings are **binary** facts. What they *do* is **unknown** — nothing in the export
reveals a handler body. But two conclusions are now well supported:

1. **The UI's event list is a strict subset of what the game accepts.** `StageProceed` and
   `TravelToRaceWeekend` have no sender in the export at all. They are not "events the UI forgot to
   wire up" — they are entry points the shipping UI never uses, most plausibly driven by native
   code or normally reached by another route.
2. **They are the highest-value candidates for the co-op blocker.** They are the only names in the
   entire binary that read as "proceed with the stage" or "travel to the race weekend", and both sit
   inside the session-flow binding block. This is a much stronger lead than any UI event.

**`StageProceed` vs `PreSessionClosed` — how they would differ (hypothesis, not proof.)**
`PreSessionClosed` is sent by the pre-session *screen* ([`PresessionView.js:250`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L250)),
i.e. it means "this dialog is done". `StageProceed` reads as "advance the weekend stage machinery",
which is one level lower and would not depend on the dialog having been dismissed. If the reported
symptom is that the sim sits at `simtime = 0` while the UI believes it is pre-session, firing
`StageProceed` is the cheaper thing to test first — it may move the stage without the
dialog-dismissal precondition. **This is a hypothesis to test, not a finding.**

**Honest limit of this analysis.** The exe was only read as a byte-string source; the handler bodies
were not decompiled, and no cross-reference scan of the 450 MB `.ecode`/`.xtext` sections was
completed (an attempted per-byte RIP-relative scan proved too slow and was abandoned). So:
handlers are located by *name and neighbourhood*, never by *code*. Anyone continuing should
disassemble the functions that reference these strings rather than repeat a string search.

**Reproduction.**

```python
# strings: 324 of the 352 UI events are present, in 12 dense clusters
import json, re
out = json.load(open("exe_strings.json"))          # (offset, ascii_string) for the whole exe
doc = open("UI-COMMANDS.md").read()
ui  = set(re.findall(r'^\| `([A-Za-z_0-9]+)` \| `project/', doc, re.M))
print(len([s for a,s in out if s in ui]))          # -> 324
```

### 3.6 Session end, results, and return to TeamManagement

The post-session chain, all proven from the export:

| step | screen | action | event |
|---|---|---|---|
| session finishes | engine | flips `Weekend.isSimulating` / `RaceSim.sessionState` | — |
| results shown | `SimulatingSessions` | Continue | `ShowSimulationResults` ([`:68`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js#L68)) |
| | `SessionResults` | NEXT | `AdvanceToNextStage` ([`:18`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/sessionResults/SessionResults.js#L18)) |
| | `PostPracticeSummaryView` | Continue | `AdvanceToNextStage` ([`:81`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostPracticeSummaryView.js#L81)) |
| | `PostQualifyingSummaryView` | Continue (350 ms delayed if both drivers eliminated) | `AdvanceToNextStage` ([`:135`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostQualifyingSummaryView.js#L135), [`:139`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostQualifyingSummaryView.js#L139)) |
| | `DriverPracticeGoals`, `FinancesView`, `PostPracticeConfidenceView` | Continue | `AdvanceToNextStage` |
| post-race | `PostRaceSummaryView` | Continue (forces both standings tabs first) | `PostRaceSummaryClosed` ([`:146`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryView.js#L146), [`:150`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryView.js#L150)) |
| post-race pits | `PostRaceSummaryViewPitStops` | Continue | `PostRacePitStopSummaryClosed` ([`:95`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryViewPitStops.js#L95)) |
| back | any session flow | Back | `OnStageBackwards` |
| leave race weekend | Options → Exit | confirm | `QuitGame` ([`RaceWeekendEvents.js:78`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendEvents.js#L78)) |

**There is no event that returns to TeamManagement.** No event name in the 352 contains
`TeamManagement`, `Return`, `Leave`, `EndRace` or `ExitRace` — verified by substring search over
the enumerated names. **inferred:** the engine swaps the document itself. The UI→UI navigation
mechanism it uses is visible, though: the engine pushes screen changes into the app over event
listeners, not events:

- [`RaceWeekendModule.js:24-25`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L24) — `addListener('SetRaceView', this.onSetRaceView)` and `addListener('SetPitView', this.onSetPitView)`
- [`RaceWeekendModule.js:114-117`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L114) — `onSetRaceView(value)` → `Engine`-driven `RaceWeekendNavigation.set(value)`
- [`TeamManagementModule.js:56`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/teamManagement/TeamManagementModule.js#L56) — `addListener('SetManagementScreenID', ...)`
- [`FrontendModule.js:24`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/FrontendModule.js#L24) — `addListener('SetFrontendScreenID', ...)`

These are **engine→UI** channels (`engine.on`), so they are the correct hook for *detecting* engine
driven transitions, and they are invisible to a wrapper around `engine.trigger`.

---

## 4. Safety classification

Scope: the events that can move the career or the weekend. Label definitions per the brief.

- `SAFE from any screen` — the raising component exists in, or the handler is agnostic to, the
  currently loaded document, and no teardown is implied.
- `SAFE only from <page>` — the handler tears down or replaces the current document.
- `UNKNOWN` — the export cannot say.

The two proven-destructive cases are carried over from [`LOBBY-GATE.md`](E:/Downloads/coop-anything-main/local/f1-manager-2024/LOBBY-GATE.md),
which records a live black screen (process alive, front-end never rebuilt) when `LoadGame` is fired
from inside a career. That observation is **not** re-verified here; it is relied on as prior
evidence.

| event | class | reason / evidence |
|---|---|---|
| `ContinueGame` | **SAFE only from `Frontend.html`** | Only raiser is `MainMenu` ([`MainMenu.js:148`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L148)), a front-end component. It loads a career, i.e. replaces the document. Live-confirmed to black-screen from an in-game page ([`LOBBY-GATE.md`](E:/Downloads/coop-anything-main/local/f1-manager-2024/LOBBY-GATE.md)). |
| `LoadGame` | **SAFE only from `Frontend.html`** | Only raisers are `LoadSaveScreen` ([`:128`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L128), [`:149`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L149)); the fast path is explicitly gated on `!this.props.inGame` ([`:126`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L126)). Live-confirmed destructive from an in-game page. |
| `NewCareer` | **SAFE only from `Frontend.html`** | Three raisers, all front-end `onFadeToBlackEnd` handlers: [`CareerSettings.js:162`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/CareerSettings.js#L162), [`CreateATeamPrincipal.js:60`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/CreateATeamPrincipal.js#L60), [`TeamSelectMenu.js:245`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/TeamSelectMenu.js#L245); starts a new world. Same category as `ContinueGame`. **inferred** (no live test recorded). |
| `StartScenario` | **SAFE only from `Frontend.html`** | Raised only by scenario selection ([`ScenarioChallengeSelection.js:103`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/ScenarioMode/ScenarioChallengeSelection.js#L103), [`ScenarioTeamSelection.js:90`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/ScenarioMode/ScenarioTeamSelection.js#L90)); loads a scenario world. **inferred** same teardown as a load. |
| `AdvanceToNextEvent` | **SAFE only from `TeamManagement.html`** | The only raising components are `ContinueButtonContainer` (mounted by `TeamManagementModule`, `Inbox`) and `TimeProgressionPanel` — both TeamManagement bundles. From `RaceWeekend.html` those modules are not loaded, so the event has no local context and its meaning is **unknown**; treat as unsafe there. From `Frontend.html` it has no meaning at all (no handler, no button). |
| `NextMonthSelected` | **SAFE from any screen** | `CalendarContents.onMonthUp` ([`:278`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/calendar/CalendarContents.js#L278)). Browsing only; no documented teardown; the handler is purely a view-range change. **inferred** safe — not live-tested. |
| `PreviousMonthSelected` | **SAFE from any screen** | Same, `:288`. |
| `CalendarDaySelected` | **SAFE from any screen** | Two sites in `CalendarContents` (`:74` on unmount, `:334`); a report, not a command. |
| `SessionOverviewClosed` | **SAFE only from `RaceWeekend.html`** | Raised by `SessionOverview` ([`:149`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/SessionOverview/SessionOverview.js#L149)); the component exists only in the RaceWeekend bundle. Engine enters a session on receipt (**inferred**), so firing it from elsewhere is meaningless and possibly harmful. |
| `PreSessionClosed` | **SAFE only from `RaceWeekend.html`** | Raised by `_PreSessionView` ([`:141`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L141), [`:250`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PresessionView.js#L250)). **This is the closest thing to a session-start event**; see §3.5 for why it still does not start the sim. |
| `AdvanceToNextStage` | **SAFE only from `RaceWeekend.html`** | Seven raisers, all under `project/modules/raceWeekend/`. Steps the weekend *within* a loaded race weekend. |
| `PlayOrSimClosed` | **SAFE only from `RaceWeekend.html`** | `SessionOptions` ([`:123`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SessionOptions.js#L123)). |
| `SimulateSingleSession` | **SAFE only from `RaceWeekend.html`** | (`:132`) Mutates the weekend's simulation state. |
| `SimulateRemainingEvent` | **SAFE only from `RaceWeekend.html`** | (`:142`) |
| `SimulateToRaceOrSprint` | **SAFE only from `RaceWeekend.html`** | (`:152`) |
| `ShowSimulationResults` | **SAFE only from `RaceWeekend.html`** | `SimulatingSessions` ([`:68`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/simulatingSessions/SimulatingSessions.js#L68)). |
| `OnStageBackwards` | **SAFE only from `RaceWeekend.html`** | Three raisers, all in the race-weekend bundle; rewinds a stage. |
| `SkipCinematics` / `SkipCarReleaseCinematics` | **SAFE only from `RaceWeekend.html`** | One raiser is `FrontendCinematicsView.js:58` (`Frontend.html`); the other six are RaceWeekend. So the name itself is safe on both, but the **effect** differs by document. |
| `PostRaceSummaryClosed` | **SAFE only from `RaceWeekend.html`** | ([`:146`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryView.js#L146), `:150`) |
| `PostRacePitStopSummaryClosed` | **SAFE only from `RaceWeekend.html`** | ([`:95`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PostRaceSummaryViewPitStops.js#L95)) |
| `SubmitActiveSessionSetup` / `CancelActiveSessionSetup` | **SAFE only from `RaceWeekend.html`** | `PitOptionsView` ([`:208`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PitOptionsView.js#L208), [`:242`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/PitOptionsView.js#L242)). Arguments reference `carID`; firing blind risks corrupting the wrong car's setup. |
| `SetPlaybackRate` / `SimPauseChange` | **SAFE only from `RaceWeekend.html`** | `RaceWeekendEvents.js:66,70`. Meaningless without a live sim. |
| `QuitGame` | **UNKNOWN** | Two raisers: `MainMenu` ([`MainMenu.js:222`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/frontend/components/MainMenu.js#L222)) and `RaceWeekendEvents.js:78`; also wired to the Options Exit dialog ([`OptionsMenu.js:56`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/options/OptionsMenu.js#L56) → `onQuitGame` at `:74-80`, which calls `QuitGame()` at `:79`). It is documented as the way back to the front-end but **has not been fired against a live game** ([`LOBBY-GATE.md`](E:/Downloads/coop-anything-main/local/f1-manager-2024/LOBBY-GATE.md), "What is still unproven"). |
| `RestartScenario` / `GameOverRetryLast5` / `GameOverRetrySeason` / `EnterTeamChangeFlow` | **UNKNOWN** | Load/rewrite the world. `GameOverScreen` ([`:111`, `:115`, `:128`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/GameOverScreen.js#L111)) lives in a RaceWeekend-reachable bundle, so these are demonstrably raisable in-game — but nothing records what the engine does. **Likely destructive; not tested.** |
| `RestoreData` / `DeleteGame` / `SaveGame` | **UNKNOWN** | Not literal strings in the export; returned from `getConfirmSelectionEvent` ([`LoadSaveScreen.js:163-176`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/loadSave/LoadSaveScreen.js#L163)). `DeleteGame`/`SaveGame` are certainly mutating. |
| all `OptionsChange*`, `Options*` | **SAFE from any screen** | Options is an overlay mounted in every app ([`OptionsMenu.js`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/options/OptionsMenu.js)). Settings changes, no teardown. **inferred.** |
| `EndTutorial`, `SkipTutorial`, `TutorialStepComplete`, `HelpScreenRequestedByPlayer` | **SAFE from any screen** | Tutorial module is common to all apps (`project/modules/tutorial/`). |
| `show_Toast`, `hide_Toast`, `hold`, `select`, `show`, `toggleOn`, `valueUp`, `valueDown`, `begin_simulating` | **not engine events** | These are `Audio.sendEvent` (`Audio.js:34,81`), which routes to `triggerListener('SendAudioEventMessage', ...)` (`Audio.js:56`). Cosmetic. |
| everything else (editor, CAS, staff, scouting, HQ, finance, sponsors) | **SAFE from its own screen** | Local career edits; no document teardown. Deliberately not enumerated individually here — see §4.1. |

### 4.1 The rule the table is an instance of

Two independent properties decide safety, and the export states both:

1. **Is the raising component loaded in the current document?** The three `.html` entry points load
   disjoint bundles ([`Frontend.html`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/Frontend.html) →
   `project/apps/FrontendApp.js`, [`RaceWeekend.html`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/RaceWeekend.html) →
   `RaceWeekendApp.js`, [`TeamManagement.html`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/TeamManagement.html) →
   `TeamManagementApp.js`). Selecting and loading a save is a *front-end* verb; advancing the
   calendar is a *TeamManagement* verb; stepping a weekend is a *RaceWeekend* verb.
2. **Does the engine handler replace the document?** Proven for the load family by live test
   ([`LOBBY-GATE.md`](E:/Downloads/coop-anything-main/local/f1-manager-2024/LOBBY-GATE.md)); for everything else this is **unknown** because the handler is native.

---

## 5. Gaps — what the export categorically cannot tell us

1. **No engine-side handler *bodies*.** The binary was read as a string source only (§3.5.1), so we
   now know *which names the game has compiled in* — 324 of the 352, plus 6 UI-absent candidates
   including `StageProceed` and `TravelToRaceWeekend`. What each handler *does* remains
   **inferred**/**unknown**. Nothing here proves that a given `sendEvent` causes a given effect.
2. **No native UFUNCTION signatures or addresses.** `engine.call('GetGameOwner')`
   ([`Player.js:17`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/Player.js#L17)) and `callBoundFunction('GetDataStore')`
   ([`DataStore.js:231`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/DataStore.js#L231)) prove a name-based call channel exists. `Mappings.usmap` (41,679 name-pool
   entries) now proves the UE reflection surface exists and exposes `GUIManager`,
   `GUIEventsManager`, `GUIEvents`, `MainMenuGUIEventsManager`, `ManagementGUIEventsManager`,
   `RaceSimGUIEventsManager`, `GUIMediator*` (20+ mediators) and `CohtmlJSEvent` — but it is a name
   pool, not a class map, so **which UFunction lives on which object is not established here**, and
   no RVA is claimed. **Not guessed.**
3. **No argument schema.** The UI's `sendEvent('X', a, b)` shows only what the *UI* passes. Whether
   the engine tolerates zero args, extra args, or a string vs number is **unknown** for every event.
   `LoadGame` is the one exception where the pairing is proven to work in practice.
4. **No `PreSessionClosed`/`StageProceed` semantics.** Neither the export nor the string table can
   distinguish "engine starts the sim" from "engine validates the setup then starts the sim" from
   "engine expects the player to have entered the session some other way". §3.5 and §3.5.1 explain
   why the symptom fits all three.
5. **`RaceSim.sessionState` writers are invisible.** The enum exists and the UI *reads* it —
   `Inactive`, `Active`, `WaitingForLightsOut`, `CompleteCheckeredFlagWaved`,
   `CompleteAllCarsSeenCheckeredFlagOrStopped`, `CompleteAllCarsStopped`
   ([`GameTypes.js`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/GameTypes.js), search `var ESessionState`) — and `sessionState` is declared on
   the schema ([`RaceSim.js:1070`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/RaceSim.js#L1070)), but **no UI code writes it**. Same for
   `RaceSim.raceTime` ([`RaceSim.js:1062`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/RaceSim.js#L1062)), `playbackRate` (`:1071`) and `isSimPaused`
   (`:1072`): read many times, written only inside dead `isCoherentPlayer()` blocks. All four are
   engine-owned.
6. **Three event names are unreachable by literal scan** and so missing from §2.3:
   `RestoreData`, `DeleteGame`, `SaveGame` (built in `getConfirmSelectionEvent`). Likewise the
   `OptionsChange*` tail, `Commands*TabSelected`, and the `EFanfareType` families (§2.2). Of these,
   `DeleteGame` and the two `Commands*TabSelected` names are now **binary**-confirmed at
   `0x5f01838`, `0x5fac000` and `0x5fac020`.
7. **Version pinning.** This document is pinned to build `17356935` / file version `1.11.0.39891`,
   exe SHA-256 `7ffe404f…b2050f6` (see the identity table at the top). If the game patches, line
   numbers, the 352 count and every binary offset can drift. Re-verify identity before following it.
   **Note:** the exe parsed as `PE32+` with 19 sections and an unusual `.ecode`/`.xtext` layout, so
   the large `.xtext` section's reported `vsize` (`0x18ca0039`, ≈ 416 MB) exceeds its raw size
   (`0x18ca0200`) — treat that section's virtual/raw mapping as approximate if you extend §3.5.1.
8. **`continueSlotName` is engine-written; its writer is native.** Whether patching it before
   `ContinueGame` reliably redirects the load is **inferred and untested**.
9. **`DS.setValue` from a script is not a usable lever.** Because `setValue` asserts on
   `isCoherentPlayer()` (§1.3b, [`DataStore.js:167`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/common/core/DataStore.js#L167)), a bridge cannot force
   `RaceSim.sessionState` or `raceTime` through the JS DataStore API in the shipped build. Writing
   those requires the native DataStore handle, not the UI.
10. **Three live tests are missing and matter**, in this order of value:
    1. `StageProceed` fired from a genuine pre-session screen (see §3.5.1 — the strongest lead).
    2. `PreSessionClosed` fired the same way, with a real setup, as the control.
    3. `QuitGame` as a scripted return-to-front-end.
    All are single-client, no-network tests, so they cannot be blocked on the relay.
11. **`RaceWeekend.html` may not be a live document at all.** `RaceWeekendModule`'s initial view is
    `Engine.isCoherentPlayer() ? PostQualifyingSummary : SessionOverview`
    ([`RaceWeekendModule.js:12`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L12)), and it reports `RaceWeekendModuleReady` once
    on mount ([`:29`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/modules/raceWeekend/RaceWeekendModule.js#L29)). Whether the engine keeps this document alive during a race or
    re-creates it per session is **unknown** — and that decides whether a bridge injected into it
    survives across sessions.

---

## Appendix A — reproduction

```powershell
$p = 'E:\FModel\Output\Exports\F1Manager24\Content\UIGameface\js'

# All engine events (517 sites / 352 names)
Get-ChildItem $p -Recurse -File -Filter *.js |
  Select-String -Pattern 'Engine\.sendEvent\)?\(\s*["'']([A-Za-z0-9_]+)["'']' -AllMatches |
  ForEach-Object { foreach ($m in $_.Matches) { $m.Groups[1].Value } } | Sort-Object -Unique

# The brief's pattern, for comparison (103 names, misses 251)
Get-ChildItem $p -Recurse -File -Filter *.js |
  Select-String -Pattern "sendEvent\('([A-Za-z0-9_]+)'" -AllMatches |
  ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique

# Audio helper, to be excluded
Get-ChildItem $p -Recurse -File -Filter *.js |
  Select-String -Pattern 'Audio\.sendEvent\)?\(' -AllMatches | Measure-Object

# The only calls into the engine
Get-ChildItem $p -Recurse -File -Filter *.js | Select-String -Pattern 'engine\.trigger\(|engine\.call\('
```

## Appendix B — the questions the bridge actually needs answered

| question | answer | confidence |
|---|---|---|
| Which slot does `ContinueGame` load? | The engine's `Save.Saves.continueSlotName`; the UI supplies only the default `'autosave'` ([`Saves.js:59`](E:/FModel/Output/Exports/F1Manager24/Content/UIGameface/js/project/data/Saves.js#L59)) and never sends it as an argument. Use `LoadGame <slot>` to choose. | proven for the UI side; engine side inferred |
| Which **UI** event starts a race session? | **None.** No Gameface event raised by the shipped UI starts a sim. The pre-session confirm sends `PreSessionClosed` and waits; the sim starts inside the engine. | proven (negative) |
| Is there an engine command the UI never sends? | **Yes, and it is the best lead.** The exe contains `StageProceed` (`0x5fa7c98`) inside the session-flow binding block, plus `TravelToRaceWeekend` (`0x5fcac28`), `ReturnToMenu` (`0x5fc9cb0`) and `ToggleAutoNextDay` (`0x5fc9c80`) — none of which appear anywhere in the export. | binary-proven names; effect unknown |
| What can a script legitimately drive? | `LoadGame`, `ContinueGame`, `AdvanceToNextEvent`, `NextMonthSelected`/`PreviousMonthSelected`, and the race-weekend screen-flow events (`SessionOverviewClosed`, `PreSessionClosed`, `AdvanceToNextStage`, `SkipCinematics`, …). | proven |
| What must a script *not* do? | Fire `LoadGame`/`ContinueGame`/`NewCareer`/`StartScenario` outside `Frontend.html`; fire `*ToNextEvent` from `RaceWeekend.html`; assume `AdvanceToNextEvent` is idempotent (it is start/stop). | proven / live-confirmed |

## Appendix C — suggested next steps, cheapest first

1. **Try `engine.trigger('StageProceed')`** from a genuine pre-session screen, with
   `engine.trigger('PreSessionClosed')` as the control in the same run. Watch `RaceSim.raceTime`
   and `RaceSim.sessionState`. This is one bridge `raw` command and no new code (§3.5.1).
2. **Try `TravelToRaceWeekend`** from TeamManagement when the calendar's next event is a race
   weekend, and `ReturnToMenu` from TeamManagement. Both are single-event tests.
3. **Disassemble the handler bodies** rather than repeating string searches: find the code that
   references the `StageProceed` string at `0x5fa7c98` and read what it validates before advancing.
   The binding block layout (§3.5.1) gives a tight neighbourhood to work in.
4. **Do not** pursue `DS.setValue` on `RaceSim.sessionState`/`raceTime` from JS — it asserts out in
   the shipped build (§5 item 9).
