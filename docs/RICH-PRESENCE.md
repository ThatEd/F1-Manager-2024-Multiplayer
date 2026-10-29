# Steam rich presence

What a friend sees, and — importantly — **how this works with no localisation and no publisher action.**

## The strings

Exactly as specified, with the state that produces each:

| situation | line |
|---|---|
| in a career, between races | `2024 Ferrari: Managing` |
| racing | `2024 Ferrari: Racing at Silverstone` |
| leading the championship | `2024 Ferrari: Leading the championship` |
| engine selection | `2024 Ferrari: Choosing engines` |
| main menu | `Main menu` |
| new-game flow | `Creating A New Game` |
| co-op career, no race loaded | `Multiplayer 2024 Ferrari 2` |
| co-op race weekend | `Multiplayer 2024 Ferrari 2 Monza` |

`Season TeamName` prefixes every career line. Multiplayer lines lead with `Multiplayer` and carry the
player count; the race line names the circuit instead of repeating the count, because a friend deciding
whether to watch cares which race, not how many people are in it.

## Why there is no localisation to get wrong

The obvious approach is `steam_display`, which names a localisation token. Steam's own documentation is
explicit that this is all-or-nothing:

> If `steam_display` is not set to a valid localization tag, then rich presence will **not be displayed**
> in the Steam client.

Registering those tokens against app 2591280 is a Steamworks Partner publishing action. Until it happens,
setting `steam_display` does not degrade gracefully — it hides the line entirely. And once registered, the
text is whatever the token file says, so the strings are no longer ours to control.

So the shipping path carries the text in **`status`**, which the same docs describe as *"a UTF-8 string
that will show up in the 'view game info' dialog"* — a plain value, displayed as-is, with no token
lookup. `keys_status()` emits:

```
status                  Multiplayer 2024 Ferrari 2 Monza     <- the literal text
coop_season             2024
coop_team               Ferrari
coop_players            2
coop_track              Monza
steam_player_group      f1m24-coop-2024-2p
steam_player_group_size 2
```

and deliberately **no** `steam_display`. Nothing can break because a token is missing, renamed, translated
differently, or never uploaded.

What this trades: `status` is shown in the game-info dialog and used for friend grouping, but it is **not**
the line under the player's name in the friends list — that is `steam_display`'s job alone. Adding the
tokens later is additive: `keys_with(.., true)` puts `steam_display` on top of the same key set, and the
same text then appears in both places. No rework, nothing to undo.

## Friend grouping

`steam_player_group` / `steam_player_group_size` group two friends together. The group id is derived from
season and player count — **not** a pid or timestamp — because both machines must compute the *same*
string or Steam shows two groups of one instead of one group of two. Tested directly: the id is identical
for two players driving different teams.

## What is verified, and what is not

| fact | evidence |
|---|---|
| Steam app id 2591280 | `steam_appid.txt` in `Binaries/Win64` and the install root |
| `steam_api64.dll` present | `Engine/Binaries/ThirdParty/Steamworks/Steamv155/Win64/steam_api64.dll`, 298,384 bytes |
| the exports we need exist | DLL contains `SteamAPI_ISteamFriends_SetRichPresence` and `..._ClearRichPresence` |
| the interface version string | the DLL contains exactly one: **`SteamAPI_SteamFriends_v017`** |
| the game sets no rich presence today | `F1Manager24.exe` has `steam_api64`, `SteamAPI_Init`, `SteamFriends`, but **zero** `SetRichPresence` / `steam_display` / `steam_player_group` |
| the code is in the built DLL | `coop_agent.dll` (351,744 bytes) contains `steam_api64.dll` ×2, `SteamAPI_SteamFriends_v017` ×1, `SetRichPresence` ×2, `steam_player_group` ×2 |
| the `status` key is byte-exact | asserted from the producing function (`b"status"`), not by scanning the DLL — the linker merges adjacent string literals, so a binary scan for a 6-byte string is unreliable |

The version string is worth calling out: assuming `Steamv155` implied `v018` would have been wrong. The
folder and the interface version are unrelated numbers. It was discovered by dumping the DLL, not guessed.

**Not verified:** that the calls *succeed* at runtime. That needs the game running with Steam attached and
a friend list to look at. `attach()` returns an `Err` with a reason rather than failing silently, so a miss
appears in the log instead of looking like "presence is just not showing".

The first build of this module was also **compiled out entirely** — with `lto` and `opt-level=3` an
unreferenced module is dropped, and the DLL contained no `steam_api64.dll` string at all while the build
reported success. It is now started from the agent's worker thread (`start_presence`), which is the only
reason it survives linking. Worth re-checking after any refactor.

## Tests

27 tests, all offline:

- every state produces a non-empty `status` — a state with none would leave a player with no line;
- `status` is byte-for-byte `b"status"` with no interior NUL (which would truncate at the API boundary);
- every key and value is inside Steam's documented length limits, because a rejected value makes the line
  vanish rather than shorten;
- `keys_status` never emits `steam_display`, so a missing token can never hide the line;
- the VDF on disk matches the Rust token tables, so editing one without the other fails the build —
  proved by planting a drifted template: `vdf has #Coop_MainMenu but not its template "Main menu"`;
- the group id is identical for two players on different teams;
- lines are clamped at 64 characters on a word boundary.

## Files

| file | role |
|---|---|
| `coop-rs/crates/coop-agent/src/presence.rs` | state → keys. Pure, 27 tests |
| `coop-rs/crates/coop-agent/src/presence_steam.rs` | resolve and call `SetRichPresence` |
| `tools/presence/steam_display.vdf` | tokens, **only needed for the friends-list line**, not for `status` |
| `coop-rs/crates/coop-agent/src/lib.rs` | `start_presence()`, called from `thread_entry` |

## Still to do

The ticker publishes the **menu** state only. Every other state is wired and tested but not yet driven: the
agent does not know when the game changes screen. The DOM hooks `press.js` already uses are the obvious
source, and that is the remaining work. Nothing about the Steam side blocks it.