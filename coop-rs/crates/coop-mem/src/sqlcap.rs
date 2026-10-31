//! Turning the game's own SQL statements into replicated career changes.
//!
//! Why this module exists
//! ----------------------
//! §20 of the audit found the gap: nothing carried management state between players. `coop-proto`'s `Msg::Stmt`
//! and `coop-mem::stmtapply` closed the *transport and application* halves. This is the missing **producer**: the
//! game writes its career through `sqlite3_exec`, so every change it makes passes through one address, and
//! capturing the statement text there is how a client's signing learns what row to tell the host about.
//!
//! What it can and cannot know
//! ---------------------------
//! It sees the statement **after** the game composed it, as a string. It does not see bound parameters, so a
//! statement that used `?` placeholders is not replicable from the text alone - and this says so rather than
//! guessing. The game's SQL shapes have never been observed in a running session, so the parser is deliberately
//! narrow and **refuses what it does not understand**, recording the text so a first live run can teach us the
//! real shapes instead of silently replicating nothing.
//!
//! That failure mode matters more than coverage. A parser that guessed at an unfamiliar statement would replicate
//! a wrong row into the other player's career - silent corruption. Refusing costs a feature that does not work yet
//! and leaves evidence behind.
//!
//! Scope, deliberately narrow
//! ---------------------------
//! * `INSERT INTO t (a, b) VALUES (1, 'x')`  -> an insert of one row
//! * `UPDATE t SET a = 1, b = 'x' WHERE k = 9` -> an update of one row
//! * everything else -> `Unsupported`, with the text kept
//!
//! Both are single-row. Multi-row inserts and a `WHERE` that is not a single equality are refused: they cannot be
//! expressed as one `Msg::Stmt`, and splitting them would risk applying half a change.

/// What a captured statement turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A statement that changes rows, expressed as one replicable unit.
    Write(Change),
    /// A read. Not replicated, and not interesting.
    Read,
    /// Something this parser does not handle. The text is kept for diagnosis.
    Unsupported(String),
}

/// One row-level change, ready to become `Msg::Stmt`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub table: String,
    pub action: Action,
    /// Column/value pairs, values already parsed into `Value`.
    pub set: Vec<(String, Value)>,
    /// How the row is identified: `Some((column, value))` for an `UPDATE`, `None` for an `INSERT` (whose key is
    /// the new row).
    pub key: Option<(String, Value)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Insert,
    Update,
}

/// A literal parsed out of a statement. Mirrors `coop_proto::StmtValue` without depending on it, so this module
/// can be tested as pure text handling.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl Value {
    pub fn as_text(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Int(i) => i.to_string(),
            Value::Real(r) => format!("{r}"),
            Value::Text(t) => t.clone(),
        }
    }
}

/// Splits a SQL script into individual statements, honouring quoted strings.
///
/// A naive `split(';')` is wrong: `INSERT INTO t VALUES ('a; b')` is one statement, and a career is full of names
/// with punctuation in them. Semicolons inside `'...'` or `"..."` do not separate, and `''` inside a string is an
/// escaped quote rather than a terminator.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    let mut quote: Option<char> = None;

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    // A doubled quote is an escaped quote, not the end of the literal.
                    if chars.peek() == Some(&q) {
                        cur.push(chars.next().unwrap());
                    } else {
                        quote = None;
                    }
                }
            }
            None => {
                if c == '\'' || c == '"' {
                    quote = Some(c);
                    cur.push(c);
                } else if c == ';' {
                    let t = cur.trim();
                    if !t.is_empty() {
                        out.push(t.to_string());
                    }
                    cur.clear();
                } else {
                    cur.push(c);
                }
            }
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// Classifies one statement.
pub fn classify(stmt: &str) -> Kind {
    let head = first_word(stmt);
    match head.as_deref() {
        Some("insert") => match parse_insert(stmt) {
            Some(c) => Kind::Write(c),
            None => Kind::Unsupported(stmt.to_string()),
        },
        Some("update") => match parse_update(stmt) {
            Some(c) => Kind::Write(c),
            None => Kind::Unsupported(stmt.to_string()),
        },
        Some("select") | Some("pragma") => Kind::Read,
        // A refusal is not a bug: an unrecognised statement is one this build cannot replicate, and recording it
        // is how the next version learns what to support.
        _ => Kind::Unsupported(stmt.to_string()),
    }
}

fn first_word(s: &str) -> Option<String> {
    s.trim_start()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
}

/// Reads an identifier, optionally quoted with `"` or backticks or brackets.
fn ident_at(s: &str) -> Option<(String, usize)> {
    let t = s.trim_start();
    let skipped = s.len() - t.len();
    let bytes: Vec<char> = t.chars().collect();
    if bytes.is_empty() {
        return None;
    }
    match bytes[0] {
        '"' | '`' => {
            let q = bytes[0];
            let mut name = String::new();
            let mut i = 1;
            while i < bytes.len() {
                if bytes[i] == q {
                    if bytes.get(i + 1) == Some(&q) {
                        name.push(q);
                        i += 2;
                        continue;
                    }
                    return Some((name, skipped + i + 1));
                }
                name.push(bytes[i]);
                i += 1;
            }
            None
        }
        '[' => {
            if let Some(end) = t.find(']') {
                return Some((t[1..end].to_string(), skipped + end + 1));
            }
            None
        }
        c if c.is_alphabetic() || c == '_' => {
            let mut i = 0;
            while i < bytes.len() && (bytes[i].is_alphanumeric() || bytes[i] == '_') {
                i += 1;
            }
            Some((bytes[..i].iter().collect(), skipped + i))
        }
        _ => None,
    }
}

fn parse_insert(stmt: &str) -> Option<Change> {
    // insert [or ...] into <table> ( <cols> ) values ( <vals> )
    let lower = stmt.to_ascii_lowercase();
    let into = lower.find("into")?;
    let after_into = &stmt[into + 4..];
    let (table, used) = ident_at(after_into)?;
    let rest = after_into[used..].trim_start();

    // Optional column list, then VALUES.
    let (cols, rest) = if rest.starts_with('(') {
        let close = matching_paren(rest)?;
        let inner = &rest[1..close];
        let cols = split_top_level(inner)
            .into_iter()
            .map(|c| ident_at(&c).map(|(n, _)| n))
            .collect::<Option<Vec<_>>>()?;
        (cols, rest[close + 1..].trim_start())
    } else {
        (Vec::new(), rest)
    };

    let lower_rest = rest.to_ascii_lowercase();
    if !lower_rest.starts_with("values") {
        return None;
    }
    let after_values = rest["values".len()..].trim_start();
    if !after_values.starts_with('(') {
        return None;
    }
    let close = matching_paren(after_values)?;
    // Anything after the closing paren that is not a separator means this is a multi-row insert or a compound
    // statement; refuse rather than replicate one row of many.
    if !after_values[close + 1..].trim().is_empty() {
        return None;
    }
    let vals = split_top_level(&after_values[1..close])
        .into_iter()
        .map(|v| parse_value(&v))
        .collect::<Option<Vec<_>>>()?;

    if cols.len() != vals.len() {
        return None;
    }
    // A `?` placeholder cannot be resolved from text; `parse_value` returns None for those, but a positional
    // placeholder could also appear where a count matches, so reject explicitly.
    let set = cols.into_iter().zip(vals).collect();
    Some(Change {
        table,
        action: Action::Insert,
        set,
        key: None,
    })
}

fn parse_update(stmt: &str) -> Option<Change> {
    let lower = stmt.to_ascii_lowercase();
    let after_update = &stmt["update".len()..];
    let (table, used) = ident_at(after_update)?;
    let rest = after_update[used..].trim_start();
    let lower_rest = rest.to_ascii_lowercase();
    let set_at = lower_rest.find("set")?;
    let after_set = rest[set_at + 3..].trim_start();

    let lower_after = after_set.to_ascii_lowercase();
    let where_at = lower_after.find("where")?;
    let assignments = after_set[..where_at].trim();
    let predicate = after_set[where_at + 5..].trim();

    let mut set = Vec::new();
    for part in split_top_level(assignments) {
        let (col, eq_at) = ident_at(&part)?;
        let after = part[eq_at..].trim_start();
        let after = after.strip_prefix('=')?;
        let val = parse_value(after)?;
        set.push((col, val));
    }
    if set.is_empty() {
        return None;
    }

    // Only a single `col = value` predicate is replicable as one row. Anything compound (AND/OR/IN) could match
    // several rows, and applying it to the other player's career would be a multi-row write nobody asked for.
    if predicate.to_ascii_uppercase().contains(" AND ")
        || predicate.to_ascii_uppercase().contains(" OR ")
        || predicate.to_ascii_uppercase().contains(" IN ")
    {
        return None;
    }
    let (col, eq_at) = ident_at(predicate)?;
    let after = predicate[eq_at..].trim_start().strip_prefix('=')?;
    let key_val = parse_value(after)?;

    let _ = lower; // kept for symmetry with parse_insert; the offsets above are already case-correct
    Some(Change {
        table,
        action: Action::Update,
        set,
        key: Some((col, key_val)),
    })
}

/// Index of the `)` matching the `(` at position 0, honouring quotes.
fn matching_paren(s: &str) -> Option<usize> {
    let bytes: Vec<char> = s.chars().collect();
    if bytes.first() != Some(&'(') {
        return None;
    }
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == q {
                    if bytes.get(i + 1) == Some(&q) {
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Splits on commas that are not inside parentheses or quotes.
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    if chars.get(i + 1) == Some(&q) {
                        cur.push(chars[i + 1]);
                        i += 1;
                    } else {
                        quote = None;
                    }
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth -= 1;
                    cur.push(c);
                }
                ',' if depth == 0 => {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
                _ => cur.push(c),
            },
        }
        i += 1;
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// Parses a SQL literal. Returns `None` for anything that is not a literal - a placeholder, a column reference, a
/// function call - because replicating an unresolved expression would write a wrong value.
pub fn parse_value(tok: &str) -> Option<Value> {
    let t = tok.trim();
    if t.is_empty() {
        return None;
    }
    if t.eq_ignore_ascii_case("null") {
        return Some(Value::Null);
    }
    if t.starts_with('\'') {
        if !t.ends_with('\'') || t.len() < 2 {
            return None;
        }
        let inner = &t[1..t.len() - 1];
        // Unescape doubled quotes; anything else is left as-is, because SQL only escapes a quote by doubling.
        return Some(Value::Text(inner.replace("''", "'")));
    }
    if t.starts_with('"') {
        // A double-quoted token is an identifier in standard SQL, and only a string if no such column exists.
        // Treating it as text would sometimes replicate a column name as data, so refuse.
        return None;
    }
    if t.starts_with('?') || t.starts_with(':') || t.starts_with('@') || t.starts_with('$') {
        return None;
    }
    if let Ok(i) = t.parse::<i64>() {
        return Some(Value::Int(i));
    }
    if let Ok(r) = t.parse::<f64>() {
        if r.is_finite() {
            return Some(Value::Real(r));
        }
        return None;
    }
    // A bare word is a column reference or a function; not a value we can replicate.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(sql: &str) -> Change {
        match classify(sql) {
            Kind::Write(c) => c,
            other => panic!("expected a write for {sql:?}, got {other:?}"),
        }
    }

    #[test]
    fn an_insert_becomes_a_change() {
        let c = change("INSERT INTO Player (PlayerID, TeamID, FirstName) VALUES (12, 7, 'Max')");
        assert_eq!(c.table, "Player");
        assert_eq!(c.action, Action::Insert);
        assert_eq!(
            c.set,
            vec![
                ("PlayerID".to_string(), Value::Int(12)),
                ("TeamID".to_string(), Value::Int(7)),
                ("FirstName".to_string(), Value::Text("Max".to_string())),
            ]
        );
        assert_eq!(c.key, None);
    }

    #[test]
    fn an_update_becomes_a_change_with_its_key() {
        let c = change("UPDATE Player SET TeamID = 7, Salary = 1250000.5 WHERE PlayerID = 99");
        assert_eq!(c.table, "Player");
        assert_eq!(c.action, Action::Update);
        assert_eq!(c.set[0], ("TeamID".to_string(), Value::Int(7)));
        assert_eq!(c.set[1], ("Salary".to_string(), Value::Real(1250000.5)));
        assert_eq!(c.key, Some(("PlayerID".to_string(), Value::Int(99))));
    }

    #[test]
    fn a_read_is_a_read_not_a_write() {
        assert_eq!(classify("SELECT TeamID FROM Player"), Kind::Read);
        assert_eq!(classify("select * from StaffPS"), Kind::Read);
        assert_eq!(classify("PRAGMA journal_mode"), Kind::Read);
    }

    #[test]
    fn a_multi_row_insert_is_refused_rather_than_half_applied() {
        // One row of many applied would diverge the two careers, which is worse than not replicating.
        match classify("INSERT INTO Player (a) VALUES (1), (2)") {
            Kind::Unsupported(_) => {}
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_compound_predicate_is_refused() {
        for sql in [
            "UPDATE Player SET TeamID = 7 WHERE PlayerID = 1 AND Season = 2",
            "UPDATE Player SET TeamID = 7 WHERE PlayerID = 1 OR 1=1",
            "UPDATE Player SET TeamID = 7 WHERE PlayerID IN (1,2)",
        ] {
            match classify(sql) {
                Kind::Unsupported(_) => {}
                other => panic!("{sql:?} must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_placeholder_is_not_a_value() {
        assert_eq!(parse_value("?"), None);
        assert_eq!(parse_value("?1"), None);
        assert_eq!(parse_value(":id"), None);
        // And a statement using one is unsupported, because the text does not carry the value.
        match classify("UPDATE Player SET TeamID = ? WHERE PlayerID = 99") {
            Kind::Unsupported(_) => {}
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_column_reference_is_not_a_value() {
        // `SET a = b` copies a column; replicating it as text "b" would be wrong.
        assert_eq!(parse_value("OtherColumn"), None);
        assert_eq!(parse_value("MAX(1,2)"), None);
        assert_eq!(parse_value("\"quoted\""), None);
    }

    #[test]
    fn a_semicolon_inside_a_string_does_not_split() {
        let stmts = split_statements("INSERT INTO t VALUES ('a; b'); UPDATE t SET x = 1 WHERE k = 2");
        assert_eq!(stmts.len(), 2, "{stmts:?}");
        assert!(stmts[0].ends_with("'a; b')"));
    }

    #[test]
    fn a_doubled_quote_is_an_escape_not_a_terminator() {
        // The statement needs a column list to be replicable at all (see the next test), so this one has it and
        // the point under test is the escaping: `O''Brien; Jr` is one string containing a quote and a semicolon.
        let stmts = split_statements("INSERT INTO t (name) VALUES ('O''Brien; Jr')");
        assert_eq!(stmts.len(), 1, "{stmts:?}");
        let c = change(&stmts[0]);
        assert_eq!(c.set[0].1, Value::Text("O'Brien; Jr".to_string()));
    }

    #[test]
    fn a_comment_or_unknown_statement_is_unsupported_not_guessed() {
        for sql in ["DROP TABLE Player", "DELETE FROM Player", "CREATE TABLE x (a)", "BEGIN", "COMMIT"] {
            match classify(sql) {
                Kind::Unsupported(t) => assert_eq!(t, sql),
                other => panic!("{sql:?} must be unsupported, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_huge_or_odd_value_is_parsed_strictly() {
        assert_eq!(parse_value("NULL"), Some(Value::Null));
        assert_eq!(parse_value("null"), Some(Value::Null));
        assert_eq!(parse_value("1e309"), None, "an overflow must not become inf");
        assert_eq!(parse_value(""), None);
        assert_eq!(parse_value("'"), None, "an unterminated literal");
        assert_eq!(parse_value(" 42 "), Some(Value::Int(42)));
    }

    #[test]
    fn quoted_identifiers_are_understood() {
        let c = change("UPDATE \"Player\" SET \"TeamID\" = 7 WHERE \"PlayerID\" = 99");
        assert_eq!(c.table, "Player");
        assert_eq!(c.set[0].0, "TeamID");
        assert_eq!(c.key, Some(("PlayerID".to_string(), Value::Int(99))));
    }

    #[test]
    fn a_column_count_mismatch_is_refused() {
        match classify("INSERT INTO Player (a, b) VALUES (1)") {
            Kind::Unsupported(_) => {}
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn insert_without_a_column_list_is_refused() {
        // Without the column list there is no way to know which column each value belongs to.
        match classify("INSERT INTO Player VALUES (1, 2, 3)") {
            Kind::Unsupported(_) => {}
            other => panic!("got {other:?}"),
        }
    }
}
