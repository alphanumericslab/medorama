//! Adapters: separate programs that read recording formats the engine does not read itself
//! (licensed or vendor formats). The protocol is specified in `docs/adapters.md`; in short,
//! an adapter is an executable named `<name>-adapter` in one of the adapter folders, with
//! these commands:
//!
//! | Command | Answer (standard output) |
//! |---|---|
//! | `describe` | one JSON object: `{name, version, protocol, formats: [{kind, label, extensions}]}` |
//! | `detect PATH…` | one JSON line per path: `{path, kind}` (`kind` null when not its format) |
//! | `index PATH DIR` | writes the recording's index files into `DIR`; progress lines `{"progress": f}`, then `{"done": true}` |
//! | `identify PATH…` | one JSON line per path: `{path, identifiers: {…}, others: [{…}]}` (identifiers the file states) |
//! | `serve` | a session on standard input and output: a JSON request per line, each answered by a JSON line and, for samples, their bytes |
//!
//! The engine runs one `serve` session per adapter and keeps it open: samples are read from
//! the original recording on demand (zoomed-in views), and the session keeps the recordings
//! it was asked about open between requests.  Sessions of different adapters run side by
//! side; a session that answers out of step is stopped.
//!
//! Every child runs with a cleared environment (an allow-list: [`ENV_ALLOWED`]), its standard
//! error kept (the last [`STDERR_KEPT`] bytes) but never printed, and within the time limits of
//! [`Timeouts`] (docs/adapters.md, "Limits and environment").

use signal_core::value::{Dict, Value};
use signal_core::{Error, Result};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// The protocol version this engine speaks.
pub const PROTOCOL: i128 = 1;

/// The environment variables a child process gets (all others are removed: the programs may
/// hold secrets in theirs); `LC_*` too.  On Windows, names are compared ignoring case.
pub const ENV_ALLOWED: [&str; 14] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "TMP",
    "TEMP",
    "TMPDIR",
    "LANG",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "LOCALAPPDATA",
    "APPDATA",
];
/// The bytes of an adapter's standard error kept (the last ones); never printed.
pub const STDERR_KEPT: usize = 64 * 1024;
/// The longest line of an adapter's answer read (longer lines are a protocol error).
const MAX_LINE: usize = 1 << 20;
/// The most an answer of `describe` or `identify` may hold.
const MAX_OUTPUT: usize = 16 << 20;
/// The arguments of one `identify` command at most (bytes, the paths and a space each): far
/// below Windows' 32,767 characters of a command line.
const IDENTIFY_ARG_BYTES: usize = 24_000;
/// The samples asked of a session in one request; longer reads are split.
pub const READ_CHUNK: i64 = 1 << 20;

/// Time limits on an adapter's commands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeouts {
    /// `describe`: the whole command
    pub describe: Duration,
    /// `identify`: per path of the command
    pub identify_per_path: Duration,
    /// `index`: the longest time without a line of output (or, after its output ends, until
    /// it exits)
    pub index_stall: Duration,
    /// `serve`: the longest wait for the next bytes of an answer
    pub serve_stall: Duration,
}

impl Default for Timeouts {
    fn default() -> Timeouts {
        Timeouts {
            describe: Duration::from_secs(30),
            identify_per_path: Duration::from_secs(60),
            index_stall: Duration::from_secs(300),
            serve_stall: Duration::from_secs(60),
        }
    }
}

static TIMEOUTS: RwLock<Option<Timeouts>> = RwLock::new(None);

/// Use these time limits from now on (tests; the defaults otherwise).
pub fn set_timeouts(t: Timeouts) {
    *TIMEOUTS.write().unwrap_or_else(|e| e.into_inner()) = Some(t);
}

/// The time limits in force.
pub fn timeouts() -> Timeouts {
    TIMEOUTS.read().unwrap_or_else(|e| e.into_inner()).unwrap_or_default()
}

/// A recording format an adapter reads.
#[derive(Debug, Clone)]
pub struct Format {
    /// The recording kind written in the index (`meta.kind`), as `describe` names it.
    pub kind: String,
    pub label: String,
    /// File extensions, lower case, without the dot.
    pub extensions: Vec<String>,
}

/// An adapter found in an adapter folder.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: String,
    pub version: String,
    /// The implementation, as the adapter states it (informative: `rs`, `cpp`, `py` ...).
    pub implementation: String,
    pub program: PathBuf,
    pub formats: Vec<Format>,
}

fn folders() -> &'static RwLock<Vec<PathBuf>> {
    static F: OnceLock<RwLock<Vec<PathBuf>>> = OnceLock::new();
    F.get_or_init(|| RwLock::new(vec![default_folder()]))
}

fn found() -> &'static RwLock<Option<Vec<Adapter>>> {
    static A: OnceLock<RwLock<Option<Vec<Adapter>>>> = OnceLock::new();
    A.get_or_init(|| RwLock::new(None))
}

/// The adapter folder used when a program's settings name none: `~/.local/share/medorama/adapters`
/// (on Windows, `%LOCALAPPDATA%\medorama\adapters`).
pub fn default_folder() -> PathBuf {
    if cfg!(windows) {
        if let Some(d) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(d).join("medorama").join("adapters");
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".local").join("share").join("medorama").join("adapters")
}

/// The folders searched for adapters, in order (a program's settings: `adapters_dir`).
pub fn set_folders(dirs: Vec<PathBuf>) {
    *folders().write().unwrap_or_else(|e| e.into_inner()) = dirs;
    *found().write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The adapters installed, each asked for its description once (`describe`).
pub fn installed() -> Vec<Adapter> {
    if let Some(a) = found().read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return a.clone();
    }
    let out = in_folders(&folders().read().unwrap_or_else(|e| e.into_inner()));
    *found().write().unwrap_or_else(|e| e.into_inner()) = Some(out.clone());
    out
}

/// The adapters in these folders, asked for their descriptions now (the first of each name, in
/// folder and name order). Changes nothing: [`installed`] keeps its own list until [`rescan`].
pub fn in_folders(dirs: &[PathBuf]) -> Vec<Adapter> {
    let mut out: Vec<Adapter> = vec![];
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut programs: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                p.is_file() && (n.ends_with("-adapter") || n.ends_with("-adapter.exe"))
            })
            .collect();
        programs.sort();
        for p in programs {
            match describe(&p) {
                Ok(a) if !out.iter().any(|x| x.name == a.name) => out.push(a),
                Ok(_) => {}
                Err(e) => eprintln!("adapter {}: {e}", p.display()),
            }
        }
    }
    out
}

/// Forget the adapters found, so that the next use looks again (after one was installed or
/// removed).
pub fn rescan() {
    *found().write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether a child process gets the environment variable `name`.
fn env_allowed(name: &OsStr) -> bool {
    let n = name.to_string_lossy();
    let n = if cfg!(windows) { n.to_ascii_uppercase() } else { n.into_owned() };
    ENV_ALLOWED.contains(&n.as_str()) || n.starts_with("LC_")
}

/// A command for the adapter `program`, with the allowed environment only.
fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.env_clear();
    for (k, v) in std::env::vars_os().filter(|(k, _)| env_allowed(k)) {
        c.env(k, v);
    }
    c
}

/// The last bytes of a child's standard error, read by a thread of their own (so that a child
/// that writes much there never blocks).
#[derive(Clone, Default)]
struct Stderr(Arc<Mutex<VecDeque<u8>>>);

impl Stderr {
    fn drain(child: &mut Child) -> Stderr {
        let kept = Stderr::default();
        if let Some(mut err) = child.stderr.take() {
            let k = kept.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = err.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut q = k.0.lock().unwrap_or_else(|e| e.into_inner());
                    q.extend(&buf[..n]);
                    let over = q.len().saturating_sub(STDERR_KEPT);
                    q.drain(..over);
                }
            });
        }
        kept
    }

    /// Its last line that is not blank, at most 200 characters, control characters removed:
    /// for an error message (never printed by the engine).
    fn last_line(&self) -> String {
        let q = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let bytes: Vec<u8> = q.iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let line = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
        line.chars().filter(|c| !c.is_control()).take(200).collect()
    }
}

/// Lines of `out` (each at most [`MAX_LINE`] bytes; a longer one ends the stream with an
/// error), read by a thread of their own.
fn lines<R: Read + Send + 'static>(out: R) -> Receiver<std::result::Result<Vec<u8>, String>> {
    let (tx, rx) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        let mut r = BufReader::new(out);
        loop {
            let mut line = vec![];
            match (&mut r).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if line.len() > MAX_LINE => {
                    let _ = tx.send(Err("an adapter wrote a line longer than 1 MiB".into()));
                    break;
                }
                Ok(_) => {
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Stop a child (and collect it).
fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Wait for a child to exit until `deadline`; killed after it.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                stop(child);
                return None;
            }
        }
    }
}

/// Run `program` with `args` within `limit`; its standard output.  The error says why, with
/// the last line of its standard error when `tell` (never for `identify`, whose errors could
/// carry identifiers).
fn run(program: &Path, args: &[&OsStr], limit: Duration, tell: bool) -> Result<String> {
    let deadline = Instant::now() + limit;
    let mut child = command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Value(format!("{}: {e}", program.display())))?;
    let err = Stderr::drain(&mut child);
    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut text = vec![];
            let r = out.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut text);
            let _ = tx.send(r.map(|_| text));
        });
    }
    let name = program.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let text = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(t)) if t.len() <= MAX_OUTPUT => t,
        Ok(Ok(_)) => {
            stop(&mut child);
            return Err(Error::Value(format!("{name} answered with more than 16 MiB")));
        }
        Ok(Err(e)) => {
            stop(&mut child);
            return Err(Error::Value(format!("{name}: {e}")));
        }
        Err(_) => {
            stop(&mut child);
            return Err(Error::Value(format!("{name} did not answer within {} s: stopped", limit.as_secs_f64())));
        }
    };
    let status = wait_until(&mut child, deadline)
        .ok_or_else(|| Error::Value(format!("{name} did not exit within {} s: stopped", limit.as_secs_f64())))?;
    if !status.success() {
        let why = if tell { err.last_line() } else { String::new() };
        return Err(Error::Value(format!(
            "{name} failed ({status}){}",
            if why.is_empty() { why } else { format!(": {why}") }
        )));
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn json(line: &str) -> Result<Dict> {
    match Value::from_json(line.trim()) {
        Ok(Value::Dict(d)) => Ok(d),
        _ => Err(Error::Format(format!("an adapter answered with something other than a JSON object: {line:.200}"))),
    }
}

fn text(d: &Dict, k: &str) -> String {
    d.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// An adapter's description (`describe`), checked against the protocol.
pub fn describe(program: &Path) -> Result<Adapter> {
    let d = json(&run(program, &["describe".as_ref()], timeouts().describe, true)?)?;
    let protocol = d.get("protocol").and_then(Value::as_int).unwrap_or(0);
    if protocol != PROTOCOL {
        return Err(Error::Value(format!("speaks protocol {protocol}; this program speaks {PROTOCOL}")));
    }
    let formats = match d.get("formats") {
        Some(Value::List(l)) => l
            .iter()
            .filter_map(|f| match f {
                Value::Dict(f) => Some(Format {
                    kind: text(f, "kind"),
                    label: text(f, "label"),
                    extensions: match f.get("extensions") {
                        Some(Value::List(e)) => e
                            .iter()
                            .filter_map(Value::as_str)
                            .map(|x| x.trim_start_matches('.').to_lowercase())
                            .collect(),
                        _ => vec![],
                    },
                }),
                _ => None,
            })
            .filter(|f| !f.kind.is_empty())
            .collect(),
        _ => vec![],
    };
    Ok(Adapter {
        name: text(&d, "name"),
        version: text(&d, "version"),
        implementation: text(&d, "implementation"),
        program: program.to_path_buf(),
        formats,
    })
}

/// The adapter and format for a file, by its extension.
pub fn for_file(path: &Path) -> Option<(Adapter, Format)> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    installed().into_iter().find_map(|a| {
        let f = a.formats.iter().find(|f| f.extensions.contains(&ext)).cloned()?;
        Some((a, f))
    })
}

/// The adapter named `name`.
pub fn named(name: &str) -> Option<Adapter> {
    installed().into_iter().find(|a| a.name == name)
}

impl Adapter {
    /// Write the index files of `path` into `dir` (`index`), reporting progress.  The adapter
    /// is stopped once `cancelled` returns true (checked ten times a second), and when it writes
    /// no line for [`Timeouts::index_stall`].
    pub fn index(&self, path: &Path, dir: &Path, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<()> {
        let stall = timeouts().index_stall;
        let mut child = command(&self.program)
            .arg("index")
            .arg(path)
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Value(format!("{}: {e}", self.program.display())))?;
        let err = Stderr::drain(&mut child);
        let rx = lines(child.stdout.take().expect("piped"));
        let fail = |why: String| Error::Value(format!("{} could not index {}: {why}", self.name, path.display()));
        let (mut done, mut deadline) = (false, Instant::now() + stall);
        loop {
            if cancelled() {
                stop(&mut child);
                return Err(Error::Value("indexing cancelled".into()));
            }
            let wait = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
            match rx.recv_timeout(wait) {
                Ok(Ok(line)) => {
                    deadline = Instant::now() + stall;
                    let Ok(d) = json(&String::from_utf8_lossy(&line)) else { continue };
                    if let Some(f) = d.get("progress").and_then(Value::as_f64) {
                        progress(if f.is_nan() { 0.0 } else { f.clamp(0.0, 1.0) });
                    }
                    if matches!(d.get("done"), Some(Value::Bool(true))) {
                        done = true;
                    }
                }
                Ok(Err(e)) => {
                    stop(&mut child);
                    return Err(fail(e));
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(RecvTimeoutError::Timeout) => {
                    stop(&mut child);
                    return Err(fail(format!("no answer for {} s: stopped", stall.as_secs_f64())));
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let status = wait_until(&mut child, Instant::now() + stall)
            .ok_or_else(|| fail(format!("it did not exit within {} s: stopped", stall.as_secs_f64())))?;
        if !status.success() || !done {
            let why = err.last_line();
            return Err(fail(if why.is_empty() { format!("it failed ({status})") } else { why }));
        }
        Ok(())
    }

    /// The identifiers in each file (`identify`), by path: each answer's `identifiers` (the
    /// first set the file states) and `others` (different sets found after it).  The paths are
    /// passed in commands of at most `IDENTIFY_ARG_BYTES` of arguments each.
    pub fn identify(&self, paths: &[PathBuf]) -> Result<HashMap<PathBuf, Dict>> {
        let mut out = HashMap::new();
        let mut rest = paths;
        while !rest.is_empty() {
            let (mut k, mut bytes) = (0, 0);
            while k < rest.len() && (k == 0 || bytes + rest[k].as_os_str().len() < IDENTIFY_ARG_BYTES) {
                bytes += rest[k].as_os_str().len() + 1;
                k += 1;
            }
            let (chunk, next) = rest.split_at(k);
            rest = next;
            let mut args: Vec<&OsStr> = vec!["identify".as_ref()];
            args.extend(chunk.iter().map(|p| p.as_os_str()));
            let limit = timeouts().identify_per_path * chunk.len() as u32;
            for line in run(&self.program, &args, limit, false)?.lines().filter(|l| !l.trim().is_empty()) {
                let d = json(line)?;
                out.insert(PathBuf::from(text(&d, "path")), d);
            }
        }
        Ok(out)
    }

    /// Physical samples `[a, b)` of a piece of an indexed recording, from its `serve` session
    /// (in requests of at most [`READ_CHUNK`] samples).
    pub fn read(&self, index: &Path, piece: usize, a: i64, b: i64) -> Result<Vec<f64>> {
        let mut out = vec![];
        let mut at = a.max(0);
        while at < b {
            let to = b.min(at.saturating_add(READ_CHUNK));
            let x = self.read_chunk(index, piece, at, to)?;
            let short = (x.len() as i64) < to - at;
            out.extend(x);
            if short {
                break; // the piece ends here
            }
            at = to;
        }
        Ok(out)
    }

    fn read_chunk(&self, index: &Path, piece: usize, a: i64, b: i64) -> Result<Vec<f64>> {
        let request = Value::Dict(signal_core::dict! {
            "op" => "read", "index" => index.to_string_lossy().to_string(), "piece" => piece as i64, "a" => a, "b" => b,
        })
        .to_json();
        // this adapter's session only: the others are not held up while it answers
        let slot = sessions().lock().unwrap_or_else(|e| e.into_inner()).entry(self.name.clone()).or_default().clone();
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        // a session that ended (the adapter was updated, or failed) is started again, once
        for attempt in 0..2 {
            if slot.as_mut().is_none_or(|s| s.ended()) {
                *slot = Some(Session::start(&self.program)?);
            }
            let s = slot.as_mut().expect("just started");
            match s.read(&request, (b - a) as usize) {
                Ok(v) => return Ok(v),
                Err(Failure::Answer(e)) => return Err(Error::Value(e)),
                // out of step, or gone: never used again
                Err(Failure::Ended) if attempt == 0 => *slot = None,
                Err(Failure::Ended) => {
                    *slot = None;
                    return Err(Error::Value(format!("{}: its session ended", self.name)));
                }
                Err(Failure::Protocol(e)) => {
                    *slot = None;
                    return Err(Error::Value(format!("{}: {e}; its session was stopped", self.name)));
                }
            }
        }
        Err(Error::Value(format!("{}: its session could not be started", self.name)))
    }
}

/// Why a session's answer was not samples.
enum Failure {
    /// The adapter answered `{"error": …}`: the session stays in step.
    Answer(String),
    /// The session ended before answering.
    Ended,
    /// A malformed answer, or none in time: the session is out of step.
    Protocol(String),
}

/// A running `serve` session: its standard output read by a thread of its own, in chunks.
struct Session {
    child: Child,
    input: ChildStdin,
    output: Receiver<Vec<u8>>,
    buf: Vec<u8>,
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<Mutex<Option<Session>>>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<Mutex<Option<Session>>>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Session {
    fn start(program: &Path) -> Result<Session> {
        let mut child = command(program)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Value(format!("{}: {e}", program.display())))?;
        Stderr::drain(&mut child);
        let input = child.stdin.take().expect("piped");
        let mut out = child.stdout.take().expect("piped");
        // a few chunks ahead at most: an adapter that writes more than it was asked waits
        let (tx, rx) = mpsc::sync_channel(16);
        std::thread::spawn(move || {
            let mut b = vec![0u8; 1 << 16];
            while let Ok(n) = out.read(&mut b) {
                if n == 0 || tx.send(b[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Ok(Session { child, input, output: rx, buf: vec![] })
    }

    fn ended(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// More bytes of the answer, waiting at most [`Timeouts::serve_stall`]; false at its end.
    fn more(&mut self) -> std::result::Result<bool, Failure> {
        let stall = timeouts().serve_stall;
        match self.output.recv_timeout(stall) {
            Ok(b) => {
                self.buf.extend(b);
                Ok(true)
            }
            Err(RecvTimeoutError::Disconnected) => Ok(false),
            Err(RecvTimeoutError::Timeout) => {
                Err(Failure::Protocol(format!("no answer for {} s", stall.as_secs_f64())))
            }
        }
    }

    /// The samples answered to `request`, at most `want` of them.
    fn read(&mut self, request: &str, want: usize) -> std::result::Result<Vec<f64>, Failure> {
        while let Ok(b) = self.output.try_recv() {
            self.buf.extend(b);
        }
        if !self.buf.is_empty() {
            return Err(Failure::Protocol("the adapter wrote more than it was asked for".into()));
        }
        if writeln!(self.input, "{request}").and_then(|_| self.input.flush()).is_err() {
            return Err(Failure::Ended);
        }
        let line = loop {
            if let Some(i) = self.buf.iter().position(|&c| c == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                break String::from_utf8_lossy(&line).into_owned();
            }
            if self.buf.len() > MAX_LINE {
                return Err(Failure::Protocol("an answer line longer than 1 MiB".into()));
            }
            if !self.more()? {
                return Err(if self.buf.is_empty() {
                    Failure::Ended
                } else {
                    Failure::Protocol("the session ended inside an answer".into())
                });
            }
        };
        let d = json(&line).map_err(|e| Failure::Protocol(e.to_string()))?;
        if let Some(e) = d.get("error").and_then(Value::as_str) {
            return Err(Failure::Answer(e.to_string()));
        }
        let n = match d.get("n").and_then(Value::as_int) {
            Some(n) if n >= 0 && n <= want as i128 => n as usize,
            _ => {
                return Err(Failure::Protocol(format!(
                    "an answer of {} samples, for {want} asked",
                    d.get("n").map(Value::py_str).unwrap_or_default()
                )))
            }
        };
        let bytes = n * 8; // n ≤ READ_CHUNK
        while self.buf.len() < bytes {
            if !self.more()? {
                return Err(Failure::Protocol("the session ended inside an answer".into()));
            }
        }
        let rest = self.buf.split_off(bytes);
        let x = (0..n).map(|i| f64::from_le_bytes(self.buf[8 * i..8 * i + 8].try_into().expect("8 bytes"))).collect();
        self.buf = rest;
        if !self.buf.is_empty() {
            return Err(Failure::Protocol("the adapter wrote more than it was asked for".into()));
        }
        Ok(x)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        stop(&mut self.child);
    }
}
