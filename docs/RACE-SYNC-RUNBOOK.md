# Live test runbook: race sync

Written after eight failed attempts that each cost a restart cycle, mostly because a *different* thing
was wrong each time and nothing distinguished them. Every step here has a check with a definite
pass/fail, and every check is one command.

The rule this document enforces: **never change anything blind again.** Before touching a design
decision, run the step that shows what is actually happening.

---

## 0. What "working" means

A sprint counts as synced only when all four are true at once:

1. the host log shows `wrote N` climbing for 22 cars, continuously;
2. the client log shows `sent N to the agent` with the same tick range;
3. the client's `coop_agent.log` shows `hook fired xN` and `mailbox applied=â€¦` climbing;
4. the client reads back `verified 22 of 22` and the cars visibly move on the client's screen.

Anything less is a partial result and must be reported as such.

### Where things actually stand (2026-10-27, after the first sprint)

**Proven:** the hook fires (~25-30/s, game stays responsive); positions replicate continuously at
~19.7 Hz; the user reports replays working and the stack holding up longer than the Lua version did.
That last part is the first real progress this project has had.

**Not yet proven:** race control in a live red flag. `Msg::Rc` is now sent by the host, forwarded by the
client and applied by the hook, with tests - but the share went down before it could be deployed for a
live test.

**Known missing, by design not accident:**

| missing | why | how to add |
|---|---|---|
| tyre condition, temperature | `hookmail::FIELDS` carries only `lat/head/dist/spd` | add to `FIELDS`; widths already supported |
| damage, faults, confidence | not a field at all - `RaceSimCarIncident` is reachable only via the event bus | needs `ERaceSimEventType`, see RACESIM-HIJACK Â§5.2 |

Symptoms map to causes cleanly, which is why they must not be debugged by guessing: "red flag not
synced / cars stuck" is missing `Msg::Rc`; "tyres not synced" is the field set; "damage not synced /
broken cars racing" is the event bus.

---

## 1. Build and deploy

```powershell
cd local\f1-manager-2024\coop-rs
$env:CARGO_TARGET_DIR='E:\TEMP\coop-target'      # the in-tree target is locked by Defender
cargo test --workspace --offline                 # expect all pass except loop_stub_volume (pre-existing)
cargo build --release --offline -p coop-agent -p coop-inject -p coop-link -p coop-mem --bins
```

```powershell
python tools\remote.py deploy-link      # pushes coop-link, coop_agent.dll, coop-inject to the client
python tools\remote.py versions         # MUST print MATCH for all three
```

`deploy-link` and `versions` read `%CARGO_TARGET_DIR%\release` now, not `coop-rs\target\release`. That
hardcoded path previously shipped a stale binary while reporting success, and the "fix that did not
work" was a 12:51 build tested against a 13:18 change.

---

## 2. Start the transport

Host (this machine):

```powershell
cd local\f1-manager-2024\coop-rs\target\release
.\watch-publisher.cmd
```

It restarts `coop-link --role host` whenever it exits, so a rebuild does not leave the session dead.

Client, after deploying:

```powershell
python tools\remote.py runfile remote-agent\attach-client.ps1
```

That script kills any old client, enables `coop_hook.enable`, injects `coop_agent.dll` if it is not
already resident, and starts `coop-link --role client`. It writes the outcome to `client-attach.txt`:

```
attached <client-pid> to game <game-pid> at <time>; agent present
```

**If that line says `AGENT NOT PRESENT`, stop.** Frames will be staged and never applied, which looks
identical to "no sync" and wastes the whole test.

---

## 3. Confirm the hook is live *before* starting a race

This is the step that did not exist before, and its absence is why failures were indistinguishable.

On the client:

```powershell
python tools\remote.py run "& 'C:\path\to\python.exe' <repo>\tools\watch_fires.py <game-pid> 10"
```

Or on the host, for the host's own game:

```powershell
python tools\watch_fires.py <game-pid> 10
```

Read the result:

| output | meaning | action |
|---|---|---|
| `could not find the hook stub` | hook not installed | check `coop_hook.enable` exists, restart the game, re-inject |
| `callback 0x0 <-- INERT` | hook installed, no callback | agent build is stale or install partly failed; restart the game and re-inject |
| `block 0xâ€¦` then `fires 0` | installed, not entered | **expected outside a race**; go to step 4 |
| `fires` climbing | live and running | good; go to step 4 |

Outside a race the sim is a class default (all distances 0, `rc_state 0`), so `fires 0` is correct and
does **not** mean a wrong RVA. Confirmed by reading the sim directly:

```
coop-probe --pid <game-pid> --rsm 0xâ€¦ --frames 3
frame 0: 22/22 cars, rc=Some(RcState { flags: 0, rc_state: 0, rf_state: 0 }), car0 dist=0.0
```

---

## 4. Start the session, then confirm it fires

Load the race weekend on both machines, then re-run `watch_fires.py`.

- `fires` climbing: the game thread is reaching our code. This has never yet been observed.
- `fires` still 0 during a live race: the RVA `0x2298080` is not the function being called each tick.
  That is a finding, not a failure - record it and go looking for the real per-tick entry point rather
  than changing the write logic, which would be guessing.

Only after `fires` climbs is it meaningful to look at whether cars move.

---

## 5. Read the three logs together

The client's line alone is not evidence:

```
[client] 400 snapshots, 22 cars, host clock 61.2, gap 25 ms, sent 400 to the agent,
         verified 22 of 22 (worst dist delta 0.12 m)
```

`sent` is what the client asked for. What actually happened is in the client's `coop_agent.log`:

```
[agent] write port: 200 frames in, mailbox applied=198 dropped=2 entries=17424
[agent] hook fired x40 (total 812) callback=0x7FF9E6F13E30 mailbox applied=198 ...
```

Interpretation:

| client `sent` | agent `applied` | meaning |
|---|---|---|
| climbing | climbing | working end to end |
| climbing | 0, hook fires 0 | the sim is not running, or the RVA is wrong (step 4) |
| climbing | 0, hook fires >0 | the mailbox handshake is stuck - a bug in `hookmail` |
| climbing | climbing but cars still | the write lands, the renderer does not follow (see RACESIM-HIJACK Â§3F) |
| flat | flat | the client never connected to the agent; check `client-attach.txt` |

---

## 6. If it crashes

The `0xF3C` fault (`EXCEPTION_ACCESS_VIOLATION reading address 0xF3C`) is a null base plus an offset:
the game dereferenced track data through a field we changed. Two causes are already known and fixed:

- `node` / `seg` were being written. They index the track graph, and the receiving world disagrees with
  them. `hookmail::FIELDS` excludes them, and a test asserts they are never written.
- writes came from `coop-link`'s own thread. They now happen in the hook callback, on the game thread.

If it still crashes, get evidence before changing anything:

```powershell
python tools\remote.py run "New-Item -ItemType File '<game bin>\coop_dump_now' -Force"
```

That makes the agent write a minidump (it suspends threads, which is why it is never automatic), and
`coop_agent.trace` will name the stage it died in. A crash with `fires == 0` means our code never ran
and is not implicated.

---

## 7. Housekeeping that saves a cycle

- **A surviving process is not a clean process.** Re-injecting a DLL by path into a process that
  already has that path loaded is a no-op: `LoadLibraryW` returns the existing handle and runs nothing.
  Symptom: the injector prints the *same* module base as last time and no new log appears. Restart the
  game to run a new agent build.
- **The share flaps.** Every remote file operation needs a retry; `remote.py` reports when it cannot
  reach `\\<client-pc>\e\coop-agent`.
- **The client game cannot be written by an unelevated process if the game is elevated.** The client
  now says so at startup instead of failing with a bare access-denied. It matters less since writes no
  longer come from that process, but the readback still needs it.
