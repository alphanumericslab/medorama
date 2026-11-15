//! `db-builder check` and `db-builder info`: what a database holds, and what in it a reader
//! cannot rely on.  The check reports, by person key or row key (never by name or MRN):
//! - persons whose patient ID names no issuer, or whose MRN names no authority;
//! - identifiers given to two persons by the same issuer (patient IDs, and MRNs of one authority);
//! - rows whose person or visit key names no person or visit (dangling), or no person at all;
//! - entries whose person is not their visit's;
//! - recordings whose file is missing.

use crate::db;
use rusqlite::Connection;
use std::path::Path;

/// One kind of problem: what, how many, and the first keys.
#[derive(Debug)]
pub struct Problem {
    pub what: String,
    pub count: usize,
    pub examples: Vec<String>,
}

const EXAMPLES: usize = 10;

fn query(c: &Connection, what: &str, sql: &str, out: &mut Vec<Problem>) -> rusqlite::Result<()> {
    let mut st = c.prepare(sql)?;
    let keys: Vec<String> = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?;
    if !keys.is_empty() {
        out.push(Problem { what: what.into(), count: keys.len(), examples: keys.into_iter().take(EXAMPLES).collect() });
    }
    Ok(())
}

/// The problems of the database at `db_path` (none: an empty list).
pub fn check(c: &Connection, db_path: &Path) -> anyhow::Result<Vec<Problem>> {
    let mut out = vec![];
    let checks: &[(&str, &str)] = &[
        (
            "persons whose patient ID names no issuer (patient_id_issuer)",
            "SELECT person_key FROM persons WHERE patient_id <> '' AND patient_id_issuer = '' ORDER BY person_key",
        ),
        (
            "persons whose MRN names no authority (mrn_authority)",
            "SELECT person_key FROM persons WHERE mrn <> '' AND mrn_authority = '' ORDER BY person_key",
        ),
        (
            "patient IDs given to two persons by the same issuer (the persons' keys, joined by +)",
            "SELECT group_concat(person_key, ' + ') FROM persons WHERE patient_id <> ''
             GROUP BY patient_id_issuer, patient_id HAVING COUNT(*) > 1 ORDER BY 1",
        ),
        (
            "MRNs given to two persons by the same authority (the persons' keys, joined by +)",
            "SELECT group_concat(person_key, ' + ') FROM persons WHERE mrn <> ''
             GROUP BY mrn_authority, lower(trim(mrn)) HAVING COUNT(*) > 1 ORDER BY 1",
        ),
        (
            "visits whose person_key names no person",
            "SELECT visit_key FROM visits WHERE person_key IS NOT NULL
             AND person_key NOT IN (SELECT person_key FROM persons) ORDER BY visit_key",
        ),
        ("visits without a person", "SELECT visit_key FROM visits WHERE person_key IS NULL ORDER BY visit_key"),
        (
            "entries whose person_key names no person",
            "SELECT entry_key FROM entries WHERE person_key IS NOT NULL
             AND person_key NOT IN (SELECT person_key FROM persons) ORDER BY entry_key",
        ),
        (
            "entries whose visit_key names no visit",
            "SELECT entry_key FROM entries WHERE visit_key IS NOT NULL
             AND visit_key NOT IN (SELECT visit_key FROM visits) ORDER BY entry_key",
        ),
        ("entries without a person", "SELECT entry_key FROM entries WHERE person_key IS NULL ORDER BY entry_key"),
        (
            "entries whose person is not their visit's",
            "SELECT e.entry_key FROM entries e JOIN visits v ON v.visit_key = e.visit_key
             WHERE e.person_key IS NOT NULL AND v.person_key IS NOT NULL AND e.person_key <> v.person_key
             ORDER BY e.entry_key",
        ),
        (
            "recordings whose person_key names no person",
            "SELECT recording_key FROM recordings WHERE person_key IS NOT NULL
             AND person_key NOT IN (SELECT person_key FROM persons) ORDER BY recording_key",
        ),
        (
            "recordings whose visit_key names no visit",
            "SELECT recording_key FROM recordings WHERE visit_key IS NOT NULL
             AND visit_key NOT IN (SELECT visit_key FROM visits) ORDER BY recording_key",
        ),
        (
            "recordings without a person",
            "SELECT recording_key FROM recordings WHERE person_key IS NULL ORDER BY recording_key",
        ),
    ];
    for (what, sql) in checks {
        query(c, what, sql, &mut out)?;
    }
    // recordings whose file is not there (relative paths from the database's folder)
    let dir = db::folder_of(db_path);
    let mut st = c.prepare("SELECT recording_key, file FROM recordings ORDER BY recording_key")?;
    let missing: Vec<String> = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .filter_map(Result::ok)
        .filter(|(_, f)| !db::resolve_path(f, &dir).exists())
        .map(|(k, f)| format!("{k} ({f})"))
        .collect();
    if !missing.is_empty() {
        out.push(Problem {
            what: "recordings whose file is missing".into(),
            count: missing.len(),
            examples: missing.into_iter().take(EXAMPLES).collect(),
        });
    }
    Ok(out)
}

/// What `info` says: the database's meta, its tables' rows, its sources.
pub fn info(c: &Connection) -> anyhow::Result<String> {
    use std::fmt::Write;
    let mut s = String::new();
    for k in ["schema", "version", "timezone", "created_by"] {
        writeln!(s, "{k:<11} {}", db::meta(c, k)?.unwrap_or_default())?;
    }
    for t in ["persons", "visits", "entries", "recordings", "extras"] {
        let n: i64 = c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))?;
        writeln!(s, "{t:<11} {n} rows")?;
    }
    let mut st = c.prepare("SELECT id, kind, rows, added_at, file FROM sources ORDER BY id")?;
    let rows: Vec<(i64, String, i64, String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    writeln!(s, "sources     {}", rows.len())?;
    for (id, kind, n, at, file) in rows {
        writeln!(s, "  {id:>4}  {kind:<28} {n:>9} rows  {at}  {file}")?;
    }
    Ok(s)
}
