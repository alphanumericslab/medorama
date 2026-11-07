//! Cohorts (docs/user-guide.md, "Cohorts"): named sets of patients and the criteria that
//! found them.  The criteria are a structured query (a language engine can write the same):
//! `{"version": 1, "include": expr, "exclude": [expr, …]}`, where an expr is a criterion or a
//! group `{"op": "and" | "or" | "not", "of": [expr, …]}`; a patient is in when they meet
//! `include` (none: everyone) and no `exclude` (looked for in their whole history).  A
//! criterion of kind `expression` holds criteria written as text (expr.rs).
//!
//! A stored cohort's definition is its criteria, its source (`Source`: the cohort a subcohort
//! is made within, or the cohorts merged; none: the whole catalog), and the patients added or
//! removed manually.  It keeps its members as a list (a snapshot): "refresh" shows, then
//! applies, what the definition selects now; members added manually stay, members removed
//! manually stay out.  Every change is in the audit log, which is the cohort's history.
//! Cohorts are private to their owner unless shared with chosen users, to read or to edit.
//!
//! | Module | Contents |
//! |---|---|
//! | `criteria` | the structured criteria, a search's matches |
//! | `values` | dates, numbers and units, as criteria test them |
//! | `resolve` | names in criteria text: columns, results, connected databases' measurements |
//! | `evaluate` | the groups, the per-search cache, the second pass after linking; searches |
//! | `in_catalog`, `in_sources` | one criterion in the catalog (SQL), in a connected database |
//! | `words` | criteria in words |
//! | `stored`, `changes` | stored cohorts: definitions, access, members; making and changing them |
//! | `export` | members, definitions and data, as CSV, JSON and Excel |

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::sql::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::categories;
use crate::db::{audit, now};
use crate::patient::{self, PatientRow};
use crate::users::User;
use crate::util::{parse_time, tz};

mod changes;
mod criteria;
mod evaluate;
mod export;
mod in_catalog;
mod in_sources;
mod resolve;
mod stored;
mod values;
mod words;

pub use self::changes::*;
pub use self::criteria::*;
pub use self::evaluate::*;
pub use self::export::*;
use self::in_catalog::*;
use self::in_sources::*;
pub use self::resolve::*;
pub use self::stored::*;
use self::values::*;
pub use self::words::*;
