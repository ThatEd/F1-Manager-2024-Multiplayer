#!/usr/bin/env python3
"""Find engine-side Gameface events that the shipped UI never sends.

Why this exists
---------------
The open question was "what starts the race sim?" and the answer was "not the UI": every control on the
pre-session screen was pressed and the sim stayed at `simtime = 0`. The UI export was the wrong place to look,
but it was not useless - it tells us which event names the game's interface *does* use, so anything in the
engine's own event table that the UI never mentions is a name with no button behind it.

That set is small, and it is exactly what the co-op bridge's `Raw <event>` command can reach and a human
cannot. `StageProceed` and `TravelToRaceWeekend` came out of it.

How
---
1. Scan the exe for the known UI event names (ASCII). Their positions cluster into the engine's event-name
   strings.
2. Walk each cluster and pull out every printable name in it.
3. Count how many appear in the Gameface export.
4. Report the names with zero UI references.

A name in a cluster is evidence it is a real event (the neighbouring names are events the UI sends). Zero UI
references means no control emits it. It does NOT mean the name is triggerable via `engine.trigger`, and it
does not say what its handler does - both need the game.

Usage
-----
    python tools/ui_event_gap.py
    python tools/ui_event_gap.py --export "<UIGameface dir>" --game "<F1 Manager 2024>"
"""

import argparse
import os
import re
import sys

DEFAULT_GAME = os.environ.get("COOP_GAME", r"E:\Steam\steamapps\common\F1 Manager 2024")
DEFAULT_EXPORT = os.environ.get(
    "COOP_EXPORT", r"E:\FModel\Output\Exports\F1Manager24\Content\UIGameface")

# Names the UI is known to send; their positions anchor the clusters. The two engine-only candidates are
# included so the tool reports on them whether or not they fall inside an anchored cluster.
ANCHORS = [
    "PreSessionClosed",
    "SessionOverviewClosed",
    "AdvanceToNextStage",
    "PostRaceSummaryClosed",
    "AnalyticsClosed",
    "ConstructorStandingsClosed",
    "RaceViewChanged",
    "PostRace",
    "StageProceed",
    "TravelToRaceWeekend",
]

# A cluster is a run of PascalCase tokens with few bytes between them.
NAME_RE = re.compile(rb"[A-Z][A-Za-z0-9_]{4,40}")

# Names that are not events: UE enum/type identifiers (`EDialogueOnboardingEventType`, `EPauseLayer`) follow a
# distinct convention, and a truncated token is an artefact of the byte window, not a name.
NOT_AN_EVENT = re.compile(r"^E[A-Z][a-z]")          # UE enum/type prefix
NOISE = {"UpdateCa", "UpdateC", "UpdateCal"}        # truncated by the window edge


def read_js_hits(export_dir):
    """Map event name -> number of JS files in the UI export that mention it."""
    hits = {}
    if not os.path.isdir(export_dir):
        return hits
    js_root = os.path.join(export_dir, "js")
    blobs = []
    for root, _dirs, files in os.walk(js_root if os.path.isdir(js_root) else export_dir):
        for f in files:
            if f.endswith(".js"):
                try:
                    blobs.append(open(os.path.join(root, f), "rb").read())
                except OSError:
                    pass
    blobs.append(b"")  # keep the loop below simple if the export is missing
    for name in set(m.group(0).decode() for b in blobs for m in NAME_RE.finditer(b)):
        hits[name] = 0
    for name in list(hits):
        needle = name.encode()
        hits[name] = sum(1 for b in blobs if needle in b)
    return hits


def clusters(data, window=0x18, span=0x120):
    """Byte spans around each anchor, merged."""
    spans = []
    for a in ANCHORS:
        i = data.find(a.encode("ascii"))
        if i >= 0:
            spans.append((max(0, i - window), i + span))
    spans.sort()
    merged = []
    for lo, hi in spans:
        if merged and lo <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], hi))
        else:
            merged.append((lo, hi))
    return merged


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--game", default=DEFAULT_GAME)
    ap.add_argument("--export", default=DEFAULT_EXPORT)
    args = ap.parse_args()

    exe = os.path.join(args.game, "F1Manager24", "Binaries", "Win64", "F1Manager24.exe")
    if not os.path.exists(exe):
        print("SKIP: exe not present at %s" % exe)
        return 0

    data = open(exe, "rb").read()
    print("exe    : %s (%d bytes)" % (exe, len(data)))
    print("export : %s%s" % (args.export, "" if os.path.isdir(args.export) else "  (MISSING)"))
    print()

    spans = clusters(data)
    if not spans:
        print("No anchor found - the event table has moved. Update ANCHORS.")
        return 1

    names = set()
    for lo, hi in spans:
        blk = data[lo:hi]
        for m in NAME_RE.finditer(blk):
            tok = m.group(0).decode()
            # Skip the ordinary words that live in the same area as the dots in the names.
            if len(tok) >= 8 and sum(c.isupper() for c in tok) >= 2:
                names.add(tok)

    # Also include names named explicitly in ANCHORS that no cluster covered - `TravelToRaceWeekend` lives in a
    # different part of the image, and it is one of the two names this whole search is about.
    for a in ANCHORS:
        if a.encode() in data:
            names.add(a)

    names = {n for n in names if not NOT_AN_EVENT.match(n) and n not in NOISE}

    hits = read_js_hits(args.export)
    export_ok = os.path.isdir(args.export)

    print("%d candidate event names across %d cluster(s)" % (len(names), len(spans)))
    if not export_ok:
        print("(no export: cannot tell which the UI uses - listing all)")
        for n in sorted(names):
            print("  %s" % n)
        return 0

    unused, used = [], []
    for n in sorted(names):
        (used if hits.get(n, 0) else unused).append(n)

    print("\nSENT BY THE UI (%d) - a button or handler emits these:" % len(used))
    for n in used:
        print("  %-28s %d file(s)" % (n, hits.get(n, 0)))

    print("\nIN THE ENGINE, NEVER SENT BY THE UI (%d) - no control emits these:" % len(unused))
    for n in unused:
        print("  %s" % n)

    print("""
A name here is reachable only through the bridge's `Raw <event>` command, because the shipped interface has
no control that emits it. That is where a hidden session-flow lever would be - e.g. `StageProceed` starts a
race weekend's stage rather than the UI doing it. It is *not* proof the name can be triggered, nor what its
handler does; both need the game running.""")
    return 0


if __name__ == "__main__":
    sys.exit(main())
