//! The database: its schema (version 1, docs/schema.md), opening it (encrypted with SQLCipher
//! when asked), its `meta` and its `sources`.  The schema is a contract: Medorama and any other
//! reader read a db-builder database by these tables' and columns' names.

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The schema's version (`meta.schema`): a change that a reader must know of is a new version.
pub const SCHEMA: i64 = 1;

/// The environment variable that holds the key of an encrypted database (64 hex digits).
pub const KEY_VAR: &str = "DB_BUILDER_KEY";

/// The default time zone of local times in the inputs (as Medorama's).
pub const DEFAULT_TIMEZONE: &str = "America/New_York";

const CREATE: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file TEXT NOT NULL,
    kind TEXT NOT NULL,
    rows INTEGER NOT NULL DEFAULT 0,
    added_at TEXT NOT NULL,
    fingerprint TEXT NOT NULL DEFAULT ''
);
CREATE TABLE persons (
    person_key TEXT PRIMARY KEY,
    patient_id TEXT NOT NULL DEFAULT '',
    patient_id_issuer TEXT NOT NULL DEFAULT '',
    mrn TEXT NOT NULL DEFAULT '',
    mrn_authority TEXT NOT NULL DEFAULT '',
    last_name TEXT NOT NULL DEFAULT '',
    first_name TEXT NOT NULL DEFAULT '',
    birth_date TEXT NOT NULL DEFAULT '',
    sex TEXT NOT NULL DEFAULT ''
);
CREATE TABLE visits (
    visit_key TEXT PRIMARY KEY,
    person_key TEXT,
    start_time TEXT,
    end_time TEXT,
    unit TEXT NOT NULL DEFAULT '',
    icu INTEGER NOT NULL DEFAULT 0,
    source_id INTEGER NOT NULL REFERENCES sources(id)
);
CREATE TABLE entries (
    entry_key TEXT PRIMARY KEY,
    person_key TEXT,
    visit_key TEXT,
    category TEXT NOT NULL DEFAULT '',
    time TEXT,
    end_time TEXT,
    name TEXT NOT NULL DEFAULT '',
    code TEXT NOT NULL DEFAULT '',
    vocabulary TEXT NOT NULL DEFAULT '',
    value_num REAL,
    value_text TEXT NOT NULL DEFAULT '',
    unit TEXT NOT NULL DEFAULT '',
    ref_low REAL,
    ref_high REAL,
    flag TEXT NOT NULL DEFAULT '',
    author TEXT NOT NULL DEFAULT '',
    text TEXT NOT NULL DEFAULT '',
    source_id INTEGER NOT NULL REFERENCES sources(id)
);
CREATE TABLE recordings (
    recording_key TEXT PRIMARY KEY,
    person_key TEXT,
    visit_key TEXT,
    file TEXT NOT NULL,
    format TEXT NOT NULL DEFAULT '',
    start_time TEXT,
    end_time TEXT,
    label TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT '',
    size INTEGER,
    fingerprint TEXT NOT NULL DEFAULT '',
    source_id INTEGER NOT NULL REFERENCES sources(id)
);
CREATE TABLE extras (
    table_name TEXT NOT NULL,
    row_key TEXT NOT NULL,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    source_id INTEGER NOT NULL REFERENCES sources(id)
);
CREATE TABLE row_sources (
    table_name TEXT NOT NULL,
    row_key TEXT NOT NULL,
    source_id INTEGER NOT NULL REFERENCES sources(id),
    PRIMARY KEY (table_name, row_key, source_id)
);
CREATE INDEX entries_person ON entries (person_key);
CREATE INDEX entries_time ON entries (time);
CREATE INDEX entries_category_name ON entries (category, name);
CREATE INDEX entries_visit ON entries (visit_key);
CREATE INDEX entries_source ON entries (source_id);
CREATE INDEX visits_person ON visits (person_key);
CREATE INDEX recordings_person ON recordings (person_key);
CREATE INDEX recordings_file ON recordings (file);
CREATE INDEX recordings_source ON recordings (source_id);
CREATE INDEX persons_patient_id ON persons (patient_id, patient_id_issuer);
CREATE INDEX persons_mrn ON persons (mrn, mrn_authority);
CREATE INDEX extras_row ON extras (table_name, row_key);
CREATE INDEX extras_source ON extras (source_id);
CREATE INDEX row_sources_source ON row_sources (source_id);
";

/// The key of an encrypted database, from [`KEY_VAR`]: 64 hex digits (a raw 256-bit key).
pub fn key_from_env() -> anyhow::Result<String> {
    let k =
        std::env::var(KEY_VAR).map_err(|_| anyhow::anyhow!("--encrypt needs the key in ${KEY_VAR} (64 hex digits)"))?;
    let k = k.trim().to_string();
    if k.len() != 64 || !k.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("${KEY_VAR} must be 64 hex digits (a 256-bit key)");
    }
    Ok(k)
}

/// How a database is opened: encrypted (with its key) or not.
#[derive(Clone, Default)]
pub struct Opening {
    pub key: Option<String>,
}

impl Opening {
    /// Encrypted with the key in [`KEY_VAR`] when `encrypt`, else plain.
    pub fn new(encrypt: bool) -> anyhow::Result<Opening> {
        Ok(Opening { key: if encrypt { Some(key_from_env()?) } else { None } })
    }
}

fn connect(path: &Path, how: &Opening) -> anyhow::Result<Connection> {
    let c = Connection::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    if let Some(k) = &how.key {
        c.execute_batch(&format!("PRAGMA key = \"x'{k}'\";"))?;
    }
    c.busy_timeout(std::time::Duration::from_secs(30))?;
    Ok(c)
}

/// A new database at `path` (refused when the file exists), its local times in `timezone`.
pub fn create(path: &Path, how: &Opening, timezone: &str) -> anyhow::Result<Connection> {
    if path.exists() {
        anyhow::bail!("{} exists already", path.display());
    }
    crate::util::tz(timezone)?;
    let mut c = connect(path, how)?;
    let tx = c.transaction()?;
    tx.execute_batch(CREATE)?;
    for (k, v) in [
        ("schema", SCHEMA.to_string()),
        ("timezone", timezone.trim().to_string()),
        ("created_by", format!("db-builder {}", env!("CARGO_PKG_VERSION"))),
    ] {
        tx.execute("INSERT INTO meta (key, value) VALUES (?1, ?2)", params![k, v])?;
    }
    bump(&tx)?;
    tx.commit()?;
    Ok(c)
}

/// A new database in memory (a dry run on a database not made yet: `add --dry-run`).
pub fn scratch(timezone: &str) -> anyhow::Result<Connection> {
    crate::util::tz(timezone)?;
    let mut c = Connection::open_in_memory()?;
    let tx = c.transaction()?;
    tx.execute_batch(CREATE)?;
    for (k, v) in [("schema", SCHEMA.to_string()), ("timezone", timezone.trim().to_string())] {
        tx.execute("INSERT INTO meta (key, value) VALUES (?1, ?2)", params![k, v])?;
    }
    tx.commit()?;
    Ok(c)
}

/// A copy of the open database in a new file `to` (refused when it exists): `VACUUM INTO`, or,
/// encrypted, SQLCipher's export into a new file with the same key, so that the copy is
/// encrypted as the database is.
pub fn save_copy(c: &Connection, to: &Path, how: &Opening) -> anyhow::Result<()> {
    if to.exists() {
        anyhow::bail!("{} exists already", to.display());
    }
    let file = to.to_string_lossy().to_string();
    match &how.key {
        None => {
            c.execute("VACUUM INTO ?1", [&file])?;
        }
        Some(k) => {
            c.execute(&format!("ATTACH DATABASE ?1 AS copy KEY \"x'{k}'\""), [&file])?;
            let done = c.query_row("SELECT sqlcipher_export('copy')", [], |_| Ok(()));
            c.execute("DETACH DATABASE copy", [])?;
            done?;
        }
    }
    Ok(())
}

/// The rules file kept beside a database by the web interface: `study.sqlite` ->
/// `study.rules.toml`.
pub fn rules_file(db: &Path) -> PathBuf {
    let stem = db.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    db.with_file_name(format!("{stem}.rules.toml"))
}

/// An existing database: its schema checked (a database of another schema, or no db-builder
/// database, is refused).
pub fn open(path: &Path, how: &Opening) -> anyhow::Result<Connection> {
    if !path.is_file() {
        anyhow::bail!("{}: no such database (make it with `db-builder new`)", path.display());
    }
    let c = connect(path, how)?;
    let schema: Option<String> = match c.query_row("SELECT value FROM meta WHERE key = 'schema'", [], |r| r.get(0)) {
        Ok(v) => Some(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) => {
            anyhow::bail!(
                "{}: cannot be read: it is encrypted (give --encrypt, with its key in ${KEY_VAR}) or its key is wrong",
                path.display()
            )
        }
        Err(e) if e.to_string().contains("no such table") => None,
        Err(e) => return Err(e.into()),
    };
    match schema.as_deref() {
        Some(s) if s == SCHEMA.to_string() => Ok(c),
        Some(s) => anyhow::bail!("{}: schema {s}, but this db-builder writes schema {SCHEMA}", path.display()),
        None => anyhow::bail!("{}: not a db-builder database (no meta.schema)", path.display()),
    }
}

/// The database at `path`, made first when it does not exist yet.
pub fn open_or_create(path: &Path, how: &Opening, timezone: Option<&str>) -> anyhow::Result<Connection> {
    if path.exists() {
        let c = open(path, how)?;
        if let Some(tz) = timezone {
            let have = meta(&c, "timezone")?.unwrap_or_default();
            if have != tz.trim() {
                anyhow::bail!(
                    "{}: its local times are in {have}; a database has one time zone (not {})",
                    path.display(),
                    tz.trim()
                );
            }
        }
        Ok(c)
    } else {
        create(path, how, timezone.unwrap_or(DEFAULT_TIMEZONE))
    }
}

pub fn meta(c: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()
}

/// The UTC time now, as ISO 8601 (`2026-11-14T10:22:31Z`).
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// What [`rewrite_paths`] changed (or would).
#[derive(Debug, Default, PartialEq)]
pub struct Rewritten {
    pub recordings: usize,
    pub files: usize,
    pub sources: usize,
}

/// The files a database lists, rewritten by `rules`' path rules (`db-builder rewrite`): the
/// recordings', the entries' files (`extras`), and the sources' inputs.  Only the paths change:
/// the files are not read, their fingerprints stay.  With `write` false, only counted.
pub fn rewrite_paths(c: &Connection, rules: &crate::rules::Rules, write: bool) -> rusqlite::Result<Rewritten> {
    // one table's column of paths, by its key: how many change
    let column = |table: &str, key: &str, col: &str, filter: &str| -> rusqlite::Result<usize> {
        let rows: Vec<(rusqlite::types::Value, String)> = c
            .prepare(&format!("SELECT {key}, {col} FROM {table}{filter}"))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut n = 0;
        for (k, old) in rows {
            let new = rules.path(&old);
            if new != old {
                n += 1;
                if write {
                    c.execute(&format!("UPDATE {table} SET {col} = ?1 WHERE {key} = ?2"), rusqlite::params![new, k])?;
                }
            }
        }
        Ok(n)
    };
    let n = Rewritten {
        recordings: column("recordings", "recording_key", "file", "")?,
        files: column("extras", "rowid", "value", " WHERE name IN ('file', 'recording_file')")?,
        sources: column("sources", "id", "file", "")?,
    };
    if write && n != Rewritten::default() {
        bump(c)?;
    }
    Ok(n)
}

/// A new `meta.version`: the UTC time and a counter, changed at every write.
pub fn bump(c: &Connection) -> rusqlite::Result<()> {
    let n = meta(c, "version")?
        .and_then(|v| v.rsplit_once(' ').and_then(|(_, n)| n.parse::<u64>().ok()))
        .map_or(1, |n| n + 1);
    c.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('version', ?1)", [format!("{} {n}", now())])?;
    Ok(())
}

/// One MiB: how much of a file's start and end its fingerprint reads.
const MIB: u64 = 1 << 20;

/// A file's fingerprint: SHA-256 (hex) of its size, its modification time (nanoseconds since
/// 1970) and its first and last MiB; a folder's, of its files' names, sizes and times.
pub fn fingerprint(path: &Path) -> anyhow::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    if path.is_dir() {
        // a folder (a WFDB collection, a DICOM series): the names, sizes and modification times
        // of its files, subfolders included (as the recording engine walks them)
        let mut files = recording_engine::sources::wfdb_folder::fingerprint(path);
        files.sort();
        let mut h = Sha256::new();
        for (n, size, t) in files {
            h.update(n.as_bytes());
            h.update(size.to_le_bytes());
            h.update(t.to_le_bytes());
        }
        return Ok(hex(&h.finalize()));
    }
    let mut f = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let m = f.metadata()?;
    let size = m.len();
    let mtime =
        m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let mut h = Sha256::new();
    h.update(size.to_le_bytes());
    h.update(mtime.to_le_bytes());
    let mut buf = vec![0u8; size.min(MIB) as usize];
    f.read_exact(&mut buf)?;
    h.update(&buf);
    if size > MIB {
        let tail = (size - MIB).max(MIB);
        f.seek(SeekFrom::Start(tail))?;
        let mut buf = vec![0u8; (size - tail) as usize];
        f.read_exact(&mut buf)?;
        h.update(&buf);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A path as the database stores it: relative to the database's folder when it lies inside
/// it (`/` between folders), else absolute.
pub fn stored_path(file: &Path, db_dir: &Path) -> String {
    match file.strip_prefix(db_dir) {
        Ok(rel) if !rel.as_os_str().is_empty() => {
            rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/")
        }
        _ => file.to_string_lossy().to_string(),
    }
}

/// A stored path read back: relative ones from the database's folder.
pub fn resolve_path(stored: &str, db_dir: &Path) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        db_dir.join(p)
    }
}

/// The folder of the database file (absolute).
pub fn folder_of(db: &Path) -> PathBuf {
    let abs = absolute(db);
    abs.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// A path made absolute (and canonical when it exists), without following it anywhere else.
pub fn absolute(p: &Path) -> PathBuf {
    let abs = PathBuf::from(recording_engine::paths::absolute(&p.to_string_lossy()));
    if let Ok(c) = std::fs::canonicalize(&abs) {
        return c;
    }
    // a file not made yet (the database): its folder canonical
    match (abs.parent(), abs.file_name()) {
        (Some(d), Some(n)) => std::fs::canonicalize(d).map(|d| d.join(n)).unwrap_or(abs),
        _ => abs,
    }
}

/// A source already added: its id and fingerprint.
pub fn source_of(c: &Connection, file: &str) -> rusqlite::Result<Vec<(i64, String)>> {
    let mut st = c.prepare("SELECT id, fingerprint FROM sources WHERE file = ?1 ORDER BY id")?;
    let rows = st.query_map([file], |r| Ok((r.get(0)?, r.get(1)?)))?.collect();
    rows
}

/// A new source: its id.
pub fn add_source(c: &Connection, file: &str, kind: &str, fingerprint: &str) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO sources (file, kind, rows, added_at, fingerprint) VALUES (?1, ?2, 0, ?3, ?4)",
        params![file, kind, now(), fingerprint],
    )?;
    Ok(c.last_insert_rowid())
}

/// What removing a source took out.
#[derive(Debug, Default)]
pub struct Removed {
    pub entries: usize,
    pub recordings: usize,
    pub visits: usize,
    pub persons: usize,
}

/// Take a source out: its entries, recordings and extras, and the visits and persons no other
/// source names (and no row left refers to).  A visit another source also names stays, as that
/// source's.
pub fn remove_source(c: &Connection, id: i64) -> anyhow::Result<Removed> {
    let found: Option<i64> = c.query_row("SELECT id FROM sources WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    if found.is_none() {
        anyhow::bail!("no source {id} (db-builder info lists them)");
    }
    let mut r = Removed {
        entries: c.execute("DELETE FROM entries WHERE source_id = ?1", [id])?,
        recordings: c.execute("DELETE FROM recordings WHERE source_id = ?1", [id])?,
        ..Default::default()
    };
    c.execute("DELETE FROM extras WHERE source_id = ?1", [id])?;
    c.execute("DELETE FROM row_sources WHERE source_id = ?1", [id])?;
    r.visits = c.execute(
        "DELETE FROM visits WHERE visit_key NOT IN (SELECT row_key FROM row_sources WHERE table_name = 'visits')
           AND visit_key NOT IN (SELECT visit_key FROM entries WHERE visit_key IS NOT NULL)
           AND visit_key NOT IN (SELECT visit_key FROM recordings WHERE visit_key IS NOT NULL)",
        [],
    )?;
    // a visit left: the source of another that names it
    c.execute(
        "UPDATE visits SET source_id = COALESCE(
             (SELECT MIN(source_id) FROM row_sources s WHERE s.table_name = 'visits' AND s.row_key = visits.visit_key),
             (SELECT MIN(source_id) FROM entries e WHERE e.visit_key = visits.visit_key),
             (SELECT MIN(source_id) FROM recordings e WHERE e.visit_key = visits.visit_key))
         WHERE source_id = ?1",
        [id],
    )?;
    r.persons = c.execute(
        "DELETE FROM persons WHERE person_key NOT IN (SELECT row_key FROM row_sources WHERE table_name = 'persons')
           AND person_key NOT IN (SELECT person_key FROM visits WHERE person_key IS NOT NULL)
           AND person_key NOT IN (SELECT person_key FROM entries WHERE person_key IS NOT NULL)
           AND person_key NOT IN (SELECT person_key FROM recordings WHERE person_key IS NOT NULL)",
        [],
    )?;
    c.execute("DELETE FROM sources WHERE id = ?1", [id])?;
    Ok(r)
}
