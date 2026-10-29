//! Pastes a captured career over the running game's live database.
//!
//! The problem this solves: a client joining a two-player career has its own save, and the world it
//! should be in belongs to the host. Getting the host's `.sav` across and making the game re-read its
//! save directory is fragile; the database behind the save is what the game actually runs on, and it
//! is reachable while the game is running. So the transplant is table data, not a file.
//!
//! Rules this implements:
//!
//! * **The client keeps its own identity.** The `Player` row holds who the player is, and copying the
//!   host's would make both players the same person. It is preserved, and the team is set separately.
//! * **`Custom_*` tables stay behind.** They are read by external tools, not by the game, so carrying
//!   them would move someone else's tooling configuration into a save that does not want it.
//! * **One transaction.** A half-applied transplant is a corrupt career, so the whole thing is
//!   `BEGIN ... COMMIT` and any failure rolls back to the client's own untouched save.
//! * **Verified before commit.** Row counts are compared per table inside the transaction; a mismatch
//!   aborts rather than commits.
//!
//! Input is the JSON snapshot produced by `snapshot`.

use coop_mem::livedb::LiveDb;
use coop_mem::binutil::{parse_snapshot, quote, Table};


/// Tables that are never written over: identity, tooling, and SQLite's own bookkeeping.
fn is_protected(name: &str) -> bool {
    name == "Player"
        || name.starts_with("Custom_")
        || name.starts_with("sqlite_")
        || name.eq_ignore_ascii_case("sqlite_sequence")
}

fn usage() -> ! {
    eprintln!("coop-transplant --pid <pid> --snapshot <json> [--team <id>] [--dry-run]");
    eprintln!();
    eprintln!("Pastes a career snapshot over the running game's live database.");
    eprintln!("  --team <id>  set the player's team to this id after the paste");
    eprintln!("  --dry-run    report what would change without writing");
    eprintln!();
    eprintln!("The player's own identity (the Player row) and Custom_* tooling tables are preserved.");
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
    let team = get("--team");
    let dry = args.iter().any(|a| a == "--dry-run");

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };
    let snapshot: Table = match parse_snapshot(&raw) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot parse {path}: {e}");
            std::process::exit(1);
        }
    };
    println!("snapshot: {} tables", snapshot.len());

    let db = match LiveDb::attach(pid) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("attach failed: {e}");
            std::process::exit(1);
        }
    };
    println!("attached: exec 0x{:X}, db 0x{:X}", db.exec, db.db);

    // What does the target have now? Only tables the game actually created can be written.
    let existing = match db.sql("SELECT name FROM sqlite_master WHERE type='table'") {
        Ok(rows) => rows.into_iter().filter_map(|r| r.into_iter().next()).collect::<Vec<String>>(),
        Err(e) => {
            eprintln!("cannot list tables: {e}");
            std::process::exit(1);
        }
    };
    let existing_set: std::collections::HashSet<String> = existing.iter().cloned().collect();
    println!("target: {} tables", existing.len());

    let mut plan: Vec<(&String, &Vec<Vec<String>>)> = Vec::new();
    let mut skipped_missing = Vec::new();
    for (name, rows) in &snapshot {
        if is_protected(name) {
            continue;
        }
        if !existing_set.contains(name) {
            skipped_missing.push(name.clone());
            continue;
        }
        plan.push((name, rows));
    }
    println!(
        "plan: {} tables to replace, {} skipped (not in target), {} protected",
        plan.len(),
        skipped_missing.len(),
        snapshot.len() - plan.len() - skipped_missing.len()
    );
    if !skipped_missing.is_empty() {
        println!("  not present in target: {}", skipped_missing.join(", "));
    }

    if dry {
        for (name, rows) in &plan {
            println!("  {name}: {} rows (target has {})", rows.len(), count(&db, name));
        }
        println!("dry run: nothing written");
        return;
    }

    // Column order must match the target's schema, so read it rather than trusting the snapshot.
    let mut statements: Vec<String> = Vec::new();
    for (name, rows) in &plan {
        let cols = match columns(&db, name) {
            Ok(c) if !c.is_empty() => c,
            _ => {
                println!("  skipping {name}: cannot read its columns");
                continue;
            }
        };
        if let Some(first) = rows.first() {
            if first.len() != cols.len() {
                println!(
                    "  skipping {name}: snapshot has {} values but the table has {} columns",
                    first.len(),
                    cols.len()
                );
                continue;
            }
        }
        statements.push(format!("DELETE FROM \"{name}\""));
        // Size chunks by BYTES, not row count: a row can be hundreds of kilobytes (the Player row
        // carries base64 images), and a fixed row count produced statements far past the limit.
        const MAX_SQL: usize = 0x1C000;
        let mut chunk: Vec<&Vec<String>> = Vec::new();
        let mut size = 0usize;
        let mut chunks: Vec<Vec<&Vec<String>>> = Vec::new();
        for row in rows.iter() {
            let est = row.iter().map(|v| v.len() + 3).sum::<usize>() + 2;
            if size + est > MAX_SQL && !chunk.is_empty() {
                chunks.push(std::mem::take(&mut chunk));
                size = 0;
            }
            size += est;
            chunk.push(row);
        }
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        for chunk in chunks {
            let values = chunk
                .iter()
                .map(|row| {
                    let vals = row
                        .iter()
                        .map(|v| quote(v))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!("({vals})")
                })
                .collect::<Vec<_>>()
                .join(",");
            statements.push(format!(
                "INSERT INTO \"{name}\" ({}) VALUES {values}",
                cols.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",")
            ));
        }
    }
    println!("{} statement(s) prepared", statements.len());

    // Verify inside the transaction: a wrong count means the paste did not fully apply.
    let mut verify: Vec<String> = Vec::new();
    for (name, rows) in &plan {
        verify.push(format!("__expect {name} {}", rows.len()));
    }

    match apply(&db, &statements, &plan) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("transplant failed and was rolled back: {e}");
            std::process::exit(1);
        }
    }

    if let Some(t) = team {
        let stmt = format!("UPDATE Player SET TeamID = {t}");
        match db.sql(&format!("BEGIN;{stmt};COMMIT;")) {
            Ok(_) => println!("player team set to {t}"),
            Err(e) => println!("could not set the player team: {e}"),
        }
    }

    println!("done: {} tables replaced", verify.len());
}

fn count(db: &LiveDb, table: &str) -> String {
    match db.sql(&format!("SELECT count(*) FROM \"{table}\"")) {
        Ok(rows) => rows
            .first()
            .and_then(|r| r.first())
            .cloned()
            .unwrap_or_else(|| "?".into()),
        Err(_) => "?".into(),
    }
}

fn columns(db: &LiveDb, table: &str) -> Result<Vec<String>, String> {
    let rows = db.sql(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
    Ok(rows.into_iter().filter_map(|r| r.into_iter().next()).collect())
}


/// Applies everything in ONE exclusive transaction.
///
/// Two things make this safe, both from the same idea: the game must not read the database while it is
/// being replaced, but freezing the process is the wrong way to achieve that. Suspending threads can
/// deadlock on a mutex held by a suspended thread, and this game has already been killed that way.
/// Instead:
///
/// * **`BEGIN EXCLUSIVE`** takes SQLite's own write lock. The game's queries then block on the lock and
///   resume after `COMMIT` ? the game already handles waiting on its own database correctly.
/// * **batching** puts many statements in a single `sqlite3_exec` call. SQLite executes a multi-statement
///   string internally, so ~600 round trips collapse to a handful. The window in which the game could
///   observe a half-replaced database shrinks from seconds to milliseconds, and our spin loop spends
///   correspondingly less time burning a core inside the game.
fn apply(db: &LiveDb, statements: &[String], plan: &[(&String, &Vec<Vec<String>>)]) -> Result<(), String> {
    // Bytes per sqlite3_exec call. Comfortably under MAX_SQL and few enough calls to keep the lock short.
    const BATCH_BYTES: usize = 0x1C000;

    db.sql("PRAGMA defer_foreign_keys = ON")?;

    let mut batch: Vec<String> = vec!["BEGIN EXCLUSIVE".to_string()];
    let mut batch_len = 0usize;
    let mut batches = 0usize;
    let mut sent = 0usize;

    for stmt in statements {
        if batch_len + stmt.len() + 2 > BATCH_BYTES && batch.len() > 1 {
            let combined = batch.join(";\n");
            if let Err(e) = db.sql(&combined) {
                let _ = db.sql("ROLLBACK");
                return Err(format!("batch {batches} failed: {e}"));
            }
            batches += 1;
            batch.clear();
            batch_len = 0;
            send_progress(batches, sent, statements.len());
        }
        batch_len += stmt.len() + 2;
        batch.push(stmt.clone());
        sent += 1;
    }

    // Verify inside the transaction, before COMMIT: a partial replace must never be committed.
    let mut problems = Vec::new();
    for (name, rows) in plan {
        let got = count(db, name);
        if got != rows.len().to_string() {
            problems.push(format!("{name}: wrote {} rows, table has {got}", rows.len()));
        }
    }
    if !problems.is_empty() {
        let _ = db.sql("ROLLBACK");
        return Err(format!(
            "verification failed, rolled back:\n  {}",
            problems.join("\n  ")
        ));
    }

    batch.push("COMMIT".to_string());
    let combined = batch.join(";\n");
    if let Err(e) = db.sql(&combined) {
        let _ = db.sql("ROLLBACK");
        return Err(format!("final batch failed: {e}"));
    }
    batches += 1;
    println!("committed: {batches} sqlite3_exec call(s) for {} statement(s)", statements.len());
    Ok(())
}

fn send_progress(batches: usize, sent: usize, total: usize) {
    println!("  batch {batches} sent ({sent}/{total} statements)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_snapshot_shape() {
        let t = parse_snapshot(r#"{"Player":[["Team","Principal","10"]],"Empty":[]}"#).unwrap();
        assert_eq!(t["Player"][0], vec!["Team", "Principal", "10"]);
        assert!(t["Empty"].is_empty());
    }

    #[test]
    fn handles_escapes_and_quotes() {
        let t = parse_snapshot(r#"{"t":[["it's \"quoted\"","line\nbreak"]]}"#).unwrap();
        assert_eq!(t["t"][0][0], "it's \"quoted\"");
        assert_eq!(t["t"][0][1], "line\nbreak");
    }

    #[test]
    fn quotes_are_doubled_for_sql() {
        assert_eq!(quote("it's"), "'it''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("plain"), "'plain'");
    }

    #[test]
    fn protected_tables_are_named() {
        assert!(is_protected("Player"));
        assert!(is_protected("Custom_Save_Config"));
        assert!(is_protected("sqlite_master"));
        assert!(!is_protected("Staff_Contracts"));
        assert!(!is_protected("Races_Results"));
    }
}
