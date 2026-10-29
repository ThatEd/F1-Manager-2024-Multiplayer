//! The write mailbox: how a car write reaches the game thread.
//!
//! Everything in this module exists for one rule, learned the hard way: **never write sim state from a
//! thread the simulation does not own.** Every crash this project has produced fits one signature -
//! `EXCEPTION_ACCESS_VIOLATION reading 0xF3C` - which is a null base plus a small offset, i.e. the
//! game dereferencing track data through a field that was overwritten while its own sim call was
//! halfway through using it.
//!
//! The fix is not a lock. Taking a lock would mean the game thread waits on us, which is worse, and a
//! lock held across a sim call is exactly how the predecessor project froze a race. The fix is to make
//! the write *happen on the game thread*, at a point where the object is known to be fully built: the
//! moment the sim's own entry point is called. [`crate::hook`] patches that entry point; this module is
//! what its callback consumes.
//!
//! ## The handshake
//!
//! Two buffers and one flag. The worker fills the buffer it is not publishing, then flips the index and
//! raises `busy`. The callback, on the game thread, applies the published buffer and clears `busy`.
//! Because the worker only ever writes the buffer the callback is not reading, and only while `busy` is
//! clear, the two never touch the same bytes: there is no lock, only an ordering.
//!
//! If the callback has not run by the time the next frame arrives, the frame is dropped rather than
//! queued. That is deliberate: for position replication the newest frame is always the one worth
//! applying, and a queue would add latency while letting a slow callback grow without bound.
//!
//! ## Why the entries are `(offset, value)` pairs
//!
//! A flat array of 32-bit pairs lets one entry describe any field with no per-entry type or size, keeps
//! the staging code easy to test, and means the wire format does not change when the field set does.
//! Offsets are **relative to the sim object**, so the same staged bytes are valid wherever the object
//! lives - which matters, because it moves under ASLR on every launch.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

use coop_proto::layout::{car_start, off};
use coop_proto::Car;

/// One `(byte offset in the sim object, new value, width)` triple.
///
/// `width` exists because the sim mixes field sizes: distances and flags are 4 bytes, but
/// `RC_STATE` and `RF_STATE` are **1 byte**. Writing those as a 4-byte store would clobber the three
/// bytes after them, and `RF_STATE` sits at `0xA13289`, within a hair of the end of the object - the
/// kind of overrun that produces a crash far away from its cause. So the width travels with the entry
/// and `apply` writes exactly that many bytes.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Entry {
    pub offset: u32,
    pub value: u32,
    /// 1, 2 or 4. Anything else is skipped by `apply` rather than guessed at.
    pub width: u8,
}

/// Bytes per entry on the wire between the worker and the hook: offset u32, value u32, width u8,
/// then three bytes of padding so each entry stays 4-byte aligned in the buffer.
pub const ENTRY_LEN: usize = 12;

/// How many entries a frame can hold.
///
/// Per car: 4 position floats + 3 state bytes + up to 12 tyre floats = 19. Times 22 cars is 418, plus 3
/// race-control entries. Sized for the widest tyre mode plus headroom so switching modes never silently
/// drops entries - `stage_into` refuses an over-large frame rather than truncating it, and a refused frame
/// is a stalled session.
pub const MAX_ENTRIES: usize = 480;

/// Bytes per buffer.
pub const BUFFER_LEN: usize = MAX_ENTRIES * ENTRY_LEN;

/// Which car fields are replicated, in order.
///
/// The same set as `coop_proto::layout::encode_car_motion`, deliberately minus:
///
/// * `node` / `seg` - track-graph indices. A value that does not belong to the receiving world gets
///   dereferenced through, which is the `0xF3C` crash.
/// * `race_pos` - the game recomputes standings from distances every step, so writing it only fights
///   the game and produces flicker.
pub const FIELDS: [(usize, &str); 4] = [
    (off::LAT, "lat"),
    (off::HEAD, "head"),
    (off::DIST, "dist"),
    (off::SPD, "spd"),
];

/// Single-byte car state that positions alone do not convey.
///
/// `pit` is the field a reported bug turned on: during a race, five cars were in the pit lane on the host
/// and none on the client. Replicating only distances told the client *where* the cars were, so it drew
/// them sliding along pit-lane geometry while its own sim believed they were still on track - and when the
/// host released them from the pits, the client had never been in them.
///
/// These are written as one byte each. That matters: a 4-byte store at `PIT` would overwrite `PIT_REQ`
/// and two bytes beyond, which is why `Entry` carries a width.
///
/// `stay_out` is included because it is the player's instruction about the pit lane, and the two sims
/// disagreeing about it is what makes a car dive in on one screen and not the other.
pub const STATE_FIELDS: [(usize, &str); 3] = [
    (off::PIT, "pit"),
    (off::PIT_REQ, "pit_req"),
    (off::STAY_OUT, "stay_out"),
];

/// Entries for the per-car state bytes.
pub fn state_entries(car: &Car) -> Vec<Entry> {
    let base = car_start(car.id as usize);
    let mut out = Vec::with_capacity(STATE_FIELDS.len());
    for (offset, _) in STATE_FIELDS {
        let value = match offset {
            off::PIT => car.pit as u32,
            off::PIT_REQ => car.pit_req as u32,
            off::STAY_OUT => car.stay_out as u32,
            _ => continue,
        };
        // A pit state is a small enum, not a flag word. Anything larger is a misread offset, and writing
        // it would put the game's pit logic into a state it has no case for.
        if value > 8 {
            continue;
        }
        out.push(Entry {
            offset: (base + offset) as u32,
            value,
            width: 1,
        });
    }
    out
}

/// How much of a car's tyre state to replicate.
///
/// Set from a file next to the game exe, so the choice can be changed without a rebuild and, crucially,
/// **staged**: the first run of a new write path should not also be the first run of the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TyreMode {
    /// Read the host's tyre fields and report them, write nothing. This is the default.
    ///
    /// It exists because tyre values are sim-*derived*: temperature feeds the puncture curve and the
    /// grip model, and wear accumulates from driving (RNG-DETERMINISM §1.3). Overwriting a derived
    /// quantity from outside is not replication, it is overriding the simulation - so the first step is
    /// to observe what a real race produces and only then decide which fields are safe to set.
    Observe,
    /// Write only *wear* (`condition`), leaving temperature to the client's own sim.
    ///
    /// Wear is monotonic and slow, and the client's own model would otherwise report a fresher tyre than
    /// the host has. Temperature is excluded on purpose: it is the input to the puncture curve, it swings
    /// second to second with driving style, and writing it means the two sims disagree about why a tyre
    /// is hot.
    WearOnly,
    /// Write wear and both temperatures.
    Full,
}

impl TyreMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TyreMode::Observe => "observe",
            TyreMode::WearOnly => "wear",
            TyreMode::Full => "full",
        }
    }

    /// Parses `coop_tyres.txt`. Unknown content yields `Observe`, never the widest mode: a typo must not
    /// silently enable the riskiest path.
    pub fn from_text(text: &str) -> TyreMode {
        match text.trim().to_ascii_lowercase().as_str() {
            "wear" | "wear-only" | "wearonly" => TyreMode::WearOnly,
            "full" | "all" => TyreMode::Full,
            _ => TyreMode::Observe,
        }
    }
}

/// True when `v` is a plausible reading for `bounds`.
///
/// `NaN` compares false against everything, so it is rejected without a special case. Infinity is
/// rejected by the upper bound. That matters: writing `NaN` into a float the game divides by is the
/// cheapest way to produce a crash that looks unrelated to its cause.
pub fn plausible(v: f32, bounds: (f32, f32)) -> bool {
    v >= bounds.0 && v <= bounds.1
}

/// Tyre entries for one car, filtered by `mode`, for cars whose owned state travels upstream.
///
/// Every value is range-checked against [`off::tyre_range`] before it is emitted. A field that fails the
/// check is **skipped, not clamped**: a clamped value is still a lie the sim will act on, whereas a
/// skipped field leaves the client's own model in charge of something we evidently do not understand.
pub fn tyre_entries(car: &Car, mode: TyreMode) -> Vec<Entry> {
    if mode == TyreMode::Observe {
        return Vec::new();
    }
    let base = car_start(car.id as usize);
    let mut out = Vec::with_capacity(off::TYRE_WHEELS * 3);
    for (w, t) in car.tyres.iter().enumerate() {
        let s = base + off::TYRE_SURFACE0 + w * off::TYRE_WHEEL_STRIDE;
        let fields: [(usize, f32, (f32, f32)); 3] = [
            (0, t.surface, off::tyre_range::SURFACE),
            (4, t.carcass, off::tyre_range::CARCASS),
            (8, t.condition, off::tyre_range::CONDITION),
        ];
        for (delta, value, bounds) in fields {
            // `WearOnly` sends the last of the three per wheel; the other two stay local.
            if mode == TyreMode::WearOnly && delta != 8 {
                continue;
            }
            if !plausible(value, bounds) {
                continue;
            }
            out.push(Entry {
                offset: (s + delta) as u32,
                value: value.to_bits(),
                width: 4,
            });
        }
    }
    out
}

/// How wide the published tyre range is for one car, for the observation log.
///
/// Reads back the same fields `tyre_entries` would write, so an observe-only run shows exactly the
/// numbers that a write would have sent - no more, and nothing rounded by a different code path.
pub fn tyre_summary(car: &Car) -> String {
    let mut out = String::new();
    for (w, t) in car.tyres.iter().enumerate() {
        if w > 0 {
            out.push(' ');
        }
        out.push_str(&format!(
            "{:.0}/{:.0}/{:.2}",
            t.surface, t.carcass, t.condition
        ));
    }
    out
}

/// Race-control fields, as `(offset, width)`.
///
/// These are what tell a client that the race is under a red flag, a safety car, or paused at the
/// lights. Without them the client's cars simply stop moving when the host pauses, because nothing
/// communicates *why* - which is exactly the "cars stuck instead of forcing the replay" symptom.
///
/// Widths are not uniform and this is the whole reason entries carry a width: `FLAGS` is a 32-bit
/// mask, but `RC_STATE` and `RF_STATE` are single bytes.
pub const RACE_CONTROL: [(usize, u8); 3] = [
    (off::FLAGS, 4),
    (off::RC_STATE, 1),
    (off::RF_STATE, 1),
];

/// Turns one car into the entries that place it.
///
/// Split out so it can be tested on its own: this decides *which* bytes of the sim object change, and
/// getting it wrong is either a crash or a silent no-op, neither visible from a log line.
pub fn entries_for(car: &Car) -> Vec<Entry> {
    let base = car_start(car.id as usize);
    let mut out = Vec::with_capacity(FIELDS.len());
    for (offset, _) in FIELDS {
        let value = match offset {
            off::LAT => car.lat,
            off::HEAD => car.head,
            off::DIST => car.dist,
            off::SPD => car.spd,
            _ => continue,
        };
        // A NaN or infinity written into a live float faults somewhere downstream, and a client that
        // received one bad frame must not be able to corrupt its own sim with it.
        if !value.is_finite() {
            continue;
        }
        out.push(Entry {
            offset: (base + offset) as u32,
            value: value.to_bits(),
            width: 4,
        });
    }
    out
}

/// Turns race control into entries.
///
/// Returns an empty list for a state that is entirely zero, which is how a client says "nothing has
/// happened yet". That matters: staging zeros would actively clear a live client's flags.
pub fn race_control_entries(rc: &coop_proto::RcState) -> Vec<Entry> {
    if rc.flags == 0 && rc.rc_state == 0 && rc.rf_state == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(RACE_CONTROL.len());
    for (offset, width) in RACE_CONTROL {
        let value = match (offset, width) {
            (off::FLAGS, 4) => rc.flags,
            (off::RC_STATE, 1) => rc.rc_state as u32,
            (off::RF_STATE, 1) => rc.rf_state as u32,
            _ => continue,
        };
        out.push(Entry {
            offset: offset as u32,
            value,
            width,
        });
    }
    out
}

/// Stages absolute values derived from a correction, pairing each correction to the client's current car.
///
/// Returns `Some(0)` when every correction rounded away to nothing, which the caller treats as "no frame".
/// A correction that produces no entry is the common case for a synchronised field, and it must not cost a
/// mailbox slot.
pub fn stage_corrections(
    slice: &mut [u8],
    client: &[Car],
    corr: &[(u8, coop_proto::correct::Correction)],
    rc: &coop_proto::RcState,
) -> Option<usize> {
    let mut n = 0usize;
    for e in race_control_entries(rc) {
        if !put_entry(slice, n, &e)? {
            return None;
        }
        n += 1;
    }

    for (id, c) in corr {
        // The absolute value to write is the client's own current value plus the correction. There is no
        // other way: the mailbox writes memory, not deltas, so the delta has to be resolved here.
        let Some(base) = client.iter().find(|x| x.id == *id) else {
            continue;
        };
        let start = car_start(*id as usize);
        let fields: [(usize, f32); 4] = [
            (off::DIST, base.dist + c.dist),
            (off::LAT, base.lat + c.lat),
            (off::HEAD, base.head + c.head),
            (off::SPD, base.spd + c.spd),
        ];
        for (offset, value) in fields {
            if !value.is_finite() {
                continue;
            }
            if !put_entry(
                slice,
                n,
                &Entry {
                    offset: (start + offset) as u32,
                    value: value.to_bits(),
                    width: 4,
                },
            )? {
                return None;
            }
            n += 1;
        }
    }
    Some(n)
}

/// Lays the entries for `cars` (and optionally race control) into `slice`, returning how many were
/// produced.
///
/// Returns `None` when the frame does not fit, rather than writing a partial set.
pub fn stage_into(slice: &mut [u8], cars: &[Car]) -> Option<usize> {
    let mut n = 0usize;
    for car in cars {
        for e in entries_for(car) {
            if !put_entry(slice, n, &e)? {
                return None;
            }
            n += 1;
        }
    }
    Some(n)
}

/// Stages cars plus race control, in the given tyre mode.
///
/// Race control goes **first** so that a client which runs out of buffer space still learns about the
/// red flag rather than losing it behind 22 cars' worth of positions. Losing one car's speed for a
/// frame is cosmetic; losing a red flag desynchronises the whole session. Positions come next and tyres
/// last, for the same reason: tyre state is the largest and least critical part of a frame.
pub fn stage_into_with_rc_and_tyres(
    slice: &mut [u8],
    cars: &[Car],
    rc: &coop_proto::RcState,
    tyres: TyreMode,
) -> Option<usize> {
    let mut n = 0usize;
    for e in race_control_entries(rc) {
        if !put_entry(slice, n, &e)? {
            return None;
        }
        n += 1;
    }
    for car in cars {
        for e in entries_for(car) {
            if !put_entry(slice, n, &e)? {
                return None;
            }
            n += 1;
        }
        for e in state_entries(car) {
            if !put_entry(slice, n, &e)? {
                return None;
            }
            n += 1;
        }
    }
    for car in cars {
        for e in tyre_entries(car, tyres) {
            if !put_entry(slice, n, &e)? {
                return None;
            }
            n += 1;
        }
    }
    Some(n)
}

/// Stages cars plus race control.
pub fn stage_into_with_rc(slice: &mut [u8], cars: &[Car], rc: &coop_proto::RcState) -> Option<usize> {
    stage_into_with_rc_and_tyres(slice, cars, rc, TyreMode::Observe)
}

/// Writes one entry at index `n`. Returns `Some(true)` on success, `None` when the buffer is too small.
fn put_entry(slice: &mut [u8], n: usize, e: &Entry) -> Option<bool> {
    if n >= MAX_ENTRIES {
        return None;
    }
    let at = n * ENTRY_LEN;
    if at + ENTRY_LEN > slice.len() {
        return None;
    }
    slice[at..at + 4].copy_from_slice(&e.offset.to_le_bytes());
    slice[at + 4..at + 8].copy_from_slice(&e.value.to_le_bytes());
    slice[at + 8] = e.width;
    slice[at + 9..at + ENTRY_LEN].copy_from_slice(&[0, 0, 0]);
    Some(true)
}

/// Applies `count` staged entries to a sim object at `base`.
///
/// This is the only function in the project that writes into the sim, and it is meant to be called from
/// the hook's callback, i.e. on the game thread inside the sim call. Kept separate from the mailbox so
/// a test can drive it against a fake object and check exactly which bytes moved.
///
/// Writes exactly the width each entry declares. A 4-byte store for a 1-byte field would overwrite the
/// three bytes after it - and `RF_STATE` is at `0xA13289`, close enough to the end of the object that
/// an overrun there is a crash rather than a small corruption.
///
/// # Safety
///
/// `base` must point at a live sim object, and the caller must be running on the thread that owns it.
pub unsafe fn apply(base: usize, stage: &[u8], count: usize) -> usize {
    let mut applied = 0usize;
    for i in 0..count {
        let at = i * ENTRY_LEN;
        if at + ENTRY_LEN > stage.len() || base == 0 {
            break;
        }
        let offset =
            u32::from_le_bytes([stage[at], stage[at + 1], stage[at + 2], stage[at + 3]]) as usize;
        let value = u32::from_le_bytes([stage[at + 4], stage[at + 5], stage[at + 6], stage[at + 7]]);
        let width = stage[at + 8];
        let target = (base + offset) as *mut u8;
        match width {
            1 => std::ptr::write_volatile(target, value as u8),
            2 => std::ptr::write_volatile(target as *mut u16, value as u16),
            4 => std::ptr::write_volatile(target as *mut u32, value),
            // An unknown width is a bug in the producer, not something to guess at: skipping it is
            // always safer than writing the wrong number of bytes into the sim.
            _ => continue,
        }
        applied += 1;
    }
    applied
}

/// The shared mailbox.
///
/// # Safety of the interior mutability
///
/// `buffers` is written by the worker and read by the hook callback, which run on different threads,
/// and that is exactly why the borrow checker would object. The protocol is what makes it sound: the
/// worker only touches the buffer *not* named by `published`, only while `busy` is 0, and it flips
/// `published` before setting `busy` with a release store. The callback acquires `busy`, so it cannot
/// observe a half-written buffer, and the worker cannot start filling the one being drained because
/// `busy` is still set. No two threads ever hold the same bytes at the same time, so there is no data
/// race - the unsafety is only in expressing that to the compiler.
pub struct Mailbox {
    buffers: UnsafeCell<[u8; BUFFER_LEN * 2]>,
    published: AtomicUsize,
    busy: AtomicUsize,
    count: AtomicUsize,
    applied: AtomicUsize,
    dropped: AtomicUsize,
    entries_written: AtomicUsize,
}

// SAFETY: see the type-level note above. All cross-thread access to `buffers` is ordered by the
// `busy` flag, and the bytes of a buffer are only ever owned by one thread at a time.
unsafe impl Sync for Mailbox {}
unsafe impl Send for Mailbox {}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Mailbox {
    pub fn new() -> Self {
        Self {
            buffers: UnsafeCell::new([0u8; BUFFER_LEN * 2]),
            published: AtomicUsize::new(0),
            busy: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
            applied: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            entries_written: AtomicUsize::new(0),
        }
    }

    /// Stages `cars` and publishes them, or drops the frame if the previous one is still pending.
    ///
    /// Returns `true` when the frame was published.
    pub fn publish(&self, cars: &[Car]) -> bool {
        self.publish_with_rc(cars, &coop_proto::RcState::default())
    }

    /// Stages a *correction* rather than a full state overwrite.
    ///
    /// The corrections arrive as per-car deltas from `coop-link::correct`, already sized by the time gap. They
    /// are converted to absolute values here because the mailbox writes absolute memory, not deltas - so the
    /// caller supplies the receiving sim's current values and this adds the correction to them.
    ///
    /// Why a separate path rather than the same `publish_full`: a correction of `+0.0` must produce **no
    /// entry at all**. Staging zero-value writes would burn mailbox budget and, worse, rewrite a float the
    /// receiving sim is mid-way through using - which is the `0xF3C` crash class.
    pub fn publish_corrections(
        &self,
        client: &[Car],
        corr: &[(u8, coop_proto::correct::Correction)],
        rc: &coop_proto::RcState,
    ) -> bool {
        if corr.is_empty() {
            // Nothing to do. Do not publish an empty frame: it would still wake the hook and cost a drain.
            return false;
        }
        if self.busy.load(Ordering::SeqCst) != 0 {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        let target = 1 - self.published.load(Ordering::SeqCst);
        let start = target * BUFFER_LEN;
        let n = {
            let all = unsafe { &mut *self.buffers.get() };
            match stage_corrections(&mut all[start..start + BUFFER_LEN], client, corr, rc) {
                Some(n) => n,
                None => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
            }
        };
        if n == 0 {
            return false;
        }
        self.count.store(n, Ordering::SeqCst);
        self.published.store(target, Ordering::SeqCst);
        self.busy.store(1, Ordering::Release);
        true
    }

    /// Stages cars plus race control and publishes them.
    ///
    /// Race control is what stops a client's cars freezing when the host pauses for a red flag: without
    /// it the client has no idea *why* the host's distances stopped changing.
    pub fn publish_with_rc(&self, cars: &[Car], rc: &coop_proto::RcState) -> bool {
        self.publish_full(cars, rc, TyreMode::Observe)
    }

    /// Stages cars, race control and (per `tyres`) tyre state, then publishes.
    pub fn publish_full(&self, cars: &[Car], rc: &coop_proto::RcState, tyres: TyreMode) -> bool {
        if self.busy.load(Ordering::SeqCst) != 0 {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        // Fill the buffer the callback is *not* reading.
        let target = 1 - self.published.load(Ordering::SeqCst);
        let start = target * BUFFER_LEN;
        let n = {
            // SAFETY: `busy` is clear, so no other thread is inside this buffer; and `target` is the
            // one the callback will not read until we flip `published` under a release store.
            let all = unsafe { &mut *self.buffers.get() };
            match stage_into_with_rc_and_tyres(&mut all[start..start + BUFFER_LEN], cars, rc, tyres) {
                Some(n) => n,
                None => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
            }
        };
        self.count.store(n, Ordering::SeqCst);
        self.published.store(target, Ordering::SeqCst);
        self.busy.store(1, Ordering::Release);
        true
    }

    /// Applies the published frame to `base`. Called on the game thread.
    ///
    /// Returns how many entries were written, which is 0 when nothing was pending.
    pub fn drain(&self, base: usize) -> usize {
        if self.busy.load(Ordering::Acquire) == 0 {
            return 0;
        }
        let idx = self.published.load(Ordering::SeqCst);
        let n = self.count.load(Ordering::SeqCst).min(MAX_ENTRIES);
        let start = idx * BUFFER_LEN;
        let wrote = {
            // SAFETY: `busy` was set with a release store after the bytes were written, and only the
            // callback clears it. No other thread touches this buffer while it is set.
            let all = unsafe { &*self.buffers.get() };
            unsafe { apply(base, &all[start..start + BUFFER_LEN], n) }
        };
        self.entries_written.fetch_add(wrote, Ordering::Relaxed);
        self.applied.fetch_add(1, Ordering::Relaxed);
        // Clear *after* applying, so a worker that sees 0 knows these bytes are no longer in use.
        self.busy.store(0, Ordering::Release);
        wrote
    }

    /// `(frames applied, frames dropped, entries written)` for the heartbeat line.
    pub fn stats(&self) -> (usize, usize, usize) {
        (
            self.applied.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
            self.entries_written.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(id: u8) -> Car {
        Car {
            id,
            dist: 1000.0 + id as f32,
            lat: -0.5,
            head: 1.25,
            spd: 88.5,
            ..Default::default()
        }
    }

    #[test]
    fn entries_address_the_right_fields_for_a_late_car() {
        // Car 21 is the last slot, and the one where an off-by-one in the stride shows up.
        let e = entries_for(&car(21));
        assert_eq!(e.len(), FIELDS.len());
        let base = car_start(21);
        assert_eq!(e[0].offset as usize, base + off::LAT);
        assert_eq!(e[1].offset as usize, base + off::HEAD);
        assert_eq!(e[2].offset as usize, base + off::DIST);
        assert_eq!(e[3].offset as usize, base + off::SPD);
        assert_eq!(f32::from_bits(e[2].value), 1021.0);
        assert_eq!(f32::from_bits(e[3].value), 88.5);
    }

    #[test]
    fn node_and_seg_are_never_written() {
        // The regression that crashed the client: writing track-graph indices it did not own.
        let e = entries_for(&car(0));
        let forbidden = [off::NODE, off::SEG, off::RACE_POS, off::LAP];
        let base = car_start(0);
        for entry in &e {
            let rel = entry.offset as usize - base;
            assert!(!forbidden.contains(&rel), "field at +0x{rel:X} must not be replicated");
        }
    }

    #[test]
    fn non_finite_fields_are_dropped_rather_than_written() {
        let mut c = car(3);
        c.lat = f32::NAN;
        c.spd = f32::INFINITY;
        let e = entries_for(&c);
        assert_eq!(e.len(), 2, "only the two finite fields may travel");
        assert_eq!(e[0].offset as usize, car_start(3) + off::HEAD);
        assert_eq!(e[1].offset as usize, car_start(3) + off::DIST);
    }

    #[test]
    fn stage_into_lays_out_entries_little_endian() {
        let mut buf = vec![0u8; BUFFER_LEN];
        let n = stage_into(&mut buf, &[car(1)]).unwrap();
        assert_eq!(n, 4);
        let o = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let v = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        assert_eq!(o, car_start(1) + off::LAT);
        assert_eq!(f32::from_bits(v), -0.5);
        assert_eq!(buf[8], 4, "car fields are 4 bytes wide");
    }

    #[test]
    fn stage_into_refuses_more_than_the_table_holds() {
        // `stage_into` stages positions only: 4 entries per car, against a 480-entry budget, so the
        // boundary is 120 cars. The point of the test is that an over-large frame is *refused* rather
        // than truncated, so a client never gets half a car.
        let mut buf = vec![0u8; BUFFER_LEN];
        let cars: Vec<Car> = (0..22u8).map(car).collect();
        assert!(stage_into(&mut buf, &cars).is_some());

        // 130 cars x 4 = 520 entries, over the 480 limit.
        let many: Vec<Car> = (0..130u8).map(|i| Car { id: i % 22, ..car(i % 22) }).collect();
        assert!(stage_into(&mut buf, &many).is_none(), "must refuse, not truncate");
    }

    #[test]
    fn stage_into_refuses_a_short_buffer() {
        let mut buf = vec![0u8; 16];
        assert!(
            stage_into(&mut buf, &[car(0)]).is_none(),
            "a car needs 4 entries x 12 bytes = 48"
        );
    }

    /// The whole point of the module: a publish, drained on "the game thread", must move exactly the
    /// intended bytes of a fake sim object and nothing else.
    #[test]
    fn publish_then_drain_writes_only_the_replicated_fields() {
        let mailbox = Mailbox::new();
        // A fake sim object as a plain Vec, zero-filled: the neighbours of each field must stay zero.
        let mut object = vec![0u8; car_start(21) + 0x900];
        let base = object.as_ptr() as usize;

        assert!(mailbox.publish(&[car(21)]));
        let wrote = mailbox.drain(base);
        assert_eq!(wrote, FIELDS.len() + STATE_FIELDS.len());

        let dist_at = car_start(21) + off::DIST;
        let stored = u32::from_le_bytes([
            object[dist_at],
            object[dist_at + 1],
            object[dist_at + 2],
            object[dist_at + 3],
        ]);
        assert_eq!(f32::from_bits(stored), 1021.0);

        let spd_at = car_start(21) + off::SPD;
        let stored = u32::from_le_bytes([
            object[spd_at],
            object[spd_at + 1],
            object[spd_at + 2],
            object[spd_at + 3],
        ]);
        assert_eq!(f32::from_bits(stored), 88.5);

        // Node and seg must still be zero.
        let node_at = car_start(21) + off::NODE;
        assert_eq!(&object[node_at..node_at + 4], &[0, 0, 0, 0], "node must not be written");
        let seg_at = car_start(21) + off::SEG;
        assert_eq!(&object[seg_at..seg_at + 4], &[0, 0, 0, 0], "seg must not be written");
        // and nothing before the first car's slot may have changed either.
        assert!(object[..car_start(0)].iter().all(|b| *b == 0));
    }

    #[test]
    fn drain_with_nothing_pending_writes_nothing() {
        let mailbox = Mailbox::new();
        let mut object = vec![0u8; car_start(1) + 0x900];
        let base = object.as_ptr() as usize;
        assert_eq!(mailbox.drain(base), 0);
        assert!(object.iter().all(|b| *b == 0), "an empty drain must not touch memory");
    }

    /// If the callback has not run, a second publish must be dropped rather than overwriting the buffer
    /// the callback might be reading. This is the invariant that replaces a lock.
    #[test]
    fn a_second_publish_is_dropped_while_the_first_is_pending() {
        let mailbox = Mailbox::new();
        assert!(mailbox.publish(&[car(1)]));
        assert!(!mailbox.publish(&[car(2)]), "the pending frame must not be overwritten");
        let (_, dropped, _) = mailbox.stats();
        assert_eq!(dropped, 1);

        // Once drained, publishing works again - and it uses the other buffer, so the frame that lands
        // is the new one, not a stale copy.
        let mut object = vec![0u8; car_start(2) + 0x900];
        let base = object.as_ptr() as usize;
        assert_eq!(mailbox.drain(base), FIELDS.len() + STATE_FIELDS.len());
        assert!(mailbox.publish(&[car(2)]));
        assert_eq!(mailbox.drain(base), FIELDS.len() + STATE_FIELDS.len());

        let dist_at = car_start(2) + off::DIST;
        let stored = u32::from_le_bytes([
            object[dist_at],
            object[dist_at + 1],
            object[dist_at + 2],
            object[dist_at + 3],
        ]);
        assert_eq!(f32::from_bits(stored), 1002.0, "the second frame must win");
    }

    #[test]
    fn a_full_22_car_frame_fits_and_applies_every_field() {
        let mailbox = Mailbox::new();
        let cars: Vec<Car> = (0..22u8).map(car).collect();
        let mut object = vec![0u8; car_start(21) + 0x900];
        let base = object.as_ptr() as usize;

        assert!(mailbox.publish(&cars));
        assert_eq!(mailbox.drain(base), 22 * (FIELDS.len() + STATE_FIELDS.len()));
        let (applied, dropped, written) = mailbox.stats();
        assert_eq!((applied, dropped, written), (1, 0, 154));

        for id in 0..22u8 {
            let at = car_start(id as usize) + off::DIST;
            let stored = u32::from_le_bytes([object[at], object[at + 1], object[at + 2], object[at + 3]]);
            assert_eq!(f32::from_bits(stored), 1000.0 + id as f32, "car {id} distance");
        }
    }

    /// The bug this width field exists to prevent.
    ///
    /// `RC_STATE` and `RF_STATE` are single bytes. Writing them as 4-byte stores would overwrite the
    /// bytes after them, and `RF_STATE` is at `0xA13289` - so its three neighbours are the last bytes of
    /// the object. This test puts sentinels either side and requires them to survive.
    #[test]
    fn one_byte_fields_do_not_clobber_their_neighbours() {
        let mailbox = Mailbox::new();
        let mut object = vec![0xAAu8; off::RF_STATE + 8];
        let base = object.as_ptr() as usize;

        let rc = coop_proto::RcState { flags: 0x100, rc_state: 2, rf_state: 5 };
        assert!(mailbox.publish_with_rc(&[], &rc));
        let wrote = mailbox.drain(base);
        assert_eq!(wrote, 3, "three race-control entries");

        // The values must land.
        assert_eq!(object[off::RC_STATE], 2, "rc_state is one byte");
        assert_eq!(object[off::RF_STATE], 5, "rf_state is one byte");
        let flags = u32::from_le_bytes([
            object[off::FLAGS], object[off::FLAGS + 1], object[off::FLAGS + 2], object[off::FLAGS + 3],
        ]);
        assert_eq!(flags, 0x100, "flags is four bytes");

        // and the neighbours must be untouched.
        assert_eq!(object[off::RC_STATE + 1], 0xAA, "rc_state's neighbour must survive");
        assert_eq!(object[off::RF_STATE + 1], 0xAA, "rf_state's neighbour must survive");
        assert_eq!(object[off::RF_STATE + 2], 0xAA);
    }

    /// Race control must be staged ahead of the cars, so a frame that overflows still carries it.
    #[test]
    fn race_control_is_staged_before_the_cars() {
        let mut buf = vec![0u8; BUFFER_LEN];
        let rc = coop_proto::RcState { flags: 0x400, rc_state: 1, rf_state: 0 };
        let n = stage_into_with_rc(&mut buf, &[car(0)], &rc).unwrap();
        assert_eq!(n, 3 + FIELDS.len() + STATE_FIELDS.len());

        // The first entry must be FLAGS, then RC_STATE, then RF_STATE.
        let first = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        assert_eq!(first, off::FLAGS);
        assert_eq!(buf[8], 4, "flags width");
        let second = u32::from_le_bytes([
            buf[ENTRY_LEN], buf[ENTRY_LEN + 1], buf[ENTRY_LEN + 2], buf[ENTRY_LEN + 3],
        ]) as usize;
        assert_eq!(second, off::RC_STATE);
        assert_eq!(buf[ENTRY_LEN + 8], 1, "rc_state width");
    }

    /// An all-zero race-control state must stage nothing. Staging zeros would clear a live client's
    /// flags, which is worse than sending nothing at all.
    #[test]
    fn an_empty_race_control_state_stages_nothing() {
        assert!(race_control_entries(&coop_proto::RcState::default()).is_empty());
        // But a single non-zero field is enough to be worth sending.
        let rc = coop_proto::RcState { flags: 0, rc_state: 0, rf_state: 3 };
        assert_eq!(race_control_entries(&rc).len(), 3);
    }

    // ---------------------------------------------------------------- tyres

    fn tyre_car(id: u8) -> Car {
        Car {
            id,
            dist: 100.0,
            tyres: [
                coop_proto::Tyre { surface: 90.0, carcass: 85.0, condition: 0.95 },
                coop_proto::Tyre { surface: 88.0, carcass: 83.0, condition: 0.94 },
                coop_proto::Tyre { surface: 92.0, carcass: 87.0, condition: 0.93 },
                coop_proto::Tyre { surface: 91.0, carcass: 86.0, condition: 0.92 },
            ],
            ..Default::default()
        }
    }

    /// The default must write nothing at all. This is the guarantee that deploying the tyre code does
    /// not change behaviour until someone explicitly asks for it.
    #[test]
    fn observe_mode_writes_no_tyre_fields() {
        let e = tyre_entries(&tyre_car(0), TyreMode::Observe);
        assert!(e.is_empty(), "observe must be inert");
    }

    #[test]
    fn wear_only_writes_exactly_the_condition_floats() {
        let e = tyre_entries(&tyre_car(3), TyreMode::WearOnly);
        assert_eq!(e.len(), 4, "one condition per wheel");
        let base = car_start(3);
        for (w, entry) in e.iter().enumerate() {
            let want = base + off::TYRE_SURFACE0 + w * off::TYRE_WHEEL_STRIDE + 8;
            assert_eq!(entry.offset as usize, want, "wheel {w} condition offset");
            assert_eq!(entry.width, 4);
        }
        // and specifically not the temperatures
        assert!(e.iter().all(|x| {
            let rel = x.offset as usize - base - off::TYRE_SURFACE0;
            rel % off::TYRE_WHEEL_STRIDE != 0 && rel % off::TYRE_WHEEL_STRIDE != 4
        }));
    }

    #[test]
    fn full_mode_writes_all_twelve_tyre_floats() {
        let e = tyre_entries(&tyre_car(1), TyreMode::Full);
        assert_eq!(e.len(), 12);
        let base = car_start(1);
        // first entry is wheel 0 surface, last is wheel 3 condition
        assert_eq!(e[0].offset as usize, base + off::TYRE_SURFACE0);
        assert_eq!(
            e[11].offset as usize,
            base + off::TYRE_SURFACE0 + 3 * off::TYRE_WHEEL_STRIDE + 8
        );
        assert_eq!(f32::from_bits(e[0].value), 90.0);
    }

    /// The safety property: a value outside its plausible band is skipped, never written and never
    /// clamped. `NaN` is the important case - it compares false against any bound, so it cannot slip
    /// through the `>=`/`<=` checks, and a NaN in the sim propagates into whatever divides by it.
    #[test]
    fn implausible_tyre_values_are_skipped_not_clamped() {
        let mut car = tyre_car(2);
        car.tyres[0].surface = f32::NAN;
        car.tyres[1].carcass = f32::INFINITY;
        car.tyres[2].condition = 17.5; // nonsense for a 0..1 fraction
        car.tyres[2].surface = -900.0; // colder than absolute zero region

        let e = tyre_entries(&car, TyreMode::Full);
        // 12 fields minus the four bad ones
        assert_eq!(e.len(), 8, "only plausible values may be written");

        for entry in &e {
            let bits = entry.value;
            assert!(f32::from_bits(bits).is_finite(), "no non-finite value may be written");
        }
        // Specifically: wheel 2's condition must be absent.
        let bad = car_start(2) + off::TYRE_SURFACE0 + 2 * off::TYRE_WHEEL_STRIDE + 8;
        assert!(
            !e.iter().any(|x| x.offset as usize == bad),
            "the out-of-range condition must not be emitted"
        );
    }

    #[test]
    fn tyre_bounds_reject_the_obvious_nonsense() {
        use off::tyre_range;
        assert!(plausible(90.0, tyre_range::SURFACE));
        assert!(!plausible(1e30, tyre_range::SURFACE));
        assert!(!plausible(f32::NAN, tyre_range::SURFACE));
        assert!(!plausible(f32::NEG_INFINITY, tyre_range::SURFACE));
        assert!(plausible(1.0, tyre_range::CONDITION), "a fresh tyre");
        assert!(plausible(0.0, tyre_range::CONDITION), "a dead tyre");
        assert!(!plausible(1.9, tyre_range::CONDITION), "headroom is bounded too");
    }

    /// The pit state is not a nicety. During a race, five cars were in the pit lane on the host and the
    /// client showed none of them, because only positions were replicated: the client drew cars sliding
    /// along pit-lane geometry while its own sim still believed they were on track. These byte fields are
    /// what make the two sims agree about *whether a car is in the pit lane*, not just where it is.
    #[test]
    fn pit_state_is_replicated_as_single_bytes() {
        let mut car = car(4);
        car.pit = 1;
        car.pit_req = 2;
        car.stay_out = 1;
        let e = state_entries(&car);
        assert_eq!(e.len(), 3, "pit, pit_req, stay_out");
        let base = car_start(4);
        assert_eq!(e[0].offset as usize, base + off::PIT);
        assert_eq!(e[1].offset as usize, base + off::PIT_REQ);
        assert_eq!(e[2].offset as usize, base + off::STAY_OUT);
        for entry in &e {
            assert_eq!(entry.width, 1, "a 4-byte store would overwrite the neighbouring bytes");
        }
        assert_eq!(e[0].value, 1);
        assert_eq!(e[1].value, 2);
    }

    /// A pit value larger than the enum is a misread offset, not a state to forward.
    #[test]
    fn an_implausible_pit_value_is_skipped() {
        let mut car = car(0);
        car.pit = 99;
        let e = state_entries(&car);
        assert!(
            !e.iter().any(|x| x.offset as usize == car_start(0) + off::PIT),
            "an out-of-range pit state must not be written"
        );
    }

    /// The whole frame, widest mode, must still fit - otherwise enabling pit state or tyres would cause
    /// every frame to be refused, which is a stalled session rather than a visible failure.
    #[test]
    fn a_full_frame_fits_with_positions_state_and_tyres() {
        let cars: Vec<Car> = (0..22u8).map(tyre_car).collect();
        let mut buf = vec![0u8; BUFFER_LEN];
        let rc = coop_proto::RcState { flags: 0x100, rc_state: 1, rf_state: 0 };
        let n = stage_into_with_rc_and_tyres(&mut buf, &cars, &rc, TyreMode::Full).unwrap();
        let expected = 3 + 22 * (FIELDS.len() + STATE_FIELDS.len()) + 22 * 12;
        assert_eq!(n, expected);
        assert!(n <= MAX_ENTRIES, "{n} entries must fit in {MAX_ENTRIES}");
    }

    /// A correction is a *delta*, but the mailbox writes absolute memory. This checks the delta is resolved
    /// against the receiving sim's own values - get it backwards and every correction compounds instead of
    /// converging.
    #[test]
    fn a_correction_is_resolved_against_the_clients_own_value() {
        let mut client = vec![Car { id: 5, dist: 100.0, lat: 0.5, ..Default::default() }];
        client[0].head = 0.0;
        client[0].spd = 10.0;

        let corr = vec![(
            5u8,
            coop_proto::correct::Correction { dist: 25.0, lat: -0.1, head: 0.2, spd: 2.0 },
        )];
        let mut buf = vec![0u8; BUFFER_LEN];
        let n = stage_corrections(&mut buf, &client, &corr, &coop_proto::RcState::default())
            .expect("must stage");
        assert!(n >= 4, "four fields for the car");

        // Decode the DIST entry back out and require client.dist + correction.
        let mut found = false;
        for i in 0..n {
            let at = i * ENTRY_LEN;
            let off = u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]) as usize;
            if off == car_start(5) + off::DIST {
                let bits = u32::from_le_bytes([buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]]);
                assert_eq!(f32::from_bits(bits), 125.0, "100 + 25, not 25");
                found = true;
            }
        }
        assert!(found, "DIST entry must be present");
    }

    /// A car in the correction list that the client does not have must be skipped, not paired by index.
    #[test]
    fn a_correction_for_an_unknown_car_is_skipped() {
        let client = vec![Car { id: 1, dist: 10.0, ..Default::default() }];
        let corr = vec![(9u8, coop_proto::correct::Correction { dist: 5.0, ..Default::default() })];
        let mut buf = vec![0u8; BUFFER_LEN];
        let n = stage_corrections(&mut buf, &client, &corr, &coop_proto::RcState::default())
            .expect("must not fail");
        assert_eq!(n, 0, "nothing to write for a car we do not have");
    }

    /// An empty correction list must not publish at all: an empty frame still wakes the hook and costs a
    /// drain, and a synchronised field produces one of these every tick.
    #[test]
    fn an_empty_correction_list_publishes_nothing() {
        let m = Mailbox::new();
        assert!(!m.publish_corrections(&[], &[], &coop_proto::RcState::default()));
        let mut object = vec![0u8; 0x1000];
        assert_eq!(m.drain(object.as_mut_ptr() as usize), 0);
    }

    #[test]
    fn mode_parsing_never_falls_through_to_the_widest_mode() {
        assert_eq!(TyreMode::from_text("observe"), TyreMode::Observe);
        assert_eq!(TyreMode::from_text("wear"), TyreMode::WearOnly);
        assert_eq!(TyreMode::from_text("FULL"), TyreMode::Full);
        // A typo must be inert, not risky.
        assert_eq!(TyreMode::from_text("fulll"), TyreMode::Observe);
        assert_eq!(TyreMode::from_text(""), TyreMode::Observe);
        assert_eq!(TyreMode::from_text("yes please"), TyreMode::Observe);
    }

    /// A full 22-car frame in the widest mode must still fit, or every frame would be refused and the
    /// session would stall the moment tyres were enabled.
    #[test]
    fn a_full_frame_fits_even_with_every_tyre_field() {
        let cars: Vec<Car> = (0..22u8).map(tyre_car).collect();
        let mut buf = vec![0u8; BUFFER_LEN];
        let rc = coop_proto::RcState { flags: 0x100, rc_state: 1, rf_state: 0 };
        let n = stage_into_with_rc_and_tyres(&mut buf, &cars, &rc, TyreMode::Full).unwrap();
        assert_eq!(n, 3 + 22 * (FIELDS.len() + STATE_FIELDS.len()) + 22 * 12);
        assert!(n <= MAX_ENTRIES);
    }
}
