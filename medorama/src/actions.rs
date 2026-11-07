//! Actions: what can be run on recordings.  `index` is built in; others are plug-ins, each a
//! folder with an `action.toml` in one of the `actions_dirs`:
//!
//! ```toml
//! name = "heart_rate"                 # identifier: a-z, 0-9, _ and -
//! title = "Extract heart rate"
//! version = "1"                       # change it when results would differ
//! description = "..."
//! scope = "recording"                 # "encounter" is reserved (not run yet)
//! kinds = ["wfdb"]                     # recordings it applies to: "wfdb", an adapter's kind (default: all)
//! requires = ["index"]                # actions that must have run on the recording first
//! command = ["python3", "hr.py"]      # run in the job's output folder; paths relative to the
//!                                     # action folder are made absolute
//! timeout_s = 3600
//! default = false                     # ticked in the Actions tab
//! [params]                            # default parameters (any TOML values)
//! window_s = 10
//! ```
//!
//! The command receives the job description (JSON, see [`job_description`]) on its standard
//! input and in `job.json`, writes its results into its working folder (the job's output
//! folder), may print `progress <0..1>` lines, may leave a `summary.json`, and exits with 0
//! on success.  It must not write anywhere else, and in particular never next to the
//! recording.

use crate::config::Config;
use crate::recordings::Recording;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

pub static INDEX_VERSION_STR: LazyLock<String> = LazyLock::new(|| recording_engine::common::INDEX_VERSION.to_string());

/// Largest summary.json kept in the database.
const MAX_SUMMARY: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub name: String,
    pub title: String,
    pub version: String,
    pub description: String,
    pub scope: String,
    pub kinds: Vec<String>,
    pub requires: Vec<String>,
    pub default: bool,
    pub builtin: bool,
    pub timeout_s: u64,
    pub params: serde_json::Value,
    /// Empty when it can run; else why not.
    pub problem: String,
    #[serde(skip)]
    pub command: Vec<String>,
    #[serde(skip)]
    pub dir: PathBuf,
}

impl Action {
    pub fn applies_to(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|k| k == kind)
    }
}

pub fn index_action() -> Action {
    Action {
        name: "index".into(),
        title: "Index".into(),
        version: INDEX_VERSION_STR.clone(),
        description: "Build the index the viewers use to show a recording (envelopes, markers, vital signs).".into(),
        scope: "recording".into(),
        kinds: vec![],
        requires: vec![],
        default: true,
        builtin: true,
        timeout_s: 0,
        params: serde_json::json!({}),
        problem: String::new(),
        command: vec![],
        dir: PathBuf::new(),
    }
}

/// The built-in action that reads the identifiers a recording holds in its own file
/// (identify.rs); queued automatically, like indexing.
pub fn identify_action() -> Action {
    Action {
        name: "identify".into(),
        title: "Read identifiers".into(),
        version: "1".into(),
        description: "Read the patient identifiers stored in the recording's own file, to cross-check them against the linkage table (stored in the database only; shown to users with the PHI permission).".into(),
        builtin: true,
        default: false,
        ..index_action()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub actions: Vec<Action>,
    /// Manifests that could not be read.
    pub problems: Vec<String>,
}

fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(i),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::json!(b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(a) => serde_json::Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => {
            serde_json::Value::Object(t.iter().map(|(k, v)| (k.clone(), toml_to_json(v))).collect())
        }
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn read_manifest(dir: &Path) -> Result<Action, String> {
    let file = dir.join("action.toml");
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let t: toml::Table = text.parse().map_err(|e| format!("{}: {e}", file.display()))?;
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let list = |k: &str| -> Result<Vec<String>, String> {
        match t.get(k) {
            None => Ok(vec![]),
            Some(v) => v
                .as_array()
                .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect::<Option<Vec<_>>>())
                .ok_or_else(|| format!("{}: {k} must be a list of texts", file.display())),
        }
    };
    let name = s("name").ok_or_else(|| format!("{}: name is required", file.display()))?;
    if !valid_name(&name) {
        return Err(format!("{}: name {name:?}: use a-z, 0-9, _ and -", file.display()));
    }
    let mut command = list("command")?;
    if command.is_empty() {
        return Err(format!("{}: command is required", file.display()));
    }
    for part in command.iter_mut() {
        let p = dir.join(&*part);
        if !Path::new(part).is_absolute() && p.is_file() {
            *part = p.to_string_lossy().to_string();
        }
    }
    let scope = s("scope").unwrap_or_else(|| "recording".into());
    let problem = match scope.as_str() {
        "recording" => String::new(),
        "encounter" => "encounter-scope actions are not supported yet".into(),
        other => return Err(format!("{}: scope {other:?}: use \"recording\" or \"encounter\"", file.display())),
    };
    Ok(Action {
        title: s("title").unwrap_or_else(|| name.clone()),
        version: match t.get("version") {
            Some(toml::Value::Integer(i)) => i.to_string(),
            Some(toml::Value::Float(f)) => f.to_string(),
            Some(toml::Value::String(v)) => v.clone(),
            _ => "1".into(),
        },
        description: s("description").unwrap_or_default(),
        scope,
        kinds: list("kinds")?,
        requires: list("requires")?,
        default: t.get("default").and_then(|v| v.as_bool()).unwrap_or(false),
        builtin: false,
        timeout_s: t.get("timeout_s").and_then(|v| v.as_integer()).map_or(DEFAULT_TIMEOUT_S, |x| x.max(0) as u64),
        params: t.get("params").map_or(serde_json::json!({}), toml_to_json),
        problem,
        command,
        dir: dir.to_path_buf(),
        name,
    })
}

impl Registry {
    /// The built-in actions and every plug-in found in the configured folders.
    pub fn load(cfg: &Config) -> Registry {
        let mut r = Registry { actions: vec![index_action(), identify_action()], problems: vec![] };
        for d in &cfg.actions_dirs {
            let Ok(it) = std::fs::read_dir(d) else { continue };
            let mut dirs: Vec<PathBuf> =
                it.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.join("action.toml").is_file()).collect();
            dirs.sort();
            for dir in dirs {
                match read_manifest(&dir) {
                    Ok(a) if r.get(&a.name).is_some() => {
                        r.problems.push(format!("{}: an action named {} exists already", dir.display(), a.name))
                    }
                    Ok(a) => r.actions.push(a),
                    Err(e) => r.problems.push(e),
                }
            }
        }
        let names: Vec<String> = r.actions.iter().map(|a| a.name.clone()).collect();
        for a in r.actions.iter_mut() {
            if let Some(missing) = a.requires.iter().find(|q| !names.contains(q)) {
                a.problem = format!("requires {missing}, which is not installed");
            }
        }
        r
    }

    pub fn get(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }
}

/// Stable hash of parameters (object keys are sorted).
pub fn params_hash(v: &serde_json::Value) -> String {
    signal_core::text::hex(&Sha256::digest(v.to_string().as_bytes()))[..16].to_string()
}

/// Defaults overridden by the given parameters.
pub fn merge_params(defaults: &serde_json::Value, given: Option<&serde_json::Value>) -> serde_json::Value {
    let mut out = defaults.clone();
    if let (Some(o), Some(serde_json::Value::Object(g))) = (out.as_object_mut(), given) {
        for (k, v) in g {
            o.insert(k.clone(), v.clone());
        }
    }
    out
}

/// What a plug-in receives (stdin and job.json).
pub fn job_description(
    job_id: i64,
    a: &Action,
    params: &serde_json::Value,
    rec: &Recording,
    index_dir: Option<&Path>,
    encounters: &[(String, String)],
    output_dir: &Path,
) -> serde_json::Value {
    serde_json::json!({
        "catalog": format!("medorama {}", env!("CARGO_PKG_VERSION")),
        "job_id": job_id,
        "action": a.name,
        "version": a.version,
        "params": params,
        "recording": {
            "id": rec.id, "path": rec.path, "kind": rec.kind, "name": rec.name, "format": rec.format,
            "t_start": rec.t_start, "t_end": rec.t_end,
            "index_dir": index_dir.map(|p| p.to_string_lossy().to_string()),
        },
        // the encounters' and patients' pseudonyms (never their imported IDs)
        "encounters": encounters.iter().map(|(e, p)| serde_json::json!({"encounter_id": e, "patient_id": p})).collect::<Vec<_>>(),
        "output_dir": output_dir.to_string_lossy(),
    })
}

/// Progress shared between a running job and its heartbeat (an f64 in an atomic).
#[derive(Default)]
pub struct Progress(AtomicU64);

impl Progress {
    pub fn set(&self, x: f64) {
        self.0.store(x.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
}

pub struct Outcome {
    pub output_dir: PathBuf,
    pub summary: String,
    /// Series derived from the waveforms (indexing: a Holter's heart rate), for the database.
    pub derived: Vec<crate::derived::Derived>,
}

/// Run the built-in index action.
pub fn run_index(cfg: &Config, rec: &Recording, progress: &Progress) -> Result<Outcome, String> {
    let key = recording_engine::index::build_index(Path::new(&rec.path), &cfg.index_dir, &|x| progress.set(x))
        .map_err(|e| e.to_string())?;
    let dir = cfg.index_dir.join(&key);
    let meta: serde_json::Value = std::fs::read_to_string(dir.join("meta.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let summary = serde_json::json!({
        "key": key,
        "format": meta.get("format"),
        "channels": meta.get("channels").and_then(|c| c.as_array()).map(|c| c.len()),
        "vitals": meta.get("vitals").and_then(|c| c.as_array()).map(|c| c.len()),
        "indexed_s": meta.get("indexed_s"),
    });
    // a recording without a heart rate of its own (a Holter): find it from the ECG
    let derived = crate::derived::derive(&cfg.index_dir, &key, &|_| {}).map_err(|e| e.to_string())?;
    Ok(Outcome { output_dir: dir, summary: summary.to_string(), derived })
}

/// The environment variables a plug-in does not get: Medorama's own (`MEDORAMA_…`: the
/// database's URL and password, the encryption key), and any whose name
/// says it is secret (a password, a secret, a token, a passphrase, a credential, `…_KEY`,
/// PostgreSQL's password settings, `DATABASE_URL`).  The rest stays: a plug-in finds its
/// interpreter and libraries as before (`PATH`, `HOME`, a virtual or conda environment).
pub fn secret_env(names: impl Iterator<Item = std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    names
        .filter(|n| {
            let u = n.to_string_lossy().to_uppercase();
            ["MEDORAMA_", "PGPASS"].iter().any(|p| u.starts_with(p))
                || ["PASSWORD", "SECRET", "TOKEN", "PASSPHRASE", "CREDENTIAL"].iter().any(|w| u.contains(w))
                || u.ends_with("_KEY")
                || u == "DATABASE_URL"
        })
        .collect()
}

/// A plug-in's time limit when its manifest gives none (`timeout_s = 0`: no limit).
pub const DEFAULT_TIMEOUT_S: u64 = 6 * 3600;

/// The process groups of the plug-ins running now (Unix: each its own group, its pid).
static RUNNING: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Stop a plug-in's whole process group: TERM, then KILL what is left after `grace`.
fn stop_group(child: &mut std::process::Child, grace: Duration) {
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let _ = Command::new("kill").args(["-TERM", &group]).status();
        let until = Instant::now() + grace;
        while Instant::now() < until && matches!(child.try_wait(), Ok(None)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        // the group, whose leader may be gone while what it started is not
        let _ = Command::new("kill").args(["-KILL", &group]).status();
    }
    #[cfg(not(unix))]
    let _ = grace;
    let _ = child.kill();
    let _ = child.wait();
}

/// Every plug-in still running stopped (TERM to its process group, KILL after 5 seconds): the
/// server or `medorama work` stopping leaves none behind.  Their workers see them end.
pub fn stop_all() {
    let groups: Vec<u32> = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if groups.is_empty() {
        return;
    }
    #[cfg(unix)]
    {
        for g in &groups {
            let _ = Command::new("kill").args(["-TERM", &format!("-{g}")]).status();
        }
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !RUNNING.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            std::thread::sleep(Duration::from_millis(100));
        }
        for g in groups {
            let _ = Command::new("kill").args(["-KILL", &format!("-{g}")]).status();
        }
    }
}

/// Run a plug-in in `output_dir`; stops it (its process group) when `cancel` is set or the
/// timeout passes.
pub fn run_external(
    a: &Action,
    description: &serde_json::Value,
    output_dir: &Path,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<Outcome, String> {
    std::fs::create_dir_all(output_dir).map_err(|e| format!("{}: {e}", output_dir.display()))?;
    let text = serde_json::to_string_pretty(description).unwrap_or_default();
    let job_file = output_dir.join("job.json");
    std::fs::write(&job_file, &text).map_err(|e| format!("{}: {e}", job_file.display()))?;
    let log_path = output_dir.join("log.txt");
    let log = std::fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
    let mut cmd = Command::new(&a.command[0]);
    // not the server's secrets: the database's password and URL, the encryption key, tokens
    for name in secret_env(std::env::vars_os().map(|(k, _)| k)) {
        cmd.env_remove(name);
    }
    cmd.args(&a.command[1..])
        .current_dir(output_dir)
        .env("MEDORAMA_JOB", &job_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log));
    // its own process group: a cancel or the time limit stops what it started too
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", a.command[0]))?;
    let pid = child.id();
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).push(pid);
    struct Running(u32);
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != self.0);
        }
    }
    let _running = Running(pid);
    // the job's description on its standard input, its standard output read, each in a thread
    // of its own: a plug-in that writes before it reads cannot block the run, and the loop
    // below can always cancel it
    if let Some(mut stdin) = child.stdin.take() {
        let text = text.clone();
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    enum Out {
        Progress(f64),
        Line(String),
    }
    let (tx, rx) = std::sync::mpsc::channel::<Out>();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let msg = match line.trim().strip_prefix("progress ").and_then(|x| x.trim().parse::<f64>().ok()) {
                    Some(x) => Out::Progress(x),
                    None => Out::Line(line),
                };
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
    }
    let started = Instant::now();
    let mut out_lines: Vec<String> = vec![];
    let mut drain = |rx: &std::sync::mpsc::Receiver<Out>| {
        while let Ok(m) = rx.try_recv() {
            match m {
                Out::Progress(x) => progress.set(x),
                Out::Line(l) if out_lines.len() < 200 => out_lines.push(l),
                Out::Line(_) => {}
            }
        }
    };
    // the whole group (its pid is the child's): TERM, a moment to end, then KILL
    let stop = |child: &mut std::process::Child| stop_group(child, Duration::from_secs(5));
    let status = loop {
        drain(&rx);
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s),
            Ok(None) => {}
            Err(e) => break Err(e.to_string()),
        }
        if cancel.load(Ordering::Relaxed) {
            stop(&mut child);
            break Err("cancelled".to_string());
        }
        if a.timeout_s > 0 && started.elapsed() > Duration::from_secs(a.timeout_s) {
            stop(&mut child);
            break Err(format!("stopped after the time limit ({} s)", a.timeout_s));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    // what it printed last (briefly: a process it left behind may keep its output open)
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Out::Progress(x)) => progress.set(x),
            Ok(Out::Line(l)) if out_lines.len() < 200 => out_lines.push(l),
            Ok(Out::Line(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if !out_lines.is_empty() {
        let _ = std::fs::write(output_dir.join("stdout.txt"), out_lines.join("\n"));
    }
    let status = status?;
    if !status.success() {
        let mut tail = String::new();
        if let Ok(mut f) = std::fs::File::open(&log_path) {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            tail = s.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        }
        return Err(format!(
            "{} exited with {status}{}",
            a.command[0],
            if tail.is_empty() { String::new() } else { format!(": {tail}") }
        ));
    }
    let summary_path = output_dir.join("summary.json");
    let summary = match std::fs::metadata(&summary_path) {
        Ok(m) if m.len() <= MAX_SUMMARY => {
            let s = std::fs::read_to_string(&summary_path).unwrap_or_default();
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(v) => v.to_string(),
                Err(e) => return Err(format!("summary.json is not JSON: {e}")),
            }
        }
        Ok(_) => serde_json::json!({"note": "summary.json too large to store"}).to_string(),
        Err(_) => String::new(),
    };
    progress.set(1.0);
    Ok(Outcome { output_dir: output_dir.to_path_buf(), summary, derived: vec![] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plug_ins_do_not_get_the_servers_secrets() {
        let names = [
            "MEDORAMA_DATABASE_PASSWORD",
            "MEDORAMA_KEY",
            "MEDORAMA_DATABASE_URL",
            "MEDORAMA_HOME",
            "PGPASSWORD",
            "PGPASSFILE",
            "DATABASE_URL",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "MY_API_KEY",
            "db_password",
            "PATH",
            "HOME",
            "LANG",
            "TMPDIR",
            "VIRTUAL_ENV",
            "CONDA_PREFIX",
            "PYTHONPATH",
            "KEYCHAIN_PATH",
        ];
        let removed: Vec<String> = secret_env(names.iter().map(|n| std::ffi::OsString::from(*n)))
            .into_iter()
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        assert_eq!(removed, names[..11].to_vec(), "the secrets, and only them");
    }
}
