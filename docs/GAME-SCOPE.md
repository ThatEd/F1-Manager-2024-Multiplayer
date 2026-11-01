# Is the entirety of F1 Manager 2024 the race sim?

**Short answer: no — and it is not close.** The race sim is one subsystem, roughly a quarter of the
UI and a tenth of the career data model. F1 Manager 2024 is a **career management game that contains
a race sim**, not a race sim with a menu attached.

This matters directly for the co-op design: everything in
[RACESIM-HIJACK.md](RACESIM-HIJACK.md) covers the part of the game that runs for ~90 minutes per
race weekend. It says nothing about the part that runs the other ~98% of a career.

Measured from the read-only FModel export and the shipped binary (build `17356935`):

```
E:\FModel\Output\Exports\F1Manager24\Content\   all content areas
E:\Steam\...\Win64\F1Manager24.exe              identifier counts
```

---

## 1. Four independent measures, same verdict

| measure | race sim | everything else | ratio |
|---|---|---|---|
| **Content size (data assets)** | `RaceSim` **2.1 MB / 227 files** | `Cinematics` 7870 MB, `Circuits` 7637 MB, `Data` 272 MB, `UITextures` 177 MB | sim is **0.01%** of content bytes |
| **UI code** | `raceWeekend` **346 / 1448** JS files (**23.9%**, 2,396 KB) | 1,102 files: cars 114, staff 102, finance 42, circuitInfo 33, sponsors 30, HQ 26, inbox 25, scouting 24, board 20, editors 19, preSeason 16, calendar 15 … | sim is **~24%** of the UI |
| **UI→engine event surface** | **85 / 352** events raised from `raceWeekend` (**24.1%**) | 267 events: CAS, editors, staff, contracts, scouting, sponsors, options, mail, HQ | sim is **~24%** of the API |
| **Binary identifier types** | sim families **241** (`RaceSim` 218, `RaceControl` 11, `Timing` 12) | career families **873** (Staff 321, Parts/Design 131, Contract 76, Building/HQ 64, Board 55, Mail 54, Sponsor 53, Scouting 48, Regulations 47, Finances 24) | career is **3.6×** the sim |

The three *code* measures agree at ~24% with unusual tightness (23.9 / 24.1 / 24), which is the
strongest signal here: the sim is consistently about a quarter of the interactive surface.

The *content* measure (0.01%) is misleading and I include it only to be contradicted: `Cinematics`
(7.9 GB) and `Circuits` (7.6 GB) are video and circuit geometry, not gameplay. **Data volume is not
feature volume.**

---

## 2. What the other 76% actually is

The UI module list is the clearest map of the game's real scope:

```
raceWeekend     346     <- the sim's UI
cars            114     car development, design, manufacture, warehouse
staff           102     contracts, scouting, development, pit crew
frontend         61     menus, CAS, scenarios, save/load
finance          42     sponsorship plans, obligations
circuitInformation 33
sponsorTargets   30
hq               26     buildings, upgrades, refurbishment
inbox            25     mail, dilemmas
scouting         24
fanfareModule    21
board            20     board objectives, confidence, reviews
editors          19     livery, logo, team branding
preSeason        16
calendar         15     the career clock
options          11
gpPrep           11
```

### The career database

`Content/Database` holds the **statement catalogue**: **26 `PSCollection` assets** (`*PS.json`, 276 KB
total) plus **336 schema diffs** (`Volta_NNNN_to_NNNN.json`) — the schema has been migrated 336 times,
which is a useful proxy for how much the career model has grown.

> **These files are not the schema, and they are not tables.** Each `*PS.json` is a `PSCollection`: a
> catalogue of named prepared statements. `StaffPS.json` holds 179 entries whose `Key` is a name the game
> calls (`AddStaffContract`) and whose `Value` is the SQL it runs
> (`INSERT OR REPLACE INTO Staff_Contracts (...) VALUES (?1 … ?12)`). There is no `StaffPS` table. This
> section previously read one file as one table, which is where the phantom names `StaffPS`, `PartsPS`,
> `TeamsPS`, `SavePS` and the rest came from; they were carried into the replication allowlist and the
> team-authority table list before audit §29.1 found it.

| catalogue | size | statements | catalogue | size | statements |
|---|---|---|---|---|---|
| `StaffPS` | **63,396 B** | 179 | `ScoutingPS` | 11,667 B | — |
| `SavePS` | 35,491 B | — | `RacesPS` | 9,651 B | — |
| `PartsPS` | 35,003 B | — | `RegulationsPS` | 8,892 B | — |
| `TeamsPS` | 20,900 B | — | `SponsorshipPS` | 7,392 B | — |
| `MailPS` | 13,799 B | — | `PlayerPS` | 7,155 B | — |
| `StandingsPS` | 12,798 B | — | `BoardPS` | 10,813 B | — |

Also `BuildingsPS`, `CalendarPS`, `FinancesPS`, `FanfaresPS`, `OnboardingPS`, `SeasonsPS`,
`SubTeamPS`, `EffectsPS`, `DifficultyPS`, `CountriesPS`, `MiscPS`, `NotificationsPS`,
`PartsAutoPS`. Together they hold **853 statements, 428 of which write** — 361 parameterised and 67
exec-visible.

**The real schema is `Volta.json`,** a `DatabaseAsset` whose `Data` property is a byte-for-byte SQLite
3 file: `user_version` **336**, **317 tables**, 2 views, seeded with 32 `Teams`, 446 `Staff_GameData`,
5 `Seasons`, 26 `Races_Tracks` and 1 `Player` row. `tools/schema_truth.py` decodes it, and
`coop-rs/crates/coop-mem/src/schema.rs` is derived from it.

**Note the shape:** `StaffPS` alone is 23% of the catalogue — nearly twice `PartsPS` and almost twice
`SavePS`. The single largest data model in the career is *people*, not cars and not the sim. There is
no sim table in the career schema at all; the sim's own state lives in the `Save_*` tables, which
exist only while a session is running.

**Careful — the catalogue is not the save.** The live career database is **in memory**, reached only
through the game's own SQL (prepare/bind at RVA `0x15116E0`, `sqlite3_exec` at `0x156D300`,
`sqlite3*` at `0x14156D300`/`0xF5754680` in the session recorded in [NOTES.md](../NOTES.md) ~line 1908),
and the `.sav` file is opaque compressed data with no readable SQL inside it (~line 1656). "26 catalogues"
is a statement about the game's data model, **not** about a file you can open; for the tables themselves,
count `Volta`'s 317.

---

## 3. What the sim is, relative to the whole

The sim owns the **race weekend** and nothing else:

- **Its UI**: `RaceWeekend.html` → `raceWeekend` module (346 files) — pre-session, practice,
  qualifying, race, strategy, pit options, post-session.
- **Its data**: `Content/RaceSim` (2.1 MB) — tyres, weather, incidents, faults, AI approach,
  accuracy, replay, notifications.
- **Its runtime**: `RaceSimManager` (+ `SimpleSimCars` for the non-simulated remainder), 119
  `ERaceSimEventType` verbs, and a per-second history ring for rendering.

Everything else — commissioning parts, negotiating contracts, hiring engineers, upgrading the
factory, satisfying the board, choosing sponsors, running the calendar, scouting drivers — is a
separate system that the sim neither reads nor writes.

**One nuance worth keeping in mind:** the sim is not only active during a race. `SimpleSimCars` sits
beside `RaceSimManager` in the save accessors and `SkipSessionData` carries
`DriverControlIncidentChancePerLap*` per track — so a reduced form runs for sessions you skip, and
for pre-race preparation. But that is still *within* the race weekend; it does not make the sim the
whole game.

---

## 4. Why this matters for the co-op design

1. **The hijack surface in [RACESIM-HIJACK.md](RACESIM-HIJACK.md) covers ~24% of the game.** It is
   the right 24% for "two players watch the same race", but a co-op *career* needs the other 76% to
   stay coherent too — and that layer is event- and database-shaped, not simulation-shaped.
2. **The career layer is far more tractable, not less.** Its state is 26 named tables reachable
   through the game's own `sqlite3_exec` (in-memory DB, proven live — [NOTES.md](../NOTES.md) ~1908),
   and its mutations travel as **267 named events** (CAS*, CustomTeam*, EditStaff*, On*StaffContract,
   Sponsor*, Parts*, Mail*, Options*). That is a much better co-op surface than the sim's read-only
   clock: a table can be read, diffed and shipped, and the existing work already does this
   (`coop-sql --swap-contracts 8 83`, idempotent, in a transaction). The sim has no such luxury
   (§5.1 of RACESIM-HIJACK: no `SetSimTime`).
3. **The asymmetry explains the earlier blockers.** `AdvanceToNextEvent` is a *career* verb (Team
   Management), `LoadGame`/`ContinueGame` are *career* verbs (front-end), and `PreSessionClosed` is a
   *sim* verb. The reason no UI event starts a race ([UI-COMMANDS.md](UI-COMMANDS.md) §3.5) is
   precisely that the UI is layered: the career layer hands over to the sim layer, and the handover
   is engine-internal.
4. **Budget accordingly.** If the goal is "two players race together", the sim is the whole problem.
   If the goal is "two players share a career", the sim is a quarter of it and the larger half is
   better instrumented.

---

## 5. The one-line answer, with its caveat

> No. F1 Manager 2024 is a **career management game with a race sim inside it**. By three
> independent code measures the sim is ~24% of the game; by data model it is one of 26 tables, and
> not a large one.

**Caveat on the numbers:** these are *surface* measures — file counts, identifier counts, table
sizes. They measure how much code and data each subsystem has, not how much player time it consumes
or how much complexity it carries. A race weekend may well be the most *dense* quarter of the game.
Nothing here says the sim is unimportant; it says it is not the whole.

---

## Appendix — reproduction

```powershell
# content areas by size
Get-ChildItem 'E:\FModel\Output\Exports\F1Manager24\Content' -Directory | ForEach-Object {
  $f = Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue
  [pscustomobject]@{ Area=$_.Name; Files=$f.Count; MB=[math]::Round(($f|Measure-Object Length -Sum).Sum/1MB,1) }
} | Sort-Object MB -Descending

# UI modules by file count
Get-ChildItem 'E:\FModel\Output\Exports\F1Manager24\Content\UIGameface\js\project\modules' -Directory |
  ForEach-Object { [pscustomobject]@{ Module=$_.Name
    Files=(Get-ChildItem $_.FullName -Recurse -File -Filter *.js | Measure-Object).Count } } |
  Sort-Object Files -Descending

# career DB tables
Get-ChildItem 'E:\FModel\Output\Exports\F1Manager24\Content\Database' -File -Filter '*PS.json' |
  Sort-Object Length -Descending | Select-Object Name, Length
```
