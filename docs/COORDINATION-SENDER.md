# The coordination sender: what was missing, and what is still unproven

`COORDINATION.md` describes the four rules and where each one runs. This file records the piece that did
not exist until now — **nothing ever produced a `Clock`, `Contract` or `Vote` frame** — what was built to
fix that, and exactly how far the proof goes.

Everything here is tagged the same way as the rest of the notes: `proven` (a test or the export shows it),
`inferred` (follows from the code but not executed), `unknown` (needs a running game).

## Why the feature was inert

| piece | state before |
|---|---|
| wire format (`coop-proto`) | tags 11/12/13 encoded and decoded; 71 tests |
| the rules (`coordination.rs`) | `ClockGate`, `TeamOwners`, `ContractLedger`, `VoteBox`; 33 tests |
| arbitration (`coop-supervisor`) | `arbitrate()` ran before relay; 8 tests |
| **producing a frame** | **nothing. Not the UI, not the agent, not the link.** |
| **delivering it** | **nothing. See below.** |

The second gap was the worse one. An agent's socket announces itself with `Msg::Agent` and never sends
`Hello` — the peer slot belongs to the game DLL (`coop-link`), which does send `Hello`. `drain_peer` keys
the arbitration identity off that `Hello`, so **every frame an agent sent fell through to
`sent data before Hello; dropped`**. Building a sender alone would have produced frames that were accepted
and discarded, with one log line per frame as the only clue.

## What was added

1. **`Msg::Agent` gained the side** (`coop-proto/src/lib.rs`), and `VERSION` went 1 → 2. `proven`.
   - The agent is the only component inside the game process, so it is the only thing that can say *this
     is the host's player*. Without the side the supervisor cannot call `arbitrate(coord, side, msg)`.
   - A version bump rather than a compatible extension: an old supervisor would decode a truncated `Agent`
     and then drop the side byte as trailing bytes — a working-looking link where every claim is refused
     for want of a side. `Agent` with side reads `pid(4) rsm(8) side(1)`; the decoder rejects a side byte
     outside `{0,1}` instead of defaulting to Host, which is pinned by a test.

2. **`coop-agent/src/coord.rs`** — the frame grammar. `proven`.
   - Pure, no I/O: `parse_reasons`, `contract_frame`, `vote_frame`, `parse_request`, `frame_for`.
   - Refuses rather than guesses: an unknown `PauseReason` byte, an empty clock set, `0xFF` as a vote
     option (that is the resolved-tie sentinel), a contract day below 1, and any **missing or non-numeric
     required field**. That last one is not pedantry — defaulting `option=` to 0 would cast a real,
     counted vote for the first option on the list.

3. **An outbox on the agent, drained by the existing heartbeat thread** (`coop-agent/src/lib.rs`).
   `proven` at the endpoint level, `inferred` in-game.
   - The HTTP handler runs on the lobby thread; the socket belongs to the heartbeat thread. The handler
     validates and enqueues, the heartbeat writes — so a stopped supervisor cannot block a `fetch` from the
     front-end and freeze the menu that issued it.
   - Bounded at 64 with the oldest dropped: the queue carries *state*, so a superseded pause report has no
     value. Frames are taken as a batch and **put back if a write fails part-way**, because the frames that
     matter are the ones a reconnect must not swallow.

4. **An inbox on the agent**, so a refusal or a resolved vote reaches the UI. `proven`.
   - The agent's socket was write-only. `arbitrate` answers a losing claim with `Msg::Bye` and a resolved
     vote with a `Vote` carrying the winner, and both landed in a socket nobody read.
   - `/coop/inbox` exposes the last few lines; `/coop/coordinate` reports `pending` and `supervisor` so
     "queued but the supervisor is not running" is visible instead of looking like a rule doing nothing.

5. **`coop-supervisor`: route by what a socket *is*.** `proven`.
   - `agent_side` is recorded from `Msg::Agent`, arbitration uses `side.or(agent_side)`, and a frame from an
     agent socket goes to the *other* side's agent (`deliver_to_agent`) instead of the peer relay.
   - `side` (from `Hello`) still wins when present, and stays the single value used for registry bookkeeping
     and disconnect cleanup, so an agent never becomes a relay target. A test asserts both directions: a
     game-peer frame reaches the other game peer and **not** an agent, and an agent frame reaches the other
     agent and not the relay (`frames_out` stays 0).

6. **`js/coop/observe.js`** — the UI half. `proven` headless, `inferred` in-game.
   - Wraps `engine.trigger`, which `UI-COMMANDS.md` proves is the single choke point, rather than
     redefining AMD modules: `sendEvent` has two shapes in the export and the bootstrap loads `require.js`
     asynchronously, so a module patch is a race. Wrapping the concrete host object survives both shapes.
   - Additive only: the original is called **first, synchronously, with identical arguments**, its return
     value is passed through, and any error in the reporting path is swallowed so a coordination problem
     can never break the screen that produced it. All four are asserted.
   - Event shapes, all read from the export: `SimPauseChange(value, pauseLayer)`
     (`RaceWeekendEvents.js:66`), `SetDilemmaResponse(mailId, response)`
     (`InboxDilemmaButton.js:53`), `OnSubmitContractOffer(terms, staffID)` /
     `OnConfirmStaffContract(staffID)` / `OnCancelStaffContract(staffID)` /
     `OnFailedStaffContract(staffID)` (`ContractProposalAndResultsPanel.js:105-216`).
   - The clock is a **set of layers**, matching the union rule, not a flag.

7. **`bridge.js`** — posts what the observer produces, plus `Team <id>` / `Day <n>` (no event carries
   either, and a contract frame without a day is refused), `CoopState` (so a missing frame is
   distinguishable from a missing event), and a slow inbox poll.

## Test totals

| suite | result |
|---|---|
| `cargo test --workspace --exclude coop-mem` | **212 passed**, 0 failed |
| `tools/headless/observe_test.js` | **67/67** |
| `tools/headless/agent_coord_e2e.js` | **16/16** — two real agents through a real supervisor process |
| `tools/headless/normal_play_test.js` | **21/21** |
| `tools/headless/press_test.js` | **25/25** |
| `tools/headless/run_tests.js` | **41/41** |
| `tools/headless/drive_test.js` / `probe_render.js` | 13/13, 15/15 |
| UI sweep | 161/165 screens |

`press_test.js` reports 25 or 24 depending on whether the career advance button is enabled in that run — a
branch, not a failure. It also reported 2 failures once under heavy parallel load and then passed 6/6 in
isolation; the suite is pre-existing and untouched by this work.

`coop-mem --test loop_stub_volume` still crashes `0xC0000005`; that is pre-existing, documented, and the
crate is unrelated to anything changed here.

Both new suites were checked in **both** directions: reverting the supervisor fix makes all four socket
tests fail, and re-adding the unconditional inbox timer makes the normal-play assertion fail.

## Bugs found while building this, and what they were

Recorded because each one would have looked like "multiplayer is broken" from outside.

- **The supervisor test I wrote passed while the feature was off.** The first version used `127.0.0.2`-style
  sockets whose `Hello` also made them relay peers, so the routing assertion failed for the wrong reason. The
  real topology is two sockets per player — `Hello` for state, `Agent` for coordination — and the test now
  mirrors that.
- **A test reader silently ate a frame.** `next()` left its resolver in the queue after timing out, so a later
  frame was handed to an already-resolved promise. It looked exactly like the supervisor failing to deliver.
- **`h * 0x01000193` is not FNV-1a in JavaScript.** `h` can be near 2^32 and the product exceeds 2^53, so the
  checksum was wrong for most payloads. `Math.imul` is required. This cost a round of debugging against the
  supervisor's `checksum mismatch` line.
- **My first inbox poll reintroduced the regression `NORMAL-PLAY.md` exists to prevent** — an unconditional
  repeating timer. The existing test missed it because it counted `setInterval` only. The assertion now
  measures the thing that costs the player something: a timer still armed past the idle window.
- **A pre-existing flaky assertion** (`client refusal names the page`) read `_pendingAck`, which a background
  poll consumes. It now reads a write-only `_lastRefusal`. Unrelated to coordination, but it made the suite
  untrustworthy.

## What is still unproven

1. **A pause in the running game, seen on the other machine.** The mapping, the frame, the routing, the
   delivery and the gate are each tested; the join is not. `SimPauseChange` is a real event
   (`RaceWeekendEvents.js:66`) but the observer has only ever run against a fake `engine`.
2. **That the engine pauses when it receives the call.** The far side now *does* call
   `SimPauseChange(value, 9)` on the other player's behalf (`clocksync.rs` → `/coop/clock` →
   `applyClock`), which is the requirement implemented rather than reported. What the engine does with that
   call is still the export's blind spot: no test here can see a sim actually stop.
3. **Whether the agent's HTTP endpoints work inside the game's web view.** The endpoints are Rust and tested
   directly; `fetch`/`XMLHttpRequest` are proven present in the front-end, CORS headers are already served.
4. **Whether claims arrive early enough to matter.** A claim is issued at `OnSubmitContractOffer`, so the
   other player can already be negotiating; they are stopped at confirm. No earlier event exists in the export.

## The clock, end to end

The requirement was "when one player's clock stops the other one's does too". That needs both halves, and
only the second one is new:

| half | where | state |
|---|---|---|
| learn that this player stopped | `observe.js` → `/coop/coordinate/clock` | tested headless |
| tell the other machine | agent outbox → supervisor → the other player's agent | tested end to end |
| union both players' reasons | `clocksync.rs`, delegating to `coordination::ClockGate` | 9 unit tests |
| make the world stop | `/coop/clock` → `applyClock` → `SimPauseChange(value, 9)` | logic + drift-guarded; **engine behaviour unknown** |

Two decisions worth keeping:

- **The union lives once, in Rust.** The UI knows only its own reasons; the agent is the only component that
  sees both. Re-deriving the union in JavaScript would be two implementations of the rule most likely to
  drift.
- **`seq`, not a flag.** Acting on `stopped` every poll would re-pause a paused world forever and the pause
  would never end. `seq` advances only on a transition, so the front-end acts exactly once per change — the
  same reasoning as `ClockAction::NoChange`, one layer up.
- **Layer 9 (`RaceViewUI`), not 2 (`UserPause`).** Our own layer means unpausing restores only what this mod
  held; using `UserPause` would let one machine cancel a pause the other player set themselves. `PauseLayer`
  and `PauseReason` are numbered differently, so this is a real hazard, not a style choice.

## Next, in order

1. **Deploy both paks and both agent DLLs, same build, before any test.** Not deferred as a nicety: the wire
   version went 1 → 2 when `Msg::Agent` gained the side field, so a mixed pair cannot cooperate. Verified in
   the other direction — sending version-1 frames to a version-2 supervisor fails **loudly** (9 of 13
   assertions fail with `sent an invalid frame: peer speaks version 1, this build speaks 2`), which is the
   designed behaviour: a clear error at the header rather than a half-decoded message. Deploying to only one
   machine therefore breaks it visibly instead of subtly, but it breaks it.
2. Launch, set `Team <id>` and `Day <n>` from the bridge, and confirm `CoopState` counts events.
3. Press pause in a race and watch the other machine's log for `clock from the other player: action=stop`,
   then confirm the sim actually holds. This is the one claim no export can settle.

**Not deployed as of this note.** The built pak (119,728 bytes) and `coop_agent.dll` are current and
verified, but the host still has the 28 Oct pak and the second machine is not reachable. A host-only deploy
was deliberately not done: it would leave one machine on each side of the version bump, which is exactly the
state the deploy script was written to prevent.
