# Paper audit: does each co-op feature hold up against the game's actual formats?

Written 2026-10-30 while the mod is removed from the game and no launch is possible. The purpose is to find,
on paper, the places where a claim in the design does not survive contact with a real file or a real call
graph - which is cheaper than finding them in a race session.

Tagging is the same as everywhere else: **verified** (checked against real data or a real call graph, the
command is given), **inferred** (follows from the code, not executed), **unknown**.

---

## 1. Save format — the "opaque blob" claim was half right

`tools/save_format.py` parses real saves. The result corrects the premise the co-op design rests on.

| claim in the docs | verdict |
|---|---|
| "the save is opaque ... it cannot be edited" (`SAVE-MERGE-PLAN.md`) | **half wrong.** The *outer container* is Unreal's `GVAS` format, fully parseable. The *payload* is compressed. |
| "frontier-compressed (7.99 bits/byte, no SQLite or zlib marker)" | **verified.** Measured 7.999 bits/byte over the whole payload. |
| "the save travels byte-exactly as a blob" | **verified and unaffected.** A partly-understood container does not make the transfer wrong; it is still hashed and carried byte for byte. |

What parsing actually shows (`python tools/save_format.py`):

```
container: GVAS (Unreal Engine SaveGame)
  FSaveGameFileVersion : 3 (AddedSaveGameClassName)
  PackageFileVersionUE4 : 522
  PackageFileVersionUE5 : 1008 (ADD_SOFTOBJECTPATH_LIST)
  engine version        : 5.1.1-2147982291 branch='++volta24+game+1.11.0'
  custom versions       : 3
  header ends at offset : 116
payload: 3,172,667 bytes (100.00% of the file)
  entropy 7.999 bits/byte, zero bytes 0.47%
  verdict: a single compressed stream
```

So the header is 116 bytes and understood; everything after it is incompressible. **verified** on two
different saves (`autosave.sav` 3.2 MB, `save0.sav` 11.6 MB) with the same header size and the same verdict.

**A wrong turn worth recording**, because it is a general trap: an earlier version of the tool read the bytes
after the custom versions as a length-prefixed string and got a confident 21-character value, then walked a
bogus property list from it. A `u32` length can always be framed as a string; nothing checked the bytes were
text. Reading `15 00 00 00` followed by 21 binary bytes as "length 21 string" produced the garbage. The tool
now stops at the verified boundary and says so instead of inventing structure.

**Consequence for co-op:** save *transfer* is sound. Save *merge* (`SAVE-MERGE-PLAN.md`) is blocked on
decompressing the payload - not on the container. That is unchanged by this audit, but the reason is now
correct: it is one compressor, not an unknown file format.

---

## 2. Team authority — implemented, tested, and never called

This is the most serious finding of the audit.

`coop-auth` contains a complete authority model: `Mode{SharedTeam,Versus}`, `Scope{PerCar,PerTeam,World}`,
`may_write(mode, player, scope, row) -> Result<(), Denied>`, and `gate_sql(sql) -> Option<String>` which
classifies a statement and refuses writes outside the player's team. It has a large test suite covering the
versus and shared-team cases, including "a different player's car is refused".

**Nothing in the shipping code calls it.** Verified by searching the whole tree for `coop_auth`, `coop-auth`,
`gate_sql`, `may_write` and `scope_of` across `.rs`, `.py`, `.js` and `.toml`:

- every hit is either **inside `coop-auth/src/lib.rs`** (the implementation and its tests), or
- a **comment** in `coop-agent/src/lib.rs` asserting that the rules "decide whether a write lands".

That includes `may_write`, whose only callers are in `coop-auth`'s own tests.

**What that means.** The design says, in `coop-agent/src/lib.rs:1164`, that team claims are *shown* and that
"the mode rules in `coop-auth` decide whether a write lands". They do not. There is no write path from the
wire to the game's data that passes through `may_write`, so:

- the *replication* path (`hookmail::apply`) writes car fields by offset with **no team check at all**;
- the *SQL* path (`coop-mem`) has no `gate_sql` in front of it;
- in `Versus`, nothing stops a client writing another player's car except the fact that the client only ever
  receives and applies whatever the host sent it.

That last point is the saving grace and also the honest statement of the gap: today the authority is
**structural** (the client applies the host's snapshot, so it cannot push its own state) rather than
**enforced** (a rule that refuses a write).

Checking the one path that *could* originate a client write removes the alarming half of this finding: the
player-instruction message `Msg::Orders` is defined, encoded and decoded in `coop-proto`, and referenced
**only in tests** - `coop-link` never sends it and the agent never applies it. Verified by searching every
crate for `Msg::Orders`: three hits, all in `coop-proto` (the enum, the encoder, the decoder) plus the test
that round-trips it. So there is currently **no client-originated write path to protect**, which is why the
absent `may_write` call has not caused a visible bug.

### Fixed: the two write paths are now gated

Rather than delete a tested policy module or leave the comments lying, the gate is wired in at both points
where a value reaches the game:

| path | where | gate |
|---|---|---|
| SQL statement | `LiveDb::sql` (`coop-mem/src/livedb.rs:525`) | `coop_auth::gate_sql` - refuses writes to unclassified tables and to rows outside this player's team |
| car memory | `LiveDb::write_car` (`coop-mem/src/lib.rs`) | `coop_auth::may_write` with `Scope::PerCar` |

Both are placed at the **chokepoint**, not at the callers, so `coop-sql`, the transplant path and anything
added later are covered by construction. `coop-mem` already depended on `coop-auth` - it was linked from the
start and never referenced, which is precisely how the gap survived so long.

With no `coop_mode.txt`/`coop_player.txt` beside the exe, `configured()` returns `None` and nothing is gated,
so single-player repairs keep working. That behaviour is deliberate and documented in `coop-auth`.

**The car-to-team mapping is a stated assumption, not a verified fact.** `write_car` treats car indices `0..1`
as this player's own (the game fields one team with two cars) and everything else as another team's. If that is
wrong, the gate refuses writes that should land - the safe direction - and a session shows it immediately as
cars that do not move, rather than as a corrupted rival. **inferred.**

Two wiring tests guard it (`sql_is_gated_by_coop_auth`, `write_car_is_gated_per_car`). They read the source and
assert the call is present and that it returns before the write, because "this function calls that one" is not
otherwise expressible without a live game - and a missing call is exactly the failure being guarded. Removing
either gate fails its test; verified by planting both.

---

## 3. Wire protocol — frame handling is sound

Checked by reading `next_frame` and the client loop rather than by trusting the tests.

- `next_frame` refuses until the whole frame is buffered, so a partial read is never decoded. **verified**
  (`coop-proto/src/lib.rs:762`).
- Every frame carries a length and an FNV-1a checksum, and `decode` refuses a mismatch. **verified**.
- The client logs a gap when `tick != previous+1` but **still applies the frame** (`coop-link/src/main.rs:1002`).
  Correct over TCP - a gap means the log line, not a lost frame - so this is **not** a defect.
- Snapshot size is bounded by content: 14-byte header + tag + 4 + 4 + 4 + 1 + 22 × ~60 B ≈ **1.4 KB**, far
  under any practical limit, and `next_frame` does not cap the length field, which is safe because the
  validation is `buf.len() < total` rather than a fixed buffer. **inferred.**

---

## 4. Save transfer — wired end to end

Contrary to a suspicion going in, the session payload is complete: the host sends `SessionOffer`, then
`SessionChunk` with offsets, then expects `SessionReady` with a total and hash
(`coop-link/src/main.rs:844` and `:1227`), and the receiver refuses out-of-order chunks. **verified by call
graph.**

The blob it carries is a `GVAS` file whose payload is compressed - see §1. Because the transfer is byte-exact
and hash-checked, understanding the *content* is not required for it to work.

---

## 5. Memory layout — internally consistent

`layout::car_start(car) = CAR_BASE + car * CAR_STRIDE`, and `hookmail::apply` writes at
`base + offset` where `offset` came from `car_start(...)`, i.e. **object-relative**. The two agree, so a car
field lands in the intended array slot. Verified by reading both sides and by the existing test that stages a
known offset (`stage_into_lays_out_entries_little_endian`). **verified.**

The one thing this cannot be checked against on paper is the *values* of the offsets: they are empirically
derived, and a game patch invalidates them silently. The `plausible()` prologue check plus the tyre range
guards are the mitigation; they catch a moved function, not a moved field.

---

## 6. What is still unknown, and cannot be settled without the game

1. ~~Whether the offsets in `layout.rs` still match the installed build's `RaceSimManager`.~~
   **Partly settled without the game**: `SIMULATE_RVA` is verified against this exe (§18). The *field*
   offsets index a runtime object and still need a live read.
2. Whether a lobby actually appears and is joinable (needs a live Steam client).
3. Whether the UE4SS crash recurs with UE4SS disabled - `tools/ue4ss_isolation.py` answers that in one launch.
4. Whether the racesim hook fires in a real session (it has never been observed firing in a race).
5. **Whether `Raw StageProceed` actually starts a session.** The name is in the engine and absent from the
   UI (§18), which makes it the best candidate lever - but a name in a string table does not prove the
   Gameface binding accepts it as a trigger, nor that the handler advances the stage rather than doing
   bookkeeping. One launch with a race weekend loaded would settle it.

---

## 7. Shared clock — correct protocol, unreliable in practice (fixed)

The union rule is sound and unit-tested: `ClockGate` holds each side's reason set, `is_stopped()` is
`!host.is_empty() || !client.is_empty()`, and `apply()` reports the Stop/Run/NoChange transition. Pauses are
deliberately *not* arbitrated in the supervisor, because a round trip per pause would make a stall look like an
unfreezing game. **verified.**

The failure mode nobody had covered is a **lost frame**, and it is not theoretical:

- `Msg::Clock` carries one side's *complete* reason set, and `ClockGate::set` replaces that side's copy
  wholesale - so the next frame repairs a loss completely. **verified.**
- But the front-end sends a clock frame from exactly **one** place, inside the `SimPauseChange` wrapper
  (`bridge.js:669`). It reports transitions, not state. **verified.**
- And the agent's heartbeat emitted **no** clock frame at all. **verified** by searching every crate for
  `coord::clock_frame`: the only caller was the HTTP handler behind `/coop/coordinate/clock`.

So one dropped frame left the host running while the client stayed stopped, and **nothing retried it** - the
divergence would last until that player happened to pause again. Over TCP a loss is rare, but the supervisor
does drop frames deliberately when a socket write times out (two-second bound, "never retry a peer that
failed"), and a reconnect drops everything queued behind it.

**Fixed:** `coop-agent` now re-publishes the pause state from its heartbeat once a second, *only while paused*,
so idle play pays nothing. That turns a loss into at most one second of divergence instead of a lost race.
This is what the protocol comment always claimed ("full state, not a delta: a lost unpause would leave the
world frozen with nothing to indicate why") - the state format was built for it, the sender was not.

`tools/headless/clock_convergence_test.js` guards it: 18 assertions, including that the repeat still exists.
Removing the re-publish fails one (17/18), which is the check that matters.

---

## 8. Contract claims — a lost claim fails *safe*, and that matters

The same "who retries?" question as §7, with a different and much better answer.

`observe.js` emits a contract frame from `OnSubmitContractOffer` (a claim), `OnConfirmStaffContract` (the
signing) and `OnCancelStaffContract`/`OnFailedStaffContract` (releases). Each is an event-driven single send -
`bridge.js:672` posts it once and never repeats. **verified.**

If a **claim** frame is lost, the supervisor never records it, so it cannot refuse the other player's claim
either: both players negotiate with the same person. The loser finds out at *confirmation* time, when the
supervisor sees the `confirm` and one of them is refused. That is a worse experience than being refused up
front, but it is **safe** - it cannot silently produce two signings.

If a **release** frame is lost, the claim stays held and the other player is refused a person who is actually
free. Annoying, self-limiting (the claim expires when the day rolls or the offer is resolved), and it errs
towards refusing rather than duplicating.

If a **signing** frame is lost, the ledger does not learn about the signing. The danger would be the other
player signing the same person the same day - but the game itself refuses that, because both peers are running
the same career and the signing already happened in the shared save. The supervisor's ledger is a *second* line
of defence, not the only one.

**Verdict: no fix needed, and deliberately not the same treatment as the clock.** Re-sending a claim is not
idempotent in the way re-sending full pause state is: "claim person 9" repeated after the claim already won is
harmless, but "confirm person 9" repeated after a *refusal* would keep re-presenting a decision that has been
made. The clock is full state and self-repairing by construction; a claim is an event with a lifecycle. Adding
retries here would be complexity in exchange for a better error message, and it risks re-opening settled
decisions - the opposite trade from §7, where the retry was free.

This is recorded rather than fixed so the asymmetry is a decision, not an oversight.

---

## 9. Tyre sync and command bytes — sound, and one crash risk removed

**Tyre sync is correctly guarded, and it is the model `coop-auth` should have followed.** Every value written
goes through `plausible(value, bounds)` against `off::tyre_range`, on *both* the staging side
(`hookmail::tyre_entries:204`) and the receiving side (`coop-agent/src/lib.rs:718`), and a value that fails is
**skipped rather than clamped** - a clamp is still a lie the sim acts on, whereas a skip leaves the client's
own model in charge of something evidently not understood. `NaN`/`inf` are refused separately for motion
fields. **verified.**

`Entry.width` is correct at every construction site: 1 for the state bytes (`RACE_CONTROL` carries `(offset,
width)` pairs explicitly), 4 for floats, and `apply` skips any width it does not recognise rather than guessing
- which matters because `RF_STATE` sits at `0xA13289`, one byte from the end of the object, so a 4-byte store
there would write past it. **verified.**

**Command bytes: defined but not wired.** `layout::cmd::ALL` (nine action bytes: stay-out, garage release, pit
request, wing, tyre set) is defined and documented as "taken from the proven Lua prototype", but nothing in
`coop-agent` stages them - the same finding as `Msg::Orders`, and consistent with it. So there is no
client-originated command path to range-check yet. When there is one, the check belongs where the tyre check
already is: at staging time, skipping implausible values. **verified absent.**

**One crash risk removed.** `Mailbox::drain` clamps the entry count but indexed the buffer directly:

```rust
unsafe { apply(base, &all[start..start + BUFFER_LEN], n) }
```

`start` derives from an `AtomicUsize` that is only ever stored as `0` or `1`, so this cannot trip today - but
it runs on the **game thread inside the sim call**, and a slice index out of range *panics*, which with
`panic = "abort"` takes the process down. A corrupt atomic should cost one frame, not the game. Now uses
`get(..)` and treats a miss as zero entries. **fixed.**

---

## 10. Team retarget — the persistence step the design depends on is not written

`TEAM-RETARGET.md` makes a single row the basis of Versus: `Player.TeamID` is "one integer on one row", and the
whole management UI scopes itself off `playerTeamID`, so changing it gives the client team B's entire command
surface with no per-team allowlist. That reasoning is sound and I did not find a flaw in it.

Its **implementation plan** has a gap. The documented sequence is:

> 1. **Edit then reload.** Do `UPDATE`, then `LoadGame` again from the front-end. The engine re-reads `Player` on
>    load and republishes. Costs one extra load, uses only proven mechanisms. **This is the recommended first
>    implementation.**

`LoadGame` re-reads the save **from disk** - documented elsewhere in this tree, and the only thing consistent
with a save/load screen reading `.sav` files. So reloading discards an `UPDATE` that only ever landed in the
live in-memory SQLite database. `coop-retarget` does exactly that and stops:

```rust
match db.sql(&script) { Ok(_) => println!("  applied"), ... }   // BEGIN; UPDATE Player ...; COMMIT;
```

`COMMIT` here commits to the runtime database inside the game process. **Searching the whole tree for anything
that writes a save finds nothing**: no save call, no serialisation, no file write of a `.sav`. So as written,
step 1 of the recommended plan cannot work - the edit is lost on the reload it uses to take effect.

**What the evidence says should happen instead.** The game does autosave, so a DB edit is *probably* captured on
the next save cycle rather than needing a manual one:

```
autosave.sav        3172783  2026-10-30 05:24:26
customisation.sav   2086025  2026-10-30 05:24:15
```

Two files rewritten eleven seconds apart, today - the engine writes them itself. And every `.sav` in the folder
uses the same `GVAS` container with the same version header, `customisation.sav` and `player.sav` included, so
autosave and manual saves are not different formats that could capture different state. **verified** (header
bytes and timestamps).

So the correct sequence is likely: `UPDATE` → let the engine autosave (or force it) → reload. That is a *later*
reload than the design assumes, and it is unverified: whether the autosave is triggered by the calendar
advancing, by leaving a screen, or on an interval is **unknown**, and whether it captures a foreign `UPDATE`
made behind the engine's back is **unknown**.

**This matters more than it looks,** because the same question governs every other DB edit the mod makes -
contract claims, retargeting, anything through `coop-sql`. If a runtime `UPDATE` is never persisted, then all
of it is session-scoped, and the two peers' databases diverge from the save they came from.

**What to do first when the game is available:** `UPDATE Player SET TeamID = <other>`, note the clock, wait for
the next `autosave.sav` mtime to change, then read the file back after a reload. That is one session and it
settles persistence for every DB write in the design. Until then, `TEAM-RETARGET.md`'s step 1 should not be
described as "uses only proven mechanisms" - the reload it relies on is the step that undoes it.

---

## 11. Save slot naming — the documented 30-name space is not the file space

`UI-COMMANDS.md:777` states, correctly reading `Saves.js:12`:

> `so the key space is exactly autoSave, save1 … save29 — 30 names.`

That is true of the **DataStore**, and I verified it in the real export (`Saves.js:12-24`: a loop to 30,
`contextID = i == 0 ? 'autoSave' : 'save' + i`, with `slotName` set to that context id). Every
`LoadGame`/`SaveGame` call sends that string - `LoadSaveScreen.js:127-128` and `:148-149` both read
`slotName` from the selected slot context and pass it straight to `sendEvent`.

It is **not** true of the files. The real save folder holds:

```
numeric slots: 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 29, 165, 166, 167, 168
plus: autosave.sav, customisation.sav, player.sav
```

`save165`–`save168`, `save0` and `customisation.sav` are outside the documented set. So `slotName` is not
decoded by the engine as "one of a fixed 30" — it is mapped to a filename over a wider range, and one of the
non-numeric names (`customisation`) has no DataStore context at all.

**Correction to my first reading of this.** I initially concluded a co-op session should therefore use an
*observed* name, because an invented one might never be loadable. That was too cautious, and the other profile
folders show why: `SaveGames4` holds `cardemir.sav`, `F2CC.sav`, `BTG_24.sav`, `BTG_PreSeason.sav` - user-chosen
names with no relation to `save<N>`. **The file layer accepts arbitrary names**, so naming a co-op transfer
something of its own is fine.

The real constraint is the other direction, and it is the one that bites: the name has to be **visible to the
save/load screen and loadable via `LoadGame`**, which reads `slotName` from a DataStore context the UI declares.
Only the 30 declared contexts are in that palette. So `coop_session.sav` may sit correctly in the folder and
still not be reachable through the UI's own load path - unless the name matches a declared context, or the
save/load screen enumerates the directory rather than a fixed list (which those odd names hint it might).
**unknown**, and cheap to settle live.

**Recommendation unchanged in effect:** write the transfer under an observed, declared name (`save<N>` for a
free N, or overwrite `autosave`), because that is proven to load. Anything else is a hypothesis.

**Also verified here, and a separate bug:** the received save does land in a `SaveGames*` folder, but *which* one
was hardcoded - see §19. The transfer itself is hash-checked before the write (`main.rs:1251-1267`).

---

## 12. A protocol version mismatch was reported as an unreadable frame

`Msg::TeamClaim` moved `VERSION` from 2 to 3 (`coop-proto/src/lib.rs:37`). The check itself is strict and
correct - `decode` refuses anything that is not exactly this version (`:659`), before it trusts the length or
the checksum - and the error even carries the reasoning: `ProtoError::BadVersion(v)` displays as
`peer speaks version 2, this build speaks 3` (`:354`).

The agent then threw that away one line before the user saw it (`coop-agent/src/lib.rs:214`, before the fix):

```rust
Err(_)     => "unreadable frame from supervisor".to_string(),
```

So a peer running an older build - the **ordinary** case, because the two machines are updated separately and
this session already shipped a version bump - produced the same message a random corrupt packet produces. That
is the same failure class as the Steam-invite bug fixed earlier: the specific reason existed, was correct, and
was discarded at the last step, so the user had nothing to act on. Worse here, because "unreadable frame"
points at the network while the real cause is a build mismatch.

Fixed: `BadVersion` is now named, with the peer's version and the action to take. A frame that is genuinely not
ours still says "unreadable" and still carries its reason, because removing that would be solving the wrong
problem. Two tests pin both halves; planting the old arm back makes the first one fail.

**A verification mistake worth recording.** My first attempt to prove the test could fail reported it *passing*
with the regression planted. The plant had silently done nothing: `.NET` file APIs ignore PowerShell's working
directory, so `[IO.File]::ReadAllText('crates\...')` resolved against the process CWD and **failed**, and the
subsequent write went to the wrong place. Had I trusted that output, I would have shipped a test I believed was
checked and never was - the exact "claims proven that were not" failure this audit exists to prevent. Re-done
with absolute paths, the test fails without the fix and passes with it.

---

## 13. Car-id bounds: no live hole, but the chokepoint trusted its input

The question was whether a hostile or corrupt frame from the other player could make the mod write outside the
sim object. Tracing it end to end:

* `Msg::Snapshot` decodes a car count and **refuses anything but exactly `CAR_COUNT`** (`coop-proto/src/lib.rs:683-686`), so a peer cannot ask for extra cars.
* The per-car `id` inside the payload is **not** range-checked by `decode_car` (`:483` reads it as a raw `u8`).
* `write_cars` skips ids past `CAR_MAX` (`coop-mem/src/lib.rs:523-525`), so that path is safe.

So there is **no live out-of-bounds write today**. But `write_car` is `pub` and computed
`car_start(car.id as usize)` with no check of its own - and `car_start` is unchecked arithmetic
(`layout.rs:222-224`), unlike the read path `car_at`, which refuses `car >= CAR_MAX` (`:157-159`). An id of 255
reaching `write_car` by any other route would land about `(255-22) * 0x10D8` - roughly **1 MB** - past the car
table, i.e. into whatever the allocator put next, and the write would report success.

The guarantee sat in one caller rather than at the chokepoint, which is the same mistake as `coop-auth` (§3): a
documented rule that holds only while every future caller remembers it. Fixed by refusing `id >= CAR_MAX` inside
`write_car`, before any address is computed or written, and before the auth gate, so a bad id is reported as a
bad id rather than as an authorisation failure. A test asserts the bound exists, precedes the write, and
returns rather than clamping to the last car.

**Not changed, deliberately:** `decode_car` still accepts any `u8`. Rejecting an out-of-range id at decode time
would also be defensible, but it would turn a single bad car into a rejected frame and drop a whole snapshot;
refusing the one car loses less. Either way the write chokepoint is now the thing that is actually enforced.

---

## 14. The page guard did not guard the event, so `Raw` bypassed it

Firing `LoadGame` or `ContinueGame` from an in-game document is the one operation this project knows destroys the
session: it tears the front-end down and never rebuilds it, leaving a black screen with the process alive
(`LOBBY-GATE.md`, and reproduced on a live client). `bridge.js` enforces this with

```js
var LOAD_FROM = ['Frontend'];
function allowedFor(cmd, page) { ... }      // bridge.js:322, before the fix
```

and refuses with "refused on <page>" - which is the right shape, and correct as far as it goes.

**It checked the command word, not the event.** The grammar also exposes `Raw <event> [args]`, documented in
that same file as "any Gameface event by name" (`:336`). `Raw` resolves its argument to `eventName`
*after* `allowedFor` has already passed, because the command it is matched on is `raw`, and `raw` is legal on
every page. So

```
Raw LoadGame autosave        (sent while RaceWeekend.html is the loaded document)
```

passed the guard, reported success, and reached `engine.trigger` with exactly the event the guard exists to
prevent. The protection covered the documented route and not the obvious one, and `Raw` is precisely the escape
hatch an operator reaches for when the normal command does not work - so this was likely to be hit in practice
rather than a theoretical hole.

**Fixed** by adding a second check at the point where a command has resolved to the event it will actually
send, keyed on the event name:

```js
var FRONTEND_ONLY_EVENTS = { LoadGame: LOAD_FROM, ContinueGame: MENU_FROM };
function eventAllowedOn(eventName, page) { ... }
```

Derived from `LOAD_FROM`/`MENU_FROM` rather than restated, so the two cannot drift. Both checks are kept: the
command check refuses earlier with a message naming the page, and the event check catches every other route -
including `Raw`, and including any command added later that forgets to register itself.

The guard also had to move **above the control-press path**, which returns early: pressing the real button is a
different route to the same dispatch, so checking after it would have left the same class of hole open.

**Tested** by `tools/headless/bridge_guard_test.js` (19 assertions), which extracts the real `allowedFor` and
`eventAllowedOn` from the source rather than restating the rule, and additionally asserts that `dispatch()`
calls the guard *before* both `engine.trigger.apply` and the press path - because a correct function that is
never called is the failure mode that actually occurred. Removing the guard call makes two assertions fail.

**Two process notes, since both cost time.** My first version of the test compared
`indexOf('eventAllowedOn') < indexOf('engine.trigger.apply')` inside a fixed 6000-character window; the trigger
lay just outside it, `indexOf` returned `-1`, and the assertion failed for a reason that had nothing to do with
the code. Slicing to the end of the function fixed it, and the test now asserts the trigger was *found* before
comparing positions, so a moved trigger cannot make the check vacuous. Separately, the suite reported FAIL while
printing `19/19 assertions passed`: the runner parses `N/M passed`, and I had written `19/19 assertions passed`,
which its regex does not match. The summary line now uses the exact shape the runner reads.

---

## 15. `Advance <n>` sent n+1 events, which in a shared session skips a race

Same branch as §14. `Advance <n>` is documented as sending the advance event *n* times, spaced, because a single
`AdvanceToNextEvent` can be swallowed by the UI with no error. The extras were emitted by a loop and then the
tail of `dispatch()` sent one more through the preferred route, so the two together fired **n+1**:

```js
for (var i = 0; i < times; i++) { ... setTimeout(..., n * 1200) ... }   // n extras, one at delay 0
eventName = 'AdvanceToNextEvent';                                       // and the tail sends one more
```

Why this is not a cosmetic off-by-one: on the calendar screens `AdvanceToNextEvent` does not merely refresh a
view, it **moves the career forward**. An unintended extra advance steps past an event - and in a shared session
that skips a race weekend for *both* players, from a command one of them issued. It also silently contradicted
the documented count, so an operator asking for three advances got four and had no way to see it: each trigger
looks identical in the log, and the counter reported `times - 1` extras because that matched the loop rather
than the total.

Fixed: the loop now sends `times - 1` spaced at `(n + 1) * 1200`, leaving the tail to send the last one through
the preferred route. That keeps the useful property that one of the n goes via the real control, and it no
longer races a delay-0 trigger against the final dispatch. Two assertions cover it - the loop bound and the
start delay - and reverting either fails the suite.

---

## 16. Framing: an unbounded length, and a resync that never resynced

Two bugs in the receive path, found by asking what a *malformed* frame does rather than a well-formed one.

**A declared frame length was trusted without limit.** `next_frame` read the length from the header and waited
for that many bytes (`coop-proto/src/lib.rs:769`). A peer - or one corrupt length field - claiming `0xFFFFFFFF`
made the splitter answer "need more bytes" for ~4 GiB. The agent tolerated this because it caps its own buffer at
1 MiB and clears beyond it (`coop-agent/src/lib.rs:599`); **`coop-link` has no cap at all**, so its
`Vec::extend_from_slice` grew without bound. That is memory exhaustion reachable from the other machine, whether
deliberate or a bit flip. `MAX_FRAME` (1 MiB) now refuses an impossible length up front - and returns an
*error* rather than `None`, because callers resynchronise on error and accumulate on `None`.

**The resynchronisation path never resynchronised.** `split_frame` documented that on a corrupt header
"dropping the buffer is the only recovery", then returned an empty frame for the caller to `drain(..0)` - which
removes nothing. The corrupt header stayed at the front of the buffer forever, and every subsequent call failed
on the same bytes. This was *latent* while an oversized length simply waited; combined with the new cap it would
have become a hot loop that spins on the same header and never reads another byte. `split_frame` now returns
`Result`, and the reader clears the buffer and reports how much it dropped.

The second bug is the more instructive one: the comment described the correct behaviour, the code did something
else, and nothing exercised the corrupt-header path because every test fed it well-formed frames. It only
surfaced by asking what the *failure* path does - and the first fix would have converted a slow failure into a
fast one. Tested by feeding bad magic and an over-cap length, and by confirming a genuinely partial frame is
still treated as "need more" so normal traffic is unaffected.

---

## 17. A test deadline that failed for reasons unrelated to the test

While running the full suite after a Rust build, `unit` failed once with `live suite -- timed out waiting for
server up`. Re-run alone, it passed; re-run after the build it passed again. The cause was a **fixed 12-second**
deadline for a cold Python server start (`tools/headless/run_tests.js:137`), which the preceding workspace build
had starved of CPU.

Not a product bug, and the honest temptation is to call it flakiness and move on. But a suite with a fixed cost
and a load-sensitive deadline is a suite that will fail for reasons unrelated to what it tests - and the failure
mode is corrosive: it teaches the person running it that a red result may be noise, which is exactly the
disposition that lets a real regression through. It had already cost one full 10-minute suite run here.

Fixed in two ways, both of which matter:

* The budget is now 60 s, overridable with `COOP_TEST_STARTUP_MS` for a slower machine, and the wait **stops
  early if the server process exits** instead of sleeping out the full budget on something already dead.
* A timeout now prints what the server said. "timed out" alone cannot distinguish *slow* from *crashed with a
  traceback* from *port already in use* - and the diagnostic was being collected and thrown away. Verified by
  forcing the budget to 1 ms: it reports `server output was: (nothing)` and fails cleanly, so the plumbing
  works and a real traceback will now be visible.

This is the same class of finding as §12 and §16: the information needed to explain the failure existed and was
discarded one layer before anyone could see it.

---

## 18. The session-flow lever: two engine events the UI has no button for

`README.md` claims the *UI* has no event that starts a race, but that the exe contains session-flow commands
the UI never sends - `StageProceed` and `TravelToRaceWeekend` - and that they are untried. That claim is
checkable from the exe alone, so I checked it rather than carrying it as an assumption.

**Both strings are present at exactly the documented offsets** (ASCII, this build):

```
0x05FA7C98  StageProceed
0x05FCAC28  TravelToRaceWeekend
```

and they sit inside a contiguous native **event-name table**, which is what makes the claim strong rather
than a coincidence of matching text:

```
0x05FA7C88  RaceViewChanged
0x05FA7C98  StageProceed
0x05FA7CA8  SessionOverviewClosed
0x05FA7CC0  AdvanceToNextStage
0x05FA7CD8  PreSessionClosed
0x05FA7CF0  AnalyticsClosed
0x05FA7D00  ConstructorStandingsClosed
0x05FA7D20  PostRaceSummaryClosed
0x05FA7D38  PostRace
```

These are the same strings the UI *does* send - `PreSessionClosed`, `SessionOverviewClosed`,
`AdvanceToNextStage`, `PostRaceSummaryClosed` all appear in the export's JS. So this is one enumeration of
engine event names, not a random pool of text, and `StageProceed` is a member of it.

**The discriminating measurement:** of the nine names in that table, the five the UI uses appear in the
Gameface JS (7, 2, 1, 2, 1 hits respectively). `StageProceed` and `TravelToRaceWeekend` have **zero** hits:

```
StageProceed               js_hits=0
TravelToRaceWeekend        js_hits=0
AdvanceToNextStage         js_hits=7
SessionOverviewClosed      js_hits=1
PreSessionClosed           js_hits=2
AnalyticsClosed            js_hits=1
ConstructorStandingsClosed js_hits=1
PostRaceSummaryClosed      js_hits=2
```

That is the README's claim, independently confirmed: these two are engine-side session flow with no control
bound to them anywhere in the UI, which is exactly why clicking everything on the pre-session screen never
started the sim. It also means the bridge's `Raw` command can dispatch events the shipped UI has no button
for - a capability the *game's own interface* does not expose.

**What this does not prove.** A name in the table does not establish that the Gameface binding accepts it via
`engine.trigger`, nor that its handler *advances the stage* rather than doing bookkeeping. `TravelToRaceWeekend`
is in a different section (`0x5FCAC28`), so it is a separate candidate, not part of the same block. Both need
one launch with a race weekend loaded to settle - now item 5 in §6.

This is a genuine advance on "we could not find any Gameface event that starts the sim": the export was the
wrong place to look for it, and the exe says which two names to try.

---

## 19. `coop-link` wrote received saves to a hardcoded folder, and the game keeps several

Chasing the slot-naming question turned up something with more bite. The game keeps a **set** of save folders,
one per profile. This machine has four:

```
SaveGames    files= 23  newest=2026-10-30 05:24:26   <- live: the engine autosaved here today
SaveGames2   files= 52  newest=2026-10-21 11:46:03
SaveGames4   files= 44  newest=2026-08-26 17:25:20
SaveGamesS   files= 28  newest=2026-10-21 12:44:35
```

`coop-link`'s `session_dir()` returned `...\F1Manager24\Saved\SaveGames` unconditionally. On *this* machine that
is correct - which is exactly why the bug would not have shown up here. On a client whose profile resolves to any
other suffix, the transfer would complete, pass its hash check, print `session saved to <path>`, and land in a
folder the game never reads. The success report is what makes it bad: the operator sees it work, and the next
step (`LoadGame`) simply cannot find the file, which points at the transfer rather than at the path.

Fixed by choosing the folder the way "live for this account" actually behaves: **the one whose newest `.sav` is
newest**, because the engine rewrites its own saves in the folder it is using while the other profiles sit
unchanged. Details that matter:

* Compare `.sav` mtimes, not the folder's - a folder's mtime changes when *we* write into it, so using it would
  let this module's own output reorder its candidates on the next call.
* A `SaveGames*` folder with no `.sav` is not a candidate: it has never held a career.
* Ties break deterministically (`SaveGames` first, then numeric suffix), so the result never depends on
  enumeration order, and `SaveGames10` sorts after `SaveGames2` rather than before it.
* With nothing to choose from it returns `SaveGames` anyway, so a later write fails with a real path in the
  message instead of silently using `.`.
* `COOP_SAVE_DIR` still wins when set - an explicit override should always beat inference.

Tested on a synthetic three-folder layout (newest wins, an empty folder is skipped, the empty case falls back)
and against the real machine, where the rule correctly picks `SaveGames`.

**Still unknown:** how the game itself decides. This infers from mtimes because the engine's selector has not been
reverse-engineered, and it would break if two profiles were both recently active. Said plainly rather than
papered over - and the one-launch check is to see which folder gains an autosave while the client plays.

---

## 20. One room or two? The career layer is designed and not built

The question put to me was whether co-op "works on paper through every format". Asking it precisely - *what state
actually reaches the other machine?* - gave a sharper answer than another pass over encoders, and it is the most
important thing in this document.

### What the wire carries

`coop-proto`'s `Msg` has 11 variants:

| message | carries |
|---|---|
| `Hello`, `Agent` | handshake, role, sim pointer |
| `Snapshot` | 22 cars' positions and state (race only) |
| `Clock` | pause reasons + tick |
| `Contract` | a contract-claim request for a person on a day |
| `Vote` | a regulation vote |
| `TeamClaim` | which team a player took |
| `SessionOffer` / `SessionChunk` / `SessionReady` | the save file, chunked |
| `Bye` | refusal |

**Not one of them carries management state.** No transfers, no car development, no finances, no staff, no
sponsors. `observe.js` says so in its own header: it turns what the player pressed into `Clock`, `Contract` and
`Vote` frames - coordination, not replication.

### The career is a snapshot, and the snapshot is the host's

`LOBBY-GATE.md:101-115` states the chosen model plainly: *"the host's career is the world"*.

1. Host plays, saves.
2. `UPDATE Player SET TeamID = <client team>`.
3. Host ships the save to the client.
4. Both press Continue, then advance together through the gate.

So both peers run **the same career DB**, shipped once and loaded into memory (§10: there is no `.sqlite` on
disk; the DB lives in the compressed `.sav` payload). The clock gate makes them advance together. That part is
coherent and implemented.

### What follows, and it is a real gap

If both run the same in-memory DB and nothing replicates transactions, then:

* The client **can** act on their own team - their `TeamID` was retargeted, so the whole management UI scopes to
  them, and `coop-auth` permits the writes. Their clicks genuinely change their own process.
* The **host never sees those changes.** The host's DB was never told about them.
* At the save boundary it is worse than "not shared": whoever saves writes *their* DB to the `.sav`, and the other
  player's entire month of work is **silently discarded**. There is no reconciliation, so the career can only ever
  hold one player's management history.

That is not a bug in an encoder - it is a missing layer. And it is precisely the layer `CAREER-COOP-PLAN.md`
§4.3 designs:

> **Do not ship table snapshots.** ... Ship the *statements*.
> `host: captures the SQL the game prepared → log { seq, tick, table, pk, sql, hash }` → client applies the same SQL

**That is not implemented anywhere.** I checked all eight crates (`coop-proto`, `coop-auth`, `coop-supervisor`,
`coop-mem`, `coop-agent`, `coop-inject`, `coop-link`, `coop-proxy`): there is no statement log, no SQL message
type, no `sqlite3_exec` capture hook, and no conflict resolution. `LiveDb::sql` is an *out-of-band* injector - the
mod composing its own SQL and pushing it through `sqlite3_exec` in the target - which is the opposite direction:
it sends statements *we* wrote, not statements *the game* wrote.

### How big the gap is, in one number

The engine's 346 distinct UI event names include a large batch of management verbs (`OnSubmitContractOffer`,
`DesignStageChanged`, `ManufactureStageChanged`, `Sponsor*`, `Parts*`, `EditStaff*`, ...). `observe.js` wraps
`engine.trigger`, so it *sees* every one of them, and then does this (`observe.js:30-36`):

> ```
> *   SimPauseChange(value, pauseLayer)
> *   OnSubmitContractOffer(terms, staffID)
> *   OnConfirmStaffContract(staffID)
> *   OnCancelStaffContract(staffID)
> *   OnFailedStaffContract(staffID)
> * Anything not in that list is observed and ignored.
> ```

Six events: one clock, four contract-arbitration, one vote. **No management verb is replicated.** That is a
deliberate choice recorded in the file, not an oversight, which is why it needs a decision rather than a patch.

> **A number in the design docs to distrust.** `CAREER-COOP-PLAN.md:154` says the game emits "267 career verbs",
> and the README said "352 events across 517 call sites". Neither reproduces. Measuring the export
> (`tools/event_surface.py`) gives **346 distinct event names and 373 call sites** across both call forms; the
> inline form alone is 103 names / 133 sites. The README is corrected; the 267 figure is still in
> `CAREER-COOP-PLAN.md` and should be re-derived before anyone sizes work from it. The conclusion here does not
> depend on it: whether there are 103, 267 or 346, **zero management verbs are replicated**.

It also means the missing layer has a producer but not a sufficient one: the wrapper can see that a player
pressed a career verb, but turning `DesignStageChanged` into the row the game will write is not something the UI
can tell you - the write happens inside the engine's SQLite. So statement replication needs a capture on the
engine's own prepare/bind path, exactly as `CAREER-COOP-PLAN.md` §4.3 assumes, and not merely a wider event list.

### The two modes in the UI, judged against that

`lobby.js:355-361` offers exactly two:

| mode | what the UI promises | does it work on paper? |
|---|---|---|
| **Shared team** | *"Two players, one team - the garage is split by car"* | **Partly.** Shared clock, shared calendar, shared race, no contract collision, per-car ownership enforced by `coop-auth`. **But** the two players' setup/development work does not cross, and only one player's version survives a save. |
| **Versus** | *"A team each, racing the other player. Shared calendar and results, your own garage."* | **No, not as "your own garage".** "Your own garage" means each player manages their own team - which is statement replication. It isn't built. |

And a contradiction worth naming: `coop-auth` implements a **third** mode, `CoopCareer` (`lib.rs:45`), which the UI
deliberately removed (`lobby.js:365`: *"Co-op career was removed: it is the same shape as Versus"*) - while still
accepting `"coop"`, `"career"` and `"coopcareer"` as its config strings (`lib.rs:65`). Meanwhile
`CAREER-COOP-PLAN.md` *recommends* model 2, "split-team, each client owns its own team's rows" - which with one DB
and no replication means "owning rows" amounts to "only the host's edits persist". The recommendation describes
the mode that cannot work yet.

### So, honestly

**Co-op did not work on paper for a full career, and the gap was one named, designed, unbuilt layer.** As of
§22 that layer's wire format and safe applier are built and tested; what remains is the **producer** and the live
call site.

Works today: two players in the same career, same clock, same calendar, same race weekend; car positions in a
live race; contract and vote arbitration; team ownership so the picker greys out what is taken.

Does not yet: **a client's management action reaching the host.** `Msg::Stmt` can carry it and `stmtapply` can
apply it safely, but nothing captures the game's own statements to put on that wire - that is a hook on the
engine's prepare path, which needs the game running.

**The next thing to build is that capture hook, not another encoder.** Failing that, the honest alternative is to
document that only the host's career edits count - in which case the UI should stop promising "your own garage"
in Versus, because it cannot deliver it.

---

## 22. `Msg::Stmt` — the career layer's wire format and applier, built

§20 ended with a named gap: nothing on the wire carried management state, so two players editing one career
diverged and the last save won. This closes the two halves that can be built and tested without the game.

### The message

`Msg::Stmt { seq, table, pk, set }` (`coop-proto/src/lib.rs`, `TAG_STMT = 15`) carries **a row identity and typed
values, not SQL text**, and that is a security decision rather than a style one. The only write path the mod has
is `LiveDb::sql`, which hands a single string to `sqlite3_exec` *inside the game process*. Accepting a peer's SQL
string would be arbitrary code execution against a live database from the other machine. Instead:

* `table` names the target, `pk` identifies the row, `set` holds `(column, StmtValue)` pairs;
* `StmtValue` is `Null | Int | Real | Text`, carrying the type explicitly because `"1"` and `1` are the same
  string and different SQL, and a career mixes integer ids, text names and float money in adjacent columns;
* `seq` is per-sender monotonic so a gap is visible;
* an unknown value tag is a decode error (`BadStmtValue`), never a guess - binding a guess writes a wrong value
  into a live career.

### The applier

`coop-mem::stmtapply` turns a message into exactly one statement: `UPDATE <table> SET <cols> WHERE <pk> = <int>`.
Three rules make string composition safe:

1. **Identifiers are allowlisted**, not trusted - `[A-Za-z_][A-Za-z0-9_]*`, max 64 chars. A table named
   `Player; DROP TABLE Player; --` is refused before any SQL exists.
2. **Values are rendered as literals by type**, text quoted with internal quotes doubled, so a payload stays a
   value. Non-finite floats become `NULL` rather than the words `NaN`, which SQLite would read as a column.
3. **The `WHERE` column comes from the local catalogue, not the peer** - the peer says which row by value, the
   local schema says which column identifies it. A peer cannot choose the predicate.

### Tested two ways, deliberately

The Rust unit tests assert the *shape* of the generated string. That is necessary and not sufficient: a subtler
escaping bug would still produce a plausible-looking string. So `tools/stmt_injection_test.py` runs the generated
statements through **real SQLite** and checks outcomes - 13 assertions including `7'); DROP TABLE Player; --`, a
leading-quote payload, backslash-then-quote, an overflow literal and the bare word `NaN`. The table survives,
unrelated rows are untouched, and every payload is stored as text. Registered as the `stmt` suite.

That distinction is the point: "the escaping looks right" and "`DROP TABLE` did not happen" are different claims,
and only the second is worth shipping on.

### What is still missing

The **producer**. `observe.js` sees 346 event names and turns six into coordination frames (§20); knowing a
player pressed `DesignStageChanged` does not say which row the game will write, because the write happens inside
the engine's SQLite. `CAREER-COOP-PLAN.md` §4.3 assumes a capture on the game's prepare path, and that is
native/UFunction work needing the game running. The layer is now reachable, not complete - and the README says so
rather than implying the career works.

---

## 21. The per-snapshot cost: 10 MiB read out of the game to send 2.4 KiB

Bandwidth was worth checking because a cache-coherency loop is an obvious objection to any co-op mod, and the
answer is that **the wire is fine and the local read is not**.

`encode_car` writes 112 bytes per car (`coop-proto/src/lib.rs:457-485`), so a 22-car `Snapshot` is **2,492 bytes**
including the 14-byte header. At the default `--hz 20` (`coop-link/src/main.rs:241`) that is **48.7 KiB/s, or
0.4 Mbit/s** - trivial for LAN, and 1.2 Mbit/s even at 60 Hz. Networking is not the constraint.

The read is. `read_cars` (`coop-mem/src/lib.rs:394`) reads `car_block_len()` bytes, and
`car_block_len() = CAR_BASE + CAR_MAX * CAR_STRIDE = 0x95BC00 + 22*0x10D8 ≈ 10.07 MiB`. So each snapshot copies
**10.07 MiB out of the game process to derive 2.43 KiB** - a ratio of roughly **4,200x**:

```
at 20 Hz  ->  201 MiB/s copied out of the game process
```

Three consequences, all of which are design problems rather than bugs:

1. **`read_object` is barely more expensive than `read_cars`.** The "cheaper" path exists to avoid race control,
   but `RF_STATE` sits at `0xA13289` - the very end of `OBJECT_SIZE 0xA132F0` - so reading the cars means
   touching 99.4% of the object anyway. Two functions with a 5% cost difference, one of which silently omits
   race control (`rc()` returns `None` rather than zeros, which is at least honest). Worth knowing before anyone
   optimises "use read_cars instead".
2. **`FRAME_GAP` is dead.** `main.rs:21` declares `const FRAME_GAP: Duration = Duration::from_millis(25);` and
   nothing reads it - the loop paces itself with `period` from `--hz` instead (`main.rs:751-754`). A constant
   named and commented as a rate limiter that has no callers is worse than no constant: it reads as though
   back-pressure exists, and a future reader tuning it would change nothing. The real cadence is `--hz`
   (default 20) or whatever a 10 MiB read per iteration permits, which the code reports live as `"{} Hz"`
   (`main.rs:743`) - the right thing to instrument. **Unknown** what it achieves in a race, because a 10 MiB
   read per frame may well dominate; one launch measures it.
3. **A sparse reader now exists, opt-in.** `read_cars_sparse` reads `car_record_used_len()` (≈3.8 KiB, derived
   from the field offsets rather than hardcoded) per car: **~83 KiB per snapshot instead of 10.07 MiB**, roughly
   120x less work, for byte-identical output. Exposed as `--sparse`, deliberately *not* the default until it has
   run in a live race, because the block read is the known-good path and a first live comparison should run both.

**Writing it went wrong once, and the test caught it.** My first version did the obvious thing - slice the record
out of the object and call `car_at(&slice, i)`. That always returns `None`: `car_at` computes
`CAR_BASE + car*CAR_STRIDE` from the buffer it is given, so inside a 3.8 KiB slice the offset is far past the
end. The two readers have genuinely different shapes and could not share `car_at`. Fixed by splitting the decoder
into `car_from_record(base, offset, id)` - which both now call, so they cannot drift - with `car_at` as the
address-computing wrapper. The regression test compares sparse against block across all 22 cars, including the
last tyre of the highest car, which is the field nearest the end of the span.

---

## Summary of what this audit changed

This is the same shape as the career gap in §20: the implemented thing works in isolation and the *cost model*
was never checked, so the design's stated constraint (bandwidth) is not the real one (a 10 MiB memcpy pipeline).

---

## Summary of what this audit changed

| finding | severity | state |
|---|---|---|
| a lost clock frame desynced the pause state permanently, because nothing re-published it | **high** | **fixed**: the agent heartbeat re-publishes while paused; guarded by an 18-assertion suite |
| `coop-auth` is complete and never called, while comments claim it enforces team rules | **medium** - nothing broken today, but the guarantee was absent | **fixed**: gated at both write chokepoints (`LiveDb::sql`, `LiveDb::write_car`), with wiring tests |
| **team retarget's "edit then reload" loses the edit**: `LoadGame` re-reads from disk, and nothing persists a runtime `UPDATE` | **high** - the recommended Versus implementation cannot work as written | recorded; the fix is a persistence step, and the first experiment is specified in §10 |
| **the documented 30-name save-slot space is not the file space** (`save165`–`save168` and `customisation.sav` exist outside it) | **medium** - a co-op session named with an invented `slotName` may never be loadable | recorded in §11; the design should use an observed slot name |
| **a protocol version mismatch was reported as "unreadable frame"**, discarding the reason that identifies it | **medium** - the ordinary failure after an update, misdiagnosed as a network problem | **fixed**: `BadVersion` is named with the peer version and the action; both halves tested |
| **`write_car` trusted its car id**: unchecked `car_start(id)` could target ~1 MB past the car table | low (no live path - `write_cars` filters) but the failure is corrupting unrelated game memory | **fixed**: bounds refused at the chokepoint, before address and before auth |
| **`Raw LoadGame` bypassed the page guard**: `allowedFor` matched the command word, and `Raw` resolves an arbitrary event name after that check | **high** - the documented black-screen was one `Raw` away on any in-game page | **fixed**: a second guard on the resolved event name, ahead of both the trigger and the press path; 22 assertions |
| **`Advance <n>` fired n+1 `AdvanceToNextEvent`**, and one of them at delay 0 | **high** - an extra advance moves the career, so a shared session silently skips a race weekend for both players | **fixed**: loop sends `times - 1`, tail sends the last; two assertions, both reverted to prove they fail |
| **a declared frame length was trusted without limit** | medium - memory exhaustion reachable from the other machine (`coop-link` had no buffer cap) | **fixed**: `MAX_FRAME` refuses it as an error, so callers resync instead of accumulating |
| **the resync path never resynced**: `split_frame` returned an empty frame that `drain(..0)` removed nothing of | medium - a corrupt header poisoned the stream forever; with the length cap it would have become a hot loop | **fixed**: `split_frame` returns `Result`, the reader clears and reports the drop |
| a live-server test had a fixed 12 s deadline that starved under load | low - but a red result that may be noise trains people to ignore red | **fixed**: 60 s, overridable, fails fast if the server exits, and prints its output |
| **`coop-link` wrote received saves to a hardcoded `SaveGames`**, while the game keeps one folder per profile | **high on another machine** - the transfer reports success and the save lands where the game never looks | **fixed**: picks the folder whose newest `.sav` is newest; tested on a synthetic layout and against the real one |
| **the career layer is designed and not built**: nothing on the wire carries management state, so both players edit one DB and only the host's edits survive a save | **critical for the stated goal** - "a full career together" is not achievable as built, and Versus advertises a garage it cannot provide | **partly fixed (§22)**: `Msg::Stmt` carries a typed row change and `stmtapply` applies it as one bounded `UPDATE`, proven against real SQLite. The **producer** (capturing the game's own statements) is still missing and needs the game |
| **~200 MiB/s is copied out of the game per snapshot** to build a 2.4 KiB frame (10.07 MiB read at 20 Hz) | medium - works, but the cost model was never checked, and a live test would blame the network | **fixed**: `read_cars_sparse` reads ~83 KiB per snapshot for identical output, behind `--sparse` |
| `FRAME_GAP` was a 25 ms "rate limiter" that nothing read | low - a name that implies back-pressure that does not exist | **fixed**: removed, with the reason recorded where it lived |
| `Mailbox::drain` indexed the shared buffer directly on the game thread | low (unreachable today) but the failure mode is the whole process | **fixed**: `get(..)` with a zero-entry fallback |
| save "opaque / unreadable" was imprecise: container is Unreal's, payload is compressed | low | corrected in this document and in `tools/save_format.py` |
| `SIMULATE_RVA` still points at the right function in the installed exe | - | **verified statically**: file bytes at the RVA match the live-read prologue exactly (`tools/verify_offsets.py`) |
| save transfer, wire framing, layout arithmetic, session payload, tyre guards, entry widths, contract-claim loss behaviour | **no defects found** | verified |

Four findings, four fixed or corrected, one new and open. The clock finding justifies the whole exercise: every
unit test passed, the state format was correct, and the feature still could not be relied on. Two claims I
expected to fail did not (save wire transfer, layout arithmetic); one did (`coop-auth` wired to nothing); and
one failure was in a place no test was looking (the clock sender).

The retarget/persistence finding is the one that changes a plan rather than fixing a bug: a documented
"recommended implementation" rests on a reload that discards the change it is meant to apply.

What remains genuinely open, and needs the game:

- whether a runtime SQL `UPDATE` is captured by the engine's autosave (§10) - **the first experiment**;
- the field offsets inside the runtime object, which no file can confirm;
- whether a lobby is joinable;
- whether the UE4SS crash recurs with UE4SS disabled (`tools/ue4ss_isolation.py`);
- whether the `write_car` team mapping in §2 is right.
