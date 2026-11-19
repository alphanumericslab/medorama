//! Each connected database's variables (docs/design/connected-databases.md, "The catalog"):
//! what it holds, read when it is connected and again when its data version changes, and kept
//! in its workspace folder (`variables.sqlite`).
//!
//! A variable is a (category, name, vocabulary, code, unit) with whether it has numbers: its
//! names and units only.  They come from the database's `variables` view when it has one (no
//! entry read), else from one grouped query over its entries.  How many entries and persons
//! hold each one is counted only when asked (`count`, the Databases tab's **Count now**), and
//! a count below [`SMALL`] is shown as "< 11".  The planner sends a criterion only to the
//! databases whose variables hold it; the Cohort tab completes names from them, offline
//! databases included.

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
    entries     INTEGER,                      -- NULL: not counted
    persons     INTEGER,
    numeric     INTEGER NOT NULL,
    PRIMARY KEY (category, name, vocabulary, code, unit)
);
CREATE INDEX IF NOT EXISTS variables_name ON variables(name);
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Counts below this are shown as "< 11", never as they are (small cells).
pub const SMALL: i64 = 11;

/// One variable of a database.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Variable {
    pub category: String,
    pub name: String,
    pub vocabulary: String,
    pub code: String,
    pub unit: String,
    /// How many entries and persons hold it, once counted; never sent as they are ([`shown`]).
    #[serde(skip)]
    pub entries: Option<i64>,
    #[serde(skip)]
    pub persons: Option<i64>,
    pub numeric: bool,
}

/// A count as it is shown: none when not counted, "< 11" when small, else the number.
pub fn shown(n: Option<i64>) -> Option<String> {
    n.map(|n| if n > 0 && n < SMALL { format!("< {SMALL}") } else { n.to_string() })
}

/// A variable as the API and the page show it: its counts as [`shown`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShownVariable {
    #[serde(flatten)]
    pub variable: Variable,
    pub entries: Option<String>,
    pub persons: Option<String>,
}

impl From<Variable> for ShownVariable {
    fn from(v: Variable) -> Self {
        let (entries, persons) = (shown(v.entries), shown(v.persons));
        ShownVariable { variable: v, entries, persons }
    }
}

fn open(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<rusqlite::Connection> {
    let w = crate::workspace::open_database_file(cfg, c, database, "variables.sqlite")?;
    // a table of 0.2, whose counts were required: made again (it is read again at the next check)
    let old: bool = w
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('variables') WHERE name = 'entries' AND \"notnull\" = 1",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)?;
    if old {
        w.execute_batch("DROP TABLE variables")?;
    }
    w.execute_batch(SCHEMA)?;
    Ok(w)
}

fn int(v: Value) -> i64 {
    match v {
        Value::Integer(x) => x,
        Value::Real(x) => x as i64,
        other => text_of(&other).parse().unwrap_or(0),
    }
}

/// Read a database's variables now (their names and units) and keep them, with the data version
/// they were read at and the categories of its recordings.  Notes are named by their kind, never
/// by their text.
pub fn read(cfg: &Config, c: &Connection, s: &Source, version: &str) -> anyhow::Result<usize> {
    let views = s.views();
    let db = s.db()?;
    let sql = if !views.variables.trim().is_empty() {
        format!("SELECT {} FROM ({}) v", crate::sources::VARIABLE_COLS, views.variables)
    } else {
        format!(
            "SELECT category, name, vocabulary, code, unit, MAX(CASE WHEN value_num IS NOT NULL THEN 1 ELSE 0 END)
             FROM ({}) e GROUP BY category, name, vocabulary, code, unit",
            views.entries
        )
    };
    let mut st = db.prepare_native(&sql)?;
    let rows: Vec<Variable> = st
        .query_map([], |r| {
            let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?).trim().to_string()) };
            let (cat, name, voc, code) = (t(0)?, t(1)?, t(2)?, t(3)?);
            Ok(Variable {
                category: crate::categories::of_entry(&cat, &name, &voc, &code),
                name,
                vocabulary: voc,
                code,
                unit: t(4)?,
                entries: None,
                persons: None,
                numeric: int(r.get::<_, Value>(5)?) != 0,
            })
        })
        .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?;
    drop(st);
    // the signals its recordings hold (`has icu-bp-ART`), by name and by group: ICU variables of
    // the vocabulary `signal`, with no values
    let mut rows = rows;
    rows.extend(s.signal_names()?.into_iter().map(|name| Variable {
        category: "icu".into(),
        name,
        vocabulary: SIGNAL.into(),
        code: String::new(),
        unit: String::new(),
        entries: None,
        persons: None,
        numeric: false,
    }));
    let recording_categories = s.recording_categories().map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?;
    let mut w = open(cfg, c, s.id)?;
    let tx = w.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM variables", [])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO variables (category, name, vocabulary, code, unit, numeric) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO UPDATE SET numeric = MAX(numeric, excluded.numeric)",
        )?;
        for x in &rows {
            ins.execute(rusqlite::params![x.category, x.name, x.vocabulary, x.code, x.unit, x.numeric])?;
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('version', ?1), ('read_at', ?2), ('recording_categories', ?3)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        rusqlite::params![version, now().to_string(), recording_categories.join(" ")],
    )?;
    tx.execute("DELETE FROM meta WHERE key = 'counted_at'", [])?;
    tx.commit()?;
    c.execute("UPDATE databases SET variables_version = ?1 WHERE id = ?2", params![version, s.id])?;
    Ok(rows.len())
}

/// Count, for each variable of a database, the entries and persons that hold it (one grouped
/// query over its entries: asked for, never done when connecting): how many variables.
pub fn count(cfg: &Config, c: &Connection, s: &Source) -> anyhow::Result<usize> {
    let v = s.views().entries;
    let db = s.db()?;
    let mut st = db.prepare_native(&format!(
        "SELECT category, name, vocabulary, code, unit, COUNT(*), COUNT(DISTINCT person_key)
         FROM ({v}) e GROUP BY category, name, vocabulary, code, unit"
    ))?;
    let rows: Vec<(String, String, String, String, String, i64, i64)> = st
        .query_map([], |r| {
            let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?).trim().to_string()) };
            let (cat, name, voc, code) = (t(0)?, t(1)?, t(2)?, t(3)?);
            Ok((
                crate::categories::of_entry(&cat, &name, &voc, &code),
                name,
                voc,
                code,
                t(4)?,
                int(r.get::<_, Value>(5)?),
                int(r.get::<_, Value>(6)?),
            ))
        })
        .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?;
    drop(st);
    let signals = s.signal_counts()?;
    let mut w = open(cfg, c, s.id)?;
    let tx = w.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("UPDATE variables SET entries = 0, persons = 0", [])?;
    {
        // the variables read before, counted (a category two names share is summed)
        let mut up = tx.prepare(
            "UPDATE variables SET entries = entries + ?6, persons = MAX(persons, ?7)
             WHERE category = ?1 AND name = ?2 AND vocabulary = ?3 AND code = ?4 AND unit = ?5",
        )?;
        for (cat, name, voc, code, unit, e, p) in &rows {
            up.execute(rusqlite::params![cat, name, voc, code, unit, e, p])?;
        }
        // a signal: the recordings that hold it, and their persons
        for (name, e, p) in &signals {
            up.execute(rusqlite::params!["icu", name, SIGNAL, "", "", e, p])?;
        }
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('counted_at', ?1) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [now().to_string()],
    )?;
    tx.commit()?;
    Ok(rows.len())
}

/// When a database's variables were last counted (none: never, or not since they were read).
pub fn counted_at(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Option<f64>> {
    let w = open(cfg, c, database)?;
    let v: Option<String> = w.query_row("SELECT value FROM meta WHERE key = 'counted_at'", [], |r| r.get(0)).ok();
    Ok(v.and_then(|t| t.parse().ok()))
}

/// The categories of a database's recordings, as read with its variables.
pub fn recording_categories(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Vec<String>> {
    let w = open(cfg, c, database)?;
    let v: Option<String> =
        w.query_row("SELECT value FROM meta WHERE key = 'recording_categories'", [], |r| r.get(0)).ok();
    Ok(v.unwrap_or_default().split(' ').filter(|x| !x.is_empty()).map(String::from).collect())
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
                numeric: r.get::<_, i64>(7)? != 0,
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

/// The vocabulary of the signals a database's recordings hold (its `signals` view), kept with its
/// variables: `has icu-bp-ART` finds the patients with a recording that holds one.
pub const SIGNAL: &str = "signal";

/// The signal names (and group names) a database's recordings hold, as kept.
pub fn signals(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Vec<String>> {
    Ok(of(cfg, c, database)?.into_iter().filter(|x| x.vocabulary == SIGNAL).map(|x| x.name).collect())
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

/// Whether the variables were read at this version of the data (the database's version
/// read now: a criterion pushes its names and units down only then, its catalog complete).
pub fn read_at(c: &Connection, database: i64, version: &str) -> anyhow::Result<bool> {
    let v: String = c.query_row("SELECT variables_version FROM databases WHERE id = ?1", [database], |r| r.get(0))?;
    Ok(!version.is_empty() && v == version)
}

/// Whether the variables were read at the database's current version.
pub fn fresh(c: &Connection, database: i64) -> anyhow::Result<bool> {
    Ok(c.query_row(
        "SELECT data_version = variables_version AND variables_version != '' FROM databases WHERE id = ?1",
        [database],
        |r| r.get::<_, bool>(0),
    )?)
}
