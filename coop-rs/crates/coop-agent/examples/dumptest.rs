//! Reproduces a minidump write on its own, so a fault in it can be attributed precisely.
//!
//! Run with `cargo run --offline -p coop-agent --example dumptest`. Prints where the dump landed, or
//! the Win32 error. On a fault, `coop_agent.trace` in `COOP_CRASH_DIR` (or next to the exe) records
//! the faulting `rip`, because the crash handler is installed before anything else.

fn main() {
    let dir = std::env::temp_dir().join("coop_dumptest");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    std::env::set_var("COOP_CRASH_DIR", &dir);

    coop_agent::crash::install();
    println!("evidence dir {}", dir.display());

    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    if mode != "all" {
        let kind = match mode.as_str() {
            "plain" => 0u32,
            _ => 0x1 | 0x20 | 0x1000,
        };
        match coop_agent::crash::write_dump_custom("mode test", kind) {
            Ok(p) => println!(
                "OK {mode}: {} ({} bytes)",
                p.display(),
                std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)
            ),
            Err(e) => println!("FAILED {mode}: {e}"),
        }
        return;
    }

    println!("-- plain --");
    match coop_agent::crash::write_dump_plain("dumptest plain") {
        Ok(path) => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("OK {} ({size} bytes)", path.display());
        }
        Err(e) => println!("FAILED {e}"),
    }

    println!("-- rich --");
    match coop_agent::crash::write_dump("dumptest rich", std::ptr::null_mut()) {
        Ok(path) => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            println!("OK {} ({size} bytes)", path.display());
        }
        Err(e) => println!("FAILED {e}"),
    }

    println!("---- trace ----");
    match std::fs::read_to_string(dir.join("coop_agent.trace")) {
        Ok(t) => print!("{t}"),
        Err(e) => println!("no trace: {e}"),
    }
}
