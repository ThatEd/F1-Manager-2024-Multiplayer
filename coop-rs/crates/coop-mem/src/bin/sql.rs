//! Runs SQL against the game's live in-memory career database.
//!
//! ```text
//! coop-sql --pid <pid> "SELECT TeamID FROM Player"
//! coop-sql --pid <pid> --file statements.sql
//! coop-sql --pid <pid> --swap-contracts 8 83          # swap the two drivers' teams
//! ```
//!
//! Everything goes through the game's own `sqlite3_exec`, so the page cache, locking and save
//! pipeline behave exactly as they do for the game. Nothing here writes to game memory directly.

use coop_mem::livedb::LiveDb;

fn usage() -> ! {
    eprintln!("coop-sql --pid <pid> (<sql> | --file <path> | --swap-contracts <idA> <idB>)");
    eprintln!();
    eprintln!("Runs SQL inside the game's own SQLite database. The career database is in-memory,");
    eprintln!("so this is the only way to read or change it while the game is running.");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };
    let pid: u32 = match get("--pid").and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => usage(),
    };

    eprintln!("[stage] attaching to {pid}");
    let db = match LiveDb::attach(pid) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("attach failed: {e}");
            std::process::exit(1);
        }
    };
    println!("attached: sqlite3_exec 0x{:X}, database 0x{:X}", db.exec, db.db);
    eprintln!("[stage] running probe");

    // A trivial read first: proves the whole path (thread, stub, callback, parse) before anything
    // is changed, and it is the same probe the attach logic uses to pick the right database.
    match db.sql("SELECT TeamID FROM Player") {
        Ok(rows) => println!("probe ok: Player.TeamID = {:?}", rows),
        Err(e) => {
            eprintln!("probe failed: {e}");
            std::process::exit(1);
        }
    }

    if args.iter().any(|a| a == "--swap-contracts") {
        let i = args.iter().position(|a| a == "--swap-contracts").unwrap();
        let a: i64 = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
        let b: i64 = args.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage());
        swap_contracts(&db, a, b);
        return;
    }

    if let Some(path) = get("--file") {
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("cannot read {path}: {e}");
                std::process::exit(1);
            }
        };
        run(&db, &text);
        return;
    }

    // Everything that is not an option is SQL.
    let sql: Vec<&String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .skip_while(|a| a.parse::<u32>().is_ok())
        .collect();
    if sql.is_empty() {
        usage();
    }
    let query = sql.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
    run(&db, &query);
}

fn run(db: &LiveDb, sql: &str) {
    // Authorisation gate.
    //
    // Every write in co-op goes through here, so this is the one place a mode rule has to be enforced. The
    // tool can still be used without a mode set (single-player maintenance), in which case writes are
    // allowed exactly as before - but if `coop_mode.txt` exists, a write that the mode forbids is refused
    // before it reaches the database rather than after.
    if let Some(denied) = coop_auth::gate_sql(sql) {
        println!("--- {sql}");
        println!("  REFUSED: {denied}");
        return;
    }
    println!("--- {sql}");
    match db.sql(sql) {
        Ok(rows) => {
            println!("{} row(s)", rows.len());
            for r in rows.iter().take(50) {
                println!("  {}", r.join(" | "));
            }
        }
        Err(e) => println!("  error: {e}"),
    }
}

/// Swaps two staff members' teams, exchanging only the two contract rows.
///
/// Written as two statements inside one explicit transaction so a failure cannot leave both drivers
/// on the same team. The `CASE` form does the exchange in a single `UPDATE`, which avoids the
/// intermediate state where both rows briefly hold the same value.
fn swap_contracts(db: &LiveDb, a: i64, b: i64) {
    println!("--- current contracts");
    let before = format!(
        "SELECT StaffID, TeamID, PosInTeam, Salary, EndSeason FROM Staff_Contracts \
         WHERE StaffID IN ({a},{b}) AND ContractType = 0 ORDER BY StaffID"
    );
    match db.sql(&before) {
        Ok(rows) => {
            for r in &rows {
                println!("  {}", r.join(" | "));
            }
        }
        Err(e) => {
            println!("  error: {e}");
            return;
        }
    }

    // Exchange the two teams in ONE statement, evaluating each row's CURRENT value.
    //
    // The first version used correlated subqueries to read the other row, which is wrong: by the time
    // the second row is evaluated the first has already been updated, so both end up on the same team.
    // `CASE WHEN TeamID = <a's team> THEN <b's team> ELSE <a's team> END` is evaluated per row against
    // the value that row has right now, so the exchange needs no intermediate state.
    let teams = match db.sql(&format!(
        "SELECT StaffID, TeamID FROM Staff_Contracts WHERE StaffID IN ({a},{b}) AND ContractType = 0"
    )) {
        Ok(rows) if rows.len() == 2 => rows,
        Ok(rows) => {
            println!("  expected 2 contract rows, found {}", rows.len());
            return;
        }
        Err(e) => {
            println!("  error: {e}");
            return;
        }
    };
    let team_of = |id: &str| -> Option<String> {
        teams.iter().find(|r| r.first().map(|s| s.as_str()) == Some(id)).and_then(|r| r.get(1).cloned())
    };
    let (ta, tb) = match (team_of(&a.to_string()), team_of(&b.to_string())) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            println!("  could not resolve both team ids");
            return;
        }
    };
    if ta == tb {
        println!("  both already on team {ta}; nothing to do");
        return;
    }

    let stmt = format!(
        "UPDATE Staff_Contracts SET TeamID = CASE TeamID WHEN {ta} THEN {tb} ELSE {ta} END \
         WHERE StaffID IN ({a},{b}) AND ContractType = 0 AND TeamID IN ({ta},{tb})"
    );
    println!("--- swap: {a} {ta}<->{tb} , {b}");
    match db.sql(&format!("BEGIN;{stmt};COMMIT;")) {
        Ok(_) => println!("  applied"),
        Err(e) => {
            println!("  error: {e}");
            let _ = db.sql("ROLLBACK;");
            return;
        }
    }

    println!("--- after");
    match db.sql(&before) {
        Ok(rows) => {
            for r in &rows {
                println!("  {}", r.join(" | "));
            }
        }
        Err(e) => println!("  error: {e}"),
    }
}
