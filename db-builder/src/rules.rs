//! Rewrite rules (`--rules FILE`, README.md "Rewrite rules"): files' paths and identifiers
//! changed as they are read, before anything is written.
//!
//! ```toml
//! [[path]]                       # a prefix, matched at a folder boundary
//! prefix = "/labs/data"
//! to = "/mnt/data"
//!
//! [[path]]                       # or a regular expression; $1 … its groups
//! regex = '^/old/(\w+)/'
//! to = '/new/$1/'
//!
//! [mrn]                          # also [patient_id], [encounter_id]
//! case = "upper"                 # "upper", "lower" or "keep" (the default)
//! replace = [{ regex = '^PT_', to = 'PT' }]
//! ```
//!
//! Path rules apply, in order, to each file a row names (`file`, `recording_file`) and, with
//! `db-builder rewrite`, to the files a database already lists.  Identifier rules apply to a
//! row's `patient_id`, `mrn` and `encounter_id`: their replacements in order, then the case.

use regex::Regex;
use std::collections::HashMap;

/// The identifier fields rules may change.
pub const IDENTIFIERS: &[&str] = &["patient_id", "mrn", "encounter_id"];
/// The fields that name files.
const FILES: &[&str] = &["file", "recording_file"];

#[derive(Debug, Clone)]
enum PathRule {
    Prefix { from: String, to: String },
    Regex { re: Regex, to: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Case {
    #[default]
    Keep,
    Upper,
    Lower,
}

#[derive(Debug, Clone, Default)]
struct IdRule {
    case: Case,
    replace: Vec<(Regex, String)>,
}

/// Rewrite rules; the default changes nothing.
#[derive(Debug, Clone, Default)]
pub struct Rules {
    paths: Vec<PathRule>,
    identifiers: HashMap<String, IdRule>,
}

fn regex(s: &str, what: &str) -> anyhow::Result<Regex> {
    Regex::new(s).map_err(|e| anyhow::anyhow!("{what}: regex {s:?}: {e}"))
}

impl Rules {
    /// Rules from their TOML text.
    pub fn parse(text: &str) -> anyhow::Result<Rules> {
        let v: toml::Table = text.parse().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut r = Rules::default();
        for (k, val) in &v {
            match k.as_str() {
                "path" => {
                    let list = val.as_array().ok_or_else(|| anyhow::anyhow!("[[path]]: a list of rules"))?;
                    for (i, p) in list.iter().enumerate() {
                        let what = format!("path rule {}", i + 1);
                        let t = p.as_table().ok_or_else(|| anyhow::anyhow!("{what}: a table"))?;
                        let s = |k: &str| t.get(k).and_then(|x| x.as_str()).map(str::to_string);
                        let to = s("to").ok_or_else(|| anyhow::anyhow!("{what}: no `to`"))?;
                        r.paths.push(match (s("prefix"), s("regex")) {
                            (Some(from), None) if !from.is_empty() => {
                                PathRule::Prefix { from: from.trim_end_matches(['/', '\\']).to_string(), to }
                            }
                            (None, Some(re)) => PathRule::Regex { re: regex(&re, &what)?, to },
                            _ => anyhow::bail!("{what}: either `prefix` or `regex`"),
                        });
                    }
                }
                f if IDENTIFIERS.contains(&f) => {
                    let t = val.as_table().ok_or_else(|| anyhow::anyhow!("[{f}]: a table"))?;
                    let mut rule = IdRule::default();
                    for (k, x) in t {
                        match k.as_str() {
                            "case" => {
                                rule.case = match x.as_str() {
                                    Some("upper") => Case::Upper,
                                    Some("lower") => Case::Lower,
                                    Some("keep") => Case::Keep,
                                    _ => anyhow::bail!("[{f}] case: \"upper\", \"lower\" or \"keep\""),
                                }
                            }
                            "replace" => {
                                let list = x.as_array().ok_or_else(|| anyhow::anyhow!("[{f}] replace: a list"))?;
                                for (i, one) in list.iter().enumerate() {
                                    let what = format!("[{f}] replace {}", i + 1);
                                    let re = one.get("regex").and_then(|x| x.as_str());
                                    let to = one.get("to").and_then(|x| x.as_str());
                                    let (Some(re), Some(to)) = (re, to) else {
                                        anyhow::bail!("{what}: {{ regex = '…', to = '…' }}")
                                    };
                                    rule.replace.push((regex(re, &what)?, to.to_string()));
                                }
                            }
                            other => anyhow::bail!("[{f}]: unknown key {other:?} (case, replace)"),
                        }
                    }
                    r.identifiers.insert(f.to_string(), rule);
                }
                other => anyhow::bail!("unknown rule {other:?} (path, patient_id, mrn, encounter_id)"),
            }
        }
        Ok(r)
    }

    /// Rules from a TOML file.
    pub fn read(file: &std::path::Path) -> anyhow::Result<Rules> {
        let text = std::fs::read_to_string(file).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        Rules::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.identifiers.is_empty()
    }

    pub fn has_paths(&self) -> bool {
        !self.paths.is_empty()
    }

    /// A file's path, rewritten by every path rule in order.
    pub fn path(&self, p: &str) -> String {
        let mut s = p.to_string();
        for r in &self.paths {
            s = match r {
                PathRule::Prefix { from, to } => match s.strip_prefix(from.as_str()) {
                    // at a folder boundary: /labs/data is not /labs/database
                    Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("{to}{rest}"),
                    _ => s,
                },
                PathRule::Regex { re, to } => re.replace(&s, to.as_str()).into_owned(),
            };
        }
        s
    }

    /// An identifier of `field` (`patient_id`, `mrn`, `encounter_id`), rewritten.
    pub fn identifier(&self, field: &str, v: &str) -> String {
        let Some(r) = self.identifiers.get(field) else { return v.to_string() };
        let mut s = v.trim().to_string();
        if s.is_empty() {
            return s;
        }
        for (re, to) in &r.replace {
            s = re.replace_all(&s, to.as_str()).into_owned();
        }
        match r.case {
            Case::Keep => s,
            Case::Upper => s.to_uppercase(),
            Case::Lower => s.to_lowercase(),
        }
    }

    /// A row's cells rewritten in place: its files and identifiers.
    pub fn apply(&self, fields: &mut HashMap<&'static str, String>) {
        if self.is_empty() {
            return;
        }
        for f in FILES {
            if let Some(v) = fields.get_mut(f).filter(|v| !v.trim().is_empty()) {
                *v = self.path(v.trim());
            }
        }
        for f in IDENTIFIERS {
            if let Some(v) = fields.get_mut(f) {
                *v = self.identifier(f, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_identifiers() {
        let r = Rules::parse(
            r#"
            [[path]]
            prefix = "/labs/data/"
            to = "/mnt/data"
            [[path]]
            regex = '^/old/(\w+)/'
            to = '/new/$1/'
            [mrn]
            case = "upper"
            replace = [{ regex = '^pt_', to = 'pt' }]
            [patient_id]
            replace = [{ regex = '^PT_(\d+)$', to = 'PT$1' }]
            "#,
        )
        .unwrap();
        assert_eq!(r.path("/labs/data/icu/a.hea"), "/mnt/data/icu/a.hea");
        assert_eq!(r.path("/labs/data"), "/mnt/data");
        assert_eq!(r.path("/labs/database/a.hea"), "/labs/database/a.hea", "a folder boundary");
        assert_eq!(r.path("/old/ward3/x.edf"), "/new/ward3/x.edf");
        assert_eq!(r.path("rel/x.edf"), "rel/x.edf");
        assert_eq!(r.identifier("mrn", " pt_12a "), "PT12A");
        assert_eq!(r.identifier("patient_id", "PT_0042"), "PT0042");
        assert_eq!(r.identifier("patient_id", "PT_X"), "PT_X");
        assert_eq!(r.identifier("encounter_id", "e1"), "e1", "no rule: as it is");
        let mut f: HashMap<&'static str, String> =
            [("file", "/labs/data/a.hea".to_string()), ("mrn", "pt_7".to_string())].into_iter().collect();
        r.apply(&mut f);
        assert_eq!((f["file"].as_str(), f["mrn"].as_str()), ("/mnt/data/a.hea", "PT7"));
    }

    #[test]
    fn mistakes_are_named() {
        for (text, says) in [
            ("[[path]]\nto = 'x'", "either `prefix` or `regex`"),
            ("[[path]]\nregex = '('\nto = 'x'", "regex"),
            ("[mrn]\ncase = 'title'", "case"),
            ("[name]\ncase = 'upper'", "unknown rule"),
        ] {
            let e = Rules::parse(text).unwrap_err().to_string();
            assert!(e.contains(says), "{text}: {e}");
        }
    }
}
