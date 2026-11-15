//! The index store (index format 3, viewers/vitalstream/docs/index-format.md, "Version 3: the
//! index store"): one SQLite database of indexes, encrypted with SQLCipher when its owner has a
//! key.  An index is built as version 2 files in a temporary folder, then packed into the store in
//! one transaction; envelopes are read back by range, a chunk at a time.

use crate::index::{check, Env, Loaded, Values};
use signal_core::value::{Dict, Value};
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The store's format (`store.format`).
pub const STORE_FORMAT: i64 = 3;
/// Values per chunk in a new store (`store.chunk_values`).
pub const CHUNK_VALUES: usize = 4096;
/// Chunks of one array kept in memory once read.
const CACHED_CHUNKS: usize = 16;

/// A little-endian numeric value of an array file.
pub trait Le: Copy + Send + Sync + 'static {
    const SIZE: usize;
    fn from_le(b: &[u8]) -> Self;
}

macro_rules! le {
    ($($t:ty),*) => {$(
        impl Le for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            fn from_le(b: &[u8]) -> Self {
                <$t>::from_le_bytes(b.try_into().expect("a whole value"))
            }
        }
    )*};
}
le!(f32, f64, i16, i32, i64);

fn values<T: Le>(bytes: &[u8]) -> Vec<T> {
    bytes.chunks_exact(T::SIZE).map(T::from_le).collect()
}

fn err(path: &Path, e: impl std::fmt::Display) -> signal_core::Error {
    signal_core::Error::Format(format!("{}: {e}", path.display()))
}

/// An index store, opened.
pub struct Store {
    path: PathBuf,
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// a connection of its own for the envelopes' chunks ([`Chunked`]), so that windows are
    /// read while an index is packed (the store is in WAL mode: a reader is not blocked by a
    /// writer)
    chunks: Arc<Mutex<rusqlite::Connection>>,
    chunk: usize,
}

/// An array of a store read by range, a chunk at a time, the last chunks read kept.
pub struct Chunked<T> {
    conn: Arc<Mutex<rusqlite::Connection>>,
    key: String,
    array: String,
    len: usize,
    chunk: usize,
    cache: Mutex<VecDeque<(usize, Arc<Vec<T>>)>>,
    _t: PhantomData<T>,
}

impl<T: Le> Chunked<T> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn chunk_at(&self, n: usize) -> Option<Arc<Vec<T>>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, v)) = cache.iter().find(|(i, _)| *i == n) {
            return Some(v.clone());
        }
        let bytes: Vec<u8> = {
            let c = self.conn.lock().unwrap_or_else(|e| e.into_inner());
            c.query_row(
                "SELECT data FROM chunks WHERE key = ?1 AND array = ?2 AND n = ?3",
                rusqlite::params![self.key, self.array, n as i64],
                |r| r.get(0),
            )
            .ok()?
        };
        // every chunk holds `chunk` values, but the last the rest
        let want = self.chunk.min(self.len.saturating_sub(n * self.chunk)) * T::SIZE;
        if bytes.len() != want {
            return None;
        }
        let v = Arc::new(values::<T>(&bytes));
        if cache.len() >= CACHED_CHUNKS {
            cache.pop_front();
        }
        cache.push_back((n, v.clone()));
        Some(v)
    }

    /// The values `[o, o+n)`, or None when the store cannot be read.
    pub fn range(&self, o: usize, n: usize) -> Option<Vec<T>> {
        if n == 0 {
            return Some(vec![]);
        }
        if o + n > self.len {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        let (first, last) = (o / self.chunk, (o + n - 1) / self.chunk);
        for c in first..=last {
            let v = self.chunk_at(c)?;
            let start = if c == first { o - c * self.chunk } else { 0 };
            let end = if c == last { o + n - c * self.chunk } else { v.len() };
            out.extend_from_slice(v.get(start..end)?);
        }
        Some(out)
    }
}

/// Removes a folder when dropped.
pub(crate) struct Removed(pub(crate) PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A folder only its owner may read.
pub(crate) fn private_dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

impl Store {
    /// Open (or make) the store at `path`; `key` a raw 256-bit SQLCipher key (SQLCipher 4's
    /// default settings), or None for a plain store.
    pub fn open(path: &Path, key: Option<&[u8; 32]>) -> signal_core::Result<Store> {
        Store::open_with(path, key, CHUNK_VALUES)
    }

    /// [`Store::open`], a new store made with `chunk_values` values per chunk (an existing one
    /// keeps its own): tests cut small arrays into several chunks.
    pub fn open_with(path: &Path, key: Option<&[u8; 32]>, chunk_values: usize) -> signal_core::Result<Store> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(signal_core::Error::io(dir))?;
        }
        let connect = || -> signal_core::Result<rusqlite::Connection> {
            let c = rusqlite::Connection::open(path).map_err(|e| err(path, e))?;
            if let Some(k) = key {
                let hex: String = k.iter().map(|b| format!("{b:02x}")).collect();
                c.execute_batch(&format!("PRAGMA key = \"x'{hex}'\";")).map_err(|e| err(path, e))?;
            }
            c.busy_timeout(std::time::Duration::from_secs(30)).map_err(|e| err(path, e))?;
            c.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))
                .map_err(|_| err(path, "cannot be read: it is encrypted, or its key is wrong"))?;
            Ok(c)
        };
        let c = connect()?;
        c.pragma_update(None, "journal_mode", "WAL").map_err(|e| err(path, e))?;
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS store (name TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS indexes (key TEXT PRIMARY KEY, meta TEXT NOT NULL, engine TEXT NOT NULL,
                 written_at REAL NOT NULL);
             CREATE TABLE IF NOT EXISTS arrays (key TEXT NOT NULL, array TEXT NOT NULL, n_values INTEGER NOT NULL,
                 PRIMARY KEY (key, array)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS chunks (key TEXT NOT NULL, array TEXT NOT NULL, n INTEGER NOT NULL,
                 data BLOB NOT NULL, PRIMARY KEY (key, array, n)) WITHOUT ROWID;",
        )
        .map_err(|e| err(path, e))?;
        c.execute(
            "INSERT OR IGNORE INTO store (name, value) VALUES ('format', ?1), ('chunk_values', ?2)",
            rusqlite::params![STORE_FORMAT.to_string(), chunk_values.max(1).to_string()],
        )
        .map_err(|e| err(path, e))?;
        let get = |name: &str| -> Option<String> {
            c.query_row("SELECT value FROM store WHERE name = ?1", [name], |r| r.get(0)).ok()
        };
        if get("format").as_deref() != Some(&STORE_FORMAT.to_string()) {
            return Err(err(path, "another index store format"));
        }
        let chunk = get("chunk_values").and_then(|v| v.parse::<usize>().ok()).filter(|&n| n > 0);
        let chunk = chunk.ok_or_else(|| err(path, "no chunk size"))?;
        // opened once the tables exist and the store is in WAL mode; it only reads
        let chunks = connect()?;
        chunks.pragma_update(None, "query_only", true).map_err(|e| err(path, e))?;
        let store = Store {
            path: path.to_path_buf(),
            conn: Arc::new(Mutex::new(c)),
            chunks: Arc::new(Mutex::new(chunks)),
            chunk,
        };
        store.sweep_tmp();
        Ok(store)
    }

    /// Remove what a writer or reader that ended without cleaning up left in the temporary
    /// folder: entries untouched for a day (no build takes that long).
    fn sweep_tmp(&self) {
        let Ok(dir) = std::fs::read_dir(self.tmp_dir()) else { return };
        let day = std::time::Duration::from_secs(86_400);
        for e in dir.flatten() {
            let old =
                e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > day);
            if old {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where indexes are built before they are packed: `<store folder>/index.tmp`.
    pub fn tmp_dir(&self) -> PathBuf {
        self.path.parent().unwrap_or(Path::new(".")).join("index.tmp")
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The engine that wrote the index of `key`, if there is one.
    pub fn engine_of(&self, key: &str) -> Option<String> {
        self.conn().query_row("SELECT engine FROM indexes WHERE key = ?1", [key], |r| r.get(0)).ok()
    }

    /// The `meta.json` text of the index of `key`, if there is one.
    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn().query_row("SELECT meta FROM indexes WHERE key = ?1", [key], |r| r.get(0)).ok()
    }

    /// The keys of the indexes held.
    pub fn keys(&self) -> Vec<String> {
        let c = self.conn();
        let Ok(mut st) = c.prepare("SELECT key FROM indexes ORDER BY key") else { return vec![] };
        st.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    /// Remove the index of `key`; whether there was one.
    pub fn remove(&self, key: &str) -> signal_core::Result<bool> {
        let mut c = self.conn();
        let tx = c.transaction().map_err(|e| err(&self.path, e))?;
        tx.execute("DELETE FROM chunks WHERE key = ?1", [key]).map_err(|e| err(&self.path, e))?;
        tx.execute("DELETE FROM arrays WHERE key = ?1", [key]).map_err(|e| err(&self.path, e))?;
        let n = tx.execute("DELETE FROM indexes WHERE key = ?1", [key]).map_err(|e| err(&self.path, e))?;
        tx.commit().map_err(|e| err(&self.path, e))?;
        Ok(n > 0)
    }

    /// Pack the version 2 files of an index (its `meta.json` and arrays, in `dir`) into the store,
    /// in one transaction: an index of the key written by the same engine is kept, any other
    /// replaced.
    pub fn pack(&self, key: &str, dir: &Path) -> signal_core::Result<()> {
        let meta = std::fs::read_to_string(dir.join("meta.json")).map_err(signal_core::Error::io(dir))?;
        let engine = match Value::from_json(&meta) {
            Ok(Value::Dict(m)) => m.get("engine").and_then(Value::as_str).unwrap_or("").to_string(),
            _ => return Err(err(dir, "meta.json is not a JSON object")),
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(signal_core::Error::io(dir))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| ["f32", "f64", "i16", "i32", "i64"].contains(&&*x.to_string_lossy()))
            })
            .collect();
        files.sort();
        let mut c = self.conn();
        let tx = c.transaction().map_err(|e| err(&self.path, e))?;
        let current: Option<String> =
            tx.query_row("SELECT engine FROM indexes WHERE key = ?1", [key], |r| r.get(0)).ok();
        if current.as_deref() == Some(engine.as_str()) {
            return Ok(());
        }
        tx.execute("DELETE FROM chunks WHERE key = ?1", [key]).map_err(|e| err(&self.path, e))?;
        tx.execute("DELETE FROM arrays WHERE key = ?1", [key]).map_err(|e| err(&self.path, e))?;
        tx.execute("DELETE FROM indexes WHERE key = ?1", [key]).map_err(|e| err(&self.path, e))?;
        let written =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        tx.execute(
            "INSERT INTO indexes (key, meta, engine, written_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![key, meta, engine, written],
        )
        .map_err(|e| err(&self.path, e))?;
        {
            let mut ins = tx
                .prepare("INSERT INTO chunks (key, array, n, data) VALUES (?1, ?2, ?3, ?4)")
                .map_err(|e| err(&self.path, e))?;
            let mut length = tx
                .prepare("INSERT INTO arrays (key, array, n_values) VALUES (?1, ?2, ?3)")
                .map_err(|e| err(&self.path, e))?;
            for f in &files {
                let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let size = match f.extension().map(|x| x.to_string_lossy().to_string()).as_deref() {
                    Some("i16") => 2,
                    Some("f32" | "i32") => 4,
                    _ => 8,
                };
                let bytes = std::fs::read(f).map_err(signal_core::Error::io(f))?;
                if bytes.len() % size != 0 {
                    return Err(err(f, "not a whole number of values"));
                }
                length
                    .execute(rusqlite::params![key, name, (bytes.len() / size) as i64])
                    .map_err(|e| err(&self.path, e))?;
                // every array has a row, an empty one a chunk of no bytes
                let step = self.chunk * size;
                let parts: Vec<&[u8]> = if bytes.is_empty() { vec![&[][..]] } else { bytes.chunks(step).collect() };
                for (n, part) in parts.into_iter().enumerate() {
                    ins.execute(rusqlite::params![key, name, n as i64, part]).map_err(|e| err(&self.path, e))?;
                }
            }
        }
        tx.commit().map_err(|e| err(&self.path, e))
    }

    /// The bytes of a whole array, or None when the index has no such array.
    fn whole(&self, key: &str, array: &str) -> Option<Vec<u8>> {
        let c = self.conn();
        let mut st = c.prepare("SELECT data FROM chunks WHERE key = ?1 AND array = ?2 ORDER BY n").ok()?;
        let parts: Vec<Vec<u8>> = st.query_map([key, array], |r| r.get(0)).ok()?.flatten().collect();
        (!parts.is_empty()).then(|| parts.concat())
    }

    /// The number of values of an array (the `arrays` table), or None when it is missing.
    fn length(&self, key: &str, array: &str) -> Option<usize> {
        let n: i64 = self
            .conn()
            .query_row("SELECT n_values FROM arrays WHERE key = ?1 AND array = ?2", [key, array], |r| r.get(0))
            .ok()?;
        usize::try_from(n).ok()
    }

    /// (meta, arrays) of the index of `key`, or None: no such index, or one whose `meta.json`
    /// and arrays do not agree (index-format.md, "Loading an index").
    pub fn load(&self, key: &str) -> Option<(Dict, Loaded)> {
        self.load_checked(key).ok()
    }

    fn load_checked(&self, key: &str) -> Result<(Dict, Loaded), String> {
        let text: String = self
            .conn()
            .query_row("SELECT meta FROM indexes WHERE key = ?1", [key], |r| r.get(0))
            .map_err(|_| "no index".to_string())?;
        let Ok(Value::Dict(meta)) = Value::from_json(&text) else { return Err("meta is not a JSON object".into()) };
        if meta.get("index_version").and_then(Value::as_int) != Some(STORE_FORMAT as i128) {
            return Err("another index version".into());
        }
        let held = |name: &str| -> Result<(Vec<u8>, usize), String> {
            let n = self.length(key, name).ok_or_else(|| format!("{name}: missing"))?;
            Ok((self.whole(key, name).ok_or_else(|| format!("{name}: missing"))?, n))
        };
        fn whole<T: Le>(name: &str, (b, n): (Vec<u8>, usize)) -> Result<Values<T>, String> {
            if b.len() != n * T::SIZE {
                return Err(format!("{name}: not its {n} values"));
            }
            Ok(Values::Held(values::<T>(&b)))
        }
        let mut env_min = vec![];
        let mut env_max = vec![];
        for lvl in 0.. {
            let (lo, hi) = (format!("env_min_{lvl}.f32"), format!("env_max_{lvl}.f32"));
            let Some(lo_n) = self.length(key, &lo) else { break };
            let hi_n = self.length(key, &hi).ok_or_else(|| format!("{hi}: missing"))?;
            let chunked = |array: String, len: usize| Chunked {
                conn: self.chunks.clone(),
                key: key.to_string(),
                array,
                len,
                chunk: self.chunk,
                cache: Mutex::new(VecDeque::new()),
                _t: PhantomData,
            };
            env_min.push(Env::Chunked(chunked(lo, lo_n)));
            env_max.push(Env::Chunked(chunked(hi, hi_n)));
        }
        // an adapter reads samples again from its index folder (protocol 1): a private folder
        // holding only its meta.json, for as long as the index is loaded
        let reading = match meta.get("adapter") {
            Some(Value::Str(a)) if !a.is_empty() => {
                // its own folder per load: two loads of one index never share one
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let dir = self.tmp_dir().join(format!("{key}.{}.{n}.read", std::process::id()));
                private_dir(&dir)
                    .and_then(|_| std::fs::write(dir.join("meta.json"), &text))
                    .map_err(|e| e.to_string())?;
                Some(Removed(dir))
            }
            _ => None,
        };
        let loaded = Loaded {
            dir: reading.as_ref().map_or_else(|| self.tmp_dir(), |r| r.0.clone()),
            env_min,
            env_max,
            mk_piece: whole("mk_piece.i32", held("mk_piece.i32")?)?,
            mk_sample: whole("mk_sample.i64", held("mk_sample.i64")?)?,
            mk_code: whole("mk_code.i16", held("mk_code.i16")?)?,
            vit_t: whole("vit_t.f64", held("vit_t.f64")?)?,
            vit_v: whole("vit_v.f64", held("vit_v.f64")?)?,
            _reading: reading,
        };
        check(&meta, &loaded)?;
        Ok((meta, loaded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "wfdb")]
    fn envelopes_are_read_while_the_store_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../viewers/vitalstream/testdata/collection");
        let store = Arc::new(Store::open_with(&dir.path().join("index.sqlite"), None, 100).unwrap());
        let key = crate::index::build_into_store(&rec, &store, &|_| {}, &|| false).unwrap();
        let (_, loaded) = store.load(&key).unwrap();
        let Some(crate::index::Env::Chunked(env)) = loaded.env_min.into_iter().next() else { panic!("not chunked") };
        // the writers' connection busy (as while an index is packed): the chunks are read
        let busy = store.conn();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(env.range(0, env.len().min(250)).map(|v| v.len()));
        });
        let got = rx.recv_timeout(std::time::Duration::from_secs(10));
        drop(busy);
        assert!(matches!(got, Ok(Some(n)) if n > 0), "{got:?}");
    }
}
