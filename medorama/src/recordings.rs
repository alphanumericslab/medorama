//! Recordings: import (by path, never copied), re-check, status, removal.
//!
//! The catalog only reads recordings.  Removing one deletes its database entries (and, if
//! asked, the catalog's own results for it); the file or folder itself is never touched.

use crate::config::{real, within, Config};
use crate::db::{audit, now};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use recording_engine::index::cache_key;
use recording_engine::sources::{kind_of, unreadable};
use serde::Serialize;
use std::path::{Path, PathBuf};

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
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        [Status::Ready, Status::Queued, Status::Indexing, Status::New, Status::Stale, Status::Failed, Status::Missing]
            .into_iter()
            .find(|x| x.as_str() == s)
    }

    /// Ordering for an encounter's summary: the most urgent of its recordings.
    fn severity(self) -> u8 {
        match self {
            Status::Ready => 0,
            Status::New => 1,
            Status::Stale => 2,
            Status::Queued => 3,
            Status::Indexing => 4,
            Status::Failed => 5,
            Status::Missing => 6,
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
    pub path: String,
    pub kind: String,
    pub name: String,
    pub size: i64,
    pub unit: String,
    pub bed: String,
    pub name_time: Option<f64>,
    /// The recording's format, as its reader names it (`WFDB collection`, an adapter's description).
    pub format: String,
    pub t_start: Option<f64>,
    pub t_end: Option<f64>,
    pub present: bool,
    pub status: Status,
    /// Indexing progress 0..1 while indexing.
    pub progress: f64,
    pub error: String,
    /// The index key (the viewers' cache key) when ready.
    pub index_key: Option<String>,
    pub fingerprint: String,
    pub added_at: f64,
    /// Filled by [`list`]: the sources it came from, the encounters it is linked to, and (PHI
    /// users) the identifiers in its file.
    pub sources: Vec<String>,
    pub encounters: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_file: Option<crate::identify::InFile>,
}

/// Columns for [`row_recording`]; `j` is the newest index job of the recording.
pub const REC_SELECT: &str = "SELECT r.id, r.path, r.kind, r.name, r.size, r.unit, r.bed, r.name_time, r.format,
    r.t_start, r.t_end, r.present, r.fingerprint, r.added_at,
    j.state, j.progress, j.error, j.fingerprint, j.action_version
  FROM recordings r
  LEFT JOIN jobs j ON j.id = (SELECT MAX(id) FROM jobs WHERE recording = r.id AND action = 'index')";

pub fn row_recording(r: &crate::sql::Row) -> crate::sql::Result<Recording> {
    let present = r.get::<_, i64>(11)? != 0;
    let fingerprint: String = r.get(12)?;
    let state: Option<String> = r.get(14)?;
    let job_fp: Option<String> = r.get(17)?;
    let job_version: Option<String> = r.get(18)?;
    let current = job_fp.as_deref() == Some(fingerprint.as_str())
        && job_version.as_deref() == Some(crate::actions::INDEX_VERSION_STR.as_str());
    let status = if !present {
        Status::Missing
    } else {
        let error: Option<String> = r.get(16)?;
        match state.as_deref() {
            // indexing cancelled by a user (jobs::cancel): not indexed again on its own (it
            // shows as failed, said so, until indexed again); cancelled by the catalog (its index
            // folder removed): new
            Some("cancelled") if current && error.as_deref() == Some("cancelled") => Status::Failed,
            None | Some("cancelled") => Status::New,
            Some("queued") => Status::Queued,
            Some("running") => Status::Indexing,
            Some(_) if !current => Status::Stale,
            Some("done") => Status::Ready,
            _ => Status::Failed,
        }
    };
    Ok(Recording {
        id: r.get(0)?,
        path: r.get(1)?,
        kind: r.get(2)?,
        name: r.get(3)?,
        size: r.get(4)?,
        unit: r.get(5)?,
        bed: r.get(6)?,
        name_time: r.get(7)?,
        format: r.get(8)?,
        t_start: r.get(9)?,
        t_end: r.get(10)?,
        present,
        progress: if status == Status::Indexing { r.get(15)? } else { 0.0 },
        error: if status == Status::Failed {
            match (state.as_deref(), r.get::<_, Option<String>>(16)?.unwrap_or_default()) {
                (Some("cancelled"), _) => "indexing was cancelled: index it to index it again".into(),
                (_, e) => e,
            }
        } else {
            String::new()
        },
        index_key: if status == Status::Ready { Some(fingerprint.clone()) } else { None },
        fingerprint,
        status,
        added_at: r.get(13)?,
        sources: vec![],
        encounters: vec![],
        in_file: None,
    })
}

pub fn get(c: &Connection, id: i64) -> anyhow::Result<Option<Recording>> {
    Ok(c.query_row(&format!("{REC_SELECT} WHERE r.id = ?1"), [id], row_recording).optional()?)
}

/// `UNIT_BED-<Unix time>`, the way bedside-monitor recordings are often named: (unit, bed,
/// time).  The unit is the part before the first `_`; a name without the time gives nothing.
pub fn parse_name(name: &str) -> (String, String, Option<f64>) {
    let stem = Path::new(name).file_stem().map_or(name.to_string(), |s| s.to_string_lossy().to_string());
    let Some((label, time)) = stem.rsplit_once('-') else { return (String::new(), String::new(), None) };
    if !(9..=11).contains(&time.len()) || !time.bytes().all(|b| b.is_ascii_digit()) || label.is_empty() {
        return (String::new(), String::new(), None);
    }
    let (unit, bed) = label.split_once('_').unwrap_or((label, ""));
    (unit.to_string(), bed.to_string(), time.parse::<f64>().ok())
}

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub import_id: i64,
    pub dry_run: bool,
    pub added: usize,
    pub already: usize,
    pub skipped: Vec<(String, String)>,
    pub linked: usize,
    /// Added recordings whose current index was already in index_dir.
    pub indexed: usize,
}

fn size_of(p: &Path) -> i64 {
    if p.is_dir() {
        std::fs::read_dir(p)
            .map(|it| it.filter_map(Result::ok).filter_map(|e| e.metadata().ok()).map(|m| m.len() as i64).sum())
            .unwrap_or(0)
    } else {
        p.metadata().map_or(0, |m| m.len() as i64)
    }
}

/// True if writing to folder `w` would write into the recording's folder: the folder holding
/// a recording file, anywhere inside a WFDB folder, or a folder inside which the recording lies.
fn writes_into(w: &Path, rec: &Path, kind: &str) -> bool {
    // a WFDB folder: anywhere inside it; a file (a WFDB record's header too): its own folder
    let into_source = if kind == "wfdb" && rec.is_dir() { within(w, rec) } else { rec.parent() == Some(w) };
    into_source || within(rec, w)
}

/// A recording found, of `kind`: to add, or skipped with the reason when this build cannot read
/// it (a WFDB recording in a build without the `wfdb` feature).
fn take(p: PathBuf, kind: String, out: &mut Vec<(PathBuf, String)>, skipped: &mut Vec<(String, String)>) {
    match unreadable(&p) {
        Some(why) => skipped.push((p.to_string_lossy().to_string(), why.into())),
        None => out.push((p, kind)),
    }
}

/// Recordings (WFDB folders, and files an installed adapter reads) at or under `path`.
fn find(path: &Path, recursive: bool, out: &mut Vec<(PathBuf, String)>, skipped: &mut Vec<(String, String)>) {
    if let Some(k) = kind_of(path) {
        take(path.to_path_buf(), k, out, skipped);
        return;
    }
    if !path.is_dir() {
        skipped.push((path.to_string_lossy().to_string(), "not a recording this program reads".into()));
        return;
    }
    let Ok(it) = std::fs::read_dir(path) else {
        skipped.push((path.to_string_lossy().to_string(), "cannot read the folder".into()));
        return;
    };
    let mut entries: Vec<PathBuf> = it
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    entries.sort();
    for e in entries {
        // a symbolic link in a folder is not followed (it may lead out of the data folders)
        let is_link = e.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(true);
        if is_link {
            skipped.push((e.to_string_lossy().to_string(), "a symbolic link: not followed".into()));
        } else if let Some(k) = kind_of(&e) {
            take(e, k, out, skipped);
        } else if recursive && e.is_dir() {
            find(&e, true, out, skipped);
        }
    }
}

/// Where a batch of recordings lies: the folder given, or the folder holding the files given;
/// for several, the deepest folder holding them all.  The default name and source of a batch.
pub fn data_folder(paths: impl IntoIterator<Item = PathBuf>) -> String {
    let mut common: Option<PathBuf> = None;
    for p in paths {
        let folder = if p.is_dir() { p } else { p.parent().map(Path::to_path_buf).unwrap_or(p) };
        common = Some(match common {
            None => folder,
            Some(c) => c.components().zip(folder.components()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect(),
        });
    }
    common.map(|c| c.to_string_lossy().to_string()).unwrap_or_default()
}

/// Add the recordings at or under each path (admins only).
pub fn import(
    c: &mut Connection,
    cfg: &Config,
    user: &User,
    paths: &[String],
    recursive: bool,
    opts: &crate::imports::Opts,
) -> anyhow::Result<ImportReport> {
    if !user.is_admin() {
        anyhow::bail!("only admins add recordings");
    }
    let mut rep = ImportReport { dry_run: opts.dry_run, ..Default::default() };
    let default_name = data_folder(paths.iter().map(|p| PathBuf::from(recording_engine::paths::absolute(p))));
    let opts = crate::imports::Opts::new(
        if opts.name.is_empty() { &default_name } else { &opts.name },
        &opts.source,
        opts.dry_run,
    );
    let mut found = vec![];
    for p in paths {
        let abs = PathBuf::from(recording_engine::paths::absolute(p));
        if !abs.exists() {
            rep.skipped.push((p.clone(), "no such file or folder".into()));
            continue;
        }
        if !cfg.import_allowed(&abs) {
            rep.skipped.push((p.clone(), "outside the configured data_roots".into()));
            continue;
        }
        find(&abs, recursive, &mut found, &mut rep.skipped);
    }
    let tx = c.transaction()?;
    let batch = crate::imports::begin(&tx, "recordings", &opts.name, &opts.source, &paths.join("; "), user)?;
    rep.import_id = batch;
    let new_ids = add_found(&tx, cfg, user, found, batch, &opts.source, &mut rep)?;
    crate::imports::finish(
        &tx,
        batch,
        &serde_json::json!({"added": rep.added, "already": rep.already, "skipped": rep.skipped.len()}),
    )?;
    if opts.dry_run {
        tx.rollback()?;
        return Ok(rep);
    }
    tx.commit()?;
    for id in &new_ids {
        adopt_index(c, cfg, *id)?;
    }
    for id in &new_ids {
        if get(c, *id)?.is_some_and(|r| r.status == Status::Ready) {
            rep.indexed += 1;
        }
    }
    rep.linked = crate::linkage::relink(c, Some(&new_ids))?;
    audit(
        c,
        Some(user),
        "import_recordings",
        &paths.join("; "),
        &format!("added {}, already {}", rep.added, rep.already),
    );
    Ok(rep)
}

/// Add recordings found on disk to the catalog as part of batch `batch` (source `source`);
/// returns the ids of the new ones.  Recordings already there are claimed by the batch too.
pub(crate) fn add_found(
    tx: &Connection,
    cfg: &Config,
    user: &User,
    found: Vec<(PathBuf, String)>,
    batch: i64,
    source: &str,
    rep: &mut ImportReport,
) -> anyhow::Result<Vec<i64>> {
    let written = cfg.written_folders();
    let t = now();
    let mut new_ids = vec![];
    for (p, kind) in found {
        let rp = real(&p);
        // where it really is (a symbolic link in a data folder may lead out of it)
        if !cfg.import_allowed(&rp) {
            rep.skipped.push((p.to_string_lossy().to_string(), "outside the configured data_roots".into()));
            continue;
        }
        if let Some(w) = written.iter().find(|w| writes_into(w, &rp, &kind)) {
            rep.skipped.push((
                rp.to_string_lossy().to_string(),
                format!("the catalog writes to {}: the recording's own folder, or a folder containing it", w.display()),
            ));
            continue;
        }
        let path = rp.to_string_lossy().to_string();
        let exists: Option<i64> =
            tx.query_row("SELECT id FROM recordings WHERE path = ?1", [&path], |r| r.get(0)).optional()?;
        if let Some(id) = exists {
            // already in the catalog: this batch (and its source) claims it too
            crate::imports::note(tx, batch, source, "recording", id)?;
            rep.already += 1;
            continue;
        }
        let name = rp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (unit, bed, name_time) = parse_name(&name);
        let id = tx.insert(
            "INSERT INTO recordings (path, kind, name, size, fingerprint, unit, bed, name_time, added_at, added_by, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9)",
            params![path, kind, name, size_of(&rp), cache_key(&rp), unit, bed, name_time, t, (user.id > 0).then_some(user.id)],
        )?;
        crate::imports::note(tx, batch, source, "recording", id)?;
        new_ids.push(id);
        rep.added += 1;
    }
    Ok(new_ids)
}

/// A path given in a database row, as recordings to add: the recordings it is or holds.
pub(crate) fn find_path(cfg: &Config, p: &str, skipped: &mut Vec<(String, String)>) -> Vec<(PathBuf, String)> {
    let abs = PathBuf::from(recording_engine::paths::absolute(p));
    let mut found = vec![];
    if !abs.exists() {
        skipped.push((p.to_string(), "no such file or folder".into()));
    } else if !cfg.import_allowed(&abs) {
        skipped.push((p.to_string(), "outside the configured data_roots".into()));
    } else {
        find(&abs, false, &mut found, skipped);
    }
    found
}

/// Record times and format from an index folder; true if it exists.
pub fn apply_index_meta(c: &Connection, id: i64, folder: &Path) -> anyhow::Result<bool> {
    let Ok(text) = std::fs::read_to_string(folder.join("meta.json")) else { return Ok(false) };
    let meta: serde_json::Value = serde_json::from_str(&text)?;
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
    c.execute(
        "UPDATE recordings SET format = ?1, t_start = ?2, t_end = ?3 WHERE id = ?4",
        params![format, lo.is_finite().then_some(lo), hi.is_finite().then_some(hi), id],
    )?;
    Ok(true)
}

/// A recording whose current index already exists in index_dir (e.g. the index folder is a
/// viewer's cache) is marked indexed without indexing it again.
pub fn adopt_index(c: &Connection, cfg: &Config, id: i64) -> anyhow::Result<()> {
    let Some(rec) = get(c, id)? else { return Ok(()) };
    if rec.status != Status::New {
        return Ok(());
    }
    let folder = cfg.index_dir.join(&rec.fingerprint);
    let Ok(text) = std::fs::read_to_string(folder.join("meta.json")) else { return Ok(()) };
    let meta: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if meta.get("index_version").and_then(|v| v.as_i64()) != Some(recording_engine::common::INDEX_VERSION) {
        return Ok(());
    }
    apply_index_meta(c, id, &folder)?;
    let derived = crate::derived::derive(&cfg.index_dir, &rec.fingerprint, &|_| {})?;
    crate::derived::store(c, id, &derived)?;
    let t = now();
    c.execute(
        "INSERT INTO jobs (action, action_version, params, params_hash, recording, fingerprint, state, progress,
                           created_at, started_at, finished_at, output_dir, summary)
         VALUES ('index', ?1, '{}', ?2, ?3, ?4, 'done', 1, ?5, ?5, ?5, ?6, '{\"adopted\":true}')",
        params![
            crate::actions::INDEX_VERSION_STR.as_str(),
            crate::actions::params_hash(&serde_json::json!({})),
            id,
            rec.fingerprint,
            t,
            folder.to_string_lossy()
        ],
    )?;
    Ok(())
}

#[derive(Debug, Default, Serialize)]
pub struct CheckReport {
    pub checked: usize,
    pub missing: usize,
    pub returned: usize,
    pub changed: usize,
    pub index_lost: usize,
}

/// Re-check every recording: present?  changed?  index folder still there?
pub fn check(c: &Connection, cfg: &Config) -> anyhow::Result<CheckReport> {
    let mut rep = CheckReport::default();
    let rows: Vec<(i64, String, bool, String)> = {
        let mut st = c.prepare("SELECT id, path, present, fingerprint FROM recordings")?;

        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?)))?
            .collect::<Result<_, _>>()?
    };
    for (id, path, present, fp) in rows {
        rep.checked += 1;
        let p = Path::new(&path);
        let t = now();
        if !p.exists() {
            if present {
                rep.missing += 1;
            }
            c.execute("UPDATE recordings SET present = 0, checked_at = ?1 WHERE id = ?2", params![t, id])?;
            continue;
        }
        if !present {
            rep.returned += 1;
        }
        let key = cache_key(p);
        if key != fp {
            rep.changed += 1;
        }
        c.execute(
            "UPDATE recordings SET present = 1, fingerprint = ?1, size = ?2, checked_at = ?3 WHERE id = ?4",
            params![key, size_of(p), t, id],
        )?;
        if let Some(rec) = get(c, id)? {
            if rec.status == Status::Ready && !cfg.index_dir.join(&rec.fingerprint).join("meta.json").is_file() {
                rep.index_lost += 1;
                c.execute(
                    "UPDATE jobs SET state = 'cancelled', error = 'index folder removed'
                     WHERE id = (SELECT MAX(id) FROM jobs WHERE recording = ?1 AND action = 'index')",
                    [id],
                )?;
            } else if rec.status == Status::New {
                adopt_index(c, cfg, id)?;
            }
        }
    }
    Ok(rep)
}

/// Remove recordings from the catalog (admins only).  With `purge`, also delete the
/// catalog's results for them (inside results_dir / index_dir only).  The recordings
/// themselves are never touched.
pub fn remove(c: &mut Connection, cfg: &Config, user: &User, ids: &[i64], purge: bool) -> anyhow::Result<usize> {
    if !user.is_admin() {
        anyhow::bail!("only admins remove entries");
    }
    let tx = c.transaction()?;
    let mut dirs = vec![];
    let mut n = 0;
    for id in ids {
        let path: Option<String> =
            tx.query_row("SELECT path FROM recordings WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        let Some(path) = path else { continue };
        if purge {
            let mut st = tx.prepare("SELECT output_dir FROM jobs WHERE recording = ?1 AND output_dir != ''")?;
            let v: Vec<String> = st.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
            dirs.extend(v);
        }
        n += tx.execute("DELETE FROM recordings WHERE id = ?1", [id])?;
        audit(&tx, Some(user), "remove_recording", &path, if purge { "with results" } else { "" });
    }
    crate::imports::prune(&tx)?;
    tx.commit()?;
    for d in dirs {
        purge_dir(cfg, Path::new(&d));
    }
    Ok(n)
}

/// How many of these recordings exist: what `remove` would remove (a dry run).
pub fn existing(c: &Connection, ids: &[i64]) -> anyhow::Result<usize> {
    let mut n = 0;
    for id in ids.iter().collect::<std::collections::BTreeSet<_>>() {
        n += c.query_row("SELECT COUNT(*) FROM recordings WHERE id = ?1", [id], |r| r.get::<_, i64>(0))? as usize;
    }
    Ok(n)
}

/// Delete a results folder, but only one inside the catalog's own output folders (and never
/// a folder a remaining recording still uses).
pub fn purge_dir(cfg: &Config, d: &Path) {
    let rd = real(d);
    let ours = [real(&cfg.results_dir), real(&cfg.index_dir)].iter().any(|root| within(&rd, root) && rd != *root);
    if ours && rd.is_dir() {
        let _ = std::fs::remove_dir_all(&rd);
    }
}

/// Recordings matching a search (search.rs: words and `field:value` terms), newest first:
/// the number of matches and one page.
#[allow(clippy::too_many_arguments)]
pub fn list(
    c: &Connection,
    user: &User,
    q: &str,
    unlinked_only: bool,
    limit: i64,
    offset: i64,
    timezone: &str,
) -> anyhow::Result<(i64, Vec<Recording>)> {
    list_in(c, user, None, q, unlinked_only, limit, offset, timezone)
}

/// [`list`] through a session's gate (Privacy Mode: built here when not given).  Searches in
/// Privacy Mode are audited (`privacy_search`).
#[allow(clippy::too_many_arguments)]
pub fn list_in(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    q: &str,
    unlinked_only: bool,
    limit: i64,
    offset: i64,
    timezone: &str,
) -> anyhow::Result<(i64, Vec<Recording>)> {
    let ids = search_ids_in(c, user, gate, q, unlinked_only, timezone)?;
    if user.privacy && !user.phi && !q.trim().is_empty() {
        crate::db::audit(c, Some(user), "privacy_search", "recordings", q);
    }
    let total = ids.len() as i64;
    let page: Vec<i64> = ids.into_iter().skip(offset.max(0) as usize).take(limit.max(0) as usize).collect();
    let mut out = vec![];
    for id in page {
        if let Some(mut r) = get(c, id)? {
            r.sources = crate::imports::sources_of(c, "recording", id)?;
            let mut st = c.prepare(
                "SELECT e.encounter_id FROM recording_links l JOIN encounters e ON e.id = l.encounter WHERE l.recording = ?1",
            )?;
            r.encounters = st.query_map([id], |x| x.get(0))?.collect::<Result<_, _>>()?;
            if user.phi {
                r.in_file = crate::identify::in_file(c, id, None)?;
            }
            out.push(r);
        }
    }
    Ok((total, out))
}

/// Ids of the recordings matching a search, newest first.
pub fn search_ids(
    c: &Connection,
    user: &User,
    q: &str,
    unlinked_only: bool,
    timezone: &str,
) -> anyhow::Result<Vec<i64>> {
    search_ids_in(c, user, None, q, unlinked_only, timezone)
}

/// [`search_ids`] through a session's gate.  In Privacy Mode it finds and orders by what the
/// session is shown (search.rs): each recording as the gate sends it (its token for its path
/// and name, its encounters' pseudonyms, its start moved back).
pub fn search_ids_in(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    q: &str,
    unlinked_only: bool,
    timezone: &str,
) -> anyhow::Result<Vec<i64>> {
    let g = crate::search::session_gate(c, user, gate)?;
    let gate = g.get();
    let terms = crate::search::parse(q);
    crate::search::check(&terms, crate::search::RECORDING_FIELDS, user.phi).map_err(anyhow::Error::msg)?;
    // (not those only an import still loading holds)
    let mut st = c.prepare(&format!(
        "{REC_SELECT} WHERE {} ORDER BY COALESCE(r.t_start, r.name_time) DESC, r.id DESC",
        crate::patient::recording_shown("r.id")
    ))?;
    let recs: Vec<Recording> = st.query_map([], row_recording)?.collect::<Result<_, _>>()?;
    let linked: std::collections::HashMap<i64, Vec<String>> = {
        let mut m: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut st = c.prepare(
            "SELECT l.recording, e.encounter_id FROM recording_links l JOIN encounters e ON e.id = l.encounter",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            m.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
        m
    };
    let sources: std::collections::HashMap<i64, Vec<String>> = {
        let mut m: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        let mut st = c.prepare("SELECT row_id, source FROM provenance WHERE kind = 'recording'")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            m.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
        m
    };
    let files: std::collections::HashMap<i64, [String; 3]> = if user.phi && !terms.is_empty() {
        let mut st = c.prepare(
            "SELECT recording, trim(full_name || ' ' || last_name || ' ' || first_name), patient_id, birth_date
             FROM recording_identifiers WHERE state = 'found'",
        )?;

        st.query_map([], |r| Ok((r.get::<_, i64>(0)?, [r.get(1)?, r.get(2)?, r.get(3)?])))?.collect::<Result<_, _>>()?
    } else {
        Default::default()
    };
    let zone = crate::util::tz(timezone);
    let recs = recs.into_iter().filter(|r| !unlinked_only || !linked.contains_key(&r.id));
    if let Some(g) = gate {
        // as shown: each recording through the gate, newest (as shown) first
        let mut found: Vec<(Option<f64>, i64)> = vec![];
        for mut r in recs {
            r.sources = sources.get(&r.id).cloned().unwrap_or_default();
            r.encounters = linked.get(&r.id).cloned().unwrap_or_default();
            let v = crate::search::shown(g, &r);
            let start = v["t_start"].as_f64().or(v["name_time"].as_f64());
            if !terms.is_empty() {
                let mut d = crate::search::Doc::default();
                for (field, key) in [
                    ("file", "name"),
                    ("path", "path"),
                    ("kind", "kind"),
                    ("format", "format"),
                    ("unit", "unit"),
                    ("bed", "bed"),
                    ("status", "status"),
                    ("source", "sources"),
                    ("encounter", "encounters"),
                ] {
                    for s in crate::search::strings(&v[key]) {
                        d.add(field, &s);
                    }
                }
                if let Some(t) = start {
                    d.add("start", &crate::util::fmt_local(t, zone));
                }
                if !d.matches(&terms, false) {
                    continue;
                }
            }
            found.push((start, r.id));
        }
        found.sort_by(|a, b| {
            b.0.is_some()
                .cmp(&a.0.is_some())
                .then(b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.1.cmp(&a.1))
        });
        return Ok(found.into_iter().map(|f| f.1).collect());
    }
    Ok(recs
        .filter(|r| {
            if terms.is_empty() {
                return true;
            }
            let mut d = crate::search::Doc::default();
            d.add("file", &r.name);
            d.add("path", &r.path);
            d.add("kind", &r.kind);
            d.add("format", &r.format);
            d.add("unit", &r.unit);
            d.add("bed", &r.bed);
            d.add("status", r.status.as_str());
            if let Some(t) = r.t_start.or(r.name_time) {
                d.add("start", &crate::util::fmt_local(t, zone));
            }
            for x in sources.get(&r.id).into_iter().flatten() {
                d.add("source", x);
            }
            for x in linked.get(&r.id).into_iter().flatten() {
                d.add("encounter", x);
            }
            if let Some([name, id, dob]) = files.get(&r.id) {
                d.add("file-name", name);
                d.add("file-id", id);
                d.add("file-dob", dob);
            }
            d.matches(&terms, user.phi)
        })
        .map(|r| r.id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let t = "1700000000";
        assert_eq!(parse_name(&format!("ICU7_12-{t}.dat")), ("ICU7".into(), "12".into(), Some(1.7e9)));
        assert_eq!(parse_name(&format!("ICU_ICU_1-{t}.dat")), ("ICU".into(), "ICU_1".into(), Some(1.7e9)));
        assert_eq!(parse_name("recording.dat"), (String::new(), String::new(), None));
        assert_eq!(parse_name("a-b.dat"), (String::new(), String::new(), None));
    }

    #[test]
    fn data_folders() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("a/x")).unwrap();
        std::fs::create_dir_all(d.join("a/y")).unwrap();
        for f in ["a/x/1.dat", "a/x/2.dat", "a/y/3.dat"] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        let show = |p: &str| d.join(p).to_string_lossy().to_string();
        assert_eq!(data_folder([d.join("a/x")]), show("a/x"));
        assert_eq!(data_folder([d.join("a/x/1.dat"), d.join("a/x/2.dat")]), show("a/x"));
        assert_eq!(data_folder([d.join("a/x/1.dat"), d.join("a/y/3.dat")]), show("a"));
        assert_eq!(data_folder(Vec::<PathBuf>::new()), "");
    }
}
