//! Settings: defaults, overridden by a TOML file, overridden by command-line options (the
//! same keys as the Python and C++ viewers').
//!
//! - The viewer's **root**, fixed when it is built (`VITALSTREAM_ROOT` at build time: install.sh
//!   sets it, to `~`, the home folder of whoever runs the viewer, unless `--root DIR` says
//!   otherwise; tools/install-medorama.sh to the whole file system, since Medorama applies its
//!   own rules; a build without it, such as a developer's `cargo build`, has the whole file
//!   system): it never opens anything outside it.
//! - `data_roots`: folders the viewer may open, inside its root.  Default: its root.  A root
//!   that contains the viewer's root is narrowed to it, and one outside it is left out (the
//!   settings narrow the root, never widen it).  A path is allowed when the file or folder it
//!   leads to, every symbolic link resolved, lies inside a data root.
//! - `max_records`, `max_depth`, `max_scan_s`: the limits on a folder of WFDB records
//!   (docs/wfdb-collection.md, "Limits").
//! - `require_password`: refuse to start without a password (Medorama sets it for the viewer
//!   it runs; a build without password support then never starts).
//! - `cache_dir`: the index cache, shared by the three viewers (they write the same index).
//!   Default, run from a clone of the repository: `<viewer>/sample_data/outputs/index`;
//!   elsewhere `~/.cache/vitalstream`.
//! - `adapters_dir`: the folder of adapters, programs that read formats the viewer does not
//!   read itself (docs/adapters.md). Default: `~/.local/share/medorama/adapters`.
//! - `auth_file`: the password (shared by the three viewers), default
//!   `~/.config/vitalstream/auth.json`.
//! - `allowed_hosts`: host names the server answers for besides `localhost`, `127.0.0.1`, `::1`
//!   and `host` (a reverse proxy's public name; `"*"`: any), docs/viewer-api.md, "Requests".
//! - `secure_cookie`: send the sign-in cookie only over HTTPS (behind an HTTPS proxy).
//!
//! The settings file: `--config FILE`, else the first that exists of `<viewer>/vitalstream.toml`
//! (only for a viewer run from the build folder of a clone of the repository) and
//! `~/.config/vitalstream/vitalstream.toml`.  A `vitalstream.toml` in the working
//! directory is never read on its own (a shared data folder could hold one).  Relative paths in
//! a settings file are relative to its folder.  The file is shared by the three viewers:
//! top-level keys apply to all of them, the keys of an `[rs]` section only to this one (`[py]`
//! and `[cpp]` are ignored).  A key of the wrong type is an error (`data_roots` must be a list
//! of strings, `port` an integer from 0 to 65535, ...).

use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Settings {
    pub timezone: String,
    pub data_roots: Vec<String>,
    pub cache_dir: PathBuf,
    pub host: String,
    pub port: u16,
    pub index_workers: usize,
    pub auth_file: PathBuf,
    pub adapters_dir: PathBuf,
    /// also show the irregularities a reader repaired (data blocks put back in order)
    pub qc_verbose: bool,
    /// the limits on a folder of WFDB records
    pub limits: Limits,
    /// refuse to start without a password
    pub require_password: bool,
    /// host names answered for besides the local ones and `host` (`*`: any)
    pub allowed_hosts: Vec<String>,
    /// the sign-in cookie only over HTTPS
    pub secure_cookie: bool,
    /// the settings file read, if any
    pub source: Option<PathBuf>,
    /// data roots left out because they lie outside the viewer's root
    pub outside: Vec<String>,
    real_roots: Vec<String>,
}

use recording_engine::sources::wfdb_folder::Limits;

/// The root fixed when this viewer was built (`VITALSTREAM_ROOT`; `~` is the home folder of
/// whoever runs it), else the whole file system; absolute.
pub fn built_root() -> String {
    match option_env!("VITALSTREAM_ROOT").filter(|r| !r.is_empty()) {
        Some(r) => absolute(r),
        None => fs_root(),
    }
}

/// Whether this viewer was built without a root: the whole file system, every drive on Windows
/// (where [`built_root`] is only the home folder's drive, the folder it shows first).
fn whole_file_system() -> bool {
    option_env!("VITALSTREAM_ROOT").filter(|r| !r.is_empty()).is_none()
}

pub use recording_engine::paths::{absolute, expand, fs_root, home, is_sep, plain, SEP};

fn realpath(p: &str) -> String {
    std::fs::canonicalize(p).map(|x| plain(&x.to_string_lossy())).unwrap_or_else(|_| p.to_string())
}

fn within(path: &str, root: &str) -> bool {
    let r = root.trim_end_matches(is_sep);
    path == root || r.is_empty() || (path.starts_with(r) && path[r.len()..].starts_with(is_sep))
}

/// The viewer's folder of the clone this program was built from (`viewers/vitalstream`), when
/// it runs from inside that clone (its build folder); never from the working directory, which
/// may be anyone's folder.
pub fn repo_root() -> Option<PathBuf> {
    let viewer = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("..")).ok()?;
    let clone = viewer.parent()?.parent()?;
    let exe = std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok())?;
    let ok = exe.starts_with(clone)
        && viewer.join("rust").join("Cargo.toml").is_file()
        && viewer.join("web").is_dir()
        && viewer.join("sample_data").is_dir();
    ok.then_some(viewer)
}

impl Settings {
    pub fn defaults() -> Settings {
        let cache = match repo_root() {
            Some(r) => r.join("sample_data").join("outputs").join("index"),
            None => PathBuf::from(expand("~/.cache/vitalstream")),
        };
        Settings {
            timezone: "America/New_York".into(),
            data_roots: vec![built_root()],
            cache_dir: cache,
            host: "127.0.0.1".into(),
            port: 8060,
            index_workers: 2,
            auth_file: PathBuf::from(expand("~/.config/vitalstream/auth.json")),
            adapters_dir: recording_engine::adapters::default_folder(),
            qc_verbose: false,
            limits: Limits::default(),
            require_password: false,
            allowed_hosts: vec![],
            secure_cookie: false,
            source: None,
            outside: vec![],
            real_roots: vec![],
        }
    }

    /// Finish: absolute roots inside the viewer's root, validated time zone.
    pub fn finish(mut self) -> Result<Settings, String> {
        let top = built_root();
        let real_top = realpath(&top);
        let mut roots: Vec<String> = vec![];
        for r in self.data_roots.iter().map(|r| absolute(r)) {
            let real = realpath(&r);
            let kept = if within(&real, &real_top) || (whole_file_system() && !within(&real_top, &real)) {
                r
            } else if within(&real_top, &real) {
                top.clone()
            } else {
                self.outside.push(r);
                continue;
            };
            if !roots.contains(&kept) {
                roots.push(kept);
            }
        }
        if roots.is_empty() {
            return Err(format!("no data root inside this viewer's root, {top} (set when it was built)"));
        }
        self.data_roots = roots;
        self.real_roots = self.data_roots.iter().map(|r| realpath(r)).collect();
        chrono_tz::Tz::from_str(&self.timezone).map_err(|_| format!("unknown time zone {:?}", self.timezone))?;
        if self.index_workers == 0 {
            self.index_workers = 1;
        }
        Ok(self)
    }

    /// True if the file or folder `path` leads to (every symbolic link resolved) lies inside a
    /// data root.
    pub fn allowed(&self, path: &str) -> bool {
        let r = realpath(&absolute(path));
        self.real_roots.iter().any(|root| within(&r, root))
    }
}

fn relative_to(base: &Path, p: &str) -> String {
    let e = PathBuf::from(expand(p));
    if e.is_absolute() {
        e.to_string_lossy().to_string()
    } else {
        base.join(e).to_string_lossy().to_string()
    }
}

/// Settings from `path` (or the first default file that exists), with overrides applied by
/// the caller afterwards.
pub fn load_settings(path: Option<&str>) -> Result<Settings, String> {
    let mut s = Settings::defaults();
    let mut candidates: Vec<PathBuf> = vec![];
    match path {
        Some(p) => candidates.push(PathBuf::from(expand(p))),
        None => {
            if let Some(r) = repo_root() {
                candidates.push(r.join("vitalstream.toml"));
            }
            candidates.push(PathBuf::from(expand("~/.config/vitalstream/vitalstream.toml")));
        }
    }
    for c in candidates {
        if !c.is_file() {
            if path.is_some() {
                return Err(format!("{}: no such settings file", c.display()));
            }
            continue;
        }
        let text = std::fs::read_to_string(&c).map_err(|e| format!("{}: {e}", c.display()))?;
        let mut table: toml::Table = text.parse().map_err(|e| format!("{}: {e}", c.display()))?;
        // this viewer's section overrides the shared keys; the other viewers' sections are ignored
        let own = table.remove("rs");
        table.remove("py");
        table.remove("cpp");
        if let Some(own) = own {
            let own = own.as_table().ok_or_else(|| format!("{}: [rs] must be a table", c.display()))?.clone();
            table.extend(own);
        }
        let base = std::fs::canonicalize(&c).ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
        let mut unknown = vec![];
        for (k, v) in &table {
            let bad = || format!("{}: bad value for {k}", c.display());
            match k.as_str() {
                "timezone" => s.timezone = v.as_str().ok_or_else(bad)?.to_string(),
                "data_roots" => {
                    s.data_roots = v
                        .as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(|x| x.as_str().map(|p| relative_to(&base, p)).ok_or_else(bad))
                        .collect::<Result<_, _>>()?
                }
                "cache_dir" => s.cache_dir = PathBuf::from(relative_to(&base, v.as_str().ok_or_else(bad)?)),
                "auth_file" => s.auth_file = PathBuf::from(relative_to(&base, v.as_str().ok_or_else(bad)?)),
                "adapters_dir" => s.adapters_dir = PathBuf::from(relative_to(&base, v.as_str().ok_or_else(bad)?)),
                "host" => s.host = v.as_str().ok_or_else(bad)?.to_string(),
                "port" => s.port = v.as_integer().and_then(|p| u16::try_from(p).ok()).ok_or_else(bad)?,
                "index_workers" => {
                    s.index_workers = v.as_integer().and_then(|p| usize::try_from(p).ok()).ok_or_else(bad)?
                }
                "qc_verbose" => s.qc_verbose = v.as_bool().ok_or_else(bad)?,
                "require_password" => s.require_password = v.as_bool().ok_or_else(bad)?,
                "secure_cookie" => s.secure_cookie = v.as_bool().ok_or_else(bad)?,
                "allowed_hosts" => {
                    s.allowed_hosts = v
                        .as_array()
                        .ok_or_else(bad)?
                        .iter()
                        .map(|x| x.as_str().filter(|h| !h.trim().is_empty()).map(str::to_string).ok_or_else(bad))
                        .collect::<Result<_, _>>()?
                }
                "max_records" => {
                    s.limits.max_records = v.as_integer().and_then(|p| usize::try_from(p).ok()).ok_or_else(bad)?
                }
                "max_depth" => {
                    s.limits.max_depth = v.as_integer().and_then(|p| usize::try_from(p).ok()).ok_or_else(bad)?
                }
                "max_scan_s" => {
                    s.limits.max_scan_s = v
                        .as_float()
                        .or_else(|| v.as_integer().map(|i| i as f64))
                        .filter(|x| *x >= 0.0)
                        .ok_or_else(bad)?
                }
                _ => unknown.push(k.clone()),
            }
        }
        if !unknown.is_empty() {
            unknown.sort();
            return Err(format!("unknown settings: {}", unknown.join(", ")));
        }
        s.source = Some(std::fs::canonicalize(&c).unwrap_or(c));
        break;
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_and_links() {
        let dir = tempfile::tempdir_in(home()).unwrap();
        let root = dir.path().join("data");
        std::fs::create_dir_all(root.join("a")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let mut s = Settings::defaults();
        s.data_roots = vec![root.to_string_lossy().to_string()];
        let s = s.finish().unwrap();
        assert!(s.allowed(&root.join("a").to_string_lossy()));
        assert!(!s.allowed(&outside.to_string_lossy()));
        // a link inside a root that leads outside it: refused
        #[cfg(unix)]
        assert!(!s.allowed(&root.join("link").to_string_lossy()));
        assert!(!s.allowed(&root.join("..").join("outside").to_string_lossy()));
        assert_eq!(absolute("/a/b/../c/./d"), "/a/c/d");
    }

    #[test]
    fn settings_files() {
        // a vitalstream.toml in the working directory is not read (it could run any program
        // through adapters_dir, or open the viewer to the network)
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("vitalstream.toml");
        std::fs::write(&toml, "adapters_dir = \"ad\"\nhost = \"0.0.0.0\"\n").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let s = load_settings(None);
        std::env::set_current_dir(cwd).unwrap();
        let s = s.unwrap();
        assert_ne!(s.source.as_deref(), Some(std::fs::canonicalize(&toml).unwrap().as_path()));
        assert_ne!(s.adapters_dir, dir.path().join("ad"));
        // named with --config, it is read; wrong types are refused
        let s = load_settings(Some(&toml.to_string_lossy())).unwrap();
        assert_eq!(
            (s.adapters_dir.clone(), s.host.as_str()),
            (std::fs::canonicalize(dir.path()).unwrap().join("ad"), "0.0.0.0")
        );
        for bad in [
            "data_roots = \"/\"",
            "port = \"8060\"",
            "port = 70000",
            "index_workers = -1",
            "allowed_hosts = \"x\"",
            "qc_verbose = 1",
        ] {
            std::fs::write(&toml, bad).unwrap();
            assert!(load_settings(Some(&toml.to_string_lossy())).unwrap_err().contains("bad value"), "{bad}");
        }
    }

    #[test]
    fn the_viewers_root() {
        // the default data root is the viewer's root; a wider one is narrowed to it, one
        // outside it is left out
        let top = built_root();
        let s = Settings::defaults().finish().unwrap();
        assert_eq!(s.data_roots, std::slice::from_ref(&top));
        assert!(s.allowed(&top));
        let mut s = Settings::defaults();
        s.data_roots = vec![fs_root()];
        let s = s.finish().unwrap();
        assert_eq!(s.data_roots, std::slice::from_ref(&top));
        if top != fs_root() {
            let outside = std::env::temp_dir().join("vitalstream-outside").to_string_lossy().to_string();
            let real_top = realpath(&top);
            if !within(&realpath(&std::env::temp_dir().to_string_lossy()), &real_top) {
                let mut s = Settings::defaults();
                s.data_roots = vec![outside.clone(), top.clone()];
                let s = s.finish().unwrap();
                assert_eq!((s.data_roots.clone(), s.outside.clone()), (vec![top.clone()], vec![absolute(&outside)]));
                assert!(!s.allowed(&outside));
            }
        }
    }
}
