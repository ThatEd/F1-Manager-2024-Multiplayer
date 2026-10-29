# Tyre sync: the mapping, and why it is staged

Written before any tyre value was written into a live sim, because the previous generation wrote first
and crashed:

> **NOTES.md:1319** - "Removed: `RcState` / `RfState` writes. Both are read and logged only. This is the
> change most likely to fix the crash."

That is the lesson this document is built around. Tyre sync is therefore staged, and each stage has an
exit criterion. Nothing advances on a hunch.

---

## 1. What is already true (verified, not assumed)

| part | status |
|---|---|
| `Car.tyres[4]` with `surface`/`carcass`/`condition` | exists |
| wire format carries `tyre_lo`, `tyre_hi`, 4x3 floats | **already encoded and decoded** |
| `layout::car_at` decodes all of it | already |
| field offsets inside the car slot | verified: last tyre offset `0x972760`, slot ends `0x972E90` |
| mailbox write widths | 1/2/4 bytes, added for race control |
| entry budget | 384 entries; full tyre state needs 3 + 88 + 264 = 355, so it fits |

**So the host and the protocol need no changes.** The only thing that ever ignored tyres was the apply
side, which is where the staging below lives.

Offsets, per wheel (stride `0xC` from `TYRE_SURFACE0 = 0x97C`):

```
+0  surface    f32   temperature, degrees C
+4  carcass    f32   temperature, degrees C
+8  condition  f32   1.0 = fresh, 0.0 = dead
```

## 2. The reason for caution, stated plainly

From [RNG-DETERMINISM.md](RNG-DETERMINISM.md):

- tyre **temperature** is an *input* to the puncture curve
  (`Tyres/TyreWear/PunctureChancePerLap.json`, §1.3) - a puncture is ~25%/lap at zero wear and ~0 beyond
  22% wear, and the curve argues it is keyed on temperature rather than wear;
- **wear** accumulates from driving, i.e. it is *derived* from the simulation, not supplied to it;
- race-day randomness is limited to mechanical faults, which are wear/temperature driven (§6).

So tyre state is **derived**. Writing it from outside is not replication, it is overriding the sim. Two
concrete hazards follow:

1. writing `condition` makes the client's wear model disagree with its own driving history - the tyre
   looks worn but nothing in the client's sim did the wearing;
2. writing `surface`/`carcass` feeds a curve that decides punctures, so a wrong value can *cause* a
   puncture or suppress one. That is a gameplay divergence, not a cosmetic one.

Neither is a reason never to do it. Both are reasons to do it in the narrowest form that solves the
reported problem, and to look at real numbers first.

## 3. The stages

Mode is read from `coop_tyres.txt` next to each game exe, at run time, so changing it needs no rebuild.
Unknown content resolves to `observe` - a typo must never enable the risky path.

### Stage 0 - `observe` (current default, writes nothing)

`stage_into_with_rc_and_tyres` emits zero tyre entries. The agent instead logs, once a second:

```
tyres observed (surface/carcass/condition per wheel):
  car0  90/85/0.95 88/83/0.94 92/87/0.93 91/86/0.92
  ...
  surface range 71.2..103.8  carcass range 68.4..99.1  out-of-range values: 0 of 88
```

**Exit criterion:** real ranges are known, and the out-of-range count is 0. If it is not 0, the bounds in
`off::tyre_range` are wrong and must be corrected *before* any write - a bound that rejects legal values
means the mapping is misunderstood.

### Stage 1 - `wear` (writes `condition` only)

Wear is the reported symptom that matters: a client with fresher tyres than the host gets different
grip, and the pit-strategy picture diverges. Wear is monotonic and slow, so a periodically-corrected
value converges instead of fighting.

Temperature stays local, because it is the puncture-curve input and it swings second to second with
driving style - writing it means the two sims disagree about *why* a tyre is hot.

**Exit criterion:** a full stint with no crash, and the client's tyre-wear display tracking the host's.
This is the stage where a crash would most likely appear, so it is a stage of its own.

### Stage 2 - `full` (adds both temperatures)

Only if stage 1 is clean. Expect this to change puncture behaviour; that is the point and also the risk.

## 4. What makes a bad value harmless

Every value goes through `plausible(v, bounds)` before it is emitted:

| field | accepted band |
|---|---|
| `surface` | -40 .. 400 |
| `carcass` | -40 .. 400 |
| `condition` | -0.01 .. 1.5 |

A value outside its band is **skipped, not clamped**. A clamped value is still a lie the sim acts on; a
skipped field leaves the client's own model in charge of something evidently not understood.

`NaN` is rejected without a special case, because it compares false against any bound. That matters more
than it looks: `NaN` written into a float the game divides by is the cheapest way to produce a crash that
looks unrelated to its cause, and it is exactly the class of value a misread offset produces.

Tests covering this: `implausible_tyre_values_are_skipped_not_clamped`,
`tyre_bounds_reject_the_obvious_nonsense`, `observe_mode_writes_no_tyre_fields`,
`wear_only_writes_exactly_the_condition_floats`, `a_full_frame_fits_even_with_every_tyre_field`.

## 5. Ordering inside a frame

Race control, then positions, then tyres. If a frame ever overflows, the least critical part is lost:

```
1  race control   (3 entries)    - a dropped red flag desynchronises the session
2  positions      (88 entries)   - a dropped speed is cosmetic
3  tyres          (264 entries)  - largest, slowest-changing, least urgent
```

`stage_into_with_rc_and_tyres` refuses an over-large frame rather than truncating it, so this ordering is
about *which* part would be at risk if the budget were ever reduced - not about partial frames.

## 6. What this does not fix

Tyres were one of three reported problems:

| symptom | cause | status |
|---|---|---|
| red flag not synced, cars stuck | `Msg::Rc` never sent | **fixed**, needs a live test |
| tyres not synced | field set | staged above |
| damage not synced, broken cars racing | `RaceSimCarIncident` reachable only via the event bus | **not started** - needs `ERaceSimEventType`, see RACESIM-HIJACK §5.2 |

Damage is a different kind of problem and should not be bundled with this: it is not a field to write but
an event stream to listen to and re-fire.
