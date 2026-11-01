#!/usr/bin/env python3
"""End-to-end: the diff producer's SQL, decoder and policy, against the game's own database.

Why this exists
---------------
`tools/quote_probe.py` (the `diff` suite) proves the SQL `careerdiff` emits runs and round-trips, using its own
mirrored queries. `careerdiff_probe` (an example binary) proves the real Rust code reads the real database and
produces exactly one change for one edit. This suite runs the example binary, so the assertion the suite enforces
is the one that matters: **every change the pass produces is accepted by `replicate::plan`**.

That last hop is the one §29.4 found broken: a statement-produced change carried only the changed column, so
`plan` looked for the team among the values being set and refused everything as `NoTeam`. The diff producer fixes
it by carrying the row's own team, and this suite is what notices if that stops being true.

The database comes from `Volta.json` (`schema_truth.py --volta`), so a clone without the UI export skips rather
than fails.

Usage
-----
    python tools/diffwire_test.py
"""

import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT = os.path.dirname(HERE)
EXPORT = os.environ.get(
    "COOP_DB_EXPORT",
    os.path.join("E:\\", "FModel", "Output", "Exports", "F1Manager24", "Content", "Database"),
)
VOLTA = os.path.join(EXPORT, "Volta.json")
SCHEMA_TRUTH = os.path.join(HERE, "schema_truth.py")

results = []


def ok(cond, name, detail=""):
    results.append(bool(cond))
    print("  %s  %s%s" % ("ok  " if cond else "FAIL", name, "" if cond else "  -- " + str(detail)))


def main():
    if not os.path.exists(VOLTA):
        print("SKIP: no Volta.json at %s (set COOP_DB_EXPORT)" % VOLTA)
        return 0
    if not shutil.which("cargo"):
        print("SKIP: cargo is not on PATH")
        return 0

    workdir = tempfile.mkdtemp(prefix="diffwire-")
    db = os.path.join(workdir, "career.db")

    # 1. The game's own database, out of the asset.
    r = subprocess.run([sys.executable, "-I", SCHEMA_TRUTH, "--volta", db],
                       capture_output=True, text=True)
    ok(r.returncode == 0 and os.path.isfile(db), "Volta.json decoded to a SQLite file", r.stdout[-200:])

    # 2. The real Rust code, end to end.
    env = dict(os.environ, CARGO_TARGET_DIR=os.environ.get("CARGO_TARGET_DIR", "E:\\TEMP\\coop-target"))
    r = subprocess.run(
        ["cargo", "run", "--offline", "--release", "-p", "coop-mem", "--example", "careerdiff_probe", "--", db],
        capture_output=True, text=True, cwd=os.path.join(PROJECT, "coop-rs"), env=env,
    )
    out = r.stdout + r.stderr
    ok("read 16 tables, 70 rows" in out, "the real producer read all 16 replicable tables", out[-400:])
    ok("two identical passes report no change" in out, "an unchanged career produces no change", out[-400:])
    ok("1 change(s) ready" in out, "one edited column produced exactly one change", out[-400:])
    ok("every change the pass produced is accepted by replicate::plan" in out,
       "replicate::plan accepts every change the pass produced",
       "this is the §29.4 fix; a refusal here means attribution is broken again")
    ok(r.returncode == 0, "the probe exited 0", out[-400:])

    shutil.rmtree(workdir, ignore_errors=True)

    failed = results.count(False)
    print("\n  %d/%d passed" % (len(results) - failed, len(results)))
    if failed:
        print("  %d failed" % failed)
        return 1
    print("\nDIFFWIRE OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
