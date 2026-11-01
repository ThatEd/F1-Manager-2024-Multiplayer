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
//! One more check, added after the table list was found to be built from asset names rather than from the schema:
//! **the predicate column must be the one the local schema declares as that table's key.** `to_update` used to take
//! whatever `pk_column` the caller passed. The caller now takes it from the peer's own statement, so a peer could
//! name `TeamID` for a table whose key is `DesignID` and the local `UPDATE` would match every row of a team instead
//! of one row. Checking it here means the peer chooses neither the value's meaning nor the predicate.
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
    /// The predicate column is not the key the local schema declares for that table.
    WrongPredicate { table: String, found: String, expected: &'static str },
    /// The table is not one this build replicates at all.
    UnknownTable(String),
}

impl Refused {
    pub fn message(&self) -> String {
        match self {
            Refused::BadIdentifier(s) => format!(
                "refused a non-identifier name {s:?}: identifiers are checked, never interpolated from a peer"
            ),
            Refused::NoAssignments => "refused a statement with no assignments".to_string(),
            Refused::NotAStmt => "not a career statement".to_string(),
            Refused::WrongPredicate { table, found, expected } => format!(
                "refused {table} selected on {found}: the local schema keys it by {expected}, and a predicate the \
                 peer chooses would let one message match many rows"
            ),
            Refused::UnknownTable(t) => format!("refused {t}: not a table this build replicates"),
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
    // The predicate must be the key the local schema declares, not whatever the peer's statement selected on. The
    // sending side refuses a statement whose predicate is not its own table's key, but a hostile or older peer can
    // still send one, and `WHERE TeamID = 7` on a 26-car grid would rewrite every row belonging to that team.
    let schema = crate::schema::lookup(&table_name)
        .ok_or_else(|| Refused::UnknownTable(table_name.clone()))?;
    if !pk_column.eq_ignore_ascii_case(schema.pk) {
        return Err(Refused::WrongPredicate {
            table: schema.name.to_string(),
            found: pk_column.to_string(),
            expected: schema.pk,
        });
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
            pk_column: b"TeamID".to_vec(),
            pk: 99,
            set: cols
                .iter()
                .map(|(c, v)| (c.as_bytes().to_vec(), v.clone()))
                .collect(),
        }
    }

    #[test]
    fn a_normal_change_becomes_one_bounded_update() {
        // `Teams` is keyed `TeamID INTEGER PRIMARY KEY AUTOINCREMENT`, so it is a real table with a single-column
        // key - which is what `Msg::Stmt` can carry. `Player` is keyed `(FirstName, LastName)` and cannot be.
        let m = stmt(
            "Teams",
            &[("PredictedRanking", StmtValue::Int(7)), ("ProfileText", StmtValue::Null)],
        );
        let u = to_update(&m, "TeamID").unwrap();
        assert_eq!(u.sql, "UPDATE Teams SET PredictedRanking = 7, ProfileText = NULL WHERE TeamID = 99;");
        assert_eq!(u.table, "Teams");
        assert_eq!(u.pk, 99);
    }

    #[test]
    fn a_table_outside_the_schema_is_refused_even_with_a_clean_name() {
        // `Player` passes the identifier check and is a real table, but it is not replication-eligible: it is keyed
        // by two columns. The applier must not be the weak half of that decision.
        let m = stmt("Player", &[("TeamID", StmtValue::Int(7))]);
        assert_eq!(
            to_update(&m, "TeamID"),
            Err(Refused::UnknownTable("Player".to_string()))
        );
    }

    #[test]
    fn a_predicate_the_peer_chooses_is_refused_when_it_is_not_the_local_key() {
        // The concrete danger: `Teams` is keyed by `TeamID`, and a peer sending `pk_column = "Formula"` would have
        // the local UPDATE match every team in Formula 2 rather than one row.
        let m = stmt("Teams", &[("PredictedRanking", StmtValue::Int(1))]);
        match to_update(&m, "Formula") {
            Err(Refused::WrongPredicate { table, found, expected }) => {
                assert_eq!(table, "Teams");
                assert_eq!(found, "Formula");
                assert_eq!(expected, "TeamID");
            }
            other => panic!("expected WrongPredicate, got {other:?}"),
        }
    }

    #[test]
    fn a_hostile_table_name_is_refused() {
        // The attack this exists to stop: a peer naming a "table" that terminates the statement.
        for bad in [
            "Teams; DROP TABLE Teams; --",
            "Teams'",
            "Teams--",
            "Teams WHERE 1=1",
            "../Teams",
            "",
        ] {
            let m = stmt(bad, &[("TeamID", StmtValue::Int(7))]);
            match to_update(&m, "TeamID") {
                Err(Refused::BadIdentifier(_)) => {}
                other => panic!("{bad:?} must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_hostile_column_name_is_refused() {
        let m = stmt("Teams", &[("TeamID = 1 WHERE 1=1 --", StmtValue::Int(7))]);
        assert!(matches!(to_update(&m, "TeamID"), Err(Refused::BadIdentifier(_))));
    }

    #[test]
    fn a_hostile_value_stays_a_value() {
        let m = stmt(
            "Teams",
            &[(
                "ProfileText",
                StmtValue::Text(b"7'); DROP TABLE Teams; --".to_vec()),
            )],
        );
        let u = to_update(&m, "TeamID").unwrap();
        // The whole payload is inside one quoted literal, with its own quote doubled.
        assert_eq!(
            u.sql,
            "UPDATE Teams SET ProfileText = '7''); DROP TABLE Teams; --' WHERE TeamID = 99;"
        );
        // Exactly one *statement*: the only semicolon outside a quoted literal is the final one. Counting
        // semicolons would be wrong here - the payload legitimately contains two, and that is the point: they are
        // inside the literal, where SQL cannot read them as separators.
        assert!(u.sql.ends_with(" WHERE TeamID = 99;"));
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
        assert_eq!(to_update(&stmt("Teams", &[]), "TeamID"), Err(Refused::NoAssignments));
    }

    #[test]
    fn a_non_statement_message_is_refused() {
        assert_eq!(to_update(&Msg::Bye, "TeamID"), Err(Refused::NotAStmt));
    }

    #[test]
    fn the_predicate_column_must_match_the_local_schema() {
        // The commentary here used to say the predicate "is supplied locally, so a peer cannot influence it". The
        // caller now takes it from the peer's statement, so that stopped being true - the schema check is what
        // restores the property. A local column name is still validated rather than trusted because it is "ours".
        let m = stmt("Teams", &[("PredictedRanking", StmtValue::Int(7))]);
        let u = to_update(&m, "TeamID").unwrap();
        assert!(u.sql.contains("WHERE TeamID = 99"));
        assert!(matches!(
            to_update(&m, "TeamID = 1 OR 1"),
            Err(Refused::BadIdentifier(_))
        ));
    }
}
