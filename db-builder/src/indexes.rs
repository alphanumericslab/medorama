//! Ready-made indexes (`db-builder indexes`): an index folder for each recording a database
//! lists, made ahead of time, and the table that names them (`recording_indexes`), which Medorama
//! reads when it opens or indexes a recording: it takes the index instead of indexing the
//! recording, as long as the file still has the size and modification time the index was made
//! from and the program that made it is the one that reads it there (docs/databases.md, "Indexes
//! and signals a recording database holds").
//!
//! Only recordings that are one file are indexed ahead (an EDF or BDF file, a file an installed
//! adapter reads): a file's size and time say whether it changed.  A WFDB record (a header and
//! its signal files) or folder is left to Medorama, which indexes it when it is opened.  Run
//! again, a recording whose index is current is kept, and only new or changed ones are indexed.

use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The table Medorama reads (its columns are its `indexes` view's).
pub const CREATE: &str = "CREATE TABLE IF NOT EXISTS recording_indexes (
    recording_key TEXT PRIMARY KEY,
    index_dir     TEXT NOT NULL,
    size          INTEGER NOT NULL,
    mtime_ns      INTEGER NOT NULL,
    md5           TEXT NOT NULL DEFAULT '',
    adapter       TEXT NOT NULL DEFAULT '',
    indexed_at    TEXT NOT NULL DEFAULT ''
)";

/// What a run did.
#[derive(Default, Debug)]
pub struct Report {
    pub made: usize,
    pub kept: usize,
    /// (the recording's file, why it was not indexed ahead)
    pub skipped: Vec<(String, String)>,
}

/// A file's size and modification time (integer nanoseconds since 1970, UTC).
pub fn stamp(p: &Path) -> Option<(i64, i64)> {
    let m = std::fs::metadata(p).ok()?;
    let ns = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as i64;
    Some((m.len() as i64, ns))
}

/// Make the index of every recording of `c` (of these categories; all when none) that is one
/// file, into `dir` (one folder per recording), and name them in `recording_indexes`.  `db` is
/// the database's file: an index folder inside its folder is named relative to it.
pub fn make(c: &mut Connection, db: &Path, dir: &Path, categories: &[String]) -> anyhow::Result<Report> {
    let base = crate::db::absolute(db).parent().map(Path::to_path_buf).unwrap_or_default();
    let dir = crate::db::absolute(dir);
    std::fs::create_dir_all(&dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
    c.execute_batch(CREATE)?;
    let rows: Vec<(String, String, String)> = c
        .prepare("SELECT recording_key, file, category FROM recordings ORDER BY recording_key")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default())))?
        .collect::<Result<_, _>>()?;
    let mut rep = Report::default();
    for (key, file, category) in rows {
        if !categories.is_empty() && !categories.iter().any(|x| x.eq_ignore_ascii_case(&category)) {
            continue;
        }
        let path = if Path::new(&file).is_absolute() { PathBuf::from(&file) } else { base.join(&file) };
        let skip = |rep: &mut Report, why: &str| rep.skipped.push((file.clone(), why.to_string()));
        if !path.is_file() {
            skip(
                &mut rep,
                if path.exists() { "a folder: Medorama indexes it when it is opened" } else { "no such file" },
            );
            continue;
        }
        let (reader, made_by) = match recording_engine::sources::reader_of(&path) {
            Ok(x) => x,
            Err(e) => {
                skip(&mut rep, &e.to_string());
                continue;
            }
        };
        if reader.iter().any(|(k, v)| *k == "kind" && v == "wfdb") {
            skip(&mut rep, "a WFDB record (its header and signal files): Medorama indexes it when it is opened");
            continue;
        }
        let Some((size, mtime_ns)) = stamp(&path) else {
            skip(&mut rep, "its size and time cannot be read");
            continue;
        };
        // its folder: named by its key's digest (any key is a valid name)
        let name = crate::db::hex(&Sha256::digest(key.as_bytes()))[..20].to_string();
        let folder = dir.join(&name);
        let current: Option<(i64, i64, String)> = c
            .query_row("SELECT size, mtime_ns, adapter FROM recording_indexes WHERE recording_key = ?1", [&key], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .ok();
        if current == Some((size, mtime_ns, made_by.clone())) && folder.join("meta.json").is_file() {
            rep.kept += 1;
            continue;
        }
        // built beside, then moved in: a folder half written is never named
        let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let built = recording_engine::index::build_index(&path, &tmp, &|_| {});
        let made = match built {
            Ok(k) => tmp.join(k),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                skip(&mut rep, &format!("not indexed: {e}"));
                continue;
            }
        };
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::rename(&made, &folder).map_err(|e| anyhow::anyhow!("{}: {e}", folder.display()))?;
        let _ = std::fs::remove_dir_all(&tmp);
        let shown = match folder.strip_prefix(&base) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => folder.to_string_lossy().to_string(),
        };
        c.execute(
            "INSERT INTO recording_indexes (recording_key, index_dir, size, mtime_ns, adapter, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (recording_key) DO UPDATE SET index_dir = excluded.index_dir, size = excluded.size,
                 mtime_ns = excluded.mtime_ns, adapter = excluded.adapter, indexed_at = excluded.indexed_at",
            rusqlite::params![key, shown, size, mtime_ns, made_by, crate::db::now()],
        )?;
        rep.made += 1;
    }
    Ok(rep)
}
