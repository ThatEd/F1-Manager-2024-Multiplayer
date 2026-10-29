//! Writes one row into every table, one at a time, pausing between each, until something breaks.
//!
//! The point is a controlled experiment rather than a big-bang transplant. Every previous attempt
//! changed many things at once and I drew conclusions from single observations; this touches one table
//! per iteration, logs the outcome, and stops the moment a table fails so the failing table is
//! unambiguous.
//!
//! It uses `LiveDb`, the same implementation the real tools use, so whatever this finds applies to them.

use coop_mem::livedb::LiveDb;

fn usage() -> ! {
    eprintln!("coop-sweep --pid <pid> --snapshot <json> [--pause-ms <n>] [--limit <n>] [--dry-run]");
    eprintln!();
    eprintln!("Inserts ONE row into each table from the snapshot, pausing between tables.");
    eprintln!("Stops at the first table whose write fails, so the cause is unambiguous.");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |n: &str| -> Option<String> {
        args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned()
    };
    let pid: u32 = match get("--pid").and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => usage(),
    };
    let path = get("--snapshot").unwrap_or_else(|| usage());
    let pause = get("--pause-ms").and_then(|s| s.parse().ok()).unwrap_or(2000u64);
    let limit = get("--limit").and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let dry = args.iter().any(|a| a == "--dry-run");

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    let snapshot = match coop_mem::binutil::parse_snapshot(&raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot parse {path}: {e}");
            std::process::exit(1);
        }
    };

    let db = match LiveDb::attach(pid) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("attach failed: {e}");
            std::process::exit(1);
        }
    };
    println!("attached: exec 0x{:X}, db 0x{:X}", db.exec, db.db);

    // Only tables the game actually has can be written.
    let existing: std::collections::HashSet<String> = match db.sql("SELECT name FROM sqlite_master WHERE type='table'") {
        Ok(rows) => rows.into_iter().filter_map(|r| r.into_iter().next()).collect(),
        Err(e) => {
            eprintln!("cannot list tables: {e}");
            std::process::exit(1);
        }
    };

    let mut names: Vec<&String> = snapshot
        .iter()
        .filter(|(n, rows)| !rows.is_empty() && !is_protected(n) && existing.contains(*n))
        .map(|(n, _)| n)
        .collect();
    names.sort();
    println!("{} tables to try, one row each, {} ms apart", names.len().min(limit), pause);

    let mut ok = 0usize;
    let mut failed = Vec::new();

    for (i, table) in names.iter().take(limit).enumerate() {
        let rows = &snapshot[*table];
        let cols = match columns(&db, table) {
            Ok(c) if !c.is_empty() => c,
            _ => {
                println!("  {}/{ } {table}: SKIP (no columns)", i + 1, names.len());
                continue;
            }
        };
        let row = &rows[0];
        if row.len() != cols.len() {
            println!(
                "  {}/{} {table}: SKIP ({} values vs {} columns)",
                i + 1,
                names.len(),
                row.len(),
                cols.len()
            );
            continue;
        }

        let stmt = format!(
            "INSERT INTO \"{table}\" ({}) VALUES ({})",
            cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(","),
            row.iter().map(|v| quote(v)).collect::<Vec<_>>().join(",")
        );

        if dry {
            println!("  {}/{} {table}: would insert {} bytes", i + 1, names.len(), stmt.len());
            continue;
        }

        match db.sql(&stmt) {
            Ok(_) => {
                ok += 1;
                println!("  {}/{} {table}: ok", i + 1, names.len());
            }
            Err(e) => {
                // A constraint or SQL error is an ordinary failure: the game is fine and the sweep
                // should say exactly which table and why, then stop.
                println!("  {}/{} {table}: FAILED - {e}", i + 1, names.len());
                failed.push((table.clone(), e.clone()));
                if e.contains("worker") || e.contains("timed out") {
                    println!("  worker lost; stopping immediately");
                    break;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(pause));
    }

    println!("\nsummary: {ok} table(s) written, {} failure(s)", failed.len());
    for (t, e) in &failed {
        println!("  {t}: {e}");
    }
}

fn is_protected(name: &str) -> bool {
    name == "Player" || name.starts_with("Custom_") || name.starts_with("sqlite_")
}

fn columns(db: &LiveDb, table: &str) -> Result<Vec<String>, String> {
    db.sql(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .map(|rows| rows.into_iter().filter_map(|r| r.into_iter().next()).collect())
}

fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}
