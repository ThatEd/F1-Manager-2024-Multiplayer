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
            Refused::Denied(d) => format!("refused by the mode rules: {}", d.as_str()),
        }
    }
}

/// Tables where a replicated single-row write is meaningful.
///
/// Deliberately an allowlist rather than a denylist. A denylist has to enumerate every table that must *not* be
/// replicated, so a table added by a game patch would silently become replicable. An allowlist fails the other way:
/// a new table is simply not replicated, reported as `UnknownTable`, which costs a feature rather than a career.
const REPLICABLE: [&str; 8] = [
    "Player",
    "StaffPS",
    "PartsPS",
    "SponsorshipPS",
    "FinancesPS",
    "ScoutingPS",
    "CarPS",
    "TeamPS",
];

pub fn is_replicable_table(table: &str) -> bool {
    REPLICABLE.iter().any(|t| table.eq_ignore_ascii_case(t))
}

/// The team a change belongs to, from a `TeamID`-shaped column among its own values.
///
/// Reads the decoded values rather than re-parsing SQL: `sqlcap` has already done that work, and matching the column
/// name covers every career table seen so far. A table that keys by team some other way yields `None` and is
/// refused, which is the safe direction.
fn team_in(change: &Change) -> Option<u32> {
    change
        .set
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case("TeamID") || c.eq_ignore_ascii_case("Team"))
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

    // `UPDATE ... WHERE k = v` carries its own key, so it replicates directly.
    let (key_col, key_val) = match &change.key {
        Some((c, v)) => (c.clone(), v.clone()),
        None => return Err(Refused::NoKey),
    };

    let team = team_in(change).ok_or(Refused::NoTeam)?;
    let key = RowKey {
        team: Some(team),
        loadout: loadout_in(change),
        car: None,
    };
    // Career rows belong to a team, not to a car - the car-scoped path is the race sim, which does not come
    // through here.
    coop_auth::may_write(mode, &player, Scope::PerTeam, &key).map_err(Refused::Denied)?;

    if !is_replicable_table(&change.table) {
        return Err(Refused::UnknownTable(change.table.clone()));
    }

    Ok(Planned {
        table: change.table.clone(),
        key_column: key_col,
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
                ("Salary".to_string(), Value::Int(1000)),
            ],
            key: Some(("StaffID".to_string(), Value::Int(key))),
        }
    }

    #[test]
    fn a_configured_update_of_my_own_team_replicates() {
        let cfg = Some((Mode::Versus, player(7)));
        let p = plan(&update("StaffPS", 7, 42), cfg).expect("should replicate");
        assert_eq!(p.table, "StaffPS");
        assert_eq!(p.key_column, "StaffID");
        assert_eq!(p.key_value, Value::Int(42));
        assert_eq!(p.set.len(), 2);
    }

    #[test]
    fn an_unconfigured_install_replicates_nothing() {
        // Single-player maintenance must not put anything on the wire.
        assert_eq!(plan(&update("StaffPS", 7, 1), None), Err(Refused::NotConfigured));
    }

    #[test]
    fn another_teams_row_is_refused() {
        let cfg = Some((Mode::Versus, player(7)));
        assert!(matches!(plan(&update("StaffPS", 9, 1), cfg), Err(Refused::Denied(_))));
    }

    #[test]
    fn an_insert_is_refused_because_its_key_is_not_in_the_text() {
        let mut c = update("StaffPS", 7, 1);
        c.key = None;
        c.action = Action::Insert;
        let cfg = Some((Mode::Versus, player(7)));
        assert_eq!(plan(&c, cfg), Err(Refused::NoKey));
    }

    #[test]
    fn a_change_without_a_team_is_refused() {
        let mut c = update("StaffPS", 7, 1);
        c.set.retain(|(k, _)| k != "TeamID");
        let cfg = Some((Mode::Versus, player(7)));
        assert_eq!(plan(&c, cfg), Err(Refused::NoTeam));
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
        for t in ["Player", "StaffPS", "PartsPS", "player"] {
            assert!(is_replicable_table(t), "{t} should be replicable");
        }
        // Engine-computed tables are the host's authoritative copy; replicating a write into them would let one
        // player rewrite results.
        for t in ["RacesPS", "StandingsPS", "CalendarPS", "sqlite_master", ""] {
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
        ] {
            let s = r.reason();
            assert!(s.len() > 20, "reason too terse: {s:?}");
        }
    }
}
