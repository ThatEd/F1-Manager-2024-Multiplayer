# Cross-player coordination: the four rules

Four requirements, each implemented in `coop-rs/crates/coop-proto/src/coordination.rs`, carried on the
wire by three new messages, and arbitrated in the supervisor. 33 new tests cover them.

The module is pure — no I/O, no game, no wall clock — because these are the rules that are hardest to
get right and easiest to get wrong silently. The agent and the link call into it rather than
re-implementing it.

## 1. One player's clock stopping stops both

**Rule:** the pause state is the *union* of both players' reasons, not a flag and not last-writer-wins.

`ClockGate` holds a `BTreeSet<PauseReason>` per side. `is_stopped()` is true while *either* set is
non-empty.

Why a set rather than a boolean: two players can stop for different reasons — one in a menu, one in a
modal notification — and whichever clears first must not start the world. A boolean means the second
player to unpause resumes a race the first is still reading a dialog in.

The reasons are the protocol's own `PauseReason` names, and the UI's `PauseLayer` is translated **by name,
not by number** — the two enums are numbered differently (layer 0 is `UserTimeControl`, reason 0 is
`UserPause`), so a numeric pass-through would be a silent off-by-one:

| protocol reason | source `PauseLayer` (`GameTypes.js:1565-1576`) |
|---|---|
| `UserTimeControl` | 0 `UserTimeControl` |
| `UserPause` | 2 `UserPause`, 6 `RedFlagUI`, 7 `EditStrategyUI`, 8 `PitOptionsUI`, 9 `RaceViewUI`, 10 `CircuitInfoUI`, 11 `EndOfSession` |
| `Notifications` | 3 |
| `Tutorial` | 4 |
| `Resync` | ours: one side is resynchronising and must not advance |

Two layers are deliberately **not** propagated: `QuickSim` (1) is a local fast-forward the player chose for
themselves, and `Replay` (5) does not stop the world. Propagating either would fight the other player's own
time control. The mapping is in `observe.js` and pinned by a test that every mapped layer names a reason the
protocol actually has.

`apply()` returns `Stop`, `Run` or **`NoChange`**. `NoChange` matters more than it looks: the game's
`SimPauseChange` (`RaceWeekendEvents.js`) crosses into the engine and resets the user's playback rate, so
the common case must not touch it at all.

**Wire:** `Msg::Clock { reasons, tick }` — the sender's full set, every time. A delta can be lost, and a
lost *unpause* leaves the world frozen with no way to notice why; a redundant "still paused" costs one
small frame.

## 2. Team authority beats local drift

**Rule:** each team has exactly one owner. The non-owner's value is discarded **even when it is newer**.

This is the "even if the AI somehow ran things on the other player's team, we change that" requirement.
`merge_team_row` picks the authoritative side by *ownership*, never by tick age or arrival order —
because the non-owner's value is precisely the thing that must not survive.

```
TeamOwners::assign(team, side, by)   // only the host allocates
merge_team_row(owners, team, local, remote, local_side) -> Keep { value, changed } | NoOwner
merge_field(...) -> BTreeMap<TeamId, Vec<u8>>   // only teams that actually changed
```

Three behaviours worth calling out, each pinned by a test:

- **An unowned team keeps the local simulation** (`NoOwner`). Pure AI teams are allowed to diverge between
  machines — they are not shared state.
- **A missing authoritative value is never back-filled from the non-owner.** Returning the wrong side's
  data because the right side had not sent yet would be worse than returning nothing.
- **A client cannot claim a team it does not own.** `assign` with `by = Client` succeeds only to
  re-confirm a team the client already holds; claiming an unowned team, or one the host holds, is refused.
  Without this the client could award itself the other player's team and every subsequent merge would
  honour it.

## 3. One instant contract per person per day

**Rule:** two players cannot sign the same person on the same day. The first accepted offer wins; the
second is refused **with a reason the player can read**.

`ContractLedger` keeps live *claims* and completed *signings* separately, because a claim expires when a
player cancels but a signing does not — and the day rule must outlive the claim.

The hard part is that either peer could enforce a rule about only its own claims and both would believe
they won. So the arbitration lives in the **supervisor**, which is the only party that sees both sides:

```
Msg::Contract { person, team, day, kind }    kind: 0 claim, 1 release, 2 signed, 3 refused
```

`arbitrate()` in `coop-supervisor/src/main.rs` maps `kind` onto the ledger and refuses with the ledger's own
message:

| situation | message the player sees |
|---|---|
| already signed today | "that person was already signed today" |
| other player negotiating | "the other player is negotiating with that person" |
| repeat from the same player | "your offer is already in progress" |

Three details that are easy to miss and are tested:

- **The day rule is re-checked at *confirm*, not only at *claim*.** A negotiation can span a day boundary,
  so checking only when the claim was granted permits a double signing.
- **A repeat claim from the same player is reported as a duplicate, not a conflict.** Otherwise a
  double-click looks like the other player interfering.
- **A remote signing that collides is surfaced, not overwritten** (`observe_remote_signing` returns false).
  One of the two players has already been told it worked; silently winning is how that becomes a bug
  report with no evidence.

## 4. Both votes count

**Rule:** every player's vote on a proposal is recorded, and the result is computed from all of them. A
proposal does not resolve until everyone has voted.

`VoteBox` keys votes by `(proposal, side)`, so:

- a player changing their mind **replaces** their vote rather than adding one — otherwise the player who
  clicked most would win;
- `result()` reports `complete()` (all expected votes in), `winner`, and `tied`;
- a tie yields `winner: None` and `tied: true` rather than picking a side.

That last point is the whole requirement. "Both votes counted" and "one vote was discarded to break a tie"
are different outcomes and a player will notice the difference.

**Wire and arbitration:** `Msg::Vote { proposal, option }` is recorded by the supervisor, which resolves
only when `complete()` and sends the outcome back to both sides in the same frame. A resolved result is
encoded as a `Vote` with option `0xFF` — outside any real option — so a peer that does not understand the
sentinel rejects it rather than applying a phantom option.

## Where each rule runs

| rule | decides | why there |
|---|---|---|
| clocks | each peer, from the same inputs | a union is computable on both sides; routing it through the supervisor would add a round trip to every pause and make a stall look like an unfreezing game |
| team authority | each peer, per merge | ownership is known locally; the merge is the only place it is needed |
| contracts | supervisor | only the supervisor sees both claims, so only it can refuse the second |
| votes | supervisor | resolving on one side uses partial information and drops the other player's vote |

## Not done yet

- **The UI half is proven only headless.** `observe.js` turns the real events into frames, the agent turns
  frames into wire traffic, and the far side now applies them (`clocksync.rs` → `SimPauseChange`), all tested
  (67 JS assertions, 16 end-to-end over real sockets, 9 gate tests). What has still never been seen is a
  player pressing pause in the running game and the other machine's sim holding. That needs a launch.
- **The 0xFF vote sentinel is a stopgap.** A dedicated `VoteResult` message would be cleaner; it was
  avoided here so an older peer fails safely rather than mis-parsing.
- **Regulation votes are not a dedicated screen.** They are two-choice dilemma mail, so the vote hook is
  `SetDilemmaResponse(mailId, responseIndex)` (`InboxDilemmaButton.js:53`), which also fires for the
  in-race comms dilemmas. Harmless — the supervisor tallies per proposal id — but it means a pit-options
  reply is sent as a vote frame too. Distinguishing them would need mail state the event does not carry.
- **The claim is issued at `OnSubmitContractOffer`, not before it.** The other player can therefore
  already be mid-negotiation with the same person; they are refused at confirm (`OnConfirmStaffContract`)
  rather than blocked from starting. Blocking earlier would need an event that fires when the
  negotiation screen opens, and the export has none.
