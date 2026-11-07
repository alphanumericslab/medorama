//! Paths as the user writes them: `~`, relative paths, the root of the file system, Windows'
//! canonical paths.  Shared by the engine (index keys), the viewers' settings and the catalog.

use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;

/// The path separator of this system (`/`, or `\` on Windows).
pub const SEP: &str = std::path::MAIN_SEPARATOR_STR;

/// The home folder (on Windows `USERPROFILE`, which Git Bash's `HOME` may not match).
pub fn home() -> String {
    let (first, second) = if cfg!(windows) { ("USERPROFILE", "HOME") } else { ("HOME", "USERPROFILE") };
    std::env::var(first).or_else(|_| std::env::var(second)).unwrap_or_else(|_| fs_root())
}

/// The root of the file system: `/`, or on Windows the root of the home folder's drive.
pub fn fs_root() -> String {
    if cfg!(windows) {
        let h = std::env::var("USERPROFILE").unwrap_or_default();
        let root: PathBuf =
            Path::new(&h).components().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir)).collect();
        let r = root.to_string_lossy().to_string();
        if r.is_empty() {
            "C:\\".into()
        } else {
            r
        }
    } else {
        "/".into()
    }
}

/// True for `/`, and on Windows also `\`.
pub fn is_sep(c: char) -> bool {
    c == '/' || c == std::path::MAIN_SEPARATOR
}

/// `~` expanded.
pub fn expand(p: &str) -> String {
    if p == "~" {
        return home();
    }
    if p.starts_with('~') && p[1..].starts_with(is_sep) {
        return format!("{}{SEP}{}", home().trim_end_matches(is_sep), &p[2..]);
    }
    p.to_string()
}

/// Absolute, normalized path with `~` expanded; symbolic links are NOT resolved.
pub fn absolute(p: &str) -> String {
    let p = PathBuf::from(expand(p));
    let p = if p.is_absolute() { p } else { std::env::current_dir().unwrap_or_default().join(p) };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    let s = out.to_string_lossy().to_string();
    if s.is_empty() {
        fs_root()
    } else {
        s
    }
}

/// Windows' canonical paths without the `\\?\` prefix (`\\?\C:\x` -> `C:\x`,
/// `\\?\UNC\srv\share` -> `\\srv\share`); other paths unchanged.
pub fn plain(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

/// The real paths of the folders the readers may open files in (None: anywhere).
static ALLOWED: RwLock<Option<Vec<String>>> = RwLock::new(None);

/// The real path (symbolic links resolved, Windows' prefix removed), or None when it does not
/// exist.
fn real(p: &Path) -> Option<String> {
    std::fs::canonicalize(p).ok().map(|x| plain(&x.to_string_lossy()))
}

fn within(path: &str, root: &str) -> bool {
    let r = root.trim_end_matches(is_sep);
    path == root || r.is_empty() || (path.starts_with(r) && path[r.len()..].starts_with(is_sep))
}

/// Restrict every file the readers open (a header, the signal files it names, a segment, a
/// `.mrk` or CSV file) to these folders, symbolic links resolved: a program's data roots
/// (viewers/vitalstream/docs/index-format.md, "Files outside the data roots").  None (the
/// default) allows any file.
pub fn set_allowed_roots(roots: Option<Vec<String>>) {
    let real_roots =
        roots.map(|r| r.iter().map(|x| real(Path::new(&absolute(x))).unwrap_or_else(|| absolute(x))).collect());
    *ALLOWED.write().unwrap_or_else(|e| e.into_inner()) = real_roots;
}

/// Whether a reader may open the file at `p`: its real path lies inside an allowed root (see
/// [`set_allowed_roots`]).  A file that does not exist is allowed: nothing can be read from it.
pub fn allowed_file(p: &Path) -> bool {
    let roots = ALLOWED.read().unwrap_or_else(|e| e.into_inner());
    let Some(roots) = roots.as_ref() else { return true };
    match real(p) {
        Some(r) => roots.iter().any(|root| within(&r, root)),
        None => p.symlink_metadata().is_err(),
    }
}
