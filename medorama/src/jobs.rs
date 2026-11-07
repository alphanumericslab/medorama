//! The job queue: a run (actions x a selection of encounters or recordings) becomes one job
//! per action and recording; workers (in the server, or `medorama work`) take
//! jobs from the queue, highest priority first.
//!
//! - A job is not queued again when the same action version with the same parameters has
//!   already succeeded on the unchanged recording (unless forced), or is queued or running.
//! - Actions an action requires (e.g. `index`) are added to the run when needed; a job waits
//!   until they have succeeded on the recording, and fails if one of them failed.
//! - A worker updates its job's heartbeat every few seconds; a running job whose heartbeat
//!   stops (a worker that crashed or was killed) goes back to the queue.  A heartbeat the
//!   database refuses is tried again: only the job's row stops a job (cancelled, or taken back).
//! - A job's end is written again until the database takes it.  A worker that stops (the
//!   server or `medorama work` stopping) stops its plug-in (its whole process group: TERM,
//!   then KILL) and puts the job back in the queue; a job run again has a new output folder
//!   (`j<id>-2`, …).

use crate::actions::{job_description, merge_params, params_hash, run_external, run_index, Progress, Registry};
use crate::config::Config;
use crate::db::{now, Db};
use crate::query::{search_ids, Search};
use crate::recordings::{self, Recording, Status};
use crate::sql::{params, Connection, OptionalExtension, TransactionBehavior};
use crate::users::User;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// A running job whose heartbeat is older than this goes back to the queue.
const STALE_S: f64 = 120.0;
const HEARTBEAT: Duration = Duration::from_secs(2);
/// Priority of indexing asked for by a person (e.g. "Index now"); automatic indexing is 0.
pub const PRIORITY_NOW: i64 = 10;

/// What a run applies to: encounters, recordings, the encounters matching a search or of a
/// cohort's members, or all recordings.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Selection {
    /// The encounters of a cohort's members (one the user may see).
    pub cohort: Option<i64>,
    pub encounters: Option<Vec<i64>>,
    pub recordings: Option<Vec<i64>>,
    pub search: Option<Search>,
    pub all: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct RunReport {
    pub run: i64,
    pub recordings: usize,
    pub queued: usize,
    /// Already done on the unchanged recording.
    pub done_already: usize,
    /// Already queued or running.
    pub pending_already: usize,
    /// The action does not apply to the recording's kind, or the recording is missing.
    pub not_applicable: usize,
}

fn selected_recordings(c: &Connection, user: &User, sel: &Selection, timezone: &str) -> anyhow::Result<Vec<i64>> {
    let mut ids: Vec<i64> = vec![];
    let mut encs: Vec<i64> = sel.encounters.clone().unwrap_or_default();
    if let Some(s) = &sel.search {
        encs.extend(search_ids(c, user, s, timezone)?);
    }
    if let Some(co) = sel.cohort {
        let members = crate::cohorts::member_ids(c, user, co)?;
        let mut st = c.prepare("SELECT id, patient FROM encounters ORDER BY id")?;
        for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (e, p) = r?;
            if members.contains(&p) {
                encs.push(e);
            }
        }
    }
    if sel.all {
        let mut st = c.prepare("SELECT id FROM recordings ORDER BY id")?;
        ids.extend(st.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?);
    }
    if let Some(r) = &sel.recordings {
        ids.extend(r);
    }
    let mut st = c.prepare("SELECT recording FROM recording_links WHERE encounter = ?1 ORDER BY recording")?;
    for e in encs {
        ids.extend(st.query_map([e], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?);
    }
    let mut seen = HashSet::new();
    ids.retain(|i| seen.insert(*i));
    Ok(ids)
}

/// The actions with those they require, required ones first.
fn with_requirements(reg: &Registry, names: &[String]) -> anyhow::Result<Vec<(String, bool)>> {
    let mut out: Vec<(String, bool)> = vec![];
    fn add(reg: &Registry, n: &str, explicit: bool, out: &mut Vec<(String, bool)>, depth: usize) -> anyhow::Result<()> {
        if depth > 16 {
            anyhow::bail!("actions require each other in a circle ({n})");
        }
        let a = reg.get(n).ok_or_else(|| anyhow::anyhow!("no action named {n}"))?;
        if !a.problem.is_empty() {
            anyhow::bail!("{n}: {}", a.problem);
        }
        for q in &a.requires {
            add(reg, q, false, out, depth + 1)?;
        }
        match out.iter_mut().find(|(m, _)| m == n) {
            Some(e) => e.1 |= explicit,
            None => out.push((n.to_string(), explicit)),
        }
        Ok(())
    }
    for n in names {
        add(reg, n, true, &mut out, 0)?;
    }
    Ok(out)
}

/// Queue a run (admins and analysts).  `params`: per action name, parameters overriding the
/// action's defaults.  `force` re-runs the chosen actions even where they are done (not the
/// actions they require).
#[allow(clippy::too_many_arguments)]
pub fn create_run(
    c: &mut Connection,
    reg: &Registry,
    user: &User,
    actions: &[String],
    params_by_action: &HashMap<String, serde_json::Value>,
    sel: &Selection,
    force: bool,
    priority: i64,
    timezone: &str,
) -> anyhow::Result<RunReport> {
    if !user.can_run() {
        anyhow::bail!("running actions needs the analyst or admin role");
    }
    if actions.is_empty() {
        anyhow::bail!("no action chosen");
    }
    let plan = with_requirements(reg, actions)?;
    let recs = selected_recordings(c, user, sel, timezone)?;
    let t = now();
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let run = tx.insert(
        "INSERT INTO runs (user_id, created_at, actions, selection, force) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            (user.id > 0).then_some(user.id),
            t,
            serde_json::to_string(actions)?,
            serde_json::to_string(sel)?,
            force as i64
        ],
    )?;
    let mut rep = RunReport { run, recordings: recs.len(), ..Default::default() };
    for rid in recs {
        let Some(rec) = recordings::get(&tx, rid)? else { continue };
        for (name, explicit) in &plan {
            let a = reg.get(name).unwrap();
            if !rec.present || !a.applies_to(&rec.kind) {
                rep.not_applicable += 1;
                continue;
            }
            let p = merge_params(&a.params, params_by_action.get(name));
            let ph = params_hash(&p);
            let pending: Option<i64> = tx
                .query_row(
                    "SELECT id FROM jobs WHERE recording = ?1 AND action = ?2 AND action_version = ?3 AND params_hash = ?4
                       AND state IN ('queued', 'running') LIMIT 1",
                    params![rid, name, a.version, ph],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(j) = pending {
                if priority > 0 {
                    tx.execute("UPDATE jobs SET priority = MAX(priority, ?1) WHERE id = ?2", params![priority, j])?;
                }
                rep.pending_already += 1;
                continue;
            }
            if !(force && *explicit) {
                let done: Option<i64> = tx
                    .query_row(
                        "SELECT id FROM jobs WHERE recording = ?1 AND action = ?2 AND action_version = ?3 AND params_hash = ?4
                           AND fingerprint = ?5 AND state = 'done' LIMIT 1",
                        params![rid, name, a.version, ph, rec.fingerprint],
                        |r| r.get(0),
                    )
                    .optional()?;
                if done.is_some() {
                    rep.done_already += 1;
                    continue;
                }
            }
            tx.execute(
                "INSERT INTO jobs (run, action, action_version, params, params_hash, recording, fingerprint, state, priority, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9)",
                params![run, name, a.version, p.to_string(), ph, rid, rec.fingerprint, priority, t],
            )?;
            rep.queued += 1;
        }
    }
    tx.commit()?;
    crate::db::audit(c, Some(user), "run", &actions.join(","), &format!("run {run}: {} jobs queued", rep.queued));
    Ok(rep)
}

/// Queue indexing for every present recording that is new or changed (automatic indexing);
/// one server at a time, when several share a PostgreSQL database.
pub fn queue_auto_index(c: &Connection) -> anyhow::Result<usize> {
    c.exclusive(crate::sql::Lock::Schedule, || queue_auto_index_now(c))?
}

fn queue_auto_index_now(c: &Connection) -> anyhow::Result<usize> {
    let mut st = c.prepare(recordings::REC_SELECT)?;
    let todo: Vec<Recording> = st
        .query_map([], recordings::row_recording)?
        .filter_map(Result::ok)
        .filter(|r| matches!(r.status, Status::New | Status::Stale))
        .collect();
    let ph = params_hash(&serde_json::json!({}));
    let t = now();
    for r in &todo {
        c.execute(
            "INSERT INTO jobs (action, action_version, params, params_hash, recording, fingerprint, state, priority, created_at)
             VALUES ('index', ?1, '{}', ?2, ?3, ?4, 'queued', 0, ?5)",
            params![crate::actions::INDEX_VERSION_STR.as_str(), ph, r.id, r.fingerprint, t],
        )?;
    }
    // identifiers in the files: once per recording (again when it changed)
    let n = c.execute(
        "INSERT INTO jobs (action, action_version, params, params_hash, recording, fingerprint, state, priority, created_at)
         SELECT 'identify', '1', '{}', ?1, r.id, r.fingerprint, 'queued', 0, ?2 FROM recordings r
         WHERE r.present = 1
           AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.recording = r.id AND j.action = 'identify'
                           AND (j.state IN ('queued', 'running') OR (j.fingerprint = r.fingerprint AND j.state IN ('done', 'failed'))))",
        params![ph, t],
    )?;
    Ok(todo.len() + n)
}

#[derive(Debug, Clone)]
pub struct Claimed {
    pub id: i64,
    pub action: String,
    pub params: serde_json::Value,
    pub recording: i64,
}

enum Ready {
    Yes,
    Wait,
    Fail(String),
}

/// Have the actions `action` requires succeeded on the recording as it is now?
fn requirements(c: &Connection, reg: &Registry, action: &str, rec: &Recording) -> anyhow::Result<Ready> {
    let Some(a) = reg.get(action) else { return Ok(Ready::Fail(format!("action {action} is no longer installed"))) };
    for q in &a.requires {
        let version = reg.get(q).map(|x| x.version.clone()).unwrap_or_default();
        let last: Option<(String, String, String)> = c
            .query_row(
                "SELECT state, fingerprint, action_version FROM jobs WHERE recording = ?1 AND action = ?2 ORDER BY id DESC LIMIT 1",
                params![rec.id, q],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match last {
            Some((s, fp, v)) if s == "done" && fp == rec.fingerprint && v == version => {}
            Some((s, _, _)) if s == "queued" || s == "running" => return Ok(Ready::Wait),
            Some((s, fp, _)) if s == "failed" && fp == rec.fingerprint => {
                return Ok(Ready::Fail(format!("requires {q}, which failed")))
            }
            _ => return Ok(Ready::Fail(format!("requires {q}, which has not run on the current recording"))),
        }
    }
    Ok(Ready::Yes)
}

/// Take the next job that can run now, if any.
pub fn claim(c: &mut Connection, reg: &Registry, worker: &str) -> anyhow::Result<Option<Claimed>> {
    let t = now();
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // servers sharing a PostgreSQL database take jobs one at a time
    tx.lock_in_transaction(crate::sql::Lock::ClaimJob)?;
    tx.execute(
        "UPDATE jobs SET state = 'queued', worker = '', progress = 0 WHERE state = 'running' AND (heartbeat IS NULL OR heartbeat < ?1)",
        [t - STALE_S],
    )?;
    let cands: Vec<(i64, String, String, i64)> = {
        let mut st = tx.prepare("SELECT id, action, params, recording FROM jobs WHERE state = 'queued' ORDER BY priority DESC, id LIMIT 500")?;

        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?
    };
    let mut chosen = None;
    for (id, action, params, rid) in cands {
        let Some(rec) = recordings::get(&tx, rid)? else { continue };
        if !rec.present {
            tx.execute(
                "UPDATE jobs SET state = 'failed', error = 'the recording is missing', finished_at = ?1 WHERE id = ?2",
                params![t, id],
            )?;
            continue;
        }
        match requirements(&tx, reg, &action, &rec)? {
            Ready::Wait => continue,
            Ready::Fail(e) => {
                tx.execute(
                    "UPDATE jobs SET state = 'failed', error = ?1, finished_at = ?2 WHERE id = ?3",
                    params![e, t, id],
                )?;
            }
            Ready::Yes => {
                tx.execute(
                    "UPDATE jobs SET state = 'running', worker = ?1, heartbeat = ?2, started_at = ?2, progress = 0, fingerprint = ?3
                     WHERE id = ?4",
                    params![worker, t, rec.fingerprint, id],
                )?;
                chosen = Some(Claimed {
                    id,
                    action,
                    params: serde_json::from_str(&params).unwrap_or_default(),
                    recording: rid,
                });
                break;
            }
        }
    }
    tx.commit()?;
    Ok(chosen)
}

/// A job's output folder: `j<id>` its first time, `j<id>-<attempt>` when run again (a job taken
/// back to the queue never writes over what an earlier attempt left).
pub fn output_dir(base: &Path, job: i64) -> PathBuf {
    let first = base.join(format!("j{job}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|k| base.join(format!("j{job}-{k}"))).find(|p| !p.exists()).expect("a free folder")
}

/// Whether a heartbeat's answer stops the job: only the job's row says so (cancelled, done
/// elsewhere, or no longer this worker's); a heartbeat the database refused does not.
fn beat_stops(state: &Result<Option<String>, String>) -> bool {
    match state {
        Ok(Some(s)) => s != "running",
        Ok(None) => true,
        Err(_) => false,
    }
}

/// Retried with waits of 1, 2, 4, 8 and 16 seconds while the database refuses it (busy, or
/// its connection lost): a job's end must be written.
fn retried<T>(what: &str, mut f: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut wait = Duration::from_secs(1);
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if wait > Duration::from_secs(16) => return Err(e),
            Err(e) => {
                eprintln!("medorama: {what}: {e}; trying again in {} s", wait.as_secs());
                std::thread::sleep(wait);
                wait *= 2;
            }
        }
    }
}

/// Run one claimed job to the end, keeping its heartbeat and progress up to date.  When `stop`
/// is set (the worker stopping), the job is stopped and put back in the queue.
pub fn execute(
    db: &Db,
    cfg: &Config,
    reg: &Registry,
    job: &Claimed,
    worker: &str,
    stop: &AtomicBool,
) -> anyhow::Result<()> {
    let (rec, encounters) = {
        let c = db.get()?;
        let rec = recordings::get(&c, job.recording)?
            .ok_or_else(|| anyhow::anyhow!("recording {} removed", job.recording))?;
        let mut st = c.prepare(
            // pseudonyms only: the imported IDs may be MRNs or account numbers (docs/security.md)
            "SELECT COALESCE(e.pseudonym, ''), COALESCE(p.pseudonym, '') FROM recording_links l JOIN encounters e ON e.id = l.encounter
             JOIN patients p ON p.id = e.patient WHERE l.recording = ?1 ORDER BY e.admit_time",
        )?;
        let encs: Vec<(String, String)> =
            st.query_map([job.recording], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        (rec, encs)
    };
    let progress = Progress::default();
    let cancel = AtomicBool::new(false);
    let finished = AtomicBool::new(false);
    let outcome = std::thread::scope(|s| {
        s.spawn(|| {
            let mut last = std::time::Instant::now();
            while !finished.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                if stop.load(Ordering::Relaxed) {
                    cancel.store(true, Ordering::Relaxed);
                }
                if last.elapsed() < HEARTBEAT {
                    continue;
                }
                last = std::time::Instant::now();
                // the row's word only: a heartbeat the database refuses (busy, a lost
                // connection) is tried again at the next beat, the job running on
                let state = db.get().map_err(|e| e.to_string()).and_then(|c| {
                    c.query_row(
                        "UPDATE jobs SET heartbeat = ?1, progress = ?2 WHERE id = ?3 AND worker = ?4 RETURNING state",
                        params![now(), progress.get(), job.id, worker],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())
                });
                if let Err(e) = &state {
                    eprintln!("medorama: job {}: its heartbeat: {e} (tried again)", job.id);
                }
                if beat_stops(&state) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        });
        let r = match reg.get(&job.action) {
            None => Err(format!("action {} is no longer installed", job.action)),
            Some(a) if a.builtin && a.name == "index" => run_index(cfg, &rec, &progress),
            Some(a) if a.builtin && a.name == "identify" => db
                .get()
                .map_err(|e| e.to_string())
                .and_then(|c| crate::identify::identify(&c, rec.id, Path::new(&rec.path), &rec.kind))
                .map(|summary| crate::actions::Outcome {
                    output_dir: PathBuf::new(),
                    summary: summary.to_string(),
                    derived: vec![],
                }),
            Some(a) => {
                let out =
                    output_dir(&cfg.results_dir.join("actions").join(&a.name).join(format!("r{}", rec.id)), job.id);
                let idx = cfg.index_dir.join(&rec.fingerprint);
                let idx = idx.join("meta.json").is_file().then_some(idx);
                let desc = job_description(job.id, a, &job.params, &rec, idx.as_deref(), &encounters, &out);
                run_external(a, &desc, &out, &progress, &cancel)
            }
        };
        finished.store(true, Ordering::Relaxed);
        r
    });
    let t = now();
    match outcome {
        Err(_) if stop.load(Ordering::Relaxed) => {
            // the worker stops: the job goes back to the queue, to run again later
            retried(&format!("job {}: putting it back in the queue", job.id), || {
                Ok(db.get()?.execute(
                    "UPDATE jobs SET state = 'queued', worker = '', progress = 0 WHERE id = ?1 AND worker = ?2 AND state = 'running'",
                    params![job.id, worker],
                )?)
            })?;
        }
        Ok(o) => {
            let n = retried(&format!("job {}: recording its end", job.id), || {
                Ok(db.get()?.execute(
                    "UPDATE jobs SET state = 'done', progress = 1, finished_at = ?1, output_dir = ?2, summary = ?3, error = ''
                     WHERE id = ?4 AND state = 'running' AND worker = ?5",
                    params![t, o.output_dir.to_string_lossy(), o.summary, job.id, worker],
                )?)
            })?;
            if n == 1 && job.action == "index" {
                let mut c = db.get()?;
                recordings::apply_index_meta(&c, rec.id, &o.output_dir)?;
                crate::derived::store(&c, rec.id, &o.derived)?;
                crate::linkage::relink(&mut c, Some(&[rec.id]))?;
            }
        }
        Err(e) => {
            retried(&format!("job {}: recording its end", job.id), || {
                Ok(db.get()?.execute(
                    "UPDATE jobs SET state = CASE WHEN state = 'running' THEN 'failed' ELSE state END, error = ?1, finished_at = ?2
                     WHERE id = ?3 AND worker = ?4",
                    params![e, t, job.id, worker],
                )?)
            })?;
        }
    }
    Ok(())
}

/// Cancel queued or running jobs: by id, or every unfinished job of a run.  Admins may cancel
/// any; others only jobs of their own runs.
pub fn cancel(c: &Connection, user: &User, job: Option<i64>, run: Option<i64>) -> anyhow::Result<usize> {
    if !user.can_run() {
        anyhow::bail!("cancelling jobs needs the analyst or admin role");
    }
    let own = if user.is_admin() { "1 = 1" } else { "run IN (SELECT id FROM runs WHERE user_id = ?3)" };
    let n = c.execute(
        &format!(
            "UPDATE jobs SET state = 'cancelled', finished_at = ?4, error = 'cancelled'
             WHERE state IN ('queued', 'running') AND (CAST(?1 AS INTEGER) IS NULL OR id = ?1) AND (CAST(?2 AS INTEGER) IS NULL OR run = ?2) AND {own}"
        ),
        params![job, run, user.id, now()],
    )?;
    Ok(n)
}

#[derive(Debug, Serialize)]
pub struct JobView {
    pub id: i64,
    pub run: Option<i64>,
    pub action: String,
    pub version: String,
    pub recording: i64,
    pub recording_name: String,
    pub state: String,
    pub priority: i64,
    pub progress: f64,
    pub created_at: f64,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub error: String,
    pub output_dir: String,
}

/// Jobs, newest first: unfinished ones, or (with `state`) those in one state.
pub fn list(c: &Connection, state: Option<&str>, run: Option<i64>, limit: i64) -> anyhow::Result<Vec<JobView>> {
    let mut st = c.prepare(
        "SELECT j.id, j.run, j.action, j.action_version, j.recording, r.name, j.state, j.priority, j.progress, j.created_at,
                j.started_at, j.finished_at, j.error, j.output_dir
         FROM jobs j JOIN recordings r ON r.id = j.recording
         WHERE (CAST(?1 AS TEXT) IS NULL OR j.state = ?1) AND (CAST(?2 AS INTEGER) IS NULL OR j.run = ?2)
         ORDER BY CASE j.state WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END, j.priority DESC, j.id DESC LIMIT ?3",
    )?;
    let v = st
        .query_map(params![state, run, limit], |r| {
            Ok(JobView {
                id: r.get(0)?,
                run: r.get(1)?,
                action: r.get(2)?,
                version: r.get(3)?,
                recording: r.get(4)?,
                recording_name: r.get(5)?,
                state: r.get(6)?,
                priority: r.get(7)?,
                progress: r.get(8)?,
                created_at: r.get(9)?,
                started_at: r.get(10)?,
                finished_at: r.get(11)?,
                error: r.get(12)?,
                output_dir: r.get(13)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Number of jobs per state.
pub fn counts(c: &Connection) -> anyhow::Result<HashMap<String, i64>> {
    let mut st = c.prepare("SELECT state, COUNT(*) FROM jobs GROUP BY state")?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// A name for this worker in the jobs table: the machine's name, the process, the worker.
pub fn worker_name(n: usize) -> String {
    format!("{}:{}:{n}", host_name(), std::process::id())
}

/// The machine's name (that of `hostname`, else the environment's), as workers name it.
pub fn host_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        let from_program = std::process::Command::new("hostname")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        from_program
            .filter(|h| !h.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
            .or_else(|| std::env::var("COMPUTERNAME").ok().filter(|h| !h.is_empty()))
            .unwrap_or_else(|| "localhost".into())
            .replace(':', "-")
    })
    .clone()
}

/// The jobs this process's workers were running, back in the queue (after they stopped).
pub fn requeue_own(c: &Connection) -> anyhow::Result<usize> {
    let me = worker_name(0);
    let prefix = &me[..me.rfind(':').unwrap_or(me.len())];
    Ok(c.execute(
        "UPDATE jobs SET state = 'queued', worker = '', progress = 0 WHERE state = 'running' AND worker LIKE ?1",
        [format!("{prefix}:%")],
    )?)
}

/// A worker: takes and runs jobs until `stop` is set (or, with `until_idle`, until no job can
/// run).
pub fn work(
    db: Arc<Db>,
    cfg: Arc<Config>,
    reg: Arc<RwLock<Registry>>,
    n: usize,
    stop: Arc<AtomicBool>,
    until_idle: bool,
) {
    let name = worker_name(n);
    while !stop.load(Ordering::Relaxed) {
        let reg_now = reg.read().unwrap().clone();
        let job = db.get().and_then(|mut c| claim(&mut c, &reg_now, &name));
        match job {
            Ok(Some(j)) => {
                if let Err(e) = execute(&db, &cfg, &reg_now, &j, &name, &stop) {
                    eprintln!("medorama: job {}: {e}", j.id);
                }
            }
            Ok(None) if until_idle => {
                // done unless other workers' jobs may still release waiting ones
                let running: i64 = db
                    .get()
                    .and_then(|c| {
                        Ok(c.query_row("SELECT COUNT(*) FROM jobs WHERE state = 'running'", [], |r| r.get(0))?)
                    })
                    .unwrap_or(0);
                if running == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Ok(None) => std::thread::sleep(Duration::from_secs(1)),
            Err(e) => {
                eprintln!("medorama: {e}");
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_jobs_row_stops_it() {
        assert!(!beat_stops(&Ok(Some("running".into()))));
        assert!(beat_stops(&Ok(Some("cancelled".into()))) && beat_stops(&Ok(None)));
        assert!(!beat_stops(&Err("database is locked".into())), "a refused heartbeat is tried again");
    }

    #[test]
    fn a_jobs_end_is_written_again_until_taken() {
        let mut tries = 0;
        let r = retried("test", || {
            tries += 1;
            if tries < 3 {
                anyhow::bail!("database is locked")
            }
            Ok(tries)
        });
        assert_eq!(r.unwrap(), 3);
    }

    #[test]
    fn a_job_run_again_has_a_new_folder() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(output_dir(d.path(), 7), d.path().join("j7"));
        std::fs::create_dir_all(d.path().join("j7")).unwrap();
        assert_eq!(output_dir(d.path(), 7), d.path().join("j7-2"));
        std::fs::create_dir_all(d.path().join("j7-2")).unwrap();
        assert_eq!(output_dir(d.path(), 7), d.path().join("j7-3"));
        assert!(!host_name().is_empty() && !host_name().contains(':'));
    }
}
