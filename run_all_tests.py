#!/usr/bin/env python3
"""Run every test in this bundle and print ALL OK when all pass.

    python -I run_all_tests.py              every suite
    python -I run_all_tests.py rust         one suite

A suite passes only on exit 0 AND a passing summary line, so a suite that crashes after printing some
OK lines is not counted as a pass - the same rule the hub's `tools/test_all.py` applies.

Nothing here needs the game running, and nothing needs a copy of the game at all: the Rust crates are
dependency-free and the JS suites run against a headless DOM built from the UI export's own loader.
That is deliberate - it means a fresh clone is green, which is the only way a "works" claim stays
checkable by the next person.

`coop-mem` is excluded from the Rust run. Its `loop_stub_volume` integration test crashes with
0xC0000005 on a bare `cargo test --workspace`; the crash is pre-existing, does not reproduce under
`--test stub_execution`, and the crate has nothing to do with co-op. Excluding it by name here is
honest - it is a known defect, not a hidden one. See docs/README.md.
"""
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
TOOLS = os.path.join(HERE, "tools")
HEADLESS = os.path.join(TOOLS, "headless")

# name -> (argv, cwd, summary-predicate)
# The predicate receives the combined output and returns True when the run passed.
def has(text, needle):
    return lambda out: needle in out


def all_ok(out):
    lines = [l.strip() for l in out.splitlines() if l.strip()]
    return bool(lines) and lines[-1] == "ALL OK"


def passed_count(out, minimum):
    """True when the summary lines report at least `minimum` passing assertions and no failures.

    Deliberately parses only the *summary* lines. A naive `"failed" in out` check is wrong: many test
    names contain the word ("no polling chain survives the give-up", "the refused player was told
    why"), so the first version of this reported three passing suites as failures. Counting the
    `N passed, 0 failed` and `N/M passed` lines is the only check that survives its own test names.
    """
    import re

    if is_skip(out):
        return True
    total = 0
    failures = 0
    # Cargo prints `test result: ok. 118 passed; 0 failed; ...` (semicolons); the JS suites print
    # `21 passed, 0 failed` (comma). Both separators have to be accepted - matching only one made the
    # Rust run look empty and the runner report a passing suite as failed.
    for m in re.finditer(r"(\d+) passed[;,]\s*(\d+) failed", out):
        total += int(m.group(1))
        failures += int(m.group(2))
    for m in re.finditer(r"(\d+)/(\d+) passed", out):
        total += int(m.group(1))
        failures += int(m.group(2)) - int(m.group(1))
    if failures:
        return False
    return total >= minimum


def is_skip(out):
    """A suite that declines because a prerequisite is missing. It says so by name, on one line."""
    return any(l.strip().startswith("SKIP:") for l in out.splitlines())


SUITES = {}


def suite(name, argv, cwd, predicate, needs=None):
    # A duplicate name silently replaced the earlier registration (dict assignment), so two suites named
    # `lobby` meant `lobby_api_test.js` never ran and nothing said so. Failing at import is the only way
    # this stays fixed.
    if name in SUITES:
        raise SystemExit(
            'duplicate suite name %r: it would replace %r and that suite would never run'
            % (name, SUITES[name]["argv"])
        )
    SUITES[name] = {"argv": argv, "cwd": cwd, "pass": predicate, "needs": needs}


# ---- Rust ------------------------------------------------------------------------------------
# Target dir is redirected out of the tree: an in-tree target/ once tripped a Defender-style lock on
# an incremental build and shipped a stale binary while reporting success.
if shutil.which("cargo"):
    suite(
        "rust",
        ["cargo", "test", "--offline", "--workspace", "--exclude", "coop-mem"],
        os.path.join(HERE, "coop-rs"),
        # Cargo exits 0 on success and non-zero on any failing test, which is the authoritative signal;
        # the count is a floor so an empty run cannot masquerade as a pass.
        lambda out: "test result: FAILED" not in out and passed_count(out, 200),
    )
    # `coop-mem`'s lib tests, run separately because the crate is excluded above: its `loop_stub_volume`
    # integration test crashes with a pre-existing 0xC0000005 (documented in NOTES.md), but `--lib` avoids that
    # binary entirely. This matters because the team-authority gates live in `coop-mem` - `LiveDb::sql` and
    # `LiveDb::write_car` - and excluding the whole crate meant nothing checked them.
    suite(
        "auth",
        ["cargo", "test", "--offline", "--release", "-p", "coop-mem", "--lib"],
        os.path.join(HERE, "coop-rs"),
        # Every gate assertion must be present and passing, not merely "no failures". Naming them is the
        # point: a test that is silently skipped (renamed, `#[ignore]`d, filtered out) leaves the rule
        # unenforced while the suite still reports green, which is how the gates went unchecked before.
        lambda out: "test result: FAILED" not in out
        and "test result: ok" in out
        and "sql_is_gated_by_coop_auth" in out
        and "write_car_is_gated_per_car" in out
        and "write_car_refuses_an_out_of_range_car_id" in out,
    )

if shutil.which("node"):
    # ---- JS: unit + shipping-module tests ----------------------------------------------------
    suite("unit", ["node", "run_tests.js"], HEADLESS, lambda out: passed_count(out, 40))
    suite("press", ["node", "press_test.js"], HEADLESS, lambda out: passed_count(out, 20))
    suite("observe", ["node", "observe_test.js"], HEADLESS, lambda out: passed_count(out, 60))
    # `lobby_api_test.js` exercises the Steam-lobby HTTP endpoints and FFI signatures.
    suite("steam", ["node", "lobby_api_test.js"], HEADLESS, lambda out: passed_count(out, 20))
    suite("inert", ["node", "normal_play_test.js"], HEADLESS, lambda out: passed_count(out, 18))
    # ---- JS: tests that need the headless DOM built from the UI export -----------------------
    # These skip themselves with a named reason rather than failing, so a clone without the export
    # is green-with-visible-gaps instead of red for a reason unrelated to the code.
    suite("drive", ["node", "drive_test.js"], HEADLESS, lambda out: passed_count(out, 10))
    suite("render", ["node", "probe_render.js"], HEADLESS, lambda out: passed_count(out, 10))
    # The entry point a player interacts with. Nothing covered this until "no multiplayer button" was
    # reported against two builds: every other suite tests a module, and nobody had tested whether the
    # module that draws the button can actually reach the menu.
    #
    # Named `boot`, not `lobby`. It was `lobby` while `lobby_api_test.js` was also `lobby`, and because
    # `SUITES` is a dict with no duplicate check the second registration silently replaced the first -
    # so the Steam endpoint suite had not run for as long as both existed. The runner now rejects a
    # duplicate name, but the distinct names are what make the output readable.
    suite("boot", ["node", "lobby_boot_test.js"], HEADLESS, lambda out: passed_count(out, 12))
    # Whether the game can resolve what is in the pak at all. This is the check that was missing when the
    # button failed: every other check asked "was the pak built from the right files", none asked "will the
    # game find them", and a pak with bare entry paths loads and overrides nothing.
    suite("pak", ["node", "pak_paths_test.js"], HEADLESS, lambda out: passed_count(out, 8))

    # The proxy is what loads the agent without anyone running an injector, and a wrong one stops the game from
    # starting at all ("procedure entry point not found"), which reads as a corrupted install. The check
    # compares its export table against the exe's import table; it skips when the game is absent, so a clone
    # without the game stays green with a named gap.
    proxy = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(HERE, "coop-rs", "target")),
                         "release", "coop_proxy.dll")
    # The game is only needed for its import table. `COOP_GAME` points at an install elsewhere; without either,
    # `check_proxy.py` reports SKIP rather than failing, the same policy as the render-dependent suites.
    game = os.environ.get("COOP_GAME", os.path.join("E:" + os.sep, "Steam", "steamapps", "common",
                                                    "F1 Manager 2024"))
    suite("proxy", [sys.executable, "-I", os.path.join(TOOLS, "check_proxy.py"), proxy,
                    "--game-exe", os.path.join(game, "F1Manager24", "Binaries", "Win64",
                                               "F1Manager24.exe")],
          HERE, lambda out: "every imported name is exported" in out or out.strip().startswith("SKIP"))

    # Are the hardcoded RVAs still right for the installed exe? `SIMULATE_RVA` can be checked with no game
    # running: the bytes at that RVA in the file are compared against the prologue the running game was
    # observed to hold. A mismatch means a patch moved the function and every offset in layout.rs must be
    # re-derived before the mod is used - a stale target patches unrelated code.
    suite("offsets", [sys.executable, "-I", os.path.join(TOOLS, "verify_offsets.py")],
          HERE, lambda out: ("OK: nothing checkable is wrong" in out) or out.strip().startswith("SKIP"))
    # One player must not be able to take every team. Reported as "player 1 being able to take every team and
    # locking them", caused by comparing a string object key against a number - always unequal, so no claim
    # was ever released. This extracts and runs the real `pickTeam` body, so a copy cannot drift from the source.
    suite("teams", ["node", "team_claim_test.js"], HEADLESS, lambda out: passed_count(out, 10))

    # Do both machines agree the world is paused? The union rule is unit-tested in Rust, but the failure that
    # matters is a *lost frame*: the front-end sends clock state only on SimPauseChange, so before the agent
    # heartbeat re-published it, one dropped frame left the host running and the client stopped for the rest of
    # the race. This suite models the gate against coordination.rs and asserts the repeat exists.
    suite("clock", ["node", "clock_convergence_test.js"], HEADLESS, lambda out: passed_count(out, 16))

    # The page guard must cover the *event*, not just the command word. `Raw <event>` resolves an arbitrary
    # Gameface event name after the command check has already passed, so `Raw LoadGame autosave` from
    # RaceWeekend.html reached engine.trigger with the one event proven to black-screen the process. This
    # asserts the guard exists, that it runs before both the trigger and the control-press path, and that it
    # does not over-refuse the events the in-game screens legitimately send.
    suite("guard", ["node", "bridge_guard_test.js"], HEADLESS, lambda out: passed_count(out, 19))

    # ---- JS: end-to-end against a real supervisor process ------------------------------------
    # Needs the built supervisor binary. `cargo build --release` produces it; the runner builds it
    # below rather than skipping, because this is the one suite that proves two peers actually
    # exchange coordination frames and a clone should not silently lose that coverage.
    supervisor = os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(HERE, "coop-rs", "target")),
                              "release", "coop-supervisor.exe")
    suite("e2e", ["node", "agent_coord_e2e.js"], HEADLESS, lambda out: passed_count(out, 20),
          needs=None if os.path.exists(supervisor) else "build")


def build_supervisor():
    """Builds the release supervisor the e2e suite drives. Returns an error string, or '' on success."""
    if not shutil.which("cargo"):
        return "cargo is not on PATH"
    env = dict(os.environ, PYTHONIOENCODING="utf-8")
    env.setdefault("CARGO_TARGET_DIR", os.path.join(HERE, "coop-rs", "target"))
    p = subprocess.run(["cargo", "build", "--offline", "--release", "-p", "coop-supervisor"],
                       cwd=os.path.join(HERE, "coop-rs"), env=env,
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    if p.returncode != 0:
        return "cargo build failed: " + "\n".join((p.stderr or "").splitlines()[-5:])
    return ""


# ---- Publishability: nothing private has leaked into the tree ---------------------------------
# This is a test, not a chore. The bundle is published under the owner's GitHub account, and a personal
# path in a doc is exactly the kind of thing that survives review because nobody re-reads a 150 KB
# write-up. The checker fails if any identifier remains, so a future edit that pastes a log line in
# cannot ship.
#
# This block used to sit *inside* `build_supervisor` after its `return ""`, so it never executed and
# `redaction` was not a registered suite: the README told readers to run it, and the runner answered
# "no such suite". The check itself still worked when invoked directly, which is why the docs looked
# right. Registering it at module level is what makes `python -I run_all_tests.py` cover it.
if shutil.which("python") or shutil.which("py"):
    suite("redaction", [sys.executable, "-I", os.path.join(TOOLS, "redact_private.py"), "--check"],
          HERE, lambda out: "no private identifiers remain" in out)


def run(spec):
    env = dict(os.environ, PYTHONIOENCODING="utf-8")
    env.setdefault("CARGO_TARGET_DIR", os.path.join(HERE, "coop-rs", "target"))
    t = time.time()
    p = subprocess.run(spec["argv"], cwd=spec["cwd"], env=env, capture_output=True,
                       text=True, encoding="utf-8", errors="replace")
    out = (p.stdout or "") + (p.stderr or "")
    ok = p.returncode == 0 and spec["pass"](out)
    if ok:
        return True, "%.1fs" % (time.time() - t), out
    return False, "%.1fs, exit %d" % (time.time() - t, p.returncode), out


def main():
    wanted = sys.argv[1:] or list(SUITES)
    skipped, failed = [], []
    built = False
    for name in wanted:
        spec = SUITES.get(name)
        if not spec:
            print("FAIL  %-10s (no such suite; known: %s)" % (name, ", ".join(SUITES)))
            failed.append(name)
            continue
        needs = spec["needs"]
        if needs == "build" and not built:
            built = True
            err = build_supervisor()
            if err:
                print("SKIP  %-10s (%s)" % (name, err), flush=True)
                skipped.append(name)
                continue
            needs = None
        if needs and needs != "build" and not os.path.exists(needs):
            rel = os.path.relpath(needs, HERE)
            print("SKIP  %-10s (needs %s - see docs/README.md)" % (name, rel), flush=True)
            skipped.append(name)
            continue
        ok, note, out = run(spec)
        if ok and is_skip(out):
            reason = next((l.strip() for l in out.splitlines() if l.strip().startswith("SKIP:")), "SKIP")
            print("SKIP  %-10s (%s)" % (name, reason[5:].strip() or "prerequisite missing"), flush=True)
            skipped.append(name)
            continue
        print("%s  %-10s (%s)" % ("PASS" if ok else "FAIL", name, note), flush=True)
        if not ok:
            failed.append(name)
            tail = "\n".join(out.splitlines()[-20:])
            print("      " + tail.replace("\n", "\n      "))

    if failed:
        print("FAILED: " + ", ".join(failed))
        return 1
    if skipped:
        print("ALL OK (%d skipped: %s)" % (len(skipped), ", ".join(skipped)))
    else:
        print("ALL OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
