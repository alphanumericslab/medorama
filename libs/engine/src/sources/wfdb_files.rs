//! WFDB records as files, without reading them (no wfdb-x): which paths are records and WFDB
//! folders, the segment table of a multi-segment record, the format labels, the limits on a
//! folder of records, and a folder's files for the cache key.  Shared by the reader
//! ([`super::wfdb_folder`], with the `wfdb` feature) and by a build without it (`wfdb_none.rs`),
//! which re-export it: a build without WFDB support still recognises a WFDB record, and refuses
//! it ([`super::NO_WFDB`]) rather than taking it for something else.

use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// The most files a folder's key is computed from (`fingerprint`).
pub const MAX_FINGERPRINT_FILES: usize = 200_000;

/// What a folder of plain records may hold before it is read: records (header files, the
/// segments of multi-segment records included), folder levels below it, and seconds for the
/// scan that counts them (docs/wfdb-collection.md, "Limits").  A collection (`index.csv`)
/// lists its records itself and is not limited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub max_records: usize,
    pub max_depth: usize,
    pub max_scan_s: f64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_records: 5000, max_depth: 20, max_scan_s: 10.0 }
    }
}

static LIMITS: RwLock<Option<Limits>> = RwLock::new(None);

/// Use `limits` for every folder read from now on (the programs' settings).
pub fn set_limits(limits: Limits) {
    *LIMITS.write().unwrap() = Some(limits);
}

/// The limits in force.
pub fn limits() -> Limits {
    LIMITS.read().unwrap().unwrap_or_default()
}

/// A folder written by the converters (index.csv), or holding WFDB records itself (RECORDS, or
/// a .hea file among its first `scan_limit` entries).  Any folder can be opened as records
/// (the reader reads its subfolders too); this tells which folders are recordings when a tree
/// is searched for them.
pub fn is_wfdb_folder(p: &Path, scan_limit: usize) -> bool {
    if !p.is_dir() {
        return false;
    }
    if p.join("index.csv").is_file() || p.join("RECORDS").is_file() {
        return true;
    }
    std::fs::read_dir(p)
        .map(|it| it.take(scan_limit).filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().ends_with(".hea")))
        .unwrap_or(false)
}

/// A WFDB header file (`.hea`): one record.
pub fn is_record(p: &Path) -> bool {
    p.is_file() && p.extension().is_some_and(|e| e == "hea")
}

/// Rows of a CSV file (Python's csv module, excel dialect) as dicts of strings.
pub fn read_csv(path: &Path) -> Vec<IndexMap<String, String>> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let mut rows: Vec<Vec<String>> = Vec::new();
    let (mut field, mut row, mut quoted, mut any) = (String::new(), Vec::new(), false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                any = true;
            }
            ',' => {
                row.push(std::mem::take(&mut field));
                any = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            c => {
                field.push(c);
                any = true;
            }
        }
    }
    if any || !field.is_empty() {
        row.push(field);
        rows.push(row);
    }
    let Some(head) = rows.first().cloned() else { return vec![] };
    rows.into_iter()
        .skip(1)
        .filter(|r| !(r.len() == 1 && r[0].is_empty()))
        .map(|r| head.iter().cloned().zip(r.into_iter().chain(std::iter::repeat(String::new()))).collect())
        .collect()
}

/// The lines of a header file that are neither blank nor comments, trimmed.
pub(crate) fn header_lines(hea: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(hea) else { return vec![] };
    String::from_utf8_lossy(&bytes)
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// A record's file with an extension added (`a.v1` → `a.v1.hea`): a record name may hold a dot,
/// which `Path::with_extension` would take for the start of an extension.
pub(crate) fn with_ext(rec: &Path, ext: &str) -> PathBuf {
    let mut s = rec.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// The segments of a multi-segment record, `(name, samples)` in order (`~` names a null
/// segment); None for a single-segment record.  `record`: the record's path without `.hea`.
pub fn segment_table(record: &Path) -> Option<Vec<(String, i64)>> {
    let lines = header_lines(&with_ext(record, "hea"));
    let first = lines.first()?;
    let (_, nseg) = first.split_whitespace().next()?.split_once('/')?;
    let nseg: usize = nseg.parse().ok()?;
    Some(
        lines[1..]
            .iter()
            .take(nseg)
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                Some((it.next()?.to_string(), it.next()?.parse().ok()?))
            })
            .collect(),
    )
}

/// The format label of what `path` opens as.
pub fn format_label(path: &Path) -> &'static str {
    if is_record(path) {
        if segment_table(&path.with_extension("")).is_some() {
            "WFDB multi-segment record"
        } else {
            "WFDB record"
        }
    } else if path.join("index.csv").is_file() {
        "WFDB collection"
    } else {
        "WFDB records"
    }
}

/// `[name, size, mtime ns]` of each file, for the cache key: every file below a folder
/// (hidden entries and links to folders skipped; a link to a file counts with its target's
/// size and time), or the files of one record (`record_files(base, name)`); `name` relative to the
/// folder (the record's folder), with `/` between components on every system
/// (viewers/vitalstream/docs/index-format.md).  A folder's walk visits names in order, enters
/// at most `max_depth` levels of subfolders ([`limits`]) and stops after
/// [`MAX_FINGERPRINT_FILES`] files.
pub(crate) fn fingerprint(path: &Path, record_files: fn(&Path, &str) -> Vec<String>) -> Vec<(String, u64, i128)> {
    fn stat(p: &Path) -> Option<(u64, i128)> {
        let m = p.metadata().ok()?;
        let mt = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i128);
        Some((m.len(), mt))
    }
    let mut out = vec![];
    if is_record(path) {
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        for f in record_files(&base, &name) {
            if let Some((size, mt)) = stat(&base.join(&f)) {
                out.push((f, size, mt));
            }
        }
        return out;
    }
    fn walk(dir: &Path, rel: &str, depth: usize, max_depth: usize, out: &mut Vec<(String, u64, i128)>) {
        let Ok(it) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, PathBuf, std::fs::FileType)> = it
            .filter_map(Result::ok)
            .filter_map(|e| Some((e.file_name().to_string_lossy().to_string(), e.path(), e.file_type().ok()?)))
            .filter(|(n, _, _)| !n.starts_with('.'))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, full, t) in entries {
            if out.len() >= MAX_FINGERPRINT_FILES {
                return;
            }
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if t.is_dir() {
                if depth < max_depth {
                    walk(&full, &r, depth + 1, max_depth, out);
                }
            } else if t.is_symlink() && !full.is_file() {
                // a link to a folder, or a broken link
            } else if let Some((size, mt)) = stat(&full) {
                out.push((r, size, mt));
            }
        }
    }
    walk(path, "", 0, limits().max_depth, &mut out);
    out.sort();
    out
}
