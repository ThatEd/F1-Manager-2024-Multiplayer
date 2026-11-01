//! A career producer that needs no hook: read the replicable tables, diff consecutive reads, send what changed.
//!
//! Why this exists
//! ---------------
//! Audit §29.2 measured that the capture hook cannot work: it patches `sqlite3_exec`, career writes go through
//! `sqlite3_prepare_v2` + bindings, and `sqlcap` parses none of the 67 statements that do reach `exec`. One of the
//! three replacements it listed is implemented here, and it is the only one that can be finished and validated
//! **without launching the game**: instead of observing the write, observe the *result*.
//!
//! The cost model is what makes it viable, and it is measured rather than assumed. `content/database/Volta` seeded
//! with the game's own data holds **70 rows across the 16 replicable tables, about 7.6 KB rendered** - the largest
//! is `Teams` at 32 rows, `Parts_Designs` at 12, `Staff_NarrativeData` at 13. One pass every few seconds costs more
//! in syscalls than in bytes, which is not true of the tables that were rejected: `StaffPS`'s 446 staff rows alone
//! would dwarf it, and they are not replicable because their tables are composite-key.
//!
//! Why text and not typed values
//! -----------------------------
//! The read happens inside the game through `LiveDb::sql`, which returns **strings** - the row callback in
//! `livedb.rs` writes NUL-terminated text into a buffer. Getting real types would need a second SQL function that
//! reports `typeof()`, and `quote()` already does that job: it renders each value the way SQL would read it back,
//! and `sqlcap::parse_value` accepts exactly the shapes it produces. The pairing is not assumed, it is tested in
//! both directions (`tools/quote_probe.py` for the SQLite half, the tests below for the Rust half).
//!
//! The separator hazard
//! --------------------
//! `LiveDb::parse_rows` splits columns on `\x1f` and rows on `\x1e`. `quote()` does **not** escape those bytes, so
//! a text value containing either would split one row into two and silently mis-attribute every following column -
//! and staff nicknames, team profile text and loglines are all free text. So the query asks for `hex(quote(col))`,
//! which is pure `[0-9A-F]` and decodes back to the exact rendered value. That costs no correctness trade-off: the
//! decoding is local, and the alternative - trusting no value to contain a control byte - is a bet on game content.

use crate::schema::{self, Table};

/// The columns read for one table, and the key they are identified by.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub table: &'static str,
    pub key: &'static str,
    /// Every column the table declares that the diff cares about: all of them.
    pub columns: Vec<String>,
}

/// One row as read back: its key, and every selected column's rendered value.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: crate::sqlcap::Value,
    /// `(column, rendered value)` in shape order, including the key column itself.
    pub cells: Vec<(String, crate::sqlcap::Value)>,
}

/// What one pass over the tables observed, so the next pass can compare.
pub type Snapshot = std::collections::BTreeMap<String, Vec<Row>>;

/// The SQL that reads one table, with every value hex-encoded so no value can contain a row separator.
///
/// `hex(quote(x))` rather than `hex(x)`: for a BLOB both are the same bytes, but for text `hex(x)` loses the type
/// distinction entirely (an integer `5` and a text `'5'` both become `35`), while `quote(x)` renders `5` and `'5'`
/// differently and `parse_value` reads them back as different types. The `typeof()` branch is what makes the BLOB
/// case explicit rather than accidental.
///
/// The column list is **explicit and never `*`**. The first version used `SELECT hex(quote(pk)), *` and it could
/// not work: `*` re-emits the key column unencoded, right after its encoded copy, so the raw `7` for an integer key
/// reaches `decode_cell` with an odd number of characters and every row fails with "column TeamID did not decode".
/// Verified against real SQLite before changing this - the row came back as `('37', 7, 'McLaren')`.
pub fn select_sql(shape: &Shape) -> String {
    let mut cols = Vec::new();
    for c in &shape.columns {
        // `quote()` is a SQL function whose output is SQL syntax; an unrecognised type is marked so the reader
        // refuses it instead of guessing. Only `blob` needs the marker - `null`, `integer`, `real` and `text` all
        // render to something `parse_value` accepts.
        cols.push(format!(
            "CASE WHEN typeof(\"{c}\") = 'blob' THEN 'X_BLOB' ELSE hex(quote(\"{c}\")) END"
        ));
    }
    format!("SELECT {} FROM \"{}\" ORDER BY \"{}\";", cols.join(", "), shape.table, shape.key)
}

/// The `PRAGMA` that lists a table's columns, in declaration order.
pub fn columns_sql(table: &'static str) -> String {
    format!("SELECT name FROM pragma_table_info('{table}');")
}

/// Builds a shape from the column names `columns_sql` returned.
///
/// Column names come from the database rather than from `schema.rs` because the schema module records only what the
/// policy needs (table, key, team); replicating a change means carrying *every* column that could have moved, and a
/// column added by a game patch has to appear here without a code change.
pub fn shape_from(t: &Table, column_names: &[String]) -> Result<Shape, String> {
    if column_names.is_empty() {
        return Err(format!("{}: the database reported no columns", t.name));
    }
    if !column_names.iter().any(|c| c.eq_ignore_ascii_case(t.pk)) {
        return Err(format!(
            "{}: the schema's key {} is not one of the table's columns; the key is wrong for this build",
            t.name, t.pk
        ));
    }
    Ok(Shape { table: t.name, key: t.pk, columns: column_names.to_vec() })
}

/// The SQL for a table when the caller does not know its columns yet: ask the database.
///
/// `shapes()` exists for callers with no schema access; the live path uses [`columns_sql`] then [`shape_from`],
/// because `SELECT *, hex(quote(...))` cannot work - `*` re-emits the key unencoded beside its encoded copy.
pub fn shapes() -> Vec<Shape> {
    schema::REPLICABLE
        .iter()
        .map(|t| Shape { table: t.name, key: t.pk, columns: Vec::new() })
        .collect()
}

/// Decodes `hex(quote(x))` back to the rendered SQL value.
///
/// Returns `None` for anything that is not an even-length run of hex digits, which is what a mis-sized read or a
/// truncated value looks like. `X_BLOB` (the marker `select_sql` emits) also returns `None`: `StmtValue` has no
/// BLOB variant, and inventing one by rendering the bytes as text would write a different value than the one read.
pub fn decode_cell(hex: &str) -> Option<String> {
    let t = hex.trim();
    if t.len() % 2 != 0 || t.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(t.len() / 2);
    let b = t.as_bytes();
    for pair in b.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    String::from_utf8(out).ok()
}

/// Parses one `SELECT` result into rows, given the column names in result order.
///
/// `raw` is what `LiveDb::sql` produces: `\x1f` between columns, `\x1e` after each row.
pub fn parse_rows(raw: &[u8], columns: &[String], key_column: &str) -> Result<Vec<Row>, String> {
    let key_index = columns
        .iter()
        .position(|c| c.eq_ignore_ascii_case(key_column))
        .ok_or_else(|| format!("the read did not return the key column {key_column}"))?;

    let text = String::from_utf8_lossy(raw);
    let mut rows = Vec::new();
    for line in text.split('\u{1e}') {
        if line.is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split('\u{1f}').collect();
        if cells.len() != columns.len() {
            return Err(format!(
                "row has {} cells but {} columns were asked for; a value likely contained a separator",
                cells.len(),
                columns.len()
            ));
        }
        let mut parsed = Vec::with_capacity(cells.len());
        for (name, cell) in columns.iter().zip(cells.iter()) {
            let rendered = decode_cell(cell).ok_or_else(|| {
                format!("column {name} did not decode; a BLOB or a truncated read cannot be replicated")
            })?;
            let v = crate::sqlcap::parse_value(&rendered).ok_or_else(|| {
                format!("column {name} rendered as {rendered:?}, which is not a value this build replicates")
            })?;
            parsed.push((name.clone(), v));
        }
        let key = parsed[key_index].1.clone();
        rows.push(Row { key, cells: parsed });
    }
    Ok(rows)
}

/// One table's difference between two snapshots.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableDiff {
    pub table: String,
    /// Rows that exist in both and differ in at least one column, as `(key, changed columns)`.
    pub changed: Vec<(crate::sqlcap::Value, Vec<(String, crate::sqlcap::Value)>)>,
    /// Rows present now that were not present before.
    pub added: Vec<crate::sqlcap::Value>,
    /// Rows that were present before and are gone now.
    pub removed: Vec<crate::sqlcap::Value>,
}

impl TableDiff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

/// Index rows by their rendered key, so two passes can be compared without assuming order.
fn index_by_key(rows: &[Row]) -> std::collections::BTreeMap<String, &Row> {
    rows.iter().map(|r| (r.key.as_text(), r)).collect()
}

/// Compares one table between two passes.
///
/// `changed` is the only part this build can carry. `Msg::Stmt` is an `UPDATE ... WHERE <key> = <v>`, so a row that
/// did not exist on the far side would have the update match nothing and report success - a silent no-op that looks
/// exactly like replication working. `added` is returned rather than silently dropped, so the caller can say how
/// many rows it could not carry; that number going non-zero is the signal that `Msg::Stmt` needs an INSERT form.
pub fn diff_table(table: &str, before: &[Row], after: &[Row]) -> TableDiff {
    let b = index_by_key(before);
    let a = index_by_key(after);
    let mut d = TableDiff { table: table.to_string(), ..Default::default() };

    for (k, now) in &a {
        match b.get(k) {
            None => d.added.push(now.key.clone()),
            Some(was) => {
                let changed: Vec<(String, crate::sqlcap::Value)> = now
                    .cells
                    .iter()
                    .filter(|(name, v)| {
                        was.cells
                            .iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, old)| old != v)
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect();
                if !changed.is_empty() {
                    d.changed.push((now.key.clone(), changed));
                }
            }
        }
    }
    for (k, was) in &b {
        if !a.contains_key(k) {
            d.removed.push(was.key.clone());
        }
    }
    d
}

/// Compares a whole snapshot. Keyed by table name so the order is stable across passes.
pub fn diff(before: &Snapshot, after: &Snapshot) -> Vec<TableDiff> {
    let mut out = Vec::new();
    for (table, now) in after {
        let empty: Vec<Row> = Vec::new();
        let was = before.get(table).unwrap_or(&empty);
        let d = diff_table(table, was, now);
        if !d.is_empty() {
            out.push(d);
        }
    }
    out
}

/// A whole comparison, with the rows that could not be expressed as an `UPDATE` counted rather than dropped.
///
/// The one entry point callers should use: computing `diffs` and `uncarriable` separately is how the two get out of
/// step, and a pass that reports changes while silently discarding the rows it cannot carry is the failure mode
/// this counter exists to prevent.
pub fn compare(before: &Snapshot, after: &Snapshot) -> Pass {
    let diffs = diff(before, after);
    let uncarriable = diffs.iter().map(|d| d.added.len() + d.removed.len()).sum();
    Pass { diffs, uncarriable, rows: after.clone() }
}

/// One full pass over the replicable tables, through a live database handle.
///
/// The column list comes from the database (`pragma_table_info`) rather than from this repository, because the diff
/// has to carry *every* column that could have moved - including one added by a game patch, which no list here
/// would know about. Everything is read through `hex(quote(col))` so no value can contain the separator
/// [`crate::livedb::LiveDb::sql`]'s row callback uses, and so each value keeps its SQL type.
///
/// Every table is attempted even after one fails, and the failures are returned together: "which table broke" is
/// the whole diagnostic value of a pass, and stopping at the first would hide the rest.
///
/// Takes `&LiveDb` rather than attaching, so a caller can hold one handle for the session. `LiveDb::attach` starts a
/// worker thread inside the game, and attaching per pass would create one every few seconds.
pub fn snapshot(db: &crate::livedb::LiveDb) -> Result<Snapshot, String> {
    let mut snap = Snapshot::new();
    let mut failures = Vec::new();
    for t in schema::REPLICABLE {
        // 1. What columns does this table really have?
        let listed = match db.sql(&columns_sql(t.name)) {
            Ok(rows) => rows,
            Err(e) => {
                failures.push(format!("{}: listing columns failed: {e}", t.name));
                continue;
            }
        };
        let columns: Vec<String> = listed
            .iter()
            .filter_map(|r| r.first().cloned())
            .filter(|c| !c.is_empty())
            .collect();
        let shape = match shape_from(t, &columns) {
            Ok(s) => s,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };

        // 2. Read it.
        let rows = match db.sql(&select_sql(&shape)) {
            Ok(rows) => rows,
            Err(e) => {
                failures.push(format!("{}: read failed: {e}", t.name));
                continue;
            }
        };

        // 3. Decode. `LiveDb::sql` returns parsed rows, so the framing is rebuilt to reuse `parse_rows`, which is
        //    the single implementation of the value-decoding rules - sharing it is the honest way to avoid a second,
        //    drifting copy of the `hex(quote(x))` contract.
        let text: String = rows.iter().map(|r| r.join("\u{1f}") + "\u{1e}").collect();
        match parse_rows(text.as_bytes(), &shape.columns, shape.key) {
            Ok(parsed) => {
                snap.insert(t.name.to_string(), parsed);
            }
            Err(e) => failures.push(format!("{}: {e}", t.name)),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(snap)
}

/// What one pass over the tables produced, and what could not be carried.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pass {
    pub diffs: Vec<TableDiff>,
    /// Rows that could not be expressed as `Msg::Stmt` because `Msg::Stmt` is an `UPDATE`.
    pub uncarriable: usize,
    /// The rows as read this pass, so `to_changes` can find each changed row's team without a second read.
    ///
    /// Carrying it here rather than re-reading is what keeps `compare` a pure function of two snapshots: the team is
    /// a property of the row (§29.4), and the row is already in hand.
    pub rows: Snapshot,
}

impl Pass {
    pub fn is_empty(&self) -> bool {
        self.diffs.is_empty()
    }
    /// How many single-row changes are ready to send.
    pub fn carriable(&self) -> usize {
        self.diffs.iter().map(|d| d.changed.len()).sum()
    }
    /// Every change this pass produced, already attributed and keyed, ready for `replicate::plan`.
    ///
    /// The rows are looked up from `self.rows`, so a caller cannot forget to pass them - which is precisely the
    /// mistake that made `to_changes`'s old signature easy to misuse.
    pub fn changes(&self) -> (Vec<crate::sqlcap::Change>, usize) {
        let mut out = Vec::new();
        let mut uncarriable = self.uncarriable;
        for d in &self.diffs {
            let rows = self.rows.get(&d.table).map(|v| v.as_slice()).unwrap_or(&[]);
            let (mut c, u) = to_changes(d, rows);
            out.append(&mut c);
            uncarriable += u;
        }
        (out, uncarriable)
    }
}

/// Turns a table diff into `Change`s that `replicate::plan` can accept.
///
/// Only `changed` rows become changes, and only for columns that differ. An `added` or `removed` row is counted in
/// `uncarriable` instead of being sent as an update that would match nothing.
///
/// **The team travels even though it did not change.** `replicate::plan` attributes a change by finding `TeamID` in
/// the values being set (§29.4), and a normal career edit sets one business column - `Parts_Designs.DesignWork`,
/// `Races_GridPenalties.Served` - and never the team. So a change carrying only the changed columns would be refused
/// as `NoTeam` for every real input. The diff has the row's whole cell list, so the team column is added to the
/// change explicitly; it then costs one unchanged assignment on the wire and makes the existing policy work without
/// weakening it.
pub fn to_changes(d: &TableDiff, rows: &[Row]) -> (Vec<crate::sqlcap::Change>, usize) {
    let uncarriable = d.added.len() + d.removed.len();
    let Some(table) = schema::lookup(&d.table) else {
        // A table the schema does not know cannot be attributed or keyed; count it rather than send something the
        // receiver will refuse for a different reason.
        return (Vec::new(), uncarriable + d.changed.len());
    };
    let changes = d
        .changed
        .iter()
        .map(|(key, cells)| {
            // The key column is the predicate, never an assignment: `UPDATE Teams SET TeamID = 1 WHERE TeamID
            // = 1` is a no-op at best and, for a table whose key is not its team column, would rewrite the
            // identity the whole WHERE depends on. `Teams` is exactly that case - keyed and teamed by `TeamID`
            // - which is how this was caught: the probe printed `Teams SET TeamName, TeamID WHERE TeamID = 1`.
            let set: Vec<_> = cells
                .iter()
                .filter(|(c, _)| !c.eq_ignore_ascii_case(table.pk))
                .cloned()
                .collect();
            // The team is appended even though it did not move, so `replicate::plan` can attribute the change
            // (§29.4). A normal career edit sets one business column and never the team, so without this every
            // change would be refused as `NoTeam`.
            let mut set = set;
            if !set.iter().any(|(c, _)| c.eq_ignore_ascii_case(table.team)) {
                if let Some(row) = rows.iter().find(|r| &r.key == key) {
                    if let Some((n, v)) = row
                        .cells
                        .iter()
                        .find(|(c, _)| c.eq_ignore_ascii_case(table.team))
                    {
                        set.push((n.clone(), v.clone()));
                    }
                }
            }
            crate::sqlcap::Change {
                table: schema::lookup(&d.table).map(|t| t.name.to_string()).unwrap_or_else(|| d.table.clone()),
                action: crate::sqlcap::Action::Update,
                // The key travels as the statement's `WHERE`, taken from the row the local engine actually returned
                // rather than from anything a peer said.
                key: Some((table.pk.to_string(), key.clone())),
                set,
            }
        })
        .collect();
    (changes, uncarriable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlcap::{parse_value, Value};

    fn row(key: i64, pairs: &[(&str, &str)]) -> Row {
        Row {
            key: Value::Int(key),
            cells: pairs.iter().map(|(n, v)| (n.to_string(), parse_value(v).unwrap())).collect(),
        }
    }

    #[test]
    fn a_changed_cell_is_carried_and_an_unchanged_row_is_not() {
        let before = vec![row(1, &[("TeamID", "1"), ("TeamName", "'McLaren'")])];
        let after = vec![row(1, &[("TeamID", "1"), ("TeamName", "'McLaren Racing'")])];
        let d = diff_table("Teams", &before, &after);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].0, Value::Int(1));
        assert_eq!(d.changed[0].1.len(), 1, "only the column that moved travels");
        assert_eq!(d.changed[0].1[0].0, "TeamName");

        // And an identical pass produces nothing at all: a producer that re-sends unchanged rows would put a
        // full-table rewrite on the wire every second, which is the failure this design exists to avoid.
        assert!(diff_table("Teams", &after, &after).is_empty());
    }

    #[test]
    fn a_new_row_is_reported_but_not_sent() {
        // `Msg::Stmt` is an UPDATE. Sending one for a row the peer does not have matches nothing and reports
        // success, which is indistinguishable from replication working - so it is counted instead.
        let before = vec![row(1, &[("TeamID", "1")])];
        let after = vec![row(1, &[("TeamID", "1")]), row(2, &[("TeamID", "2")])];
        let d = diff_table("Teams", &before, &after);
        assert_eq!(d.added, vec![Value::Int(2)]);
        assert!(d.changed.is_empty());
        let (changes, uncarriable) = to_changes(&d, &after);
        assert!(changes.is_empty(), "an added row must not become an UPDATE");
        assert_eq!(uncarriable, 1);
    }

    #[test]
    fn a_removed_row_is_reported_but_not_sent() {
        let before = vec![row(1, &[("TeamID", "1")]), row(2, &[("TeamID", "2")])];
        let after = vec![row(1, &[("TeamID", "1")])];
        let d = diff_table("Teams", &before, &after);
        assert_eq!(d.removed, vec![Value::Int(2)]);
        let (changes, uncarriable) = to_changes(&d, &after);
        assert!(changes.is_empty());
        assert_eq!(uncarriable, 1);
    }

    /// The one that made §29.4's finding fixable.
    ///
    /// A real career edit changes one business column and never the team - `Teams.PredictedRanking`,
    /// `Parts_Designs.DesignWork`. `replicate::plan` attributes by finding the team among the values being *set*, so
    /// a change carrying only the changed column is refused as `NoTeam`. The diff has the row's whole cell list, so
    /// the team is added back and the existing policy works unweakened.
    #[test]
    fn the_team_travels_even_though_it_did_not_change() {
        // `Teams` is keyed by `TeamID`, so the key column and the team column are the same one here - which is the
        // case where the key must NOT travel as an assignment.
        let before = vec![row(1, &[("TeamID", "1"), ("TeamName", "'McLaren'")])];
        let after = vec![row(1, &[("TeamID", "1"), ("TeamName", "'McLaren Racing'")])];
        let d = diff_table("Teams", &before, &after);
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].1.len(), 1, "only TeamName moved");

        let (changes, uncarriable) = to_changes(&d, &after);
        assert_eq!(uncarriable, 0);
        assert_eq!(changes.len(), 1);
        let c = &changes[0];
        // The changed column, plus the team, or `plan` refuses the change as `NoTeam`.
        assert!(c.set.iter().any(|(n, _)| n == "TeamName"), "{:?}", c.set);
        assert!(
            c.set.iter().any(|(n, v)| n.eq_ignore_ascii_case("TeamID") && *v == Value::Int(1)),
            "the team must travel or plan refuses the change: {:?}",
            c.set
        );
        // And the key must not: it is the predicate, and assigning it is a no-op at best.
        assert_eq!(
            c.set.iter().filter(|(n, _)| n.eq_ignore_ascii_case("TeamID")).count(),
            1,
            "the team column appears once - as the attribution, not as a second assignment"
        );
        assert_eq!(c.key.as_ref().map(|(k, _)| k.as_str()), Some("TeamID"));

        // And the policy accepts it, which is the point of carrying the team at all.
        let cfg = Some((coop_auth::Mode::Versus, coop_auth::Player::new(1, 0, true)));
        crate::replicate::plan(c, cfg).expect("the plan must accept a diff-produced change");
    }

    /// A table whose key is not its team column must not put the key into the SET list at all.
    ///
    /// `Parts_Designs` is keyed `DesignID` and teamed by `TeamID`. Assigning `DesignID` in an UPDATE whose WHERE
    /// is `DesignID = x` would rewrite the identity the predicate depends on - at best a no-op, at worst a row
    /// that no longer matches anything.
    #[test]
    fn the_key_column_is_never_an_assignment() {
        let before = vec![row(5, &[("DesignID", "5"), ("TeamID", "3"), ("DesignWork", "10")])];
        let after = vec![row(5, &[("DesignID", "5"), ("TeamID", "3"), ("DesignWork", "11")])];
        let d = diff_table("Parts_Designs", &before, &after);
        let (changes, _) = to_changes(&d, &after);
        assert_eq!(changes.len(), 1);
        assert!(
            !changes[0].set.iter().any(|(n, _)| n.eq_ignore_ascii_case("DesignID")),
            "the key must be the predicate, not an assignment: {:?}",
            changes[0].set
        );
    }

    #[test]
    fn a_row_whose_key_is_not_the_team_still_attributes() {
        // `Teams_Colours` is keyed `ColourID` and carries `TeamID`, so key and team differ.
        let before = vec![row(9, &[("ColourID", "9"), ("TeamID", "4"), ("Primary", "'ff0000'")])];
        let after = vec![row(9, &[("ColourID", "9"), ("TeamID", "4"), ("Primary", "'00ff00'")])];
        let d = diff_table("Teams_Colours", &before, &after);
        let (changes, _) = to_changes(&d, &after);
        assert_eq!(changes[0].key.as_ref().map(|(k, _)| k.as_str()), Some("ColourID"));
        assert!(changes[0].set.iter().any(|(n, _)| n == "TeamID"));
        let cfg = Some((coop_auth::Mode::Versus, coop_auth::Player::new(4, 0, true)));
        crate::replicate::plan(&changes[0], cfg).expect("attributable from the row's own team");
    }

    #[test]
    fn hex_round_trips_every_shape_quote_emits() {
        // The exact strings `quote()` produces, from `tools/quote_probe.py` against real SQLite. Decoding must
        // return them unchanged and `parse_value` must read them back as the type they were.
        let cases: [(&str, Value); 6] = [
            ("NULL", Value::Null),
            ("''", Value::Text(String::new())),
            ("'O''Brien'", Value::Text("O'Brien".to_string())),
            ("0", Value::Int(0)),
            ("-5", Value::Int(-5)),
            ("3.5", Value::Real(3.5)),
        ];
        for (rendered, want) in cases {
            let hex: String = rendered.bytes().map(|b| format!("{b:02X}")).collect();
            let back = decode_cell(&hex).expect("decodes");
            assert_eq!(back, rendered);
            assert_eq!(parse_value(&back), Some(want), "{rendered} must keep its type");
        }
    }

    #[test]
    fn a_text_value_containing_a_row_separator_survives() {
        // The concrete hazard: `LiveDb::parse_rows` splits on `\x1f` and `\x1e`, and `quote()` does not escape
        // them. Team profile text and staff nicknames are free text, so this is reachable with real game content.
        let rendered = "'has\u{1f}and\u{1e}semicolon;'";
        let hex: String = rendered.bytes().map(|b| format!("{b:02X}")).collect();
        assert!(!hex.contains('\u{1f}') && !hex.contains('\u{1e}'), "hex cannot carry the separators");
        assert_eq!(decode_cell(&hex).unwrap(), rendered);
    }

    #[test]
    fn a_blob_marker_is_refused_rather_than_rounded_to_text() {
        assert_eq!(decode_cell("X_BLOB"), None);
        // Odd length, non-hex, and empty are all "not a value".
        assert_eq!(decode_cell("ABC"), None);
        assert_eq!(decode_cell("ZZ"), None);
        assert_eq!(decode_cell(""), None);
    }

    #[test]
    fn a_row_with_the_wrong_cell_count_is_refused() {
        // If a separator ever did slip through, the row count would not match and the next columns would be
        // shifted - worse than failing, because it would write plausible wrong values.
        let raw = b"abc\x1fdef\x1e";
        let cols = vec!["TeamID".to_string(), "TeamName".to_string(), "Extra".to_string()];
        let err = parse_rows(raw, &cols, "TeamID").unwrap_err();
        assert!(err.contains("cells"), "{err}");
    }

    #[test]
    fn a_read_without_the_key_column_is_refused() {
        let raw = b"abc\x1fdef\x1e";
        let cols = vec!["TeamID".to_string(), "TeamName".to_string()];
        let err = parse_rows(raw, &cols, "DesignID").unwrap_err();
        assert!(err.contains("DesignID"), "{err}");
    }

    #[test]
    fn parse_rows_reads_a_real_shaped_result() {
        // Two rows as `hex(quote(...))` would emit them: an integer key with the text `A`, and key 2 with `B`.
        let hex = |s: &str| -> String { s.bytes().map(|b| format!("{b:02X}")).collect() };
        let row1 = format!("{}\x1f{}\x1e", hex("1"), hex("'A'"));
        let row2 = format!("{}\x1f{}\x1e", hex("2"), hex("'B'"));
        let raw = format!("{row1}{row2}");
        let cols = vec!["TeamID".to_string(), "TeamName".to_string()];
        let rows = parse_rows(raw.as_bytes(), &cols, "TeamID").expect("parses");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, Value::Int(1));
        assert_eq!(rows[0].cells[1].1, Value::Text("A".to_string()));
        assert_eq!(rows[1].key, Value::Int(2));
        assert_eq!(rows[1].cells[1].1, Value::Text("B".to_string()));
    }

    #[test]
    fn every_replicable_table_has_a_select_and_no_wildcard() {
        for t in schema::REPLICABLE {
            // Two columns is enough to exercise the shape; the real list comes from the database.
            let shape = Shape {
                table: t.name,
                key: t.pk,
                columns: vec![t.pk.to_string(), t.team.to_string()],
            };
            let sql = select_sql(&shape);
            assert!(sql.contains(t.name), "{sql}");
            assert!(sql.contains(t.pk), "{sql}");
            assert!(sql.contains("hex(quote("), "{sql}");
            // `*` re-emits the key unencoded beside its encoded copy, so the raw value reaches `decode_cell` and
            // every row fails. This is the bug the first version of this function had.
            assert!(!sql.contains('*'), "the column list must be explicit, not a wildcard: {sql}");
            assert_eq!(sql.matches("hex(quote(").count(), shape.columns.len(), "{sql}");
        }
    }

    #[test]
    fn a_shape_needs_the_key_the_schema_names() {
        let t = schema::lookup("Teams").expect("Teams is replicable");
        let ok = shape_from(t, &["TeamID".to_string(), "TeamName".to_string()]);
        assert_eq!(ok.unwrap().key, "TeamID");

        // A schema whose key is not in the table's columns means the key is wrong for this build, and every
        // predicate built from it would match nothing while reporting success.
        let bad = shape_from(t, &["TeamName".to_string()]);
        assert!(bad.unwrap_err().contains("TeamID"));
        assert!(shape_from(t, &[]).is_err());
    }

    #[test]
    fn a_snapshot_diff_covers_only_the_tables_that_moved() {
        let mut before = Snapshot::new();
        before.insert("Teams".into(), vec![row(1, &[("TeamID", "1")])]);
        before.insert("Teams_Colours".into(), vec![row(9, &[("ColourID", "9")])]);
        let mut after = Snapshot::new();
        after.insert("Teams".into(), vec![row(1, &[("TeamID", "2")])]);
        after.insert("Teams_Colours".into(), vec![row(9, &[("ColourID", "9")])]);

        let d = diff(&before, &after);
        assert_eq!(d.len(), 1, "an unchanged table must not appear");
        assert_eq!(d[0].table, "Teams");
        let pass = compare(&before, &after);
        assert_eq!(pass.carriable(), 1);
        // The rows travel with the pass, so attribution does not need a second read.
        assert_eq!(pass.rows.get("Teams").map(|r| r.len()), Some(1));
    }

    #[test]
    fn a_pass_supplies_its_own_rows_for_attribution() {
        // The reason `Pass` carries the rows: `to_changes` needs each changed row's team, and a caller that had to
        // remember to pass them separately is how attribution silently stops working.
        let before: Snapshot = [(
            "Parts_Designs".to_string(),
            vec![row(5, &[("DesignID", "5"), ("TeamID", "3"), ("DesignWork", "10")])],
        )]
        .into_iter()
        .collect();
        let after: Snapshot = [(
            "Parts_Designs".to_string(),
            vec![row(5, &[("DesignID", "5"), ("TeamID", "3"), ("DesignWork", "12")])],
        )]
        .into_iter()
        .collect();
        let (changes, uncarriable) = compare(&before, &after).changes();
        assert_eq!(uncarriable, 0);
        assert_eq!(changes.len(), 1);
        assert!(changes[0].set.iter().any(|(n, v)| n == "TeamID" && *v == Value::Int(3)));
        let cfg = Some((coop_auth::Mode::Versus, coop_auth::Player::new(3, 0, true)));
        crate::replicate::plan(&changes[0], cfg).expect("attributed from the pass's own rows");
    }
}
