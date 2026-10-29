# Career co-op: the sim is 24% of the game, so the other 76% needs a plan

[GAME-SCOPE.md](GAME-SCOPE.md) establishes that the race sim is ~24% of F1 Manager 2024 by three
independent code measures, and that the career layer is 3.6× larger than the sim in the binary.
[RACESIM-HIJACK.md](RACESIM-HIJACK.md) maps how to slave the sim. **This document maps the other
three layers**, which is where a co-op career actually lives.

On paper. No live tests. Evidence tags as before: *(no tag)* proven from the cited file,
**binary** proven from the exe, **inferred** reasoned, `unknown` not established.

---

## 0. The answer

**Yes — the career layers follow the same principle**, and they follow it *better*, because they have
three mechanisms the sim lacks:

| the sim has | the career layer has |
|---|---|
| no `SetSimTime` (§5.1 of the hijack map) | **a live `sqlite3_exec`** — arbitrary SQL, already proven |
| a per-process RNG with no seed | **no randomness in the mutating path** — data-driven and deterministic |
| state that is not persisted | **26 named tables** that *are* the state |
| 119 events that mostly report | **267 events that mostly mutate** |

So the right shape is not "hijack the career like we hijacked the sim". It is:

> **The sim needs state injection (input substitution). The career needs a replicated transaction
> log.** Different mechanism, same direction: host authoritative, client follows.

The one thing that does **not** follow is the *goal*. Slaving the sim makes two players watch the
same race. Slaving the career makes two players *share* one. Those are different products and they
need different rules — §5.

---

## 1. The four layers, and what each one is actually made of

| layer | what it is | state | mutation path | how a client follows |
|---|---|---|---|---|
| **L1 Race sim** | the race weekend | engine memory, unsaved | `ERaceSimEventType` + per-car fields | [RACESIM-HIJACK.md](RACESIM-HIJACK.md) strategies A/B/F |
| **L2 UI** | Gameface web view, 1,448 JS files | DataStore, engine-owned | `engine.trigger` (352 events) | [UI-COMMANDS.md](UI-COMMANDS.md) — but see §2 |
| **L3 Career model** | 26 tables | **in-memory SQLite** | game's own `sqlite3_exec` (RVA `0x156D300`) | replicated transaction log (§4) |
| **L4 Front-end** | menus, CAS, save/load | DataStore + files | `LoadGame`, `ContinueGame`, CAS*, `QuitGame` | document swap (§3) |

The layering is real, not a diagram convenience: the three `.html` entry points load disjoint
bundles, and `AdvanceToNextEvent` (a career verb) hands over to `PreSessionClosed` (a sim verb) through
an engine-internal transition the UI cannot see ([UI-COMMANDS.md](UI-COMMANDS.md) §3.5). That handover
point is where a career sync must not break.

---

## 2. L2 — the UI layer: follow, but mostly do not write

**The principle carries over, with a twist that changes the design.**

[UI-COMMANDS.md](UI-COMMANDS.md) §1 established that `engine.trigger` is a single choke point, so it
looks like the obvious place to intercept. But §1.3b of the same document found that `DS.setValue`
asserts on `isCoherentPlayer()` and **throws in the shipped build**. Consequence:

> **From the UI you can raise events, but you cannot write state.** Every `if (Engine.isCoherentPlayer())`
> block in the UI is dead code in the game.

So the UI layer is **read-mostly** for co-op purposes:

| direction | mechanism | status |
|---|---|---|
| client reads career state | `DS.getValue` / `DS.getPropertyListener` through the bridge | works today |
| client *raises* an intent | `engine.trigger(name)` via the bridge | works today (proven: `ContinueGame`, `LoadGame`, `Advance`) |
| client *writes* state | `DS.setValue` | **throws** — not available |
| client receives engine-driven changes | `engine.on` / `Engine.addListener` | works (`SetRaceView`, `SetManagementScreenID`, `SetFrontendScreenID`) |

**Therefore the UI is a sensor and a button, not an actuator.** That is exactly the right role for a
client: it is what a remote player should be able to do anyway. The three engine→UI channels at
[UI-COMMANDS.md](UI-COMMANDS.md) §3.6 (`SetRaceView`, `SetManagementScreenID`, `SetFrontendScreenID`)
are the *correct* hook for knowing the host moved the client somewhere — and they are invisible to a
wrapper around `engine.trigger`, so a design that only wraps `trigger` will miss them.

**One asymmetry to build in deliberately.** 267 of the 352 events are raised outside `raceWeekend`;
most are local-editing verbs (`EditStaff*`, `LiveryEditor_*`, `CustomTeam*`) that mutate the career.
If a client raises one of those and the UI cannot write the result, the client's UI will show a change
the engine never accepted. `unknown` whether any of them are honoured engine-side from a client; the
safe assumption is **no**, so L2 intents from a client must be *forwarded to the host* rather than
applied locally (§4.3).

---

## 3. L4 — the front-end: it is a document swap, and it is already solved

This layer is the smallest and the best understood. The rule is proven and live-confirmed:

> `LoadGame` and `ContinueGame` are **front-end-only** events. Firing either from inside a career
> leaves a black screen with the process alive ([LOBBY-GATE.md](LOBBY-GATE.md)).

The co-op consequence, already in [LOBBY-GATE.md](LOBBY-GATE.md) and now enforced twice in code by the
headless client ([tools/headless/README.md](HEADLESS-HARNESS.md)):

1. every client returns to the front-end (`QuitGame`, wired as bridge `menu`);
2. the host pushes the save;
3. everyone loads together;
4. the session then advances with `AdvanceToNextEvent` from inside the career.

**L4 needs no new mechanism.** It needs the lobby to gate on `page=Frontend`, which both the server
(`take_for`) and the bridge (`allowedFor`) already do. The only untested link is `QuitGame` itself
([LOBBY-GATE.md](LOBBY-GATE.md), "still unproven").

`unknown`: whether the front-end → career → front-end cycle can be repeated indefinitely in one
process, or whether state accumulates. Nothing in the export answers that.

---

## 4. L3 — the career model: the real work

This is the 76%, and it is where the design effort belongs.

### 4.1 What the layer actually is

- **Schema:** 26 tables in `Content/Database/*PS.json` (276 KB), plus **336** schema migrations.
  Largest is `StaffPS` at 23% of the schema (people, not cars — see [GAME-SCOPE.md](GAME-SCOPE.md) §2).
- **Live DB:** **in memory**, no file, no pager image that maps to it. Reachable **only** through the
  game's own `sqlite3_exec` at RVA `0x156D300`, with the `sqlite3*` found by signature and validated
  by `db+0x20 == db+0x2A0`, `*(i32*)(db+0x28) == 2`, and `pager->zName == ":memory:"`
  ([NOTES.md](../NOTES.md) ~1908).
- **Proven capable of:** arbitrary `SELECT`, idempotent transactions, and real writes —
  `coop-sql --swap-contracts 8 83` produced a verifiable swap. A bad statement
  (`INSERT INTO Board_TeamRating` with an unknown `SeasonID`) returned **SQLite error 19 and the game
  was fine** (~1972), which is evidence the write path fails safely.
- **Not in the save:** the `.sav` is opaque compressed data (entropy 7.99–8.00, no `SQLite format 3`,
  no `CREATE TABLE`) — [NOTES.md](../NOTES.md) ~1656. So you cannot diff two saves to sync a career; you
  must go through the live DB.

### 4.2 Why this layer is *better* for co-op than the sim

Four structural advantages, each traceable to a finding:

1. **It has an actuator.** The sim has no setter; the career has `sqlite3_exec`. One handle, arbitrary
   SQL, already demonstrated.
2. **It has no randomness in the mutating path.** [RNG-DETERMINISM.md](RNG-DETERMINISM.md) found the
   randomness concentrated in the *race sim* (incidents, faults, accuracy). Career mutations
   (a contract, an upgrade, a hire) are data operations. `inferred`: so the same SQL on two machines
   produces the same state, which the sim never does.
3. **State is addressable.** A race is a continuous trajectory you must sample at 43 Hz. A career is
   26 tables you can key by primary key and diff by row.
4. **It fails safely.** The error-19 result shows a rejected statement does not corrupt the session —
   important when the writer is a network peer.

### 4.3 The mechanism: replicate transactions, not state

**Do not ship table snapshots.** The DB is in-memory and large; `StaffPS` alone is 23% of the schema
and a full career has thousands of rows. Ship the *statements*.

```
host                                    client
────                                    ──────
player acts  ──►  game emits event (267 career verbs)
                  │
                  ├─► host applies it natively (the player's own click)
                  ├─► host captures the SQL the game prepared
                  └─► log { seq, tick, table, pk, sql, hash }

                        ──► replicate log ──►  client applies the SAME sql via its own sqlite3_exec
                                               then re-reads the row to confirm
```

Why the statement and not the row: the game's own SQL already encodes the exact semantics
(`UPDATE Contract SET TeamID=... WHERE ...`), including cascades and triggers. Re-deriving that from a
row image is how you get divergent careers.

**What the host must capture.** `sqlite3_exec` is a poor hook for *capture*: it runs arbitrary
batched SQL and does not tell you which table changed. The better hook is **prepare**, which yields
the statement text plus its bind array — a structured, already-parameterised record.

The notes propose exactly this ("Find the prepare/execute call site and call it with our own SQL",
~1878). An earlier `pre-sqlite3_prepare` lead was discarded at ~1854, but note **why**: the string it
matched lived in `AppXDeploymentClient.dll`, "a Windows component - a false positive". That is a
false positive, not a finding that prepare is the wrong hook — so prepare-level capture remains
**genuinely unexplored**, which is why it is step 3 of the plan in §7.

**Ordering and idempotency.** Two facts make this tractable:
- the DB writes are already wrapped in transactions (`coop-sql` does this), so a log entry maps to one
  atomic unit;
- the contract swap was made **idempotent** and is described as such. Idempotency is a requirement, not
  a nicety: a client that applies the same entry twice must not double a contract.

**Conflict policy — the hard question, stated plainly.** Two players *sharing one career* will both
want to act. Three coherent options:

| model | who acts | fits |
|---|---|---|
| **1. Single-actor (host plays, client watches)** | host only | simplest; client is a spectator with a UI. Zero conflicts. |
| **2. Split-team (each client owns its own team's rows)** | each client owns `TeamID = X` rows | matches the existing ownership design in [LOBBY-GATE.md](LOBBY-GATE.md) ("the host's career is the world", `UPDATE Player SET TeamID`) |
| **3. Shared (both act on the same team)** | both | needs row-level conflict resolution; not recommended |

**Recommendation: model 2.** It reuses the ownership idea already recorded, needs no merge logic,
and the only shared rows are the ones both players legitimately read (`RacesPS`, `StandingsPS`,
`CalendarPS`) — and those are engine-computed, so only the host's copy is authoritative.

---

## 5. What "the same principle" must NOT mean

The sim plan is *input substitution*: feed the client's sim the host's numbers and let it compute.
Applying that idea naively to the career breaks in three specific ways, and these are the design's
real traps.

### 5.1 Do not submit intents as events and hope the engine agrees

The UI cannot write state (§2). If both clients raise `AdvanceToNextEvent`, both advance their own
career clock — and since the career clock is *engine-side*, the two careers silently diverge with no
error. `AdvanceToNextEvent` is **start/stop**, not idempotent ([UI-COMMANDS.md](UI-COMMANDS.md) §3.3):
a second press means STOP. So two clients pressing "advance" is not a race condition you can retry,
it is a state machine being fed contradictory inputs.

### 5.2 Do not let both clients run career code

Every career mutation has two halves: the UI raises an event, and the engine writes the DB. If both
clients raise the event, both engines write — to *different* in-memory DBs that never reconcile.
There is no mechanism in the evidence for merging two in-memory SQLite databases. So exactly **one**
client may execute a given mutation. That is why §4.3 has the host apply natively and the client
*replicate*, not re-decide.

### 5.3 Do not assume the career DB survives a race weekend

The handover at L3→L1 is engine-internal. `RACESIM-HIJACK.md` §5.1 notes the sim has no clock setter
and the save does not carry RNG; `NOTES.md` ~1908 notes the DB is in memory with no file. `unknown`:
whether the DB handle is stable across the career→race→career transition, or is rebuilt. **This is the
single most important unknown for a career co-op design** — if the handle moves, a client must
re-discover it after every race, exactly as the sim's `RaceSimManager` address was found to move
between sessions ([NOTES.md](../NOTES.md) ~1513: "the live address **moves between sessions**
(`0x1750E0000` earlier, `0x173530000` now), so caching is only ever a cache").

---

## 6. Coverage matrix: career layers

| career state | L2 UI event | L3 SQL | L4 document | tractability |
|---|---|---|---|---|
| calendar / career clock | `AdvanceToNextEvent`, `NextMonthSelected` | `CalendarPS` | — | **good** — single actor, one verb |
| contracts, staff, scouting | `On*StaffContract`, `EditStaff*`, `OnSubmit*Scout` | `StaffPS`, `ScoutingPS` | — | **good** — row-scoped, idempotent-able |
| parts, design, manufacture | `Parts*`, `DesignStageChanged`, `Warehouse*` | `PartsPS` | — | good |
| finances, sponsorship | `Sponsor*`, `Condition*`, `ConfirmSponsor*` | `FinancesPS`, `SponsorshipPS` | — | good |
| board objectives | `EnterTeamChangeFlow`, fanfares | `BoardPS` | — | medium — mostly engine-computed |
| mail / dilemmas | `MailOpen`, `MailRead`, `SetDilemmaResponse` | `MailPS` | — | medium — responses are choices, not data |
| standings / results | — | `StandingsPS`, `RacesPS` | — | **host-only** — engine-computed |
| save / load | `LoadGame`, `ContinueGame`, `NewSave` | `SavePS` | **yes** | solved apart from `QuitGame` |
| race weekend | `SessionOverviewClosed`, `PreSessionClosed` | — | — | see RACESIM-HIJACK |

The pattern: **everything the player *decides* is tractable; everything the game *computes* is
host-only.** That is the same split that made the sim hard, and it is a much smaller share of the
career layer than of the sim.

---

## 7. Sequenced plan, cheapest first

Each step is independently useful and is sized to the evidence already in hand.

1. **Freeze the ownership model.** Decide model 1 vs model 2 (§4.3). This is a product decision, and
   every later step depends on it. Cost: a decision.
2. **Prove the DB handle survives a race weekend.** One question, huge leverage (§5.3): record the
   `sqlite3*` before and after a race. If it moves, the design needs re-discovery; if not, the rest
   gets much simpler.
3. **Capture the prepare call, not exec.** Log `{sql, binds}` for one mutation on the host and replay
   it on a second process. If a contract edit lands identically on both, the whole L3 mechanism is
   proven. This is the L3 equivalent of the sim's `maxDistDelta=0.00` result.
4. **Add the replicated-log transport.** Reuse the existing file/relay path; the record is small and
   idempotent, so torn writes are detectable the same way the sim payload's were.
5. **Gate L2 intents.** A client's UI event that would mutate state is forwarded to the host instead of
   applied locally (§2). Without this a client can raise a verb its own engine will not honour.
6. **Then, and only then, the sim.** L1 is the hardest layer and the smallest share of a career. It is
   also the layer whose *product* goal (shared race) is separable from the career's (shared career).

**The ordering is the point.** The sim got the attention because it is the visible, dramatic part.
The evidence says the career is three times larger, has an actuator, has no randomness in the mutating
path, and fails safely — so it is both the bigger job and the cheaper one.

---

## 8. Gaps

1. **Prepare-call offset `unknown`.** The exec offset is known (`0x156D300`); the prepare site is not
   yet identified. This is the keystone of §4.3.
2. **DB handle stability across a race `unknown`** (§5.3) — highest-value open question.
3. **Whether any career event is honoured engine-side from a non-owner** `unknown`; assume no (§2).
4. **Schema-vs-runtime drift `unknown`.** The 26 `*PS.json` files are schema definitions; whether the
   live DB exactly matches (26 tables, same columns) is not established. The 336 migrations make
   drift plausible.
5. **No live tests.** Nothing in this document was executed. Every mechanism here is argued from
   evidence already recorded in [NOTES.md](../NOTES.md), not re-measured.
6. **`QuitGame` still untested** — the one L4 link, unchanged since [LOBBY-GATE.md](LOBBY-GATE.md).

---

## Appendix — the numbers behind "24%", one line each

```
raceWeekend UI        346 / 1448 JS files        = 23.9%
raceWeekend events     85 / 352  engine events   = 24.1%
sim families          241 vs career 873 binary identifiers  = 3.6x smaller
career schema          26 tables, StaffPS 23% of them; sim has no table
```

Full working in [GAME-SCOPE.md](GAME-SCOPE.md) §1, all 24 figures machine-verified.
