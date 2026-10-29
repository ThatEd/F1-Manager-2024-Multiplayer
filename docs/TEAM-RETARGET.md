# Retargeting the client's UI to a different team

**Goal.** From one shared save, make the client's UI present a *different team* — their own — with
all the commands they need, and make sure that team is not also making its own decisions.

This is much narrower than the replication plan in [CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md), and it
turns out to need **two integers and one table**, because the game already separates "which team is
the player's" from "which teams the AI runs".

On paper. No live tests. Tags: *(no tag)* proven from the cited file, **binary** from the exe,
**inferred** reasoned, `unknown` not established.

---

## 0. The two levers, found

| what you want | how the game models it | the exact statement |
|---|---|---|
| "the client's UI is team X" | `Player.TeamID` — a **single integer** in one row | `UPDATE Player SET TeamID = ?1` |
| "team X is not AI-driven" | `Teams_AI_Records.LastDecisionEvaluationDay` per `(TeamID, SeasonID)` | `UPDATE Teams_AI_Records SET LastDecisionEvaluationDay = @Day WHERE SeasonID = @SeasonID AND (@OptTeamID IS NULL OR TeamID = @OptTeamID)` |

Both are **verbatim from the game's own statement set**, not invented:

- `SetPlayerTeam` → `UPDATE Player SET TeamID = ?1`
  (``Database/TeamsPS.json``, entry `SetPlayerTeam`)
- `GetPlayerData` → `SELECT FirstName, LastName, TeamID, UniqueSeed, FirstGameDay, TeamPrincipalRating, PreviousTeamPrincipalRating, CustomTeamEnabled FROM Player`
  (``Database/PlayerPS.json``, entry `GetPlayerData`)
- `SetAITeamLastDecisionEvaluationDay`, `GetAITeamLastDecisionEvaluationDay`, `EnsureAITeamRecordsExist`
  (``Database/TeamsPS.json``, entries `SetAITeamLastDecisionEvaluationDay` etc.)

**This is the single most useful finding in this document:** there is no "player team" object, no
selection flag, no per-team `IsHuman` column. It is **one integer on one row**, and the game already
ships the statement that sets it.

---

## 1. Where the SQL actually lives (and why that matters)

A detail that changes what is editable:

| thing | where it is |
|---|---|
| statement **key names** (`SetPlayerTeam`, `EnsureAITeamRecordsExist`, …) | **binary** — present as a tight run of strings in one block, e.g. at `0x5eabc40`-`0x5eabc9a`: `GetTeamName`, `GetTeamFormula`, `SetPlayerTeam`, `PurgeMailChains`, `PurgeEventLogsUpTo` |
| statement **SQL text** | **not in the binary** — `UPDATE Player SET TeamID` and `Teams_AI_Records` have **0 occurrences** as ASCII and 0 as UTF-16 |
| therefore the SQL lives in… | the **`Content/Database/*PS.json` pak assets** (26 files, 276 KB) |

**Consequences:**

1. We can read every query the game will ever run against the career DB, exactly, offline. That is
   what §0 is built on — no guessing.
2. Editing the PS assets would change the game's own queries, but the shipping form is `.uasset`
   (the `.json` is an FModel export), so it would mean repacking a modified `.uasset` in a pak. That
   is a *supported* route in this project but a brittle one — version-coupled and hard to review.
3. **Better: do not edit the queries. Call them.** The exec handle is proven
   (`sqlite3_exec`, RVA `0x156D300`, `sqlite3*` found by signature, `pager->zName == ":memory:"` —
   [NOTES.md](../NOTES.md) ~1908), and arbitrary SQL through it is proven
   (`coop-sql --swap-contracts 8 83`). So the plan is: **run the game's own statements, verbatim, with
   the client's values.**

That last point matters for safety: running the game's own SQL means the same cascades, triggers and
invariants apply as when the player does it through the UI.

---

## 2. The design

### 2.1 Which team is "the player's" — and where the UI reads it

The UI never touches the DB. It reads a DataStore property the engine publishes:

```js
DS.getValue(['Teams'], 'playerTeamID')          // StandardBackground.js:13, NavTab.js:57, TeamSelectMenu.js:60
DS.getValue(['RaceSim'], 'playerTeamID')        // MinimapContentApp.js:45
```

and derives everything from it — team colours, the two drivers, the two race engineers
(``NavTab.js:57-61``
listens on `['Teams','TeamsList',playerTeamID]` for `driver1ID`, `driver2ID`, `raceEngineer1ID`,
`raceEngineer2ID`). So **retargeting the UI is not a UI change at all** — it is making the engine
publish a different `playerTeamID`.

That is the good news and the whole trick: **the entire management UI is already team-agnostic.** Every
screen reads `playerTeamID` and scopes itself. Nothing needs re-implementing.

### 2.2 Why the team cannot be switched mid-career through the UI

There *is* a team-change screen —
``TeamSelectMenuChangeTeam``
— but it is a **front-end, new-career** flow, and it does not simply reassign you:

- ``TeamSelectMenu.js:177`` — `sendEvent('ChangeTeam', teamID)` (selection)
- ``TeamSelectMenu.js:243-248`` — on fade-to-black end it sends **`NewCareer`**, then **`ConfirmTeamChange`**
- and it gates on **`TeamPrincipalRating`** vs the target team's required rating
  (``:93-99``) — you cannot just pick any team

So `ConfirmTeamChange` is **not** the lever: it starts a *new career at that team*, with eligibility
rules. It discards the shared world. That is the opposite of what we want.

**Therefore retargeting must go through the DB**, not through `ConfirmTeamChange`.

### 2.3 The sequence

```
host                                         client
────                                         ──────
1. career exists, host's team = A
2. save the career  ──── share the .sav ───►  receive it
3.                                            LoadGame <slot>   (Frontend only — LOBBY-GATE rule)
4.                                            DB is now live, in memory
5.                                            UPDATE Player SET TeamID = <B>     ← the game's own SetPlayerTeam
6.                                            make B not AI-driven (see §2.4)
7.                                            reload  (or force republish — see §2.5)
8.                                            UI now reads playerTeamID = B and shows team B
```

Step 7 is the awkward one and is discussed honestly in §2.5.

### 2.4 Turning the AI off for that team

**The gate is the AI's cadence, and it is one column.**

The AI does not have an "enabled" flag. It has `AITeamLastDecisionEvaluationDay` per `(TeamID, SeasonID)`
in `Teams_AI_Records`, with accessors named exactly for that purpose:

```
EnsureAITeamRecordsExist                  INSERT OR IGNORE INTO Teams_AI_Records (TeamID, SeasonID, PoachingAttempts,
                                          SuccessfulPoachingAttempts, SponsorSelectionDay) VALUES (?1,?2,0,0,0)
GetAITeamLastDecisionEvaluationDay        SELECT LastDecisionEvaluationDay FROM Teams_AI_Records
                                          WHERE TeamID = ?1 AND SeasonID = ?2
SetAITeamLastDecisionEvaluationDay        UPDATE Teams_AI_Records SET LastDecisionEvaluationDay = @Day
                                          WHERE SeasonID = @SeasonID AND (@OptTeamID IS NULL OR TeamID = @OptTeamID)
```

The semantics are **inferred** from the name plus its use: the AI evaluates a team's decisions when
`LastDecisionEvaluationDay < current Day`, and records the day it did. If that value is *ahead* of the
calendar, the condition can never be true and the team never evaluates.

```
UPDATE Teams_AI_Records
   SET LastDecisionEvaluationDay = <sentinel far in the future>
 WHERE TeamID = <B> AND SeasonID = <current season>
```

**Why a sentinel and not a "disable" flag:** there is no flag, and adding one is not possible without
editing the game's queries (§1). The sentinel reuses the mechanism the game already trusts — it is the
same column the game itself writes, so nothing else needs to know.

**Risk, stated plainly:** `unknown` what the maximum sane `Day` is, and whether any other code reads
this column expecting a plausible value (e.g. a UI "last evaluated" display, or a catch-up loop that
would iterate day by day). A sentinel like `999999` is safe from the second concern only if the code
compares rather than loops — **unverified**. A conservative alternative is to bump it one day forward
per day, which is idempotent and cannot diverge far from reality.

**The subtlety that decides where to apply it:**

| side | that team is… | AI disable needed? |
|---|---|---|
| **client** | **the player's team** (`Player.TeamID = B`) | probably **not** — the game does not run AI for the player's team, or the player would be managed by it. `inferred`, not proven. |
| **host** | **an AI team** (host's `TeamID = A`) | **yes, mandatory** — otherwise the host's world has team B acting on its own, while the client is also acting for B |

So the mandatory disable is **on the host, for the client's team**. That is the non-obvious part of
this design: the client disabling AI for its own team is defensive; the *host* disabling AI for the
client's team is what makes the arrangement coherent.

Also note `EnsureAITeamRecordsExist` is `INSERT **OR IGNORE**` and carries a `SeasonID` — so between
seasons the row is recreated with `LastDecisionEvaluationDay` defaulting to 0 and the AI wakes up
again. **The disable must be re-applied each season**, or the client's team silently starts making its
own decisions again at the season boundary. `inferred` from the `OR IGNORE` + `SeasonID` shape.

### 2.5 The republish problem — the honest hard part

`playerTeamID` is **engine-published**. The UI cannot write it (`DS.setValue` throws — see
[CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) §2), and the DB edit does not automatically push a new value
into the DataStore. `unknown` whether the engine re-reads `Player.TeamID` continuously or once at
career load.

Three options, cheapest first:

1. **Edit then reload** (§2.3 step 7). Do `UPDATE`, then `LoadGame` again from the front-end. The
   engine re-reads `Player` on load and republishes. Costs one extra load, uses only proven
   mechanisms. **This is the recommended first implementation.**
2. **Edit during load**, i.e. run the `UPDATE` between "DB created" and "UI published". Needs the same
   prepare/execute hooking that [CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) §4.3 already calls for as
   step 3. Cleaner, but it is the unproven part.
3. **Find the publisher** and call it. `unknown`; would be the best answer if it exists.

**Do not** try to make the UI show team B by rewriting `playerTeamID` in the DataStore. It throws.

---

## 3. What this gets you, and what it does not

### 3.1 What "all the commands they need" means here — and it already works

Because the whole management UI scopes itself off `playerTeamID`, **every screen and every command
becomes available for team B automatically.** There is no per-team allowlist to fix. Concretely, these
already operate on "the player's team" via `playerTeamID` and would simply target B:

| area | UI module | commands (from [UI-COMMANDS.md](UI-COMMANDS.md)) |
|---|---|---|
| car development | `cars` (114 files) | `Parts*`, `Warehouse*`, `DesignStageChanged`, `ManufactureStageChanged` |
| staff & contracts | `staff` (102 files) | `On*StaffContract`, `EditStaff*`, `Staff*` |
| finance & sponsors | `finance`, `sponsorTargets` | `Sponsor*`, `Condition*`, `Obligation*` |
| HQ | `hq` | `StartBuildingUpgrade`, `StartBuildingRefurbish` |
| scouting | `scouting` | `OnSubmitQuickScout`, `OnSubmitDetailedScout`, `OnBookmarkToggle` |
| board | `board` | — (read-mostly) |
| calendar | `calendar` | `AdvanceToNextEvent`, `NextMonthSelected` |

### 3.2 What it does not get you

1. **Two worlds, not one shared world.** The client's DB is its own in-memory copy. A contract B signs
   changes the client's DB; the host's copy of B still has the old state. This design gives the client
   *control of a team*, not *a shared career*. Sharing needs the replication in
   [CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) §4.3.
2. **The host's copy of B still exists.** Having disabled B's AI, the host's B is now *frozen* — it will
   not act, and nothing will update it. If the intended experience is "the client plays B and the host
   sees B's results", the client's changes must reach the host (§4).
3. **`playerTeamID` staleness** (§2.5) — until republished, the client's UI still shows A.
4. **Per-season re-application** (§2.4).

---

## 4. If you want the host to see the client's team too

Three options, in increasing fidelity:

| option | how | cost |
|---|---|---|
| **A. Independent worlds** | each side is just its own career; nobody sees the other | none — this is what §2 gives you for free |
| **B. Relay the client's statements** | client sends its `{sql, binds}` to the host, host runs them on its own DB | small; the statements are the game's own, so invariantly safe |
| **C. Full replication** | [CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) §4.3 | the real job |

**B is the sweet spot** and it composes with §2 naturally: the client is already the authority for
team B, so its statements *are* the truth for B. Relay them, and on the host apply only statements that
touch B's rows. That is a row-ownership filter, not a merge — the same model
[CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) §4.3 recommends.

**Note the ordering hazard:** once the host's B has AI disabled and no longer acts, the host must not
also run B's AI *before* the relay arrives on the first day. Disable first, then relay.

---

## 5. Feasibility summary

| step | mechanism | status |
|---|---|---|
| read the exact SQL for any career action | `*PS.json` in the pak assets | **proven** (this document) |
| find the AI gate | `Teams_AI_Records.LastDecisionEvaluationDay` | **proven** (statements) — semantics **inferred** |
| retarget the team | `UPDATE Player SET TeamID = ?` = the game's own `SetPlayerTeam` | **proven** statement; execution mechanism proven (`sqlite3_exec`) |
| UI follows automatically | `playerTeamID` drives every screen | **proven** (UI source) |
| statements load + bind correctly | `statements.rs` + `params.rs`, 853 statements scanned | **proven** (tests) |
| make the engine *publish* the new value | reload, or hook during load | **partly proven** — reload is a workaround; the clean hook is `unknown` |
| disable AI | high `LastDecisionEvaluationDay` | **unproven** — semantics inferred, no live test |
| keep it disabled across a season | re-apply after `EnsureAITeamRecordsExist` | **inferred** from `INSERT OR IGNORE` + `SeasonID` |

---

## 6. Gaps

1. **The AI gate's exact comparison is `unknown`.** `LastDecisionEvaluationDay` is *named* as the gate and
   has get/set accessors, but the comparison operator and whether it is `>=` or `>` lives in native code
   we have not read. The sentinel approach survives either, but the "bump one day at a time" variant
   needs to know.
2. **Whether the player's team is skipped by the AI at all is `unknown`.** If it is *not* skipped, then
   the client must disable AI for its own team too — a one-line change, but it decides whether the
   client-side step exists.
3. **Whether `playerTeamID` is republished on load only, or continuously** — decides between §2.5 options
   1 and 3.
4. **Whether the 26-file schema matches the live DB** — 336 migrations exist; drift is plausible
   ([CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) §8 gap 4).
5. **No live tests.** Nothing here was executed. The two statements are quoted exactly from the assets;
   their *effects* are reasoned.

---

## 6a. Implemented

The planning half of this design is now code, with tests that run without the game:

```
coop-rs/crates/coop-mem/src/params.rs       parameter binding for sqlite3_exec
coop-rs/crates/coop-mem/src/statements.rs   the game's own 853 statements, loaded from the assets
coop-rs/crates/coop-mem/src/retarget.rs     Plan::for_client / for_host / ai_reenable
coop-rs/crates/coop-mem/src/bin/retarget.rs coop-retarget CLI
```

**Why a binding layer was needed.** `sqlite3_exec` takes no bind array, but the game's statements are
written against parameters: 853 statements use **58 positional** (`?1`..`?58`) and **337 named**
(`@TeamID`, `@OptSeasonID`, …) parameters. Executing them by string surgery has two concrete hazards,
both handled in `params.rs`:

- a named parameter with no value is **not** NULL in SQLite, it is the literal string `"@Day"`, so
  `WHERE TeamID = @Day` matches nothing and looks like success;
- player-visible text (team names, staff names) reaching SQL by concatenation is an injection vector
  on our side. Values are type-tagged and rendered as literals; `O'Brien` becomes `'O''Brien'`.

**Statement source.** The SQL is *not* in the executable — only the key names are (as a tight run of
strings at `0x5eabc40`-`0x5eabc9a`: `GetTeamName`, `GetTeamFormula`, `SetPlayerTeam`,
`PurgeMailChains`, `PurgeEventLogsUpTo`). The SQL lives in the 26 `*PS.json` pak assets, so
`statements.rs` loads them and fails loudly if the export is absent.

**What the CLI produces** (verified by `--dry-run`, no game required):

```
coop-retarget --team 7 --season 1
  [SetPlayerTeam]                     UPDATE Player SET TeamID = 7
  [SetAITeamLastDecisionEvaluationDay] UPDATE Teams_AI_Records SET LastDecisionEvaluationDay = 999999
                                       WHERE SeasonID = 1 AND (7 IS NULL OR TeamID = 7)

coop-retarget --host-for-team 7 --season 1
  [SetAITeamLastDecisionEvaluationDay] (as above, no SetPlayerTeam)
```

The `(7 IS NULL OR TeamID = 7)` shape is the game's own `(@OptTeamID IS NULL OR TeamID = @OptTeamID)`
with the value bound. SQLite evaluates it to `FALSE OR TeamID = 7`, i.e. scoped correctly — the
`IS NULL` arm surviving is expected, and there is a test that pins the *bound* form rather than
asserting its absence, because asserting absence fails on correct output.

**Test status:** 44 lib tests pass, 30 of them new. One pre-existing integration test
(`loop_stub_volume`) crashes with `0xC0000005`; **it does so identically with these three files
removed**, so it is an environment issue with the exec stub, not a regression from this work.

**Not yet done:** the live half. The game was not running at build time, so nothing was executed
against a real career. `--dry-run` output is verified; `--status` and `--team` are untested against a
live process.

## 7. The 6-line version

```
1. share the host's .sav
2. client: LoadGame <slot>                    (Frontend only)
3. client: UPDATE Player SET TeamID = <B>     (the game's own SetPlayerTeam)
4. host:   Teams_AI_Records[B] = far future   (the AI's only cadence gate)
5. client: reload, so the engine republishes playerTeamID
6. every management screen is already team-scoped, so nothing else changes
```

Step 6 is why this is worth doing: the UI was never team-specific, so **retargeting is a data change,
not a UI change**. The hard parts are step 3's republish and step 4's unfamiliarity, not the feature
surface.
