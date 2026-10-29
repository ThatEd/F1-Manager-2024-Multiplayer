# Steam "Join Game": how a friend finds and joins a session

The requirement was narrow: **when the host is sitting on the host-game tab, a friend should be able to click
Join Game in Steam and end up in that session.** This records what Steam needs for that, what was built, and
what is still unverified.

Tags are the same as everywhere else in these notes: `proven` (a test or a shipped binary shows it), `inferred`
(follows from the code, not executed), `unknown` (needs a live Steam client and the game running).

## What Steam actually requires

Read from Valve's documentation rather than assumed:

- A joinable session is a **lobby** — "a entity that lives on the Steam back-end servers". A friend's client
  finds it with `SteamFriends()->GetFriendGamePlayed()`, whose `FriendGameInfo_t` carries `m_steamIDLobby`, and
  joins with `ISteamMatchmaking::JoinLobby`. Without a lobby there is nothing to click. `proven` (docs).
- If the invitee is **not running the game**, accepting an invite launches it with
  `+connect_lobby <64-bit lobby id>`, and "it's up to your game to decide to obey it or not". `proven` (docs).
- If the invitee **is** running the game, a `GameLobbyJoinRequested_t` callback is posted containing the lobby
  id. `proven` (docs).
- Lobbies should only exist while needed: "only create lobbies when needed ... when the user invites a friend
  to play, or triggers a manual action that requires a lobby". This is why the lobby's lifetime is the host
  tab, not the process. `proven` (docs).

**F1 Manager 2024 ships no matchmaking of its own.** Its exe contains `steam_api64` / `SteamAPI_Init` /
`SteamFriends` and nothing about lobbies, so every piece below is ours.

## What was built

### 1. `lobby_policy.rs` — the decisions, pure and tested (13 tests)

- `MARKER_VALUE = "f1m24-coop"` and `KEY_VERSION` carrying `coop_proto::VERSION`. **Both are needed**, and the
  reason is specific: every player in a friend's lobby is running app id 2591280, so the app id cannot tell
  *(a co-op session)* from *(a friend playing F1 Manager alone)*. Without a marker, a "Join Game" click would
  attempt to connect to a single-player game.
- `accept_friend()` refuses in a fixed order with four **distinct** messages. Telling someone "different co-op
  build" when they are simply playing alone sends them looking for a problem that does not exist, so the
  distinction is asserted, not just intended.
- `HostState` ties the lobby to the tab: `Idle` (no lobby), `Waiting` (joinable), `Filled` (lobby retained,
  **closed to new joins**). `Filled` is deliberate — a second friend clicking Join Game on a two-player session
  would be admitted by Steam and then refused by the mod, which reads as a bug. `SetLobbyJoinable(false)` keeps
  the lobby (the current peer's route depends on it) while closing it.
- `parse_connect_lobby()` handles the launch-from-invite command line, including the quoted form and
  case-insensitive matching.

### 2. `lobby_steam.rs` — the FFI, every signature taken from a header

The flat wrappers do **not** mirror the class methods' return types, and three of them would corrupt memory if
guessed by analogy:

| function | class method returns | flat wrapper returns |
|---|---|---|
| `GetLobbyData` | `bool`-looking | **`const char *`** |
| `SetLobbyData` | `bool` | `bool` |
| `CreateLobby` / `JoinLobby` | — | **`SteamAPICall_t` (u64)** |

All types here were copied from `steam_api_flat.h` / `isteammatchmaking.h` and are pinned by assertions in
`tools/headless/lobby_api_test.js` so a future edit cannot quietly "simplify" them to `bool`.

**The callback object is a vtable and its layout is load-bearing.** `CCallbackBase` is
`{ vptr, u8 flags, i32 id }` = 16 bytes, id at offset 12, three vtable slots in declaration order
(`Run(void*)`, `Run(void*,bool,SteamAPICall_t)`, `GetCallbackSizeBytes()`). `GameLobbyJoinRequested_t` is
`k_iSteamFriendsCallbacks + 33` carrying two `u64`s. Rust asserts the sizes, the alignments and both offsets,
because a mismatch means Steam writing into the wrong field of our struct **inside the game process**.

Also verified: `FriendGameInfo_t` puts `m_steamIDLobby` at **offset 16** of a 24-byte struct — get that wrong
and every join targets a number assembled out of port fields. Asserted.

Every function used is exported by the shipped `steam_api64.dll` (298,384 bytes). `proven` (binary scan).

### 3. Agent endpoints

| endpoint | purpose |
|---|---|
| `GET /coop/lobby` | the session as a friend would see it: `state`, `lobby_id`, `joinable`, marker, version |
| `GET/POST /coop/lobby/set?state=waiting\|filled\|idle` | drives the lobby from the host tab |
| `GET/POST /coop/lobby/join?lobby=<id>` or `?friend=<steamid64>` | joins; refuses **before** connecting |
| `GET /coop/connectlobby` | reports `+connect_lobby <id>` from the process command line |

`GET` is accepted alongside `POST` for the same reason as the coordination endpoints: the caller is
`XMLHttpRequest` inside the game's web view, where a preflight is one more thing to get wrong.

**`state` and `lobby_id` are reported separately on purpose.** "The host tab is open but nothing was
advertised" is the failure that wastes a session, and it is only visible if intent and reality are both in the
response. A failed `attach` answers `state=unavailable` with the reason — never a cheerful "waiting".

### 4. Front-end

- `lobby.js`: `Screen.prototype.host` opens the lobby (`state=waiting`) when the host tab is chosen;
  `goBack` closes it (`state=idle`) when leaving. The call is **fire-and-forget** — the visible status is about
  the sim being armed, and reporting a Steam failure as "cannot host" would point at the wrong problem.
- `lobby.js`: `Screen.prototype.join` asks Steam to join, and surfaces a refusal instead of leaving the client
  waiting for a host that was never going to answer.
- `bridge.js`: `connectFromInvite()` runs once on boot and reads `/coop/connectlobby`, so launching the game by
  accepting an invite actually joins instead of landing on the main menu.

## Test totals

| suite | result |
|---|---|
| `cargo test --workspace --exclude coop-mem` | **231 passed** (was 212; +13 policy, +6 FFI layout) |
| `tools/headless/lobby_api_test.js` | **25/25** |
| `tools/headless/observe_test.js` | 67/67 |
| `tools/headless/normal_play_test.js` | 21/21 |
| `tools/headless/agent_coord_e2e.js` | 16/16 |
| `tools/headless/run_tests.js` | 41/41 |

`lobby_api_test.js` checks the routes, the honest-degradation path, the FFI signatures against the header
types, and that the host tab is wired in `lobby.js` (not `bridge.js` — the first version of that assertion
looked in the wrong file and failed, which is why it now names its subject).

## What is still unverified, and why it matters here

1. **That a lobby appears and is joinable.** `CreateLobby` is asynchronous and returns a `SteamAPICall_t`.
   `create()` now registers a `LobbyCreated_t` call result for that specific call and records the id, so the
   host does learn it - but whether Steam actually creates the lobby still needs a live Steam client.
2. **That the host learns its own lobby id.** `owned_lobby()` confirms by asking `GetLobbyOwner` and comparing
   with the local user, which works without a call-result handler but requires knowing the local SteamID.
   `unknown` until run.
3. **That the `GameLobbyJoinRequested_t` callback fires.** The struct layout and callback id are asserted
   against the headers, but registration and dispatch cannot be tested outside a live Steam client.
4. **That a friend's "Join Game" appears at all.** This is Steam's UI, driven by the lobby existing. Steam's own
   test page (`steamcommunity.com/dev/testrichpresence`) does **not** cover lobbies, so this can only be
   checked from a second Steam account.
5. **Publisher registration.** Lobbies need no rich-presence localisation tokens, which is why this path was
   chosen over `steam_display`. That much is a genuine advantage over the presence work: nothing here depends
   on a publisher-registered tag.

## Three bugs that made invites impossible, found after the first "invites do not work" report

All three are fixed. Recording them because each one *looked* like a Steam problem from the outside.

1. **The callback id was wrong.** `GAME_LOBBY_JOIN_REQUESTED` was written as `3301 + 33`, i.e. `3334`.
   `k_iSteamFriendsCallbacks` is **300**, not 3301 (`SteamConstants.cs`, generated from Valve's
   `steam_api_common.h`), so the real id is **333**. `RegisterCallback` accepts an unknown id silently, so the
   mod registered "successfully" and then waited forever for an invite that had already arrived. The unit test
   asserted `3301 + 33` - it pinned the bug, which is why it never caught it.
2. **No lobby id was ever obtained.** `create()` discarded the `SteamAPICall_t` and returned `0`, so `LOBBY`
   stayed `(Waiting, 0)`. With no id, the metadata marker was never written, `SetLobbyJoinable` was never
   called, and there was no id to put in an invite. Now `create()` calls `RegisterCallResult` for the specific
   call and `lobby_set` adopts the id from `take_created_lobby()` on the next pass.
3. **The invite path refused the normal case.** `lobby_join` with no arguments returned
   `"give lobby=<id> or friend=<steamid64>"` - but no arguments *is* what the front-end sends after a player
   clicks Join Game, because the id arrives on the callback. Separately, `GET /coop/lobby` reported
   `pending_join` with `take_wanted_lobby()`, so the host tab's status poll *consumed* the invite it was
   displaying. It now uses `peek_wanted_lobby()` and only `lobby_join` clears it.

There was also nothing that opened Steam's invite dialog: `ActivateGameOverlayInviteDialog` was bound but never
called, so an invite could not be sent by hand either. `POST /coop/lobby/invite` and an "Invite a friend" button
on the host tab now do that, and refuse when there is no lobby id rather than opening a dialog that cannot work.

## Deliberately not done

- **`ISteamParties` beacons.** The DLL exports `SteamAPI_SteamParties_v002` and it is the modern way to
  advertise a joinable party, but it is more surface (reservation callbacks, beacon locations) for the same
  visible result. A lobby is the documented mechanism for "friend clicks Join Game" and is one interface
  instead of two.
- **A lobby browser.** Two-player co-op between friends needs no search UI; the friends list *is* the browser.
- **`SetRichPresence` "join" wiring.** Presence is already handled separately (`RICH-PRESENCE.md`) and uses
  `status`, which needs no registered token. Lobby joinability does not depend on it.
