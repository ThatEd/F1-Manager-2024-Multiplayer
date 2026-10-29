//! The game's own statement set, loaded from the `Content/Database/*PS.json` assets.
//!
//! Why load them rather than hardcode the few we need.
//!
//! Every career mutation the game performs is a named statement in one of 26 `*PS.json` assets
//! (853 statements in total). Reusing them means the co-op path inherits the game's own cascades,
//! triggers and invariants - a contract signed through `OnSubmitContractOffer`'s SQL behaves exactly
//! as it does when the player clicks the button. Re-deriving that SQL by hand would be both larger
//! and subtly wrong.
//!
//! Two consequences worth stating plainly:
//!
//! * The SQL text is **not** in the executable. Only the statement *key names* are (they appear as
//!   adjacent strings around `0x5eabc40`, e.g. `GetTeamName`, `GetTeamFormula`, `SetPlayerTeam`,
//!   `PurgeMailChains`, `PurgeEventLogsUpTo`). So the assets are the only source, and a missing
//!   export means this module cannot work.
//! * These are `.uasset` in the shipped game; the `.json` files are an FModel export. Reading them
//!   is a development convenience, not a runtime dependency of the game.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One statement from one asset.
#[derive(Debug, Clone)]
pub struct Statement {
    /// The asset it came from, e.g. `TeamsPS`.
    pub table: String,
    /// The key, e.g. `SetPlayerTeam`.
    pub key: String,
    /// The SQL, with its game parameters intact.
    pub sql: String,
}

/// Every statement the game knows, indexed by key.
#[derive(Debug, Default)]
pub struct Catalog {
    by_key: BTreeMap<String, Vec<Statement>>,
    statements: Vec<Statement>,
}

impl Catalog {
    /// Loads every `*PS.json` in `dir`.
    ///
    /// A key can legitimately appear in more than one asset, so lookups return a slice: the caller
    /// must decide which table it means rather than have this module guess.
    pub fn load(dir: &Path) -> Result<Self, String> {
        if !dir.exists() {
            return Err(format!(
                "statement assets not found at {}. Point --database at Content/Database.",
                dir.display()
            ));
        }
        let mut cat = Catalog::default();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.to_string_lossy().ends_with("PS.json"))
            .collect();
        entries.sort();

        for path in entries {
            let table = path
                .file_stem()
                .map(|s| s.to_string_lossy().trim_end_matches(".json").to_string())
                .unwrap_or_default();
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            for (key, sql) in parse_entries(&text) {
                let st = Statement { table: table.clone(), key: key.clone(), sql };
                cat.by_key.entry(key).or_default().push(st.clone());
                cat.statements.push(st);
            }
        }
        if cat.statements.is_empty() {
            return Err(format!("no statements parsed from {}", dir.display()));
        }
        Ok(cat)
    }

    /// Looks a key up, optionally narrowed to one asset.
    pub fn get(&self, key: &str, table: Option<&str>) -> Result<&Statement, String> {
        let all = self
            .by_key
            .get(key)
            .ok_or_else(|| format!("no statement named {key:?} in the asset set"))?;
        match table {
            Some(t) => all
                .iter()
                .find(|s| s.table == t)
                .ok_or_else(|| format!("statement {key:?} is not in {t:?}")),
            None if all.len() == 1 => Ok(&all[0]),
            None => Err(format!(
                "statement {key:?} is ambiguous, it exists in {}: pass --table",
                all.iter().map(|s| s.table.as_str()).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    pub fn len(&self) -> usize {
        self.statements.len()
    }

    pub fn is_empty(&self) -> bool {
        self.statements.is_empty()
    }

    /// Keys ending in a substring, for discovery (`coop-sql --find Player`).
    pub fn find(&self, needle: &str) -> Vec<&Statement> {
        let n = needle.to_ascii_lowercase();
        self.statements
            .iter()
            .filter(|s| s.key.to_ascii_lowercase().contains(&n))
            .collect()
    }
}

/// Splits `{"Key": "...", "Value": "..."}` entries out of an asset's JSON.
///
/// A hand-rolled scan rather than a JSON dependency: the crate has no third-party dependencies and
/// the asset shape here is fixed and flat (an array of objects with `Key` then `Value`). It handles
/// escaped quotes and backslashes, which real statements do contain.
fn parse_entries(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;

    fn read_string(bytes: &[u8], mut i: usize) -> Option<(String, usize)> {
        // expects bytes[i] == b'"'
        if i >= bytes.len() || bytes[i] != b'"' {
            return None;
        }
        i += 1;
        let start = i;
        let mut escaped = false;
        while i < bytes.len() {
            let c = bytes[i];
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                return Some((unescape(&String::from_utf8_lossy(&bytes[start..i])), i + 1));
            }
            i += 1;
        }
        None
    }

    let mut key: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let Some((name, next)) = read_string(bytes, i) else { break };
        i = next;
        // Skip whitespace and the colon.
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b':' {
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'"' {
            if let Some((value, next)) = read_string(bytes, i) {
                i = next;
                match name.as_str() {
                    "Key" => key = Some(value),
                    "Value" => {
                        if let Some(k) = key.take() {
                            out.push((k, value));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = r"E:\FModel\Output\Exports\F1Manager24\Content\Database";

    #[test]
    fn parses_key_value_pairs() {
        let text = r#"{"Properties": {"Entries": [{"Key": "A", "Value": "SELECT 1"}, {"Key": "B", "Value": "SELECT 2"}]}}"#;
        let v = parse_entries(text);
        assert_eq!(v, vec![("A".into(), "SELECT 1".into()), ("B".into(), "SELECT 2".into())]);
    }

    #[test]
    fn parses_statements_containing_escaped_quotes() {
        // Real assets contain things like: WHERE Name = "Current"
        let text = r#"{"Key": "X", "Value": "SELECT * FROM T WHERE Name = \"Current\""}"#;
        let v = parse_entries(text);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].1, r#"SELECT * FROM T WHERE Name = "Current""#);
    }

    #[test]
    fn loads_the_real_catalog() {
        if !Path::new(DB).exists() {
            eprintln!("skipping: asset export not present");
            return;
        }
        let cat = Catalog::load(Path::new(DB)).unwrap();
        eprintln!("loaded {} statements", cat.len());
        assert!(cat.len() > 800, "expected the full set, got {}", cat.len());

        // The two statements the retarget flow depends on must be there, verbatim.
        let sp = cat.get("SetPlayerTeam", Some("TeamsPS")).unwrap();
        assert_eq!(sp.sql.trim(), "UPDATE Player SET TeamID = ?1");

        let ai = cat.get("SetAITeamLastDecisionEvaluationDay", Some("TeamsPS")).unwrap();
        assert!(ai.sql.contains("UPDATE Teams_AI_Records SET LastDecisionEvaluationDay"));
        assert!(ai.sql.contains("@OptTeamID"));

        let ensure = cat.get("EnsureAITeamRecordsExist", Some("TeamsPS")).unwrap();
        assert!(ensure.sql.contains("INSERT OR IGNORE INTO Teams_AI_Records"));

        let ai_get = cat.get("GetAITeamLastDecisionEvaluationDay", Some("TeamsPS")).unwrap();
        assert!(ai_get.sql.contains("SELECT LastDecisionEvaluationDay FROM Teams_AI_Records"));
    }

    #[test]
    fn unknown_key_is_a_clear_error() {
        if !Path::new(DB).exists() {
            return;
        }
        let cat = Catalog::load(Path::new(DB)).unwrap();
        let e = cat.get("NoSuchStatement", None).unwrap_err();
        assert!(e.contains("NoSuchStatement"));
    }

    #[test]
    fn find_locates_player_and_ai_keys() {
        if !Path::new(DB).exists() {
            return;
        }
        let cat = Catalog::load(Path::new(DB)).unwrap();
        let keys: Vec<&str> = cat.find("PlayerTeam").iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"SetPlayerTeam"), "found: {keys:?}");
        let ai: Vec<&str> = cat.find("AITeamLastDecision").iter().map(|s| s.key.as_str()).collect();
        assert!(ai.contains(&"SetAITeamLastDecisionEvaluationDay"), "found: {ai:?}");
    }
}
