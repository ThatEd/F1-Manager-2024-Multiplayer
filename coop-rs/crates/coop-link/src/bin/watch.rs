//! Watches the live game for a save load, using small targeted reads.
//!
//! Reading the whole 10.5 MB RaceSimManager once is fine for a snapshot and unacceptable at 20 Hz for
//! minutes. Detecting a load only needs four numbers: the sim clock, the race-control state, the
//! flags word, and car 0's distance. Those are ~20 bytes at fixed offsets, so this can run for as long
//! as needed without the game noticing.
//!
//! A load shows up as a discontinuity: the object is replaced (every read fails), or the sim clock
//! jumps backwards / to zero, or the car-0 distance resets. Each transition is printed with what
//! changed, so the log can be read afterwards to see exactly what the game did.

use coop_proto::layout;
use coop_mem::Process;
use std::io::Write;
use std::time::{Duration, Instant};

/// Prints a line AND flushes it.
///
/// Rust's stdout is line-buffered to a terminal but **block**-buffered to a file or pipe. A watcher
/// run with its output redirected therefore appears to log nothing at all: the lines sit in an
/// 8 KB buffer that never fills while the process waits for a rare event. That is exactly what
/// happened the first time this tool was used to observe a save load, and the observation was lost.
/// `macro_rules!` so the call sites read like `println!` and cannot drift back to it.
macro_rules! say {
    ($($arg:tt)*) => {{
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let secs_today = now % 86_400;
        println!(
            "[{:02}:{:02}:{:02}] {}",
            secs_today / 3600,
            (secs_today % 3600) / 60,
            secs_today % 60,
            format!($($arg)*)
        );
        let _ = std::io::stdout().flush();
    }};
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pid: u32 = args
        .iter()
        .position(|a| a == "--pid")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("coop-watch --pid <pid> [--hz N] [--minutes N]");
            std::process::exit(2);
        });
    let hz: u32 = args
        .iter()
        .position(|a| a == "--hz")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let minutes: u32 = args
        .iter()
        .position(|a| a == "--minutes")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    let proc = match Process::open(pid) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[watch] cannot open pid {pid}: {e}");
            std::process::exit(1);
        }
    };
    say!("[watch] pid {pid} at {hz} Hz for {minutes} min");

    let period = Duration::from_millis(1000 / hz.max(1) as u64);
    let deadline = Instant::now() + Duration::from_secs(minutes as u64 * 60);
    let mut rsm: usize = 0;
    let mut last: Option<Sample> = None;
    let mut loads = 0u32;
    let mut rescan_reason = String::new();
    let mut last_print = Instant::now();
    // Full discovery reads every writable region of a 500 MB image, so it must not run at sample
    // rate. Back off while there is nothing live and reset as soon as something is found.
    let mut miss: u32 = 0;

    while Instant::now() < deadline {
        // Rediscovery is a ~100k-object scan, so it only runs when the cached address stops working.
        if rsm == 0 || !readable(&proc, rsm) {
            match discover(&proc) {
                Ok(found) => {
                    if rsm != 0 {
                        say!(
                            "[watch] object replaced: 0x{rsm:X} -> 0x{found:X}   ({rescan_reason})"
                        );
                        loads += 1;
                    } else if reason_is_live(&rescan_reason) {
                        say!("[watch] live RaceSimManager 0x{found:X}   ({rescan_reason})");
                    } else if found != 0 {
                        say!("[watch] RaceSimManager 0x{found:X}");
                    }
                    rsm = found;
                    last = None;
                }
                Err(e) => {
                    // Menu, loading screen, or being torn down. Not an error worth spamming, and the
                    // scan itself is expensive, so wait longer after each consecutive miss.
                    if last_print.elapsed() >= Duration::from_secs(5) {
                        say!("[watch] no live object ({e})");
                        last_print = Instant::now();
                    }
                    rsm = 0;
                    last = None;
                    miss = miss.saturating_add(1);
                    let backoff = Duration::from_secs(1 + (miss.min(6) as u64));
                    std::thread::sleep(backoff);
                    continue;
                }
            }
            rescan_reason.clear();
            miss = 0;
            std::thread::sleep(period);
            continue;
        }

        let sample = match Sample::read(&proc, rsm) {
            Some(s) => s,
            None => {
                rescan_reason = "a small read failed".to_string();
                rsm = 0;
                std::thread::sleep(period);
                continue;
            }
        };

        if let Some(prev) = last {
            if let Some(why) = prev.discontinuity(&sample) {
                say!(
                    "[watch] discontinuity: {why}\n           before {}\n           after  {}",
                    prev, sample
                );
                report_save_activity();
                loads += 1;
            }
        }
        last = Some(sample);

        // Print unconditionally on a slow cadence: silence must mean "not running", never "nothing
        // changed". Two earlier attempts to observe a save load produced no data because output only
        // happened on a transition that did not occur.
        if last_print.elapsed() >= Duration::from_secs(3) {
            say!("[watch] {sample}");
            last_print = Instant::now();
        }
        std::thread::sleep(period);
    }

    say!("[watch] finished, {loads} discontinuity/discovery event(s)");
}

fn reason_is_live(_r: &str) -> bool {
    false
}

/// Prints the game's save files, so a sim transition can be tied to what was written and when.
fn report_save_activity() {
    let dir = std::env::var("LOCALAPPDATA")
        .map(|d| format!(r"{d}\F1Manager24\Saved\SaveGames"))
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        say!("           (no save directory at {dir})");
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "sav").unwrap_or(false))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let modified = m.modified().ok()?;
            let age = std::time::SystemTime::now().duration_since(modified).ok()?;
            Some((age, e.file_name().to_string_lossy().to_string(), m.len()))
        })
        .collect();
    files.sort_by_key(|(age, _, _)| *age);
    for (age, name, len) in files.into_iter().take(3) {
        say!(
            "           save {name} {len} bytes, written {}s ago",
            age.as_secs()
        );
    }
}

fn discover(proc: &Process) -> Result<usize, String> {
    let base = proc
        .module_base("F1Manager24.exe")
        .map_err(|e| format!("module base: {e}"))?;
    coop_mem::objects::find_sim(proc, base)
}

/// A handful of fields that are enough to tell one session state from another.
#[derive(Clone, Copy)]
struct Sample {
    sim: f32,
    playback: f32,
    flags: u32,
    rc: u8,
    dist: f32,
    spd: f32,
}

impl Sample {
    fn read(proc: &Process, rsm: usize) -> Option<Self> {
        let f32_at = |off: usize| -> Option<f32> {
            proc.read(rsm + off, 4)
                .ok()
                .map(|b| f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
        };
        let u32_at = |off: usize| -> Option<u32> {
            proc.read(rsm + off, 4)
                .ok()
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let u8_at = |off: usize| -> Option<u8> { proc.read(rsm + off, 1).ok().map(|b| b[0]) };

        Some(Self {
            sim: f32_at(layout::off::SIM_TIME)?,
            playback: f32_at(layout::off::PLAYBACK)?,
            flags: u32_at(layout::off::FLAGS)?,
            rc: u8_at(layout::off::RC_STATE)?,
            dist: f32_at(layout::CAR_BASE + layout::off::DIST)?,
            spd: f32_at(layout::CAR_BASE + layout::off::SPD)?,
        })
    }

    /// Returns why this sample cannot be a continuation of `self`.
    fn discontinuity(&self, next: &Sample) -> Option<String> {
        if next.sim < self.sim - 0.001 {
            return Some(format!("sim clock went backwards {:.3} -> {:.3}", self.sim, next.sim));
        }
        if self.sim > 1.0 && next.sim < 0.001 {
            return Some(format!("sim clock reset to zero (was {:.3})", self.sim));
        }
        if self.dist > 1.0 && next.dist < 0.001 {
            return Some(format!("car 0 distance reset to zero (was {:.1})", self.dist));
        }
        if next.rc != self.rc {
            return Some(format!("race control {} -> {}", self.rc, next.rc));
        }
        if next.flags != self.flags {
            return Some(format!("flags {:#X} -> {:#X}", self.flags, next.flags));
        }
        None
    }
}

impl std::fmt::Display for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sim {:9.3}  playback {:6.3}  flags {:#06X}  rc {}  car0 dist {:10.1} spd {:6.1}",
            self.sim, self.playback, self.flags, self.rc, self.dist, self.spd
        )
    }
}

fn readable(proc: &Process, rsm: usize) -> bool {
    proc.read(rsm + layout::CAR_BASE + layout::off::DIST, 4).is_ok()
}
