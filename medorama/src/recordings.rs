//! Recordings (docs/design/connected-databases.md, "Files and indexes"): the recordings a
//! recording database lists, and what Medorama keeps of those it has opened, in that database's
//! workspace folder (`recordings.sqlite`): whether the file is there, its fingerprint (the index
//! key), the format and times its index found, and the series derived from it.
//!
//! Connecting reads no recording.  A patient's recordings are read from the database when their
//! page (or a job, a cohort's selection) asks for them ([`sync_persons`]): only then are their
//! files checked and kept here, and indexed.  [`sync`] reads every recording a database lists:
//! it is asked for (`medorama databases index`), never done when connecting.
//!
//! A recording is numbered `database × 1,000,000,000 + its number in the database's file`, one
//! number across Medorama (jobs, the page, the viewer).  Medorama only reads recordings: the
//! file or folder itself is never written, moved or deleted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::db::now;
use crate::sources::Source;
use crate::sql::{params, Connection};
use crate::users::User;
use recording_engine::index::cache_key_of;
use recording_engine::store::STORE_FORMAT;

/// A recording's index key (index format 3: its database's index store).
fn cache_key(path: &Path) -> String {
    cache_key_of(path, STORE_FORMAT)
}
use recording_engine::sources::{kind_of, unreadable};
use serde::Serialize;

/// The numbers of one database's recordings: `database × STRIDE + n`.
pub const STRIDE: i64 = 1_000_000_000;

pub fn global(database: i64, n: i64) -> i64 {
    database * STRIDE + n
}

/// (database, number in its file) of a recording's number.
pub fn split(id: i64) -> (i64, i64) {
    (id / STRIDE, id % STRIDE)
}

/// Status of a recording, as shown by the colored dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Indexed and current (green).
    Ready,
    /// Waiting for indexing.
    Queued,
    Indexing,
    /// Never indexed.
    New,
    /// Changed since it was indexed.
    Stale,
    Failed,
    /// Not found at the last check.
    Missing,
    /// A file no reader indexes (a picture, a PDF, a 12-lead ECG in DICOM or XML): opened by its
    /// viewer from its entry, never indexed.
    File,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::Queued => "queued",
            Status::Indexing => "indexing",
            Status::New => "new",
            Status::Stale => "stale",
            Status::Failed => "failed",
            Status::Missing => "missing",
            Status::File => "file",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        [
            Status::Ready,
            Status::Queued,
            Status::Indexing,
            Status::New,
            Status::Stale,
            Status::Failed,
            Status::Missing,
            Status::File,
        ]
        .into_iter()
        .find(|x| x.as_str() == s)
    }

    /// Ordering for a summary: the most urgent of several recordings.
    fn severity(self) -> u8 {
        match self {
            Status::File => 0,
            Status::Ready => 1,
            Status::New => 2,
            Status::Stale => 3,
            Status::Queued => 4,
            Status::Indexing => 5,
            Status::Failed => 6,
            Status::Missing => 7,
        }
    }
}

/// The summary status of several recordings (None when there are none).
pub fn summary(statuses: &[Status]) -> Option<Status> {
    statuses.iter().copied().max_by_key(|s| s.severity())
}

#[derive(Debug, Clone, Serialize)]
pub struct Recording {
    pub id: i64,
    pub database: i64,
    /// The database's name.
    pub source: String,
    /// The database's key for it (never sent: identifies it in the database).
    #[serde(skip)]
    pub key: String,
    #[serde(skip)]
    pub person_key: String,
    #[serde(skip)]
    pub visit_key: String,
    /// Its Medorama patient, once its person is linked.
    pub patient: Option<i64>,
    pub path: String,
    pub kind: String,
    pub name: String,
    pub size: i64,
    /// The recording's format, as its reader names it (`WFDB collection`, an adapter's description).
    pub format: String,
    pub t_start: Option<f64>,
    pub t_end: Option<f64>,
    /// The Patient Journey's lane (categories.rs).
    pub category: String,
    pub label: String,
    pub present: bool,
    pub status: Status,
    /// Indexing progress 0..1 while indexing.
    pub progress: f64,
    pub error: String,
    /// The index key (the viewers' cache key) when ready.
    pub index_key: Option<String>,
    pub fingerprint: String,
    pub added_at: f64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS recordings (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    recording_key  TEXT NOT NULL UNIQUE,
    person_key     TEXT NOT NULL DEFAULT '',
    visit_key      TEXT NOT NULL DEFAULT '',
    path           TEXT NOT NULL,
    kind           TEXT NOT NULL DEFAULT '',
    name           TEXT NOT NULL DEFAULT '',
    size           INTEGER NOT NULL DEFAULT 0,
    fingerprint    TEXT NOT NULL DEFAULT '',
    format         TEXT NOT NULL DEFAULT '',
    t_start        REAL,
    t_end          REAL,
    label          TEXT NOT NULL DEFAULT '',
    category       TEXT NOT NULL DEFAULT 'icu',
    present        INTEGER NOT NULL DEFAULT 1,
    problem        TEXT NOT NULL DEFAULT '',
    added_at       REAL NOT NULL,
    checked_at     REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS recordings_person ON recordings(person_key);
CREATE TABLE IF NOT EXISTS series (
    recording  INTEGER NOT NULL,
    name       TEXT NOT NULL,
    unit       TEXT NOT NULL DEFAULT '',
    t          BLOB NOT NULL,
    v          BLOB NOT NULL,
    lo         BLOB NOT NULL,
    hi         BLOB NOT NULL,
    PRIMARY KEY (recording, name)
);
";

/// A recording database's `recordings.sqlite`.
pub fn open(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<rusqlite::Connection> {
    let w = crate::workspace::open_database_file(cfg, c, database, "recordings.sqlite")?;
    w.execute_batch(SCHEMA)?;
    Ok(w)
}

fn size_of(p: &Path) -> i64 {
    if p.is_dir() {
        std::fs::read_dir(p)
            .map(|it| {
                it.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len() as i64).sum()
            })
            .unwrap_or(0)
    } else {
        std::fs::metadata(p).map(|m| m.len() as i64).unwrap_or(0)
    }
}

/// What reading the recordings of a database found.
#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub recordings: usize,
    pub added: usize,
    pub missing: usize,
    pub changed: usize,
    pub removed: usize,
    /// Rows new to the privacy gate's dictionary, and rows whose path or label it names changed.
    #[serde(skip)]
    pub named: usize,
    #[serde(skip)]
    pub renamed: usize,
}

/// Why a recording's file is not read (empty: it is): missing, outside `data_roots`, inside the
/// workspace, or unreadable.
fn problem_of(cfg: &Config, path: &Path) -> String {
    if !path.exists() {
        "no such file or folder".into()
    } else if !cfg.in_data_roots(path) {
        "outside the folders data may come from (data_roots)".into()
    } else if crate::config::within(path, &crate::config::real(&cfg.workspace_dir)) {
        "inside Medorama's workspace".into()
    } else {
        unreadable(path).unwrap_or_default().to_string()
    }
}

/// Read every recording a database lists and keep what Medorama needs of each: its file's
/// presence and fingerprint, its patient (`medorama databases index`: asked for, never done
/// when connecting).  Recordings the database no longer lists go (their series too).
pub fn sync(cfg: &Config, c: &Connection, s: &Source) -> anyhow::Result<SyncReport> {
    if !s.has_recordings() {
        return Ok(SyncReport::default());
    }
    let listed = s.all_recordings(s.zone(&cfg.timezone))?;
    let lock = writer(s.id);
    let _one = lock.lock().unwrap_or_else(|e| e.into_inner());
    keep(cfg, c, s, &listed, None)
}

/// Read these persons' recordings from the database now, and keep what Medorama needs of each
/// (their files checked): what a patient's page, a job or a cohort's selection reads.  Their
/// recordings the database no longer lists go.
///
/// A page asks for a patient's recordings in several requests at once (its summary, journey and
/// report): one reads them, and the others, waiting meanwhile, take what it read (it was read
/// after they asked); a request that comes later reads them again.
pub fn sync_persons(cfg: &Config, c: &Connection, s: &Source, persons: &[String]) -> anyhow::Result<SyncReport> {
    if persons.is_empty() || !s.has_recordings() {
        return Ok(SyncReport::default());
    }
    let key = format!("{}\u{1}{}\u{1}{}", s.id, s.url, persons.join("\u{1}"));
    let asked = std::time::Instant::now();
    let lock = writer(s.id);
    let _one = lock.lock().unwrap_or_else(|e| e.into_inner());
    // (read by another request since this one asked: what it read is as new)
    if lately().lock().unwrap().get(&key).is_some_and(|done| *done >= asked) {
        return Ok(SyncReport::default());
    }
    let listed = s.recordings(persons, s.zone(&cfg.timezone))?;
    let rep = keep(cfg, c, s, &listed, Some(persons))?;
    let mut l = lately().lock().unwrap();
    // (only the reads of the last minute matter: requests wait seconds at most)
    l.retain(|_, t| t.elapsed() < std::time::Duration::from_secs(60));
    l.insert(key, std::time::Instant::now());
    Ok(rep)
}

/// When the recordings of persons were last read, by database and persons.
fn lately() -> &'static std::sync::Mutex<HashMap<String, std::time::Instant>> {
    static L: std::sync::OnceLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

/// One writer of a database's `recordings.sqlite` at a time in this server (another process,
/// a command run beside it, waits on SQLite's lock: the transactions are immediate).
fn writer(database: i64) -> std::sync::Arc<std::sync::Mutex<()>> {
    static W: std::sync::OnceLock<std::sync::Mutex<HashMap<i64, std::sync::Arc<std::sync::Mutex<()>>>>> =
        std::sync::OnceLock::new();
    W.get_or_init(Default::default).lock().unwrap().entry(database).or_default().clone()
}

/// Keep these recordings, as listed: each file checked, the rows added or updated; the rows the
/// listing no longer holds (of `persons`; none: of the whole database) removed.  The privacy
/// gate's dictionary is built again when what it names changed.
fn keep(
    cfg: &Config,
    c: &Connection,
    s: &Source,
    listed: &[crate::sources::Recording],
    persons: Option<&[String]>,
) -> anyhow::Result<SyncReport> {
    let mut rep = SyncReport::default();
    // each file checked first (on a network share, the slow part), the index then written in
    // one short transaction
    struct Checked {
        path: std::path::PathBuf,
        problem: String,
        kind: String,
        fp: String,
        size: i64,
    }
    let checked: Vec<Checked> = listed
        .iter()
        .map(|r| {
            let path = crate::config::real(&s.recording_path(&r.file));
            let problem = problem_of(cfg, &path);
            let present = problem.is_empty();
            Checked {
                kind: if present { kind_of(&path).unwrap_or_default() } else { String::new() },
                fp: if present { cache_key(&path) } else { String::new() },
                size: if present { size_of(&path) } else { 0 },
                problem,
                path,
            }
        })
        .collect();
    let mut w = open(cfg, c, s.id)?;
    let tx = w.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let t = now();
    for (r, ck) in listed.iter().zip(checked) {
        rep.recordings += 1;
        let Checked { path, problem, kind, fp, size } = ck;
        let present = problem.is_empty();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let category = if r.category.trim().is_empty() {
            "icu".to_string()
        } else {
            crate::categories::named(&r.category).unwrap_or("icu").to_string()
        };
        let before: Option<(String, bool, String, String)> = tx
            .query_row(
                "SELECT fingerprint, present, path, label FROM recordings WHERE recording_key = ?1",
                [&r.key],
                |x| Ok((x.get(0)?, x.get::<_, i64>(1)? != 0, x.get(2)?, x.get(3)?)),
            )
            .ok();
        // what the gate's dictionary names (paths, labels): new, or changed
        match &before {
            None => rep.named += 1,
            Some(b) if b.2 != path.to_string_lossy() || b.3 != r.label => rep.renamed += 1,
            _ => {}
        }
        match &before {
            None => rep.added += 1,
            Some((f, ..)) if present && *f != fp => rep.changed += 1,
            _ => {}
        }
        if !present && before.as_ref().is_none_or(|b| b.1) {
            rep.missing += 1;
        }
        tx.execute(
            // the size and fingerprint Medorama's own (the fingerprint is the viewers' index key);
            // the format and times the database's until indexing reads them from the recording,
            // and the database's again once the file changed
            "INSERT INTO recordings (recording_key, person_key, visit_key, path, kind, name, size, fingerprint, t_start,
                                     t_end, label, category, present, problem, added_at, checked_at, format)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15, ?16)
             ON CONFLICT (recording_key) DO UPDATE SET person_key = excluded.person_key, visit_key = excluded.visit_key,
                path = excluded.path, kind = excluded.kind, name = excluded.name, size = excluded.size,
                fingerprint = excluded.fingerprint, label = excluded.label, category = excluded.category,
                present = excluded.present, problem = excluded.problem, checked_at = excluded.checked_at,
                t_start = CASE WHEN recordings.fingerprint <> excluded.fingerprint THEN excluded.t_start
                               ELSE COALESCE(recordings.t_start, excluded.t_start) END,
                t_end = CASE WHEN recordings.fingerprint <> excluded.fingerprint THEN excluded.t_end
                             ELSE COALESCE(recordings.t_end, excluded.t_end) END,
                format = CASE WHEN recordings.fingerprint <> excluded.fingerprint OR recordings.format = ''
                              THEN excluded.format ELSE recordings.format END",
            rusqlite::params![
                r.key,
                r.person,
                r.visit,
                path.to_string_lossy(),
                kind,
                name,
                size,
                fp,
                r.start,
                r.end,
                r.label,
                category,
                present,
                problem,
                t,
                r.format
            ],
        )?;
    }
    let keys: std::collections::HashSet<&str> = listed.iter().map(|r| r.key.as_str()).collect();
    let gone: Vec<(i64, String)> = {
        let v: Vec<(i64, String, String)> = {
            let mut st = tx.prepare("SELECT id, recording_key, person_key FROM recordings")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<Vec<_>, _>>()?;
            v
        };
        let scope: Option<std::collections::HashSet<&str>> = persons.map(|p| p.iter().map(String::as_str).collect());
        v.into_iter()
            .filter(|(_, k, p)| !keys.contains(k.as_str()) && scope.as_ref().is_none_or(|s| s.contains(p.as_str())))
            .map(|(id, k, _)| (id, k))
            .collect()
    };
    for (id, _) in &gone {
        tx.execute("DELETE FROM series WHERE recording = ?1", [id])?;
        tx.execute("DELETE FROM recordings WHERE id = ?1", [id])?;
        c.execute("DELETE FROM jobs WHERE database_id = ?1 AND recording = ?2", params![s.id, global(s.id, *id)])?;
    }
    rep.removed = gone.len();
    tx.commit()?;
    // recordings only added: the gate's dictionary takes them as they are; a path or a label
    // changed, or a recording gone: it is built again
    if rep.renamed + rep.removed > 0 {
        c.execute("UPDATE data_version SET version = version + 1, identifiers = identifiers + 1", [])?;
    } else if rep.named > 0 {
        c.execute("UPDATE data_version SET version = version + 1, recordings = recordings + 1", [])?;
    }
    // the patients of the recordings' persons, linked now
    let persons: Vec<String> = listed.iter().map(|r| r.person.clone()).filter(|p| !p.is_empty()).collect();
    crate::persons::patients(cfg, c, s, &persons, None)?;
    Ok(rep)
}

/// What every recording's index job says: (state, progress, error, fingerprint, version) of the
/// newest index job of each recording of a database.
fn index_jobs(c: &Connection, database: i64) -> anyhow::Result<HashMap<i64, IndexJob>> {
    let mut st = c.prepare(
        "SELECT recording, state, progress, error, fingerprint, action_version FROM jobs
         WHERE id IN (SELECT MAX(id) FROM jobs WHERE database_id = ?1 AND action = 'index' GROUP BY recording)",
    )?;
    let v = st
        .query_map([database], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                IndexJob {
                    state: r.get(1)?,
                    progress: r.get(2)?,
                    error: r.get(3)?,
                    fingerprint: r.get(4)?,
                    version: r.get(5)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

struct IndexJob {
    state: String,
    progress: f64,
    error: String,
    fingerprint: String,
    version: String,
}

fn status_of(present: bool, kind: &str, fingerprint: &str, job: Option<&IndexJob>) -> (Status, f64, String) {
    if !present {
        return (Status::Missing, 0.0, String::new());
    }
    if kind.is_empty() {
        return (Status::File, 0.0, String::new());
    }
    let Some(j) = job else { return (Status::New, 0.0, String::new()) };
    let current = j.fingerprint == fingerprint && j.version == crate::actions::INDEX_VERSION_STR.as_str();
    match j.state.as_str() {
        // indexing cancelled by a user (jobs::cancel): failed until indexed again; cancelled by
        // Medorama (its index gone from its store): new
        "cancelled" if current && j.error == "cancelled" => {
            (Status::Failed, 0.0, "indexing was cancelled: index it to index it again".into())
        }
        "cancelled" => (Status::New, 0.0, String::new()),
        "queued" => (Status::Queued, 0.0, String::new()),
        "running" => (Status::Indexing, j.progress, String::new()),
        _ if !current => (Status::Stale, 0.0, String::new()),
        "done" => (Status::Ready, 0.0, String::new()),
        _ => (Status::Failed, 0.0, j.error.clone()),
    }
}

const COLS: &str =
    "id, recording_key, person_key, visit_key, path, kind, name, size, fingerprint, format, t_start, t_end,
    label, category, present, problem, added_at";

/// A database's recordings, as kept, with their status; `filter` an SQL condition on them.
fn read(cfg: &Config, c: &Connection, s: &Source, filter: &str, args: &[String]) -> anyhow::Result<Vec<Recording>> {
    let w = open(cfg, c, s.id)?;
    let jobs = index_jobs(c, s.id)?;
    let mut st =
        w.prepare(&format!("SELECT {COLS} FROM recordings WHERE {filter} ORDER BY t_start IS NULL, t_start, id"))?;
    let rows: Vec<Recording> = st
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            let n: i64 = r.get(0)?;
            let id = global(s.id, n);
            let present = r.get::<_, i64>(14)? != 0;
            let fingerprint: String = r.get(8)?;
            let problem: String = r.get(15)?;
            let kind: String = r.get(5)?;
            let (status, progress, error) = status_of(present, &kind, &fingerprint, jobs.get(&id));
            Ok(Recording {
                id,
                database: s.id,
                source: s.name.clone(),
                key: r.get(1)?,
                person_key: r.get(2)?,
                visit_key: r.get(3)?,
                patient: None,
                path: r.get(4)?,
                kind,
                name: r.get(6)?,
                size: r.get(7)?,
                index_key: (status == Status::Ready).then(|| fingerprint.clone()),
                fingerprint,
                format: r.get(9)?,
                t_start: r.get(10)?,
                t_end: r.get(11)?,
                label: r.get(12)?,
                category: r.get(13)?,
                present,
                status,
                progress,
                error: if !present { problem } else { error },
                added_at: r.get(16)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    drop(st);
    let persons: Vec<String> = rows.iter().map(|r| r.person_key.clone()).collect();
    let known = crate::persons::known(cfg, c, s.id, &persons)?;
    Ok(rows
        .into_iter()
        .map(|mut r| {
            r.patient = known.get(&r.person_key).copied();
            r
        })
        .collect())
}

/// One recording, by its number (none if its database is gone or being removed).
pub fn get(cfg: &Config, c: &Connection, id: i64) -> anyhow::Result<Option<Recording>> {
    let (db, n) = split(id);
    let Some(s) = crate::sources::get(c, db)? else { return Ok(None) };
    Ok(read(cfg, c, &s, "id = ?1", &[n.to_string()])?.into_iter().next())
}

/// A database's recordings kept after this one (its number in the database's file), in order
/// (the privacy gate's dictionary, which takes the new ones).
pub fn after(cfg: &Config, c: &Connection, s: &Source, n: i64) -> anyhow::Result<Vec<Recording>> {
    let mut v = read(cfg, c, s, "id > ?1", &[n.to_string()])?;
    v.sort_by_key(|r| r.id);
    Ok(v)
}

/// Several recordings by number, each database's file opened once (a job queue, a check):
/// by number, those found.
pub fn get_many(cfg: &Config, c: &Connection, ids: &[i64]) -> anyhow::Result<HashMap<i64, Recording>> {
    let mut by_db: HashMap<i64, Vec<i64>> = HashMap::new();
    for &id in ids {
        let (db, n) = split(id);
        by_db.entry(db).or_default().push(n);
    }
    let mut out = HashMap::new();
    for (db, ns) in by_db {
        let Some(s) = crate::sources::get(c, db)? else { continue };
        for chunk in ns.chunks(500) {
            let marks = (1..=chunk.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
            let args: Vec<String> = chunk.iter().map(|n| n.to_string()).collect();
            for r in read(cfg, c, &s, &format!("id IN ({marks})"), &args)? {
                out.insert(r.id, r);
            }
        }
    }
    Ok(out)
}

/// Every recording of a database.
pub fn of_database(cfg: &Config, c: &Connection, s: &Source) -> anyhow::Result<Vec<Recording>> {
    read(cfg, c, s, "1 = 1", &[])
}

/// The recordings of these persons of a database, read from it now (their files checked and
/// kept: [`sync_persons`]); a database offline: those kept when it was last read.
pub fn of_persons(cfg: &Config, c: &Connection, s: &Source, persons: &[String]) -> anyhow::Result<Vec<Recording>> {
    let mut out = vec![];
    for chunk in persons.chunks(500) {
        if let Err(e) = sync_persons(cfg, c, s, chunk) {
            eprintln!("medorama: {}: recordings: {e}", s.name);
        }
        let marks = (1..=chunk.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        out.extend(read(cfg, c, s, &format!("person_key IN ({marks})"), chunk)?);
    }
    Ok(out)
}

/// A patient's recordings, in every database granted to `user` for viewing.
pub fn of_patient(cfg: &Config, c: &Connection, user: &User, patient: i64) -> anyhow::Result<Vec<Recording>> {
    let granted = crate::databases::granted(c, user, "view")?;
    let mut out = vec![];
    for (db, keys) in crate::persons::persons_of(cfg, c, patient, Some(&granted))? {
        let Some(s) = crate::sources::get(c, db)? else { continue };
        if !s.has_recordings() {
            continue;
        }
        out.extend(of_persons(cfg, c, &s, &keys)?);
    }
    out.sort_by(|a, b| a.t_start.partial_cmp(&b.t_start).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// How many of a database's recordings Medorama keeps (those opened, or indexed by
/// `medorama databases index`).
pub fn count(cfg: &Config, c: &Connection, database: i64) -> anyhow::Result<i64> {
    let dir = crate::workspace::database_dir(cfg, database);
    if !dir.join("recordings.sqlite").is_file() {
        return Ok(0);
    }
    let w = open(cfg, c, database)?;
    Ok(w.query_row("SELECT COUNT(*) FROM recordings", [], |r| r.get(0))?)
}

/// Every recording of every database (the job queue's "all recordings"; checks).
pub fn all(cfg: &Config, c: &Connection) -> anyhow::Result<Vec<Recording>> {
    let mut out = vec![];
    for s in crate::sources::list(c)? {
        if count(cfg, c, s.id)? > 0 {
            out.extend(of_database(cfg, c, &s)?);
        }
    }
    Ok(out)
}

/// Record times and format from an index's `meta.json`; false when there is none.
pub fn apply_index_meta(cfg: &Config, c: &Connection, id: i64, text: &str) -> anyhow::Result<bool> {
    if text.is_empty() {
        return Ok(false);
    }
    let meta: serde_json::Value = serde_json::from_str(text)?;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in meta.get("pieces").and_then(|v| v.as_array()).into_iter().flatten() {
        let (t0, n, fs) = (p["t0"].as_f64(), p["n"].as_f64(), p["fs"].as_f64());
        if let (Some(t0), Some(n), Some(fs)) = (t0, n, fs) {
            if fs > 0.0 && p.get("timed").and_then(|t| t.as_bool()) != Some(false) {
                lo = lo.min(t0);
                hi = hi.max(t0 + n / fs);
            }
        }
    }
    for k in ["t_start", "t_end"] {
        if let Some(x) = meta.get(k).and_then(|v| v.as_f64()) {
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    let text = |k: &str| meta.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let format = text("description").or(text("format")).unwrap_or("").to_string();
    let (db, n) = split(id);
    let w = open(cfg, c, db)?;
    w.execute(
        "UPDATE recordings SET format = ?1, t_start = ?2, t_end = ?3 WHERE id = ?4",
        rusqlite::params![format, lo.is_finite().then_some(lo), hi.is_finite().then_some(hi), n],
    )?;
    Ok(true)
}

/// Replace a recording's derived series (derived.rs).
pub fn store_series(cfg: &Config, c: &Connection, id: i64, series: &[crate::derived::Derived]) -> anyhow::Result<()> {
    let (db, n) = split(id);
    let lock = writer(db);
    let _one = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut w = open(cfg, c, db)?;
    let tx = w.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM series WHERE recording = ?1", [n])?;
    for s in series {
        tx.execute(
            "INSERT INTO series (recording, name, unit, t, v, lo, hi) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                n,
                s.name,
                s.unit,
                crate::derived::blob(&s.t),
                crate::derived::blob(&s.v),
                crate::derived::blob(&s.lo),
                crate::derived::blob(&s.hi)
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// A recording's derived series.
pub fn load_series(cfg: &Config, c: &Connection, id: i64) -> anyhow::Result<Vec<crate::derived::Derived>> {
    let (db, n) = split(id);
    let w = open(cfg, c, db)?;
    let mut st = w.prepare("SELECT name, unit, t, v, lo, hi FROM series WHERE recording = ?1 ORDER BY name")?;
    let v = st
        .query_map([n], |r| {
            Ok(crate::derived::Derived {
                name: r.get(0)?,
                unit: r.get(1)?,
                t: crate::derived::unblob(&r.get::<_, Vec<u8>>(2)?),
                v: crate::derived::unblob(&r.get::<_, Vec<u8>>(3)?),
                lo: crate::derived::unblob(&r.get::<_, Vec<u8>>(4)?),
                hi: crate::derived::unblob(&r.get::<_, Vec<u8>>(5)?),
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// What checking the recordings found.
#[derive(Debug, Default, Serialize)]
pub struct CheckReport {
    pub checked: usize,
    pub missing: usize,
    pub returned: usize,
    pub changed: usize,
    pub index_lost: usize,
}

/// Re-check every recording of the online databases: present?  changed?  index still there?
pub fn check(cfg: &Config, c: &Connection) -> anyhow::Result<CheckReport> {
    let mut rep = CheckReport::default();
    for s in crate::sources::list(c)? {
        if count(cfg, c, s.id)? == 0 {
            continue;
        }
        let w = open(cfg, c, s.id)?;
        let rows: Vec<(i64, String, bool, String)> = {
            let mut st = w.prepare("SELECT id, path, present, fingerprint FROM recordings")?;
            let v = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)))?
                .collect::<Result<_, _>>()?;
            v
        };
        for (n, path, present, fp) in rows {
            rep.checked += 1;
            let p = Path::new(&path);
            let t = now();
            let problem = problem_of(cfg, p);
            if !problem.is_empty() {
                if present {
                    rep.missing += 1;
                }
                w.execute(
                    "UPDATE recordings SET present = 0, problem = ?1, checked_at = ?2 WHERE id = ?3",
                    rusqlite::params![problem, t, n],
                )?;
                continue;
            }
            if !present {
                rep.returned += 1;
            }
            let key = cache_key(p);
            if key != fp {
                rep.changed += 1;
            }
            w.execute(
                "UPDATE recordings SET present = 1, problem = '', fingerprint = ?1, size = ?2, checked_at = ?3 WHERE id = ?4",
                rusqlite::params![key, size_of(p), t, n],
            )?;
        }
        drop(w);
        // indexes the store no longer holds: the database's recordings read once, its store opened once
        let store = crate::workspace::index_store(cfg, c, s.id).ok();
        for rec in of_database(cfg, c, &s)? {
            let held = store.as_ref().is_some_and(|st| st.engine_of(&rec.fingerprint).is_some());
            if rec.status == Status::Ready && !held {
                rep.index_lost += 1;
                c.execute(
                    "UPDATE jobs SET state = 'cancelled', error = 'index gone from its store'
                     WHERE id = (SELECT MAX(id) FROM jobs WHERE recording = ?1 AND action = 'index')",
                    [rec.id],
                )?;
            }
        }
    }
    Ok(rep)
}

/// Remove, from each database's index store, the indexes no recording of it uses any more (a
/// recording changed or gone): how many.  A removed database's store goes with its folder.
pub fn sweep_index(cfg: &Config, c: &Connection) -> anyhow::Result<usize> {
    let mut n = 0;
    for s in crate::sources::list(c)? {
        if count(cfg, c, s.id)? == 0 || !crate::workspace::database_dir(cfg, s.id).join("index.sqlite").exists() {
            continue;
        }
        let w = open(cfg, c, s.id)?;
        let used: std::collections::HashSet<String> = w
            .prepare("SELECT fingerprint FROM recordings")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        let store = crate::workspace::index_store(cfg, c, s.id)?;
        for k in store.keys().into_iter().filter(|k| !used.contains(k)) {
            n += store.remove(&k).map_err(|e| anyhow::anyhow!("{e}"))? as usize;
        }
    }
    Ok(n)
}

/// Delete a results folder, but only one inside Medorama's workspace.
pub fn purge_dir(cfg: &Config, d: &Path) {
    let rd = crate::config::real(d);
    let root = crate::config::real(&cfg.workspace_dir);
    let ours = crate::config::within(&rd, &root) && rd != root;
    if ours && rd.is_dir() {
        let _ = std::fs::remove_dir_all(&rd);
    }
}

/// Where a job's results go: inside its recording's database folder of the workspace, so that
/// they go with it (`results/<action>/r<recording>/j<job>`).
pub fn results_dir(cfg: &Config, id: i64, action: &str) -> PathBuf {
    let (db, n) = split(id);
    crate::workspace::database_dir(cfg, db).join("results").join(action).join(format!("r{n}"))
}

/// The recordings a user may view, matching a search (words in their name, format, status or
/// database), newest first: the number of matches and one page.
pub fn list(
    cfg: &Config,
    c: &Connection,
    user: &User,
    q: &str,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<Recording>)> {
    list_with(cfg, c, user, q, limit, offset, None)
}

/// [`list`], with the privacy gate at hand (the server's, its dictionary kept).  The recordings
/// a session sees de-identified (Privacy Mode; outside it, those of databases granted without
/// `phi`) are searched by what it is shown only: their token, label (as the gate shows it),
/// format, status, database and kind, never their path or file name; and they are sorted by
/// their times as shown (each patient's own offset), so that the order tells no real time.
pub fn list_with(
    cfg: &Config,
    c: &Connection,
    user: &User,
    q: &str,
    limit: usize,
    offset: usize,
    gate: Option<&crate::gate::Restricted>,
) -> anyhow::Result<(usize, Vec<Recording>)> {
    let granted = crate::databases::granted(c, user, "view")?;
    let deidentified = crate::patient::without_phi(c, user)?;
    let words: Vec<String> = q.split_whitespace().map(|w| w.to_lowercase()).collect();
    let any = granted.iter().any(|d| deidentified.contains(d));
    let gate = if any {
        crate::search::session_gate(c, cfg, &User { privacy: true, ..user.clone() }, gate)?
    } else {
        crate::search::SessionGate::None
    };
    let offsets = gate.get().map(crate::search::Offsets::of_gate);
    let mut all = vec![];
    for s in crate::sources::list(c)? {
        if !granted.contains(&s.id) || count(cfg, c, s.id)? == 0 {
            continue;
        }
        let shown = deidentified.contains(&s.id);
        for r in of_database(cfg, c, &s)? {
            let hay = match gate.get().filter(|_| shown) {
                Some(g) => {
                    let label = g.text(&r.label);
                    let label = g.dict.scrub_any_names(&label).unwrap_or(label);
                    let token = crate::gate::recording_token(r.id);
                    format!("{token} {label} {} {} {} {}", r.format, r.status.as_str(), r.source, r.kind)
                }
                None => format!("{} {} {} {} {} {}", r.name, r.label, r.format, r.status.as_str(), r.source, r.kind),
            }
            .to_lowercase();
            if words.iter().all(|w| hay.contains(w)) {
                let at = match (&offsets, shown) {
                    (Some(o), true) => r.t_start.map(|t| o.shift(r.patient, t)),
                    _ => r.t_start,
                };
                all.push((at, r));
            }
        }
    }
    all.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.id.cmp(&a.1.id)));
    let total = all.len();
    Ok((total, all.into_iter().skip(offset).take(limit).map(|(_, r)| r).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_split_back() {
        assert_eq!(split(global(7, 123)), (7, 123));
        assert_eq!(split(global(1, 1)), (1, 1));
        assert!(global(9_000_000, STRIDE - 1) < (1i64 << 53), "within a browser's exact integers");
    }
}
