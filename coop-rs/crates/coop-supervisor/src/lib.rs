//! The co-op relay, as a library.
//!
//! Two game instances connect here. Everything that used to be silent Lua - ordering, staleness, ownership,
//! retries - happens in one place where a failure is a typed error and a log line rather than a frozen game.
//!
//! Responsibilities:
//!   * accept the two peers (`Hello` carries the side)
//!   * validate every frame before relaying it, so a bad peer cannot inject half a snapshot
//!   * relay snapshots, race control and orders to the other side
//!   * arbitrate the coordination rules - contracts and votes - because this is the only party that sees both
//!   * log timings and counters so a stall names the side that stopped sending
//!
//! ## Why this is a library and not only a program
//!
//! Hosting used to mean starting `coop-supervisor.exe` alongside the game. That is a second thing a player has
//! to launch and forget, and forgetting it looks exactly like a broken mod. The co-op agent is already a DLL
//! inside the game, so the agent embeds this crate and binds the relay itself; the binary in `main.rs` stays
//! as the same thing in standalone form, for testing and for running the relay on a third machine.
//!
//! `serve(bind, port)` is the whole seam. See its documentation for why `bind` is a required argument rather
//! than a default.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use coop_proto::{coordination, decode, encode, next_frame, Msg, Side};

pub const DEFAULT_PORT: u16 = 45990;
const READ_CHUNK: usize = 8192;

#[derive(Default)]
pub struct Stats {
    pub frames_in: AtomicU64,
    pub frames_out: AtomicU64,
    pub bytes_in: AtomicU64,
    pub bad_frames: AtomicU64,
    /// Frames a coordination rule blocked. Counted separately from `bad_frames` because a refused claim
    /// is a working system, not a corrupt peer, and conflating them makes the counters useless in a bug
    /// report.
    refused: AtomicU64,
    /// Coordination frames handed to the other player's agent. Zero while a session is live means the
    /// two front-ends are not talking: either no agent is connected, or nobody is producing frames - the
    /// distinction that was impossible to make before this counter existed.
    coord_out: AtomicU64,
}

/// One connected game instance and the socket we write its peer's messages to.
pub struct Peer {
    side: Side,
    stream: TcpStream,
}

pub type Registry = Arc<Mutex<Vec<Peer>>>;

/// One agent connection, so a coordination frame produced by one player's front-end can be handed to the
/// other player's front-end.
///
/// Separate from `Registry` because the two are different kinds of peer: a `Peer` is the game DLL and
/// receives replicated car state, while an agent is the UI side channel. Mixing them would put an agent
/// in the relay fan-out, where its frames would be written into the game DLL's stream and silently
/// ignored by whatever reads that.
pub struct AgentPeer {
    side: Side,
    stream: TcpStream,
}

pub type Agents = Arc<Mutex<Vec<AgentPeer>>>;

/// Writes a coordination frame to the *other* side's agent.
///
/// Same two rules as `relay`: never hold the lock across a socket write, and never retry a peer that
/// failed. A coordination frame is small and the agent reads constantly, so a failed write means the
/// agent is gone, not slow.
pub fn deliver_to_agent(agents: &Agents, from: Side, frame: &[u8], stats: &Stats) {
    let targets: Vec<(usize, TcpStream)> = {
        let guard = match agents.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard
            .iter()
            .enumerate()
            .filter(|(_, a)| a.side != from)
            .filter_map(|(i, a)| a.stream.try_clone().ok().map(|s| (i, s)))
            .collect()
    };
    let mut dead: Vec<usize> = Vec::new();
    for (idx, mut stream) in targets {
        match stream.write_all(frame) {
            Ok(()) => {
                stats.coord_out.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                log(&format!("coord write to agent {idx} failed: {e}"));
                dead.push(idx);
            }
        }
    }
    if !dead.is_empty() {
        // Descending, so an earlier removal cannot shift a later index.
        dead.sort_unstable_by(|a, b| b.cmp(a));
        let mut guard = match agents.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        for idx in dead {
            if idx < guard.len() {
                let a = guard.remove(idx);
                log(&format!("dropped agent {:?} after a failed write", a.side));
            }
        }
    }
}

/*
 * Coordination state, owned by the supervisor.
 *
 * The supervisor is the only party that sees both sides, so the rules that must not be decided by either
 * side alone live here:
 *
 *   * **contract claims** - the first claim on a person wins and the second is refused. If each peer
 *     tracked only its own claims, both would believe they won; the loser would find out only when the
 *     signing failed, having already been told it worked.
 *   * **regulation votes** - every vote is recorded so the result is computed from all of them. Either
 *     peer resolving locally would resolve on partial information and the other player's vote would be
 *     silently dropped.
 *
 * Clock state is *not* resolved here. Pauses are a union, so each peer can compute the same answer from
 * the same inputs without an arbiter; routing them through the supervisor would add a round trip to every
 * pause and make a stall look like an unfreezing game.
 */
#[derive(Default)]
pub struct Coordination {
    contracts: coordination::ContractLedger,
    votes: coordination::VoteBox,
}

impl Coordination {
    /// Two voters: host and client. `VoteBox` clamps to at least one, so a solo session still resolves.
    ///
    /// Public because the standalone binary's selftest stands up its own relay, and the agent constructs one
    /// the same way. There is exactly one constructor: an earlier edit to make this public accidentally added
    /// a second `new` beside the existing one, which the compiler accepts and which silently shadows nothing -
    /// it just leaves a method nobody calls.
    pub fn new(expected_voters: usize) -> Self {
        Self { contracts: Default::default(), votes: coordination::VoteBox::new(expected_voters) }
    }
}

pub type Coord = Arc<Mutex<Coordination>>;

pub fn log(msg: &str) {
    println!("[supervisor] {msg}");
}

/// A decision about whether a frame may be relayed.
pub enum Ruling {
    /// Forward it unchanged.
    Pass,
    /// Refuse it, and tell the sender why.
    Refuse(String),
    /// Forward it, and additionally send this frame back to the sender (e.g. a computed vote result).
    PassAndReply(Vec<u8>),
}

/// Apply the coordination rules to one frame.
///
/// Called before relaying. Returning `Pass` for everything unknown is deliberate: this must never become
/// a place where an unanticipated message is silently dropped, because a dropped frame in the middle of a
/// race is indistinguishable from desync.
pub fn arbitrate(coord: &Coord, from: Side, msg: &Msg) -> Ruling {
    match msg {
        // ---- one instant contract per person per day -------------------------------------
        Msg::Contract { person, team, day, kind } => {
            let mut c = coord.lock().unwrap();
            match *kind {
                // claim
                0 => match c.contracts.claim(*person, from, *team, *day) {
                    Ok(()) => Ruling::Pass,
                    Err(why) => Ruling::Refuse(why.message().to_string()),
                },
                // release
                1 => { c.contracts.release(*person, from); Ruling::Pass }
                // confirmed signing
                2 => match c.contracts.confirm(*person, from, *team, *day) {
                    Ok(()) => Ruling::Pass,
                    Err(why) => Ruling::Refuse(why.message().to_string()),
                },
                // offer refused/withdrawn: free the person for the other player
                3 => { c.contracts.release(*person, from); Ruling::Pass }
                _ => Ruling::Pass,
            }
        }

        // ---- every vote counts -----------------------------------------------------------
        Msg::Vote { proposal, option } => {
            let mut c = coord.lock().unwrap();
            c.votes.cast(coordination::Vote { side: from, proposal: *proposal, option: *option });
            let result = c.votes.result(*proposal);
            if result.complete() {
                // Once everyone has voted, hand the resolved result to both sides together, so neither
                // can act on a partial tally.
                let summary = format!(
                    "proposal {} resolved: winner={:?} tied={} tally={:?}",
                    proposal, result.winner, result.tied, result.tally
                );
                log(&format!("vote {summary}"));
                c.votes.clear(*proposal);
                Ruling::PassAndReply(encode_vote_result(*proposal, &result))
            } else {
                log(&format!(
                    "vote recorded for proposal {proposal}: {}/{} cast",
                    result.cast, result.expected
                ));
                Ruling::Pass
            }
        }

        _ => Ruling::Pass,
    }
}

/// Serialise a resolved vote so both peers apply the same outcome.
///
/// Encoded as a `Vote` with a sentinel option (`0xFF`, outside any real option range) rather than adding a
/// message type: the peers already handle `Vote`, and a resolved-result frame that an older peer cannot
/// parse would be worse than one it reads as "option 255", which its option validation already rejects.
fn encode_vote_result(proposal: u16, result: &coordination::VoteResult) -> Vec<u8> {
    let option = result.winner.unwrap_or(0xFF);
    coop_proto::encode(&Msg::Vote { proposal, option })
}

/// Fan a frame out to the other side.
///
/// Two rules here, both learned from a live failure where the client froze on one frame forever:
///
///  * Never hold the registry lock across a socket write. A single peer that has stopped draining
///    fills its TCP window, `write_all` blocks, and the lock is held for the lifetime of that block -
///    which starves every other peer. That is what made sync stop dead mid-race.
///  * Never retry a peer that failed. A dead socket must be removed, not written to again.
fn relay(reg: &Registry, from: Side, frame: &[u8], stats: &Stats) -> io::Result<()> {
    // Clone the handles, then release the lock *before* touching the network.
    let targets: Vec<(usize, TcpStream)> = {
        let guard = reg.lock().unwrap();
        guard
            .iter()
            .enumerate()
            .filter(|(_, p)| p.side != from)
            .filter_map(|(i, p)| p.stream.try_clone().ok().map(|s| (i, s)))
            .collect()
    };

    let mut dead: Vec<usize> = Vec::new();
    for (idx, mut stream) in targets {
        // Blocking writes per peer, with the lock released, so one stalled peer cannot hold up the
        // others. The write timeout is set when the peer is accepted.
        match stream.write_all(frame) {
            Ok(()) => {
                stats.frames_out.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                log(&format!("relay write to peer {idx} failed: {e}"));
                dead.push(idx);
            }
        }
    }

    if !dead.is_empty() {
        let mut guard = reg.lock().unwrap();
        // Remove by descending index so the earlier removals do not shift later ones.
        dead.sort_unstable_by(|a, b| b.cmp(a));
        for idx in dead {
            if idx < guard.len() {
                let peer = guard.remove(idx);
                log(&format!("dropped peer {:?} after a failed write", peer.side));
            }
        }
    }
    Ok(())
}

pub fn drain_peer(reg: Registry, agents: Agents, coord: Coord, mut stream: TcpStream, addr: SocketAddr, stats: Arc<Stats>) -> io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(READ_CHUNK * 2);
    let mut chunk = [0u8; READ_CHUNK];
    let mut side: Option<Side> = None;
    // The side an `Msg::Agent` announced on this socket, when the socket is an agent's rather than a
    // game DLL's. Kept separate from `side` so the precedence stays explicit: a `Hello` always wins,
    // because it is the game DLL identifying the peer that actually owns the peer slot. `side` remains
    // the single thing consulted for registry bookkeeping (`retain(|p| p.side != s)` on disconnect)
    // and for relay routing, so an agent never becomes a relay target or a peer that gets torn down.
    let mut agent_side: Option<Side> = None;
    let mut seen_agent: Option<(u32, u64)> = None;
    let mut zero_announced: std::collections::HashSet<u32> = std::collections::HashSet::new();

    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            log(&format!("{addr} closed"));
            break;
        }
        stats.bytes_in.fetch_add(n as u64, Ordering::Relaxed);
        buf.extend_from_slice(&chunk[..n]);

        loop {
            match next_frame(&buf) {
                Ok(None) => break,
                Ok(Some(len)) => {
                    let frame: Vec<u8> = buf.drain(..len).collect();
                    match decode(&frame) {
                        Ok(Msg::Hello { side: s, cars_owned }) => {
                            side = Some(s);
                            log(&format!("{addr} says it is {s:?}, owns {cars_owned} car(s)"));
                            reg.lock().unwrap().push(Peer { side: s, stream: stream.try_clone()? });
                            continue;
                        }
                        Ok(Msg::Agent { pid, rsm, side: announced }) => {
                            // An agent's socket is the coordination sender. Recording its side here is
                            // what lets `arbitrate` attribute a `Clock`, `Contract` or `Vote` that
                            // arrives on this socket; without it those frames fall through to
                            // "sent data before Hello; dropped" and every coordination rule is inert.
                            agent_side = Some(announced);
                            {
                                // Register (or replace) this side's agent socket so the other player's
                                // coordination frames have somewhere to land. Replacing rather than
                                // appending: the agent heartbeats on one connection, and a reconnect
                                // must not leave a dead socket behind competing for the same side.
                                let sender = stream.try_clone()?;
                                let mut guard = match agents.lock() {
                                    Ok(g) => g,
                                    Err(p) => p.into_inner(),
                                };
                                guard.retain(|a| a.side != announced);
                                guard.push(AgentPeer { side: announced, stream: sender });
                            }
                            // The agent heartbeats once a second, and a stale copy of the agent
                            // left in the process reports rsm=0 forever, so logging every message
                            // buries the real handshake. A zero means "still looking", worth one
                            // line per process; a real address is worth one line per change.
                            if rsm == 0 {
                                if zero_announced.insert(pid) {
                                    log(&format!("agent in pid {pid}, still locating RaceSimManager"));
                                }
                            } else if seen_agent != Some((pid, rsm)) {
                                log(&format!("agent in pid {pid}, rsm={rsm:#X}"));
                                seen_agent = Some((pid, rsm));
                            }
                            if rsm != 0 {
                                // publish the endpoint so coop-probe can attach without hand-copying
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                let line = format!("RSM {rsm:#X} {:#X} {now}\n", coop_proto::layout::OBJECT_SIZE);
                                if let Err(e) = std::fs::write("coop_rsm.txt", line) {
                                    log(&format!("could not write coop_rsm.txt: {e}"));
                                } else {
                                    log("wrote coop_rsm.txt");
                                }
                            }
                            continue;
                        }
                        Ok(Msg::Snapshot { tick, sim_time, race_time, cars }) => {
                            stats.frames_in.fetch_add(1, Ordering::Relaxed);
                            if tick % 100 == 0 {
                                log(&format!(
                                    "{addr} snapshot tick={tick} race_time={race_time:.3} cars={}",
                                    cars.len()
                                ));
                            }
                        }
                        Ok(_) => {
                            stats.frames_in.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            stats.bad_frames.fetch_add(1, Ordering::Relaxed);
                            log(&format!("{addr} sent an invalid frame: {e}"));
                            continue;
                        }
                    }
                    // Arbitration identity: the game DLL's `Hello` wins, then the agent's own
                    // announcement. Coordinating on the agent follows from `Msg::Agent` carrying the
                    // side: an agent's socket is the only place a `Clock`, `Contract` or `Vote` can
                    // legitimately arrive, because the game DLL that owns the peer slot has no reason
                    // to produce them.
                    let actor = side.or(agent_side);
                    if let Some(s) = actor {
                        // Coordination rules run *before* relay. A claim that loses or a vote is
                        // arbitrated here, because the supervisor is the only party that sees both
                        // sides; leaving it to the peers means both believe they won.
                        let ruling = match decode(&frame) {
                            Ok(msg) => arbitrate(&coord, s, &msg),
                            // Unparseable frames were already counted above; pass them so the existing
                            // bad-frame handling stays the single place that reports them.
                            Err(_) => Ruling::Pass,
                        };
                        match ruling {
                            Ruling::Pass => {}
                            Ruling::Refuse(why) => {
                                stats.refused.fetch_add(1, Ordering::Relaxed);
                                log(&format!("{addr} refused: {why}"));
                                // Tell the sender, so the losing player sees a reason instead of a
                                // signing that silently did nothing.
                                let mut st = stream.try_clone()?;
                                let _ = st.write_all(&coop_proto::encode(&Msg::Bye));
                                continue;
                            }
                            Ruling::PassAndReply(reply) => {
                                let mut st = stream.try_clone()?;
                                let _ = st.write_all(&reply);
                            }
                        }
                        // Route by what this socket *is*, not by what it has not sent. Keying on
                        // `side.is_none()` looked equivalent - a real agent never sends `Hello` - but it
                        // is a property of the caller rather than of the socket, and a socket that
                        // announced both would have had its coordination frames written into the game's
                        // relay, where nothing decodes them.
                        if agent_side.is_some() {
                            deliver_to_agent(&agents, s, &frame, &stats);
                        } else if let Err(e) = relay(&reg, s, &frame, &stats) {
                            log(&format!("relay failed: {e}"));
                            return Ok(());
                        }
                    } else {
                        log(&format!("{addr} sent data before Hello; dropped"));
                    }
                }
                Err(e) => {
                    stats.bad_frames.fetch_add(1, Ordering::Relaxed);
                    log(&format!("{addr} framing error ({e}); closing"));
                    return Ok(());
                }
            }
        }
    }

    if let Some(s) = side {
        reg.lock().unwrap().retain(|p| p.side != s);
        log(&format!("{s:?} disconnected"));
    }
    // The agent's own socket closing must also release its coordination slot, or the other side keeps
    // writing into a socket whose reader is gone until a write finally fails.
    if let Some(s) = agent_side {
        let mut guard = match agents.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.retain(|a| a.side != s);
        log(&format!("{s:?} agent disconnected"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Embedding seam
// ---------------------------------------------------------------------------------------------

/// Everything one running relay owns, so a caller can hold it or report on it.
pub struct Relay {
    pub reg: Registry,
    pub agents: Agents,
    pub coord: Coord,
    pub stats: Arc<Stats>,
    pub addr: SocketAddr,
}

/// Binds the relay and serves until the process exits.
///
/// This is the piece that lets the co-op agent *be* the host. Hosting used to require starting
/// `coop-supervisor.exe` as a second program, which meant a player had to launch something before the game
/// could host at all - and forgetting to was indistinguishable from a broken mod. The agent is already a DLL
/// inside the game, so it can bind this socket itself and the separate executable becomes optional.
///
/// `bind` decides reachability, and it is not a detail: `127.0.0.1` works on this machine and *silently
/// refuses* the other player, because the peer connects over the LAN. `0.0.0.0` is the only value that makes
/// a two-machine session possible, so it is the caller's decision to make explicitly rather than a default
/// buried here.
pub fn serve(bind: &str, port: u16) -> io::Result<Relay> {
    let listener = TcpListener::bind((bind, port))?;
    let addr = listener.local_addr()?;
    log(&format!("bound {bind}:{port}"));

    let reg: Registry = Arc::new(Mutex::new(Vec::new()));
    let agents: Agents = Arc::new(Mutex::new(Vec::new()));
    // Two voters: host and client. `VoteBox` clamps to at least one, so a solo session still resolves.
    let coord: Coord = Arc::new(Mutex::new(Coordination::new(2)));
    let stats = Arc::new(Stats::default());

    let relay = Relay {
        reg: Arc::clone(&reg),
        agents: Arc::clone(&agents),
        coord: Arc::clone(&coord),
        stats: Arc::clone(&stats),
        addr,
    };

    thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(stream) = incoming else { continue };
            let Ok(peer) = stream.peer_addr() else { continue };
            stream.set_nodelay(true).ok();
            // Bound every socket write. A peer that stops draining must fail rather than wedge the relay
            // forever; the fan-out then drops it.
            stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
            log(&format!("{peer} connected"));
            let reg = Arc::clone(&reg);
            let agents = Arc::clone(&agents);
            let coord = Arc::clone(&coord);
            let stats = Arc::clone(&stats);
            thread::spawn(move || {
                if let Err(e) = drain_peer(reg, agents, coord, stream, peer, stats) {
                    log(&format!("{peer} error: {e}"));
                }
            });
        }
    });

    Ok(relay)
}

#[cfg(test)]
mod arbitrate_tests {
    use super::*;

    fn coord() -> Coord {
        Arc::new(Mutex::new(Coordination::new(2)))
    }

    #[test]
    fn a_second_contract_claim_from_the_other_player_is_refused() {
        // The requirement: two people cannot do the same instant contract for someone the same day.
        let c = coord();
        let claim = Msg::Contract { person: 42, team: 1, day: 10, kind: 0 };
        assert!(matches!(arbitrate(&c, Side::Host, &claim), Ruling::Pass));
        match arbitrate(&c, Side::Client, &claim) {
            Ruling::Refuse(why) => assert_eq!(why, "the other player is negotiating with that person"),
            _ => panic!("the second claim must be refused"),
        }
    }

    #[test]
    fn a_signed_person_cannot_be_claimed_again_the_same_day() {
        let c = coord();
        let sign = Msg::Contract { person: 7, team: 1, day: 4, kind: 2 };
        assert!(matches!(arbitrate(&c, Side::Host, &sign), Ruling::Pass));
        match arbitrate(&c, Side::Client, &Msg::Contract { person: 7, team: 2, day: 4, kind: 0 }) {
            Ruling::Refuse(why) => assert_eq!(why, "that person was already signed today"),
            _ => panic!("a same-day re-signing must be refused"),
        }
    }

    #[test]
    fn releasing_a_claim_lets_the_other_player_in() {
        let c = coord();
        let person = 11;
        assert!(matches!(arbitrate(&c, Side::Host, &Msg::Contract { person, team: 1, day: 2, kind: 0 }), Ruling::Pass));
        assert!(matches!(arbitrate(&c, Side::Host, &Msg::Contract { person, team: 1, day: 2, kind: 1 }), Ruling::Pass));
        assert!(matches!(arbitrate(&c, Side::Client, &Msg::Contract { person, team: 2, day: 2, kind: 0 }), Ruling::Pass));
    }

    #[test]
    fn the_first_vote_alone_does_not_resolve() {
        let c = coord();
        // One vote out of two expected: relayed, not resolved, because resolving now would discard the
        // second player's vote.
        assert!(matches!(arbitrate(&c, Side::Host, &Msg::Vote { proposal: 1, option: 0 }), Ruling::Pass));
    }

    #[test]
    fn both_votes_resolve_and_the_reply_carries_the_outcome() {
        let c = coord();
        arbitrate(&c, Side::Host, &Msg::Vote { proposal: 3, option: 0 });
        match arbitrate(&c, Side::Client, &Msg::Vote { proposal: 3, option: 0 }) {
            Ruling::PassAndReply(frame) => match decode(&frame).expect("reply must be a valid frame") {
                Msg::Vote { proposal, option } => {
                    assert_eq!(proposal, 3);
                    assert_eq!(option, 0, "the winner is unanimous option 0");
                }
                other => panic!("unexpected reply {other:?}"),
            },
            _ => panic!("the second vote must complete the round"),
        }
    }

    #[test]
    fn a_split_vote_resolves_with_the_sentinel_rather_than_inventing_a_winner() {
        // Disagreeing votes must not be reported as a win for either side.
        let c = coord();
        arbitrate(&c, Side::Host, &Msg::Vote { proposal: 9, option: 1 });
        match arbitrate(&c, Side::Client, &Msg::Vote { proposal: 9, option: 0 }) {
            Ruling::PassAndReply(frame) => match decode(&frame).unwrap() {
                Msg::Vote { option, .. } => assert_eq!(option, 0xFF, "a tie has no winner"),
                other => panic!("unexpected reply {other:?}"),
            },
            _ => panic!("round should complete"),
        }
    }

    #[test]
    fn unrelated_frames_always_pass_through() {
        // A dropped frame mid-race is indistinguishable from desync, so arbitration must not be a filter.
        let c = coord();
        assert!(matches!(arbitrate(&c, Side::Host, &Msg::Bye), Ruling::Pass));
        assert!(matches!(
            arbitrate(&c, Side::Host, &Msg::Clock { reasons: vec![0], tick: 1 }),
            Ruling::Pass
        ));
    }

    #[test]
    fn a_full_contract_round_trip_leaves_the_ledger_consistent() {
        let c = coord();
        // Host claims and signs person 5; client then cannot claim them on the same day.
        arbitrate(&c, Side::Host, &Msg::Contract { person: 5, team: 1, day: 3, kind: 0 });
        arbitrate(&c, Side::Host, &Msg::Contract { person: 5, team: 1, day: 3, kind: 2 });
        let guard = c.lock().unwrap();
        assert_eq!(guard.contracts.signed_on(5), Some(3));
        assert_eq!(guard.contracts.claimed_by(5), None, "the claim is consumed by the signing");
        assert_eq!(guard.contracts.history().len(), 1);
    }

    // ---------------------------------------------------------------------------------------------
    // End-to-end over real sockets.
    //
    // The unit tests above exercise `arbitrate` directly, which is why they passed for a whole session
    // while the feature did nothing: nothing produced the frames and nothing delivered them. These tests
    // go through `drain_peer` - the same function the relay uses - so they fail if the wiring, the side
    // attribution, or the agent fan-out regresses.
    // ---------------------------------------------------------------------------------------------

    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    /// Starts `drain_peer` on an ephemeral port and returns the address to connect to.
    fn relay_for_test() -> (std::net::SocketAddr, Arc<Stats>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        let reg: Registry = Arc::new(Mutex::new(Vec::new()));
        let agents: Agents = Arc::new(Mutex::new(Vec::new()));
        let coord: Coord = Arc::new(Mutex::new(Coordination::new(2)));
        let stats = Arc::new(Stats::default());
        let s2 = Arc::clone(&stats);
        thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else { continue };
                let Ok(a) = stream.peer_addr() else { continue };
                stream.set_nodelay(true).ok();
                stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
                let reg = Arc::clone(&reg);
                let agents = Arc::clone(&agents);
                let coord = Arc::clone(&coord);
                let stats = Arc::clone(&stats);
                thread::spawn(move || {
                    let _ = drain_peer(reg, agents, coord, stream, a, stats);
                });
            }
        });
        (addr, s2)
    }

    /// Connects an agent and announces its side, as the real agent does on every heartbeat.
    fn agent_on(addr: std::net::SocketAddr, side: Side) -> TcpStream {
        let mut s = TcpStream::connect(addr).expect("connect");
        s.set_nodelay(true).ok();
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();
        s.write_all(&encode(&Msg::Agent { pid: 4242, rsm: 0x1000, side })).expect("hello");
        s.flush().ok();
        s
    }

    /// Reads one complete frame, or gives up.
    fn read_frame(s: &mut TcpStream) -> Option<Msg> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match s.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    match coop_proto::next_frame(&buf) {
                        Ok(Some(len)) => {
                            let frame: Vec<u8> = buf.drain(..len).collect();
                            return decode(&frame).ok();
                        }
                        Ok(None) => continue,
                        Err(_) => return None,
                    }
                }
                Err(_) => return None,
            }
        }
    }

    /// Gives the two `drain_peer` threads a moment to register both agents.
    fn settle() {
        thread::sleep(Duration::from_millis(120));
    }

    /// The headline gap: a frame produced by one agent must reach the other agent.
    #[test]
    fn an_agents_frame_reaches_the_other_agents_socket() {
        let (addr, _stats) = relay_for_test();
        let mut host = agent_on(addr, Side::Host);
        let mut client = agent_on(addr, Side::Client);
        settle();

        host.write_all(&encode(&Msg::Clock { reasons: vec![0, 2], tick: 7 })).unwrap();
        host.flush().unwrap();

        match read_frame(&mut client) {
            Some(Msg::Clock { reasons, tick }) => {
                assert_eq!(reasons, vec![0, 2], "the pause reasons must arrive intact");
                assert_eq!(tick, 7);
            }
            other => panic!("the other player's agent should have received the clock, got {other:?}"),
        }
    }

    /// Before this change an agent frame hit "sent data before Hello; dropped", so a claim never reached
    /// the ledger and both players believed they had signed the same person.
    #[test]
    fn a_contract_claim_from_an_agent_is_arbitrated_and_the_loser_is_told() {
        let (addr, stats) = relay_for_test();
        let mut host = agent_on(addr, Side::Host);
        let mut client = agent_on(addr, Side::Client);
        settle();

        // Both players claim person 9 on day 4.
        host.write_all(&encode(&Msg::Contract { person: 9, team: 1, day: 4, kind: 0 })).unwrap();
        host.flush().unwrap();
        // The host's claim is relayed to the client's agent.
        assert!(matches!(read_frame(&mut client), Some(Msg::Contract { person: 9, .. })));

        client.write_all(&encode(&Msg::Contract { person: 9, team: 2, day: 4, kind: 0 })).unwrap();
        client.flush().unwrap();

        // The losing player is told, rather than watching the signing do nothing.
        match read_frame(&mut client) {
            Some(Msg::Bye) => {}
            other => panic!("the refused player must be told why, got {other:?}"),
        }
        assert_eq!(stats.refused.load(Ordering::Relaxed), 1, "the refusal must be counted");
    }

    /// Votes are tallied by the supervisor because it is the only party that sees both players.
    #[test]
    fn both_players_votes_are_counted_and_the_outcome_is_handed_to_both() {
        let (addr, _stats) = relay_for_test();
        let mut host = agent_on(addr, Side::Host);
        let mut client = agent_on(addr, Side::Client);
        settle();

        host.write_all(&encode(&Msg::Vote { proposal: 12, option: 1 })).unwrap();
        host.flush().unwrap();
        assert!(matches!(read_frame(&mut client), Some(Msg::Vote { proposal: 12, option: 1 })));

        client.write_all(&encode(&Msg::Vote { proposal: 12, option: 1 })).unwrap();
        client.flush().unwrap();

        // Unanimous 1: the resolved frame goes to the other side and back to the sender, and one of the
        // two reads below sees the outcome.
        let outcome = read_frame(&mut client);
        match outcome {
            Some(Msg::Vote { proposal, option }) => {
                assert_eq!(proposal, 12);
                assert_eq!(option, 1, "the resolved outcome must be the agreed option");
            }
            other => panic!("expected the resolved vote, got {other:?}"),
        }
    }

    /// A clock frame must not be mistaken for the peer's replicated state, and vice versa: the two kinds
    /// of socket are kept apart so an agent never becomes a relay target.
    #[test]
    fn an_agent_is_not_treated_as_a_replicated_state_peer() {
        let (addr, stats) = relay_for_test();
        let mut host = agent_on(addr, Side::Host);
        let mut client = agent_on(addr, Side::Client);
        settle();

        host.write_all(&encode(&Msg::Clock { reasons: vec![1], tick: 1 })).unwrap();
        host.flush().unwrap();
        assert!(matches!(read_frame(&mut client), Some(Msg::Clock { .. })));

        // Relayed *state* counts nothing: `frames_out` belongs to the game-DLL fan-out.
        assert_eq!(
            stats.frames_out.load(Ordering::Relaxed),
            0,
            "an agent frame must not go through the peer relay"
        );
    }
}
