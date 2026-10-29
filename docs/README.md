# Documentation

One page per topic. Every claim is labelled so a reader can tell what was run from what was reasoned:

| Label | Meaning |
|---|---|
| `proven` | A test ran, or a shipped binary/export shows it. The command or the file:line is given. |
| `inferred` | Follows from the code or the docs, but has not been executed. |
| `unknown` | Needs a running game, a Steam client, or another machine to settle. |

If a page says "works", there is a test run behind it. If it says "should work", there is not.

## Start here

| Page | What it answers |
|---|---|
| [../README.md](../README.md) | The honest state table. Read this first. |
| [GETTING-STARTED.md](GETTING-STARTED.md) | Set up a machine, build, deploy, and what to try first. |
| [UI-COMMANDS.md](UI-COMMANDS.md) | The complete event surface (352 events, 517 call sites) and which ones move the world. |
| [../NOTES.md](../NOTES.md) | The research log, in order, with the mistakes left in. |

## How it is built

| Page | Topic |
|---|---|
| [COOP-PLAN.md](COOP-PLAN.md) | The overall plan and the networking model. |
| [CAREER-COOP-PLAN.md](CAREER-COOP-PLAN.md) | The four layers: UI, career DB, race sim, saves. The career DB is in-memory SQLite with `sqlite3_exec` as the live actuator. |
| [RACESIM-HIJACK.md](RACESIM-HIJACK.md) | The race sim as a slave: what can be set (a lot) and what cannot (the clock). |
| [GAME-SCOPE.md](GAME-SCOPE.md) | How much of the game the race sim actually is: ~24%. |
| [HEADLESS-HARNESS.md](HEADLESS-HARNESS.md) | Rendering and driving the game's own UI with no game running. |

## The four coordination rules

| Page | Topic |
|---|---|
| [COORDINATION.md](COORDINATION.md) | The rules themselves and where each one runs. |
| [COORDINATION-SENDER.md](COORDINATION-SENDER.md) | Why they were inert for a long time: nothing produced a frame, and the supervisor dropped every frame an agent sent. |
| [TEAM-RETARGET.md](TEAM-RETARGET.md) | Playing as different teams, and disabling the AI for the player's team. |
| [SAVE-MERGE-PLAN.md](SAVE-MERGE-PLAN.md) | How two careers are reconciled. |

## Getting in and out

| Page | Topic |
|---|---|
| [STEAM-JOIN.md](STEAM-JOIN.md) | Steam "Join Game": lobbies, `+connect_lobby`, and the callback layouts. |
| [RICH-PRESENCE.md](RICH-PRESENCE.md) | What friends see, and why `status` rather than `steam_display`. |
| [LOBBY-GATE.md](LOBBY-GATE.md) | The black-screen gate: which events destroy the front-end document. |
| [LOAD-SAVE-CALL.md](LOAD-SAVE-CALL.md) | Loading a save from outside. |
| [CLIENT-UI-PRESS.md](CLIENT-UI-PRESS.md) | Pressing real controls instead of firing events blind. |

## Race weekend

| Page | Topic |
|---|---|
| [RACE-SYNC-RUNBOOK.md](RACE-SYNC-RUNBOOK.md) | Running two machines against one race. |
| [TYRE-SYNC.md](TYRE-SYNC.md) | Tyre state replication. |
| [RNG-DETERMINISM.md](RNG-DETERMINISM.md) | Whether two independent sessions can be made to agree. Answer: not by editing data. |
| [UI-MENU-INTEGRATION.md](UI-MENU-INTEGRATION.md) | The co-op screen and how it patches the main menu. |

## Rules that are not obvious

| Page | Topic |
|---|---|
| [NORMAL-PLAY.md](NORMAL-PLAY.md) | The mod must be invisible when nobody is hosting. What was measured and what was fixed. |

## Known defects

- **`coop-mem --test loop_stub_volume` crashes `0xC0000005`.** Pre-existing, reproducible on a bare
  `cargo test --workspace`, does not reproduce under `--test stub_execution`, and the crate is not on
  the co-op path. `run_all_tests.py` excludes it by name rather than hiding it.
- **The race-start gap is a layering fact, not a missing name.** Session start is mapped from the exe
  (`StageProceed` `0x5fa7c98`, `TravelToRaceWeekend` `0x5fcac28`) but is not part of the UI layer —
  starting a session is what the advance-weekend button does, and the UI's own `PreSessionClosed` only
  dismisses a dialog. The commands exist and have no sender in the export; they have not been fired yet.
  See the "Starting a race session" section of [UI-COMMANDS.md](UI-COMMANDS.md). Any plan that assumes a
  race can be started with a UI event is wrong until one of those is tried.
