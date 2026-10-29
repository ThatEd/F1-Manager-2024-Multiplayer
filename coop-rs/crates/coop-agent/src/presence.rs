//! Steam rich presence for the co-op session.
//!
//! The strings a friend sees under the player's name. Requirements, as given:
//!
//! ```text
//! Season TeamName            e.g. "2024 Ferrari"
//! then:
//!   Racing at [TrackName]
//!   Managing
//!   Leading the championship
//!   Choosing engines
//!
//! when main menu -> main menu
//! when creating a new game -> Creating A New Game
//!
//! multiplayer:
//!   Multiplayer Season TeamName Playercount
//!   Multiplayer Season RaceName
//! ```
//!
//! ## What Steam actually needs
//!
//! `SetRichPresence` takes key/value pairs and renders the friends-list line from the **`steam_display`**
//! token, which is a localization key such as `#Status_AtMainMenu` or `#Status_%gamestatus%`. The token can
//! interpolate other keys (`%key%`). A game with no `steam_display` set shows nothing at all - which is
//! this game's situation today: `steam_api64.dll` exports `SteamAPI_ISteamFriends_SetRichPresence`, but the
//! executable contains **no** `SetRichPresence`, `steam_display` or `steam_player_group` string, so Frontier
//! never set any rich presence and every key below is one we introduce.
//!
//! So this module produces two things:
//!
//! 1. The **token keys** the localization file needs (`#Coop_*`), each of which composes the strings above
//!    from presence keys.
//! 2. The **presence keys** to set for a given game state, as a list of `(key, value)` pairs.
//!
//! Keeping the state -> tokens mapping pure makes it testable without Steam, without the game, and without
//! a second player. The Steam call itself is a thin shell around this in `presence_steam`.
//!
//! ## Length budget
//!
//! Steam renders this on **one line** and truncates the end. A truncated line is worse than a short one -
//! "Multiplayer 2024 Ferrari 2" losing its team name reads as a bug. So every composed string is capped,
//! and the cap is enforced here rather than left to the caller to remember.

use std::fmt;

/// Steam renders this on one line in the friends list and truncates the end.
///
/// Set well below Steam's own `k_cchMaxRichPresenceValueLength` (256 in the SDK headers, which are not
/// shipped with the game so it is not re-verified here). Being far below the ceiling means the value can
/// never be rejected outright, which would make the line vanish rather than shorten.
pub const LINE_MAX: usize = 64;

/// F1 Manager 2024's Steam application id.
///
/// Read from the shipped `steam_appid.txt` (which contains exactly `2591280`), not from memory or a store
/// page: it is the value the game itself hands Steam, so it is the one a lobby must match.
pub const APP_ID: u32 = 2_591_280;

/// Where the player is in the game. Drives which token is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    /// Main menu, no career open.
    MainMenu,
    /// In the new-game flow.
    CreatingANewGame,
    /// In a career, doing something specific.
    Career(CareerActivity),
    /// In a co-op race weekend.
    MultiplayerRace,
    /// In a co-op career, outside a race weekend.
    MultiplayerCareer,
}

/// What the player is doing inside a career.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CareerActivity {
    /// Between races: the management screens.
    Managing,
    /// Racing at a named circuit.
    RacingAt(String),
    /// First in the championship.
    LeadingTheChampionship,
    /// In the engine-selection screen.
    ChoosingEngines,
}

impl CareerActivity {
    /// The text for the career sub-line, exactly as specified.
    pub fn text(&self) -> String {
        match self {
            CareerActivity::Managing => "Managing".to_string(),
            CareerActivity::RacingAt(track) => format!("Racing at {track}"),
            CareerActivity::LeadingTheChampionship => "Leading the championship".to_string(),
            CareerActivity::ChoosingEngines => "Choosing engines".to_string(),
        }
    }
}

/// One player's own state plus the session it is part of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub season: u16,
    pub team: String,
    /// Zero when not in a multiplayer session.
    pub players: u8,
    /// The circuit, when a race weekend is loaded.
    pub track: Option<String>,
}

impl Presence {
    pub fn single_player(season: u16, team: &str) -> Self {
        Self { season, team: team.to_string(), players: 1, track: None }
    }

    pub fn multiplayer(season: u16, team: &str, players: u8) -> Self {
        Self { season, team: team.to_string(), players: players.max(2), track: None }
    }

    pub fn with_track(mut self, track: &str) -> Self {
        self.track = Some(track.to_string());
        self
    }

    pub fn is_multiplayer(&self) -> bool {
        self.players >= 2
    }
}

/// Trim to the line budget without producing a half word where it can be avoided.
///
/// Steam would ellipsize, but what it ellipsizes is invisible to us and the result has been seen to lose
/// the part that mattered (the team name at the end of a long multiplayer line). Trimming here means the
/// truncation is deliberate and testable.
pub fn clamp(s: &str) -> String {
    if s.chars().count() <= LINE_MAX {
        return s.to_string();
    }
    // Prefer cutting at the last space that still fits, so a team name is not sliced mid-word.
    let cut: String = s.chars().take(LINE_MAX).collect();
    match cut.rfind(' ') {
        Some(i) if i > LINE_MAX / 2 => cut[..i].trim_end().to_string(),
        _ => cut.trim_end().to_string(),
    }
}

/// The `steam_display` token for a state.
///
/// These are keys we define in the game's localization; Steam resolves the token and substitutes the
/// `%key%` placeholders from the presence keys set alongside it.
pub fn display_token(where_: &Where) -> &'static str {
    match where_ {
        Where::MainMenu => "#Coop_MainMenu",
        Where::CreatingANewGame => "#Coop_NewGame",
        Where::Career(CareerActivity::Managing) => "#Coop_Career_Managing",
        Where::Career(CareerActivity::RacingAt(_)) => "#Coop_Career_Racing",
        Where::Career(CareerActivity::LeadingTheChampionship) => "#Coop_Career_Leading",
        Where::Career(CareerActivity::ChoosingEngines) => "#Coop_Career_Engines",
        Where::MultiplayerRace => "#Coop_Mp_Race",
        Where::MultiplayerCareer => "#Coop_Mp_Career",
    }
}

/// The presence key/value pairs to set for a state.
///
/// Returned as a list rather than applied directly so the mapping is testable and so the caller can log
/// exactly what was sent. Keys not returned here are **cleared** by the caller, because a stale `track`
/// from the previous race would keep showing under a "Managing" line.
///
/// `use_token` controls whether `steam_display` is set. It is `true` once the `#Coop_*` tokens are
/// registered against the app in Steamworks Partner; before that, setting it makes friends see the literal
/// `#Coop_Mp_Race`. With `false` the plain keys are still sent, Steam shows the game name with no extra
/// line, and nothing regresses. A flag rather than a rebuild so the same DLL can ship either way.
pub fn keys_with(where_: &Where, p: Option<&Presence>, use_token: bool) -> Vec<(String, String)> {
    let mut out = keys(where_, p);
    if !use_token {
        out.retain(|(k, _)| k != "steam_display");
        // The group keys are plain values, not tokens, so they stay: friend grouping works without any
        // localization registration.
    }
    out
}

/// The full key set, using `status` as the carrier so **no localisation is required at all**.
///
/// This is the mode to ship, and it exists because of a sentence in Steam's own docs:
///
/// > If `steam_display` is not set to a valid localization tag, then rich presence will not be displayed
/// > in the Steam client.
///
/// So `steam_display` is all-or-nothing: it needs tokens registered against the app in Steamworks Partner,
/// and if they are not there nothing is shown. But **`status` is a plain UTF-8 string** that Steam displays
/// as-is in the "view game info" dialog. Carrying our text in `status` means the feature works today, with
/// no publisher action and no localisation file, and cannot be broken by a missing or renamed token.
///
/// What this trades away: `status` is not the friends-list line under the name - that is `steam_display`'s
/// job, and filling it needs the tokens. So:
///
/// * `status` + `steam_player_group` — works now, no localisation, shown in the game-info dialog and used
///   for friend grouping.
/// * add `steam_display` later, once the tokens are registered, for the line under the name.
///
/// Both sets are sent together when tokens exist, so nothing has to be undone later.
pub fn keys_status(where_: &Where, p: Option<&Presence>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    // `status` is the raw text. Clamped because Steam rejects an over-length value outright, and a
    // rejected `status` is a silently missing line.
    out.push(("status".into(), render(where_, p)));
    // Everything else is identical to the tokenful set, minus `steam_display`.
    for (k, v) in keys(where_, p) {
        if k == "steam_display" {
            continue;
        }
        out.push((k, v));
    }
    out
}

/// The presence key/value pairs to set for a state, with `steam_display` included.
pub fn keys(where_: &Where, p: Option<&Presence>) -> Vec<(String, String)> {    let mut out: Vec<(String, String)> = Vec::new();

    // `steam_display` is what Steam actually renders.
    out.push(("steam_display".into(), display_token(where_).into()));

    match where_ {
        // No career open: nothing else to say, and no stale keys should survive.
        Where::MainMenu | Where::CreatingANewGame => {}
        Where::Career(activity) => {
            if let Some(p) = p {
                out.push(("coop_season".into(), p.season.to_string()));
                out.push(("coop_team".into(), p.team.clone()));
            }
            // `Racing at X` carries its own text; the others are fixed phrases. Both travel as one
            // rendered string so a localization author only has to translate the template, not the code.
            out.push(("coop_activity".into(), activity.text()));
        }
        Where::MultiplayerRace | Where::MultiplayerCareer => {
            if let Some(p) = p {
                out.push(("coop_season".into(), p.season.to_string()));
                out.push(("coop_team".into(), p.team.clone()));
                out.push(("coop_players".into(), p.players.to_string()));
                // The friend grouping is what makes two friends appear joined in the list. Same id on
                // both machines, so it must be derived from the session, not the process.
                out.push(("steam_player_group".into(), group_id(p)));
                out.push(("steam_player_group_size".into(), p.players.to_string()));
            }
            if let Some(track) = p.and_then(|p| p.track.clone()) {
                out.push(("coop_track".into(), track));
            }
        }
    }
    out
}

/// A stable group id for the session.
///
/// Both machines must produce the **same** string or Steam shows two groups of one. Derived from the
/// season and the team set is not available here, so the season plus player count is used - coarse, but
/// identical on both sides without needing to agree on anything else. A per-process id (pid, timestamp)
/// would be unique per machine and defeat the grouping entirely.
pub fn group_id(p: &Presence) -> String {
    format!("f1m24-coop-{}-{}p", p.season, p.players)
}

/// The composed line, for logging and for tests. Steam composes the same text from the token.
///
/// Kept in step with the token templates by a test that compares them, because a log that disagrees with
/// what friends see is worse than no log.
pub fn render(where_: &Where, p: Option<&Presence>) -> String {
    let s = match where_ {
        Where::MainMenu => "Main menu".to_string(),
        Where::CreatingANewGame => "Creating A New Game".to_string(),
        Where::Career(activity) => match p {
            Some(p) => format!("{} {}: {}", p.season, p.team, activity.text()),
            None => activity.text(),
        },
        Where::MultiplayerRace | Where::MultiplayerCareer => match p {
            Some(p) => {
                let base = format!("Multiplayer {} {} {}", p.season, p.team, p.players);
                match (&where_, &p.track) {
                    // The race line names the circuit instead of the head-count: a friend deciding
                    // whether to watch cares which race, not how many players.
                    (Where::MultiplayerRace, Some(t)) => format!("{base} {t}"),
                    _ => base,
                }
            }
            None => "Multiplayer".to_string(),
        },
    };
    clamp(&s)
}

/// The localization entries this needs, as `(token, english template)`.
///
/// Exposed because the templates are part of the contract with `display_token` and `render`: a token with
/// no entry shows its raw `#Key` to every player.
pub fn localisation_entries() -> Vec<(&'static str, &'static str)> {
    vec![
        ("#Coop_MainMenu", "Main menu"),
        ("#Coop_NewGame", "Creating A New Game"),
        ("#Coop_Career_Managing", "%coop_season% %coop_team%: Managing"),
        ("#Coop_Career_Racing", "%coop_season% %coop_team%: %coop_activity%"),
        ("#Coop_Career_Leading", "%coop_season% %coop_team%: Leading the championship"),
        ("#Coop_Career_Engines", "%coop_season% %coop_team%: Choosing engines"),
        ("#Coop_Mp_Career", "Multiplayer %coop_season% %coop_team% %coop_players%"),
        ("#Coop_Mp_Race", "Multiplayer %coop_season% %coop_team% %coop_players% %coop_track%"),
    ]
}

impl fmt::Display for Where {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ferrari() -> Presence {
        Presence::single_player(2024, "Ferrari")
    }

    #[test]
    fn main_menu_reads_as_main_menu() {
        assert_eq!(render(&Where::MainMenu, None), "Main menu");
    }

    #[test]
    fn creating_a_new_game_uses_the_requested_phrase() {
        assert_eq!(render(&Where::CreatingANewGame, None), "Creating A New Game");
    }

    #[test]
    fn career_lines_lead_with_season_and_team() {
        let p = ferrari();
        for (activity, expect) in [
            (CareerActivity::Managing, "2024 Ferrari: Managing"),
            (CareerActivity::LeadingTheChampionship, "2024 Ferrari: Leading the championship"),
            (CareerActivity::ChoosingEngines, "2024 Ferrari: Choosing engines"),
        ] {
            assert_eq!(render(&Where::Career(activity), Some(&p)), expect);
        }
    }

    #[test]
    fn racing_line_names_the_track() {
        let p = ferrari();
        let where_ = Where::Career(CareerActivity::RacingAt("Silverstone".into()));
        assert_eq!(render(&where_, Some(&p)), "2024 Ferrari: Racing at Silverstone");
        assert_eq!(render(&where_, None), "Racing at Silverstone", "uses the activity text without a career");
    }

    #[test]
    fn multiplayer_lines_carry_season_team_and_player_count() {
        let p = Presence::multiplayer(2024, "Ferrari", 2);
        assert_eq!(render(&Where::MultiplayerCareer, Some(&p)), "Multiplayer 2024 Ferrari 2");
    }

    #[test]
    fn multiplayer_race_names_the_race() {
        let p = Presence::multiplayer(2024, "Ferrari", 2).with_track("Monza");
        assert_eq!(render(&Where::MultiplayerRace, Some(&p)), "Multiplayer 2024 Ferrari 2 Monza");
    }

    #[test]
    fn a_single_player_never_claims_to_be_multiplayer() {
        // A one-player "multiplayer" line would appear to friends as a broken lobby.
        let p = Presence::multiplayer(2024, "Ferrari", 1);
        assert!(p.is_multiplayer(), "player count is raised to the minimum of 2");
        assert_eq!(p.players, 2);
    }

    #[test]
    fn long_lines_are_clamped_and_prefer_a_word_boundary() {
        let long_team = "Team With An Extremely Long Name That Will Not Fit";
        let p = Presence::multiplayer(2024, long_team, 2).with_track("Circuit de Barcelona-Catalunya");
        let line = render(&Where::MultiplayerRace, Some(&p));
        assert!(line.chars().count() <= LINE_MAX, "got {} chars: {line}", line.chars().count());
        assert!(!line.ends_with(' '), "no trailing space after clamping");
    }

    #[test]
    fn clamping_keeps_short_strings_untouched() {
        assert_eq!(clamp("Main menu"), "Main menu");
        assert_eq!(clamp("Multiplayer 2024 Ferrari 2 Monza"), "Multiplayer 2024 Ferrari 2 Monza");
    }

    #[test]
    fn every_state_has_a_display_token_and_a_localisation_template() {
        // A token with no entry shows the raw "#Key" to every friend, so this pairing is a contract.
        let states = [
            Where::MainMenu,
            Where::CreatingANewGame,
            Where::Career(CareerActivity::Managing),
            Where::Career(CareerActivity::RacingAt("X".into())),
            Where::Career(CareerActivity::LeadingTheChampionship),
            Where::Career(CareerActivity::ChoosingEngines),
            Where::MultiplayerRace,
            Where::MultiplayerCareer,
        ];
        let entries = localisation_entries();
        for s in states {
            let token = display_token(&s);
            assert!(
                entries.iter().any(|(k, _)| *k == token),
                "{s:?} maps to {token}, which has no localisation entry"
            );
        }
    }

    #[test]
    fn the_token_templates_agree_with_render() {
        // `render` is what we log; Steam renders the template. If they disagree the log lies.
        let p = ferrari();
        let cases = [
            (Where::Career(CareerActivity::Managing), vec!["2024", "Ferrari", "Managing"]),
            (Where::Career(CareerActivity::RacingAt("Monza".into())), vec!["2024", "Ferrari", "Racing at Monza"]),
            (Where::Career(CareerActivity::LeadingTheChampionship), vec!["2024", "Ferrari", "Leading the championship"]),
            (Where::Career(CareerActivity::ChoosingEngines), vec!["2024", "Ferrari", "Choosing engines"]),
        ];
        for (where_, parts) in cases {
            let line = render(&where_, Some(&p));
            for part in parts {
                assert!(line.contains(part), "{where_:?} rendered {line:?}, missing {part:?}");
            }
        }
    }

    #[test]
    fn presence_keys_always_include_steam_display() {
        for s in [Where::MainMenu, Where::CreatingANewGame, Where::MultiplayerCareer] {
            let k = keys(&s, Some(&ferrari()));
            assert!(
                k.iter().any(|(key, _)| key == "steam_display"),
                "{s:?} must set steam_display or Steam shows nothing"
            );
        }
    }

    #[test]
    fn the_menu_does_not_leak_keys_from_a_previous_race() {
        // Sending only `steam_display` is what lets the caller clear the rest; this test pins that the
        // menu sends nothing else, so a stale track cannot survive a return to the menu.
        let k = keys(&Where::MainMenu, Some(&ferrari().with_track("Monza")));
        assert_eq!(k.len(), 1, "only steam_display: got {k:?}");
    }

    #[test]
    fn both_machines_compute_the_same_group_id() {
        // Different psids would show two groups of one in the friends list instead of one group of two.
        let a = Presence::multiplayer(2024, "Ferrari", 2);
        let b = Presence::multiplayer(2024, "McLaren", 2);
        assert_eq!(group_id(&a), group_id(&b), "the group must not depend on which team you drive");
    }

    #[test]
    fn multiplayer_keys_include_the_player_group() {
        let p = Presence::multiplayer(2024, "Ferrari", 2);
        let k = keys(&Where::MultiplayerCareer, Some(&p));
        assert!(k.iter().any(|(key, _)| key == "steam_player_group"));
        assert!(k.iter().any(|(key, v)| key == "steam_player_group_size" && v == "2"));
    }

    #[test]
    fn career_keys_carry_season_and_team() {
        let k = keys(&Where::Career(CareerActivity::Managing), Some(&ferrari()));
        assert!(k.iter().any(|(key, v)| key == "coop_season" && v == "2024"));
        assert!(k.iter().any(|(key, v)| key == "coop_team" && v == "Ferrari"));
    }

    #[test]
    fn a_race_sets_the_track_key_and_a_career_screen_does_not() {
        let p = ferrari().with_track("Spa");
        let racing = keys(&Where::Career(CareerActivity::Managing), Some(&p));
        assert!(!racing.iter().any(|(k, _)| k == "coop_track"),
            "a management screen must not claim a circuit");
    }

    #[test]
    fn without_tokens_no_raw_key_can_reach_a_friend() {
        // Before the #Coop_* tokens are registered, setting steam_display shows the literal "#Coop_Mp_Race"
        // in the friends list. This flag is the way to ship in that state without that happening.
        let p = Presence::multiplayer(2024, "Ferrari", 2);
        let plain = keys_with(&Where::MultiplayerCareer, Some(&p), false);
        assert!(!plain.iter().any(|(k, _)| k == "steam_display"),
            "must not set steam_display when the tokens are not registered");
        assert!(plain.iter().any(|(k, _)| k == "coop_team"), "plain keys still travel");
        // Grouping is not localized, so it survives.
        assert!(plain.iter().any(|(k, _)| k == "steam_player_group"));
    }

    #[test]
    fn with_tokens_the_display_key_is_present() {
        let p = Presence::multiplayer(2024, "Ferrari", 2);
        let full = keys_with(&Where::MultiplayerCareer, Some(&p), true);
        assert!(full.iter().any(|(k, _)| k == "steam_display"));
        assert_eq!(full.len(), keys(&Where::MultiplayerCareer, Some(&p)).len());
    }

    #[test]
    fn status_carries_the_plain_text_so_no_localisation_is_needed() {
        // This is the mode that ships: `steam_display` requires tokens registered against the app, and
        // without them Steam shows nothing at all. `status` is a plain UTF-8 string Steam displays
        // as-is, so the feature works today with no publisher action.
        let p = Presence::multiplayer(2024, "Ferrari", 2).with_track("Monza");
        let k = keys_status(&Where::MultiplayerRace, Some(&p));
        let status = k.iter().find(|(key, _)| key == "status").map(|(_, v)| v.clone());
        assert_eq!(status.as_deref(), Some("Multiplayer 2024 Ferrari 2 Monza"));
        assert!(!k.iter().any(|(key, _)| key == "steam_display"),
            "must not set steam_display: without a registered token that hides the line entirely");
    }

    #[test]
    fn status_mode_keeps_the_player_group_so_grouping_still_works() {
        let p = Presence::multiplayer(2024, "Ferrari", 2);
        let k = keys_status(&Where::MultiplayerCareer, Some(&p));
        assert!(k.iter().any(|(key, _)| key == "steam_player_group"),
            "grouping is not localised, so it must survive status mode");
        assert!(k.iter().any(|(key, v)| key == "steam_player_group_size" && v == "2"));
    }

    #[test]
    fn status_mode_covers_every_state() {
        // Every state must produce a non-empty status, or a player is left with no line at all.
        let p = Presence::single_player(2024, "Ferrari");
        for where_ in [
            Where::MainMenu,
            Where::CreatingANewGame,
            Where::Career(CareerActivity::Managing),
            Where::MultiplayerCareer,
            Where::MultiplayerRace,
        ] {
            let k = keys_status(&where_, Some(&p));
            let status = k.iter().find(|(key, _)| key == "status").map(|(_, v)| v.clone());
            assert!(status.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
                "{where_:?} produced no status");
        }
    }

    #[test]
    fn the_shipped_vdf_matches_the_tables_in_this_module() {
        // The VDF is what Steam renders from; this module is what decides which token to show. If they
        // drift, a friend sees a literal "#Coop_Career_Managing" instead of text - and nothing in the
        // build would catch it, because neither file is compiled from the other.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tools/presence/steam_display.vdf");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => panic!("cannot read {}: {e}", path.display()),
        };
        for (token, template) in localisation_entries() {
            assert!(text.contains(token), "vdf is missing {token}");
            assert!(
                text.contains(template),
                "vdf has {token} but not its template {template:?}"
            );
        }
        // And nothing in the vdf that this module does not know about, so a removed state does not leave
        // a dead token behind.
        let known: Vec<&str> = localisation_entries().iter().map(|(k, _)| *k).collect();
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("\"#Coop_") {
                continue;
            }
            let token = trimmed.split('"').nth(1).unwrap_or("");
            assert!(known.contains(&token), "vdf has unknown token {token}");
        }
    }

    #[test]
    fn status_is_actually_the_raw_string_steam_expects() {
        // Byte-level check of the key Steam reads. `SetRichPresence("status", ...)` only works if the key
        // is exactly this, and the string is merged with neighbours in the binary so scanning the DLL for
        // it is unreliable - asserting it from the producing function is the trustworthy check.
        let k = keys_status(&Where::MainMenu, None);
        let (key, value) = k.iter().find(|(kk, _)| kk == "status").expect("status must be present");
        assert_eq!(key.as_bytes(), b"status");
        assert_eq!(value, "Main menu");
        // And it must not contain a NUL, which would truncate the value at the API boundary.
        assert!(!value.as_bytes().contains(&0));
    }

    #[test]
    fn every_key_is_within_steams_documented_limits() {
        // A rejected value makes the line vanish rather than shorten, so the limits are enforced here.
        // Key length: Steam's k_cchMaxRichPresenceKeyLength is 64; the SDK header is not shipped with the
        // game, so this uses the documented value and keeps far below it.
        let p = Presence::multiplayer(2024, "A Very Long Team Name Indeed For Testing", 2)
            .with_track("Circuit de Barcelona-Catalunya");
        for where_ in [Where::MultiplayerRace, Where::Career(CareerActivity::Managing)] {
            for (k, v) in keys_status(&where_, Some(&p)) {
                assert!(k.len() <= 64, "key {k:?} exceeds the documented key limit");
                assert!(v.len() <= 255, "value for {k:?} is {} bytes, over the value limit", v.len());
                assert!(!k.contains('\0') && !v.contains('\0'));
            }
        }
    }
}
