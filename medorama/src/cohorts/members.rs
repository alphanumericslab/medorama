//! A cohort's members, in its file of the workspace (`cohorts/c-NNNN.sqlite`, encrypted with
//! the cohort's own key; docs/design/databases-only.md, "Cohorts").
//!
//! Each member records the databases whose answers put it there (`provenance`): removing one of
//! them removes every member it contributed to, in every cohort ([`purge_database`]).  Members
//! added manually record the databases that linked the patient then.  Patients removed manually
//! stay out when the criteria are applied again (`removed`).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::config::Config;
use crate::db::now;
use crate::sql::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS members (
    patient   INTEGER PRIMARY KEY,
    how       TEXT NOT NULL CHECK (how IN ('criteria', 'manual')),
    added_by  INTEGER,
    added_at  REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS provenance (
    patient   INTEGER NOT NULL,
    database  INTEGER NOT NULL,
    PRIMARY KEY (patient, database)
);
CREATE INDEX IF NOT EXISTS provenance_database ON provenance(database);
CREATE TABLE IF NOT EXISTS removed (
    patient     INTEGER PRIMARY KEY,
    removed_by  INTEGER,
    removed_at  REAL NOT NULL
);
";

pub(crate) fn open(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<rusqlite::Connection> {
    let w = crate::workspace::open_cohort_file(cfg, c, cohort)?;
    w.execute_batch(SCHEMA)?;
    Ok(w)
}

/// The members: patient → (how, added at).
pub(crate) fn how(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<HashMap<i64, (String, f64)>> {
    let w = open(cfg, c, cohort)?;
    let mut st = w.prepare("SELECT patient, how, added_at FROM members")?;
    let v = st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?;
    Ok(v)
}

pub(crate) fn ids(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<HashSet<i64>> {
    let w = open(cfg, c, cohort)?;
    let mut st = w.prepare("SELECT patient FROM members")?;
    let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

pub(crate) fn count(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<i64> {
    let w = open(cfg, c, cohort)?;
    Ok(w.query_row("SELECT COUNT(*) FROM members", [], |r| r.get(0))?)
}

/// The members added manually.
pub(crate) fn manual(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<Vec<i64>> {
    let w = open(cfg, c, cohort)?;
    let mut st = w.prepare("SELECT patient FROM members WHERE how = 'manual' ORDER BY patient")?;
    let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// The patients removed manually.
pub(crate) fn removed(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<HashSet<i64>> {
    let w = open(cfg, c, cohort)?;
    let mut st = w.prepare("SELECT patient FROM removed")?;
    let v = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Add members not in yet, each with how it came and the databases it came from; patients
/// removed manually before are no longer kept out.  Returns those added.
pub(crate) fn add(
    cfg: &Config,
    c: &Connection,
    cohort: i64,
    rows: &[(i64, &str, BTreeSet<i64>)],
    by: Option<i64>,
) -> anyhow::Result<Vec<i64>> {
    let mut w = open(cfg, c, cohort)?;
    let tx = w.transaction()?;
    let t = now();
    let mut added = vec![];
    {
        let mut ins =
            tx.prepare("INSERT OR IGNORE INTO members (patient, how, added_by, added_at) VALUES (?1, ?2, ?3, ?4)")?;
        let mut from = tx.prepare("INSERT OR IGNORE INTO provenance (patient, database) VALUES (?1, ?2)")?;
        let mut back = tx.prepare("DELETE FROM removed WHERE patient = ?1")?;
        for (p, how, dbs) in rows {
            if ins.execute(rusqlite::params![p, how, by, t])? > 0 {
                added.push(*p);
                back.execute([p])?;
            }
            for d in dbs {
                from.execute([p, d])?;
            }
        }
    }
    tx.commit()?;
    Ok(added)
}

/// Remove members; `keep_out`: they stay out when the criteria are applied again.
pub(crate) fn remove(
    cfg: &Config,
    c: &Connection,
    cohort: i64,
    patients: &[i64],
    keep_out: bool,
    by: Option<i64>,
) -> anyhow::Result<usize> {
    let mut w = open(cfg, c, cohort)?;
    let tx = w.transaction()?;
    let mut n = 0;
    for p in patients {
        n += tx.execute("DELETE FROM members WHERE patient = ?1", [p])?;
        tx.execute("DELETE FROM provenance WHERE patient = ?1", [p])?;
        if keep_out {
            tx.execute(
                "INSERT INTO removed (patient, removed_by, removed_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (patient) DO UPDATE SET removed_by = excluded.removed_by, removed_at = excluded.removed_at",
                rusqlite::params![p, by, now()],
            )?;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Copy a cohort's members, their provenance and its removed patients into another (a clone).
pub(crate) fn copy(cfg: &Config, c: &Connection, from: i64, to: i64) -> anyhow::Result<()> {
    let src = open(cfg, c, from)?;
    let mut dst = open(cfg, c, to)?;
    let tx = dst.transaction()?;
    {
        let mut st = src.prepare("SELECT patient, how, added_by, added_at FROM members")?;
        let mut ins =
            tx.prepare("INSERT OR IGNORE INTO members (patient, how, added_by, added_at) VALUES (?1, ?2, ?3, ?4)")?;
        for r in st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, f64>(3)?))
        })? {
            let (p, h, b, t) = r?;
            ins.execute(rusqlite::params![p, h, b, t])?;
        }
        let mut st = src.prepare("SELECT patient, database FROM provenance")?;
        let mut ins = tx.prepare("INSERT OR IGNORE INTO provenance (patient, database) VALUES (?1, ?2)")?;
        for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (p, d) = r?;
            ins.execute([p, d])?;
        }
        let mut st = src.prepare("SELECT patient, removed_by, removed_at FROM removed")?;
        let mut ins =
            tx.prepare("INSERT OR IGNORE INTO removed (patient, removed_by, removed_at) VALUES (?1, ?2, ?3)")?;
        for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, f64>(2)?)))? {
            let (p, b, t) = r?;
            ins.execute(rusqlite::params![p, b, t])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Delete a cohort's file and shred its key (the cohort was deleted).
pub(crate) fn delete(cfg: &Config, c: &Connection, cohort: i64) -> anyhow::Result<()> {
    crate::workspace::shred(c, &crate::workspace::cohort_owner(cohort))?;
    crate::workspace::remove_file(cfg, &crate::workspace::cohort_file(cfg, cohort))
}

/// Remove, from every cohort, the members a database (or what was derived from it)
/// contributed to (docs/design/databases-only.md, "Removing a database", step 4).  Also the
/// patients removed manually who were known only through it: their number goes with it.
pub fn purge_database(cfg: &Config, c: &Connection, database: i64, derived: &[i64]) -> anyhow::Result<usize> {
    let ids: Vec<i64> = c.prepare("SELECT id FROM cohorts")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    // derived databases are recorded under their own number, offset so as not to meet a database's
    let mut gone: Vec<i64> = vec![database];
    gone.extend(derived.iter().map(|d| -d));
    let marks = gone.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ");
    let mut n = 0;
    for id in ids {
        if !crate::workspace::cohort_file(cfg, id).is_file() {
            continue;
        }
        let mut w = open(cfg, c, id)?;
        let tx = w.transaction()?;
        n += tx.execute(
            &format!(
                "DELETE FROM members WHERE patient IN (SELECT patient FROM provenance WHERE database IN ({marks}))"
            ),
            [],
        )?;
        tx.execute(
            &format!(
                "DELETE FROM provenance WHERE patient IN (SELECT patient FROM provenance WHERE database IN ({marks}))"
            ),
            [],
        )?;
        tx.commit()?;
    }
    Ok(n)
}

/// After a removal: patients no longer linked by any database leave every cohort's list of
/// patients removed manually too (their numbers are gone).
pub fn forget_unlinked(cfg: &Config, c: &Connection) -> anyhow::Result<()> {
    let ids: Vec<i64> = c.prepare("SELECT id FROM cohorts")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let linked: HashSet<i64> =
        c.prepare("SELECT id FROM patients")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for id in ids {
        if !crate::workspace::cohort_file(cfg, id).is_file() {
            continue;
        }
        let w = open(cfg, c, id)?;
        for table in ["members", "removed", "provenance"] {
            let mut st = w.prepare(&format!("SELECT DISTINCT patient FROM {table}"))?;
            let ps: Vec<i64> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for p in ps.into_iter().filter(|p| !linked.contains(p)) {
                w.execute(&format!("DELETE FROM {table} WHERE patient = ?1"), [p])?;
            }
        }
    }
    Ok(())
}
