//! An import's progress as it runs, and the imports the server runs in the background.
//!
//! An import reports its phase (reading the rows, building the search index, matching), the
//! rows read and how far through its files it is; it stops at the next chunk of rows when
//! cancelled (what it stored removed: nothing of it is kept).  An import waits while another
//! runs (its phase `waiting`): they run one at a time.  The page started an import
//! as a task ([`Tasks`]) and asks for its progress, from any tab, until it is done; a task's
//! end (its report, or why it failed) is kept a while for whoever started it.

use crate::db::now;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What an import is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Waiting for another import to end (imports run one at a time).
    Waiting,
    /// Reading the rows and storing them (most of the time).
    Rows,
    /// Building the search index of the items added.
    Index,
    /// Matching recordings and items that waited for their patient or encounter.
    Match,
    Done,
}

impl Phase {
    fn key(self) -> &'static str {
        match self {
            Phase::Waiting => "waiting",
            Phase::Rows => "rows",
            Phase::Index => "index",
            Phase::Match => "match",
            Phase::Done => "done",
        }
    }
}

/// An import's progress (shared with whoever shows it).
#[derive(Debug)]
pub struct Progress {
    phase: Mutex<Phase>,
    rows: AtomicU64,
    /// How far through the files, in millionths.
    read: AtomicU64,
    /// The part of the whole this import is (start, width), and the rows read before it: a
    /// folder of tables imported one after another.
    span: Mutex<(f64, f64, u64)>,
    started: f64,
    cancel: AtomicBool,
}

impl Default for Progress {
    fn default() -> Self {
        Progress {
            phase: Mutex::new(Phase::Rows),
            rows: AtomicU64::new(0),
            read: AtomicU64::new(0),
            span: Mutex::new((0.0, 1.0, 0)),
            started: now(),
            cancel: AtomicBool::new(false),
        }
    }
}

/// The share of the time reading takes (the index and matching the rest, roughly).
const READING: f64 = 0.92;

impl Progress {
    pub fn new() -> Arc<Progress> {
        Arc::new(Progress::default())
    }

    pub fn phase(&self, p: Phase) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = p;
    }

    /// Rows read so far, and how far through the files (0 to 1).
    pub fn rows(&self, rows: u64, read: f64) {
        let base = self.span.lock().unwrap_or_else(|e| e.into_inner()).2;
        self.rows.store(base + rows, Ordering::Relaxed);
        self.read.store((read.clamp(0.0, 1.0) * 1e6) as u64, Ordering::Relaxed);
    }

    /// The next import is the part of the whole from `start` to `start + width` (0 to 1).
    pub fn span(&self, start: f64, width: f64) {
        *self.span.lock().unwrap_or_else(|e| e.into_inner()) = (start, width, self.rows.load(Ordering::Relaxed));
        self.read.store(0, Ordering::Relaxed);
        self.phase(Phase::Rows);
    }

    /// Asks the import to stop (at its next chunk of rows).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// An error if the import was asked to stop: it ends, rolled back.
    pub fn check(&self) -> anyhow::Result<()> {
        if self.cancelled() {
            anyhow::bail!("the import was cancelled: nothing of it was kept");
        }
        Ok(())
    }

    /// The progress as the page shows it: phase, rows, the share done (0 to 1), rows a
    /// second, and seconds left (once there is enough to tell).
    pub fn view(&self) -> Value {
        let phase = *self.phase.lock().unwrap_or_else(|e| e.into_inner());
        let rows = self.rows.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Relaxed) as f64 / 1e6;
        let elapsed = (now() - self.started).max(0.001);
        let (start, width, _) = *self.span.lock().unwrap_or_else(|e| e.into_inner());
        let part = match phase {
            Phase::Waiting => 0.0,
            Phase::Rows => READING * read,
            Phase::Index => READING,
            Phase::Match => (READING + 1.0) / 2.0,
            Phase::Done => 1.0,
        };
        let done = if phase == Phase::Done { 1.0 } else { start + width * part };
        let left = (phase == Phase::Rows && done > 0.02 && elapsed > 3.0).then(|| elapsed * (1.0 - done) / done);
        json!({
            "phase": phase.key(),
            "rows": rows,
            "done": (done * 1000.0).round() / 1000.0,
            "elapsed_s": elapsed.round(),
            "rate": (rows as f64 / elapsed).round(),
            "left_s": left.map(f64::round),
            "cancelling": self.cancelled(),
        })
    }
}

/// An import the server runs in the background.
pub struct Task {
    pub id: String,
    pub user: i64,
    /// What is imported (the database's name).
    pub label: String,
    pub progress: Arc<Progress>,
    pub started: f64,
    /// Its report, or why it failed; and when it ended.
    end: Mutex<Option<(Result<Value, String>, f64)>>,
}

impl Task {
    pub fn finish(&self, result: Result<Value, String>) {
        self.progress.phase(Phase::Done);
        *self.end.lock().unwrap_or_else(|e| e.into_inner()) = Some((result, now()));
    }

    pub fn view(&self) -> Value {
        let end = self.end.lock().unwrap_or_else(|e| e.into_inner());
        let (state, report, error) = match &*end {
            None => ("running", Value::Null, Value::Null),
            Some((Ok(r), _)) => ("done", r.clone(), Value::Null),
            Some((Err(e), _)) if self.progress.cancelled() => ("cancelled", Value::Null, Value::String(e.clone())),
            Some((Err(e), _)) => ("failed", Value::Null, Value::String(e.clone())),
        };
        json!({
            "id": self.id, "label": self.label, "state": state, "started": self.started,
            "progress": self.progress.view(), "report": report, "error": error,
        })
    }

    fn ended(&self) -> Option<f64> {
        self.end.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, t)| *t)
    }
}

/// How long a task's end is kept for whoever started it.
const KEEP_S: f64 = 3600.0;

/// The server's background imports.
#[derive(Default)]
pub struct Tasks {
    all: Mutex<Vec<Arc<Task>>>,
}

impl Tasks {
    /// A new task for `user`, importing `label`.
    pub fn start(&self, user: i64, label: &str) -> anyhow::Result<Arc<Task>> {
        let mut b = [0u8; 12];
        getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
        let task = Arc::new(Task {
            id: b.iter().map(|x| format!("{x:02x}")).collect(),
            user,
            label: label.to_string(),
            progress: Progress::new(),
            started: now(),
            end: Mutex::new(None),
        });
        let mut all = self.all.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        all.retain(|x| x.ended().is_none_or(|e| t - e < KEEP_S));
        all.push(task.clone());
        Ok(task)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Task>> {
        self.all.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|t| t.id == id).cloned()
    }

    /// The tasks running, and those ended within the hour, newest first.
    pub fn list(&self) -> Vec<Arc<Task>> {
        let mut v = self.all.lock().unwrap_or_else(|e| e.into_inner()).clone();
        v.reverse();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_tasks() {
        let p = Progress::new();
        p.rows(500, 0.5);
        let v = p.view();
        assert_eq!((v["phase"].as_str(), v["rows"].as_u64()), (Some("rows"), Some(500)));
        assert!((v["done"].as_f64().unwrap() - 0.46).abs() < 1e-9);
        assert!(p.check().is_ok());
        p.cancel();
        assert!(p.check().is_err() && p.view()["cancelling"] == true);
        let tasks = Tasks::default();
        let t = tasks.start(1, "ecg").unwrap();
        assert_eq!(tasks.get(&t.id).unwrap().view()["state"], "running");
        t.finish(Ok(json!({"rows": 3})));
        let v = tasks.get(&t.id).unwrap().view();
        assert_eq!((v["state"].as_str(), v["report"]["rows"].as_u64()), (Some("done"), Some(3)));
        assert_eq!(v["progress"]["done"], 1.0);
        assert_eq!(tasks.list().len(), 1);
        // a folder's second table of two
        let p = Progress::new();
        p.rows(10, 1.0);
        p.span(0.5, 0.5);
        p.rows(4, 0.5);
        let v = p.view();
        assert_eq!(v["rows"], 14);
        assert!((v["done"].as_f64().unwrap() - (0.5 + 0.5 * 0.46)).abs() < 1e-9);
    }
}
