//! Import formats described in a TOML file, column by column: a site adds its own exports (an
//! ECG management system's, a reporting system's ...) without a new build.  Each `<key>.toml`
//! in a folder of `formats_dirs`:
//!
//! ```toml
//! key = "acme-ecg"                       # its name (--format acme-ecg); letters, digits, -
//! label = "ACME ECG export"
//! category = "ecg"                       # the kind of data its rows are (lab, ecg, img, note, ...)
//! about = "One row per ECG, from ACME's export."
//! detect = ["EcgFile", "PatId", "AcqDate"]   # recognised when a table has all these columns
//! required = ["mrn", "file"]             # rows with any of these empty are left out (counted)
//!
//! [[column]]                             # the converted table's columns, in order
//! name = "mrn"                           # a catalog column (mrn, time, file, ...) or one of its own
//! from = "PatId"                         # copied (trimmed)
//!
//! [[column]]
//! name = "time"
//! from = "AcqDate"
//! date = "%m-%d-%Y"                      # read in this form (chrono), written as ISO 8601
//! time_from = "AcqTime"                  # with the time of day of this column
//! time = "%H:%M:%S"
//!
//! [[column]]
//! name = "sex"
//! from = "Gender"
//! map = { MALE = "M", FEMALE = "F" }     # values replaced (others: empty)
//!
//! [[column]]
//! name = "QTc (ms)"                      # a measurement: its unit in its name
//! from = "QTc"
//! number = true                          # kept only when it is a number
//!
//! [[column]]
//! name = "interpretation"
//! join = "Statement/{n}/Text"            # numbered columns, in order of n, joined
//! separator = "\n"
//! ```
//!
//! A column a table lacks is empty (exports differ in their columns); a column not described is
//! left out.  So a format keeps only what it names: identifiers it maps to the catalog's
//! patient columns (`mrn`, `last_name`, `birth_date` ...) are masked in Privacy Mode; it should
//! not keep other identifying columns (staff, account numbers).

use super::{Converter, Format};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declared {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub detect: Vec<String>,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(rename = "column")]
    pub columns: Vec<Column>,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(default)]
    pub from: String,
    /// The form `from`'s dates (or dates and times) are in.
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub time_from: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub map: BTreeMap<String, String>,
    #[serde(default)]
    pub number: bool,
    /// Numbered columns (`{n}` the number), joined in order.
    #[serde(default)]
    pub join: String,
    #[serde(default = "newline")]
    pub separator: String,
}

fn newline() -> String {
    "\n".into()
}

fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 40
        && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && k.starts_with(|c: char| c.is_ascii_lowercase())
}

impl Declared {
    /// A format from its text (`path`: where it was read, for messages).
    pub fn parse(text: &str, path: &Path) -> anyhow::Result<Declared> {
        let mut d: Declared = toml::from_str(text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        d.path = path.to_path_buf();
        let bad = |m: String| anyhow::anyhow!("{}: {m}", path.display());
        if !valid_key(&d.key) {
            return Err(bad(format!("key {:?}: lowercase letters, digits and -, starting with a letter", d.key)));
        }
        if d.key == "auto" || d.key == "none" {
            return Err(bad(format!("key {:?} is reserved", d.key)));
        }
        if !d.category.is_empty() && crate::categories::named(&d.category).is_none() {
            return Err(bad(format!("category {:?}: not one the catalog knows", d.category)));
        }
        if d.columns.is_empty() {
            return Err(bad("no [[column]]".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &d.columns {
            if c.name.trim().is_empty() {
                return Err(bad("a column without a name".into()));
            }
            if !seen.insert(c.name.to_lowercase()) {
                return Err(bad(format!("the column {:?} twice", c.name)));
            }
            if c.from.is_empty() == c.join.is_empty() {
                return Err(bad(format!("column {:?}: either from or join", c.name)));
            }
            if !c.join.is_empty() && !c.join.contains("{n}") {
                return Err(bad(format!("column {:?}: join needs {{n}}, the number in the columns' names", c.name)));
            }
            if !c.time_from.is_empty() && c.date.is_empty() {
                return Err(bad(format!("column {:?}: time_from needs date (the form of the date)", c.name)));
            }
        }
        for r in &d.required {
            if !d.columns.iter().any(|c| c.name.eq_ignore_ascii_case(r)) {
                return Err(bad(format!("required {r:?} is not one of its columns")));
            }
        }
        Ok(d)
    }
}

/// The formats described in these folders (every `*.toml`), each with its file; what cannot be
/// read goes to `problems`.
pub fn load_dirs(dirs: &[PathBuf], problems: &mut Vec<String>) -> Vec<(Declared, PathBuf)> {
    let mut out: Vec<(Declared, PathBuf)> = vec![];
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        files.sort();
        for p in files {
            match std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|t| Declared::parse(&t, &p)) {
                Ok(d) if out.iter().any(|(x, _)| x.key == d.key) => {
                    problems.push(format!("{}: the format {} is defined twice", p.display(), d.key))
                }
                Ok(d) => out.push((d, p)),
                Err(e) => problems.push(e.to_string()),
            }
        }
    }
    out
}

impl Format for Declared {
    fn key(&self) -> &str {
        &self.key
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn category(&self) -> &str {
        &self.category
    }
    fn about(&self) -> String {
        self.about.clone()
    }
    fn origin(&self) -> String {
        self.path.display().to_string()
    }
    fn detect(&self, h: &[String]) -> bool {
        !self.detect.is_empty() && self.detect.iter().all(|c| h.iter().any(|x| x.trim() == c))
    }
    fn start(&self, h: &[String]) -> anyhow::Result<Box<dyn Converter>> {
        if let Some(c) = self.detect.iter().find(|c| !h.iter().any(|x| x.trim() == c.as_str())) {
            anyhow::bail!("not a {} table: it has no column {c}", self.label);
        }
        let at = |name: &str| h.iter().position(|x| x.trim() == name);
        let mut plan = vec![];
        for c in &self.columns {
            let joined = if c.join.is_empty() {
                vec![]
            } else {
                let (pre, post) = c.join.split_once("{n}").unwrap_or((&c.join, ""));
                let mut found: Vec<(u64, usize)> = h
                    .iter()
                    .enumerate()
                    .filter_map(|(i, x)| {
                        let n = x.trim().strip_prefix(pre)?.strip_suffix(post)?;
                        n.parse::<u64>().ok().map(|n| (n, i))
                    })
                    .collect();
                found.sort();
                found.into_iter().map(|(_, i)| i).collect()
            };
            plan.push(Planned {
                col: c.clone(),
                from: if c.from.is_empty() { None } else { at(&c.from) },
                time_from: if c.time_from.is_empty() { None } else { at(&c.time_from) },
                joined,
            });
        }
        let required: Vec<usize> = self
            .required
            .iter()
            .filter_map(|r| self.columns.iter().position(|c| c.name.eq_ignore_ascii_case(r)))
            .collect();
        Ok(Box::new(Rows { plan, required, names: self.columns.iter().map(|c| c.name.clone()).collect() }))
    }
}

struct Planned {
    col: Column,
    from: Option<usize>,
    time_from: Option<usize>,
    joined: Vec<usize>,
}

struct Rows {
    plan: Vec<Planned>,
    required: Vec<usize>,
    names: Vec<String>,
}

/// A date (or a date and time) in `form`, as ISO 8601; `time` (in `time_form`) added.
fn iso(value: &str, form: &str, time: Option<(&str, &str)>) -> String {
    let v = value.trim();
    if v.is_empty() {
        return String::new();
    }
    // a year below 1000 (`%Y` reading `24`): left as written, refused as no time when imported
    let year_ok = |y: i32| y >= 1000;
    if let Ok(dt) = NaiveDateTime::parse_from_str(v, form) {
        if !year_ok(chrono::Datelike::year(&dt)) {
            return v.to_string();
        }
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    let Ok(d) = NaiveDate::parse_from_str(v, form) else { return String::new() };
    if !year_ok(chrono::Datelike::year(&d)) {
        return v.to_string();
    }
    match time.and_then(|(t, f)| NaiveTime::parse_from_str(t.trim(), f).ok()) {
        Some(t) => format!("{} {}", d.format("%Y-%m-%d"), t.format("%H:%M:%S")),
        None if time.is_some() => format!("{} 00:00:00", d.format("%Y-%m-%d")),
        None => d.format("%Y-%m-%d").to_string(),
    }
}

impl Converter for Rows {
    fn headers(&self) -> Vec<String> {
        self.names.clone()
    }

    fn row(&mut self, r: &[String]) -> Result<Vec<String>, String> {
        let cell = |i: Option<usize>| i.and_then(|i| r.get(i)).map(|s| s.trim()).unwrap_or("");
        let mut out = Vec::with_capacity(self.plan.len());
        for p in &self.plan {
            let c = &p.col;
            let mut v = if !p.joined.is_empty() || !c.join.is_empty() {
                p.joined.iter().map(|i| cell(Some(*i))).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(&c.separator)
            } else {
                cell(p.from).to_string()
            };
            if !c.map.is_empty() {
                v = c.map.iter().find(|(k, _)| k.eq_ignore_ascii_case(&v)).map(|(_, x)| x.clone()).unwrap_or_default();
            }
            if !c.date.is_empty() {
                let t = p.time_from.map(|_| (cell(p.time_from), c.time.as_str()));
                v = iso(&v, &c.date, t);
            }
            if c.number && !v.parse::<f64>().is_ok_and(f64::is_finite) {
                v.clear();
            }
            out.push(v);
        }
        if let Some(i) = self.required.iter().find(|i| out[**i].is_empty()) {
            return Err(format!("rows without {}", self.names[*i]));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACME: &str = r#"
key = "acme-ecg"
label = "ACME ECG export"
category = "ecg"
detect = ["EcgFile", "PatId", "AcqDate"]
required = ["mrn", "file"]

[[column]]
name = "mrn"
from = "PatId"

[[column]]
name = "time"
from = "AcqDate"
date = "%m-%d-%Y"
time_from = "AcqTime"
time = "%H:%M:%S"

[[column]]
name = "file"
from = "EcgFile"

[[column]]
name = "sex"
from = "Gender"
map = { MALE = "M", FEMALE = "F" }

[[column]]
name = "QTc (ms)"
from = "QTc"
number = true

[[column]]
name = "interpretation"
join = "Statement/{n}/Text"
separator = " | "
"#;

    fn h(cols: &[&str]) -> Vec<String> {
        cols.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn a_described_format_converts() {
        let f = Declared::parse(ACME, Path::new("acme-ecg.toml")).unwrap();
        let head = h(&[
            "PatId",
            "EcgFile",
            "AcqDate",
            "AcqTime",
            "Gender",
            "QTc",
            "Statement/10/Text",
            "Statement/2/Text",
            "Doctor",
        ]);
        assert!(f.detect(&head) && !f.detect(&h(&["PatId"])));
        let mut c = f.start(&head).unwrap();
        assert_eq!(c.headers(), ["mrn", "time", "file", "sex", "QTc (ms)", "interpretation"]);
        let row = h(&[
            "SYN1",
            "/e/1.xml",
            "01-31-2024",
            "14:05:09",
            "female",
            "438",
            "Abnormal ECG",
            "Sinus rhythm",
            "Dr. Zorbluff",
        ]);
        assert_eq!(
            c.row(&row).unwrap(),
            ["SYN1", "2024-01-31 14:05:09", "/e/1.xml", "F", "438", "Sinus rhythm | Abnormal ECG"]
        );
        let odd = h(&["SYN1", "/e/2.xml", "31/01/2024", "", "unknown", "n/a", "", "", ""]);
        assert_eq!(c.row(&odd).unwrap(), ["SYN1", "", "/e/2.xml", "", "", ""], "what cannot be read is empty");
        let no_file = h(&["SYN1", "", "01-31-2024", "", "", "", "", "", ""]);
        assert_eq!(c.row(&no_file).unwrap_err(), "rows without file");
        assert!(f.start(&h(&["PatId"])).is_err());
    }

    #[test]
    fn mistakes_are_said() {
        let bad = |text: &str| Declared::parse(text, Path::new("x.toml")).unwrap_err().to_string();
        let base = "key = \"x\"\nlabel = \"X\"\n";
        assert!(bad(&format!("{base}[[column]]\nname = \"a\"\n")).contains("either from or join"));
        assert!(bad(&format!("{base}[[column]]\nname = \"a\"\njoin = \"S/n\"\n")).contains("{n}"));
        assert!(bad("key = \"Bad Key\"\nlabel = \"X\"\n[[column]]\nname = \"a\"\nfrom = \"b\"\n").contains("lowercase"));
        assert!(
            bad(&format!("{base}category = \"nope\"\n[[column]]\nname = \"a\"\nfrom = \"b\"\n")).contains("category")
        );
        assert!(
            bad(&format!("{base}required = [\"z\"]\n[[column]]\nname = \"a\"\nfrom = \"b\"\n")).contains("required")
        );
        assert!(bad(&format!("{base}colour = 1\n[[column]]\nname = \"a\"\nfrom = \"b\"\n")).contains("colour"));
        // two files, one key: the second is said, not used
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), ACME).unwrap();
        std::fs::write(dir.path().join("b.toml"), ACME).unwrap();
        std::fs::write(dir.path().join("c.toml"), "not toml [").unwrap();
        let mut problems = vec![];
        let found = load_dirs(&[dir.path().to_path_buf()], &mut problems);
        assert_eq!(found.len(), 1);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }
}
