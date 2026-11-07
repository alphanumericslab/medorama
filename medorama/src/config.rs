//! Settings: the catalog's home folder holds `medorama.toml` and (by default) the database.
//! `medorama init` writes both; the results folder is chosen then.
//!
//! Home folder, the first given: `--home DIR`, the environment variable `MEDORAMA_HOME`, the
//! catalog `init` created last (remembered in [`pointer_file`]), else [`builtin_home`].
//! Relative paths in `medorama.toml` are relative to the home folder.

use recording_engine::paths::{absolute, expand, plain};
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const DEFAULT_PORT: u16 = 8090;
pub const CONFIG_NAME: &str = "medorama.toml";

#[derive(Debug, Clone)]
pub struct Config {
    /// Folder of medorama.toml.
    pub home: PathBuf,
    /// The SQLite database (holds identifiers: keep it private).
    pub database: PathBuf,
    /// A PostgreSQL database instead (`postgres://user@host/db?sslmode=require`), without its
    /// password; `$MEDORAMA_DATABASE_URL` overrides it.  Empty: the SQLite `database`.
    pub database_url: String,
    /// The PostgreSQL password, in a file readable only by Medorama's account
    /// (`$MEDORAMA_DATABASE_PASSWORD` overrides it).
    pub database_password_file: Option<PathBuf>,
    /// Certificate authorities (PEM) trusted for the database server's certificate, besides
    /// the system's.
    pub database_ca_file: Option<PathBuf>,
    /// Where actions write their results (`<results_dir>/actions/...`).
    pub results_dir: PathBuf,
    /// Index folders (one per recording, the viewers' format).
    pub index_dir: PathBuf,
    /// The folder of adapters: programs that read recording formats Medorama does not read
    /// itself (docs/adapters.md).
    pub adapters_dir: PathBuf,
    pub host: String,
    pub port: u16,
    /// Jobs run at the same time by the server (and by `work`).
    pub workers: usize,
    /// The server queues indexing for new and changed recordings by itself.
    pub auto_index: bool,
    /// The server re-checks the recordings (missing, changed) this often; 0 = never.
    pub check_interval_s: u64,
    /// Time zone for times in imported CSV files without an offset, and for display.
    pub timezone: String,
    /// The viewer the catalog runs for its users: `auto`, `cpp`, `rs`, `py`, `none`, or the
    /// path of a viewer program (see viewer.rs).
    pub viewer: String,
    /// A viewer running elsewhere, linked to instead of running one (e.g. http://host:8060/).
    pub viewer_url: String,
    /// The viewer the catalog runs also marks the data a reader repaired (its `qc_verbose`).
    pub qc_verbose: bool,
    /// Folders recordings may be imported from; empty = anywhere.
    pub data_roots: Vec<String>,
    /// Folders holding action plug-ins (each a subfolder with action.toml).
    pub actions_dirs: Vec<PathBuf>,
    /// Folders holding import formats described in TOML files (formats/declared.rs).
    pub formats_dirs: Vec<PathBuf>,
    pub session_hours: u64,
    /// A session unused this long ends (minutes; 0: only `session_hours` ends it).
    pub session_idle_minutes: u64,
    /// Mark the sign-in cookie Secure (set when the catalog is served over HTTPS).
    pub secure_cookies: bool,
    /// How users sign in (signin.rs): `sign_in = "password"` (the default), or `"proxy"` with
    /// `proxy_user_header`, `proxy_addresses` and `proxy_logout_url`.
    pub sign_in: crate::signin::Method,
    /// File holding the database key (builds with the `encryption` feature).
    pub key_file: Option<PathBuf>,
    /// A file the audit log's sealed entries are appended to (audit.rs), on storage the
    /// database's administrators cannot change.
    pub audit_export: Option<PathBuf>,
}

/// The sign-in method the settings name (signin.rs): `password` (the default), or `proxy` with
/// the header the proxy sets (default `X-Remote-User`), the addresses it connects from (default
/// this computer: `127.0.0.1`, `::1`) and its sign-out address.  The proxy's settings without
/// `sign_in = "proxy"` are an error, so that a half-made change is not silently ignored.
fn sign_in_method(
    method: Option<String>,
    header: Option<String>,
    from: Option<Vec<String>>,
    logout_url: Option<String>,
) -> Result<crate::signin::Method, String> {
    match method.as_deref().unwrap_or("password") {
        "password" => {
            if header.is_some() || from.is_some() || logout_url.is_some() {
                return Err("proxy_user_header, proxy_addresses and proxy_logout_url need sign_in = \"proxy\"".into());
            }
            Ok(crate::signin::Method::Password)
        }
        "proxy" => {
            let header = header.unwrap_or_else(|| "X-Remote-User".into());
            if axum::http::HeaderName::from_bytes(header.as_bytes()).is_err() {
                return Err(format!("proxy_user_header: {header:?} is not a header name"));
            }
            let from = from.unwrap_or_else(|| vec!["127.0.0.1".into(), "::1".into()]);
            let from = from
                .iter()
                .map(|a| a.trim().parse().map_err(|_| format!("proxy_addresses: {a:?} is not an IP address")))
                .collect::<Result<Vec<std::net::IpAddr>, _>>()?;
            if from.is_empty() {
                return Err("proxy_addresses: name the address the proxy connects from".into());
            }
            Ok(crate::signin::Method::Proxy { header, from, logout_url: logout_url.unwrap_or_default() })
        }
        other => Err(format!("sign_in: {other:?} is not a sign-in method (password, proxy)")),
    }
}

/// The built-in home folder: `~/.local/share/medorama` (Windows: `%LOCALAPPDATA%\medorama`).
pub fn builtin_home() -> PathBuf {
    match std::env::var("LOCALAPPDATA").ok().filter(|_| cfg!(windows)) {
        Some(l) => PathBuf::from(l).join("medorama"),
        None => PathBuf::from(expand("~/.local/share/medorama")),
    }
}

/// A file in the user's settings folder: `~/.config/<parts>` (Windows: `%APPDATA%\<parts>`).
fn settings_file(parts: &[&str]) -> PathBuf {
    if cfg!(windows) {
        if let Ok(a) = std::env::var("APPDATA") {
            return parts.iter().fold(PathBuf::from(a), |p, part| p.join(part));
        }
    }
    parts.iter().fold(PathBuf::from(expand("~/.config")), |p, part| p.join(part))
}

/// The file where `init` remembers the home folder of the catalog it created, so that later
/// commands find it without `--home`: `~/.config/medorama/home` (Windows:
/// `%APPDATA%\medorama\home`).
pub fn pointer_file() -> PathBuf {
    settings_file(&["medorama", "home"])
}

/// Remember `home` as the catalog later commands use by default.
pub fn remember_home(home: &Path) -> std::io::Result<PathBuf> {
    let p = pointer_file();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, format!("{}\n", home.display()))?;
    Ok(p)
}

/// The home folder when `--home` is not given: `$MEDORAMA_HOME`, else the one `init`
/// remembered (if it still holds a catalog), else the built-in one.
pub fn default_home() -> PathBuf {
    if let Some(h) = std::env::var("MEDORAMA_HOME").ok().filter(|h| !h.is_empty()) {
        return PathBuf::from(absolute(&h));
    }
    if let Ok(t) = std::fs::read_to_string(pointer_file()) {
        let h = PathBuf::from(t.trim());
        if !t.trim().is_empty() && h.join(CONFIG_NAME).is_file() {
            return h;
        }
    }
    builtin_home()
}

fn rel(base: &Path, p: &str) -> PathBuf {
    let e = PathBuf::from(expand(p));
    if e.is_absolute() {
        e
    } else {
        PathBuf::from(absolute(&base.join(e).to_string_lossy()))
    }
}

/// True if `path` is `root` or lies inside it (both absolute, links not resolved).
pub fn within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// `path` with symbolic links resolved where it exists (else as given, made absolute).
pub fn real(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(p) => PathBuf::from(plain(&p.to_string_lossy())),
        Err(_) => PathBuf::from(absolute(&path.to_string_lossy())),
    }
}

impl Config {
    /// Defaults for a home folder and a results folder.
    pub fn new(home: &Path, results_dir: &Path) -> Config {
        Config {
            home: home.to_path_buf(),
            database: home.join("medorama.sqlite"),
            database_url: String::new(),
            database_password_file: None,
            database_ca_file: None,
            results_dir: results_dir.to_path_buf(),
            index_dir: results_dir.join("index"),
            adapters_dir: recording_engine::adapters::default_folder(),
            host: "127.0.0.1".into(),
            port: DEFAULT_PORT,
            workers: 2,
            auto_index: true,
            check_interval_s: 900,
            timezone: "America/New_York".into(),
            viewer: "auto".into(),
            viewer_url: String::new(),
            qc_verbose: false,
            data_roots: vec![],
            actions_dirs: vec![home.join("actions")],
            formats_dirs: vec![home.join("formats")],
            session_hours: 12,
            session_idle_minutes: 60,
            secure_cookies: false,
            sign_in: crate::signin::Method::Password,
            key_file: None,
            audit_export: None,
        }
    }

    /// Read `<home>/medorama.toml`.
    pub fn load(home: &Path) -> Result<Config, String> {
        let file = home.join(CONFIG_NAME);
        let text = std::fs::read_to_string(&file).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "no catalog in {}.\n  Create one: medorama init --results DIR [--home DIR]\n  Or use an existing one: --home DIR (or set MEDORAMA_HOME)",
                    home.display()
                )
            } else {
                format!("{}: {e}", file.display())
            }
        })?;
        let table: toml::Table = text.parse().map_err(|e| format!("{}: {e}", file.display()))?;
        let results = table
            .get("results_dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{}: results_dir is required", file.display()))?;
        let mut c = Config::new(home, &rel(home, results));
        let mut index_set = false;
        let mut unknown = vec![];
        let (mut sign_in, mut proxy_header, mut proxy_from, mut proxy_logout) = (None, None, None, None);
        for (k, v) in &table {
            let bad = || format!("{}: bad value for {k}", file.display());
            let s = || v.as_str().ok_or_else(bad);
            let n = || v.as_integer().and_then(|x| u64::try_from(x).ok()).ok_or_else(bad);
            match k.as_str() {
                "results_dir" => {}
                "database" => c.database = rel(home, s()?),
                "database_url" => c.database_url = s()?.trim().to_string(),
                "database_password_file" => c.database_password_file = Some(rel(home, s()?)),
                "database_ca_file" => c.database_ca_file = Some(rel(home, s()?)),
                "index_dir" => {
                    c.index_dir = rel(home, s()?);
                    index_set = true;
                }
                "adapters_dir" => c.adapters_dir = rel(home, s()?),
                "host" => c.host = s()?.to_string(),
                "port" => c.port = u16::try_from(n()?).map_err(|_| bad())?,
                "workers" => c.workers = (n()? as usize).max(1),
                "auto_index" => c.auto_index = v.as_bool().ok_or_else(bad)?,
                "check_interval_s" => c.check_interval_s = n()?,
                "timezone" => c.timezone = s()?.to_string(),
                "viewer" => c.viewer = s()?.to_string(),
                "viewer_url" => c.viewer_url = s()?.to_string(),
                "qc_verbose" => c.qc_verbose = v.as_bool().ok_or_else(bad)?,
                "data_roots" => {
                    c.data_roots = v
                        .as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(|x| x.as_str().map(|p| rel(home, p).to_string_lossy().to_string()).ok_or_else(bad))
                        .collect::<Result<_, _>>()?
                }
                "actions_dirs" => {
                    c.actions_dirs = v
                        .as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(|x| x.as_str().map(|p| rel(home, p)).ok_or_else(bad))
                        .collect::<Result<_, _>>()?
                }
                "formats_dirs" => {
                    c.formats_dirs = v
                        .as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(|x| x.as_str().map(|p| rel(home, p)).ok_or_else(bad))
                        .collect::<Result<_, _>>()?
                }
                "session_hours" => c.session_hours = n()?.max(1),
                "session_idle_minutes" => c.session_idle_minutes = n()?,
                "secure_cookies" => c.secure_cookies = v.as_bool().ok_or_else(bad)?,
                "sign_in" => sign_in = Some(s()?.to_string()),
                "proxy_user_header" => proxy_header = Some(s()?.trim().to_string()),
                "proxy_addresses" => {
                    proxy_from = Some(
                        v.as_array()
                            .ok_or_else(bad)?
                            .iter()
                            .map(|x| x.as_str().ok_or_else(bad).map(str::to_string))
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                }
                "proxy_logout_url" => proxy_logout = Some(s()?.trim().to_string()),
                "key_file" => c.key_file = Some(rel(home, s()?)),
                "audit_export" => c.audit_export = Some(rel(home, s()?)),
                _ => unknown.push(k.clone()),
            }
        }
        if !unknown.is_empty() {
            unknown.sort();
            return Err(format!("{}: unknown settings: {}", file.display(), unknown.join(", ")));
        }
        c.sign_in = sign_in_method(sign_in, proxy_header, proxy_from, proxy_logout)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        if !index_set {
            c.index_dir = c.results_dir.join("index");
        }
        if let Some(u) = std::env::var("MEDORAMA_DATABASE_URL").ok().filter(|u| !u.trim().is_empty()) {
            c.database_url = u.trim().to_string();
        }
        c.check()?;
        // the adapters the recording engine runs, for every command and the server
        recording_engine::adapters::set_folders(vec![c.adapters_dir.clone()]);
        Ok(c)
    }

    /// Consistency checks shared by `load` and `init`.
    pub fn check(&self) -> Result<(), String> {
        chrono_tz::Tz::from_str(&self.timezone).map_err(|_| format!("unknown time zone {:?}", self.timezone))?;
        if !self.database_url.is_empty() {
            let u = self.database_url.to_lowercase();
            if !(u.starts_with("postgres://") || u.starts_with("postgresql://")) {
                return Err(format!("database_url: a PostgreSQL URL (postgres://…), not {:?}", self.database_url));
            }
            if crate::db::shown_url(&self.database_url) != self.database_url {
                return Err(
                    "database_url: leave the password out (database_password_file or $MEDORAMA_DATABASE_PASSWORD)"
                        .into(),
                );
            }
        }
        Ok(())
    }

    /// The text of a new medorama.toml.
    pub fn to_toml(&self) -> String {
        let q = |p: &Path| toml::Value::String(p.to_string_lossy().to_string()).to_string();
        let index = if self.index_dir == self.results_dir.join("index") {
            format!("# index_dir = {}", q(&self.index_dir))
        } else {
            format!("index_dir = {}", q(&self.index_dir))
        };
        format!(
            r#"# Medorama settings (docs/administration.md, "Settings").  Relative paths are
# relative to this folder.

# Where the catalog writes: action results (actions/...) and, unless index_dir says
# otherwise, the recordings' indexes (index/).  Never inside a folder of recordings.
results_dir = {results}
# Index folders.  Point this at the viewers' cache_dir and they open recordings the catalog
# has indexed without indexing them again.
{index}
# Adapters: programs that read recording formats Medorama does not read itself, such as
# licensed vendor formats (docs/adapters.md).
adapters_dir = {adapters}

# The database.  It holds identifiers (MRN, names, dates of birth): keep it private.
{database}

host = "127.0.0.1"
port = {port}
# Jobs (indexing, actions) run at the same time.
workers = {workers}
# Index new and changed recordings in the background while the server runs.
auto_index = true
# Re-check the recordings (missing, changed) every so many seconds; 0 = never.
check_interval_s = 900
# Time zone of CSV times written without an offset, and of the times shown.
timezone = {tz}
# The viewer the catalog runs and shows in its VitalStream tab: "auto" (the one installed with
# tools/install-medorama, else the first found of the C++, Rust and Python viewers), "cpp", "rs",
# "py", "none", or the path of a viewer program.
viewer = "auto"
# Or a viewer running elsewhere, opened in a new browser tab instead (e.g. "http://host:8060/").
viewer_url = ""
# The viewer also marks (dashed outline) the data a reader repaired, such as data blocks a file
# stored out of order and the reader put back in order.  Breaks are shaded either way.
qc_verbose = false
# Folders recordings may be imported from (empty: anywhere).
data_roots = {roots}
# Folders of action plug-ins (each a subfolder with an action.toml).
actions_dirs = ["actions"]
# Folders of import formats described in TOML files (each a <key>.toml: docs/administration.md,
# "Import formats"), besides the built-in ones.
formats_dirs = ["formats"]
# Sign-ins last this long.
session_hours = 12
# A sign-in unused this long ends (minutes; 0: never before session_hours). The page's own
# checks for new data do not count as use.
session_idle_minutes = 60
# Set to true when the catalog is served over HTTPS (behind a reverse proxy).
secure_cookies = false
# How users sign in: "password" (Medorama's own accounts), or "proxy": the institution's sign-in
# (single sign-on, MFA, a VPN gateway) done by an authenticating reverse proxy in front of
# Medorama, which names the user in a header (docs/sign-in.md).  Accounts, their roles and the
# PHI permission stay in Medorama, named as the proxy names the users.
# sign_in = "proxy"
# proxy_user_header = "X-Remote-User"        # the header the proxy sets to the user's name
# proxy_addresses = ["127.0.0.1", "::1"]     # the proxy connects from these; anyone else's header is ignored
# proxy_logout_url = "/oauth2/sign_out"      # where Sign out leads: the proxy's own sign-out
# The audit log is sealed every minute (tamper evidence); also append the sealed entries to this
# file, on storage the database's administrators cannot change (docs/security.md, "Audit logging").
# audit_export = "/var/log/medorama/audit.jsonl"
"#,
            database = if self.database_url.is_empty() {
                "# database = \"medorama.sqlite\"\n# Or a PostgreSQL database, for several servers and many users (docs/administration.md, \"Settings\"):\n# database_url = \"postgres://medorama@db.example.org/medorama?sslmode=require\"\n# database_password_file = \"/etc/medorama/database-password\"".to_string()
            } else {
                let mut d = format!("database_url = {}", toml::Value::String(self.database_url.clone()));
                if let Some(f) = &self.database_password_file {
                    d += &format!("\ndatabase_password_file = {}", q(f));
                }
                if let Some(f) = &self.database_ca_file {
                    d += &format!("\ndatabase_ca_file = {}", q(f));
                }
                d
            },
            results = q(&self.results_dir),
            roots = toml::Value::Array(self.data_roots.iter().map(|r| toml::Value::String(r.clone())).collect()),
            adapters = q(&self.adapters_dir),
            port = self.port,
            workers = self.workers,
            tz = toml::Value::String(self.timezone.clone()),
        )
    }

    /// True if recordings may be imported from `path` (absolute), which must exist: its real
    /// path (symbolic links resolved) lies in a data root's real path.  A link inside a root
    /// that points out of it is outside.
    pub fn import_allowed(&self, path: &Path) -> bool {
        if self.data_roots.is_empty() {
            return true;
        }
        if !path.exists() {
            return false;
        }
        let r = real(path);
        self.data_roots.iter().any(|root| within(&r, &real(Path::new(root))))
    }

    /// What `serve` warns about at start: sessions that would travel without HTTPS.
    pub fn start_warnings(&self) -> Vec<String> {
        let mut out = vec![];
        let loopback = matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1");
        if !loopback && !self.secure_cookies {
            out.push(
                "reachable from other machines without HTTPS.  Serve it behind a reverse proxy with TLS and set \
                 secure_cookies = true (docs/administration.md, \"Deployment\")."
                    .to_string(),
            );
        }
        if matches!(self.sign_in, crate::signin::Method::Proxy { .. }) && !self.secure_cookies {
            out.push(
                "sign_in = \"proxy\" with secure_cookies = false: the proxy serves the catalog over HTTPS; set \
                 secure_cookies = true (docs/sign-in.md)."
                    .to_string(),
            );
        }
        out
    }

    /// The folders the catalog writes to (results, indexes, its home).
    pub fn written_folders(&self) -> Vec<PathBuf> {
        let mut v = vec![real(&self.results_dir), real(&self.index_dir), real(&self.home)];
        if self.database_url.is_empty() {
            if let Some(d) = self.database.parent() {
                v.push(real(d));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_settings() {
        use crate::signin::Method;
        let o = |x: &str| Some(x.to_string());
        assert_eq!(sign_in_method(None, None, None, None), Ok(Method::Password));
        let m = sign_in_method(o("proxy"), None, None, None).unwrap();
        assert_eq!(
            m,
            Method::Proxy {
                header: "X-Remote-User".into(),
                from: vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()],
                logout_url: String::new()
            }
        );
        // a proxy's setting without the method: a mistake, not ignored
        assert!(sign_in_method(None, o("X-User"), None, None).unwrap_err().contains("need sign_in"));
        assert!(sign_in_method(o("proxy"), None, Some(vec!["proxy.example.org".into()]), None).is_err());
        assert!(sign_in_method(o("proxy"), None, Some(vec![]), None).is_err());
        assert!(sign_in_method(o("proxy"), o("bad header"), None, None).is_err());
        assert!(sign_in_method(o("saml"), None, None, None).unwrap_err().contains("not a sign-in method"));
        // in medorama.toml
        let dir = tempfile::tempdir().unwrap();
        let c = Config::new(dir.path(), &dir.path().join("results"));
        let text = c.to_toml() + "sign_in = \"proxy\"\nproxy_addresses = [\"10.0.0.5\"]\nproxy_logout_url = \"/out\"\n";
        std::fs::write(dir.path().join(CONFIG_NAME), text).unwrap();
        let d = Config::load(dir.path()).unwrap();
        assert_eq!(
            d.sign_in,
            Method::Proxy {
                header: "X-Remote-User".into(),
                from: vec!["10.0.0.5".parse().unwrap()],
                logout_url: "/out".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn data_roots_are_compared_by_real_path() {
        let dir = tempfile::tempdir().unwrap();
        let base = real(dir.path());
        let (root, outside) = (base.join("data"), base.join("home"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("medorama.sqlite"), "x").unwrap();
        std::fs::write(root.join("a.dat"), "x").unwrap();
        std::os::unix::fs::symlink(outside.join("medorama.sqlite"), root.join("b.dat")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("away")).unwrap();
        let mut c = Config::new(&outside, &base.join("results"));
        c.data_roots = vec![root.to_string_lossy().to_string()];
        assert!(c.import_allowed(&root.join("a.dat")));
        assert!(!c.import_allowed(&root.join("b.dat")), "a link out of the root");
        assert!(!c.import_allowed(&root.join("away/medorama.sqlite")), "a folder linked out of the root");
        assert!(!c.import_allowed(&root.join("missing.dat")), "must exist");
        // a root given through a link is its real folder
        std::os::unix::fs::symlink(&root, base.join("link")).unwrap();
        c.data_roots = vec![base.join("link").to_string_lossy().to_string()];
        assert!(c.import_allowed(&root.join("a.dat")) && c.import_allowed(&base.join("link/a.dat")));
    }

    #[test]
    fn start_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Config::new(dir.path(), &dir.path().join("results"));
        assert!(c.start_warnings().is_empty());
        c.host = "0.0.0.0".into();
        assert_eq!(c.start_warnings().len(), 1);
        c.sign_in = sign_in_method(Some("proxy".into()), None, None, None).unwrap();
        assert_eq!(c.start_warnings().len(), 2);
        c.secure_cookies = true;
        assert!(c.start_warnings().is_empty());
    }

    #[test]
    fn data_roots_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let mut c = Config::new(dir.path(), &dir.path().join("results"));
        c.data_roots = vec![data.to_string_lossy().to_string()];
        std::fs::write(dir.path().join(CONFIG_NAME), c.to_toml()).unwrap();
        assert_eq!(Config::load(dir.path()).unwrap().data_roots, c.data_roots);
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::new(dir.path(), &dir.path().join("results"));
        std::fs::write(dir.path().join(CONFIG_NAME), c.to_toml()).unwrap();
        let d = Config::load(dir.path()).unwrap();
        assert_eq!(d.results_dir, dir.path().join("results"));
        assert_eq!(d.index_dir, dir.path().join("results").join("index"));
        assert_eq!(d.actions_dirs, vec![dir.path().join("actions")]);
        assert_eq!(d.port, DEFAULT_PORT);
        std::fs::write(dir.path().join(CONFIG_NAME), "results_dir = \"r\"\nbogus = 1\n").unwrap();
        assert!(Config::load(dir.path()).unwrap_err().contains("unknown settings: bogus"));
    }
}
