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

The risk is therefore future-facing but real: the first feature that lets a client originate a write - the
orders path, a strategy change, a contract signing via SQL - lands in a codebase whose comments say the
authority rules are already enforced, and they are not.

**Action for the next session:** before any client-originated write ships, either wire `gate_sql` in front of
`coop-mem`'s statement execution and a scope check in front of `hookmail::apply`, or delete `coop-auth` and
change the comments. Leaving a tested policy module uncalled while a comment claims it is enforced is the worst
of the three options, because it reads as done.

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

## Summary of what this audit changed

| finding | severity | state |
|---|---|---|
| a lost clock frame desynced the pause state permanently, because nothing re-published it | **high** - "both players pause together" is a headline feature and it could silently stop working | **fixed**: the agent heartbeat re-publishes while paused; guarded by a new 18-assertion suite |
| `coop-auth` is complete and never called, while comments claim it enforces team rules | **medium** - no client write path exists yet, so nothing is broken today; the next such feature would be unprotected | recorded here, not fixed; needs a decision |
| save "opaque / unreadable" was imprecise: container is Unreal's, payload is compressed | low | corrected in this document and in `tools/save_format.py` |
| save transfer, wire framing, layout arithmetic, session payload | **no defects found** | verified |

The clock finding is the one that justifies the whole exercise: every unit test passed, the state format was
correct, and the feature still could not be relied on, because the only thing that made it self-healing was
missing. Two claims I expected to fail did not (save wire transfer, layout arithmetic); one did (`coop-auth`
wired to nothing); and one failure was in a place no test was looking (the clock sender).
