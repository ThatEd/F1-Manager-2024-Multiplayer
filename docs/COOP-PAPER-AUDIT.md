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

1. Whether the offsets in `layout.rs` still match the installed build's `RaceSimManager`.
2. Whether a lobby actually appears and is joinable (needs a live Steam client).
3. Whether the UE4SS crash recurs with UE4SS disabled - `tools/ue4ss_isolation.py` answers that in one launch.
4. Whether the racesim hook fires in a real session (it has never been observed firing in a race).

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

**Why this matters for co-op, concretely.** The bridge already uses `LoadGame <slot>` as its proven path to load
a chosen save. Two consequences:

1. Writing a save under a *made-up* `slotName` such as `coop_session` is **not** known to work: the engine has
   to map the string to a file, and every observed mapping is `save<N>`, `autosave`, `customisation` or
   `player`. A name it does not recognise could be ignored, or could create a file that the save/load screen
   never lists. **unknown** either way - and the honest thing is to use an observed name rather than invent one.
2. Conversely, `LoadGame` accepts context ids the UI palette does not show (`save165` has a file but no
   DataStore slot), so the receiving side's ability to load a specific file is broader than "30 slots" suggests.

**Correction for the design:** when the client receives a session payload, name it after an **observed** slot
convention (`save<N>` with `N` chosen to be free, or overwrite `autosave`), and drive the load with the matching
`slotName`. Do not assume a 30-name ceiling, and do not assume an arbitrary string maps to a file.

**Also verified here:** the received save does land where the game reads it. `coop-link`'s `session_dir()`
resolves `%LOCALAPPDATA%\F1Manager24\Saved\SaveGames` - the same directory the game and the save/load screen use -
and the transfer is hash-checked before the write (`main.rs:1251-1267`). So the *transport* is correct; only the
naming assumption was shaky.

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
