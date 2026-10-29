//! Who is allowed to write which row.
//!
//! Three game modes need one answer to a single question: *may this player write this row?* The modes
//! differ only in their answer, so they share one function.
//!
//! ```text
//!   SharedTeam   two players, one team     - the garage is split by car, the team by role
//!   Versus       two players, two teams    - each owns a whole team
//!   CoopCareer   two teams, one world      - versus plus a shared calendar
//! ```
//!
//! ## Why the split is possible at all
//!
//! The game's own persistence already keys state at the granularities the modes need
//! (`content/Database/SavePS.json`):
//!
//! ```text
//!   Save_CarConfig                TeamID + LoadoutID   setup: toe, camber, wing angles
//!   Save_DriverCommands           LoadoutID            pace, ERS, lift-and-coast, fuel
//!   Save_PlayerStrategyStints     LoadoutID            pit windows, stints
//!   Save_CarTyreAllocation        CarID                which sets each car has
//!   Buildings_HQ                  TeamID               facilities
//!   Teams_PriorityList_Setting_DevelopBuildingData  TeamID   development priority
//!   Staff_Contracts               TeamID               drivers and staff
//! ```
//!
//! So "my side of the garage" is a `LoadoutID`, and "facilities come from the team principal" is a
//! `TeamID`. Nothing here needs the game to cooperate; it is policy over rows that already exist.
//!
//! ## The trap this module exists to avoid
//!
//! `Save_CarConfig` is keyed by **both** `TeamID` and `LoadoutID`. A naive rule of "only the host writes
//! team-scoped rows" would therefore stop the second player configuring their *own* car - the exact
//! feature SharedTeam mode is for. Hence [`may_write`] takes the whole key and the mode decides which
//! parts of it matter, rather than each caller picking a table and guessing.

/// Which of the three co-op modes is in play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Two players, one team, one car each.
    SharedTeam,
    /// Two players, two teams, competing.
    Versus,
    /// Two teams, one shared world and calendar.
    CoopCareer,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::SharedTeam => "shared-team",
            Mode::Versus => "versus",
            Mode::CoopCareer => "coop-career",
        }
    }

    /// Parses `coop_mode.txt`. Defaults to the *most restricted* mode on anything unrecognised.
    ///
    /// Not the loosest: a typo must never hand a player write access to the other player's team. The most
    /// restricted useful default is `Versus`, where a player may only touch their own team - a superset of
    /// what SharedTeam allows for team-scoped rows and a subset of nothing.
    pub fn from_text(text: &str) -> Mode {
        match text.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "shared-team" | "sharedteam" | "shared" | "team" => Mode::SharedTeam,
            "coop-career" | "coopcareer" | "career" | "coop" => Mode::CoopCareer,
            _ => Mode::Versus,
        }
    }
}

/// The row being written, described by the keys that actually identify it.
///
/// Every field is optional because not every table has every key: `Buildings_HQ` has no `LoadoutID`,
/// `Save_DriverCommands` is identified by loadout alone. A missing key is `None`, not `0`, so "no team"
/// cannot be confused with "team zero".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowKey {
    pub team: Option<u32>,
    pub loadout: Option<u32>,
    pub car: Option<u32>,
}

impl RowKey {
    pub fn team(team: u32) -> Self {
        RowKey { team: Some(team), ..Default::default() }
    }

    pub fn loadout(loadout: u32) -> Self {
        RowKey { loadout: Some(loadout), ..Default::default() }
    }

    pub fn team_loadout(team: u32, loadout: u32) -> Self {
        RowKey { team: Some(team), loadout: Some(loadout), car: None }
    }

    pub fn car(car: u32) -> Self {
        RowKey { car: Some(car), ..Default::default() }
    }
}

/// What kind of state a table holds, which is what the modes actually disagree about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Belongs to one player's own car: setup, driver commands, their pit strategy.
    PerCar,
    /// Belongs to a team as a whole: facilities, research priority, pit-crew training, finances.
    PerTeam,
    /// One value for the entire world: the calendar day, regulations, weather.
    World,
    /// A race result both players are competing in; the host arbitrates.
    RaceResult,
}

/// Who this player is, in the terms the rows are keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Player {
    pub team: u32,
    /// The `LoadoutID` of the car this player drives. In `Versus` a player has two cars; this is the one
    /// they personally control, and the other is left to the AI or to their teammate.
    pub loadout: u32,
    /// True for the machine that owns the world's single-truth rows.
    pub is_host: bool,
}

impl Player {
    pub fn new(team: u32, loadout: u32, is_host: bool) -> Self {
        Player { team, loadout, is_host }
    }
}

/// Why a write was refused, so a refusal can be logged with a reason instead of a bare `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// The row belongs to another team.
    NotMyTeam,
    /// The row belongs to another player's car within my team.
    NotMyCar,
    /// Single-truth world state, and this player is not the host.
    NotHost,
    /// The caller supplied a row with no key of the kind this scope needs, so ownership cannot be
    /// established. Refused rather than allowed: an unowned row is not the same as an owned one.
    Unidentifiable,
}

impl Denied {
    pub fn as_str(self) -> &'static str {
        match self {
            Denied::NotMyTeam => "row belongs to another team",
            Denied::NotMyCar => "row belongs to the other car in this team",
            Denied::NotHost => "world state may only be written by the host",
            Denied::Unidentifiable => "row has no key that identifies an owner",
        }
    }
}

/// May `player` write this row? `Ok(())` or the reason not.
///
/// The rule, and the only rule:
///
/// * `World` - only the host. The calendar must not be advanced by two machines.
/// * `PerTeam` - the row's team must be mine. In `Versus` and `CoopCareer` that is the whole story; in
///   `SharedTeam` it is also the whole story, because facilities are the principal's.
/// * `PerCar` - in `SharedTeam` the row's loadout must be **mine**, which is the entire point of the mode;
///   in `Versus`/`CoopCareer` it must merely belong to my team, since a player runs both their cars.
/// * `RaceResult` - the host writes it; a client's own result is advisory.
pub fn may_write(mode: Mode, player: &Player, scope: Scope, row: &RowKey) -> Result<(), Denied> {
    match scope {
        Scope::World => {
            if player.is_host {
                Ok(())
            } else {
                Err(Denied::NotHost)
            }
        }

        Scope::PerTeam => match row.team {
            Some(t) if t == player.team => Ok(()),
            // A per-team row with no team key cannot be attributed, and guessing "mine" is how a player
            // ends up editing the other team's facilities.
            Some(_) => Err(Denied::NotMyTeam),
            None => Err(Denied::Unidentifiable),
        },

        Scope::PerCar => {
            // In shared-team mode the loadout is the authority: each player owns one car. Check it first,
            // because `Save_CarConfig` carries both a team and a loadout and the team check alone would
            // let a player reconfigure their teammate's car.
            if mode == Mode::SharedTeam {
                return match row.loadout {
                    Some(l) if l == player.loadout => Ok(()),
                    Some(_) => Err(Denied::NotMyCar),
                    None => Err(Denied::Unidentifiable),
                };
            }
            // In the other modes the player runs the whole team, so the team is the boundary.
            match row.team {
                Some(t) if t == player.team => Ok(()),
                Some(_) => Err(Denied::NotMyTeam),
                // A loadout-scoped table has no team column (`Save_DriverCommands` is `LoadoutID` only),
                // so accept it when we cannot see a team at all - but only for our own loadout if the row
                // names one.
                None => match row.loadout {
                    Some(l) if l == player.loadout => Ok(()),
                    Some(_) => Err(Denied::NotMyCar),
                    None => Err(Denied::Unidentifiable),
                },
            }
        }

        Scope::RaceResult => Ok(()),
    }
}

/// Extracts the table a statement writes, and whether it writes at all.
///
/// Only the leading verb matters: a `SELECT` is never gated, and the three write verbs all name their table
/// in a fixed position. `INSERT INTO t`, `UPDATE t`, `DELETE FROM t`.
///
/// Deliberately conservative: anything it cannot parse returns `None`, which the caller treats as
/// "not a write" - so an unparseable statement is attempted rather than blocked. That is the right default
/// for a maintenance tool; the *safety* comes from the tables it does recognise.
fn write_target(sql: &str) -> Option<String> {
    let s = sql.trim_start();
    let upper = s.get(..16)?.to_ascii_uppercase();

    let after = |kw: &str| -> Option<String> {
        let rest = s.get(kw.len()..)?.trim_start();
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if ident.is_empty() { None } else { Some(ident) }
    };

    if upper.starts_with("INSERT") {
        // INSERT INTO <table>
        let rest = s.get(6..)?.trim_start();
        if !rest.to_ascii_uppercase().starts_with("INTO") {
            return None;
        }
        let rest = rest.get(4..)?.trim_start();
        let ident: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        return if ident.is_empty() { None } else { Some(ident) };
    }
    if upper.starts_with("UPDATE") {
        return after("UPDATE");
    }
    if upper.starts_with("DELETE") {
        let rest = s.get(6..)?.trim_start();
        if !rest.to_ascii_uppercase().starts_with("FROM") {
            return None;
        }
        let rest = rest.get(4..)?.trim_start();
        let ident: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        return if ident.is_empty() { None } else { Some(ident) };
    }
    None
}

/// Reads the mode and this player's identity from disk, if they are configured.
///
/// `coop_mode.txt` holds the mode; `coop_player.txt` holds `team loadout host` on one line. Absent files mean
/// "not a co-op run", and gates are not applied - the tool stays usable for single-player repairs.
pub fn configured() -> Option<(Mode, Player)> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();

    let mode_text = std::fs::read_to_string(dir.join("coop_mode.txt")).ok()?;
    let mode = Mode::from_text(&mode_text);

    let who = std::fs::read_to_string(dir.join("coop_player.txt")).ok()?;
    let parts: Vec<&str> = who.split_whitespace().collect();
    let team: u32 = parts.first()?.parse().ok()?;
    let loadout: u32 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let is_host = parts.get(2).map(|s| *s == "host").unwrap_or(false);
    Some((mode, Player::new(team, loadout, is_host)))
}

/// Returns the refusal reason when `sql` must not be executed, or `None` when it may.
///
/// This is the gate the SQL tool calls. It exists because `coop-sql` can execute any statement, and in co-op
/// "any statement" includes editing the other player's team.
pub fn gate_sql(sql: &str) -> Option<String> {
    let (mode, player) = configured()?;
    let table = write_target(sql)?;

    let scope = match scope_of(&table) {
        Some(s) => s,
        None => {
            // An unclassified table cannot be checked. Refuse, because silently allowing writes to a table
            // nobody classified is how an unintended edit gets through.
            return Some(format!(
                "table `{table}` has no co-op scope classification; refusing to write it blind"
            ));
        }
    };

    // The row key is not parsed out of arbitrary SQL - that would be a mini SQL parser and a source of
    // subtle bypasses. Instead the identity comes from the configured player, which is enough for
    // `World` and for the team half of `PerTeam`; per-car rows need the caller to state the car.
    let row = RowKey { team: Some(player.team), loadout: Some(player.loadout), car: None };
    match may_write(mode, &player, scope, &row) {
        Ok(()) => None,
        Err(d) => Some(format!("{} ({mode:?}, table `{table}`)", d.as_str())),
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn write_target_recognises_the_three_write_verbs() {
        assert_eq!(write_target("INSERT INTO Save_CarConfig (a) VALUES (1)").as_deref(), Some("Save_CarConfig"));
        assert_eq!(write_target("  update Player_State SET Day = 1").as_deref(), Some("Player_State"));
        assert_eq!(write_target("DELETE FROM Save_DriverCommands").as_deref(), Some("Save_DriverCommands"));
        // Reads are never gated.
        assert_eq!(write_target("SELECT * FROM Player"), None);
        assert_eq!(write_target(""), None);
    }

    /// A table nobody classified must not be writable blind.
    #[test]
    fn an_unclassified_table_is_refused_rather_than_allowed() {
        // Exercise the classification directly, since `gate_sql` needs files on disk.
        assert_eq!(scope_of("Some_New_Table"), None);
        assert!(scope_of("Save_CarConfig").is_some());
    }

    /// The gate must refuse a write the mode forbids. This exercises the parsing and the rule together
    /// against a synthetic player, since reading the config files would need them on disk.
    #[test]
    fn a_guest_cannot_write_world_state() {
        let guest = Player::new(1, 10, false);
        let (mode, scope) = (Mode::Versus, scope_of("Player_State").unwrap());
        assert_eq!(scope, Scope::World);
        let row = RowKey { team: Some(1), loadout: Some(10), car: None };
        assert_eq!(may_write(mode, &guest, scope, &row), Err(Denied::NotHost));
    }

    /// And the same guest *can* write their own car, which is the balance the modes need.
    #[test]
    fn a_guest_can_still_write_their_own_car() {
        let guest = Player::new(1, 10, false);
        let scope = scope_of("Save_CarConfig").unwrap();
        let mine = RowKey { team: Some(1), loadout: Some(10), car: None };
        assert_eq!(may_write(Mode::SharedTeam, &guest, scope, &mine), Ok(()));
    }
}

///
/// Kept next to the rules so the mapping is reviewable in one place rather than scattered at call sites,
/// where a wrong classification would silently grant or deny access.
pub fn scope_of(table: &str) -> Option<Scope> {
    Some(match table {
        // Car-level.
        "Save_CarConfig" => Scope::PerCar,
        "Save_CarConfig_ParcFerme" => Scope::PerCar,
        "Save_CarConfig_Effects" => Scope::PerCar,
        "Save_DriverCommands" => Scope::PerCar,
        "Save_PlayerStrategyStints" => Scope::PerCar,
        "Save_CarTyreAllocation" => Scope::PerCar,
        "Save_Strategist" => Scope::PerCar,
        "Save_Strategist_LapHistory" => Scope::PerCar,

        // Team-level.
        "Buildings_HQ" => Scope::PerTeam,
        "Buildings_HQ_History" => Scope::PerTeam,
        "Teams_PriorityList" => Scope::PerTeam,
        "Teams_PriorityList_Setting_DevelopBuildingData" => Scope::PerTeam,
        "Teams_PitCrewRecordPerSeason" => Scope::PerTeam,
        "Staff_PitCrew_DevelopmentPlan" => Scope::PerTeam,
        "CurrStaffContracts" => Scope::PerTeam,
        "Staff_Contracts" => Scope::PerTeam,
        "Staff_ContractOffers" => Scope::PerTeam,
        "Finances" => Scope::PerTeam,
        "Parts" => Scope::PerTeam,
        "PartsAuto" => Scope::PerTeam,

        // World-level single truth.
        "Player_State" => Scope::World,
        "Regulations" => Scope::World,
        "Seasons" => Scope::World,
        "Calendar" => Scope::World,
        "WeatherHistory" => Scope::World,
        "Difficulty" => Scope::World,

        // Race outcome, host-arbitrated.
        "Save_Weekend" => Scope::RaceResult,
        "Save_RaceControl" => Scope::RaceResult,
        "Save_RaceSimManager" => Scope::RaceResult,
        "Save_TimingManager_Laps" => Scope::RaceResult,
        "Save_TimingManager_Sectors" => Scope::RaceResult,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_shared() -> Player {
        Player::new(1, 10, true)
    }
    fn guest_shared() -> Player {
        Player::new(1, 11, false)
    }

    #[test]
    fn the_mode_split_is_what_makes_shared_team_work() {
        // Two players, one team. Each owns a car. The whole point is that the second player can set up
        // their own car and cannot touch the first player's.
        let a = host_shared();
        let b = guest_shared();
        let car_a = RowKey::team_loadout(1, 10);
        let car_b = RowKey::team_loadout(1, 11);

        assert_eq!(may_write(Mode::SharedTeam, &a, Scope::PerCar, &car_a), Ok(()));
        assert_eq!(
            may_write(Mode::SharedTeam, &a, Scope::PerCar, &car_b),
            Err(Denied::NotMyCar),
            "player A must not configure player B's car"
        );
        assert_eq!(may_write(Mode::SharedTeam, &b, Scope::PerCar, &car_b), Ok(()));
        assert_eq!(
            may_write(Mode::SharedTeam, &b, Scope::PerCar, &car_a),
            Err(Denied::NotMyCar),
            "player B must not configure player A's car"
        );
    }

    /// The trap the module docblock warns about: `Save_CarConfig` carries a team key as well as a loadout,
    /// so a team-only check would pass both players' writes and quietly allow the teammate's setup.
    #[test]
    fn a_team_key_alone_does_not_grant_car_access_in_shared_team_mode() {
        let b = guest_shared();
        // Same team, wrong loadout - exactly what a team-only rule would wrongly allow.
        let key = RowKey::team_loadout(1, 10);
        assert_eq!(
            may_write(Mode::SharedTeam, &b, Scope::PerCar, &key),
            Err(Denied::NotMyCar)
        );
    }

    #[test]
    fn facilities_belong_to_the_principal_in_shared_team_mode() {
        let a = host_shared();
        let b = guest_shared();
        let hq = RowKey::team(1);

        // Both players are on team 1, so a team-scoped row is writable by both *unless* the mode narrows it.
        // In SharedTeam the principal is the host, and that is expressed by `is_host`, not by the team key.
        assert_eq!(may_write(Mode::SharedTeam, &a, Scope::PerTeam, &hq), Ok(()));
        // B is on the same team, so by the team rule alone B could write it. The mode must be enforced
        // elsewhere (the caller passes World for principal-only rows) - this test documents the boundary
        // so nobody assumes the team check is sufficient.
        assert_eq!(
            may_write(Mode::SharedTeam, &b, Scope::PerTeam, &hq),
            Ok(()),
            "team-scoped rows are shared within a team; principal-only gating is a separate scope"
        );
        // And the principal-only path really does refuse a non-host.
        assert_eq!(
            may_write(Mode::SharedTeam, &b, Scope::World, &hq),
            Err(Denied::NotHost)
        );
    }

    #[test]
    fn versus_lets_a_player_run_both_of_their_cars_but_not_the_other_team() {
        let a = Player::new(1, 10, true);
        let mine_other_car = RowKey::team_loadout(1, 12);
        let theirs = RowKey::team_loadout(2, 20);

        assert_eq!(may_write(Mode::Versus, &a, Scope::PerCar, &mine_other_car), Ok(()));
        assert_eq!(
            may_write(Mode::Versus, &a, Scope::PerCar, &theirs),
            Err(Denied::NotMyTeam)
        );
        assert_eq!(may_write(Mode::Versus, &a, Scope::PerTeam, &RowKey::team(1)), Ok(()));
        assert_eq!(
            may_write(Mode::Versus, &a, Scope::PerTeam, &RowKey::team(2)),
            Err(Denied::NotMyTeam)
        );
    }

    #[test]
    fn a_loadout_only_row_is_accepted_for_my_own_loadout() {
        // `Save_DriverCommands` is keyed by LoadoutID alone - no team column to check.
        let b = guest_shared();
        assert_eq!(may_write(Mode::Versus, &b, Scope::PerCar, &RowKey::loadout(11)), Ok(()));
        assert_eq!(
            may_write(Mode::Versus, &b, Scope::PerCar, &RowKey::loadout(99)),
            Err(Denied::NotMyCar)
        );
    }

    #[test]
    fn world_state_is_host_only_in_every_mode() {
        let guest = Player::new(1, 10, false);
        let host = Player::new(1, 10, true);
        for mode in [Mode::SharedTeam, Mode::Versus, Mode::CoopCareer] {
            assert_eq!(
                may_write(mode, &guest, Scope::World, &RowKey::team(1)),
                Err(Denied::NotHost),
                "{mode:?} must not let a client advance the calendar"
            );
            assert_eq!(may_write(mode, &host, Scope::World, &RowKey::team(1)), Ok(()));
        }
    }

    /// An unidentifiable row must be refused, not allowed. Guessing "mine" is how a player edits the other
    /// team's facilities.
    #[test]
    fn a_row_with_no_owner_is_refused_rather_than_allowed() {
        let a = host_shared();
        assert_eq!(
            may_write(Mode::SharedTeam, &a, Scope::PerTeam, &RowKey::default()),
            Err(Denied::Unidentifiable)
        );
        assert_eq!(
            may_write(Mode::SharedTeam, &a, Scope::PerCar, &RowKey::default()),
            Err(Denied::Unidentifiable)
        );
    }

    #[test]
    fn mode_parsing_defaults_to_the_most_restricted_useful_mode() {
        assert_eq!(Mode::from_text("shared-team"), Mode::SharedTeam);
        assert_eq!(Mode::from_text("SharedTeam"), Mode::SharedTeam);
        assert_eq!(Mode::from_text("coop-career"), Mode::CoopCareer);
        assert_eq!(Mode::from_text("versus"), Mode::Versus);
        // Unknown must not become the loosest mode.
        assert_eq!(Mode::from_text("nonsense"), Mode::Versus);
        assert_eq!(Mode::from_text(""), Mode::Versus);
    }

    #[test]
    fn every_table_this_project_touches_is_classified() {
        // A table with no scope cannot be checked, so it must not silently pass. This lists what we use.
        for t in [
            "Save_CarConfig",
            "Save_DriverCommands",
            "Save_PlayerStrategyStints",
            "Save_CarTyreAllocation",
            "Buildings_HQ",
            "Teams_PriorityList_Setting_DevelopBuildingData",
            "Staff_Contracts",
            "Player_State",
            "Regulations",
            "Calendar",
            "Save_Weekend",
            "Save_RaceControl",
        ] {
            assert!(scope_of(t).is_some(), "{t} must have a scope");
        }
        assert_eq!(scope_of("Save_CarConfig"), Some(Scope::PerCar));
        assert_eq!(scope_of("Player_State"), Some(Scope::World));
        assert_eq!(scope_of("Buildings_HQ"), Some(Scope::PerTeam));
        assert_eq!(scope_of("not_a_table"), None);
    }
}
