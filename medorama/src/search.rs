//! The search syntax (docs/user-guide.md, "Searching"), shared by the Encounters and Recordings
//! tabs and the commands:
//!
//! - words: each must appear in some field (`smith 1950`);
//! - `field:value`: only in that field (`name:smith`, `dob:1950-03`, `source:pilot`);
//! - quotes for spaces: `source:"ICU pilot"`, `"van der"`;
//! - never case-sensitive; a value matches anywhere in the field (`name:smi` finds Smith).
//!
//! Identifier fields (names, date of birth, MRN, encounter number, and what the recordings'
//! files hold) are searchable only with the PHI permission; asking for one without it is an
//! error, not an empty result.
//!
//! In Privacy Mode a search, a filter and an order use only what the session is shown (gate.rs):
//! pseudonyms, tokens and placeholders in place of identifiers, each patient's dates moved back
//! by their own offset, so that no answer can tell a real value the page does not show
//! ([`SessionGate`], [`Offsets`], [`shown`]).  Dates typed in the page are dates as shown.

use std::collections::HashMap;

use crate::gate::Restricted;
use crate::sql::Connection;
use crate::users::User;

/// One search term: a field (or any field) and the text to find, lower-cased.
#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    pub field: Option<String>,
    pub value: String,
}

/// The terms of a search text.
pub fn parse(q: &str) -> Vec<Term> {
    let mut out = vec![];
    let chars: Vec<char> = q.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        // a field name: a letter, then letters, digits, '-' or '_', then ':' (so `12:30` is text)
        let mut j = i;
        while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '-' || chars[j] == '_') {
            j += 1;
        }
        let mut field = None;
        if j > i && chars[i].is_alphabetic() && j < chars.len() && chars[j] == ':' {
            field = Some(chars[i..j].iter().collect::<String>().to_lowercase());
            i = j + 1;
        }
        let value: String = if i < chars.len() && chars[i] == '"' {
            let start = i + 1;
            let mut k = start;
            while k < chars.len() && chars[k] != '"' {
                k += 1;
            }
            i = (k + 1).min(chars.len());
            chars[start..k.min(chars.len())].iter().collect()
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            chars[start..i].iter().collect()
        };
        let value = value.trim().to_lowercase();
        if !value.is_empty() || field.is_some() {
            out.push(Term { field, value });
        }
    }
    out
}

/// Fields searchable only with the PHI permission.
pub const PHI_FIELDS: &[&str] = &["name", "first", "last", "dob", "mrn", "csn", "file-name", "file-id", "file-dob"];

/// The fields of the Encounters search.
pub const ENCOUNTER_FIELDS: &[&str] = &[
    "encounter",
    "patient",
    "source",
    "unit",
    "bed",
    "admit",
    "discharge",
    "disposition",
    "sex",
    "status",
    "file",
    "name",
    "first",
    "last",
    "dob",
    "mrn",
    "csn",
    "file-name",
    "file-id",
    "file-dob",
];

/// The fields of the Recordings search.
pub const RECORDING_FIELDS: &[&str] = &[
    "file",
    "path",
    "kind",
    "format",
    "unit",
    "bed",
    "start",
    "status",
    "source",
    "encounter",
    "file-name",
    "file-id",
    "file-dob",
];

/// Check the terms against the fields of a list and the user's permission.
pub fn check(terms: &[Term], fields: &[&str], phi: bool) -> Result<(), String> {
    for t in terms {
        let Some(f) = &t.field else { continue };
        if !fields.contains(&f.as_str()) {
            let shown: Vec<&str> = fields.iter().copied().filter(|x| phi || !PHI_FIELDS.contains(x)).collect();
            return Err(format!("unknown field {f}: (fields: {})", shown.join(", ")));
        }
        if !phi && PHI_FIELDS.contains(&f.as_str()) {
            return Err(format!("searching by {f}: needs the PHI permission"));
        }
    }
    Ok(())
}

/// What one entry offers to the search: (field, lower-cased text) pairs.
#[derive(Debug, Default)]
pub struct Doc {
    pub values: Vec<(&'static str, String)>,
}

impl Doc {
    pub fn add(&mut self, field: &'static str, text: &str) {
        if !text.is_empty() {
            self.values.push((field, text.to_lowercase()));
        }
    }

    /// Every term found (unqualified terms: in any field this user may search).
    pub fn matches(&self, terms: &[Term], phi: bool) -> bool {
        terms.iter().all(|t| {
            self.values.iter().any(|(f, v)| {
                let allowed = phi || !PHI_FIELDS.contains(f);
                allowed && t.field.as_deref().is_none_or(|want| want == *f) && v.contains(&t.value)
            })
        })
    }
}

// ------------------------------------------------------------------ Privacy Mode

/// A Privacy Mode gate built here, for callers that have none at hand (the server keeps one with
/// its dictionary cached: `server::restricted`).
pub fn build_gate(c: &Connection) -> anyhow::Result<Restricted> {
    let version: i64 = c.query_row("SELECT version FROM data_version", [], |r| r.get(0))?;
    Ok(Restricted {
        settings: crate::deid::load(c)?,
        dict: std::sync::Arc::new(crate::gate::Dict::build(c, version)?),
        secret: crate::gate::secret(c)?,
    })
}

/// The gate a user's search goes through: the one given, else (a session in Privacy Mode) one
/// built here, so that a search never runs on real values for such a session; none otherwise.
pub enum SessionGate<'a> {
    None,
    Given(&'a Restricted),
    Own(Box<Restricted>),
}

impl SessionGate<'_> {
    pub fn get(&self) -> Option<&Restricted> {
        match self {
            SessionGate::None => None,
            SessionGate::Given(g) => Some(g),
            SessionGate::Own(g) => Some(g),
        }
    }
}

pub fn session_gate<'a>(c: &Connection, user: &User, given: Option<&'a Restricted>) -> anyhow::Result<SessionGate<'a>> {
    Ok(match given {
        Some(g) => SessionGate::Given(g),
        None if user.privacy => SessionGate::Own(Box::new(build_gate(c)?)),
        None => SessionGate::None,
    })
}

/// A value as a session in Privacy Mode is shown it: its JSON through the gate.
pub fn shown(g: &Restricted, v: &impl serde::Serialize) -> serde_json::Value {
    let mut v = serde_json::to_value(v).unwrap_or_default();
    g.json(&mut v);
    v
}

/// A JSON string (or array of strings) as text, for [`Doc::add`].
pub fn strings(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(a) => a.iter().flat_map(strings).collect(),
        serde_json::Value::Number(n) => vec![n.to_string()],
        _ => vec![],
    }
}

/// How far Privacy Mode moves each patient's times back (deid.rs): the catalog's offset and the
/// patient's own days; nothing when it shows real dates.  Searches compare dates as shown with
/// a time moved by it, and sort by shown times, so that neither reveals real ones.
pub struct Offsets {
    hides: bool,
    base: f64,
    spread: f64,
    by_patient: HashMap<i64, f64>,
}

impl Offsets {
    fn of(settings: &crate::deid::Settings, draws: &HashMap<i64, f64>) -> Offsets {
        let hides = settings.hides("dates");
        Offsets {
            hides,
            base: settings.base_seconds() as f64,
            spread: (settings.spread_days.max(0) * 86400) as f64,
            by_patient: if hides {
                draws.iter().map(|(p, d)| (*p, settings.for_draw(Some(*d)).offset_seconds() as f64)).collect()
            } else {
                HashMap::new()
            },
        }
    }

    /// The offsets of the catalog's settings and patients.
    pub fn load(c: &Connection) -> anyhow::Result<Offsets> {
        let draws: HashMap<i64, f64> = c
            .prepare("SELECT id, date_draw FROM patients WHERE date_draw IS NOT NULL")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(Offsets::of(&crate::deid::load(c)?, &draws))
    }

    /// The offsets a gate moves dates by.
    pub fn of_gate(g: &Restricted) -> Offsets {
        Offsets::of(&g.settings, &g.dict.draws)
    }

    /// Seconds a patient's times move back by (`None`: no patient, the catalog's offset alone).
    pub fn seconds(&self, patient: Option<i64>) -> f64 {
        if !self.hides {
            return 0.0;
        }
        patient.and_then(|p| self.by_patient.get(&p).copied()).unwrap_or(self.base)
    }

    /// A patient's time as shown.
    pub fn shift(&self, patient: Option<i64>, t: f64) -> f64 {
        t - self.seconds(patient)
    }

    /// The least and the most any patient's times move back by.
    pub fn range(&self) -> (f64, f64) {
        if self.hides {
            (self.base, self.base + self.spread)
        } else {
            (0.0, 0.0)
        }
    }

    /// Whether dates move at all.
    pub fn hides_dates(&self) -> bool {
        self.hides
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(f: Option<&str>, v: &str) -> Term {
        Term { field: f.map(str::to_string), value: v.into() }
    }

    #[test]
    fn syntax() {
        assert_eq!(parse("Smith  name:DOE"), vec![t(None, "smith"), t(Some("name"), "doe")]);
        assert_eq!(parse(r#"source:"ICU Pilot" "van der""#), vec![t(Some("source"), "icu pilot"), t(None, "van der")]);
        assert_eq!(parse("dob:1950-03 file-id:SYN"), vec![t(Some("dob"), "1950-03"), t(Some("file-id"), "syn")]);
        assert_eq!(parse("time 12:30"), vec![t(None, "time"), t(None, "12:30")]);
        assert!(check(&parse("name:x"), ENCOUNTER_FIELDS, false).unwrap_err().contains("PHI"));
        assert!(check(&parse("color:x"), ENCOUNTER_FIELDS, true).unwrap_err().contains("unknown field color"));
        let mut d = Doc::default();
        d.add("name", "Doe, Jane");
        d.add("source", "Pilot A");
        assert!(d.matches(&parse("name:jane source:pilot"), true));
        assert!(d.matches(&parse("JANE"), true));
        assert!(!d.matches(&parse("jane"), false), "identifier fields only with PHI");
        assert!(!d.matches(&parse("source:jane"), true));
    }
}
