//! `coop-probe`: read a live RaceSimManager from outside the game and print what we see.
//!
//! This is the read half of the split, exercised by hand before it is wired into the supervisor.
//!
//! ```text
//! coop-probe --pid 12345 --rsm 0x196FA0000
//! coop-probe --endpoint ..\coop_rsm.txt --frames 40
//! ```

use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use coop_mem::{read_endpoint, Process};

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         coop-probe --pid <pid> --rsm <hex addr> [--frames N] [--hz H]\n  \
         coop-probe --endpoint <file> [--pid <pid>] [--frames N] [--hz H]\n\n\
         --endpoint reads `RSM <addr> <size> <unix>` written by the in-process agent;\n\
         a file older than 5 s is refused because a stale address points into freed memory."
    );
    std::process::exit(2)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut pid: Option<u32> = None;
    let mut rsm: Option<usize> = None;
    let mut endpoint: Option<PathBuf> = None;
    let mut frames = 1u32;
    let mut gua: Option<usize> = None;
    let mut find_rsm = false;
    let mut scan_str: Option<String> = None;
    let mut hz = 40u32;

    let mut i = 0;
    while i < args.len() {
        let before = i;
        match args[i].as_str() {
            "--pid" => {
                pid = args.get(i + 1).and_then(|s| s.parse().ok());
                i += 2;
            }
            "--rsm" => {
                rsm = args
                    .get(i + 1)
                    .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok());
                i += 2;
            }
            "--endpoint" => {
                endpoint = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--frames" => {
                frames = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(1);
                i += 2;
            }
            "--hz" => {
                hz = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(40);
                i += 2;
            }
            "--discover" => {
                find_rsm = true;
                gua = None;
                i += 1;
            }
            "--gua" => {
                gua = args
                    .get(i + 1)
                    .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok());
                i += 2;
            }
            "--scanstr" => {
                scan_str = args.get(i + 1).cloned();
                i += 2;
            }
            "--find-rsm" => {
                find_rsm = true;
                i += 1;
            }
            "-h" | "--help" => usage(),
            other => {
                eprintln!("unknown argument: {other}");
                usage()
            }
        }
        if i == before {
            // An arm that forgets to advance `i` spins this loop forever and the process hangs
            // with no output at all, which is indistinguishable from a deadlock in the reader.
            eprintln!("argument {} did not advance the parser", args[i]);
            usage();
        }
    }

    if let Some(path) = &endpoint {
        match read_endpoint(path, 5) {
            Ok((a, _size)) => {
                println!("endpoint: RSM 0x{a:X}");
                rsm = Some(a);
            }
            Err(e) => {
                eprintln!("endpoint unusable: {e}");
                std::process::exit(1);
            }
        }
    }

    let pid = pid.unwrap_or_else(|| usage());

    // Find a literal string anywhere in the process and show what surrounds it. Used to locate
    // bindings that are keyed by name - a Gameface event handler keeps the event name, so the name is
    // a way in that needs no function-address guesswork.
    if let Some(needle) = scan_str {
        let proc = match Process::open(pid) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let base = proc.module_base("").unwrap_or(0);
        let (_, size) = proc.image_range(base).unwrap_or((base, 0x2000_0000));
        // Scan the whole user address space in broad strokes: the binding may live on the heap, not
        // in the image.
        let start = 0x1_0000usize;
        let end = 0x7FFF_FFFF_0000usize;
        let mut found = 0;
        for region in proc.regions(start, end - start) {
            let len = region.region_size;
            if len == 0 || len > 0x4000_0000 {
                continue;
            }
            let Ok(bytes) = proc.read(region.base_address, len) else { continue };
            let ascii = needle.as_bytes();
            let mut i = 0;
            while i + ascii.len() <= bytes.len() {
                if &bytes[i..i + ascii.len()] == ascii {
                    let addr = region.base_address + i;
                    let ctx_start = i.saturating_sub(32);
                    let ctx_end = (i + ascii.len() + 64).min(bytes.len());
                    println!(
                        "0x{addr:X} in region 0x{:X} (size 0x{len:X})",
                        region.base_address
                    );
                    println!("  ctx: {}", hex(&bytes[ctx_start..ctx_end]));
                    found += 1;
                    if found >= 40 {
                        break;
                    }
                }
                i += 1;
            }
            if found >= 40 {
                break;
            }
            let _ = size;
        }
        println!("{found} occurrence(s) of {needle:?}");
        return;
    }

    if find_rsm {
        eprintln!("[stage] opening pid {pid}");
        let proc = match Process::open(pid) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let gua = match gua {
            Some(a) => a,
            None => match { eprintln!("[stage] reading PEB"); proc.module_base("F1Manager24.exe") } {
                Ok(base) => {
                    let gua = base + coop_proto::layout::GUA_RVA;
                    println!("module base 0x{base:X} -> GUObjectArray 0x{gua:X}");
                    gua
                }
                Err(e) => {
                    eprintln!("module base lookup failed: {e}");
                    std::process::exit(1);
                }
            },
        };
        eprintln!("[stage] locating GUObjectArray (robust)");
        let base = match proc.image_range(0) {
            _ => {
                // `image_range(0)` cannot work on its own; the module base is what we need.
                match proc.module_base("F1Manager24.exe") {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("module base lookup failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
        };
        let _ = base;
        let module_base = proc.module_base("F1Manager24.exe").unwrap_or(0);
        let (gua_used, arr) = match coop_mem::objects::discover_anywhere(&proc, module_base) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("layout discovery failed: {e}");
                std::process::exit(1);
            }
        };
        eprintln!("[stage] GUObjectArray resolved at 0x{gua_used:X}");
        eprintln!("[stage] scanning {} objects for class size", arr.num_elements);
        let candidates = coop_mem::objects::find_all_by_class_size(&proc, &arr, coop_proto::layout::OBJECT_SIZE as i32);
        println!("{} candidate(s) with class size 0x{:X}", candidates.len(), coop_proto::layout::OBJECT_SIZE);
        match coop_mem::objects::pick_live(&proc, &candidates) {
            Some(obj) => {
                println!("RaceSimManager at 0x{obj:X}");
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let line = format!("RSM {obj:#X} {:#X} {now}\n", coop_proto::layout::OBJECT_SIZE);
                if let Err(e) = std::fs::write("coop_rsm.txt", line) {
                    eprintln!("could not write coop_rsm.txt: {e}");
                } else {
                    println!("wrote coop_rsm.txt");
                }
            }
            None => {
                eprintln!("could not identify a single live instance (CDO vs live); not guessing");
                std::process::exit(1);
            }
        }
        return;
    }

    let rsm = rsm.unwrap_or_else(|| usage());

    let proc = match Process::open(pid) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!(
        "attached to pid {pid}; reading {} bytes at 0x{rsm:X}",
        coop_proto::layout::object_len()
    );

    let period = Duration::from_millis((1000 / hz.max(1)) as u64);
    // One buffer, reused. Allocating 10 MB per frame was most of the measured cost, not the read.
    let mut buf = vec![0u8; coop_proto::layout::object_len()];
    let mut best = f64::MAX;
    let mut total = 0.0;
    for n in 0..frames {
        let t0 = std::time::Instant::now();
        match proc.read_into(rsm, &mut buf) {
            Ok(()) => {
                let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
                best = best.min(elapsed);
                total += elapsed;
                let cars = coop_proto::layout::cars(&buf);
                let rc = coop_proto::layout::rc(&buf);
                let c0 = cars.first();
                println!(
                    "frame {n}: {}/{} cars, {:.2} ms, rc={:?}, car0 dist={} spd={} tyre0={:.1}",
                    cars.len(),
                    coop_proto::layout::CAR_MAX,
                    elapsed,
                    rc,
                    c0.map(|c| format!("{:.1}", c.dist)).unwrap_or_else(|| "?".into()),
                    c0.map(|c| format!("{:.1}", c.spd)).unwrap_or_else(|| "?".into()),
                    c0.map(|c| c.tyres[0].condition).unwrap_or(-1.0),
                );
            }
            Err(e) => {
                eprintln!("frame {n}: {e}");
                if n == 0 {
                    std::process::exit(1);
                }
            }
        }
        if n + 1 < frames {
            sleep(period);
        }
    }
    if frames > 1 {
        println!(
            "timing: best {:.2} ms, mean {:.2} ms over {frames} frame(s)",
            best,
            total / frames as f64
        );
    }
}
