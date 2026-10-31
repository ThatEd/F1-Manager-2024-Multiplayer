# LAN test: the ordered plan, and what each result means

Written for a two-machine session on 2026-10-29. It supersedes the ordering in
[RACE-SYNC-RUNBOOK.md](RACE-SYNC-RUNBOOK.md) where the two disagree, and it says which is which.

**Read this first: the runbook contradicts itself, and I did not resolve it.**

| §0 "What working means" | §4 "the hook fires" |
|---|---|
| "**Proven:** the hook fires (~25-30/s, game stays responsive); positions replicate continuously at ~19.7 Hz" | "`fires` climbing: the game thread is reaching our code. **This has never yet been observed.**" |

Both cannot be true. Everything below therefore treats replication as **unconfirmed** and puts the
hook check *before* any session work, because if `fires` is 0 nothing downstream means anything. If the
2026-10-27 session really did see `fires` climbing, the check below will confirm it in ten seconds and
cost nothing.

---

## 0. What is already done (verified just now)

Both machines are on the **same build**. Re-run the gate after any change; it exits non-zero unless the
whole chain is live.

| | host | client |
|---|---|---|
| `CoopBridge_P.pak` | `Content\Paks\~mods\` 123,816 bytes | same |
| `coop_agent.dll` | 428,544 bytes, `4e60b3b591825bd0` | same |
| `coop-link.exe` | 389,120 bytes | same |
| `coop_side.txt` | `host` | `client` |
| `coop_hook.enable` | present | present |

**The real bug was the paths inside the pak, not its folder.** Loose paks mount from anywhere under
`Content\Paks\`, including its root — the `~mods` convention is only what mod managers use. What was wrong is
that our entries were stored as `Frontend.html` instead of
`F1Manager24/Content/UIGameface/Frontend.html`. With mount point `../../../` (three up from
`Paks` = the game root), a bare entry resolves to `<game root>/Frontend.html`, which is nothing — so the pak
loaded, overrode nothing, and said nothing.

Everything about the pak looked correct: byte-identical on both machines, right version (`V11`), right mount
point, right compression, and it listed exactly the intended files. Comparing its entry list against a working
mod pak in the install (`F1M2021_TEST_PR_S_UI_P.pak`, which ships
`F1Manager24/Content/UIGameface/js/...`) is what exposed it.

`deploy_pak.py` now stages the tree so entries carry the full path, and `verify_build` **fails** if an entry is
missing the `F1Manager24/Content/UIGameface/` prefix. That check is the one that was missing: every existing
check verified the pak was built from the right files, and none verified the game could find them.

(An earlier note in this file claimed `~mods` was required and that the root was ignored. That was wrong and
has been removed — the folder never mattered.)

The two side files matter more than they look. The agent cannot see the launcher's environment, so it
reads its identity from `coop_side.txt`; with both machines on the default every pause report would be
attributed to the host and arbitration would silently pick the wrong owner rather than fail.

Gate: `python tools\preflight.py` — currently 7 failures, all of the form "nothing is running yet".

---

## 1. Start the supervisor **first**, and bind it for the LAN

```powershell
cd E:\TEMP\coop-target\release
.\coop-supervisor.exe --port 45990
```

Do **not** pass `--bind 127.0.0.1`. The default is `0.0.0.0`, which is what a second machine needs; the
agent and publisher both connect to loopback for their own reasons, so a loopback-only supervisor works
on this machine and silently refuses the client.

Expect `[supervisor] bound 0.0.0.0:45990` and a firewall rule named `coop-supervisor` (already exists).

Check: `python tools\preflight.py` → `PASS relay listening on 45990`.

## 2. Start the host publisher

```powershell
cd E:\TEMP\coop-target\release
.\watch-publisher.cmd
```

It waits for the game, guards against a duplicate watcher (two publishers double every frame at the
client, which corrupts frame order without crashing), and restarts `coop-link` if it dies.

Check: `PASS exactly one host publisher`, and `PASS publisher targets the live game`.

## 3. Launch the game on both machines - the agent loads itself

**No injection step.** The agent is loaded by a `version.dll` forwarder that the game imports, so it is inside the
process before the first frame with no tool to run. This section used to say "launch, then inject" and run
`tools/remote.py attach-client`; that step is exactly the manual ritual the DLL exists to remove, and following it
would reintroduce the failure it was built to prevent.

Host, this machine, **before** launching:

```powershell
python tools\preflight.py
```

That checks the pak is current, the forwarder is installed and its exports match the exe's imports, and - if a
client share is reachable - that the client has the same build. Then launch the game normally on both machines.

**Do not** hand-copy files or run an injector. If the agent is not loaded, `preflight` says which artefact is
missing; re-deploying the bundle is the fix, not attaching by hand.

The client is checked from the host once both games are up:

```powershell
python tools\remote.py ping        # is the client share reachable
python tools\remote.py ps          # lists processes there; look for F1Manager24 and coop_agent
python tools\remote.py logs        # pulls the client's logs back
```

Expect the client's `coop_agent.log` to show the agent loaded and its port listening. If `ps` shows
`F1Manager24` but no `coop_agent` module in its logs, the deploy is stale or incomplete - fix the deploy, do not
inject. Frames staged for an agent that is not running are indistinguishable from "no sync" and waste a session.

### If the game will not start

`version.dll` was chosen because the exe imports only three named functions from it, so a forwarder can satisfy
exactly those and add ours. `tools/check_proxy.py` refuses to install a proxy whose exports do not match the exe's
imports - the failure that once made the game refuse to launch at all.

The recovery is to remove the forwarder. It is **one file**, `version.dll` beside `F1Manager24.exe`, and the
game's own `version.dll` is not replaced (it is forwarded, not overwritten), so deleting ours simply removes the
mod:

```powershell
Remove-Item '<game>\F1Manager24\Binaries\Win64\version.dll'
```

Then launch again. If it starts, the co-op mod was the cause and the agent build needs fixing before anything else
is tested. If it still fails, the cause is not this mod - check UE4SS next
(`python tools\ue4ss_isolation.py status`), because a `0xC0000409` from an unhandled C++ exception inside
**UE4SS** was the proven cause of an earlier launch failure, with `coop_agent.dll` in none of the faulting stacks.

## 4. Confirm the hook fires **before any race**

## 4. Confirm the hook fires **before any race**

This is the step that decides whether the rest is worth doing.

```powershell
python tools\watch_fires.py <host-game-pid> 10
python tools\remote.py run "& '<python>' '<repo>\tools\watch_fires.py' <client-game-pid> 10"
```

| output | meaning | action |
|---|---|---|
| `could not find the hook stub` | agent not loaded | redeploy the bundle, then restart the game |
| `callback 0x0 <-- INERT` | installed, no callback | stale agent build; redeploy and restart the game |
| `block 0x…` then `fires 0` | installed, not entered | **expected outside a race** — go to step 5 |
| `fires` climbing | live | good — go to step 5 |

Outside a race `fires 0` is correct: the sim is a class default (all distances 0), so the patched
function has not been called. It does **not** mean the RVA is wrong.

## 5. Load a race weekend on both, then re-check

Same commands as step 4, during a live session.

- **`fires` climbing** → the game thread is reaching our code. Now the write path can be judged.
- **`fires` still 0 during a live race** → `0x2298080` is not the per-tick entry point. That is a
  finding, not a failure: record it and go looking for the real one rather than changing the write
  logic, which would be guessing.

## 6. Only now: read the three logs together

The client's own line is not evidence — `sent` is what it *asked for*.

```
[client] ... sent 400 to the agent, verified 22 of 22 (worst dist delta 0.12 m)
[agent]  ... mailbox applied=198 dropped=2 entries=17424
[agent]  ... hook fired x40 (total 812) callback=0x…
```

| client `sent` | agent `applied` | meaning |
|---|---|---|
| climbing | climbing | working end to end |
| climbing | 0, fires 0 | sim not running, or wrong RVA |
| climbing | 0, fires >0 | mailbox handshake stuck — a `hookmail` bug |
| climbing | climbing but cars still | write lands, renderer does not follow |
| flat | flat | client never reached the agent |

---

## The thing this session most needs to settle

Separate from replication, and untested by any automated suite:

**Does a race session start when `StageProceed` is fired?**

Session start is mapped from the exe but is not in the UI layer. The UI's `PreSessionClosed` only
dismisses the pre-session dialog. `StageProceed` (`0x5fa7c98`) sits in the same binding block and has no
sender anywhere in the export.

With the game on a genuine pre-session screen:

```powershell
python tools\watch_session.py <host-game-pid> 120
# then, through the mod's bridge:
#   Raw StageProceed
```

Success is **`simtime` leaving 0 and climbing**, with the hook's `fires=` counter moving with it. If it
stays 0, try `Raw TravelToRaceWeekend` from TeamManagement with a race as the next calendar event, then
`Raw PreSessionClosed` alone to confirm the baseline.

This is the highest-value result the session can produce: it either unblocks everything downstream or
rules out the strongest lead with evidence.

---

## If it crashes

`EXCEPTION_ACCESS_VIOLATION reading address 0xF3C` is a null base plus an offset — the game dereferenced
track data through a field we changed. Before changing anything:

```powershell
python tools\remote.py run "New-Item -ItemType File '<game bin>\coop_dump_now' -Force"
```

The agent writes a minidump and `coop_agent.trace` names the stage it died in. **A crash with `fires == 0`
means our code never ran and is not implicated** — do not "fix" the write path for it.

## Housekeeping that saves a cycle

- **The agent is loaded at process start, so a redeploy needs a game restart.** There is no in-process reload:
  the forwarder runs during DLL load, and swapping the file underneath a running game leaves the old module mapped
  at the same base with no new log - which reads as "the deploy did nothing". Restart the game after any change to
  the agent.
- **The share flaps.** Every remote file operation needs a retry; `remote.py` says when it cannot reach the
  agent.
- **Rollback is one command.** The previous paks and DLLs are in `E:\TEMP\lan-backup-20261029-1923`.
