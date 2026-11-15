//! Each connected database's variables (docs/design/databases-only.md, "Variables"): what it
//! holds, read when it is connected and again when its data version changes, and kept in its
//! workspace folder (`variables.sqlite`).
//!
//! A variable is a (category, name, vocabulary, code, unit) with how many entries and persons
//! hold it and whether it has numbers.  The planner sends a criterion only to the databases
//! whose variables hold it; the Cohort tab completes names from them, offline databases
//! included.

use crate::config::Config;
use crate::db::now;
use crate::sources::{text_of, Source};
use crate::sql::{params, Connection, Value};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS variables (
    category    TEXT NOT NULL,
    name        TEXT NOT NULL,
    vocabulary  TEXT NOT NULL DEFAULT '',
    code        TEXT NOT NULL DEFAULT '',
    unit        TEXT NOT NULL DEFAULT '',
    entries     INTEGER NOT NULL,
    persons     INTEGER NOT NULL,
    numeric     INTEGER NOT NULL,
    PRIMARY KEY (category, name, vocabulary, code, unit)
);
CREATE INDEX IF NOT EXISTS variables_name ON variables(name);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// One variable of a database.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Variable {
    pub category: String,
    pub name: String,
    pub vocabulary: String,
    pub code: String,
    pub unit: String,
    pub entries: i64,
    pub persons: i64,
    pub numeric: bool,
}

fn open(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<rusqlite::Connection> {
    let w = crate::workspace::open_database_file(cfg, c, database, "variables.sqlite")?;
    w.execute_batch(SCHEMA)?;
    Ok(w)
}

/// Read a database's variables now and keep them, with the data version they were read at.
/// Notes are counted, never named by their text.
pub fn read(cfg: &Config, c: &Connection, s: &Source, version: &str) -> anyhow::Result<usize> {
    let v = s.views().entries;
    let db = s.db()?;
    let mut st = db.prepare_native(&format!(
        "SELECT category, name, vocabulary, code, unit, COUNT(*), COUNT(DISTINCT person_key),
                MAX(CASE WHEN value_num IS NOT NULL THEN 1 ELSE 0 END)
         FROM ({v}) e GROUP BY category, name, vocabulary, code, unit"
    ))?;
    let rows: Vec<Variable> = st
        .query_map([], |r| {
            let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?).trim().to_string()) };
            let n = |i: usize| -> crate::sql::Result<i64> {
                Ok(match r.get::<_, Value>(i)? {
                    Value::Integer(x) => x,
                    Value::Real(x) => x as i64,
                    other => text_of(&other).parse().unwrap_or(0),
                })
            };
            let (cat, name, voc, code) = (t(0)?, t(1)?, t(2)?, t(3)?);
            Ok(Variable {
                category: crate::categories::of_entry(&cat, &name, &voc, &code),
                name,
                vocabulary: voc,
                code,
                unit: t(4)?,
                entries: n(5)?,
                persons: n(6)?,
                numeric: n(7)? != 0,
            })
        })
        .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?;
    let mut w = open(cfg, c, s.id)?;
    let tx = w.transaction()?;
    tx.execute("DELETE FROM variables", [])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO variables (category, name, vocabulary, code, unit, entries, persons, numeric)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT DO UPDATE SET entries = entries + excluded.entries, persons = MAX(persons, excluded.persons),
                numeric = MAX(numeric, excluded.numeric)",
        )?;
        for x in &rows {
            // a note's name is its kind ("Discharge summary"), never its text
            ins.execute(rusqlite::params![
                x.category,
                x.name,
                x.vocabulary,
                x.code,
                x.unit,
                x.entries,
                x.persons,
                x.numeric
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('version', ?1), ('read_at', ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        rusqlite::params![version, now().to_string()],
    )?;
    tx.commit()?;
    c.execute("UPDATE databases SET variables_version = ?1 WHERE id = ?2", params![version, s.id])?;
    Ok(rows.len())
}

/// A database's variables, as kept.
pub fn of(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Vec<Variable>> {
    let w = open(cfg, c, database)?;
    let mut st = w.prepare(
        "SELECT category, name, vocabulary, code, unit, entries, persons, numeric FROM variables
         ORDER BY category, name, unit",
    )?;
    let v = st
        .query_map([], |r| {
            Ok(Variable {
                category: r.get(0)?,
                name: r.get(1)?,
                vocabulary: r.get(2)?,
                code: r.get(3)?,
                unit: r.get(4)?,
                entries: r.get(5)?,
                persons: r.get(6)?,
                numeric: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// The names of what a database holds, by category, with whether they have numbers: as
/// `Source::names` gives them, from the variables kept (a database offline still has them).
pub fn names(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Vec<(String, String, bool)>> {
    let mut v: Vec<(String, String, bool)> = of(cfg, c, database)?
        .into_iter()
        .filter(|x| x.category != "note")
        .map(|x| (x.category, x.name, x.numeric))
        .collect();
    // the results of actions run on its recordings (`hrv.rmssd`)
    v.extend(crate::derived::result_names(cfg, c, database)?.into_iter().map(|n| ("result".to_string(), n, true)));
    v.sort();
    v.dedup_by(|a, b| {
        if a.0 == b.0 && a.1 == b.1 {
            b.2 |= a.2;
            true
        } else {
            false
        }
    });
    Ok(v)
}

/// Whether a database holds a variable of this name (any case; `contains`: a part of it).
pub fn holds(cfg: &Config, c: &Connection, database: i64, name: &str, contains: bool) -> anyhow::Result<bool> {
    if !crate::derived::results_like(cfg, c, database, name, !contains)?.is_empty() {
        return Ok(true);
    }
    let w = open(cfg, c, database)?;
    let n = name.trim().to_lowercase();
    let (sql, arg) = if contains {
        ("SELECT EXISTS (SELECT 1 FROM variables WHERE lower(name) LIKE ?1 ESCAPE '\\')", format!("%{}%", like(&n)))
    } else {
        ("SELECT EXISTS (SELECT 1 FROM variables WHERE lower(name) = ?1)", n)
    };
    Ok(w.query_row(sql, [arg], |r| r.get(0))?)
}

/// Whether a database holds entries of a category.
pub fn holds_category(cfg: &Config, c: &Connection, database: i64, category: &str) -> anyhow::Result<bool> {
    let w = open(cfg, c, database)?;
    Ok(w.query_row("SELECT EXISTS (SELECT 1 FROM variables WHERE category = ?1)", [category], |r| r.get(0))?)
}

fn like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Whether the variables were read at the database's current version.
pub fn fresh(c: &Connection, database: i64) -> anyhow::Result<bool> {
    Ok(c.query_row(
        "SELECT data_version = variables_version AND variables_version != '' FROM databases WHERE id = ?1",
        [database],
        |r| r.get::<_, bool>(0),
    )?)
}
