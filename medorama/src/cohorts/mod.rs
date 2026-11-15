//! Cohorts (docs/user-guide.md, "Cohorts"): named sets of patients and the criteria that
//! found them.  The criteria are a structured query (a language engine can write the same):
//! `{"version": 1, "include": expr, "exclude": [expr, …]}`, where an expr is a criterion or a
//! group `{"op": "and" | "or" | "not", "of": [expr, …]}`; a patient is in when they meet
//! `include` (none: everyone) and no `exclude` (looked for in their whole history).  A
//! criterion of kind `expression` holds criteria written as text (expr.rs).
//!
//! A stored cohort's definition is its criteria, the databases it searches (none: every one its
//! user may search), its source (`Source`: the cohort a subcohort is made within, or the
//! cohorts merged), and the patients added or removed manually.  It keeps its members as a list
//! (a snapshot), in its own encrypted file of the workspace, each with the databases it came
//! from: "refresh" shows, then applies, what the definition selects now; members added manually
//! stay, members removed manually stay out; a database removed takes out every member it
//! contributed to.  Every change is in the audit log, which is the cohort's history.  Cohorts
//! are private to their maker unless shared with chosen users (grants), to read or to edit.
//!
//! | Module | Contents |
//! |---|---|
//! | `criteria` | the structured criteria, a search's matches |
//! | `values` | dates, numbers and units, as criteria test them |
//! | `resolve` | names in criteria text: databases and their measurements |
//! | `evaluate` | the scope, the groups and provenance, the second pass after linking; searches |
//! | `in_sources` | one criterion in a connected database |
//! | `words` | criteria in words |
//! | `stored`, `changes` | stored cohorts: definitions, access, members; making and changing them |
//! | `members` | a cohort's members and their provenance, in the workspace |
//! | `export` | members, definitions and data, as CSV, JSON and Excel |

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::sql::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::categories;
use crate::config::Config;
use crate::db::{audit, now};
use crate::patient::{self, PatientRow};
use crate::users::User;
use crate::util::{parse_time, tz};

mod changes;
mod criteria;
mod evaluate;
mod export;
mod in_sources;
mod members;
mod resolve;
mod stored;
mod values;
mod words;

pub use self::changes::*;
pub use self::criteria::*;
pub use self::evaluate::*;
pub use self::export::*;
use self::in_sources::*;
pub use self::members::{forget_unlinked, purge_database};
pub use self::resolve::*;
pub use self::stored::*;
use self::values::*;
pub use self::words::*;

/// Whether an entry's written value is free text that "the text of notes and reports" covers
/// (Admin › Privacy Mode): a long one (a comment, a result written out), as on the patient page.
pub fn is_long_text(value: &str) -> bool {
    value.chars().count() > 40 || value.contains('\n')
}
