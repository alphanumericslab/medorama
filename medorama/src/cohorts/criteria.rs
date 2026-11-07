//! Criteria: their structured form (criteria, groups, versions 1 and 2) and a search's matches.

use super::*;

/// One criterion.  `kind` says which fields apply:
///
/// | kind | fields | a patient meets it when |
/// |---|---|---|
/// | `age` | `min`, `max` | their age now is in the range (PHI permission: it comes from the date of birth) |
/// | `sex` | `text` | their sex is `text` (`F`, `M`, …) |
/// | `has` | `category` | they have data of that category (`icu`, `holter`, `ecg`, …) |
/// | `encounter` | `from`, `to`, `text` (unit), `icu` | an encounter overlaps the dates, in a unit containing `text`, with (or without) an ICU stay |
/// | `diagnosis` | `text` | a condition's code starts with `text` or its name contains it |
/// | `measurement` | `name`, the value, `from`, `to`, `import` | a result or vital sign named like `name` (in that database) has the value, in the dates |
/// | `column` | `import`, `name` (the column), the value or `text` | a row of that database, for the patient, has the value in that column (or text in it) |
/// | `expression` | `text` | the criteria written as text (expr.rs) |
/// | `medication` | `text`, `from`, `to` | a drug's name contains `text` |
/// | `text` | `text` | any item's text contains `text` (notes only with the PHI permission) |
/// | `database` | `import` | a database (an import) holds data about them |
///
/// The value: `cmp` (`>`, `>=`, `<`, `<=`, `=`, `!=`) and `value` in `unit` (converted to the
/// data's unit: ms and s, µV and mV …), or the range `min`–`max` (inclusive).  `latest`: only
/// each patient's latest value counts; else any value, ever.
///
/// `from` and `to` written in Privacy Mode (`privacy_dates`) are dates as the session was shown
/// them: each patient's times are moved back by their own offset before they are compared
/// (search.rs `Offsets`), whoever evaluates the criterion.  They are stored moved forward by
/// the catalog's offset, so that the gate shows them back as they were typed
/// ([`Criteria::as_written_by`]).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Criterion {
    pub kind: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub not: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub from: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icu: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cmp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub latest: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub privacy_dates: bool,
    /// Of an `expression`: what its text meant when the criteria were saved (the names it uses
    /// bound to their databases), evaluated as such; a later import that makes a name mean
    /// something else does not change a saved cohort (the search says so).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Box<Expr>>,
}

/// A group of criteria: all of them (`and`), any (`or`), or not the one (`not`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Group {
    pub op: String,
    pub of: Vec<Expr>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // a few per query
pub enum Expr {
    Group(Group),
    Leaf(Criterion),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Criteria {
    /// The criteria's format version, 1.
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Expr>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Expr>,
    /// How the session that wrote them read the data: criteria on notes' text, ages and
    /// columns find other patients for a session that reads them otherwise, which may then not
    /// refresh the cohort ([`Criteria::check_reader`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written_as: Option<Reader>,
}

impl Default for Criteria {
    fn default() -> Self {
        Criteria { version: 1, include: None, exclude: vec![], written_as: None }
    }
}

/// How a session reads the data criteria test: identifiers (the PHI permission), Privacy Mode
/// (values as the gate shows them), the text of notes.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq)]
pub struct Reader {
    pub phi: bool,
    pub privacy: bool,
    pub notes: bool,
}

impl Reader {
    pub fn of(user: &User) -> Reader {
        Reader { phi: user.phi, privacy: user.privacy && !user.phi, notes: user.reads_notes() }
    }

    fn words(&self) -> &'static str {
        match (self.phi, self.privacy, self.notes) {
            (true, ..) => "with the PHI permission",
            (_, true, true) => "in Privacy Mode, notes' text shown",
            (_, true, false) => "in Privacy Mode",
            (false, false, _) => "without the PHI permission",
        }
    }
}

impl Criteria {
    /// Whether nothing is asked (everyone is in).
    pub fn is_empty(&self) -> bool {
        self.include.is_none() && self.exclude.is_empty()
    }
    /// The criteria as `user` wrote them, to search or store: how the session reads the data
    /// (`written_as`), criteria written as text bound to what their names mean now
    /// (`resolved`); in Privacy Mode, their dates are dates as shown (`privacy_dates`), stored
    /// moved forward by the catalog's offset (whole days for a date alone, so that it shows
    /// back as typed).
    pub fn as_written_by(&self, c: &Connection, user: &User) -> anyhow::Result<Criteria> {
        let mut q = self.clone();
        q.written_as = Some(Reader::of(user));
        // criteria written as text: bound to what their names mean now, unless sent bound
        fn resolve(e: &mut Expr, c: &Connection, user: &User) -> anyhow::Result<()> {
            match e {
                // (as sent: a saved meaning kept; else what the text means now)
                Expr::Leaf(k) if k.kind == "expression" && k.resolved.is_none() => {
                    k.resolved = Some(Box::new(parse_text(c, user, &k.text)?));
                }
                Expr::Leaf(_) => {}
                Expr::Group(g) => {
                    for x in g.of.iter_mut() {
                        resolve(x, c, user)?;
                    }
                }
            }
            Ok(())
        }
        for e in q.include.iter_mut().chain(q.exclude.iter_mut()) {
            resolve(e, c, user)?;
        }
        if !user.privacy {
            return Ok(q);
        }
        let s = crate::deid::load(c)?;
        let base = s.base_seconds();
        let forward = |t: &str| {
            if !s.hides("dates") || t.trim().is_empty() {
                t.to_string()
            } else if t.trim().len() <= 10 {
                crate::deid::shift_text(t, -(base + 86399).div_euclid(86400) * 86400)
            } else {
                crate::deid::shift_text(t, -base)
            }
        };
        fn walk(e: &mut Expr, f: &dyn Fn(&mut Criterion)) {
            match e {
                Expr::Leaf(k) => f(k),
                Expr::Group(g) => g.of.iter_mut().for_each(|x| walk(x, f)),
            }
        }
        let fix = |k: &mut Criterion| {
            k.privacy_dates = s.hides("dates") && !(k.from.trim().is_empty() && k.to.trim().is_empty());
            k.from = forward(&k.from);
            k.to = forward(&k.to);
        };
        q.include.iter_mut().chain(q.exclude.iter_mut()).for_each(|e| walk(e, &fix));
        Ok(q)
    }

    /// Whether `user` reads the data as the session that wrote the criteria did, as far as
    /// the criteria depend on it (notes' text, ages, columns, criteria written as text): else
    /// a refresh by them would select other patients by the same criteria, and is refused.
    pub fn check_reader(&self, user: &User) -> anyhow::Result<()> {
        let Some(w) = self.written_as else { return Ok(()) };
        let now = Reader::of(user);
        let depends = self.leaves().iter().any(|k| matches!(k.kind.as_str(), "text" | "age" | "column" | "expression"));
        if depends && w != now {
            anyhow::bail!(
                "these criteria were written {}; read {} they would select by other values (notes' text, ages, \
                 columns as shown): refresh the cohort as they were written, or ask its owner",
                w.words(),
                now.words()
            );
        }
        Ok(())
    }

    pub(super) fn leaves(&self) -> Vec<&Criterion> {
        fn walk<'a>(e: &'a Expr, out: &mut Vec<&'a Criterion>) {
            match e {
                Expr::Leaf(k) => out.push(k),
                Expr::Group(g) => g.of.iter().for_each(|x| walk(x, out)),
            }
        }
        let mut out: Vec<&Criterion> = vec![];
        self.include.iter().chain(self.exclude.iter()).for_each(|e| walk(e, &mut out));
        out
    }
}

/// A patient a search found, with why.
#[derive(Debug, Serialize)]
pub struct Match {
    #[serde(flatten)]
    pub patient: PatientRow,
    pub reasons: Vec<String>,
}

pub(super) const MAX_REASONS: usize = 3;
