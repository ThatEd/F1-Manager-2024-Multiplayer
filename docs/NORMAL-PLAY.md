# Normal play must not regress

The same pak is installed on both machines. A player who never opens multiplayer has to get a vanilla
experience: same menus, same timing, no background cost. This note records what the mod does when co-op
is *not* in use, what was wrong, and how it is now measured.

## What was wrong

Two polls ran as unconditional `setInterval`s for the life of every UI document:

| loop | cadence | what it cost with no server |
|---|---|---|
| `lobby.js` live reload | every 3 s | 20 failed HTTP connects/minute, forever |
| `bridge.js` source reload | every 3 s | 20 failed HTTP connects/minute, forever |
| `bridge.js` host probe | every 2 s after a failed sweep | 30 failed attempts/minute, forever |

That is a mod doing constant network work on a machine with nothing to talk to. It does not crash
anything, but it is not invisible, and "invisible when unused" was the requirement.

## What changed

All three loops now back off and then stop:

- **live reload / source reload** — 3 s while the server answers, then 6 s, 12 s, 24 s, then give up
  after 4 consecutive misses.
- **host probe** — a full sweep of the candidate addresses, repeated up to `PROBE_GIVE_UP` (6) times,
  then stop.

Stopping is not the feature going dead. Two things re-arm it:

- `window.__coopRearm()` on the bridge resets the counters and polls again.
- The co-op screen calls it on mount, so **opening the multiplayer menu always re-establishes the
  connection**, however long the single-player session ran first.

### The inbox poll: the same regression, caught the second time

Adding the coordination inbox (`COORDINATION-SENDER.md`) reintroduced this exact bug — a repeating
`setTimeout` that ran for the life of the document whether or not anything was listening. So did the clock
poll, added after it. Both now have their own give-up counter (`INBOX_GIVE_UP`, `CLOCK_GIVE_UP`) and
`__coopRearm` resets both.

They were caught because the assertion was strengthened, not because the old one failed — and it took three
attempts:

1. The original counted `setInterval` only.
2. `tail === 0` on requests could not see a chain that reschedules without ever reaching `open()`.
3. A bound on long-period timers worked for the inbox but not the clock, whose period is 400 ms — every
   pending timer looked short.
4. What finally discriminates is the **count of armed timers**: 1 with the guards in place (`watchScreens`,
   the game's own screen detection), 2 with either polling loop unguarded. The bound has to be exact; my
   first version allowed 2 and therefore passed in both cases, proving nothing. Measured both ways before
   choosing the number.

## Measured

`node tools/headless/normal_play_test.js` — 21 assertions, no game, no server:

```
bridge.js, no command server reachable
  ..    HTTP attempts in 10 virtual minutes: 17        (was 1,200)
  ..    last attempt at t=24000ms                       (was: never stops)
  ok    bridge threw nothing while offline
  ok    bridge stops polling once the host is clearly absent
  ok    bridge does not hammer the loopback while idle
  ok    bridge does not touch the DOM
  ok    bridge exposes a re-arm hook
  ok    re-arm makes the bridge probe again

lobby.js, no command server reachable
  ..    lobby.rev polls in 10 virtual minutes: 4       (was 200)
  ok    lobby stops live-reload polling once offline is established
  ok    lobby does not poll every 3 s forever
  ok    lobby adds at most one fallback element

bridge.js with a reachable server, idle
  ..    /hello requests in 60 virtual seconds: 242     (250 ms idle cadence, as designed)
  ok    healthy polling stays responsive
  ok    healthy idle polling is not run away

after co-op give-up
  ok    no setInterval survives into idle play
```

The test runs the **shipped files** in a fake browser with a virtual clock, so "ten minutes of idle
play" costs milliseconds and the attempt count is exact. A stub `XMLHttpRequest` records attempts
instead of making them, which is why it works with no server and no game.

## The other normal-play risk: the menu wrapper

`lobby.js` wraps `MainMenu.render` to inject the Multiplayer button, which means the game's own menu
render runs inside mod code. Two protections:

1. **A throw in the wrapper cannot break the menu.** The original render is called first and the
   result kept; anything the injection does afterwards is inside a `try` that returns the untouched
   tree on failure.
2. **Missing ingredients mean no button, not a broken vnode.** `h(undefined, props)` does not throw —
   preact happily builds a vnode with a null component — so a missing `MenuButton` would have put an
   unrenderable node into the real menu while the patch reported success. It now checks
   `ctx.MenuButton` and `ctx.nav` first and skips injection entirely.

Verified against the real rendered menu, not by inspection:

```
baseline: text 322 classes 156
lobby evaluated
after:    text 322 classes 156
menu intact: true
buttons still present: 6
```

That runs the shipped `lobby.js` inside the headless render host with no server reachable: the main
menu is byte-identical afterwards (same text length, same class set, same six buttons).

## What is still running during normal play

For completeness, what remains is small and bounded:

- **Menu button injected once per render.** A `MenuButton` vnode is prepended to `MainMenu_rightBox`;
  the tree is rebuilt, not mutated, and `EMPTY_CHILDREN` is never written to.
- **`screensLUT` entry added once.** One extra registry entry, read only when that screen is selected.
- **The probe, until it gives up.** ~17 requests over the first 24 s of a front-end document, then
  silence.
- **One `setTimeout` chain for `watchScreens`** at 500 ms, which is the bridge's own stage tracking and
  was already there before this change.

## If a player reports the menu looking wrong

Order of suspicion, based on what has actually broken before:

1. **Stale pak in the install.** This has wasted more time than every other cause combined. Run
   `python tools/deploy_pak.py --check` — it compares the installed pak byte-for-byte and refuses to
   claim success on a mismatch.
2. **`lobby.js` failed to load its modules** — a red fallback box appears with a `[coop]` log line.
   That is the designed failure mode: visible, not silent.
3. **A live reload applied while the game was mid-session.** Stop the command server; the reload stops
   on its own after 4 misses.
