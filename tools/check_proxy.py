#!/usr/bin/env python3
"""Check that a proxy DLL exports exactly what the game imports from the DLL it replaces.

Why this exists
---------------
A proxy DLL sits in the game's directory using the *name* of a system DLL, so Windows loads it first and the
game's import table is resolved against it. If it is missing even one name the game imports, the process does
not start: Windows shows "procedure entry point <name> could not be located" and the game never runs. That
reads as a corrupted install, not as a mod bug, and it is the one mistake a proxy cannot recover from.

So this compares two lists that must match, and refuses to install anything if they do not:

    python tools/check_proxy.py build/version.dll --game-exe <path to F1Manager24.exe>

Run it before deploying. `deploy_pak.py` calls it as part of the deploy.
"""

import argparse
import os
import sys

try:
    import pefile
except ImportError:  # pragma: no cover - the tool is only useful with pefile
    sys.exit("pefile is required: pip install pefile")

# The system DLL the proxy replaces. Kept here so the tool documents the choice it is checking.
PROXY_NAME = "version.dll"


def imports_of(exe_path, dll_name):
    """Names the given exe imports from `dll_name`, or None if it does not import it at all."""
    pe = pefile.PE(exe_path, fast_load=True)
    pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]])
    for entry in getattr(pe, "DIRECTORY_ENTRY_IMPORT", []) or []:
        name = (entry.dll or b"").decode(errors="replace").lower()
        if name == dll_name.lower():
            return [
                (imp.name.decode(errors="replace") if imp.name else "ordinal:%d" % imp.ordinal)
                for imp in entry.imports
            ]
    return None


def exports_of(dll_path):
    pe = pefile.PE(dll_path, fast_load=True)
    pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]])
    exp = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
    if exp is None:
        return []
    return [sym.name.decode(errors="replace") if sym.name else "ordinal:%d" % sym.ordinal
            for sym in exp.symbols]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("proxy", help="the built proxy DLL (e.g. version.dll)")
    ap.add_argument("--game-exe", required=True, help="F1Manager24.exe")
    args = ap.parse_args()

    if not os.path.exists(args.proxy):
        print("FAIL  proxy not built: %s" % args.proxy)
        return 1
    if not os.path.exists(args.game_exe):
        print("SKIP  game exe not present: %s" % args.game_exe)
        return 0

    want = imports_of(args.game_exe, PROXY_NAME)
    if want is None:
        print("FAIL  the game does not import %s at all - the proxy mechanism would never be used"
              % PROXY_NAME)
        return 1

    # Ordinal-only imports cannot be forwarded by name, so they must be absent or the scheme needs rethinking.
    ordinals = [w for w in want if w.startswith("ordinal:")]
    got = exports_of(args.proxy)

    print("the game imports %d name(s) from %s: %s" % (len(want), PROXY_NAME, ", ".join(want)))
    print("the proxy exports %d name(s)" % len(got))
    if ordinals:
        print()
        print("FAIL  %d ordinal import(s) from %s - a name-based forwarder cannot satisfy these:"
              % (len(ordinals), PROXY_NAME))
        for o in ordinals:
            print("        %s" % o)
        return 1

    missing = [w for w in want if w not in got]
    if missing:
        print()
        print("FAIL  the proxy does not export what the game imports:")
        for m in missing:
            print("        %s" % m)
        print()
        print("Deploying this would stop the game from starting with 'procedure entry point not found'.")
        return 1

    extra = [g for g in got if g not in want]
    print()
    print("ok    every imported name is exported")
    if extra:
        # Not an error: exporting more than the game needs is harmless and keeps this file usable by other
        # loaders. Worth one line so an unexpected symbol is visible.
        print("note  %d extra export(s): %s" % (len(extra), ", ".join(sorted(extra))))
    return 0


if __name__ == "__main__":
    sys.exit(main())
