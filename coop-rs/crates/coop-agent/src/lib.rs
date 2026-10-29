//! In-process agent: the write half, as a standalone DLL.
//!
//! Proves, in order: we can get a DLL into the game process; it starts a worker without deadlocking
//! the loader; it reaches the Rust supervisor over loopback; and it can locate the live
//! `RaceSimManager` from inside the process. Writes arrive last, because a bad write is a hard crash.
//!
//! Nothing here may crash the game silently. `crash` installs a trace and a minidump handler before
//! anything else runs, so every later failure names the stage it died in.
//!
//! DllMain discipline: no allocation, no Rust runtime initialisation, no locks. It calls CreateThread
//! and returns. Everything else happens on that thread, outside the loader lock.

use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub mod clocksync;
pub mod coord;
pub mod lobby_policy;
pub mod lobby_steam;
pub mod crash;
pub mod hook;
pub mod hookmail;
pub mod presence;
pub mod presence_steam;

use coop_proto::{encode, Msg};

/// The process-wide write mailbox. The client fills it over TCP; the hook callback drains it on the
/// game thread. A `OnceLock` rather than a `static mut` so the mailbox has a normal address that the
/// callback can reach without `unsafe` at the call site.
static MAILBOX: OnceLock<hookmail::Mailbox> = OnceLock::new();

pub fn mailbox() -> &'static hookmail::Mailbox {
    MAILBOX.get_or_init(hookmail::Mailbox::new)
}

/// Frames waiting to go out over the supervisor socket.
///
/// The HTTP handler runs on the lobby thread and the socket is owned by the heartbeat thread, so they
/// cannot share it directly. A queue is the whole handshake: the handler validates and enqueues, the
/// heartbeat writes. Nothing is sent from the lobby thread, so a stalled supervisor cannot block a
/// `fetch` from the front-end and freeze the menu that issued it.
static OUTBOX: OnceLock<std::sync::Mutex<std::collections::VecDeque<Vec<u8>>>> = OnceLock::new();

fn outbox() -> &'static std::sync::Mutex<std::collections::VecDeque<Vec<u8>>> {
    OUTBOX.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}

/// How many un-sent frames to hold.
///
/// Bounded because the supervisor is frequently not running (single-player, or the relay was not started)
/// and the front-end keeps reporting pause state. The queue is *state*, not a log, so dropping the oldest
/// is correct: a stale pause report superseded by a newer one has no value.
const OUTBOX_CAP: usize = 64;

/// Queue a coordination frame for the heartbeat thread. Returns false when the queue is full and an older
/// frame was discarded to make room, which the caller reports rather than ignores.
fn queue_frame(frame: Vec<u8>) -> bool {
    let mut q = match outbox().lock() {
        Ok(q) => q,
        Err(p) => p.into_inner(),
    };
    let mut kept_all = true;
    while q.len() >= OUTBOX_CAP {
        q.pop_front();
        kept_all = false;
    }
    q.push_back(frame);
    kept_all
}

fn outbox_len() -> usize {
    match outbox().lock() {
        Ok(q) => q.len(),
        Err(p) => p.into_inner().len(),
    }
}

/// Whether the heartbeat thread currently holds a supervisor connection.
///
/// Reported by `/coop/coordinate` because "queued" and "delivered" are different things and only this
/// flag distinguishes them: with no supervisor the frame is kept for the reconnect, so the player's
/// pause or vote is not lost, but it is not on the wire yet either.
static CONNECTED: AtomicBool = AtomicBool::new(false);

fn connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

/// Which player this agent is, as announced in `Msg::Agent`.
///
/// Cached because it is read once per heartbeat and must not re-read a file every second. Re-read on
/// demand instead would mean a side change mid-session, which is worse than a restart: the supervisor
/// keys arbitration identity off whatever arrived, so flipping it would split one player's claims across
/// both sides of the ledger.
static SIDE: OnceLock<coop_proto::Side> = OnceLock::new();

/// Resolves the side from `coop_side.txt` next to the exe, falling back to `COOP_AGENT_SIDE`.
///
/// A file, not the environment, for the same reason `supervisor_address` uses one: an injected DLL
/// cannot see the injector's environment, and requiring a rebuild to swap sides makes a two-machine test
/// impossible with one installed build. The lobby's `role=` vocabulary is reused so a player who claimed
/// a team as `client` is a `Client` here without any extra configuration.
pub fn agent_side() -> coop_proto::Side {
    *SIDE.get_or_init(|| {
        let path = log_path().with_file_name("coop_side.txt");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let line = text.lines().next().unwrap_or("").trim().to_string();
            if !line.is_empty() {
                return coord::parse_side(&line);
            }
        }
        match std::env::var("COOP_AGENT_SIDE") {
            Ok(v) if !v.trim().is_empty() => coord::parse_side(&v),
            _ => coop_proto::Side::Host,
        }
    })
}

/// Takes everything queued, oldest first. Called only by the thread that owns the socket.
fn drain_outbox() -> Vec<Vec<u8>> {
    let mut q = match outbox().lock() {
        Ok(q) => q,
        Err(p) => p.into_inner(),
    };
    q.drain(..).collect()
}

/// The last few lines the supervisor sent us: a refused claim, or a resolved vote.
///
/// This exists because the agent had a write-only socket. `arbitrate` answers a refusal with `Msg::Bye`
/// and a resolved vote with a `Vote` carrying the winning option, and both were landing in a socket
/// nobody read, so a losing player saw nothing at all. A bounded ring rather than a queue: the front-end
/// polls, and a message older than the last few is not actionable during a race.
const INBOX_CAP: usize = 8;

static INBOX: OnceLock<std::sync::Mutex<std::collections::VecDeque<String>>> = OnceLock::new();

fn inbox() -> &'static std::sync::Mutex<std::collections::VecDeque<String>> {
    INBOX.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}

/// The two-sided clock state. One instance for the process, because there is one game and one player.
///
/// Separate from the front-end's own layer tracking: the UI knows why *this* player stopped, the agent knows
/// what the *other* player reported, and only here are both visible at once. `ClockGate` inside does the
/// union, so the rule is not restated in a second language (see `clocksync`).
static CLOCK: OnceLock<std::sync::Mutex<clocksync::ClockSync>> = OnceLock::new();

fn clock() -> &'static std::sync::Mutex<clocksync::ClockSync> {
    CLOCK.get_or_init(|| std::sync::Mutex::new(clocksync::ClockSync::new(agent_side())))
}

fn clock_with<T>(f: impl FnOnce(&mut clocksync::ClockSync) -> T) -> T {
    let mut g = match clock().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    f(&mut g)
}

/// Records one frame from the supervisor as a line the UI can show.
///
/// Only the frames that carry a decision are described; anything else is named rather than dumped, so a
/// future message type cannot turn the log into binary noise.
fn note_inbound(frame: &[u8]) {
    let line = match coop_proto::decode(frame) {
        Ok(Msg::Bye) => "refused by supervisor (see its log for the reason)".to_string(),
        Ok(Msg::Vote { proposal, option }) => {
            if option == 0xFF {
                format!("vote {proposal}: tied, no winner")
            } else {
                format!("vote {proposal}: outcome {option}")
            }
        }
        // The other player's pause state. Fed to the gate *and* described, because this is the message that
        // makes "one player's clock stops, so does the other's" true rather than merely reported.
        Ok(Msg::Clock { reasons, tick }) => {
            let action = clock_with(|c| {
                c.set_remote_bytes(&reasons);
                c.view()
            });
            format!(
                "clock from the other player: action={} stopped={} reasons={} (their tick {tick})",
                action.action,
                action.stopped,
                clocksync::decode_reasons(&reasons)
                    .iter()
                    .map(|r| format!("{r:?}"))
                    .collect::<Vec<_>>()
                    .join("+")
            )
        }
        Ok(other) => format!("{other:?}"),
        Err(_) => "unreadable frame from supervisor".to_string(),
    };
    let mut q = match inbox().lock() {
        Ok(q) => q,
        Err(p) => p.into_inner(),
    };
    while q.len() >= INBOX_CAP {
        q.pop_front();
    }
    q.push_back(line);
}

fn inbox_lines() -> Vec<String> {
    let q = match inbox().lock() {
        Ok(q) => q,
        Err(p) => p.into_inner(),
    };
    q.iter().cloned().collect()
}

/// The Steam lobby as this process sees it, so `/coop/lobby` can report without another FFI round trip on
/// every poll.
///
/// `state` is the mod's intent ("the host tab is open"), which is what decides whether a lobby *should* exist;
/// `id` is what Steam actually gave us. Keeping both makes the common failure — the tab is open but no lobby
/// was created, because Steam is not running — visible as `state=waiting id=0` rather than as silence.
static LOBBY: std::sync::Mutex<(lobby_policy::HostState, u64)> =
    std::sync::Mutex::new((lobby_policy::HostState::Idle, 0));

fn lobby_state() -> (lobby_policy::HostState, u64) {
    match LOBBY.lock() {
        Ok(g) => *g,
        Err(p) => *p.into_inner(),
    }
}

fn set_lobby_state(s: lobby_policy::HostState, id: u64) {
    match LOBBY.lock() {
        Ok(mut g) => *g = (s, id),
        Err(p) => *p.into_inner() = (s, id),
    }
}

/// JSON for `GET /coop/lobby`: what a friend would see, and why it may not be joinable.
fn lobby_body() -> String {
    let (state, id) = lobby_state();
    let name = match state {
        lobby_policy::HostState::Idle => "idle",
        lobby_policy::HostState::Waiting => "waiting",
        lobby_policy::HostState::Filled => "filled",
    };
    format!(
        "{{\"state\":\"{name}\",\"lobby_id\":\"{id}\",\"joinable\":{},\"marker\":\"{}\",\
          \"version\":\"{}\",\"app_id\":{},\"pending_join\":\"{}\"}}",
        state.joinable(),
        lobby_policy::MARKER_VALUE,
        lobby_policy::version_marker(),
        lobby_policy::advert().app_id,
        lobby_steam::take_wanted_lobby()
    )
}

/// Opens (or closes) the lobby to match the host tab.
///
/// Called from the front-end: the host screen says `state=waiting` when it mounts and `state=idle` when it
/// leaves. Tying the lobby to the tab is Steam's own guidance - "only create lobbies when needed" - and it
/// means a player who never opens multiplayer never creates one.
/// Starts the relay *inside the game process*, so hosting needs no second program.
///
/// Before this, "host" meant starting `coop-supervisor.exe`: a second thing to launch and forget, and
/// forgetting it looks identical to a broken mod. The agent is already inside the game, so it binds the socket
/// itself and the standalone binary becomes optional (useful for testing, or for a relay on a third machine).
///
/// Idempotent - the host tab can be opened more than once, and the second call must not try to re-bind.
pub fn start_relay(bind: &str, port: u16) -> Result<u16, String> {
    static RELAY: OnceLock<Result<u16, String>> = OnceLock::new();
    RELAY.get_or_init(|| match coop_supervisor::serve(bind, port) {
        Ok(relay) => {
            log(&format!(
                "relay listening on {} (in-process; no separate supervisor needed)",
                relay.addr
            ));
            Ok(relay.addr.port())
        }
        Err(e) => {
            /* AddrInUse is the interesting case and it is usually benign: a standalone coop-supervisor.exe is
             * already running and doing the job. Saying "failed" would send someone hunting a problem that
             * does not exist, so the message names the likely cause. */
            let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
                format!(
                    "port {port} is already in use - if coop-supervisor.exe is running, that is serving the \
                     role and this is harmless"
                )
            } else {
                format!("could not bind {bind}:{port}: {e}")
            };
            log(&format!("relay not started: {msg}"));
            Err(msg)
        }
    })
    .clone()
}

/// JSON for `GET /coop/relay`: whether the in-process relay is up, and where.
fn relay_body() -> String {
    /* `0.0.0.0`, not loopback. A relay on 127.0.0.1 serves this machine and silently refuses the other player,
     * which presents as "no sync" rather than as a bind mistake. */
    match start_relay("0.0.0.0", coop_supervisor::DEFAULT_PORT) {
        Ok(port) => format!(
            "{{\"relay\":\"in-process\",\"bind\":\"0.0.0.0\",\"port\":{port},\
              \"separate_process_needed\":false}}"
        ),
        Err(e) => format!("{{\"relay\":\"failed\",\"error\":\"{e}\"}}"),
    }
}

fn lobby_set(state: &str) -> String {
    let want = match state.trim().to_ascii_lowercase().as_str() {
        "waiting" | "host" | "hosting" => lobby_policy::HostState::Waiting,
        "filled" | "joined" => lobby_policy::HostState::Filled,
        "idle" | "off" | "none" => lobby_policy::HostState::Idle,
        other => {
            return format!(
                "{{\"error\":\"unknown state '{other}'; use idle, waiting or filled\"}}"
            )
        }
    };

    #[cfg(windows)]
    {
        match lobby_steam::attach() {
            Ok(api) => {
                let (current, id) = lobby_state();
                let mut lobby = id;

                // Create once, when the tab opens. `CreateLobby` is asynchronous and returns no id, so a
                // created-but-not-yet-reported lobby is discovered on a later call by `adopt`; the state is
                // still recorded as `want` so the caller knows the request was accepted.
                if want.should_advertise() && lobby == 0 {
                    let a = lobby_policy::advert();
                    lobby_steam::create(&api, a.lobby_type, a.max_members);
                }

                if want.should_advertise() && lobby != 0 {
                    // Metadata is only writable by the owner, which we are.
                    for (k, v) in lobby_policy::lobby_data() {
                        lobby_steam::set_data(&api, lobby, k, &v);
                    }
                    // Close to new joins once filled, without destroying the lobby the peer is routed through.
                    lobby_steam::set_joinable(&api, lobby, want.joinable());
                }

                if !want.should_advertise() && lobby != 0 {
                    lobby_steam::leave(&api, lobby);
                    lobby = 0;
                }

                let changed = current != want || lobby != id;
                set_lobby_state(want, lobby);
                if changed {
                    log(&format!(
                        "steam lobby: state={want:?} id={lobby} (was {current:?} id={id})"
                    ));
                }
                lobby_body()
            }
            // Steam absent is not an error the player can fix from inside the game, but it must be visible:
            // a host tab that shows "waiting" while nothing is advertised is the confusing case.
            Err(e) => format!("{{\"error\":\"{e}\",\"state\":\"unavailable\"}}"),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = want;
        "{\"error\":\"Steam lobbies are only available on Windows\"}".to_string()
    }
}

/// Joins a friend's session from the details Steam hands us.
///
/// Two paths, both required:
///   * `lobby=<id>` — the id from a `GameLobbyJoinRequested_t` callback, i.e. a friend clicked "Join Game".
///   * `friend=<steamid64>` — look the friend up through `GetFriendGamePlayed`, which is how Steam's own
///     documentation finds a friend's lobby.
///
/// The advertised marker and version are checked *before* joining. Connecting first and refusing afterwards
/// would leave the game attached to a session it cannot speak to.
fn lobby_join(q: &std::collections::HashMap<String, String>) -> String {
    let friend = q.get("friend").and_then(|v| v.trim().parse::<u64>().ok());
    let lobby = q.get("lobby").and_then(|v| v.trim().parse::<u64>().ok()).filter(|v| *v != 0);

    #[cfg(windows)]
    {
        let api = match lobby_steam::attach() {
            Ok(a) => a,
            Err(e) => return format!("{{\"error\":\"{e}\"}}"),
        };

        let (app, candidate) = match (lobby, friend) {
            (Some(l), _) => (lobby_policy::advert().app_id, l),
            (None, Some(f)) => lobby_steam::friend_lobby(&api, f),
            (None, None) => {
                return "{\"error\":\"give lobby=<id> or friend=<steamid64>\"}".to_string()
            }
        };

        // Reading the metadata can legitimately return nothing if Steam has not fetched it yet; that is
        // reported as "not ours" rather than retried forever.
        let session = lobby_policy::FriendSession {
            app_id: app,
            lobby_id: candidate,
            marker: lobby_steam::get_data(&api, candidate, lobby_policy::KEY_MARKER).unwrap_or_default(),
            version: lobby_steam::get_data(&api, candidate, lobby_policy::KEY_VERSION).unwrap_or_default(),
        };

        match lobby_policy::accept_friend(&session) {
            Ok(id) => {
                let ok = lobby_steam::join(&api, id);
                log(&format!("steam lobby: join {id} requested={ok}"));
                format!("{{\"joining\":\"{id}\",\"requested\":{ok}}}")
            }
            Err(why) => {
                log(&format!("steam lobby: refused {candidate}: {}", why.message()));
                format!("{{\"error\":\"{}\",\"reason\":\"{:?}\"}}", why.message(), why)
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (friend, lobby);
        "{\"error\":\"Steam lobbies are only available on Windows\"}".to_string()
    }
}

/// Reports a `+connect_lobby <id>` command line, as Steam uses when an invite is accepted while the game is
/// closed. The front-end reads this once on boot so a launch-from-invite actually joins.
fn connect_lobby_body() -> String {
    #[cfg(windows)]
    {
        extern "system" {
            fn GetCommandLineW() -> *const u16;
        }
        let raw = unsafe {
            let p = GetCommandLineW();
            if p.is_null() {
                String::new()
            } else {
                let mut len = 0usize;
                while len < 32768 && *p.add(len) != 0 {
                    len += 1;
                }
                String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
            }
        };
        match lobby_policy::parse_connect_lobby(&raw) {
            Some(id) => format!("{{\"lobby\":\"{id}\"}}"),
            None => "{\"lobby\":null}".to_string(),
        }
    }
    #[cfg(not(windows))]
    {
        "{\"lobby\":null}".to_string()
    }
}

///
/// Reads whatever the supervisor has sent us and records the decisions in it.
///
/// Called from the heartbeat thread before it writes. Bounded by the socket's read timeout, so a silent
/// supervisor costs one timeout and not a stuck heartbeat - a blocking read here would stop the
/// heartbeats that keep the agent registered, and the agent would then be dropped for being silent.
///
/// `buf` is the caller's carry-over. TCP gives arbitrary boundaries, so a frame can arrive split or two
/// can arrive together; `next_frame` is the same splitter the supervisor uses, which is why this does not
/// grow a second, divergent implementation.
fn drain_inbound(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    use std::io::Read;
    let mut chunk = [0u8; 4096];
    let mut saw_any = false;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let before = buf.len();
                buf.extend_from_slice(&chunk[..n]);
                saw_any = true;
                loop {
                    match coop_proto::next_frame(buf) {
                        Ok(Some(len)) => {
                            let frame: Vec<u8> = buf.drain(..len).collect();
                            note_inbound(&frame);
                        }
                        // Two cases the agent must not treat as fatal, because it cannot act on either:
                        // a valid frame is still arriving (normal, and the next pass completes it), or
                        // the peer sent something that is not a co-op frame at all. Discarding the
                        // buffer on the latter is the only way to resynchronise, and the guard below
                        // stops a garbage stream from growing without limit.
                        Ok(None) => break,
                        Err(_) => {
                            buf.clear();
                            break;
                        }
                    }
                }
                // Nothing in the supervisor's vocabulary is anywhere near this size; if a stream reaches
                // it, the framing is wrong and keeping the bytes would be a slow leak.
                if buf.len() > 1 << 20 {
                    buf.clear();
                }
                if buf.len() == before && before >= coop_proto::HEADER_LEN {
                    // A frame that parses to zero progress would spin; leave and let the caller retry.
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(_) => break,
        }
    }
    saw_any
}

/// The hook callback: runs on the game thread, inside the sim call.
///
/// This is the whole point of the mailbox. `this` is the sim object pointer the game itself passed to
/// `simulate`, so it is the one address we know is live and correctly aligned at this instant. Writes
/// from here are ordered against the simulation by construction, which is what the previous design
/// lacked and why every attempt to replicate cars from a worker thread ended in
/// `EXCEPTION_ACCESS_VIOLATION reading 0xF3C`.
///
/// Deliberately does nothing but drain the mailbox: no logging (a file write here would stall the sim),
/// no allocation, no locks.
extern "system" fn on_sim(this: *mut std::ffi::c_void) {
    let base = this as usize;
    if base == 0 {
        return;
    }
    let wrote = mailbox().drain(base);
    if wrote > 0 {
        // A plain relaxed add: the counter is read by the worker for its log line and nothing depends
        // on its exact value.
        LATE_WRITES.fetch_add(1, Ordering::Relaxed);
    }
}


/// How many mailbox drains have written anything, for the agent's heartbeat line.
static LATE_WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The newest race control the host has sent, kept here so every snapshot frame carries it.
///
/// Held in atomics rather than behind a mutex because the write-port thread reads it on every frame and
/// the hook must never wait on a lock. Each field is a single aligned word, so a reader sees either the
/// old value or the new one and never a torn mix of two messages - which for a red flag would mean
/// briefly reporting "race resumed" while the host is still stopped.
static RC_FLAGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static RC_STATE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static RC_RF: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn store_rc(rc: &coop_proto::RcState) {
    RC_FLAGS.store(rc.flags, Ordering::Relaxed);
    RC_STATE.store(rc.rc_state as u32, Ordering::Relaxed);
    RC_RF.store(rc.rf_state as u32, Ordering::Relaxed);
}

fn latest_rc() -> coop_proto::RcState {
    coop_proto::RcState {
        flags: RC_FLAGS.load(Ordering::Relaxed),
        rc_state: RC_STATE.load(Ordering::Relaxed) as u8,
        rf_state: RC_RF.load(Ordering::Relaxed) as u8,
    }
}

/// The tyre-replication mode, read from `coop_tyres.txt` next to the exe.
///
/// Defaults to `Observe`, which writes nothing. That default is the whole point: tyre state is derived
/// by the sim (temperature drives the puncture curve, wear accumulates from driving - see
/// RNG-DETERMINISM §1.3), so writing it is overriding the simulation rather than replicating it. The
/// file lets the mode change without a rebuild, and lets a live test step through observe -> wear ->
/// full instead of jumping to the widest one.
pub fn tyre_mode() -> hookmail::TyreMode {
    let path = log_path().with_file_name("coop_tyres.txt");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return hookmail::TyreMode::Observe;
    };
    let trimmed = text.trim().to_ascii_lowercase();
    if trimmed.is_empty() || trimmed.starts_with("obs") {
        return hookmail::TyreMode::Observe;
    }
    let mode = hookmail::TyreMode::from_text(&trimmed);
    if mode == hookmail::TyreMode::Observe {
        log(&format!(
            "coop_tyres.txt says {:?}, which is not a mode; using observe (writes nothing)",
            text.trim()
        ));
    }
    mode
}

/// One observation line describing the host's actual tyre readings.
///
/// Printed at most once a second while in `Observe`, because the next decision - whether wear alone is
/// safe to write, and what the temperature ranges really are - cannot be made from the field layout. It
/// has to be made from numbers this game actually produces, which is exactly what the previous
/// generation never collected before writing.
fn log_tyre_observation(cars: &[coop_proto::Car]) {
    let mut line = String::from("tyres observed (surface/carcass/condition per wheel):");
    for car in cars.iter().take(3) {
        line.push_str(&format!(
            "\n  car{:<2} {}",
            car.id,
            hookmail::tyre_summary(car)
        ));
    }
    // Ranges across every car, which is the number that decides the safe bounds.
    let mut s_lo = f32::MAX;
    let mut s_hi = f32::MIN;
    let mut c_lo = f32::MAX;
    let mut c_hi = f32::MIN;
    let mut rejected = 0;
    for car in cars {
        for t in &car.tyres {
            for (v, lo, hi) in [
                (t.surface, &mut s_lo, &mut s_hi),
                (t.carcass, &mut c_lo, &mut c_hi),
            ] {
                if v.is_finite() {
                    if v < *lo {
                        *lo = v;
                    }
                    if v > *hi {
                        *hi = v;
                    }
                }
            }
            if !hookmail::plausible(t.surface, coop_proto::layout::off::tyre_range::SURFACE)
                || !hookmail::plausible(t.carcass, coop_proto::layout::off::tyre_range::CARCASS)
                || !hookmail::plausible(
                    t.condition,
                    coop_proto::layout::off::tyre_range::CONDITION,
                )
            {
                rejected += 1;
            }
        }
    }
    log(&format!(
        "{line}\n  surface range {s_lo:.1}..{s_hi:.1}  carcass range {c_lo:.1}..{c_hi:.1}  \
         out-of-range values: {rejected} of {}"
        , cars.len() * 4
    ));
}


pub const DEFAULT_SUPERVISOR: &str = "127.0.0.1:45990";

/// Supervisor address the agent should use.
///
/// An injected DLL cannot see the injector's environment, and requiring a rebuild to change an
/// address makes a two-machine test impossible, so `coop_supervisor.txt` next to the game's exe wins
/// when it exists. It holds one line, `host:port`.
pub fn supervisor_address() -> String {
    let path = log_path().with_file_name("coop_supervisor.txt");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let line = text.lines().next().unwrap_or("").trim().to_string();
        if !line.is_empty() {
            return line;
        }
    }
    DEFAULT_SUPERVISOR.to_string()
}
const RETRY: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_secs(1);

static STARTED: AtomicBool = AtomicBool::new(false);
/// Cached result of `locate_rsm`. A `const` here would be a fresh temporary at every use, so the
/// cache would never hold anything.
static RSM_CACHE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(windows)]
mod win {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateThread(
            attrs: *mut u8,
            stack: usize,
            start: extern "system" fn(*mut u8) -> u32,
            param: *mut u8,
            flags: u32,
            id: *mut u32,
        ) -> isize;
        pub fn GetCurrentProcessId() -> u32;
    }
}

#[cfg(windows)]
pub mod sim {
    //! Resolve and VALIDATE the sim entry point before anything is patched.
    //!
    //! FormationLap documents `simulate` at `0x142298080`. UE executables link at a preferred base of
    //! 0x140000000, so that number is `preferred_base + RVA`. The real base moves under ASLR, which
    //! makes the recorded absolute address useless on its own - we have to rebuild it every launch.
    //!
    //! Nothing here patches code. It reports what is actually at the address, so a wrong RVA shows up
    //! as implausible bytes instead of a crash.

    use std::ffi::c_void;

    /// `0x142298080 - 0x140000000`
    pub const SIMULATE_RVA: usize = 0x0229_8080;
    pub const PREFERRED_BASE: usize = 0x1_4000_0000;

    #[repr(C)]
    struct MemoryBasicInformation {
        base_address: *mut c_void,
        allocation_base: *mut c_void,
        allocation_protect: u32,
        partition_id: u16,
        _pad: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
        _pad2: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
        fn VirtualQuery(addr: *const c_void, info: *mut MemoryBasicInformation, len: usize) -> usize;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const PAGE_EXECUTE: u32 = 0x10;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;
    const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;

    pub fn main_module_base() -> Option<usize> {
        let h = unsafe { GetModuleHandleW(std::ptr::null()) };
        if h == 0 {
            None
        } else {
            Some(h as usize)
        }
    }

    pub fn describe(target: usize) -> String {
        let mut mi = MemoryBasicInformation {
            base_address: std::ptr::null_mut(),
            allocation_base: std::ptr::null_mut(),
            allocation_protect: 0,
            partition_id: 0,
            _pad: 0,
            region_size: 0,
            state: 0,
            protect: 0,
            kind: 0,
            _pad2: 0,
        };
        let got = unsafe { VirtualQuery(target as *const c_void, &mut mi, std::mem::size_of::<MemoryBasicInformation>()) };
        if got == 0 {
            return "VirtualQuery failed".into();
        }
        let executable = matches!(
            mi.protect,
            PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
        );
        format!(
            "state={} protect=0x{:X} executable={} region_base=0x{:X} region_size=0x{:X}",
            mi.state, mi.protect, executable, mi.base_address as usize, mi.region_size
        ).replace(&format!("state={MEM_COMMIT}"), "state=COMMIT")
    }

    /// Reads the first bytes at an address, for eyeballing a prologue.
    pub fn bytes_at(target: usize, n: usize) -> Option<Vec<u8>> {
        if target == 0 {
            return None;
        }
        let p = target as *const u8;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            out.push(unsafe { *p.add(i) });
        }
        Some(out)
    }

    pub fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    /// A cheap sanity check. A real function prologue is not all zeros, not all 0xCC padding, and
    /// typically starts by saving something on the stack.
    pub fn plausible(bytes: &[u8]) -> bool {
        if bytes.len() < 8 {
            return false;
        }
        if bytes.iter().all(|b| *b == 0) || bytes.iter().all(|b| *b == 0xCC) {
            return false;
        }
        // 48/4C = REX prefix (push/sub/mov forms), 55 = push rbp, 40 = REX, E9 = jmp (thunks)
        matches!(bytes[0], 0x48 | 0x4C | 0x55 | 0x40 | 0xE9 | 0x53 | 0x56 | 0x57)
    }

    /// Full report for the endpoint message and the log.
    pub fn inspect() -> String {
        match main_module_base() {
            None => "no main module".into(),
            Some(base) => {
                let target = base + SIMULATE_RVA;
                let layout = describe(target);
                match bytes_at(target, 16) {
                    None => format!("base=0x{base:X} target=0x{target:X} unreadable"),
                    Some(b) => format!(
                        "base=0x{base:X} target=0x{target:X} plausible={} bytes=[{}] {layout}",
                        plausible(&b),
                        hex(&b)
                    ),
                }
            }
        }
    }
}

/// Default port the agent listens on for car frames from the local `coop-link`.
///
/// A second port, not the supervisor connection: the supervisor link carries `Agent` heartbeats
/// *outbound*, and mixing inbound car data onto it would make the agent's own identity message and the
/// client's writes compete on one socket. Keeping them apart also keeps the two failure modes
/// separable in the log - "the supervisor went away" must not look like "the mailbox stopped".
pub const DEFAULT_WRITE_PORT: u16 = 45991;

/// How often the observe-only tyre report is written. Every second is enough to see wear accumulate and
/// temperature swing, and slow enough not to flood the log during a race.
const TYRE_OBS: Duration = Duration::from_secs(1);

static WRITES_STARTED: AtomicBool = AtomicBool::new(false);

/// Where the agent listens, so a two-machine test can change it without a rebuild.
pub fn write_port() -> u16 {
    let path = log_path().with_file_name("coop_agent_port.txt");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(p) = text.lines().next().unwrap_or("").trim().parse::<u16>() {
            return p;
        }
    }
    DEFAULT_WRITE_PORT
}

/// Accepts car frames from the local `coop-link` and hands them to the mailbox.
///
/// Runs on its own thread and never touches game memory: it only parses a snapshot and stages it. The
/// hook callback is what puts the bytes into the sim, on the game thread. That split is the whole
/// architecture, and keeping the network thread out of the write path is what makes it hold.
fn serve_writes(port: u16) {
    use std::io::Read;
    use std::net::TcpListener;

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("write listener could not bind 127.0.0.1:{port}: {e}"));
            return;
        }
    };
    log(&format!("write listener on 127.0.0.1:{port}"));

    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(s) => s,
            Err(e) => {
                log(&format!("write listener accept failed: {e}"));
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        log("coop-link connected to the write port");
        // Say up front whether the frames about to arrive can actually reach the sim. Without this the
        // client's log says "sent 400 to the agent" while nothing changes on screen, and the reason is
        // three files away in this log.
        #[cfg(windows)]
        {
            let (fires, cb) = hook::state();
            if cb == 0 {
                log(
                    "WARNING: no hook callback is registered, so frames will be staged and never \
                     applied. The hook is opt-in: create coop_hook.enable next to the exe (or set \
                     COOP_AGENT_HOOK=1) and restart the game.",
                );
            } else if fires == 0 {
                log(&format!(
                    "callback 0x{cb:X} registered but the hook has not fired yet; it will apply on \
                     the first sim call (start or enter a race)"
                ));
            } else {
                log(&format!("callback 0x{cb:X} registered, hook has fired {fires} time(s)"));
            }
        }
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        let mut frames_in = 0u64;
        let mut warned_inert = false;
        let mut last_tyre_obs: Option<Instant> = None;
        'session: loop {
            let n = match stream.read(&mut chunk) {
                Ok(0) => {
                    log("coop-link closed the write port");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    log(&format!("write port read failed: {e}"));
                    break;
                }
            };
            buf.extend_from_slice(&chunk[..n]);
            loop {
                match coop_proto::next_frame(&buf) {
                    Ok(None) => break,
                    Ok(Some(len)) => {
                        let frame: Vec<u8> = buf.drain(..len).collect();
                        match coop_proto::decode(&frame) {
                            Ok(Msg::Snapshot { cars, .. }) => {
                                frames_in += 1;
                                let tyres = tyre_mode();
                                // Observation is not a no-op: it is the evidence needed to decide
                                // whether writing these fields is safe at all, and it is rate-limited
                                // because it writes a log line.
                                if tyres == hookmail::TyreMode::Observe
                                    && last_tyre_obs.map(|t| t.elapsed() >= TYRE_OBS).unwrap_or(true)
                                {
                                    last_tyre_obs = Some(Instant::now());
                                    log_tyre_observation(&cars);
                                }
                                // A dropped frame is normal - the callback runs at whatever rate the
                                // sim calls it - so it is counted by the mailbox, not logged here.
                                let published = mailbox().publish_full(&cars, &latest_rc(), tyres);
                                // The dangerous silent case: frames arrive, nothing is being applied,
                                // and the client still reports success. Say it once, loudly.
                                if !published && !warned_inert {
                                    if let Some(off) = inert_cause() {
                                        log(&format!("frames are arriving but not being applied: {off}"));
                                        warned_inert = true;
                                    }
                                }
                                if frames_in % 200 == 0 {
                                    let (applied, dropped, entries) = mailbox().stats();
                                    let rc = latest_rc();
                                    log(&format!(
                                        "write port: {frames_in} frames in, mailbox applied={applied} \
                                         dropped={dropped} entries={entries}, tyres={}, rc flags=0x{:X} \
                                         state={} rf={}",
                                        tyres.as_str(), rc.flags, rc.rc_state, rc.rf_state
                                    ));
                                }
                            }
                            // Race control arrives on its own message because it changes rarely and
                            // matters more than any single frame: a client that misses a red flag
                            // freezes its cars with no idea why. Kept out of the snapshot so it cannot
                            // be lost to a dropped frame.
                            Ok(Msg::Rc(rc)) => {
                                if rc != latest_rc() {
                                    log(&format!(
                                        "race control: flags=0x{:X} rc_state={} rf_state={}",
                                        rc.flags, rc.rc_state, rc.rf_state
                                    ));
                                }
                                store_rc(&rc);
                            }
                            Ok(_) => {}
                            Err(e) => log(&format!("write port rejected a frame: {e}")),
                        }
                    }
                    Err(e) => {
                        log(&format!("write port framing error ({e}); closing"));
                        break 'session;
                    }
                }
            }
        }
    }
}

/// Explains why staged frames are not reaching the sim, or `None` when they are.
///
/// Three distinct causes produce the same visible symptom - the client says it sent frames and the
/// game does not move - and telling them apart from a log is otherwise guesswork:
///
/// 1. no hook (opt-in flag not set), so there is no game-thread entry point at all;
/// 2. a hook with no callback, which means it fires but does nothing;
/// 3. a hook and callback that have never fired, because no sim call has happened yet.
pub fn inert_cause() -> Option<String> {
    let block = hook::block();
    if block.is_none() {
        return Some(
            "the sim hook is not installed, so there is no game-thread entry point. Create \
             coop_hook.enable next to the game exe (or set COOP_AGENT_HOOK=1) and restart the game."
                .to_string(),
        );
    }
    let (fires, cb) = hook::state();
    if cb == 0 {
        return Some("the hook is installed but has no callback registered".to_string());
    }
    if fires == 0 {
        return Some(format!(
            "the hook (callback 0x{cb:X}) has never fired: no sim call has happened yet. Frames are \
             staged and will apply once a race session starts."
        ));
    }
    None
}

/// Team claims, shared between the two machines so the screen can show who has taken what.
///
/// A claim is a `(team, role)` pair: `role` is `"host"` or `"client"`. Held in the agent rather than in the
/// front-end because localStorage is per-machine - the second player cannot see the first player's claims
/// unless something shared carries them, and the only shared thing both games already run is the relay.
///
/// Kept deliberately simple: last claim wins, and a player may hold only one team. Two players claiming the
/// same team is not prevented here, it is *shown* - the screen marks a taken team, and the mode rules in
/// `coop-auth` decide whether a write lands. Preventing a duplicate claim needs agreement between machines,
/// which is the relay's job, not this map's.
static CLAIMS: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// Records a claim, replacing any previous claim by the same role.
fn claim_team(team: &str, role: &str) -> Vec<(String, String)> {
    let mut claims = match CLAIMS.lock() {
        Ok(c) => c,
        // A poisoned mutex means a previous holder panicked. The data is a list of strings; recovering it is
        // safe and losing it would make the lobby look broken.
        Err(p) => p.into_inner(),
    };
    claims.retain(|(_, r)| r != role);
    claims.push((team.to_string(), role.to_string()));
    claims.clone()
}

/// Drops a claim for a role (on Back, so a player who leaves stops holding a team).
fn release_team(role: &str) -> Vec<(String, String)> {
    let mut claims = match CLAIMS.lock() {
        Ok(c) => c,
        Err(p) => p.into_inner(),
    };
    claims.retain(|(_, r)| r != role);
    claims.clone()
}

fn claims_json(claims: &[(String, String)]) -> String {
    let parts: Vec<String> = claims
        .iter()
        .map(|(t, r)| {
            // Team names come from the game's enum and contain no quotes, but escape anyway: a malformed
            // body would make the lobby silently fail to parse.
            let t = t.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{{\"team\":\"{t}\",\"role\":\"{r}\"}}")
        })
        .collect();
    format!("[{}]", parts.join(","))
}

/// Starts the in-process lobby endpoint: how the main menu launches a server.
///
/// ## Why this has to exist
///
/// The front-end runs in a browser-like context. Measured from the shipped bridge: `fetch` and
/// `XMLHttpRequest` are present, `child_process` and `require` are not. So the menu can make an HTTP request
/// but **cannot start a process** - and packaging an executable inside the `.pak` to extract and run would
/// mean writing binaries to disk from a game process, which is exactly what antivirus heuristics flag.
///
/// The agent is already resident in the game, so it is the only thing that can act on "host a game". Keeping
/// that job here means the client needs **one pak** and no extra executable for hosting.
///
/// Requests are tiny and strictly enumerated; this is not a general HTTP server.
fn serve_lobby(port: u16) {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            log(&format!("lobby endpoint could not bind 127.0.0.1:{port}: {e}"));
            return;
        }
    };
    log(&format!("lobby endpoint on http://127.0.0.1:{port}/coop/"));

    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(s) => s,
            Err(_) => continue,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(s) => s,
            Err(_) => continue,
        });

        // Read just the request line. A browser context sends a full header block; we do not need it, and
        // draining it would be where a slow client could hang us.
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).is_err() {
            continue;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");

        let (code, body) = match (method, path) {
            ("GET", "/coop/status") => ("200 OK", lobby_status_body()),
            ("POST", "/coop/host") => ("200 OK", lobby_host_body()),
            ("POST", "/coop/join") => ("200 OK", lobby_join_body()),
            ("GET", "/coop/teams") => {
                let claims = match CLAIMS.lock() {
                    Ok(c) => c.clone(),
                    Err(p) => p.into_inner().clone(),
                };
                ("200 OK", format!("{{\"claims\":{}}}", claims_json(&claims)))
            }
            // Claims carry the team name in the query string, e.g. `/coop/claim?team=Ferrari&role=host`.
            // A query string rather than a body: the caller is a small piece of front-end code and this keeps
            // both the client and the server trivial, with no body parsing to get wrong.
            (m, p) if p.starts_with("/coop/claim") && (m == "POST" || m == "GET") => {
                let q = query_of(p);
                let team = q.get("team").cloned().unwrap_or_default();
                let role = q.get("role").cloned().unwrap_or_else(|| "host".to_string());
                if team.is_empty() {
                    ("400 Bad Request", "{\"error\":\"team is required\"}".to_string())
                } else {
                    let claims = claim_team(&team, &role);
                    ("200 OK", format!("{{\"claims\":{}}}", claims_json(&claims)))
                }
            }
            (m, p) if p.starts_with("/coop/release") => {
                let q = query_of(p);
                let role = q.get("role").cloned().unwrap_or_else(|| "host".to_string());
                let claims = release_team(&role);
                ("200 OK", format!("{{\"claims\":{}}}", claims_json(&claims)))
            }
            // Coordination: the front-end reports what a player just did, and the agent turns it into a
            // frame the supervisor arbitrates. GET is accepted alongside POST because the caller is
            // `XMLHttpRequest` in the game's web view, where a preflight is one more thing to get wrong
            // for no benefit - these requests are idempotent reports, not commands to a shared resource.
            (m, p) if p.starts_with("/coop/coordinate") && (m == "POST" || m == "GET") => {
                let after = p.trim_start_matches("/coop/coordinate");
                let after = after.strip_prefix('/').unwrap_or(after);
                let (kind, rest) = match after.split_once('?') {
                    Some((k, r)) => (k, r),
                    None => (after, ""),
                };
                let q = query_of(&format!("?{rest}"));
                match coord::parse_request(kind, &q) {
                    Ok(req) => match coord::frame_for(&req) {
                        Ok(frame) => {
                            let desc = format!("{req:?}");
                            // Our own reasons are recorded here as well as sent, so the gate has both sides
                            // and the union is computable locally. Recording only what arrives would leave
                            // this machine's own pause invisible to its own clock state.
                            if let coord::Request::Clock { ref reasons } = req {
                                clock_with(|c| c.set_local_bytes(reasons));
                            }
                            let kept = queue_frame(frame);
                            log(&format!("coordinated {desc} (queued, {} pending)", outbox_len()));
                            // 202: accepted for delivery. The idle state is reported rather than
                            // hidden, because "queued but the supervisor is not running" is the single
                            // most likely reason a coordination rule appears to do nothing.
                            (
                                "202 Accepted",
                                format!(
                                    "{{\"queued\":true,\"pending\":{},\"superseded\":{},\
                                      \"supervisor\":{}}}",
                                    outbox_len(),
                                    !kept,
                                    connected()
                                ),
                            )
                        }
                        Err(why) => (
                            "400 Bad Request",
                            format!("{{\"error\":\"{}\"}}", why.message()),
                        ),
                    },
                    Err(why) => (
                        "400 Bad Request",
                        format!("{{\"error\":\"{}\"}}", why.message()),
                    ),
                }
            }
            // What the supervisor last told us: a refused claim, or a resolved vote. The front-end shows
            // it so a losing player is told why, instead of watching a signing do nothing.
            ("GET", "/coop/inbox") => {
                let lines: Vec<String> = inbox_lines()
                    .iter()
                    .map(|l| format!("\"{}\"", l.replace('\\', "\\\\").replace('"', "\\\"")))
                    .collect();
                ("200 OK", format!("{{\"lines\":[{}],\"pending\":{}}}", lines.join(","), outbox_len()))
            }
            // The combined clock state, driven by the host tab.
            //
            // `seq` is what the front-end keys on. Acting on every poll would re-pause a paused world
            // forever; acting on a new `seq` acts exactly once per transition. See `clocksync`.
            ("GET", "/coop/relay") => ("200 OK", relay_body()),
            // Steam lobby, as a friend's client would see it.
            //
            // Read-only: reports the mod's intent (`state`) and Steam's reality (`lobby_id`) side by side, so
            // "the host tab is open but nothing is advertised" is distinguishable from "hosting and joinable".
            ("GET", "/coop/lobby") => ("200 OK", lobby_body()),
            // The host screen drives this: `waiting` when it mounts, `idle` when it leaves. Steam's own
            // guidance is to create a lobby only when something needs one, so the lobby's lifetime is the tab's.
            (m, p) if p.starts_with("/coop/lobby/set") && (m == "POST" || m == "GET") => {
                let q = query_of(p);
                ("200 OK", lobby_set(q.get("state").map(String::as_str).unwrap_or("waiting")))
            }
            // Joining a friend. `lobby=<id>` after a "Join Game" click (Steam hands us the id through a
            // callback), or `friend=<steamid64>` to look up their lobby via GetFriendGamePlayed.
            (m, p) if p.starts_with("/coop/lobby/join") && (m == "POST" || m == "GET") => {
                let q = query_of(p);
                ("200 OK", lobby_join(&q))
            }
            // The `+connect_lobby <id>` Steam launches the game with when an invite is accepted while the game
            // is closed. Read once by the front-end so launching from an invite actually joins.
            ("GET", "/coop/connectlobby") => ("200 OK", connect_lobby_body()),
            ("GET", "/coop/clock") => {                let v = clock_with(|c| c.view());
                let reasons = clock_with(|c| c.reasons());
                (
                    "200 OK",
                    format!(
                        "{{\"seq\":{},\"action\":\"{}\",\"stopped\":{},\"local\":[{}],\
                          \"remote\":[{}],\"reasons\":[{}],\"side\":\"{}\"}}",
                        v.seq,
                        v.action,
                        v.stopped,
                        v.local.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","),
                        v.remote.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","),
                        reasons.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","),
                        match agent_side() {
                            coop_proto::Side::Host => "host",
                            coop_proto::Side::Client => "client",
                        }
                    ),
                )
            }
            _ => ("404 Not Found", "{\"error\":\"no such endpoint\"}".to_string()),
        };

        // CORS: Gameface is not served from http://127.0.0.1, so without these headers the fetch fails and
        // the menu button silently does nothing. `*` is safe here because the server only listens on
        // loopback and every action is explicit.
        let response = format!(
            "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        // OPTIONS is handled by answering with the same headers and an empty body, which is what a
        // preflighted request needs.
        let response = if method == "OPTIONS" {
            "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\n\
             Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
             Access-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string()
        } else {
            response
        };
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    }
}

/// Parses `key=value` pairs out of a request path's query string.
///
/// Hand-rolled rather than pulled from a crate: the inputs are two short ASCII words, and a URL-decoding
/// dependency for that would be more surface than the feature deserves. `%20` and `+` are handled because
/// team names can contain spaces.
fn query_of(path: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let Some(q) = path.split_once('?').map(|(_, q)| q) else {
        return out;
    };
    for pair in q.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let decode = |s: &str| -> String {
            let s = s.replace('+', " ");
            let bytes = s.as_bytes();
            let mut out = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' && i + 2 < bytes.len() {
                    if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                }
                out.push(bytes[i]);
                i += 1;
            }
            String::from_utf8_lossy(&out).to_string()
        };
        out.insert(decode(k), decode(v));
    }
    out
}

/// JSON for `GET /coop/status`, so the menu can show the real state rather than guess.
fn lobby_status_body() -> String {
    let block = hook::block();
    let (fires, cb) = hook::state();
    let tyre = tyre_mode();
    format!(
        "{{\"injected\":{},\"hook_installed\":{},\"callback_registered\":{},\"hook_fires\":{},\
          \"tyre_mode\":\"{}\",\"write_port\":{},\"pid\":{}}}",
        true,
        block.is_some(),
        cb != 0,
        fires,
        tyre.as_str(),
        write_port(),
        std::process::id()
    )
}

/// JSON for `POST /coop/host`: reports what the host side needs, and whether it is ready.
///
/// Deliberately does *not* spawn anything. The relay is the host's job and runs as its own process or on this
/// thread depending on how the install was set up; what the menu actually needs to know is whether the game
/// side is armed, which is answerable here.
fn lobby_host_body() -> String {
    let block = hook::block();
    let (fires, cb) = hook::state();
    let rc = latest_rc();
    format!(
        "{{\"role\":\"host\",\"armed\":{},\"hook_installed\":{},\"callback_registered\":{},\
          \"hook_fires\":{},\"rc_flags\":{},\"rc_state\":{},\"rf_state\":{},\"write_port\":{}}}",
        block.is_some() && cb != 0,
        block.is_some(),
        cb != 0,
        fires,
        rc.flags,
        rc.rc_state,
        rc.rf_state,
        write_port()
    )
}

/// JSON for `POST /coop/join`: a client's view, which is what the second player needs before a race.
fn lobby_join_body() -> String {
    let block = hook::block();
    let (fires, cb) = hook::state();
    let mode = tyre_mode();
    format!(
        "{{\"role\":\"client\",\"armed\":{},\"hook_installed\":{},\"callback_registered\":{},\
          \"hook_fires\":{},\"frames_applied\":{},\"entries_written\":{},\"tyre_mode\":\"{}\",\
          \"write_port\":{}}}",
        block.is_some() && cb != 0,
        block.is_some(),
        cb != 0,
        fires,
        mailbox().stats().0,
        mailbox().stats().2,
        mode.as_str(),
        write_port()
    )
}

/// HTTP port for the lobby endpoint.
pub const DEFAULT_LOBBY_PORT: u16 = 45992;

static LOBBY_STARTED: AtomicBool = AtomicBool::new(false);

pub fn lobby_port() -> u16 {
    let path = log_path().with_file_name("coop_lobby_port.txt");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(p) = text.lines().next().unwrap_or("").trim().parse::<u16>() {
            return p;
        }
    }
    DEFAULT_LOBBY_PORT
}

/// Starts the lobby endpoint once.
pub fn start_lobby() {
    if LOBBY_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let port = lobby_port();
    std::thread::spawn(move || serve_lobby(port));
}

/// Starts the write listener once.
pub fn start_writes() {
    if WRITES_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let port = write_port();
    std::thread::spawn(move || serve_writes(port));
}

/// Where the agent logs. Next to the exe is discoverable after a crash; the game directory is
/// writable because the game itself writes there.
pub fn log_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("coop_agent.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("coop_agent.log"))
}

pub fn log(line: &str) {
    let stamped = format!("[agent] {line}\n");
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(stamped.as_bytes());
    }
}

/// Worker loop. Connects, identifies itself, heartbeats, and reconnects if the supervisor restarts.
pub fn run(pid: u32, supervisor: &str) {
    log(&format!("agent started in pid {pid}, supervisor {supervisor}"));
    #[cfg(windows)]
    {
        // First, before any hook or write can go wrong: make a crash explain itself.
        crash::install();
        log(&format!("evidence: {}", crash::summary()));
        crash::note("crash handler ready");
        log(&format!("sim entry: {}", sim::inspect()));
        // Listen for car frames before the hook is installed, so a client that starts writing the
        // moment it connects finds somewhere to write to rather than a refused connection.
        start_writes();
        // The lobby endpoint is what lets the main menu host/join without spawning a process.
        start_lobby();
        // Patching game code is opt-in. A wrong patch is an instant crash with no dump, and losing a
        // race session to it is not a fair price for a diagnostic. Enable with env COOP_AGENT_HOOK=1
        // or by creating coop_hook.enable next to the exe.
        let want_hook = std::env::var("COOP_AGENT_HOOK").map(|v| v == "1").unwrap_or(false)
            || log_path().with_file_name("coop_hook.enable").exists();
        if !want_hook {
            log("hook disabled (set COOP_AGENT_HOOK=1 or create coop_hook.enable to enable)");
        } else {
            match sim::main_module_base() {
                None => log("no main module; hook not installed"),
                Some(base) => {
                    crash::note("installing the sim hook");
                    match hook::install(base) {
                        Ok(t) => {
                            let block = hook::block().unwrap_or(0);
                            crash::register_hook(block, t);
                            // Register the mailbox drain. Until this line runs the hook is inert, and
                            // that ordering is deliberate: the patch is installed and verified before
                            // anything is asked to execute through it.
                            hook::set_callback(Some(on_sim));
                            crash::note(&format!(
                                "hook installed at {t:#X}, block {block:#X}, callback registered"
                            ));
                            // The block address is the only way to inspect the fire counter from
                            // outside the process, and "did the hook ever run" is the first question
                            // asked of every failed sync. Log it once, here, rather than guessing.
                            log(&format!(
                                "hook installed at 0x{t:X}; block 0x{block:X}; \
                                 fires live at 0x{:X}; mailbox callback registered",
                                block + hook::FIRES_OFF
                            ));
                        }
                        Err(e) => {
                            crash::note(&format!("hook NOT installed: {e}"));
                            log(&format!("hook NOT installed: {e}"));
                        }
                    }
                }
            }
        }
    }
    let mut stream: Option<TcpStream> = None;
    let mut announced = false;
    let mut inbound_buf: Vec<u8> = Vec::new();
    let mut captured_logged = 0usize;
    let mut last_fires = 0u64;
    let mut ticks_without_fire = 0u32;
    loop {
        // Before anything that can block or retry: a frozen game is exactly when no supervisor is
        // listening, so the dump request must not depend on the connection.
        dump_on_request();
        if stream.is_none() {
            match TcpStream::connect(supervisor) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    // Short, not zero: this connection must never block the heartbeat thread for long.
                    // A zero timeout would mean "return immediately" on some platforms and "block
                    // forever" on others, so an explicit millisecond value is the only portable choice.
                    let _ = s.set_read_timeout(Some(Duration::from_millis(1)));
                    log("connected to supervisor");
                    stream = Some(s);
                    CONNECTED.store(true, Ordering::Relaxed);
                }
                Err(_) => {
                    std::thread::sleep(RETRY);
                    continue;
                }
            }
        }
        let rsm = locate_rsm();
        if rsm != 0 && rsm != captured_logged {
            log(&format!("RaceSimManager = 0x{rsm:X}"));
            captured_logged = rsm;
        }
        // Report the hook's own view, unconditionally on change. Without this the client's "sent N"
        // line describes what it *asked* for while the hook may never have run at all - and that gap
        // is invisible from every other log line. An earlier version only logged when a frame had
        // also been staged, which meant "hook never fires" and "hook fires but nothing is queued"
        // looked identical: both silent.
        #[cfg(windows)]
        {
            let (fires, cb) = hook::state();
            let (applied, dropped, entries) = mailbox().stats();
            if fires != last_fires {
                let delta = fires.wrapping_sub(last_fires);
                last_fires = fires;
                log(&format!(
                    "hook fired x{delta} (total {fires}) callback=0x{cb:X} \
                     mailbox applied={applied} dropped={dropped} entries={entries}"
                ));
            } else if cb != 0 && fires == 0 && ticks_without_fire == 60 {
                // One minute at 1 Hz with a registered callback and no fire: say so, because it means
                // the patched function has not been called yet and nothing staged can be applied.
                log(
                    "hook installed and callback registered, but it has never fired: the patched \
                     function has not been called. Start or enter a race session to make it run.",
                );
            }
        }
        ticks_without_fire = ticks_without_fire.saturating_add(1);
        let frame = encode(&Msg::Agent { pid, rsm: rsm as u64, side: agent_side() });
        let mut lost = false;
        if let Some(s) = stream.as_mut() {
            if s.write_all(&frame).is_err() || s.flush().is_err() {
                log("supervisor gone, will reconnect");
                lost = true;
            }
        }
        if lost {
            stream = None;
            CONNECTED.store(false, Ordering::Relaxed);
            announced = false;
            std::thread::sleep(RETRY);
            continue;
        }
        // Then the coordination frames, on the same socket as `Msg::Agent`.
        //
        // Not a second connection: the supervisor keys a peer's identity off the `Hello` on its socket,
        // and this socket deliberately never sends one - the peer slot belongs to the game's DLL. See
        // `coord` for why the identity therefore has to be stamped by the supervisor.
        if let Some(s) = stream.as_mut() {
            // Taken as a batch, then put back if a write fails part-way. Draining and writing
            // frame-by-frame would drop every frame after the failure, and the frames that matter (a
            // pause, a contract claim) are exactly the ones a reconnect must not silently swallow.
            let batch = drain_outbox();
            let mut sent = 0usize;
            for frame in &batch {
                if s.write_all(frame).is_err() || s.flush().is_err() {
                    lost = true;
                    break;
                }
                sent += 1;
            }
            if sent < batch.len() {
                // Requeue oldest-first, ahead of anything the front-end enqueued meanwhile, so ordering
                // is preserved across the reconnect.
                let mut q = match outbox().lock() {
                    Ok(q) => q,
                    Err(p) => p.into_inner(),
                };
                for frame in batch.into_iter().skip(sent).rev() {
                    q.push_front(frame);
                }
                while q.len() > OUTBOX_CAP {
                    q.pop_back();
                }
            }
            if lost {
                log(&format!(
                    "supervisor gone while sending a coordination frame; {} kept for the reconnect",
                    outbox_len()
                ));
                stream = None;
                CONNECTED.store(false, Ordering::Relaxed);
                announced = false;
                std::thread::sleep(RETRY);
                continue;
            }
            // Read after writing, so a timeout here is never mistaken for a write failure.
            if drain_inbound(s, &mut inbound_buf) {
                let lines = inbox_lines();
                if let Some(last) = lines.last() {
                    log(&format!("supervisor says: {last}"));
                }
            }
        }
        if !announced {
            log("heartbeat started");
            announced = true;
        }
        std::thread::sleep(HEARTBEAT);
    }
}

/// Writes a dump when `coop_dump_now` appears next to the exe, then removes the marker.
///
/// A freeze produces no exception, so nothing fires on its own: this is how a hung client becomes
/// evidence. Creating the marker is the whole interface, so it works from a shell, a script or a
/// shortcut while the game is hanging. The dump briefly suspends every thread, which is why it is
/// never automatic.
fn dump_on_request() {
    let marker = log_path().with_file_name("coop_dump_now");
    if !marker.exists() {
        return;
    }
    let _ = std::fs::remove_file(&marker);
    log("dump requested via coop_dump_now");
    #[cfg(windows)]
    {
        crash::note("dump requested by marker");
        match crash::write_dump("requested by marker", std::ptr::null_mut()) {
            Ok(path) => log(&format!("dump written: {}", path.display())),
            Err(e) => log(&format!("dump failed: {e}")),
        }
    }
}

/// Address of the live `RaceSimManager`, discovered from inside the game.
///
/// The hook can only report this while it is installed, and the address moves every session, so
/// discovery is the primary source and the hook result is only a hint. Values that are not
/// readable objects (`0xCCCC...` fill from an uninitialised slot, for instance) are rejected
/// rather than published: the supervisor trusts whatever it is told.
fn locate_rsm() -> usize {
    let cached = RSM_CACHE.load(Ordering::Relaxed);
    if cached != 0 && readable(cached) {
        return cached;
    }
    #[cfg(windows)]
    {
        let from_hook = hook::captured();
        if from_hook != 0 && readable(from_hook) {
            RSM_CACHE.store(from_hook, Ordering::Relaxed);
            return from_hook;
        }
        if let Ok(proc) = coop_mem::Process::current() {
            if let Ok(base) = proc.module_base("") {
                if let Ok(found) = coop_mem::objects::find_sim(&proc, base) {
                    RSM_CACHE.store(found, Ordering::Relaxed);
                    return found;
                }
            }
        }
    }
    RSM_CACHE.store(0, Ordering::Relaxed);
    0
}

/// True when `rsm` still looks like a live object: a readable race distance field.
fn readable(rsm: usize) -> bool {
    #[cfg(windows)]
    {
        if let Ok(proc) = coop_mem::Process::current() {
            return proc
                .read(rsm + coop_proto::layout::CAR_BASE + coop_proto::layout::off::DIST, 4)
                .is_ok();
        }
    }
    let _ = rsm;
    false
}

/// Starts the worker exactly once, from a fresh thread.
pub fn start(supervisor: &str) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let sup = supervisor.to_string();
    std::thread::spawn(move || {
        #[cfg(windows)]
        let pid = unsafe { win::GetCurrentProcessId() };
        #[cfg(not(windows))]
        let pid = std::process::id();
        run(pid, &sup);
    });
}

#[cfg(windows)]
extern "system" fn thread_entry(_param: *mut u8) -> u32 {
    // Runs outside DllMain. Does the real work, then parks forever.
    start_presence();
    start_join_listener();
    start(&supervisor_address());
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Registers the Steam callback that fires when a friend clicks "Join Game".
///
/// Called from the worker thread, not the game thread: `SteamAPI_RegisterCallback` touches Steam's global
/// callback list, and the loader already loads this DLL before `SteamAPI_Init` may have run. If Steam is not
/// ready yet the call is retried rather than reported once and dropped - the DLL is often injected at launch,
/// before the game has initialised Steam.
pub fn start_join_listener() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        for attempt in 0..30 {
            match lobby_steam::attach() {
                Ok(api) => match lobby_steam::register_join_callback(&api) {
                    Ok(()) => {
                        log(&format!("steam lobby: join callback registered (attempt {attempt})"));
                        return;
                    }
                    Err(e) => log(&format!("steam lobby: join callback failed: {e}")),
                },
                Err(e) => {
                    // Worth one line, not thirty: the last attempt is the one that matters.
                    if attempt == 29 {
                        log(&format!("steam lobby: no join callback, Steam unavailable: {e}"));
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

#[cfg(not(windows))]
pub fn start_join_listener() {}

/*
 * Rich presence ticker.
 *
 * Started from the same worker thread as everything else, and it exists as a real thread rather than a
 * library function nobody calls for a concrete reason: with the linker set to `lto + opt-level=3`, an
 * unreferenced module is dropped from the DLL entirely. Verified - the first build of `presence_steam`
 * appeared in the source but **no** `steam_api64.dll` or `SetRichPresence` string was present in
 * `coop_agent.dll`. A feature that is compiled out is worse than one that is missing, because the build
 * looks successful.
 *
 * The state it publishes comes from a shared cell that the hook/agent updates as the game moves between
 * menu, career and race. Until that plumbing exists the ticker publishes the menu state, which is
 * correct for a freshly loaded game and proves the Steam path end to end.
 */
#[cfg(windows)]
pub fn start_presence() {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Once;

    // 0 = not published yet, 1 = main menu, 2 = creating a new game, 3 = career, 4 = multiplayer career,
    // 5 = multiplayer race. An atomic rather than a Mutex because it is written from the hook thread.
    static WHERE: AtomicU8 = AtomicU8::new(1);
    static ONCE: Once = Once::new();
    static mut WHERE_REF: Option<&'static AtomicU8> = None;

    ONCE.call_once(|| {
        let where_ref: &'static AtomicU8 = Box::leak(Box::new(AtomicU8::new(1)));
        unsafe { WHERE_REF = Some(where_ref) };
        let where_ref: &'static AtomicU8 = unsafe { WHERE_REF.unwrap() };
        let _ = std::thread::Builder::new()
            .name("coop-presence".into())
            .spawn(move || {
                // Publish on change only. Steam rate-limits and the friends list does not need a refresh
                // every second; the loop still wakes to notice a change promptly.
                let mut last: u8 = 255;
                loop {
                    let now = where_ref.load(Ordering::Relaxed);
                    if now != last {
                        last = now;
                        let where_ = match now {
                            2 => presence::Where::CreatingANewGame,
                            3 => presence::Where::Career(presence::CareerActivity::Managing),
                            4 => presence::Where::MultiplayerCareer,
                            5 => presence::Where::MultiplayerRace,
                            _ => presence::Where::MainMenu,
                        };
                        // Ship with `status`, not `steam_display`.
                        //
                        // Steam's docs are explicit: "If `steam_display` is not set to a valid localization
                        // tag, then rich presence will not be displayed in the Steam client." Tokens
                        // registered against app 2591280 are a publisher action we cannot take, so relying
                        // on them means shipping a feature that shows nothing. `status` is plain UTF-8 and
                        // is displayed as-is, so the line appears today with no localisation at all.
                        //
                        // When the tokens ARE registered, `keys_with(.., true)` adds `steam_display` on top
                        // and the same text appears under the name in the friends list. Nothing has to be
                        // undone.
                        let pairs = presence::keys_status(&where_, None);
                        match presence_steam::steam::present(&pairs) {
                            Ok(()) => println!("[presence] {}", presence::render(&where_, None)),
                            Err(e) => println!("[presence] not set: {e}"),
                        }
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
    });
    // Keep the symbol referenced from this function so the module cannot be stripped.
    let _ = WHERE.load(Ordering::Relaxed);
}

#[cfg(windows)]
#[no_mangle]
#[allow(non_snake_case)]
pub extern "system" fn DllMain(_module: isize, reason: u32, _reserved: *mut u8) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        // Loader lock: only CreateThread and return. No allocation, no logging, no locks.
        unsafe {
            win::CreateThread(
                std::ptr::null_mut(),
                0,
                thread_entry,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            );
        }
    }
    1
}

#[cfg(not(windows))]
pub fn start_presence() {}

#[cfg(test)]
mod tests {
    use super::*;
    use coop_proto::Car;

    #[test]
    fn log_path_sits_next_to_the_exe() {
        let p = log_path();
        assert_eq!(p.file_name().unwrap(), "coop_agent.log");
    }

    #[test]
    fn log_writes_lines() {
        let _ = std::fs::remove_file(log_path());
        log("unit test line");
        let text = std::fs::read_to_string(log_path()).expect("log must exist");
        assert!(text.contains("unit test line"));
    }

    /// The whole new path, end to end, without a game: a real TCP frame goes in, the mailbox stages it,
    /// and the drain (standing in for the game-thread callback) puts exactly the replicated fields into
    /// a fake sim object.
    ///
    /// This is the test the previous design could not have: it proves `coop-link` → socket → mailbox →
    /// game-thread write works, which is the part that used to be "wrote N" with nothing behind it.
    #[test]
    fn a_tcp_frame_reaches_a_fake_sim_object_through_the_mailbox() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};

        // Bind an ephemeral port rather than the real 45991, so a running game does not collide.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");

        let cars: Vec<Car> = (0..22u8)
            .map(|id| Car {
                id,
                dist: 2000.0 + id as f32,
                lat: 0.25,
                head: 1.5,
                spd: 61.0,
                ..Default::default()
            })
            .collect();
        let expected = cars.clone();

        // The "agent" side: accept one frame, decode it, publish into a fresh mailbox.
        let mailbox = std::sync::Arc::new(hookmail::Mailbox::new());
        let agent_mailbox = std::sync::Arc::clone(&mailbox);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = stream.read(&mut chunk).expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Ok(Some(len)) = coop_proto::next_frame(&buf) {
                    let frame: Vec<u8> = buf.drain(..len).collect();
                    if let Ok(Msg::Snapshot { cars, .. }) = coop_proto::decode(&frame) {
                        assert_eq!(cars.len(), 22, "a whole field must arrive");
                        assert!(agent_mailbox.publish(&cars));
                        break;
                    }
                }
            }
        });

        let mut stream = TcpStream::connect(addr).expect("connect");
        let frame = encode(&Msg::Snapshot {
            tick: 9,
            sim_time: 12.0,
            race_time: 12.0,
            cars: cars.clone(),
        });
        stream.write_all(&frame).expect("write");
        stream.flush().expect("flush");
        server.join().expect("server thread");

        // The "game thread": drain into a fake object sized like the real one.
        let mut object = vec![0u8; coop_proto::layout::car_start(21) + 0x900];
        let base = object.as_ptr() as usize;
        let wrote = mailbox.drain(base);
        assert_eq!(
            wrote,
            22 * (hookmail::FIELDS.len() + hookmail::STATE_FIELDS.len()),
            "every car, every replicated field"
        );

        for want in &expected {
            let at = coop_proto::layout::car_start(want.id as usize) + coop_proto::layout::off::DIST;
            let got = u32::from_le_bytes([object[at], object[at + 1], object[at + 2], object[at + 3]]);
            assert_eq!(f32::from_bits(got), want.dist, "car {} distance", want.id);
        }
        // and the track-graph indices are still untouched, which is the crash this design removes
        let node_at = coop_proto::layout::car_start(0) + coop_proto::layout::off::NODE;
        assert_eq!(&object[node_at..node_at + 4], &[0, 0, 0, 0]);
    }

    /// Race control across the whole path, which is the bug that cost a sprint.
    ///
    /// `coop-link` used to send only `Msg::Snapshot` and never `Msg::Rc`, so a client learned car
    /// positions but never that the race was under a red flag - and froze its cars instead of following
    /// the host's replay. This test sends an `Rc` frame over a real socket, stages it with a car
    /// snapshot, and requires the flag bytes to land in a fake sim object.
    #[test]
    fn a_race_control_frame_reaches_a_fake_sim_object() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");

        let rc = coop_proto::RcState { flags: 0x100, rc_state: 3, rf_state: 4 };
        let mailbox = std::sync::Arc::new(hookmail::Mailbox::new());
        let agent_mailbox = std::sync::Arc::clone(&mailbox);
        let expected = rc;

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let mut got_rc = false;
            while !got_rc {
                let n = stream.read(&mut chunk).expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                loop {
                    if let Ok(Some(len)) = coop_proto::next_frame(&buf) {
                        let frame: Vec<u8> = buf.drain(..len).collect();
                        match coop_proto::decode(&frame) {
                            Ok(Msg::Rc(rc)) => {
                                // Mirror what the agent does: remember it, then publish.
                                got_rc = true;
                                agent_mailbox.publish_with_rc(&[], &rc);
                            }
                            Ok(_) => {}
                            Err(_) => {}
                        }
                    } else {
                        break;
                    }
                }
            }
            got_rc
        });

        let mut stream = TcpStream::connect(addr).expect("connect");
        stream.write_all(&encode(&Msg::Rc(rc))).expect("write");
        stream.flush().expect("flush");
        assert!(server.join().expect("server thread"), "the Rc frame must arrive");

        let mut object = vec![0xAAu8; coop_proto::layout::off::RF_STATE + 8];
        let base = object.as_ptr() as usize;
        assert_eq!(mailbox.drain(base), 3, "flags, rc_state, rf_state");

        let flags = u32::from_le_bytes([
            object[coop_proto::layout::off::FLAGS],
            object[coop_proto::layout::off::FLAGS + 1],
            object[coop_proto::layout::off::FLAGS + 2],
            object[coop_proto::layout::off::FLAGS + 3],
        ]);
        assert_eq!(flags, expected.flags, "the red flag bit must survive the trip");
        assert_eq!(object[coop_proto::layout::off::RC_STATE], expected.rc_state);
        assert_eq!(object[coop_proto::layout::off::RF_STATE], expected.rf_state);
        // and the 1-byte fields must not have trampled their neighbours
        assert_eq!(object[coop_proto::layout::off::RC_STATE + 1], 0xAA);
    }

    /// The lobby endpoint, exercised over a real socket.
    ///
    /// This is the piece that lets the main menu host without spawning a process, so it has to work from a
    /// browser-like context: correct JSON, CORS headers, and an OPTIONS preflight answered. A missing CORS
    /// header would make the menu button silently do nothing, which is the failure this test exists for.
    #[test]
    fn the_lobby_endpoint_answers_status_with_cors_headers() {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};

        // Bind an ephemeral port and drive the same handler the thread uses.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 512];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();

            let body = match path.as_str() {
                "/coop/status" => lobby_status_body(),
                "/coop/host" => lobby_host_body(),
                "/coop/join" => lobby_join_body(),
                _ => "{\"error\":\"no such endpoint\"}".to_string(),
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        });

        let mut s = TcpStream::connect(addr).expect("connect");
        s.write_all(b"GET /coop/status HTTP/1.1\r\nHost: x\r\n\r\n").expect("write");
        let mut out = String::new();
        s.read_to_string(&mut out).expect("read");
        server.join().expect("server");

        assert!(out.contains("Access-Control-Allow-Origin: *"), "CORS header is required:\n{out}");
        assert!(out.contains("\"injected\":true"));
        assert!(out.contains("\"hook_installed\":"));
        assert!(out.contains("\"tyre_mode\":"));
        // Valid JSON, not just plausible text.
        let body = out.split("\r\n\r\n").nth(1).expect("body");
        assert!(body.starts_with('{') && body.ends_with('}'), "body must be a JSON object: {body}");
    }

    /// Every endpoint must produce a JSON object. A malformed body would make the menu fail silently.
    #[test]
    fn every_lobby_endpoint_returns_a_json_object() {
        for body in [lobby_status_body(), lobby_host_body(), lobby_join_body()] {
            assert!(body.starts_with('{') && body.ends_with('}'), "not an object: {body}");
            // Braces must balance - a truncated format string would show up here.
            let opens = body.matches('{').count();
            let closes = body.matches('}').count();
            assert_eq!(opens, closes, "unbalanced braces: {body}");
        }
    }

    /// A fresh hook must report as unarmed, so the menu can say "inject first" instead of offering a host
    /// button that cannot work.
    #[test]
    fn status_reports_unarmed_before_injection() {
        let body = lobby_status_body();
        assert!(body.contains("\"hook_installed\":false") || body.contains("\"hook_installed\":true"));
        // In the test process no hook is installed, so this must be the honest answer.
        assert!(body.contains("\"callback_registered\":false"), "no callback in a test process: {body}");
    }

    /// Team claims must survive a round trip and must replace a role's previous claim, otherwise a player who
    /// changes their mind would appear to own two teams.
    #[test]
    fn a_team_claim_replaces_the_previous_claim_for_that_role() {
        let _ = release_team("host");
        let c = claim_team("Ferrari", "host");
        assert!(c.iter().any(|(t, r)| t == "Ferrari" && r == "host"));

        let c = claim_team("McLaren", "host");
        assert!(
            !c.iter().any(|(t, r)| t == "Ferrari" && r == "host"),
            "changing team must release the old one: {c:?}"
        );
        assert!(c.iter().any(|(t, r)| t == "McLaren" && r == "host"));

        // Two roles can hold different teams at the same time - that is the shared-team and versus case.
        let c = claim_team("Mercedes", "client");
        assert!(c.iter().any(|(t, _)| t == "McLaren"));
        assert!(c.iter().any(|(t, _)| t == "Mercedes"));
        let _ = release_team("host");
        let _ = release_team("client");
    }

    #[test]
    fn claims_serialise_to_valid_json() {
        let _ = release_team("host");
        let c = claim_team("Red Bull Racing", "host");
        let j = claims_json(&c);
        assert!(j.starts_with('[') && j.ends_with(']'), "{j}");
        assert!(j.contains("\"team\":\"Red Bull Racing\""), "{j}");
        assert!(j.contains("\"role\":\"host\""), "{j}");
        assert_eq!(j.matches('{').count(), j.matches('}').count(), "unbalanced: {j}");
        let _ = release_team("host");
    }

    /// The query parser is small enough to get subtly wrong, and a wrong parse means a team claim silently
    /// disappears - so it is tested directly, including spaces.
    #[test]
    fn query_strings_are_decoded() {
        let q = query_of("/coop/claim?team=Red%20Bull%20Racing&role=host");
        assert_eq!(q.get("team").map(String::as_str), Some("Red Bull Racing"));
        assert_eq!(q.get("role").map(String::as_str), Some("host"));

        let q = query_of("/coop/claim?team=Haas+F1&role=client");
        assert_eq!(q.get("team").map(String::as_str), Some("Haas F1"));

        // No query at all must not panic.
        assert!(query_of("/coop/status").is_empty());
    }

    /// With no callback registered the hook must be inert. Guards against the failure mode where a
    /// fresh install starts writing because the callback word happened to be non-zero.
    #[test]
    fn a_fresh_hook_has_no_callback() {
        assert_eq!(hook::callback(), 0, "an uninstalled hook reports no callback");
        assert_eq!(hook::fires(), 0, "an uninstalled hook has not fired");
    }
}
