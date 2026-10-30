#!/usr/bin/env python3
"""Read the verdict on the last game launch: did the agent load, and did the game survive?

Why this exists
---------------
"the game did not start" has several possible causes that look identical from outside, and a wrong guess
wastes a launch. This reads the two logs the proxy and the agent write, plus the Windows crash record, and
says which case it was - so the answer does not depend on someone interpreting four files by hand.

It is read-only. It never launches anything and never changes state.

Usage
-----
    python tools/launch_verdict.py
    python tools/launch_verdict.py --game "<path to F1 Manager 2024>"   # another install or a test copy
"""

import argparse
import datetime
import os
import sys

DEFAULT_GAME = os.environ.get("COOP_GAME", r"E:\Steam\steamapps\common\F1 Manager 2024")
BIN = os.path.join("F1Manager24", "Binaries", "Win64")


def tail(path, n=40):
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            return [l.rstrip("\n") for l in f.readlines()[-n:]]
    except OSError:
        return []


def exists(path):
    try:
        return os.path.exists(path)
    except OSError:
        return False


def crash_events(since_minutes=120):
    """Windows Application-log entries naming the game, with their fault codes."""
    try:
        import subprocess
        ps = (
            "Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=(Get-Date).AddMinutes(-%d)} "
            "-ErrorAction SilentlyContinue | Where-Object { $_.Message -match 'F1Manager24' } | "
            "Select-Object -First 5 | ForEach-Object { "
            "\"$($_.TimeCreated)|$($_.Id)|$($_.Message -split \"`n\" | Select-String 'Exception code|Faulting module name|Fault offset' | ForEach-Object { $_.Line.Trim() } | Join-String -Separator ';')\" }"
            % since_minutes
        )
        out = subprocess.run(["powershell", "-NoProfile", "-Command", ps],
                             capture_output=True, text=True, timeout=90).stdout
        return [l for l in out.splitlines() if l.strip()]
    except Exception:
        return []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--game", default=DEFAULT_GAME)
    ap.add_argument("--minutes", type=int, default=120, help="how far back to look for a crash record")
    args = ap.parse_args()

    bin_dir = os.path.join(args.game, BIN)
    proxy_log = os.path.join(bin_dir, "coop_proxy.log")
    agent_log = os.path.join(bin_dir, "coop_agent.log")
    marker = os.path.join(bin_dir, "coop_proxy.enable")

    print("install: %s" % args.game)
    print("  version.dll       : %s" % ("present" if exists(os.path.join(bin_dir, "version.dll")) else "ABSENT"))
    print("  coop_agent.dll    : %s" % ("present" if exists(os.path.join(bin_dir, "coop_agent.dll")) else "absent"))
    print("  coop_proxy.enable : %s" % ("present (agent will load)" if exists(marker) else "absent (forwarder only)"))
    print()

    p = tail(proxy_log, 12)
    print("proxy log (last lines):")
    if not p:
        print("  (none - the proxy has never run in this install)")
    for line in p:
        print("  " + line)
    print()

    a = tail(agent_log, 25)
    # Only the agent's lines from the most recent start: the log is appended across launches, so the last
    # "agent started" marks the beginning of the session in question.
    starts = [i for i, l in enumerate(a) if "agent started in pid" in l]
    recent = a[starts[-1]:] if starts else []
    print("agent log (this launch):")
    if not recent:
        print("  (no 'agent started' line - the agent did not run)")
    for line in recent:
        print("  " + line)
    print()

    fired = any("hook fired" in l for l in recent)
    panicked = [l for l in recent if "PANIC" in l]
    imported = any("loaded coop_agent.dll" in l for l in p)
    forwarded_only = any("forwarding only" in l for l in p)

    crashes = crash_events(args.minutes)

    print("verdict:")
    version_dll = exists(os.path.join(bin_dir, "version.dll"))
    if panicked:
        print("  AGENT PANICKED - this is our bug, and the first line below names it:")
        for l in panicked:
            print("    " + l)
        print("  Disable it with coop-disable.cmd, and this log line is the thing to fix.")
    elif not p:
        # No log is not a diagnosis. The file is created on the proxy's first run, so its absence means
        # "has not run since the log was cleared", which is true of every install that has not been launched yet.
        print("  the proxy has not run in this install yet (no coop_proxy.log).")
        if version_dll:
            print("  version.dll is installed, so the next launch will exercise it.")
        else:
            print("  version.dll is also ABSENT - something removed it, so the agent cannot load at all.")
        print("  Launch the game, then run this again; the log and this verdict will say what happened.")
    elif forwarded_only:
        print("  CO-OP WAS NOT ENABLED for that launch (coop_proxy.enable was absent), so the game ran")
        print("  single-player. That is the safe state, not a fault.")
    elif not imported:
        print("  the proxy ran but did NOT load coop_agent.dll. See the proxy log above for the reason")
        print("  (usually the file is missing or failed to load).")
    elif fired:
        # Checked before the crash records on purpose: a live agent proves the process survived, so any crash
        # entry in the window belongs to an earlier attempt. Reporting "it crashed" for a working session was
        # a real false negative in an earlier version of this tool.
        print("  AGENT IS LIVE and the game reached a race session (the hook fired).")
        if crashes:
            print("  (Windows has crash records in the last %d min, but from earlier attempts - this" % args.minutes)
            print("   session is running.)")
    elif crashes:
        print("  THE GAME CRASHED with the agent loaded. Windows recorded:")
        for c in crashes:
            print("    " + c)
        print("  Disable it with coop-disable.cmd before launching again.")
    elif recent:
        print("  the agent loaded and started, but the hook has not fired yet - that happens outside a")
        print("  race session, so this is expected on the main menu. No crash recorded.")
    else:
        print("  the proxy loaded the agent but the agent produced no log. Unknown - check the game is not")
        print("  running from a different install than %s" % args.game)

    if crashes and not recent:
        print()
        print("note: Windows recorded crashes but this install's agent never ran. Those are from another")
        print("      install or before this one was deployed.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
