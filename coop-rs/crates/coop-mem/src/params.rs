//! Binds parameterised statements for `sqlite3_exec`.
//!
//! Why this module exists.
//!
//! The career database is only reachable through the game's own `sqlite3_exec`, which takes **no
//! bind array**. The game's own statement set, however, is written against parameters: scanning the
//! 26 `Content/Database/*PS.json` assets finds 853 statements using 58 distinct positional
//! parameters (`?1`..`?58`) and 337 distinct named parameters (`@TeamID`, `@OptSeasonID`, ...).
//!
//! Those statements cannot be executed as-is, and they must not be executed by string surgery. Two
//! concrete hazards, both real:
//!
//! * A named parameter with no value is **not** NULL in SQLite - it is the literal string `"@Day"`.
//!   `WHERE TeamID = @Day` then silently matches nothing instead of erroring.
//! * Player-visible text (a team name, a staff member's name) reaching SQL by concatenation is an
//!   injection vector on *our* side, even though the game's own values are trusted.
//!
//! So binding happens here, once, in a way that is testable without a game: values are type-tagged
//! and rendered as SQL literals, and a parameter that has no value is an **error**, never a
//! passthrough.
//!
//! Crucially, this module does **not** invent semantics. The game's statements are used verbatim
//! except for parameter substitution, so every cascade, trigger and invariant behaves as it does
//! when the player clicks the button.

use std::collections::BTreeMap;

/// A value to substitute for a statement parameter.
///
/// The variant matters: SQLite treats `0` and `"0"` differently in comparisons against TEXT columns,
/// and the game mixes both. Keeping the type explicit means a caller cannot accidentally bind the
/// string `"0"` where the game binds the integer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Int(i64),
    Real(String),
    Text(String),
    /// Already-validated SQL to insert verbatim. Deliberately NOT constructible from arbitrary text
    /// by the parser; only callers that know what they are doing use this (e.g. a column list).
    Raw(String),
}

impl Value {
    /// Renders the value as a SQL literal safe to embed in a statement.
    pub fn to_sql(&self) -> String {
        match self {
            Value::Null => "NULL".to_string(),
            Value::Int(i) => i.to_string(),
            Value::Real(r) => {
                // Reject anything that is not a plain numeric literal, so a "real" cannot smuggle
                // syntax through. Accepts 1, -1.5, 1e3, and rejects "; DROP".
                if r.parse::<f64>().is_ok() && !r.contains("--") && r.parse::<f64>().map(|f| f.is_finite()) == Ok(true) {
                    r.clone()
                } else {
                    // Unreachable for well-formed input; fall back to a quoted string rather than
                    // emitting unvalidated text.
                    Value::Text(r.clone()).to_sql()
                }
            }
            Value::Text(t) => quote(t),
            Value::Raw(s) => s.clone(),
        }
    }
}

/// Render `s` as a single-quoted SQL string literal.
///
/// SQLite escapes a single quote by doubling it. Nothing else needs escaping, and embedded NULs are
/// impossible in a Rust `&str` sourced from a JS string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// A parameter found in a statement, in the order it first appears in the text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Param {
    /// `?N` - 1-based, as SQLite numbers them.
    Positional(u32),
    /// `@Name` - stored without the `@`.
    Named(String),
}

impl std::fmt::Display for Param {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Param::Positional(n) => write!(f, "?{n}"),
            Param::Named(n) => write!(f, "@{n}"),
        }
    }
}

/// Values to bind, by parameter.
#[derive(Debug, Default, Clone)]
pub struct Bindings {
    positional: BTreeMap<u32, Value>,
    named: BTreeMap<String, Value>,
    /// When true, a parameter in the statement with no binding is an error. When false it is
    /// replaced with NULL.
    ///
    /// Defaults to true: silently NULLing an unbound parameter is exactly the failure mode that
    /// makes a statement match nothing and look like it worked.
    pub strict: bool,
}

impl Bindings {
    pub fn new() -> Self {
        Bindings { strict: true, ..Default::default() }
    }

    /// Non-strict: unbound parameters become NULL. Only for exploratory reads.
    pub fn lenient() -> Self {
        Bindings { strict: false, ..Default::default() }
    }

    pub fn pos(mut self, n: u32, v: Value) -> Self {
        self.positional.insert(n, v);
        self
    }

    pub fn named(mut self, name: &str, v: Value) -> Self {
        self.named.insert(name.trim_start_matches('@').to_string(), v);
        self
    }

    pub fn set_pos(&mut self, n: u32, v: Value) {
        self.positional.insert(n, v);
    }

    pub fn set_named(&mut self, name: &str, v: Value) {
        self.named.insert(name.trim_start_matches('@').to_string(), v);
    }
}

/// Scans a statement for parameters, in first-appearance order.
///
/// Skips single-quoted string literals, double-quoted identifiers and `--` line comments, so a `?`
/// inside a literal is not mistaken for a parameter. SQLite's `?NNN` form is supported; bare `?`
/// (auto-numbered) is rejected because the game never uses it and supporting it would silently
/// renumber against `?N` usage.
pub fn scan(sql: &str) -> Result<Vec<Param>, String> {
    let b = sql.as_bytes();
    let mut i = 0;
    let mut out: Vec<Param> = Vec::new();
    let mut seen_pos = std::collections::BTreeSet::new();
    let mut seen_named = std::collections::BTreeSet::new();

    while i < b.len() {
        match b[i] {
            b'\'' => {
                // single-quoted literal, '' escapes
                i += 1;
                loop {
                    if i >= b.len() {
                        return Err("unterminated string literal".to_string());
                    }
                    if b[i] == b'\'' {
                        if i + 1 < b.len() && b[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                if i >= b.len() {
                    return Err("unterminated quoted identifier".to_string());
                }
                i += 1;
            }
            b'-' if i + 1 < b.len() && b[i + 1] == b'-' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'?' => {
                i += 1;
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if start == i {
                    return Err(
                        "bare '?' parameters are not supported; the game's statements always use ?N"
                            .to_string(),
                    );
                }
                let n: u32 = sql[start..i]
                    .parse()
                    .map_err(|_| format!("parameter number out of range: ?{}", &sql[start..i]))?;
                if n == 0 {
                    return Err("?0 is not a valid SQLite parameter".to_string());
                }
                if seen_pos.insert(n) {
                    out.push(Param::Positional(n));
                }
            }
            b'@' => {
                i += 1;
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                if start == i {
                    return Err("'@' without a parameter name".to_string());
                }
                let name = sql[start..i].to_string();
                if seen_named.insert(name.clone()) {
                    out.push(Param::Named(name));
                }
            }
            _ => i += 1,
        }
    }
    Ok(out)
}

/// Substitutes every parameter with its bound value, returning a statement `sqlite3_exec` can run.
///
/// Returns an error naming the first unbound parameter, so a mistake reads as
/// `unbound parameter @OptTeamID` rather than as a statement that quietly does nothing.
pub fn bind(sql: &str, b: &Bindings) -> Result<String, String> {
    let params = scan(sql)?;
    if params.is_empty() {
        return Ok(sql.to_string());
    }

    // Validate first, so a partially-substituted statement is never produced.
    let mut missing: Vec<String> = Vec::new();
    for p in &params {
        let have = match p {
            Param::Positional(n) => b.positional.contains_key(n),
            Param::Named(n) => b.named.contains_key(n),
        };
        if !have && b.strict {
            missing.push(p.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "unbound parameter(s): {}. Pass a value, or use Bindings::lenient() to NULL them.",
            missing.join(", ")
        ));
    }

    // Replace in one pass over the original text, reusing the same scanner states. This is done by
    // re-walking with the same rules rather than by `replace()`, because `replace("@TeamID", ..)`
    // would also rewrite `@TeamID2`.
    let mut out = String::with_capacity(sql.len() + 32);
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut last = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                loop {
                    if i >= bytes.len() {
                        return Err("unterminated string literal".to_string());
                    }
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                i += 1;
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'?' => {
                let start = i;
                i += 1;
                let s = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let n: u32 = sql[s..i].parse().map_err(|_| "bad positional parameter".to_string())?;
                out.push_str(&sql[last..start]);
                out.push_str(&b.positional.get(&n).cloned().unwrap_or(Value::Null).to_sql());
                last = i;
            }
            b'@' => {
                let start = i;
                i += 1;
                let s = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let name = &sql[s..i];
                out.push_str(&sql[last..start]);
                out.push_str(&b.named.get(name).cloned().unwrap_or(Value::Null).to_sql());
                last = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[last..]);
    Ok(out)
}

/// Convenience: bind and return, naming the statement key on failure.
pub fn bind_keyed(key: &str, sql: &str, b: &Bindings) -> Result<String, String> {
    bind(sql, b).map_err(|e| format!("{key}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_positional_and_named() {
        let p = scan("SELECT a FROM t WHERE x = ?1 AND y = @TeamID AND z = ?2").unwrap();
        assert_eq!(
            p,
            vec![
                Param::Positional(1),
                Param::Named("TeamID".into()),
                Param::Positional(2)
            ]
        );
    }

    #[test]
    fn dedupes_repeated_parameters() {
        // Teams_AI_Records statements repeat @SeasonID; the scan must list it once.
        let p = scan("UPDATE t SET a = @Day WHERE SeasonID = @SeasonID AND (@OptTeamID IS NULL OR TeamID = @OptTeamID)")
            .unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Param::Named("Day".into()));
    }

    #[test]
    fn ignores_parameters_inside_literals_and_comments() {
        let p = scan("SELECT '?1' AS lit, \"@nope\" AS ident FROM t -- ?2 @also\nWHERE a = ?3").unwrap();
        assert_eq!(p, vec![Param::Positional(3)]);
    }

    #[test]
    fn handles_high_parameter_numbers() {
        // The game really does use ?58.
        let p = scan("SELECT ?58").unwrap();
        assert_eq!(p, vec![Param::Positional(58)]);
    }

    #[test]
    fn rejects_bare_question_mark() {
        assert!(scan("SELECT ? FROM t").is_err());
    }

    #[test]
    fn rejects_zero() {
        assert!(scan("SELECT ?0").is_err());
    }

    #[test]
    fn rejects_unterminated_literal() {
        assert!(scan("SELECT 'abc").is_err());
    }

    #[test]
    fn binds_the_real_set_player_team_statement() {
        // Verbatim from Database/TeamsPS.json, entry `SetPlayerTeam`.
        let sql = "UPDATE Player SET TeamID = ?1";
        let out = bind(sql, &Bindings::new().pos(1, Value::Int(7))).unwrap();
        assert_eq!(out, "UPDATE Player SET TeamID = 7");
    }

    #[test]
    fn binds_the_real_ai_disable_statement() {
        // Verbatim from Database/TeamsPS.json, `SetAITeamLastDecisionEvaluationDay`.
        let sql = "UPDATE Teams_AI_Records SET LastDecisionEvaluationDay = @Day \
                   WHERE SeasonID = @SeasonID AND (@OptTeamID IS NULL OR TeamID = @OptTeamID)";
        let out = bind(
            sql,
            &Bindings::new()
                .named("Day", Value::Int(999_999))
                .named("SeasonID", Value::Int(1))
                .named("OptTeamID", Value::Int(7)),
        )
        .unwrap();
        assert!(out.contains("LastDecisionEvaluationDay = 999999"));
        assert!(out.contains("SeasonID = 1"));
        assert!(out.contains("TeamID = 7"));
        // No parameter may survive as bare text: that would become a literal string in SQLite.
        assert!(!out.contains('@'));
        assert!(!out.contains('?'));
    }

    #[test]
    fn unbound_is_an_error_not_a_passthrough() {
        // This is the whole point: SQLite would otherwise bake the literal "@Day" into the query.
        let err = bind("SELECT * FROM t WHERE d = @Day", &Bindings::new()).unwrap_err();
        assert!(err.contains("@Day"), "error names the parameter: {err}");
    }

    #[test]
    fn lenient_nulls_instead_of_erroring() {
        let out = bind("SELECT * FROM t WHERE d = @Day", &Bindings::lenient()).unwrap();
        assert_eq!(out, "SELECT * FROM t WHERE d = NULL");
    }

    #[test]
    fn text_is_quoted_and_escaped() {
        let out = bind("SELECT * FROM t WHERE n = @Name", &Bindings::new().named("Name", Value::Text("O'Brien".into())))
            .unwrap();
        assert_eq!(out, "SELECT * FROM t WHERE n = 'O''Brien'");
    }

    #[test]
    fn text_cannot_break_out_of_the_literal() {
        // A staff or team name is player-visible and must never become syntax.
        let evil = "x'; DROP TABLE Player; --";
        let out = bind("SELECT * FROM t WHERE n = @Name", &Bindings::new().named("Name", Value::Text(evil.into())))
            .unwrap();
        assert_eq!(out, "SELECT * FROM t WHERE n = 'x''; DROP TABLE Player; --'");
        // Exactly one statement remains: the injected semicolon is inside the literal.
        assert_eq!(out.matches("DROP TABLE").count(), 1);
        assert!(out.starts_with("SELECT"));
        assert!(out.trim_end().ends_with('\''));
    }

    #[test]
    fn named_prefix_collision_does_not_clobber() {
        // @TeamID must not match inside @TeamID2. A naive replace() gets this wrong.
        let out = bind(
            "SELECT @TeamID, @TeamID2",
            &Bindings::new().named("TeamID", Value::Int(1)).named("TeamID2", Value::Int(2)),
        )
        .unwrap();
        assert_eq!(out, "SELECT 1, 2");
    }

    #[test]
    fn leaves_parameter_free_sql_untouched() {
        let sql = "SELECT Day FROM Player_State";
        assert_eq!(bind(sql, &Bindings::new()).unwrap(), sql);
    }

    #[test]
    fn real_value_rejects_non_numeric() {
        let out = bind("SELECT @R", &Bindings::new().named("R", Value::Real("1.5".into()))).unwrap();
        assert_eq!(out, "SELECT 1.5");
        let bad = bind("SELECT @R", &Bindings::new().named("R", Value::Real("1;DROP".into()))).unwrap();
        assert_eq!(bad, "SELECT '1;DROP'");
    }

    #[test]
    fn null_binds_as_null() {
        let out = bind("SELECT @X", &Bindings::new().named("X", Value::Null)).unwrap();
        assert_eq!(out, "SELECT NULL");
    }

    #[test]
    fn every_statement_in_the_asset_set_scans_cleanly() {
        // Guards against the scanner rejecting real game SQL. Run against the extracted assets when
        // they are present; skipped otherwise so the crate tests independently of the export.
        //
        // The statements are pulled out with a real scan of `"Value": "<sql>"` pairs rather than a
        // split(), because a split() runs past the end of one JSON object and into the next - which
        // produced a false "unterminated identifier" on BoardPS.json the first time this ran.
        let root = std::path::Path::new(r"E:\FModel\Output\Exports\F1Manager24\Content\Database");
        if !root.exists() {
            eprintln!("skipping: asset export not present");
            return;
        }

        fn unescape(s: &str) -> String {
            let mut out = String::with_capacity(s.len());
            let mut it = s.chars();
            while let Some(c) = it.next() {
                if c != '\\' {
                    out.push(c);
                    continue;
                }
                match it.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('/') => out.push('/'),
                    Some(other) => out.push(other),
                    None => break,
                }
            }
            out
        }

        // Returns the raw contents of every `"Value": "..."` string in the file.
        fn values(text: &str) -> Vec<String> {
            const NEEDLE: &str = "\"Value\": \"";
            let mut out = Vec::new();
            let mut from = 0;
            while let Some(rel) = text[from..].find(NEEDLE) {
                let start = from + rel + NEEDLE.len();
                let bytes = text.as_bytes();
                let mut i = start;
                let mut escaped = false;
                while i < bytes.len() {
                    let c = bytes[i];
                    if escaped {
                        escaped = false;
                    } else if c == b'\\' {
                        escaped = true;
                    } else if c == b'"' {
                        break;
                    }
                    i += 1;
                }
                out.push(unescape(&text[start..i]));
                from = i + 1;
            }
            out
        }

        let mut n = 0usize;
        let mut errors = Vec::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if !path.to_string_lossy().ends_with("PS.json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for sql in values(&text) {
                // Only SQL statements: the assets also carry a few non-SQL strings.
                let upper = sql.trim_start().to_ascii_uppercase();
                if !(upper.starts_with("SELECT")
                    || upper.starts_with("INSERT")
                    || upper.starts_with("UPDATE")
                    || upper.starts_with("DELETE")
                    || upper.starts_with("WITH"))
                {
                    continue;
                }
                n += 1;
                if let Err(e) = scan(&sql) {
                    errors.push(format!("{path:?}: {e}\n  {sql}"));
                }
            }
        }
        assert!(n > 500, "expected the full statement set, saw {n}");
        assert!(
            errors.is_empty(),
            "scanner rejected {} statement(s):\n{}",
            errors.len(),
            errors.join("\n")
        );
        eprintln!("scanned {n} statements cleanly");
    }
}
