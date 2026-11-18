//! The patient index (docs/design/connected-databases.md, "The subject index"): each connected
//! database's persons, linked to Medorama's patient numbers.
//!
//! A person names themselves by (issuer, identifier) pairs: their patient ID under its issuer,
//! their MRN under its authority.  Each pair is assumed to name one patient (a requirement on the
//! databases: Medorama does not resolve identity).  Linking keeps, in the control store, a keyed
//! hash of each pair with the patient number it maps to (`patient_keys`): two databases' persons
//! with an equal pair are one patient, and no pair can be read back.  What the database says of
//! the person (identifiers, names, birth date, sex) and their encounters (key, start, end, ICU)
//! are kept in its workspace folder, `persons.sqlite`, encrypted with its key, and nothing more:
//! the privacy gate's dictionary and the patient list read them there; everything else is read
//! from the database when a patient is opened.  A person whose pairs lead to two patients is a
//! conflict: listed, never linked.

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
    linked_at          REAL NOT NULL,
    earliest           REAL,                       -- the first and latest time of their data,
    latest             REAL                        -- when the persons view gives them
);
CREATE INDEX IF NOT EXISTS persons_patient ON persons(patient);
-- each person's encounters, as the database's visits give them (the patient list's dates)
CREATE TABLE IF NOT EXISTS encounters (
    visit_key   TEXT NOT NULL,
    person_key  TEXT NOT NULL,
    start_time  REAL,
    end_time    REAL,
    icu         INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (person_key, visit_key)
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
    // (0.2 kept a summary of each person's data: no longer)
    w.execute_batch("DROP TABLE IF EXISTS summary")?;
    w.execute_batch(SCHEMA)?;
    // (0.3.0 kept no dates of a person's data)
    let cols: Vec<String> =
        w.prepare("PRAGMA table_info(persons)")?.query_map([], |r| r.get(1))?.collect::<Result<_, _>>()?;
    for col in ["earliest", "latest"] {
        if !cols.iter().any(|c| c == col) {
            w.execute_batch(&format!("ALTER TABLE persons ADD COLUMN {col} REAL"))?;
        }
    }
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
    written();
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
    let wt = rusqlite::Transaction::new_unchecked(&w, rusqlite::TransactionBehavior::Immediate)?;
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
                                  first_name, birth_date, sex, hashes, linked_at, earliest, latest)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT (person_key) DO UPDATE SET patient = excluded.patient, patient_id = excluded.patient_id,
                patient_id_issuer = excluded.patient_id_issuer, mrn = excluded.mrn,
                mrn_authority = excluded.mrn_authority, last_name = excluded.last_name,
                first_name = excluded.first_name, birth_date = excluded.birth_date, sex = excluded.sex,
                hashes = excluded.hashes,
                earliest = COALESCE(excluded.earliest, earliest), latest = COALESCE(excluded.latest, latest)",
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
                now(),
                p.earliest,
                p.latest
            ],
        )?;
        rep.linked += 1;
    }
    wt.commit()?;
    // (counted before and after: a search that read the index in between reads it again)
    written();
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

/// Link every person of `s`, forget the persons it no longer holds, and read their encounters
/// (`medorama databases refresh`; connecting): one pass over its persons and one over its visits.
pub fn sync(cfg: &Config, c: &Connection, s: &Source, user: Option<&User>) -> anyhow::Result<Linked> {
    let all = s.every_person(s.zone(&cfg.timezone))?;
    let mut rep = Linked::default();
    for chunk in all.chunks(2000) {
        let r = link(cfg, c, s, chunk, user)?;
        rep.linked += r.linked;
        rep.new_patients += r.new_patients;
        rep.conflicts += r.conflicts;
    }
    let now_keys: std::collections::HashSet<&str> = all.iter().map(|p| p.key.as_str()).collect();
    let w = open(cfg, c, s.id)?;
    written();
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
    encounters_read(cfg, c, s)?;
    written();
    Ok(rep)
}

/// Read every encounter of `s` (its visits: key, person, start, end, ICU) into its persons file.
pub fn encounters_read(cfg: &Config, c: &Connection, s: &Source) -> anyhow::Result<usize> {
    let visits = s.every_visit(s.zone(&cfg.timezone))?;
    let mut w = open(cfg, c, s.id)?;
    let tx = w.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM encounters", [])?;
    {
        let mut ins = tx.prepare(
            "INSERT INTO encounters (visit_key, person_key, start_time, end_time, icu) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT DO UPDATE SET start_time = excluded.start_time, end_time = excluded.end_time, icu = excluded.icu",
        )?;
        for v in &visits {
            ins.execute(rusqlite::params![v.key, v.person, v.start, v.end, v.icu])?;
        }
    }
    tx.commit()?;
    Ok(visits.len())
}

/// What the patient list shows of a database's persons, from their encounters: person key →
/// (encounters, the latest start, whether one was in intensive care).
pub fn encounter_summaries(
    cfg: &Config,
    c: &Connection,
    database: i64,
) -> anyhow::Result<HashMap<String, (i64, Option<f64>, bool)>> {
    let w = open(cfg, c, database)?;
    let mut st = w.prepare(
        "SELECT person_key, COUNT(*), MAX(COALESCE(start_time, end_time)), MAX(icu) FROM encounters GROUP BY person_key",
    )?;
    let v = st
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0))))?
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

/// The patient numbers of these persons, as linked before: many of them (a criterion's
/// thousands) from the shared index, a few from the file.
pub fn known(cfg: &Config, c: &Connection, database: i64, keys: &[String]) -> anyhow::Result<HashMap<String, i64>> {
    if keys.len() > 5000 {
        let wanted: HashSet<&str> = keys.iter().map(String::as_str).collect();
        return Ok(all_of(cfg, c, database)?
            .iter()
            .filter(|k| wanted.contains(k.person_key.as_str()))
            .map(|k| (k.person_key.clone(), k.patient))
            .collect());
    }
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
    /// The first and latest time of their data, when the database's persons view gives them.
    pub earliest: Option<f64>,
    pub latest: Option<f64>,
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
        earliest: r.get(10)?,
        latest: r.get(11)?,
    })
}

const KNOWN_COLS: &str = "person_key, patient, patient_id, patient_id_issuer, mrn, mrn_authority, last_name, \
                          first_name, birth_date, sex, earliest, latest";

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

/// Every person of a database, as kept (the patient list, the gate's dictionary, a search's
/// everyone): shared, and read again only when the index changed (written by this process, or
/// its file or journal changed: another process's refresh), at most [`SHARED_PERSONS`] kept in
/// all.  A search over a database of a million persons reads them once, not at each search.
pub fn all_of(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<std::sync::Arc<Vec<Known>>> {
    let stamp = Stamp::of(cfg, database);
    if let Some(v) = stamp.as_ref().and_then(|st| shared().get(st)) {
        return Ok(v);
    }
    let w = open(cfg, c, database)?;
    let mut st = w.prepare(&format!("SELECT {KNOWN_COLS} FROM persons ORDER BY patient, person_key"))?;
    let v: std::sync::Arc<Vec<Known>> =
        std::sync::Arc::new(st.query_map([], |r| known_of(database, r))?.collect::<Result<_, _>>()?);
    if let Some(st) = stamp {
        shared().put(st, v.clone());
    }
    Ok(v)
}

/// The persons kept in memory in all (each database's whole index, or none of it).
pub const SHARED_PERSONS: usize = 5_000_000;

/// Which index (its file), and when it was last written: this process's writes counted, and
/// the file's and its journal's times and sizes (another process's).
#[derive(Debug, Clone, PartialEq)]
struct Stamp {
    file: std::path::PathBuf,
    writes: u64,
    files: Vec<Option<(std::time::SystemTime, u64)>>,
}

static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// An index was written (or is about to be): what was shared of every index is read again.
fn written() {
    WRITES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

impl Stamp {
    fn of(cfg: &Config, database: i64) -> Option<Stamp> {
        let f = crate::workspace::database_dir(cfg, database).join("persons.sqlite");
        let at = |p: &std::path::Path| std::fs::metadata(p).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
        let main = at(&f)?;
        let journal = |ext: &str| at(&f.with_file_name(format!("persons.sqlite-{ext}")));
        Some(Stamp {
            writes: WRITES.load(std::sync::atomic::Ordering::SeqCst),
            files: vec![Some(main), journal("wal"), journal("journal")],
            file: f,
        })
    }
}

type Shared = HashMap<std::path::PathBuf, (Stamp, std::sync::Arc<Vec<Known>>)>;

struct SharedIndexes(std::sync::Mutex<Shared>);

fn shared() -> &'static SharedIndexes {
    static S: std::sync::OnceLock<SharedIndexes> = std::sync::OnceLock::new();
    S.get_or_init(|| SharedIndexes(Default::default()))
}

impl SharedIndexes {
    fn get(&self, stamp: &Stamp) -> Option<std::sync::Arc<Vec<Known>>> {
        let m = self.0.lock().unwrap();
        m.get(&stamp.file).filter(|(st, _)| st == stamp).map(|(_, v)| v.clone())
    }

    fn put(&self, stamp: Stamp, v: std::sync::Arc<Vec<Known>>) {
        let mut m = self.0.lock().unwrap();
        m.remove(&stamp.file);
        // (an index of a database since removed: its file gone)
        m.retain(|f, _| f.is_file());
        let kept: usize = m.values().map(|(_, v)| v.len()).sum();
        if kept + v.len() <= SHARED_PERSONS {
            m.insert(stamp.file.clone(), (stamp, v));
        }
    }
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
