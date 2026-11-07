//! Import formats: tables exported by a known system, recognised by their columns and converted
//! into a table Medorama reads (docs/administration.md, "Import formats").  A format keeps what is worth
//! showing and searching, under the catalog's column names, in the values' usual forms (dates in
//! ISO, units in the measurements' names), and leaves out what is not (layout, staff, visits'
//! bookkeeping).  The rows stored are the converted ones; the import records the format.
//!
//! Built in (code, for what a description cannot say):
//!
//! | Format | What | Category |
//! |---|---|---|
//! | `ge-muse` | GE MUSE ECG metadata (one row per resting ECG; the XML file's elements as columns), the patient by MRN | ecg |
//! | `ge-muse-patient-id` | the same, the patient by a patient ID (a research ID, not an MRN) | ecg |
//! | `clif-…` | CLIF's tables (every table of CLIF 2.1 and five of 3.0: patient, hospitalization, ADT, vitals, labs, respiratory support, medications ...: [`clif`]) | by table |
//!
//! Described (a TOML file in a folder of `formats_dirs`: [`declared`]): column by column, a site
//! adds its own exports without a new build.

pub mod clif;
pub mod declared;
mod ge_muse;

use std::sync::Arc;

/// A table's rows converted one by one.
pub trait Converter {
    /// The converted table's columns.
    fn headers(&self) -> Vec<String>;
    /// A row converted, or why it is left out (counted, never named: rows hold identifiers).
    fn row(&mut self, record: &[String]) -> Result<Vec<String>, String>;
    /// The rows a row makes: one (`row`), or several (a ventilator's settings, each an entry;
    /// none when it holds nothing to keep).
    fn rows(&mut self, record: &[String]) -> Result<Vec<Vec<String>>, String> {
        self.row(record).map(|r| vec![r])
    }
}

/// A known export.
pub trait Format: Send + Sync {
    /// Its name on the command line and in the page (`ge-muse`).
    fn key(&self) -> &str;
    fn label(&self) -> &str;
    /// The category of its items (`ecg`, `lab`, ...): the kind of data it holds.
    fn category(&self) -> &str;
    /// What it is, for the page.
    fn about(&self) -> String;
    /// Where it comes from: `built in`, or its file.
    fn origin(&self) -> String {
        "built in".into()
    }
    /// Whether a table with these columns is one (recognised without being named).
    fn detect(&self, headers: &[String]) -> bool;
    /// The converter for a table with these columns (an error: not this format).
    fn start(&self, headers: &[String]) -> anyhow::Result<Box<dyn Converter>>;
}

/// The built-in formats.
pub fn builtin() -> Vec<Arc<dyn Format>> {
    let mut v: Vec<Arc<dyn Format>> = vec![Arc::new(ge_muse::Muse::new(false)), Arc::new(ge_muse::Muse::new(true))];
    v.extend(clif::tables().into_iter().map(|t| Arc::new(t) as Arc<dyn Format>));
    v
}

/// Every format the catalog knows: the built-in ones, then those described in `formats_dirs`
/// (a file that cannot be read is said in the second list, and left out).
pub fn all(cfg: &crate::config::Config) -> (Vec<Arc<dyn Format>>, Vec<String>) {
    let mut out = builtin();
    let mut problems = vec![];
    for (f, path) in declared::load_dirs(&cfg.formats_dirs, &mut problems) {
        if out.iter().any(|x| x.key() == f.key()) {
            problems.push(format!("{}: the format {} is defined twice", path.display(), f.key()));
            continue;
        }
        out.push(Arc::new(f));
    }
    (out, problems)
}

/// Which format an import uses.
#[derive(Clone, Default)]
pub enum Choice {
    /// The one the columns say, if any.
    #[default]
    Detect,
    /// None: the table as it is.
    None,
    Named(Arc<dyn Format>),
}

impl Choice {
    /// `auto` (or empty), `none`, or a format's key, among `known`.
    pub fn parse(s: &str, known: &[Arc<dyn Format>]) -> anyhow::Result<Choice> {
        match s.trim().to_lowercase().as_str() {
            "" | "auto" => Ok(Choice::Detect),
            "none" => Ok(Choice::None),
            k => known.iter().find(|f| f.key().eq_ignore_ascii_case(k)).map(|f| Choice::Named(f.clone())).ok_or_else(
                || {
                    anyhow::anyhow!(
                        "no import format {k:?} (known: auto, none, {})",
                        known.iter().map(|f| f.key().to_string()).collect::<Vec<_>>().join(", ")
                    )
                },
            ),
        }
    }

    /// The format for a table with these columns, among `known`.
    pub fn resolve(&self, headers: &[String], known: &[Arc<dyn Format>]) -> Option<Arc<dyn Format>> {
        match self {
            Choice::Detect => known.iter().find(|f| f.detect(headers)).cloned(),
            Choice::None => None,
            Choice::Named(f) => Some(f.clone()),
        }
    }
}

/// The formats, for the page: key, label, category, what, where from.
pub fn describe(known: &[Arc<dyn Format>]) -> Vec<serde_json::Value> {
    known
        .iter()
        .map(|f| {
            serde_json::json!({"key": f.key(), "label": f.label(), "category": f.category(), "about": f.about(), "origin": f.origin()})
        })
        .collect()
}
