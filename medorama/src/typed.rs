//! A table database's columns as numbers: for each database, a table of its own
//! (`dbcol_<import>`: the row's line, then each column's value as a number, NULL where the cell
//! is not one), written as the rows are imported.  A cohort's criterion on a column
//! (`ecg.QTc > 470`) finds its rows there, by an index on that column made the first time it is
//! asked for, rather than by reading every row's cells; the rows found are then tested as
//! before (the cells as imported stay what is tested and shown).
//!
//! Numbers are read as the criteria read them: the cell's text, trimmed, as Rust reads a
//! number.  The table goes with its database (removed: dropped).  Databases imported before
//! it existed get theirs when the server starts ([`build_missing`]); until then their criteria
//! read the cells.  A database of more than [`MAX_COLUMNS`] columns has none (a row of a table
//! is limited: PostgreSQL's to about a thousand numbers): its criteria read the cells.

use crate::sql::{Connection, Dialect, Value};

/// The typed table of a database.
pub fn name(import: i64) -> String {
    format!("dbcol_{import}")
}

/// A cell as a number, as a criterion reads it (NaN is no number).
pub fn number(cell: &str) -> Option<f64> {
    cell.trim().parse::<f64>().ok().filter(|x| !x.is_nan())
}

/// The most columns a database's typed table has.
pub const MAX_COLUMNS: usize = 500;

/// The typed table of a database of `columns` columns, empty (none for more than
/// [`MAX_COLUMNS`]).
pub fn create(c: &Connection, import: i64, columns: usize) -> crate::sql::Result<()> {
    if columns > MAX_COLUMNS {
        return Ok(());
    }
    let (line, real) = match c.dialect() {
        Dialect::Sqlite => ("INTEGER PRIMARY KEY", "REAL"),
        Dialect::Postgres => ("BIGINT PRIMARY KEY", "DOUBLE PRECISION"),
    };
    let cols: String = (0..columns).map(|i| format!(", c{i} {real}")).collect();
    c.execute_batch_native(&format!("CREATE TABLE {} (line {line}{cols})", name(import)))
}

/// Rows (line, cells) added to the typed table of a database of `columns` columns.
pub fn insert(c: &Connection, import: i64, columns: usize, rows: &[(i64, &[String])]) -> crate::sql::Result<()> {
    if columns == 0 || columns > MAX_COLUMNS {
        return Ok(());
    }
    let values: Vec<Vec<Value>> = rows
        .iter()
        .map(|(line, cells)| {
            std::iter::once(Value::Integer(*line))
                .chain((0..columns).map(|i| cells.get(i).and_then(|v| number(v)).map_or(Value::Null, Value::Real)))
                .collect()
        })
        .collect();
    let cols: String = (0..columns).map(|i| format!(", c{i}")).collect();
    c.insert_rows(&format!("INSERT INTO {} (line{cols})", name(import)), "", &values)
}

/// The typed table's statistics, for PostgreSQL's planner (once it is filled): else it guesses
/// how many rows a criterion keeps, and may read the whole database for a few.
pub fn analyze(c: &Connection, import: i64) -> crate::sql::Result<()> {
    match c.dialect() {
        Dialect::Postgres if exists(c, import)? => c.execute_batch_native(&format!("ANALYZE {}", name(import))),
        Dialect::Postgres => Ok(()),
        Dialect::Sqlite => Ok(()),
    }
}

/// Whether a database has its typed table.
pub fn exists(c: &Connection, import: i64) -> crate::sql::Result<bool> {
    c.table_exists(&name(import))
}

/// The index of a column of a database's typed table, made the first time it is asked for.
/// Never waited for: while an import writes (SQLite's one writer), the column is read without
/// it (still a table of numbers, not the cells).
pub fn index(c: &Connection, import: i64, pos: usize) {
    let t = name(import);
    let _ = c
        .without_waiting(|c| c.execute_batch_native(&format!("CREATE INDEX IF NOT EXISTS {t}_c{pos} ON {t} (c{pos})")));
}

/// The typed tables of removed databases, dropped.
pub fn drop_removed(c: &Connection) -> crate::sql::Result<()> {
    let ids: Vec<i64> = c
        .prepare("SELECT id FROM imports WHERE state = 'removed'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in ids {
        if exists(c, id)? {
            c.execute_batch_native(&format!("DROP TABLE {}", name(id)))?;
        }
    }
    Ok(())
}

/// The typed table of a database imported before typed tables, made from its rows as imported
/// (a few thousand at a time); returns whether it was made.
pub fn build(c: &mut Connection, import: i64) -> anyhow::Result<bool> {
    let tx = c.transaction()?;
    if exists(&tx, import)? {
        return Ok(false);
    }
    let columns: i64 =
        tx.query_row("SELECT COUNT(*) FROM import_columns WHERE import_id = ?1", [import], |r| r.get(0))?;
    if columns as usize > MAX_COLUMNS {
        return Ok(false);
    }
    create(&tx, import, columns as usize)?;
    let mut after = i64::MIN;
    loop {
        let rows: Vec<(i64, Vec<String>)> = tx
            .prepare("SELECT line, cells FROM import_rows WHERE import_id = ?1 AND line > ?2 ORDER BY line LIMIT 5000")?
            .query_map(crate::sql::params![import, after], |r| {
                let cells: String = r.get(1)?;
                Ok((r.get(0)?, serde_json::from_str(&cells).unwrap_or_default()))
            })?
            .collect::<Result<_, _>>()?;
        let Some(last) = rows.last().map(|r| r.0) else { break };
        let refs: Vec<(i64, &[String])> = rows.iter().map(|(l, cells)| (*l, cells.as_slice())).collect();
        insert(&tx, import, columns as usize, &refs)?;
        after = last;
    }
    analyze(&tx, import)?;
    tx.commit()?;
    Ok(true)
}

/// The typed tables of the active table databases that have none, made; returns how many.
pub fn build_missing(c: &mut Connection) -> anyhow::Result<usize> {
    let ids: Vec<i64> = c
        .prepare("SELECT id FROM imports WHERE kind = 'table' AND state = 'active' ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut n = 0;
    for id in ids {
        n += build(c, id)? as usize;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_criteria_read_them() {
        assert_eq!(number(" 470 "), Some(470.0));
        assert_eq!(number("4.2e2"), Some(420.0));
        assert_eq!(number("inf"), Some(f64::INFINITY));
        assert_eq!((number("NaN"), number(""), number("470 ms"), number("sinus")), (None, None, None, None));
    }
}
