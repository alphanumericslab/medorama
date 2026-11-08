//! Reading recordings: the readers built into the engine, and every other format through an
//! installed adapter ([`crate::adapters`]).  Each reader gives `build(path, progress, cancelled)
//! -> (meta, arrays)` for the index (viewers/vitalstream/docs/index-format.md), and a [`Reader`] of
//! physical samples `[a, b)` of a piece; past that point, every recording is the same.
//!
//! | Reader | Kind | Reads |
//! |---|---|---|
//! | [`wfdb_folder`] | `wfdb` | WFDB records (with wfdb-x): one record, a multi-segment record, a collection, a folder of records; without the `wfdb` feature, refused ([`NO_WFDB`]) |
//! | [`edf`] | `edf` | EDF, EDF+ (continuous and discontinuous) and BDF files |
//! | adapters | theirs | the formats installed adapters list (docs/adapters.md) |
//!
//! A new built-in reader is a module here, a line in [`kind_of`], `build` and [`Reader`], and
//! the same reader in the C++ and Python viewers (checked by `make parity`).
//!
//! WFDB support is the `wfdb` feature (on by default), the only part of the engine that uses
//! wfdb-x.  Without it, `wfdb_none.rs` is the `wfdb_folder` module: WFDB records and folders are
//! still recognised ([`kind_of`] says `wfdb`), and refused with [`NO_WFDB`] when they are
//! opened ([`unreadable`], and `build`); every other format is read as in the default build.

pub mod edf;
mod wfdb_files;
#[cfg(feature = "wfdb")]
pub mod wfdb_folder;
#[cfg(not(feature = "wfdb"))]
#[path = "wfdb_none.rs"]
pub mod wfdb_folder;

use crate::adapters::{self, Adapter};
use crate::common::Arrays;
use signal_core::value::{Dict, Value};
use signal_core::{Error, Result};
use std::path::{Path, PathBuf};

pub type Built = (Dict, Arrays);

/// The refusal of a WFDB record or folder by a build without the `wfdb` feature.
pub const NO_WFDB: &str = "this build has no WFDB support (built without the `wfdb` feature)";

/// Why this build cannot read the recording at `p`, if it cannot: a WFDB record or a folder
/// (which opens as WFDB records) in a build without the `wfdb` feature ([`NO_WFDB`]).
/// The programs refuse such a recording when it is opened or added, with this message.
pub fn unreadable(p: &Path) -> Option<&'static str> {
    (!wfdb_folder::SUPPORTED && (p.is_dir() || wfdb_folder::is_record(p))).then_some(NO_WFDB)
}

/// The kind of recording at `p`: `"wfdb"` for a WFDB record (`.hea` file) or a WFDB folder
/// (one that holds records itself, or a collection), `"edf"` for an EDF or BDF file, an adapter's kind for a file it reads (by
/// extension), or `None`.  Any folder can also be opened as the records below it
/// (`build`); a folder that holds none itself is not a recording when a tree is searched.
pub fn kind_of(p: &Path) -> Option<String> {
    if p.is_file() {
        if wfdb_folder::is_record(p) {
            return Some("wfdb".into());
        }
        if edf::is_edf(p) {
            return Some("edf".into());
        }
        return adapters::for_file(p).map(|(_, f)| f.kind);
    }
    if wfdb_folder::is_wfdb_folder(p, 2000) {
        return Some("wfdb".into());
    }
    None
}

/// Why `p` is not opened as a recording: for a WFDB signal file (`.dat`, `.mat`), to open its
/// header in its place; else that it is not a recording this program reads.
pub fn not_a_recording(p: &Path) -> String {
    wfdb_folder::signal_file_note(p).unwrap_or_else(|| format!("not a recording this program reads: {}", p.display()))
}

/// The label of the format a recording at `p` is read as (`WFDB record`, `EDF+`, an adapter's
/// label), or an empty string.
pub fn format_label(p: &Path) -> String {
    if p.is_dir() || wfdb_folder::is_record(p) {
        return wfdb_folder::format_label(p).into();
    }
    if edf::is_edf(p) {
        return edf::format_label(p).into();
    }
    adapters::for_file(p).map(|(_, f)| f.label).unwrap_or_default()
}

/// The index of a WFDB folder (built here), or of a file an adapter reads (the adapter writes
/// the index files into `dir`; its `meta.json` is returned with the arrays left on disk).
pub(crate) enum Building {
    Here(Built),
    ByAdapter(Dict),
}

pub(crate) fn build(path: &Path, dir: &Path, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<Building> {
    if path.is_dir() || wfdb_folder::is_record(path) {
        let (mut meta, arrays) = wfdb_folder::build(path, progress, cancelled)?;
        meta.insert("kind".into(), "wfdb".into());
        meta.insert("format".into(), wfdb_folder::format_label(path).into());
        return Ok(Building::Here((meta, arrays)));
    }
    if edf::is_edf(path) {
        let (mut meta, arrays) = edf::build(path, progress, cancelled)?;
        meta.insert("kind".into(), "edf".into());
        meta.insert("format".into(), edf::format_label(path).into());
        return Ok(Building::Here((meta, arrays)));
    }
    let (adapter, format) = adapters::for_file(path).ok_or_else(|| Error::Value(not_a_recording(path)))?;
    adapter.index(path, dir, progress, cancelled)?;
    let text = std::fs::read_to_string(dir.join("meta.json")).map_err(Error::io(&dir.join("meta.json")))?;
    let Ok(Value::Dict(mut meta)) = Value::from_json(&text) else {
        return Err(Error::Format(format!("{} wrote no valid meta.json", adapter.name)));
    };
    meta.insert("kind".into(), format.kind.into());
    meta.insert("format".into(), format.label.into());
    meta.insert("adapter".into(), adapter.name.clone().into());
    Ok(Building::ByAdapter(meta))
}

/// Physical samples of an indexed recording's pieces.
pub enum Reader {
    Wfdb(wfdb_folder::Reader),
    Edf(edf::Reader),
    Adapter(Adapter, PathBuf),
    None(String),
}

impl Reader {
    /// The reader of the recording indexed in `dir` (its `meta`).
    pub fn new(meta: &Dict, dir: &Path) -> Reader {
        let path = Path::new(meta.get("path").and_then(Value::as_str).unwrap_or(""));
        if meta.get("kind").and_then(Value::as_str) == Some("wfdb") {
            return Reader::Wfdb(wfdb_folder::Reader::new(path, meta));
        }
        if meta.get("kind").and_then(Value::as_str) == Some("edf") {
            return Reader::Edf(edf::Reader::new(path, meta));
        }
        match meta.get("adapter").and_then(Value::as_str) {
            Some(name) => match adapters::named(name) {
                Some(a) => Reader::Adapter(a, dir.to_path_buf()),
                None => Reader::None(format!("the {name} adapter is not installed: its samples cannot be read")),
            },
            None => Reader::None("no reader for this recording".into()),
        }
    }

    /// [`Reader::read`] of several `(piece, a, b)` at once (a WFDB record's signals over the
    /// same samples are decoded once).
    pub fn read_many(&self, wanted: &[(usize, i64, i64)]) -> Result<Vec<Vec<f64>>> {
        match self {
            Reader::Wfdb(r) => r.read_many(wanted),
            _ => wanted.iter().map(|&(p, a, b)| self.read(p, a, b)).collect(),
        }
    }

    pub fn read(&self, piece: usize, a: i64, b: i64) -> Result<Vec<f64>> {
        match self {
            Reader::Wfdb(r) => r.read(piece, a, b),
            Reader::Edf(r) => r.read(piece, a, b),
            Reader::Adapter(adapter, dir) => adapter.read(dir, piece, a, b),
            Reader::None(why) => Err(Error::Value(why.clone())),
        }
    }
}
