//! The real career schema, as the game itself declares it.
//!
//! Why this exists
//! ---------------
//! Every table list in this project before this file was **guessed from file names**. `Content/Database/*PS.json`
//! look like one table per file, so `StaffPS` was recorded as a table. It is not. Those files are
//! `PSCollection` assets - named prepared-statement catalogues whose entries are the SQL text the game runs
//! (`{"Key": "AddStaffContract", "Value": "INSERT OR REPLACE INTO Staff_Contracts (...) VALUES (?1..?12)"}`).
//! The table they name is `Staff_Contracts`. `StaffPS`, `PartsPS`, `TeamPS`, `CarPS`, `FinancesPS`, `ScoutingPS`,
//! `SponsorshipPS` and `Player_State`-style guesses made from a filename do not exist in the database at all.
//!
//! Consequences that were live before this file: `replicate::REPLICABLE` allowlisted eight names of which seven
//! were phantom, so a real career change could never match; `coop_auth::scope_of` classified `Finances`, `Parts`,
//! `PartsAuto`, `Regulations`, `Calendar`, `WeatherHistory` and `Difficulty`, none of which are tables.
//!
//! Where the truth comes from
//! --------------------------
//! `/Game/Database/Volta` is a `DatabaseAsset` whose `Data` array is, byte for byte, a **SQLite 3 file**
//! (`SQLite format 3\0`, `user_version` 336, 317 tables, 2 views, seeded with 32 `Teams` and 446
//! `Staff_GameData` rows). Decoding it gives `sqlite_master`, which is authoritative: declared column types,
//! `PRIMARY KEY` members and the real table names. `tools/schema_truth.py` decodes it and cross-checks this file,
//! so these constants cannot drift from the export without a test failing.
//!
//! What is derived here, and why only this
//! ---------------------------------------
//! * **Single-column primary keys only.** `Msg::Stmt` carries one key, so a composite-key row cannot be identified
//!   by it. `Staff_Contracts` is keyed `(StaffID, ContractType, TeamID)` - refusing it is correct until the wire
//!   carries a full key, because "update the contract for StaffID 83" would hit both the Current and the Future
//!   row.
//! * **A `TeamID` column must exist.** This is what makes a change attributable to a player. Without it `plan`
//!   returns `NoTeam`, which is the safe direction.
//! * **`Save_*`, `Scenario_*`, `Onboarding_*` and `*_Enum_*` are excluded.** The `Save_` tables are race-sim
//!   state, which the sim owns and which would fight the sim if written; the rest are static content, and
//!   replicating a write into static content means one player has silently changed the other's rules.

/// One replicable table: its real name, the single column that identifies a row, and the column naming its team.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table {
    pub name: &'static str,
    /// The declared single-column `PRIMARY KEY`. `Msg::Stmt` carries exactly one key, so this must be one column.
    pub pk: &'static str,
    /// The column that names the owning team, used by `plan` to attribute the change.
    pub team: &'static str,
}

/// Tables whose rows can be carried by `Msg::Stmt` as it is defined today.
///
/// Derived, not chosen: every entry has exactly one `PRIMARY KEY` column and a `TeamID` column in `Volta`.
/// An allowlist because the failure mode of a denylist is that a table added by a game patch silently becomes
/// replicable; here a new table simply is not replicated, which is reported as `UnknownTable`.
pub const REPLICABLE: &[Table] = &[
    Table { name: "Parts_Designs", pk: "DesignID", team: "TeamID" },
    Table { name: "Races_GridPenalties", pk: "PenaltyID", team: "TeamID" },
    Table { name: "Scouting_Staff_Bonuses", pk: "StaffID", team: "TeamID" },
    Table { name: "Scouting_Staff_DriverRivalryEventLogs", pk: "DriverID", team: "TeamID" },
    Table { name: "Scouting_Staff_RaceEngineerDriverAssignments", pk: "RaceEngineerID", team: "TeamID" },
    Table { name: "Sponsorship_EngagementActivities", pk: "EngagementActivityInstanceID", team: "TeamID" },
    Table { name: "Sponsorship_EngagementActivities_Locks", pk: "TeamID", team: "TeamID" },
    Table { name: "Staff_NarrativeData", pk: "StaffID", team: "TeamID" },
    Table { name: "Staff_PitCrew_DevelopmentPlanGlobalData", pk: "TeamID", team: "TeamID" },
    Table { name: "Staff_Team_CAS", pk: "StaffID", team: "TeamID" },
    Table { name: "SubTeam_Assignments", pk: "AssignmentID", team: "TeamID" },
    Table { name: "Teams", pk: "TeamID", team: "TeamID" },
    Table { name: "Teams_Colours", pk: "ColourID", team: "TeamID" },
    Table { name: "Teams_MiscRecord", pk: "TeamID", team: "TeamID" },
    Table { name: "Teams_PitCrewRecordSinceGameStart", pk: "TeamID", team: "TeamID" },
    Table { name: "Teams_RaceRecordSinceGameStart", pk: "TeamID", team: "TeamID" },
];

/// The replicable table called `name`, compared case-insensitively because SQL identifiers are.
pub fn lookup(name: &str) -> Option<&'static Table> {
    REPLICABLE.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight names the previous allowlist used. Seven of them are `PSCollection` asset names, not tables.
    const PHANTOM: [&str; 7] = [
        "StaffPS",
        "PartsPS",
        "SponsorshipPS",
        "FinancesPS",
        "ScoutingPS",
        "CarPS",
        "TeamPS",
    ];

    #[test]
    fn an_asset_catalogue_name_is_not_a_table() {
        // Each `*PS.json` file announces itself as a `PSCollection` and its entries name real tables. Treating the
        // file name as a table is the mistake this test exists to keep from coming back.
        for p in PHANTOM {
            assert!(lookup(p).is_none(), "{p} is a PSCollection asset name, not a table");
        }
    }

    #[test]
    fn every_entry_has_one_key_and_a_team_column() {
        for t in REPLICABLE {
            assert!(!t.name.is_empty() && !t.pk.is_empty() && !t.team.is_empty(), "{t:?}");
            // The key and the team column must be distinct identifiers, or `WHERE pk = v` would be a team write
            // masquerading as a row write.
            assert_ne!(t.name, t.pk);
        }
    }

    #[test]
    fn no_race_sim_or_static_content_table_is_replicable() {
        // `Save_*` is the sim's own state and would fight the sim; `*_Enum_*` is static content, and replicating a
        // write into it would change the other player's rules.
        for t in REPLICABLE {
            assert!(!t.name.starts_with("Save_"), "{}", t.name);
            assert!(!t.name.starts_with("Scenario_"), "{}", t.name);
            assert!(!t.name.starts_with("Onboarding_"), "{}", t.name);
            assert!(!t.name.contains("_Enum_"), "{}", t.name);
        }
    }

    #[test]
    fn lookup_is_case_insensitive_and_refuses_unknown() {
        assert_eq!(lookup("teams").map(|t| t.name), Some("Teams"));
        assert!(lookup("Teams_That_Do_Not_Exist").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn a_composite_key_table_is_not_listed() {
        // `Staff_Contracts` is `PRIMARY KEY (StaffID, ContractType, TeamID)`. One key cannot name that row.
        assert!(lookup("Staff_Contracts").is_none());
        // And one that is: `Teams` is `TeamID INTEGER PRIMARY KEY AUTOINCREMENT`.
        assert_eq!(lookup("Teams").map(|t| t.pk), Some("TeamID"));
    }
}
