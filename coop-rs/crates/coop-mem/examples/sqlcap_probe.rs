//! What does `sqlcap` make of a statement the game actually runs through `sqlite3_exec`?
//!
//! This is a diagnostic, not a test. It reads a JSON array of statement strings - `tools/schema_truth.py --dump`
//! writes exactly that file - and prints what `sqlcap::classify` and `replicate::plan` do with each.
//!
//! Why it is worth keeping
//! -----------------------
//! Audit §29.2 found that the capture hook patches `sqlite3_exec`, that 361 of the catalogue's 428 write statements
//! are parameterised and therefore never reach it, and that the 67 that do reach it are housekeeping. The claim
//! "zero of 67 parse" is a measurement, and this is the thing that measures it. Without it the number would be a
//! remembered result rather than a reproducible one, which is the class of claim this project has repeatedly had to
//! retract.
//!
//! Usage
//! -----
//!     python -I tools/schema_truth.py --dump exec_visible.json
//!     cargo run --release -p coop-mem --example sqlcap_probe -- exec_visible.json

use coop_mem::{replicate, sqlcap};

fn main() {
    let path = std::env::args().nth(1).expect("usage: sqlcap_probe <statements.json>");
    let text = std::fs::read_to_string(&path).expect("read");
    // The file is a JSON array of strings; parsed here rather than pulling in a JSON crate for one diagnostic.
    let mut stmts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in text.chars() {
        if in_str {
            if esc {
                cur.push(match c {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    o => o,
                });
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                stmts.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_str = true;
        }
    }

    // A configured player, so the question asked is "would this have been replicated?" rather than "is co-op set
    // up?", which is a different and less interesting refusal.
    let cfg = Some((coop_auth::Mode::Versus, coop_auth::Player::new(1, 0, true)));
    let (mut reads, mut unsupported, mut wrote, mut refused) = (0, 0, 0, 0);
    for s in &stmts {
        match sqlcap::classify(s) {
            sqlcap::Kind::Read => reads += 1,
            sqlcap::Kind::Unsupported(t) => {
                unsupported += 1;
                println!("  unsupported [{}]", t.chars().take(110).collect::<String>());
            }
            sqlcap::Kind::Write(c) => match replicate::plan(&c, cfg) {
                Ok(_) => {
                    wrote += 1;
                    println!("  WOULD REPLICATE: {}", c.table);
                }
                Err(r) => {
                    refused += 1;
                    println!("  parsed but refused: {} -- {}", c.table, r.reason());
                }
            },
        }
    }
    println!("\n{} statements the game runs through sqlite3_exec:", stmts.len());
    println!("  read            : {reads}");
    println!("  unsupported     : {unsupported}");
    println!("  parsed + planned: {wrote}");
    println!("  parsed + refused: {refused}");
}
