# Getting started

Two audiences: someone who wants to **play**, and someone who wants to **continue the work**. Read the
one you are.

Before either: the honest state. **The plumbing is proven; the multiplayer has never been played.**
No two humans have completed a co-op session with this code. The table in [../README.md](../README.md)
says exactly what has run and what has not.

---

## Part 1 — Playing

> This will not work yet, and this section says why rather than pretending otherwise. Co-op needs a
> working race start (see Part 2), and that is the unsolved problem. What *does* work today is driving
> the game's UI from outside on one machine.

### What you need

- F1 Manager 2024 (Steam appid 2591280), installed and closed.
- The Rust toolchain (`cargo`) to build the agent.
- `repak` to build the pak — `tools/bin/repak.exe` is the expected location, or on `PATH`.
- **The same build on both machines.** The wire protocol is versioned and a mismatch is refused loudly
  at the header, so a one-sided update does not silently half-work: it fails.

### Build and deploy

```bash
python -I tools/deploy_pak.py --no-deploy    # build and verify only, changes nothing
python -I tools/deploy_pak.py                # build, verify, then copy into the install
```

The script refuses to finish unless the pak in the install is byte-identical to the one built from
source. That check exists because a stale pak once made a finished feature look like a code defect —
see the header of the file.

Then build the agent and inject it:

```bash
cd coop-rs
cargo build --release -p coop-agent
# inject coop_agent.dll into the running game; tools/remote.py and tools/preflight.py help.
```

### Telling a machine which player it is

The agent cannot see the injector's environment, so identity comes from a file next to the game exe:

| File | Contents | Meaning |
|---|---|---|
| `coop_side.txt` | `host` or `client` | Which player this machine is. |
| `coop_supervisor.txt` | `host:port` | Where the supervisor is. |
| `coop_lobby_port.txt` | a port | The agent's own HTTP endpoint, if not the default 45992. |

The lobby's `role=` vocabulary is reused, so a player who claimed a team as `client` is a `Client`
here with no extra configuration.

### What to expect, honestly

- **The UI mod does nothing when nobody is hosting.** It probes, gives up, and stops. Measured:
  ~17 loopback attempts per 10 idle minutes, then silence (`docs/NORMAL-PLAY.md`).
- **`Continue` and `Load` work.** Both press the real controls; both were proven in a running game.
- **A race cannot be started from outside yet.** Not because session start is unmapped — it is mapped
  from the exe — but because it is not in the UI layer (see Part 2).

---

## Part 2 — Continuing the work

### Run the tests first

Nothing needs the game. This is the fastest way to know the tree is healthy.

```bash
python -I run_all_tests.py            # everything
python -I run_all_tests.py rust       # 231 Rust tests
python -I run_all_tests.py e2e        # two real agents through a real supervisor
```

You should see `ALL OK`. Suites that need the UI export skip themselves with a named reason, so a
clone without a decompiled copy is green-with-visible-gaps rather than red.

### The one thing to fix first

**Starting a race session from outside.** Everything downstream is blocked on it — but the framing
matters, because the usual summary ("no event starts a race") is wrong in a way that sends people at the
wrong layer.

What is actually true:

- **Session start is not a UI event, and that is expected.** Starting a session is what the *advance
  weekend* button does. The UI event for it, `PreSessionClosed` (`PresessionView.js:250`), only means
  "this dialog is done" — it closes a screen, it does not start a sim.
- **The exe contains the stage machinery the UI never calls.** `StageProceed` (`0x5fa7c98`) sits
  immediately beside `SessionOverviewClosed`, `AdvanceToNextStage` and `PreSessionClosed` in the
  session-flow binding block; `TravelToRaceWeekend` (`0x5fcac28`) is the only "travel to race weekend"
  name anywhere. Neither has a sender in the entire UI export — they are entry points the shipping UI
  reaches some other way, most plausibly from native code.
- **So the work is not "find an event" — it is "fire the right command and see what moves."** The race
  sim object exists and is readable and sits at `simtime = 0` until a human clicks.

The unrun test plan, in order, is in [UI-COMMANDS.md](UI-COMMANDS.md):

1. `engine.trigger('StageProceed')` from a genuine pre-session screen — the cheapest thing to try, and it
   may move the stage without the dialog-dismissal precondition.
2. `engine.trigger('TravelToRaceWeekend')` from TeamManagement when the calendar's next event is a race.
3. Only then, decompile the handler that references the `StageProceed` string at `0x5fa7c98` and read
   what it validates before advancing.

**What each handler does is `unknown`** — the exe was read as a byte-string source, not decompiled, so
these are labelled leads with offsets, not findings. If you conclude it cannot be driven from the UI
side at all, that is also a result worth writing down — say so and why.

### How to add a feature without breaking the honest-state rule

1. **Write the test before the claim.** Every "works" in these docs has a test run behind it.
2. **Plant the regression.** Write the test, break the code deliberately, and watch it fail. Two suites
   in this project passed while testing nothing; both were caught this way.
3. **Label the claim** `proven` / `inferred` / `unknown` and give the file:line or the command.
4. **Keep the mod invisible.** Anything that polls must stop when there is no host. Add a give-up to
   any new loop, and assert on *armed timers*, not on `setInterval`.

### Traps that have already cost days

Full list in the README. The four that will bite a newcomer fastest:

| Trap | Reality |
|---|---|
| `Engine.isCoherentPlayer()` | Inverted from its name: `false` in-engine. Every `if (isCoherentPlayer())` branch is dead code in the shipped build. |
| Firing `LoadGame` from an in-game page | Black screen, process alive. Front-end documents only. |
| `AdvanceToNextEvent` | Start/**stop**, not idempotent. |
| Reading identifiers as ASCII only | Not this game's trap, but the same class: a DLL string scan can miss a present literal because of linker pooling. Assert from the producing function. |

### Where the pieces are

| I want to… | Look at |
|---|---|
| change what the UI mod does | `pak/f1manager24/Content/UIGameface/js/coop/` — `press.js`, `observe.js`, `bridge.js`, `lobby.js` |
| change the wire format | `coop-rs/crates/coop-proto/src/lib.rs` — bump `VERSION` if a message changes shape |
| change a coordination rule | `coop-rs/crates/coop-proto/src/coordination.rs`, then `coop-supervisor/src/main.rs::arbitrate` |
| change what the game is told to do | `coop-agent/src/clocksync.rs` (pause), `coop-agent/src/lib.rs` (endpoints) |
| add a Steam feature | `coop-agent/src/lobby_policy.rs` (decisions) + `lobby_steam.rs` (FFI) |
| understand an event | `docs/UI-COMMANDS.md`. Do not guess names; list only strings that appear in the export. |

### Contributing back

This bundle is published so the work is not lost. Two things make a handoff useful:

1. **Update the state table in the README.** If something now works, say what test proves it. If
   something got worse, say that too.
2. **Write down the dead ends.** Several pages here exist only to stop the next person repeating a
   crash. A "this does not work, here is the crash" note is worth as much as a feature.
