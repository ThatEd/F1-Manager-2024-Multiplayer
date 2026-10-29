# F1 Manager 2024 Multiplayer

Co-op for **F1 Manager 2024** (Frontier Developments, UE 5.1, Steam appid 2591280). Two players run
one career together: each manages their own team, the world clock is shared, and Steam's friends list
is how you get into each other's session.

This repo is the working bundle: code, tests and findings. It contains **no game files, no assets
and no decompiled binaries** — see [Getting the data](#getting-the-data).

## Where it actually stands

Read this before anything else. The honest summary is **the plumbing is proven, the multiplayer has
never been played.** Two humans have not completed a co-op session, because it has never been
launched with this code. Everything below reflects what has been *run*, not what is expected to work.

| Area | State | Evidence |
|---|---|---|
| UI command surface | **Mapped.** 352 events across 517 call sites; `engine.trigger` proven to be the single choke point | [docs/UI-COMMANDS.md](docs/UI-COMMANDS.md) |
| Driving the UI from outside | **Works.** Pressing real controls emits the real events; `Continue` and `Load` proven live in-game on the host machine | [docs/CLIENT-UI-PRESS.md](docs/CLIENT-UI-PRESS.md) |
| Starting a race session | **Not solved, but the lever is identified.** The *UI layer* has no event that starts a sim — the pre-session confirm emits only `PreSessionClosed`. The exe, though, contains session-flow commands the UI never sends: **`StageProceed`** (`0x5fa7c98`, inside the `SessionOverviewClosed`/`AdvanceToNextStage`/`PreSessionClosed` block) and `TravelToRaceWeekend` (`0x5fcac28`). Untried | [docs/UI-COMMANDS.md](docs/UI-COMMANDS.md) |
| Car state replication | **Code complete, unrun.** Protocol, layout, mailbox and hook all tested; the hook has never fired in a live race | [docs/RACESIM-HIJACK.md](docs/RACESIM-HIJACK.md) |
| Shared clock (both players pause together) | **Code complete, unrun.** Union rule unit-tested; the far side calls `SimPauseChange` on receipt. Never seen in a game | [docs/COORDINATION.md](docs/COORDINATION.md) |
| One contract per person per day | **Code complete, unrun.** Arbitration tested in the supervisor, never exercised by two live clients | [docs/COORDINATION.md](docs/COORDINATION.md) |
| Regulation votes counted for both | **Code complete, unrun.** Vote box tested; no live vote has been cast | [docs/COORDINATION.md](docs/COORDINATION.md) |
| Team retarget (play as different teams) | **Code complete, unrun.** `SetPlayerTeam` + the AI gate located; written, not executed against a career | [docs/TEAM-RETARGET.md](docs/TEAM-RETARGET.md) |
| Steam "Join Game" | **Code complete, unrun.** Lobby create/join/`+connect_lobby` implemented; needs a live Steam client | [docs/STEAM-JOIN.md](docs/STEAM-JOIN.md) |
| Rich presence | **Code complete, unrun.** `status` key path avoids the publisher-token problem of `steam_display` | [docs/RICH-PRESENCE.md](docs/RICH-PRESENCE.md) |

**What "unrun" means here:** unit tests pass, the code compiles, and the design is written down — but
the game has not executed it. Do not read "code complete" as "works".

**What has genuinely been proven in a running game** (on one machine, before this bundle):
`ContinueGame` from the main menu loads a career, and `LoadGame <slot>` loads a chosen save, both by
pressing the real on-screen controls. That is it. Everything else is one launch away from being known.

## Quick start

Nothing here needs the game. Python 3.10+, Node 16+ and a Rust toolchain are enough.

```bash
python -I run_all_tests.py          # every suite; prints ALL OK
python -I run_all_tests.py rust     # 231 Rust tests
python -I run_all_tests.py e2e      # two real agents through a real supervisor
python -I run_all_tests.py redaction   # no private identifiers in the tree
```

Currently: **231 Rust tests, 196 JS assertions across 8 suites**, plus a publishability check that fails
if a private path has leaked into the tree. All of it passes from a clean clone.

Verified by cloning the published repo into an empty directory and running it there: `ALL OK (3 skipped:
press, drive, render)`. Three suites render real components from the game's decompiled UI, so without the
export they skip themselves with a named reason rather than failing — a fresh clone is green with visible
gaps instead of red for a reason that has nothing to do with the code.

`e2e` builds the supervisor itself rather than skipping, because it is the only suite that proves two
peers actually exchange coordination frames. Without the decompiled UI export (`F1M_UI_EXPORT`), the three
render-dependent suites skip themselves — see [Getting the data](#getting-the-data).

To build and deploy the UI mod (needs the game installed, and the game closed):

```bash
python -I tools/deploy_pak.py --no-deploy   # build + verify the pak only
python -I tools/deploy_pak.py               # build, verify, then copy into the install
```

`deploy_pak.py` refuses to finish unless the pak in the install is byte-identical to the one built
from source. That check exists because a stale pak once made a finished feature look like a code bug.

## How it works, in one page

The game's whole UI is a Coherent Gameface web view, and **every user action funnels through one
function**:

```js
// js/common/core/Engine.js
function sendEvent(eventName, ...args) { engine.trigger(eventName, ...args); }
```

That makes `engine.trigger` the game's scripting API for its own UI. A custom pak overrides
`Frontend.html` / `RaceWeekend.html` / `TeamManagement.html` and adds four modules:

| File | What it does |
|---|---|
| `js/coop/press.js` | Presses the **real on-screen controls** instead of firing events blind. The view decides what to emit. |
| `js/coop/observe.js` | Wraps `engine.trigger` so a real player action becomes a coordination frame. Additive: the original always runs first. |
| `js/coop/bridge.js` | Polls a loopback command server and calls `engine.trigger(...)`. Also drives the shared clock and the launch-from-invite path. |
| `js/coop/lobby.js` | The co-op screen: pick team, host or join. Opens the Steam lobby on the host tab. |

Around that sits a Rust workspace (`coop-rs/`):

```
coop-proto/       wire format + the four coordination rules (pure, no I/O)
coop-supervisor/  the host relay: arbitrates contracts and votes, relays car state
coop-agent/       DLL injected into the game: hook, mailbox, Steam lobby, rich presence
coop-link/        the game-side client that sends snapshots
coop-mem/         process memory primitives (SQLite hook, object walking)
```

The same mod pak is installed on **both** machines and does nothing when nobody is hosting — see
[docs/NORMAL-PLAY.md](docs/NORMAL-PLAY.md), which also records the three background polling loops
that had to be fixed to get there.

## Layout

```
F1M2024/                        (this repository)
  README.md                     this file
  run_all_tests.py              every suite, no game needed
  docs/                         one write-up per topic, every claim labelled
  coop-rs/                      the Rust workspace (7 crates)
  pak/                          the UI mod source that gets packed
  tools/
    headless/                   render + drive the game's UI with no game running
    deploy_pak.py               build, verify and deploy the pak
    coop_server.py              the loopback command server the bridge polls
    remote.py, preflight.py     install/launch helpers for the second machine
```

## The four coordination rules

These are what "co-op" means concretely. All four are unit-tested; none has been exercised by two
live players.

1. **One player's clock stopping stops both.** The pause state is the *union* of both players'
   reasons, not a flag. Whichever player clears first must not start the world.
2. **Team authority beats local drift.** The non-owner's value is discarded **even when it is newer** —
   so if the AI ran something on a player's team, it is overwritten.
3. **One contract per person per day.** The first accepted offer wins; the second is refused with a
   reason the player can read. Enforced in the supervisor, the only party that sees both claims.
4. **Both votes count.** A player changing their mind replaces their vote rather than adding one. A tie
   reports no winner instead of picking a side.

`docs/COORDINATION.md` has the reasoning and `docs/COORDINATION-SENDER.md` records what was missing:
the protocol and the rules existed for a long time with **nothing producing a frame**, which is worth
reading before changing any of it.

## Things that will bite you

Every one of these cost real time on this project.

| Trap | What actually happens |
|---|---|
| `Engine.isCoherentPlayer()` | **Inverted from its name.** `false` in-engine, `true` in the Coherent *preview player*. So `DS.setValue` throws in the shipped build and every `if (isCoherentPlayer())` branch is dead code. |
| Firing `LoadGame` from an in-game page | Destroys the front-end document: black screen, process still alive. Front-end only. |
| `AdvanceToNextEvent` | Start/**stop**, not idempotent. Pressing it twice stops what you just started. |
| A `setInterval` you forgot | The mod is installed on both machines; an unconditional poll is visible single-player cost. Three were fixed; two regressions were caught afterwards. |
| `h * 0x01000193` for FNV-1a in JS | Exceeds 2^53, so the checksum is wrong for most payloads. Needs `Math.imul`. |
| Counting `setInterval` to prove a timer is gone | A repeating `setTimeout` chain is invisible to it. Count *armed timers* instead, and pick an exact bound — a bound loose enough to allow the regression proves nothing. |
| Trusting a DLL string scan | Linker string pooling makes a short literal absent from the binary while the code is present, and vice versa. Assert from the producing function or a test. |
| A test that cannot fail | Two suites in this project passed while testing nothing. Every suite here was checked by planting the regression and watching it fail. |

## Getting the data

The repo ships code, not game data. Two things are generated from **your own copy** of the game:

| Data | Needed by | Why it is not committed |
|---|---|---|
| The Gameface UI export | `tools/headless` render + drive tests (skipped without it) | decompiled from the game's paks |
| Offsets and per-build addresses | `coop-mem`, the injection path | per-build artefacts for one exe hash |

Point the tools at an export with the `F1M_UI_EXPORT` environment variable — the directory containing
`UIGameface/`, i.e. `.../F1Manager24/Content`:

```powershell
$env:F1M_UI_EXPORT = "D:\exports\F1Manager24\Content"
python -I run_all_tests.py
```

Without it, the three suites that render real components skip themselves with a named reason and the
rest still pass. See [docs/HEADLESS-HARNESS.md](docs/HEADLESS-HARNESS.md) for how the harness boots the
game's own RequireJS tree from that export.

`tools/redact_private.py` runs as part of the suite. It is a test, not a chore: this bundle is published
under the owner's account, and a personal path in a doc is the kind of thing that survives review
because nobody re-reads a 150 KB write-up. It fails the run if any identifier remains.

## What is not here

- **A working race start.** The single biggest gap — but not for the reason it is usually summarised as.
  Session starts *are* mapped from the exe; they are simply not part of the **UI** layer, because
  starting a session is what the advance-weekend button does. The UI's own event for that,
  `PreSessionClosed`, only means "this dialog is done". The commands that read as the actual stage
  machinery — `StageProceed` (`0x5fa7c98`) and `TravelToRaceWeekend` (`0x5fcac28`) — are present in the
  binary's session-flow binding block and have **no sender anywhere in the UI export**. They are the
  strongest lead and nobody has fired them yet; what each handler *does* is `unknown` because the bodies
  were not decompiled. Evidence and the unrun test plan: [docs/UI-COMMANDS.md](docs/UI-COMMANDS.md).
- **Any verification of the coordination layer against a live game.** See the table at the top.
- **A one-click installer.** Deployment is `deploy_pak.py` plus an injected DLL; see
  [docs/RACE-SYNC-RUNBOOK.md](docs/RACE-SYNC-RUNBOOK.md).

## Credit and licence

Built on the hub's playbook ([coop-anything](https://github.com/ThatEd/coop-anything)) and on the
research notes of prior agents working this game; every claim in `docs/` is labelled `proven`,
`inferred` or `unknown` so a reader can tell which is which.

MIT — see [LICENSE](LICENSE).
