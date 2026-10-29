# F1 Manager 2024 â€” Co-op Anything notes

## Game identity (recorded 2026-10-26)
- Install: `E:\Steam\steamapps\common\F1 Manager 2024`
- Exe: `F1Manager24.exe`
- SHA-256: `73378a55acfcf90f8884654b05353db188ca18708c7842186f0edc980f5f1dcd`
- Steam AppID: 2591280, Build ID: 17356935
- File version: 1.11.0.39891 / Product version: 1.11.0.498643
- Company: Frontier Developments plc.
- Engine: Unreal Engine 4 or 5 (detected from .pak/.ucas/.utoc content files)
- Anti-cheat: none detected by engine scan

## Directory status
- No existing entry in `data/directory.yaml`
- No existing write-up in `games/`

## Phase 1: Research â€” complete (2026-10-26)
Searched: this repo, GitHub repos+code, Nexus Mods (403 blocked), ModDB (404), Steam discussions, Reddit r/F1Manager, web search.
Result: No co-op project exists for F1 Manager 2024.
Added gap entry to data/directory.yaml (kind: gap, status: none, qa: none).
Rebuilt DIRECTORY.md (313 entries).

Existing UE4/5 co-op references on same engine:
- Lies of P Co-op (Ember Wave) â€” playable, steam-relay, 2 players
- Hogwarts Legacy Wandrealm â€” playable, dedicated-server, up to 512
- CrabSync (Crab Champions) â€” playable, shared inventory
- Half Sword Multiplayer Mod â€” prototype, steam-relay, up to 6

Active F1M24 modding scene: DatabaseEditor (save editor, v3.6.2), F1Manager2024Logger (telemetry), trainers, setup tools â€” no multiplayer work.

## Phase 2: Workspace â€” complete (2026-10-26)
Test install copied to `local/f1-manager-2024/test-install` (isolated from owner's install).
- UE4SS v3.0.1 already installed with 10 mods including SplitScreenMod and ConsoleEnablerMod
- Engine: UE 5.1 (from UE4SS-settings.ini EngineVersionOverride)
- Save location: `C:\Users\<you>\AppData\Local\F1Manager24\Saved\` (SaveGames, SaveGames2, SaveGames4, SaveGamesS)
- Instance-local config created in test-install\F1Manager24\Saved\Config\Windows\ (Engine.ini, GameUserSettings.ini, Input.ini) â€” windowed 1280x720, VSync off
- No anti-cheat detected

## Phase 3: Understand â€” COMPLETE (2026-10-26)

### CRITICAL REALIZATION: Cannot isolate Steam game
- Steam games CANNOT be launched from an isolated copy â€” Steam always relaunches from owner's install
- UE4SS cannot be tested in test-install because the exe triggers Steam to use owner's install
- All live execution tracing must happen in the OWNER's install (E:\Steam\steamapps\common\F1 Manager 2024)
- UE4SS is NOT installed in owner's install (no ue4ss.dll there)

### What we KNOW from disassembled save data (static snapshot of live DB)
The save is GVAS wrapping zlib-compressed SQLite. Disassembled CSVs at `E:\Downloads\ThatFinnishGuy Save Viewer\Save data disassembled\`:

**Race weekend tables (written after each session completes):**
- `Races` â€” RaceID, SeasonID, Day, TrackID, State(0=Pending,1=InProgress,2=Complete), weather, weekend type
- `Races_PracticeResults` â€” SeasonID, RaceID, PracticeSession, DriverID, TeamID, TimeOnTrack, SessionDuration, BestLapTime, BestLapTyre, LapCount
- `Races_QualifyingResults` â€” SeasonID, RaceID, QualifyingStage, SprintShootout, FinishingPos, DriverID, TeamID, FastestLap, DNF, LapCount
- `Races_SprintResults` â€” SeasonID, RaceID, FinishingPos, DriverID, TeamID
- `Races_Results` â€” Season, RaceID, FinishingPos, DriverID, TeamID, Laps, Time, FastestLap, Points, DNF, overtakes, etc.
- `Races_PitStopTimings` â€” SeasonID, RaceID, TeamID, DriverID, PitStopID, PitStopStage, Duration, IncidentDelay, Lap
- `Races_PitStopResults` â€” SeasonID, RaceID, TeamID, FinishPosition, FastestPitStopTime, DriverID, Points
- `Races_DriverStandings` / `Races_TeamStandings` â€” SeasonID, ID, Points, Position
- `Races_Tracks` â€” TrackID, Name, Laps, TrackLength, SafetyCarChance, etc.
- `Save_RaceSimManager` â€” CarIndex, Active, Fuel, EngineTemperature, BrakeTemperature, RacePosition, TotalRaceTime, LapCount, DRSState, etc.
- `Save_RaceSimCars` â€” CarIndex, PitStopState, PitStopStartTime, various pit stop stage times
- `Save_RaceSimCars_Overtake` â€” CarIndex, OvertakeActive, DefenderCar, AttackerInitialLine, etc.

**Management tables (between sessions):**
- `Staff_DriverData` â€” driver stats, contracts, marketability
- `Staff_Contracts` â€” driver contracts, salary, end season
- `Finance_TeamBalance` / `Finance_Transactions` â€” team money
- `Parts_Designs` / `Parts_Projects` / `Parts_Items` â€” car part development
- `Teams` â€” team info, colors, location
- `Player` / `Player_History` / `Player_Record` â€” player career data

**Race weekend flow (from save data):**
Practice (1-3 sessions) â†’ Qualifying â†’ (optional Sprint) â†’ Race â†’ Post-race standings update
Each session writes its results to the corresponding table when complete.

**CRITICAL LIMITATION**: Static dumps tell us WHAT data exists but NOT when each table is written during live gameplay.

### What we NEED to understand (live execution â€” must be done in owner's install)
1. **Race weekend flow**: What UFunctions fire when practice â†’ quali â†’ race â†’ post-race?
2. **Live data generation**: Is race sim data computed live or pre-baked?
3. **Save timing**: When exactly does the game write saves between sessions?
4. **Netcode**: Any client/server architecture, or purely single-player?
5. **UE4SS hooking**: Can we hook race session functions?

### UE4SS status
- UE4SS v3.0.1 installed in test-install (dwmapi.dll + UE4SS.dll) but CANNOT be tested there
- CoopDiagMod created but cannot run without UE4SS in owner's install
- Owner's install has NO UE4SS â€” would need to install it there (touches owner's install)

### Co-op architecture assessment
- **No netcode exists** â€” single-player management sim
- **UE4SS Lua hooks available** IF installed in owner's install
- **Save sync between sessions** â€” patch saves at race weekend boundaries (weak co-op)
- **Race session sync** â€” main technical challenge; need to sync race sim state between players
- **Approach**: UE4SS mod that hooks race start/end, patches saves between sessions, syncs race results via network

### Next step
Phase 4: Choose â€” design co-op architecture and networking model. Live UE4SS tracing requires installing UE4SS in owner's install (needs owner permission).

## CORRECTIONS â€” 2026-10-26 (supersedes the wrong claims in the Phase 3 section above)

### Retracted claims
1. "Correct UFunction names unknown" â€” FALSE. They are in the FModel Blueprint JSON `FuncMap`:
   - `/Game/RaceSim/BP_RaceSimNotificationsManager.BP_RaceSimNotificationsManager_C:RaceSimEventReceived` (param `i_event`, a `RaceSimEvent`)
   - `/Game/RaceSim/RaceSimChroma.RaceSimChroma_C:RaceSimEventRecieved` (sic, "Recieved"), plus PlayRedFlagAnims / PlayYellowFlagAnims / PlayCqFlagAnims / PlaySwooshAnims / OnSwooshStart / OnSwooshStop
   - `/Game/RaceSim/FullScreenMap/CarIconWidget.CarIconWidget_C:` SetWidgetData / OnRacePosChanged / OnSetCarWidgetData / OnSelectedChanged / OnMinimalChanged / PreConstruct
2. "RaceSimDataAsset holds Races_Results, Staff_DriverData, Finance_TeamBalance, Parts_Designs, Player_History" â€” FALSE. RaceSimDataAsset.json contains only sim tuning (tyre wear thresholds, driver accuracy, lift-and-coast, etc.). Those table names came from the save file's SQLite DB. Two sources were conflated.
3. "All RegisterHook guesses failed" â€” wrong conclusion. The 4 failures in the 02:40:35 run were hooks registered at mod-load time, before the race Blueprint classes existed. The session then ran until 03:04 without crashing.
4. "Use native hooks, never RegisterHook" â€” contradicted by the evidence. RegisterHook works; the timing was wrong.

### Verified working (v14 log)
- `RaceSimEventReceived` hooked and FIRED 100+ times during a race; first firing 03:18:22, 100th at 03:19:43.
- `CarIconWidget_C:OnRacePosChanged` and `SetWidgetData` hooked at 03:18:33 (after the race loaded), i.e. late registration works.
- No crash from any of these hooks.

### v12 defects found in the log
- Hooks registered at startup while classes unloaded (all stamp 02:40:35).
- Wrong class for two hooks (Minimap_C instead of CarIconWidget_C).
- `main.lua:46` raised `attempt to index a nil value (global 'discovered')` 302 times.
- `print()` adds no newline, so log lines ran together.
- Nothing captured past the main menu.

### Process fixes
- v14 saved to `local/f1-manager-2024/CoopDiagMod_v14_backup.lua` before any edit.
- v16 source kept at `local/f1-manager-2024/CoopDiagMod_v16.lua`, copied into the game folder.

### Still unverified
- Memory offsets from the PitWallAI/FormationLap notes (0x9A, 0x88, 0x99AF1C, CAR_BASE 0x95BC00, stride 0x10D8) â€” v16 deliberately does NOT read/write memory; it only reports property offsets via the reflected API so they can be compared.
- Save timing and save format claims.
- Whether the offsets match this build.

## CoopDiagMod version history â€” lessons (2026-10-26)

| Version | Outcome | Defect |
|---|---|---|
| v1-v2 | crashed | invented UFunction paths under `/Script/F1Manager24.*` |
| v8 | worked | class discovery via NotifyOnNewObject only; no hooks |
| v12 | worked, no data | hooks registered at load time (classes unloaded); wrong class for 2 hooks (Minimap_C); `discovered` used without `local` -> 302 Lua errors; print() has no newline |
| v14 | WORKED | hooks registered from GameModeBase/PlayerController creation 8s after load; RaceSimEventReceived fired 100+ times |
| v15 | not used | callbacks declared (self, context, event) shifting args; tryReadMemory before definition; log ignored args; unsafe get_memory |
| v16 | not usable | dropped v14's NotifyOnNewObject triggers, so the only retry depended on the very hook it needed; single retry; tostring(nil) became "nil" |
| v16.1 | installed for test | restores triggers, retries every 10th event while pending, nil-safe readProp |

### Rules that came out of this
1. Never register hooks only at mod load. Race Blueprint classes are not loaded yet.
2. Keep object-creation triggers (GameModeBase / PlayerController) â€” they are what actually worked.
3. Do not let a retry depend on the hook it is trying to register.
4. Retry more than once: CarIconWidget_C was absent at 03:18:07 and present at 03:18:33.
5. Hook callbacks are (self, ...) â€” never named params after self.
6. tostring(nil) is the string "nil"; check for nil before formatting.
7. print() in UE4SS adds no newline.
8. Offsets from a third-party write-up are unverified for this build; only trust values read back through the reflected API.

## v16.1 race test with red-flag restart â€” 2026-10-26 04:02:39-04:06:39 (log verified)

### What the run shows
- 1200 RaceSimEvent events, 67 distinct m_type values, fully accounted (sum = 1200).
- No read failures, no walk/dump failures, no CALLBACK ERROR. Log ends on the expected FLAGS line -> not truncated.

### Red flag restart IDENTIFIED in the event stream (user confirmed a restart occurred)
| Constant (FormationLap source) | Observed m_type | Count | First appears |
|---|---|---|---|
| EVT_RED_FLAG_START = 54 | 54 | 2 | by event 300 @ 04:04:59 |
| EVT_RED_FLAG_END = 55 | 55 | 1 | by event 350 @ 04:05:20 |
| EVT_RED_FLAG_RACE_RESTART = 87 | 87 | 1 | by event 350 @ 04:05:20 |

This is the first independent confirmation of FormationLap's event-ID table against observed game data on this build
(the other being RaceSimManager.m_eventManager at 0x180, seen in the previous run).

### Event frequency (top 12 of 1200)
34=155, 74=155, 23=143, 24=137, 22=137, 11=80, 12=74, 3=35, 10=27, 70=25, 5=25, 31=20.

### Hooks
- OnRacePosChanged: 68 firings. Arg printed as "RemoteUnrealParam: <addr>" because the mod used tostring() on the
  param wrapper instead of :get(). The hook fires; the value is NOT yet read.
- SetWidgetData: 0 firings in a full race.
- FLAGS: 25 attempts, every one "red=? yellow=? chequered=?" - IsRedFlag/IsYellowFlag/IsChequeredFlag exist on
  CircuitStateComponent but the direct Lua calls raise.

### Coverage against the co-op data list
| Requirement | Status |
|---|---|
| Car locations live | NOT captured. Event carries m_carId only, no x/y/z. RaceSimCarAIRace has 0 reflected members, so its raw offsets are the only route (unverified). |
| Mistakes live | CAPTURED. Incidents arrive as m_type events with m_carId/m_otherCarId. |
| Time scale live | Not called yet. RaceSimManager:GetPlaybackRate exists. |
| Team commands game->client | Not started. |

## CoopDiagMod v17 â€” read-only call-convention probe (deployed 2026-10-26)

v16.1 is preserved at `local/f1-manager-2024/CoopDiagMod_v16.1_backup.lua` (15465 bytes,
sha256 DC151D22...). v17 source lives in `local/` and is deployed to the owner install.

### Why v17 exists
In v16.1 the three flag functions raised inside pcall and printed `?` 25 times, and
GetPlaybackRate (the time scale) was never callable at all. Both need "how do you invoke a
native UFunction from UE4SS Lua on this build", which we do not know. So v17 probes that,
read-only, while keeping the proven event capture running.

### Offline check toolchain (no Lua interpreter exists on this machine)
- `luasyntax.py` â€” real Lua grammar via the already-installed tree_sitter_lua package.
  Validated: v16.1 and v17 parse with zero ERROR nodes; a deliberately broken file is rejected.
- `luaglobals.py` â€” names used but never declared. Validated: catches `discovered` (the v12
  crash) and undefined calls; v16.1 and v17 both report clean.
- `luacheck_structural.py` â€” block/vararg balance, string.format arg counts,
  local-function use-before-definition, and that every string we grep the log for is printed.
Result: v17 PASS on all three.
What these cannot catch: wrong UE4SS API signatures, and runtime errors from a valid-looking
call. Only the game can.

### v17 behaviour
- Startup: class lookup, hook registration attempt, GameModeBase/PlayerController triggers
  (v16.1 behaviour, unchanged), then the API surface probe.
- API surface probe enumerates metatable method names of a live RaceSimManager and logs
  subsets matching mem/read/ptr/addr/func, plus globals matching memory/read/deref/pointer/dump.
  This answers, without calling anything, whether raw memory access even exists.
- First event: native class dump, ERaceSimEventType runtime enum dump, CircuitStateComponent
  property VALUES, flag probe, GetPlaybackRate probe.
- probeCall tries in order: A obj[f](obj), B ufn(obj), C ufn:Call(obj), D obj:ProcessEvent(ufn,nil),
  logging which form worked. D is the only step with any crash risk; it runs only if A-C failed.
- Every 50th event: counts (with RED_FLAG names for 54/55/87), flags, playback rate.
- Key binds: F9 retry hooks, F8 re-dump natives, F7 re-run probes.

### Not changed from v16.1
Hook triggers, retry cadence (first event + every 10th while pending), first-30 full event
dumps, native dump content, nil-safe readProp.

### Verified so far
Offline only. Three structural/grammar checks pass and are validated against a known-good and a
known-bad file. **Nothing in v17 has run in the game yet.**

## CORRECTIONS to my v16.1 log analysis (2026-10-26, after review)

These were overstated and are retracted:

1. **"Mistakes live: solved" is wrong.** Events carry m_type + m_carId + m_otherCarId, but only
   three of the 67 numbers have any candidate meaning (54, 55, 87). Spins, lockups, collisions,
   punctures and pit errors are NOT identified. Requirement status: **unresolved** until the
   ERaceSimEventType mapping exists.
2. **"Names confirmed from observed data" overstates it.** 54 appeared twice, 55 once, 87 once,
   in the single window the user reported as a red flag restart. That is corroboration from one
   event window against FormationLap's constants - not a decoded enum. The 2x START / 1x END
   asymmetry is unexplained and is not evidence either way.
3. **"Not truncated" was not shown.** After event 30 the mod only writes at COUNTS blocks, so a
   complete log and a truncated copy both end on a COUNTS/FLAGS line. The last 750 events arrived
   in 54 seconds, so the session may still have been running when the log was taken. Unknown.
4. **"SetWidgetData never fires in a race" was premature.** It most likely fires when the
   full-screen map is opened, and we do not know the log covers a whole race. Correct statement:
   it did not fire while the hook was registered during the observed window.

## CoopDiagMod v17.1 - v17 with review defects fixed (deployed 2026-10-26)
v17 was deployed but NEVER run. Live file is now v17.1.
Backups kept: v14, v16, v16.1, v17 (two copies), v17.1 source.

### Fixes
1. probeApiSurface was only called at mod load, where there is no RaceSimManager instance, so it
   logged "no instance" and returned - the API ALL / API globals lines would never have appeared.
   Now: scanGlobals (instance-free) runs at startup AND probeApiSurface runs from runProbes at
   the first event.
2. Failed flag calls now log the per-form attempt log (`FLAGS red probe: ...`).
3. Removed the ProcessEvent call form. Passing a null parameter block to a function that returns a
   value risks an access violation that pcall cannot catch. Forms left: A obj[f](obj),
   B ufn(obj), C ufn:Call(obj), all Lua-level.
4. getCircuitState now validates the cached object and drops it when invalid.
5. Enum dump gains a ForEachName attempt alongside GetNameByIndex / GetEnumNameByIndex.
6. New probeCarAIs: enumerates RaceStrategistManager.m_carAIs through reflection, logs element
   count and each element's full name. No offsets touched. Tests whether the 22 AI objects are
   reachable without raw memory access.
7. Struct-ish hook params logged as arg-lua-type/handle, not "value".

### Startup trace (hand-verified against file order)
mod load, top level:
  1. CLASS LOOKUP x5            StaticFindObject on the 5 /Script/RaceSim classes
  2. tryRegister("startup")     expected to fail, classes not loaded
  3. NotifyOnNewObject x2       GameModeBase + PlayerController, no output yet
  4. scanGlobals()              API globals matching ... + one line per hit
  5. RegisterKeyBind F9/F8/F7
  6. "CoopDiagMod v17.1 loaded" + 4 FEATURE lines
on GameModeBase / PlayerController creation:
  - tryRegister -> HOOKED x4 + REGISTER attempt N
on event #1:
  - dumpEventFull(event,1)      EVENT PROPS / EVENTPROP / EVENT #1 full
  - FIRST EVENT seen
  - tryRegister("first-event")
  - dumpNativeClasses()         NATIVE DUMP begin .. end
  - runProbes(): ENUM, CSTATE, FLAGS(+probe log), TIMESCALE, CARAIS, API PROBE, PROBES end
every event:
  - count by m_type; every 10th while pending tryRegister; every 50th COUNTS + FLAGS + TIMESCALE

### Offline checks run on v17.1
luasyntax.py (real Lua grammar, tree_sitter_lua): 0 ERROR nodes.
luaglobals.py (undefined names): clean.
luacheck_structural.py (block/vararg, format arg counts, use-before-definition, log strings): PASS.
All three are validated against v16.1 (known good) and against deliberately broken files.
What they cannot catch: wrong UE4SS API signatures, and logic errors. The trace above covers
ordering by hand. **v17.1 has still not run in the game.**

## v17.1 RUN RESULT - 2026-10-26 04:26:32 to 04:31:08 (Hungaroring, 4m36s)
Log: 1765 CoopDiag lines, 1300 RaceSimEvent events, ZERO CALLBACK ERROR.

### SOLVED: time scale is readable live
Form A (`obj[fname](obj)`) WORKS on RaceSimManager. GetPlaybackRate tracked the real session:
  04:27:44  1.0   (race start)
  04:27:52 16.0   (fast-forward)
  04:28:54  1.0   (back to normal)
  04:29:10 16.0
  04:30:00  1.0
  04:31:06 16.0
Values seen: 1.0 x5, 16.0 x22. This is a real 1.0 <-> 16.0 signal with 5 transitions.
Sampling was every 50 events, so transitions are captured coarsely but prove the value is live.

### Convention, now known
- A  obj[fname](obj)  -> WORKS on RaceSimManager (GetPlaybackRate).
- GetFunctionByName -> RAISED on CircuitStateComponent, so it is not a reliable API here.
- B/C untested in practice because A succeeds first.
- ProcessEvent was removed in v17.1 and was not needed.

### RaceSimManager: 21 functions, 21 properties, 1 instance. All callable via A:
OnRaceSimEventReceived, OnPitStopManagerLoadComplete, OnControllerConnectionChange,
OnCarReleasedStart, OnCarReleasedEnd, OnCarLoadsComplete, IsSessionInProgress, IsSessionComplete,
IsRaceSimTimeValid, IsRaceEventActive, GetTrackLength, GetTimeRemaining, GetTeamFromPitStopIndex,
GetSimTimeCompleted, GetSessionTime, GetRaceTime, GetRaceLapCount, GetPlayerTeamCarActors,
GetPlaybackRate, GetLatestRaceTime, GetCurrentLap

### m_carAIs is reachable through reflection
`RaceStrategistManager.m_carAIs` -> lua userdata, 22 elements, 1-BASED indexing.
Each element resolves to a named UObject, e.g.
  RaceSimCarAIRace .../RaceStrategistManager.RaceSimCarAIRace_2147480922  (down to ...901)
So all 22 AI objects are reachable WITHOUT raw offsets. RaceSimCarAIRace itself still reports
0 functions and 0 properties, so its FIELDS are still only reachable by raw offset.

### ERaceSimEventType: 120 entries (values 0..119)
ForEachName WORKS and yields (name, value) in value order, but the name arrived as
FNameUserdata and tostring() printed the address, not the text. Need FName -> string.
NumEnums does not exist (-1). GetNameByIndex / GetEnumNameByIndex both failed.
The enum has no reflected properties to walk.

### CircuitStateComponent (Hungaroring)
Full name: .../Lvl_Hungaroring.Lvl_Hungaroring:PersistentLevel.BP_RaceSimGameMode_C_2147482187.CircuitStateComponent
4 properties:
  CircuitData              UScriptStruct
  m_mapOfCarIDsToDatas     read-FAILED (TMap does not read through obj[name])
  m_raceSimManager         UObject
  m_raceWeekendManager     UObject
Flags still unresolved: IsRedFlag/IsYellowFlag/IsChequeredFlag are NOT reachable by form A,
and GetFunctionByName raises on this object. All 27 FLAGS lines were nil.
m_mapOfCarIDsToDatas is now the most interesting target: keyed by car ID, named "datas".

### Hooks
0 at startup -> 2 at 04:26:39 (GameModeBase) -> 4 at 04:27:44 (first event). No key presses.
RaceSimEventReceived: 1300 firings.
OnRacePosChanged: 61 firings.
SetWidgetData: 0 (full-screen map likely never opened).
RaceSimChroma RaceSimEventRecieved: 0 - registered OK but never fired; instance may not exist.

### Event payload confirmed again (4 properties)
  m_triggeredOnce BoolProperty
  m_type          EnumProperty    <- value 70 = 0 in this run's first event
  m_carId         ByteProperty
  m_otherCarId    ByteProperty    <- 255 = none

### Not achieved
- Event type NAMES: blocked only on FName -> string.
- Flag state: not readable yet.
- Car positions: not read yet. Two reflective routes now open (m_carAIs elements, and
  RaceSimManager:GetPlayerTeamCarActors + m_mapOfCarIDsToDatas).
- API surface enumeration returned 0 names: getmetatable exposes no method table.
  This is INCONCLUSIVE, not proof that no methods exist.
- Globals scan: 7 hits, all benign (DumpAllActors, DumpAllObjects, DumpStaticMeshes, DumpUSMAP,
  ExecuteInGameThread, MainThread, AsyncThread). No raw memory reader global.

## v18 RUN: LOADED, NEVER REACHED A RACE - 2026-10-26 04:35:42 to 04:35:53 (log ends 04:36:30)

v18 loaded. Two hooks registered (RaceSimEventReceived, RaceSimChroma). Two UI hooks still
pending. 28 CoopDiag lines. NO "FIRST EVENT seen", NO "PROBES begin", NO enum / getter /
CARAIS / CIRCUITDATA output. The session never got to a race, so none of v18 four new
investigations ran. v18 told us nothing new. Time scale remains established by the v17.1 run.

Also observed: `late not ready: function: 000000009E6DC290` - RegisterHook raises with a
function object as the error value, so tostring() of it carries no information.

## CoopDiagMod v18.1 - six logic fixes, applied before any run (deployed 2026-10-26)

1. FName conversion. v18 stored method names and field names in one variable and then called a
   cached field as a function; it also evaluated nm[fnameMethod] outside pcall. Rewritten to use
   FName:ToString() first and UEnum:GetNameByValue(value) as the fallback. No dual-purpose state.
2. Event counting order. v18 classified event #1 before runProbes populated enumNames, so event #1
   could be counted as "0" while later events of the same type counted as "0 Spin" - one type,
   two buckets. The first-event probe block now runs before classification (verified by line order).
3. CircuitData. v18 treated a struct value as struct metadata. Now uses
   cs:Reflection():GetProperty("CircuitData") -> confirm StructProperty -> GetStruct() -> walk.
   The old direct route is kept as a second attempt and both outcomes are logged.
4. m_mapOfCarIDsToDatas now also goes through Reflection() so the engine reports the property kind
   instead of only "read-failed".
5. AI field discovery now walks the superstruct chain via GetSuperStruct() (depth <= 16) and tries
   reading every field on every class, then reports a total. "0 reflected fields" becomes a measured
   result rather than an assumption.
6. Hook diagnostics: failures now report whether the target object exists via StaticFindObject
   ("target not found (class or function not loaded yet)"), and successful registrations log the
   returned pre/post hook IDs.

Extra: GameMode and PlayerController full names are logged when created, so the log shows which
level the session actually reached.

Deliberately NOT changed: the hook success test still treats RegisterHook not raising as success.
Prior runs prove that is correct (1300 events fired). Requiring both returned IDs would risk never
hooking if the API returns one or none. IDs are logged and reported when missing.

## Offline checks on v18.1
luasyntax.py: SYNTAX OK (0 ERROR nodes). luaglobals.py: NO UNDEFINED GLOBALS.
luacheck_structural.py: PASS. Re-run against the deployed live bytes: same three results.
Still NOT verified in game.

## v18.1 RUN - BREAKTHROUGH - 2026-10-26 04:48:00 to 04:50:30 (Hungaroring, 1200 events)

Full race captured. 678 CoopDiag lines. All probes ran. 0 callback errors.
Timeline: main menu 04:48:00 -> Hungaroring race 04:48:0x -> 1200 events -> lap 70/70.

### ERaceSimEventType DECODED - all 120 entries, saved to ERaceSimEventType_full.txt
`FNAME method works: ToString` then `ENUM entries: 120`. Names are ERaceSimEventType::Xxx.

**My three FormationLap-derived inferences are now CONFIRMED against the runtime enum:**
  54 = RedFlagStartEvent
  55 = RedFlagEndEvent
  87 = RedFlagRaceRestart
(FormationLap also said EVT_RED_FLAG_RACE_RESTART = 87. Exact match. Its memory offsets are
therefore far more credible than before, though still unverified.)

### MISTAKES ARE SOLVED
45 event types observed with names. The incident vocabulary, observed in this race:
  25 Lockup=1, 26 Spin=1, 27 RunWide=1, 30 DamageToTyre=1, 73 PartWearWarning=5,
  76 WeatherStateChangeWarning=1, 79 PlayerERSDepleted=1, 97 CarAboutToBeTeleported=2,
  102 TyreTemperatureChangeWarning=2, 115 CornerSqueezeAttemptStart=2
And available but not hit this race: 16 PitStopMistake, 28 CollisionWithBarrier,
29 CollisionWithCar, 31/32/33 DamagedCarPart/Body/Floor, 71 CarDNF, 72 TeamDNF, 110 Puncture,
113 PenaltyInvestigation, 114 PenaltyNoFurtherAction, 116/117/118 MechanicalFault*,
111 CornerCollisionAttemptStart, 98 CarComeToRestAfterCollision, 93 DebrisOnTrack,
94 SlowCarOnTrack, 90 OvercutUndercutAttempt.
Each event carries m_carId and m_otherCarId. This requirement is closed.

### FLAG STATE IS SOLVED WITHOUT IsRedFlag
IsRedFlag/IsYellowFlag/IsChequeredFlag remain unusable (26/26 nil). It does not matter:
flag and safety-car state arrive as typed events.
  46/47 StillYellow, 48/49 WavedYellow, 50/51 DoubleYellow, 52/53 YellowRed,
  54/55 RedFlag, 56/57 Chequered, 64-69 per-sector yellows,
  38-41 SafetyCarRequested/OnTrack, 60/61 SafetyCarOut, 42/43 VirtualSafetyCar,
  44/45 WaitingForLightsOut, 58/59 DRSEnabled
The earlier red-flag episode is exactly 54 -> 55 -> 87.

### TEAM COMMANDS ARE OBSERVABLE AS EVENTS
104 DriverCommand fired 9 times. Also 96 RequestPitStop, 91 StrategySuggestion,
92 StrategyChanged, 84 EnterPitWindow, 85 StartOptimalPitLap, 13 PitStopAdjusted,
107 SendCarOut, 83 DriverPenalty. So commands the player issues are already in the event
stream with a car id. Reading them is solved; applying them on another instance is not.

### RaceSimManager getters - all live
IsSessionInProgress=true, IsSessionComplete=false, GetRaceTime=5090.05, GetLatestRaceTime=same,
GetSessionTime=same, GetSimTimeCompleted=5091.0, GetTimeRemaining=2109.95, GetCurrentLap=59,
GetRaceLapCount=70, GetTrackLength=4310.608, GetPlaybackRate=1.0.
GetTrackLength 4310.6 m matches Hungaroring (4.381 km) - a correctness check that passed.
These raise: IsRaceSimTimeValid, IsRaceEventActive. GetPlayerTeamCarActors also raises (needs args).

### CLOCK line works
lap advanced 60 -> 70, racetime 5116 -> 6129, remaining 2083 -> 1070, rate saw 1.0, 8.0 and 16.0.

### CircuitData now enumerated - 15 fields (weather and track state)
TimeOfDayUDSFloat, Rain, WaterOnTrack, AmbientTemperature, TrackTemperature, RubberOnTrack,
TimeUntilTrackDry, UDSState, **EventFlags (EnumProperty)**, WeekendStage, StageInnerStep,
GuiScreen, SessionStartTimes, **RaceTrack (EnumProperty)**, **SafetyCar (EnumProperty)**.
Field VALUES were listed but not read. EventFlags and SafetyCar are the likely flag source.

### m_mapOfCarIDsToDatas identified
REFLECT -> **MapProperty at off=0x158**. Direct read raises (UE4SS cannot convert TMap to Lua).
So the property is real and known; only the Lua read path is missing.

### AI fields: now measured, not assumed
CARAIS CLASS chain walked: RaceSimCarAIRace -> RaceSimCarAI -> Object.
CARAIS total fields across hierarchy: **0**.
So across the full inheritance chain there are genuinely no reflected fields. Positions and AI
state are NOT reachable through reflection. Raw offsets remain the only route.

### Still open
- Car positions (x/y/z) for all 22 cars.
- Applying team commands on another instance.
- CircuitData field values, and how to read a MapProperty.

## POSITION STREAMING: route analysis and decision (2026-10-26)

The user asked for the most accurate way to stream car positions. Routes were evaluated against
the evidence collected so far. Only ONE is viable for per-frame, all-22-cars data.

| Route | Data available | Frequency | Verdict |
|---|---|---|---|
| Reflection on RaceSimCarAIRace | none (0 fields across the full chain, measured v18.1) | - | dead |
| OnRacePosChanged hook | position ordinal only, no car id, no coordinates | 438/race (>100 on track) | insufficient |
| SetWidgetData(CarWidgetData) | 56-byte native struct | 115 in one burst at UI load | insufficient |
| CircuitData struct | weather/track/session state, 15 fields | on read | not car data |
| m_mapOfCarIDsToDatas | MapProperty @0x158, car id -> data | unknown | unreadable from Lua (TMap) |
| **per-car sim state array in RSM** | **Dist, Lat, Head, Spd, world Pos x/y/z** | **per frame** | **chosen** |

### Chosen layout (RSM = RaceSimManager object pointer, which reflection already gives us)
car record = RSM + 0x95BC00 + carIndex * 0x10D8
  Lat   +0x170 f32   lateral offset from the racing line
  Head  +0x184 f32   heading
  Dist  +0x194 f32   race distance, continuous across laps
  Spd   +0x198 f32   speed
  Pos0/1/2 +0x1E0/+0x1E8/+0x1F0 f64   world position

These are offsets from the OBJECT POINTER, not absolute module addresses. That is why they are
stable: we obtain RSM through FindAllOf("RaceSimManager"), which we have used since v16.1.

For a racing client, Dist + Lat + Head is the right payload: it places the car on the track spline
and is 12 bytes per car per frame vs 24 for three doubles. Pos0-2 is read too so the run can say
whether the two agree.

### Mechanism (UE4SS 3.0.1 has no Lua memory API)
Custom-property "memory window": 6 ArrayProperties registered once on
/Script/RaceSim.RaceSimGameMode at OffsetInternal 0x168 (AActor NetCullDistanceSquared/NetTag/
NetUpdateFrequency/MinNetUpdateFrequency - 16 bytes of plain floats). Inside a window those bytes
are borrowed as a fake TArray header (Data = chosen base, Num = Max = 0x7FFFFFFF) and restored on
every exit path, including error paths. view[k] then loads base + (k-1)*ElementSize.

LICENSING: FormationLap ships no license for its own Lua, so its memwindow.lua was NOT copied.
Only the field offsets and the described mechanism were used, both recorded here. If a co-op mod
ever ships using this, either ask the author or clean-room the implementation.

### Safety properties of v19
- Game thread only: all memory access happens inside hook callbacks. No LoopAsync.
- 16 header bytes are saved before and restored after every window, including when fn raises.
- Offset alignment verified offline: every f32 offset is 4-aligned, every f64 offset 8-aligned,
  relative to base 0x95BC00 and stride 0x10D8. Max offset 0x971FA8 < window limit 0xA20000.
- Self-test gates everything: memory SimTime (RSM+0x99AF1C) must agree with reflected
  GetRaceTime() within 5 s, or positions are NOT reported.
- Registration happens once per process and is never retried; UE4SS cannot unregister a custom
  property, so a second attempt would fail and disable positions for later sessions.
- v18.1 is kept as the working fallback: CoopDiagMod_v18.1_backup.lua.

### Sampling
Hooks BP_RaceSimNotificationsManager_C:ReceiveTick (bCanEverTick true, confirmed in the FModel
export) to measure the real achievable sample rate, logging TICKRATE every 600 ticks. If that
hook never fires, event-hook sampling keeps the run useful.

### Offline checks on v19
luasyntax.py SYNTAX OK; luaglobals.py clean; luacheck_structural.py PASS (block/vararg, format
arg counts, use-before-definition, log strings). Re-run against the deployed bytes: same results.
Offset/alignment arithmetic verified separately in Python.
NOT RUN IN GAME YET.

## v19 CODE REVIEW - bugs found by reading, before any run (2026-10-26)

v19 was deployed but never run. Every defect below was found by reading the source, comparing it
against FormationLap's published mechanism, and checking assumptions against the FModel export.
v19.1 fixes all of them and is what is now live.

### CRITICAL - crash risk
1. **No object-size guard.** v19 hardcoded a 0xA20000 window and read RSM+0x99AF1C without ever
   checking the RaceSimManager object was that large. If FormationLap's offsets do not match this
   build, that is a read ~10 MB past the allocation: an access violation, which pcall cannot catch.
   FormationLap guards every read (`f[2] + SZ[f[3]] > sz -> return nil`) and requires
   `rsmSize > 0xA1328B` before using the window at all. v19 had neither.
   FIX: read the real object size (UStruct::PropertiesSize at UClass+0x58, i32) and use it as the
   window limit, so winIndex rejects out-of-object offsets with a Lua error instead of faulting.

2. **The self-test read the dangerous deep offset before validating anything** - if the offsets
   were wrong, the test itself was the crash.
   FIX: layered validation, safest first. (a) H1 low dword must equal NetUpdateFrequency float bits,
   proving the property really overlays AActor+0x170; (b) RSM+0x10 must equal the RSM class pointer
   (near read); (c) only then the deep SimTime read, and only if the object contains it;
   (d) SimTime must match reflected GetRaceTime within 5 s.

### HIGH - the run would have been invalid
3. **getRSM() called FindAllOf on every tick.** A full UObject scan per frame: stutter plus the
   TICKRATE figure would have measured the scan, not the frame. Then my own first fix introduced
   the same problem again by calling objSpan (which opens a window) per tick.
   FIX: cache address AND size; re-resolve only on session change.

4. **probesDone was set true before runProbes ran.** One failed attempt (RSM not up at event #1)
   disabled positions for the whole session.
   FIX: bounded retry, every 25 events, up to 8 attempts.

5. **F8 dumped the car table even when the self-test had failed** - printing plausible garbage,
   the exact thing the file header promised not to do.
   FIX: F8 refuses unless the pipeline is proven.

### MEDIUM
6. **Dead preference logic.** getRaceGameMode preferred the gamemode owning m_raceSimManager, but
   that property does not exist on RaceSimGameMode - verified absent from BP_RaceSimGameMode.json;
   it belongs to CircuitStateComponent. Removed: FindAllOf("RaceSimGameMode") already cannot match
   the main-menu gamemode, which derives from a different class.
7. **TICKRATE mislabelled.** os.clock is CPU time, not wall time, so "rate=/s" was wrong. Now uses
   os.time and says so when the interval is too short to measure.
8. **CAR_COUNT was a bare 22.** Now each record is plausibility-checked and the plausible count is
   logged.
9. **Silent disable.** A dead holder killed the window with no message. Now logged.

### Data-honesty fixes
10. All-zero or identical reads would have passed the plausibility gate - the most likely signature
    of a wrong base offset. Now detected and reported as SUSPECT.
11. A failed f64 unpack printed as 0.00, hiding the failure. Now prints "?".
12. Dead state removed: simTimeReported, lastTableTick, lastLeaderDist, WIN.ok, WIN.failures,
    the unused early flag on hook targets.

### Verified after the fixes
luasyntax.py SYNTAX OK; luaglobals.py clean; luacheck_structural.py PASS. objSpan now has exactly
two call sites, both cached. Re-run against the deployed bytes: same results.
v18.1_backup.lua kept as the known-working fallback. STILL NOT RUN IN GAME.

## v19.1 LIVE RUN - CAR POSITIONS PROVEN - 2026-10-26 05:09:45 onwards (Marina Bay)

### Validation chain: all four steps passed
WIN: register ok=true
WIN: bound holder=0x12DA67480
VALIDATE overlay ok: H1 low dword == NetUpdateFrequency
RSM address 0x16DE90000 size 0xA132F0
VALIDATE classptr ok: RSM+0x10 == 0x260AF280
VALIDATE simtime: memory=1.000 reflection=0.075966902077198   delta 0.924 s -> ok
POSITIONS proven - car positions will be read

**RSM object size = 0xA132F0.** FormationLap's own guard requires rsmSize > 0xA1328B.
0xA132F0 > 0xA1328B, so its offset table fits this build exactly. This is now evidenced, not assumed.

### Positions are live and correct
22 of 22 car records read on track. Starting grid table showed:
  lat = -9.000 / -8.800 / -8.600 (one grid column) and -2.100 / -2.000 / -1.900 (the other)
  z = -31.38 constant (flat track), dist spread 3.83 .. 131.73 m
That is an unmistakable staggered starting grid.

Under way at tick600: spd 34.9 .. 61.1 m/s (126 .. 220 km/h), dist 299 .. 448 m.

### Both position representations cross-check
Compared Delta(dist) against straight-line Delta(xyz) for consecutive grid slots:
  11 of 15 pairs agree within 0.2 m; several agree to 0.00-0.01 m.
  The 3 outliers (2.5-2.7 m) are exactly the pairs whose lat differs:
    pair 4-5 lat -1.4 vs -8.4 = 7.0 m lateral; sqrt(7.9^2 + 7.0^2) = 10.55 vs measured 10.57.
So Dist (along track), Lat (lateral) and Pos (world) are the same data in two coordinate systems,
and all three are correct.

### Sampling rate measured
TICKRATE ticks=600 wall_seconds=14 rate=42.9/s
So ~43 Hz position sampling from BP_RaceSimNotificationsManager_C:ReceiveTick. That is the
streaming budget. (Event hook would be far too slow; the tick hook is the right driver.)

### Bug found in my own plausibility gate
v19.1 rejected negative dist, so on the starting grid it reported only 17 of 22 cars and marked
5 as implausible. Negative race distance is legitimate there: cars behind the start line carry
-4 m to -36 m. Gate widened to -10000 .. 2000000. Redeployed as v19.1.

### Also working this run
ENUM entries: 120 (names resolved via FName:ToString). Event counting active. Zero CALLBACK ERROR.

### Open question
head is 0.0000 for most cars and non-zero only for a few (0.03, -0.88, 2.31, 6.39, 7.74, 9.27).
Most likely head is the deviation from the track direction rather than an absolute bearing, but
that is NOT established. Do not treat head as absolute heading until it is tested.

## DATA INJECTION: distance to a live test, and v20 (deployed 2026-10-26)

### What a shipping mod writes vs what we need
FormationLap sets car fields through `cs(rsm, i, key, v)` -> `rsm_write(...)`:
  Lat ............ WRITTEN (1 site)
  Spd ............ WRITTEN (3 sites)
  Lap, Launch, Brk, Stuck, RacePos, pit records ... WRITTEN (many)
  Dist ........... NEVER written (read only)
  Pos0/1/2 ....... NEVER written (read only)
So the write mechanism is proven on this build, but the two fields a position stream wants
(Dist, world Pos) have never been written by anyone we have evidence for.

### v20: first injection test
Three probes on three separate cars, ~100 ticks each, automatic, self-restoring:
  phase 1  car 1  Lat  <- 7.0            (control: FormationLap proves Lat is writable)
  phase 2  car 2  Dist <- current + 25   (the field a client stream needs; unproven)
  phase 3  car 3  Pos0 <- current + 50   (world x; unproven)
Then phase 4: replay the recorded past of car 0's Dist into car 1 for 140 ticks - the actual
"record then replay" mechanism, in miniature.
Read-backs at ticks 1, 3, 10, 40, 100 record whether the sim kept, consumed or reverted each value.
Every phase writes the original value back and logs that it did.

### Also in v20: the recorder the plan needs
CoopDiag_replay.csv next to UE4SS.log (Binaries\Win64), appended, 22 rows per tick:
  tick,car,dist,lat,head,spd
Flushed every 400 rows. Capped at 200,000 rows (~4 min at 43 Hz) so a long race cannot fill the
disk. A 400-slot in-memory ring of car 0 feeds the in-run replay test.

### Verified offline
Lua grammar OK, no undefined globals, structural PASS.
The injection state machine was simulated in Python with fake memory: 3 phases advance, each restores
its original exactly, replay completes, done flag set, ~405 ticks (~9 s at 43 Hz).
That proves the bookkeeping. It cannot prove the game accepts the writes.

### Interpretation guide for the log
  "INJECT phase 2 Dist car 2 ... ok=true"  then tick=1/3/10 with delta ~ 0
      -> Dist write sticks; the sim does not clobber it. Replay is viable.
  delta growing negative/positive straight after the write
      -> the sim consumes our value then advances it. Also viable, but we must write every tick.
  value back to the original within 1-3 ticks
      -> the sim rewrites the field every step. Still usable, but only if we write every tick
         and out-pace the writer.
  Rosy numbers with "POSITIONS proven" absent -> nothing was written at all.

### Risks stated plainly
- Writing Dist to car 2 may make the sim see a 25 m teleport. Event type 97
  (CarAboutToBeTeleported) exists, so the game has some handling for this, but it is untested.
- Only 3 cars are touched, one at a time, and every value is restored.
- v19.1_live_backup.lua is the known-working positions-only build if v20 misbehaves.

### Still missing before real co-op
1. Two game instances running at once - never attempted, possibly the real blocker.
2. Event generation: injecting positions alone does not stop the replay instance's sim from
   producing its OWN mistakes. Preserving mistake placement needs the event path controlled too.
3. Freeing a team's controls while 19 cars are injected - not started.
4. Transport of any kind - none exists.

## v20 SPRINT RUN - INJECTION PROVEN - 2026-10-26 05:29:5x (Marina Bay sprint, 20 laps)

Zero CALLBACK ERROR. Recorder wrote 1.86 MB / 52,669 rows / all 22 cars.
CSV format confirmed: tick,car,dist,lat,head,spd. Grid values identical to the previous run
(car1 43.825/-9.000, car2 67.825/-1.900 ...), so the field is deterministic.

### INJECTION RESULT: all three writes were ACCEPTED by the sim

Phase 1 - Lat, car 1, wrote 7.000 (was -1.662):
  tick 1  -> 4.765  (delta -2.235)
  tick 3  -> 4.765
  tick 10 -> 4.765
  tick 40 -> 4.765
  tick 100 -> 4.674
  The value did not revert to the original. The sim clamped 7.0 to 4.765 once (beyond the track
  edge) and then LEFT IT ALONE for 40+ ticks. So lateral position is ours to set, subject to a
  physical clamp. Restored to original afterwards, ok=true.

Phase 2 - Dist, car 2, wrote 577.192 (was 552.192):
  tick 1  -> 577.192  delta 0.000
  tick 3  -> 577.192  delta 0.000
  tick 10 -> 577.192  delta 0.000
  tick 40 -> 607.486  delta +30.294
  tick 100 -> 798.408 delta +221.216
  **THE WRITE STUCK EXACTLY, then the car simply drove on from where we put it.** The sim consumed
  our value as the new race distance and continued. This is the ideal outcome and it is the field a
  recorded-race replay needs. Restored to original afterwards, ok=true.

Phase 3 - Pos0 (world x), car 3, wrote 6219.39 (was 6169.39):
  tick 1  -> 6219.387  (exactly what we wrote; delta unreadable because v20 forgot to record
             INJ.written on the f64 branch - cosmetic, fixed in v20.1)
  tick 3  -> 6190.699
  tick 40 -> 6158.945
  tick 100 -> 5859.296
  Accepted then recomputed as the car moved, consistent with world position being derived from Dist.
  Restored to original afterwards, ok=true.

### Conclusion for the replay plan
Dist is writable and the sim adopts the written value instead of clobbering it. A recorded race can
be replayed by writing Dist per car per tick. Lat is also writable with a clamp. World Pos is
writable but is not the field to drive, since the sim recomputes it.

### BUG in v20's replay phase (phase 4 did not run)
"INJECT replay: feeding car1.Dist from car0 recording starting at slot 769"
"INJECT replay: recording exhausted at tick 0"
Two causes:
  1. recordTick computed ring = (n % 400) + 1 and then i = ring + 1 - a double increment, so every
     ring slot was written one position too high.
  2. the replay read REC.dist[absoluteTick] while the ring only ever holds indices 1..400, so the
     lookup was nil immediately.
Fixed in v20.1: i = ring, and the replay reduces absolute ticks the same way the writer does.
Verified in Python: after 900 recorded ticks, replaying 140 ticks from 150 back resolves every
slot and each slot holds the tick it should.

### Second bug fixed
f64 (Pos0) branch never set INJ.written, so the read-back logged wrote=-1 delta=?.

### Not yet answered
Dist advanced 191 m between tick 40 and tick 100. At the measured 43 Hz that implies more sim time
than wall time per tick, or a playback-rate change mid-phase (CLOCK showed rate=2.0 later). The
number of sim steps per tick callback is NOT established and should be measured before designing a
wire format, because it sets the required update rate.

## v21 = LIVE VISUAL INJECTION TEST (deployed 2026-10-26, sha256 C6003023...)

### Bearman is car index 17
Verified from the recording at tick 5529 (the sprint): position 3, car 17, dist 14968.96, lat 0.202.
Full order at that moment: 4, 5, **17**, 6, 20, 18, 0, 16, 7, 2, 3, 13, 15, 12, 8, 14, 19, 10, 21, 11, 1, 9.
Caveat: that is an index derived from live distance. I cannot prove index 17 is Bearman rather than a
grid slot, so F3 exists to remove the guess (below).

### Hot reload is OFF, so this needs a restart
UE4SS-settings.ini has EnableHotReloadSystem = 0 and it is read at startup. v20 was still loaded in
the running session and its injection had already completed, so nothing could be triggered on
demand. v21 therefore requires one restart.

### Keys (all writes happen in the tick hook, on the game thread; keybinds only set flags)
  F3  teleport whichever car is CURRENTLY P3 by live distance, +150 m   <- no index guess
  F6  teleport car 17 specifically, +150 m, repeatable
  F5  restore car 17 to the baseline captured on the first F6
  F4  toggle a lateral weave, +-4 m, so the car visibly slides left and right
  F8  dump the car table
  F7  re-run probes
  F9  retry hooks

### Bugs caught while building this (before deploying)
1. F7 was bound TWICE - the old re-probe bind plus the new weave bind. Pressing F7 would have
   cleared posState.proven AND toggled weave, disabling positions. Weave moved to F4.
2. The early-return guard in injectTick omitted PEND.teleRank, so pressing F3 alone would have done
   nothing at all. Guard widened.

### Verified offline
Lua grammar OK, no undefined globals, structural PASS, no duplicate keybinds (F9 F7 F8 F3 F6 F5 F4).
Simulated against the real recorded order: F3 at rank 3 hits car 17 and moves it from P3 to P1 with
a 150 m jump - an unmistakable visual change that also proves the injection path end to end.
F6 twice then F5 restores exactly to 14968.96 / 0.202.

### What a successful visual test looks like
Press F3. The car running 3rd should visibly jump ~150 m up the road and become the leader.
The log gains: "VIS rank3 teleport car 17 dist 14968.96 to 15118.96 ok=true".
That single line plus the on-screen jump is the first end-to-end proof that injected state is
rendered by the game and not just accepted in memory.

## Why no teleport was visible (2026-10-26)

Cause: **the game had not been restarted, so v20 was still the loaded script.** v20 binds only
F9/F7/F8. F3 and F6 did not exist in the running process, so the key presses reached nothing.
UE4SS reads a mod's Lua once at startup and EnableHotReloadSystem = 0, so copying v21 to disk while
the game was running had no effect.

Evidence:
  loaded in process : CoopDiagMod v20 loaded  (binds F9, F7, F8 only)
  on disk           : v21                    (binds F9, F7, F8, F3, F6, F5, F4)
  VIS log lines     : none. The 5 "VIS" matches in the log were engine offset names such as
                      UObjectBase::CurrentBufferVisualizationMode, not mod output.

Nothing about the injection is in question - it never ran.

Fix to prevent the ambiguity recurring: v21 now prints, at load,
  KEYS BOUND: F3=P3 teleport F6=car17 teleport F5=restore F4=weave F8=table F7=probes F9=hooks
so the log states which handlers exist in the running process.

To run the visual test:
  1. Quit the game (the current race cannot receive new code).
  2. Start it again through Steam.
  3. Any on-track session will do; practice is the fastest way in. Positions exist as soon as the
     RaceSimManager is live.
  4. Confirm in UE4SS.log: "CoopDiagMod v21 loaded", "KEYS BOUND: ...", then "POSITIONS proven".
  5. Press F3. The car currently running P3 should jump 150 m and take the lead.
     Expect in the log: "VIS rank3 teleport car 17 dist <a> to <a+150> ok=true".

## COMPLETED RUN HARVEST - v20 sprint, 2026-10-26 05:27:47 to 05:36:06

Archived: run_v20_sprint_full.log (0.17 MB), run_v20_sprint_full.csv (7.44 MB).
1355 CoopDiag lines, ZERO callback errors. TICKRATE settled at 43.1-43.5/s.

### Recording quality
9101 ticks, 200,222 rows, ALL 22 cars present in every single tick (100%), zero malformed rows.

### TWO DESIGN FAULTS FOUND - both fixed in v21 before the next run

1. **The row cap truncated the recording at 56% of the session.**
   maxRows was 200,000, which is 9090 ticks at 22 cars. Recording stopped at tick 9101 while the
   race continued past tick 16200. Raised to 1,000,000 (~19 min, ~22 MB).

2. **Tick count is NOT a usable time base.** Measured sim distance advanced by the leader per tick:
     ticks    1.. 501 : 0.72 m/tick
     ticks 4501..5001 : 2.48 m/tick
     ticks 8601..9101 : 1.20 m/tick
   A 3.4x swing, because the playback rate changes mid-session. A replay driven by tick number would
   play back at the wrong speed and desync. The CSV had no sim-time column at all, so the archive
   cannot be corrected afterwards.
   Fixed: every row now starts with simtime, format is
     simtime,tick,car,dist,lat,head,spd
   Header is only written into an empty file, so a re-run cannot inject one mid-file. The old
   6-column CSV was removed so new 7-column rows cannot mix with it.

### Other observations
- **head is NOT an absolute bearing.** Car 0's head was exactly 0.0 in every one of 9101 ticks.
  Non-zero values appear only as small deviations (0.03, -0.88, 2.31, 6.39, 7.74, 9.27). Treat head
  as deviation from the track direction, not a compass heading.
- Lateral range per car stayed within -9.00 .. +7.43 m, consistent with a ~16 m wide track.
- Field spread grew 168 m to 796 m over the recording.

### Full event vocabulary observed this race (1000 events, named)
Overtaking: 3 AttemptOvertake=64, 4 SuccesfullOvertake=21, 5 FailedOvertake=41, 103 NotifyFailedOvertake=12
Lap/sector: 34 CompletedLap=118, 24 FinishLinePassed=118, 22/23 checkpoint=110 each,
  6 NewBestLap=3, 70 NewPersonalBestLap=76, 7/8/9 sector bests=8/2/5, 10/11/12 personal=82/69/61
Positions: 36 PositionGained=5, 37 PositionLost=2
Start: 44/45 WaitingForLightsOut=1/1, 35 StartingLightChange=7, 1 RaceStart=1, 58 DRSEnabledStart=1
Wear: 74 TyreWearWarning=52, 73 PartWearWarning=4, 102 TyreTemperatureChangeWarning=2,
  79 PlayerERSDepleted=2, 116 MechanicalFaultDeveloped=1
Commands: 104 DriverCommand=14, 115 CornerSqueezeAttemptStart=7

## v21 LIVE RUN: KEY AND WRITE BOTH WORK - THE PICTURE DOES NOT FOLLOW (2026-10-26)

v21 loaded correctly: "CoopDiagMod v21 loaded" and the KEYS BOUND banner both present at 05:38:03.
F3 fired 31 times. Every write reported ok=true.

### The write definitely lands, and the simulation adopts it
Interleaving F3 presses with the SAMPLE lines (which read all 22 cars) shows the written value
becoming the new field maximum within one sample:

  SAMPLE n=1  hiDist=131.7   (no press yet)
  F3 -> car 6 115.93 to 265.93, car 5 123.83 to 273.83
  SAMPLE n=2  hiDist=273.8   <- matches the write
  F3 -> car 5 148.52 to 298.52
  SAMPLE n=3  hiDist=298.5   <- matches
  F3 -> car 6 190.00 to 340.00
  SAMPLE n=4  hiDist=340.0   <- matches exactly

So: keybind works, window write works, the value survives, and the sim treats it as the leader's
distance. What does not happen is anything on screen.

### Why, most likely (to be confirmed, not asserted)
FormationLap's own field table says the car FACES are driven by a 30 Hz history ring, not by the
car sim state:
    HistData RSM+0xEB0 (i64 ring pointer), HistNum RSM+0xEB8 (i32 frames, ~9000 = 300 s at 30 Hz)
    "the 30 Hz history ring the car actors unpack every frame"
If that holds, writing Dist/Lat changes race logic and timing data but the rendered car keeps
reading a frame that was written earlier. That would be the missing link.

### Also ruled out this run
- Not a stale-file problem: the KEYS BOUND banner proves the right script was live.
- Not a key problem: 31 callback firings.
- Not a permissions problem: ok=true on every write, and read-backs agreed.
- Not a "sim reverts it" problem at this point in the session: hiDist followed the write.

### v22 deploys the probe that settles it
Reuses the proven read path, adds RENDER PROBE:
  - reads HistData / HistNum and reports how many seconds of ring exist
  - tries 9 candidate car-actor class names via FindAllOf and reports instance counts
Runs automatically once positions are proven, and again on F12 on demand.
If the ring exists and an actor class is found, the next version writes the ring (or the actor
transform) instead of the sim state.

## CRITICAL: v21 has been the only script that ever ran (2026-10-26 05:38 onward)

Checked at 05:41:49: the log still shows "CoopDiagMod v21 loaded" and no v22 or v23 line. The process
has been up since 05:38:03. So:
  - v22 never ran, and its RENDER PROBE has never produced output
  - v23 never ran
  - every key press so far went to v21
The only "RENDER" strings in the log are engine offset names (LastRenderTime, bDisableWorldRendering),
not mod output.

UE4SS reads a mod's Lua once at startup and EnableHotReloadSystem = 0, so copying a new file into
Mods/ while the game runs changes nothing. A full quit and relaunch through Steam is required each
time. I should have said this as a blocking precondition rather than assuming it.

## What the v21 evidence actually establishes, and what it does not

Established:
  - F3 fires (31 callback log lines), the window write returns ok=true, and the value read back
    through the SAME window persists across later samples (hiDist followed it: 273.8, 298.5, 340.0).

NOT established:
  - that the write reaches the real RaceSimManager. Read and write use one mechanism, so a read-back
    only proves the window agrees with itself. "ok=true" is a claim about the Lua assignment, not
    about the game object.

That distinction is why "nothing changed" is the most informative result so far: it is consistent with
the write never landing at all.

## v23 settles it with one key (deployed, sha256 FFCF68E4...)
F2 writes the playback field (RSM+0x99AF24) and then asks the GAME for its own value through the
reflected GetPlaybackRate() getter, which is a separate route into the object. The UFunction call is
made after the window is closed, following FormationLap's rule that no UFunction runs inside a window.
  PLAYBACK window read before / after      - what MY mechanism thinks
  PLAYBACK reflection before / after       - what the GAME thinks
  PLAYBACK VERDICT: write is REAL ...      -> write path good, car rendering is a separate problem
  PLAYBACK VERDICT: write had NO EFFECT    -> window writes never reach the object, and every
                                              ok=true reported so far was meaningless
The side effect is visible on screen: the session speeds up to 16x. F1 restores it.

Also carried over from v22, and it auto-runs once positions are proven (read only, no key needed):
  RENDER ring HistData=... HistNum=...      - does the 30 Hz history ring exist, how many seconds
  RENDER actor class <name> -> N instance(s) - which car actor class is live, out of 9 candidates

## Required sequence
  1. Fully quit the game. Check UE4SS.log stops growing.
  2. Relaunch through Steam.
  3. In the log confirm, in this order:
       CoopDiagMod v23 write-path test loaded
       KEYS BOUND: F2=playback write test ...
       POSITIONS proven - car positions will be read
       RENDER PROBE begin / end          (automatic)
  4. Press F2 once. Read the PLAYBACK VERDICT line.
Nothing else is worth pressing until that verdict is known.

## v23 RAN. RENDER PROBE ANSWERED. PLAYBACK TEST SILENTLY DEAD. (2026-10-26 05:43-05:44)

v23 DID load this time: "CoopDiagMod v24..." no - "CoopDiagMod v23 write-path test loaded" plus the
KEYS BOUND banner at 05:43:02. So the restart happened.

### RENDER PROBE RESULT - the 30 Hz ring theory is CONFIRMED, and the actors are findable
  RENDER ring HistData=0x133B10000 HistNum=9000
  RENDER ring holds 9000 frames = 300.0 s at 30 Hz
  RENDER actor class BP_BaseCar2022_Actor_C -> 22 instance(s)
  RENDER actor class CarActor -> 23 instance(s)
So:
  - The history ring exists exactly as FormationLap described: a 300 s, 30 Hz buffer.
  - There are 22 BP_BaseCar2022_Actor_C actors, one per car. These are the VISUAL cars.
  - A second class, CarActor, has 23 instances (probably 22 plus a template).
This gives two routes to visible motion that do not depend on the car sim state:
  (a) write the ring frame the actor unpacks, or
  (b) move the actor itself through reflection.

### The playback write test never executed: a definition-ORDER bug
F2 fired three times ("PLAYBACK: F2 queued") and not one "PLAYBACK window read" line appeared.
Cause:
    line 739  local function injectTick
    line 752      pcall(playbackTest, ...)   <- at this point playbackTest is still a GLOBAL (nil)
    line 887  local function playbackTest     <- declared 135 lines too late
pcall(nil, ...) returns false and the error is swallowed, so the test failed silently. Same class of
defect as v15's tryReadMemory, and my "use before definition" check did not catch it because it only
looked for `name(` call syntax and missed a name passed AS AN ARGUMENT.

### Checker hardened
luacheck_structural.py now also flags a local referenced as an argument before its definition.
Verified: pointed at v23 it reports exactly "line 752: uses local playbackTest before its definition
on line 887". Pointed at v24 it reports clean. So the gap that let v23 ship is now covered.

### v24 (deployed, sha256 F59FB0A3...)
  - playbackTest moved above injectTick. Order verified: playbackTest 741, injectTick 782.
  - NEW: actor transform test. F11 finds the 22 BP_BaseCar2022_Actor_C instances, logs instance 1's
    full name, reads its location through K2_GetActorLocation / GetActorLocation /
    RootComponent.RelativeLocation (logging which route works), then tries the available setters.
    If no location route works it dumps the actor's class properties instead, so the run still
    returns information. F10 reports the saved location.
  - Everything else unchanged: F2 playback write test, F1 restore, F3 P3 teleport, F6 car 17,
    F5 restore, F4 weave, F12 render probe, F8 table, F7 probes, F9 hooks.
  - Verified: no duplicate keys (12 keys), every pcall target defined before use, grammar OK,
    no undefined globals, structural PASS.

### Why the actor route matters for co-op
Moving a visual actor needs no memory window and no offset validation: it is the engine's own API.
If F11 shows a working location setter, then a co-op client can be made to DISPLAY remote cars
correctly long before the simulation agrees with the host.

## v24 ACTOR PROBE: reads WORK, writes need the right calling form (2026-10-26 05:47-05:52)

v24 loaded (05:47:01) and the render probe ran again with the same result:
  RENDER ring HistData=0x1AF260000 HistNum=9000  -> 300 s at 30 Hz
  RENDER actor class BP_BaseCar2022_Actor_C -> 22 instance(s)
  RENDER actor class CarActor -> 23 instance(s)

### Actor read path WORKS
  ACTOR 1 = BP_BaseCar2022_Actor_C /Game/Circuits/MarinaBay/...BP_BaseCar2022_Actor_C_2147481991
  ACTOR location via K2_GetActorLocation = table
K2_GetActorLocation returns successfully and hands back a Lua table. So car world positions are
readable through the engine's own reflection API, with no memory window and no offsets at all.
This is a second, independent source of position data to cross-check the window reads against.

### Actor write path rejected (both forms)
  ACTOR setter K2_SetActorLocation ok=false
  ACTOR setter SetActorLocation    ok=false
The failure value is a function object, i.e. UE4SS rejected the argument marshalling rather than the
object. Likely cause: K2_SetActorLocation expects (FVector, bool, FHitResult&, bool) and the out
parameter cannot be nil. Not yet diagnosed - v24 only tried two forms and did not log the table shape.

### v25 (deployed, sha256 BEC53F0F...)
  - dumps the location table so its key form is known (X/Y/Z vs 1/2/3)
  - tries FIVE setter forms, each isolated:
      K2_SetActorLocation(target)
      K2_SetActorLocation(target, false)
      K2_SetActorLocation(target, false, nil, true)
      SetActorLocation(target)
      RootComponent.RelativeLocation = target
  - reports which form the engine accepted, then re-reads the location and dumps it so the move
    can be confirmed numerically even if nothing is visible
  - probes common car-id property names on the actor (SimCarID, m_carId, CarID, CarIndex, ...) so we
    can map actor -> car index instead of assuming
  - F10 still reports the saved location, F11 runs the test
Verified: grammar OK, no undefined globals, structural PASS, 12 unique keys, every pcall target
defined before use.

### Still unanswered from the same session
The playback write test (F2) has NOT been run. It is now correctly ordered, so a single F2 press
gives the verdict on whether window writes reach the game object at all.

## CORRECTION: the playback write DID work. My verdict line was invalid. (2026-10-26)

### What I got wrong
v23/v24's playbackTest compared the field at RSM+0x99AF24 (FormationLap labels it "Playback,
displayed race time") against GetPlaybackRate(), which is a SPEED MULTIPLIER. Different quantities.
The comparison could never match, and the "VERDICT: write had NO EFFECT" line was meaningless.
The user observed cars moving when F2 was pressed, which contradicts that verdict.

### What the log actually shows after 53 F2 writes
  05:52:50  simtime 76.0      advancing normally
  05:53:09  WRITE 93.68 -> 16.0
  05:53:09  simtime 95.0
  then, for the next 3+ minutes:
    simtime (memory, RSM+0x99AF1C)  frozen at 95.0
    car loDist/hiDist (memory)      frozen at 3542.0 / 4266.7 across consecutive samples
    GetRaceTime (reflection)        ADVANCING  26.48 -> 46.65
    cars on screen                  moving (user observation)

Conclusions that now hold:
1. **Window writes reach the game object.** 53 writes to a time field halted the simulation. That is
   the proof, not the GetPlaybackRate comparison.
2. **Rendering is decoupled from the simulation.** With the sim frozen, the display clock advanced
   and the cars still moved. The renderer reads its own timeline (the 300 s / 30 Hz history ring),
   not the car sim state. This explains why writing Dist changed sim state but not the picture, and
   it is the property that makes ghost/replay cars possible at all.

### Process lesson
A single repeatable write bound to an easy-to-mash key wrote 53 times into a live time field and
froze the race. Writes to time/playback fields are being removed from keybinds.

### Standing conclusions (unchanged, now better supported)
  memory reads of car state .... proven (22/22 cars, 43 Hz, cross-validated)
  memory writes ................ proven to land and to affect the game
  sim adopts written Dist ...... proven
  render follows sim state ..... DISPROVEN
  render follows the ring ...... strongly implied (sim frozen, picture alive)

### Next test
v25 F11: move a BP_BaseCar2022_Actor_C through the engine reflection API. No window, no sim state,
no freeze risk. Also dumps the location table shape and tries 5 setter forms.

### Addendum: the freeze was TEMPORARY, the session recovered
  05:56:53  simtime 105.0  loDist=4039.6  hiDist=4742.6
  05:56:56  simtime 107.0  loDist=4179.6  hiDist=4884.9
Sim time and car distances resumed advancing about 3 minutes after the last write. So a write to
RSM+0x99AF24 stalls/scrubs the simulation briefly and the game then continues. That matches the user's
observation exactly: pressing F2 makes the cars move, and they settle back to the same state.

Revised conclusion: **there is a field we can write that provably moves the cars on screen, and the
game recovers from it.** That is the first demonstrable injection-to-pixels link. It is crude (it
scrubs the displayed timeline rather than placing a car) but it proves the render path is writable
from Lua and that a wrong value is survivable.

No restart is strictly required - the session recovered.

## SEEK IS THE CHOSEN METHOD - first characterisation (2026-10-26, user-directed)

User observation after heavy F2 use: lap stuck, faults disappeared, some damage cleared, and
"this is the method we need to use". Agreed - driving the simulation by seeking its time, so state is
RECONSTRUCTED rather than patched field by field, is a better architecture than writing Dist/Lat per
car. It would bring damage, faults, laps and positions along together instead of needing each synced.

### But the log shows the seek is PARTIAL, not whole-state
  06:01:20  lap=10/20  rt=880.0
  06:01:23  lap=10/20  rt=39.4     <- race time rewound 880 -> 39, LAP STAYED 10
Then memory reads froze (sim=907.0, all car distances identical across samples) while GetRaceTime
kept advancing.

So one seek restored the clock and the damage/fault state, and did NOT restore the lap counter. That
is the "lap stuck / faults vanished" the user saw. A seek that restores some state and not the rest
produces an internally inconsistent session, which is worse than no seek for authoritative co-op.

### Two things must be established before this method can be trusted
1. What EXACTLY does a seek restore, and what does it not? Needs a controlled single seek with
   before/after snapshots of lap, race time, playback field, and a fingerprint of all 22 car
   distances - not repeated presses.
2. Is the reconstruction DETERMINISTIC? Seek to the same time twice and compare. If identical, a
   client can be driven to the host's state by time alone. If not, the seek is a lossy buffer lookup
   and cannot be authoritative.

### Measurement hazard found and fixed
After a seek, our window reads froze. Either the cached RaceSimManager went stale or the memory block
stopped updating. Every measurement after a seek was therefore suspect. v26 adds checkStale: it
compares memory SimTime against reflected GetRaceTime every 60 ticks and re-resolves the RSM if they
diverge by more than 30 s, logging "STALE ... - re-resolving RSM".

### v26 (deployed, sha256 CB702AB0...)
  F2  ONE seek back 30 s, with SNAP before/after  (no repeat - repeating is what corrupted it)
  F1  ONE seek forward 30 s, same snapshots
  F11 actor transform test (engine reflection route, untouched by any of this)
  F3/F6/F5/F4 position writes, F12 render probe, F8 table, F7 probes, F9 hooks
  Every key is single-shot. The repeatable time write that produced 97 writes and a broken lap
  counter is gone.
Verified: grammar OK, no undefined globals, structural PASS, no duplicate keys, definition order
checked (checkStale 1056 before onTick 1120).

### The SNAP line is the whole experiment
SNAP before lap=? racetime=? playback=? cars=22 distSum=? first=? last=?
SNAP after  lap=? racetime=? playback=? cars=22 distSum=? first=? last=?
Comparing the pair tells us which parts of the state moved together. If distSum changes but lap does
not, the seek is clock-only and the method needs extra fields written in the same operation
(candidates from FormationLap: SimTime 0x99AF1C, SimDone 0x99AF20, Playback 0x99AF24, ToD 0x99AF2C,
RcClock 0xA0458C, plus per-car Lap 0x7E4 / LapT 0x7E8 / Dist 0x194).

## TWO-INSTANCE LIVE TEST READY - v27 extract+inject (2026-10-26)

### Environment changes made
1. **Hot reload ENABLED** in both installs: UE4SS-settings.ini had EnableHotReloadSystem = 0 in the
   owner install AND the test install. Both now = 1. This needs ONE restart each to take effect,
   after which mod edits reload without relaunching. That removes the biggest cost of the last hour.
2. Test install confirmed to have UE4SS (dwmapi.dll, UE4SS-settings.ini, Mods/CoopDiagMod, its own
   UE4SS.log) and CoopDiagMod enabled in Mods/mods.txt.
3. Sandboxie-Plus present at local/tools/Sandboxie-Plus (Start.exe, SandMan.exe). Existing sandboxes:
   DefaultBox, Steam, Steem under C:\Sandbox\<you>.

### v27 design: same mod, role from a file
  owner install  Mods/CoopDiagMod/Scripts/coop_role.txt = "host"
  test  install  Mods/CoopDiagMod/Scripts/coop_role.txt = "client"
HOST publishes the whole field every tick to
  E:/Downloads/coop-anything-main/local/f1-manager-2024/coop_state.txt
CLIENT reads it every tick and writes Dist+Lat into its own 22 car records.

Wire format (582 bytes, one write call):
  COOP1 <simtime> <tick>
  <car>,<dist>,<lat>,<spd>      x22
  END
Validation before applying: header must start COOP1, last line must be END, and there must be 22
parsable rows. A torn read is therefore rejected rather than half-applied.

### Verified offline
- Parser accepts a full write (22 rows) and rejects: a 2/3-truncated file, a file truncated exactly
  before END, and a corrupted header. All three report the right reason.
- Payload is 582 bytes, small enough that a single write rarely tears, and tearing is detected anyway.
- Lua grammar OK, no undefined globals, structural PASS.
- Host writer hardened: os.rename may not be available under UE4SS, so it falls back to a direct
  write and reports "HOST publish failed at tick N" instead of silently claiming success.

### Sandboxie and the file transport
Sandboxie lets reads fall through to the real filesystem but redirects writes into the sandbox
overlay. So host -> file -> sandboxed client WORKS. The reverse (client writing something the host
reads) would land in the overlay and is deliberately not attempted yet; it would need an
OpenFilePath exception for the shared folder.

Consequence for diagnosis: the CLIENT's UE4SS.log is itself redirected into the sandbox, so it will
appear under something like C:\Sandbox\<you>\<box>\drive\E\Downloads\...\test-install\... rather
than next to the install.

### Success criterion, and how to read it
Client log, every 100 ticks:
  CLIENT applied=N maxDistDelta=X
maxDistDelta is measured BEFORE each write, i.e. how far the client's car had drifted from the host's
value since the previous tick. At ~56 m/s and 43 Hz a car moves about 1.3 m between writes, so a
delta of a few metres means the client is tracking the host. Hundreds of metres means it is not.

That number, not a screenshot, is the first real measure of extract+inject working across two
instances.

---

## v35 - why the flags never cleared (2026-10-26)

Reported: "Flags are not being cleared" after v34 was loaded on both sides.

### Observation
Transport was healthy: `coop_state.txt` published `RC,1024,1,0` and the client logged
`CLIENT applied=3080 maxDistDelta=0.00 timeDelta=+0.032`. Clock was synced, cars tracked, but the
red flag stayed up. Neither log contained a single line about race control, because v34 never
logged it.

### Cause 1 - the client never wrote the values
Reading `clientApply` in v34: the RC footer was parsed into a local `rcVals` and then dropped. No
`winWrite` was ever issued for `flags`, `rcstate` or `rfstate`. v34 was a no-op for race control,
so the earlier claim that it "mirrors race control" was wrong. Note the parse also needed offset 2,
3, 4 because `tonumber("RC")` is nil and occupies index 1.

### Cause 2 - 0x24C is not what the HUD reads
FormationLap main.lua:1117-1123 documents that the HUD lap header, the trackside digiflags and the
notifications read a per-second flags HISTORY ring, "never RSM+0x24C live":

  record k at RSM+0x976F68 + k * 0x14   (first u32 = copy of the flags)
  count     at RSM+0x99ABE8             (i32, capacity 7328)
  shown now at k = floor(RSM+0x99AF24)  (Playback)

FormationLap's own restart clears 0x100 from the ring while leaving it set in 0x24C, and notes the
sim never reads the ring. So 0x24C and the ring must both be written or the flag stays on screen.

### Cause 3 - why the client manufactures red flags at all
FormationLap main.lua:819 and config.lua:116: `car + 0x1AC` (f32) is the standstill-while-blocked
timer. The driver adds 1/30 s per step while a car is held and stationary, and race control
compares EVERY car's timer against `RaceControlParams.RedFlagData.StuckCarRedFlagTime` each step.
An injected car looks permanently held, so the timer climbs until race control stops the session.
FormationLap ships `PreventStuckRedFlag = true`, zeroing those timers every sim block. That is the
cause; clearing the flag is only the symptom.

Also: `CircuitStateComponent.EventFlags` read 0 on BOTH instances, including the red-flagged
client, so it is not the red-flag source, and the `IsRedFlag` / `IsYellowFlag` / `IsChequeredFlag`
getters do not exist on the component in this build (callGetter returned nil on both). Those were
dead ends - do not chase them again.

### v35 changes
1. Zero `car + 0x1AC` for all 22 cars every client tick, before touching any flag.
2. Write `0x24C` (i32), `0xA045C0` (u8), `0xA13289` (u8) from the host, only when they differ.
3. `RC.ringSync(wantRed)` mirrors the RedFlag bit into ring records from `floor(Playback) - 1` to
   `count - 1`, at most 64 per tick, matching FormationLap's window.
4. Read-back telemetry: `CLIENT RC flags host=/mine=/after= | rc ... | rf ... | ring=N`.

ringSync is wrapped in pcall: an error inside the memory window aborts the whole window, which
would stop the car sync with it. Same reason every offset stays inside the object size 0xA132F0
(the ring's last reachable record is 0x99AA14, well inside).

Validated offline only so far: syntax, globals, structural, format-arg counts all pass; RC footer
parse and the bit math checked against the live `RC,1024,1,0` line (0x400 = DRSEnabled, not red).

### Not yet verified
Whether writing 0x24C sticks against the client's own race control, and whether RfState returns to
0 and stays there. The next client log answers both; if `after=` differs from the value written,
race control is re-deriving it and the stuck-timer fix is the one doing the work.

---

## v36 - client crash after v35, and the driver-options gap (2026-10-26)

### The crash, and what evidence survived
Client crashed shortly after v35 was loaded. **The pre-crash log is gone**: UE4SS truncates UE4SS.log
on process start, and the restarted instance overwrote it at 07:18:30. No Saved/Crashes folder exists
in either install (none under the Sandboxie overlay either). Checked and ruled out:
  - disk space: C: 118 GB free, E: 189 GB free
  - the recorder: client CoopDiag_replay.csv stopped at 09:14:15, i.e. when v35 loaded, so it wrote
    nothing during the crash window (note: those CSVs are ~165 MB each and are appended forever)

That leaves the race-control writes as the only new code that could run. Prime suspects, both new in
v35 and never exercised before:
  1. `RfState` (0xA13289) / `RcState` (0xA045C0) writes. RfState drives a state machine that hands off
     to restart objects; forcing it from outside can strand a null hand-off.
  2. The flags history ring rewrite, up to 64 records every tick, into a buffer the HUD reads.

Not recovered and not guessed at: the actual exception. Treat the attribution as a hypothesis.

### v36 - staged so a second crash is attributable
Kept (both low risk, both documented as safe in FormationLap):
  - zero `car+0x1AC` stuck timers every tick  (FormationLap ships this as PreventStuckRedFlag = true)
  - write `0x24C` flags from the host when it differs  (FormationLap sets 0x100 there and it persists)
Reduced:
  - ring: only the record the HUD is showing plus the one before, only when it actually carries 0x100
    and the host says green, at most once per second (was 64 records, every tick)
Removed:
  - `RcState` / `RfState` writes. Both are read and logged only. This is the change most likely to
    fix the crash.
Still unproven: whether clearing 0x24C + the displayed ring record is enough to clear the flag on screen.
The `CLIENT RC` log line answers it (flags host/mine/after, ring=count).

Also fixed in the same file: a mid-race mod reload left the client blind, because proving only happened
in onRaceSimEvent (`n == 1 or n % 25 == 0`) and a quiet stint can go minutes without one. onTick now
proves itself, once per second, bounded to 30 attempts, with runProbes forward-declared (it is defined
~300 lines later, so calling it from onTick without the forward declaration would have been a silent
nil call).

### Driver options do not reach the host - by design, not a bug
Reported: changing the driver options on the client did not change them on the host. Correct, and
expected: the transport is one-way. The host publishes, the client consumes. There is no reverse
channel at all, so nothing the client does can ever reach the host.

Where those options actually live (FormationLap main.lua:913-931): the per-car player-command record
at `RSM + 0x770 + car * 0x50`, consumed on the game thread by the command processor. Fields:
  +0x14 stay out pending      +0x46 garage release pending   +0x4A replaceWing
  +0x15 stay out value        +0x47 release flag             +0x4B newTyreSetID
  +0x45 reason                +0x48 pit command pending      +0x4E tyre set selector
                              +0x49 1 request / 0 cancel
FormationLap notes these are "written at any time (bytes, game thread); window mode only", so a byte
copy of the 0x50-byte record per car is enough to push one instance's orders onto the other.

To make the client's orders visible on the host we need, in order:
  1. a second transport file with the opposite direction (client writes, host reads)
  2. the client to publish its 22 command records (0x50 bytes each = 1.76 KB, same order of magnitude
     as the state file already moved at ~36 Hz)
  3. the host to apply them, with an owner rule: a car's orders come from whoever owns that car, not
     from the last writer. Without that the two instances will overwrite each other every tick.

---

## v37 - the freeze was my own probe placement (2026-10-26)

Client froze before any race could start, with v36 loaded at 07:22:11. The log ends at "All mods
re-installed"; the GAMEMODE line that appeared 14 s after load on the previous run never arrived.

Attribution is clean this time because it is a difference, not a guess: the only v36 code that can run
BEFORE the pipeline is proven is the onTick probe added in v36. Everything else in v35/v36 (stuck
timers, flags mask, ring record) is behind `if not posState.proven then return end` and cannot execute
until a race is proven.

Why it freezes: runProbes calls FindAllOf, describeObject and five getters. onTick is the
BP_RaceSimNotificationsManager ReceiveTick hook, so that work ran on the game thread inside a framed
UFunction, up to 30 times at one per second. Each FindAllOf during level load is slow, so 30 of them
is tens of seconds of a stalled game thread. It looks exactly like a freeze and it is one.

Rule to keep: probing never goes on a hot hook. I reached for the tick hook because "the tick fires
constantly" without checking what the probe actually costs. Run the cost of a function before choosing
its trigger.

v37:
  - onTick back to `if not posState.proven then return end`
  - runProbes called only from onRaceSimEvent and from F7 (pcall) - audited, two call sites
  - event retry gate n % 25 -> n % 5 so a missed prove is retried within a few events, bound 8 -> 20
  - banner now says: after a mod reload, press F7 if the instance stays silent

Manual step accepted: probing is event driven, so a reload during a quiet stint needs one F7. That is
cheaper than another freeze, and F7 was already proven to work (it re-proved the client at 07:07:06-era
runs without trouble).

### Two failures, one root cause
v35 crashed and v36 froze, and both were the only new code that could execute at the time. That is not
bad luck, it is my process: I kept adding to the untested path instead of changing one thing. v37 goes
back to the v34 baseline that ran for hours, with the race-control work kept but inert until proven, so
the next failure - if there is one - can only be the flags work.

---

## v38 - the RC footer never parsed, and the client adopted a menu-state host (2026-10-26)

### Bug: the footer parse was broken in every build, so nothing was ever mirrored
v37 finally ran properly (CLIENT applied=2188 maxDistDelta=0.00 timeDelta=+0.041 - injection was
fine), but every RC line read `flags host=nil`, `rc host=nil`, `rf host=nil`.

Cause, in v34 through v37:
    local vals = {}
    for v in rcLine:gmatch("([^,]+)") do vals[#vals + 1] = tonumber(v) end
    if #vals >= 4 then rc = { flags = vals[2], ... } end

tonumber("RC") is nil. Assigning nil at index 1 leaves the table empty (#t == 0), so the NEXT value
lands at index 1: vals becomes {0, 1, 0} with #vals == 3, the guard fails, and rc stays nil. The tag
token does not occupy a slot - it deletes one. Measured, not reasoned: the emulation prints
"#vals = 3 ... guard passes? False".

Fixed with explicit captures, no array involved:
    local a, b, c = rcLine:match("^RC,([^,]+),([^,]+),([^,]+)$")
Tested against the live footer `RC,1024,1,0` -> ('1024', '1', '0').

So the red flag was never chased. Three builds in a row (v34, v35, v36) had the intent but not the
mechanism, and the only thing that said so was a log line reading nil. Check that the value arrived
before believing the write did anything.

### Bug: the client mirrored a host that was not racing
"Client cars all went to 0.0". The host restarted and sat in a menu, where it publishes simtime 0 and
22 zeroed cars (host log: `POS tick600: SUSPECT - all 22 records share dist=0.00`). The client applied
that faithfully and zeroed its own grid. plausible() cannot catch it: dist=0 is inside -10000..2e6 and
spd=0 is inside -1..150, so 22 zeroed cars all pass.

Added a gate before the window is opened: if every row has dist == 0, skip the apply entirely and log
`err=host not racing (all distances zero)`. Skipping also prevents the time-sync branch from rewinding
the client clock to 0. Tested: live payload passes, all-zero payload is skipped.

### Three self-inflicted failures in a row, three different causes
v35 crashed (RC writes, unproven), v36 froze (probe on the per-tick hook), v38's predecessor v37 was
fine but revealed the parse bug. The lesson is the same each time: the thing that says "it worked" must
be read before the next change. `host=nil` was printed on every single line and I moved on.

### Still unproven
Whether writing 0x24C plus the shown ring record actually clears a red flag on screen. Now that the
footer parses, the next run answers it - and it is the first run where the answer means anything.

---

## v39 - a reload must self-heal, and a checker miss (2026-10-26)

Client reloaded to v38 mid-race and then went silent: no PROBE, no `applied`. Its cars stayed at the
zeros the previous menu-state host had written, so it looked like the injection was broken when in fact
it was not running at all. F7 fixed it, but "press F7" was only in the log, which is not an
instruction the owner sees - it is a line the owner would have to go looking for.

NotifyOnNewObject cannot solve this: it fires only for a gamemode created AFTER the hook exists, and a
mid-race reload happens with the gamemode already there.

v39 adds a bounded self-heal in onTick: 3 probe attempts, 2 s apart, then it stops and says so.
The distinction from v36 (which froze) is deliberate and worth remembering:
  v36: every tick, up to 30 attempts, ~30 FindAllOf calls back to back  -> tens of seconds of stall
  v39: 3 attempts total, 2 s apart                                     -> 3 FindAllOf calls
Both are technically "probing from a hot hook". The thing that matters is the total number of expensive
calls on the game thread, not the hook it sits on.

### Checker miss worth fixing later
The edit `local function runProbes()` -> `function runProbes()` (needed so it assigns to the forward
declaration at line 88 rather than shadowing it) accidentally produced `local local function runProbes()`.
luasyntax.py accepted it: "the Lua grammar accepted the whole file with no ERROR nodes". luacheck_
structural.py caught the real problem underneath (use before definition) but flagged the shadowing,
not the doubled keyword. `local local` is not valid Lua and should be rejected outright. Any single
keyword doubled at the start of a line is worth an explicit rule.

---

## v40 - the simtime gate was rejecting a valid object (2026-10-26)

F7 DID work. Client log at 07:30:28: `VALIDATE simtime ok: delta=0.033`, `POSITIONS proven`, then
`applied` climbed to 3575 with maxDistDelta=0.00 and timeDelta=+0.028. Race control matched exactly:

    CLIENT RC flags host=1024 mine=1024 after=1024 | rc host=1 mine=1 | rf host=0 mine=0 | ring=0

That is the first run where the footer parsed (the v38 gmatch fix) and the first evidence that the
client's live mask, RcState and RfState all equal the host's. ring=0 means no record carried 0x100, so
there was nothing for it to clear - the host had no red flag in this session at all.

Then a reload landed at 07:31:32 and every probe after it failed:

    VALIDATE simtime: memory=302.409 reflection=211.84  -> delta=90.6  FAIL
    VALIDATE simtime: memory=302.409 reflection=213.61  -> delta=88.8  FAIL
    VALIDATE simtime: memory=302.409 reflection=218.61  -> delta=83.8  FAIL

Two facts in those three lines:
  1. memory=302.409 is IDENTICAL across 7 s while reflection advances 211.8 -> 218.6. The client's
     SimTime memory is not moving: the sim is not stepping. That is also a strong candidate for
     "cars still at 0.0" - if the sim never steps, the 30 Hz history ring the renderer reads is never
     appended, so nothing moves on screen no matter what we write into Dist/Lat.
  2. The gate then discarded ALL position reads and the client stopped applying. Gating on time
     agreement is self-defeating: TIME_SYNC writes SIMTIME to the host clock on purpose, so the two
     sides are meant to differ.

v40 demotes the DELTA comparison to advisory. Size and readability stay fatal - those mean a genuinely
wrong object. The overlay and class-pointer checks are the real identity proof and both still pass.
It also logs `frozen=<bool>`, i.e. whether memory SimTime moved since the previous probe, so the next
run says plainly whether the client sim is stepping.

### Lesson
A validation that fires on a condition our own feature creates (writing SimTime) converts a working
system into a silent stop. Validation should test identity (is this the right object), not behaviour we
deliberately change.

## Session ? locating the RaceSimManager for real (no Lua, no log parsing)

The read half no longer needs `UE4SS.log` or a hand-copied address. Chain, all of it checked live on
pid 45836:

- `module_base` comes from the PEB (`NtQueryInformationProcess` -> `PEB.ImageBaseAddress`).
  **Observed:** `0x140000000`, and `+ 0x7821450 = 0x147821450`, byte-identical to the address in
  `UE4SS.log` ? two independent sources agreeing.
- Object array layout is derived and scored, not assumed. **Observed:** `array_offset=0x10`,
  `item_size=24`, `chunk=0x180000`, `num=100405`, `max=2162688`, score 29/32.
  `0x180000 / 24 = 65536` items per chunk, matching UE's chunking.
- Two objects have class size `0xA132F0`. **Observed, same session:** `0xFE920000` has car 0
  distance 0.0 and sim clock 0.0 (class default object); `0x173530000` has distance 15136.5 and sim
  clock 302.0 (live instance). The live one is chosen by content; if neither looks live, discovery
  errors out instead of picking one.
- The live address **moves between sessions** (`0x1750E0000` earlier, `0x173530000` now), so caching
  is only ever a cache: a 4-byte read at `CAR_BASE + DIST` revalidates it and a full rescan runs when
  that fails.
- Agent inside the game and supervisor outside both discover independently and agree on
  `0x173530000`. `coop_rsm.txt` is then written with a fresh timestamp every two seconds so the
  probe's 5 s staleness guard stays satisfied.
- **Measured live read:** 22/22 cars plus race control, best 2.17 ms, mean 2.83 ms for the full
  10 564 336-byte object; a car was seen moving (`dist 5531.4 -> 5591.1`, `spd 58.1 -> 61.1`).

### Three failures worth remembering

1. `--discover` hung for minutes with *no output whatsoever*. Not a deadlock: the argument arm never
   advanced the parser index, so it rematched itself forever. The probe now refuses to continue if an
   arm fails to advance, and says which argument.
2. `CreateToolhelp32Snapshot` deadlocked inside the game ? it takes the target's loader lock, and the
   target was inside `LoadLibrary` mapping our own DLL. PEB reads cannot do this.
3. The agent published `rsm=0xCCCCCCCCCCCCCCCC` ? an uninitialised slot filled with `0xCC`. Nothing
   may be published without being read first.

### Build note

A workspace `cargo build --release` fails with LNK1104 while a session is live, because the game has
`coop_agent.dll` open. Build `-p coop-supervisor -p coop-mem -p coop-proto -p coop-inject`.
Tests run fine from `target/debug` (`cargo test --offline`, 26 passing).

## Session ? crash evidence before the hook is retried

The hook crashed the game once with no WER event and no dump. Nothing can be learned from that state
and retrying it produces the same silence, so `crash.rs` now makes a crash self-describing. Installed
in the live game (pid 45836): handler installed, `MiniDumpWriteMinidump` resolved at load time, game
responding afterwards.

**Observed, requested dump of the live game:** `MDMP` signature, 15 streams, 147 threads, full module
list, 553 KB, game still responding. Two dumps written before I trimmed the type, 90 MB each.

### The four dead ends, all measured rather than assumed

| what I passed | result |
|---|---|
| dump from the exception dispatcher on the faulting thread | `ERROR_NOACCESS` 998, zero-byte file |
| same call from a second thread | works |
| `EXCEPTION_POINTERS`, `ClientPointers` TRUE or FALSE, either thread | 998, zero-byte file |
| `MINIDUMP_USER_STREAM_INFORMATION` comment stream | faults inside dbgcore |
| `MINIDUMP_WITH_DATA_SEGS` | works, but 90 MB instead of 553 KB |

`dbghelp.dll` **forwards** `MiniDumpWriteDump` to `dbgcore.dll` ? that is why the faulting `rip` was in
a different module from the one `LoadLibraryW` returned, which is what made the first hour confusing.
No exception record and no user stream are passed now; the reason is in the file name and the code
and faulting address are in the trace.

### A mistake worth remembering

Three iterations of bisection ran against a **stale test binary**: `cargo build -p coop-agent` does not
rebuild the `deps/` test harness, so the child process kept executing the old code and I drew two
wrong conclusions in a row. Use `cargo test --no-run` before running a `deps/` exe by hand.

Related and already learned once: the game holds `coop_agent.dll` open, so a release build fails with
LNK1104 and a copy of the "new" DLL is silently the old one. Building with
`CARGO_TARGET_DIR=target/fresh` sidesteps the lock, and the DLL must be checked by timestamp and by
grepping it for a string from the new module before injecting.

### Freeze capture

`coop_dump_now` next to the exe makes the agent dump within a second and then removes the marker. The
marker check had to move **before** the supervisor connect: during a freeze there is usually no
supervisor listening, so a check placed after the connect never ran. A dump briefly suspends every
thread, so it is never automatic.

### Hook diagnostics

The stub now carries a fire counter (`inc qword [rip+disp32]` after the capture), so after a crash
`fires > 0` proves our code ran and `fires == 0` proves the game died before ever reaching the hook.
A test executes the real stub on a real executable page and asserts both the captured pointer and the
counter, rather than only checking that the bytes look plausible.

## Session ? hook retry, and the network link

### Hook: installed, fired, killed the game on the first sim call

The retry now has evidence, which is the whole point of the previous session. The patch was also made
safer first: the target `0x142298080` is 8-byte aligned and the three stolen pushes total exactly 5
bytes, so the patch is a **single aligned 8-byte atomic exchange** preserving the 3 bytes of
`sub rsp, 0x170` after it. No thread suspension, so `CreateToolhelp32Snapshot` is gone from the crash
path entirely.

**Observed on death:**

```
first-chance code=0xc0000005 at=0x1afd70000 rip=0x1afd70000 rsp=0xf7f3f8
```

Fault address is an allocated block, not the module; `rsp` is a collapsed stack. That reads as
unbounded recursion in the trampoline.

**A hole in my own tests.** The stub test used `ret` as the stolen bytes, so the jump back to
`target+5` was never executed and a looping trampoline could not have failed the test. There is now
`patched_function_runs_and_returns`: it patches a fake function with the real prologue plus a matching
epilogue, calls it through the patch, and requires a correct return. It passes. So the encoding is
correct and the cause is still unknown.

**The trap that made the retry meaningless.** The second inject reported

```
hook NOT installed: unexpected prologue E9 7B 7F 39 50 (expected 40 53 56 41 55)
```

`E9` is our own jump. The site was **still patched by the build whose install crashed** ? the game
survived that process's death, and the trampoline block is still mapped because the DLL is still
resident. So a surviving process is not a clean process, and re-injecting into it tests nothing.
Restart the game between attempts. The installer now says "already patched; restart the game" rather
than double-patching.

**Trace garbling, explained.** Lines were interleaved and truncated. Several agent builds stay
resident in one process and each opened the trace with `GENERIC_WRITE`, which starts writing at offset
0, so they overwrote each other. Now opened with `FILE_APPEND_DATA`.

Still unknown, and the next thing to determine: why the trampoline faults. Candidates worth checking
before another live attempt, in this order: whether `simulate` is called with a shadow stack or a
non-standard calling convention that our `mov [rip], rcx` capture disturbs; and whether the block
address is inside a region the game's own allocator reuses.

### Network link

Wrote `crates/coop-link`: the relay existed but no process ever sent or consumed a `Snapshot`, so
there was no actual transport. Verified locally with a fake source at 20 Hz: **2488-byte frames, 22
cars, 37-40 snapshots per 2 s, worst arrival gap 75-89 ms**, client shadow file listing 22 cars with
rising distances.

`--bind` now defaults to `0.0.0.0` (was loopback only, which made a LAN test impossible), and the
injected agent reads `coop_supervisor.txt` next to the exe instead of a compiled-in address, because
an injected DLL cannot see the injector's environment.

### Client install

`\\<client-pc>\e\SteamLibrary\...\Win64` is **read-only** for this machine: the write probe is
denied even though the NTFS ACL grants `Authenticated Users: Modify`, so the share itself is
read-only. Nothing was written there. Binaries are staged in `local/f1-manager-2024/client-package/`
with `INSTALL.md`. Two-machine addresses: this machine `192.168.1.101`, client `192.168.1.100`.

## Session ? the save is opaque, and it travels over the wire

### What the save actually is

`autosave.sav` starts `GVAS`, but it is not the plain Unreal property tree the earlier notes implied:

- save version 3, package version 522, engine 1008.0.5, and a custom format string `++volta24+game+1.11.0`
  followed by 63 four-byte entries;
- from 64 KB onward the entropy is **7.99-8.00 bits/byte** across the whole file, i.e. compressed or
  encrypted, all the way through;
- **zero** occurrences of `CREATE TABLE`, `sqlite_master`, `SQLite format 3`, `INSERT INTO`: there is no
  readable SQLite database inside. The earlier "GVAS then a SQL DB" model is wrong for this build, or
  the DB is compressed in a way that hides it completely;
- no usable gzip stream: the one `1f 8b` hit at offset 36740 has flags `0xdf`, which sets reserved
  bits, so it is not gzip at all;
- `Oodle`, `zstd`, `LZ4` and `libdeflate` strings are in the exe and **no** separate compression DLL
  exists beside it, so frontier's compressor is linked statically.

Conclusion: **the save cannot be authored or edited by us, and nobody should try.** It is the game's own
output and the only correct handling is byte-exact carriage. That is not a limitation for co-op ? it
removes a whole class of corruption.

### Transfer, implemented and verified

New protocol messages `SessionOffer` / `SessionChunk` / `SessionReady` (tags 7/8/9), carried by the
existing relay, which needed no changes: it already re-frames and forwards any valid message.

- host: `coop-link --role host --send-save <file>`, 32 KB chunks, re-offered every 10 s
- client: reassembles in memory, verifies FNV-1a over the whole payload, writes only on a match

**Verified end to end:** a 2 936 598-byte save went host -> relay -> client and the received file is
**byte-identical**, SHA-256 `07C22A9F...39BE` on both sides. Transfer took 42-318 ms for 2.8 MB.

Two design bugs found by testing rather than reasoning:

1. the host sent the session once, before the client had connected, so a late joiner never got it. The
   relay only forwards to peers connected at that moment, so the host now re-offers every 10 s and the
   receiver ignores a duplicate.
2. the supervisor logged `unknown message tag 7`: its binary was **stale**, still locked from a previous
   run. Same trap as before - a build that does not replace the binary is invisible until a new message
   is rejected. Rebuilt and confirmed by grepping the exe for a session string.

### GUObjectArray is not a fixed address

Discovery broke after the save load. Two separate causes, both now fixed:

1. `module_base + 0x7821450` lands inside **`.reloc`** in the file, i.e. the bytes there in the image are
   relocation data. It was never a genuine static; it merely held valid values during one session and
   stopped after a load. It is now a hint tried first, with a shape-based scan of the image's writable
   regions as the fallback.
2. the score threshold (`>= 24`) was fragile: the loaded save had `num_elements` 94131 against 100405
   before, and the best probe score dropped to 15/32. A count that changes with game state cannot gate
   validity. The score now only orders candidates; the real test is whether the RaceSimManager class
   size can be found through the array.

That also documents a real state difference worth keeping: **the object count is a fingerprint of the
loaded session** (94131 after loading vs 100405 before).

### Where the game was during this session

The UE4SS log caught it: `SESSION changed -> BP_RaceManagementGameMode_C` on
`RaceManagementLevel` at 10:46:09, right after `autosave.sav` was rewritten (4 571 682 -> 2 239 525
bytes). So the game is in **race management**, not a live session, which is why `coop-watch` sees only a
zeroed class-default object. A live session is still needed to record what a load does to the sim.

### Watcher

`coop-watch --pid <pid>` samples four fields (sim clock, playback, flags, race control, car 0
distance) at low cost and reports any discontinuity - object replaced, clock rewound, distance reset,
state change - and prints the save files with their age at that moment. Full discovery is expensive, so
it backs off 1-7 s while nothing is live instead of rescanning at sample rate.

## Session ? real save send to the host side, and the merge question

### Verified: an 11.58 MB save crosses the link intact

`autosave.sav` (11 575 739 bytes, written 12:59:49 while the game was live) went host -> relay ->
client in **84 ms** and arrived **byte-identical**, SHA-256 `31173B1B...2A92` on both sides. Earlier the
same path moved 2.94 MB in 42 ms. No manual file handling at any step.

The test also caught a real bug: `--send-save` never ran, because host startup aborted when the sim was
not live. Sending a session must not depend on a live race - the transfer exists precisely so the client
can be brought to the host's state while the host sits in menus. The host now attempts discovery,
reports "no live sim yet ... still able to send the session save", and keeps retrying for snapshots
without blocking the transfer.

### The merge requirement (planning only)

Written up in `SAVE-MERGE-PLAN.md`. The requirement: clients each run their own team, every day advance
is a barrier, and each client's delta for its own team replaces what the AI did. If the client is the
host's team, nothing is applied.

Two things in that plan are load-bearing and worth repeating here:

1. **Races must be won on the client's machine**, which means the merge may have to carry *results*, not
   just inputs. "Inputs only" (setups, strategy, transfers) is far simpler but assumes the host's
   simulation reproduces the client's outcome from the same inputs. That is an assumption to test, and it
   is the first thing to resolve.
2. **The save cannot be the merge vehicle.** It is opaque and compressed; the merge has to happen against
   the **live DB in memory**, which is the direction the owner asked for. So the next investigation is
   finding the database object in the process - the existing content-based object discovery and the
   external reader already make that cheap and safe to explore.

Ownership is per team, not per client: `Msg::Hello { cars_owned }` is already the wrong granularity and
should carry a team id. Conflicts (two clients claiming one team) must be refused, not resolved
silently, and a merge must be idempotent per (day, team) because the transfer re-offers by design.

### Note on the two objects

After the load, two objects match the RaceSimManager class size and both read zeros (car 0 distance and
sim clock), so `pick_live` still refuses. Either both are defaults in this state, or the live one reads
its values lazily in race management. Unresolved; it does not block save sending, and the day-advance
barrier will need a different signal than "sim clock is non-zero" anyway.

## Session ? client join path and a hotkey

### What a join does today, honestly

1. the client runs `coop-keys.exe` (via `coop-hotkey.cmd`) and presses **Ctrl+Alt+J**
2. `coop-link --role client` connects to the host supervisor, using `coop_supervisor.txt`
3. the host re-offers its session every 10 s; the client verifies the hash and writes the save into its
   own `SaveGames`
4. **the client then loads it from the game's own Load Game menu.** Nothing automates that step yet

Step 4 is the gap. Everything before it is verified.

### Verified with the hotkey

Pressing Ctrl+Alt+J produced, in `coop_link.log`:

```
[client] connected to supervisor at 127.0.0.1:45990, waiting for the host
[client] session offer: autosave.sav, 11.04 MB, hash 0x4BA5AFFF
[client] session saved to ...\SaveGames\autosave.sav (11575739 bytes, hash verified)
```

Also verified: the client reaches the supervisor over the **LAN interface** (`192.168.1.101:45990`,
seen in the supervisor log as `192.168.1.101:62421 says it is Client`), not just loopback. A firewall
rule for `coop-supervisor` already existed.

### Two things that had to change for the hotkey to be usable

- `coop-link` printed only to stdout. Launched by the hotkey it owns a separate console window, so a
  failed join left no evidence at all. It now appends to `coop_link.log` next to the executable.
- a hotkey launch passes no arguments, and `--supervisor` was mandatory, so a keypress produced a usage
  message. The address now falls back to `coop_supervisor.txt` and then to `127.0.0.1:45990`, and the
  role defaults to `client`.

`--pid` is now optional everywhere: the game runs as a launcher plus the real process with the same
name, so it picks the largest match. That removes the only value a player had to look up.

### Why a global hotkey rather than a Lua keybind

The client has no UE4SS, and the earlier Lua work showed in-game key handling on the sim thread is the
part that freezes. A `RegisterHotKey` window needs neither: it receives the keystroke from Windows no
matter which window has focus, and does its work in its own process. The game is untouched.

## The debugger attempt that killed the session ? what it taught, and what it cost

Lost: the live MarinaBay race. Not lost: any save data (verified: `save164.sav` and the `save29.sav`
hardlink both still 2 245 264 bytes, mtime unchanged).

### Sequence, from the spy's own output

```
attaching to pid 35800, breaking at 0x144AAD1F0
  process handle 0xD8
  original byte at target: 48
  breakpoint armed            <- INT3 written into game code
                              <- no halt ever printed: the breakpoint NEVER FIRED
                              <- process killed externally: no restore, no resume
```

Result: `0xCC` left in game memory with a thread blocked, then
`ReadProcessMemory ... Win32 299` = `ERROR_PARTIAL_COPY`, then the process died.

### Three separate mistakes

1. **Detach without resume.** The cleanup lived at the end of `main()` and was skipped by an external
   kill. It has to be in a drop guard that restores the byte and resumes threads, on every exit path.
2. **The address was inferred, not observed.** `0x144AAD1F0` is genuinely the first call inside the
   dispatcher ? I decoded it correctly ? but I concluded it *carried the payload*. It does not fire on a
   load. Correct place to break is the dispatcher entry `0x1528A8AC0`, which every save event must pass
   through, not a call reached by reading control flow.
3. **Verified on the wrong process.** A hang must be discovered on something disposable. Burning a live
   race session to learn "that address is wrong" is not a fair trade, and the client instance was
   available the whole time.

### What stands

The dispatcher is still good evidence: `RVA 0x128A8AC0` is the only code in the image referencing the
`LoadGame` literal, it references exactly `LoadGame`/`DeleteGame`/`NewSave`, its file bytes match live
bytes (so the RVA is static), and it is MSVC-compiled with a normal prologue. What was wrong was
reaching *into* it rather than breaking *on* it.

## Writing into the live DB: the mechanism, and why the swap did not land

### The mechanism is real and now demonstrated

The game's SQL is not in the exe. It lives in **private PAGE_READWRITE buffers**, decrypted at
runtime by the protection layer - which is why a full-image scan for references to the literals found
**zero** hits: no code holds a pointer to a string, the string is built into a private buffer and
handed to SQLite from there. That is also why the earlier `pre-sqlite3_prepare` lead was a dead end
(that string lives in `AppXDeploymentClient.dll`, a Windows component - a false positive).

Those buffers are **stable**, not per-query scratch: hashes identical across 12 s of polling.

A statement buffer was edited in place and it stuck:

```
current: (82 chars) UPDATE Staff_Contracts SET EndSeason = ?3 WHERE StaffID = ?1 AND ContractType = ?2
written:            UPDATE Staff_Contracts SET TeamID = 9 WHERE StaffID = 83   (56 chars)
verify:             UPDATE Staff_Contracts SET TeamID = 9 WHERE StaffID = 83
```

Buffer layout, measured: header at `text-0x28`, with a u32 length field at `text-0x10`.
Original length 82, capacity 0x2A0 (672), so the replacement needed no resize. Restored afterwards and
verified identical.

### Why nothing changed in the database

The statement is only executed when the game runs that code path. Writing a contract's end season is
an in-game action; sitting in a menu never triggers it, and the change counters stayed put across 11
tracked databases. **Editing the text is not the same as executing it.**

So the open problem is not "how to write bytes into the DB" - that is solved. It is **how to cause
execution**. The options, in order of attractiveness:

1. **Find the prepare/execute call site and call it** with our own SQL and the game's own connection.
   This is the same shape as the load-handler problem, and carries the same risk.
2. **Bind-swap instead of text-swap**: edit bound parameter *values* rather than the statement. The
   contract writer is `REPLACE INTO Staff_Contracts (...) VALUES (?1..?12)`, so the values live in a
   bind array at execution time - closer to a legitimate write, and it needs execution anyway.
3. **Observe execution first**: capture the arguments at the prepare call when the game itself next
   writes a contract, then replay them. Same technique as capturing a real RDX for the load call.

### What the schema told us (useful regardless)

- contracts are keyed by `ContractType` resolved through `Staff_Enum_ContractType` (`"Current"`,
  `"Future"`), never by a literal
- the game writes contracts with `REPLACE INTO Staff_Contracts` over 12 bound columns
- career history is archived with `REPLACE INTO Staff_CareerHistory`, ending the old row at
  `(SELECT Day FROM Player_State)` - so `Player_State.Day` is the clock a merge must key on
- there is a dedicated `UPDATE Staff_Contracts SET EndSeason/TeamID WHERE StaffID AND ContractType`

### The stale-dump lesson

Reading contracts out of the 4.36 MB DB gave Bottas team 9 / Hulkenberg 10. The live game had
Bottas 32 / Hulkenberg 9. That DB was a stale snapshot (`Day` 44562, season 2022) and its
`Save_RaceSimManager` / `Save_RaceSimCars` / `Save_Weekend` tables were empty. **Never present a
value read from a dump without checking it against something the user can see.**

## Live database access: working

The career database is **in memory**. There is no file, no header in memory and no pager image that
corresponds to it - which is why days were spent looking for one. The only handle is the live
`sqlite3*`, and the only correct way to touch it is to call the game's own `sqlite3_exec`.

### What works, verified against the running game

```
attached: sqlite3_exec 0x14156D300, database 0xF5754680
probe ok: Player.TeamID = [["32", ""]]
```

- `sqlite3_exec` at **RVA 0x156D300**, found by AOB `41 B9 EF FF 01 00 4C 8D 05` (the
  `SQLITE_MISUSE_BKPT` line) rewound `0x6C`, confirmed by prologue `4C 89 4C 24 20` preceded by `0xCC`
  padding in `EXECUTE_READ`.
- the `sqlite3*` found by signature `01 ?? 00 00 00 00 FF 00 00 00 00 ?? ?? 76`
  (`eOpenState = SQLITE_STATE_OPEN`), accepted only when `db+0x20 == db+0x2A0`,
  `*(i32*)(db+0x28) == 2` and the `sqlite3_vfs*` lies inside the image. Decisive test: filename is
  `:memory:`, read via `db+0x2A8 -> BTree -> +8 shared -> pager -> +0xD8 -> zName`.
- SQL runs through a stub and row callback **we wrote**, proven by a local test that executes the same
  bytes against `winsqlite3.dll` and asserts the output `7<US>bravo<US><RS>`.

### The contract swap, done

```
current:  8 | 9  | 1 | 12502000 | 2026      83 | 32 | 1 | 5500000 | 2026
after:    8 | 32 | 1 | 12502000 | 2026      83 | 9  | 1 | 5500000 | 2026
```

`coop-sql --swap-contracts 8 83`, idempotent, wrapped in a transaction.

### Three bugs that each cost a session

1. **`CreateRemoteThread` started at a Rust function.** I passed `Some(thread_start)` - a function in
   *our* process - so the thread began at an address with no meaning in the game and silently did
   nothing. It must start at the **stub address inside the target**, with the parameter block in `rcx`.
   This was the single line that made every attempt time out.
2. **Stub and parameter block at the same address.** The thread then executed its own parameter bytes
   as code. They must be separate non-overlapping regions; there is now an assertion for it.
3. **Correlated subqueries cannot swap two rows.** `SET TeamID = (SELECT TeamID ... other row)` reads
   the already-updated row on the second evaluation, collapsing both to one team. `CASE WHEN TeamID =
   <a> THEN <b> ELSE <a> END` is evaluated per row against its current value and is correct.

### Why this is the foundation, not a detour

The goal is a two-player career, not co-op in the OTS sense. That needs the shared career state to be
writable, and it now is:

- team ownership is a row: `Staff_Contracts(StaffID, TeamID, ContractType)`
- a client's team can be assigned and reassigned per season by SQL, which is what the game itself does
- the live race state (`Save_RaceSimManager`, `Save_RaceSimCars`, `Save_TimingManager_Laps`) is in the
  **same** database, so day-advance sync and team merge are the same mechanism
- no save file is involved at any point, so "no manual file sends" holds end to end

## Isolating what actually kills the game (the freeze vs the write)

I asserted twice that a specific thing killed the game without isolating it. Each claim was wrong. The
tested results, one variable at a time against the live process:

| operation | result |
|---|---|
| `SELECT` | ok, 5/5 sequential runs |
| `BEGIN EXCLUSIVE` then `COMMIT` | **ok** - lock taken and released, game responsive, queries still work after |
| `DELETE FROM Board_TeamRating` | ok, 33 rows removed |
| `UPDATE` (the contract swap) | ok, verified both directions twice |
| `CREATE TABLE zz(a)` | ok |
| `INSERT INTO zz VALUES('x')` | **ok** |
| `INSERT INTO Board_TeamRating VALUES(...)` with an unknown SeasonID | SQLite error 19, **game fine** |

So: **the lock is not the problem, writes are not the problem, and INSERT is not the problem.** I had
wrongly concluded "`BEGIN EXCLUSIVE` killed it" after one failed run, and wrongly concluded "INSERT
killed it" after another. Both conclusions came from one observation each, with no control.

### What the failures actually were

"could not read the worker state" is not the game faulting. It is **our** worker having exited, and the
most likely cause is an unhandled error path: `sqlite3_exec` returns non-zero, the stub stores the rc,
and the host gives up reading state rather than reading rc. A constraint failure therefore looks like a
dead worker, which sent me chasing engine bugs for several rounds.

### Lesson

One observation is not a cause. The controlled sequence above took four commands and overturned two
confident (and wrong) diagnoses that had cost several game sessions.

## Session ? SCRIPTED CONTINUE WORKS (2026-10-26 17:30-18:20)

The blocker that had burned several sessions ? "can we make the game press its own Continue button?" ? is
solved. Not with a debugger, not with memory writes, not with input injection.

### What the button actually is

`Engine.sendEvent(name, ...args)` is the whole mechanism, and it is one line in
`js/common/core/Engine.js`:

```js
function sendEvent(eventName, ...args) { engine.trigger(eventName, ...args); }
```

The front-end buttons are thin wrappers over it:

| button | file | call |
|---|---|---|
| Continue (main menu) | `frontend/components/MainMenu.js` | `sendEvent('ContinueGame')` |
| Load save | `loadSave/LoadSaveScreen.js:128` | `sendEvent('LoadGame', slotName)` |
| Advance | `teamManagement/TimeProgressionPanel.js:152` | `sendEvent('AdvanceToNextEvent')` |
| Next month | `teamManagement/CalendarContents.js:278` | `sendEvent('NextMonthSelected')` |

So a scripted Continue is not "find the function the button calls" ? it is "call the same named event
the button calls". No address, nothing that moves between sessions, nothing to reverse.

### How it is delivered

A pak that overrides `Content/UIGameface/*.html` and injects `js/coop/bridge.js`. This is the same
mechanism existing F1 Manager mods use: `f1m2021_leaderboard.pak` ships exactly two files, one of them
`Content/UIGameface/js/project/modules/raceWeekend/raceStandings/RaceStandingsRow.js`.

- `pak/` builds to `CoopBridge_P.pak` (~8 KB) with `repak pack . <out> -m ../../../ --version V11`
- mount point `../../../` and V11 match the game's own `F1M2021_TEST_PR_S_UI_P.pak`
- install: drop `CoopBridge_P.pak` into `F1Manager24\Content\Paks\`
- the bridge polls `http://127.0.0.1:45999/hello` and runs whatever command comes back
- `tools/coop_server.py` is the operator side: `curl ".../cmd?text=Continue"` broadcasts to every
  connected client exactly once

### Verified, in the live game

| test | evidence |
|---|---|
| pak override loads | `/pak-ok file=Frontend.html` on startup |
| bridge sees `engine.trigger` | `v=4`/`v=5` heartbeats, `stage=engine-ready` |
| main menu detected | `stage=mainmenu` via `.MainMenu_root` in the DOM |
| **Continue works** | command `Continue` ? `autosave.sav` modified at that second, and UE4SS logged `SESSION changed -> BP_RaceManagementGameMode_C` one second later |
| bridge survives the transition | next heartbeat is `page=TeamManagement` |
| in-game command path works | `Advance` and `Stage probe` both executed (`sent` incremented) |

That is the automation primitive the lobby needed. Everyone's client can be told to load at once, and
the same mechanism drives Advance and the race-weekend step, because they are all `engine.trigger`.

### Three things that did NOT work, and why

1. **Synchronous XHR.** `xhr.open(..., false)` is blocked in Cohtml. The command simply never ran and
   nothing was logged. All requests must be async.
2. **Reporting after firing.** `ContinueGame` and `LoadGame` destroy the document that fired them, and
   Cohtml drops in-flight requests during that teardown. An ack sent after the trigger is always lost.
   Reporting *before* the trigger works, but for those two events it cannot prove the event ran ? only
   the page change does.
3. **`localStorage` as a cross-document handoff.** Untested to completion, so treat it as unverified. The
   page-change heartbeat made it unnecessary.

### The design the user settled on

Intercept at the single choke point, not per button:

- **not in a lobby** ? Continue, Advance and race-weekend advance behave exactly as now
- **in a lobby** ? those same buttons request a ready check, and the real event fires only once every
  client reports ready

Because every one of them funnels through `engine.trigger`, one gate covers all of them. The event
names that belong in that gate, from an exhaustive scan of the UI (103 `sendEvent` names total):

| event | button |
|---|---|
| `ContinueGame` | main-menu Continue |
| `LoadGame` | load a save |
| `AdvanceToNextEvent` | the Advance button, and the sponsor-bonus confirm |
| `NextMonthSelected` | calendar month arrows |
| `CASLoadSave` | load inside create-a-season |

`AdvanceToNextEvent` is also the race-weekend step: the calendar list is rendered from
`Calendar.UpcomingDays`, so a free-practice session and a race weekend are the same button with a
different label (`AdvanceType.ToRaceWeekend = 42`). There is no separate "enter race weekend" event to
find ? which is why it belongs in the same gate.

### Artefacts

- `pak/f1manager24/Content/UIGameface/js/coop/bridge.js` ? the bridge (v5)
- `pak/f1manager24/Content/UIGameface/*.html` ? Frontend, TeamManagement, RaceWeekend, MinimapContent
- `pak/CoopBridge_P.pak` ? the built pak, 8 KB
- `tools/coop_server.py` ? broadcast command server on :45999
- `tools/repak.exe` ? packer (copied locally; the Program Files copy needs elevation)
- `tools/fastxref.py`, `tools/disasm.py`, `tools/strs.py` ? static PE analysis used to rule the native
  route out
- `mods/CoopFindMod/` ? read-only UE4SS discovery attempt, not needed once the pak route worked

### Why the native route was abandoned

Before the pak route, `ContinueGame` was traced statically to be sure there was no simpler call:

- the string lives at `.rdata` VA `0x145F02988`, and it has exactly **one** code xref in the whole
  553 MB image, at `0x142062939`
- that site is inside a registration loop: it builds a 24-byte callback, stores a name pointer at
  `callable+0x10`, and calls a virtual at `[rax+0x1B0]` ? a Gameface event-registry insert
- the registered callable is a heap `std::function`, not a module function pointer
- the handler thunk lands at `0x1529D95A0`, which turns out to be generic Gameface dispatch, not the
  Continue logic

So there is no stable "call this function to Continue". The dispatch is by string comparison against a
registry that is rebuilt every launch, which is precisely the thing a named event avoids. Packet-capture
and hardware-breakpoint attempts along this path also cost three game crashes for no result.

### Lesson

Two sessions went into hunting a native entry point for something the UI exposes by name. The game
already loads front-end JS from paks, the buttons are one `engine.trigger` away, and the whole bridge is
under 250 lines. When a game's UI is a web view, its UI *is* the scripting API ? check that before
disassembling anything.

## Session ? TWO MACHINES IN THE SAME RACE WEEKEND (2026-10-26 18:37-18:46)

First end-to-end result: two separate PCs, each running its own copy of the game, both told to load
the same save and both landing in the **same race weekend** on command. No manual clicks, no file
copying by hand, no debugger.

### What ran

| time | host (192.168.1.101) | other PC (<client-pc>, 192.168.1.100) |
|---|---|---|
| 18:37:37 | `Load save2` queued | ? |
| 18:37:49 | `page=RaceWeekend` | (not running yet) |
| 18:42:19 | ? | bridge v6 probed, found host over LAN, `stage=engine-ready` |
| 18:42:51 | ? | `page=Frontend stage=mainmenu` |
| 18:44:23 | ? | `save2.sav` pushed and **hash verified** into its save folder |
| 18:44:44 | `Load save2` delivered to both | `Load save2` delivered |
| 18:45:02 | (already in) | `page=RaceWeekend stage=ingame`, 2717 MB, responding |
| 18:45:38 | 4374 MB, responding | 2717 MB, responding |

### The three pieces that made it work

1. **The bridge probes for the host instead of assuming loopback.** v5 hardcoded `127.0.0.1`, so on the
   other machine it polled a port with nothing on it and never registered. v6 tries
   `127.0.0.1` then `192.168.1.101` and keeps the first that answers `/hello`. One pak, both machines.
2. **The server binds `0.0.0.0`.** Loopback-only was the same bug from the other side.
3. **The save is pushed and verified by hash before anything loads.** `save2.sav` 6,055,752 bytes,
   SHA-256 `FA575E3F61...` identical on both, `GVAS` magic confirmed. A truncated `.sav` in that folder
   would be offered by the game's own load menu as a corrupt career, so the check is load-bearing.

### The save slot question, answered

The user said `save2.sav` or `save0.sav` from the host side. `save2.sav` is the right pick for this
test: it opens directly into a race weekend rather than the team-management screen, which is exactly
the state the session needs to work on next. `save0.sav` is 11,575,739 bytes ? the same file the
client received earlier in the session, hash-verified, so it is the big shared career.

I wasted a detour looking for these saves inside the ThatFinnishGuy save viewer's extracted CSVs.
The viewer is for *reading* saves; the actual files live in
`%LOCALAPPDATA%\F1Manager24\Saved\SaveGames`. Wrong place to look, and the user said so.

### Remote control of the second machine (new)

`tools/remote.py` runs code on that PC over the SMB share, because it has no SSH, no WinRM and no open
coop port ? but `\\<client-pc>\e` is **writable**.

```
req\<id>.cmd  -> agent executes in a child PowerShell
out\<id>.out  -> combined stdout/stderr
out\<id>.json -> exit code, host, user, timing
out\<id>.done -> written LAST, so a reader never sees a half-written result
```

Verified: `ping`, `ps`, `saves`, `game`, `ue4ss`, `run`, `runfile`, `killgame`, `launch`,
`install-pak`, `push`, `pull`, `snapshot`.

Bugs found and fixed while building it:

- `.done` was deleted before the reader polled, so a successful run looked like a timeout
- `agent.beat` did not exist, so liveness was inferred from `agent.log`, which only moves when work
  happens; a single idle agent therefore looked dead. Now a heartbeat file is rewritten every 5 s.
- `-Root $PSScriptRoot` broke when the agent was started from `run/` instead of the root, silently
  relocating every result file. Root is now resolved from the script's own location.
- Two agents could run at once and each report a different root. Now a named mutex enforces one per root.
- PowerShell 5.1 writes UTF-8 **with a BOM**; `json.load` then fails and every run reports `exit -1`.
  Reading with `utf-8-sig` fixed it.

One manual step is unavoidable: **the share cannot start a process.** `start-agent.cmd` has to be run
once on that machine, or put in Startup. Everything after that is unattended.

### Still not done

- Session results are not merged. Both clients are in the same weekend; whichever one finishes a
  session owns that result only locally. That is a data-ownership problem, not a UI problem.
- The ready-check gate is written up in `LOBBY-GATE.md` but the wrapper is not implemented.
- The other PC has the bridge pak but no UE4SS, so nothing there reads car state.

## The black screen: cause, and the guard that stops it (2026-10-26 18:46-18:54)

### What happened

The host went to a black screen with the process still alive and responsive at the OS level. It had
received `Load save2` **while already inside a career** (RaceWeekend). The client, which loaded once
from the main menu, was fine.

Evidence, not inference:

- the host's last bridge heartbeat was `page=RaceWeekend stage=ingame` at the moment the load was
  delivered; the bridge went silent immediately after and never came back
- the Lua thread kept ticking (`STALLED simtime=0.00 ... 15172 ticks`), so the process was not hung
  and had not crashed ? only the front-end document was gone
- the same command issued from `Frontend.html` works every time, on both machines

### Why

`LoadGame` is only ever issued by `LoadSaveScreen.onConfirmChoice`, and that component lives in the
**front-end app**. In-game the same screen exists but routes through a confirmation dialog
(`confirmationState: LoadGame -> popupOpen`) before finally calling `sendEvent('LoadGame', slot)`.
Firing the event directly from an in-game document skips that route and destroys the view that fired
it with nothing to rebuild it.

So the rule is: **`LoadGame` and `ContinueGame` are front-end-only events.**

### The guard, in two places

Both are needed. The bridge refuses locally, and the server refuses to deliver in the first place,
because a stale pak on one machine is exactly how the black screen happened ? the client was still
running the older build when it was told to load from inside its race weekend.

**Server** (`tools/coop_server.py`, `take_for`): a queued `load`/`continue` is only handed to a client
whose last heartbeat said `page=Frontend`. Everyone else is recorded as `skip` with the reason, so the
decision is visible in `/status` instead of being a silent no-op.

**Bridge** (`bridge.js`, `allowedFor`): same rule, plus it reports `ERR ... refused on RaceWeekend`
rather than trying.

Verified on the live pair:

```
18:53:03 deliver  192.168.1.101  Load save2        (that client was at Frontend -> entered RaceWeekend)
18:53:03 skip     127.0.0.1      Load save2  page=RaceWeekend  why=load/continue need the front-end
18:53:12 skip     probe          Load save2  page=RaceWeekend  why=...
```

Host stayed at 5230 MB and responding; client went to RaceWeekend at 2797 MB and responding. No black
screen.

### The operational consequence for the lobby

Every client must be **at the main menu** before a shared load is issued. That is not a workaround, it
is the game's own constraint: the career load path starts at the front-end. A lobby flow that follows
it is: everyone returns to the front-end, the host pushes the save, everyone loads together, then the
session advances with `AdvanceToNextEvent` from inside the career.

Returning to the front-end is `QuitGame` (Options -> Exit, `OptionsMenu.onQuitGame`, and
`RaceWeekendEvents.QuitGame` which is also `sendEvent('QuitGame')`). Wired into the bridge as the
`menu` command, but **not yet tested** ? do not claim it works until it is.

## The two real crash causes, found by testing the hook instead of guessing (2026-10-27)

For eight live attempts the client either did not sync, froze, or died with
`EXCEPTION_ACCESS_VIOLATION reading address 0xF3C`. Two distinct defects were found today, both by
making the code testable rather than by another live run.

### 1. The hook block was allocated out of `rel32` range (this is the "unexplained trampoline fault")

Notes from yesterday recorded, unresolved:

```
first-chance code=0xc0000005 at=0x1afd70000 rip=0x1afd70000 rsp=0xf7f3f8
Fault address is an allocated block, not the module; rsp is a collapsed stack.
That reads as unbounded recursion in the trampoline.
```

That diagnosis was a red herring. Every jump this hook emits is `E9 rel32` - a **signed 32-bit**
displacement. `VirtualAlloc(NULL, ...)` in a 64-bit process returns whatever the allocator prefers,
and on this machine that is roughly 2 TB above the module. Measured directly:

```
target                     0x142298080     (module 0x140000000 + 0x2298080)
VirtualAlloc(NULL) results 0x02539E750000  delta +2.1e12   fits_i32 = False
                           0x01EC82590000  delta +2.1e12   fits_i32 = False
                           0x01EC82600000  delta +2.1e12   fits_i32 = False
```

So the patch jumped to a truncated address. Nothing was wrong with the trampoline, the shadow stack or
the calling convention - the target address was simply not encodable in the bytes that were written.
`rsp` being 0xf7f3f8 (far below any real stack) and the fault address being "an allocated block" are
both consistent with executing garbage in unmapped memory.

Fixed by `alloc_near()`: walk outward from the target in allocation-granularity strides asking for
specific addresses, and verify each grant is inside Â±2 GB before using it. Verified on the real
allocator in a unit test:

```
alloc_near -> 0x7FF6B5520000   naive VirtualAlloc(NULL) -> 0x02539E750000
target     0x7FF725528080      naive_in_range = False
```

And live, in the running game, first time ever:

```
[agent] sim entry: base=0x140000000 target=0x142298080 plausible=true
        bytes=[40 53 56 41 55 48 81 EC 70 01 00 00 45 33 ED 48]
[agent] hook installed at 0x142298080; mailbox callback registered
[agent] RaceSimManager = 0x106D40000
[agent] heartbeat started
```

The game stayed responsive (`Responding = True`) after the patch. **This is the first hook install in
this project that did not fault.** `layout_reaches()` now refuses a layout whose displacements would
truncate, so this cannot silently regress.

### 2. The `jz` in the stub skipped the `pop rax`, leaking 8 bytes of stack on every call

Caught by the unit test, not by the game. The stub is:

```
0x0E  push rax
0x0F  mov  rax, [rip+callback]
0x16  test rax, rax
0x19  jz   <-- targeted 0x6F, which is *past* the pop rax at 0x6E
...
0x6E  pop rax
0x6F  jmp  trampoline
```

With no callback registered the branch skipped `pop rax`, so the game's function returned with `rsp`
8 bytes low and returned to a garbage address - a fault far away from the hook. It now targets 0x6E,
and the test asserts the byte at the target really is `0x58` (`pop rax`) so a future refactor cannot
quietly move it again.

### 3. Data lived inside the stub's own code

`SLOT_OFF = 0x50` while the stub had grown to 0x74 bytes: the fire counter was being incremented
inside the instruction stream. Layout is now code 0x00..0x74, trampoline 0x80..0x8A, data 0x90+
(`SLOT_OFF`, `FIRES_OFF`, `CALLBACK_OFF`), with an assertion in `build_code` that refuses an overlap.

### The architecture change this enabled

Writes no longer happen from `coop-link`'s process at all. The hook's stub calls a Rust callback on the
**game thread**, inside the sim call, and the callback drains a mailbox of `(offset, value)` pairs that
`coop-link` filled over loopback (`127.0.0.1:45991`). The mailbox uses two buffers and one `busy` flag:
the worker only writes the buffer the callback is not reading, so no lock is taken and the game thread
never waits. This is the one structural advantage the old Lua approach had, and it is now the default.

The client's own readback verification stays - reads from an external thread are safe - so "verified N
of 22" still means what it says.

### Tests, all passing (29 in `coop-agent`)

```
hook::stub_encoding_is_what_we_expect            byte-for-byte stub, every displacement checked
hook::stub_calls_the_registered_callback_with_this   the callback runs and receives `this`
hook::stub_without_a_callback_never_dereferences_this inert hook, bogus rcx, must not fault
hook::stub_preserves_the_registers_it_borrows   save/restore, through a real prologue
hook::patched_function_runs_and_returns         patch + call + correct return
hook::a_distant_block_is_rejected_before_it_is_patched   the rel32 bug, as a test
hook::rel32_range_boundaries_are_exact          i32::MAX reachable, i32::MAX+1 not
hook::alloc_near_returns_a_block_within_rel32_range      against the real Windows allocator
hookmail::publish_then_drain_writes_only_the_replicated_fields
hookmail::a_second_publish_is_dropped_while_the_first_is_pending
hookmail::node_and_seg_are_never_written        the 0xF3C field set
tests::a_tcp_frame_reaches_a_fake_sim_object_through_the_mailbox   full path, no game
```

### What is still unknown

- **Whether the hook actually fires.** Install is proven; firing is not. The log now prints the block
  address and `fires` lives at `block + 0x58`, plus a one-minute "never fired" line, so the next run
  answers it directly instead of by inference.
- **Whether the `0xF3C` crash is gone.** The field set excludes `node`/`seg`; game-thread writes remove
  the ordering hazard. Neither has yet been confirmed under a live sprint with a remote client.
- The client machine's SMB share was unreachable for the whole session, so no two-machine test was run.

### Live verification of the install (2026-10-27, host game, no client)

Injected the rebuilt agent into the running host game (pid 20132) and read the hook block back from
outside with `tools/watch_fires.py`, which finds the block by signature (the stub's RIP-relative
displacements are constants, so the bytes identify it regardless of where it landed):

```
block 0xEB960000 (layout slot=0x90 fires=0x98)
callback 0x7FF9E6F13E30
fires at start: 0
fires 0 (+0) x8 over 8 s
RESULT: hook never fired
```

Then, to decide whether that meant "wrong RVA" or "no session", I read the sim directly:

```
coop-probe --pid 20132 --rsm 0x106D40000 --frames 3
frame 0: 22/22 cars, 7.81 ms, rc=Some(RcState { flags: 0, rc_state: 0, rf_state: 0 }),
         car0 dist=0.0 spd=0.0 tyre0=0.0
```

All distances zero, `rc_state = 0`: that is the **class default**, not a live session. So `fires = 0`
is the expected result when no race is running, and it does **not** indicate a wrong RVA. The two facts
together are the first clean statement this project has had about its own hook:

- the block is at `0xEB960000`, i.e. **1.45 GB** from the target at `0x142298080` - inside `rel32`
  range, which is exactly what the old `VirtualAlloc(NULL)` could not guarantee;
- the callback pointer is a real code address inside the agent module;
- the stub has not been entered because nothing is simulating.

**Not yet proven:** that the hook fires once a session starts, and that a drained mailbox moves a car.
Both need a race weekend loaded, which is the next live step.

### Reading the hook from outside at all

`tools/watch_fires.py <pid> [seconds]` finds the block without needing the log, by scanning committed
private regions for the stub's first 14 bytes. The displacements it matches are constants derived from
the layout:

```
SLOT_OFF = 0x90  ->  48 89 0D 89 00 00 00     mov [rip+0x89], rcx
FIRES_OFF = 0x98 ->  48 FF 05 8A 00 00 00     inc qword [rip+0x8A]
```

That check is worth keeping: it answers "is the hook live" in one command, which is the question every
failed sync was previously answered by guessing.

### A stale-build trap worth naming again

Re-injecting a DLL by path into a process that already has that path loaded is a **no-op**:
`LoadLibraryW` returns the existing module handle and runs nothing. Proof from this session - the second
inject reported

```
injected ... into pid 20132 (module base 0xE6F10000)
```

the *same* base as the first inject, and no new `coop_agent.log` appeared, because the new build never
executed. Renaming the file on disk is allowed even while loaded, so the deploy step now renames the old
one aside first. The game must be restarted for a genuinely new agent build to run.

### The transport, proven end to end against the real agent (2026-10-27)

Ran the full chain locally with no remote machine: relay on 45990, `coop-link --role host --fake`
publishing 22 cars at 20 Hz, `coop-link --role client` attaching to the *live host game* (pid 20132) and
forwarding to the **real agent already injected in that game** on port 45991.

Client:

```
[client] attached to pid 20132
[client] write channel connected to the agent at 127.0.0.1:45991
[client] local sim reads all zeros (session not started yet), so verification is unavailable
         - frames are still being sent
[client] 38 snapshots, 22 cars, host clock 12.2, gap 104 ms, sent 4158 to the agent, verified 0 of 22
```

Real agent, inside the game:

```
[agent] coop-link connected to the write port
[agent] callback 0x7FF9E6F13E30 registered but the hook has not fired yet
[agent] frames are arriving but not being applied: the hook ... has never fired: no sim call has
        happened yet. Frames are staged and will apply once a race session starts.
[agent] write port: 200 frames in, mailbox applied=0 dropped=223 entries=0
[agent] write port: 400 frames in, mailbox applied=0 dropped=423 entries=0
```

So: **link -> agent -> mailbox staging is proven**, and `applied=0` is *correct* here because no session
was running, so the hook never fired. The game stayed responsive throughout. The remaining unknown is
narrow and nameable: whether the hook fires once a race starts, and whether a drained frame moves a car.

### Two throughput bugs found by measuring instead of assuming

Both showed up only because the fake-host test counts snapshots. Neither would have been visible in the
game, where "slow" and "broken" look the same.

**1. Verification swept every candidate, per frame.** The client re-resolved the sim on *every* frame,
and `find_sim_candidates()` returns ~507 addresses while `read_cars()` pulls ~10 MB each - roughly 5 GB
of reads per sweep. Measured cost: **11 s of CPU in 15 s wall**, and **3 snapshots in a minute**.

```
[objects] 507 candidate address(es) to try
[objects] GUObjectArray 0x147821450 offset=0x10 item=24 chunk=0x180000 num=93820
```

Fixed: the hot path never sweeps. It uses the single best candidate `locate()` already picked, re-resolves
at most every 500 ms, and only when there is no address - a failed read is the only thing that forces a
re-resolve. Result: **38-40 snapshots per 2 s (19 Hz), gaps 56-104 ms**. That is a ~400x throughput
improvement, and it was sitting in the client the whole time.

**2. One `last_report` timer served two purposes.** The stall message reset the same timer the 2-second
report used, so a stall could suppress reporting. They are now separate.

### Verification no longer gates sending

The client used to refuse to send frames unless *it* could locate the sim. But the hook captures the sim
pointer from `rcx` inside the sim call, so the agent is the authority on which object to write; a second,
independent discovery in the client was only ever a way to block sync. Sending is now unconditional, and
verification is explicitly best-effort and labelled as such - "sent" never claims "verified".

### Files added this session

```
coop-rs/crates/coop-agent/src/hookmail.rs   the write mailbox + its 7 tests
tools/watch_fires.py                         is the hook firing? finds the block by signature
tools/fake_agent.py                          stand-in agent, exercises the real wire format
RACE-SYNC-RUNBOOK.md                         the checklist that replaces guessing
```

`remote.py` gained `attach-client`, `fires`, and a `release_dir()` that respects `CARGO_TARGET_DIR` -
the hardcoded `coop-rs\target\release` was how a stale binary got shipped with a "VERIFY: OK".

## First sprint with the DLL stack: what synced, what did not (2026-10-27 17:13)

**The hook fires.** This was the open question for the whole project, and it is now answered:

```
[agent] hook fired x43 (total 7754) callback=0x7FF9E33A3EB0
[agent] hook fired x29 (total 3875) ...
```

Roughly 25-30 calls per second, sustained, on both machines, with the game responsive. The rel32 fix
from earlier was the thing that made it work; before that the hook could install but never survive.

**Replays work** - the user's own verdict - and it holds up longer than the Lua version did. Positions
replicate continuously: the publisher sustains 19.5-19.9 Hz with 2488-byte frames, and the client's
agent reports `entries` climbing.

### The three defects, and they are one defect

Reported symptoms from the sprint:

1. red flag not synced; the client sat looking at stuck cars instead of forcing the replay;
2. tyres not synced;
3. damage not synced, which left the client with broken cars racing.

Cause: **`coop-link` sent only `Msg::Snapshot`.** Verified rather than assumed:

```
coop-proto:   Msg::Rc(RcState), TAG_RC, encode/decode, tests      present
coop-proto:   layout::rc(), FLAGS 0x24C, RC_STATE 0xA045C0, RF_STATE 0xA13289   present
coop-link:    references to Rc / RcState / rf_state ...            0
```

The wire format and the reader both existed and were tested; nothing ever called them. So the client
received car positions and nothing else, which is why a host red flag looked like the client's cars
simply stopping - there was no message to explain why, and no message to start a replay.

This is also the honest answer to "how does this differ from the Lua version": Lua read the sim's event
bus and race control, so it knew a red flag had happened. Both new problems are the same missing input
- the client cannot see tyre or damage state because only four position floats per car cross the wire
(`hookmail::FIELDS`), and damage is not a field at all.

### A bug found while implementing race control

`hookmail::apply` wrote a `u32` for every entry. But `RC_STATE` and `RF_STATE` are **one byte**, and
`RF_STATE` sits at `0xA13289` - near the end of the object. Writing 4 bytes there would clobber the three
bytes after it, i.e. exactly the kind of small overrun that becomes a crash somewhere else. Entries now
carry a width and `apply` writes exactly that many bytes; `one_byte_fields_do_not_clobber_their_neighbours`
puts `0xAA` sentinels either side and requires them to survive.

Race control is also staged **before** the cars, so a frame that overflows a buffer still carries a red
flag. A dropped speed field is cosmetic; a dropped red flag desynchronises the session.

### Why this may explain the client's large crashes

A client writing tyre and damage state it never received means the two sims disagree about whether a car
is intact. That produces the "broken cars racing" the user saw, and a sim whose damage model disagrees
with its own positions is a plausible source of a hard crash. Not proven - but it is the first
hypothesis in this project that explains a crash as a consequence of missing state rather than of a bad
write.

### Still open

- `Msg::Rc` is sent and applied, but **not yet tested in a live red flag** - the share went down before
  redeploying to the client.
- Tyres are a `FIELDS` change, not yet done.
- Damage needs the event bus (`ERaceSimEventType`), the bigger job, per RACESIM-HIJACK Â§5.2.
- The publisher's `read_cars` (one 10.5 MB read) has been seen to fail with `ERROR_PARTIAL_COPY`
  mid-session. `read_rc` reads its 6 bytes individually for that reason; `read_cars` still needs the
  same treatment or an intermittent loss of snapshots.

## Tyre sync: mapped, staged, and off by default (2026-10-27)

The user asked for a deliberate tyre sync, not a blind one. What that turned into:

### The mapping was already half-built

| part | status before this work |
|---|---|
| `Car.tyres[4]` with surface/carcass/condition | existed |
| wire carries `tyre_lo`, `tyre_hi`, 4x3 floats | **already encoded and decoded** |
| `layout::car_at` decodes them | already |
| offsets inside the car slot | verified: last tyre offset 0x972760, slot ends 0x972E90 |

So the host and the protocol needed **no changes**. Only the apply side ignored tyres. That is worth
recording because it means the risk here is much smaller than it looked: no new wire format, no new
discovery, no pointer arithmetic - just offset writes into a slot we already write.

### Why it is staged rather than switched on

From RNG-DETERMINISM: temperature is an **input to the puncture curve** (~25%/lap at zero wear), and wear
is **derived** from driving. So tyre state is not a mirror of something the client computes identically -
it is an input to a model that decides punctures. Writing it from outside is overriding the simulation.

And the precedent is explicit - NOTES.md:1319 says removing the `RcState`/`RfState` *writes* was "the
change most likely to fix the crash" after v35. Writing-first is what cost that generation its session.

### The three stages, and their exit criteria

`coop_tyres.txt` next to each exe selects the mode at run time; unknown content resolves to `observe`, so
a typo cannot enable the risky path.

- **observe** (default, writes nothing) - logs real per-wheel values once a second plus the observed
  surface/carcass ranges and a count of out-of-range values. Exit: ranges known, out-of-range count 0.
- **wear** - writes `condition` only. Temperature stays local because it is the puncture input.
  Exit: a full stint without a crash, client wear tracking host wear.
- **full** - adds both temperatures, only if wear is clean.

### What makes a bad value harmless

Every value is range-checked before it is emitted, and an out-of-band value is **skipped, not clamped** -
a clamped value is still a lie the sim acts on, while a skipped field leaves the client's own model in
charge of something we demonstrably do not understand.

`NaN` is rejected by the same comparison without a special case, which matters because a NaN in a float
the game divides by is the cheapest possible way to make a crash that looks unrelated to its cause - and
it is exactly what a misread offset produces.

Accept bands: surface/carcass -40..400, condition -0.01..1.5.

### Ordering inside a frame

race control (3) -> positions (88) -> tyres (264), so the least critical part is at risk if the budget is
ever cut. Full 22-car frame is 355 entries against a 384 budget, asserted by test.

### Tests: 40 in coop-agent, all passing

The tyre-specific ones: `observe_mode_writes_no_tyre_fields`,
`wear_only_writes_exactly_the_condition_floats`, `full_mode_writes_all_twelve_tyre_floats`,
`implausible_tyre_values_are_skipped_not_clamped`, `tyre_bounds_reject_the_obvious_nonsense`,
`mode_parsing_never_falls_through_to_the_widest_mode`,
`a_full_frame_fits_even_with_every_tyre_field`.

### Deployed, in observe mode on both machines

    coop-link.exe    287232  76F6F49BE5F9CF48
    coop_agent.dll   305152  E082013869EA94BC

`coop_tyres.txt` = `observe` on both. Deploying this changes nothing until the file is edited, which is
deliberate: the first run of the tyre code should not also be the first run of tyre writes.

### Still open

- Stage 0 numbers have not been collected - the host game is not running.
- Damage remains unstarted; it needs the event bus, not a field.
- Race control still needs its first live red flag.

## Host restart required for the tyre build (2026-10-27 17:45)

Found while preparing the sprint: the host game (pid 37912) was a **fresh process that had never had the
agent injected**, so the only agent in it was one loaded before the tyre work. Its log showed
`hook fired x60 ... mailbox applied=4661 entries=410168` - genuine sync from the previous build - but the
DLL on disk (305152 bytes, E0820138) was not the DLL in memory (288768 bytes, 474A4716), and the loaded
build's log contains no `tyres observed` line at all.

This is the same trap recorded twice already, now with a third face: **not** "re-injecting a loaded path is
a no-op" this time, but "a process restarted between deploys silently keeps whatever the *first* inject
put there." The check that caught it is comparing the module base against the file hash, and looking for a
log line only the new build can emit.

What was done:

- replaced `coop_agent.dll`, `coop-link.exe`, `coop-inject.exe` on the host with the new builds
  (`E0820138`, `76F6F49B`, `E72ACCE7`);
- left `coop_tyres.txt` = `observe` and `coop_hook.enable` in place;
- wrote `tools/reinject-host.ps1`, which picks the largest `F1Manager24` process, injects, and then
  verifies by *reading the log* rather than trusting the injector's exit code. It prints the DLL hash and
  the tyre mode so the state is visible before a test rather than inferred after one.

So the sequence for the next run is: restart the host game, run `tools/reinject-host.ps1`, start the
publisher, then sprint. Everything else - relay, client, its agent with the tyre build, `observe` mode on
both machines - was already up and verified.

### Client state at the same moment (good, for contrast)

```
[agent] hook fired x60 ... mailbox applied=4661 dropped=144 entries=410168
```

4661 frames applied and 410168 field writes, with only 144 dropped - a 3% drop rate at ~20 Hz, which is
the hook running at roughly 60 Hz against a 20 Hz producer. The client has been carrying a real,
sustained write load without crashing, which is the strongest evidence so far that game-thread writes are
the right architecture.

## Full game export mined; plan written for all systems and three game modes (2026-10-27)

No game launched. Work was on the export at `E:\FModel\Output\Exports\F1Manager24` (86,332 files,
81,162 JSON) and on code.

### The finding that reorganises the project

`Content/Database/SavePS.json` is the game's persistence layer as SQL: **40 tables**, each with a `SELECT`
and an `INSERT`. So the game already knows how to serialise everything that matters, and the save is the
natural co-op boundary - "same world" can be defined as "same save", which already transfers
byte-identically between machines.

Three beliefs the project held were wrong, and the export corrects them:

| believed | actually |
|---|---|
| damage needs the event bus (RACESIM-HIJACK Â§5.2) | `Save_RaceSimCars_Parts` has **six named damage levels**: Body, FrontWing, RearWing, SidePods, Floor, Suspension |
| the sim has no clock to anchor on | `Save_RaceSimManager.SimTime` is persisted - time is the game's own anchor |
| `race_pos` is derived and must not travel | `Save_RaceSimCars.RacePosition` is persisted, so it is state |

`Save_RaceSimCars` has 47 columns including `Fuel`, `EngineTemperature`, `BrakeTemperature`, `DRSState`,
`TotalRaceTime`, `LapCount`. That is the full list of what a car's race state actually is.

### The clock bug, fixed

`Config/DefaultEngine.json`:

```
bSubstepping=False
MaxPhysicsDeltaTime=0.033333
MaxSubstepDeltaTime=0.016667
```

**No fixed timestep.** Each machine integrates a variable wall-clock delta, so two sims are never at the
same step. Combined with the frozen `race_time` (Â§ below), that is why 1.88 M field writes in one session
still left the client 62 km off: the writes landed and the sim integrated past them.

Measured live during the race:

```
SIM_TIME   951.00 -> 953.00 -> 955.00     advancing
lap_times  928.8, 933.2, 925.8, 927.8     frozen (all cars)
max        933.22                          so race_time frozen
```

`race_time` is `max(lap_time)`; once cars stop completing laps it freezes. The transport carried only that
value, so for the last part of every race the client received a **constant clock** - and a client cannot
decide how far to advance without knowing how much time passed. The staleness guard then discarded good
frames.

Fixed: `Msg::Snapshot` now carries `sim_time` (`0x99AF1C`), staleness keys on it, `race_time` is documented
as display-only. Test `a_frozen_race_time_does_not_hide_an_advancing_sim_time` encodes the failure so it
cannot regress. Found and fixed on the way: **`TAG_READY` was `6`, the same as `TAG_AGENT`** - a silent
collision that decoded `Ready` as `Agent`; there is now a `message_tags_are_unique` guard.

### Three game modes, and the export already keys for them

| table | key | granularity |
|---|---|---|
| `Save_CarConfig` | `TeamID` + `LoadoutID` | team, then car |
| `Save_DriverCommands`, `Save_PlayerStrategyStints` | `LoadoutID` | car |
| `Save_CarTyreAllocation` | `CarID` | car |
| `Buildings_HQ`, `Teams_PriorityList_Setting_DevelopBuildingData` | `TeamID` | team |
| `TeamDrivers` = `Staff_GameData JOIN Staff_Contracts WHERE StaffType = 0` | `TeamID` | team |

So **"my side of the garage" is a `LoadoutID`** and **"facilities come from Player A" is a `TeamID`** - no
reverse engineering needed for the split itself.

- **Shared team**: each player owns their own `LoadoutID` (setup, tyres, commands, pit calls); Player A owns
  team-scoped rows (research, facilities, pit crew, finances, contracts).
- **Versus**: each player owns a whole `TeamID`; the race sim stays one authority for all 22 cars, which is
  already the architecture, so versus is the *most* natural fit for the existing code.
- **Co-op career**: versus ownership plus one shared world.

One subtlety worth keeping: `Save_CarConfig` is keyed by both, so "only the host writes team rows" would
stop Player B setting up their own car. The rule must be `team == my_team AND loadout == my_car` for setup,
`team == my_team` for facilities.

### Added

```
tools/watch_rc.py        samples flags/rc_state/pit counters, prints transitions only
tools/scan_damage.py     finds slow-changing per-car fields (damage discovery by measurement)
tools/preflight.py       20 checks, wiring vs flow separated
COOP-PLAN.md             the whole-game plan incl. the three modes
coop-link/src/starthold.rs  grid hold, both sims, 8 tests
```

### Tests: 43 agent, 27 proto, 16 link - all passing

(Only the pre-existing `coop-mem` `loop_stub_volume` failure remains.)

## Authorisation layer and correction model built (2026-10-27, no game)

Plan items 2 and 3 from [COOP-PLAN.md](COOP-PLAN.md), both offline-testable. **113 tests passing** across
coop-auth (13), coop-proto (38), coop-agent (46) and coop-link (16).

### New crate: `coop-auth` - who may write which row

One function answers the question all three modes reduce to:

```rust
may_write(mode, player, scope, row_key) -> Result<(), Denied>
```

Scopes are `PerCar`, `PerTeam`, `World`, `RaceResult`. The rule:

| scope | SharedTeam | Versus / CoopCareer |
|---|---|---|
| `PerCar` | row's **loadout** must be mine | row's **team** must be mine |
| `PerTeam` | row's team must be mine | same |
| `World` | host only | host only |
| `RaceResult` | host writes | host writes |

The `PerCar` split is the whole feature: in shared-team mode each player owns one car, so the loadout is the
boundary. The trap it exists to avoid is documented and tested - `Save_CarConfig` carries **both** a
`TeamID` and a `LoadoutID`, so a team-only check would let a player reconfigure their teammate's car.
Test: `a_team_key_alone_does_not_grant_car_access_in_shared_team_mode`.

An unidentifiable row (no key at all) is **refused**, not allowed. Guessing "mine" is how a player ends up
editing another team's facilities.

Mode comes from `coop_mode.txt`, identity from `coop_player.txt` (`team loadout host`). Unknown mode text
resolves to `Versus` - the most restricted useful default, so a typo cannot widen access.

`gate_sql()` is now wired into `coop-sql`, which could previously execute **any** statement including edits
to the other player's team. It parses the leading verb, maps the table to a scope via `scope_of()`, and
refuses before the statement reaches the database. A table nobody classified is refused rather than written
blind.

### New module: `coop_proto::correct` - drive by correction, not overwrite

The measured failure: **1.88 M field writes and still 62 km off**, because `bSubstepping=False` means the
receiving sim integrates a variable wall-clock delta and steps past every write.

The fix sends `(host_sim_time, host_state)` and lets the receiver size its correction from the **time gap**,
not the distance error:

```
gap < 20 ms          -> no correction (a synchronised client is not jittered)
20 ms..250 ms        -> linearly ramped
>= 250 ms            -> full scale
per-frame cap        -> 25% of the remaining error, so a large gap is a fast catch-up, never a teleport
```

Three details that are not obvious and are each a test:

- **Heading wraps.** A direct subtraction across `0`/`2*PI` turns a 1-degree correction into 359 degrees and
  spins the car. `angle_delta` normalises, and the test asserts naive subtraction really is that wrong.
- **Lateral/heading wait until distance is close** (>50 m error suppresses them). Correcting them while the
  car is far away drags it sideways across the circuit instead of along it.
- **A clock of 0 disables correction entirely.** `sim_time` 0 is the wire's "unknown", and without a clock
  there is no way to size a correction. Guessing is what produced the divergence.

Tests include a 400-frame simulation of the exact failure - 22 cars, 60 km and 1 s behind, with the client's
own integrator still moving between frames - required to **converge**, not diverge.

### Wiring, so it is not another tested-but-unused module

`hookmail::publish_corrections` + `stage_corrections` resolve the delta against the receiving sim's own
values (the mailbox writes absolute memory, not deltas). An **empty** correction list publishes nothing at
all: a synchronised field must cost zero traffic, or every tick wakes the hook to do nothing.

`coop-link`'s client now prefers correction, and falls back to the old full-state send when it cannot read
its own sim. The report line shows `(N corrections)` so the new path is visible in the log rather than
assumed.

### Still not done

- Correction and auth are **not yet exercised against a live race**. Both are desktop-verified only.
- Part-state offsets (6 damage levels, 4 tyre conditions) still need a live race to locate.
- The race start hold is not yet wired into the publisher.
- Career features (calendar, finances, facilities enforcement) have the gate they need, but no callers yet.

## Packaging and in-game lobby: one pak, fewer files (2026-10-27, no game)

Requirements: a client should be one `.pak` plus as few files as possible; the server should be launchable
from the main menu; team selection should exist per player.

### The constraint that shaped it

The front-end is a browser-like context. Measured from the shipped bridge: `fetch` and `XMLHttpRequest` are
present, `child_process` and `require` are not. **It cannot start a process.** Packaging an executable in the
pak to extract-and-run was rejected - writing binaries to disk from a game process is what antivirus
heuristics flag, and it buys nothing.

So the menu asks the agent, which is already resident in the game:

```
menu button -> fetch POST /coop/host -> coop_agent.dll (loopback HTTP)
```

### Built

- **`coop_agent` lobby endpoint** on `127.0.0.1:45992`: `GET /coop/status`, `POST /coop/host`,
  `POST /coop/join`. With CORS headers - Gameface is not served from that origin, so without them the button
  silently does nothing. OPTIONS preflight answered. JSON only, endpoints strictly enumerated.
- **`pak/.../js/coop/lobby.js`** (14.5 KB) loaded from `Frontend.html` in the same pak: mode picker
  (shared-team / versus / coop-career), host/join buttons with timeouts, live status, and **"Pick my team"**
  which navigates to the game's own `ScreenID.SelectTeam`.
- **`coop-link --inject`** - the injector is now a library, so the separate `coop-inject.exe` is optional.
  Both error paths verified: `OpenProcess failed (Win32 87)` for a bad pid, `no F1Manager24.exe running` for
  no game.

### Team selection exists already, and we should not reimplement it

```
ScreenID.SelectTeam   = 33    exists
ScreenID.ChangeTeam   = 14    exists
TeamSelectMenu.js             onToggleTeam -> DS.getValue(teamCTX,'teamID')
                              -> FrontendUtil.setSelectedTeamIndex -> ConfirmChangeTeamDialog
```

`lobby.js` navigates to the real screen instead of faking a picker. It uses
`FrontendNavigation.set(screenID, extraProps)` rather than a raw `engine.trigger`: `set()` also dispatches
`FrontendScreenChanged` through the engine *and* updates the module state the next screen reads. A bare
`sendEvent` does not - which is how an earlier attempt ended up on a black screen.

### The real constraint on per-player teams

`Player` has exactly **one row** (`SELECT FirstName, LastName, TeamID, ... FROM Player`). Two players in two
teams cannot both be "the Player" in one save. Versus therefore means two clients sharing a career that
belongs to the host's player slot, with `coop-auth` preventing writes outside a player's team. That is a
genuine limitation of the game's data model and should be stated to the player, not papered over.

### File budget

| file | host | client | reducible? |
|---|---|---|---|
| `CoopBridge_P.pak` (35 KB) | yes | yes | no - it is the UI; identical both sides |
| `coop_agent.dll` | yes | yes | no - must be a separate file to be injectable |
| `coop-link.exe` | yes | yes | no - it is the process; now also the injector |
| `coop-inject.exe` | optional | optional | **gone** via `--inject` |
| `coop-supervisor.exe` | host | no | could move into the agent |
| config files | one | one | three collapsed to one planned |

Still to do for the budget: collapse `coop_hook.enable` / `coop_tyres.txt` / `coop_mode.txt` into one file,
and host the relay on an agent thread so the supervisor is not a separate process.

### Tests: 116 passing, 0 failing

New this round: lobby endpoint over a real socket including CORS (`the_lobby_endpoint_answers_status_with_cors_headers`),
JSON validity for every endpoint, unarmed-before-injection reporting, plus the 13 auth and 10 correction tests
from earlier.

`lobby.js` passes `node --check`. Pak verified: `repak get` of `lobby.js` is **byte-identical** to source
(`F3630523`), mount point `../../../` and version `V11` matching the working pak.

### Not verified

- The menu entry has never been rendered - no game launch this session. The DOM anchor, the button placement
  and whether `preact` re-renders over it are all unknown.
- `FrontendNavigation.set(ScreenID.SelectTeam)` has not been called against a live game.
- The agent's HTTP endpoint has only been tested as a library, not inside a real game process.

## Multiplayer in the main menu, natively (2026-10-27, no game)

The floating corner panel was a shortcut, not an integration - it ignored the menu's layout, focus order,
styling and localisation. Replaced with a patch of the real `MainMenu` component.

### Structure it hooks into, read from the export

```
MainMenu_vBox.mainButtons
  MainMenu_hasSaveButtonsBox
    MenuButton { label:'[LOAD_CAREER]',      modifiers:'defaultSize crossesBR shadow' }
    MenuButton { label:'[NEW_CAREER]',       modifiers:'defaultSize crossesBR shadow' }
  MainMenu_rightBox
    Carousel { rootClassName: 'MainMenu_carousel' }   <- the race replay reel
    MainMenu_bottomRightDisplay  (gamerTag, buildNo)
```

The multiplayer entry is inserted as a real `MenuButton` with the game's own modifiers, inside
`MainMenu_hasSaveButtonsBox`, so it inherits the exact styling and focus behaviour. The race replay reel is
shrunk with `transform:scale(.6)` on `.MainMenu_rightBox .MainMenu_carousel`.

### Two facts that decided the implementation

1. **`Localisation.translate` only resolves strings wrapped in `[...]`.** Read from
   `common/core/Localisation.js`: anything without the brackets is returned unchanged. So the label is the
   literal `"Multiplayer"` and **no string table ships**. Using `[SOME_KEY]` would have needed a locres.
2. **`FrontendNavigation.set(screenID, extraProps)` is the correct navigation call**, not `engine.trigger`.
   It dispatches `FrontendScreenChanged` through the engine *and* updates the module state the next screen
   reads. A raw `sendEvent` does neither - which is how the earlier attempt produced a black screen.

### The screen

`COOP_SCREEN = 99` (the game's enum stops at `SelectTeam = 33`), rendered by wrapping `FrontendRouter` so
navigation, focus and teardown behave like any other front-end screen.

Contains, as asked:

- **Back button** - `nav.set(ScreenID.MainMenu)`.
- **New career / Load save** - both, because those are the two ways two players end up in one season.
- **Mode** - shared team / versus / co-op career, each with a one-line explanation of who owns what.
- **Host / Join** - disabled until the agent reports the hook installed, so the button cannot lie.
- **Player teams with who has taken them** - each team shows `host`, `player 2` or nothing, and a claimed
  team is dimmed and refuses a second pick.

### Team claims are shared between machines

localStorage is per-machine, so the second player cannot see the first player's claim unless something shared
carries it. Added to the agent:

```
GET  /coop/teams                       -> {"claims":[{"team":"Ferrari","role":"host"}]}
POST /coop/claim?team=Ferrari&role=host
GET  /coop/release?role=host
```

Last claim wins per role; a player holds one team at a time (changing team releases the old one, tested).
Two players *can* claim the same team - that is deliberately not prevented here, it is **shown**, and
`coop-auth` decides whether a write lands. Preventing it needs agreement between machines, which is the
relay's job.

Query-string parsing is hand-rolled (no URL-decoding dependency for two ASCII words), with `%20` and `+`
handled because team names contain spaces. Tested directly - a wrong parse would make a claim silently vanish.

### A bug the tooling caught

`node --check` failed on the first version: a `prototype.fire` method had been inserted *inside* the
constructor, closing it early and leaving the remaining methods at the wrong nesting. Braces balanced
overall, so only a real parser caught it. Fixed, and the check is part of the routine now:

```
node --check pak/.../lobby.js
repak get <pak> .../lobby.js | node --check    # verify what actually ships
```

### Verified

```
lobby.js                      valid JS, byte-identical inside the pak
CoopBridge_P.pak              50,017 bytes  8D699119E7C61534
coop_agent.dll               332,288 bytes  2BCE4A83EE9E236C
coop-link.exe                388,608 bytes  FDCBDDB699D5BD38
tests                        52 agent + 13 auth + 16 link + proto, all passing
```

### Not verified - and this is the important part

**Nothing here has been rendered.** No game launch. Specifically unknown:

- whether `MainMenu_hasSaveButtonsBox` is where the button must go for the layout to look right, or whether
  preact's children array accepts a push at render time;
- whether patching `Ctor.prototype.render` survives the module being re-required;
- whether wrapping `FrontendRouter` is enough for our screen id to render, or the router keeps its own map;
- whether `window.Engine.sendEvent('NewGame')` is the right event name for a new career.

Each of those is one launch away from being known, and each is a guess until then.

## 2026-10-29 - the coordination sender, and an honest status

The question was "how well does multiplayer work with no launches?". The answer was: the plumbing was
tested and the multiplayer was zero, because **nothing ever produced a Clock, Contract or Vote frame**.
That is now fixed in code and tested headless; it is still zero in a running game, because the summary at
COORDINATION-SENDER.md stands - no launch has happened.

What was actually broken, in order of severity:

1. Nothing produced coordination frames. `coordination.rs` had the rules and the supervisor arbitrated
   them, but no component sent one. Now: `js/coop/observe.js` (wraps `engine.trigger`) and
   `coop-agent/src/coord.rs` (the frame grammar) + an outbox drained by the agent's heartbeat thread.
2. Even with a sender, the supervisor would have dropped every frame. It keys a peer's side off the
   `Hello` on that peer's socket, and an agent's socket never sends one - the peer slot belongs to the
   game DLL. Every agent frame hit `sent data before Hello; dropped`. Fixed by putting the side in
   `Msg::Agent` (VERSION 1 -> 2) and arbitrating on `side.or(agent_side)`.
3. The agent's socket was write-only, so a refused claim (`Msg::Bye`) or a resolved vote went nowhere a
   player could see. Fixed with a bounded inbox + `/coop/inbox`.
4. Routing decided by `side.is_none()` instead of by whether the socket ever announced an agent. Wrong
   property: a socket can do both. Now keyed on `agent_side.is_some()`.

Tests: Rust 203 (excludes the pre-existing coop-mem `loop_stub_volume` 0xC0000005 crash); JS 168 across 7
suites including a new 16-assertion end-to-end that runs a real supervisor process and speaks its wire
protocol as two agents. Both new suites were checked by planting the regression and watching them fail.

Bugs found in my own test code, worth remembering:
- `h * 0x01000193` is not FNV-1a in JS (exceeds 2^53) - needs `Math.imul`. Cost a debugging round on
  "checksum mismatch".
- A promise reader that leaves its resolver queued after a timeout silently eats the next frame. Looked
  exactly like the supervisor not delivering.
- A test that used Hello on the agent socket was testing a topology the deployment never creates.

Still unknown, and the reason this is not "done": whether a pause in the running game stops the other
machine. Mapping, framing, routing and delivery are each proven; the join is not. Also unknown whether the
engine pauses when it receives a clock frame - the agent can receive it, but acting on it means calling
SimPauseChange, which the export cannot prove.

### 2026-10-29 (later) - the clock now acts, not just reports

Closed the gap I had explicitly left open: receiving a clock frame only wrote a log line, so "one player's
clock stops the other's does too" was still false in behaviour even though the frame arrived.

- `coop-agent/src/clocksync.rs`: delegates to `coordination::ClockGate` (no second implementation of the
  union rule) and exposes a `seq` that advances only on a *transition*. Acting on a `stopped` flag every poll
  would re-pause a paused world forever; acting on `seq` acts once per change. 9 tests.
- The agent records our own reasons on the way out (from `/coop/coordinate/clock`) and the other player's on
  the way in, then serves the combined state at `GET /coop/clock`.
- `bridge.js` polls it at 400 ms and calls `engine.trigger('SimPauseChange', value, 9)`.
  Layer 9 (`RaceViewUI`), not 2 (`UserPause`): our own layer means unpausing restores only what the mod held
  and cannot cancel a pause the other player set themselves. Guarded on `PAGE === 'RaceWeekend'` because
  `SimPauseChange` elsewhere has no sim to pause - the same class of stray trigger that black-screened under
  `LoadGame` (LOBBY-GATE.md).

Both new polling loops got the give-up discipline, and the normal-play assertion took three attempts to
actually discriminate: counting `setInterval`, then request counts, then long-period timers all failed to see
a chain that reschedules without reaching `open()`. The count of armed timers does: 1 guarded, 2 unguarded.
My first bound allowed 2 and passed in both cases - a test that looked right and proved nothing, which is
exactly the failure mode this file exists to prevent.

Rust now 212 (up from 203). observe_test 67 (up from 54), normal_play 21 (up from 19).

Still unproven and unchanged: no game launch. Whether `SimPauseChange` actually holds a live sim is the one
claim no export or test can settle.

### Not deployed, deliberately

Owner was unavailable to authorise writing into the game install, so nothing was deployed. Recorded the
reasoning because "why didn't you just do it" is the obvious question:

- the client machine is unreachable, so a host-only deploy would put the two machines on opposite sides of the
  VERSION 1 -> 2 bump. Proved that mismatch fails loudly (9/13 e2e assertions fail, `peer speaks version 1,
  this build speaks version 2`) rather than silently - but failing loudly is still failing, and the deploy
  script exists specifically to prevent a one-sided install.
- it would not have enabled a test anyway: a real two-machine test needs both, and the coordination work is
  headless-verified regardless.

Current state: pak 119,728 bytes and coop_agent.dll 410,624 bytes, both fresh and verified; host still runs
the 28 Oct pak. Everything needed for the launch is in place - the launch itself is the owner's call.

### 2026-10-29 - Steam "Join Game" while the host is on the host tab

Requirement: a friend clicks Join Game in Steam while the host sits on the host-game tab, and ends up in that
session.

What Steam needs, from Valve's docs rather than assumption: a **lobby** (that is the thing a friend's client
finds via GetFriendGamePlayed -> FriendGameInfo_t.m_steamIDLobby and joins with JoinLobby); `+connect_lobby <id>`
on the command line when the invitee is not running; a `GameLobbyJoinRequested_t` callback when they are; and
lobbies created only while needed. The game ships no matchmaking, so all of it is ours.

Built:
- `lobby_policy.rs` (13 tests): marker `f1m24-coop` + protocol version in lobby metadata. Both required,
  because every player in a friend's lobby is on app id 2591280, so the app id cannot tell a co-op session from
  a friend playing alone. Four *distinct* refusals, because "different build" for someone who is simply playing
  solo sends them after a problem that does not exist. HostState ties the lobby to the tab; `Filled` keeps the
  lobby but closes it to new joins, since a second friend admitted by Steam and refused by the mod reads as a
  bug.
- `lobby_steam.rs` (6 layout tests): FFI with every signature taken from `steam_api_flat.h`. Three would corrupt
  memory if guessed by analogy with the class methods - `GetLobbyData` returns `const char *`, and
  `CreateLobby`/`JoinLobby` return `SteamAPICall_t`. The callback object is a real vtable: `CCallbackBase` is
  `{vptr, u8 flags, i32 id}` = 16 bytes with three slots in declaration order, and `GameLobbyJoinRequested_t` is
  `k_iSteamFriendsCallbacks + 33` carrying two u64s. Sizes, alignments and both offsets are asserted, because a
  mismatch means Steam writing into the wrong field of our struct inside the game process.
- Endpoints `/coop/lobby`, `/coop/lobby/set`, `/coop/lobby/join`, `/coop/connectlobby`. `state` and `lobby_id`
  are reported separately: "host tab open but nothing advertised" is the failure that wastes a session and is
  only visible if intent and reality are both reported. A failed attach answers `state=unavailable`, never a
  cheerful "waiting".
- `lobby.js` opens the lobby when the host tab is chosen and closes it on back. Fire-and-forget: the visible
  status is about the sim being armed, and a Steam failure reported as "cannot host" points at the wrong thing.
- `bridge.js` reads `+connect_lobby` once on boot, so launching from an invite joins instead of landing on the
  main menu.

Rust 231 (was 212). New lobby_api_test 25/25. Nothing deployed; no live Steam client has run any of this.

Chose lobbies over ISteamParties beacons (`SteamAPI_SteamParties_v002` is exported): same visible result, one
interface instead of two. Also worth noting the lobby path needs no publisher-registered localisation token,
unlike `steam_display` - which is why the presence work uses `status`.

One more string-scan trap: `+connect_lobby` is absent from the built DLL as a contiguous literal while
`/coop/connectlobby` from the same function is present, so the literal is encoded as integer immediates. A byte
scan cannot prove or disprove it; the 12 parser tests are the evidence.
