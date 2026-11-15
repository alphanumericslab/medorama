//! db-builder's web interface, run by Medorama (docs/administration.md, "db-builder"): the
//! tool that turns tables, exports and folders of recordings into databases Medorama connects.
//! Medorama does not proxy it and shares no sign-in with it: the page's "Create a database…"
//! opens db-builder's own page, in a browser tab of its own, where its own account signs in.
//!
//! - **Off** when `db_builder_root` is empty (the default).
//! - **Linked** when `db_builder_url` names a db-builder running elsewhere: Medorama runs none.
//! - **Run** otherwise: Medorama finds the `db-builder` program as it finds the viewer (next to
//!   itself, `.venv/bin`, the workspace's `target/release`, the `PATH`) and runs
//!   `db-builder serve --root <db_builder_root> --host 127.0.0.1 --port <db_builder_port>
//!   --home <home>/db-builder` from its start to its end, started again (after a pause) when it
//!   stops.  Its home holds its password (set once with `db-builder serve --set-password
//!   --home <home>/db-builder`: until then it is not started, and the page says why), its log of
//!   actions, and its output (`db-builder.out`).

use crate::config::Config;
use serde::Serialize;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the process is looked at; the pause before it is started again grows from the
/// first to the last of these when it keeps stopping.
const LOOK: Duration = Duration::from_secs(2);
const PAUSE_MIN: Duration = Duration::from_secs(5);
const PAUSE_MAX: Duration = Duration::from_secs(60);

/// What Medorama's page is told (admins only: `GET config`).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Status {
    /// `off`, `link` (db_builder_url) or `run` (Medorama runs it).
    pub mode: String,
    /// Where its page is (empty when off).
    pub url: String,
    pub running: bool,
    /// Why it does not run, if it does not.
    pub reason: String,
}

#[derive(Debug, Clone)]
enum Mode {
    Off,
    Link(String),
    Run { program: Result<PathBuf, String>, root: PathBuf, port: u16, home: PathBuf },
}

#[derive(Default)]
struct State {
    child: Option<Child>,
    reason: String,
}

/// db-builder under Medorama.
pub struct DbBuilder {
    mode: Mode,
    state: Mutex<State>,
    stop: AtomicBool,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The `db-builder` program: where the viewer is looked for (viewer.rs).
fn find_program() -> Result<PathBuf, String> {
    crate::viewer::search_dirs().iter().map(|d| d.join(exe("db-builder"))).find(|p| p.is_file()).ok_or_else(|| {
        "the db-builder program is not installed next to Medorama (tools/install-medorama installs it)".into()
    })
}

impl DbBuilder {
    pub fn new(cfg: &Config) -> DbBuilder {
        let mode = if !cfg.db_builder_url.trim().is_empty() {
            Mode::Link(cfg.db_builder_url.trim().to_string())
        } else {
            match &cfg.db_builder_root {
                None => Mode::Off,
                Some(root) => Mode::Run {
                    program: find_program(),
                    root: root.clone(),
                    port: cfg.db_builder_port,
                    home: cfg.home.join("db-builder"),
                },
            }
        };
        DbBuilder { mode, state: Mutex::new(State::default()), stop: AtomicBool::new(false) }
    }

    pub fn status(&self) -> Status {
        match &self.mode {
            Mode::Off => Status { mode: "off".into(), url: String::new(), running: false, reason: String::new() },
            Mode::Link(url) => Status { mode: "link".into(), url: url.clone(), running: false, reason: String::new() },
            Mode::Run { port, .. } => {
                let st = self.state.lock().unwrap();
                Status {
                    mode: "run".into(),
                    url: format!("http://127.0.0.1:{port}/"),
                    running: st.child.is_some(),
                    reason: st.reason.clone(),
                }
            }
        }
    }

    /// Why it cannot be started now, if it cannot: no program, no root folder, no password.
    fn cannot(&self) -> Option<String> {
        let Mode::Run { program, root, home, .. } = &self.mode else { return Some("not run by Medorama".into()) };
        if let Err(e) = program {
            return Some(e.clone());
        }
        if !root.is_dir() {
            return Some(format!("db_builder_root {}: no such folder", root.display()));
        }
        if !home.join("password").is_file() {
            return Some(format!(
                "its password is not set yet: run db-builder serve --set-password --home {}",
                home.display()
            ));
        }
        None
    }

    fn spawn(&self) -> Result<Child, String> {
        let Mode::Run { program: Ok(program), root, port, home } = &self.mode else {
            return Err("not run by Medorama".into());
        };
        let out = home.join("db-builder.out");
        let log = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(program);
        cmd.arg("serve")
            .arg("--root")
            .arg(root)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .arg("--home")
            .arg(home)
            // its databases are its own: not their key, nor Medorama's secrets (the master key,
            // the control store's password …), as plug-ins get none (actions.rs secret_env)
            .env_remove("DB_BUILDER_KEY");
        for n in crate::actions::secret_env(std::env::vars_os().map(|(k, _)| k)) {
            cmd.env_remove(n);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", program.display()))
    }

    /// Run it from now on, on a thread of its own that starts it again when it stops, until
    /// [`DbBuilder::stop`].  Nothing is done unless Medorama runs it.
    pub fn start(self: &Arc<Self>) {
        if !matches!(self.mode, Mode::Run { .. }) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let mut pause = PAUSE_MIN;
            let mut next = Instant::now();
            let mut started = Instant::now();
            while !me.stop.load(Ordering::Relaxed) {
                {
                    let mut st = me.state.lock().unwrap();
                    let ended = st.child.as_mut().and_then(|c| c.try_wait().ok().flatten());
                    if let Some(status) = ended {
                        st.child = None;
                        // one that ran a while: started again soon; one that keeps stopping: later
                        if started.elapsed() > PAUSE_MAX {
                            pause = PAUSE_MIN;
                        }
                        st.reason = format!("it stopped ({status}): started again in {} s", pause.as_secs());
                        eprintln!("medorama: db-builder stopped ({status}); see {}", me.out_file());
                        next = Instant::now() + pause;
                        pause = (pause * 2).min(PAUSE_MAX);
                    } else if st.child.is_none() && Instant::now() >= next {
                        match me.cannot() {
                            Some(why) => st.reason = why,
                            None => match me.spawn() {
                                Ok(c) => {
                                    st.child = Some(c);
                                    st.reason.clear();
                                    started = Instant::now();
                                }
                                Err(e) => st.reason = e,
                            },
                        }
                        next = Instant::now() + PAUSE_MIN;
                    }
                }
                std::thread::sleep(LOOK);
            }
        });
    }

    fn out_file(&self) -> String {
        match &self.mode {
            Mode::Run { home, .. } => home.join("db-builder.out").display().to_string(),
            _ => String::new(),
        }
    }

    /// Stop it (Medorama stops).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut c) = self.state.lock().unwrap().child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// What `medorama serve` says of it at start.
    pub fn describe(&self) -> String {
        match &self.mode {
            Mode::Off => "off (db_builder_root is empty)".into(),
            Mode::Link(url) => format!("{url} (db_builder_url)"),
            Mode::Run { root, port, .. } => match self.cannot() {
                None => format!("http://127.0.0.1:{port}/, building databases in {}", root.display()),
                Some(why) => format!("not started: {why}"),
            },
        }
    }
}

impl Drop for DbBuilder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_status_follows_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::new(dir.path(), &dir.path().join("ws"));
        assert_eq!(DbBuilder::new(&cfg).status().mode, "off");
        cfg.db_builder_root = Some(dir.path().join("build"));
        cfg.db_builder_port = 18091;
        let s = DbBuilder::new(&cfg).status();
        assert_eq!((s.mode.as_str(), s.url.as_str(), s.running), ("run", "http://127.0.0.1:18091/", false));
        // not started without its folder, its program or its password; says why
        let d = DbBuilder::new(&cfg);
        assert!(d.cannot().is_some());
        std::fs::create_dir_all(dir.path().join("build")).unwrap();
        let d = DbBuilder {
            mode: Mode::Run {
                program: Ok("/bin/true".into()),
                root: dir.path().join("build"),
                port: 1,
                home: dir.path().join("h"),
            },
            state: Mutex::new(State::default()),
            stop: AtomicBool::new(false),
        };
        assert!(d.cannot().unwrap().contains("--set-password"));
        cfg.db_builder_url = "http://host:8091/".into();
        let s = DbBuilder::new(&cfg).status();
        assert_eq!((s.mode.as_str(), s.url.as_str()), ("link", "http://host:8091/"));
    }
}
