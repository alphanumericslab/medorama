//! The viewer Medorama runs for its users (docs/architecture.md, "Running separately and
//! together"): which one, starting it on first use, restarting it, stopping it.
//!
//! - **Which:** the setting `viewer`: `auto` (the default: the choice recorded by
//!   tools/install-medorama in `medorama.viewer` next to Medorama's program, else
//!   the first of vitalstream-cpp, vitalstream-rs, vitalstream-py found), `cpp`, `rs`, `py`,
//!   `none`, or the path of a viewer program.  With `viewer_url` set, Medorama links to that
//!   viewer instead and runs none.
//! - **How:** as its own process on 127.0.0.1 and a free port, with a settings file the
//!   Medorama writes (`<home>/viewer/vitalstream.toml`: cache folder, adapter folder, time zone,
//!   folders), and a private password file (`<home>/viewer/auth.json`) whose signing key only
//!   Medorama knows: its gateway signs every request it passes on, and the
//!   viewer's port is useless to anyone else.  Output goes to `<home>/viewer/viewer.log`.
//! - **When:** at the first request for it; again if its process has ended, or if a request
//!   cannot reach it and it fails its health check.  A request to a running viewer costs no
//!   check and waits for no other request (only starting one is done one at a time), so that a
//!   viewer busy with a large recording is never taken for a dead one.  Medorama stops it when
//!   it stops itself (Ctrl-C, or a service manager's SIGTERM).  A viewer left running by an
//!   earlier run that ended otherwise (`viewer.pid`) is stopped and a new one started, so that
//!   the viewer always has Medorama's current settings and add-ons.
//! - **Indexes and adapters:** the viewer reads the index stores of the recording databases
//!   (index format 3, viewers/vitalstream/docs/index-format.md), whose paths and keys Medorama
//!   writes on its standard input (`--keys-on-stdin`: at every start, then a line when a database
//!   is connected or removed; never on a command line, in the environment or in a file); what it
//!   indexes itself goes into its own store (`<workspace>/viewer/index.sqlite`, with a key of its
//!   own).  It uses Medorama's `adapters_dir`.

use crate::config::Config;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Viewers that can run under Medorama, in the order `auto` tries them.
const FLAVORS: &[(&str, &str)] = &[("cpp", "vitalstream-cpp"), ("rs", "vitalstream-rs"), ("py", "vitalstream-py")];
/// The file tools/install-medorama writes next to the programs.
pub const CHOICE_FILE: &str = "medorama.viewer";
const START_TIMEOUT: Duration = Duration::from_secs(30);

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Folders to look in: next to this program (as called, e.g. `.venv/bin`, and as resolved,
/// e.g. `target/release`), the clone's `.venv`, the C++ viewer's build folders and the
/// workspace's `target/release`, then the `PATH`.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    if let Some(a) = std::env::args_os().next() {
        let p = PathBuf::from(a);
        if p.components().count() > 1 {
            if let Some(d) = p.parent() {
                dirs.push(recording_engine::paths::absolute(&d.to_string_lossy()).into());
            }
        }
    }
    if let Ok(e) = std::env::current_exe() {
        for p in [e.clone(), std::fs::canonicalize(&e).unwrap_or(e)] {
            if let Some(d) = p.parent() {
                dirs.push(d.to_path_buf());
            }
            if let Some(root) = p.ancestors().find(|a| a.join("medorama").join("Cargo.toml").is_file()) {
                let vs = root.join("viewers").join("vitalstream");
                dirs.push(root.join(".venv").join(if cfg!(windows) { "Scripts" } else { "bin" }));
                dirs.push(vs.join("cpp").join("build").join("viewer").join("bin"));
                dirs.push(vs.join("cpp").join("build").join("release").join("bin"));
                dirs.push(root.join("target").join("release"));
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

/// A file the installer wrote next to the programs (`.venv/bin`), trimmed: `medorama.viewer`,
/// `medorama.demo` (demo.rs).
pub fn installer_choice(file: &str) -> Option<String> {
    search_dirs().iter().take(4).find_map(|d| std::fs::read_to_string(d.join(file)).ok()).map(|s| s.trim().to_string())
}

fn find_program(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    // the Python viewer is a script (vitalstream-py, or vitalstream-py.exe on Windows)
    dirs.iter().map(|d| d.join(exe(name))).find(|p| p.is_file())
}

/// The viewer to run, or why there is none.
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    /// `cpp`, `rs`, `py` (or `custom` for a program given by path).
    pub flavor: String,
    #[serde(skip)]
    pub program: PathBuf,
}

pub fn choose(cfg: &Config) -> Result<Choice, String> {
    let dirs = search_dirs();
    let mut want = cfg.viewer.trim().to_string();
    if want.is_empty() || want == "auto" {
        // the installer's choice, next to Medorama's program
        want = installer_choice(CHOICE_FILE).unwrap_or_else(|| "auto".into());
    }
    match want.as_str() {
        "none" => Err("no viewer is installed with Medorama (viewer = \"none\")".into()),
        "auto" => FLAVORS
            .iter()
            .find_map(|(f, n)| find_program(n, &dirs).map(|p| Choice { flavor: f.to_string(), program: p }))
            .ok_or_else(|| {
                "no viewer found next to Medorama: install one with tools/install-medorama.sh --viewer cpp|rs|py".into()
            }),
        f if FLAVORS.iter().any(|(k, _)| *k == f) => {
            let name = FLAVORS.iter().find(|(k, _)| *k == f).unwrap().1;
            find_program(name, &dirs).map(|p| Choice { flavor: f.to_string(), program: p }).ok_or_else(|| {
                format!("{name} is not installed (viewer = \"{f}\"): tools/install-medorama.sh --viewer {f}")
            })
        }
        path => {
            let p = PathBuf::from(recording_engine::paths::absolute(path));
            if p.is_file() {
                Ok(Choice { flavor: "custom".into(), program: p })
            } else {
                Err(format!("viewer = {path:?}: no such program"))
            }
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("system random number generator");
    b
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

struct Running {
    port: u16,
    child: Child,
    /// The stores' lines to the viewer's standard input, written by a thread of their own (a
    /// viewer that stops reading never blocks the server).
    lines: Option<std::sync::mpsc::Sender<String>>,
}

/// The thread that writes lines to a child's standard input until the sender is dropped or
/// the child stops reading.
fn pipe_writer(mut stdin: std::process::ChildStdin) -> std::sync::mpsc::Sender<String> {
    use std::io::Write;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in rx {
            if writeln!(stdin, "{line}").and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
    });
    tx
}

/// The lines that give the viewer its index stores: `{"store": PATH, "key": HEX}` (`"own":
/// true` for the one it writes), from Medorama's control store at each start.
pub type StoreLines = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

/// The viewer's store lines now: each recording database's index store with its key, and the
/// viewer's own store (`"own": true`).  A database whose key cannot be had is left out.
pub fn store_lines(cfg: &Config, c: &crate::sql::Connection) -> anyhow::Result<Vec<String>> {
    let line = |path: &Path, k: &crate::workspace::Key, own: bool| {
        serde_json::json!({"store": path.to_string_lossy(), "key": crate::workspace::key_hex(k), "own": own})
            .to_string()
    };
    let m = crate::workspace::loaded()?;
    let mut out = vec![];
    for s in crate::sources::list(c)? {
        if !s.has_recordings() {
            continue;
        }
        if let Ok(Some(k)) = crate::workspace::key(c, m, &crate::workspace::database_owner(s.id)) {
            out.push(line(&crate::workspace::database_dir(cfg, s.id).join("index.sqlite"), &k, false));
        }
    }
    let (own, k) = crate::workspace::viewer_store(cfg, c)?;
    out.push(line(&own, &k, true));
    Ok(out)
}

/// The line that makes the viewer forget a database's store (before it is removed).
pub fn forget_line(cfg: &Config, database: i64) -> String {
    let path = crate::workspace::database_dir(cfg, database).join("index.sqlite");
    serde_json::json!({"forget": path.to_string_lossy()}).to_string()
}

/// What Medorama's page needs to know about the viewer.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// `embedded` (Medorama runs a viewer), `link` (viewer_url), or `none`.
    pub mode: String,
    pub flavor: Option<String>,
    pub running: bool,
    /// For `none`: why.
    pub reason: String,
    pub url: String,
}

/// The viewer under Medorama.
pub struct Supervisor {
    choice: Result<Choice, String>,
    link: String,
    dir: PathBuf,
    /// The viewer's cache folder (its own store is given on its standard input).
    cache_dir: PathBuf,
    adapters_dir: PathBuf,
    timezone: String,
    data_roots: Vec<String>,
    qc_verbose: bool,
    secret: Vec<u8>,
    /// The viewer started (held only to read its port or see whether its process ended).
    state: std::sync::Mutex<Option<Running>>,
    /// One start (and one recovery) at a time.
    starting: tokio::sync::Mutex<()>,
    /// The stores' lines (set by the server: [`Supervisor::set_store_lines`]).
    store_lines: std::sync::Mutex<Option<StoreLines>>,
}

impl Supervisor {
    pub fn new(cfg: &Config) -> Supervisor {
        let dir = cfg.home.join("viewer");
        let choice = if cfg.viewer_url.is_empty() { choose(cfg) } else { Err("viewer_url is set".into()) };
        let secret = if choice.is_ok() { Self::auth_secret(&dir).unwrap_or_else(|_| random(32)) } else { vec![] };
        Supervisor {
            choice,
            link: cfg.viewer_url.clone(),
            dir,
            cache_dir: cfg.workspace_dir.join("viewer"),
            adapters_dir: cfg.adapters_dir.clone(),
            timezone: cfg.timezone.clone(),
            data_roots: cfg.data_roots.clone(),
            qc_verbose: cfg.qc_verbose,
            secret,
            state: std::sync::Mutex::new(None),
            starting: tokio::sync::Mutex::new(()),
            store_lines: std::sync::Mutex::new(None),
        }
    }

    /// What lists the stores' lines, written to the viewer at each start.
    pub fn set_store_lines(&self, f: StoreLines) {
        *self.store_lines.lock().unwrap() = Some(f);
    }

    /// One line to the running viewer (a database connected: its store; removed: `forget`).
    pub fn tell(&self, line: &str) {
        if let Some(r) = self.state.lock().unwrap().as_ref() {
            if let Some(w) = &r.lines {
                let _ = w.send(line.to_string());
            }
        }
    }

    /// The private password file (created once, readable only by Medorama's account).
    fn auth_secret(dir: &Path) -> std::io::Result<Vec<u8>> {
        let file = dir.join("auth.json");
        if let Ok(t) = std::fs::read_to_string(&file) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if let Some(s) = v.get("secret").and_then(|s| s.as_str()) {
                    if let Some(b) = (0..s.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
                        .collect::<Option<Vec<u8>>>()
                    {
                        if b.len() >= 16 {
                            return Ok(b);
                        }
                    }
                }
            }
        }
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let secret = random(32);
        // The viewers' password-file format (scrypt hash, salt, signing key).  The hash is of
        // no password anyone knows: nobody signs in to this viewer with a password; Medorama
        // signs requests with the key.
        let data = serde_json::json!({
            "scheme": "scrypt", "salt": hex(&random(16)), "hash": hex(&random(32)), "secret": hex(&secret),
        });
        let tmp = dir.join("auth.json.tmp");
        std::fs::write(&tmp, data.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &file)?;
        Ok(secret)
    }

    /// The sign-in cookie the viewer accepts (`<expiry>.<HMAC-SHA256(key, expiry)>`), named
    /// `vitalstream_<port>`.
    pub fn cookie(&self, port: u16) -> String {
        let expires = now() + 3600;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts any key length");
        mac.update(expires.to_string().as_bytes());
        format!("vitalstream_{port}={expires}.{}", hex(&mac.finalize().into_bytes()))
    }

    pub fn status(&self) -> Status {
        if !self.link.is_empty() {
            return Status {
                mode: "link".into(),
                flavor: None,
                running: false,
                reason: String::new(),
                url: self.link.clone(),
            };
        }
        match &self.choice {
            Ok(c) => Status {
                mode: "embedded".into(),
                flavor: Some(c.flavor.clone()),
                running: self.state.lock().unwrap().is_some(),
                reason: String::new(),
                url: "viewer/".into(),
            },
            Err(e) => {
                Status { mode: "none".into(), flavor: None, running: false, reason: e.clone(), url: String::new() }
            }
        }
    }

    pub fn available(&self) -> bool {
        self.link.is_empty() && self.choice.is_ok()
    }

    fn healthy(&self, port: u16) -> bool {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        use std::io::{Read, Write};
        let req = format!(
            "GET /api/v1/config HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {}\r\nConnection: close\r\n\r\n",
            self.cookie(port)
        );
        if s.write_all(req.as_bytes()).is_err() {
            return false;
        }
        let mut head = [0u8; 12];
        s.read_exact(&mut head).is_ok() && &head[9..12] == b"200"
    }

    /// The port of the viewer this Medorama started, while its process runs (no request made).
    fn running_port(&self) -> Option<u16> {
        let mut st = self.state.lock().unwrap();
        let r = st.as_mut()?;
        if matches!(r.child.try_wait(), Ok(Some(_))) {
            *st = None;
            let _ = std::fs::remove_file(self.dir.join("viewer.pid"));
            return None;
        }
        Some(r.port)
    }

    /// The port of a running viewer: the one already running, or a new one.
    pub async fn ensure(&self) -> Result<u16, String> {
        let choice = self.choice.clone()?;
        if let Some(port) = self.running_port() {
            return Ok(port);
        }
        let _one = self.starting.lock().await;
        // another request may have started it meanwhile
        if let Some(port) = self.running_port() {
            return Ok(port);
        }
        // a viewer left by an earlier run of Medorama that did not stop it: its settings and
        // add-ons may be out of date
        if let Some((pid, port)) = std::fs::read_to_string(self.dir.join("viewer.pid")).ok().and_then(|t| {
            let mut w = t.split_whitespace();
            Some((w.next()?.parse::<u32>().ok()?, w.next()?.parse::<u16>().ok()?))
        }) {
            tokio::task::block_in_place(|| self.stop_leftover(pid, port));
        }
        let port = tokio::task::block_in_place(|| self.start(&choice))?;
        Ok(port)
    }

    /// A request could not connect to the viewer on `port`: the port of one that answers (the
    /// same, if it passes its health check after all; else a new one, the old one stopped).
    pub async fn unreachable(&self, port: u16) -> Result<u16, String> {
        {
            let _one = self.starting.lock().await;
            match self.running_port() {
                Some(p) if p != port => return Ok(p), // replaced meanwhile
                Some(p) if tokio::task::block_in_place(|| self.healthy(p)) => return Ok(p),
                Some(_) => self.stop(),
                None => {}
            }
        }
        self.ensure().await
    }

    fn settings_file(&self, port: u16) -> Result<PathBuf, String> {
        let q = |p: &Path| toml::Value::String(p.to_string_lossy().to_string()).to_string();
        let roots =
            if self.data_roots.is_empty() { vec![recording_engine::paths::fs_root()] } else { self.data_roots.clone() };
        let roots: Vec<String> = roots.iter().map(|r| toml::Value::String(r.clone()).to_string()).collect();
        let mut text = format!(
            "# Written by medorama for the viewer it runs; rewritten at every start.\n\
             host = \"127.0.0.1\"\nport = {port}\ncache_dir = {}\nadapters_dir = {}\nauth_file = {}\ntimezone = {}\n\
             data_roots = [{}]\nrequire_password = true\n",
            q(&self.cache_dir),
            q(&self.adapters_dir),
            q(&self.dir.join("auth.json")),
            toml::Value::String(self.timezone.clone()),
            roots.join(", ")
        );
        // only when on: a viewer older than the setting refuses keys it does not know
        if self.qc_verbose {
            text.push_str("qc_verbose = true\n");
        }
        let file = self.dir.join("vitalstream.toml");
        std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
        Ok(file)
    }

    fn start(&self, choice: &Choice) -> Result<u16, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let port = TcpListener::bind(("127.0.0.1", 0))
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .map_err(|e| format!("no free port: {e}"))?;
        let settings = self.settings_file(port)?;
        let log_path = self.dir.join("viewer.log");
        let log = std::fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&choice.program);
        // none of Medorama's secrets in its environment (its keys come on its standard input)
        for n in crate::actions::secret_env(std::env::vars_os().map(|(k, _)| k)) {
            cmd.env_remove(n);
        }
        let mut child = cmd
            .arg("--config")
            .arg(&settings)
            .arg("--no-browser")
            .arg("--keys-on-stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", choice.program.display()))?;
        // its stores, with their keys, before anything is asked of it
        let lines = child.stdin.take().map(pipe_writer);
        if let (Some(w), Some(f)) = (&lines, self.store_lines.lock().unwrap().clone()) {
            for l in f() {
                let _ = w.send(l);
            }
        }
        let t0 = Instant::now();
        while t0.elapsed() < START_TIMEOUT {
            if let Ok(Some(status)) = child.try_wait() {
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(6).collect();
                return Err(format!(
                    "{} stopped ({status}): {}",
                    choice.program.display(),
                    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                ));
            }
            if self.healthy(port) {
                let _ = std::fs::write(self.dir.join("viewer.pid"), format!("{} {port}\n", child.id()));
                *self.state.lock().unwrap() = Some(Running { port, child, lines });
                return Ok(port);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = child.kill();
        let _ = child.wait();
        Err(format!(
            "{} did not answer within {} s (log: {})",
            choice.program.display(),
            START_TIMEOUT.as_secs(),
            log_path.display()
        ))
    }

    /// Stop the viewer this Medorama started.
    pub fn stop(&self) {
        if let Some(mut r) = self.state.lock().unwrap().take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
            let _ = std::fs::remove_file(self.dir.join("viewer.pid"));
        }
    }

    /// Stop the viewer process `pid` an earlier run left, if a viewer still answers on its
    /// `port` (so that the process is that viewer), and wait until it has gone.
    fn stop_leftover(&self, pid: u32, port: u16) {
        if self.healthy(port) {
            #[cfg(unix)]
            let _ = Command::new("kill").arg(pid.to_string()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            #[cfg(windows)]
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let end = Instant::now() + Duration::from_secs(5);
            while self.healthy(port) && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = std::fs::remove_file(self.dir.join("viewer.pid"));
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The installer's files (`medorama.viewer`, `medorama.demo`) are read from the clone's
    /// `.venv/bin`, whatever the program resolves to: on Linux, `.venv/bin/medorama` is a link
    /// that the running program's path resolves to `target/release` (a test: `target/debug/deps`).
    #[test]
    fn the_installers_files_are_read_from_the_clones_venv() {
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let root = exe.ancestors().find(|a| a.join("medorama").join("Cargo.toml").is_file()).unwrap().to_path_buf();
        let venv = root.join(".venv").join(if cfg!(windows) { "Scripts" } else { "bin" });
        let looked: Vec<PathBuf> = search_dirs().into_iter().take(4).collect();
        assert!(
            looked.iter().any(|d| std::fs::canonicalize(d).unwrap_or(d.clone()) == venv || *d == venv),
            "{looked:?}"
        );
    }

    /// A supervisor of a "viewer" that is a process which never answers (as a viewer busy with
    /// a large recording), on a port nobody listens on.
    fn busy() -> (tempfile::TempDir, Supervisor, u16) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::new(dir.path(), &dir.path().join("results"));
        let mut s = Supervisor::new(&cfg);
        s.choice = Ok(Choice { flavor: "custom".into(), program: PathBuf::from("/bin/sleep") });
        let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        *s.state.lock().unwrap() = Some(Running { port, child, lines: None });
        (dir, s, port)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_running_viewer_is_used_without_a_check() {
        let (_dir, s, port) = busy();
        // many requests at once: none waits, none checks, the viewer is kept
        let t = Instant::now();
        for _ in 0..50 {
            assert_eq!(s.ensure().await, Ok(port));
        }
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
        assert!(s.status().running);
        // its process ended: noticed, and a new one started (here, one that cannot start)
        s.state.lock().unwrap().as_mut().unwrap().child.kill().unwrap();
        s.state.lock().unwrap().as_mut().unwrap().child.wait().unwrap();
        assert!(s.ensure().await.is_err());
        assert!(!s.status().running);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreachable_viewer_is_checked_then_replaced() {
        let (_dir, s, port) = busy();
        // nothing answers on its port: it fails its check, is stopped, and a new one is started
        assert!(s.unreachable(port).await.is_err(), "/bin/sleep is no viewer");
        assert!(!s.status().running);
    }
}
