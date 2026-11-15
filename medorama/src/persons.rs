//! The patient index (docs/design/databases-only.md, "Identity: the patient index"): each
//! connected database's persons, linked to Medorama's patient numbers.
//!
//! A person names themselves by (issuer, identifier) pairs: their patient ID under its issuer,
//! their MRN under its authority.  Each pair is assumed to name one patient (a requirement on the
//! databases: Medorama does not resolve identity).  Linking keeps, in the control store, a keyed
//! hash of each pair with the patient number it maps to (`patient_keys`): two databases' persons
//! with an equal pair are one patient, and no pair can be read back.  What the database says of
//! the person (identifiers, names, birth date, sex) is kept in its workspace folder,
//! `persons.sqlite`, encrypted with its key: the privacy gate's dictionary and the patient list
//! read it there.  A person whose pairs lead to two patients is a conflict: listed, never linked.

use std::collections::{BTreeSet, HashMap, HashSet};

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::config::Config;
use crate::db::{audit, now};
use crate::sources::{Person, Source};
use crate::sql::{params, Connection};
use crate::users::User;

/// What linking did.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Linked {
    pub linked: usize,
    pub new_patients: usize,
    pub conflicts: usize,
    pub removed: usize,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS persons (
    person_key         TEXT PRIMARY KEY,
    patient            INTEGER NOT NULL,
    patient_id         TEXT NOT NULL DEFAULT '',
    patient_id_issuer  TEXT NOT NULL DEFAULT '',
    mrn                TEXT NOT NULL DEFAULT '',
    mrn_authority      TEXT NOT NULL DEFAULT '',
    last_name          TEXT NOT NULL DEFAULT '',
    first_name         TEXT NOT NULL DEFAULT '',
    birth_date         TEXT NOT NULL DEFAULT '',
    sex                TEXT NOT NULL DEFAULT '',
    hashes             TEXT NOT NULL DEFAULT '',   -- its pairs' hashes, apart by spaces (patient_keys)
    linked_at          REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS persons_patient ON persons(patient);
-- what the patient list shows of each person, read at each refresh (summarize)
CREATE TABLE IF NOT EXISTS summary (
    person_key  TEXT PRIMARY KEY,
    visits      INTEGER NOT NULL DEFAULT 0,
    latest      REAL,
    entries     INTEGER NOT NULL DEFAULT 0,
    kinds       TEXT NOT NULL DEFAULT ''      -- category keys, apart by spaces
);
CREATE TABLE IF NOT EXISTS conflicts (
    person_key  TEXT PRIMARY KEY,
    detail      TEXT NOT NULL,
    found_at    REAL NOT NULL
);
";

/// A database's `persons.sqlite`, opened with its key (made when first opened).
pub fn open(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<rusqlite::Connection> {
    let w = crate::workspace::open_database_file(cfg, c, database, "persons.sqlite")?;
    w.execute_batch(SCHEMA)?;
    Ok(w)
}

/// The keyed hash of an (issuer, identifier) pair: equal pairs hash equal, and none can be
/// read back without the gate's secret.  Issuers and identifiers compare as trimmed text in any
/// case (`x123` is `X123`, as db-builder matches MRNs); a database's own person key (issuer
/// `key:…`) exactly.
pub fn hash(secret: &[u8], issuer: &str, identifier: &str) -> String {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("any key length");
    let issuer = issuer.trim().to_lowercase();
    let identifier =
        if issuer.starts_with("key:") { identifier.trim().to_string() } else { identifier.trim().to_lowercase() };
    m.update(issuer.as_bytes());
    m.update(&[0]);
    m.update(identifier.as_bytes());
    m.finalize().into_bytes().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// A person's (issuer, identifier) pairs; a person with none is known by the database and
/// their key alone (they link only to themselves).
pub fn pairs(db: &Source, p: &Person) -> Vec<(String, String)> {
    let mut v = vec![];
    if !p.patient_id.trim().is_empty() {
        v.push((p.patient_id_issuer.clone(), p.patient_id.trim().to_string()));
    }
    if !p.mrn.trim().is_empty() {
        v.push((format!("mrn:{}", p.mrn_authority), p.mrn.trim().to_string()));
    }
    if v.is_empty() {
        v.push((format!("key:db:{}", db.id), p.key.clone()));
    }
    v
}

/// Link these persons of `s`: each to the patient its pairs name in any database, else to a
/// new patient.  Persons linked before are linked again (their identifiers as they are now).
pub fn link(
    cfg: &Config,
    c: &Connection,
    s: &Source,
    persons: &[Person],
    user: Option<&User>,
) -> anyhow::Result<Linked> {
    let secret = crate::gate::secret(c)?;
    let w = open(cfg, c, s.id)?;
    let mut rep = Linked::default();
    let own = c.is_autocommit();
    // one transaction at a time decides patients: on SQLite the write lock taken at once (two
    // deferred transactions would both decide, and the second be refused); on PostgreSQL the lock
    let tx = if own {
        Some(c.unchecked_transaction_with_behavior(crate::sql::TransactionBehavior::Immediate)?)
    } else {
        None
    };
    c.lock_in_transaction(crate::sql::Lock::Identity)?;
    let wt = w.unchecked_transaction()?;
    for p in persons {
        let hashes: Vec<String> = pairs(s, p).iter().map(|(i, v)| hash(&secret, i, v)).collect();
        let found: BTreeSet<i64> = c
            .query_in("SELECT patient FROM patient_keys WHERE hash IN ({})", &hashes, |r| r.get::<_, i64>(0))?
            .into_iter()
            .collect();
        let patient = match found.len() {
            0 => {
                let id = c.insert("INSERT INTO patients (created_at) VALUES (?1)", params![now()])?;
                crate::pseudonyms::assign_patient(c, id)?;
                rep.new_patients += 1;
                id
            }
            1 => *found.iter().next().unwrap(),
            _ => {
                w.execute(
                    "INSERT INTO conflicts (person_key, detail, found_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT (person_key) DO UPDATE SET detail = excluded.detail, found_at = excluded.found_at",
                    rusqlite::params![p.key, format!("its identifiers name {} different patients", found.len()), now()],
                )?;
                rep.conflicts += 1;
                continue;
            }
        };
        w.execute("DELETE FROM conflicts WHERE person_key = ?1", [&p.key])?;
        // the person's former pairs (an identifier changed in the database) go
        let before: String =
            w.query_row("SELECT hashes FROM persons WHERE person_key = ?1", [&p.key], |r| r.get(0)).unwrap_or_default();
        for old in before.split(' ').filter(|h| !h.is_empty() && !hashes.iter().any(|n| n == h)) {
            c.execute("DELETE FROM patient_keys WHERE database_id = ?1 AND hash = ?2", params![s.id, old])?;
        }
        for h in &hashes {
            c.execute(
                "INSERT INTO patient_keys (hash, database_id, patient) VALUES (?1, ?2, ?3)
                 ON CONFLICT (hash, database_id) DO UPDATE SET patient = excluded.patient",
                params![h, s.id, patient],
            )?;
        }
        w.execute(
            "INSERT INTO persons (person_key, patient, patient_id, patient_id_issuer, mrn, mrn_authority, last_name,
                                  first_name, birth_date, sex, hashes, linked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (person_key) DO UPDATE SET patient = excluded.patient, patient_id = excluded.patient_id,
                patient_id_issuer = excluded.patient_id_issuer, mrn = excluded.mrn,
                mrn_authority = excluded.mrn_authority, last_name = excluded.last_name,
                first_name = excluded.first_name, birth_date = excluded.birth_date, sex = excluded.sex,
                hashes = excluded.hashes",
            rusqlite::params![
                p.key,
                patient,
                p.patient_id,
                p.patient_id_issuer,
                p.mrn,
                p.mrn_authority,
                p.last_name,
                p.first_name,
                p.birth_date,
                normal_sex(&p.sex),
                hashes.join(" "),
                now()
            ],
        )?;
        rep.linked += 1;
    }
    wt.commit()?;
    if let Some(tx) = tx {
        tx.commit()?;
    }
    if rep.linked + rep.conflicts > 0 {
        audit(
            c,
            user,
            "link",
            &s.name,
            &format!("{} linked ({} new patients), {} conflicts", rep.linked, rep.new_patients, rep.conflicts),
        );
    }
    Ok(rep)
}

fn normal_sex(s: &str) -> &'static str {
    match s.trim().to_ascii_uppercase().as_str() {
        "F" | "FEMALE" | "W" => "F",
        "M" | "MALE" => "M",
        _ => "",
    }
}

/// Link every person of `s`, and forget the persons it no longer holds (`medorama databases
/// refresh`; connecting).
pub fn sync(cfg: &Config, c: &Connection, s: &Source, user: Option<&User>) -> anyhow::Result<Linked> {
    let all = s.all_persons()?;
    let mut rep = Linked::default();
    for chunk in all.chunks(2000) {
        let r = link(cfg, c, s, chunk, user)?;
        rep.linked += r.linked;
        rep.new_patients += r.new_patients;
        rep.conflicts += r.conflicts;
    }
    let now_keys: std::collections::HashSet<&str> = all.iter().map(|p| p.key.as_str()).collect();
    let w = open(cfg, c, s.id)?;
    let gone: Vec<(String, String)> = {
        let mut st = w.prepare("SELECT person_key, hashes FROM persons")?;
        let v = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        v.into_iter().filter(|(k, _)| !now_keys.contains(k.as_str())).collect()
    };
    if !gone.is_empty() {
        for (k, _) in &gone {
            w.execute("DELETE FROM persons WHERE person_key = ?1", [k])?;
        }
        // the hashes the persons left keep (another person of the database with the same pair
        // keeps its link), read once: not one scan per hash gone
        let kept: HashSet<String> = {
            let mut st = w.prepare("SELECT hashes FROM persons")?;
            let rows: Vec<String> = st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?;
            rows.iter().flat_map(|h| h.split(' ')).filter(|h| !h.is_empty()).map(str::to_string).collect()
        };
        for (_, hashes) in &gone {
            for h in hashes.split(' ').filter(|h| !h.is_empty() && !kept.contains(*h)) {
                c.execute("DELETE FROM patient_keys WHERE database_id = ?1 AND hash = ?2", params![s.id, h])?;
            }
        }
    }
    {
        let mut st = w.prepare("SELECT person_key FROM conflicts")?;
        let stale: Vec<String> = st
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|k| !now_keys.contains(k.as_str()))
            .collect();
        for k in stale {
            w.execute("DELETE FROM conflicts WHERE person_key = ?1", [k])?;
        }
    }
    rep.removed = gone.len();
    if rep.removed > 0 {
        sweep(c)?;
    }
    summarize(cfg, c, s)?;
    Ok(rep)
}

/// Read what the patient list shows of each person of `s`: their visits, latest time, entries
/// and the categories they have (three grouped queries, at each refresh), kept in its
/// persons file.
pub fn summarize(cfg: &Config, c: &Connection, s: &Source) -> anyhow::Result<()> {
    use crate::sources::{text_of, time_of};
    use crate::sql::Value;
    let zone = s.zone(&cfg.timezone);
    let v = s.views();
    let db = s.db()?;
    let mut sum: HashMap<String, (i64, Option<f64>, i64, BTreeSet<String>)> = HashMap::new();
    let num = |x: &Value| match x {
        Value::Integer(i) => *i,
        Value::Real(r) => *r as i64,
        other => text_of(other).parse().unwrap_or(0),
    };
    if !v.visits.trim().is_empty() {
        let mut st = db.prepare_native(&format!(
            "SELECT person_key, COUNT(*), MAX(start_time), MAX(CASE WHEN icu = 1 THEN 1 ELSE 0 END) FROM ({}) v GROUP BY person_key",
            v.visits
        ))?;
        let rows: Vec<(String, i64, Option<f64>, bool)> = st
            .query_map([], |r| {
                Ok((
                    text_of(&r.get::<_, Value>(0)?),
                    num(&r.get::<_, Value>(1)?),
                    time_of(&r.get::<_, Value>(2)?, zone),
                    num(&r.get::<_, Value>(3)?) != 0,
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (k, n, latest, icu) in rows {
            let e = sum.entry(k).or_default();
            e.0 = n;
            e.1 = latest;
            if icu {
                e.3.insert("icu".into());
            }
        }
    }
    let mut st = db.prepare_native(&format!(
        "SELECT person_key, category, COUNT(*), MAX(time) FROM ({}) e GROUP BY person_key, category",
        v.entries
    ))?;
    let rows: Vec<(String, String, i64, Option<f64>)> = st
        .query_map([], |r| {
            Ok((
                text_of(&r.get::<_, Value>(0)?),
                text_of(&r.get::<_, Value>(1)?),
                num(&r.get::<_, Value>(2)?),
                time_of(&r.get::<_, Value>(3)?, zone),
            ))
        })?
        .collect::<Result<_, _>>()?;
    drop(st);
    for (k, cat, n, t) in rows {
        let e = sum.entry(k).or_default();
        e.2 += n;
        e.1 = match (e.1, t) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        let cat = crate::categories::of_entry(&cat, "", "", "");
        if cat != "allergy" {
            e.3.insert(cat);
        }
    }
    let mut w = open(cfg, c, s.id)?;
    let tx = w.transaction()?;
    tx.execute("DELETE FROM summary", [])?;
    {
        let mut ins =
            tx.prepare("INSERT INTO summary (person_key, visits, latest, entries, kinds) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for (k, (visits, latest, entries, kinds)) in &sum {
            ins.execute(rusqlite::params![
                k,
                visits,
                latest,
                entries,
                kinds.iter().cloned().collect::<Vec<_>>().join(" ")
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// What the patient list shows of a database's persons: person key → (visits, latest,
/// entries, categories).
pub fn summaries(
    cfg: &Config,
    c: &Connection,
    database: i64,
) -> anyhow::Result<HashMap<String, (i64, Option<f64>, i64, Vec<String>)>> {
    let w = open(cfg, c, database)?;
    let mut st = w.prepare("SELECT person_key, visits, latest, entries, kinds FROM summary")?;
    let v = st
        .query_map([], |r| {
            let kinds: String = r.get(4)?;
            Ok((
                r.get(0)?,
                (
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    kinds.split(' ').filter(|k| !k.is_empty()).map(String::from).collect(),
                ),
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Remove the patients no database links any more (and their pseudonyms with them).
pub fn sweep(c: &Connection) -> anyhow::Result<usize> {
    Ok(c.execute("DELETE FROM patients WHERE id NOT IN (SELECT patient FROM patient_keys)", [])?)
}

/// The patient numbers of these persons of `s`: those linked before, and the others linked now.
/// Persons in conflict are left out.
pub fn patients(
    cfg: &Config,
    c: &Connection,
    s: &Source,
    keys: &[String],
    user: Option<&User>,
) -> anyhow::Result<HashMap<String, i64>> {
    let keys: Vec<String> = keys.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();
    let mut out = known(cfg, c, s.id, &keys)?;
    let missing: Vec<String> = keys.into_iter().filter(|k| !out.contains_key(k)).collect();
    if missing.is_empty() {
        return Ok(out);
    }
    for chunk in missing.chunks(2000) {
        let persons = s.persons(chunk)?;
        link(cfg, c, s, &persons, user)?;
    }
    out.extend(known(cfg, c, s.id, &missing)?);
    Ok(out)
}

/// The patient numbers of these persons, as linked before.
pub fn known(cfg: &Config, c: &Connection, database: i64, keys: &[String]) -> anyhow::Result<HashMap<String, i64>> {
    let w = open(cfg, c, database)?;
    let mut out = HashMap::new();
    for chunk in keys.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut st = w.prepare(&format!("SELECT person_key, patient FROM persons WHERE person_key IN ({marks})"))?;
        let rows =
            st.query_map(rusqlite::params_from_iter(chunk.iter()), |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
        for r in rows {
            let (k, p) = r?;
            out.insert(k, p);
        }
    }
    Ok(out)
}

/// The databases that link a patient (those being removed left out).
pub fn databases_of(c: &Connection, patient: i64) -> anyhow::Result<Vec<i64>> {
    let mut st = c.prepare(
        "SELECT DISTINCT k.database_id FROM patient_keys k JOIN databases d ON d.id = k.database_id
         WHERE k.patient = ?1 AND d.status != 'removing' ORDER BY k.database_id",
    )?;
    let v = st.query_map([patient], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// The keys of a patient's persons in each database that links them, of those given (none:
/// every one).
pub fn persons_of(
    cfg: &Config,
    c: &Connection,
    patient: i64,
    only: Option<&[i64]>,
) -> anyhow::Result<Vec<(i64, Vec<String>)>> {
    let mut out = vec![];
    for db in databases_of(c, patient)? {
        if only.is_some_and(|o| !o.contains(&db)) {
            continue;
        }
        let w = open(cfg, c, db)?;
        let mut st = w.prepare("SELECT person_key FROM persons WHERE patient = ?1 ORDER BY person_key")?;
        let keys: Vec<String> = st.query_map([patient], |r| r.get(0))?.collect::<Result<_, _>>()?;
        if !keys.is_empty() {
            out.push((db, keys));
        }
    }
    Ok(out)
}

/// What a database says of a person, as kept when they were linked.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Known {
    pub database: i64,
    pub person_key: String,
    pub patient: i64,
    pub patient_id: String,
    pub patient_id_issuer: String,
    pub mrn: String,
    pub mrn_authority: String,
    pub last_name: String,
    pub first_name: String,
    pub birth_date: String,
    pub sex: String,
}

fn known_of(database: i64, r: &rusqlite::Row) -> rusqlite::Result<Known> {
    Ok(Known {
        database,
        person_key: r.get(0)?,
        patient: r.get(1)?,
        patient_id: r.get(2)?,
        patient_id_issuer: r.get(3)?,
        mrn: r.get(4)?,
        mrn_authority: r.get(5)?,
        last_name: r.get(6)?,
        first_name: r.get(7)?,
        birth_date: r.get(8)?,
        sex: r.get(9)?,
    })
}

const KNOWN_COLS: &str =
    "person_key, patient, patient_id, patient_id_issuer, mrn, mrn_authority, last_name, first_name, birth_date, sex";

/// Everything the databases (of those given; none: every one) say of a patient's persons.
pub fn records_of(cfg: &Config, c: &Connection, patient: i64, only: Option<&[i64]>) -> anyhow::Result<Vec<Known>> {
    let mut out = vec![];
    for db in databases_of(c, patient)? {
        if only.is_some_and(|o| !o.contains(&db)) {
            continue;
        }
        let w = open(cfg, c, db)?;
        let mut st = w.prepare(&format!("SELECT {KNOWN_COLS} FROM persons WHERE patient = ?1 ORDER BY person_key"))?;
        out.extend(st.query_map([patient], |r| known_of(db, r))?.collect::<Result<Vec<_>, _>>()?);
    }
    Ok(out)
}

/// Every person of a database, as kept (the patient list, the gate's dictionary).
pub fn all_of(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Vec<Known>> {
    let w = open(cfg, c, database)?;
    let mut st = w.prepare(&format!("SELECT {KNOWN_COLS} FROM persons ORDER BY patient, person_key"))?;
    let v = st.query_map([], |r| known_of(database, r))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// A database's conflicts: (person key, what is wrong).  The keys never leave the server
/// except to those who manage the database.
pub fn conflicts(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<Vec<(String, String)>> {
    let w = open(cfg, c, database)?;
    let mut st = w.prepare("SELECT person_key, detail FROM conflicts ORDER BY person_key")?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// How many persons and patients a database links.
pub fn counts(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<(i64, i64, i64)> {
    let w = open(cfg, c, database)?;
    let persons: i64 = w.query_row("SELECT COUNT(*) FROM persons", [], |r| r.get(0))?;
    let patients: i64 = w.query_row("SELECT COUNT(DISTINCT patient) FROM persons", [], |r| r.get(0))?;
    let conflicts: i64 = w.query_row("SELECT COUNT(*) FROM conflicts", [], |r| r.get(0))?;
    Ok((persons, patients, conflicts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_hash_alike_and_apart() {
        let s = b"0123456789abcdef0123456789abcdef";
        assert_eq!(hash(s, "HOSP", "123"), hash(s, " hosp ", " 123 "));
        assert_ne!(hash(s, "HOSP", "123"), hash(s, "OTHER", "123"));
        assert_ne!(hash(s, "HOSP", "123"), hash(s, "HOSP1", "23"));
        assert_ne!(hash(s, "HOSP", "123"), hash(b"another secret, another hash...", "HOSP", "123"));
        assert_eq!(normal_sex("female"), "F");
        assert_eq!(normal_sex("u"), "");
    }
}
