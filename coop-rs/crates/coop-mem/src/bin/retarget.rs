//! Retargets the running game's "player team" and stops that team's AI.
//!
//! ```text
//! coop-retarget --pid <pid> --list                      # statement catalogue, no writes
//! coop-retarget --pid <pid> --status
//! coop-retarget --pid <pid> --team 7                     # this client: Player.TeamID = 7, AI off for 7
//! coop-retarget --pid <pid> --host-for-team 7            # host side: only stop 7's AI
//! coop-retarget --pid <pid> --reenable-ai 7 --day 42
//! coop-retarget --pid <pid> --team 7 --dry-run
//! ```
//!
//! Everything goes through the game's own `sqlite3_exec` and the game's own statements, so the
//! career database keeps its page cache, locking and save pipeline intact.
//!
//! This intentionally does **not** import saves, swap contracts or transplant data. It does the one
//! job the retarget design needs: change which team the UI presents, and stop that team deciding for
//! itself.

use coop_mem::livedb::LiveDb;
use coop_mem::retarget::Plan;
use coop_mem::statements::Catalog;

fn usage() -> ! {
    eprintln!(
        r#"coop-retarget --pid <pid> [--database <dir>] <command>

Commands
  --list                     print every statement key in the game's asset set
  --find <substr>            print keys containing <substr>
  --status                   read Player.TeamID and the AI record for --team
  --team <id>                client: set Player.TeamID = id and disable AI for id
  --host-for-team <id>       host:   disable AI for id only (host keeps its own team)
  --reenable-ai <id>         undo the AI disable, writing --day (default 0)

Options
  --database <dir>           Content/Database (default: the FModel export path)
  --season <n>               season id for the AI record (default 1)
  --day <n>                  day to write with --reenable-ai (default 0)
  --dry-run                  print the SQL, run nothing
"#
    );
    std::process::exit(2)
}

const DEFAULT_DB: &str =
    r"E:\FModel\Output\Exports\F1Manager24\Content\Database";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };
    let flag = |name: &str| args.iter().any(|a| a == name);

    if flag("--help") || flag("-h") || args.is_empty() {
        usage();
    }

    let db_dir = val("--database").unwrap_or_else(|| DEFAULT_DB.to_string());
    let cat = match Catalog::load(std::path::Path::new(&db_dir)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Catalogue-only commands need no game.
    if flag("--list") {
        println!("{} statements from {db_dir}", cat.len());
        for s in cat.find("") {
            println!("  {:<40} {}", s.key, s.table);
        }
        return;
    }
    if let Some(needle) = val("--find") {
        let hits = cat.find(&needle);
        println!("{} statement(s) matching {needle:?}", hits.len());
        for s in hits {
            println!("  {:<44} {}", s.key, s.table);
            println!("      {}", s.sql.trim());
        }
        return;
    }

    let season: i64 = val("--season").and_then(|s| s.parse().ok()).unwrap_or(1);
    let dry = flag("--dry-run");

    // Work out the plan before touching the game, so a bad request fails early with no writes.
    // --status first: `--status --team 7` is a read of team 7, not a write.
    if flag("--status") {
        let pid: u32 = match val("--pid").and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => {
                eprintln!("--status needs --pid");
                std::process::exit(2);
            }
        };
        let team: i64 = val("--team").and_then(|s| s.parse().ok()).unwrap_or(0);
        let plan = match Plan::for_host(&cat, team, season) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        status(pid, &cat, &plan, team);
        return;
    }

    let plan = if let Some(t) = val("--team") {
        let id: i64 = t.parse().unwrap_or_else(|_| usage());
        Plan::for_client(&cat, id, season)
    } else if let Some(t) = val("--host-for-team") {
        let id: i64 = t.parse().unwrap_or_else(|_| usage());
        Plan::for_host(&cat, id, season)
    } else if let Some(t) = val("--reenable-ai") {
        let id: i64 = t.parse().unwrap_or_else(|_| usage());
        let day: i64 = val("--day").and_then(|s| s.parse().ok()).unwrap_or(0);
        Plan::ai_reenable(&cat, id, season, day)
    } else {
        usage()
    };

    let plan = match plan {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot plan: {e}");
            std::process::exit(1);
        }
    };

    println!("plan: role={:?} team={} season={}", plan.role, plan.team_id, plan.season_id);
    for s in &plan.statements {
        println!("  [{}] {}", s.key, s.sql);
    }
    if dry {
        println!("--dry-run: nothing executed");
        return;
    }

    let pid: u32 = match val("--pid").and_then(|s| s.parse().ok()) {
        Some(p) => p,
        None => {
            eprintln!("--pid is required to execute (or pass --dry-run)");
            std::process::exit(2);
        }
    };

    let db = match LiveDb::attach(pid) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("attach failed: {e}");
            std::process::exit(1);
        }
    };
    println!("attached: sqlite3_exec 0x{:X}, database 0x{:X}", db.exec, db.db);

    // Prove the path before writing, exactly as coop-sql does.
    match db.sql("SELECT TeamID FROM Player") {
        Ok(rows) => println!("pre-write check: Player.TeamID = {:?}", rows),
        Err(e) => {
            eprintln!("pre-write check failed, not writing: {e}");
            std::process::exit(1);
        }
    }

    // One transaction: either the whole retarget lands or none of it does. A half-applied change
    // would leave the UI on one team and the AI disabled for another.
    let mut script = String::from("BEGIN;");
    for s in &plan.statements {
        script.push_str(&s.sql);
        if !s.sql.trim_end().ends_with(';') {
            script.push(';');
        }
    }
    script.push_str("COMMIT;");

    println!("--- applying in one transaction");
    match db.sql(&script) {
        Ok(_) => println!("  applied"),
        Err(e) => {
            println!("  error: {e}");
            let _ = db.sql("ROLLBACK;");
            std::process::exit(1);
        }
    }

    // Read back, and say plainly whether it took. A statement that "succeeded" but changed nothing
    // is the failure mode this exists to catch.
    match plan.verify_sql(&cat).and_then(|q| db.sql(&q)) {
        Ok(rows) => {
            println!("--- verify: {:?}", rows);
            verify(&plan, &rows);
        }
        Err(e) => println!("  verify failed: {e}"),
    }
}

fn verify(plan: &Plan, rows: &[Vec<String>]) {
    match plan.role {
        coop_mem::retarget::Role::Client => {
            // GetPlayerData returns TeamID as the third column.
            let team = rows.first().and_then(|r| r.get(2)).cloned();
            match team {
                Some(t) if t == plan.team_id.to_string() => {
                    println!("  OK: Player.TeamID is now {t}")
                }
                Some(t) => println!(
                    "  NOT APPLIED: Player.TeamID is {t}, expected {} (the engine may republish \
                     playerTeamID only on (re)load - see TEAM-RETARGET.md 2.5)",
                    plan.team_id
                ),
                None => println!("  UNEXPECTED shape: {rows:?}"),
            }
        }
        coop_mem::retarget::Role::Host => {
            let day = rows.first().and_then(|r| r.first()).cloned();
            println!("  AI LastDecisionEvaluationDay for team {} = {:?}", plan.team_id, day);
        }
    }
}

fn status(pid: u32, cat: &Catalog, plan: &Plan, team: i64) {
    let db = match LiveDb::attach(pid) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("attach failed: {e}");
            std::process::exit(1);
        }
    };
    println!("attached: database 0x{:X}", db.db);

    match db.sql("SELECT FirstName, LastName, TeamID FROM Player") {
        Ok(rows) => println!("Player: {:?}", rows),
        Err(e) => println!("Player read failed: {e}"),
    }

    match plan.verify_sql(cat).and_then(|q| db.sql(&q)) {
        Ok(rows) => println!("AI record for team {team}: {:?}", rows),
        Err(e) => println!("AI record read failed: {e}"),
    }

    // The AI record row is created on demand per (team, season), so "no rows" is normal before the
    // team is first evaluated - it is not an error.
    println!("(no rows above for the AI record means the team has not been evaluated this season)");
}
