#!/usr/bin/env python3
"""Count the Gameface UI's event surface, and check the numbers the docs claim.

Why this exists
---------------
The project's own README said "352 events across 517 call sites" and the career plan said "267 career verbs".
Measuring the export directly gives 103 distinct names across 133 call sites - matching what the original task
brief predicted. The documentation had drifted from the artefact, and nobody could tell, because the number was
copied from prose rather than measured.

A wrong count is not cosmetic here: `CAREER-COOP-PLAN.md` sizes the career-replication work off "267 career
verbs", and the README sizes the whole UI surface off 352. Both are cited as evidence.

What it counts
--------------
Every literal `sendEvent('Name'` call site in the decompiled UI. That is the only form that proves the UI
*itself* sends an event; events assembled from a variable (the confirm dialogs return their event name from
`getConfirmSelectionEvent`) are real but cannot be enumerated this way, which is why the count is a floor for
"events in use" and an exact count for "events written inline".

Usage
-----
    python tools/event_surface.py
    python tools/event_surface.py --export "<UIGameface dir>" --expect-names 103 --expect-sites 133
"""

import argparse
import os
import re
import sys

DEFAULT_EXPORT = os.environ.get(
    "COOP_EXPORT", r"E:\FModel\Output\Exports\F1Manager24\Content\UIGameface")

INLINE = re.compile(r"sendEvent\(\s*'([A-Za-z0-9_]+)'")
# The compiled TypeScript form: (0, Engine_1.sendEvent)('Name', ...).
#
# This form is a *superset* of the plain match - `sendEvent('` appears inside it - so counting both naively
# double-counts. The two numbers that matter are the union of names (how many distinct events the UI can send)
# and the total call sites for the TS form alone, which is what the shipped code overwhelmingly uses.
TS = re.compile(r"Engine_\d+\.sendEvent\)\(\s*'([A-Za-z0-9_]+)'")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--export", default=DEFAULT_EXPORT)
    ap.add_argument("--expect-names", type=int, default=346)
    ap.add_argument("--expect-sites", type=int, default=373)
    args = ap.parse_args()

    root = os.path.join(args.export, "js")
    if not os.path.isdir(root):
        print("SKIP: no export at %s" % args.export)
        return 0

    names, plain_sites, ts_sites = set(), 0, 0
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            if not f.endswith(".js"):
                continue
            try:
                s = open(os.path.join(dirpath, f), encoding="utf-8", errors="ignore").read()
            except OSError:
                continue
            for m in INLINE.finditer(s):
                names.add(m.group(1))
                plain_sites += 1
            for m in TS.finditer(s):
                names.add(m.group(1))
                ts_sites += 1

    print("export            : %s" % args.export)
    print("distinct names    : %d   (union of both call forms)" % len(names))
    print("inline-form sites : %d   sendEvent('X')" % plain_sites)
    print("TS-form sites     : %d   (0, Engine_1.sendEvent)('X')" % ts_sites)

    ok = True
    if len(names) != args.expect_names:
        print("MISMATCH: %d distinct names, documentation says %d" % (len(names), args.expect_names))
        ok = False
    if ts_sites != args.expect_sites:
        print("MISMATCH: %d TS-form call sites, documentation says %d" % (ts_sites, args.expect_sites))
        ok = False

    if ok:
        print("\nOK: measured surface matches the documented one (%d names, %d sites)."
              % (args.expect_names, args.expect_sites))
        print("    A change here is not necessarily a bug - a game patch can add events - but the README and")
        print("    CAREER-COOP-PLAN.md quote these numbers as evidence, so update them with a measurement")
        print("    rather than a copy of prose.")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
