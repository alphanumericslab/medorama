//! The patient pages (docs/design/patient-page.md): the list of patients, one patient's
//! summary, journey, report and the detail of each entry.  Everything here is read-only and
//! built from the databases loaded now: removing one removes what it shows.
//!
//! Identifiers (names, MRNs, dates of birth) and the text of notes go only to users with the
//! PHI permission; others see pseudonymous IDs, and that a note exists.
//!
//! | Module | Contents |
//! |---|---|
//! | `identity` | identifiers, placeholders, pseudonyms, ages |
//! | `list` | the Patient Catalog: browsing in SQL, text search, a page's details |
//! | `summary` | one patient's summary: encounters and stays, conditions, medications, allergies |
//! | `items` | a patient's items, the catalog's and the connected databases', and their report entries |
//! | `journey`, `trends`, `report`, `detail` | the page's parts |
//! | `files` | attached files: kinds, viewers, what may be read |
//! | `loading` | what an import still loading holds, left out (SQL fragments) |
//! | `monitoring` | ICU monitoring from a recording's index |

use crate::categories;
use crate::config::Config;
use crate::db::audit;
use crate::search::{parse, Doc, Term};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use serde::Serialize;
use sha2::Digest;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

mod detail;
mod files;
mod identity;
mod items;
mod journey;
mod list;
mod loading;
mod monitoring;
mod report;
mod summary;
mod trends;

pub use self::detail::*;
pub use self::files::*;
pub use self::identity::*;
use self::items::*;
pub use self::journey::*;
pub use self::list::*;
pub use self::loading::*;
use self::monitoring::*;
pub use self::report::*;
pub use self::summary::*;
pub use self::trends::*;
