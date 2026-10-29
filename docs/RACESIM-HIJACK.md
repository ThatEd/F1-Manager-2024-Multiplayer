# RaceSim hijack surface: making the sim a slave to a host

Functional, on-paper analysis: **which parts of F1 Manager 2024's race sim can be made to follow an
external authority, and by what mechanism.** No live tests. Nothing here was executed.

Evidence base (all read-only):

```
E:\FModel\Output\Exports\F1Manager24\Content\RaceSim\        227 files (113 .json + 113 .uasset)
E:\FModel\Output\Exports\F1Manager24\Content\UIGameface\js\  the UI (DataStore view of the sim)
E:\Steam\...\Win64\F1Manager24.exe                           build 17356935, SHA-256 7ffe404f…b2050f6
```

Tags as before: *(no tag)* = proven from the cited file; **binary** = proven from the exe at the
cited offset; **inferred** = reasoned, not executed; **unknown** = not established.

Scope note: this document is about **the sim's own surface** — its classes, delegates, events and
data. It deliberately does not audit the project's existing co-op tooling.

---

## 0. The shape of the answer

The sim is **not** a lockstep simulation with an authority concept, and it has **no settable clock**.
It is a collection of independent subsystems that each consume state and emit **119 named events**.
That produces one dominant conclusion and one sharp constraint:

**Constraint (the reason "slave" is hard):** there is **no `SetSimTime`**. The sim's time is
read-only across the entire reflective surface — `GetRaceTime`, `GetSessionTime`, `GetSimTimeCompleted`,
`GetCurrentLap`, `GetTimeRemaining`, `GetPlaybackRate` exist; `SetSimTime` / `SetRaceTime` /
`SetSessionTime` / `SetCurrentLap` / `SetLapCount` return **0 occurrences** in the binary (**binary**).
The 36 `Set*` names that do exist are either UI verbs (`SetRaceView`, `SetCameraType`) or **save
(de)serialisation** (`SetSaveRaceSimManager`, `SetSaveCarAI*`, `SetSaveWeekend` — 13 of them,
**binary**). So a host cannot tell a client "you are at t=53.2" through any supported API.

**Consequence:** a host cannot make the sim *compute the same state*. It can only make the sim
*display and accept the same state*, by driving the subsystems that consume per-car inputs and by
injecting the event stream. That reframes the question from "how do we make the sim deterministic"
to "which of the sim's inputs and outputs are writable, and what does writing them actually move".

**The strongest single lever** is `ERaceSimEventType` (**binary**, 119 members, full list in §2):
one enum that names every observable racing occurrence, exposed as a **broadcast delegate**. A hijack
can (a) *listen* to it to learn the host's race, or (b) *fire* it to make a client believe the race
happened, without touching a single simulated float.

---

## 1. What actually composes the sim

`BP_RaceSimGameMode_C` derives from `RaceSimGameMode` (`/Script/RaceSim`) and carries only **7**
Blueprint-visible components:

| component | class | hijack value |
|---|---|---|
| `BP_RaceSimNotificationsManager` | `RaceSimNotificationsManager` | **high** — the delegate fan-out point (§3) |
| `RaceSimChroma` | `RaceSimChroma` | low — cosmetic lighting; exposes `FlagState` (44 BP-visible props) |
| `BP_Audio_RaceSimTeamCommsManager` | audio | none for state |
| `BP_CustomRenderSettingsManagerComponent` | render | none for state |
| `RaceSimOnboarding` | onboarding | low |
| `AudioSceneManager` | audio | none |
| `DefaultSceneRoot` | SceneComponent | none |

So **the GameMode is not the sim.** It is a thin presenter. The state lives in the C++ side, which the
binary reflects as these types (**binary**):

`RaceSimManager`, `RaceSimData`, `RaceSimCars`, `RaceSimCar`, `RaceSimCarParts`, `RaceSimCarSetup`,
`RaceSimCarStrategy`, `RaceSimCarPitStop`, `RaceSimCarIncident`, `RaceSimCarMechancialIssues`,
`RaceSimCarCollisionEventData`, `RaceSimCarOvertake`, `RaceSimCarMisc`, `RaceSimTrackComponent`,
`RaceSimCornerClaim`, `RaceSimBarrierData`, `RaceSimEvent`, `RaceSimConstData`,
`RaceSimCollisionData*`, `RaceSimRaceControl*`, `RaceSimTest*`, `RaceSimReplayDelegate`.

**`RaceSimManager` is the hub** — `m_raceSimManger` is referenced from the car-damage blueprint
(**binary**, `0x5f34665`), and `RaceSimManagerInitialized` is a dedicated initialisation delegate
(**binary**). That is the object to slave first.

---

## 2. The event bus — one enum, 119 verbs

`ERaceSimEventType` (**binary**, values 0–119; the enum's own `Count` is present). This is the
complete list of things the sim considers observable, and therefore the complete list of things a
hijack can either **listen for** or **fabricate**:

```
AttemptOvertake              CarAboutToBeTeleported       CarComeToRestAfterCollision  CarDNF
ChequeredFlagEndEvent        ChequeredFlagStartEvent      CollisionWithBarrier         CollisionWithCar
CompletedLap                 CornerCollisionAttemptStart  CornerSqueezeAttemptStart    DRSEnabledEndEvent
DRSEnabledStartEvent         DamageToTyre                 DamagedCarBody               DamagedCarFloor
DamagedCarPart               DebrisOnTrack                DoubleYellowFlagEndEvent     DoubleYellowFlagSectorEndEvent
DoubleYellowFlagSectorStartEvent   DoubleYellowFlagStartEvent   DriverCommand         DriverPenalty
DriverSetupConfidenceLevelChanged   EndPitStop             EnterGarage                  EnterPitLane
EnterPitWindow               ExitPitLane                  FailedOvertake               FinishLinePassed
FinishedRace                 FlyingLapComplete            FlyingLapStart               INVALID
Lockup                       LowFuelDelta                 MandatoryCompoundsNotUsed    MechanicalFaultDeveloped
MechanicalFaultEscalated     MechanicalFaultResolved      NewBestLap                   NewPersonalBestLap
NewSector1BestTime           NewSector1PersonalBestTime   NewSector2BestTime           NewSector2PersonalBestTime
NewSector3BestTime           NewSector3PersonalBestTime   NotifyFailedOvertake         OvercutUndercutAttempt
PartWearWarning              PenaltyInvestigation         PenaltyNoFurtherAction       PitStopAdjusted
PitStopMistake               PlayerERSCharged             PlayerERSDepleted            PositionGained
PositionLost                 PracticeStateChange          Puncture                     QualifyingRunWarning
QueueRedFlagRacePitOptions   RaceStandingChanged          RaceStart                    RaceWon
RedFlagEndEvent              RedFlagPitsTransition        RedFlagRaceRestart           RedFlagStartEvent
RequestPitStop               RunWide                      SafetyCarBeginUnlapping      SafetyCarNoUnlapping
SafetyCarOnTrackEndEvent     SafetyCarOnTrackStartEvent   SafetyCarOutEndEvent         SafetyCarOutStartEvent
SafetyCarRequestedEndEvent   SafetyCarRequestedStartEvent Sector1CheckpointPassed      Sector2CheckpointPassed
SendCarOut                   SessionEnd                   SessionStart                 SlowCarOnTrack
Spin                         StartOptimalPitLap           StartPitStop                 StartedRaining
StartingLightChange          StillYellowFlagEndEvent      StillYellowFlagSectorEndEvent StillYellowFlagSectorStartEvent
StillYellowFlagStartEvent    StoppedRaining               StrategyChanged              StrategySuggestion
SuccesfullOvertake           TeamDNF                      TyreTemperatureChangeWarning TyreWearWarning
VirtualSafetyCarEndEvent     VirtualSafetyCarEndingSoon   VirtualSafetyCarStartEvent   WaitingForLightsOutEndEvent
WaitingForLightsOutStartEvent WaitingForPitLaneStartEndEvent WaitingForPitLaneStartStartEvent WaterOnTrackChange
WavedYellowFlagEndEvent      WavedYellowFlagSectorEndEvent WavedYellowFlagSectorStartEvent WavedYellowFlagStartEvent
WeatherStateChangeWarning    YellowRedFlagEndEvent        YellowRedFlagStartEvent
```

### 2.1 Why this changes the problem

Note what is **absent**: there is no `CarMoved`, no `PositionUpdated`, no per-frame kinematics event.
The bus carries **semantic** occurrences, not a state stream. Two consequences:

1. **The bus is a poor sync channel but an excellent authority channel.** You cannot reconstruct a
   car's position from it, but you can reconstruct *everything the player is told*, and you can
   manufacture any "something happened" message.
2. **`SessionStart` and `RaceStart` are events, not functions.** A host that can fire
   `SessionStart` → `RaceStart` drives the client's session lifecycle without a clock setter. **inferred.**

### 2.2 Payload

`RaceSimEvent` carries `m_type` (`EnumProperty`), `m_carId` (`ByteProperty`), `m_otherCarId`
(`ByteProperty`, `255` = none), `m_triggeredOnce` (`Bool`). Sub-payloads are fetched by accessor
(**binary**, `0x5f3aaa3`): `GetCollisionEventData`, `GetMechanicalFaultPayload`, `GetTyreDamageType`,
`GetTyreDamageWheel`.

The delivery points, all on `RaceSimManager` (**binary**): `OnRaceSimEventReceived`,
`RaceSimEventReceived`, and per-consumer `OnCarRaceSimEvent`. The payload is `InstancedStruct`-style,
which is why `GetMechanicalFaultPayload` needs a type-specific getter (**inferred**).

---

## 3. The six hijack strategies

Ordered by how much of the sim they actually control, and by cost.

### Strategy A — delegate substitution (the only true "authority" hook)

**What:** replace or drive `RaceSimManager`'s event-broadcast delegates so the sim reports a
host-supplied script rather than its own computations.

**Evidence:** `OnRaceSimEventReceived`, `OnCarRaceSimEvent__DelegateSignature`,
`RaceSimEvent__DelegateSignature`, `RaceSimReplayDelegate__DelegateSignature`,
`OnSendNewNotification` (on the NotificationsManager, `MulticastInlineDelegateProperty`,
Blueprint-visible) — all **binary** / asset-confirmed.

**Why it is the strongest:** it is the *only* route that lets a host change what the client's
subsystems are told happened, without fighting the simulation for ownership of state. The flag
subsystem, the notification stack, the replay director and the HUD all consume this one stream.

**Feasibility:** multicast delegates in UE are arrays of weak object pointers; appending a listener
is ordinary reflection work. Firing one from outside is harder — it requires calling the broadcast
helper, not writing the array. **Unknown** whether the sim re-broadcasts a client-side injected event
or only its own (i.e. whether injecting is closed-loop or one-way).

**Caveat that decides everything:** if the sim recomputes its own state every tick, a substituted
delegate only changes *presentation*, and the next tick overwrites it. Strategy A is therefore a
**presentation slave, not a state slave**, unless paired with Strategy D.

### Strategy B — per-car input injection (already the proven one)

**What:** write the sim's per-car kinematic fields each tick so the client's cars track the host's.

**Evidence:** the per-car record layout is established — `RSM + 0x95BC00 + carIndex*0x10D8`, with
`Lat +0x170`, `Head +0x184`, `Dist +0x194`, `Spd +0x198`, `Pos0/1/2 +0x1E0/0x1E8/0x1F0`
([NOTES.md](../NOTES.md) ~line 500). Whether these offsets hold for build `17356935` is `unknown` —
they came from a different offset table and the notes already warn about that.

**What it buys:** the car positions the *sim* works with. **What it does not buy:** rendering —
`NOTES.md` records "render follows sim state … DISPROVEN" (~line 1107), with the renderer reading a
separate 30 Hz history ring (`HistData RSM+0xEB0`, `HistNum RSM+0xEB8`). So B makes the sim correct
and the screen may still disagree. That is why B alone is not a slave; it needs F.

**Feasibility:** high — this is the best-characterised path in the whole project.

### Strategy C — race-control and flag injection

**What:** make the client's flag/VSC/SC/damage state equal the host's.

**Evidence:** `SetRaceControl`, `SetRaceControlIncidents`, `SetRaceControlInvestigations`,
`SetRaceState` exist as real setters (**binary**) — the *only* genuine state setters in the sim.
Supporting data: `RaceSimRaceControlRedFlagData`, `RaceSimRaceControlSafetyCarData`,
`RaceSimRaceControlIncidentResponse`, `RaceSimWaterLevelThresholds` (**binary**).

**The trap, already documented:** a flag on screen is read from **two** places — a live mask at
`RSM+0x24C` and a per-second history ring at `RSM+0x976F68 + k*0x14`, where the shown record is
`k = floor(RSM+0x99AF24)` (Playback). `NOTES.md` ~line 1250 records that "notifications read a
per-second flags HISTORY ring, never RSM+0x24C live", and that both must be written or the flag stays
up. Additionally `RfState` at `0xA13289` drives a state machine that hands off, so a single write is
not sufficient (**inferred**).

**Feasibility:** medium, and **the least trustworthy of the six** because it is the only one whose
current status is "wrote it, still unproven on screen" (~line 1430).

### Strategy D — stage and lifecycle injection (the one nobody has tried)

**What:** drive the session's own state machine with the sim's lifecycle events instead of a clock.

**Evidence:** the enum contains a complete lifecycle verb set — `SessionStart`, `RaceStart`,
`WaitingForLightsOutStartEvent`, `WaitingForLightsOutEndEvent`, `StartingLightChange`,
`ChequeredFlagStartEvent`, `ChequeredFlagEndEvent`, `SessionEnd`, `FinishedRace`, `RedFlagStartEvent`,
`RedFlagEndEvent`, `RedFlagRaceRestart`, plus the five SafetyCar phases. **binary.**

**Why this matters:** because `SetSimTime` does not exist (§0), the *only* supported way to move a
client's session from "pre-race" to "racing" is through these events. This is the direct functional
counterpart of the UI finding in [UI-COMMANDS.md](UI-COMMANDS.md) §3.5 — that no Gameface event
starts a session because the sim is engine-owned. On the sim side, the engine *does* expose the
lifecycle — as events, not setters.

**Feasibility:** **unknown**, and that is the single most valuable unknown in this document (§7).

### Strategy E — replay/seek (the tempting trap)

**What:** treat the sim as a replay buffer and seek the client to the host's timestamp.

**Evidence for the temptation:** `SetPlaybackRate`, `SetPlaybackPosition`, `SetPlaybackStart`,
`SetPlaybackEnd`, `SetPlaybackStartSeconds`, `SetPlaybackEndSeconds`, `SetPlaybackSpeed` all exist
(**binary**), and there is a first-class `RaceSimReplayDelegate` and `m_replayDataAsset`. This looks
exactly like a seek API.

**Why it fails as an authority model:** seeking is a **presentation** operation over the history ring,
not a state restore. The existing record is unambiguous: one seek restored the clock and the
fault/damage state but **left the lap counter at 10** (`rt 880 → 39`, `lap stayed 10`), producing "an
internally inconsistent session, which is worse than no seek for authoritative co-op"
([NOTES.md](../NOTES.md) ~line 1141). Also `SetPlaybackRate` is a **speed multiplier**, a different
quantity from the playback field — a mix-up that already produced one false "no effect" verdict
(~line 1076).

**Verdict:** E is a useful *diagnostic* and a bad *hijack*.

### Strategy F — render-path injection (the missing half)

**What:** write the renderer's own timeline so the picture follows the injected state.

**Evidence:** the renderer reads a distinct buffer — `HistData RSM+0xEB0` (i64 ring pointer),
`HistNum RSM+0xEB8` (i32 frame count, ~9000 ≈ 300 s at 30 Hz) ([NOTES.md](../NOTES.md) ~line 909).
The "render follows the ring … strongly implied (sim frozen, picture alive)" conclusion is at
~line 1108.

**Why it is not optional:** §2.1's absence of a `CarMoved` event, plus Strategy B's proven
sim-vs-render split, means **a complete slave must write two buffers, not one**. Any design that only
does B will produce clients whose *sim* is right and whose *screen* is stale.

---

## 4. Coverage matrix

What each strategy actually captures. "Yes" = the strategy can set it; "no" = untouched.

| sim state | A delegate | B car inject | C race control | D lifecycle | E seek | F render |
|---|---|---|---|---|---|---|
| car Dist / Lat / Head / Spd | no | **yes** | no | no | partial | no |
| car world position (f64×3) | no | **yes** | no | no | no | no |
| race clock | no | no | no | **inferred** | **yes** | no |
| lap counter | no | no | no | **inferred** | **no** (proven) | no |
| flags / VSC / SC / red | **yes** (as messages) | no | **yes** | partial | partial | no |
| damage & faults | **yes** (as messages) | no | partial | no | partial | no |
| tyre wear / temperature | no | no | no | no | partial | no |
| pit stops / strategy | **yes** (as messages) | no | no | no | partial | no |
| what the player is told | **yes** | no | partial | partial | no | no |
| what is on screen | partial | no | no | no | **yes** | **yes** |

Two readings of this matrix matter:

- **No single strategy is sufficient.** A full slave is **A + B + F** at minimum (messages, sim
  state, pixels), with C for race control and D for lifecycle.
- **The cheapest useful slave is not the most complete one.** A is presentation-only and B+F are
  positional; if the goal is "both players see the same race", A+F may suffice without B, because
  the sim's own computed positions were already shown to agree across runs when nothing random
  diverges ([RNG-DETERMINISM.md](RNG-DETERMINISM.md) §6, and the deterministic-field note in
  [NOTES.md](../NOTES.md) ~716).

---

## 5. The two structural obstacles no strategy removes

### 5.1 There is no clock setter, so "state" cannot be commanded

`SetSimTime`, `SetRaceTime`, `SetSessionTime`, `SetCurrentLap`, `SetLapCount`, `SetTimeRemaining`,
`SetFlags`, `SetWaterOnTrack` — **all zero occurrences** in the binary (**binary**). Only
`SetWeather` exists, and that is an asset/editor property, not a live command.

Every strategy above therefore works by **input substitution** (feed the sim what the host computed)
or **output substitution** (tell the sim's consumers what happened). None of them *instructs* the
sim. This is the one honest reason "slave" is the wrong word for what is achievable: the sim can be
**fed** and **lied to**, not **commanded**.

### 5.2 Damage, wear, temperature and confidence only arrive via events

Because `RaceSimCarIncident` / `RaceSimCarMechancialIssues` carry the damage model and their only
update paths in the reflective surface are `SetSave*` (serialisation) and the event bus, the vehicle
condition a player sees can only be moved by **firing the right events** (A) or by writing raw
memory that no supported API exposes (B extended).

This is why A and B are complementary rather than alternatives: A moves everything *semantic*,
B moves everything *spatial*.

---

## 6. What is genuinely new here versus already known

Useful to state, so the map is not mistaken for a re-description of existing work:

| finding | status |
|---|---|
| 119-member `ERaceSimEventType`, complete vocabulary | **new** (§2) |
| `CarAboutToBeTeleported` — the sim has a sanctioned teleport event | **new** (§2) |
| Full `Set*` audit: 13 `SetSave*`, **no clock setter at all** | **new** (§0, §5.1) |
| The GameMode is a 7-component presenter, not the sim | **new** (§1) |
| `RaceSimManager` named as the hub, with its initialisation delegate | **new** (§1) |
| Event payload accessors (`GetMechanicalFaultPayload` etc.) | **new** (§2.2) |
| Replay/seek is presentation, not state restore | already recorded (~line 1141) |
| Render is decoupled from sim state | already recorded (~line 1108) |
| Flags need live mask + history ring | already recorded (~line 1250) |
| Per-car offsets and stride | already recorded (~line 500); **unverified for this build** |

---

## 7. The unknowns, in priority order

1. **Is the event bus injectable, or listen-only?** Strategy A hinges on it. If a client's subsystems
   re-read their own state each tick, delegate substitution is cosmetic only. Cheap to answer in
   principle: subscribe, log, and see whether an injected event reaches the notification stack.
2. **Do the lifecycle events actually drive the state machine?** Strategy D. This is the only route to
   "start the session from outside" on the sim side, and it is the direct sim-side analogue of the
   UI-side dead end in [UI-COMMANDS.md](UI-COMMANDS.md) §3.5. Highest value if it works.
3. **Do the per-car offsets survive build 17356935?** They came from a foreign offset table; the notes
   already carry a warning that FormationLap's guard is size-based, not correctness-based.
4. **Is `m_raceSimManger` (the blueprint's field) the same object as the reflected
   `RaceSimManager`?** `unknown`; the typo'd field name suggests a distinct accessor.
5. **Whether `RaceSimTest*` exposes any *setter*.** It is a large struct family (Body, Floor, Engine,
   ERS, Gearbox, Driver, Car, Input, Strategy…) and, if any are Blueprint-callable, they would be the
   sanctioned way to author state — but the FModel export shows `funcs=0` for every RaceSim asset
   inspected, so the C++ functions behind them are invisible here.
6. **Whether writing the flag ring is enough**, or `RfState` (a state machine) must be driven too.
   Recorded as still-unproven at ~line 1430.

---

## 8. Bottom line

- **The sim can be made to follow a host, but not to obey one.** The absence of any clock or lap
  setter means there is no command interface; there is only an **input surface** (per-car fields,
  race-control setters) and an **output surface** (a 119-verb event bus plus delegates).
- **The single highest-value unexplored lever is the lifecycle events** (`SessionStart` →
  `WaitingForLightsOut*` → `StartingLightChange` → `RaceStart` → `ChequeredFlag*` → `SessionEnd`).
  They are the sim-side answer to the question that blocked the UI side, and nothing in the existing
  work has used them.
- **A complete slave needs three layers**, not one: semantic (A), spatial (B), and pixels (F).
  Race control (C) and lifecycle (D) are the two special cases that decide whether the session reads
  as *the same race* or merely *the same positions*.
- **Seek (E) should stay a diagnostic.** It is proven partial and proven to produce inconsistent
  sessions.

---

## Appendix — reproduction

```python
# the event vocabulary
import re
raw = open(r"E:\Steam\steamapps\common\F1 Manager 2024\F1Manager24\Binaries\Win64\F1Manager24.exe",'rb').read()
ev = sorted(set(m.group().decode() for m in re.finditer(rb"ERaceSimEventType::([A-Za-z0-9_]+)", raw)))
print(len(ev), "event types")

# the setter audit — this is the load-bearing negative result
for p in [b"SetSimTime", b"SetRaceTime", b"SetSessionTime", b"SetCurrentLap",
          b"SetLapCount", b"SetTimeRemaining", b"SetFlags"]:
    print(p.decode(), raw.count(p))            # all 0
print(len(set(m.group() for m in re.finditer(rb"SetSave[A-Za-z]{2,40}", raw))), "SetSave* names")
```
