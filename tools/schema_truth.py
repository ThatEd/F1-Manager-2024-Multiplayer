#!/usr/bin/env python3
"""Cross-check the hardcoded career schema against the game's own database.

Why this exists
---------------
`coop-mem/src/schema.rs` lists the tables a replicated career change may touch. That list is only worth anything if
it matches the database the game actually builds, and the previous list did not: it was derived from *file names*
in `Content/Database`, where `StaffPS.json` was read as a table called `StaffPS`.

`StaffPS.json` is a `PSCollection` - a named catalogue of prepared statements. Its first entry is

    {"Key": "AddStaffContract",
     "Value": "INSERT OR REPLACE INTO Staff_Contracts (...) VALUES (?1, ?2, ...)"}

so the table is `Staff_Contracts`. There is no `StaffPS` table; there is no `Finances`, `Calendar` or `Regulations`
table either. Every list in the project that said otherwise was wrong in the same way.

The truth
---------
`/Game/Database/Volta` is a `DatabaseAsset` whose `Data` property is a byte array beginning `SQLite format 3\\0`.
Writing it out gives a real SQLite file with `user_version` 336, 317 tables and 2 views. `sqlite_master` is then
authoritative for names, declared types, primary-key membership and seed rows, and needs no guessing.

What this checks
----------------
1. Every table `schema.rs` lists exists, has the `pk` column it claims, and really declares that column as its
   **only** primary-key column (a composite key cannot be carried by `Msg::Stmt`'s single key).
2. Every table it lists has the `team` column it claims.
3. Every table that meets the same criteria in the real schema and is written by the catalogue **is** listed -
   otherwise the list is silently narrower than it claims to be.
4. Round trip: the set derived here equals the set in the source file.
5. Every table name `coop-auth`'s `scope_of` classifies exists in the database.

Exits 0 with `SCHEMA OK`, or 1 listing what disagrees. Skips with a named reason when the export is absent, the
same policy as the other export-dependent checks.
"""
import json
import os
import re
import sqlite3
import sys
import tempfile

EXPORT = os.environ.get(
    "COOP_DB_EXPORT",
    os.path.join("E:\\", "FModel", "Output", "Exports", "F1Manager24", "Content", "Database"),
)
VOLTA = os.path.join(EXPORT, "Volta.json")
SCHEMA_RS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "coop-rs", "crates",
                         "coop-mem", "src", "schema.rs")
AUTH_RS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "coop-rs", "crates",
                       "coop-auth", "src", "lib.rs")

WRITE = re.compile(r"(?i)^\s*(INSERT|REPLACE|UPDATE|DELETE)\b")


def skip(why):
    print("SKIP: " + why)
    sys.exit(0)


def load_volta():
    """Decode the DatabaseAsset's byte array into a temporary SQLite file and return a connection."""
    with open(VOLTA, encoding="utf-8-sig") as f:
        asset = json.load(f)
    data = bytes(asset[0]["Properties"]["Data"])
    if not data.startswith(b"SQLite format 3\x00"):
        print("FAIL: Volta.Data is not a SQLite file (starts %r)" % data[:16])
        sys.exit(1)
    path = os.path.join(tempfile.mkdtemp(prefix="volta-"), "career.db")
    with open(path, "wb") as f:
        f.write(data)
    con = sqlite3.connect(path)
    con.row_factory = sqlite3.Row
    return con, len(data)


def real_schema(con):
    out = {}
    for row in con.execute("SELECT name FROM sqlite_master WHERE type='table'"):
        name = row["name"]
        cols = [dict(c) for c in con.execute('PRAGMA table_info("%s")' % name)]
        out[name] = {
            "columns": [c["name"] for c in cols],
            "pk": [c["name"] for c in cols if c["pk"]],
            "team": next((c["name"] for c in cols if c["name"].lower() in ("teamid", "team")), None),
        }
    return out


def written_tables():
    """Tables the statement catalogue writes to, and whether each statement names TeamID."""
    written, has_team = set(), set()
    for entry in sorted(os.listdir(EXPORT)):
        if not entry.endswith("PS.json"):
            continue
        with open(os.path.join(EXPORT, entry), encoding="utf-8-sig") as f:
            asset = json.load(f)
        for e in asset[0]["Properties"]["Entries"]:
            sql = e["Value"]
            if not WRITE.match(sql):
                continue
            m = re.search(r"(?i)\b(?:INSERT\s+(?:OR\s+\w+\s+)?INTO|REPLACE\s+INTO|UPDATE|DELETE\s+FROM)\s+"
                          r"([A-Za-z_][A-Za-z0-9_]*)", sql)
            if m:
                written.add(m.group(1))
                if re.search(r"(?i)\bTeamID\b", sql):
                    has_team.add(m.group(1))
    return written, has_team


def exec_visible():
    """The write statements whose data is in their own text, i.e. the only ones `sqlite3_exec` can execute.

    These are the statements the capture hook can ever see. Everything else carries `?N`/`@Name` bindings, which
    `sqlite3_exec` never receives - the values are bound after `sqlite3_prepare_v2` instead. Audit §29.2 measured
    361 parameterised against 67 literal writes, and `sqlcap` parses none of the 67; `--dump` writes this list out
    so `examples/sqlcap_probe.rs` can re-measure it.
    """
    out, bound = [], 0
    for entry in sorted(os.listdir(EXPORT)):
        if not entry.endswith("PS.json"):
            continue
        with open(os.path.join(EXPORT, entry), encoding="utf-8-sig") as f:
            asset = json.load(f)
        for e in asset[0]["Properties"]["Entries"]:
            sql = e["Value"]
            if not WRITE.match(sql):
                continue
            if re.search(r"\?\d|@[A-Za-z]", sql):
                bound += 1
                continue
            out.append(" ".join(sql.split()))
    return out, bound


def dump(path):
    stmts, bound = exec_visible()
    with open(path, "w", encoding="utf-8") as f:
        json.dump(stmts, f, ensure_ascii=False, indent=0)
    print("wrote %d exec-visible write statements to %s" % (len(stmts), path))
    print("%d writes carry bindings and never reach sqlite3_exec" % bound)
    print("")
    print("  cargo run --release -p coop-mem --example sqlcap_probe -- %s" % path)
    return 0


def parse_replicable():
    """`Table { name: "X", pk: "Y", team: "Z" }` entries from schema.rs."""
    with open(SCHEMA_RS, encoding="utf-8") as f:
        src = f.read()
    pat = re.compile(r'Table\s*\{\s*name:\s*"([^"]+)",\s*pk:\s*"([^"]+)",\s*team:\s*"([^"]+)"\s*\}')
    return [(m.group(1), m.group(2), m.group(3)) for m in pat.finditer(src)]


def parse_scope_of():
    """`"X" => Scope::Y` arms in coop-auth's scope_of."""
    with open(AUTH_RS, encoding="utf-8") as f:
        src = f.read()
    at = src.find("pub fn scope_of(")
    if at < 0:
        return []
    body = src[at:]
    end = body.find("\n}")
    body = body[: end if end > 0 else len(body)]
    return re.findall(r'"([A-Za-z_][A-Za-z0-9_]*)"\s*=>\s*Scope::', body)


def main():
    if "--dump" in sys.argv:
        # `--dump <path>`: write the statements `sqlite3_exec` can see, for `examples/sqlcap_probe.rs`.
        at = sys.argv.index("--dump")
        target = sys.argv[at + 1] if at + 1 < len(sys.argv) else "exec_visible.json"
        if not os.path.isdir(EXPORT):
            skip("no export at %s (set COOP_DB_EXPORT)" % EXPORT)
        return dump(target)

    if not os.path.exists(VOLTA):
        skip("no Volta.json at %s (set COOP_DB_EXPORT)" % VOLTA)
    if not os.path.exists(SCHEMA_RS):
        skip("schema.rs not found at %s" % SCHEMA_RS)

    con, size = load_volta()
    schema = real_schema(con)
    tables = con.execute("SELECT count(*) FROM sqlite_master WHERE type='table'").fetchone()[0]
    views = con.execute("SELECT count(*) FROM sqlite_master WHERE type='view'").fetchone()[0]
    version = con.execute("PRAGMA user_version").fetchone()[0]
    print("Volta: %d bytes, user_version %d, %d tables, %d views" % (size, version, tables, views))

    written, has_team = written_tables()
    listed = parse_replicable()
    failures = []

    if not listed:
        failures.append("schema.rs declares no Table entries (or they no longer parse)")

    # ---- 1 and 2: what is listed must be real, with the claimed shape --------------------------------
    for name, pk, team in listed:
        if name not in schema:
            failures.append("%s is listed but is not a table in the database" % name)
            continue
        s = schema[name]
        if pk not in s["columns"]:
            failures.append("%s: key column %s does not exist" % (name, pk))
        elif s["pk"] != [pk]:
            failures.append("%s: key is %s, not the single column %s" % (name, s["pk"], pk))
        if team not in s["columns"]:
            failures.append("%s: team column %s does not exist" % (name, team))

    # ---- 3 and 4: the list must be neither narrower nor wider than the derivation ---------------------
    derived = sorted(
        t for t, s in schema.items()
        if t in written and t in has_team
        and not t.startswith(("Save_", "Scenario_", "Onboarding_"))
        and "_Enum_" not in t
        and len(s["pk"]) == 1
    )
    listed_names = sorted(n for n, _, _ in listed)
    for t in derived:
        if t not in listed_names:
            failures.append("%s qualifies (single PK, TeamID, written) but is not listed" % t)
    for t in listed_names:
        if t not in derived:
            failures.append("%s is listed but does not qualify" % t)
    print("\nreplicable by derivation: %d, listed in schema.rs: %d" % (len(derived), len(listed_names)))

    # ---- 5: every table coop-auth classifies must exist ----------------------------------------------
    scoped = parse_scope_of()
    print("tables classified by scope_of: %d" % len(scoped))
    for t in scoped:
        if t not in schema:
            failures.append("scope_of classifies %s, which is not a table in the database" % t)

    print("\ntables the game writes: %d (of which %d mention TeamID)" % (len(written), len(has_team)))
    phantom_written = sorted(t for t in written if t not in schema)
    if phantom_written:
        print("  written but not tables (parser artefacts, not failures): %s" % ", ".join(phantom_written))

    if failures:
        print("\nFAIL: %d disagreement(s) between the code and the game's own schema:" % len(failures))
        for f in failures:
            print("  - " + f)
        return 1
    print("\nSCHEMA OK: the listed tables, their keys and their team columns all match Volta.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
