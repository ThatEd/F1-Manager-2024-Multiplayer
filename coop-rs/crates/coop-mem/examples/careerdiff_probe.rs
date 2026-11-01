//! Drive `careerdiff` against the game's own schema, end to end, with no game running.
//!
//! Why this exists
//! ---------------
//! The unit tests in `careerdiff` use hand-written rows. That proves the diffing logic and nothing about whether the
//! SQL those functions emit works against the database the game actually ships - and the first version of
//! `select_sql` used `SELECT hex(quote(pk)), *`, which re-emits the key unencoded and makes every row fail to
//! decode. A unit test could not have caught that; this does.
//!
//! It reads `Volta.json`, decodes it to a SQLite file, runs the generated `SELECT` for all 16 replicable tables
//! through the real `parse_rows`, applies a real `UPDATE`, and asserts exactly one changed cell. If this passes,
//! the producer is proven against the game's own data and only the `LiveDb` transport remains untested.
//!
//! Usage
//! -----
//!     python -I tools/schema_truth.py --volta career.db
//!     cargo run --release -p coop-mem --example careerdiff_probe -- career.db

use coop_mem::{careerdiff, schema};

fn read(con: &rusqlite_like::Db, t: &schema::Table) -> Result<Vec<careerdiff::Row>, String> {
    // The column list comes from the database, exactly as the live path does it.
    let cols = con.columns(t.name)?;
    let shape = careerdiff::shape_from(t, &cols)?;
    let rows = con.query(&careerdiff::select_sql(&shape))?;
    // Rebuild the framing `LiveDb::parse_rows` produces, so this exercises the same decoder.
    let text: String = rows.iter().map(|r| r.join("\u{1f}") + "\u{1e}").collect();
    careerdiff::parse_rows(text.as_bytes(), &shape.columns, shape.key)
}

/// The smallest thing that can run a SQLite query: shells out to `python`, because adding a SQLite crate to test
/// code would put a second SQLite implementation in a project that deliberately has none.
mod rusqlite_like {
    use std::process::Command;

    pub struct Db {
        pub path: String,
    }

    impl Db {
        pub fn columns(&self, table: &str) -> Result<Vec<String>, String> {
            let rows = self.py(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
            Ok(rows.into_iter().filter_map(|r| r.into_iter().next()).filter(|c| !c.is_empty()).collect())
        }

        pub fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, String> {
            self.py(sql)
        }

        pub fn exec(&self, sql: &str) -> Result<(), String> {
            self.py(sql).map(|_| ())
        }

        /// Runs `sql` and prints rows as `\x1f`-separated, `\x1e`-terminated - the same shape `LiveDb` produces.
        fn py(&self, sql: &str) -> Result<Vec<Vec<String>>, String> {
            let script = r#"
import sqlite3, sys
con = sqlite3.connect(sys.argv[1])
cur = con.execute(sys.argv[2])
out = []
for row in cur:
    out.append("\x1f".join("" if v is None else str(v) for v in row) + "\x1e")
sys.stdout.write("".join(out))
con.commit()
"#;
            let r = Command::new("python")
                .arg("-c")
                .arg(script)
                .arg(&self.path)
                .arg(sql)
                .output()
                .map_err(|e| format!("could not run python: {e}"))?;
            if !r.status.success() {
                return Err(String::from_utf8_lossy(&r.stderr).to_string());
            }
            let text = String::from_utf8_lossy(&r.stdout).to_string();
            let rows = text
                .split('\u{1e}')
                .filter(|r| !r.is_empty())
                .map(|r| r.split('\u{1f}').map(|s| s.to_string()).collect())
                .collect();
            Ok(rows)
        }
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: careerdiff_probe <career.db>");
    let db = rusqlite_like::Db { path };

    let mut before = careerdiff::Snapshot::new();
    let mut rows = 0usize;
    for t in schema::REPLICABLE {
        match read(&db, t) {
            Ok(r) => {
                rows += r.len();
                before.insert(t.name.to_string(), r);
            }
            Err(e) => {
                println!("FAIL {}: {e}", t.name);
                std::process::exit(1);
            }
        }
    }
    println!("read {} tables, {} rows through the real decoder", before.len(), rows);

    // An unchanged database must produce nothing: a producer that re-sends identical rows would put a full-table
    // rewrite on the wire every pass.
    let mut after = careerdiff::Snapshot::new();
    for t in schema::REPLICABLE {
        after.insert(t.name.to_string(), read(&db, t).expect("re-read"));
    }
    let quiet = careerdiff::compare(&before, &after);
    if !quiet.is_empty() {
        println!("FAIL: two identical passes reported {} change(s)", quiet.carriable());
        std::process::exit(1);
    }
    println!("stable: two identical passes report no change");

    // One real edit.
    db.exec("UPDATE \"Teams\" SET TeamName = TeamName || ' (x)' WHERE TeamID = (SELECT MIN(TeamID) FROM Teams)")
        .expect("edit");
    let mut edited = careerdiff::Snapshot::new();
    for t in schema::REPLICABLE {
        edited.insert(t.name.to_string(), read(&db, t).expect("re-read after edit"));
    }
    let pass = careerdiff::compare(&after, &edited);
    let carried = pass.carriable();
    println!(
        "after one edit: {} table(s) changed, {} change(s) ready, {} uncarriable",
        pass.diffs.len(),
        carried,
        pass.uncarriable
    );
    if pass.diffs.len() != 1 || carried != 1 {
        println!("FAIL: one column of one row changed, so exactly one change was expected");
        for d in &pass.diffs {
            println!("  {} changed={} added={} removed={}", d.table, d.changed.len(), d.added.len(), d.removed.len());
        }
        std::process::exit(1);
    }
    let (key, cells) = &pass.diffs[0].changed[0];
    println!("  {} key={} -> {}", pass.diffs[0].table, key.as_text(), cells.len());
    if cells.len() != 1 {
        println!("FAIL: only TeamName moved, so one column should travel");
        std::process::exit(1);
    }
    println!("\nOK: the diff producer works against the game's own schema.");
}
