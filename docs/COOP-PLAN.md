# Co-op plan: from four failed races to a design that can work

Written 2026-10-27 after the fourth live sprint failed to hold location sync. No game is running; this is
the plan that the next implementation session works from.

The purpose is not to list features. It is to record **what the failures actually proved**, because the
same class of mistake has now cost four sessions: adding replicated fields while never establishing the
premise those fields depend on.

---

## 1. What the failures proved

### 1.1 The sim does not advance in fixed steps

From the export, `Config/DefaultEngine.json`:

```
MaxPhysicsDeltaTime=0.033333
bSubstepping=False
bSubstepDeltaTime (MaxSubstepDeltaTime)=0.016667
MaxSubsteps=6
InitialAverageFrameRate=0.016667
```

`bSubstepping=False` means the simulation consumes a **variable wall-clock delta**, up to 33.3 ms. There
is no fixed tick, so two machines are never at the same step, and no amount of per-field writing makes
them agree. This is the premise that was missing.

Consequence: **position replication cannot converge.** Writing `dist` sets a value the sim then advances
from on its *own* schedule, at a rate set by frame time. The write and the next integrate step disagree,
and the disagreement is proportional to frame-time difference between the machines. In the last session
that showed up as `applied=20681, entries=1881971` - fourteen thousand writes in eight seconds - while
the distance delta grew to 62 km. The writes were landing; the sim was simply stepping past them.

### 1.2 `race_time` is not a clock

Measured live:

```
SIM_TIME   951.00 -> 953.00 -> 955.00      (advancing)
lap_times  928.8, 933.2, 925.8, 927.8      (frozen, all cars)
max          933.22                          (so race_time frozen too)
```

`race_time` is `max(lap_time)`. Once cars stop completing laps (session ending, cars retiring), that
value stops changing even though `SIM_TIME` advances. The transport used `race_time` as the snapshot's
timestamp, so the client received a **constant clock** for the last part of every race. NOTES.md already
said "race_time is `max(lap_time)` - NOT monotonic ... must not be used as a liveness clock"; it was then
used as one. `SIM_TIME` at `0x99AF1C` is the field that actually advances.

### 1.3 Positions are not the only per-car state that matters

Reported: "the entire grid pitted on host and on client they did not", and "cars leaving the pits on red
flag from a restart not being held". Measured on the host mid-race with `tools/watch_rc.py`:

```
  t     flags                        rc  rf  pits preqs stay moving  max_dist
   0.0  0x400 (DRS_ENABLED)          1   0   5    2    0    22      45853.5
```

Five cars in the pit lane on the host. `PIT 0x8A8`, `PIT_REQ 0x8A9`, `STAY_OUT 0x904` were **not**
replicated, so the client was told where cars were but not that they were in the pit lane. The pit
transition is also when the host repairs/replaces parts, which is the reported "damage not synced, broken
cars racing": the client never ran the transition.

### 1.4 The failures were plumbing, not physics

Every failed session had a different *wiring* fault, not a different *algorithm* fault:

| session | actual fault |
|---|---|
| 1 | two host publishers, so the relay fanned duplicate tick streams (ticks went backwards) |
| 2 | client attached to a game pid that no longer existed |
| 3 | client's live game had no agent injected at all |
| 4 | agent loaded was a pre-restart build without the new fields |

`tools/preflight.py` now checks 20 conditions against live process state and distinguishes *wiring*
(must pass before a race) from *flow* (can only pass once a race runs). That last split matters: the first
version demanded frame flow before a race existed, which invited weakening the check to get green.

---

## 2. The design consequence: authority, not replication

If the sim cannot be made step-identical, then **agreeing on state by writing state is the wrong model.**
The correct model is that one sim is authoritative and the other is *driven*, and the driver has to
account for the fact that the driven sim keeps integrating on its own.

Three candidate architectures, ordered by how much they depend on the thing that failed:

### Option A - continuous correction against a clocked reference (recommended)

Instead of "write the host's distance", send `(host_sim_time, host_state)` and have the client compute the
correction it needs. Concretely:

1. The snapshot carries `SIM_TIME` (`0x99AF1C`), which advances monotonically. This replaces `race_time`.
2. The client's agent, running inside the sim call, compares its own `SIM_TIME` to the host's.
3. It writes a correction proportional to the *time gap*, not to the raw distance gap. A client running
   30 ms behind the host gets a different correction than one running 300 ms behind.

Why this is the right first target: it uses only fields we have already proven writable, and it fixes the
specific measured failure (constant clock, growing distance delta). It does not require reflection, a
reverse channel, or lifecycle events.

### Option B - make the client's sim not advance while being driven

Freeze the client's simulation and only ever move cars by host writes. This is closer to "the client is a
display". Problem: the client's sim still needs to run its own pit/flag/damage logic or those diverge
(§1.3). So B does not remove the need for state replication - it makes it total, which is worse.

### Option C - replace the client's controller with a replay of the host's inputs

The most faithful and the most work: the host publishes the *inputs* (per-car command records at
`RSM + 0x770 + car*0x50`, mapped in NOTES ~line 1335) and the client's sim produces its own outcome. This
is the only option that could survive a red flag restart naturally, because each sim runs the transition
itself from the same inputs. It depends on the sim being deterministic given inputs - which
[RNG-DETERMINISM.md](RNG-DETERMINISM.md) argues it largely is, apart from mechanical faults.

**Recommendation:** A now (small, fixes the measured failure), then C for the parts A cannot cover
(pit/red-flag transitions, damage). B is not recommended.

---

## 3. What the export settles

`Content/Database/SavePS.json` is the game's own persistence layer as SQL: **40 tables**, each with a
`SELECT` and an `INSERT`. Two consequences that reorganise the work:

1. The game already knows how to serialise every piece of state that matters, so nothing has to be
   discovered by memory scanning - only the *live* offsets do.
2. The save is the natural co-op boundary: "same world" can be defined as "same save", which already
   transfers byte-identically between machines.

### 3.1 The race sim is fully enumerated

```
Save_RaceSimManager   ID, SimTime, RaceEventFlags, DRSTimeLastDetected1..3, SafetyCarState, ShownCarLeaving
Save_RaceSimCars      47 cols incl. RacePosition, Fuel, EngineTemperature, BrakeTemperature,
                      DRSState, TotalRaceTime, LapCount, LapType, CornerAccuracy, RaceLineAccuracyOffset
Save_RaceSimCars_Parts 6 explicit damage levels + 4 tyres x (surface, carcass, condition)
Save_RaceSimCars_Incident     RecoveryState, ImpactSpeed, HasLeftTrack, IncidentStartTime, ...
Save_RaceSimCar_MechancialIssues  FaultType, FaultName, FaultDevelopedTime, escalation checks
Save_RaceControl      SafetyCarReleasedSimTime, AdditionalSessionTime, RedFlagTimeToSkip, VscEndSimTime
Save_TimingManager_Laps  CarIndex, Lap, LapTimeMS, RaceTime, LapState
Save_WeatherHistory   Rain, WaterOnTrack, AmbientTemperature, TrackTemperature, RubberOnTrack
Save_Weekend          WeekendStage, CurrentGuiScreen, ActiveStepID, CurrentStageInnerStep, TimeRemaining
```

Three corrections to what the project believed:

| believed | actually |
|---|---|
| damage needs the event bus | `Save_RaceSimCars_Parts`: `BodyDamageLevel`, `FrontWingDamageLevel`, `RearWingDamageLevel`, `SidePodsDamageLevel`, `FloorDamageLevel`, `SuspensionDamageLevel` - six named levels |
| the sim has no clock to anchor on | `Save_RaceSimManager.SimTime` is persisted, so time is the game's own anchor |
| `race_pos` is derived, must not travel | `Save_RaceSimCars.RacePosition` is persisted - it is state |

### 3.2 The sim has no fixed timestep

`Config/DefaultEngine.json`: `bSubstepping=False`, `MaxPhysicsDeltaTime=0.033333`. Each machine integrates
a variable wall-clock delta, so two sims are never at the same step. This is why 1.88 M field writes over a
session still left the client 62 km off: the writes landed and the sim integrated past them.

---

## 4. Game modes: who owns what

Three modes. The export shows the game's own keying already supports all three:

| table | key | granularity |
|---|---|---|
| `Save_CarConfig` (toe, camber, anti-roll, wing angles) | `TeamID` + `LoadoutID` | team, then car |
| `Save_CarTyreAllocation` | `CarID` | car |
| `Save_DriverCommands` (pace, ERS, lift-and-coast, fuel) | `LoadoutID` | car |
| `Save_PlayerStrategyStints` (pit windows, stints) | `LoadoutID` | car |
| `Save_Strategist` | `TeamID` + `LoadoutID` | team, then car |
| `Buildings_HQ`, `Teams_PriorityList_Setting_DevelopBuildingData` | `TeamID` | team |
| `TeamDrivers` = `Staff_GameData JOIN Staff_Contracts WHERE StaffType = 0` | `TeamID` | team |

So **"my side of the garage" is a `LoadoutID`** and **"facilities come from Player A" is a `TeamID`**.

### Mode 1 - Shared team (one team, two drivers)

| system | owner |
|---|---|
| car setup, tyre allocation, driver commands, pit strategy | each player owns **their own `LoadoutID`** |
| car development / research (`PartsPS`) | **Player A** (team principal) |
| facilities / HQ (`BuildingsPS`, `Buildings_HQ`) | **Player A** |
| pit crew training | **Player A** |
| finances | Player A writes, B reads |
| contracts / scouting | Player A decides, B is informed |

The subtlety: `Save_CarConfig` is keyed by *both* `TeamID` and `LoadoutID`, so "only the host writes team
rows" would stop Player B setting up their own car. The rule must be
`team == my_team AND loadout == my_car` for setup, and `team == my_team` for facilities.

### Mode 2 - Versus (two teams)

Each player owns a whole `TeamID`; shared world (calendar, regulations, results, weather) is single-truth.
The race sim stays one authority for all 22 cars, which is already the current architecture. Versus is
therefore the *most* natural fit for the code that exists, not the least.

### Mode 3 - Co-op career (two teams, one world)

Mode 2 ownership plus shared progression: one calendar, one set of regulations, one set of results.

### The authorisation layer this needs

One function, checked before any write:

```
fn may_write(mode, my_team, my_loadout, table, key) -> bool
```

- `SharedTeam`: team-scoped tables require `key.team == my_team`; loadout-scoped require
  `key.loadout == my_loadout`.
- `Versus` / `CoopCareer`: any table whose row belongs to my team; shared-world tables only for the host.

This is a policy layer, testable with synthetic rows and no game. It is what makes all three modes work with
one codebase, and it should be written before any career feature because every career feature needs it.

---

## 5. Packaging and in-game entry

Two requirements: a client should be **one `.pak`**, and as few loose files as possible for both sides; and
the server should be launchable **from the main menu**, with per-player team selection.

Both are achievable, and the export shows how - with one hard constraint that shapes the design.

### 5.1 The hard constraint: Gameface cannot launch a process

The bridge runs in a browser-like context. Measured from the existing `bridge.js`:

```
fetch              1     present - HTTP works
XMLHttpRequest     6     present
process            3     only as a word in strings; no Node context
child_process      0     absent
require(           0     absent - not a module loader context
```

So the front-end **can make an HTTP request but cannot start `coop-supervisor.exe`**. Anything that spawns a
process has to be the injected DLL, which is already resident in the game.

The design that follows:

```
main menu button  ->  fetch POST /coop/host        (bridge, in the pak)
                  ->  loopback HTTP server          (coop_agent.dll, in-process)
                  ->  CreateProcess coop-supervisor (or run it on the agent's own thread)
```

That keeps the pak tiny, because the server is not in the pak - it is in the DLL that is already there. It
also means "launch server from the menu" needs no new loose files at all.

Alternative considered: package the supervisor *inside* the pak and have the agent extract-and-run it. Worse -
it means writing executables to disk from a game process, which antivirus and the game's own file checks will
dislike, and it buys nothing over calling the DLL.

### 5.2 What can live in the single client pak

Everything the front-end needs. The export gives us the real screens:

| need | how, with the export as evidence |
|---|---|
| a menu entry | override `MainMenu.js` (23 KB, `preact.Component`) and add one button to its carousel |
| navigate to team selection | `FrontendNavigation.set(ScreenID.SelectTeam)` - `ScreenID.SelectTeam = 33`, already exists |
| per-player team choice | `TeamSelectMenu.js` already does it: `onToggleTeam` reads `DS.getValue(teamCTX,'teamID')`, calls `FrontendUtil.setSelectedTeamIndex`, and commits through `ConfirmChangeTeamDialog` -> `onConfirmChangeTeam` |
| host/client choice | a second screen, or a mode switch on the same one |
| talk to the agent | `fetch('http://127.0.0.1:<port>/coop/...')`, which the bridge can already do |

So the pak needs: `Frontend.html` (already overridden), `js/coop/bridge.js` (exists), a `js/coop/lobby.js`
(new), and `MainMenu.js` overridden to add the button - plus whatever `TeamSelectMenu.js` override is needed
to surface "you are player 2, pick your team". That is **one pak**, on both machines, client and host.

`FrontendNavigation.set` is also better than the current approach of firing `engine.trigger('ContinueGame')`:
it dispatches `FrontendScreenChanged` through the engine properly **and** updates the module state that the
next screen reads. A raw `sendEvent` does not. That is worth changing regardless of this feature.

### 5.3 File budget per side

Current reality is more files than necessary, and the count is the thing being minimised:

| file | host | client | why it must be separate |
|---|---|---|---|
| `CoopBridge_P.pak` | yes | yes | the UI and bridge; identical on both |
| `coop_agent.dll` | yes | yes | injected; cannot be in a pak because it is loaded by the injector |
| `coop-link.exe` | yes | yes | it is the process; cannot be in a pak |
| `coop-supervisor.exe` | yes | no | host only; **only needed if the agent does not host in-process** |
| `coop-inject.exe` | yes | yes | injects the DLL |
| `coop_hook.enable` | yes | yes | opt-in flag |
| `coop_tyres.txt`, `coop_mode.txt`, `coop_player.txt` | yes | yes | runtime config, editable without a rebuild |

Reductions worth doing:

1. **Fold the injector into `coop-link`.** It already links `coop-mem`; a `--inject` subcommand removes one
   file from both sides. Real saving, no risk.
2. **Have the agent host the relay thread**, so `coop-supervisor.exe` is optional on the host and absent on the
   client. The relay is ~150 lines of `TcpStream` handling; it does not need its own process on the host when
   the agent is already there.
3. **Config defaults.** `coop_hook.enable` can default to *enabled when `coop_mode.txt` exists*; `coop_tyres.txt`
   already defaults to `observe`. That removes two files.
4. **One config file** with `mode=`, `player=`, `team=`, `loadout=` lines, replacing three.

Target after that: **pak + 3 files per side** (`coop-link.exe` including inject, `coop_agent.dll`, one config
file), and the host additionally gets the relay **in-process** rather than as a fourth file.

What cannot be reduced: the DLL must be a separate file to be injectable, and `coop-link` must be an
executable. Those two are structural.

### 5.4 Team selection per player

The game already models this: a save has a `Player` row with one `TeamID`, and `ChangeTeam` is a first-class
screen (`ScreenID.ChangeTeam = 14`) whose confirm path is `onConfirmChangeTeam` ->
`ConfirmChangeTeamDialog`. So the flow is not "invent a lobby"; it is:

1. Host creates/loads the career, then uses `SelectTeam` to pick the team both players will share (shared-team
   mode) or their own (versus).
2. The host patches the save's `Player`/`Player_State` rows for the *guest* - which is a single-row SQL update,
   already proven on a live DB - so the guest's copy opens as their own team.
3. The host ships the save (already implemented, byte-identical, hash-checked).
4. Both load, and `coop_authority` decides who may write what from then on.

The `Player` table has exactly one row, which is the real constraint: **two players in two teams cannot both be
"the Player" in one save**. `COOP-PLAN` §4 already treats the save as host-owned for versus, and the guest
operates on the same save with `coop_auth` preventing writes outside their team. That is workable but it is a
compromise worth stating plainly: the game has one player slot, so versus is two clients sharing one career
that belongs to the host's slot.

### 5.5 What to build, in order

| # | task | needs game | needs DLL |
|---|---|---|---|
| 1 | fold `coop-inject` into `coop-link --inject` | no | no |
| 2 | collapse three config files into one | no | no |
| 3 | `coop_agent` hosts the relay thread | no | yes |
| 4 | HTTP endpoint on the agent: `POST /coop/host`, `GET /coop/status` | no | yes |
| 5 | `lobby.js` in the pak: menu button, mode pick, team pick | partly | no |
| 6 | override `MainMenu.js` to add the entry | partly | no |
| 7 | switch the bridge from `sendEvent` to `FrontendNavigation.set` | yes | no |

Items 1-2 are pure refactors, testable now. 3-4 are agent work, also testable now (the relay has its own
selftest). 5-7 need the game to confirm the UI renders.

---


Send `(host_sim_time, host_state)`; the client sizes its correction by the **time gap**, not the raw
distance gap.

- gap < 20 ms: no correction, so a healthy client is not jittered;
- otherwise scale by `clamp(gap / 250 ms, 0, 1)`, applied as a bounded per-frame delta;
- hard cap per frame, so a large gap produces fast catch-up rather than a teleport. A teleport is what
  "entire locations desynced" looked like as the sim re-integrated.

Pure function of numbers, unit-testable including the failing case (300 ms behind, 60 km error -> bounded).

Correction cannot synthesise a pit stop or a red-flag restart. For those, either replicate the resulting
state (positions, pit flags, part condition - all now mapped) or move to replicating **inputs**
(`Save_DriverCommands`) and letting each sim run its own transition. Resulting-state first; inputs for
transitions later.

---

## 6. Implementation order

| # | task | scope | offline | status |
|---|---|---|---|---|
| 1 | `sim_time` on the wire, staleness keyed on it | race | yes | **done** |
| 2 | `correct.rs` time-gap-scaled correction | race | yes | **done** |
| 3 | authorisation layer `(mode, team, loadout)` | **all modes** | yes | **done** |
| 4 | part state: 6 damage levels, 4 tyre conditions | race | needs offsets | not started |
| 5 | `starthold` wired, gated on `sim_time` going backwards | race | partly | not started |
| 6 | `Save_Weekend` stage agreement | weekend | yes, SQL | not started |
| 7 | calendar `Day` host-only | career | yes, SQL | gate ready |
| 8 | finances/board host-authoritative | career | yes, SQL | gate ready |
| 9 | facilities/development writable only by principal | career | yes, SQL | gate ready |
| 10 | `Msg::Ready` handshake + UI gate | both | yes | message done, not wired |

Items 1-3 are complete and covered by 113 passing tests. Item 3 unblocks items 7-9: the gate exists, they
need callers.

---

## 7. Honest status

- **Works:** DLL injection; game-thread hook (25-60 calls/s sustained); mailbox write path; transport at
  ~19.8 Hz; race-control delivery; tyre observation; 20-check preflight gate; `sim_time` carried and
  staleness keyed on it (27 proto tests, 8 hold tests, 43 agent tests).
- **Works briefly then diverges:** location sync - reached `verified 22 of 22 (worst dist delta 0.00 m)`
  and never held, for §3.2.
- **Not started, now cheap:** the career side and all three mode rules - SQL-shaped, offline-testable.
- **Not started, hard:** pit / red-flag transition fidelity.

No full race has yet run with a synced client, and no mode's authorisation exists. Next two steps are
`correct.rs` and the authorisation layer; neither needs the game.
