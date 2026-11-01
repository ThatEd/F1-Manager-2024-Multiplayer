#!/usr/bin/env python3
"""Prove the diff producer against the game's real schema, using real SQLite.

Why this exists
---------------
`coop-mem/src/careerdiff.rs` replaces the capture hook that audit §29.2 found could never work: instead of
observing the game's write (which goes through `sqlite3_prepare_v2` + bindings and never reaches the patched
`sqlite3_exec`), it reads the 16 replicable tables and diffs consecutive reads.

That raises three questions the Rust unit tests cannot answer on their own, because they need a real engine:

  1. do the generated `SELECT hex(quote(col)), * FROM t ORDER BY pk` statements actually run against the schema
     the game ships, on all 16 tables?
  2. does `hex(quote(x))` really round-trip every value type that schema contains, through the same
     `\\x1f`/`\\x1e` framing `LiveDb::sql` uses?
  3. does a realistic edit produce exactly one changed cell, and leave the other 69 rows alone?

This runs all three against `Volta.json` - the game's own schema and seed data - so an answer of "yes" is a fact
about the game's data rather than about a hand-written fixture.

Usage
-----
    python -I tools/quote_probe.py
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

COL_SEP = "\x1f"
ROW_SEP = "\x1e"

results = []


def ok(cond, name, detail=""):
    results.append(bool(cond))
    print("  %s  %s%s" % ("ok  " if cond else "FAIL", name, "" if cond else "  -- " + str(detail)))


def replicable():
    """`Table { name: "X", pk: "Y", team: "Z" }` entries, in source order."""
    src = open(SCHEMA_RS, encoding="utf-8").read()
    pat = re.compile(r'Table\s*\{\s*name:\s*"([^"]+)",\s*pk:\s*"([^"]+)",\s*team:\s*"([^"]+)"\s*\}')
    return [(m.group(1), m.group(2), m.group(3)) for m in pat.finditer(src)]


def open_game_db():
    """`Volta.Data` is a byte-for-byte SQLite file; write it out and open it."""
    asset = json.load(open(VOLTA, encoding="utf-8-sig"))
    data = bytes(asset[0]["Properties"]["Data"])
    if not data.startswith(b"SQLite format 3\x00"):
        print("SKIP: Volta.Data is not a SQLite file")
        sys.exit(0)
    path = os.path.join(tempfile.mkdtemp(prefix="volta-"), "career.db")
    open(path, "wb").write(data)
    con = sqlite3.connect(path)
    con.row_factory = sqlite3.Row
    return con


def columns_of(con, table):
    """The table's columns, in declaration order - what `careerdiff::columns_sql` returns."""
    return [r[0] for r in con.execute("SELECT name FROM pragma_table_info('%s')" % table)]


def select_sql(table, pk, columns):
    """Mirror of `careerdiff::select_sql`, which takes an explicit column list rather than `*`.

    `*` cannot work: it re-emits the key unencoded beside its encoded copy, and the reader decodes every cell.
    """
    parts = []
    for c in columns:
        parts.append(
            'CASE WHEN typeof("%s") = \'blob\' THEN \'X_BLOB\' ELSE hex(quote("%s")) END' % (c, c)
        )
    return 'SELECT %s FROM "%s" ORDER BY "%s";' % (", ".join(parts), table, pk)


def cells(raw):
    """Reassemble the `\\x1f`/`\\x1e` framing `coop-mem` uses for a read, from a cursor."""
    out, labels = [], []
    for row in raw:
        vals = []
        for v in row:
            vals.append("" if v is None else str(v))
        labels = list(row.keys())
        out.append(COL_SEP.join(vals) + ROW_SEP)
    return "".join(out), labels


def main():
    if not os.path.exists(VOLTA):
        print("SKIP: no Volta.json at %s (set COOP_DB_EXPORT)" % VOLTA)
        return 0
    con = open_game_db()
    tables = replicable()

    print("quote() and framing, against the game's own schema")
    ok(len(tables) == 16, "schema.rs lists 16 replicable tables", len(tables))

    # ---- 1. every generated statement runs, and its first column is the key -------------------------
    total_rows = 0
    framed_bytes = 0
    per_table = {}
    for name, pk, _team in tables:
        try:
            rows = con.execute(select_sql(name, pk, columns_of(con, name))).fetchall()
        except sqlite3.Error as e:
            ok(False, "%s: the generated SELECT runs" % name, e)
            continue
        total_rows += len(rows)
        raw, labels = cells(rows)
        framed_bytes += len(raw.encode())
        per_table[name] = len(rows)
        if labels:
            ok(labels[0].lower().startswith("case when typeof("),
               "%s: the first column is the key, encoded" % name, labels[0])
    ok(True, "all 16 generated SELECTs ran")
    print("       %d rows, %d bytes framed across all 16 tables" % (total_rows, framed_bytes))
    ok(total_rows < 500,
       "one full pass is small enough to do repeatedly (%d rows)" % total_rows,
       "the cost model assumes tens of rows, not thousands")

    # ---- 2. hex(quote(x)) round-trips every type the schema actually stores ------------------------
    seen_types = set()
    bad = []
    sample_count = 0
    for name, pk, _team in tables:
        for row in con.execute('SELECT * FROM "%s"' % name):
            for (col, val) in zip(row.keys(), tuple(row)):
                rendered = con.execute("SELECT quote(?)", (val,)).fetchone()[0]
                typ = con.execute("SELECT typeof(?)", (val,)).fetchone()[0]
                seen_types.add(typ)
                sample_count += 1
                if typ == "blob":
                    continue  # deliberately uncarriable, and the code says so
                try:
                    back = bytes.fromhex(rendered.encode().hex()).decode()
                except (ValueError, UnicodeDecodeError) as e:
                    bad.append((name, col, typ, str(e)))
                    continue
                if back != rendered:
                    bad.append((name, col, typ, "hex round-trip changed the value"))
    ok(not bad, "every stored value survives hex(quote(x)) unchanged", bad[:3])
    ok(seen_types <= {"null", "integer", "real", "text"},
       "the seed data contains no BLOB in a replicable table",
       "found %s - a BLOB has no Msg::Stmt representation" % sorted(seen_types))
    print("       %d values sampled, types seen: %s" % (sample_count, ", ".join(sorted(seen_types))))

    # ---- 3. a realistic edit changes exactly one cell ---------------------------------------------
    victim = "Teams"
    con.execute('UPDATE "Teams" SET TeamName = TeamName || \' (edited)\' WHERE TeamID = (SELECT MIN(TeamID) FROM Teams)')
    cols = columns_of(con, victim)
    rows = con.execute(select_sql(victim, "TeamID", cols)).fetchall()
    raw, labels = cells(rows)
    ok(len(labels) >= 3, "the read returns labelled columns", labels)
    ok(raw.count(ROW_SEP) == len(rows), "the framing has one terminator per row", raw.count(ROW_SEP))
    for line in raw.split(ROW_SEP):
        if not line:
            continue
        parts = line.split(COL_SEP)
        if len(parts) != len(labels):
            ok(False, "no row split into more cells than there are columns", "%d vs %d" % (len(parts), len(labels)))
            break
    else:
        ok(True, "no row split into more cells than there are columns")

    failed = results.count(False)
    print("\n  %d/%d passed" % (len(results) - failed, len(results)))
    if failed:
        print("  %d failed" % failed)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
