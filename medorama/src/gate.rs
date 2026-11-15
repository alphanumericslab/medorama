//! The privacy gate (docs/security.md, "The privacy gate"): what leaves the server for a
//! session in Privacy Mode.
//!
//! Privacy Mode is the state of a session, not of the data: Medorama keeps every identifier
//! as imported, and the gate decides, at the moment of sending, how an answer leaves the server.
//! A session is in Privacy Mode unless its user has the PHI permission and turned it off
//! (`server::Auth`); for such a session ("restricted"), every answer passes through here:
//!
//! | Answer | Leaves as |
//! |---|---|
//! | the API's JSON | dates and times moved back, ages above 89 as "90+" (deid.rs); identifiers replaced: imported patient and encounter IDs by their pseudonyms, MRNs and account numbers by placeholders, recordings' paths by tokens ([`Dict`]) |
//! | patients' identities | placeholders: `[Patient Name]`, `[MRN]`, `[Date of Birth]` (written where the page would show them) |
//! | WFDB headers, GE MUSE XML | de-identified (deid.rs), file names replaced by tokens |
//! | DICOM | the header de-identified (dicom.rs), the pixels as they are, marked |
//! | pictures, videos, PDF | as they are, marked: what they show may identify the patient |
//! | any other file | refused |
//!
//! Handlers that know what they send ask the gate ([`file`], [`Restricted::json`], the
//! placeholders); a layer of the server (`server::privacy`) then refuses any answer of a
//! restricted session that the gate did not clear, so that a new endpoint cannot leak by
//! forgetting it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::sql::{Connection, OptionalExtension};
use serde_json::Value;

use crate::deid::Settings;

/// What Privacy Mode writes where an identifier would be.
pub const PATIENT_NAME: &str = "[Patient Name]";
pub const MRN: &str = "[MRN]";
pub const CSN: &str = "[CSN]";
pub const BIRTH_DATE: &str = "[Date of Birth]";
pub const PATIENT_ID: &str = "[Patient ID]";
pub const STAFF_NAME: &str = "[Staff Name]";
pub const FILE_NAME: &str = "[File Name]";
pub const RECORDING: &str = "[Recording]";
pub const WITHHELD: &str = "[Withheld]";
pub const FOLDER: &str = "[Folder]";

/// The header the gate puts on files whose content it cannot inspect (pictures, videos, PDF,
/// DICOM pixels): the page shows its warning on them.
pub const MAY_HOLD_PHI: &str = "x-medorama-may-contain-phi";
/// The (internal) header with which the gate clears an answer of a restricted session; the
/// server's last layer removes it, and refuses answers without it.
pub const CLEARED: &str = "x-medorama-gate";

// ------------------------------------------------------------------ the dictionary

/// Every identifier Medorama knows, with what Privacy Mode writes instead: the databases'
/// patient IDs (their patients' pseudonyms), MRNs (placeholders), recordings' paths and file
/// names (tokens).  Built from each connected database's persons and recordings as kept in the
/// workspace, kept while they do not change.
pub struct Dict {
    /// The version of the identifiers it was built at (`data_version.identifiers`).
    pub version: i64,
    /// Whole values, lower case: a string equal to one of these (case ignored) is replaced.
    whole: HashMap<String, String>,
    /// Words, lower case: identifiers of 4 characters or more with a digit, replaced inside any
    /// text (case ignored) …
    words: HashMap<String, String>,
    /// … and by their digits ([`normal`]: `SYN-0004`, `syn0004` and `SYN4` are one).
    normal: HashMap<String, String>,
    /// The words of every linked patient's names (lower case, 3 letters or more): replaced in
    /// text that may be about any patient ([`Dict::scrub_any_names`]).
    names: std::collections::HashSet<String>,
    /// Recordings' paths (by their tokens) and the folders they lie in ([`FOLDER`]), replaced
    /// inside any text (an error that names a file) …
    paths: HashMap<String, String>,
    /// … whose lengths, longest first.
    path_lengths: Vec<usize>,
    /// Patients' pseudonyms, by Medorama's patient number.
    pub pseudonyms: HashMap<i64, String>,
    /// Each patient's date draw (Privacy Mode's own offset), by patient number …
    pub draws: HashMap<i64, f64>,
    /// … and by the pseudonyms that name them.
    pub draws_by_pseudonym: HashMap<String, f64>,
    /// Recordings' tokens and their real paths.
    pub recordings: HashMap<String, String>,
    /// … and their patient's date draw (a recording linked to an encounter).
    pub draws_by_recording: HashMap<String, f64>,
}

/// A word worth replacing inside text: long enough not to be a common short code.
fn wordlike(s: &str) -> bool {
    s.chars().count() >= 4
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// An identifier as compared by its digits: lower case, without separators (`-`, `_`, `.`,
/// `/`, spaces), each run of digits without its leading zeros; none for a form too short to be
/// told from other numbers (fewer than 4 characters, or fewer than 6 digits alone: a year).
fn normal(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut in_digits = false;
    for ch in s.trim().chars().filter(|c| !matches!(c, '-' | '_' | '.' | '/' | ' ')) {
        if !ch.is_ascii_alphanumeric() {
            return None;
        }
        if ch.is_ascii_digit() {
            // a run's leading zeros dropped
            if !in_digits && ch == '0' {
                continue;
            }
            in_digits = true;
        } else {
            in_digits = false;
        }
        out.push(ch.to_ascii_lowercase());
    }
    let digits = out.chars().all(|c| c.is_ascii_digit());
    let long = if digits { out.len() >= 6 } else { out.len() >= 4 };
    (long && out.chars().any(|c| c.is_ascii_digit())).then_some(out)
}

impl Dict {
    pub fn build(c: &Connection, cfg: &crate::config::Config, version: i64) -> anyhow::Result<Dict> {
        let mut d = Dict {
            version,
            whole: HashMap::new(),
            words: HashMap::new(),
            normal: HashMap::new(),
            names: Default::default(),
            paths: HashMap::new(),
            path_lengths: vec![],
            pseudonyms: HashMap::new(),
            draws: HashMap::new(),
            draws_by_pseudonym: HashMap::new(),
            recordings: HashMap::new(),
            draws_by_recording: HashMap::new(),
        };
        let rows: Vec<(i64, String, Option<f64>)> = c
            .prepare("SELECT id, COALESCE(pseudonym, ''), date_draw FROM patients")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        for (id, ps, draw) in rows {
            if let Some(dr) = draw {
                d.draws.insert(id, dr);
                d.draws_by_pseudonym.insert(ps.clone(), dr);
            }
            d.pseudonyms.insert(id, ps);
        }
        for s in crate::sources::list(c)? {
            for k in crate::persons::all_of(cfg, c, s.id)? {
                let ps = d.pseudonyms.get(&k.patient).cloned().unwrap_or_else(|| PATIENT_ID.into());
                d.add(&k.patient_id, ps);
                d.add(&k.mrn, MRN.into());
                for w in name_words(&[k.last_name, k.first_name]) {
                    if w.chars().count() >= 3 && w.chars().all(char::is_alphabetic) {
                        d.names.insert(w.to_lowercase());
                    }
                }
            }
            if crate::recordings::count(cfg, c, s.id)? == 0 {
                continue;
            }
            for r in crate::recordings::of_database(cfg, c, &s)? {
                let token = recording_token(r.id);
                d.whole.insert(r.path.to_lowercase(), token.clone());
                d.whole.insert(r.name.to_lowercase(), RECORDING.into());
                // its folders: a folder may be named after the patient
                for dir in Path::new(&r.path).ancestors().skip(1) {
                    let dir = dir.to_string_lossy();
                    if dir.len() > 1 && dir.contains(['/', '\\']) {
                        d.paths.entry(dir.to_string()).or_insert_with(|| FOLDER.into());
                    }
                }
                d.paths.insert(r.path.clone(), token.clone());
                if let Some(dr) = r.patient.and_then(|p| d.draws.get(&p).copied()) {
                    d.draws_by_recording.insert(token.clone(), dr);
                }
                d.recordings.insert(token, r.path);
            }
        }
        let mut lengths: Vec<usize> = d.paths.keys().map(|k| k.len()).collect();
        lengths.sort_unstable_by(|a, b| b.cmp(a));
        lengths.dedup();
        d.path_lengths = lengths;
        Ok(d)
    }

    /// An identifier and what replaces it: as a whole value always; inside text when it is
    /// [`wordlike`], case ignored and by its digits ([`normal`]).
    fn add(&mut self, from: &str, to: String) {
        let from = from.trim().to_lowercase();
        if from.is_empty() {
            return;
        }
        if wordlike(&from) {
            self.words.insert(from.clone(), to.clone());
            if let Some(n) = normal(&from) {
                self.normal.insert(n, to.clone());
            }
        }
        self.whole.insert(from, to);
    }

    /// What replaces a word of text, if it is an identifier (case ignored, by its digits).
    fn word(&self, w: &str) -> Option<&String> {
        let lower = w.to_ascii_lowercase();
        self.words.get(&lower).or_else(|| normal(&lower).and_then(|n| self.normal.get(&n)))
    }

    /// Text with every identifier it holds replaced; `None` when nothing changes.
    pub fn scrub(&self, s: &str) -> Option<String> {
        let t = s.trim().to_lowercase();
        if let Some(to) = self.whole.get(&t).or_else(|| normal(&t).and_then(|n| self.normal.get(&n))) {
            return Some(to.clone());
        }
        self.scrub_words(s)
    }

    /// Every linked patient's names (whole words, case ignored) replaced by [`PATIENT_NAME`]:
    /// for text that may be about any patient (a recording's label, a plug-in's summary, the
    /// reasons of a cohort search); `None` when nothing changes.
    pub fn scrub_any_names(&self, s: &str) -> Option<String> {
        if self.names.is_empty() {
            return None;
        }
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        let mut changed = false;
        let mut it = s.char_indices().peekable();
        while let Some((i, ch)) = it.next() {
            if !ch.is_alphabetic() || s[..i].chars().next_back().is_some_and(char::is_alphanumeric) {
                continue;
            }
            let mut end = i + ch.len_utf8();
            while let Some(&(j, c)) = it.peek() {
                if !c.is_alphabetic() {
                    break;
                }
                end = j + c.len_utf8();
                it.next();
            }
            if s[end..].chars().next().is_some_and(|c| c.is_ascii_digit()) {
                continue;
            }
            if self.names.contains(&s[i..end].to_lowercase()) {
                out.push_str(&s[last..i]);
                out.push_str(PATIENT_NAME);
                last = end;
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        out.push_str(&s[last..]);
        Some(out)
    }

    /// Identifiers inside text replaced (not a value that is one whole: a number may equal a
    /// short ID), and recordings' paths and folders; `None` when nothing changes.
    pub fn scrub_words(&self, s: &str) -> Option<String> {
        let p = self.scrub_paths(s);
        let s = p.as_deref().unwrap_or(s);
        if self.words.is_empty() {
            return p.clone();
        }
        let mut out = String::with_capacity(s.len());
        let mut changed = false;
        let mut last = 0;
        let b = s.as_bytes();
        let mut i = 0;
        let word = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
        while i < b.len() {
            if !word(b[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < b.len() && word(b[i]) {
                i += 1;
            }
            // the whole run (an ID with dashes), else each alphanumeric part of it
            let run = s[start..i].trim_matches(['-', '_']);
            let run_start = start + s[start..i].find(run).unwrap_or(0);
            if let Some(to) = self.word(run) {
                out.push_str(&s[last..run_start]);
                out.push_str(to);
                last = run_start + run.len();
                changed = true;
                continue;
            }
            let mut j = start;
            while j < i {
                if !b[j].is_ascii_alphanumeric() {
                    j += 1;
                    continue;
                }
                let k0 = j;
                while j < i && b[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                if let Some(to) = self.word(&s[k0..j]) {
                    out.push_str(&s[last..k0]);
                    out.push_str(to);
                    last = j;
                    changed = true;
                }
            }
        }
        if !changed {
            return p.clone();
        }
        out.push_str(&s[last..]);
        Some(out)
    }

    /// Recordings' paths and their folders replaced wherever they appear in text (whole path
    /// components only), the longest first; `None` when none does.
    fn scrub_paths(&self, s: &str) -> Option<String> {
        if self.paths.is_empty() || !s.contains(['/', '\\']) {
            return None;
        }
        let part = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.');
        let mut out = String::with_capacity(s.len());
        let mut last = 0;
        let mut i = 0;
        while i < s.len() {
            let at_start = s[..i].chars().next_back().is_none_or(|c| !part(c));
            let found = at_start
                .then(|| {
                    self.path_lengths.iter().find_map(|&n| {
                        let key = s.get(i..i + n)?;
                        let to = self.paths.get(key)?;
                        s[i + n..].chars().next().is_none_or(|c| !part(c)).then_some((n, to))
                    })
                })
                .flatten();
            if let Some((_, to)) = found {
                out.push_str(&s[last..i]);
                out.push_str(to);
            }
            let found = found.map(|(n, _)| n);
            match found {
                Some(n) => {
                    i += n;
                    last = i;
                }
                None => i += s[i..].chars().next().map_or(1, char::len_utf8),
            }
        }
        if last == 0 {
            return None;
        }
        out.push_str(&s[last..]);
        Some(out)
    }
}

/// How a recording is named to a restricted session (its path never leaves the server).
/// An event's text (an EDF+ annotation) when Privacy Mode hides notes' text.
pub const ANNOTATION: &str = "[Annotation]";

pub fn recording_token(id: i64) -> String {
    format!("mo-rec:{id}")
}

/// The real path behind a recording token, if it is one.
pub fn recording_path(d: &Dict, token: &str) -> Option<String> {
    d.recordings.get(token).cloned()
}

// ------------------------------------------------------------------ a restricted session

/// What the gate needs for a restricted session.
pub struct Restricted {
    pub settings: Settings,
    pub dict: Arc<Dict>,
    /// Medorama's secret (DICOM UIDs), wiped when dropped.
    pub secret: zeroize::Zeroizing<Vec<u8>>,
}

/// Keys whose strings name staff (authors, signers, overreaders).
const STAFF_KEYS: &[&str] = &["author", "signed_by", "overreader", "confirmed_by", "editor", "physician"];

/// Keys whose strings are addresses inside Medorama (internal numbers only): never scrubbed.
const ADDRESSES: &[&str] = &["url", "src", "href", "link", "hash", "viewer"];

/// Keys whose value names the patient an object is about, once identifiers are replaced: a
/// pseudonym (the patient's or an encounter's), or Medorama's patient number.
const PATIENT_KEYS: &[&str] = &["pseudonym", "patient_id", "encounter_id", "patient"];

/// Keys whose strings (and everything under them) are free text that may be about any patient:
/// a recording's label and error, a plug-in's summary, the reasons of a cohort search.  Every
/// linked patient's names are replaced in them ([`Dict::scrub_any_names`]), and contact
/// details ([`safe_harbor`]).
const FREE_KEYS: &[&str] = &["label", "error", "summary", "reasons", "why", "recording_name"];

impl Restricted {
    /// The gate narrowed to one patient: their dates move by their own offset too.
    pub fn for_patient(&self, patient: i64) -> Restricted {
        Restricted {
            settings: self.settings.for_draw(self.dict.draws.get(&patient).copied()),
            dict: self.dict.clone(),
            secret: self.secret.clone(),
        }
    }

    /// The gate narrowed to a recording's patient (its token, `mo-rec:<id>`).
    pub fn for_recording(&self, token: &str) -> Restricted {
        Restricted {
            settings: self.settings.for_draw(self.dict.draws_by_recording.get(token).copied()),
            dict: self.dict.clone(),
            secret: self.secret.clone(),
        }
    }

    /// The settings for an entry's key (`labs:<patient>:…`): its patient's, so that the key
    /// reads the same on every page and comes back through [`Restricted::real_key`].
    pub fn key_settings(&self, key: &str) -> Settings {
        let p = key.split(':').nth(1).and_then(|e| e.parse::<i64>().ok());
        self.settings.for_draw(p.and_then(|p| self.dict.draws.get(&p).copied()))
    }

    /// An entry's key as Medorama knows it, from the key this gate sent.
    pub fn real_key(&self, key: &str) -> String {
        real_key(key, &self.key_settings(key))
    }

    /// The settings for the patient an answer's object names, if it names one.
    fn settings_of(&self, m: &serde_json::Map<String, Value>) -> Option<Settings> {
        let draw = PATIENT_KEYS.iter().find_map(|k| match m.get(*k) {
            Some(Value::String(s)) => self.dict.draws_by_pseudonym.get(s.as_str()).copied(),
            Some(Value::Number(n)) if *k == "patient" => n.as_i64().and_then(|p| self.dict.draws.get(&p).copied()),
            _ => None,
        });
        // a recording: by its token (its path, as scrubbed: `mo-rec:<id>`), else through its
        // encounters (their pseudonyms, or objects that hold one)
        let draw = draw.or_else(|| self.dict.draws_by_recording.get(m.get("path")?.as_str()?).copied());
        let draw = draw.or_else(|| {
            m.get("encounters")?.as_array()?.iter().find_map(|e| {
                let id = match e {
                    Value::String(s) => s.as_str(),
                    e => e.get("encounter_id")?.as_str()?,
                };
                self.dict.draws_by_pseudonym.get(id).copied()
            })
        });
        draw.map(|d| self.settings.for_draw(Some(d)))
    }

    /// An API answer as a restricted session gets it.  Each object that names a patient has its
    /// dates moved by that patient's own offset; the rest by the session's (its patient's on a
    /// patient's page: [`Restricted::for_patient`]).
    pub fn json(&self, v: &mut Value) {
        // identifiers first, as they are stored (a recording's name holds its time), then dates
        self.scrub_value(v, "", false);
        crate::deid::answer_with(v, &self.settings, &|m| self.settings_of(m));
    }

    /// `free`: under one of [`FREE_KEYS`].
    fn scrub_value(&self, v: &mut Value, key: &str, free: bool) {
        if let Value::String(s) = v {
            if let Some(k) = shown_key(s, &self.key_settings(s)) {
                *s = k;
                return;
            }
        }
        let free = free || FREE_KEYS.contains(&key);
        match v {
            // staff names (a note's author): withheld
            Value::String(s) if STAFF_KEYS.contains(&key) && !s.trim().is_empty() => *s = STAFF_NAME.into(),
            Value::String(s) if !ADDRESSES.contains(&key) => {
                if let Some(n) = self.dict.scrub(s) {
                    *s = n;
                }
                // "Confirmed by …": staff named in text
                if let Some(n) = crate::deid::staff_phrases(s) {
                    *s = n;
                }
                if free {
                    if let Some(n) = self.dict.scrub_any_names(s) {
                        *s = n;
                    }
                    if let Some(n) = safe_harbor(s) {
                        *s = n;
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|x| self.scrub_value(x, key, free)),
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    self.scrub_value(x, k, free);
                }
            }
            _ => {}
        }
    }

    /// Text (a header, a note, a CSV cell) as a restricted session gets it: identifiers
    /// replaced, contact details too ([`safe_harbor`]), dates moved back.
    pub fn text(&self, s: &str) -> String {
        let s = self.dict.scrub(s).unwrap_or_else(|| s.to_string());
        let s = safe_harbor(&s).unwrap_or(s);
        self.settings.shift_text(&s)
    }

    /// A patient's own names, MRNs and date of birth replaced in text written about them (a
    /// note), besides [`Restricted::text`], and staff named in it ("Confirmed by …"): free text
    /// may still name them in other ways, so the page keeps its warning on it.
    pub fn note(&self, s: &str, names: &[String], birth: &str) -> String {
        // before dates are moved: the date of birth as written
        let s = self.text(&own_words(s, names, birth));
        crate::deid::staff_phrases(&s).unwrap_or(s)
    }

    /// Free text about a patient (a note's, a report's, any entry's text or long value) as a
    /// restricted session gets it: [`WITHHELD`] when Privacy Mode withholds the text of notes and
    /// reports, else [`Restricted::note`].
    pub fn entry_text(&self, s: &str, names: &[String], birth: &str) -> String {
        if s.trim().is_empty() {
            return s.to_string();
        }
        if self.settings.hides("notes") {
            return WITHHELD.into();
        }
        self.note(s, names, birth)
    }

    /// Free text inside a file (a WFDB header's comment, a MUSE statement, a DICOM text value)
    /// as [`Restricted::note`] writes it, except that a value equal to a short identifier is
    /// kept (in a file, it may be a number).
    pub fn free_text(&self, s: &str, names: &[String], birth: &str) -> String {
        let s = own_words(s, names, birth);
        let s = self.dict.scrub_words(&s).unwrap_or(s);
        let s = safe_harbor(&s).unwrap_or(s);
        self.settings.shift_text(&s)
    }

    /// What names a patient in their own notes: their names, and their date of birth as
    /// written (the server's copy; never sent).
    pub fn identity_words(c: &Connection, cfg: &crate::config::Config, patient: i64) -> (Vec<String>, String) {
        let records = crate::persons::records_of(cfg, c, patient, None).unwrap_or_default();
        let mut names = vec![];
        let mut birth = String::new();
        for k in records {
            names.push(k.last_name);
            names.push(k.first_name);
            if birth.is_empty() {
                birth = k.birth_date;
            }
        }
        (name_words(&names), birth)
    }

    pub fn pseudonym(&self, patient: i64) -> String {
        self.dict.pseudonyms.get(&patient).cloned().unwrap_or_else(|| PATIENT_ID.to_string())
    }
}

/// The words that name a patient, from their names as stored: each word (split on spaces), then
/// the parts of compound ones (`Smith-Jones`, `O'Brien`), each once, the longest first.
pub fn name_words(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for n in names {
        for w in n.split_whitespace() {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric());
            let parts = w.split(['-', '\'', '\u{2019}']).filter(|p| !p.is_empty());
            for x in std::iter::once(w).chain(parts) {
                if !x.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(x)) {
                    out.push(x.to_string());
                }
            }
        }
    }
    // a compound name before its parts
    out.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    out
}

/// A patient's own names (whole words) and date of birth replaced in text.
pub fn own_words(s: &str, names: &[String], birth: &str) -> String {
    let mut out = if birth.is_empty() { s.to_string() } else { s.replace(birth, BIRTH_DATE) };
    for n in names.iter().filter(|n| n.chars().count() >= 2) {
        out = replace_word(&out, n, PATIENT_NAME);
    }
    out
}

/// A patient's own names (whole words) and date of birth replaced in every string of an answer
/// about them (their notes, reports, the titles of entries), and contact details
/// ([`safe_harbor`]).
pub fn scrub_names(v: &mut Value, names: &[String], birth: &str) {
    fn walk(v: &mut Value, key: &str, names: &[String], birth: &str) {
        match v {
            Value::String(s) if !ADDRESSES.contains(&key) => {
                let out = own_words(s, names, birth);
                let out = safe_harbor(&out).unwrap_or(out);
                if out != *s {
                    *s = out;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|x| walk(x, key, names, birth)),
            Value::Object(m) => m.iter_mut().for_each(|(k, x)| walk(x, k, names, birth)),
            _ => {}
        }
    }
    walk(v, "", names, birth);
}

/// An entry's key as Privacy Mode sends it: the keys of a day's laboratory results and vital
/// signs hold their real time (`labs:<patient>:<unix time>`, `vit:<patient>:<day>`); they
/// leave with it moved back (`vit:` keys as `vit:<patient>:t<moved start of the day>`), and
/// come back through [`real_key`].
pub fn shown_key(key: &str, s: &Settings) -> Option<String> {
    let off = if s.hides("dates") { s.offset_seconds() } else { return None };
    let (kind, rest) = key.split_once(':')?;
    let (enc, when) = rest.split_once(':')?;
    enc.parse::<i64>().ok()?;
    match kind {
        "labs" => Some(format!("labs:{enc}:{}", when.parse::<f64>().ok()? - off as f64)),
        "vit" if !when.starts_with('t') => Some(format!("vit:{enc}:t{}", when.parse::<i64>().ok()? * 86400 - off)),
        _ => None,
    }
}

/// An entry's key as Medorama knows it, from the key Privacy Mode sent ([`shown_key`]).
pub fn real_key(key: &str, s: &Settings) -> String {
    let off = if s.hides("dates") { s.offset_seconds() } else { return key.to_string() };
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    match parts.as_slice() {
        ["labs", enc, when] => {
            when.parse::<f64>().map_or(key.to_string(), |t| format!("labs:{enc}:{}", t + off as f64))
        }
        ["vit", enc, when] => when
            .strip_prefix('t')
            .and_then(|t| t.parse::<i64>().ok())
            .map_or(key.to_string(), |t| format!("vit:{enc}:{}", (t + off).div_euclid(86400))),
        _ => key.to_string(),
    }
}

/// `text` with every whole-word, case-insensitive occurrence of `word` replaced, compared
/// character by character (lower-casing may change a text's length: `İ`, `ẞ`).
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let w: Vec<char> = word.chars().collect();
    if w.is_empty() {
        return text.to_string();
    }
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i + w.len() <= chars.len() {
        let at = chars[i].0;
        let end = chars.get(i + w.len()).map_or(text.len(), |c| c.0);
        let before = i == 0 || !chars[i - 1].1.is_alphanumeric();
        let after = chars.get(i + w.len()).is_none_or(|c| !c.1.is_alphanumeric());
        if before && after && w.iter().enumerate().all(|(k, &c)| same(chars[i + k].1, c)) {
            out.push_str(&text[last..at]);
            out.push_str(with);
            last = end;
            i += w.len();
            continue;
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

// ------------------------------------------------------------------ Safe Harbor's other identifiers

/// What Privacy Mode writes where free text holds contact details or numbers that identify
/// (HIPAA Safe Harbor's telephone numbers, e-mail addresses, social security numbers, web
/// addresses, street addresses, medical record numbers after their label).
pub const PHONE: &str = "[Phone]";
pub const EMAIL: &str = "[Email]";
pub const SSN: &str = "[SSN]";
pub const URL: &str = "[URL]";
pub const ADDRESS: &str = "[Address]";

/// The words that end a street address (`12 Elm Street`, `4 Oak Ave.`).
const STREETS: &[&str] = &[
    "street",
    "st",
    "avenue",
    "ave",
    "road",
    "rd",
    "boulevard",
    "blvd",
    "lane",
    "ln",
    "drive",
    "court",
    "ct",
    "way",
    "place",
    "pl",
    "terrace",
    "circle",
    "parkway",
    "pkwy",
    "highway",
    "hwy",
    "square",
    "sq",
    "alley",
];

fn digit_run(b: &[u8], at: usize) -> usize {
    b[at.min(b.len())..].iter().take_while(|c| c.is_ascii_digit()).count()
}

/// A web address at `at` (`http://…`, `https://…`, `www.…`): where it ends.
fn url_at(s: &str, at: usize) -> Option<usize> {
    let rest = &s[at..];
    let lower: String = rest.chars().take(8).collect::<String>().to_ascii_lowercase();
    if !["http://", "https://", "www."].iter().any(|p| lower.starts_with(p)) {
        return None;
    }
    let len =
        rest.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '(' | ')')).unwrap_or(rest.len());
    let t = rest[..len].trim_end_matches(['.', ',', ';', ':', '!', '?']);
    (t.len() > 4).then_some(at + t.len())
}

/// An e-mail address starting at `at`: where it ends.
fn email_at(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    let local = b[at..].iter().take_while(|c| c.is_ascii_alphanumeric() || b"._%+-".contains(c)).count();
    if local == 0 || b.get(at + local) != Some(&b'@') {
        return None;
    }
    let d0 = at + local + 1;
    let dom = b[d0..].iter().take_while(|c| c.is_ascii_alphanumeric() || b".-".contains(c)).count();
    let domain = s[d0..d0 + dom].trim_end_matches(['.', '-']);
    let tld = domain.rsplit('.').next().unwrap_or("");
    (domain.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
        .then_some(d0 + domain.len())
}

/// A social security number at `at` (`123-45-6789`): where it ends.
fn ssn_at(b: &[u8], at: usize) -> Option<usize> {
    let ok = digit_run(b, at) == 3
        && b.get(at + 3) == Some(&b'-')
        && digit_run(b, at + 4) == 2
        && b.get(at + 6) == Some(&b'-')
        && digit_run(b, at + 7) == 4;
    ok.then_some(at + 11)
}

/// A telephone number at `at`: `(555) 123-4567`, `555-123-4567`, `555.123.4567`,
/// `555 123 4567`, with `+1 ` or `1-` before them, or `+` and 8 to 15 digits in groups
/// (`+44 20 7946 0958`): where it ends.
fn phone_at(b: &[u8], at: usize) -> Option<usize> {
    let sep = |i: usize| matches!(b.get(i), Some(b'-' | b'.' | b' '));
    let mut i = at;
    // a country code
    if b.get(i) == Some(&b'+') {
        let n = digit_run(b, i + 1);
        if (1..=3).contains(&n) && sep(i + 1 + n) {
            let after = i + 2 + n;
            // the North American form after it, else digits in groups
            if let Some(e) = phone_at(b, after).filter(|_| n == 1) {
                return Some(e);
            }
            let (mut j, mut digits) = (i + 1, 0);
            loop {
                let k = digit_run(b, j);
                if k == 0 {
                    break;
                }
                digits += k;
                j += k;
                if !(sep(j) && digit_run(b, j + 1) > 0) {
                    break;
                }
                j += 1;
            }
            return ((8..=15).contains(&digits)).then_some(j);
        }
        return None;
    }
    if b.get(i) == Some(&b'1') && b.get(i + 1) == Some(&b'-') && digit_run(b, i + 2) == 3 {
        i += 2;
    }
    let after_area = if b.get(i) == Some(&b'(') && digit_run(b, i + 1) == 3 && b.get(i + 4) == Some(&b')') {
        let j = i + 5;
        if b.get(j) == Some(&b' ') {
            j + 1
        } else {
            j
        }
    } else if digit_run(b, i) == 3 && sep(i + 3) {
        i + 4
    } else {
        return None;
    };
    let ok = digit_run(b, after_area) == 3 && sep(after_area + 3) && digit_run(b, after_area + 4) == 4;
    ok.then_some(after_area + 8)
}

/// A street address at `at` (`12 Elm Street`, `4410 Old Mill Rd.`, `P.O. Box 77`): where it
/// ends.
fn address_at(s: &str, at: usize) -> Option<usize> {
    let b = s.as_bytes();
    let lower = s[at..].chars().take(9).collect::<String>().to_ascii_lowercase();
    for p in ["p.o. box ", "po box ", "p.o.box "] {
        if lower.starts_with(p) {
            let n = digit_run(b, at + p.len());
            return (n > 0).then_some(at + p.len() + n);
        }
    }
    let n = digit_run(b, at);
    if !(1..=6).contains(&n) || b.get(at + n) != Some(&b' ') {
        return None;
    }
    let mut i = at + n + 1;
    for words in 0..5 {
        let len = s[i..].chars().take_while(|c| c.is_alphabetic()).map(char::len_utf8).sum::<usize>();
        if len == 0 {
            return None;
        }
        let word = &s[i..i + len];
        let end = i + len;
        let ends = !s[end..].chars().next().is_some_and(char::is_alphanumeric);
        if words > 0 && ends && STREETS.contains(&word.to_lowercase().as_str()) {
            let end = if b.get(end) == Some(&b'.') { end + 1 } else { end };
            return Some(end);
        }
        // the street's name: capitalized words
        if !word.chars().next().is_some_and(char::is_uppercase) || b.get(end) != Some(&b' ') {
            return None;
        }
        i = end + 1;
    }
    None
}

/// A medical record number after its label (`MRN: 0012345`, `MR# 77-1`, `medical record
/// number 4471`): where its value starts and ends.
fn labelled_mrn_at(s: &str, at: usize) -> Option<(usize, usize)> {
    let lower = s[at..].chars().take(24).collect::<String>().to_ascii_lowercase();
    let label =
        ["medical record number", "mrn", "mr#", "mr no", "mr number"].into_iter().find(|l| lower.starts_with(l))?;
    let b = s.as_bytes();
    let mut i = at + label.len();
    if b.get(i).is_some_and(|c| c.is_ascii_alphanumeric()) && !label.ends_with('#') {
        return None;
    }
    while b.get(i).is_some_and(|c| matches!(c, b' ' | b':' | b'#' | b'.' | b'=')) {
        i += 1;
    }
    let len = b[i..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'-').count();
    let v = &s[i..i + len];
    (len >= 3 && v.chars().any(|c| c.is_ascii_digit())).then_some((i, i + len))
}

/// Free text with telephone numbers, e-mail and web addresses, social security numbers,
/// street addresses and labelled medical record numbers replaced (Privacy Mode); `None` when
/// none is found.  Rules, not understanding: what they miss, the warning on free text covers.
pub fn safe_harbor(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        let prev = s[..i].chars().next_back();
        let boundary = prev.is_none_or(|c| !c.is_alphanumeric());
        let numeric_boundary = boundary && !matches!(prev, Some('/' | '.' | '-' | ':' | '+'));
        let found: Option<(usize, usize, &str)> = if !boundary {
            None
        } else if let Some(e) = url_at(s, i) {
            Some((i, e, URL))
        } else if let Some(e) = email_at(s, i) {
            Some((i, e, EMAIL))
        } else if let Some((v, e)) = labelled_mrn_at(s, i) {
            Some((v, e, MRN))
        } else if !numeric_boundary {
            None
        } else if let Some(e) = ssn_at(b, i).filter(|e| digit_run(b, *e) == 0) {
            Some((i, e, SSN))
        } else if let Some(e) = phone_at(b, i).filter(|e| digit_run(b, *e) == 0) {
            Some((i, e, PHONE))
        } else {
            address_at(s, i).map(|e| (i, e, ADDRESS))
        };
        match found {
            Some((from, end, to)) => {
                out.push_str(&s[last..from]);
                out.push_str(to);
                last = end;
                i = end;
            }
            None => i += s[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if last == 0 {
        return None;
    }
    out.push_str(&s[last..]);
    Some(out)
}

/// Medorama's secret (32 random bytes, made once): it keys the patient index's hashes
/// (persons.rs), the tokens of entries and visits, file tokens and DICOM UIDs' replacements.
/// It is kept wrapped by the master key like a data key (owner [`crate::workspace::GATE_OWNER`],
/// in the `keys` table), never in plain text: with it, a copy of the control store would let
/// medical record numbers, which are few, be tried one by one against the index's hashes.  A
/// secret an earlier version kept in plain text (the `gate_secret` setting) is wrapped at its
/// first use, and its setting deleted.  Never sent anywhere.
pub fn secret(c: &Connection) -> anyhow::Result<zeroize::Zeroizing<Vec<u8>>> {
    use crate::workspace::{self, Key, GATE_OWNER};
    let m = workspace::loaded()?;
    if let Some(k) = workspace::key(c, m, GATE_OWNER)? {
        return Ok(k.bytes());
    }
    let plain: Option<String> =
        c.query_row("SELECT value FROM settings WHERE key = 'gate_secret'", [], |r| r.get(0)).optional()?;
    let old = plain.and_then(|v| serde_json::from_str::<String>(&v).ok()).and_then(|h| from_hex(&h));
    let k = match old.as_deref().and_then(|b| Key::from_bytes(b)) {
        Some(k) => k,
        None => Key::random()?,
    };
    // another connection may have kept one first: the one kept
    let k = workspace::keep_key(c, m, GATE_OWNER, &k)?;
    c.execute("DELETE FROM settings WHERE key = 'gate_secret'", [])?;
    Ok(k.bytes())
}

fn from_hex(h: &str) -> Option<zeroize::Zeroizing<Vec<u8>>> {
    (h.len() == 64)
        .then(|| (0..32).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect::<Option<Vec<u8>>>())
        .flatten()
        .map(zeroize::Zeroizing::new)
}

// ------------------------------------------------------------------ files

/// File names as a restricted session sees them: `mo-f0003.dcm`, the n-th file of an
/// attachment (its own file first, then a WFDB record's signal files or a folder's files).
pub fn file_token(index: usize, name: &str) -> String {
    let ext =
        Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    format!("mo-f{index:04}{ext}")
}

/// The index a file token names.
pub fn token_index(token: &str) -> Option<usize> {
    let rest = token.strip_prefix("mo-f")?;
    rest.get(..4)?.parse().ok().filter(|_| rest.len() == 4 || rest[4..].starts_with('.'))
}

/// What the gate does with a file, by its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A WFDB record's header: de-identified, its file names replaced.
    WfdbHeader,
    /// A WFDB record's samples (a `.dat` file a header of the same attachment names): numbers
    /// only, sent as they are.
    Signal,
    /// A WFDB record's samples in a MATLAB file (a `.mat` file a header of the same attachment
    /// names): its first matrix only, its descriptive text and the matrix's name blanked
    /// (`wfdb_folder::mat_deidentified`); one that is not a MAT file WFDB reads is refused.
    MatSignal,
    /// GE MUSE XML: de-identified.
    MuseXml,
    /// DICOM: the header de-identified, the pixels as they are (marked).
    Dicom,
    /// EDF and BDF: the header de-identified, annotation texts left out, the samples as they are.
    Edf,
    /// Pictures, videos, PDF: as they are, marked.
    Opaque,
    /// Anything else: refused.
    Refused,
}

/// The kind of an attachment's file; `signals`: the signal files its WFDB headers name
/// (patient/files.rs `signal_names`): any other `.dat` file could hold anything, and is refused.
pub fn file_kind(path: &Path, signals: &[String]) -> FileKind {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let named = path.file_name().is_some_and(|n| signals.iter().any(|s| *s == n.to_string_lossy()));
    match ext.as_str() {
        "hea" => FileKind::WfdbHeader,
        "dat" if named => FileKind::Signal,
        "dat" => FileKind::Refused,
        "mat" if named => FileKind::MatSignal,
        "mat" => FileKind::Refused,
        "xml" => FileKind::MuseXml,
        "dcm" | "dicom" => FileKind::Dicom,
        "edf" | "bdf" => FileKind::Edf,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "mp4" | "m4v" | "webm" | "mov" | "ogv" | "pdf" => {
            FileKind::Opaque
        }
        _ if crate::patient::is_dicom(path) => FileKind::Dicom,
        _ => FileKind::Refused,
    }
}

/// An XML file's texts (between its tags) through `f`; its tags as they are.
pub fn xml_texts(x: &str, f: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(x.len());
    let mut rest = x;
    while let Some(open) = rest.find('<') {
        out.push_str(&f(&rest[..open]));
        rest = &rest[open..];
        let close = rest.find('>').map_or(rest.len(), |c| c + 1);
        out.push_str(&rest[..close]);
        rest = &rest[close..];
    }
    out.push_str(&f(rest));
    out
}

/// A WFDB header as a restricted session gets it (deid.rs `wfdb_header`), with the record's
/// and its signal files' names replaced by tokens (`names`: real name → token), and the
/// patient's own names and date of birth (`who`: [`Restricted::identity_words`]) replaced in
/// every comment, whatever its form.
pub fn wfdb_header(
    r: &Restricted,
    text: &str,
    record_token: &str,
    names: &HashMap<String, String>,
    who: (&[String], &str),
) -> String {
    // before dates are moved: the date of birth as written
    let text: String = text
        .lines()
        .map(|l| if l.trim_start().starts_with('#') { own_words(l, who.0, who.1) } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n");
    let deid = crate::deid::wfdb_header(&text, &r.settings);
    let mut out = vec![];
    let mut seen_record = false;
    for line in deid.lines() {
        let t = line.trim_start();
        if t.starts_with('#') || t.is_empty() {
            // the dates are already moved (deid.rs): identifiers only, not the dates again
            out.push(r.dict.scrub(line).unwrap_or_else(|| line.to_string()));
            continue;
        }
        let mut f: Vec<String> = t.split_whitespace().map(String::from).collect();
        if !seen_record {
            seen_record = true;
            // "name[/segments] nsig [fs …]"
            let rest = f[0].split_once('/').map(|(_, s)| format!("/{s}")).unwrap_or_default();
            f[0] = format!("{record_token}{rest}");
        } else if let Some(tok) = names.get(&f[0]) {
            f[0] = tok.clone();
        }
        let line = f.join(" ");
        out.push(r.dict.scrub(&line).unwrap_or(line));
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_text_breaks_the_scrubbers() {
        // identifiers, contact details and names beside characters of two to four bytes
        let pieces =
            ["Doe", "doe", "Jane", "555", "-", "0123", "@", "x.org", " ", "St", "°", "é", "😀", "Ü", "MRN", ":", "1"];
        let names = vec!["Doe".to_string(), "Jane".to_string()];
        let n = pieces.len();
        let mut text = String::new();
        for a in 0..n {
            for b in 0..n {
                for c in 0..n {
                    for d in 0..n {
                        text.clear();
                        for k in [a, b, c, d] {
                            text.push_str(pieces[k]);
                        }
                        let _ = safe_harbor(&text);
                        let mut v = serde_json::json!({"text": text.clone()});
                        scrub_names(&mut v, &names, "1950-03-04");
                    }
                }
            }
        }
    }

    fn dict() -> Dict {
        let mut d = Dict {
            version: 0,
            whole: HashMap::new(),
            words: HashMap::new(),
            normal: HashMap::new(),
            names: ["zyxwortha".to_string(), "quilbert".into()].into(),
            paths: HashMap::new(),
            path_lengths: vec![],
            pseudonyms: HashMap::new(),
            draws: HashMap::from([(1, 0.25), (2, 0.75)]),
            draws_by_pseudonym: HashMap::from([("MO-1111-1111".into(), 0.25), ("MO-2222-2222".into(), 0.75)]),
            recordings: HashMap::new(),
            draws_by_recording: HashMap::from([(recording_token(5), 0.75)]),
        };
        for (a, b) in
            [("P900004", "MO-7F3K-2QXM"), ("SYN0000004", MRN), ("E9100002", "MO-E-AAAA-AAAA"), ("P1", "MO-1111-1111")]
        {
            d.add(a, b.into());
        }
        // a bedside-monitor recording name, made at run time (the repository's hook refuses real-looking ones)
        d.whole.insert(format!("/data/icu/icu7_12-{}", 1_700_000_000), recording_token(5));
        d
    }

    #[test]
    fn identifiers_are_replaced_in_values_and_text() {
        let d = dict();
        assert_eq!(d.scrub("P900004").unwrap(), "MO-7F3K-2QXM");
        assert_eq!(d.scrub("P1").unwrap(), "MO-1111-1111", "a short ID: as a whole value only");
        assert_eq!(d.scrub("added manually: P900004, P1").unwrap(), "added manually: MO-7F3K-2QXM, P1");
        assert_eq!(d.scrub("MRN SYN0000004 in E9100002.").unwrap(), "MRN [MRN] in MO-E-AAAA-AAAA.");
        assert_eq!(d.scrub("example_P900004_20240311.xml").unwrap(), "example_MO-7F3K-2QXM_20240311.xml");
        assert!(d.scrub("P9000045 and XP900004").is_none(), "only whole words");
        assert_eq!(d.scrub(&format!("/data/icu/ICU7_12-{}", 1_700_000_000)).unwrap(), "mo-rec:5");
        assert!(d.scrub("nothing here").is_none());
    }

    #[test]
    fn identifiers_are_found_whatever_their_case_and_zeros() {
        let d = dict();
        // the MRN SYN0000004, written otherwise
        for t in ["syn0000004", "SYN-0000004", "Syn_0000004", "MRN syn4 here"] {
            let out = d.scrub(t).unwrap_or_else(|| panic!("{t}"));
            assert!(out.contains(MRN) && !out.to_lowercase().contains("syn"), "{t}: {out}");
        }
        assert_eq!(d.scrub("p900004").unwrap(), "MO-7F3K-2QXM");
        // a short form of digits alone is not taken for one (a year)
        assert_eq!(normal("0002024"), None);
        assert_eq!(normal("00083651927").as_deref(), Some("83651927"));
        assert_eq!(normal("SYN-0004").as_deref(), Some("syn4"));
        assert_eq!(normal("P01"), None);
        assert!(d.scrub("in 2024 and 2025").is_none());
    }

    #[test]
    fn names_are_replaced_whatever_their_letters() {
        // lower-casing İ makes it longer: compared character by character, whole words only
        assert_eq!(
            replace_word("Dr İnce saw İNCE, not İncey.", "İnce", PATIENT_NAME),
            "Dr [Patient Name] saw [Patient Name], not İncey."
        );
        assert_eq!(replace_word("ÉLODIE and élodie", "Élodie", "X"), "X and X");
        assert_eq!(replace_word("Smithson Smith", "smith", "X"), "Smithson X");
        // every linked patient's names, in text about any of them
        let d = dict();
        assert_eq!(
            d.scrub_any_names("ECG of ZYXWORTHA, Quilbert's").unwrap(),
            "ECG of [Patient Name], [Patient Name]'s"
        );
        assert!(d.scrub_any_names("Zyxworthas and Quilbert2").is_none(), "whole words only");
        let r = Restricted { settings: Settings::default(), dict: Arc::new(d), secret: Default::default() };
        let mut v = serde_json::json!({"label": "Quilbert's ECG", "results": [{"summary": {"note": "for Zyxwortha"}}], "name": "Quilbert"});
        r.json(&mut v);
        assert_eq!(v["label"], "[Patient Name]'s ECG");
        assert_eq!(v["results"][0]["summary"]["note"], "for [Patient Name]");
        assert_eq!(v["name"], "Quilbert", "only under the keys of free text about anyone");
    }

    #[test]
    fn contact_details_are_replaced_in_text() {
        let t = "Call (555) 123-4567 or 555.123.4568, +1 555 123 4569, +44 20 7946 0958; SSN 123-45-6789; \
                 mail jane.doe+x@example.org; see https://example.org/p?id=7. Lives at 12 Elm Street, \
                 then 4410 Old Mill Rd. and P.O. Box 77. MRN: 0012345.";
        let out = safe_harbor(t).unwrap();
        for gone in ["555", "7946", "6789", "jane", "example", "Elm", "Mill", "Box 77", "0012345"] {
            assert!(!out.contains(gone), "{gone}: {out}");
        }
        for p in [PHONE, SSN, EMAIL, URL, ADDRESS, MRN] {
            assert!(out.contains(p), "{p}: {out}");
        }
        assert!(out.ends_with("MRN: [MRN]."), "{out}");
        // not contact details: dates, times, measurements, doses
        for kept in [
            "2024-01-02 08:00",
            "BP 120/80, HR 72",
            "Lactate 4.1 mmol/L",
            "QT 400 ms",
            "2 tablets daily",
            "recording-1704110400",
            "v1.2.3",
            "20240102083000",
            "Dr. Osei",
            "3 Main points",
        ] {
            assert_eq!(safe_harbor(kept), None, "{kept}");
        }
        // in a note as Privacy Mode shows it
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: Default::default() };
        assert!(r.note("phone 555-123-4567", &[], "").contains(PHONE));
    }

    #[test]
    fn paths_and_their_folders_are_replaced_in_text() {
        let mut d = dict();
        // a bedside-monitor recording's name, built here (such names are kept out of the sources)
        let path = format!("/data/Zyx_Quil/ICU9_3-{}", 1_704_200_000);
        let path = path.as_str();
        d.paths.insert(path.into(), recording_token(9));
        for dir in ["/data/Zyx_Quil", "/data"] {
            d.paths.insert(dir.into(), FOLDER.into());
        }
        d.path_lengths = vec![path.len(), "/data/Zyx_Quil".len(), "/data".len()];
        assert_eq!(
            d.scrub(&format!("no such file: {path}/x.dat; see /data/Zyx_Quil and /database")).unwrap(),
            "no such file: mo-rec:9/x.dat; see [Folder] and /database"
        );
        assert_eq!(d.scrub("/data2/x"), None, "whole path components only");
        assert_eq!(d.scrub_words(path).unwrap(), "mo-rec:9");
    }

    #[test]
    fn compound_names_are_split() {
        let w = name_words(&["Smith-Jones".into(), "Mary O'Brien".into()]);
        assert_eq!(w[0], "Smith-Jones");
        for x in ["Smith", "Jones", "O'Brien", "Brien", "Mary"] {
            assert!(w.contains(&x.to_string()), "{x}: {w:?}");
        }
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: Default::default() };
        let n = r.note("Mrs Smith-Jones (née Jones), and O'Brien", &w, "");
        assert!(!n.contains("Smith") && !n.contains("Jones") && !n.contains("Brien"), "{n}");
    }

    #[test]
    fn notes_lose_the_patients_own_names() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: Default::default() };
        let n = r.note(
            "Jane Doe (DOE, JANE), born 1950-03-04, seen 2024-01-02. Janet is her sister.",
            &["Jane".into(), "Doe".into()],
            "1950-03-04",
        );
        assert_eq!(n.matches(PATIENT_NAME).count(), 4, "{n}");
        assert!(n.contains("Janet") && n.contains(BIRTH_DATE) && !n.contains("2024-01-02"), "{n}");
    }

    #[test]
    fn entry_keys_hold_no_real_time() {
        let s = Settings::default();
        let off = s.offset_seconds() as f64;
        let k = shown_key("labs:3:1704175800", &s).unwrap();
        assert_eq!(k, format!("labs:3:{}", 1704175800.0 - off));
        assert_eq!(real_key(&k, &s), "labs:3:1704175800");
        let k = shown_key("vit:3:19724", &s).unwrap();
        assert!(k.starts_with("vit:3:t-"), "{k}");
        assert_eq!(real_key(&k, &s), "vit:3:19724");
        assert!(shown_key("item:41", &s).is_none() && shown_key("labs:x:1", &s).is_none());
        assert!(shown_key(&k, &s).is_none(), "never twice");
    }

    #[test]
    fn each_patient_has_their_own_offset() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: Default::default() };
        let day = 86400.0;
        let (t1, t2) = (1_704_175_800.0, 1_704_175_800.0 + 3.0 * day);
        let mut v = serde_json::json!({"items": [
            {"pseudonym": "MO-1111-1111", "time": t1, "end": t2},
            {"patient": 2, "time": t1},
            {"time": t1},
        ]});
        r.json(&mut v);
        let a = v["items"][0]["time"].as_f64().unwrap();
        let b = v["items"][1]["time"].as_f64().unwrap();
        let base = v["items"][2]["time"].as_f64().unwrap();
        assert_eq!(base, t1 - r.settings.base_seconds() as f64, "no patient: Medorama's offset");
        assert!(a < base && b < a, "each patient moved further, by their own days: {a} {b} {base}");
        assert_eq!(v["items"][0]["end"].as_f64().unwrap() - a, t2 - t1, "a patient's intervals are kept");
        // the same patient, on their page, by another session: the same dates
        let mut w = serde_json::json!({"time": t1});
        r.for_patient(1).json(&mut w);
        assert_eq!(w["time"].as_f64().unwrap(), a);
        // an entry's key: by its patient, and back
        let k = shown_key("labs:2:1704175800", &r.key_settings("labs:2:1")).unwrap();
        assert_eq!(k, format!("labs:2:{}", t1 - (t1 - b)));
        assert_eq!(r.real_key(&k), "labs:2:1704175800");
        // a recording: by its patient's
        assert_eq!(r.for_recording(&recording_token(5)).settings.offset_seconds(), (t1 - b) as i64);
        // none spread: Medorama's offset for everyone
        let flat = Settings { spread_days: 0, ..Settings::default() };
        assert_eq!(flat.for_draw(Some(0.99)).offset_seconds(), flat.base_seconds());
    }

    #[test]
    fn a_recording_moves_by_its_patients_offset() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: Default::default() };
        let t1 = 1_704_175_800.0;
        let patient2 = t1 - r.for_patient(2).settings.offset_seconds() as f64;
        let patient1 = t1 - r.for_patient(1).settings.offset_seconds() as f64;
        // as the recordings list answers: its path (its token once scrubbed), its encounters' IDs
        let path = format!("/data/icu/ICU7_12-{}", 1_700_000_000);
        let mut v = serde_json::json!({"items": [
            {"path": path, "encounters": [], "t_start": t1},
            {"path": "/elsewhere/x.edf", "encounters": ["MO-1111-1111"], "t_start": t1},
            {"path": "/elsewhere/y.edf", "encounters": [], "t_start": t1},
        ]});
        r.json(&mut v);
        assert_eq!(v["items"][0]["path"], "mo-rec:5");
        assert_eq!(v["items"][0]["t_start"].as_f64().unwrap(), patient2, "by its token");
        assert_eq!(v["items"][1]["t_start"].as_f64().unwrap(), patient1, "by its encounter");
        let base = t1 - r.settings.base_seconds() as f64;
        assert_eq!(v["items"][2]["t_start"].as_f64().unwrap(), base, "linked to no one: Medorama's offset");
        assert!(patient2 != base && patient1 != base);
    }

    #[test]
    fn file_tokens_and_kinds() {
        assert_eq!(file_token(3, "IMAGE-000001.DCM"), "mo-f0003.dcm");
        assert_eq!(file_token(0, "I0"), "mo-f0000");
        assert_eq!(token_index("mo-f0003.dcm"), Some(3));
        assert_eq!(token_index("mo-f0003"), Some(3));
        assert_eq!(token_index("mo-f003"), None);
        assert_eq!(token_index("record.hea"), None);
        assert_eq!(file_kind(Path::new("a.hea"), &[]), FileKind::WfdbHeader);
        assert_eq!(file_kind(Path::new("a.PNG"), &[]), FileKind::Opaque);
        assert_eq!(file_kind(Path::new("a.csv"), &[]), FileKind::Refused);
        assert_eq!(file_kind(Path::new("notes.txt"), &[]), FileKind::Refused);
        // samples only when a header of the attachment names them
        assert_eq!(file_kind(Path::new("/d/a.dat"), &["a.dat".into()]), FileKind::Signal);
        assert_eq!(file_kind(Path::new("/d/b.dat"), &["a.dat".into()]), FileKind::Refused);
        assert_eq!(file_kind(Path::new("/d/a.mat"), &["a.mat".into()]), FileKind::MatSignal);
        assert_eq!(file_kind(Path::new("/d/b.mat"), &["a.mat".into()]), FileKind::Refused);
    }

    #[test]
    fn wfdb_headers_lose_names_and_file_names() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: Default::default() };
        let text = "P900004_ecg 2 500 5000 08:30:00 15/01/2024\nP900004_ecg.dat 16 200/mV 16 0 0 0 0 I\nP900004_ecg.dat 16 200/mV 16 0 0 0 0 II\n# Name: Jane Doe\n# MRN: SYN0000004\n# Age: 94\n# Confirmed by: Dr. K. Osei\n# Confirmed at: 2024-01-16 09:00\n";
        let names: HashMap<String, String> = [("P900004_ecg.dat".to_string(), "mo-f0001.dat".to_string())].into();
        let out = wfdb_header(&r, text, "mo-f0000", &names, (&[], ""));
        assert!(out.starts_with("mo-f0000 2 500 5000"), "{out}");
        assert!(out.contains("mo-f0001.dat 16 200/mV"));
        for gone in ["P900004", "Jane", "SYN0000004", "15/01/2024", "Osei", "2024-01-16"] {
            assert!(!out.contains(gone), "{gone}: {out}");
        }
        assert!(out.contains("# Age: 90+") && out.contains("# Confirmed by: [Staff Name]"), "{out}");
        // dates moved once, not twice: 2024-01-16 09:00 back 36570 days is 1923-12-02 09:00
        assert!(out.contains("# Confirmed at: 1923-12-02 09:00"), "{out}");
    }
}
