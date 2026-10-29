//! Memory layout of the race simulation, owned by Rust now.
//!
//! These offsets came out of the Lua prototype (see `archive/demo-v78/README.md` in the F1 Manager
//! work directory) and are the one place they should be written down from here on. The C++ side will
//! write with the same numbers, sourced from this module, so the two halves cannot drift apart.
//!
//! Everything is object-relative to the `RaceSimManager` pointer.

/// First car record, relative to the RaceSimManager.
pub const CAR_BASE: usize = 0x95BC00;
/// Distance between car records.
pub const CAR_STRIDE: usize = 0x10D8;
/// Cars in a race.
pub const CAR_MAX: usize = 22;
/// Real object size on build 17356935.
/// GUObjectArray, as an RVA from the main module base. Verified live: 0x140000000 + 0x7821450.
pub const GUA_RVA: usize = 0x7821450;

/// `simulate`, as an RVA from the main module base.
pub const SIMULATE_RVA: usize = 0x2298080;

pub const OBJECT_SIZE: usize = 0xA132F0;

/// Per-car field offsets.
pub mod off {
    pub const LAT: usize = 0x170;
    pub const HEAD: usize = 0x184;
    pub const SPLT: usize = 0x18C;
    pub const SEG: usize = 0x190;
    pub const DIST: usize = 0x194;
    pub const SPD: usize = 0x198;
    pub const ACC: usize = 0x19C;
    pub const STUCK: usize = 0x1AC;
    pub const POS0: usize = 0x1E0;
    pub const POS1: usize = 0x1E8;
    pub const POS2: usize = 0x1F0;
    pub const NODE: usize = 0x1FC;
    pub const RACE_POS: usize = 0x7E0;
    pub const LAP: usize = 0x7E4;
    pub const LAP_TIME: usize = 0x7E8;
    pub const DOWN: usize = 0x7F1;
    pub const DONE: usize = 0x7F3;
    pub const DRS: usize = 0x870;
    pub const PIT: usize = 0x8A8;
    pub const PIT_REQ: usize = 0x8A9;
    pub const STAY_OUT: usize = 0x904;
    pub const LAP_TYPE: usize = 0xEFA;
    pub const FUEL: usize = 0x778;
    pub const TYRE_LO: usize = 0x924;
    pub const TYRE_HI: usize = 0x928;
    /// Wheel 0 surface. Carcass and condition follow at +4 and +8; each wheel advances by 0xC.
    pub const TYRE_SURFACE0: usize = 0x97C;
    pub const TYRE_WHEEL_STRIDE: usize = 0xC;
    pub const TYRE_WHEELS: usize = 4;

    /// Plausibility bounds for the three continuous tyre floats, per wheel.
    ///
    /// These exist because a bad float written into the sim is not a cosmetic error: temperature feeds
    /// the puncture curve and the grip model, and a value like `NaN` or `1e30` propagates into
    /// comparisons and divisions the game then trusts. Every crash this project has had came from
    /// writing a value whose meaning we did not know; a range check converts that class of mistake
    /// from a crash into a refused write.
    ///
    /// The bounds are deliberately one-sided generous. `condition` is the one position we are confident
    /// about because the decoder already reads it and it behaves like a fraction; the two temperatures
    /// are accepted over a wide band so an unusual-but-legal value (a cold tyre in the wet, an overheated
    /// one behind a safety car) is not refused. Anything outside these bands is not a tyre reading at
    /// all - it is a sentinel, an uninitialised field, or a misread offset.
    pub mod tyre_range {
        /// Surface temperature, degrees Celsius. Generous: a parked car can be ambient, a car on the
        /// limit can exceed a hundred.
        pub const SURFACE: (f32, f32) = (-40.0, 400.0);
        /// Carcass temperature, same reasoning.
        pub const CARCASS: (f32, f32) = (-40.0, 400.0);
        /// Remaining condition as a fraction. The sim uses 1.0 for a fresh tyre, so 0..1 is the domain;
        /// a little headroom either side absorbs a representation choice we may have wrong.
        pub const CONDITION: (f32, f32) = (-0.01, 1.5);
    }

    pub const COMMAND_BASE: usize = 0x770;
    pub const COMMAND_STRIDE: usize = 0x50;

    /// Byte offsets inside a car's command record that carry the player's instructions.
    ///
    /// These are the action bytes only, taken from the proven Lua prototype (CoopDiagMod v57, which had
    /// the client's strategy reaching the host). The record is consumed on the game thread by the
    /// command processor, so these must be applied ON CHANGE rather than re-written every tick: a
    /// request byte that stays high is a request that is issued repeatedly.
    pub mod cmd {
        /// Stay-out request: pending flag and value.
        pub const STAY_OUT_PENDING: usize = 0x14;
        pub const STAY_OUT_VALUE: usize = 0x15;
        /// Release from the garage: pending flag and value.
        pub const GARAGE_PENDING: usize = 0x46;
        pub const GARAGE_VALUE: usize = 0x47;
        /// Pit command pending flag, and request (1) / cancel (0).
        pub const PIT_PENDING: usize = 0x48;
        pub const PIT_REQUEST: usize = 0x49;
        /// Replace wing flag.
        pub const REPLACE_WING: usize = 0x4A;
        /// New tyre set selector.
        pub const TYRE_SET: usize = 0x4B;
        pub const TYRE_SELECTOR: usize = 0x4E;

        /// Every action byte, in the order the prototype published them.
        pub const ALL: [usize; 9] = [
            STAY_OUT_PENDING,
            STAY_OUT_VALUE,
            GARAGE_PENDING,
            GARAGE_VALUE,
            PIT_PENDING,
            PIT_REQUEST,
            REPLACE_WING,
            TYRE_SET,
            TYRE_SELECTOR,
        ];
    }

    /// Absolute offset of a car's command record.
    #[inline]
    pub fn command_start(car: usize) -> usize {
        COMMAND_BASE + car * COMMAND_STRIDE
    }

    pub const SIM_TIME: usize = 0x99AF1C;
    pub const SIM_DONE: usize = 0x99AF20;
    pub const PLAYBACK: usize = 0x99AF24;
    pub const FLAGS: usize = 0x24C;
    pub const RC_STATE: usize = 0xA045C0;
    pub const RF_STATE: usize = 0xA13289;
}

use crate::{Car, Tyre};

#[inline]
fn f32_at(buf: &[u8], at: usize) -> Option<f32> {
    buf.get(at..at + 4)
        .map(|b| f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

#[inline]
fn i32_at(buf: &[u8], at: usize) -> Option<i32> {
    buf.get(at..at + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[inline]
fn u8_at(buf: &[u8], at: usize) -> Option<u8> {
    buf.get(at).copied()
}

/// Decode one car from a buffer that starts at the RaceSimManager base.
///
/// Returns `None` rather than garbage when the buffer is too short, which is how a truncated
/// ReadProcessMemory call surfaces instead of as plausible-looking zeroes.
pub fn car_at(base: &[u8], car: usize) -> Option<Car> {
    if car >= CAR_MAX {
        return None;
    }
    let o = CAR_BASE.checked_add(car.checked_mul(CAR_STRIDE)?)?;
    let mut tyres = [Tyre::default(); 4];
    for (w, t) in tyres.iter_mut().enumerate() {
        let s = o + off::TYRE_SURFACE0 + w * off::TYRE_WHEEL_STRIDE;
        t.surface = f32_at(base, s)?;
        t.carcass = f32_at(base, s + 4)?;
        t.condition = f32_at(base, s + 8)?;
    }
    Some(Car {
        id: car as u8,
        dist: f32_at(base, o + off::DIST)?,
        lat: f32_at(base, o + off::LAT)?,
        head: f32_at(base, o + off::HEAD)?,
        spd: f32_at(base, o + off::SPD)?,
        acc: f32_at(base, o + off::ACC)?,
        splt: f32_at(base, o + off::SPLT)?,
        seg: f32_at(base, o + off::SEG)?,
        node: i32_at(base, o + off::NODE)?,
        pos: u8_at(base, o + off::RACE_POS)?,
        lap: i32_at(base, o + off::LAP)?,
        fuel: f32_at(base, o + off::FUEL)?,
        lap_time: f32_at(base, o + off::LAP_TIME)?,
        down: u8_at(base, o + off::DOWN)?,
        done: u8_at(base, o + off::DONE)?,
        drs: i32_at(base, o + off::DRS)?,
        pit: u8_at(base, o + off::PIT)?,
        pit_req: u8_at(base, o + off::PIT_REQ)?,
        stay_out: u8_at(base, o + off::STAY_OUT)?,
        lap_type: u8_at(base, o + off::LAP_TYPE)?,
        tyre_lo: f32_at(base, o + off::TYRE_LO)?,
        tyre_hi: f32_at(base, o + off::TYRE_HI)?,
        tyres,
    })
}

/// Decode every car present in the buffer.
pub fn cars(base: &[u8]) -> Vec<Car> {
    (0..CAR_MAX).filter_map(|i| car_at(base, i)).collect()
}

#[inline]
fn put_f32(buf: &mut [u8], at: usize, v: f32) -> Option<()> {
    let s = buf.get_mut(at..at + 4)?;
    s.copy_from_slice(&v.to_bits().to_le_bytes());
    Some(())
}

#[inline]
fn put_i32(buf: &mut [u8], at: usize, v: i32) -> Option<()> {
    let s = buf.get_mut(at..at + 4)?;
    s.copy_from_slice(&v.to_le_bytes());
    Some(())
}

#[inline]
fn put_u8(buf: &mut [u8], at: usize, v: u8) -> Option<()> {
    *buf.get_mut(at)? = v;
    Some(())
}

/// First byte of a car's record.
#[inline]
pub fn car_start(car: usize) -> usize {
    CAR_BASE + car * CAR_STRIDE
}

/// Write the fields of `c` into `buf` at car slot `c.id`.
///
/// Returns the inclusive byte range `[start, end)` that was modified, so the caller can push only
/// that span to the target process.
/// Encode only the fields that place a car on the track.
///
/// Writing the full record crashed the client twice, and the log showed how: frame 1 wrote and
/// verified all 22 cars, then frame 2 was refused at the same address and the game faulted with
/// `EXCEPTION_ACCESS_VIOLATION reading 0xF3C`.
///
/// The cause is that `encode_car` overwrites state the game's own simulation consumes - `node`,
/// `seg`, `lap`, `pos`, fuel and tyre condition - while that simulation is still stepping. Supplying
/// a track-graph index the client's own world does not agree with makes the game dereference track
/// data through it, which is exactly a null base plus a small offset.
///
/// So position is replicated and state is not. Distance, lateral offset, heading and reference
/// position are what another car needs in order to appear in the right place; everything else the
/// receiving game already computes for itself.
pub fn encode_car_motion(buf: &mut [u8], c: &Car) -> Option<(usize, usize)> {
    let car = c.id as usize;
    if car >= CAR_MAX {
        return None;
    }
    let o = car_start(car);
    put_f32(buf, o + off::DIST, c.dist)?;
    put_f32(buf, o + off::LAT, c.lat)?;
    put_f32(buf, o + off::HEAD, c.head)?;
    put_f32(buf, o + off::SPD, c.spd)?;
    // NODE and SEG are deliberately not written: they index the track graph, and a value that does
    // not belong to the receiving world is read through by the game.
    put_u8(buf, o + off::RACE_POS, c.pos)?;
    Some((o, o + CAR_STRIDE))
}

pub fn encode_car(buf: &mut [u8], c: &Car) -> Option<(usize, usize)> {
    let car = c.id as usize;
    if car >= CAR_MAX {
        return None;
    }
    let o = car_start(car);
    put_f32(buf, o + off::DIST, c.dist)?;
    put_f32(buf, o + off::LAT, c.lat)?;
    put_f32(buf, o + off::HEAD, c.head)?;
    put_f32(buf, o + off::SPD, c.spd)?;
    put_f32(buf, o + off::ACC, c.acc)?;
    put_f32(buf, o + off::SPLT, c.splt)?;
    put_f32(buf, o + off::SEG, c.seg)?;
    put_i32(buf, o + off::NODE, c.node)?;
    put_u8(buf, o + off::RACE_POS, c.pos)?;
    put_i32(buf, o + off::LAP, c.lap)?;
    put_f32(buf, o + off::FUEL, c.fuel)?;
    put_f32(buf, o + off::LAP_TIME, c.lap_time)?;
    put_u8(buf, o + off::DOWN, c.down)?;
    put_u8(buf, o + off::DONE, c.done)?;
    put_i32(buf, o + off::DRS, c.drs)?;
    put_u8(buf, o + off::PIT, c.pit)?;
    put_u8(buf, o + off::PIT_REQ, c.pit_req)?;
    put_u8(buf, o + off::STAY_OUT, c.stay_out)?;
    put_u8(buf, o + off::LAP_TYPE, c.lap_type)?;
    put_f32(buf, o + off::TYRE_LO, c.tyre_lo)?;
    put_f32(buf, o + off::TYRE_HI, c.tyre_hi)?;
    for (w, t) in c.tyres.iter().enumerate() {
        let s = o + off::TYRE_SURFACE0 + w * off::TYRE_WHEEL_STRIDE;
        put_f32(buf, s, t.surface)?;
        put_f32(buf, s + 4, t.carcass)?;
        put_f32(buf, s + 8, t.condition)?;
    }
    Some((o, o + CAR_STRIDE))
}

/// Bytes needed to hold one car record, positioned by [`car_start`].
pub fn car_written_len() -> usize {
    car_start(CAR_MAX - 1) + CAR_STRIDE
}

/// Race control out of the same buffer.
pub fn rc(base: &[u8]) -> Option<crate::RcState> {
    Some(crate::RcState {
        flags: base
            .get(off::FLAGS..off::FLAGS + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?,
        rc_state: u8_at(base, off::RC_STATE)?,
        rf_state: u8_at(base, off::RF_STATE)?,
    })
}

/// Bytes needed to decode every car.
pub fn car_block_len() -> usize {
    CAR_BASE + (CAR_MAX - 1) * CAR_STRIDE + CAR_STRIDE
}

/// Bytes needed to read race control as well as the cars.
///
/// `RF_STATE` sits at 0xA13289, essentially the end of the object, so a reader that wants race
/// control has to read (almost) the whole thing. Reading only the car block returns a correct
/// snapshot with race control unavailable - which `rc()` reports as `None` rather than zeroes.
pub fn object_len() -> usize {
    OBJECT_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_with(dist: f32, spd: f32) -> Vec<u8> {
        let mut buf = vec![0u8; OBJECT_SIZE];
        for car in 0..CAR_MAX {
            let o = CAR_BASE + car * CAR_STRIDE;
            let d = (dist + car as f32).to_bits();
            buf[o + off::DIST..o + off::DIST + 4].copy_from_slice(&d.to_le_bytes());
            buf[o + off::SPD..o + off::SPD + 4].copy_from_slice(&spd.to_bits().to_le_bytes());
            buf[o + off::RACE_POS] = (car + 1) as u8;
            for w in 0..4 {
                let s = o + off::TYRE_SURFACE0 + w * off::TYRE_WHEEL_STRIDE;
                buf[s..s + 4].copy_from_slice(&91.5f32.to_bits().to_le_bytes());
                buf[s + 4..s + 8].copy_from_slice(&88.25f32.to_bits().to_le_bytes());
                buf[s + 8..s + 12].copy_from_slice(&0.94f32.to_bits().to_le_bytes());
            }
        }
        buf[off::FLAGS..off::FLAGS + 4].copy_from_slice(&1024u32.to_le_bytes());
        buf[off::RC_STATE] = 1;
        buf
    }

    #[test]
    fn decodes_all_22_cars() {
        let buf = block_with(1000.0, 50.0);
        let cars = cars(&buf);
        assert_eq!(cars.len(), 22);
        assert_eq!(cars[0].dist, 1000.0);
        assert_eq!(cars[21].dist, 1021.0);
        assert_eq!(cars[3].pos, 4);
        assert_eq!(cars[0].tyres[3].condition, 0.94);
    }

    #[test]
    fn short_buffer_yields_no_garbage() {
        let buf = block_with(1000.0, 50.0);
        let short = &buf[..CAR_BASE + 5 * CAR_STRIDE];
        let cars = cars(short);
        assert_eq!(cars.len(), 5, "only fully readable cars are returned");
    }

    #[test]
    fn refuses_car_past_the_grid() {
        let buf = block_with(0.0, 0.0);
        assert!(car_at(&buf, CAR_MAX).is_none());
    }

    #[test]
    fn race_control_needs_the_whole_object() {
        let full = block_with(0.0, 0.0);
        let cars_only = &full[..car_block_len()];
        assert!(rc(&full).is_some(), "full object must expose race control");
        assert!(rc(cars_only).is_none(), "car-only read must not invent race control");
    }

    #[test]
    fn reads_race_control() {
        let buf = block_with(0.0, 0.0);
        let rc = rc(&buf).expect("rc must decode");
        assert_eq!(rc.flags, 1024);
        assert_eq!(rc.rc_state, 1);
        assert_eq!(rc.rf_state, 0);
    }

    /// The write side must be the exact inverse of the read side, or a synced client silently
    /// agrees with nobody. This is the round-trip that proves it.
    #[test]
    fn encode_then_decode_round_trips_every_field() {
        let mut buf = vec![0u8; OBJECT_SIZE];
        let mut c = Car {
            id: 7,
            dist: 12345.5,
            lat: -3.25,
            head: 1.5,
            spd: 210.75,
            acc: -0.5,
            splt: 12.125,
            seg: 0.75,
            node: 428,
            pos: 4,
            lap: 9,
            fuel: 12.5,
            lap_time: 91.234,
            down: 1,
            done: 0,
            drs: 2,
            pit: 1,
            pit_req: 1,
            stay_out: 0,
            lap_type: 3,
            tyre_lo: 70.0,
            tyre_hi: 74.5,
            tyres: [Tyre::default(); 4],
        };
        for (w, t) in c.tyres.iter_mut().enumerate() {
            t.surface = 91.0 + w as f32;
            t.carcass = 88.0 + w as f32;
            t.condition = 0.90 - w as f32 * 0.01;
        }

        let (start, end) = encode_car(&mut buf, &c).expect("encode");
        assert_eq!(start, car_start(7));
        assert_eq!(end - start, CAR_STRIDE);

        let back = car_at(&buf, 7).expect("decode");
        assert_eq!(back, c, "round trip must preserve every field");
    }

    /// A write must not disturb a neighbouring car. Two car records are 0x10D8 apart, so an
    /// off-by-one in a stride would corrupt the car ahead and stay invisible in a single-car test.
    #[test]
    fn encode_leaves_neighbours_untouched() {
        let mut buf = vec![0u8; OBJECT_SIZE];
        let neighbour = Car {
            id: 8,
            dist: 999.0,
            ..Car::default()
        };
        encode_car(&mut buf, &neighbour).unwrap();
        let before = car_at(&buf, 8).unwrap();

        let target = Car {
            id: 7,
            dist: 1.0,
            ..Car::default()
        };
        encode_car(&mut buf, &target).unwrap();

        assert_eq!(car_at(&buf, 8).unwrap(), before, "car 8 must not change");
        assert_eq!(car_at(&buf, 7).unwrap().dist, 1.0);
    }

    #[test]
    fn encode_refuses_car_past_the_grid() {
        let mut buf = vec![0u8; OBJECT_SIZE];
        let c = Car {
            id: CAR_MAX as u8,
            ..Car::default()
        };
        assert!(encode_car(&mut buf, &c).is_none());
    }
}
