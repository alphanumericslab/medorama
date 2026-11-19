//! The job queue: a run (actions x a selection of patients or recordings) becomes one job per
//! action and recording of a recording database; workers (in the server, or `medorama work`)
//! take jobs from the queue, highest priority first.  A job's results go into its recording's
//! database folder of the workspace, and go with it.
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

/// What a run applies to: recordings, the recordings of patients or of a cohort's members, or
/// all recordings (of the databases the user may view).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Selection {
    /// The recordings of a cohort's members (one the user may see).
    pub cohort: Option<i64>,
    pub patients: Option<Vec<i64>>,
    pub recordings: Option<Vec<i64>>,
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

fn selected_recordings(c: &Connection, cfg: &Config, user: &User, sel: &Selection) -> anyhow::Result<Vec<i64>> {
    let viewable = crate::databases::granted(c, user, "view")?;
    let mut ids: Vec<i64> = vec![];
    let mut patients: Vec<i64> = sel.patients.clone().unwrap_or_default();
    if let Some(co) = sel.cohort {
        patients.extend(crate::cohorts::member_ids(c, cfg, user, co)?);
    }
    if sel.all {
        ids.extend(recordings::all(cfg, c)?.into_iter().filter(|r| viewable.contains(&r.database)).map(|r| r.id));
    }
    if let Some(r) = &sel.recordings {
        ids.extend(r.iter().filter(|id| viewable.contains(&recordings::split(**id).0)));
    }
    for p in patients {
        ids.extend(recordings::of_patient(cfg, c, user, p)?.into_iter().map(|r| r.id));
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
    cfg: &Config,
    reg: &Registry,
    user: &User,
    actions: &[String],
    params_by_action: &HashMap<String, serde_json::Value>,
    sel: &Selection,
    force: bool,
    priority: i64,
) -> anyhow::Result<RunReport> {
    if !user.can_run() {
        anyhow::bail!("running actions needs the analyst or admin role");
    }
    if actions.is_empty() {
        anyhow::bail!("no action chosen");
    }
    let plan = with_requirements(reg, actions)?;
    // each database's file opened once, before the transaction holds the write lock
    let ids = selected_recordings(c, cfg, user, sel)?;
    let mut by_id = recordings::get_many(cfg, c, &ids)?;
    let recs: Vec<Recording> = ids.iter().filter_map(|id| by_id.remove(id)).collect();
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
    for rec in recs {
        let rid = rec.id;
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
                "INSERT INTO jobs (run, action, action_version, params, params_hash, database_id, recording, fingerprint,
                                   state, priority, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', ?9, ?10)",
                params![run, name, a.version, p.to_string(), ph, rec.database, rid, rec.fingerprint, priority, t],
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
pub fn queue_auto_index(c: &Connection, cfg: &Config) -> anyhow::Result<usize> {
    c.exclusive(crate::sql::Lock::Schedule, || queue_auto_index_now(c, cfg))?
}

fn queue_auto_index_now(c: &Connection, cfg: &Config) -> anyhow::Result<usize> {
    let todo: Vec<Recording> =
        recordings::all(cfg, c)?.into_iter().filter(|r| matches!(r.status, Status::New | Status::Stale)).collect();
    let ph = params_hash(&serde_json::json!({}));
    let t = now();
    for r in &todo {
        c.execute(
            "INSERT INTO jobs (action, action_version, params, params_hash, database_id, recording, fingerprint, state,
                               priority, created_at)
             VALUES ('index', ?1, '{}', ?2, ?3, ?4, ?5, 'queued', 0, ?6)",
            params![crate::actions::INDEX_VERSION_STR.as_str(), ph, r.database, r.id, r.fingerprint, t],
        )?;
    }
    Ok(todo.len())
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
pub fn claim(c: &mut Connection, cfg: &Config, reg: &Registry, worker: &str) -> anyhow::Result<Option<Claimed>> {
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
    // the candidates' recordings, each database's file opened once
    let rids: Vec<i64> = cands.iter().map(|c| c.3).collect();
    let recs = recordings::get_many(cfg, &tx, &rids)?;
    for (id, action, params, rid) in cands {
        let Some(rec) = recs.get(&rid) else {
            // its database removed, or being removed
            tx.execute("DELETE FROM jobs WHERE id = ?1", [id])?;
            continue;
        };
        if !rec.present {
            tx.execute(
                "UPDATE jobs SET state = 'failed', error = 'the recording is missing', finished_at = ?1 WHERE id = ?2",
                params![t, id],
            )?;
            continue;
        }
        match requirements(&tx, reg, &action, rec)? {
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
    let prepared = (|| -> anyhow::Result<_> {
        let c = db.get()?;
        let rec = recordings::get(cfg, &c, job.recording)?
            .ok_or_else(|| anyhow::anyhow!("recording {} removed", job.recording))?;
        // indexing: into its database's index store (opened here, with the database's key)
        let store =
            if job.action == "index" { Some(crate::workspace::index_store(cfg, &c, rec.database)?) } else { None };
        // indexing: the ready-made index its database may hold, taken instead of building one
        let held = if job.action == "index" { recordings::held_index(&c, &rec) } else { None };
        // its patient's pseudonym only: a database's identifiers may be MRNs (docs/security.md)
        let patient = match rec.patient {
            Some(p) => crate::patient::pseudonym_of(&c, p)?,
            None => String::new(),
        };
        Ok((rec, patient, store, held))
    })();
    let (rec, patient, store, held) = match prepared {
        Ok(x) => x,
        Err(e) => {
            // failed before it ran (its recording gone, its key unreadable): written as failed,
            // else it would be taken for a stale job and run again every STALE_S
            let why = e.to_string();
            retried(&format!("job {}: recording its failure", job.id), || {
                Ok(db.get()?.execute(
                    "UPDATE jobs SET state = 'failed', error = ?1, finished_at = ?2 WHERE id = ?3 AND worker = ?4 AND state = 'running'",
                    params![why, now(), job.id, worker],
                )?)
            })?;
            return Err(e);
        }
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
            Some(a) if a.builtin && a.name == "index" => match &store {
                Some(st) => run_index(st, &rec, held.as_ref(), &progress, &cancel),
                None => Err("no index store".into()),
            },
            Some(a) => {
                let out = output_dir(&recordings::results_dir(cfg, rec.id, &a.name), job.id);
                let desc = job_description(job.id, a, &job.params, &rec, &patient, &out);
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
                let c = db.get()?;
                recordings::apply_index_meta(cfg, &c, rec.id, &o.meta)?;
                recordings::store_series(cfg, &c, rec.id, &o.derived)?;
            } else if n == 1 {
                // the action's numbers: a derived database of its recording's database
                crate::derived::store_result(cfg, &*db.get()?, &rec, &job.action, &o.summary)?;
            }
        }
        Err(e) => {
            retried(&format!("job {}: recording its end", job.id), || {
                // a job cancelled meanwhile keeps its 'cancelled' error (status_of reads it)
                Ok(db.get()?.execute(
                    "UPDATE jobs SET state = 'failed', error = ?1, finished_at = ?2
                     WHERE id = ?3 AND worker = ?4 AND state = 'running'",
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
    // ?3 is used either way: PostgreSQL cannot give a type to a parameter the query leaves out
    let own = if user.is_admin() {
        "CAST(?3 AS INTEGER) IS NOT NULL"
    } else {
        "run IN (SELECT id FROM runs WHERE user_id = ?3)"
    };
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
    /// The database of its recording.
    #[serde(skip)]
    pub database: i64,
    /// Who asked for its run.
    #[serde(skip)]
    pub by: Option<i64>,
}

/// Jobs, newest first: unfinished ones, or (with `state`) those in one state.
pub fn list(
    c: &Connection,
    cfg: &Config,
    state: Option<&str>,
    run: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<JobView>> {
    list_in(c, cfg, state, run, limit, None)
}

/// The jobs a user may see: an admin every one; others those of the databases granted to them
/// for viewing, and the errors of their own runs only (an error may name another user's files):
/// another's error is `failed`.
pub fn list_for(
    c: &Connection,
    cfg: &Config,
    user: &User,
    state: Option<&str>,
    run: Option<i64>,
    limit: i64,
) -> anyhow::Result<Vec<JobView>> {
    if user.is_admin() {
        return list(c, cfg, state, run, limit);
    }
    let dbs = crate::databases::granted(c, user, "view")?;
    let mut v = list_in(c, cfg, state, run, limit, Some(&dbs))?;
    for j in v.iter_mut().filter(|j| j.by != Some(user.id) && !j.error.is_empty()) {
        j.error = "failed".into();
    }
    Ok(v)
}

/// The number of jobs per state that [`list_for`] shows a user.
pub fn counts_for(c: &Connection, user: &User) -> anyhow::Result<HashMap<String, i64>> {
    if user.is_admin() {
        return counts(c);
    }
    let dbs = crate::databases::granted(c, user, "view")?;
    if dbs.is_empty() {
        return Ok(HashMap::new());
    }
    // (numbers only: never typed text)
    let ids = dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ");
    let mut st = c.prepare(&format!("SELECT state, COUNT(*) FROM jobs WHERE database_id IN ({ids}) GROUP BY state"))?;
    let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// [`list`], of these databases' recordings only.
fn list_in(
    c: &Connection,
    cfg: &Config,
    state: Option<&str>,
    run: Option<i64>,
    limit: i64,
    databases: Option<&[i64]>,
) -> anyhow::Result<Vec<JobView>> {
    // (numbers only: never typed text)
    let only = match databases {
        None => String::new(),
        Some([]) => return Ok(vec![]),
        Some(d) => {
            format!(" AND j.database_id IN ({})", d.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", "))
        }
    };
    let mut st = c.prepare(&format!(
        "SELECT j.id, j.run, j.action, j.action_version, j.recording, '', j.state, j.priority, j.progress, j.created_at,
                j.started_at, j.finished_at, j.error, j.output_dir, j.database_id, r.user_id
         FROM jobs j LEFT JOIN runs r ON r.id = j.run
         WHERE (CAST(?1 AS TEXT) IS NULL OR j.state = ?1) AND (CAST(?2 AS INTEGER) IS NULL OR j.run = ?2){only}
         ORDER BY CASE j.state WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END, j.priority DESC, j.id DESC LIMIT ?3"
    ))?;
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
                database: r.get(14)?,
                by: r.get(15)?,
            })
        })?
        .collect::<Result<Vec<JobView>, _>>()?;
    let ids: Vec<i64> = v.iter().map(|j| j.recording).collect();
    let recs = recordings::get_many(cfg, c, &ids)?;
    Ok(v.into_iter()
        .map(|mut j| {
            j.recording_name = recs.get(&j.recording).map(|r| r.name.clone()).unwrap_or_default();
            j
        })
        .collect())
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
        let job = db.get().and_then(|mut c| claim(&mut c, &cfg, &reg_now, &name));
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
