//! Retargets a career's "player team" and disables that team's AI.
//!
//! The whole of this is, as [TEAM-RETARGET.md] found, **one integer and one column**:
//!
//! * `Player.TeamID` (single row) decides which team the UI presents. The UI is already
//!   team-agnostic - every management screen scopes itself off the engine-published
//!   `playerTeamID` - so retargeting is a data change, not a UI change.
//! * `Teams_AI_Records.LastDecisionEvaluationDay` per `(TeamID, SeasonID)` is the AI's only cadence
//!   gate. There is no "AI enabled" flag anywhere in the schema or the binary.
//!
//! This module is deliberately **pure**: it plans statements and reads nothing. Executing them is
//! [`crate::livedb::LiveDb`]'s job. Keeping the planning separate means the logic is testable
//! without a game, which matters because the interesting parts here are the invariants, not the
//! calls.
//!
//! ## The thing that is easy to get wrong
//!
//! The AI disable must be applied on the **host**, for the **client's** team. On the client that
//! team is the player's team, so the AI presumably skips it anyway; but on the host's copy it is an
//! ordinary AI team that will keep making its own decisions while the client also acts for it. That
//! is the non-obvious half of the design, and [`Plan::for_host`] exists so it cannot be forgotten.

use crate::params::{bind, Bindings, Value};
use crate::statements::Catalog;

/// A sentinel far beyond any plausible career day.
///
/// Chosen to be obviously out of range rather than merely large, so a stale value is recognisable in
/// a database dump. NOTE: the AI gate's comparison operator lives in native code we have not read,
/// so this is inferred to work by comparison (`LastDecisionEvaluationDay < Day`) rather than by
/// looping. See TEAM-RETARGET.md gap 1.
pub const AI_DISABLED_DAY: i64 = 999_999;

/// Which side of the pair we are planning for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The machine whose career is authoritative.
    Host,
    /// The machine being retargeted to its own team.
    Client,
}

/// A statement ready to run, with the key it came from for logging.
#[derive(Debug, Clone)]
pub struct Planned {
    pub key: String,
    pub sql: String,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub role: Role,
    /// Team the UI will present (the client's team).
    pub team_id: i64,
    pub season_id: i64,
    pub statements: Vec<Planned>,
}

impl Plan {
    /// Full retarget: this machine's UI becomes `team_id`, and `team_id`'s AI is stopped.
    ///
    /// For a client both steps are wanted. For a host only the AI step is wanted - the host keeps
    /// its own team - which is why [`Plan::for_host`] exists and is the one the pair flow calls.
    pub fn for_client(cat: &Catalog, team_id: i64, season_id: i64) -> Result<Plan, String> {
        let mut statements = Vec::new();
        statements.push(build(cat, "SetPlayerTeam", Some("TeamsPS"), &{
            let mut b = Bindings::new();
            b.set_pos(1, Value::Int(team_id));
            b
        })?);
        statements.push(ai_disable(cat, team_id, season_id)?);
        Ok(Plan { role: Role::Client, team_id, season_id, statements })
    }

    /// Host side: no `Player.TeamID` change, but the client's team must stop deciding for itself.
    pub fn for_host(cat: &Catalog, client_team_id: i64, season_id: i64) -> Result<Plan, String> {
        Ok(Plan {
            role: Role::Host,
            team_id: client_team_id,
            season_id,
            statements: vec![ai_disable(cat, client_team_id, season_id)?],
        })
    }

    /// Undo the AI disable, e.g. if the pairing is dissolved and the team should go back to the AI.
    pub fn ai_reenable(cat: &Catalog, team_id: i64, season_id: i64, day: i64) -> Result<Plan, String> {
        let mut b = Bindings::new();
        b.set_named("Day", Value::Int(day));
        b.set_named("SeasonID", Value::Int(season_id));
        b.set_named("OptTeamID", Value::Int(team_id));
        Ok(Plan {
            role: Role::Host,
            team_id,
            season_id,
            statements: vec![build(cat, "SetAITeamLastDecisionEvaluationDay", Some("TeamsPS"), &b)?],
        })
    }

    /// Read-back statement proving the plan took effect.
    pub fn verify_sql(&self, cat: &Catalog) -> Result<String, String> {
        // `GetPlayerData` returns TeamID without parameters, so it is safe to run as-is.
        let key = match self.role {
            Role::Client => "GetPlayerData",
            Role::Host => "GetAITeamLastDecisionEvaluationDay",
        };
        let table = match self.role {
            Role::Client => "PlayerPS",
            Role::Host => "TeamsPS",
        };
        let st = cat.get(key, Some(table))?;
        if self.role == Role::Client {
            Ok(st.sql.clone())
        } else {
            let mut b = Bindings::new();
            b.set_pos(1, Value::Int(self.team_id));
            b.set_pos(2, Value::Int(self.season_id));
            bind(&st.sql, &b)
        }
    }
}

fn build(cat: &Catalog, key: &str, table: Option<&str>, b: &Bindings) -> Result<Planned, String> {
    let st = cat.get(key, table)?;
    let sql = bind(&st.sql, b).map_err(|e| format!("{key}: {e}"))?;
    Ok(Planned { key: key.to_string(), sql })
}

fn ai_disable(cat: &Catalog, team_id: i64, season_id: i64) -> Result<Planned, String> {
    // The game's own statement is:
    //   UPDATE Teams_AI_Records SET LastDecisionEvaluationDay = @Day
    //    WHERE SeasonID = @SeasonID AND (@OptTeamID IS NULL OR TeamID = @OptTeamID)
    // `@OptTeamID` MUST be bound to the team, or the statement updates EVERY team in the season.
    let mut b = Bindings::new();
    b.set_named("Day", Value::Int(AI_DISABLED_DAY));
    b.set_named("SeasonID", Value::Int(season_id));
    b.set_named("OptTeamID", Value::Int(team_id));
    build(cat, "SetAITeamLastDecisionEvaluationDay", Some("TeamsPS"), &b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const DB: &str = r"E:\FModel\Output\Exports\F1Manager24\Content\Database";

    fn cat() -> Option<Catalog> {
        if !Path::new(DB).exists() {
            eprintln!("skipping: asset export not present");
            return None;
        }
        Some(Catalog::load(Path::new(DB)).unwrap())
    }

    #[test]
    fn client_plan_sets_team_and_stops_ai() {
        let Some(c) = cat() else { return };
        let p = Plan::for_client(&c, 7, 1).unwrap();
        assert_eq!(p.statements.len(), 2);
        assert_eq!(p.statements[0].key, "SetPlayerTeam");
        assert_eq!(p.statements[0].sql.trim(), "UPDATE Player SET TeamID = 7");
        assert_eq!(p.statements[1].key, "SetAITeamLastDecisionEvaluationDay");
    }

    #[test]
    fn host_plan_does_not_change_player_team() {
        let Some(c) = cat() else { return };
        let p = Plan::for_host(&c, 7, 1).unwrap();
        assert_eq!(p.role, Role::Host);
        assert_eq!(p.statements.len(), 1, "host must not touch Player.TeamID");
        assert!(p.statements.iter().all(|s| s.key != "SetPlayerTeam"));
    }

    #[test]
    fn ai_disable_is_scoped_to_one_team_and_season() {
        // The dangerous failure mode: @OptTeamID left unbound turns the UPDATE into
        // "disable the AI for every team in the season".
        let Some(c) = cat() else { return };
        let p = Plan::for_client(&c, 7, 3).unwrap();
        let sql = &p.statements[1].sql;
        assert!(sql.contains("TeamID = 7"), "must scope to the team: {sql}");
        assert!(sql.contains("SeasonID = 3"), "must scope to the season: {sql}");
        assert!(sql.contains(&AI_DISABLED_DAY.to_string()), "must push the day out: {sql}");
        assert!(!sql.contains("@OptTeamID"), "parameter must be bound, not left as text: {sql}");
        assert!(!sql.contains('?'), "no positional parameter may survive: {sql}");

        // The game's own WHERE is `(@OptTeamID IS NULL OR TeamID = @OptTeamID)`. Binding the value
        // leaves the literal text `(7 IS NULL OR TeamID = 7)`, which SQLite evaluates to
        // `FALSE OR TeamID = 7`. So the IS NULL arm survives on purpose and is harmless - the real
        // invariant is that the surviving arm is a bound constant, never a bare parameter, and that
        // the team filter is present. Asserting `!contains("IS NULL")` would fail on correct output.
        assert!(
            sql.contains("(7 IS NULL OR TeamID = 7)"),
            "expected the bound optional-team clause, got: {sql}"
        );
        // Whatever precedes the OR must be a literal, so the clause cannot go NULL-true for everyone.
        assert!(!sql.contains("(@"), "no parameter may remain inside the optional clause: {sql}");
    }

    #[test]
    fn ai_disable_never_yields_an_unbounded_where() {
        // A statement that would hit every row is the one thing this must never emit.
        let Some(c) = cat() else { return };
        for team in [1, 2, 7, 11] {
            let p = Plan::for_client(&c, team, 1).unwrap();
            let sql = &p.statements[1].sql;
            assert!(sql.contains(&format!("TeamID = {team}")), "{sql}");
        }
    }

    #[test]
    fn reenable_writes_a_sane_day() {
        let Some(c) = cat() else { return };
        let p = Plan::ai_reenable(&c, 7, 1, 42).unwrap();
        assert!(p.statements[0].sql.contains("LastDecisionEvaluationDay = 42"));
        assert!(p.statements[0].sql.contains("TeamID = 7"));
    }

    #[test]
    fn verify_sql_is_bound_and_safe() {
        let Some(c) = cat() else { return };
        let client = Plan::for_client(&c, 7, 1).unwrap();
        let v = client.verify_sql(&c).unwrap();
        assert!(v.contains("FROM Player"));
        assert!(!v.contains('?') && !v.contains('@'));

        let host = Plan::for_host(&c, 7, 1).unwrap();
        let v = host.verify_sql(&c).unwrap();
        assert!(v.contains("LastDecisionEvaluationDay"));
        assert!(v.contains("TeamID = 7"));
        assert!(!v.contains('?') && !v.contains('@'));
    }

    #[test]
    fn sentinel_is_obviously_out_of_range() {
        // A career day is a small integer; this exists so a stale disable is recognisable.
        assert!(AI_DISABLED_DAY > 100_000);
        assert_eq!(AI_DISABLED_DAY % 9, 0, "documented as 999999");
    }
}
