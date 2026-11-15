//! The root folder of `db-builder serve --root DIR` (README.md, "Web interface"): the one folder
//! the web interface may browse, read and write in.  Every path the page sends goes through
//! [`Root::existing`] (a file or folder to read) or [`Root::new_file`] (a file to make), which
//! refuse whatever lies outside the root: a `..`, an absolute path elsewhere, a symbolic link
//! (or a folder of links) leading out.  Paths are compared by their real paths, links resolved.

use std::path::{Component, Path, PathBuf};

/// The root folder, by its real path.
#[derive(Debug, Clone)]
pub struct Root {
    real: PathBuf,
}

/// Why a path was refused (said to the page as it is).
pub const OUTSIDE: &str = "outside the root folder";

/// A path's real path (links resolved, Windows' `\\?\` prefix removed), if it exists.
fn real(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok().map(|r| PathBuf::from(recording_engine::paths::plain(&r.to_string_lossy())))
}

impl Root {
    /// The root: an existing folder.
    pub fn new(dir: &Path) -> anyhow::Result<Root> {
        let real = real(dir).ok_or_else(|| anyhow::anyhow!("{}: no such folder", dir.display()))?;
        if !real.is_dir() {
            anyhow::bail!("{}: not a folder", dir.display());
        }
        Ok(Root { real })
    }

    /// The root's real path.
    pub fn path(&self) -> &Path {
        &self.real
    }

    /// Whether a real path is the root or lies in it.
    fn holds(&self, real: &Path) -> bool {
        real.starts_with(&self.real)
    }

    /// A path as given (relative to the root, or absolute), before anything is resolved: no
    /// `..`, no NUL, nothing but text.
    fn given(&self, given: &str) -> Result<PathBuf, String> {
        let g = given.trim();
        if g.contains('\0') {
            return Err("not a path".into());
        }
        let p = Path::new(g);
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!("{g}: `..` is not allowed: {OUTSIDE}"));
        }
        Ok(if p.is_absolute() { p.to_path_buf() } else { self.real.join(p) })
    }

    /// An existing file or folder (empty: the root itself), by its real path, refused when that
    /// lies outside the root.
    pub fn existing(&self, given: &str) -> Result<PathBuf, String> {
        let p = self.given(given)?;
        let Some(r) = real(&p) else {
            // a link whose target is missing is refused as a link, not as a missing file
            if p.symlink_metadata().is_ok() {
                return Err(format!("{}: a link to nothing", self.shown(&p)));
            }
            return Err(format!("{}: no such file or folder", given.trim()));
        };
        if !self.holds(&r) {
            return Err(format!("{}: {OUTSIDE}", given.trim()));
        }
        Ok(r)
    }

    /// A file to make (a database, a copy, a rules file): its folder exists in the root, its
    /// name is a plain name, and it is not a symbolic link (one could lead out, or to nothing,
    /// which writing would make outside).  It may exist already (a rules file rewritten).
    pub fn new_file(&self, given: &str) -> Result<PathBuf, String> {
        let p = self.given(given)?;
        let name = match p.components().next_back() {
            Some(Component::Normal(n)) => n.to_os_string(),
            _ => return Err(format!("{}: not a file name", given.trim())),
        };
        let dir = p.parent().ok_or_else(|| format!("{}: not a file name", given.trim()))?;
        let dir = self.existing(&dir.to_string_lossy())?;
        if !dir.is_dir() {
            return Err(format!("{}: not a folder", self.shown(&dir)));
        }
        let file = dir.join(name);
        if let Ok(m) = file.symlink_metadata() {
            if m.file_type().is_symlink() {
                return Err(format!("{}: a symbolic link: not written through", self.shown(&file)));
            }
            if m.is_dir() {
                return Err(format!("{}: a folder", self.shown(&file)));
            }
        }
        Ok(file)
    }

    /// A path inside the root as the page shows it: relative to the root, `/` between folders
    /// (the root itself: empty).
    pub fn shown(&self, p: &Path) -> String {
        match p.strip_prefix(&self.real) {
            Ok(rel) => {
                rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/")
            }
            Err(_) => p.to_string_lossy().to_string(),
        }
    }

    /// The entries of a folder in the root: hidden ones left out, links listed only when they
    /// lead inside the root (by their real path, as a folder or a file).  Sorted, folders first.
    pub fn list(&self, dir: &Path, max: usize) -> Result<(Vec<Entry>, bool), String> {
        let it = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", self.shown(dir)))?;
        let mut out = vec![];
        let mut truncated = false;
        for e in it.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let Some(r) = real(&path) else { continue };
            if !self.holds(&r) {
                continue;
            }
            if out.len() >= max {
                truncated = true;
                break;
            }
            let m = std::fs::metadata(&r).ok();
            out.push(Entry {
                path: self.shown(&dir.join(&name)),
                dir: r.is_dir(),
                size: m.as_ref().filter(|m| m.is_file()).map_or(0, |m| m.len()),
                name,
            });
        }
        out.sort_by_key(|e| (!e.dir, e.name.to_lowercase()));
        Ok((out, truncated))
    }

    /// The SQLite files (`.sqlite`, `.sqlite3`) in the root and its folders, at most `depth`
    /// folders down and `max` of them; hidden entries skipped, links not followed.
    pub fn databases(&self, depth: usize, max: usize) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![(self.real.clone(), 0usize)];
        while let Some((d, level)) = stack.pop() {
            let Ok(it) = std::fs::read_dir(&d) else { continue };
            let mut entries: Vec<_> = it.filter_map(Result::ok).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = e.file_name().to_string_lossy().to_string();
                let Ok(t) = e.file_type() else { continue };
                if name.starts_with('.') || t.is_symlink() {
                    continue;
                }
                let p = e.path();
                if t.is_dir() && level < depth {
                    stack.push((p, level + 1));
                } else if t.is_file()
                    && p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("sqlite") || x.eq_ignore_ascii_case("sqlite3"))
                {
                    out.push(p);
                    if out.len() >= max {
                        return out;
                    }
                }
            }
        }
        out.sort();
        out
    }
}

/// An entry of a folder, as the file chooser shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Entry {
    pub name: String,
    /// Its path relative to the root.
    pub path: String,
    pub dir: bool,
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx() -> (tempfile::TempDir, Root, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let base = real(t.path()).unwrap();
        let root = base.join("root");
        std::fs::create_dir_all(root.join("data/sub")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::write(root.join("data/a.csv"), "x").unwrap();
        std::fs::write(base.join("outside/secret.csv"), "x").unwrap();
        std::fs::write(base.join("root-sibling.csv"), "x").unwrap();
        (t, Root::new(&root).unwrap(), base)
    }

    #[test]
    fn inside_is_allowed() {
        let (_t, r, base) = fx();
        assert_eq!(r.existing("").unwrap(), r.path());
        assert_eq!(r.existing("data/a.csv").unwrap(), r.path().join("data/a.csv"));
        assert_eq!(r.existing("./data/a.csv").unwrap(), r.path().join("data/a.csv"));
        let abs = base.join("root/data/a.csv");
        assert_eq!(r.existing(&abs.to_string_lossy()).unwrap(), abs);
        assert_eq!(r.new_file("data/new.sqlite").unwrap(), r.path().join("data/new.sqlite"));
        assert_eq!(r.new_file("new.sqlite").unwrap(), r.path().join("new.sqlite"));
        assert_eq!(r.shown(&r.path().join("data/a.csv")), "data/a.csv");
    }

    #[test]
    fn outside_is_refused() {
        let (_t, r, base) = fx();
        for p in ["..", "../outside/secret.csv", "data/../../outside/secret.csv", "data/../a.csv", "/", "/etc/passwd"] {
            assert!(r.existing(p).is_err(), "{p}");
        }
        assert!(r.existing(&base.join("outside/secret.csv").to_string_lossy()).is_err());
        // a sibling whose name starts with the root's
        assert!(r.existing(&base.join("root-sibling.csv").to_string_lossy()).is_err());
        assert!(r.existing("data/missing.csv").unwrap_err().contains("no such"));
        for p in ["../x.sqlite", "/tmp/x.sqlite", "missing/x.sqlite", "data/..", "", "data/sub"] {
            assert!(r.new_file(p).is_err(), "{p}");
        }
        assert!(r.new_file(&base.join("outside/x.sqlite").to_string_lossy()).is_err());
        assert!(r.existing("a\0b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn links_out_are_refused() {
        use std::os::unix::fs::symlink;
        let (_t, r, base) = fx();
        symlink(base.join("outside/secret.csv"), r.path().join("data/link.csv")).unwrap();
        symlink(base.join("outside"), r.path().join("away")).unwrap();
        symlink(r.path().join("data/a.csv"), r.path().join("inner.csv")).unwrap();
        symlink(base.join("outside/none.sqlite"), r.path().join("dangling.sqlite")).unwrap();
        assert!(r.existing("data/link.csv").unwrap_err().contains(OUTSIDE));
        assert!(r.existing("away").is_err());
        assert!(r.existing("away/secret.csv").is_err());
        assert!(r.new_file("away/x.sqlite").is_err(), "a new file in a folder linked out");
        assert!(r.new_file("dangling.sqlite").is_err(), "writing through a link to nothing");
        assert!(r.new_file("inner.csv").is_err(), "writing through a link");
        assert!(r.existing("dangling.sqlite").is_err());
        // a link inside the root, to a file inside it: read as that file
        assert_eq!(r.existing("inner.csv").unwrap(), r.path().join("data/a.csv"));
        // listed: the link inside; not the links out
        let (items, _) = r.list(r.path(), 100).unwrap();
        let names: Vec<&str> = items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["data", "inner.csv"]);
        let (items, _) = r.list(&r.path().join("data"), 100).unwrap();
        let names: Vec<&str> = items.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sub", "a.csv"]);
        // the root given through a link is its real folder
        symlink(r.path(), base.join("root-link")).unwrap();
        let r2 = Root::new(&base.join("root-link")).unwrap();
        assert_eq!(r2.path(), r.path());
        assert!(r2.existing(&base.join("root-link/data/a.csv").to_string_lossy()).is_ok());
    }

    #[test]
    fn databases_are_found() {
        let (_t, r, _) = fx();
        std::fs::write(r.path().join("a.sqlite"), "").unwrap();
        std::fs::write(r.path().join("data/sub/b.SQLITE"), "").unwrap();
        std::fs::write(r.path().join("data/.hidden.sqlite"), "").unwrap();
        let found: Vec<String> = r.databases(4, 100).iter().map(|p| r.shown(p)).collect();
        assert_eq!(found, ["a.sqlite", "data/sub/b.SQLITE"]);
        assert_eq!(r.databases(0, 100).len(), 1);
    }
}
