# Lobby gate: one choke point for Continue, Advance and race weekend

## The problem this solves

Two players share one career. They must reach the next day, the next session and the next race
weekend *together*. Today each client presses its own button whenever it likes, so the two careers
drift apart the moment one player is slower.

## The single choke point

Every one of these buttons is a named Gameface event:

```
MainMenu.js                    sendEvent('ContinueGame')
LoadSaveScreen.js:128          sendEvent('LoadGame', slotName)
TimeProgressionPanel.js:152    sendEvent('AdvanceToNextEvent')
CalendarContents.js:278        sendEvent('NextMonthSelected')
```

and `sendEvent` is:

```js
function sendEvent(eventName, ...args) { engine.trigger(eventName, ...args); }
```

So there is exactly one function to intercept for all of them. The bridge wraps
`engine.trigger`. Anything not on the gate list passes straight through, untouched.

## Behaviour

| state | Continue | Advance | race weekend advance |
|---|---|---|---|
| **solo** (no lobby) | works as now | works as now | works as now |
| **in a lobby** | ready check | ready check | ready check |

"In a lobby" means the bridge is attached to a lobby session â€” the operator started the lobby and
this client joined it. It is not a global mod switch: a player who never joins a lobby is never
affected, which is the requirement the user stated.

### Ready check, concretely

1. The player presses Continue (or Advance, or the race-weekend button).
2. The gate swallows the event and sends `ready <event> <client-id>` to the server.
3. The server counts ready clients against expected clients.
4. When all are ready, the server broadcasts `go <event>`.
5. Each bridge fires the real event through the original `engine.trigger`.

The gate must also show the player something, or the button looks broken. Minimum viable: the button
label changes and a toast appears ("waiting for host"). The label already comes from
`AdvanceTypesData.getButtonData(advanceType)`, so overriding that is the cheap route.

### Why a ready check and not just a broadcast

A broadcast alone forces the event whether or not a player is ready â€” the exact bug from earlier in
this project, where a client was mid-load and got told to advance. The ready check is also where the
host's authorisation lives: only the host may trigger `go`, so a client cannot drag everyone forward.

### Timeouts

A player who never presses, or who disconnects, must not wedge the session forever.

- per-client ready deadline (say 60 s) â†’ the server reports who is missing and clears the check
- a client dropping out of the lobby reduces the expected count
- `go` carries a TTL; a client that polls after expiry ignores it rather than acting on a stale command

## Events that belong on the gate

From an exhaustive scan of the UI export (103 distinct `sendEvent` names):

| event | what the player pressed |
|---|---|
| `ContinueGame` | main-menu Continue |
| `LoadGame` | load a save from the load screen |
| `AdvanceToNextEvent` | the Advance button; also the sponsor-bonus confirm |
| `NextMonthSelected` | calendar month arrows |
| `CASLoadSave` | load from inside create-a-season |

`AdvanceToNextEvent` covers the race weekend. The calendar's next list is rendered from
`Calendar.UpcomingDays`, so free practice and a race weekend are the same button with a different
label; `AdvanceType.ToRaceWeekend = 42` versus `AdvanceType.ToPracticeSession1`. There is no separate
"enter race weekend" event to hunt for.

Deliberately NOT gated: everything cosmetic and editor-related (`LiveryEditor_*`, `EditStaff*`,
`TeamNameChange`, â€¦). Those change local state that is saved with the career, not the calendar, and
gating them would make the mod intrusive.

## Two events are front-end-only, and this is not optional

`LoadGame` and `ContinueGame` may only be fired from `Frontend.html`. Firing `LoadGame` from an in-game
view (`RaceWeekend.html`, `TeamManagement.html`) tears the front-end down and never rebuilds it,
leaving a **black screen with the process alive and responding**. Confirmed on a live client.

`LoadSaveScreen.onConfirmChoice` is the only issuer of `LoadGame`, and in-game it routes through a
confirmation dialog first; firing the raw event shortcuts that route.

So the lobby has a precondition: **every client must be at the main menu before a shared load is
issued.** Returning there is `sendEvent('QuitGame')`, the same event Options â†’ Exit fires. The bridge
exposes it as `menu`, and the server refuses to deliver a load to anyone whose last heartbeat was not
`page=Frontend`. Both refusals are logged, so a skipped client is visible rather than a silent no-op.

## Ownership of the shared career

The gate only synchronises *when* the world advances. It does not decide *whose* world.

Chosen model (the one the user described): the host's career is the world.

1. Host plays, saves the career.
2. `UPDATE Player SET TeamID = <client team>` so the client opens the same world as their own team.
3. Host ships the save to the client.
4. Both press Continue together via the gate, and from then on both advance together via the gate.

The live-DB route (`coop-sql`, `:memory:` handle, `sqlite3_exec` at RVA `0x156D300`) was proven for
reads, single-row updates and transactions, but bulk transplant fails deterministically at
`Player_Record` (59 columns) and is not needed for this model. It stays available for
single-value edits like the team assignment.

## What is still unproven

- The gate wrapper itself. The bridge can fire events; wrapping `engine.trigger` and passing through
  has not been run against the live game yet.
- Whether a gate-swallowed Continue leaves the UI in a state the player can recover from (the button
  may show a fade-to-black animation before the event). `LoadSaveScreen` sets `fadeToBlack: true`
  *after* sending the event, so a swallowed event may need the fade suppressed too.
- `QuitGame` as a scripted return-to-front-end. The bridge exposes it as `menu`, but it has not been
  fired against a live game. The whole "everyone back to the menu, then load" flow depends on it.
- The player-facing ready UI. A toast is the minimum; a real dialog is better.
- Race weekend *session* sync (practice/qualifying/race results between two teams). The gate gets both
  clients into the same weekend; it does not merge their sessions afterwards. That is the next design
  problem, and it is a data problem (who owns the result of a session both players ran), not a UI
  problem.

## Proven, as of 2026-10-26 18:54

| claim | evidence |
|---|---|
| two machines can load the same career on command | host + `<client-pc>` both `page=RaceWeekend stage=ingame` after one `Load save2` |
| the save arrives intact | `save2.sav` 6,055,752 B, SHA-256 identical both sides, `GVAS` magic |
| one pak works on both | bridge probes `127.0.0.1` then `192.168.1.101` and keeps the first that answers |
| a load from the front-end is safe | both clients entered RaceWeekend, both responding after |
| a load from inside a career is NOT safe | host black-screened, front-end never rebuilt, process alive |
| the guard prevents it | client delivered, host `skip ... page=RaceWeekend`, host unaffected |
