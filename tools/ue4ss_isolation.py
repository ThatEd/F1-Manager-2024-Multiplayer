#!/usr/bin/env python3
"""Is the game's crash caused by our DLL, by UE4SS, or by the two together?

Why this exists
---------------
Three launch failures were investigated as `0xc0000409` aborts, and the dumps showed an unhandled C++ exception
thrown by **UE4SS**, with no frame of ours anywhere in the faulting stack. But the crash only appeared while our
`version.dll` was installed, so "not our code" is not the same as "not our problem": loading a DLL during process
attach changes module load order for everything in the process, which is enough to move which UE4SS path faults.

The question that settles it is cheap and nobody has run it:

    does the game start with our DLLs present but UE4SS disabled?

That distinguishes two very different situations, and the fix differs completely between them:

  * starts with UE4SS off  -> our DLL and UE4SS are incompatible when both are loaded
  * still crashes          -> UE4SS is broken in that install independently of us

This tool does not edit game files. It renames UE4SS's two files aside and back, so the operation is reversible
with a single command, and it refuses to run while the game is open.

Usage
-----
    python tools/ue4ss_isolation.py status      # what is installed, what is enabled
    python tools/ue4ss_isolation.py off         # disable UE4SS (renames, does not delete)
    python tools/ue4ss_isolation.py on          # re-enable UE4SS
"""

import argparse
import os
import sys

GAME = os.environ.get("COOP_GAME", r"E:\Steam\steamapps\common\F1 Manager 2024")
BIN = os.path.join(GAME, "F1Manager24", "Binaries", "Win64")

# The pair that makes UE4SS load: `dwmapi.dll` is the proxy Windows resolves first, and it loads UE4SS.dll.
# Renaming either one stops UE4SS; both are renamed so `status` cannot be fooled by a half-state.
UE4SS_FILES = ["dwmapi.dll", "UE4SS.dll"]


def game_running():
    """True when F1Manager24.exe is running, via tasklist (no psutil dependency)."""
    import subprocess
    try:
        out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq F1Manager24.exe"],
                             capture_output=True, text=True, timeout=30).stdout or ""
    except (OSError, subprocess.SubprocessError):
        return False
    return "F1Manager24.exe" in out


def coop_installed():
    try:
        return [n for n in os.listdir(BIN) if n.lower().startswith("coop") and n.lower().endswith(".dll")]
    except OSError:
        return []


def state():
    rows = []
    for name in UE4SS_FILES:
        on = os.path.exists(os.path.join(BIN, name))
        off = os.path.exists(os.path.join(BIN, name + ".off"))
        rows.append((name, on, off))
    return rows


def cmd_status():
    if not os.path.isdir(BIN):
        print("game binaries not found: %s" % BIN)
        print("set COOP_GAME if the game lives somewhere else")
        return 2
    print("install: %s" % BIN)
    print()
    print("UE4SS:")
    for name, on, off in state():
        mark = "enabled" if on else ("disabled (as %s.off)" % name if off else "ABSENT")
        print("  %-14s %s" % (name, mark))
    print()
    coop = coop_installed()
    print("co-op DLLs: %s" % (", ".join(coop) if coop else "none"))
    print("version.dll: %s" % ("present" if os.path.exists(os.path.join(BIN, "version.dll")) else "absent"))
    print()
    print("To run the isolation test: install the co-op DLLs, then `off`, launch once, then `on`.")
    return 0


def cmd_toggle(disable):
    if game_running():
        print("the game is running - close it first (files are locked and a launch would be part-load)")
        return 1
    changed = []
    errors = []
    for name in UE4SS_FILES:
        live = os.path.join(BIN, name)
        parked = live + ".off"
        try:
            if disable:
                if os.path.exists(live):
                    os.replace(live, parked)
                    changed.append("%s -> %s.off" % (name, name))
                elif not os.path.exists(parked):
                    errors.append("%s is not present at all" % name)
            else:
                if os.path.exists(parked):
                    os.replace(parked, live)
                    changed.append("%s.off -> %s" % (name, name))
                elif not os.path.exists(live):
                    errors.append("%s and %s.off are both missing" % (name, name))
        except OSError as exc:
            errors.append("%s: %s" % (name, exc))
    for line in changed:
        print("  " + line)
    for line in errors:
        print("  ! " + line)
    if not changed and not errors:
        print("  nothing to do (already in the requested state)")
    print()
    if disable:
        print("UE4SS is off. Launch the game once.")
        print("  - if it starts and the co-op log shows the agent running, the conflict is UE4SS + our DLL")
        print("  - if it still crashes, UE4SS was broken here independently")
        print("Then: python tools/ue4ss_isolation.py on")
    else:
        print("UE4SS is on again. Nothing else was changed.")
    return 1 if errors else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["status", "off", "on"])
    args = ap.parse_args()
    if args.action == "status":
        return cmd_status()
    return cmd_toggle(args.action == "off")


if __name__ == "__main__":
    sys.exit(main())
