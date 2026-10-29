//! `coop-inject`: CLI over `coop-inject`'s library.
//!
//! Kept as a separate binary so existing scripts (and `remote.py`) keep working, but the work lives in the
//! library so `coop-link --inject` can do the same job without a second executable on either machine.

fn usage() -> ! {
    eprintln!(
        "usage:\n  coop-inject --pid <pid> --dll <path>\n  coop-inject --name <process name> --dll <path>\n\n\
         Nothing is written to game state by the injector; it only loads the agent DLL, which connects to \
         the supervisor and reports its pid.\n\n\
         The same job is available as `coop-link --inject <pid>` using the DLL beside it, which saves \
         shipping this executable at all."
    );
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };

    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
    }

    let dll = get("--dll").unwrap_or_else(|| usage());
    let pid = match (get("--pid").and_then(|s| s.parse::<u32>().ok()), get("--name")) {
        (Some(p), _) => p,
        (None, Some(n)) => coop_inject::pid_by_name(&n)
            .unwrap_or_else(|| {
                eprintln!("no running process named {n}");
                std::process::exit(1)
            }),
        (None, None) => usage(),
    };

    match coop_inject::inject(pid, &dll) {
        Ok(base) => println!("injected {dll} into pid {pid} (module base 0x{base:X})"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
