//! Applying a replicated career statement to the local database.
//!
//! Why this module exists
//! ----------------------
//! Both players load the same career save and edit it in memory. Nothing used to carry those edits between them,
//! so a client's signings, development and finances existed only in the client's process: the host never learned
//! of them, and whichever player saved last overwrote the other's work. `coop-proto`'s `Msg::Stmt` now carries a
//! change; this turns one into a statement the local game can execute.
//!
//! The security rule, and how it is enforced
//! -----------------------------------------
//! The only write path available is `LiveDb::sql`, which takes **one string** and hands it to `sqlite3_exec`
//! inside the game process. There is no `prepare`/`bind` exposed, so a parameterised update cannot be expressed
//! at that layer yet. That makes string composition unavoidable - which means a peer-supplied value must never
//! be allowed to reach the SQL as syntax.
//!
//! So the design is:
//!
//! * **Identifiers are checked against an allowlist**, not trusted. A table or column name that is not
//!   `[A-Za-z_][A-Za-z0-9_]*` is refused, so nothing can carry a quote, a semicolon or a comment marker.
//! * **Values are rendered as literals by type**, using `StmtValue::display`-equivalent quoting rules, never as
//!   raw text. Text is escaped by doubling single quotes, which is what SQL requires for a literal.
//! * **Only `UPDATE ... WHERE <pk column> = <int>` is produced.** Never `DELETE`, never `INSERT`, never a bare
//!   statement, so the worst a malformed message can do is fail.
//!
//! This is deliberately conservative: a replicated change is either a well-formed single-row update or it is
//! refused with a reason. `escape` and `ident_ok` are unit-tested against the obvious attacks rather than trusted.

use coop_proto::{Msg, StmtValue};

/// A statement ready to execute, plus what it was derived from for logging.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub sql: String,
    pub table: String,
    pub pk: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// A table or column name that is not a plain identifier.
    BadIdentifier(String),
    /// Nothing to set.
    NoAssignments,
    /// The message was not a `Stmt`.
    NotAStmt,
}

impl Refused {
    pub fn message(&self) -> String {
        match self {
            Refused::BadIdentifier(s) => format!(
                "refused a non-identifier name {s:?}: identifiers are checked, never interpolated from a peer"
            ),
            Refused::NoAssignments => "refused a statement with no assignments".to_string(),
            Refused::NotAStmt => "not a career statement".to_string(),
        }
    }
}

/// Is `s` a plain SQL identifier? Letters, digits and underscore, not starting with a digit.
///
/// This is the first line of defence: the only thing that makes string-composed SQL safe against a hostile peer
/// is that nothing they send can be syntax. A table named `Player; DROP TABLE Player --` fails here.
pub fn ident_ok(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_') && s.len() <= 64
}

/// Renders a value as a SQL literal.
///
/// Text is quoted and internal quotes doubled - the only escaping SQL requires for a literal. Numbers are
/// formatted from their types, so no value can introduce a quote or a semicolon. `NULL` is the keyword, not the
/// four-character string.
pub fn literal(v: &StmtValue) -> String {
    match v {
        StmtValue::Null => "NULL".to_string(),
        StmtValue::Int(i) => i.to_string(),
        // `{:?}` for f64 gives the shortest round-trippable form; a NaN or infinity is not a valid SQL literal,
        // so it becomes NULL rather than the words "NaN" (which SQLite would read as a column name).
        StmtValue::Real(r) => {
            if r.is_finite() {
                format!("{r:?}")
            } else {
                "NULL".to_string()
            }
        }
        StmtValue::Text(t) => {
            let s = String::from_utf8_lossy(t);
            let mut out = String::with_capacity(s.len() + 2);
            out.push('\'');
            for c in s.chars() {
                if c == '\'' {
                    out.push('\'');
                }
                // A NUL cannot appear in a SQL literal; drop it rather than truncate the statement.
                if c == '\0' {
                    continue;
                }
                out.push(c);
            }
            out.push('\'');
            out
        }
    }
}

/// Turns a replicated change into a single-row `UPDATE`.
///
/// `pk_column` is supplied by the caller from the local catalogue rather than taken from the message: the peer
/// says *which row* by value, and the local schema says which column identifies it. That keeps a peer from
/// choosing the predicate.
pub fn to_update(msg: &Msg, pk_column: &str) -> Result<Update, Refused> {
    let Msg::Stmt { table, pk, set, .. } = msg else {
        return Err(Refused::NotAStmt);
    };
    if set.is_empty() {
        return Err(Refused::NoAssignments);
    }
    let table_name = String::from_utf8_lossy(table).into_owned();
    if !ident_ok(&table_name) {
        return Err(Refused::BadIdentifier(table_name));
    }
    if !ident_ok(pk_column) {
        return Err(Refused::BadIdentifier(pk_column.to_string()));
    }

    let mut sql = format!("UPDATE {table_name} SET ");
    for (i, (col, val)) in set.iter().enumerate() {
        let col = String::from_utf8_lossy(col).into_owned();
        if !ident_ok(&col) {
            return Err(Refused::BadIdentifier(col));
        }
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str(&col);
        sql.push_str(" = ");
        sql.push_str(&literal(val));
    }
    // The key is an integer we formatted ourselves, so it cannot carry syntax.
    sql.push_str(&format!(" WHERE {pk_column} = {pk};"));

    Ok(Update {
        sql,
        table: table_name,
        pk: *pk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmt(table: &str, cols: &[(&str, StmtValue)]) -> Msg {
        Msg::Stmt {
            seq: 1,
            table: table.as_bytes().to_vec(),
            pk: 99,
            set: cols
                .iter()
                .map(|(c, v)| (c.as_bytes().to_vec(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn a_normal_change_becomes_one_bounded_update() {
        let m = stmt(
            "Player",
            &[("TeamID", StmtValue::Int(7)), ("Notes", StmtValue::Null)],
        );
        let u = to_update(&m, "PlayerID").unwrap();
        assert_eq!(u.sql, "UPDATE Player SET TeamID = 7, Notes = NULL WHERE PlayerID = 99;");
        assert_eq!(u.table, "Player");
        assert_eq!(u.pk, 99);
    }

    #[test]
    fn a_hostile_table_name_is_refused() {
        // The attack this exists to stop: a peer naming a "table" that terminates the statement.
        for bad in [
            "Player; DROP TABLE Player; --",
            "Player'",
            "Player--",
            "Player WHERE 1=1",
            "../Player",
            "",
        ] {
            let m = stmt(bad, &[("TeamID", StmtValue::Int(7))]);
            match to_update(&m, "PlayerID") {
                Err(Refused::BadIdentifier(_)) => {}
                other => panic!("{bad:?} must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_hostile_column_name_is_refused() {
        let m = stmt("Player", &[("TeamID = 1 WHERE 1=1 --", StmtValue::Int(7))]);
        assert!(matches!(to_update(&m, "PlayerID"), Err(Refused::BadIdentifier(_))));
    }

    #[test]
    fn a_hostile_value_stays_a_value() {
        let m = stmt(
            "Player",
            &[(
                "Notes",
                StmtValue::Text(b"7'); DROP TABLE Player; --".to_vec()),
            )],
        );
        let u = to_update(&m, "PlayerID").unwrap();
        // The whole payload is inside one quoted literal, with its own quote doubled.
        assert_eq!(
            u.sql,
            "UPDATE Player SET Notes = '7''); DROP TABLE Player; --' WHERE PlayerID = 99;"
        );
        // Exactly one *statement*: the only semicolon outside a quoted literal is the final one. Counting
        // semicolons would be wrong here - the payload legitimately contains two, and that is the point: they are
        // inside the literal, where SQL cannot read them as separators.
        assert!(u.sql.ends_with(" WHERE PlayerID = 99;"));
        let (before_literal, _) = u.sql.split_once('\'').expect("a quoted literal");
        assert_eq!(
            before_literal.matches(';').count(),
            0,
            "no semicolon may appear before the literal opens"
        );
        // And the payload cannot end the quote it is inside: every quote in it is doubled.
        assert!(u.sql.contains("'');"));
    }

    #[test]
    fn non_finite_reals_become_null_not_words() {
        // `NaN` would be read by SQLite as a column reference; NULL is the only honest rendering.
        assert_eq!(literal(&StmtValue::Real(f64::NAN)), "NULL");
        assert_eq!(literal(&StmtValue::Real(f64::INFINITY)), "NULL");
        assert_eq!(literal(&StmtValue::Real(1.5)), "1.5");
    }

    #[test]
    fn text_is_quoted_and_its_quotes_doubled() {
        assert_eq!(literal(&StmtValue::Text(b"O'Brien".to_vec())), "'O''Brien'");
        assert_eq!(literal(&StmtValue::Text(b"plain".to_vec())), "'plain'");
        assert_eq!(literal(&StmtValue::Text(Vec::new())), "''");
        // A NUL would truncate the statement, so it is dropped rather than emitted.
        assert_eq!(literal(&StmtValue::Text(b"a\0b".to_vec())), "'ab'");
    }

    #[test]
    fn a_statement_with_nothing_to_set_is_refused() {
        assert_eq!(to_update(&stmt("Player", &[]), "PlayerID"), Err(Refused::NoAssignments));
    }

    #[test]
    fn a_non_statement_message_is_refused() {
        assert_eq!(to_update(&Msg::Bye, "PlayerID"), Err(Refused::NotAStmt));
    }

    #[test]
    fn the_predicate_column_comes_from_the_caller_not_the_peer() {
        // A peer cannot influence the WHERE clause: it is supplied locally.
        let m = stmt("Player", &[("TeamID", StmtValue::Int(7))]);
        let u = to_update(&m, "PlayerID").unwrap();
        assert!(u.sql.contains("WHERE PlayerID = 99"));
        // And a bad local column name is refused too, rather than trusted because it is "ours".
        assert!(matches!(
            to_update(&m, "PlayerID = 1 OR 1"),
            Err(Refused::BadIdentifier(_))
        ));
    }
}
