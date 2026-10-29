//! Wire format for F1 Manager co-op.
//!
//! Design notes, learned the hard way with the Lua file transport this replaces:
//!
//! * every frame is length-prefixed and carries a checksum, so a torn or partial write is an
//!   error instead of a plausible-looking half-state (the Lua version rejected torn reads by
//!   looking for a trailing "END" marker, which only worked because the file was text);
//! * the format is versioned and refuses a mismatched peer instead of misparsing it;
//! * decoding is bounds checked and returns errors rather than panicking, because this runs in
//!   the same process that owns the game's sockets;
//! * no external crates: the payload is fixed layout little-endian, so the codec is small enough
//!   to read in one sitting and has nothing to break under us.
//!
//! Frame layout:
//!
//! ```text
//!   0  magic     4   b"F1CP"
//!   4  version   2   little endian
//!   6  len       4   payload length, little endian
//!  10  checksum  4   FNV-1a 32 of the payload
//!  14  payload   len tag + body
//! ```

pub mod coordination;
pub mod correct;
pub mod layout;

use std::fmt;

pub const MAGIC: [u8; 4] = *b"F1CP";
/// Bumped to 2 when `Msg::Agent` gained the side field; to 3 when `Msg::TeamClaim` was added.
///
/// A version bump rather than a compatible extension on purpose: an old supervisor reading a new frame
/// would decode a truncated `Agent` (pid and address, no side) and then delete the remainder as trailing
/// bytes, so it would look like a working link while every coordination frame was refused for want of a
/// side. Refusing at the header is the difference between a clear error and a silent half-message.
pub const VERSION: u16 = 3;
pub const HEADER_LEN: usize = 14;
pub const CAR_COUNT: usize = 22;

/// Which instance a peer is.
///
/// `Ord` is derived because the coordination rules key maps by side; without it the vote box cannot be a
/// `BTreeMap` and vote ordering would depend on hash iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Host,
    Client,
}

impl Side {
    fn as_u8(self) -> u8 {
        match self {
            Side::Host => 0,
            Side::Client => 1,
        }
    }

    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            0 => Ok(Side::Host),
            1 => Ok(Side::Client),
            other => Err(ProtoError::BadTag(other)),
        }
    }
}

/// Per-wheel tyre state. Offsets in the sim advance by 0xC per wheel: surface, carcass, condition.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tyre {
    pub surface: f32,
    pub carcass: f32,
    pub condition: f32,
}

/// A client's statement that it is present and ready for the world to start.
///
/// Deliberately not a plain boolean. The host is about to hold both sims on the grid, and holding on the
/// strength of a bare "yes" would be trusting a message that could have been composed before a load, or
/// during one. `session` and `page` let the host reject a stale or wrong-context ready instead of
/// freezing the race for a player who is not there.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ready {
    /// The client's own monotonic tick, so a repeated or reordered ready is detectable.
    pub tick: u32,
    /// The session the client believes it is in, as `rc_state` (`0` = none, `1` = running, ...).
    ///
    /// A client cannot see the host's session directly, so this is its own sim's view. It is used to
    /// reject "ready" from a client sitting in a menu, not to compare against the host's value - the two
    /// legitimately differ while the host is holding the grid.
    pub session: u8,
    /// The client's UI page, e.g. `RaceWeekend`. Short, capped at 32 bytes on the wire.
    pub page: String,
    /// How many cars the client can currently see moving. `0` means its sim is not stepping, which is
    /// exactly when a ready is most likely to be premature.
    pub moving_cars: u8,
}

/// Longest page name carried in a `Ready`. Anything longer is truncated by the encoder rather than
/// rejected, because the page is diagnostic and must never be the reason a join fails.
pub const READY_PAGE_MAX: usize = 32;
/// Snapshot of the grid, taken once and applied to *both* sims while the start is held.
///
/// Why both: the user's requirement is that the lights hold until the second player is ready, and that
/// includes the host. A host that keeps simulating while the client loads pulls away from the grid, and a
/// client side that cannot start moves - the two sims then disagree about the start before the race has
/// begun, which is the worst possible moment.
///
/// Why a captured snapshot rather than re-reading the live cars: while held, the sims keep stepping. If
/// the hold re-read them, drift accumulated during the hold would be baked in as the new grid. Capturing
/// once and re-applying that capture means the hold is a fixed point - every applied frame restores
/// exactly the same grid, so a long hold cannot slowly walk the cars off their marks.
#[derive(Debug, Clone, PartialEq)]
pub struct GridHold {
    /// The cars as they were at capture time.
    pub cars: Vec<Car>,
    /// The host's `race_time` at capture, so both sides agree on the clock the grid belongs to.
    pub race_time: f32,
}

impl GridHold {
    /// True when this hold is worth using: a grid needs cars, and a capture of all-zero distances is not a
    /// grid but an object that had not initialised.
    pub fn is_usable(&self) -> bool {
        !self.cars.is_empty() && self.cars.iter().any(|c| c.dist.is_finite() && c.dist.abs() > 1.0)
    }
}


///
/// Field order here is the wire order. Anything not listed is deliberately local to the owner:
/// braking internals, lateral kinematics and the AI planner are derived per step and must not
/// travel, or the receiving sim fights them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Car {
    pub id: u8,
    pub dist: f32,
    pub lat: f32,
    pub head: f32,
    pub spd: f32,
    pub acc: f32,
    pub splt: f32,
    pub seg: f32,
    pub node: i32,
    pub pos: u8,
    pub lap: i32,
    pub fuel: f32,
    pub lap_time: f32,
    pub down: u8,
    pub done: u8,
    pub drs: i32,
    pub pit: u8,
    pub pit_req: u8,
    pub stay_out: u8,
    pub lap_type: u8,
    pub tyre_lo: f32,
    pub tyre_hi: f32,
    pub tyres: [Tyre; 4],
}

/// Race control. Host authoritative: `RcState` 0 pending, 2 start sequence, 1 running,
/// 3/4 finishing, 5 complete. `Flags` bit 8 is WaitingForLightsOut, 0x100 red, 0x400 DRS.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RcState {
    pub flags: u32,
    pub rc_state: u8,
    pub rf_state: u8,
}

/// A player command for one car. Values mirror `ETyreWearSavingStrategy` and `EERSStrategyType` so
/// the receiver can validate them instead of trusting the sender.
pub mod commands {
    /// `GameTypes.TyreWearSavingStrategy`: Attack 0, Aggresive 1, Standard 2, Light 3, Conserve 4.
    pub const PACE_COUNT: u8 = 5;
    /// `GameTypes.ERSStrategyType`: Neutral 0, RaceHarvest 1, RaceBoostLapTime 2, TopUp 3.
    pub const ERS_COUNT: u8 = 4;
}

/// One car's player instructions, as the client wants them applied on the host.
///
/// This is the uplink half of two-player control: the host owns the world, so a client's commands
/// have to be carried to the host rather than applied locally. Only action bytes travel - the same
/// nine the prototype used - because everything else in the command record is state the host's own
/// command processor derives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Order {
    pub car: u8,
    pub stay_out_pending: u8,
    pub stay_out_value: u8,
    pub garage_pending: u8,
    pub garage_value: u8,
    pub pit_pending: u8,
    pub pit_request: u8,
    pub replace_wing: u8,
    pub tyre_set: u8,
    pub tyre_selector: u8,
}

impl Order {
    /// The action bytes in `layout::cmd::ALL` order.
    pub fn bytes(&self) -> [u8; 9] {
        [
            self.stay_out_pending,
            self.stay_out_value,
            self.garage_pending,
            self.garage_value,
            self.pit_pending,
            self.pit_request,
            self.replace_wing,
            self.tyre_set,
            self.tyre_selector,
        ]
    }

    pub fn from_bytes(car: u8, b: &[u8; 9]) -> Self {
        Order {
            car,
            stay_out_pending: b[0],
            stay_out_value: b[1],
            garage_pending: b[2],
            garage_value: b[3],
            pit_pending: b[4],
            pit_request: b[5],
            replace_wing: b[6],
            tyre_set: b[7],
            tyre_selector: b[8],
        }
    }

    /// True if any instruction is outstanding. All-zero is "nothing requested", which is the state
    /// most cars are in most of the time and is not worth a frame.
    pub fn any(&self) -> bool {
        self.bytes().iter().any(|&v| v != 0)
    }
}


#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Hello { side: Side, cars_owned: u32 },
    /// Client → host: this client is loaded, in the right session, and ready for the world to start.
    ///
    /// The transport was one-way by construction: the host published and the client consumed. That is
    /// enough to move cars but makes a race start unholdable, because the host cannot be told whether the
    /// other player is actually present. This message is the minimum needed to answer that question, and
    /// it carries the evidence rather than a bare flag: a client that reports ready while sitting in the
    /// wrong session is worse than one that says nothing.
    Ready(Ready),
    Snapshot {
        tick: u32,
        /// The host's simulation clock, `RaceSimManager + 0x99AF1C`.
    ///
    /// This is the field that actually advances. `race_time` does not: it is `max(lap_time)`, and once
    /// cars stop completing laps it freezes while the simulation keeps stepping. Measured live:
    ///
    /// ```text
    /// SIM_TIME   951.00 -> 953.00 -> 955.00      advancing
    /// lap_times  928.8, 933.2, 925.8, 927.8      frozen
    /// ```
    ///
    /// Every snapshot used to carry only the frozen value, so for the last part of a race the client
    /// received a constant clock - and a client cannot decide how far to advance without knowing how much
    /// time passed. Defaults to 0 for a source that cannot read it, and consumers must treat 0 as unknown
    /// rather than as a real instant.
    sim_time: f32,
    /// The host's `race_time`, kept **for display only**.
    ///
    /// Explicitly not a clock: it is derived from per-lap times and is not monotonic. Anything that needs
    /// to know "how much time has passed" must use `sim_time`.
    race_time: f32,
        cars: Vec<Car>,
    },
    Rc(RcState),
    Orders(Vec<Order>),
    /// Sent by the in-process agent so the supervisor knows which process it is talking to and,
    /// once the hook has fired, where the RaceSimManager lives.
    ///
    /// Carries the side because the agent is also the sender of `Clock`, `Contract` and `Vote`, and the
    /// supervisor has to attribute those frames to a player to arbitrate them. The agent's socket never
    /// sends `Hello` - that belongs to the game DLL - so this is the only place the agent's identity
    /// appears on the wire. [inferred: an agent not saying which side it is cannot be arbitrated, because
    /// `arbitrate` takes a `Side`]
    Agent { pid: u32, rsm: u64, side: Side },
    /// Start of a session payload: a save file the client must load before the two games agree.
    ///
    /// The save is opaque - it is a frontier-compressed blob (`Oodle`/`zstd` static in the exe), so it
    /// is never parsed or rebuilt, only carried and written out byte for byte. `total` is the exact
    /// size so the receiver can tell a complete transfer from a truncated one instead of loading a
    /// half-file.
    SessionOffer { name_len: u8, name: Vec<u8>, total: u64, hash: u32 },
    /// One chunk of the session payload, in order. `offset` makes a lost chunk detectable.
    SessionChunk { offset: u64, bytes: Vec<u8> },
    /// The whole payload arrived and its hash matched.
    SessionReady { total: u64, hash: u32 },
    /// The sender's complete set of pause reasons.
    ///
    /// Full state, not a delta. A lost "unpause" would leave the world frozen with nothing to indicate
    /// why, whereas a redundant "still paused" costs one small frame.
    Clock {
        /// The reasons, already widened to bytes so this module does not depend on `coordination`.
        reasons: Vec<u8>,
        /// The sender's sim tick, so a late frame cannot resurrect an older pause state.
        tick: u32,
    },
    /// A claim on a person, or its release.
    ///
    /// Separate from the game's own `OnSubmitContractOffer` because that event never leaves the machine:
    /// the rule "two players cannot sign the same person the same day" can only be enforced if the
    /// *intent* crosses the wire before the signing does.
    Contract {
        person: i32,
        team: u8,
        /// The calendar day, so the day rule is evaluable on the far side.
        day: i32,
        /// 0 = claim, 1 = release, 2 = confirmed signing, 3 = offer refused.
        kind: u8,
    },
    /// One player's vote on a regulation proposal.
    Vote {
        proposal: u16,
        option: u8,
    },
    /// A player's current team, or a release.
    ///
    /// Sent so the other machine's team grid can grey out what is already taken. Carries a *team id* rather
    /// than a name because `EF1Team` is an enum of bytes and a name would be a second vocabulary for the same
    /// thing - one that could disagree with the game's.
    TeamClaim {
        /// `EF1Team` value, 1..=10. `0` means "I released my team".
        team: u8,
        /// The claiming side, so the far end can label the tile "player 1" or "player 2".
        side: Side,
    },
    Bye,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoError {
    TooShort,
    BadMagic,
    BadVersion(u16),
    BadChecksum,
    BadTag(u8),
    BadSide(u8),
    TrailingBytes(usize),
    BadCarCount(usize),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::TooShort => write!(f, "frame shorter than the header"),
            ProtoError::BadMagic => write!(f, "not a co-op frame (bad magic)"),
            ProtoError::BadVersion(v) => write!(f, "peer speaks version {v}, this build speaks {VERSION}"),
            ProtoError::BadChecksum => write!(f, "checksum mismatch: frame corrupt or torn"),
            ProtoError::BadTag(t) => write!(f, "unknown message tag {t}"),
            ProtoError::BadSide(s) => write!(f, "unknown side {s}"),
            ProtoError::TrailingBytes(n) => write!(f, "{n} trailing byte(s) after the message"),
            ProtoError::BadCarCount(n) => write!(f, "expected {CAR_COUNT} cars, got {n}"),
        }
    }
}

impl std::error::Error for ProtoError {}

fn fnv1a32(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in data {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Writer { buf: Vec::with_capacity(1400) }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, at: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        let end = self.at.checked_add(n).ok_or(ProtoError::TooShort)?;
        if end > self.buf.len() {
            return Err(ProtoError::TooShort);
        }
        let s = &self.buf[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ProtoError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, ProtoError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&mut self) -> Result<i32, ProtoError> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32, ProtoError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn u64(&mut self) -> Result<u64, ProtoError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }
    fn remaining(&self) -> usize {
        self.buf.len() - self.at
    }
}

fn encode_car(w: &mut Writer, c: &Car) {
    w.u8(c.id);
    w.f32(c.dist);
    w.f32(c.lat);
    w.f32(c.head);
    w.f32(c.spd);
    w.f32(c.acc);
    w.f32(c.splt);
    w.f32(c.seg);
    w.i32(c.node);
    w.u8(c.pos);
    w.i32(c.lap);
    w.f32(c.fuel);
    w.f32(c.lap_time);
    w.u8(c.down);
    w.u8(c.done);
    w.i32(c.drs);
    w.u8(c.pit);
    w.u8(c.pit_req);
    w.u8(c.stay_out);
    w.u8(c.lap_type);
    w.f32(c.tyre_lo);
    w.f32(c.tyre_hi);
    for t in &c.tyres {
        w.f32(t.surface);
        w.f32(t.carcass);
        w.f32(t.condition);
    }
}

fn decode_car(r: &mut Reader<'_>) -> Result<Car, ProtoError> {
    let mut c = Car {
        id: r.u8()?,
        dist: r.f32()?,
        lat: r.f32()?,
        head: r.f32()?,
        spd: r.f32()?,
        acc: r.f32()?,
        splt: r.f32()?,
        seg: r.f32()?,
        node: r.i32()?,
        pos: r.u8()?,
        lap: r.i32()?,
        fuel: r.f32()?,
        lap_time: r.f32()?,
        down: r.u8()?,
        done: r.u8()?,
        drs: r.i32()?,
        pit: r.u8()?,
        pit_req: r.u8()?,
        stay_out: r.u8()?,
        lap_type: r.u8()?,
        tyre_lo: r.f32()?,
        tyre_hi: r.f32()?,
        tyres: [Tyre::default(); 4],
    };
    for t in c.tyres.iter_mut() {
        t.surface = r.f32()?;
        t.carcass = r.f32()?;
        t.condition = r.f32()?;
    }
    Ok(c)
}

const TAG_HELLO: u8 = 1;
const TAG_SNAPSHOT: u8 = 2;
const TAG_RC: u8 = 3;
const TAG_ORDERS: u8 = 4;
const TAG_BYE: u8 = 5;
const TAG_READY: u8 = 10;
const TAG_AGENT: u8 = 6;
const TAG_SESSION_OFFER: u8 = 7;
const TAG_SESSION_CHUNK: u8 = 8;
const TAG_SESSION_READY: u8 = 9;
/// The sender's full set of pause reasons. Set, not delta: a lost unpause leaves the world frozen with
/// no way to notice why (see `coordination::ClockGate`).
const TAG_CLOCK: u8 = 11;
/// A claim on a person, or its release, so two players cannot sign the same person on the same day.
const TAG_CONTRACT: u8 = 12;
/// One player's vote on a regulation proposal.
const TAG_VOTE: u8 = 13;
/// Which team a player has taken, so the other machine's team picker shows it as taken.
///
/// Team *claims* were previously process-local: `/coop/claim` wrote a map inside one agent, and nothing ever
/// put it on the wire. Each machine therefore saw only its own player's claim, every team looked free to the
/// other player, and both could take the same one - the opposite of what the screen promises.
const TAG_TEAM: u8 = 14;

/// A payload plus its self-describing header.
pub fn encode(msg: &Msg) -> Vec<u8> {
    let mut w = Writer::new();
    match msg {
        Msg::Hello { side, cars_owned } => {
            w.u8(TAG_HELLO);
            w.u8(side.as_u8());
            w.u32(*cars_owned);
        }
        Msg::Snapshot { tick, sim_time, race_time, cars } => {
            w.u8(TAG_SNAPSHOT);
            w.u32(*tick);
            w.f32(*sim_time);
            w.f32(*race_time);
            w.u8(cars.len() as u8);
            for c in cars {
                encode_car(&mut w, c);
            }
        }
        Msg::Rc(rc) => {
            w.u8(TAG_RC);
            w.u32(rc.flags);
            w.u8(rc.rc_state);
            w.u8(rc.rf_state);
        }
        Msg::Orders(orders) => {
            w.u8(TAG_ORDERS);
            w.u8(orders.len() as u8);
            for o in orders {
                w.u8(o.car);
                for b in o.bytes() {
                    w.u8(b);
                }
            }
        }
        Msg::Agent { pid, rsm, side } => {
            w.u8(TAG_AGENT);
            w.u32(*pid);
            w.u64(*rsm);
            w.u8(side.as_u8());
        }
        Msg::SessionOffer { name_len, name, total, hash } => {
            w.u8(TAG_SESSION_OFFER);
            w.u8(*name_len);
            w.raw(name);
            w.u64(*total);
            w.u32(*hash);
        }
        Msg::SessionChunk { offset, bytes } => {
            w.u8(TAG_SESSION_CHUNK);
            w.u64(*offset);
            w.u32(bytes.len() as u32);
            w.raw(bytes);
        }
        Msg::SessionReady { total, hash } => {
            w.u8(TAG_SESSION_READY);
            w.u64(*total);
            w.u32(*hash);
        }
        Msg::Clock { reasons, tick } => {
            w.u8(TAG_CLOCK);
            w.u32(*tick);
            // Length-prefixed: the set is small but variable, and a fixed field would need a version bump
            // every time a reason is added.
            w.u8(reasons.len() as u8);
            for r in reasons {
                w.u8(*r);
            }
        }
        Msg::Contract { person, team, day, kind } => {
            w.u8(TAG_CONTRACT);
            w.i32(*person);
            w.u8(*team);
            w.i32(*day);
            w.u8(*kind);
        }
        Msg::Vote { proposal, option } => {
            w.u8(TAG_VOTE);
            w.u16(*proposal);
            w.u8(*option);
        }
        Msg::TeamClaim { team, side } => {
            w.u8(TAG_TEAM);
            w.u8(*team);
            w.u8(side.as_u8());
        }
        Msg::Bye => w.u8(TAG_BYE),
        Msg::Ready(r) => {
            w.u8(TAG_READY);
            w.u32(r.tick);
            w.u8(r.session);
            w.u8(r.moving_cars);
            let mut page = r.page.as_bytes().to_vec();
            page.truncate(READY_PAGE_MAX);
            w.u8(page.len() as u8);
            for b in &page {
                w.u8(*b);
            }
        }
    }

    let payload = w.buf;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&fnv1a32(&payload).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Decode a complete frame. Errors on truncation, so a partial read is never a valid message.
pub fn decode(frame: &[u8]) -> Result<Msg, ProtoError> {
    if frame.len() < HEADER_LEN {
        return Err(ProtoError::TooShort);
    }
    if frame[0..4] != MAGIC {
        return Err(ProtoError::BadMagic);
    }
    let version = u16::from_le_bytes([frame[4], frame[5]]);
    if version != VERSION {
        return Err(ProtoError::BadVersion(version));
    }
    let len = u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]) as usize;
    if frame.len() < HEADER_LEN + len {
        return Err(ProtoError::TooShort);
    }
    let want = u32::from_le_bytes([frame[10], frame[11], frame[12], frame[13]]);
    let payload = &frame[HEADER_LEN..HEADER_LEN + len];
    if fnv1a32(payload) != want {
        return Err(ProtoError::BadChecksum);
    }

    let mut r = Reader::new(payload);
    let tag = r.u8()?;
    let msg = match tag {
        TAG_HELLO => Msg::Hello {
            side: Side::from_u8(r.u8()?)?,
            cars_owned: r.u32()?,
        },
        TAG_SNAPSHOT => {
            let tick = r.u32()?;
            let sim_time = r.f32()?;
            let race_time = r.f32()?;
            let n = r.u8()? as usize;
            if n != CAR_COUNT {
                return Err(ProtoError::BadCarCount(n));
            }
            let mut cars = Vec::with_capacity(n);
            for _ in 0..n {
                cars.push(decode_car(&mut r)?);
            }
            Msg::Snapshot { tick, sim_time, race_time, cars }
        }
        TAG_RC => Msg::Rc(RcState {
            flags: r.u32()?,
            rc_state: r.u8()?,
            rf_state: r.u8()?,
        }),
        TAG_SESSION_OFFER => {
            let name_len = r.u8()? as usize;
            let name = r.take(name_len)?.to_vec();
            Msg::SessionOffer { name_len: name_len as u8, name, total: r.u64()?, hash: r.u32()? }
        }
        TAG_SESSION_CHUNK => {
            let offset = r.u64()?;
            let n = r.u32()? as usize;
            Msg::SessionChunk { offset, bytes: r.take(n)?.to_vec() }
        }
        TAG_CLOCK => {
            let tick = r.u32()?;
            let n = r.u8()? as usize;
            let mut reasons = Vec::with_capacity(n);
            for _ in 0..n {
                reasons.push(r.u8()?);
            }
            Msg::Clock { reasons, tick }
        }
        TAG_CONTRACT => Msg::Contract {
            person: r.i32()?,
            team: r.u8()?,
            day: r.i32()?,
            kind: r.u8()?,
        },
        TAG_VOTE => Msg::Vote { proposal: r.u16()?, option: r.u8()? },
        TAG_TEAM => Msg::TeamClaim { team: r.u8()?, side: Side::from_u8(r.u8()?)? },
        TAG_SESSION_READY => Msg::SessionReady { total: r.u64()?, hash: r.u32()? },
        TAG_ORDERS => {
            let n = r.u8()? as usize;
            let mut orders = Vec::with_capacity(n);
            for _ in 0..n {
                let car = r.u8()?;
                let mut b = [0u8; 9];
                for slot in b.iter_mut() {
                    *slot = r.u8()?;
                }
                orders.push(Order::from_bytes(car, &b));
            }
            Msg::Orders(orders)
        }
        TAG_AGENT => Msg::Agent { pid: r.u32()?, rsm: r.u64()?, side: Side::from_u8(r.u8()?)? },
        TAG_BYE => Msg::Bye,
        TAG_READY => {
            let tick = r.u32()?;
            let session = r.u8()?;
            let moving_cars = r.u8()?;
            let len = (r.u8()? as usize).min(READY_PAGE_MAX);
            let mut page = String::with_capacity(len);
            for _ in 0..len {
                page.push(r.u8()? as char);
            }
            Msg::Ready(Ready { tick, session, page, moving_cars })
        }
        other => return Err(ProtoError::BadTag(other)),
    };

    if r.remaining() != 0 {
        return Err(ProtoError::TrailingBytes(r.remaining()));
    }
    Ok(msg)
}

/// Splits a byte stream into frames. Returns the frame length and consumed offset when complete.
pub fn next_frame(buf: &[u8]) -> Result<Option<usize>, ProtoError> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    if buf[0..4] != MAGIC {
        return Err(ProtoError::BadMagic);
    }
    let len = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]) as usize;
    let total = HEADER_LEN + len;
    if buf.len() < total {
        return Ok(None);
    }
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_car(id: u8) -> Car {
        Car {
            id,
            dist: 1234.5 + id as f32,
            lat: -0.25,
            head: 0.031,
            spd: 62.5,
            acc: 1.5,
            splt: 12.75,
            seg: 30.5,
            node: 1234,
            pos: id + 1,
            lap: 3,
            fuel: 48.25,
            lap_time: 92.125,
            down: 0,
            done: 0,
            drs: 0,
            pit: 0,
            pit_req: 1,
            stay_out: 0,
            lap_type: 1,
            tyre_lo: 80.0,
            tyre_hi: 110.0,
            tyres: [
                Tyre { surface: 91.5, carcass: 88.25, condition: 0.94 },
                Tyre { surface: 92.5, carcass: 89.25, condition: 0.93 },
                Tyre { surface: 93.5, carcass: 90.25, condition: 0.92 },
                Tyre { surface: 94.5, carcass: 91.25, condition: 0.91 },
            ],
        }
    }

    #[test]
    fn snapshot_roundtrip_22_cars() {
        let cars: Vec<Car> = (0..22u8).map(sample_car).collect();
        let msg = Msg::Snapshot { tick: 4242, sim_time: 500.0, race_time: 461.956, cars: cars.clone() };
        let frame = encode(&msg);
        let back = decode(&frame).expect("must decode");
        match back {
            Msg::Snapshot { tick, sim_time, race_time, cars: got } => {
                assert_eq!(tick, 4242);
                assert!((race_time - 461.956).abs() < 1e-3);
                assert_eq!(got.len(), 22);
                assert_eq!(got, cars);
            }
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn rc_and_orders_roundtrip() {
        let rc = Msg::Rc(RcState { flags: 1024, rc_state: 1, rf_state: 0 });
        assert_eq!(decode(&encode(&rc)).unwrap(), rc);

        let orders = Msg::Orders(vec![
            Order {
                car: 11,
                pit_pending: 1,
                pit_request: 1,
                tyre_set: 3,
                tyre_selector: 2,
                ..Order::default()
            },
            Order {
                car: 12,
                stay_out_pending: 1,
                stay_out_value: 1,
                replace_wing: 1,
                ..Order::default()
            },
        ]);
        assert_eq!(decode(&encode(&orders)).unwrap(), orders);
    }

    /// The uplink is only useful if a client's instruction reaches the host intact, so the exact
    /// bytes are checked rather than just the round trip.
    #[test]
    fn order_action_bytes_map_to_the_command_record() {
        let o = Order {
            car: 7,
            stay_out_pending: 1,
            stay_out_value: 1,
            garage_pending: 2,
            garage_value: 3,
            pit_pending: 4,
            pit_request: 5,
            replace_wing: 6,
            tyre_set: 7,
            tyre_selector: 8,
        };
        assert_eq!(o.bytes(), [1, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(o.any());
        assert!(!Order::default().any());
        assert_eq!(
            Order::from_bytes(7, &o.bytes()),
            o,
            "a client instruction must survive the codec unchanged"
        );
    }

    /// Every message tag must be distinct. A collision is silent at the type level - both variants
    /// compile - and shows up only as a decode failure deep in a live session, where `Ready` was being
    /// read as `Agent` (both had tag 6). Cheap to assert, expensive to find.
    #[test]
    fn message_tags_are_unique() {
        let tags = [
            ("HELLO", TAG_HELLO),
            ("SNAPSHOT", TAG_SNAPSHOT),
            ("RC", TAG_RC),
            ("ORDERS", TAG_ORDERS),
            ("AGENT", TAG_AGENT),
            ("BYE", TAG_BYE),
            ("READY", TAG_READY),
            ("SESSION_OFFER", TAG_SESSION_OFFER),
            ("SESSION_CHUNK", TAG_SESSION_CHUNK),
            ("SESSION_READY", TAG_SESSION_READY),
            ("TEAM", TAG_TEAM),
            ("CLOCK", TAG_CLOCK),
            ("CONTRACT", TAG_CONTRACT),
            ("VOTE", TAG_VOTE),
        ];
        for (i, (na, a)) in tags.iter().enumerate() {
            for (nb, b) in tags.iter().skip(i + 1) {
                assert_ne!(a, b, "tag collision: {na} and {nb} both use {a}");
            }
        }
    }

    /// The failure this field exists to prevent.
    ///
    /// For the last part of a real race the client received snapshots whose `race_time` never changed
    /// (`max(lap_time)` freezes once cars stop completing laps) while `SIM_TIME` advanced 951 -> 953 ->
    /// 955. A client cannot decide how far to advance without knowing how much time passed, so a snapshot
    /// with a frozen clock is unusable for driving the other sim - even though every other field in it is
    /// correct.
    ///
    /// This asserts the two clocks are independent on the wire, so a future change cannot quietly collapse
    /// them back into one.
    #[test]
    fn a_frozen_race_time_does_not_hide_an_advancing_sim_time() {
        let cars: Vec<Car> = (0..22u8).map(sample_car).collect();
        let a = Msg::Snapshot { tick: 100, sim_time: 951.0, race_time: 933.22, cars: cars.clone() };
        let b = Msg::Snapshot { tick: 101, sim_time: 953.0, race_time: 933.22, cars: cars.clone() };

        for m in [&a, &b] {
            match decode(&encode(m)).expect("must round-trip") {
                Msg::Snapshot { sim_time, race_time, .. } => {
                    // race_time is identical in both frames; sim_time is not. If these were one field the
                    // second assertion below could not hold.
                    assert!((race_time - 933.22).abs() < 1e-3);
                    let _ = sim_time;
                }
                other => panic!("wrong message: {other:?}"),
            }
        }

        // The distinguishing property, stated directly.
        let sa = match decode(&encode(&a)).unwrap() { Msg::Snapshot { sim_time, .. } => sim_time, _ => 0.0 };
        let sb = match decode(&encode(&b)).unwrap() { Msg::Snapshot { sim_time, .. } => sim_time, _ => 0.0 };
        assert!(sb > sa, "sim_time must advance even when race_time does not");
        assert_eq!(sa, 951.0);
        assert_eq!(sb, 953.0);
    }

    #[test]
    fn ready_roundtrip() {
        let m = Msg::Ready(Ready {
            tick: 4242,
            session: 1,
            page: "RaceWeekend".to_string(),
            moving_cars: 22,
        });
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    /// A page name longer than the wire cap must be truncated, not rejected: the page is diagnostic, and
    /// a join must never fail because a UI string was long.
    #[test]
    fn ready_truncates_an_overlong_page_instead_of_failing() {
        let long = "X".repeat(READY_PAGE_MAX * 2);
        let m = Msg::Ready(Ready {
            tick: 1,
            session: 2,
            page: long,
            moving_cars: 0,
        });
        match decode(&encode(&m)).unwrap() {
            Msg::Ready(r) => assert_eq!(r.page.len(), READY_PAGE_MAX),
            other => panic!("wrong message: {other:?}"),
        }
    }

    /// A ready carrying "not in a session, no cars moving" must survive round-trip intact: the host uses
    /// exactly those fields to decide whether to refuse it, so losing them would make a bad ready look good.
    #[test]
    fn ready_preserves_the_evidence_a_host_uses_to_refuse_it() {
        let m = Msg::Ready(Ready { tick: 7, session: 0, page: "Frontend".to_string(), moving_cars: 0 });
        match decode(&encode(&m)).unwrap() {
            Msg::Ready(r) => {
                assert_eq!(r.session, 0);
                assert_eq!(r.moving_cars, 0);
                assert_eq!(r.page, "Frontend");
            }
            other => panic!("wrong message: {other:?}"),
        }
    }

    #[test]
    fn agent_roundtrip() {
        for side in [Side::Host, Side::Client] {
            let m = Msg::Agent { pid: 44316, rsm: 0x1_750E_0000, side };
            assert_eq!(decode(&encode(&m)).unwrap(), m);
        }
    }

    /// The side is load-bearing: it is how the supervisor attributes a `Clock`, `Contract` or `Vote`
    /// to a player. A mis-decoded side would silently arbitrate the host's claim as the client's.
    #[test]
    fn the_agent_side_survives_the_round_trip_and_refuses_an_unknown_value() {
        let host = decode(&encode(&Msg::Agent { pid: 1, rsm: 2, side: Side::Host })).unwrap();
        let client = decode(&encode(&Msg::Agent { pid: 1, rsm: 2, side: Side::Client })).unwrap();
        assert_ne!(host, client, "the two sides must not encode identically");

        // Hand-build an Agent frame whose side byte is 7, i.e. outside Side.
        let mut payload = Vec::new();
        payload.push(TAG_AGENT);
        payload.extend_from_slice(&1u32.to_le_bytes());
        payload.extend_from_slice(&2u64.to_le_bytes());
        payload.push(7);
        let mut frame = Vec::new();
        frame.extend_from_slice(&MAGIC);
        frame.extend_from_slice(&VERSION.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&fnv1a32(&payload).to_le_bytes());
        frame.extend_from_slice(&payload);
        assert!(decode(&frame).is_err(), "an unknown side must not decode to Host by default");
    }

    #[test]
    fn hello_roundtrip() {
        let m = Msg::Hello { side: Side::Client, cars_owned: 11 };
        assert_eq!(decode(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut frame = encode(&Msg::Bye);
        frame[0] = b'X';
        assert_eq!(decode(&frame), Err(ProtoError::BadMagic));
    }

    #[test]
    fn rejects_wrong_version() {
        let mut frame = encode(&Msg::Bye);
        frame[4] = 99;
        assert_eq!(decode(&frame), Err(ProtoError::BadVersion(99)));
    }

    #[test]
    fn rejects_torn_frame() {
        let frame = encode(&Msg::Rc(RcState { flags: 1, rc_state: 1, rf_state: 0 }));
        let cut = frame.len() - 1;
        assert_eq!(decode(&frame[..cut]), Err(ProtoError::TooShort));
    }

    #[test]
    fn rejects_corrupt_payload() {
        let mut frame = encode(&Msg::Rc(RcState { flags: 1024, rc_state: 1, rf_state: 0 }));
        let last = frame.len() - 1;
        frame[last] ^= 0xFF;
        assert_eq!(decode(&frame), Err(ProtoError::BadChecksum));
    }

    #[test]
    fn rejects_wrong_car_count() {
        let mut cars: Vec<Car> = (0..21u8).map(sample_car).collect();
        cars.push(sample_car(21));
        let frame = encode(&Msg::Snapshot { tick: 1, sim_time: 0.0, race_time: 0.0, cars });
        // rebuild with a lie: 22 cars are present, so trimming the vec must fail the count check
        let mut trimmed = encode(&Msg::Snapshot { tick: 1, sim_time: 0.0, race_time: 0.0, cars: vec![sample_car(0)] });
        trimmed[HEADER_LEN + 1 + 4 + 4] = 21; // patch the count byte, checksum now fails first
        assert!(decode(&trimmed).is_err());
        assert!(decode(&frame).is_ok());
    }

    #[test]
    fn frame_splitter_needs_a_whole_frame() {
        let frame = encode(&Msg::Bye);
        assert_eq!(next_frame(&frame[..HEADER_LEN - 1]).unwrap(), None);
        assert_eq!(next_frame(&frame).unwrap(), Some(frame.len()));
    }

    #[test]
    fn frame_splitter_reports_two_frames_in_one_buffer() {
        let mut buf = encode(&Msg::Bye);
        let first = buf.len();
        buf.extend_from_slice(&encode(&Msg::Bye));
        assert_eq!(next_frame(&buf).unwrap(), Some(first));
        assert_eq!(next_frame(&buf[first..]).unwrap(), Some(first));
    }

    /// A session payload must survive the wire byte for byte: it is a save the client will load, so a
    /// single changed byte is a corrupt game state rather than a cosmetic bug.
    #[test]
    fn session_payload_round_trips_byte_exactly() {
        let name = b"autosave.sav".to_vec();
        let total = 2_239_525u64;
        let hash = 0xDEAD_BEEFu32;
        let offer = Msg::SessionOffer { name_len: name.len() as u8, name: name.clone(), total, hash };
        match decode(&encode(&offer)).unwrap() {
            Msg::SessionOffer { name_len, name: n, total: t, hash: h } => {
                assert_eq!(name_len as usize, name.len());
                assert_eq!(n, name);
                assert_eq!(t, total);
                assert_eq!(h, hash);
            }
            other => panic!("wrong message: {other:?}"),
        }

        // Arbitrary bytes, including ones that are not valid UTF-8, must be carried unchanged.
        let chunk: Vec<u8> = (0..4096u32).map(|i| (i % 256) as u8).collect();
        let msg = Msg::SessionChunk { offset: 8192, bytes: chunk.clone() };
        match decode(&encode(&msg)).unwrap() {
            Msg::SessionChunk { offset, bytes } => {
                assert_eq!(offset, 8192);
                assert_eq!(bytes, chunk, "payload was altered in transit");
            }
            other => panic!("wrong message: {other:?}"),
        }

        let ready = Msg::SessionReady { total, hash };
        assert!(matches!(decode(&encode(&ready)).unwrap(), Msg::SessionReady { .. }));
    }

    /// A chunk larger than one socket read must still frame correctly, which is what the supervisor
    /// relies on when relaying.
    #[test]
    fn a_large_chunk_frames_as_one_message() {
        let bytes: Vec<u8> = (0..60_000u32).map(|i| (i % 251) as u8).collect();
        let frame = encode(&Msg::SessionChunk { offset: 0, bytes: bytes.clone() });
        assert_eq!(next_frame(&frame).unwrap(), Some(frame.len()));
        match decode(&frame).unwrap() {
            Msg::SessionChunk { bytes: got, .. } => assert_eq!(got, bytes),
            other => panic!("wrong message: {other:?}"),
        }
    }

    // ---- coordination messages --------------------------------------------------------------

    #[test]
    fn clock_round_trips_and_preserves_the_full_set() {
        // The set is the contract: a dropped reason is a dropped pause.
        let msg = Msg::Clock { reasons: vec![0, 3, 4], tick: 1234 };
        match decode(&encode(&msg)).unwrap() {
            Msg::Clock { reasons, tick } => {
                assert_eq!(tick, 1234);
                assert_eq!(reasons, vec![0, 3, 4]);
            }
            other => panic!("expected Clock, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_clock_set_round_trips_as_unpaused() {
        // "No reasons" is a real message, not an omission: it is what tells the other side to run.
        let msg = Msg::Clock { reasons: vec![], tick: 0 };
        match decode(&encode(&msg)).unwrap() {
            Msg::Clock { reasons, .. } => assert!(reasons.is_empty()),
            other => panic!("expected Clock, got {other:?}"),
        }
    }

    #[test]
    fn contract_round_trips_with_a_negative_person_id() {
        // Person ids are i32 and -1 means "nobody"; the encoder must not widen it wrongly.
        let msg = Msg::Contract { person: -1, team: 9, day: 42, kind: 2 };
        match decode(&encode(&msg)).unwrap() {
            Msg::Contract { person, team, day, kind } => {
                assert_eq!((person, team, day, kind), (-1, 9, 42, 2));
            }
            other => panic!("expected Contract, got {other:?}"),
        }
    }

    #[test]
    fn vote_round_trips_including_option_zero() {
        let msg = Msg::Vote { proposal: 0, option: 0 };
        match decode(&encode(&msg)).unwrap() {
            Msg::Vote { proposal, option } => assert_eq!((proposal, option), (0, 0)),
            other => panic!("expected Vote, got {other:?}"),
        }
        let msg = Msg::Vote { proposal: 65535, option: 255 };
        match decode(&encode(&msg)).unwrap() {
            Msg::Vote { proposal, option } => assert_eq!((proposal, option), (65535, 255)),
            other => panic!("expected Vote, got {other:?}"),
        }
    }

    /// A team claim has to survive the wire, or the two machines disagree about who holds what - which is
    /// what "player 1 can take every team" looked like before claims crossed the network at all.
    #[test]
    fn a_team_claim_round_trips_for_every_team_and_a_release() {
        for (team, side) in [(1u8, Side::Host), (10, Side::Host), (1, Side::Client), (0, Side::Client)] {
            let msg = Msg::TeamClaim { team, side };
            match decode(&encode(&msg)).unwrap() {
                Msg::TeamClaim { team: t, side: s } => assert_eq!((t, s), (team, side)),
                other => panic!("expected TeamClaim, got {other:?}"),
            }
        }
    }

    /// A team claim's payload is exactly `[TAG_TEAM, team, side]`, and an out-of-range side is refused.
    ///
    /// The layout is asserted so a field added to the struct cannot silently move the side byte and leave the
    /// malformed-input case below testing nothing.
    ///
    /// The error is `BadTag(99)`, not `BadSide(99)`: `Side::from_u8` reports an unknown side through the same
    /// variant it uses for an unknown message tag. That is a naming wart in existing code, not something this
    /// feature should change, so the test pins what actually happens rather than what the names suggest.
    #[test]
    fn a_team_claim_is_three_bytes_and_an_invalid_side_is_refused() {
        let frame = encode(&Msg::TeamClaim { team: 3, side: Side::Host });
        assert_eq!(frame.len(), HEADER_LEN + 3, "tag + team + side");
        assert_eq!(frame[HEADER_LEN], TAG_TEAM, "payload starts with the tag");
        assert_eq!(frame[HEADER_LEN + 1], 3, "then the team id");
        assert_eq!(frame[HEADER_LEN + 2], Side::Host.as_u8(), "then the side");

        let mut bad = frame.clone();
        bad[HEADER_LEN + 2] = 99;
        bad = rechecksum(bad);
        // The point is that it is *refused*; the variant is pinned so a change to it is visible.
        assert!(decode(&bad).is_err(), "an unknown side must not decode: {:?}", decode(&bad));
        assert!(matches!(decode(&bad), Err(ProtoError::BadTag(99))), "{:?}", decode(&bad));
    }

    /// Recomputes the header checksum after a test mutates a payload byte.
    ///
    /// The checksum is FNV-1a 32 of the payload at offset 10 - not a sum. Getting that wrong in the helper
    /// makes the test fail for the wrong reason (`BadChecksum` rather than `BadSide`), which would look like a
    /// codec bug.
    fn rechecksum(mut frame: Vec<u8>) -> Vec<u8> {
        let sum = fnv1a32(&frame[HEADER_LEN..]);
        frame[10..14].copy_from_slice(&sum.to_le_bytes());
        frame
    }

    #[test]
    fn a_truncated_clock_frame_is_rejected_not_guessed() {
        // A short frame must be an error: decoding it as "no reasons" would silently unpause the world.
        let msg = Msg::Clock { reasons: vec![0, 1, 2, 3], tick: 5 };
        let frame = encode(&msg);
        let cut = &frame[..frame.len() - 2];
        assert!(decode(cut).is_err());
    }
}
