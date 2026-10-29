# Headless UI client for the F1 Manager 2024 co-op test

Two separate things live here:

1. **A headless protocol client** — speaks the same command protocol as the in-game pak bridge, so a
   co-op test can have a *client that needs no second game process*.
2. **A headless UI render harness** — boots the game's real Gameface app and renders its real views
   in Node, with no game process at all.

```
tools/headless/coop_client.js    the protocol client
tools/headless/run_tests.js      41 offline tests (unit + live against the real server)
tools/headless/dom.js            hand-written DOM host (no npm dependencies)
tools/headless/export_loader.js  boots the game's own RequireJS app tree in a vm
tools/headless/css_index.js      indexes the 1,002 shipped stylesheets
tools/headless/render_one.js     renders exactly one screen, in its own process
tools/headless/run_ui_sweep.js   renders all 165 screens, N in parallel
tools/headless/probe_render.js   15 assertions: does preact + the engine handshake work?
tools/headless/drive_test.js     13 assertions: do clicks actually reach engine.trigger?
tools/headless/ui_driver.js      applies a command by pressing the real control
tools/headless/press_test.js     25 assertions against the SHIPPING pak press module
tools/headless/observe_test.js   67 assertions against the SHIPPING pak observer module
tools/headless/normal_play_test.js 21 assertions: is the mod inert when unused?
tools/headless/agent_coord_e2e.js 16 assertions: two real agents through a real supervisor process
tools/headless/README.md         this file

.css-index.json is a generated cache (~350 KB, rebuilt with `node css_index.js --force`). Without it
the stylesheet scan costs about a minute, which per-screen child processes cannot afford.
```

## What it is, and what it is not

The in-game bridge exists because the game's UI is a Coherent/Gameface web view, and every user
action funnels through `engine.trigger(name)` ([UI-COMMANDS.md](UI-COMMANDS.md) §1). A pak
override injects `js/coop/bridge.js` into `Frontend.html`; that bridge polls a loopback command
server. **The server never talks to Cohtml** — it hands out a command string on `GET /hello` and reads
an ack string. Those are plain HTTP, so a client can be a Node process.

| | in-game bridge | this harness |
|---|---|---|
| speaks `/hello`, `/report` | yes | yes |
| can fire `engine.trigger` | yes | **no — no engine** |
| needs a game process | yes | no |
| can render | yes | no |
| good for | real end-to-end | lobby/protocol/server logic, fault injection, CI |

**It cannot pretend the game did something.** There is deliberately no "fabricate success" mode:
`--mode=none` reports `would trigger X (no engine attached)`. A test harness that invents its own
pass result is not a test. If you need to prove the game reacted, use a real client.

## Quick start

```powershell
# start the command server (unchanged)
python tools\coop_server.py

# in another shell: register a headless client at the main menu
node tools\headless\coop_client.js --mode=none --page=Frontend

# queue commands exactly as you would for a real client
curl "http://127.0.0.1:45999/cmd?text=Continue"
curl "http://127.0.0.1:45999/cmd?text=Load%20save2"
curl "http://127.0.0.1:45999/status"
```

## Options

```
--server=HOST     coop_server host (default 127.0.0.1)
--port=N          coop_server port (default 45999)
--mode=MODE       none | replay | ui     (default none)
--replay=FILE     JSONL recording (replay mode)
--page=PAGE       Frontend | RaceWeekend | TeamManagement   (default Frontend)
--stage=STAGE     string reported to the server             (default menu)
--poll=MS         poll interval                             (default 250)
--local-address=IP  bind the socket to this local address    (default: OS chooses)
--quiet           less output
--help
```

`--page` is not cosmetic: it is what the server's delivery filter keys on.

### `--local-address` is required when a real bridge is also local

`coop_server.py` identifies a client by **source IP** (`client_address[0]`), not by any id the
client sends. Two clients on one machine therefore collide into a single registry entry, and each
overwrites the other's `page`/`pkg` on every poll — so the harness would silently steal the game's
commands, or be mistaken for it. Observed live: with both on `127.0.0.1`, `report-bridge` returned
`127.0.0.1: pkg=coop-bridge` on one poll and `127.0.0.1: pkg=headless` on the next.

Fix: bind the harness to a second loopback address. All of `127.0.0.0/8` is loopback on Windows.

```powershell
node tools\headless\coop_client.js --mode=none --page=Frontend --local-address=127.0.0.2
```

This is why `run_two_client_test.ps1` defaults to `-LocalAddress 127.0.0.2`. With it, `/status`
shows two independent entries:

```
127.0.0.1  pkg=coop-bridge  page=RaceWeekend
127.0.0.2  pkg=headless     page=Frontend
```

## The render harness

`coop_client.js` can drive the command protocol but cannot render anything. Separately,
`run_ui_sweep.js` renders the game's real UI, in Node, with no game process:

```powershell
node tools\headless\probe_render.js                  # 15 assertions, ~15 s
node tools\headless\run_ui_sweep.js --jobs=6         # all 165 screens, ~6-8 min
node tools\headless\run_ui_sweep.js --app=frontend   # one app
node tools\headless\run_ui_sweep.js --json=ui-sweep.json
node tools\headless\run_ui_sweep.js --list           # print the screen list only
node tools\headless\render_one.js --app=frontend --screen=MainMenu --json
```

### Why this works at all: the game ships its own headless path

The apps branch on `Engine.isCoherentPlayer()`, and every branch is a complete alternative
implementation of the engine-facing systems:

| system | in-engine | headless (`isCoherentPlayer()` true) |
|---|---|---|
| DataStore | `CppImpl` → `engine.call('GetDataStore')` | `JSImpl`, a pure-JS store (`DataStore.js:207`) |
| System | `CppImpl` → `GetSystemInformation` | `SystemJSImpl`, returns constants (`System.js:148`) |
| Localisation | `CppImpl` → `GetLocalisation` | `LocalisationProviderJSImpl` (`Localisation.js:9`) |
| data | the real save | `generateFromSchemaFragment(...)` over `project/data/*` |

So the harness does not fake a UI mode. It puts the engine in the state Frontier already built and
uses for browser preview, and that state populates the DataStore with generated dummy data. The
render then runs the real components with the real `preact` build.

### Two engine facts the loader has to reproduce

Both are read out of the export, not assumed:

1. **`engine.SendMessage` never settles without a native handler.** `cohtml.js:132-143` only resolves
   a pending `engine.call` when `engine._mocks[name]` exists. Native Coherent instead calls
   `engine._Reject(id)` → `"No handler registered"` (`cohtml.js:235-241`). Without reproducing that,
   `IsRunningInEngine` hangs forever, `_isReady` stays `false`, and `Engine.isCoherentPlayer()`
   *throws* (`Engine.js:34-40`) on every preact flush (`preact.js:152`).
2. **`DOMContentLoaded` must be driven.** `cohtml.js:276-283` picks the browser branch inside a `vm`
   (no CommonJS `module`), so it waits for an event the host never fires. `_OnContentLoaded()` is what
   triggers `'Ready'` and resolves `engine.whenReady`.

### Stylesheets are loaded and checked, not assumed

The export ships **1,002 .css files (3.8 MB)** at `Content/UIGameface/css`, which is exactly what
`CSSUtil.loadCSS` requests (`common/util/CSSUtil.js:6-32`). `css_index.js` indexes them and the
harness uses that three ways:

1. **Custom properties are real.** ~20 call sites read CSS variables through
   `getComputedStyle(document.body)` — every graph (`DonutChart.js:34`, `PitGraph.js:146`,
   `TrackGraph.js:22`, `FuelGraph.js:18`, `CarGraph.js:39`, `QualifyingGraph.js:31`), plus
   `LayoutUtil.js:16` and `Repeater.js:167`. 680 `:root`-level custom properties are indexed and served,
   so e.g. `--team1` returns `#E8002D` instead of `''`.
2. **Class coverage is measured.** For each rendered screen the report records how many of the classes
   it emitted actually have a rule in the shipped CSS (`css.emitted` / `css.styled`), with the unstyled
   ones listed. This is what makes "rendered" more than "a component ran".
3. **Missing sheets are reported, not faked.** Some requested stylesheets are simply not in the export
   (`css/project/TeamManagement.css`, `css/project/RaceWeekend.css`, and many per-component files). A
   `<link>` whose file is absent is recorded in `missingStylesheets` rather than being told it loaded —
   firing `onload` for a 404 is how a harness talks itself into believing the layout arrived.

What is still absent is layout: no box model, no cascade resolution, no paint, and
`getBoundingClientRect()` returns zeros. So styles are known to *exist* for a class; they are not known
to produce a particular geometry.

What is still absent is layout: no box model, no cascade resolution, no paint, and
`getBoundingClientRect()` returns zeros. So styles are known to *exist* for a class; they are not known
to produce a particular geometry.

### Driving the UI, not just rendering it

`drive_test.js` proves the harness can *operate* the interface — 13 assertions:

```powershell
node tools\headless\drive_test.js        # ~20 s
```

A press runs the real path: DOM mouse events → preact `eventProxy` (`preact.js:219-226`) → `ButtonBehaviourT.onMouseDown/onClick` (`ButtonBehaviourT.js:117-145`) → `performSelect` → `props.onSelect` → `Engine.sendEvent` → `engine.trigger`. Measured results:

| action | result |
|---|---|
| press "Load Career" | emits `FrontendScreenChanged("Load")` |
| press "Credits" | emits `FrontendScreenChanged("Credits")`, screen mounts |
| press "New Career" | emits `NewCareer` path plus a `DSCallID*` payload listing `autoSave`, `save1`…`save29` |
| click a TeamManagement nav tab | emits `ManagementScreenChanged`, or `ManagementScreenChanged` + `CalendarDaySelected` for Calendar |

Two traps were found by building this, and both had made the harness *silently* inert:

1. **A `click` alone sends nothing.** `onClick` returns early unless `state.down` is already true
   (`ButtonBehaviourT.js:119-120`), and only `onMouseDown` sets it (:131). A synthetic press must be
   `mousedown` **then** `click`. Every button on the main menu did nothing until this was found.
2. **`node._listeners` belongs to preact.** preact installs one `eventProxy` per node+type via
   `addEventListener` (:430) and swaps the handler through `node._listeners[type]` (:431). The DOM host
   originally used that same name for its own map, so preact's write clobbered it and every `onClick`
   in the UI died. The host now uses `_domListeners`, and dispatch calls standard listeners only —
   calling both fired handlers twice (measured: one click incremented a counter by 2).

### How "did this screen render?" is measured

Not by CSS class. The exported `ScreenID` name and the component's root class diverge — provably:
`StartupBrightness` renders `StartupFlow_root`, `CreateATeamLogo` renders `EditTeamLogo_root`,
`ScenarioMode` renders `ScenarioScreen_root`. Instead `render_one.js` wraps the screen component's own
`render()` and counts calls. The module resolves the component out of its registry at render time
(`FrontendModule.js:46`, `TeamManagementModule.js:69`, `RaceWeekendModule.js:42`), so the real render
path is what gets counted.

Success = `renders > 0` **and** the DOM grew. A view that renders and *then* throws (an animation
callback touching a DataStore context that does not exist) is a render, and is reported with its
`error` populated rather than counted as a failure.

### Why one process per screen

The first version navigated all 86 TeamManagement screens in one process, which is closer to what a
player does but measurably wrong here:

- **A throw poisons the render queue.** `preact.js:139-150` keeps a module-local `items` array and the
  non-preview branch *rethrows* (`preact.js:159-174`). The throwing component is popped, but
  everything queued behind it is stranded. Observed: the first view to throw made the next 80 report
  "render() never ran".
- **State accumulates.** 86 mounted screens' listeners, rAF loops and DataStore contexts ran the
  process out of memory (`Ineffective mark-compacts near heap limit`) after ~30 screens.

A fresh process per screen makes each result independent and each crash attributable. The cost is
wall-clock, which `--jobs` recovers. Children run with `--max-old-space-size=512` so a view whose data
walk never terminates fails in ~15 s instead of stalling every worker.

### Results (measured 2026-10-28, build 17356935)

Full sweep, `--jobs=6`:

```
frontend         29/31 rendered
teamManagement   84/86 rendered
raceWeekend      48/48 rendered
TOTAL 161/165 screens rendered in 776s

style coverage   27,836 / 29,684 emitted classes have a rule in the shipped CSS (93.8%)
localisation     0 rendered screens with an unresolved [SYMBOL]
stylesheets      0 requested sheets missing from the export
```

Rendered screens are substantial, not stubs: nodes 13–892 (median 246), text 0–1,500 characters
(median 306). Per-screen class coverage has a median of 93.3% and a minimum of 81.8%. Text is real
English — the main menu reads `Continue`, `Load Career`, `New Career`, `Create A Team`, `Quit`,
`Settings`; the calendar reads `Calendar`, `Feb 2022`, `Today`, `4 February`.

Three screens recovered on a retry (attempt 2), which is why the sweep retries only "component never
ran" and timeouts — see `isRetryable`.

**The four that do not render, with the reason:**

| screen | reason |
|---|---|
| `CreateATeamPeopleOverview` | `DS.getValues([...CUSTOM_TEAM_CONTEXT,'CustomTeamDriverAgeRanges','BandLevels','1'], ['minAge','maxAge'])` (`CreateATeamPersonBrowser.js:9-10`) — the context does not exist. `CustomTeamDriverAgeRanges` appears **nowhere else in the export**, not in any schema file, so no amount of generated data creates it. |
| `CreateATeamLivery` | indexes a `DLCID` property that the generated `DLC` context does not provide. |
| `CarsCurrentProjects` | exhausts a 512 MB heap. `CarsCurrentProjects.js:85` binds a `DataStoreCollection` to `['Parts','Projects']`; the walk over it does not terminate against generated data. |
| `StaffEngineeringCarsCurrentProjects` | same screen tree, same cause. |

**46 further screens render and *then* report an error**, all the same class of missing generated
value: `length` 17, `indexOf` 6, `maxEnergyStored` 5, `currentRank` 5, `DLCID` 3, `minAge` 2,
`manufacturerID` 2, then `firstName`/`laps`/`type`/`trackID`/`concat`/`payout` 1 each. These populate
the report's `error` field but do not fail the screen: the view mounted and something it does *after*
mounting could not find a value. Collapsing that into a failure would hide 46 working views behind a
data gap.

**What this measurement is not.** It says a component mounts, draws DOM whose classes are mostly
covered by the shipped CSS, and whose strings resolve. It does not say the screen is *correct*, and it
exercises no interaction: no clicks, no focus navigation, no DataStore writes (`DS.setValue` throws by
design, [UI-COMMANDS.md](UI-COMMANDS.md) §1). For driving the game, use `coop_client.js` and a
real client.

### Known limits

- **No layout, no CSS, no paint.** `getBoundingClientRect()` returns zeros, so focus navigation,
  spatial navigation and drag are not exercised. The stylesheets *are* loaded and parsed (see below),
  so class coverage and custom properties are real; positions and sizes are not.
- **Localisation works, via a shape conversion.** The pak loads `localisation/volta/<lang>/volta.archive`
  (UTF-16LE, `{Children:[{Key,Translation:{Text}}]}`); FModel exported the same data as
  `Content/Localization/Volta/<lang>/Volta.json` (UTF-8, flat `{namespace:{KEY:"text"}}`, 124
  namespaces, 88,079 entries). `export_loader.js` builds the archive shape the game's own
  `parseJSONLocFile` expects, so text now reads in English — `[CONTINUE]` → `Continue`,
  `[LOAD_CAREER]` → `Load Career`. Not every symbol exists: `CARS_NEW_PROJECT` is absent from the
  export, so it renders as its raw `[SYMBOL]`. The report separates the two cases per screen
  (`loc.notInExport` vs `loc.inExportButUntranslated`) rather than blaming the loader for both.
- **Dummy data, not a save.** A screen that renders is a screen whose component mounts against
  generated data. It says nothing about correctness with a real career.
- **Missing contexts are the main failure mode.** `Cannot read properties of undefined (reading 'x')`
  means the view indexed a DataStore context the export has no schema for (e.g.
  `CustomTeamDriverAgeRanges`, which appears in `CreateATeamPersonBrowser.js:9` and in no schema file).
  That is a reportable gap in the export, not a broken view.
- **Not connected to the command protocol.** This harness renders; `coop_client.js` speaks to the
  server. They share the DOM host and the export loader, nothing else.

## The safety rule it enforces

`LoadGame` and `ContinueGame` may only run from `Frontend.html`. Firing either from inside a career
tears the front-end down and leaves a **black screen with the process alive** — reproduced on a live
client and documented in [LOBBY-GATE.md](LOBBY-GATE.md).

The refusal is implemented **twice, independently**:

1. **Server side** (`coop_server.py::take_for`) — the job is added to `skipped` and never delivered.
   The client cannot run what it never receives.
2. **Client side** (`coop_client.js::allowedFor`) — mirrors `bridge.js::allowedFor`, so a client that
   is handed the command anyway (stale server, or a direct `handle()` call) still refuses.

Both are covered by tests. The client-side one caught a real bug during development: splitting an
untrimmed command string makes `parts[0] === ''` for `"  Load save2"`, which silently defeats the
guard. The fix is to trim before splitting, and there is a test pinning it.

## Modes

| mode | reports | use for |
|---|---|---|
| `none` | `would trigger <Event> (no engine attached)` | protocol and lobby tests, CI, second-client stand-in |
| `replay` | the outcome recorded in a JSONL file | re-running a known sequence deterministically |
| `ui` | what a real press produced, tagged `via=ui` or `via=synthetic` | actually applying a command to the rendered UI |

### `--mode=ui`: commands really drive the interface

`none` and `replay` are honest but inert — neither changes any state. `ui` boots the real app for the
command's page, finds the control a player would press, and presses it. Measured against the live
server:

```
recv: Continue
ui ContinueGame -> ui - pressed FrontendContinueButton
server: OK Continue :: pressed FrontendContinueButton via=ui

recv: Load save1
ui LoadGame -> ui - LoadGame("autoSave") from the save list
server: OK Load save1 :: LoadGame("autoSave") from the save list via=ui
```

The ack carries `via`, so a caller can tell a real press (`via=ui`) from a fallback send
(`via=synthetic`) without reading logs. `synthetic` means: no control matched, so `engine.sendEvent`
was called directly — the event fired, but **the UI never agreed the action was available**. Reporting
that as a normal success would be the same dishonesty as `none` pretending to work.

Round trip is ~17 s for `Continue` and ~20 s for `Load`, dominated by the app boot (11 s) rather than
the press (0.7 s). Well inside the server's 60 s TTL.

`NextMonthSelected` and `AdvanceToNextEvent` are **not** drivable yet and report `synthetic`:

- `AdvanceToNextEvent` — the career advance button needs `Calendar.UpcomingEvents.<day>.Events.<i>`,
  which generated data leaves empty, so `ContinueButtonContainer` throws on
  `this.state.eventName.toUpperCase()` (ContinueButtonContainer.js:280) before it can be pressed.
  `ui_driver.js` seeds `eventName`/`advancing` to get past the first throw, which then exposes the next
  gap (`event.filterCategory`, CalendarUtils.js:777). Closing that needs the real career data, not more
  seeding.
- `NextMonthSelected` — the calendar month controls have no stable text or class to target.


## The end-to-end runner

```powershell
# one real game (already running with the coop pak) + one headless client
powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\headless\run_two_client_test.ps1 `
  -Steps "report-bridge,headless,cmd:Advance,wait:4,career-headless,expect-refused:Load save2,stop-headless"
```

Steps: `status`, `wait:<s>`, `report-bridge`, `expect-frontend`, `cmd:<text>`,
`expect-refused:<text>`, `headless`, `career-headless`, `frontend-headless`, `stop-headless`.

`career-headless` restarts the harness as if it were inside a career so the server-side refusal can
be exercised **without touching the player's own session**. It never calls a game-changing command
on the real client.

A passing run looks like this (measured 2026-10-28 against a live build):

```
INFO    report-bridge   BRIDGE 127.0.0.1: page=RaceWeekend stage=ingame sent=6
PASS    headless        headless client registered
INFO    report-bridge   BRIDGE 127.0.0.1: page=RaceWeekend stage=ingame sent=6
INFO    report-bridge   127.0.0.2: pkg=headless page=Frontend
PASS    cmd:Advance     delivered=2 skipped=0 | bridge sent 6->7 | ack=...
PASS    career-headless       headless client now reports page=RaceWeekend
PASS    expect-refused:Load save2   load/continue need the front-end
PASS    frontend-headless     headless client back at Frontend
```

`delivered=2` and `bridge sent 6->7` are the two facts worth reading: the command reached **both**
clients, and the real in-game bridge counted it. `pkg=coop-bridge` is how the live bridge
identifies itself; `pkg=headless` is the harness.

## Tests

```powershell
node tools\headless\run_tests.js                # 41 tests, includes a live server
node tools\headless\run_tests.js --skip-live    # 26 unit tests, no python needed
node tools\headless\probe_render.js             # 15 assertions, engine handshake + preact
node tools\headless\drive_test.js               # 13 assertions, clicks actually drive the UI
node tools\headless\run_ui_sweep.js             # 161/165 screens rendered
```

Current state, all run offline against the export (no game process, no network):

| suite | result |
|---|---|
| `run_tests.js --skip-live` | 26 / 26 |
| `probe_render.js` | 15 / 15 |
| `drive_test.js` | 13 / 13 |
| `press_test.js` | 25 / 25 |
| `observe_test.js` | 67 / 67 |
| `agent_coord_e2e.js` | 16 / 16 |
| `normal_play_test.js` | 21 / 21 |
| `run_ui_sweep.js` | 161 / 165 screens |

`run_tests.js` covers the protocol client; `press_test.js` loads the **shipping** pak module (see
`../CLIENT-UI-PRESS.md`); `observe_test.js` loads the **shipping** observer module and checks what a real
UI event becomes on the wire (see `../COORDINATION-SENDER.md`); `agent_coord_e2e.js` spawns the real
`coop-supervisor` and speaks its protocol as two agents, which is the closest thing to a game launch the
coordination layer has had; `normal_play_test.js` proves the mod is inert when no co-op session is running
(see `../NORMAL-PLAY.md`). The live layer of `run_tests.js` needs `python tools\coop_server.py`.

Verified end-to-end against the live server (2026-10-28), `--mode=ui`:

```
cmd "Continue"    -> OK Continue :: pressed FrontendContinueButton via=ui
cmd "Load save1"  -> OK Load save1 :: LoadGame("autoSave") from the save list via=ui
```

The live layer **starts the real `tools/coop_server.py`** on a throwaway port (46231) rather than
using a mock, because a mock would only test my reading of the server. It then asserts the actual
contract:

```
ok  server started and answers /status
ok  headless client registered in /status
ok  server recorded page=Frontend
ok  Continue delivered and acked
ok  headless reported it WOULD trigger, not that it did
ok  server refused to DELIVER Load to a client in a career
ok  skip recorded with a reason
ok  skip recorded the client page
ok  skip reason names the front-end rule
ok  client never received the refused Load
ok  client-side guard also refuses Load in a career
ok  client refusal names the page
ok  Advance queued and delivered
ok  Advance was NOT refused inside a career
```

## What this does not solve

- **No RaceSim.** The harness has no simulation, so it cannot answer any of the open questions in
  [RACESIM-HIJACK.md](RACESIM-HIJACK.md) §7 — in particular whether the `ERaceSimEventType`
  lifecycle verbs (`SessionStart`, `RaceStart`, …) actually drive a session. That needs a real client
  firing events.
- **No render.** The render path is separate from sim state; a headless client has neither.
- **Not a substitute for the two-machine test.** It replaces one of two *UI* clients, so the pairing
  to aim for is: one real game (host) + one headless client, then one real + one real.

## Files this was built against

Read-only references used to get the contract right, not modified:

- `tools/coop_server.py` — `PORT = 45999`, `GET /hello` (query `page,stage,sent,pkg,v,ack`), `GET /report?page=&text=`, `GET /cmd?text=&ttl=`, `GET /status`, `GET /quit`
- `pak/f1manager24/Content/UIGameface/js/coop/bridge.js` — `allowedFor()`, `dispatch()` grammar
- `LOBBY-GATE.md` — the black-screen evidence behind the page rule
- `UI-COMMANDS.md` §1 — why `engine.trigger` is the choke point the bridge sits on
