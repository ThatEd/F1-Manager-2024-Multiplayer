# RNG determinism: can two sessions draw the same random numbers?

Read-only investigation of whether F1 Manager 2024's race-sim random chances can be made to
produce **identical outcomes in two independent game sessions**, so two co-op clients see the same
race.

Evidence base (all read-only):

```
E:\FModel\Output\Exports\F1Manager24\Content\RaceSim\          227 files (113 .json + 113 .uasset)
E:\FModel\Output\Exports\F1Manager24\Content\UIGameface\js\    the UI (read-only DataStore view)
E:\Steam\...\Win64\F1Manager24.exe                             build 17356935, SHA-256 7ffe404fâ€¦b2050f6
C:\Users\<you>\AppData\Local\F1Manager24\Saved\SaveGames\*.sav  GVAS saves
```

Same evidence rules as [UI-COMMANDS.md](UI-COMMANDS.md):

| tag | meaning |
|---|---|
| *(no tag)* | **Proven from the asset or file at the cited path.** |
| **binary** | **Proven from `F1Manager24.exe`** at the cited file offset. |
| **inferred** | Reasoned from the above, without executing anything. |
| **unknown** | Not established by this evidence. |

**Nothing below was run.** No live race was sampled. Every claim about what happens *at runtime* is
`inferred` and labelled as such.

---

## 0. Answer first

**Yes on paper, with one caveat â€” every "chance" in the race sim is a table-valued *input* evaluated
against the sim state, not a stored roll.** There is no RNG state in the save and no seed anywhere in
the assets. *Caveat:* `MultiplierChanceTable` carries only an `m_max` per level with no `m_min`
(Â§1.1), so whether that value is a fixed ceiling or the top of a random draw is `unknown`; if it is a
draw, there is one hidden roll per car per lap that the data does not describe.

**No in practice, not by editing the probability data.** Two sessions differ because the sim's
random stream is seeded per process (**inferred**, Â§3) and â€” more importantly â€” because the
probabilities are *functions of live state that itself differs between runs*: driver confidence,
tyre temperature, wear, and where each car is on track. Setting `Chance = 1.0` everywhere does not
make two sessions agree; it only makes both of them always fire, at different moments.

**The viable route is not to change the odds, but to replay the same rolls** â€” either by pinning the
RNG (Â§5.1), or by recording the *decisions* (not the rolls) and replaying them (Â§5.2). Â§5.2 is what
I'd build; what *not* to attempt is Â§5.3, and forcing the odds (the idea that looks obvious) is
dispatched in Â§4.

A useful, cheap measurement comes first (Â§6): the per-lap incident probability is **exactly 0** on 22
of the 26 real circuits, which suggests the race-relevant randomness may be limited to the
mechanical-fault system and the pre-race accuracy draw.

---

## 1. Where the randomness lives â€” complete inventory

The RaceSim content is 113 JSON assets. Every "chance" field found, by asset:

### 1.1 `Incidents/RaceIncidentsDataAsset.json` â€” driver-control incidents

| field | value | meaning |
|---|---|---|
| `MultiplierChanceTable` | `NoChance {m_max 0.0}`, `VeryLowChance 0.5`, `LowChance 0.9`, `MediumChance 1.2`, `HighChance 1.5`, `VeryHighChance 2.5` | maps the car's incident-risk level to a **chance multiplier** |
| `IncidentSettings[n]` | per-incident spatial triggers (`FromNodeType`/`ToNodeType`, `MinimumSpeedThresholdKMPH`, `MinimumCurvatureThreshold`) | **where** an incident may occur, not how likely |
| `FaultPropogationConfig` | e.g. Gearbox/Critical â†’ Engine `ChanceToAffect 0.66`, Body `0.5`, SidePods `0.5`, each with `ConditionReduction {X,Y}` | a second roll, per damaged neighbouring part |
| `LossOfGripDeceleration`, `CollisionDamageData`, `LockupData`, `RunWideData`, `CrashData`, `CornerIncidentsData` | physical consequences | not probabilities |

`MultiplierChanceTable` is the key: the car's *stated* incident risk is a **multiplier on a
per-lap base probability**, not the probability itself.

Two things about that table are worth flagging. First, the enum it keys on is
**`EIncidentChance`** â€” `NoChance, VeryLowChance, LowChance, MediumChance, HighChance,
VeryHighChance, Count` â€” which is a **different, native enum** from the UI's `EChanceOfIncident`
(see Â§1.8). **binary**, eight occurrences of `EIncidentChance`, at `0x5f6e3f0` onward; the full
value list is visible at `0x5f6e490`. Second, **every entry carries only `m_max`, with no `m_min`**.
If the multiplier is a random draw in `[0, m_max]` rather than a fixed ceiling, then there is an
*additional roll per lap* that this evidence cannot see â€” see Â§7 gap 5.

### 1.2 `RaceTrackDataAsset.json` â€” per-track bases (26 circuits + 2 sentinels)

Each of the 28 `TrackDataMap` entries carries:

| field | AlbertPark | Monaco | note |
|---|---|---|---|
| `SkipSessionData/DriverControlIncidentChancePerLapMin` | 0.0 | 0.0 | **see Â§6** |
| `SkipSessionData/DriverControlIncidentChancePerLapMax` | 0.0 | 0.0 | |
| `FeederSeriesData/IncidentChanceModifier` | 1.0 | **1.5** | Monaco 1.5Ã—, Jeddah 1.25Ã— |
| `TrackIncidentDirection/m_openingLapCrashChance` | **0.1** | 0.1 | all 28 tracks |
| `TrackIncidentDirection/m_openingLapCrashCornerCollisionSkew` | 0.5 | 0.5 | all 28 |
| `TrackIncidentMultipliers/m_puncture` | 1.0 | 1.0 | |

Only **4 of the 26 real circuits** are non-zero â€” Shanghai, PaulRicard, SpaFrancorchamps, Sochi â€” and
at those the values are `Min 0.001 / Max 0.0001`, so **Min > Max**; `unknown` which one binds. The
other 22 entries are all `0.0 / 0.0`. Two further non-zero entries are the enum sentinels
`ERaceTrack_MAX` and `ERaceTrack::INVALID`, not circuits; they carry the same `0.001/0.0001` pair and
should be ignored when reasoning about real tracks. (28 `TrackDataMap` entries = 26 circuits + 2
sentinels.)

### 1.3 `Tyres/TyreWear/PunctureChancePerLap.json` â€” wear-keyed curve

| wear time | value |
|---|---|
| 0.0 | 0.25 |
| 0.2 | 0.075 |
| 0.21 | 0.03 |
| 0.22 | â‰ˆ0 |
| 1.0 | â‰ˆ0 |

A puncture is 25%-per-lap at *zero* wear and ~0 beyond 22% wear â€” strongly `inferred` to be keyed on
**temperature** rather than wear (the file sits under `TyreWear` but the curve argues otherwise).

### 1.4 `Incidents/Distributions/*.json` â€” 4 outcome curves

`CrashDistribution`, `CornerCollisionDistribution`, `RacingIncidentDistribution`,
`MechanicalFailureDistribution`. Each is a single `FloatCurve` on `[0,1]`, used to scale the
consequence of an incident:

| curve | keys (time â†’ value) |
|---|---|
| `CrashDistribution` | 0.0â†’0, 0.3â†’0.10, 1.0â†’0.95 |
| `CornerCollisionDistribution` | 0.0â†’0, 0.5â†’0.35, 1.0â†’0.95 |
| `RacingIncidentDistribution` | 0.0â†’0, 1.0â†’0.95 |
| `MechanicalFailureDistribution` | 0.0â†’0, 0.2â†’0.40, 1.0â†’1.00 |

The mechanical one is the odd one out: it reaches 1.0 (a guaranteed failure at maximum score) and
climbs earliest, at 0.2.

### 1.5 `Incidents/DT_{Engine,ERS,Gearbox}Faults.json` â€” fault selection and escalation

Per fault row (e.g. `TemporaryPowerReduction`):

```
TriggerScoreMinMax        {X:40, Y:60}
TriggerWeightMinMax       {X:3.0, Y:1.0}      <- selection weight
EscalationTarget          "MajorPowerReduction"
EscalationChanceMinMax    {X:0.0, Y:1.0}      <- a random chance
EscalationTimeMinMax      {X:300, Y:1800}     <- a random duration
DeescalationChanceMinMax  {X:1.0, Y:1.0}
DeescalationTimeMinMax    {X:120, Y:300}
```

So even a *chosen* fault gets a random escalation chance and random timers.

### 1.6 `DriverConfidenceDataAsset` â€” the feedback loop (mostly *not* random)

`DriverConfidenceDataAsset` looks like a randomness table but is **event-driven**, which matters
because it means confidence does *not* diverge on its own:

- `ConfidenceEventModifiers` are keyed on **named sim events** â€” `SuccesfullOvertake` (+6.0 race),
  `Spin` (âˆ’15.0 race), `Lockup` (âˆ’8.0), `CollisionWithBarrier` (âˆ’25.0), `NewBestLap` (+2.0),
  `FlyingLapComplete` (+1.85 quali) â€” with `PracticeModifier`/`QualifyingModifier`/`RaceModifier`.
- `TrendRatePerMinute` Â±2.5 (race), Â±0.75 (qualifying).
- Only `LowConfidenceIncidentChanceMultiplierIncrement` 0.0154 and
  `HighConfidenceIncidentChanceMultiplierIncrement` 0.0081 and `PeakStateIncidentChanceMultiplier` 0.5
  touch probability.

So confidence is a **deterministic integrator over incident events**: no incidents â†’ no divergence.
But it is also a **positive feedback loop** â€” a spin lowers confidence, which raises the incident
multiplier, which makes the next spin more likely. One early divergence amplifies.

### 1.7 The accuracy draw â€” the dominant non-incident variation

`Accuracy/DriverAccuracyLapVariation.json` = 0.125 (below driver stat 0.7, falling to 0.01 at 1.0);
`DriverAccuracyCornerVariation` 0.035; `DriverAccuracyLapMeanAccuracy` 0.4â†’1.0 across the stat range.
`SimulateRaceDataAsset.FaultEscalationChanceMin` 0.15/0.2.

A **12.5% lap-time variation** is far larger than any incident probability in these assets. This is
the dominant run-to-run difference, and it is driven by a driver stat, not by a discrete dice roll â€”
so it may well *already* be reproducible across two sessions with the same save, unlike the incident
path.

### 1.8 The multiplier chain, proven end to end

`chanceOfIncident` is a per-car, per-lap DataStore property:

- Schema: ``js/project/data/RaceSim.js:133``
  `chanceOfIncident` is a direct child of the `PlayerCar` ContextList opened at
  ``RaceSim.js:8``
  (`id_prefix: 'PlayerCar', generator_count: 2`) â†’ **`['RaceSim','PlayerCar'+N','chanceOfIncident']`**.
- Enum `EChanceOfIncident`: `NoChance 0 â€¦ VeryHigh 5`
  (``GameTypes.js``, search `var EChanceOfIncident`).
  **This is the UI's mirror enum, not the engine's.** The tables in
  `RaceIncidentsDataAsset` key on the native `EIncidentChance` (Â§1.1), which has different member
  names (`VeryLowChance` vs `VeryLow`). **binary**: `EIncidentChance` has 8 occurrences in the exe;
  `EChanceOfIncident` has **0** â€” the UI name exists only in JS.
- It is **read-only to the UI** â€” the UI only renders it as a dial
  (``MechanicalFaultComponents.js:35-43``).
  **Every write is engine-side.**
- `MultiplierChanceTable` (above) turns that enum into 0.0â€“2.5Ã—.
- `TrackIncidentMultipliers` and `FeederSeriesData/IncidentChanceModifier` scale it per track.

So the *odds* are a pure function of (track, car risk enum, car state) â€” **all table-driven, no
stored roll.** That is the "yes on paper" half of the answer.

---

## 2. The save carries no RNG state

`autosave.sav` (6,063,306 bytes, GVAS) contains **no** sim runtime state:

| probe | hits in save |
|---|---|
| `raceTime`, `SimTime`, `simTime` | 0 |
| `currentLap`, `sessionState`, `isSimulating` | 0 |
| `playbackRate`, `qualifying1Cutoff` | 0 |
| `waterOnTrack`, `trackTemperature`, `Weather` | 0 |
| `RaceSimManager`, `RaceSimData`, `RaceSimState` | 0 |
| `RandomSeed`, `Seed`, `RandomStream`, `RNG` as a *property* | 0 |

The only `RaceSim` strings present are `RaceSimDifficulty` / `m_aiPerformance` (a settings struct).
Five incidental `RNG`/`rng` byte matches exist but are binary noise inside compressed blocks, not
property names â€” the save *does* store readable UE property names elsewhere (`RaceSimDifficulty`,
`m_aiPerformance`), so their absence is meaningful.

**Conclusion:** a mid-race save does not persist the sim's position in a random stream. Whatever
seeds it is created fresh when the session starts. `inferred`: there is no "resume the same rolls"
lever in the save, and copying a save between machines does not carry randomness with it. That is
consistent with [UI-COMMANDS.md](UI-COMMANDS.md) Â§5 item 5 â€” `raceTime` and `sessionState` have no UI
writer either, they are engine-owned and unsaved.

---

## 3. Where the seed lives: the engine, and nowhere else

The JS export contains **no gameplay RNG**. The only `seed`/`random` hits in the entire 1,448-file UI
export are inside `common/lib/lottie.js` â€” the animation library
(``lottie.js:654-694``,
`seedrandom`/`autoseed`), used for decorative motion. Nothing in `project/modules/raceWeekend/`
draws a random number.

The exe likewise shows **no game-level RNG type**:

| pattern | hits | what they are |
|---|---|---|
| `RandomStream` | 7 | UE **Blueprint** math nodes (`MakeRandomStream`, `BreakRandomStream`, `ResetRandomStream`, `SeedRandomStream`) |
| `RandomSeed` | 14 | Niagara particles, Wwise (`SetRandomSeed@SoundEngine`), ISM instancing |
| `Deterministic` | 2 | Blueprint `RandomFloatDataflowNode::Deterministic`; MovieScene `DeterministicSequenceID` |
| `FixedTimeStep` / `FixedTick` | 2 / 2 | **physics** asset and **Niagara**, not the race sim |
| `FRandomStream`, `mt19937`, `xorshift`, `Pcg`, `MersenneTwister` | **0** | â€” |
| `PerformanceVariation`, `FormVariation`, `DriverForm`, `IncidentRoll`, `ChanceRoll` | **0** | â€” |

**inferred (and this is the honest limit):** the race sim's random source is either an internal
implementation with no reflectable name, or an unremarkable engine one reached indirectly, so it
cannot be located by name. The absence of *any* game-level RNG symbol in both the UI and the binary's
reflective strings is strong evidence that randomness is created inside the sim manager
(`RaceSimManager` â€” **binary**, 6 occurrences, e.g. `GetSaveRaceSimManager` at `0x5ead400` /
`SetSaveRaceSimManager` at `0x5ead7d8`) and is **not exposed as a tunable asset**. The presence of
save/load accessors for the manager is itself worth noting: the sim manager *is* a save-facing
object, yet no RNG state appears in the save (Â§2).

Practical reading: there is no data asset to edit that changes the seed. If the seed is
`unknown`-sourced (time, GUID, process entropy), then **two sessions will not agree by default**, and
no asset edit changes that.

---

## 4. The tempting idea that does not work: force the odds

"Set every chance to 1.0 so the same things always happen." Three independent reasons this fails:

1. **Chance 1.0 is not synchronisation, it is saturation.** If every car is guaranteed an incident
   every lap, *when* each fires still depends on which car reaches which trigger node first and what
   its `ConditionReduction`/`ChanceToAffect` roll was â€” i.e. still on the differing state and the
   differing stream. Two sessions would both crash constantly but differently.
2. **Most chances are already effectively 0 or state-gated.** `DriverControlIncidentChancePerLapMin/Max`
   is 0.0 on 22 of 26 real circuits (Â§1.2); the puncture curve is â‰ˆ0 past 22% wear;
   `IncidentSettings` requires a specific `FromNodeType â†’ ToNodeType` transition at a minimum
   speed/curvature. Raising a multiplier on a spatially-gated trigger does not create determinism.
3. **It destroys the thing you are trying to sync.** The goal is both clients seeing the *same race*,
   not a race with no randomness. Overriding the tables changes the simulated outcome on both sides
   identically *in distribution* but not *in realisation* â€” the worst of both worlds.

Also note `chanceOfIncident` is not even writable from the UI: `DS.setValue` asserts on
`isCoherentPlayer()` and throws in the shipped build ([UI-COMMANDS.md](UI-COMMANDS.md) Â§1.3b), and the
UI only reads the property anyway.

**Verdict: not viable as a `DS.setValue` change, and not viable as a pak data edit.**

---

## 5. What would work: replay the decisions, not the rolls

The important structural fact is that **only the racing line is deterministic today.** The prior
work in [NOTES.md](../NOTES.md) already recorded that the car **field** reproduces exactly:

> "Grid values identical to the previous run (car1 43.825/-9.000, car2 67.825/-1.900 â€¦), so the field
> is deterministic." â€” [NOTES.md](../NOTES.md) ~line 716

That is v1 of the problem already solved: positions and heading are reproducible. What is *not*
reproducible is the decision layer â€” incident triggers, fault selection, escalation, and the
accuracy variation that feeds them.

Two routes, in increasing cost:

### 5.1 Pin the stream (cheap if a seed is reachable â€” currently `unknown`)

If the sim manager's RNG accepts a seed, set the **same seed on both clients** and the rolls agree
exactly, with no protocol at all. This is the ideal and it is **unproven**: no RNG object was found
by name (Â§3), so the first step is a runtime search for a seed-like field in the RSM block (the same
memory the earlier work already reads for `SimTime`). **This is the one experiment worth doing first**
because the payoff is total and the cost is one memory scan.

### 5.2 Capture and replay the decisions (works without a seed)

Because every chance arrives through a small number of *discrete, named* outcomes, intercepting the
decisions is tractable where intercepting the float stream is not:

| what to capture | granularity | from the asset evidence |
|---|---|---|
| incident fired: `(carID, incidentType, lap, trackNode)` | per incident, rare (Â§1.2) | `IncidentSettings[n]`, `EChanceOfIncident` |
| fault chosen: `(carID, faultName)` + `EscalationChance` result + timers | per fault | `DT_*Faults`, `TriggerWeightMinMax` |
| propogation: `(fromPart, toPart, affected?)` | per fault, rare | `FaultPropogationConfig.ChanceToAffect` |
| lap-time accuracy offset per car per session | one value per car | `DriverAccuracyLapVariation` |

The host records that list; the client applies it instead of rolling (force "did fire" = recorded
bool, force chosen fault = recorded name, force timer = recorded value). This is the same
**effect-capture** shape the playbook already recommends for narrative/effect sync, and it is
exactly one small table:

```
{ seed_or_id, events: [ {tick, car, kind, payload} ... ] }
```

**Why this is the right shape:** the per-lap incident chance appears to be 0 on most tracks (Â§6), so
the real event rate may be very low â€” a handful of rows per race. A replay log that is usually empty
is cheap to ship.

### 5.3 Do not attempt

- Replaying the float stream itself (needs the exact RNG algorithm and call order â€” `unknown`).
- Syncing the outcomes by forcing `chanceOfIncident` per car, because the *UI cannot write it* and the
  underlying base is per-track, not per-car.
- Forcing a mid-race save to carry the stream, because the save does not contain it (Â§2).

---

## 6. The cheap measurement to do before building any of this

Â§1.2 shows `DriverControlIncidentChancePerLapMin`/`Max` = 0.0 on **22 of the 26 real circuits**, with
`Min 0.001 / Max 0.0001` on only 4 (Shanghai, PaulRicard, SpaFrancorchamps, Sochi) â€” **Min greater
than Max**, which is almost certainly an authoring mistake or a curve read in reverse. Two questions
decide how big the problem really is:

1. **Is the per-lap driver-control incident probability actually 0 in normal running?** If yes, the
   race-day randomness is limited to mechanical faults (wear/temperature driven), the opening-lap
   crash (`m_openingLapCrashChance` 0.1, all tracks), and the accuracy variation â€” a much smaller
   surface than "the whole sim is random".
2. **Do two sessions with the same save and the same inputs diverge at all before the first
   pit stop?** If the field is deterministic ([NOTES.md](../NOTES.md)) and nothing rolls until wear
   builds, the first N laps may already match.

Both are answerable with the recorder that already exists, without writing any sync code.

---

## 7. Gaps â€” what this could not establish

1. **No runtime observation.** No race was run for this note. The event *rate* is `unknown`; only the
   table values are proven.
2. **The seed/source is `unknown`.** Not findable by name in the UI or the exe's reflective strings;
   would need a memory search of the `RaceSimManager` block.
3. **`DriverControlIncidentChancePerLapMin > Max`** on 5 tracks â€” `unknown` which bound applies, or
   whether the pair is a curve domain rather than a range.
4. **Whether `chanceOfIncident` is recomputed per lap or per session** â€” `unknown`. If per session, its
   influence on a single race is a constant and the divergence comes only from the accuracy draw.
5. **`MultiplierChanceTable` entries have only `m_max` and no `m_min`.** If the multiplier is drawn
   uniformly in `[0, m_max]` per evaluation rather than used as a fixed ceiling, there is a *per-lap
   random draw* whose scale this evidence shows but whose semantics it does not. **This is the single
   most important remaining unknown for Â§5.2**, because it decides whether the incident path has one
   hidden roll per car per lap. `unknown`; the use site is native.
6. **The pre-race `SimulateRaceDataAsset` is a separate path.** Its `FaultEscalationChanceMin` and
   `MajorCollisionMaxDamagedParts` govern the *pre-simulated* sessions (quick-sim), not the live race.
   Do not confuse the two when tuning: quick-sim outcomes are not the live race's outcomes.
7. **`EChanceOfIncident` (UI) vs `EIncidentChance` (native) are distinct enums** with different
   naming. The UI never sends either to the engine, and no UI code maps one to the other â€” so how the
   engine's `EIncidentChance` per car is assigned is **unknown** (it is not derived from the UI's
   `chanceOfIncident`, which is read-only display state).
8. **`RaceSimEventsDebugSystem`** appears in the UI's debug enum list but has 0 occurrences in the exe
   string table â€” possibly stripped, possibly constructed. `unknown`.

---

## Appendix â€” reproduction

```python
# every chance-like field in the RaceSim content, with the asset that owns it
import json, os, re
root = r"E:\FModel\Output\Exports\F1Manager24\Content\RaceSim"
for dp,_,fs in os.walk(root):
    for fn in fs:
        if not fn.endswith('.json'): continue
        d = json.load(open(os.path.join(dp,fn), encoding='utf-8'))
        def flat(o, p=""):
            if isinstance(o, dict):
                for k,v in o.items(): yield from flat(v, p+"/"+k)
            elif isinstance(o, list):
                for i,v in enumerate(o): yield from flat(v, p+f"[{i}]")
            else: yield p, o
        for pth, val in flat(d):
            if re.search(r'chance|Chance|variance|Variance|MinMax|Random|random',
                         pth.rsplit('/',1)[-1]):
                print(f"{fn:38} {pth:58} {val!r}")

# the save carries no sim state
raw = open(r"C:\Users\<you>\AppData\Local\F1Manager24\Saved\SaveGames\autosave.sav",'rb').read()
for probe in [b"raceTime", b"sessionState", b"isSimulating", b"RandomSeed", b"RandomStream"]:
    print(probe, raw.count(probe))
```
