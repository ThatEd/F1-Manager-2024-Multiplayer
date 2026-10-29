//! Both ends of the network link, so a two-machine test can actually carry race state.
//!
//! Before this, the transport existed but nothing used it: the supervisor relayed `Host` frames to
//! every `Client`, and no process ever sent a `Snapshot` or consumed one. This binary is the missing
//! half on each side.
//!
//! * `--role host`   reads the live game through `coop-mem` and publishes a `Snapshot` per tick.
//! * `--role client` receives snapshots, writes the newest one over a memory-mapped file, and reports
//!   the error against whatever it last received.
//!
//! The host reads the game externally, so this stays honest about the architecture: reads never
//! block the game's threads, and nothing here writes to game memory yet.

mod starthold;

use coop_proto::{decode, encode, Car, Msg, Side};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const FRAME_GAP: Duration = Duration::from_millis(25);

/// Frames with a non-advancing host clock before the session is assumed over. At 20 Hz this is
/// about two seconds: long enough to ride out a red flag or a pause, short enough that the client
/// cannot sit there writing into a finished session.
const STALE_FRAMES: u32 = 40;

/// Consecutive write failures before the address is dropped and rediscovered. Never retry a denied
/// write indefinitely: that is what pushed thousands of writes into freed memory and crashed a game.
const WRITE_FAIL_LIMIT: u32 = 3;

/// How often the client re-checks local sim identity for readback verification.
///
/// `locate()` and `find_sim_candidates()` walk GUObjectArray and cost milliseconds. Running them per
/// frame at 20 Hz throttled the client to roughly three snapshots a minute - it spent its time scanning
/// rather than forwarding, which presents as "sync is extremely slow" rather than as a bug in a scan.
const VERIFY_INTERVAL: Duration = Duration::from_millis(500);

/// Tracks whether the host's session is still advancing.
///
/// Two bugs were fixed here after a session crashed at the start line:
///
///  * the clock must not be trusted as "stalled" until it has been seen to advance at least once,
///    because at a race start it is legitimately 0.0 for many frames. Treating that as stalled
///    paused writes exactly when sync mattered most;
///  * `last_clock` starts at a negative sentinel so the first frame is never compared against 0.0,
///    which made every session look frozen from frame one.
#[derive(Debug)]
struct Staleness {
    last_clock: f32,
    last_tick: u32,
    unchanged: u32,
    reported: bool,
    seen_motion: bool,
}

impl Default for Staleness {
    fn default() -> Self {
        Staleness {
            last_clock: -1.0,
            last_tick: u32::MAX,
            unchanged: 0,
            reported: false,
            seen_motion: false,
        }
    }
}

impl Staleness {
    /// Feed one frame. Returns true when the session must be treated as over.
    ///
    /// `tick` is the host's send counter: monotonic, so it is the only sound liveness signal.
    /// `race_time` is passed for the message only, because it resets every lap.
    fn observe_tick(&mut self, tick: u32, sim_time: f32) -> bool {
        if self.last_tick != tick {
            self.last_tick = tick;
            self.unchanged = 0;
            self.reported = false;
            // Liveness is judged from `sim_time`, which advances monotonically. `race_time` was used
            // here and is wrong: it is `max(lap_time)` and freezes once cars stop completing laps, so it
            // reported "stalled" for the whole last part of a race and the client discarded good frames.
            if sim_time > 0.0 {
                self.seen_motion = true;
            }
            return false;
        }
        self.unchanged += 1;
        self.seen_motion && self.unchanged > STALE_FRAMES
    }

    /// Clock-based observation, kept for the tests and for callers with a monotonic clock.
    fn observe(&mut self, clock: f32) -> bool {
        if clock > self.last_clock {
            if clock > 0.0 {
                self.seen_motion = true;
            }
            self.last_clock = clock;
            self.unchanged = 0;
            self.reported = false;
            return false;
        }
        self.unchanged += 1;
        self.seen_motion && self.unchanged > STALE_FRAMES
    }

    /// True exactly once per stall, so the message is logged once rather than every frame.
    fn should_report(&mut self) -> bool {
        if self.reported {
            false
        } else {
            self.reported = true;
            true
        }
    }
}

/// Counts consecutive write failures and says when the address must be abandoned.
#[derive(Debug, Default)]
struct WriteFailures {
    consecutive: u32,
}

impl WriteFailures {
    /// Record one failure. Returns true when the caller must drop the address and rediscover.
    fn failed(&mut self) -> bool {
        self.consecutive += 1;
        if self.consecutive >= WRITE_FAIL_LIMIT {
            self.consecutive = 0;
            true
        } else {
            false
        }
    }

    fn succeeded(&mut self) {
        self.consecutive = 0;
    }
}

/// Used when neither `--supervisor` nor `coop_supervisor.txt` says otherwise.
const DEFAULT_SUPERVISOR: &str = "127.0.0.1:45990";

/// Reads `coop_supervisor.txt` next to the executable, the same file the in-process agent uses.
fn configured_supervisor() -> Option<String> {
    let path = std::env::current_exe().ok()?.with_file_name("coop_supervisor.txt");
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() { None } else { Some(line) }
}

fn usage() -> ! {
    eprintln!(
        "coop-link --role host   --supervisor <host:port> --pid <pid> [--hz N]\n\
         coop-link --role client --supervisor <host:port> [--hz N]\n\
         coop-link --inject [--pid <pid> | --name <image>] [--dll <path>]\n\
         \n\
         host:   reads the live RaceSimManager and publishes a Snapshot per tick\n\
         host --fake: publishes synthetic snapshots, to prove the link with no game running\n\
         host --send-save <file>: also ships that save to the client as the session payload\n\
         client: receives snapshots and writes corrections into the local sim\n\
         inject: loads coop_agent.dll beside this executable into the running game\n\
                 (so no separate injector has to be shipped)\n\
         \n\
         Point both at the supervisor, which relays Host frames to every Client."
    );
    std::process::exit(2);
}

/// Loads `coop_agent.dll` (beside this executable by default) into the running game.
///
/// Folded in from a separate tool deliberately: the injector is small, and every file removed from a co-op
/// install is one fewer thing that can be the wrong build. The DLL must still be its own file - it is loaded
/// by the game, not by us - but the injector does not have to be.
fn run_inject(args: &[String]) -> Result<(), String> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or("cannot find my own directory")?;
    let dll = arg(args, "--dll")
        .unwrap_or_else(|| exe_dir.join("coop_agent.dll").to_string_lossy().to_string());
    if !std::path::Path::new(&dll).is_file() {
        return Err(format!("no agent DLL at {dll}"));
    }

    let pid = match (arg(args, "--pid").and_then(|s| s.parse::<u32>().ok()), arg(args, "--name")) {
        (Some(p), _) => p,
        (None, Some(n)) => coop_inject::pid_by_name(&n)
            .ok_or_else(|| format!("no running process named {n}"))?,
        (None, None) => coop_inject::pid_by_name("F1Manager24.exe")
            .ok_or("no F1Manager24.exe running; pass --pid or --name")?,
    };

    let base = coop_inject::inject(pid, &dll)?;
    println!("injected {dll} into pid {pid} (module base 0x{base:X})");
    println!("check coop_agent.log beside the game exe for the hook and the write listener");
    Ok(())
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Appends one line to `coop_link.log` beside the executable, and echoes it to the console.
///
/// The link can be started by the hotkey, in which case its console is a window the player may not
/// have in front of them; without a file there is no way to see why a join failed.
fn say(line: &str) {
    println!("{line}");
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.with_file_name("coop_link.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            use std::io::Write;
            let _ = writeln!(f, "[{stamp}] {line}");
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `--inject` is checked before the role so it works with no other arguments, which is how the lobby
    // will call it: one binary, one flag.
    if args.iter().any(|a| a == "--inject") {
        if let Err(e) = run_inject(&args) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    let role = arg(&args, "--role").unwrap_or_else(|| "client".to_string());
    // A hotkey launch passes no arguments, so the address has to come from somewhere durable:
    // coop_supervisor.txt next to the executable, then the compiled-in default.
    let supervisor = arg(&args, "--supervisor")
        .or_else(configured_supervisor)
        .unwrap_or_else(|| DEFAULT_SUPERVISOR.to_string());
    let hz: u32 = arg(&args, "--hz").and_then(|s| s.parse().ok()).unwrap_or(20);

    let result = match role.as_str() {
        "host" if args.iter().any(|a| a == "--fake") => {
            let save = arg(&args, "--send-save");
            run_fake_host(&supervisor, hz, save.as_deref())
        }
        "host" => {
            // The game runs as a launcher plus the real client; picking the largest match means the
            // player never has to look up a pid, and we never attach to the empty launcher.
            let pid: u32 = match arg(&args, "--pid").and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None => match coop_mem::find_processes("F1Manager24.exe").first() {
                    Some(&p) => {
                        println!("[host] auto-selected game pid {p}");
                        p
                    }
                    None => {
                        eprintln!("no F1Manager24.exe process found; start the game or pass --pid");
                        std::process::exit(1);
                    }
                },
            };
            let save = arg(&args, "--send-save");
            run_host(&supervisor, pid, hz, save.as_deref())
        }
        "client" => {
            // A client that is not attached to its own game cannot apply anything; it only mirrors
            // the host into a text file. Auto-select the largest F1Manager24 process for the same
            // reason the host does: the small one is the launcher.
            let pid = arg(&args, "--pid")
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| coop_mem::find_processes("F1Manager24.exe").first().copied());
            run_client(&supervisor, hz, pid)
        }
        other => {
            eprintln!("unknown role {other}");
            usage()
        }
    };
    if let Err(e) = result {
        eprintln!("[link] {e}");
        std::process::exit(1);
    }
}

fn connect(supervisor: &str, side: Side, cars_owned: u32) -> Result<TcpStream, String> {
    let stream = TcpStream::connect(supervisor)
        .map_err(|e| format!("connect to supervisor at {supervisor}: {e}"))?;
    stream.set_nodelay(true).ok();
    let mut s = stream;
    let hello = encode(&Msg::Hello { side, cars_owned });
    s.write_all(&hello).map_err(|e| format!("hello: {e}"))?;
    s.flush().ok();
    Ok(s)
}

/// Port the in-process agent listens on for car frames.
///
/// Matches `coop_agent::DEFAULT_WRITE_PORT`; kept as a separate constant rather than a dependency so
/// `coop-link` stays a plain external tool that can be built and run without the agent's build.
const AGENT_WRITE_PORT: u16 = 45991;

/// Where this client's agent is listening. `coop_agent_addr.txt` next to the executable overrides, so a
/// two-machine test does not need a rebuild and a non-default port stays possible.
fn agent_address() -> String {
    let configured = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("coop_agent_addr.txt")));
    if let Some(path) = configured {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let line = text.lines().next().unwrap_or("").trim().to_string();
            if !line.is_empty() {
                return line;
            }
        }
    }
    format!("127.0.0.1:{AGENT_WRITE_PORT}")
}

/// The channel that puts host car state into the local game.
///
/// Sends whole `Snapshot` frames to the in-process agent, which stages them in a mailbox that the
/// game-thread hook drains. This exists because writing sim fields from *this* process was the crash:
/// `EXCEPTION_ACCESS_VIOLATION reading address 0xF3C`, three times, always shortly after our writes.
/// Writing to the sim from a thread the simulation does not own is not a race that can be tuned away -
/// the game is midway through consuming those fields when we change them underneath it.
///
/// Reconnects on its own so an agent restart (or a game restart) does not end the session.
struct WriteChannel {
    addr: String,
    stream: Option<TcpStream>,
    sent: u64,
    refused: u64,
    reported: bool,
    failures: WriteFailures,
}

impl WriteChannel {
    fn new(addr: String) -> Self {
        WriteChannel {
            addr,
            stream: None,
            sent: 0,
            refused: 0,
            reported: false,
            failures: WriteFailures::default(),
        }
    }

    /// Sends one frame, reconnecting if needed. Returns whether the frame was accepted.
    fn send_frame(&mut self, frame: &[u8]) -> bool {
        if self.stream.is_none() {
            match TcpStream::connect(&self.addr) {
                Ok(s) => {
                    s.set_nodelay(true).ok();
                    println!("[client] write channel connected to the agent at {}", self.addr);
                    self.refused = 0;
                    self.stream = Some(s);
                }
                Err(e) => {
                    self.refused += 1;
                    if !self.reported {
                        println!(
                            "[client] no agent at {} ({e}); is the game running with coop_agent.dll \
                             injected? Not writing to the sim from here.",
                            self.addr
                        );
                        self.reported = true;
                    }
                    return false;
                }
            }
        }
        let stream = self.stream.as_mut().expect("stream was just connected");
        match stream.write_all(frame).and_then(|()| stream.flush()) {
            Ok(()) => {
                self.sent += 1;
                self.reported = false;
                self.failures.succeeded();
                true
            }
            Err(e) => {
                // Drop the socket immediately rather than retrying into it: a half-open socket that
                // still accepts writes while nothing reads them looks exactly like healthy sync.
                println!("[client] write channel lost ({e}); will reconnect");
                self.stream = None;
                self.failures.failed();
                false
            }
        }
    }

    /// Forwards race control. Separate from [`send`] so its success is never confused with a snapshot's:
    /// a lost red flag is far worse than a lost frame of positions.
    fn send_rc(&mut self, rc: coop_proto::RcState) -> bool {
        let frame = encode(&Msg::Rc(rc));
        self.send_frame(&frame)
    }

    /// Sends a correction frame: absolute values resolved from deltas, so the agent writes them directly.
    ///
    /// The delta arithmetic happens here, in the client, because only the client can read its own sim's
    /// current values. The agent receives plain absolute writes, exactly as the mailbox expects.
    ///
    /// Returns false when there is nothing worth sending, so a synchronised field costs no traffic at all.
    fn send_corrections(
        &mut self,
        local: &[Car],
        corr: &[(u8, coop_proto::correct::Correction)],
        tick: u32,
        sim_time: f32,
        race_time: f32,
    ) -> bool {
        if corr.is_empty() {
            return false;
        }
        // Apply the corrections to our own snapshot of the local state, so the frame carries absolute
        // positions. Only the corrected fields are touched; the rest of each car is sent as-is and the
        // agent's `stage_corrections` will pick the same fields.
        let mut target: Vec<Car> = local.to_vec();
        for (id, c) in corr {
            if let Some(car) = target.iter_mut().find(|x| x.id == *id) {
                car.dist += c.dist;
                car.lat += c.lat;
                car.head += c.head;
                car.spd += c.spd;
            }
        }
        let frame = encode(&Msg::Snapshot {
            tick,
            sim_time,
            race_time,
            cars: target,
        });
        self.send_frame(&frame)
    }

    /// Sends one car snapshot.
    fn send(&mut self, cars: &[Car], tick: u32, sim_time: f32, race_time: f32) -> bool {
        let frame = encode(&Msg::Snapshot {
            tick,
            sim_time,
            race_time,
            cars: cars.to_vec(),
        });
        self.send_frame(&frame)
    }
}

/// Reads frames from the supervisor. Yields one frame body at a time.
struct Frames {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Frames {
    fn new(stream: TcpStream) -> Self {
        Self { stream, buf: Vec::with_capacity(16 * 1024) }
    }

    /// Blocks until a whole frame is available.
    fn next_frame(&mut self) -> Result<Vec<u8>, String> {
        let mut chunk = [0u8; 8192];
        loop {
            // A frame is self-delimiting: header carries its length, and `decode` rejects short input
            // rather than mis-parsing, so try to decode as soon as we have anything.
            if !self.buf.is_empty() {
                if let Some(frame) = split_frame(&self.buf) {
                    self.buf.drain(..frame.len());
                    return Ok(frame);
                }
            }
            let n = self.stream.read(&mut chunk).map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Err("supervisor closed the connection".into());
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

/// Pulls one complete frame out of `buf`, or None if more bytes are needed.
///
/// Uses `coop-proto`'s own splitter rather than duplicating the header format, so a partial frame
/// stays in the buffer instead of being mis-parsed as a short one.
fn split_frame(buf: &[u8]) -> Option<Vec<u8>> {
    match coop_proto::next_frame(buf) {
        Ok(Some(total)) if buf.len() >= total => Some(buf[..total].to_vec()),
        Ok(_) => None,
        Err(_) => {
            // A corrupt header cannot be framed; dropping the buffer is the only recovery, and the
            // caller will report the gap rather than pretending it decoded.
            buf.last().map(|_| Vec::new())
        }
    }
}

/// How often race control is re-sent even when unchanged.
///
/// A client that restarts, reconnects or loads a save must learn the current flag state without waiting
/// for the next flag change - which for a red flag could be the rest of the race.
const RC_RESEND: Duration = Duration::from_secs(1);

/// Reads the host's simulation clock.
///
/// `SIM_TIME` at `0x99AF1C` is the field that actually advances. `race_time` (`max(lap_time)`) does not:
/// measured live, `SIM_TIME` went 951 -> 953 -> 955 while every car's `lap_time` stayed frozen, so the
/// snapshot carried a constant clock for the whole last part of the race.
///
/// A 4-byte read, deliberately separate from `read_cars`: the 10.5 MB object read has been seen to fail
/// with `ERROR_PARTIAL_COPY` mid-session, and losing the clock is losing the ability to drive the other
/// sim at all.
fn read_sim_time(proc: &coop_mem::Process, rsm: usize) -> Result<f32, String> {
    use coop_proto::layout::off;
    let b = proc
        .read(rsm + off::SIM_TIME, 4)
        .map_err(|e| format!("read sim_time: {e}"))?;
    let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    // A NaN or a negative clock is not a reading; 0 is the documented "unknown" value on the wire, so
    // treat anything implausible as unknown rather than as a real instant.
    if !v.is_finite() || v < 0.0 {
        return Err(format!("implausible sim_time {v}"));
    }
    Ok(v)
}

/// Reads race control out of the sim.
///
/// Deliberately reads the three fields directly instead of slurping the whole 10.5 MB object: the fields
/// are 6 bytes total, `RF_STATE` sits at `0xA13289`, and a single large read of the object has been seen
/// to fail with `ERROR_PARTIAL_COPY` mid-session - which would silently lose every flag change.
fn read_rc(proc: &coop_mem::Process, rsm: usize) -> Result<coop_proto::RcState, String> {
    use coop_proto::layout::off;

    let u32_at = |at: usize| -> Result<u32, String> {
        let b = proc.read(at, 4).map_err(|e| format!("read 0x{at:X}: {e}"))?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let u8_at = |at: usize| -> Result<u8, String> {
        let b = proc.read(at, 1).map_err(|e| format!("read 0x{at:X}: {e}"))?;
        Ok(b[0])
    };

    Ok(coop_proto::RcState {
        flags: u32_at(rsm + off::FLAGS)?,
        rc_state: u8_at(rsm + off::RC_STATE)?,
        rf_state: u8_at(rsm + off::RF_STATE)?,
    })
}

/// True when any car's distance has advanced, which is the only honest evidence that this object is
/// the live sim rather than a class default or a pre-start copy.
///
/// Deliberately does not use `read_cars`: that reads the whole 10.5 MB object, and this is called
/// repeatedly while waiting for a session to start. Twenty-two four-byte reads is the whole cost.
fn has_motion(proc: &coop_mem::Process, base: usize) -> bool {
    use coop_proto::layout;
    for car in 0..layout::CAR_MAX {
        let at = base + layout::car_start(car) + layout::off::DIST;
        if let Ok(b) = proc.read(at, 4) {
            let d = f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            if d.is_finite() && d > 1.0 {
                return true;
            }
        }
    }
    false
}

fn run_host(supervisor: &str, pid: u32, hz: u32, save: Option<&str>) -> Result<(), String> {
    let proc = coop_mem::Process::open(pid).map_err(|e| format!("open pid {pid}: {e}"))?;
    let mut stream = connect(supervisor, Side::Host, 22)?;
    say(&format!("[host] connected to supervisor at {supervisor}, reading pid {pid} at {hz} Hz"));

    // Sending the session must not depend on a live race sim: the whole point of the transfer is to
    // get the client to the same place, and that is done while the host sits in menus. Discovery is
    // therefore attempted, not required - a failure here just means no snapshots until a race exists.
    let mut rsm = match locate(&proc) {
        Ok(r) => {
            println!("[host] RaceSimManager at 0x{r:X}");
            r
        }
        Err(e) => {
            println!("[host] no live sim yet ({e}); still able to send the session save");
            0
        }
    };
    let period = Duration::from_millis(1000 / hz.max(1) as u64);
    let mut tick = 0u32;
    let mut confirmed = false;
    let mut candidates: Vec<usize> = Vec::new();
    let mut last_discovery = Instant::now() - Duration::from_secs(60);
    let mut last_report = Instant::now();
    let mut sent = 0u32;
    let mut last_offer: Option<Instant> = None;
    let mut last_rc: Option<coop_proto::RcState> = None;
    let mut last_rc_sent: Option<Instant> = None;

    loop {
        if let Some(path) = save {
            if last_offer.is_none() || last_offer.unwrap().elapsed() >= SESSION_RESEND {
                if let Err(e) = send_session(&mut stream, path) {
                    eprintln!("[host] session send failed: {e}");
                }
                last_offer = Some(Instant::now());
            }
        }
        if rsm == 0 || !confirmed {
            // Hold every plausible object and wait to see which one steps. A pre-start sim is not
            // distinguishable from a class-default by inspection, so the only correct signal is
            // motion - and waiting for it costs nothing, because the client is already attached and
            // idle.
            if last_discovery.elapsed() >= Duration::from_secs(2) {
                let base = match proc.module_base("F1Manager24.exe") {
                    Ok(b) => b,
                    Err(_) => {
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                };
                candidates = coop_mem::objects::find_sim_candidates(&proc, base);
                last_discovery = Instant::now();
                if candidates.len() > 1 && !confirmed {
                    println!("[host] {} candidate(s); waiting for one to start stepping", candidates.len());
                }
            }

            // Probe only the distance field, not the whole car block.
            //
            // `read_cars` pulls ~10.5 MB per candidate; doing that for a handful of candidates four
            // times a second is tens of MB/s of process reads purely to answer "has anything moved
            // yet". Reading the 22 distance floats costs 88 bytes and answers the same question.
            let mut stepped = None;
            for &cand in &candidates {
                if has_motion(&proc, cand) {
                    stepped = Some(cand);
                    break;
                }
            }

            match stepped {
                Some(found) => {
                    if found != rsm {
                        println!("[host] live sim is 0x{found:X} - locking on");
                        rsm = found;
                    }
                    confirmed = true;
                }
                None => {
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }

        let started = Instant::now();
        let cars = match proc.read_cars(rsm) {
            Ok(c) => c,
            Err(_) => {
                // The object went away - a race ending or a level change. Rediscover, and skip this
                // tick rather than publishing a partially valid snapshot.
                match locate(&proc) {
                    Ok(r) => {
                        println!("[host] RaceSimManager moved to 0x{r:X}");
                        rsm = r;
                    }
                    Err(_) => rsm = 0,
                }
                continue;
            }
        };

        // Confirm the chosen object is the one actually stepping. Until then, keep watching: an
        // all-zero pre-start pick can be the wrong object, and committing to it publishes a dead
        // object for the whole race - which is an unsynchronised start.
        if !confirmed {
            let moving = cars.iter().any(|c| c.dist > 1.0);
            if moving {
                confirmed = true;
                println!("[host] confirmed live: cars are moving at 0x{rsm:X}");
            }
        }
        // `race_time` is for display only (it is `max(lap_time)` and freezes when cars stop
        // completing laps). The clock that actually advances is SIM_TIME, and the client needs it to
        // know how far to advance its own sim.
        let race_time = cars.iter().map(|c| c.lap_time).fold(0.0f32, f32::max);
        let sim_time = read_sim_time(&proc, rsm).unwrap_or(0.0);

        // Race control first, and only when it changes.
        //
        // This is what tells the client the race is under a red flag. Without it the client sees the
        // host's distances stop changing and simply freezes its own cars, because nothing explains why -
        // which is exactly the reported symptom.
        //
        // Sent as its own message, not folded into the snapshot, so it cannot be lost with a dropped
        // frame. Re-sent every second even when unchanged, so a client that reconnects or restarts does
        // not have to wait for the *next* flag change to learn the current state.
        if let Ok(rc) = read_rc(&proc, rsm) {
            let changed = last_rc.map(|p| p != rc).unwrap_or(true);
            let due = last_rc_sent.map(|t| t.elapsed() >= RC_RESEND).unwrap_or(true);
            if changed || due {
                if changed && last_rc.is_some() {
                    println!(
                        "[host] race control: flags=0x{:X} rc_state={} rf_state={}",
                        rc.flags, rc.rc_state, rc.rf_state
                    );
                }
                let f = encode(&Msg::Rc(rc));
                if let Err(e) = stream.write_all(&f).and_then(|_| stream.flush()) {
                    return Err(format!("supervisor went away: {e}"));
                }
                last_rc = Some(rc);
                last_rc_sent = Some(Instant::now());
            }
        }

        let frame = encode(&Msg::Snapshot { tick, sim_time, race_time, cars });
        if let Err(e) = stream.write_all(&frame).and_then(|_| stream.flush()) {
            return Err(format!("supervisor went away: {e}"));
        }
        sent += 1;
        tick = tick.wrapping_add(1);

        if last_report.elapsed() >= Duration::from_secs(2) {
            println!(
                "[host] {sent} snapshots, tick {tick}, {:.1} Hz, last frame {} bytes",
                sent as f32 / last_report.elapsed().as_secs_f32().max(0.001),
                frame.len()
            );
            last_report = Instant::now();
            sent = 0;
        }

        let elapsed = started.elapsed();
        if elapsed < period {
            std::thread::sleep(period - elapsed);
        }
    }
}

/// Publishes synthetic, moving snapshots so the network path can be verified on its own.
///
/// A LAN test should not need a live race: if this reaches the other machine, the transport, the
/// relay, the framing and the client all work, and anything still broken is in the game side. Cars
/// move at a constant speed on distinct distances, so a client can tell them apart.
fn run_fake_host(supervisor: &str, hz: u32, save: Option<&str>) -> Result<(), String> {
    let mut stream = connect(supervisor, Side::Host, 22)?;
    println!("[host] fake source connected to {supervisor} at {hz} Hz");
    if let Some(path) = save {
        send_session(&mut stream, path)?;
    }
    let period = Duration::from_millis(1000 / hz.max(1) as u64);
    let start = Instant::now();
    let mut last_offer: Option<Instant> = None;
    let mut tick = 0u32;
    let mut last_report = Instant::now();
    let mut sent = 0u32;

    loop {
        // Re-offer the session on a slow cadence so a client joining later still gets it. Re-sending
        // is safe: the receiver writes only on a verified hash and ignores a duplicate.
        if let Some(path) = save {
            if last_offer.is_none() || last_offer.unwrap().elapsed() >= SESSION_RESEND {
                if let Err(e) = send_session(&mut stream, path) {
                    eprintln!("[host] session send failed: {e}");
                }
                last_offer = Some(Instant::now());
            }
        }
        let t = start.elapsed().as_secs_f32();
        let cars: Vec<Car> = (0..22u8)
            .map(|id| {
                let spd = 40.0 + id as f32;
                Car {
                    id,
                    dist: 1000.0 + spd * t,
                    spd,
                    lap: 1 + (spd * t / 5000.0) as i32,
                    pos: id + 1,
                    tyre_lo: 1.0 - (t / 100.0).min(0.9),
                    ..Default::default()
                }
            })
            .collect();
        let frame = encode(&Msg::Snapshot { tick, sim_time: t, race_time: t, cars });
        stream
            .write_all(&frame)
            .and_then(|_| stream.flush())
            .map_err(|e| format!("supervisor went away: {e}"))?;
        sent += 1;
        tick = tick.wrapping_add(1);

        if last_report.elapsed() >= Duration::from_secs(2) {
            println!(
                "[host] {sent} fake snapshots, {} bytes each, sim clock {t:.1}s",
                frame.len()
            );
            sent = 0;
            last_report = Instant::now();
        }
        std::thread::sleep(period);
    }
}

/// Chunk size for the session payload.
///
/// 32 KB keeps a single frame comfortably under the relay's re-framing capacity and the socket
/// buffers, so a 12 MB save is ~370 frames rather than one message that has to fit in memory on every
/// hop. Small enough that a lost chunk costs little, big enough not to be dominated by frame headers.
const SESSION_CHUNK: usize = 32 * 1024;

/// How often the host re-offers the session payload.
///
/// The relay forwards only to peers connected at that moment, so a client that joins after the first
/// transfer would otherwise never receive a session at all. Re-sending is idempotent on the receiving
/// side: it writes only when the hash matches and ignores a duplicate.
const SESSION_RESEND: Duration = Duration::from_secs(10);

/// Sends a save file to the client as the session payload.
///
/// The file is treated as opaque bytes. It is NOT parsed, rebuilt or re-compressed: the game stores it
/// with frontier's own compressor (`Oodle`/`zstd` are linked statically into the exe), so the only
/// correct handling is byte-exact carriage of the game's own output. The receiver verifies the hash,
/// which is what makes a truncated or corrupted transfer a loud failure instead of a save that loads
/// half a season.
fn send_session(stream: &mut TcpStream, path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read save {path}: {e}"))?;
    let name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "session.sav".to_string());
    if name.len() > 255 {
        return Err("save name is too long to send".into());
    }
    let hash = fnv1a32(&bytes);
    let total = bytes.len() as u64;
    say(&format!(
        "[host] sending session payload {name}: {:.2} MB, hash {hash:#010X}, chunk {SESSION_CHUNK}",
        total as f64 / 1_048_576.0
    ));

    stream
        .write_all(&encode(&Msg::SessionOffer {
            name_len: name.len() as u8,
            name: name.clone().into_bytes(),
            total,
            hash,
        }))
        .map_err(|e| format!("send offer: {e}"))?;

    let started = Instant::now();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + SESSION_CHUNK).min(bytes.len());
        stream
            .write_all(&encode(&Msg::SessionChunk {
                offset: offset as u64,
                bytes: bytes[offset..end].to_vec(),
            }))
            .map_err(|e| format!("send chunk at {offset}: {e}"))?;
        offset = end;
    }
    stream
        .write_all(&encode(&Msg::SessionReady { total, hash }))
        .map_err(|e| format!("send ready: {e}"))?;
    stream.flush().ok();

    say(&format!(
        "[host] session payload sent in {} ms",
        started.elapsed().as_millis()
    ));
    Ok(())
}

/// FNV-1a 32-bit, matching the protocol's own checksum so one hash implementation is used everywhere.
fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

fn locate(proc: &coop_mem::Process) -> Result<usize, String> {
    let base = proc.module_base("F1Manager24.exe").map_err(|e| format!("module base: {e}"))?;
    coop_mem::objects::find_sim(proc, base)
}

/// The client, with reconnect.
///
/// A single `run_client` attempt ends the process the moment the relay goes away, and a relay
/// restart therefore killed sync permanently - the client sat on a dead socket receiving nothing
/// while its log stayed silent, which looked exactly like "no sync" for an hour here. Reconnecting
/// with backoff is what makes the client survive an operator restart or a share hiccup.
fn run_client(supervisor: &str, hz: u32, pid: Option<u32>) -> Result<(), String> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match run_client_once(supervisor, hz, pid) {
            Ok(()) => return Ok(()),
            Err(e) => {
                println!("[client] disconnected ({e}); reconnecting (attempt {attempt})");
                let backoff = Duration::from_millis(500u64 * attempt.min(10) as u64);
                std::thread::sleep(backoff);
            }
        }
    }
}

fn run_client_once(supervisor: &str, hz: u32, pid: Option<u32>) -> Result<(), String> {
    let stream = connect(supervisor, Side::Client, 0)?;
    say(&format!("[client] connected to supervisor at {supervisor}, waiting for the host"));

    // Attaching to the local game is what turns a viewer into a synced client. Without it this
    // process only mirrors the host's state into a text file, which looks like progress and is not.
    let mut proc = None;
    let mut rsm = 0usize;
    if let Some(pid) = pid {
        let p = coop_mem::Process::open(pid).map_err(|e| format!("open pid {pid}: {e}"))?;
        say(&format!("[client] attached to pid {pid}"));
        // A game started elevated cannot be written to from an unelevated client, and Windows
        // reports that as a bare access-denied at a valid address. Naming it here saves anyone
        // debugging the wrong thing for an hour.
        if p.target_is_elevated() {
            say("[client] WARNING: the local game runs at a higher integrity level than this \
                  process, so writes will be refused. Restart coop-link elevated (or run the game \
                  unelevated) to sync.");
        }
        match locate(&p) {
            Ok(a) => {
                say(&format!("[client] RaceSimManager at 0x{a:X}"));
                rsm = a;
                proc = Some(p);
            }
            Err(e) => {
                say(&format!("[client] no live RaceSimManager yet ({e}); will retry when a race starts"));
                proc = Some(p);
            }
        }
    } else {
        say("[client] no --pid given: mirror only, the local game will NOT be written");
    }

    let mut frames = Frames::new(stream);
    let hz = hz.max(1);
    let period = Duration::from_millis(1000 / hz as u64);

    let mut session = SessionReceiver::default();
    let mut received = 0u32;
    let mut applied = 0u32;
    let mut verified = 0u32;
    let mut worst_delta = 0f32;
    let mut rejections = 0u32;
    let mut staleness = Staleness::default();
    let mut channel = WriteChannel::new(agent_address());
    let mut last_verify = Instant::now() - VERIFY_INTERVAL;
    let mut not_stepping_reported = false;
    let mut zero_frames_reported = false;
    let mut corrections = 0u32;
    let mut last_rc: Option<coop_proto::RcState> = None;
    let mut last_tick: Option<u32> = None;
    let mut last_report = Instant::now();
    let mut worst_interval = Duration::ZERO;
    let mut last_arrival: Option<Instant> = None;

    // A frame from the host is larger than the socket buffer, so read in a loop and only count a
    // complete message; a partial read is not a lost frame.
    loop {
        let frame = frames.next_frame()?;
        let msg = decode(&frame).map_err(|e| format!("decode: {e:?}"))?;

        // Session transfer frames are handled first and never fall through to snapshot logic.
        let is_session = matches!(
            msg,
            Msg::SessionOffer { .. } | Msg::SessionChunk { .. } | Msg::SessionReady { .. }
        );
        if is_session {
            if let Some(text) = handle_session(&mut session, msg) {
                say(&format!("[client] {text}"));
            }
            continue;
        }
        let Msg::Snapshot { tick, sim_time, race_time, cars } = msg else {
            // Race control rides its own message. Forwarding it is what stops the client's cars
            // freezing under a red flag with no idea why.
            if let Msg::Rc(rc) = msg {
                if last_rc != Some(rc) {
                    println!(
                        "[client] race control: flags=0x{:X} rc_state={} rf_state={}",
                        rc.flags, rc.rc_state, rc.rf_state
                    );
                    last_rc = Some(rc);
                }
                // Send it even when the cars are stationary - that is precisely when it matters.
                channel.send_rc(rc);
            }
            continue;
        };

        if let Some(previous) = last_tick {
            if tick != previous.wrapping_add(1) {
                println!("[client] gap: tick {previous} -> {tick}");
            }
        }
        last_tick = Some(tick);

        let now = Instant::now();
        if let Some(prev) = last_arrival {
            let gap = now - prev;
            if gap > worst_interval {
                worst_interval = gap;
            }
        }
        last_arrival = Some(now);

        // The host's state goes to the local game through the agent's game-thread hook.
        //
        // Sending deliberately does **not** depend on this process locating the sim. The hook captures
        // the sim pointer from `rcx` inside the sim call, so the agent is the authority on which object
        // to write. Requiring a second, independent discovery here only added a way to block sync: a
        // client that failed to pattern-match the object would refuse to send frames even though the
        // agent knew exactly where to put them. That is a self-inflicted "no sync".
        //
        // Reads for verification still happen here, because reads from any thread are safe - they
        // cannot change what the game sees.

        // A session that has stopped advancing must not be written to.
        //
        // Keyed on the host's *tick*, not on `race_time`: race_time is `max(lap_time)` and resets to
        // zero every lap, so it is not monotonic and declaring a stall from it paused writes at the
        // start line and at every lap boundary. Tick only ever increases.
        if staleness.observe_tick(tick, sim_time) {
            if staleness.should_report() {
                println!(
                    "[client] session appears over (sim_time fixed at {sim_time:.1}); \
                     writes paused"
                );
            }
            rsm = 0;
            write_shadow(tick, race_time, &cars)?;
            received += 1;
            continue;
        }

        // A frame whose cars are all at distance zero carries no information and is actively
        // dangerous: it would plant the local sim at the start line if it landed mid-race. The host
        // publisher refuses to publish such a frame, and this is the second line of defence, because a
        // zero write is indistinguishable from a legitimate grid start once it is on the wire.
        let any_motion = cars.iter().any(|c| c.dist.is_finite() && c.dist.abs() > 1.0);
        if !any_motion {
            if !zero_frames_reported {
                println!(
                    "[client] host is sending an all-zero frame (session not started yet?); \
                     not forwarding it to the agent"
                );
                zero_frames_reported = true;
            }
            write_shadow(tick, race_time, &cars)?;
            received += 1;
            continue;
        }
        zero_frames_reported = false;

        // Drive by correction, not by overwrite.
        //
        // `bSubstepping=False` means the receiving sim integrates a variable wall-clock delta, so a raw
        // state write is integrated past: measured in the last session, 1.88 M field writes still left the
        // client 62 km off. A correction sized by the *time gap* converges instead, because it moves the
        // car a fraction of the way each frame and lets the sim's own integrator track it.
        //
        // Needs the receiving sim's current values, so it only runs once we have located the local sim. If
        // we have not, fall back to the previous full-state send - worse, but it is what shipped before and
        // it does at least move cars.
        let mut used_correction = false;
        if let (Some(p), true) = (proc.as_ref(), rsm != 0) {
            if let Ok(local) = p.read_cars(rsm) {
                let client_time = read_sim_time(p, rsm).unwrap_or(0.0);
                let corr = coop_proto::correct::correct_field(&cars, &local, sim_time, client_time);
                if channel.send_corrections(&local, &corr, tick, sim_time, race_time) {
                    applied += corr.len() as u32;
                    corrections += corr.len() as u32;
                    used_correction = true;
                }
            }
        }
        if !used_correction {
            if channel.send(&cars, tick, sim_time, race_time) {
                applied += cars.len() as u32;
            } else {
                rejections += 1;
            }
        }

        // Verification is best-effort and clearly labelled: if this process cannot identify the live
        // sim, the frame was still sent and the agent may well have applied it, so the report says
        // "sent" without claiming "verified".
        //
        // Cost discipline matters more than precision here. `find_sim_candidates()` enumerates ~507
        // addresses inside GUObjectArray and `read_cars()` pulls ~10 MB per candidate, so one sweep is
        // roughly 5 GB of reads. Doing that on a timer - even at 0.5 Hz, even once per session - starved
        // the transport badly enough that the client forwarded a single frame in twelve seconds. So the
        // hot path never sweeps: it uses the single best candidate `locate()` already picked, and a
        // failed read is the only thing that forces a re-resolve.
        if let Some(p) = proc.as_ref() {
            if rsm == 0 && last_verify.elapsed() >= VERIFY_INTERVAL {
                last_verify = Instant::now();
                if let Ok(a) = locate(p) {
                    println!("[client] RaceSimManager at 0x{a:X} (for verification only)");
                    rsm = a;
                }
            }

            if rsm != 0 {
                match p.read_cars(rsm) {
                    Ok(back) => {
                        let mut worst = 0f32;
                        let mut matched = 0;
                        let mut moving = 0;
                        for (want, got) in cars.iter().zip(back.iter()) {
                            if want.id != got.id {
                                continue;
                            }
                            if got.dist.abs() > 1.0 {
                                moving += 1;
                            }
                            let d = (want.dist - got.dist).abs();
                            if d.is_finite() && d < 0.5 {
                                matched += 1;
                            }
                            if d.is_finite() && d > worst {
                                worst = d;
                            }
                        }
                        verified = matched as u32;
                        worst_delta = worst;
                        // All-zero distances mean the local session has not started, so the delta is
                        // meaningless rather than bad. Say that once instead of printing a 0-of-22 that
                        // reads like a sync failure.
                        if moving == 0 && !not_stepping_reported {
                            println!(
                                "[client] local sim reads all zeros (session not started yet), so \
                                 verification is unavailable - frames are still being sent"
                            );
                            not_stepping_reported = true;
                        } else if moving > 0 {
                            not_stepping_reported = false;
                        }
                    }
                    Err(e) => {
                        // A failed read means the address went stale, not that the frame failed.
                        if rejections <= 5 {
                            println!("[client] readback failed ({e}); re-resolving");
                        }
                        rsm = 0;
                        last_verify = Instant::now() - VERIFY_INTERVAL;
                    }
                }
            }
        }


        write_shadow(tick, race_time, &cars)?;
        received += 1;

        if last_report.elapsed() >= Duration::from_secs(2) {
            let rejected = if rejections > 0 {
                format!(", {rejections} rejected")
            } else {
                String::new()
            };
            println!(
                "[client] {received} snapshots, {} cars, sim_time {sim_time:.1}, gap {} ms, \
                 sent {applied} to the agent ({corrections} corrections), verified {verified} of {} \
                 (worst dist delta {worst_delta:.2} m){rejected}",
                cars.len(),
                worst_interval.as_millis(),
                cars.len()
            );
            last_report = Instant::now();
            received = 0;
            worst_interval = Duration::ZERO;
        }

        // Read as fast as frames arrive; the host sets the rate. Sleeping would only add latency.
        let _ = period;
    }
}

/// Assembles a session payload from the wire, then writes it out exactly once.
///
/// Deliberately holds the bytes in memory until the hash matches. A partially written `.sav` in the
/// game's SaveGames directory is worse than no save at all: the game would list it and could load a
/// truncated season. On a hash mismatch the buffer is dropped and the transfer reported as failed.
#[derive(Default)]
struct SessionReceiver {
    name: Option<String>,
    total: u64,
    hash: u32,
    buf: Vec<u8>,
    written: bool,
}

/// Returns a line to print when the message was a session frame, or None to keep processing.
fn handle_session(state: &mut SessionReceiver, msg: Msg) -> Option<String> {
    match msg {
        Msg::SessionOffer { name, total, hash, .. } => {
            let name = String::from_utf8_lossy(&name).to_string();
            if total > 512 * 1024 * 1024 {
                return Some(format!("refusing a {total}-byte session payload as implausible"));
            }
            state.name = Some(name.clone());
            state.total = total;
            state.hash = hash;
            state.buf = Vec::with_capacity(total as usize);
            state.written = false;
            Some(format!(
                "session offer: {name}, {:.2} MB, hash {hash:#010X}",
                total as f64 / 1_048_576.0
            ))
        }
        Msg::SessionChunk { offset, bytes } => {
            let Some(_) = state.name else {
                return Some("session chunk before an offer; ignoring".into());
            };
            // Offsets must arrive in order. A gap means a lost frame, and guessing would produce a
            // corrupt save, so the transfer is abandoned rather than patched up.
            if offset != state.buf.len() as u64 {
                state.name = None;
                state.buf.clear();
                return Some(format!(
                    "session chunk at {offset} but {} bytes held; transfer abandoned",
                    state.buf.len()
                ));
            }
            state.buf.extend_from_slice(&bytes);
            None
        }
        Msg::SessionReady { total, hash } => {
            let Some(name) = state.name.clone() else {
                return Some("session ready with no offer; ignoring".into());
            };
            if state.buf.len() as u64 != total {
                state.name = None;
                return Some(format!(
                    "session incomplete: {} of {total} bytes; not written",
                    state.buf.len()
                ));
            }
            let got = fnv1a32(&state.buf);
            if got != hash {
                state.name = None;
                state.buf.clear();
                return Some(format!(
                    "session hash mismatch (got {got:#010X}, want {hash:#010X}); not written"
                ));
            }
            if state.written {
                return Some("session already written; ignoring duplicate".into());
            }
            let dir = session_dir();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Some(format!("could not create {dir}: {e}"));
            }
            let path = std::path::Path::new(&dir).join(&name);
            match std::fs::write(&path, &state.buf) {
                Ok(()) => {
                    state.written = true;
                    Some(format!(
                        "session saved to {} ({} bytes, hash verified)",
                        path.display(),
                        state.buf.len()
                    ))
                }
                Err(e) => Some(format!("could not write {}: {e}", path.display())),
            }
        }
        _ => None,
    }
}

/// Where a received session is written: the game's own SaveGames folder.
fn session_dir() -> String {
    if let Ok(dir) = std::env::var("COOP_SAVE_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
    match std::env::var("LOCALAPPDATA") {
        Ok(local) => format!(r"{local}\F1Manager24\Saved\SaveGames"),
        Err(_) => ".".to_string(),
    }
}

/// Keeps the newest snapshot on disk so another tool - and a human - can inspect what the client
/// actually holds, without needing a debugger attached.
fn write_shadow(tick: u32, race_time: f32, cars: &[Car]) -> Result<(), String> {
    let mut text = String::with_capacity(cars.len() * 48 + 64);
    text.push_str(&format!("TICK {tick} RACE_TIME {race_time:.3} CARS {}\n", cars.len()));
    for c in cars {
        text.push_str(&format!(
            "car {:2} dist {:10.2} spd {:6.2} lap {:2} pos {:2} tyre {:.2}\n",
            c.id, c.dist, c.spd, c.lap, c.pos, c.tyre_lo
        ));
    }
    // Written beside the binary, not in the working directory. Started through a remote agent the
    // cwd is arbitrary (the agent's own folder), which made the file effectively unfindable.
    let path = shadow_path();
    std::fs::write(&path, text).map_err(|e| format!("shadow {}: {e}", path.display()))
}

/// Where the newest received snapshot is written. `COOP_SHADOW` overrides; the default is the
/// directory holding this executable, so it always lands next to the tool that produced it.
fn shadow_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("COOP_SHADOW") {
        return std::path::PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("coop_shadow.txt")))
        .unwrap_or_else(|| std::path::PathBuf::from("coop_shadow.txt"))
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    /// The exact failure that crashed a live client: the host clock stops advancing but frames keep
    /// arriving, and the client must refuse to write. Before the fix it wrote 4,315 times into an
    /// object the game had released and the game faulted.
    #[test]
    fn a_frozen_host_clock_stops_writes() {
        let mut s = Staleness::default();

        // A live session never trips the guard, however long it runs.
        let mut clock = 0.0f32;
        for _ in 0..10_000 {
            clock += 0.05;
            assert!(!s.observe(clock), "a live clock must never look stale");
        }

        // The clock freezes. It must be tolerated briefly, then reported as over.
        let mut tripped_at = None;
        for n in 0..200 {
            if s.observe(clock) {
                tripped_at = Some(n);
                break;
            }
        }
        let n = tripped_at.expect("a frozen clock must eventually be treated as session over");
        assert!(
            n as u32 >= STALE_FRAMES,
            "must tolerate at least STALE_FRAMES ({STALE_FRAMES}) stalled frames, tripped at {n}"
        );
        assert!(
            n as u32 <= STALE_FRAMES + 1,
            "must trip promptly once the budget is spent, tripped at {n}"
        );
    }

    /// A clock that resumes must clear the guard, or a red flag would disable sync permanently.
    #[test]
    fn a_resumed_clock_clears_the_stall() {
        let mut s = Staleness::default();
        let mut clock = 0.0f32;
        for _ in 0..100 {
            clock += 0.05;
            assert!(!s.observe(clock));
        }
        // The clock freezes: tolerated for a while, then reported as over.
        let mut tripped = false;
        for _ in 0..(STALE_FRAMES + 5) {
            tripped |= s.observe(clock);
        }
        assert!(tripped, "a stalled clock must eventually read as session over");

        // Recovery: the guard must clear as soon as the clock advances again, otherwise a red flag
        // or a pause would disable sync for the rest of the race.
        for _ in 0..10 {
            clock += 0.05;
            assert!(!s.observe(clock), "an advancing clock must clear the stall");
        }
    }

    /// The bug that paused writes at the start line: at a race start the host clock is legitimately
    /// 0.0 for many frames. A fixed clock before any motion means "not started", not "over".
    #[test]
    fn a_grid_holding_at_zero_is_not_a_dead_session() {
        let mut s = Staleness::default();
        for _ in 0..500 {
            assert!(
                !s.observe(0.0),
                "a session that has not started must never be declared over"
            );
        }
    }

    /// The same case through the tick path, which is what the client actually uses.
    #[test]
    fn an_advancing_tick_never_looks_dead() {
        let mut s = Staleness::default();
        let mut tick = 0u32;
        for _ in 0..1000 {
            tick += 1;
            assert!(!s.observe_tick(tick, 0.0));
        }
    }

    /// A repeated tick with a frozen clock, after motion was seen, is a finished session.
    #[test]
    fn a_repeated_tick_after_motion_is_a_dead_session() {
        let mut s = Staleness::default();
        let mut tick = 0u32;
        // Session runs.
        for i in 0..100 {
            tick += 1;
            assert!(!s.observe_tick(tick, i as f32));
        }
        // Host stops sending new ticks but keeps re-sending the last one.
        let mut tripped = false;
        for _ in 0..(STALE_FRAMES + 5) {
            tripped |= s.observe_tick(tick, 99.0);
        }
        assert!(tripped, "a frozen tick after motion must be treated as over");
    }
    #[test]
    fn the_stall_is_reported_once() {
        let mut s = Staleness::default();
        let clock = 12.0f32;
        s.observe(clock);
        assert!(s.should_report());
        for _ in 0..100 {
            assert!(!s.should_report(), "must not repeat the message every frame");
        }
    }

    /// A denied write must never be retried forever: that is what damaged freed memory.
    #[test]
    fn repeated_write_failures_abandon_the_address() {
        let mut f = WriteFailures::default();
        for _ in 0..(WRITE_FAIL_LIMIT - 1) {
            assert!(!f.failed(), "must tolerate a brief transient failure");
        }
        assert!(f.failed(), "must give up the address at WRITE_FAIL_LIMIT");
        // After giving up, the count restarts rather than firing every frame.
        assert!(!f.failed(), "counter must reset after abandoning the address");
    }

    #[test]
    fn one_success_clears_the_failure_count() {
        let mut f = WriteFailures::default();
        f.failed();
        f.failed();
        f.succeeded();
        assert!(!f.failed(), "a success must reset the consecutive count");
        assert!(!f.failed());
        assert!(f.failed(), "and the limit must still be reachable afterwards");
    }
}
