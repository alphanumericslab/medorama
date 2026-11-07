//! The privacy gate (docs/security.md, "The privacy gate"): what leaves the server for a
//! session in Privacy Mode.
//!
//! Privacy Mode is the state of a session, not of the data: the catalog keeps every identifier
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

/// The placeholder of a database's column, by the catalog field it maps to, in Privacy Mode;
/// `None`: the column is shown.  Unmapped columns hold what the importer could not recognise:
/// only their measurements are shown (databases.rs).
pub fn column_placeholder(maps_to: &str, s: &Settings) -> Option<&'static str> {
    match maps_to {
        "last_name" | "first_name" if s.hides("names") => Some(PATIENT_NAME),
        "birth_date" if s.hides("birth_date") => Some(BIRTH_DATE),
        "mrn" | "mrn_authority" if s.hides("mrn") => Some(MRN),
        "csn" if s.hides("account_numbers") => Some(CSN),
        "patient_id" => Some(PATIENT_ID),
        "encounter_id" => None, // replaced by the dictionary with the encounter's pseudonym
        "text" if s.hides("notes") => Some(WITHHELD),
        "file" => Some(FILE_NAME),
        "author" => Some(STAFF_NAME),
        _ => None,
    }
}

/// How a database's column leaves the server: `None`, shown (its cells through the gate in
/// Privacy Mode); else the placeholder every non-empty cell becomes.  The column itself is
/// always listed, under its imported name.
pub fn column_mask(
    user: &crate::users::User,
    gate: Option<&Restricted>,
    header: &str,
    maps_to: &str,
) -> Option<&'static str> {
    match gate {
        None if crate::databases::column_visible(user, header, maps_to) => None,
        None => Some(WITHHELD),
        Some(g) => match column_placeholder(maps_to, &g.settings) {
            Some(p) => Some(p),
            None if crate::databases::column_visible(
                &crate::users::User { phi: false, ..user.clone() },
                header,
                maps_to,
            ) =>
            {
                None
            }
            None => Some(WITHHELD),
        },
    }
}

/// A cell as it leaves: its placeholder (when not empty), or its text through the gate.
pub fn cell(gate: Option<&Restricted>, mask: Option<&'static str>, v: &str) -> String {
    match (mask, gate) {
        (Some(_), _) if v.trim().is_empty() => String::new(),
        (Some(p), _) => p.to_string(),
        (None, Some(g)) => g.text(v),
        (None, None) => v.to_string(),
    }
}

// ------------------------------------------------------------------ the dictionary

/// Every identifier the catalog holds, with what Privacy Mode writes instead: imported patient
/// and encounter IDs (their pseudonyms), MRNs and account numbers (placeholders), recordings'
/// paths and file names (tokens).  Built from the database, kept while the data do not change.
pub struct Dict {
    /// The version of the identifiers it was built at (`data_version.identifiers`).
    pub version: i64,
    /// Whole values: a string equal to one of these is replaced.
    whole: HashMap<String, String>,
    /// Words: identifiers of 4 characters or more with a digit, replaced inside any text.
    words: HashMap<String, String>,
    /// Recordings' paths (by their tokens) and the folders they lie in ([`FOLDER`]), replaced
    /// inside any text (an error that names a file) …
    paths: HashMap<String, String>,
    /// … whose lengths, longest first.
    path_lengths: Vec<usize>,
    /// Patients' pseudonyms, by the catalog's patient number.
    pub pseudonyms: HashMap<i64, String>,
    /// Each patient's date draw (Privacy Mode's own offset), by patient number …
    pub draws: HashMap<i64, f64>,
    /// … and by the pseudonyms that name them: theirs, their encounters', their aliases.
    pub draws_by_pseudonym: HashMap<String, f64>,
    /// … and by the catalog's encounter number (entries' keys name their encounter).
    pub draws_by_encounter: HashMap<i64, f64>,
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

impl Dict {
    pub fn build(c: &Connection, version: i64) -> anyhow::Result<Dict> {
        let mut d = Dict {
            version,
            whole: HashMap::new(),
            words: HashMap::new(),
            paths: HashMap::new(),
            path_lengths: vec![],
            pseudonyms: HashMap::new(),
            draws: HashMap::new(),
            draws_by_pseudonym: HashMap::new(),
            draws_by_encounter: HashMap::new(),
            recordings: HashMap::new(),
            draws_by_recording: HashMap::new(),
        };
        let add = |d: &mut Dict, from: String, to: String| {
            let from = from.trim().to_string();
            if from.is_empty() {
                return;
            }
            if wordlike(&from) {
                d.words.insert(from.clone(), to.clone());
            }
            d.whole.insert(from, to);
        };
        let rows: Vec<(i64, String, String, Option<f64>)> = c
            .prepare("SELECT id, patient_id, COALESCE(pseudonym, ''), date_draw FROM patients")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?;
        for (id, pid, ps, draw) in rows {
            add(&mut d, pid, ps.clone());
            if let Some(dr) = draw {
                d.draws.insert(id, dr);
                d.draws_by_pseudonym.insert(ps.clone(), dr);
            }
            d.pseudonyms.insert(id, ps);
        }
        let rows: Vec<(i64, String, String, i64)> = c
            .prepare("SELECT id, encounter_id, COALESCE(pseudonym, ''), patient FROM encounters")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?;
        for (id, eid, ps, patient) in rows {
            if let Some(dr) = d.draws.get(&patient).copied() {
                d.draws_by_pseudonym.insert(ps.clone(), dr);
                d.draws_by_encounter.insert(id, dr);
            }
            add(&mut d, eid, ps);
        }
        let aliases: Vec<(String, i64)> = c
            .prepare("SELECT pseudonym, patient FROM pseudonym_aliases")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (ps, patient) in aliases {
            if let Some(dr) = d.draws.get(&patient).copied() {
                d.draws_by_pseudonym.insert(ps, dr);
            }
        }
        for sql in ["SELECT mrn FROM patient_mrns", "SELECT mrn FROM patient_identifiers WHERE mrn != ''"] {
            let mrns: Vec<String> = c.prepare(sql)?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for m in mrns {
                add(&mut d, m, MRN.into());
            }
        }
        let csns: Vec<String> = c
            .prepare("SELECT csn FROM encounter_identifiers WHERE csn != ''")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        for x in csns {
            add(&mut d, x, CSN.into());
        }
        let recs: Vec<(i64, String, String)> = c
            .prepare("SELECT id, path, name FROM recordings")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        for (id, path, name) in recs {
            let token = recording_token(id);
            d.whole.insert(path.clone(), token.clone());
            d.whole.insert(name, RECORDING.into());
            // its folders: a folder may be named after the patient
            for dir in Path::new(&path).ancestors().skip(1) {
                let dir = dir.to_string_lossy();
                if dir.len() > 1 && dir.contains(['/', '\\']) {
                    d.paths.entry(dir.to_string()).or_insert_with(|| FOLDER.into());
                }
            }
            d.paths.insert(path.clone(), token.clone());
            d.recordings.insert(token, path);
        }
        let mut lengths: Vec<usize> = d.paths.keys().map(|k| k.len()).collect();
        lengths.sort_unstable_by(|a, b| b.cmp(a));
        lengths.dedup();
        d.path_lengths = lengths;
        let links: Vec<(i64, i64)> = c
            .prepare("SELECT recording, MIN(encounter) FROM recording_links GROUP BY recording")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        for (rec, enc) in links {
            if let Some(dr) = d.draws_by_encounter.get(&enc).copied() {
                d.draws_by_recording.insert(recording_token(rec), dr);
            }
        }
        Ok(d)
    }

    /// Text with every identifier it holds replaced; `None` when nothing changes.
    pub fn scrub(&self, s: &str) -> Option<String> {
        if let Some(to) = self.whole.get(s.trim()) {
            return Some(to.clone());
        }
        self.scrub_words(s)
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
            if let Some(to) = self.words.get(run) {
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
                if let Some(to) = self.words.get(&s[k0..j]) {
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
    /// The catalog's secret (DICOM UIDs).
    pub secret: Vec<u8>,
}

/// Keys whose strings name staff (authors, signers, overreaders).
const STAFF_KEYS: &[&str] = &["author", "signed_by", "overreader", "confirmed_by", "editor", "physician"];

/// Keys whose strings are addresses inside the catalog (internal numbers only): never scrubbed.
const ADDRESSES: &[&str] = &["url", "src", "href", "link", "hash", "viewer"];

/// Keys whose value names the patient an object is about, once identifiers are replaced: a
/// pseudonym (the patient's or an encounter's), or the catalog's patient number.
const PATIENT_KEYS: &[&str] = &["pseudonym", "patient_id", "encounter_id", "patient"];

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

    /// The settings for an entry's key (`labs:<encounter>:…`): its encounter's patient's, so that
    /// the key reads the same on every page and comes back through [`Restricted::real_key`].
    pub fn key_settings(&self, key: &str) -> Settings {
        let enc = key.split(':').nth(1).and_then(|e| e.parse::<i64>().ok());
        self.settings.for_draw(enc.and_then(|e| self.dict.draws_by_encounter.get(&e).copied()))
    }

    /// An entry's key as the catalog knows it, from the key this gate sent.
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
        self.scrub_value(v, "");
        crate::deid::answer_with(v, &self.settings, &|m| self.settings_of(m));
    }

    fn scrub_value(&self, v: &mut Value, key: &str) {
        if let Value::String(s) = v {
            if let Some(k) = shown_key(s, &self.key_settings(s)) {
                *s = k;
                return;
            }
        }
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
            }
            Value::Array(items) => items.iter_mut().for_each(|x| self.scrub_value(x, key)),
            Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    self.scrub_value(x, k);
                }
            }
            _ => {}
        }
    }

    /// Text (a header, a note, a CSV cell) as a restricted session gets it: identifiers
    /// replaced, dates moved back.
    pub fn text(&self, s: &str) -> String {
        let s = self.dict.scrub(s).unwrap_or_else(|| s.to_string());
        self.settings.shift_text(&s)
    }

    /// A patient's own names, MRNs and date of birth replaced in text written about them (a
    /// note), besides [`Restricted::text`]: free text may still name them in other ways, so the
    /// page keeps its warning on it.
    pub fn note(&self, s: &str, names: &[String], birth: &str) -> String {
        // before dates are moved: the date of birth as written
        self.text(&own_words(s, names, birth))
    }

    /// Free text inside a file (a WFDB header's comment, a MUSE statement, a DICOM text value)
    /// as [`Restricted::note`] writes it, except that a value equal to a short identifier is
    /// kept (in a file, it may be a number).
    pub fn free_text(&self, s: &str, names: &[String], birth: &str) -> String {
        let s = own_words(s, names, birth);
        let s = self.dict.scrub_words(&s).unwrap_or(s);
        self.settings.shift_text(&s)
    }

    /// What names a patient in their own notes: their names, and their date of birth as
    /// written (the server's copy; never sent).
    pub fn identity_words(c: &Connection, patient: i64) -> (Vec<String>, String) {
        let row: Option<(String, String, String)> = c
            .query_row(
                "SELECT last_name, first_name, birth_date FROM patient_identifiers WHERE patient = ?1",
                [patient],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (last, first, birth) = row.unwrap_or_default();
        (name_words(&[last, first]), birth)
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
/// about them (their notes, reports, the titles of entries).
pub fn scrub_names(v: &mut Value, names: &[String], birth: &str) {
    fn walk(v: &mut Value, key: &str, names: &[String], birth: &str) {
        match v {
            Value::String(s) if !ADDRESSES.contains(&key) => {
                let out = own_words(s, names, birth);
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
/// signs hold their real time (`labs:<encounter>:<unix time>`, `vit:<encounter>:<day>`); they
/// leave with it moved back (`vit:` keys as `vit:<encounter>:t<moved start of the day>`), and
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

/// An entry's key as the catalog knows it, from the key Privacy Mode sent ([`shown_key`]).
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

/// `text` with every whole-word, case-insensitive occurrence of `word` replaced.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let lower = text.to_lowercase();
    let w = word.to_lowercase();
    if w.is_empty() || lower.len() != text.len() {
        // lower-casing changed the length (non-ASCII): compare as is
        return text.replace(word, with);
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(rel) = lower[from..].find(&w) {
        let at = from + rel;
        let end = at + w.len();
        let before = text[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            out.push_str(&text[last..at]);
            out.push_str(with);
            last = end;
        }
        from = end;
    }
    out.push_str(&text[last..]);
    out
}

/// The catalog's secret (32 random bytes, made once, in the settings table): it keys DICOM
/// UIDs' replacements.  Never sent anywhere.
pub fn secret(c: &Connection) -> anyhow::Result<Vec<u8>> {
    let v: Option<String> =
        c.query_row("SELECT value FROM settings WHERE key = 'gate_secret'", [], |r| r.get(0)).optional()?;
    if let Some(hex) = v.and_then(|v| serde_json::from_str::<String>(&v).ok()) {
        if let Some(b) = from_hex(&hex) {
            return Ok(b);
        }
    }
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow::anyhow!("the system's random generator failed: {e}"))?;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    c.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ('gate_secret', ?1, NULL, ?2)
         ON CONFLICT(key) DO NOTHING",
        crate::sql::params![serde_json::to_string(&hex)?, crate::db::now()],
    )?;
    // another connection may have made it first: read what is stored
    let v: String = c.query_row("SELECT value FROM settings WHERE key = 'gate_secret'", [], |r| r.get(0))?;
    serde_json::from_str::<String>(&v)
        .ok()
        .and_then(|h| from_hex(&h))
        .ok_or_else(|| anyhow::anyhow!("the catalog's secret is unreadable"))
}

fn from_hex(h: &str) -> Option<Vec<u8>> {
    (h.len() == 64)
        .then(|| (0..32).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok()).collect::<Option<Vec<u8>>>())
        .flatten()
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

    fn dict() -> Dict {
        let mut d = Dict {
            version: 0,
            whole: HashMap::new(),
            words: HashMap::new(),
            paths: HashMap::new(),
            path_lengths: vec![],
            pseudonyms: HashMap::new(),
            draws: HashMap::from([(1, 0.25), (2, 0.75)]),
            draws_by_pseudonym: HashMap::from([("MO-1111-1111".into(), 0.25), ("MO-2222-2222".into(), 0.75)]),
            draws_by_encounter: HashMap::from([(3, 0.25), (4, 0.75)]),
            recordings: HashMap::new(),
            draws_by_recording: HashMap::from([(recording_token(5), 0.75)]),
        };
        for (a, b) in
            [("P900004", "MO-7F3K-2QXM"), ("SYN0000004", MRN), ("E9100002", "MO-E-AAAA-AAAA"), ("P1", "MO-1111-1111")]
        {
            if wordlike(a) {
                d.words.insert(a.into(), b.into());
            }
            d.whole.insert(a.into(), b.into());
        }
        // a bedside-monitor recording name, made at run time (the repository's hook refuses real-looking ones)
        d.whole.insert(format!("/data/icu/ICU7_12-{}", 1_700_000_000), recording_token(5));
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
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: vec![] };
        let n = r.note("Mrs Smith-Jones (née Jones), and O'Brien", &w, "");
        assert!(!n.contains("Smith") && !n.contains("Jones") && !n.contains("Brien"), "{n}");
    }

    #[test]
    fn notes_lose_the_patients_own_names() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: vec![] };
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
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: vec![] };
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
        assert_eq!(base, t1 - r.settings.base_seconds() as f64, "no patient: the catalog's offset");
        assert!(a < base && b < a, "each patient moved further, by their own days: {a} {b} {base}");
        assert_eq!(v["items"][0]["end"].as_f64().unwrap() - a, t2 - t1, "a patient's intervals are kept");
        // the same patient, on their page, by another session: the same dates
        let mut w = serde_json::json!({"time": t1});
        r.for_patient(1).json(&mut w);
        assert_eq!(w["time"].as_f64().unwrap(), a);
        // an entry's key: by its encounter's patient, and back
        let k = r
            .dict
            .draws_by_encounter
            .get(&4)
            .map(|_| shown_key("labs:4:1704175800", &r.key_settings("labs:4:1")))
            .unwrap()
            .unwrap();
        assert_eq!(k, format!("labs:4:{}", t1 - (t1 - b)));
        assert_eq!(r.real_key(&k), "labs:4:1704175800");
        // a recording: by its patient's
        assert_eq!(r.for_recording(&recording_token(5)).settings.offset_seconds(), (t1 - b) as i64);
        // none spread: the catalog's offset for everyone
        let flat = Settings { spread_days: 0, ..Settings::default() };
        assert_eq!(flat.for_draw(Some(0.99)).offset_seconds(), flat.base_seconds());
    }

    #[test]
    fn a_recording_moves_by_its_patients_offset() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: vec![] };
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
        assert_eq!(v["items"][2]["t_start"].as_f64().unwrap(), base, "linked to no one: the catalog's offset");
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
    }

    #[test]
    fn wfdb_headers_lose_names_and_file_names() {
        let r = Restricted { settings: Settings::default(), dict: Arc::new(dict()), secret: vec![] };
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
