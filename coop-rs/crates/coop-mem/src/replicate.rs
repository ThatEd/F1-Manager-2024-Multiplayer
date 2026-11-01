//! Deciding what a captured career statement should be replicated as.
//!
//! The bridge between capture and the wire: `sqlcap` says what the game wrote, `coop_auth` says whether this player
//! may write it, and `coop_proto::Msg::Stmt` is what goes out. Kept pure - no I/O, no globals - so every decision is
//! testable against a fake player and mode.
//!
//! Refusing is the default on every uncertainty. The failure mode of guessing is a wrong row in the other player's
//! career, which stays invisible until it matters; the cost of refusing is a change that does not replicate, which
//! is reported.

use crate::sqlcap::{Change, Value};
use coop_auth::{Denied, Mode, Player, RowKey, Scope};

/// One replicated change, ready to be given a sequence number and encoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub table: String,
    pub key_column: String,
    pub key_value: Value,
    pub set: Vec<(String, Value)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// No `coop_mode.txt`/`coop_player.txt`: single-player maintenance. Nothing must be sent.
    NotConfigured,
    /// An `INSERT` has no key in the statement text, and guessing one would be a gamble on a database we only
    /// observe.
    NoKey,
    /// The change names no team, so it cannot be attributed to a player.
    NoTeam,
    /// A table the mod does not classify. Refused, because writing into a table we do not understand is how a
    /// career gets corrupted with nothing reporting it.
    UnknownTable(String),
    /// The statement's predicate is not the table's declared single-column key, so one key cannot name the row.
    WrongKey { table: String, found: String, expected: &'static str },
    /// The mode rules said no.
    Denied(Denied),
}

impl Refused {
    pub fn reason(&self) -> String {
        match self {
            Refused::NotConfigured => "co-op not configured (single-player); nothing replicated".to_string(),
            Refused::NoKey => "the statement carries no row key, so the row cannot be identified".to_string(),
            Refused::NoTeam => "the statement names no team, so the row cannot be attributed to a player".to_string(),
            Refused::UnknownTable(t) => format!("table {t:?} is not classified as replicable"),
            Refused::WrongKey { table, found, expected } => format!(
                "{table} is keyed by {expected}, but the statement selects on {found}; one key cannot name that row"
            ),
            Refused::Denied(d) => format!("refused by the mode rules: {}", d.as_str()),
        }
    }
}

/// Tables where a replicated single-row write is meaningful.
///
/// The list now comes from [`crate::schema`], derived from the game's own `Volta` database rather than from the
/// names of the files in `Content/Database`. The previous list was `Player`, `StaffPS`, `PartsPS`, `SponsorshipPS`,
/// `FinancesPS`, `ScoutingPS`, `CarPS`, `TeamPS` - seven of which are `PSCollection` **asset** names, not tables, so
/// a real career change could never match one. `tools/schema_truth.py` cross-checks the derived list.
///
/// Still an allowlist rather than a denylist. A denylist has to enumerate every table that must *not* be
/// replicated, so a table added by a game patch would silently become replicable. An allowlist fails the other way:
/// a new table is simply not replicated, reported as `UnknownTable`, which costs a feature rather than a career.
pub fn is_replicable_table(table: &str) -> bool {
    crate::schema::lookup(table).is_some()
}

/// The team a change belongs to, read from the team column the schema declares for that table.
///
/// Reads the decoded values rather than re-parsing SQL: `sqlcap` has already done that work. The column name comes
/// from the schema entry rather than from a guess, because guessing is how the phantom table names were produced;
/// every entry in [`crate::schema::REPLICABLE`] carries the team column the database actually declares.
fn team_in(change: &Change, team_column: &str) -> Option<u32> {
    change
        .set
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(team_column))
        .and_then(|(_, v)| match v {
            Value::Int(i) if *i >= 0 => Some(*i as u32),
            _ => None,
        })
}

/// The loadout (car) a change names, when it names one. `None` is fine for team-scoped rows.
fn loadout_in(change: &Change) -> Option<u32> {
    change
        .set
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case("LoadoutID"))
        .and_then(|(_, v)| match v {
            Value::Int(i) if *i >= 0 => Some(*i as u32),
            _ => None,
        })
}

/// Decide what to replicate, or why not.
pub fn plan(change: &Change, configured: Option<(Mode, Player)>) -> Result<Planned, Refused> {
    let (mode, player) = configured.ok_or(Refused::NotConfigured)?;

    // The table is looked up **first**, before anything about the row is interpreted. Two reasons: a table that is
    // not replicable should be reported as such rather than as `NoTeam` (which is what the old order produced for
    // every real write once the allowlist was wrong), and the schema entry is what says which column names the team
    // and which column is the key. Guessing either from the statement is how the phantom table names arose.
    let schema = crate::schema::lookup(&change.table).ok_or_else(|| Refused::UnknownTable(change.table.clone()))?;

    // `UPDATE ... WHERE k = v` carries its own key, so it replicates directly. The statement's own predicate must
    // be the single key column the schema declares, or `Msg::Stmt`'s one key cannot identify the row: `UPDATE
    // Staff_Contracts SET ... WHERE StaffID = 8` names one column, but the row is `(StaffID, ContractType, TeamID)`,
    // so applying it at the far end would hit both the Current and the Future contract. That table is deliberately
    // absent from `schema::REPLICABLE`; this check is the second barrier for a table that is present but whose
    // statement uses a different predicate.
    let (key_col, key_val) = match &change.key {
        Some((c, v)) => (c.clone(), v.clone()),
        None => return Err(Refused::NoKey),
    };
    if !key_col.eq_ignore_ascii_case(schema.pk) {
        return Err(Refused::WrongKey {
            table: change.table.clone(),
            found: key_col,
            expected: schema.pk,
        });
    }

    let team = team_in(change, schema.team).ok_or(Refused::NoTeam)?;
    let key = RowKey {
        team: Some(team),
        loadout: loadout_in(change),
        car: None,
    };
    // Career rows belong to a team, not to a car - the car-scoped path is the race sim, which does not come
    // through here.
    coop_auth::may_write(mode, &player, Scope::PerTeam, &key).map_err(Refused::Denied)?;

    Ok(Planned {
        table: schema.name.to_string(),
        key_column: schema.pk.to_string(),
        key_value: key_val,
        set: change.set.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlcap::Action;

    fn player(team: u32) -> Player {
        Player::new(team, 0, true)
    }

    fn update(table: &str, team: i64, key: i64) -> Change {
        Change {
            table: table.to_string(),
            action: Action::Update,
            set: vec![
                ("TeamID".to_string(), Value::Int(team)),
                ("TeamPrincipalRating".to_string(), Value::Int(key)),
            ],
            key: Some(("TeamID".to_string(), Value::Int(key))),
        }
    }

    #[test]
    fn a_configured_update_of_my_own_team_replicates() {
        let cfg = Some((Mode::Versus, player(7)));
        let p = plan(&update("Teams", 7, 42), cfg).expect("should replicate");
        assert_eq!(p.table, "Teams");
        assert_eq!(p.key_column, "TeamID");
        assert_eq!(p.key_value, Value::Int(42));
        assert_eq!(p.set.len(), 2);
    }

    #[test]
    fn a_phantom_asset_name_is_refused_as_an_unknown_table() {
        // The eight-name allowlist this replaced was mostly `PSCollection` asset names. Reporting one as
        // `UnknownTable` rather than as `NoTeam` matters: `NoTeam` said "the statement did not name a team", when
        // the truth was "this table does not exist".
        let cfg = Some((Mode::Versus, player(7)));
        assert_eq!(
            plan(&update("StaffPS", 7, 1), cfg),
            Err(Refused::UnknownTable("StaffPS".to_string()))
        );
    }

    #[test]
    fn an_unconfigured_install_replicates_nothing() {
        // Single-player maintenance must not put anything on the wire.
        assert_eq!(plan(&update("Teams", 7, 1), None), Err(Refused::NotConfigured));
    }

    #[test]
    fn another_teams_row_is_refused() {
        let cfg = Some((Mode::Versus, player(7)));
        assert!(matches!(plan(&update("Teams", 9, 1), cfg), Err(Refused::Denied(_))));
    }

    #[test]
    fn an_insert_is_refused_because_its_key_is_not_in_the_text() {
        let mut c = update("Teams", 7, 1);
        c.key = None;
        c.action = Action::Insert;
        let cfg = Some((Mode::Versus, player(7)));
        assert_eq!(plan(&c, cfg), Err(Refused::NoKey));
    }

    #[test]
    fn a_change_without_a_team_is_refused() {
        let mut c = update("Teams", 7, 1);
        c.set.retain(|(k, _)| k != "TeamID");
        let cfg = Some((Mode::Versus, player(7)));
        // `Teams` has no `TeamID` in its values any more, so the row cannot be attributed.
        assert_eq!(plan(&c, cfg), Err(Refused::NoTeam));
    }

    #[test]
    fn a_statement_keyed_on_something_other_than_the_primary_key_is_refused() {
        // `` WHERE StaffID = 8` names one column, but `Staff_Team_CAS` is keyed by `StaffID` - so this checks the
        // rule with a table that *is* replicable: `Teams` is keyed `TeamID`, and a statement selecting on
        // `TeamName` cannot identify a row through `Msg::Stmt`'s single integer key.
        let mut c = update("Teams", 7, 1);
        c.key = Some(("TeamName".to_string(), Value::Text("McLaren".to_string())));
        let cfg = Some((Mode::Versus, player(7)));
        match plan(&c, cfg) {
            Err(Refused::WrongKey { table, found, expected }) => {
                assert_eq!(table, "Teams");
                assert_eq!(found, "TeamName");
                assert_eq!(expected, "TeamID");
            }
            other => panic!("expected WrongKey, got {other:?}"),
        }
    }

    #[test]
    fn a_composite_key_table_is_refused_because_one_key_cannot_name_the_row() {
        // `Staff_Contracts` is `PRIMARY KEY (StaffID, ContractType, TeamID)`. `UPDATE ... WHERE StaffID = 8` would
        // hit both the Current and the Future contract at the far end, so the row must not travel at all.
        assert!(lookup_absent("Staff_Contracts"));
        let cfg = Some((Mode::Versus, player(7)));
        let mut c = update("Staff_Contracts", 7, 8);
        c.key = Some(("StaffID".to_string(), Value::Int(8)));
        assert_eq!(
            plan(&c, cfg),
            Err(Refused::UnknownTable("Staff_Contracts".to_string()))
        );
    }

    fn lookup_absent(t: &str) -> bool {
        crate::schema::lookup(t).is_none()
    }

    #[test]
    fn an_unclassified_table_is_refused_rather_than_written() {
        let cfg = Some((Mode::Versus, player(7)));
        assert_eq!(
            plan(&update("SomeNewTable", 7, 1), cfg),
            Err(Refused::UnknownTable("SomeNewTable".to_string()))
        );
    }

    #[test]
    fn the_allowlist_covers_career_tables_and_excludes_engine_ones() {
        for t in ["Teams", "Teams_Colours", "teams", "Parts_Designs"] {
            assert!(is_replicable_table(t), "{t} should be replicable");
        }
        // Engine-computed tables are the host's authoritative copy; replicating a write into them would let one
        // player rewrite results. The `*PS` names are statement catalogues, not tables, so they must not match
        // either - that is the bug this replaced.
        for t in ["RacesPS", "StaffPS", "StandingsPS", "CalendarPS", "sqlite_master", ""] {
            assert!(!is_replicable_table(t), "{t} must not be replicated");
        }
    }

    #[test]
    fn a_refusal_always_says_something_actionable() {
        for r in [
            Refused::NotConfigured,
            Refused::NoKey,
            Refused::NoTeam,
            Refused::UnknownTable("X".into()),
            Refused::WrongKey { table: "T".into(), found: "A".into(), expected: "B" },
        ] {
            let s = r.reason();
            assert!(s.len() > 20, "reason too terse: {s:?}");
        }
    }
}
