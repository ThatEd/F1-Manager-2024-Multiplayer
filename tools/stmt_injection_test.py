#!/usr/bin/env python3
"""Prove the statement applier's escaping defeats injection, using real SQLite.

Why this exists
---------------
`coop-mem::stmtapply` turns a peer's `Msg::Stmt` into SQL text, because the only write path exposed by the mod -
`LiveDb::sql` - hands a single string to `sqlite3_exec` inside the game. There is no prepare/bind available at
that layer, so string composition is unavoidable, and the defence is:

  * identifiers checked against an allowlist (`ident_ok`), never interpolated;
  * values rendered as literals by type, with text quoted and internal quotes doubled;
  * only `UPDATE ... WHERE <local pk column> = <int>` ever produced.

The Rust unit tests assert the *shape* of the output string. That is not the same as proving SQLite reads it as
one statement with the payload as data - a cleverer escape bug would still produce a plausible-looking string.
This runs the generated statements through the real engine, so "the payload cannot execute" is a fact about
SQLite rather than an expectation about a string.

It mirrors the Rust implementation rather than importing it. That duplication is the cost of testing across the
language boundary; if the Rust changes, `the_rules_mirror_the_rust` style assertions in the Rust tests fail, and
this script is the second check on the behaviour that matters.

Usage
-----
    python tools/stmt_injection_test.py
"""

import sqlite3
import sys

results = []


def ok(cond, name, detail=""):
    results.append((bool(cond), name))
    print(f"  {'ok  ' if cond else 'FAIL'}  {name}" + ("" if cond else f"  -- {detail}"))


# ---- mirrors of crates/coop-mem/src/stmtapply.rs ------------------------------------------------
def ident_ok(s):
    if not s or len(s) > 64:
        return False
    if not (s[0].isalpha() or s[0] == "_"):
        return False
    return all(c.isalnum() or c == "_" for c in s)


def literal(kind, val):
    if kind == "null":
        return "NULL"
    if kind == "int":
        return str(val)
    if kind == "real":
        return repr(val) if val == val and val not in (float("inf"), float("-inf")) else "NULL"
    return "'" + val.replace("\x00", "").replace("'", "''") + "'"


def to_update(table, pk, assignments, pk_column):
    if not ident_ok(table) or not ident_ok(pk_column):
        return None
    # The local schema decides the predicate. `Player` is keyed `(FirstName, LastName)` and `Teams` is keyed
    # `TeamID`; a wrapper that let the caller pick any column would let one message match many rows.
    if KEY.get(table) != pk_column:
        return None
    parts = []
    for col, (kind, val) in assignments:
        if not ident_ok(col):
            return None
        parts.append(f"{col} = {literal(kind, val)}")
    if not parts:
        return None
    return f"UPDATE {table} SET {', '.join(parts)} WHERE {pk_column} = {pk};"


# The tables this build replicates, and the single-column key the game's own schema declares for each. A subset of
# `coop-rs/crates/coop-mem/src/schema.rs`; `tools/schema_truth.py` proves that list against the game's database.
KEY = {"Teams": "TeamID", "Teams_Colours": "ColourID", "Parts_Designs": "DesignID"}


def fresh():
    db = sqlite3.connect(":memory:")
    db.execute("CREATE TABLE Teams (TeamID INTEGER PRIMARY KEY, PredictedRanking INTEGER, ProfileText TEXT)")
    db.execute("INSERT INTO Teams VALUES (99, 1, 'orig')")
    db.execute("INSERT INTO Teams VALUES (100, 1, 'other')")
    return db


print("statement injection test (real SQLite)")

# ---- 1. the classic payload, applied as a value ------------------------------------------------
payload = "7'); DROP TABLE Teams; --"
sql = to_update("Teams", 99, [("ProfileText", ("text", payload))], "TeamID")
db = fresh()
db.executescript(sql)
rows = db.execute("SELECT TeamID, ProfileText FROM Teams ORDER BY TeamID").fetchall()
ok(rows[0][1] == payload, "a hostile value is stored as literal text", str(rows))
ok(len(rows) == 2, "no row was deleted", str(rows))
ok(db.execute("SELECT count(*) FROM sqlite_master WHERE name='Teams'").fetchone()[0] == 1,
   "the Teams table still exists")
ok(rows[1][1] == "other", "an unrelated row is untouched")

# ---- 2. the same payload as a table or column name must be refused before SQL exists ------------
ok(to_update("Teams; DROP TABLE Teams; --", 99, [("PredictedRanking", ("int", 7))], "TeamID") is None,
   "a table name carrying a statement is refused")
ok(to_update("Teams", 99, [("PredictedRanking = 1 WHERE 1=1 --", ("int", 7))], "TeamID") is None,
   "a column name carrying a predicate is refused")
ok(to_update("Teams", 99, [("PredictedRanking", ("int", 7))], "TeamID = 1 OR 1") is None,
   "a bad local key column is refused too")
# The one that matters most: a well-formed name that simply is not this table's key. `WHERE PredictedRanking = 99`
# would match every team with that ranking instead of one row.
ok(to_update("Teams", 99, [("PredictedRanking", ("int", 7))], "PredictedRanking") is None,
   "a predicate the peer chooses, but that is not the table's key, is refused")

# ---- 3. a legitimate update still works ---------------------------------------------------------
db = fresh()
db.executescript(to_update("Teams", 99, [("PredictedRanking", ("int", 7)), ("ProfileText", ("null", None))], "TeamID"))
got = db.execute("SELECT PredictedRanking, ProfileText FROM Teams WHERE TeamID=99").fetchone()
ok(got == (7, None), "a normal change applies exactly", str(got))
ok(db.execute("SELECT PredictedRanking FROM Teams WHERE TeamID=100").fetchone()[0] == 1,
   "and touches only the named row")

# ---- 4. values that would be syntax if unquoted -------------------------------------------------
for bad, name in (("'; DROP TABLE Teams; --", "leading quote"),
                  ("\\'; DROP TABLE Teams; --", "backslash then quote"),
                  ("1e309", "overflow literal"),
                  ("NaN", "NaN word")):
    sql = to_update("Teams", 99, [("ProfileText", ("text", bad))], "TeamID")
    db = fresh()
    db.executescript(sql)
    ok(db.execute("SELECT count(*) FROM sqlite_master WHERE name='Teams'").fetchone()[0] == 1,
       f"payload survives as data: {name}")

failed = sum(1 for good, _ in results if not good)
print(f"\n  {len(results) - failed}/{len(results)} passed")
if failed:
    print(f"  {failed} failed")
    sys.exit(1)
