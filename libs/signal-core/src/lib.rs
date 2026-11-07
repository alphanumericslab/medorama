//! The values, number formatting and text helpers that Medorama's readers, its recording
//! engine and the VitalStream viewers share: Python-compatible, so that the Rust, C++ and
//! Python implementations write byte-identical output.
//!
//! | Module | Contents |
//! |---|---|
//! | [`value`] | [`Value`], [`Dict`](value::Dict) and the [`dict!`] macro: JSON and `str()` text as Python writes them |
//! | [`pyfmt`] | number and time formatting identical to Python's (`repr`, `%g`, `round`, CSV) |
//! | [`text`] | `hex`, Python's `str.strip()` and `float(str)` |

pub mod pyfmt;
pub mod text;
pub mod value;

pub use value::Value;

/// An error of a reader or of the engine.
#[derive(Debug)]
pub enum Error {
    /// An I/O error, with the path it concerns.
    Io(std::io::Error, String),
    /// A file that is not what its format says.
    Format(String),
    /// A value out of range, or a request that cannot be met.
    Value(String),
    /// An error of the WFDB library (wfdb-x).
    Wfdb(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e, p) => write!(f, "{p}: {e}"),
            Error::Format(m) | Error::Value(m) | Error::Wfdb(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// An I/O error of `path`: `fs::read(p).map_err(Error::io(p))`.
    pub fn io(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
        move |e| Error::Io(e, path.display().to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
