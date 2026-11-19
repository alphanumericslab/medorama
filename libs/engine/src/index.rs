//! Index cache: per source a folder `<key>/` with `meta.json` and one little-endian binary
//! file per array (envelopes, markers, vital-sign series), memory-mapped when loaded (index
//! format 2); or an index store (format 3, [`crate::store`]), built the same way and packed.  The
//! key derives from the source's real path, size and modification time.

use crate::common::{clean, Arrays, INDEX_VERSION};
use crate::sources;
use crate::store::Removed;
use memmap2::Mmap;
use sha2::{Digest, Sha256};
use signal_core::value::{Dict, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// The real path (symbolic links resolved), on Windows without the `\\?\` prefix
/// (viewers/vitalstream/docs/index-format.md, "The key").
pub fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|p| PathBuf::from(crate::paths::plain(&p.to_string_lossy())))
        .unwrap_or_else(|_| path.to_path_buf())
}

pub fn cache_key(path: &Path) -> String {
    cache_key_of(path, INDEX_VERSION)
}

/// The key of `path` in index format `version` (2: a folder, 3: a store).
pub fn cache_key_of(path: &Path, version: i64) -> String {
    let p = real_path(path);
    let fp = if p.is_dir() || sources::wfdb_folder::is_record(&p) {
        Value::List(
            sources::wfdb_folder::fingerprint(&p)
                .into_iter()
                .map(|(n, s, m)| Value::List(vec![n.into(), s.into(), Value::Int(m)]))
                .collect(),
        )
    } else {
        let m = std::fs::metadata(&p).ok();
        let mtime = m
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i128);
        Value::List(vec![m.map_or(0, |m| m.len()).into(), Value::Int(mtime)])
    };
    let text = Value::List(vec![version.into(), p.to_string_lossy().to_string().into(), fp]).to_json();
    signal_core::text::hex(&Sha256::digest(text.as_bytes()))[..20].to_string()
}

fn write_array<T: Copy>(dir: &Path, name: &str, v: &[T], to_le: impl Fn(T) -> Vec<u8>) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(v));
    for &x in v {
        bytes.extend(to_le(x));
    }
    std::fs::write(dir.join(name), bytes)
}

/// Write the array files of an index into `dir` (viewers/vitalstream/docs/index-format.md, "Files of one index");
/// adapters written in Rust use it for their `index` command.
pub fn write_arrays(dir: &Path, a: &Arrays) -> signal_core::Result<()> {
    let io = |r: std::io::Result<()>| r.map_err(signal_core::Error::io(dir));
    for (lvl, (lo, hi)) in a.env_min.iter().zip(&a.env_max).enumerate() {
        io(write_array(dir, &format!("env_min_{lvl}.f32"), lo, |x| x.to_le_bytes().to_vec()))?;
        io(write_array(dir, &format!("env_max_{lvl}.f32"), hi, |x| x.to_le_bytes().to_vec()))?;
    }
    io(write_array(dir, "mk_piece.i32", &a.mk_piece, |x| x.to_le_bytes().to_vec()))?;
    io(write_array(dir, "mk_sample.i64", &a.mk_sample, |x| x.to_le_bytes().to_vec()))?;
    io(write_array(dir, "mk_code.i16", &a.mk_code, |x| x.to_le_bytes().to_vec()))?;
    io(write_array(dir, "vit_t.f64", &a.vit_t, |x| x.to_le_bytes().to_vec()))?;
    io(write_array(dir, "vit_v.f64", &a.vit_v, |x| x.to_le_bytes().to_vec()))
}

/// Index `path` into `cache_dir`; returns the key.  `progress` gets fractions 0..1.
pub fn build_index(path: &Path, cache_dir: &Path, progress: &dyn Fn(f64)) -> signal_core::Result<String> {
    build_index_until(path, cache_dir, progress, &|| false)
}

/// The writers of each key in this process, one at a time.
fn key_lock(key: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, l| Arc::strong_count(l) > 1); // keys no writer holds any more
    locks.entry(key.to_string()).or_default().clone()
}

/// A folder of `cache_dir` named uniquely for this writer: `<key>.<process id>.<n>.<suffix>`.
fn unique(cache_dir: &Path, key: &str, suffix: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    cache_dir.join(format!("{key}.{}.{}.{suffix}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// [`build_index`] that stops (an error, nothing left behind) once `cancelled` returns true.
pub fn build_index_until(
    path: &Path,
    cache_dir: &Path,
    progress: &dyn Fn(f64),
    cancelled: &dyn Fn() -> bool,
) -> signal_core::Result<String> {
    std::fs::create_dir_all(cache_dir).map_err(signal_core::Error::io(cache_dir))?;
    let key = cache_key(path);
    let lock = key_lock(&key);
    let _writing = lock.lock().unwrap_or_else(|e| e.into_inner());
    progress(0.0);
    let folder = cache_dir.join(&key);
    let tmp = Removed(unique(cache_dir, &key, "tmp"));
    build_files(path, &tmp.0, &key, INDEX_VERSION, progress, cancelled)?;
    // an index already there, complete and written by this engine, is kept (the same content);
    // any other is moved aside first, so that readers never see a folder half replaced
    let current = load_from(&folder)
        .ok()
        .is_some_and(|(m, _)| m.get("engine").and_then(Value::as_str) == Some(env!("CARGO_PKG_VERSION")));
    if !current {
        let old = Removed(unique(cache_dir, &key, "old"));
        if folder.exists() {
            std::fs::rename(&folder, &old.0).map_err(signal_core::Error::io(&folder))?;
        }
        if let Err(e) = std::fs::rename(&tmp.0, &folder) {
            // another program moved its index of this key in first
            if load_from(&folder).is_err() {
                return Err(signal_core::Error::io(&folder)(e));
            }
        }
    }
    progress(1.0);
    Ok(key)
}

/// Index `path` into an index store (format 3): built in a private temporary folder, then packed
/// in one transaction; returns the key.  Stops (an error, nothing left behind) once `cancelled`
/// returns true.
pub fn build_into_store(
    path: &Path,
    store: &crate::store::Store,
    progress: &dyn Fn(f64),
    cancelled: &dyn Fn() -> bool,
) -> signal_core::Result<String> {
    let key = cache_key_of(path, crate::store::STORE_FORMAT);
    let lock = key_lock(&format!("{}\u{1}{key}", store.path().display()));
    let _writing = lock.lock().unwrap_or_else(|e| e.into_inner());
    progress(0.0);
    let tmp_root = store.tmp_dir();
    crate::store::private_dir(&tmp_root).map_err(signal_core::Error::io(&tmp_root))?;
    let tmp = Removed(unique(&tmp_root, &key, "tmp"));
    build_files(path, &tmp.0, &key, crate::store::STORE_FORMAT, progress, cancelled)?;
    store.pack(&key, &tmp.0)?;
    progress(1.0);
    Ok(key)
}

/// Take a ready-made index of `path` into an index store (format 3): `folder` holds what an
/// adapter's `index` command writes (its `meta.json` and arrays; a recording database may list it,
/// docs/databases.md, "Indexes and signals a recording database holds").  Its array files are
/// copied into a private temporary folder, its `meta.json` gets the recording's path, name and
/// key as a build writes them, it is checked as a reader would check it, and packed in one
/// transaction; the folder itself is only read.  `made_by` is who the database says made it: it
/// must be the program that would read the recording now ([`sources::reader_of`]: an adapter's
/// name and version, or this engine's), else the index is refused (the caller indexes the
/// recording itself).  Returns the key.
pub fn import_into_store(
    path: &Path,
    folder: &Path,
    made_by: &str,
    store: &crate::store::Store,
) -> signal_core::Result<String> {
    let (reader, wanted) = sources::reader_of(path)?;
    // a file's size and time say whether it changed; a WFDB record's header alone does not
    if !path.is_file() || reader.iter().any(|(k, v)| *k == "kind" && v == "wfdb") {
        return Err(signal_core::Error::Value(format!(
            "{}: only a recording that is one file is taken indexed ahead (a WFDB record is indexed where it is read)",
            path.display()
        )));
    }
    if made_by.trim() != wanted {
        return Err(signal_core::Error::Value(format!(
            "{}: made by {:?}, not by {wanted:?}, which reads the recording here",
            folder.display(),
            made_by.trim()
        )));
    }
    let key = cache_key_of(path, crate::store::STORE_FORMAT);
    let lock = key_lock(&format!("{}\u{1}{key}", store.path().display()));
    let _writing = lock.lock().unwrap_or_else(|e| e.into_inner());
    let text = std::fs::read_to_string(folder.join("meta.json")).map_err(signal_core::Error::io(folder))?;
    let Ok(Value::Dict(mut meta)) = Value::from_json(&text) else {
        return Err(signal_core::Error::Format(format!("{}: meta.json is not a JSON object", folder.display())));
    };
    let tmp_root = store.tmp_dir();
    crate::store::private_dir(&tmp_root).map_err(signal_core::Error::io(&tmp_root))?;
    let tmp = Removed(unique(&tmp_root, &key, "tmp"));
    std::fs::create_dir_all(&tmp.0).map_err(signal_core::Error::io(&tmp.0))?;
    for e in std::fs::read_dir(folder).map_err(signal_core::Error::io(folder))?.flatten() {
        let p = e.path();
        let array = p.extension().is_some_and(|x| ["f32", "f64", "i16", "i32", "i64"].contains(&&*x.to_string_lossy()));
        if array && p.is_file() {
            std::fs::copy(&p, tmp.0.join(e.file_name())).map_err(signal_core::Error::io(&p))?;
        }
    }
    for (k, v) in reader {
        meta.insert(k.into(), v.into());
    }
    let real = real_path(path);
    meta.insert("path".into(), real.to_string_lossy().to_string().into());
    meta.insert("name".into(), path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default().into());
    meta.insert("key".into(), key.clone().into());
    meta.insert("index_version".into(), crate::store::STORE_FORMAT.into());
    meta.insert("engine".into(), env!("CARGO_PKG_VERSION").into());
    std::fs::write(tmp.0.join("meta.json"), clean(Value::Dict(meta)).to_json())
        .map_err(signal_core::Error::io(&tmp.0))?;
    if let Err(why) = load_files(&tmp.0, crate::store::STORE_FORMAT) {
        return Err(signal_core::Error::Format(format!("{}: the index cannot be read: {why}", folder.display())));
    }
    store.pack(&key, &tmp.0)?;
    Ok(key)
}

/// Write the index files of `path` (its arrays and `meta.json`, index format `version`) into the
/// new folder `dir`, and check them as a reader would.
fn build_files(
    path: &Path,
    dir: &Path,
    key: &str,
    version: i64,
    progress: &dyn Fn(f64),
    cancelled: &dyn Fn() -> bool,
) -> signal_core::Result<()> {
    let t_start = Instant::now();
    std::fs::create_dir_all(dir).map_err(signal_core::Error::io(dir))?;
    let io = |r: std::io::Result<()>| r.map_err(signal_core::Error::io(dir));
    let mut meta = match sources::build(path, dir, progress, cancelled)? {
        sources::Building::Here((meta, a)) => {
            write_arrays(dir, &a)?;
            meta
        }
        // the adapter wrote the arrays into `dir`
        sources::Building::ByAdapter(meta) => meta,
    };
    let real = real_path(path);
    meta.insert("path".into(), real.to_string_lossy().to_string().into());
    meta.insert("name".into(), path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default().into());
    meta.insert("key".into(), key.into());
    meta.insert("index_version".into(), version.into());
    meta.insert("engine".into(), env!("CARGO_PKG_VERSION").into());
    meta.insert("indexed_s".into(), signal_core::pyfmt::round_to(t_start.elapsed().as_secs_f64(), 1).into());
    io(std::fs::write(dir.join("meta.json"), clean(Value::Dict(meta)).to_json()))?;
    // what a reader would refuse is not kept (times or rates that are not finite numbers, ...)
    if let Err(why) = load_files(dir, version) {
        return Err(signal_core::Error::Format(format!("{}: the index would be inconsistent: {why}", path.display())));
    }
    Ok(())
}

/// A memory-mapped array file (empty files are not mapped).
pub struct Mapped<T> {
    map: Option<Mmap>,
    _t: std::marker::PhantomData<T>,
}

impl<T: Copy> Mapped<T> {
    fn open(path: &Path) -> std::io::Result<Mapped<T>> {
        let f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        if len % std::mem::size_of::<T>() as u64 != 0 {
            return Err(std::io::Error::other("not a whole number of values"));
        }
        let map = if len == 0 {
            None
        } else {
            // SAFETY: cache files are written once (their folder is renamed into place) and
            // never modified afterwards.
            Some(unsafe { Mmap::map(&f)? })
        };
        Ok(Mapped { map, _t: std::marker::PhantomData })
    }

    pub fn as_slice(&self) -> &[T] {
        match &self.map {
            None => &[],
            Some(m) => {
                let n = m.len() / std::mem::size_of::<T>();
                assert_eq!(m.as_ptr() as usize % std::mem::align_of::<T>(), 0);
                // SAFETY: page-aligned mapping of n little-endian values of a plain numeric
                // type (the platforms supported are little-endian).
                unsafe { std::slice::from_raw_parts(m.as_ptr() as *const T, n) }
            }
        }
    }
}

/// An array read whole: a version 2 file memory-mapped, or values held.
pub enum Values<T> {
    Mapped(Mapped<T>),
    Held(Vec<T>),
}

impl<T: Copy> Values<T> {
    pub fn as_slice(&self) -> &[T] {
        match self {
            Values::Mapped(m) => m.as_slice(),
            Values::Held(v) => v,
        }
    }
}

/// An envelope array: a version 2 file memory-mapped, or a store's array read by range.
pub enum Env {
    File(Mapped<f32>),
    Chunked(crate::store::Chunked<f32>),
}

impl Env {
    pub fn len(&self) -> usize {
        match self {
            Env::File(m) => m.as_slice().len(),
            Env::Chunked(c) => c.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The values `[o, o+n)`, or None when they cannot be read.
    pub fn range(&self, o: usize, n: usize) -> Option<std::borrow::Cow<'_, [f32]>> {
        match self {
            Env::File(m) => m.as_slice().get(o..o + n).map(std::borrow::Cow::Borrowed),
            Env::Chunked(c) => c.range(o, n).map(std::borrow::Cow::Owned),
        }
    }
}

pub struct Loaded {
    /// The index folder (a store's index of an adapter's recording: a private folder holding its
    /// `meta.json`, for the adapter to read samples again).
    pub dir: PathBuf,
    pub env_min: Vec<Env>,
    pub env_max: Vec<Env>,
    pub mk_piece: Values<i32>,
    pub mk_sample: Values<i64>,
    pub mk_code: Values<i16>,
    pub vit_t: Values<f64>,
    pub vit_v: Values<f64>,
    /// That private folder, removed when the index is dropped.
    pub(crate) _reading: Option<Removed>,
}

pub fn index_dir(cache_dir: &Path, key: &str) -> PathBuf {
    cache_dir.join(key)
}

/// (meta, arrays) from the cache, or None: no index under `key`, one of another version, or
/// one whose `meta.json` and arrays do not agree ([`check`]).
pub fn load_index(cache_dir: &Path, key: &str) -> Option<(Dict, Loaded)> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    load_from(&index_dir(cache_dir, key)).ok()
}

fn load_from(dir: &Path) -> Result<(Dict, Loaded), String> {
    load_files(dir, INDEX_VERSION)
}

/// The index files in `dir`, of index format `version`, checked.
fn load_files(dir: &Path, version: i64) -> Result<(Dict, Loaded), String> {
    let text = std::fs::read_to_string(dir.join("meta.json")).map_err(|e| format!("meta.json: {e}"))?;
    let Ok(Value::Dict(meta)) = Value::from_json(&text) else { return Err("meta.json is not a JSON object".into()) };
    if meta.get("index_version").and_then(Value::as_int) != Some(version as i128) {
        return Err("another index version".into());
    }
    fn open<T: Copy>(dir: &Path, name: String) -> Result<Mapped<T>, String> {
        Mapped::open(&dir.join(&name)).map_err(|e| format!("{name}: {e}"))
    }
    fn held<T: Copy>(dir: &Path, name: &str) -> Result<Values<T>, String> {
        open(dir, name.into()).map(Values::Mapped)
    }
    let mut env_min = vec![];
    let mut env_max = vec![];
    for lvl in 0.. {
        if !dir.join(format!("env_min_{lvl}.f32")).exists() {
            break;
        }
        env_min.push(Env::File(open(dir, format!("env_min_{lvl}.f32"))?));
        env_max.push(Env::File(open(dir, format!("env_max_{lvl}.f32"))?));
    }
    let loaded = Loaded {
        dir: dir.to_path_buf(),
        env_min,
        env_max,
        mk_piece: held(dir, "mk_piece.i32")?,
        mk_sample: held(dir, "mk_sample.i64")?,
        mk_code: held(dir, "mk_code.i16")?,
        vit_t: held(dir, "vit_t.f64")?,
        vit_v: held(dir, "vit_v.f64")?,
        _reading: None,
    };
    check(&meta, &loaded)?;
    Ok((meta, loaded))
}

/// Whether `meta.json` agrees with the arrays (viewers/vitalstream/docs/index-format.md,
/// "Loading an index"): every offset and length inside its array, so that queries never read
/// outside one.
pub fn check(meta: &Dict, a: &Loaded) -> Result<(), String> {
    let list = |k: &str| match meta.get(k) {
        Some(Value::List(l)) => Ok(l.as_slice()),
        None => Ok(&[][..]),
        _ => Err(format!("{k} is not a list")),
    };
    let int = |v: &Value, k: &str| -> Result<i128, String> {
        v.get(k).and_then(Value::as_int).ok_or_else(|| format!("a {k} that is not a whole number"))
    };
    let num = |v: &Value, k: &str| -> Result<f64, String> {
        match v.get(k) {
            Some(Value::Int(i)) => Ok(*i as f64),
            Some(Value::Float(f)) if f.is_finite() => Ok(*f),
            _ => Err(format!("a {k} that is not a finite number")),
        }
    };
    let channels = list("channels")?.len() as i128;
    for (i, p) in list("pieces")?.iter().enumerate() {
        let bad = |why: String| format!("piece {i}: {why}");
        if int(p, "id").map_err(bad)? != i as i128 {
            return Err(bad("its id is not its position".into()));
        }
        let ch = int(p, "ch").map_err(bad)?;
        if !(0..channels).contains(&ch) {
            return Err(bad(format!("channel {ch} is not among the {channels} channels")));
        }
        let fs = num(p, "fs").map_err(bad)?;
        if !(fs > 0.0) {
            return Err(bad(format!("a sampling rate of {fs}")));
        }
        num(p, "t0").map_err(bad)?;
        if int(p, "n").map_err(bad)? < 0 {
            return Err(bad("a negative number of samples".into()));
        }
        let env = match p.get("env") {
            Some(Value::List(l)) => l.as_slice(),
            None => &[],
            _ => return Err(bad("env is not a list".into())),
        };
        if env.len() > a.env_min.len() {
            return Err(bad(format!("{} envelope levels; the index has {}", env.len(), a.env_min.len())));
        }
        for (lvl, e) in env.iter().enumerate() {
            let pair = e.as_list().filter(|x| x.len() == 2).and_then(|x| Some((x[0].as_int()?, x[1].as_int()?)));
            let Some((off, len)) = pair else {
                return Err(bad(format!("envelope level {lvl} is not [offset, length]")));
            };
            let size = a.env_min[lvl].len().min(a.env_max[lvl].len()) as i128;
            if off < 0 || len < 0 || off + len > size {
                return Err(bad(format!("envelope level {lvl} [{off}, {len}] lies outside its {size} values")));
            }
        }
    }
    let pieces = list("pieces")?.len() as i128;
    let (mp, ms, mc) = (a.mk_piece.as_slice(), a.mk_sample.as_slice(), a.mk_code.as_slice());
    if mp.len() != ms.len() || mp.len() != mc.len() {
        return Err("the marker arrays differ in length".into());
    }
    if mp.iter().any(|&p| !(0..pieces).contains(&(p as i128))) || mp.windows(2).any(|w| w[0] > w[1]) {
        return Err("mk_piece names a piece that does not exist, or is not in order".into());
    }
    let names = list("marker_names")?.len() as i128;
    if mc.iter().any(|&c| !(0..names).contains(&(c as i128))) {
        return Err("mk_code names a marker name that does not exist".into());
    }
    let (vt, vv) = (a.vit_t.as_slice().len() as i128, a.vit_v.as_slice().len() as i128);
    if vt != vv {
        return Err("vit_t and vit_v differ in length".into());
    }
    for v in list("vitals")? {
        let (off, n) = (int(v, "offset")?, int(v, "n")?);
        if off < 0 || n < 0 || off + n > vt {
            return Err(format!("a vital-sign series [{off}, {n}] lies outside the {vt} values"));
        }
    }
    Ok(())
}
