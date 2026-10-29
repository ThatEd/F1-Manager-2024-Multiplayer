//! The standalone relay.
//!
//! The relay itself is the library (`coop-supervisor`). This binary is the same thing in a form you can run on
//! its own - useful for testing, and for putting the relay on a third machine so neither player has to host it.
//!
//! A two-machine session does **not** need this: the co-op agent embeds the library and binds the relay inside
//! the game process. Keeping the binary is what lets the relay live somewhere other than a game, not something
//! a player has to start.

use std::io;
use std::net::TcpListener;

use coop_supervisor::{serve, DEFAULT_PORT};

// The selftest below exercises the relay end to end, so it needs the library's internals. Naming them
// explicitly is what keeps the relay itself in one place instead of a copy per binary.
use coop_supervisor::{drain_peer, log, Agents, Coordination, Coord, Registry, Stats};

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--discover") {
        let pid = args
            .iter()
            .position(|a| a == "--pid")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--discover needs --pid <pid>"))?;
        let watch = args.iter().any(|a| a == "--watch");
        let out = args
            .iter()
            .position(|a| a == "--out")
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("coop_rsm.txt"));
        return run_discovery(pid, watch, &out);
    }

    if args.iter().any(|a| a == "--selftest") {
        return selftest();
    }

    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);

    /* Default `0.0.0.0`, because a relay on loopback only serves the machine it runs on - it would work
     * locally and silently refuse the other player, which reads as "no sync" rather than as a misconfigured
     * bind. `--bind 127.0.0.1` stays available for a single-machine test. */
    let bind = args
        .iter()
        .position(|a| a == "--bind")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "0.0.0.0".to_string());

    let relay = serve(&bind, port)?;
    coop_supervisor::log(&format!("listening on {}", relay.addr));
    // Park: the accept loop runs on its own thread.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use coop_proto::{decode, encode, Msg, Side};
fn selftest() -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    log(&format!("selftest: relay on {addr}"));
    let reg: Registry = Arc::new(Mutex::new(Vec::new()));
    let agents: Agents = Arc::new(Mutex::new(Vec::new()));
    // Two voters: host and client. `VoteBox` clamps to at least one, so a solo session still resolves.
    let coord: Coord = Arc::new(Mutex::new(Coordination::new(2)));
    let stats = Arc::new(Stats::default());

    {
        let reg = Arc::clone(&reg);
        let agents = Arc::clone(&agents);
        let stats = Arc::clone(&stats);
        let coord = Arc::clone(&coord);
        thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(stream) = incoming else { continue };
                let Ok(a) = stream.peer_addr() else { continue };
                stream.set_nodelay(true).ok();
                let reg = Arc::clone(&reg);
                let agents = Arc::clone(&agents);
                let stats = Arc::clone(&stats);
                let coord = Arc::clone(&coord);
                thread::spawn(move || {
                    let _ = drain_peer(reg, agents, coord, stream, a, stats);
                });
            }
        });
    }

    let hello = |side: Side| -> io::Result<TcpStream> {
        let mut s = TcpStream::connect(addr)?;
        s.set_nodelay(true)?;
        s.write_all(&encode(&Msg::Hello { side, cars_owned: 11 }))?;
        Ok(s)
    };

    let mut host = hello(Side::Host)?;
    let mut client = hello(Side::Client)?;
    thread::sleep(Duration::from_millis(250));

    let cars: Vec<coop_proto::Car> = (0..22u8)
        .map(|i| coop_proto::Car { id: i, dist: 1000.0 + i as f32, spd: 50.0, ..Default::default() })
        .collect();
    let snap = Msg::Snapshot { tick: 7, sim_time: 12.5, race_time: 12.5, cars };
    let frame = encode(&snap);
    host.write_all(&frame)?;
    host.flush()?;

    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut got = vec![0u8; frame.len()];
    client.read_exact(&mut got)?;

    if got != frame {
        log("selftest FAILED: relayed frame differs from the one sent");
        std::process::exit(1);
    }
    match decode(&got) {
        Ok(Msg::Snapshot { tick: 7, sim_time: _, cars, race_time }) => {
            log(&format!(
                "selftest: client received snapshot tick=7 race_time={race_time} cars={}",
                cars.len()
            ));
        }
        other => {
            log(&format!("selftest FAILED: unexpected message {other:?}"));
            std::process::exit(1);
        }
    }
    log(&format!(
        "selftest OK: frames_in={} frames_out={} bytes_in={} bad={}",
        stats.frames_in.load(Ordering::Relaxed),
        stats.frames_out.load(Ordering::Relaxed),
        stats.bytes_in.load(Ordering::Relaxed),
        stats.bad_frames.load(Ordering::Relaxed),
    ));
    Ok(())
}

/// Locates the live `RaceSimManager` in `pid` and publishes `RSM <addr> <size> <unix>`.
///
/// The address changes every session (and the class-default object sits at a stable-looking but
/// useless address), so nothing may cache it. When watching, a cheap 4-byte read validates the
/// cached address each pass and a full object-array scan only runs when that read fails, which
/// keeps the cost off the game's CPU.
fn run_discovery(pid: u32, watch: bool, out: &std::path::Path) -> io::Result<()> {
    use coop_proto::layout;
    let size = layout::OBJECT_SIZE;
    let mut current: Option<usize> = None;
    loop {
        let mut publish = false;
        let mut note = None;
        match coop_mem::Process::open(pid) {
            Err(e) => {
                note = Some(format!("open pid {pid}: {e}"));
                current = None;
            }
            Ok(proc) => {
                let still_live = current.map(|rsm| {
                    proc.read(rsm + layout::CAR_BASE + layout::off::DIST, 4).is_ok()
                }).unwrap_or(false);
                if still_live {
                    publish = true;
                } else {
                    current = None;
                    let base = proc
                        .module_base("F1Manager24.exe")
                        .map_err(|e| io::Error::other(e.to_string()));
                    match base.and_then(|b| {
                        coop_mem::objects::find_sim(&proc, b).map_err(io::Error::other)
                    }) {
                        Ok(rsm) => {
                            note = Some(format!("discovered RaceSimManager at {rsm:#X}"));
                            current = Some(rsm);
                            publish = true;
                        }
                        Err(e) => note = Some(format!("discovery: {e}")),
                    }
                }
            }
        }
        if let Some(rsm) = current.filter(|_| publish) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            std::fs::write(out, format!("RSM {rsm:#X} {size:#X} {now}\n"))?;
        }
        if let Some(n) = note {
            log(&n);
        }
        if !watch {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}


