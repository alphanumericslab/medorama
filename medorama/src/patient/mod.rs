//! The patient pages (docs/design/patient-page.md): the list of patients, one patient's
//! summary, journey, report and the detail of each entry.  Everything here is read-only and
//! read from the connected databases the user may view, now: removing one removes what it
//! shows.
//!
//! Identifiers (names, MRNs, dates of birth) and the text of notes go only to users with the
//! PHI permission on the database that holds them; others see pseudonyms, and that a note
//! exists.
//!
//! | Module | Contents |
//! |---|---|
//! | `identity` | identifiers, placeholders, pseudonyms, ages |
//! | `list` | the Patient Catalog: the patients of the databases the user may view, searched |
//! | `summary` | one patient's summary: visits, conditions, medications, allergies |
//! | `items` | a patient's data from the connected databases: entries, visits, recordings |
//! | `journey`, `trends`, `report`, `detail` | the page's parts |
//! | `files` | recordings' files: kinds, viewers, what may be read |
//! | `restricted` | the data of databases a user sees de-identified outside Privacy Mode (granted without `phi`) |
//! | `monitoring` | ICU monitoring from a recording's index |
//! | `history` | the patients each user has looked at |

use crate::categories;
use crate::config::Config;
use crate::db::audit;
use crate::search::{parse, Doc, Term};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use serde::Serialize;
use sha2::Digest;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

mod detail;
mod files;
mod history;
mod identity;
mod items;
mod journey;
mod list;
mod monitoring;
mod report;
mod restricted;
mod summary;
mod trends;

pub use self::detail::*;
pub use self::files::*;
pub use self::history::*;
pub use self::identity::*;
pub use self::items::Visit;
use self::items::*;
pub use self::journey::*;
pub use self::list::*;
use self::monitoring::*;
pub use self::report::*;
// every page reads a patient's data through `restricted::load` (not `items::load`): the data of
// databases granted without `phi` leave de-identified
use self::restricted::load;
pub use self::restricted::{without_phi, PerDatabase};
pub use self::summary::*;
pub use self::trends::*;
