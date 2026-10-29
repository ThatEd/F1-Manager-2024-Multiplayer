//! Steam lobby policy: which lobby invites a 2-player target, and whether what a friend advertises is ours.
//!
//! Pure and dependency-free so it is testable without Steam, a game or a network. The unsafe FFI lives in
//! `lobby_steam`; everything about *meaning* lives here.
//!
//! ## Why a lobby at all
//!
//! Steam's own documentation is explicit that a lobby is what makes a friend's session joinable: "A lobby
//! is a entity that lives on the Steam back-end"; a client finds its friends' lobbies with
//! `SteamFriends()->GetFriendGamePlayed()` and a valid `FriendGameInfo_t::m_steamIDLobby`, and joins with
//! `ISteamMatchmaking::JoinLobby`. F1 Manager 2024 ships no matchmaking of its own, so the mod has to create
//! the lobby for "Join Game" to appear at all.
//!
//! ## Why the metadata keys rather than trusting the app id
//!
//! Every player in a friend's lobby is running app id 2591280, so the app id cannot distinguish *(this mod's
//! coop session)* from *(a friend who happens to be playing F1 Manager alone)*. It also cannot distinguish a
//! lobby this mod created from one a different mod created. So the lobby carries an explicit marker, and a
//! join is accepted only when that marker is present and the version matches.

use crate::presence::APP_ID;

/// Steady-state party size. Two players: this is a two-team co-op mod, not a server browser.
pub const PARTY_SIZE: u8 = 2;

/// Lobby metadata key carrying the marker that identifies a co-op lobby as ours.
pub const KEY_MARKER: &str = "coop_mod";
/// The value of [`KEY_MARKER`]. Deliberately not the app id: a marker that is just the app id would also be
/// matched by an unrelated tool advertising the same number.
pub const MARKER_VALUE: &str = "f1m24-coop";
/// Lobby metadata key carrying the wire-protocol version, so a peer running a different build is refused
/// before it connects rather than after the first frame fails to decode.
pub const KEY_VERSION: &str = "coop_proto";

/// The whole wire version, as a string, for [`KEY_VERSION`].
///
/// Taken from `coop_proto::VERSION` rather than hard-coded so the lobby and the transport cannot disagree -
/// bumping the protocol once must not leave the lobby advertising the old number.
pub fn version_marker() -> String {
    coop_proto::VERSION.to_string()
}

/// What the host advertises while it is waiting for a player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advert {
    /// The app id, so a human reading the lobby list can tell it is the right game.
    pub app_id: u32,
    pub marker: &'static str,
    pub version: String,
    /// Visible vs invite-only. See [`ELobbyType`].
    pub lobby_type: ELobbyType,
    pub max_members: i32,
}

/// `ISteamMatchmaking` lobby visibility (`ELobbyType` in `isteammatchmaking.h`).
///
/// Only the two values this mod uses, so an invalid number cannot be constructed by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ELobbyType {
    /// `k_ELobbyTypePrivate` — invite-only, invisible to search. The default for a two-friend co-op session:
    /// it needs no lobby list UI, and an unlisted lobby cannot be stumbled into by a stranger.
    Private,
    /// `k_ELobbyTypeFriendsOnly` — listed to friends only.
    FriendsOnly,
}

impl ELobbyType {
    /// The value Steam expects.
    pub fn as_i32(self) -> i32 {
        match self {
            // isteammatchmaking.h: k_ELobbyTypePrivate = 0, k_ELobbyTypeFriendsOnly = 1
            ELobbyType::Private => 0,
            ELobbyType::FriendsOnly => 1,
        }
    }
}

/// The metadata to write after the lobby is created.
///
/// `Private` by default: the game has no lobby browser, so a public lobby would be one nothing can display
/// while still being joinable by anyone who guessed. Friends-only stays joinable from the friends list, which
/// is the actual requirement.
pub fn advert() -> Advert {
    Advert {
        app_id: APP_ID,
        marker: MARKER_VALUE,
        version: version_marker(),
        lobby_type: ELobbyType::Private,
        max_members: PARTY_SIZE as i32,
    }
}

/// The key/value pairs to `SetLobbyData`, in the order they should be written.
pub fn lobby_data() -> Vec<(&'static str, String)> {
    let a = advert();
    vec![
        (KEY_MARKER, a.marker.to_string()),
        (KEY_VERSION, a.version),
        // Player count is deliberately not written: Steam docs say "You will not need to put in player
        // counts, as a lobby search will already look for lobbies with available slots." Writing it would be
        // a field nobody reads and one more thing to keep in sync.
    ]
}

/// Why a lobby a friend is in was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// The lobby does not carry the mod's marker, so it is not a co-op session: the friend is playing alone.
    NotOurs,
    /// The marker is present but the protocol version differs. Refused *before* connecting, because the
    /// transport would reject every frame anyway and a half-connected session looks like a crash.
    VersionMismatch,
    /// The lobby advertises a different app id.
    WrongApp,
    /// Steam reported no lobby at all, which is the normal case for a single-player friend.
    NoLobby,
}

impl Reject {
    /// The line the operator sees. Names the state, not the plumbing.
    pub fn message(self) -> &'static str {
        match self {
            Reject::NotOurs => "that friend is not hosting a co-op session",
            Reject::VersionMismatch => "that player is running a different co-op build",
            Reject::WrongApp => "that lobby is not F1 Manager 2024",
            Reject::NoLobby => "that friend has no joinable session",
        }
    }
}

/// One friend's advertised session, as read back from Steam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendSession {
    pub app_id: u32,
    /// `FriendGameInfo_t::m_steamIDLobby` - zero when the friend is in no lobby.
    pub lobby_id: u64,
    /// `GetLobbyData(lobby, KEY_MARKER)`; empty when the caller has not fetched it yet.
    pub marker: String,
    /// `GetLobbyData(lobby, KEY_VERSION)`.
    pub version: String,
}

/// Decides whether a friend's session is one this build can join.
///
/// Order matters: the cheap checks come first, and each rejection has a different message because the player's
/// next action differs. Telling someone "different co-op build" when they are simply playing alone would send
/// them looking for a version problem that does not exist.
pub fn accept_friend(session: &FriendSession) -> Result<u64, Reject> {
    if session.lobby_id == 0 {
        return Err(Reject::NoLobby);
    }
    if session.app_id != APP_ID {
        return Err(Reject::WrongApp);
    }
    // An empty marker is "not fetched yet" or "not ours"; either way it is not evidence of a co-op lobby, and
    // joining on the absence of information is how a player ends up connected to something unexpected.
    if session.marker != MARKER_VALUE {
        return Err(Reject::NotOurs);
    }
    if session.version != version_marker() {
        return Err(Reject::VersionMismatch);
    }
    Ok(session.lobby_id)
}

/// The `+connect_lobby` command line Steam launches the game with when a friend accepts an invite.
///
/// Steam documents the exact shape: "it will launch the game with the command-line: `+connect_lobby <64-bit
/// lobby id>`". Parsed here because the alternative - reading it in the agent from `GetCommandLineW` - cannot
/// distinguish a lobby id from any other argument.
pub fn parse_connect_lobby(cmdline: &str) -> Option<u64> {
    let mut it = cmdline.split_whitespace();
    while let Some(tok) = it.next() {
        if tok.eq_ignore_ascii_case("+connect_lobby") {
            let id = it.next()?;
            let id = id.trim_matches('"');
            // A 64-bit lobby id is decimal per Steam's own examples. Rejecting a non-numeric token is better
            // than defaulting to 0, which would look like "no lobby".
            return id.parse::<u64>().ok().filter(|v| *v != 0);
        }
    }
    None
}

/// Whether a host should still be advertising.
///
/// A lobby is created when the host opens the host-game tab and torn down when it leaves it. Steam's own
/// guidance - "only create lobbies when needed ... when the user invites a friend to play, or triggers a
/// manual action that requires a lobby" - is why this is tied to the tab rather than to game launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    /// Not hosting. No lobby should exist.
    Idle,
    /// On the host tab with no player yet. The lobby exists and is joinable.
    Waiting,
    /// A player has joined. The lobby stays: the friend's client used it to find us, and tearing it down
    /// would make a reconnect impossible.
    Filled,
}

impl HostState {
    /// Whether a lobby should be open in this state.
    pub fn should_advertise(self) -> bool {
        !matches!(self, HostState::Idle)
    }

    /// Whether Steam should still show the session as joinable.
    ///
    /// `Waiting` only: once a player has joined, a second friend clicking "Join Game" would be let into a
    /// two-player session and then refused by the mod, which reads as a bug. `SetLobbyJoinable(false)` is the
    /// documented way to keep the lobby (so the current peer keeps its route) while closing it to new joins.
    pub fn joinable(self) -> bool {
        matches!(self, HostState::Waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(app: u32, lobby: u64, marker: &str, version: &str) -> FriendSession {
        FriendSession {
            app_id: app,
            lobby_id: lobby,
            marker: marker.to_string(),
            version: version.to_string(),
        }
    }

    #[test]
    fn a_lobby_this_mod_created_is_accepted() {
        let s = session(APP_ID, 12345, MARKER_VALUE, &version_marker());
        assert_eq!(accept_friend(&s), Ok(12345));
    }

    #[test]
    fn a_friend_playing_alone_is_not_a_session() {
        // The normal case: a friend is playing F1 Manager single-player, so there is no lobby at all. This must
        // say so, not "different build" - the player's next action is different.
        assert_eq!(accept_friend(&session(APP_ID, 0, "", "")), Err(Reject::NoLobby));
    }

    #[test]
    fn a_lobby_without_the_marker_is_refused_even_for_the_same_game() {
        // Same app id, a valid lobby, but not this mod: a friend in someone else's lobby, or a lobby created by
        // another tool. Joining it would connect the game to something that does not speak our protocol.
        let s = session(APP_ID, 777, "", &version_marker());
        assert_eq!(accept_friend(&s), Err(Reject::NotOurs));
        assert_eq!(accept_friend(&s).unwrap_err().message(), "that friend is not hosting a co-op session");
    }

    #[test]
    fn a_different_protocol_version_is_refused_before_connecting() {
        // The transport refuses a mismatched version at the header, so connecting first would produce a session
        // that looks established and then fails on the first frame.
        let s = session(APP_ID, 777, MARKER_VALUE, "999");
        assert_eq!(accept_friend(&s), Err(Reject::VersionMismatch));
    }

    #[test]
    fn a_lobby_for_another_game_is_refused() {
        assert_eq!(accept_friend(&session(1, 777, MARKER_VALUE, &version_marker())), Err(Reject::WrongApp));
    }

    #[test]
    fn the_version_marker_follows_the_wire_version() {
        // Bumping coop_proto::VERSION must move the lobby marker with it; a host advertising the old number
        // would be refused by clients on the new build forever.
        assert_eq!(version_marker(), coop_proto::VERSION.to_string());
        assert_eq!(lobby_data().iter().find(|(k, _)| *k == KEY_VERSION).unwrap().1, version_marker());
    }

    #[test]
    fn lobby_data_carries_the_marker_and_the_version_and_nothing_else() {
        let d = lobby_data();
        assert_eq!(d.len(), 2, "only the two keys that are read: {d:?}");
        assert_eq!(d[0].0, KEY_MARKER);
        assert_eq!(d[1].0, KEY_VERSION);
    }

    #[test]
    fn a_private_two_player_lobby_is_the_default() {
        let a = advert();
        assert_eq!(a.lobby_type, ELobbyType::Private);
        assert_eq!(a.max_members, 2);
        assert_eq!(a.app_id, APP_ID);
    }

    #[test]
    fn lobby_type_numbers_match_the_sdk_enum() {
        // k_ELobbyTypePrivate = 0, k_ELobbyTypeFriendsOnly = 1 (isteammatchmaking.h). A wrong number would
        // create a public lobby, which for a game with no lobby browser is invisible and uninvited traffic.
        assert_eq!(ELobbyType::Private.as_i32(), 0);
        assert_eq!(ELobbyType::FriendsOnly.as_i32(), 1);
    }

    #[test]
    fn the_host_only_advertises_while_on_the_host_tab() {
        assert!(!HostState::Idle.should_advertise());
        assert!(HostState::Waiting.should_advertise());
        // Once filled the lobby stays open - the peer route depends on it - but stops accepting joins.
        assert!(HostState::Filled.should_advertise());
        assert!(HostState::Waiting.joinable());
        assert!(!HostState::Filled.joinable(),
            "a second friend must not be let into a two-player session");
        assert!(!HostState::Idle.joinable());
    }

    #[test]
    fn the_launch_command_steams_invite_uses_is_parsed() {
        // Exactly what Steam documents for accepting an invite while the game is not running.
        assert_eq!(parse_connect_lobby("F1Manager24.exe +connect_lobby 109775241234567890"), Some(109775241234567890));
        assert_eq!(parse_connect_lobby("+connect_lobby 42"), Some(42));
        // Steam may quote it.
        assert_eq!(parse_connect_lobby("+connect_lobby \"42\""), Some(42));
        assert_eq!(parse_connect_lobby("F1Manager24.exe -windowed"), None);
        assert_eq!(parse_connect_lobby(""), None);
        // A missing or zero id is not a lobby.
        assert_eq!(parse_connect_lobby("+connect_lobby"), None);
        assert_eq!(parse_connect_lobby("+connect_lobby 0"), None);
        assert_eq!(parse_connect_lobby("+connect_lobby notanumber"), None);
    }

    #[test]
    fn the_launch_command_is_matched_case_insensitively() {
        // Command lines are not normalised by Windows, and a player's shortcut can carry either case.
        assert_eq!(parse_connect_lobby("+Connect_Lobby 42"), Some(42));
    }

    #[test]
    fn every_rejection_has_a_message_a_player_can_act_on() {
        for r in [Reject::NotOurs, Reject::VersionMismatch, Reject::WrongApp, Reject::NoLobby] {
            assert!(!r.message().is_empty(), "{r:?}");
        }
        // They must be distinguishable: two different causes printing the same line is how a version problem
        // gets misreported as a missing session.
        let msgs: Vec<&str> = [Reject::NotOurs, Reject::VersionMismatch, Reject::WrongApp, Reject::NoLobby]
            .iter()
            .map(|r| r.message())
            .collect();
        let mut unique = msgs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), msgs.len());
    }
}
