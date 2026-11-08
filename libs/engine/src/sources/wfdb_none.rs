//! A build without WFDB support (without the `wfdb` feature: `--no-default-features`): no
//! wfdb-x code at all.  WFDB records are still recognised by their files (a `.hea` file, a
//! folder of records, a collection: `wfdb_files.rs`), so that opening or adding one is refused
//! with [`NO_WFDB`] rather than taken for something else; every other format is read as in the
//! default build.  The same functions as the `wfdb_folder` module of the default build, so the
//! callers are the same.

use super::wfdb_files::with_ext;
pub use super::wfdb_files::{
    format_label, is_record, is_wfdb_folder, limits, mat_deidentified, read_csv, segment_table, set_limits,
    signal_file_note, Limits, MAX_FINGERPRINT_FILES,
};
use super::{Built, NO_WFDB};
use signal_core::value::Dict;
use std::path::{Path, PathBuf};

/// Whether this build reads WFDB records (the `wfdb` feature).
pub const SUPPORTED: bool = false;

fn refused() -> signal_core::Error {
    signal_core::Error::Value(NO_WFDB.into())
}

/// Refused: [`NO_WFDB`].
pub fn build(_path: &Path, _progress: &dyn Fn(f64), _cancelled: &dyn Fn() -> bool) -> signal_core::Result<Built> {
    Err(refused())
}

/// Refused: [`NO_WFDB`].
pub fn scan(_path: &Path, _limits: &Limits) -> signal_core::Result<Dict> {
    Err(refused())
}

/// The files of record `name` known without reading its header: the header itself (the signal
/// files it names are known only to wfdb-x).
pub fn record_files(_base: &Path, name: &str) -> Vec<String> {
    vec![format!("{name}.hea")]
}

/// [`record_files`], as paths below `base`.
pub fn files_read(base: &Path, name: &str) -> Vec<PathBuf> {
    vec![with_ext(&base.join(name), "hea")]
}

/// Refused: [`NO_WFDB`] (the record is not read at all).
pub fn check_files(_base: &Path, _name: &str) -> signal_core::Result<()> {
    Err(refused())
}

/// Refused: [`NO_WFDB`].
pub fn check_record(_hea: &Path) -> signal_core::Result<()> {
    Err(refused())
}

/// `[name, size, mtime ns]` of each file, for the cache key (`wfdb_files.rs`).
pub fn fingerprint(path: &Path) -> Vec<(String, u64, i128)> {
    super::wfdb_files::fingerprint(path, record_files)
}

/// The samples of a WFDB recording indexed by a build with WFDB support: refused.
pub struct Reader;

impl Reader {
    pub fn new(_path: &Path, _meta: &Dict) -> Reader {
        Reader
    }

    pub fn read(&self, _piece: usize, _a: i64, _b: i64) -> signal_core::Result<Vec<f64>> {
        Err(refused())
    }

    pub fn read_many(&self, _wanted: &[(usize, i64, i64)]) -> signal_core::Result<Vec<Vec<f64>>> {
        Err(refused())
    }
}
