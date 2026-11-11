//! Databases (docs/importing.md): one kind of import for every table, a CSV file or
//! a spreadsheet (Excel `.xlsx`/`.xls`, OpenDocument `.ods`; the first sheet).
//!
//! A table's columns are mapped once onto the catalog's columns ([`GROUPS`]).  Each row may
//! then carry, in any combination:
//! - **a patient**: `patient_id` and/or `mrn` (with `mrn_authority`), name, date of birth, sex;
//! - **an encounter**: `encounter_id` (or `csn`), admission and discharge, unit, disposition,
//!   and a bed stay (`care_unit`, `bed`, `bed_start`, `bed_end`; `recording_file`: the file name
//!   of the recording made there, linked to the encounter);
//! - **an item**: a measurement, a drug, a code, a note or report (`category`, `name`, `time`,
//!   `end_time`, `value`, `unit`, `ref_low`, `ref_high`, `flag`, `code`, `text`, …);
//! - **a file**: `file`, a recording (a WFDB folder or record, or a file an installed adapter
//!   reads: added to the catalog and linked to the row's encounter) or an attachment (an image,
//!   a PDF, a text file: shown with the item).
//!
//! Rows describing patients and encounters go through the identity rules
//! ([`crate::linkage::apply_row`]: conflicts are held back for review); items are matched to
//! their encounter or patient ([`crate::ehr::insert_item`]: unmatched ones wait).  The whole
//! table is one batch, removed as a whole: one file, or several with the same columns
//! ([`import_parts`]), read row by row (a table of millions of rows is never held whole).  A
//! known export (GE MUSE ECG metadata ...) is converted first ([`crate::formats`]).

use crate::categories;
use crate::config::Config;
use crate::db::{audit, now};
use crate::ehr::{insert_item_id, Cache, ItemRow, Keys, Stored};
use crate::linkage::{apply_row, Force, Outcome};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use crate::util::{parse_time, tz};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// The catalog's columns, by group: what a table's columns are mapped onto.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("Patient", &["patient_id", "mrn", "mrn_authority", "last_name", "first_name", "birth_date", "sex"]),
    ("Encounter", &["encounter_id", "csn", "admit_time", "discharge_time", "admit_unit", "disposition"]),
    ("Location", &["care_unit", "bed", "bed_start", "bed_end", "recording_file"]),
    (
        "Item",
        &[
            "category",
            "name",
            "time",
            "end_time",
            "value",
            "unit",
            "ref_low",
            "ref_high",
            "flag",
            "code",
            "vocabulary",
            "text",
            "author",
            "service",
            "domain",
        ],
    ),
    ("File", &["file"]),
    ("Other", &["source"]),
];

pub fn columns() -> Vec<&'static str> {
    GROUPS.iter().flat_map(|g| g.1.iter().copied()).collect()
}

/// Header names (lower case, letters and digits only) a column is recognised by.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("patient_id", &["patientid", "patient", "subjectid", "subject", "pid", "personid"]),
    ("mrn", &["mrn", "medicalrecordnumber", "medrecno", "mrno"]),
    ("mrn_authority", &["mrnauthority", "authority", "assigningauthority"]),
    ("last_name", &["lastname", "surname", "familyname"]),
    ("first_name", &["firstname", "givenname"]),
    ("birth_date", &["birthdate", "dob", "dateofbirth"]),
    ("sex", &["sex", "gender"]),
    ("encounter_id", &["encounterid", "encounter", "visitid", "visit", "admissionid", "hadmid", "stayid"]),
    ("csn", &["csn", "accountnumber", "account", "fin"]),
    ("admit_time", &["admittime", "admit", "admissiontime", "admitted", "admitdate", "admissiondate"]),
    ("discharge_time", &["dischargetime", "discharge", "dischtime", "discharged", "dischargedate"]),
    ("admit_unit", &["admitunit", "admissionunit"]),
    ("disposition", &["disposition", "dischargedisposition"]),
    ("care_unit", &["careunit", "ward", "location"]),
    ("bed", &["bed", "bedlabel"]),
    ("bed_start", &["bedstart", "intime", "transferin"]),
    ("bed_end", &["bedend", "outtime", "transferout"]),
    ("recording_file", &["recordingfile", "recording", "recordingname"]),
    ("category", &["category", "kind"]),
    (
        "name",
        &[
            "name",
            "test",
            "testname",
            "item",
            "parameter",
            "label",
            "drug",
            "medication",
            "description",
            "notetype",
            "type",
            "study",
        ],
    ),
    (
        "time",
        &[
            "time",
            "datetime",
            "charttime",
            "date",
            "timestamp",
            "resulttime",
            "collected",
            "performed",
            "starttime",
            "start",
        ],
    ),
    ("end_time", &["endtime", "end", "stop", "stoptime"]),
    ("value", &["value", "result", "valuenum", "resultvalue", "dose", "amount", "measurement"]),
    ("unit", &["unit", "units", "uom", "valueuom"]),
    ("ref_low", &["reflow", "low", "lower", "normallow", "rangelow"]),
    ("ref_high", &["refhigh", "high", "upper", "normalhigh", "rangehigh"]),
    ("flag", &["flag", "abnormal", "abnormalflag"]),
    ("code", &["code", "icd", "icd10", "cpt", "loinc", "ndc", "rxnorm"]),
    ("vocabulary", &["vocabulary", "codesystem", "system"]),
    (
        "text",
        &[
            "text",
            "note",
            "notetext",
            "report",
            "reporttext",
            "interpretation",
            "comment",
            "comments",
            "body",
            "impression",
            "reaction",
            "finding",
            "findings",
            "conclusion",
        ],
    ),
    ("author", &["author", "provider", "clinician", "physician"]),
    ("service", &["service", "department", "specialty"]),
    ("domain", &["domain"]),
    ("file", &["file", "path", "filepath", "image", "link", "filename"]),
    ("source", &["source"]),
];

fn squash(h: &str) -> String {
    h.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// A cell as text: dates as `YYYY-MM-DD HH:MM:SS` (read in the catalog's time zone), whole
/// numbers without a decimal point.
fn cell(d: &calamine::Data) -> String {
    use calamine::Data;
    match d {
        Data::Empty | Data::Error(_) => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(x) => x
            .as_datetime()
            .map(|t| {
                if t.time() == chrono::NaiveTime::MIN {
                    t.format("%Y-%m-%d").to_string()
                } else {
                    t.format("%Y-%m-%d %H:%M:%S").to_string()
                }
            })
            .unwrap_or_default(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

/// The header and rows of a table: a CSV file, a Parquet file, or the first sheet of a spreadsheet.
pub fn read_rows(data: &[u8]) -> anyhow::Result<(Vec<String>, Vec<Vec<String>>)> {
    let part = Part { name: String::new(), data: PartData::Bytes(data) };
    let r = open_rows(&part)?;
    let rows = r.rows.map(|x| x.map(|(_, cells)| cells)).collect::<anyhow::Result<_>>()?;
    Ok((r.headers, rows))
}

/// The largest spreadsheet read (a spreadsheet is read whole; a larger table is a CSV file).
pub const MAX_SPREADSHEET: u64 = 100 << 20;

/// A spreadsheet's first sheet: its header, and its rows with their line numbers.  The reader
/// (calamine) can panic on a damaged file: that is an error of the import, never the end of
/// the server's thread.
fn sheet(data: &[u8]) -> anyhow::Result<(Vec<String>, Vec<(u64, Vec<String>)>)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sheet_now(data)))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the spreadsheet is damaged (it cannot be read)")))
}

fn sheet_now(data: &[u8]) -> anyhow::Result<(Vec<String>, Vec<(u64, Vec<String>)>)> {
    use calamine::Reader;
    if data.len() as u64 > MAX_SPREADSHEET {
        anyhow::bail!(
            "the spreadsheet is larger than {} MB: a spreadsheet is read whole; save the table as CSV (read row by row)",
            MAX_SPREADSHEET >> 20
        );
    }
    let mut wb = calamine::open_workbook_auto_from_rs(Cursor::new(data.to_vec()))
        .map_err(|e| anyhow::anyhow!("cannot read the spreadsheet: {e}"))?;
    let range = wb
        .worksheet_range_at(0)
        .ok_or_else(|| anyhow::anyhow!("the spreadsheet has no sheet"))?
        .map_err(|e| anyhow::anyhow!("cannot read the first sheet: {e}"))?;
    let first = range.start().map_or(0, |(r, _)| r as u64);
    let mut rows = range
        .rows()
        .enumerate()
        .map(|(i, r)| (first + i as u64 + 1, r.iter().map(|c| cell(c).trim().to_string()).collect::<Vec<_>>()));
    let headers = rows.next().ok_or_else(|| anyhow::anyhow!("the first sheet is empty"))?.1;
    Ok((headers, rows.collect()))
}

/// A text file that is no comma-separated UTF-8 text, refused with what it is: UTF-16 (a
/// byte-order mark), or columns separated by `;` or tabs (its header has none of them apart
/// by commas).  Medorama reads comma-separated UTF-8 (docs/importing.md, "Files").
fn check_text(head: &[u8], headers: &[String]) -> anyhow::Result<()> {
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        anyhow::bail!("the file is UTF-16 text: save it as CSV in UTF-8 (\"CSV UTF-8\" in Excel)");
    }
    if let [only] = headers {
        for (sep, what) in [(';', "semicolons"), ('\t', "tabs")] {
            if only.contains(sep) {
                anyhow::bail!(
                    "the columns are separated by {what}, not commas: save the table as CSV with commas (comma-separated values)"
                );
            }
        }
    }
    Ok(())
}

/// A CSV error said plainly: text that is not UTF-8 named as such, at its line.
fn csv_error(e: csv::Error) -> anyhow::Error {
    match e.kind() {
        csv::ErrorKind::Utf8 { pos, .. } => anyhow::anyhow!(
            "{}is not UTF-8 text (the file may be in Latin-1 or Windows-1252): save it as CSV in UTF-8",
            pos.as_ref().map_or(String::new(), |p| format!("line {} ", p.line()))
        ),
        _ => anyhow::Error::from(e),
    }
}

/// The mapping a table's headers suggest: catalog column -> header ("" when none).
pub fn guess_mapping(headers: &[String]) -> HashMap<String, String> {
    let sq: Vec<String> = headers.iter().map(|h| squash(h)).collect();
    let mut taken = vec![false; headers.len()];
    let mut out: HashMap<String, String> = columns().iter().map(|c| (c.to_string(), String::new())).collect();
    // exact names first, then synonyms, so `name` does not take a `note_type` meant for it
    for pass in 0..2 {
        for (col, words) in SYNONYMS {
            if !out[*col].is_empty() {
                continue;
            }
            let want = squash(col);
            let found = sq
                .iter()
                .enumerate()
                .position(|(i, h)| !taken[i] && if pass == 0 { *h == want } else { words.contains(&h.as_str()) });
            if let Some(i) = found {
                taken[i] = true;
                out.insert(col.to_string(), headers[i].clone());
            }
        }
    }
    // `unit` beside a bed and no value is the care unit
    if !out["unit"].is_empty() && out["value"].is_empty() && !out["bed"].is_empty() && out["care_unit"].is_empty() {
        let h = std::mem::take(out.get_mut("unit").unwrap());
        out.insert("care_unit".into(), h);
    }
    out
}

/// The category a table's file name or headers suggest ("" when none).
pub fn guess_category(file_name: &str, headers: &[String]) -> String {
    let mut words: Vec<String> = file_name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_string)
        .collect();
    words.extend(headers.iter().map(|h| h.to_lowercase()));
    words.iter().find_map(|w| categories::named(w)).unwrap_or("").to_string()
}

/// A table as uploaded: its headers, the mapping and category suggested, what it would add.
#[derive(Debug, Serialize)]
pub struct Inspection {
    pub headers: Vec<String>,
    pub rows: usize,
    /// A known export it is (converted as it is imported: [`crate::formats`]): its label, its key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_key: Option<String>,
    pub mapping: HashMap<String, String>,
    pub category: String,
    /// The catalog's columns, by group, for the mapping form.
    pub groups: Vec<(String, Vec<String>)>,
    pub categories: Vec<(String, String)>,
    /// The first rows (only for users with the PHI permission: tables hold identifiers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<Vec<String>>>,
}

pub fn inspect(
    cfg: &Config,
    file_name: &str,
    data: &[u8],
    phi: bool,
    choice: &crate::formats::Choice,
) -> anyhow::Result<Inspection> {
    inspect_part(cfg, &Part { name: file_name.to_string(), data: PartData::Bytes(data) }, phi, choice)
}

/// As [`inspect`], for a table uploaded or a file on this server: read row by row, its rows
/// counted and only the first five kept, so that a table of any size is inspected.
pub fn inspect_part(
    cfg: &Config,
    part: &Part,
    phi: bool,
    choice: &crate::formats::Choice,
) -> anyhow::Result<Inspection> {
    let Rows { headers, rows, .. } = open_rows(part)?;
    // a known export: shown as it will be stored, converted
    let known = crate::formats::all(cfg).0;
    let format = choice.resolve(&headers, &known);
    let format_key = format.as_ref().map(|f| f.key().to_string());
    let mut conv = match &format {
        Some(f) => Some(f.start(&headers)?),
        None => None,
    };
    let (mut n, mut sample) = (0usize, vec![]);
    for row in rows {
        let (_, cells) = row?;
        if !cells.iter().any(|v| !v.is_empty()) {
            continue;
        }
        let out = match conv.as_mut() {
            Some(c) => c.rows(&cells).unwrap_or_default(),
            None => vec![cells],
        };
        for r in out {
            n += 1;
            if sample.len() < 5 {
                sample.push(r);
            }
        }
    }
    let (headers, category) = match (&format, &conv) {
        (Some(f), Some(c)) => (c.headers(), f.category().to_string()),
        _ => {
            let cat = guess_category(&part.name, &headers);
            (headers, cat)
        }
    };
    Ok(Inspection {
        format: format.map(|f| f.label().to_string()),
        format_key: format_key.clone(),
        mapping: guess_mapping(&headers),
        category,
        groups: GROUPS.iter().map(|(g, c)| (g.to_string(), c.iter().map(|x| x.to_string()).collect())).collect(),
        categories: categories::CATEGORIES.iter().map(|c| (c.0.to_string(), c.1.to_string())).collect(),
        rows: n,
        sample: phi.then_some(sample),
        headers,
    })
}

#[derive(Debug, Default, Serialize)]
pub struct TableReport {
    pub import_id: i64,
    pub dry_run: bool,
    pub rows: usize,
    pub patients_added: usize,
    /// Patients given a pseudonymous ID by the catalog (rows with an MRN but no patient_id).
    pub patients_named: usize,
    pub encounters_added: usize,
    pub bed_stays_added: usize,
    /// Rows held back: they contradict the catalog (the Review tab).
    pub conflicts: usize,
    pub items_added: usize,
    pub duplicates: usize,
    pub with_encounter: usize,
    pub patient_level: usize,
    pub waiting: usize,
    pub recordings_added: usize,
    pub recordings_already: usize,
    pub attachments: usize,
    pub recordings_linked: usize,
    pub ehr_matched: usize,
    pub ignored_columns: Vec<String>,
    pub warnings: Vec<String>,
}

const MAX_WARNINGS: usize = 50;

fn warn(w: &mut Vec<String>, msg: String) {
    if w.len() < MAX_WARNINGS {
        w.push(msg);
    } else if w.len() == MAX_WARNINGS {
        w.push("(more warnings not shown)".into());
    }
}

/// What a row became, kept with the row (import_rows), and where it is in its file.
struct RowNote {
    status: &'static str,
    item: Option<i64>,
    recording: Option<i64>,
    /// Its line in its file, and which file (of an import's files).
    line: u64,
    part: usize,
}

impl Default for RowNote {
    fn default() -> Self {
        RowNote { status: "skipped", item: None, recording: None, line: 0, part: 0 }
    }
}

/// Where a row is, as warnings name it: `line 12`, or `b.csv, line 12` (an import of several
/// files).
struct Line<'a> {
    file: Option<&'a str>,
    line: u64,
}

impl std::fmt::Display for Line<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.file {
            Some(name) => write!(f, "{name}, line {}", self.line),
            None => write!(f, "line {}", self.line),
        }
    }
}

/// The catalog column each of a table's columns is read as ("" when none): a mapped header, or
/// a header named as the catalog's column.  A catalog column is read from the first column
/// that names it; a second one (`value,value`) is kept as imported only, with a warning.
fn column_map(
    headers: &[String],
    mapping: &HashMap<String, String>,
    cols: &[&'static str],
) -> (Vec<&'static str>, Vec<String>) {
    let mut maps: Vec<&'static str> = vec![];
    let mut warnings = vec![];
    for h in headers {
        let k = cols
            .iter()
            .copied()
            .find(|k| mapping.get(*k).map_or(h.eq_ignore_ascii_case(k), |m| m.eq_ignore_ascii_case(h)))
            .unwrap_or("");
        if !k.is_empty() && maps.contains(&k) {
            warnings.push(format!(
                "the column {h} appears twice: the first is read as {k}, this one is kept as imported only"
            ));
            maps.push("");
        } else {
            maps.push(k);
        }
    }
    (maps, warnings)
}

/// Keep a database's columns as they were, with the catalog column each is read as (`maps`).
fn store_columns(tx: &Connection, batch: i64, headers: &[String], maps: &[&'static str]) -> anyhow::Result<()> {
    for (i, (h, k)) in headers.iter().zip(maps).enumerate() {
        tx.execute(
            "INSERT INTO import_columns (import_id, pos, header, maps_to) VALUES (?1, ?2, ?3, ?4)",
            params![batch, i as i64, h, k],
        )?;
    }
    Ok(())
}

/// Keep rows as they were (line, cells) with what each became, 100 to a statement.
fn store_rows(tx: &Connection, batch: i64, maps: &[&str], rows: &[(i64, Vec<String>, RowNote)]) -> anyhow::Result<()> {
    use crate::sql::Value;
    // their numbers, by column (crate::typed)
    let typed: Vec<(i64, &[String])> = rows.iter().map(|(line, cells, _)| (*line, cells.as_slice())).collect();
    crate::typed::insert(tx, batch, maps.len(), &typed)?;
    let at = |row: &[String], key: &str| {
        maps.iter().position(|m| *m == key).and_then(|i| row.get(i)).map(|s| s.trim().to_string()).unwrap_or_default()
    };
    for chunk in rows.chunks(100) {
        let mut values: Vec<Value> = vec![Value::Integer(batch)];
        let mut tuples = vec![];
        for (line, row, note) in chunk {
            let first = values.len() + 1;
            values.extend([
                Value::Integer(*line),
                Value::Text(serde_json::to_string(row)?),
                Value::Text(note.status.into()),
                Value::Text(at(row, "patient_id")),
                Value::Text(at(row, "mrn")),
                Value::Text(at(row, "encounter_id")),
                note.item.map_or(Value::Null, Value::Integer),
                note.recording.map_or(Value::Null, Value::Integer),
            ]);
            tuples
                .push(format!("(?1, {})", (first..first + 8).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ")));
        }
        tx.execute(
            &format!(
                "INSERT INTO import_rows (import_id, line, cells, status, key_patient, key_mrn, key_encounter, item, recording)
                 VALUES {}",
                tuples.join(", ")
            ),
            crate::sql::params_from_iter(values),
        )?;
    }
    Ok(())
}

/// One file of a table to import.
pub struct Part<'a> {
    /// Its name (a file name): the import's origin, a category's guess.
    pub name: String,
    pub data: PartData<'a>,
}

pub enum PartData<'a> {
    Bytes(&'a [u8]),
    File(PathBuf),
}

/// A file's rows as they come, each with its line in the file: a CSV file row by row; a
/// spreadsheet's first sheet whole (a spreadsheet is small); a Parquet file's rows by number.
struct Rows<'a> {
    headers: Vec<String>,
    rows: Box<dyn Iterator<Item = anyhow::Result<(u64, Vec<String>)>> + 'a>,
    /// How far through the file the rows read are, from 0 to 1 (an import's progress).
    read: Box<dyn Fn() -> f64 + 'a>,
}

type LineRows<'a> = Box<dyn Iterator<Item = anyhow::Result<(u64, Vec<String>)>> + 'a>;

/// Rows counted as they are read, of `total`.
fn counting<'a>(rows: LineRows<'a>, total: u64) -> (LineRows<'a>, Box<dyn Fn() -> f64 + 'a>) {
    let n = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let m = n.clone();
    let rows = rows.inspect(move |_| m.set(m.get() + 1));
    (Box::new(rows), Box::new(move || if total == 0 { 1.0 } else { (n.get() as f64 / total as f64).min(1.0) }))
}

/// A reader that counts the bytes read through it.
struct Counting<R> {
    inner: R,
    n: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: std::io::Read> std::io::Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let k = self.inner.read(buf)?;
        self.n.set(self.n.get() + k as u64);
        Ok(k)
    }
}

fn is_spreadsheet(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04") || head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
}

/// A file's columns (its header), without reading its rows.
pub fn columns_of(part: &Part) -> anyhow::Result<Vec<String>> {
    Ok(open_rows(part)?.headers)
}

/// A file's columns and its rows, read one by one (CLIF's cultures, read again to link their
/// susceptibilities).
pub fn rows_of<'a>(
    part: &'a Part,
) -> anyhow::Result<(Vec<String>, Box<dyn Iterator<Item = anyhow::Result<Vec<String>>> + 'a>)> {
    let r = open_rows(part)?;
    Ok((r.headers, Box::new(r.rows.map(|x| x.map(|(_, cells)| cells)))))
}

fn open_rows<'a>(part: &'a Part) -> anyhow::Result<Rows<'a>> {
    use std::io::{BufRead, Read};
    let whole = |data: &[u8]| -> anyhow::Result<Rows<'a>> {
        let (headers, rows) = sheet(data)?;
        let total = rows.len() as u64;
        let (rows, read) = counting(Box::new(rows.into_iter().map(Ok)), total);
        Ok(Rows { headers, rows, read })
    };
    let parquet = |(headers, rows, total): (Vec<String>, crate::parquet_rows::RowsIter, u64)| {
        // (a row's number, after the header's line, as a CSV file's would be)
        let rows: LineRows<'a> = Box::new(rows.enumerate().map(|(i, r)| r.map(|cells| (i as u64 + 2, cells))));
        let (rows, read) = counting(rows, total);
        Rows { headers, rows, read }
    };
    let bytes = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let (mut reader, size): (Box<dyn BufRead + 'a>, u64) = match &part.data {
        PartData::Bytes(b) if is_spreadsheet(b) => return whole(b),
        PartData::Bytes(b) if crate::parquet_rows::is_parquet(b) => {
            return Ok(parquet(crate::parquet_rows::open_bytes(bytes::Bytes::copy_from_slice(b))?));
        }
        PartData::Bytes(b) => {
            (Box::new(std::io::BufReader::new(Counting { inner: *b, n: bytes.clone() })), b.len() as u64)
        }
        PartData::File(p) => {
            let open = || std::fs::File::open(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()));
            let mut head = [0u8; 4];
            let n = open()?.read(&mut head)?;
            if is_spreadsheet(&head[..n]) {
                let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                if size > MAX_SPREADSHEET {
                    anyhow::bail!(
                        "{}: the spreadsheet is larger than {} MB: a spreadsheet is read whole; save the table as CSV (read row by row)",
                        p.display(),
                        MAX_SPREADSHEET >> 20
                    );
                }
                return whole(&std::fs::read(p)?);
            }
            if crate::parquet_rows::is_parquet(&head[..n]) {
                return Ok(parquet(crate::parquet_rows::open(p)?));
            }
            let f = open()?;
            let size = f.metadata().map(|m| m.len()).unwrap_or(0);
            (Box::new(std::io::BufReader::with_capacity(1 << 20, Counting { inner: f, n: bytes.clone() })), size)
        }
    };
    let read: Box<dyn Fn() -> f64 + 'a> =
        Box::new(move || if size == 0 { 1.0 } else { (bytes.get() as f64 / size as f64).min(1.0) });
    let head = reader.fill_buf()?.iter().take(4).copied().collect::<Vec<u8>>();
    // Excel's byte-order mark
    if head.starts_with(b"\xEF\xBB\xBF") {
        reader.consume(3);
    }
    check_text(&head, &[])?;
    let mut rd = csv::ReaderBuilder::new().flexible(true).trim(csv::Trim::All).from_reader(reader);
    let headers: Vec<String> = rd.headers().map_err(csv_error)?.iter().map(str::to_string).collect();
    check_text(&head, &headers)?;
    // each row with its line: where the record starts (the reader counts blank lines before
    // it into the record's start, so it is taken from where the record ends, less its cells' own
    // line breaks)
    let mut rec = csv::StringRecord::new();
    let rows = std::iter::from_fn(move || match rd.read_record(&mut rec) {
        Ok(false) => None,
        Err(e) => Some(Err(csv_error(e))),
        Ok(true) => {
            let start = rec.position().map_or(0, |p| p.line());
            let inner = rec.iter().map(|c| c.matches('\n').count() as u64).sum::<u64>();
            let end = rd.position().line().saturating_sub(1 + inner);
            Some(Ok((start.max(end), rec.iter().map(str::to_string).collect())))
        }
    });
    Ok(Rows { headers, rows: Box::new(rows), read })
}

/// Files a viewer of its own opens: recordings (WFDB, EDF, a format an adapter reads).
fn is_recording(p: &Path) -> bool {
    p.is_dir()
        || p.extension().is_some_and(|e| e.eq_ignore_ascii_case("hea"))
        || recording_engine::sources::edf::is_edf(p)
        || recording_engine::adapters::for_file(p).is_some()
}

/// Columns that describe a patient or an encounter (beyond the keys that name them).
const DESCRIBING: &[&str] = &[
    "last_name",
    "first_name",
    "birth_date",
    "sex",
    "admit_time",
    "discharge_time",
    "admit_unit",
    "disposition",
    "care_unit",
    "bed",
    "bed_start",
    "bed_end",
    "recording_file",
];
const ITEM: &[&str] = &["name", "value", "text", "file"];

/// Measurements a database (ECG …) may give as columns, with their usual unit.
const MEASURES: &[(&str, &str)] = &[
    ("vent. rate", "bpm"),
    ("ventricular rate", "bpm"),
    ("atrial rate", "bpm"),
    ("heart rate", "bpm"),
    ("hr", "bpm"),
    ("pr", "ms"),
    ("qrs", "ms"),
    ("qt", "ms"),
    ("qtc", "ms"),
    ("qtcb", "ms"),
    ("qtcf", "ms"),
    ("rr", "ms"),
    ("p axis", "°"),
    ("r axis", "°"),
    ("qrs axis", "°"),
    ("t axis", "°"),
];

/// A database column's measurement name and unit: `QT (ms)`, `Lactate [mmol/L]`, or a known
/// name alone (`QTc`, `R axis`).  Cohorts compare its values in that unit (`QT > 0.4 s`).
pub fn measure_column(header: &str) -> Option<(String, String)> {
    let h = header.trim();
    if let Some(open) = h.rfind(['(', '[']) {
        let close = if h.as_bytes()[open] == b'(' { ')' } else { ']' };
        if let Some(unit) = h[open + 1..].strip_suffix(close) {
            let name = h[..open].trim();
            let unit = unit.trim();
            if !name.is_empty() && !unit.is_empty() && unit.chars().count() <= 12 {
                return Some((name.to_string(), unit.to_string()));
            }
        }
    }
    let low = h.to_lowercase();
    MEASURES.iter().find(|(n, _)| *n == low).map(|(_, u)| (h.to_string(), u.to_string()))
}

/// Import a table (admins only): one batch; `category` is the category of the table's items
/// when it has no `category` column.  Relative file paths are read from `base` (the folder of
/// the table, for commands; else the working folder).  A known export is converted first.
#[allow(clippy::too_many_arguments)]
pub fn import(
    c: &mut Connection,
    cfg: &Config,
    user: &User,
    origin: &str,
    data: &[u8],
    mapping: &HashMap<String, String>,
    category: &str,
    base: Option<&Path>,
    opts: &crate::imports::Opts,
) -> anyhow::Result<TableReport> {
    let parts = [Part { name: origin.to_string(), data: PartData::Bytes(data) }];
    import_parts(c, cfg, user, &parts, mapping, category, base, opts, crate::formats::Choice::Detect)
}

/// Rows stored at a time: what an import holds of its table at once.
const CHUNK: usize = 5000;

/// After each chunk committed (SQLite), a pause that lets the other writers waiting for the
/// database's one write lock (a page's change, the audit log) take it.
const PAUSE: std::time::Duration = std::time::Duration::from_millis(30);

/// Import one table from one or more files with the same columns (the same once converted,
/// for a known export): one batch, one database.  The files are read row by row.  Imports run
/// one at a time (another waits: its progress says `waiting`).  On SQLite the batch is
/// committed a chunk of rows at a time, `loading` until it ends (the database's write lock is
/// free between chunks); an import that stops part way (an error, a cancel) is removed as a
/// whole, as are those a crash left loading ([`crate::imports::discard_unfinished`]), so that
/// nothing of it is kept.
#[allow(clippy::too_many_arguments)]
pub fn import_parts(
    c: &mut Connection,
    cfg: &Config,
    user: &User,
    parts: &[Part],
    mapping: &HashMap<String, String>,
    category: &str,
    base: Option<&Path>,
    opts: &crate::imports::Opts,
    format: crate::formats::Choice,
) -> anyhow::Result<TableReport> {
    if !user.is_admin() {
        anyhow::bail!("only admins import databases");
    }
    let lock = c.import_lock(|| {
        if let Some(p) = &opts.progress {
            p.phase(crate::progress::Phase::Waiting);
            p.check()?;
        }
        Ok(())
    })?;
    if let Some(p) = &opts.progress {
        p.phase(crate::progress::Phase::Rows);
    }
    if lock.first() {
        crate::imports::discard_unfinished(c, cfg, true)?;
    }
    let mut batch = None;
    // slash dates read as the catalog writes its dates (day first, or month first)
    let order = crate::util::date_order_of(&crate::display::load(c)?.date);
    let r = crate::util::with_date_order(order, || {
        import_locked(c, cfg, user, parts, mapping, category, base, opts, format, &mut batch)
    });
    if let (Err(_), Some(b)) = (&r, batch) {
        if let Err(e) = crate::imports::discard(c, cfg, b) {
            eprintln!("medorama: import {b} stopped, and what it had added could not be removed yet ({e}): it is at the next start");
        }
    }
    r
}

/// The highest EHR item id so far: the items a transaction adds come after it.
fn last_item(tx: &Connection) -> crate::sql::Result<i64> {
    tx.query_row("SELECT COALESCE(MAX(id), 0) FROM ehr_items", [], |r| r.get(0))
}

#[allow(clippy::too_many_arguments)]
fn import_locked(
    c: &mut Connection,
    cfg: &Config,
    user: &User,
    parts: &[Part],
    mapping: &HashMap<String, String>,
    category: &str,
    base: Option<&Path>,
    opts: &crate::imports::Opts,
    format: crate::formats::Choice,
    batch_out: &mut Option<i64>,
) -> anyhow::Result<TableReport> {
    let Some(first) = parts.first() else { anyhow::bail!("no file to import") };
    let origin =
        if parts.len() == 1 { first.name.clone() } else { format!("{} and {} more", first.name, parts.len() - 1) };
    let mut rows = open_rows(first)?;
    let known = crate::formats::all(cfg).0;
    let format = format.resolve(&rows.headers, &known);
    let mut convert = match &format {
        Some(f) => Some(f.start(&rows.headers).map_err(|e| anyhow::anyhow!("{}: {e}", first.name))?),
        None => None,
    };
    let headers = convert.as_ref().map_or_else(|| rows.headers.clone(), |c| c.headers());
    // no category chosen: the format's, else the one the file name or headers suggest
    let guessed;
    let category = if category.trim().is_empty() {
        guessed = format
            .as_ref()
            .filter(|f| !f.category().is_empty())
            .map_or_else(|| guess_category(&first.name, &headers), |f| f.category().to_string());
        guessed.as_str()
    } else {
        category
    };
    let mapping: HashMap<String, String> = if mapping.is_empty() {
        guess_mapping(&headers)
    } else {
        mapping.iter().filter(|(_, h)| !h.is_empty()).map(|(k, v)| (k.clone(), v.clone())).collect()
    };
    let cols = columns();
    for k in mapping.keys() {
        if !cols.contains(&k.as_str()) {
            anyhow::bail!("mapping: unknown column {k:?} (known: {})", cols.join(", "));
        }
    }
    // each column's catalog column (as `linkage::read_table` reads them); the others are the
    // database's own
    let (maps, doubled) = column_map(&headers, &mapping, &cols);
    let colmap: Vec<(usize, &'static str)> =
        maps.iter().enumerate().filter(|(_, k)| !k.is_empty()).map(|(i, k)| (i, *k)).collect();
    let ignored: Vec<String> =
        headers.iter().zip(&maps).filter(|(_, k)| k.is_empty()).map(|(h, _)| h.clone()).collect();
    let zone = tz(&cfg.timezone);
    let mut rep = TableReport { dry_run: opts.dry_run, ignored_columns: ignored, ..Default::default() };
    for w in doubled {
        warn(&mut rep.warnings, w);
    }
    // SQLite: committed a chunk at a time (a dry run, and PostgreSQL, in one transaction)
    let chunked = c.dialect() == crate::sql::Dialect::Sqlite && !opts.dry_run;
    let mut tx = c.transaction()?;
    tx.lock_in_transaction(crate::sql::Lock::Identity)?;
    let batch = crate::imports::begin(&tx, "table", &opts.name, &opts.source, &origin, user)?;
    *batch_out = Some(batch);
    let mut text_index = crate::ehr::defer_text_index(&tx)?;
    let mut items_after = if chunked { last_item(&tx)? } else { 0 };
    rep.import_id = batch;
    store_columns(&tx, batch, &headers, &maps)?;
    crate::typed::create(&tx, batch, headers.len())?;
    let mut cache = Cache::new();
    let mut sink =
        Sink { rep, rec_rep: crate::recordings::ImportReport::default(), new_recordings: vec![], links: vec![] };
    let mut chunk: Vec<(i64, Vec<String>, RowNote)> = Vec::with_capacity(CHUNK);
    // the chunk's rows (as read), decided together: patients and encounters, then items
    let mut pending: Vec<HashMap<&'static str, String>> = vec![];
    let mut chunk_eids: std::collections::HashSet<String> = Default::default();
    let files: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    let ctx = Ctx { cfg, user, batch, opts, category, base, zone, files: if parts.len() > 1 { &files } else { &[] } };
    // rows a format leaves out, by why (counted: rows hold identifiers)
    let mut left_out: std::collections::BTreeMap<String, usize> = Default::default();
    let mut n = 0usize;
    // how far through the files: by their sizes
    let sizes: Vec<f64> = parts
        .iter()
        .map(|p| match &p.data {
            PartData::Bytes(b) => b.len() as f64,
            PartData::File(f) => std::fs::metadata(f).map(|m| m.len() as f64).unwrap_or(0.0),
        })
        .collect();
    let total: f64 = sizes.iter().sum::<f64>().max(1.0);
    let mut before = 0.0;
    let report = |n: usize, before: f64, size: f64, read: f64| {
        if let Some(p) = &opts.progress {
            p.rows(n as u64, (before + size * read) / total);
        }
    };
    for (pi, part) in parts.iter().enumerate() {
        if pi > 0 {
            before += sizes[pi - 1];
        }
        if pi > 0 {
            rows = open_rows(part)?;
            let theirs = match &format {
                Some(f) => {
                    let conv = f.start(&rows.headers).map_err(|e| anyhow::anyhow!("{}: {e}", part.name))?;
                    let h = conv.headers();
                    convert = Some(conv);
                    h
                }
                None => rows.headers.clone(),
            };
            if theirs != headers {
                anyhow::bail!(
                    "{}: its columns are not those of {}: a database is made of files with the same columns",
                    part.name,
                    first.name
                );
            }
        }
        for record in &mut rows.rows {
            let (file_line, record) = record.map_err(|e| anyhow::anyhow!("{}: {e}", part.name))?;
            if record.iter().all(|v| v.trim().is_empty()) {
                continue;
            }
            // a format may make several rows of one (a ventilator's settings, each an entry)
            let converted = match convert.as_mut() {
                Some(conv) => match conv.rows(&record) {
                    Ok(r) => r,
                    Err(why) => {
                        *left_out.entry(why).or_default() += 1;
                        continue;
                    }
                },
                None => vec![record],
            };
            for cells in converted {
                let row: HashMap<&'static str, String> =
                    colmap.iter().map(|(i, k)| (*k, cells.get(*i).cloned().unwrap_or_default())).collect();
                // the row's number in the database (its key: unique, in order); its line, for warnings
                let line = n + 2;
                n += 1;
                let mut note = RowNote { line: file_line, part: pi, ..Default::default() };
                if !opts.row_by_row {
                    // a row whose patient is an encounter's this chunk adds: the chunk first
                    if owner_wanted(&row).is_some_and(|e| chunk_eids.contains(e)) {
                        ctx.chunk(&tx, &mut pending, &mut chunk, &mut sink, &mut cache)?;
                        chunk_eids.clear();
                    }
                    if let Some(e) = row.get("encounter_id").map(|e| e.trim()).filter(|e| !e.is_empty()) {
                        chunk_eids.insert(e.to_string());
                    }
                    pending.push(row);
                    chunk.push((line as i64, cells, note));
                    continue;
                }
                let mut at = Row {
                    rep: &mut sink.rep,
                    rec_rep: &mut sink.rec_rep,
                    new_recordings: &mut sink.new_recordings,
                    links: &mut sink.links,
                };
                let item = import_row(&tx, &ctx, &row, &mut note, &mut at, None)?;
                if let Some((item, source)) = item {
                    let (stored, id) = insert_item_id(&tx, batch, &source, &item, &mut cache)?;
                    counted(&mut sink.rep, &mut note, stored, id);
                }
                chunk.push((line as i64, cells, note));
            }
            if chunk.len() >= CHUNK {
                if let Some(p) = &opts.progress {
                    p.check()?;
                }
                ctx.chunk(&tx, &mut pending, &mut chunk, &mut sink, &mut cache)?;
                chunk_eids.clear();
                store_rows(&tx, batch, &maps, &chunk)?;
                chunk.clear();
                cache.flush(&tx)?;
                report(n, before, sizes[pi], (rows.read)());
                if chunked {
                    // the chunk committed, the write lock given a moment to others
                    crate::ehr::index_text(&tx, batch, items_after, text_index.take())?;
                    tx.commit()?;
                    std::thread::sleep(PAUSE);
                    tx = c.transaction()?;
                    text_index = crate::ehr::defer_text_index(&tx)?;
                    items_after = last_item(&tx)?;
                    // (what others changed meanwhile is read again)
                    cache = Cache::new();
                }
            }
        }
    }
    if let Some(p) = &opts.progress {
        p.check()?;
    }
    ctx.chunk(&tx, &mut pending, &mut chunk, &mut sink, &mut cache)?;
    cache.flush(&tx)?;
    store_rows(&tx, batch, &maps, &chunk)?;
    crate::typed::analyze(&tx, batch)?;
    report(n, total, 0.0, 1.0);
    let Sink { mut rep, rec_rep, new_recordings, links } = sink;
    drop(chunk);
    for (why, k) in left_out.iter().rev() {
        rep.warnings.insert(0, format!("{k} {why}: left out ({})", format.as_ref().map_or("", |f| f.label())));
    }
    rep.recordings_added = rec_rep.added;
    rep.recordings_already = rec_rep.already;
    for (p, why) in &rec_rep.skipped {
        warn(&mut rep.warnings, format!("recording {p}: {why}"));
    }
    // a recording a row names with its encounter belongs to that encounter, while this batch is
    // there (not a decision by hand: removing the batch removes the link)
    for (rec, eid) in &links {
        tx.execute(
            "INSERT OR IGNORE INTO import_links (import_id, recording, encounter_id) VALUES (?1, ?2, ?3)",
            params![batch, rec, eid],
        )?;
    }
    if rep.waiting > 0 {
        rep.warnings.insert(
            0,
            format!(
                "{} item(s) wait for their patient or encounter: they are matched when another database names them",
                rep.waiting
            ),
        );
    }
    let summary = serde_json::json!({
        "rows": rep.rows, "patients_added": rep.patients_added, "encounters_added": rep.encounters_added,
        "items_added": rep.items_added, "recordings_added": rep.recordings_added, "conflicts": rep.conflicts,
        "waiting": rep.waiting, "warnings": rep.warnings.len(), "files": parts.len(),
        "format": format.as_ref().map(|f| f.key()), "left_out": left_out.values().sum::<usize>(),
    });
    crate::imports::finish_as(&tx, batch, &summary, opts.keep_loading)?;
    if !opts.dry_run {
        // (a dry run's rollback puts the trigger back)
        if let Some(p) = &opts.progress {
            p.phase(crate::progress::Phase::Index);
        }
        crate::ehr::index_text(&tx, batch, items_after, text_index)?;
    }
    if opts.dry_run {
        tx.rollback()?;
        return Ok(rep);
    }
    if let Some(p) = &opts.progress {
        p.check()?;
    }
    tx.commit()?;
    // what follows works on the import as stored: a failure is said, not the import's
    if let Some(p) = &opts.progress {
        p.phase(crate::progress::Phase::Match);
    }
    for id in &new_recordings {
        if let Err(e) = crate::recordings::adopt_index(c, cfg, *id) {
            warn(&mut rep.warnings, format!("recording {id}: its index could not be read ({e}): it is indexed again"));
        }
    }
    match crate::linkage::relink(c, None) {
        Ok(n) => rep.recordings_linked = n,
        Err(e) => warn(
            &mut rep.warnings,
            format!("the recordings could not be linked yet ({e}): they are at the next import or indexing"),
        ),
    }
    match crate::ehr::match_pending(c) {
        Ok(n) => rep.ehr_matched = n,
        Err(e) => warn(
            &mut rep.warnings,
            format!("waiting items could not be matched yet ({e}): they are at the next import"),
        ),
    }
    audit(c, Some(user), "import_table", &origin, &summary.to_string());
    Ok(rep)
}

/// What an import's rows add to, all of them.
struct Sink {
    rep: TableReport,
    rec_rep: crate::recordings::ImportReport,
    new_recordings: Vec<i64>,
    links: Vec<(i64, String)>,
}

/// What every row of an import is read with.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    cfg: &'a Config,
    user: &'a User,
    batch: i64,
    opts: &'a crate::imports::Opts,
    category: &'a str,
    base: Option<&'a Path>,
    zone: chrono_tz::Tz,
    /// The files' names, when there are several (warnings name the file).
    files: &'a [&'a str],
}

impl Ctx<'_> {
    /// Where a row is, as warnings say it.
    fn line(&self, note: &RowNote) -> Line<'_> {
        Line { file: self.files.get(note.part).copied(), line: note.line }
    }

    /// The rows read since the last chunk (`pending`, the last of `chunk`'s): their patients
    /// and encounters decided together ([`crate::bulk`]), then each row as `import_row` reads
    /// it, in order, then their items stored together.
    fn chunk(
        &self,
        tx: &Connection,
        pending: &mut Vec<HashMap<&'static str, String>>,
        chunk: &mut [(i64, Vec<String>, RowNote)],
        sink: &mut Sink,
        cache: &mut Cache,
    ) -> anyhow::Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let start = chunk.len() - pending.len();
        let rows = std::mem::take(pending);
        let mut wanted: Vec<String> = rows.iter().filter_map(|r| owner_wanted(r)).map(String::from).collect();
        wanted.sort();
        wanted.dedup();
        let owners = owner_of(tx, &wanted)?;
        let owner: Vec<Option<String>> =
            rows.iter().map(|r| owner_wanted(r).and_then(|e| owners.get(e).cloned())).collect();
        let mut reqs = vec![];
        let mut req_of: Vec<Option<usize>> = vec![None; rows.len()];
        for (k, r) in rows.iter().enumerate() {
            if wants_identity(r, owner[k].is_some()) {
                req_of[k] = Some(reqs.len());
                reqs.push(crate::bulk::Request {
                    source: row_source(r, self.opts),
                    row: linkage_row(r, owner[k].as_deref()),
                });
            }
        }
        let mut outcomes: Vec<Option<Outcome>> =
            crate::bulk::apply_rows(tx, self.batch, &reqs, self.zone, Some(self.user))?.into_iter().map(Some).collect();
        let mut items = vec![];
        for (k, r) in rows.iter().enumerate() {
            let (_, _, note) = &mut chunk[start + k];
            let decided = Decided { owner: owner[k].clone(), outcome: req_of[k].and_then(|j| outcomes[j].take()) };
            let mut at = Row {
                rep: &mut sink.rep,
                rec_rep: &mut sink.rec_rep,
                new_recordings: &mut sink.new_recordings,
                links: &mut sink.links,
            };
            if let Some((item, source)) = import_row(tx, self, r, note, &mut at, Some(decided))? {
                items.push((start + k, item, source));
            }
        }
        store_items(tx, self.batch, &mut items, chunk, &mut sink.rep, cache)
    }
}

/// What an import's rows add to as they go.
struct Row<'a> {
    rep: &'a mut TableReport,
    rec_rep: &'a mut crate::recordings::ImportReport,
    new_recordings: &'a mut Vec<i64>,
    links: &'a mut Vec<(i64, String)>,
}

/// What a row's patient and encounter came to, decided with the other rows of its chunk
/// ([`crate::bulk`]).
struct Decided {
    owner: Option<String>,
    outcome: Option<Outcome>,
}

/// A row's source label: its `source` column, else the import's.
fn row_source(row: &HashMap<&'static str, String>, opts: &crate::imports::Opts) -> String {
    match row.get("source").map(|s| s.trim()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => opts.source.clone(),
    }
}

/// The encounter ID of a row that names its encounter only and describes it (a table of bed
/// stays by stay): its patient is the encounter's.
fn owner_wanted<'a>(row: &'a HashMap<&'static str, String>) -> Option<&'a str> {
    let g = |k: &str| row.get(k).map(|s| s.trim()).unwrap_or("");
    (g("patient_id").is_empty()
        && g("mrn").is_empty()
        && !g("encounter_id").is_empty()
        && DESCRIBING.iter().any(|k| !g(k).is_empty()))
    .then(|| g("encounter_id"))
}

/// The patient IDs of these encounters' patients.
fn owner_of(tx: &Connection, eids: &[String]) -> crate::sql::Result<HashMap<String, String>> {
    Ok(tx
        .query_in(
            "SELECT e.encounter_id, p.patient_id FROM encounters e JOIN patients p ON p.id = e.patient
             WHERE e.encounter_id IN ({})",
            eids,
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?
        .into_iter()
        .collect())
}

/// Whether a row says who its patient (and encounter) is: a patient key, and something about
/// them, or nothing else (a row of a linkage table).
fn wants_identity(row: &HashMap<&'static str, String>, owner: bool) -> bool {
    let g = |k: &str| row.get(k).map(|s| s.trim()).unwrap_or("");
    let has_key = !g("patient_id").is_empty() || !g("mrn").is_empty() || owner;
    let has_item = ITEM.iter().any(|k| !g(k).is_empty());
    has_key && (DESCRIBING.iter().any(|k| !g(k).is_empty()) || !has_item)
}

/// A row as the linkage rules read it.
fn linkage_row(row: &HashMap<&'static str, String>, owner: Option<&str>) -> HashMap<String, String> {
    let mut lrow: HashMap<String, String> = row.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    if let Some(pid) = owner {
        lrow.insert("patient_id".into(), pid.to_string());
    }
    // the linkage rules call the care unit `unit`
    lrow.insert("unit".into(), lrow.get("care_unit").cloned().unwrap_or_default());
    lrow
}

/// One row of a table: its patient and encounter (the identity rules), its item or file.
fn import_row(
    tx: &Connection,
    ctx: &Ctx,
    row: &HashMap<&'static str, String>,
    note: &mut RowNote,
    at: &mut Row,
    decided: Option<Decided>,
) -> anyhow::Result<Option<(ItemRow, String)>> {
    let Ctx { cfg, user, batch, opts, category, base, zone, .. } = *ctx;
    let line = ctx.line(note);
    let Row { rep, rec_rep, new_recordings, links } = at;
    rep.rows += 1;
    let g = |k: &str| row.get(k).map(|s| s.trim()).unwrap_or("");
    let source = row_source(row, opts);
    let has_item = ITEM.iter().any(|k| !g(k).is_empty());
    let given = decided.is_some();
    let (mut owner, mut outcome) = match decided {
        Some(d) => (d.owner, d.outcome),
        None => (None, None),
    };
    // a row naming its encounter only (a table of bed stays by stay): the encounter's patient
    if let Some(eid) = owner_wanted(row) {
        if !given {
            owner = owner_of(tx, &[eid.to_string()])?.remove(eid);
        }
        if owner.is_none() && !has_item {
            note.status = "invalid";
            warn(
                &mut rep.warnings,
                format!("{line}: no encounter {} (import its encounters first)", g("encounter_id")),
            );
            return Ok(None);
        }
    }
    // the patient and encounter the row describes
    if wants_identity(row, owner.is_some()) {
        let lrow = linkage_row(row, owner.as_deref());
        let decided = match outcome.take() {
            Some(o) => o,
            None => apply_row(tx, batch, &source, &lrow, zone, Force::default(), Some(user))?,
        };
        match decided {
            Outcome::Applied { patient_added, patient_named, encounter_added, bed_stay_added } => {
                note.status = "identity";
                rep.patients_added += patient_added as usize;
                rep.patients_named += patient_named as usize;
                rep.encounters_added += encounter_added as usize;
                rep.bed_stays_added += bed_stay_added as usize;
            }
            Outcome::Held(problem, detail) => {
                note.status = "held";
                rep.conflicts += 1;
                tx.execute(
                    "INSERT INTO held_rows (import_id, source, line, data, problem, detail, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![batch, source, note.line as i64, serde_json::to_string(&lrow)?, problem, detail, now()],
                )?;
                warn(&mut rep.warnings, format!("{line}: {detail}; held back for review"));
                return Ok(None);
            }
            Outcome::Invalid(e) => {
                note.status = "invalid";
                warn(&mut rep.warnings, format!("{line}: {e}"));
                return Ok(None);
            }
        }
    }
    if !has_item {
        return Ok(None);
    }
    let keys = Keys {
        encounter: g("encounter_id").into(),
        csn: g("csn").into(),
        mrn: g("mrn").into(),
        authority: g("mrn_authority").into(),
        patient: g("patient_id").into(),
    };
    if keys.encounter.is_empty() && keys.csn.is_empty() && keys.mrn.is_empty() && keys.patient.is_empty() {
        note.status = "invalid";
        warn(&mut rep.warnings, format!("{line}: no patient_id, mrn, encounter_id or csn"));
        return Ok(None);
    }
    // a file: a recording, or an attachment of the item
    let mut file_path = String::new();
    if !g("file").is_empty() {
        let given = Path::new(g("file"));
        let p = match base {
            Some(b) if given.is_relative() => b.join(given),
            _ => given.to_path_buf(),
        };
        let abs = PathBuf::from(recording_engine::paths::absolute(&p.to_string_lossy()));
        if !abs.exists() {
            warn(&mut rep.warnings, format!("{line}: no such file: {}", g("file")));
        } else if !cfg.import_allowed(&abs) {
            warn(&mut rep.warnings, format!("{line}: {} is outside the configured data_roots", g("file")));
        } else if is_recording(&abs)
            && categories::named(g("category"))
                .or_else(|| categories::named(category))
                .is_none_or(categories::is_recording)
        {
            // a WFDB header is that record (docs/wfdb-collection.md, "One record"); a folder, the
            // recordings in it
            let found = crate::recordings::find_path(cfg, &abs.to_string_lossy(), &mut rec_rep.skipped);
            let paths: Vec<PathBuf> = found.iter().map(|(p, _)| crate::config::real(p)).collect();
            new_recordings.extend(crate::recordings::add_found(tx, cfg, user, found, batch, &source, rec_rep)?);
            // its column: ICU monitoring, unless the row or the table says otherwise (Holter …)
            let cat = categories::named(g("category")).or_else(|| categories::named(category)).unwrap_or("icu");
            for p in paths {
                let id: Option<i64> = tx
                    .query_row("SELECT id FROM recordings WHERE path = ?1", [p.to_string_lossy()], |r| r.get(0))
                    .optional()?;
                if let Some(id) = id {
                    note.status = "recording";
                    note.recording.get_or_insert(id);
                    tx.execute("UPDATE recordings SET category = ?1 WHERE id = ?2", params![cat, id])?;
                    if !keys.encounter.is_empty() {
                        links.push((id, keys.encounter.clone()));
                    } else if !(keys.patient.is_empty() && keys.mrn.is_empty()) {
                        // a patient alone is no link: a recording belongs to an encounter
                        warn(
                            &mut rep.warnings,
                            format!(
                                "{line}: {} names no encounter_id: it is linked only by its bed and time, else to no patient",
                                g("file")
                            ),
                        );
                    }
                }
            }
            return Ok(None);
        } else if crate::patient::file_kind(&abs.to_string_lossy()) == "other"
            && categories::named(g("category"))
                .or_else(|| categories::named(category))
                .is_none_or(categories::is_recording)
        {
            // a recording in a format no installed reader reads (one an add-on reads, not yet
            // installed), and no file a viewer opens: not kept as an attachment nothing could
            // open; the row comes in when the table is imported again
            // a WFDB signal file: its header is the recording
            let why = recording_engine::sources::wfdb_folder::signal_file_note(&abs).map_or_else(
                || {
                    "not a recording Medorama reads. A format read by an add-on needs it installed \
                     (docs/adapters.md); then remove this import and import the table again."
                        .to_string()
                },
                |n| format!("{n} (name it in the file column, then import the table again)"),
            );
            warn(&mut rep.warnings, format!("{line}: {}: {why}", g("file")));
            note.status = "skipped";
            return Ok(None);
        } else {
            file_path = crate::config::real(&abs).to_string_lossy().to_string();
            rep.attachments += 1;
        }
    }
    let parsed = (
        parse_time(g("time"), zone),
        parse_time(g("end_time"), zone),
        crate::ehr::number(g("ref_low")),
        crate::ehr::number(g("ref_high")),
    );
    let (time, end, ref_low, ref_high) = match parsed {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        (Err(e), ..) | (_, Err(e), ..) | (_, _, Err(e), _) | (.., Err(e)) => {
            note.status = "invalid";
            warn(&mut rep.warnings, format!("{line}: {e}"));
            return Ok(None);
        }
    };
    let row_domain = g("domain").to_lowercase();
    let cat = categories::resolve(g("category"), &row_domain, g("name"), category);
    let domain = if crate::ehr::DOMAINS.contains(&row_domain.as_str()) {
        row_domain
    } else {
        categories::domain_of(&cat).to_string()
    };
    let name = if !g("name").is_empty() {
        g("name").to_string()
    } else if !file_path.is_empty() {
        Path::new(&file_path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        categories::label(&cat).to_string()
    };
    let item = ItemRow {
        keys,
        domain,
        category: cat,
        name,
        time,
        end,
        vocabulary: g("vocabulary").into(),
        code: g("code").into(),
        value: g("value").into(),
        text: g("text").into(),
        unit: g("unit").into(),
        ref_low,
        ref_high,
        flag: g("flag").into(),
        author: g("author").into(),
        service: g("service").into(),
        file_path,
    };
    Ok(Some((item, source)))
}

/// What became of a row's item, counted and kept with the row.
fn counted(rep: &mut TableReport, note: &mut RowNote, stored: Stored, item_id: i64) {
    note.item = Some(item_id);
    note.status = if matches!(stored, Stored::Duplicate) { "duplicate" } else { "item" };
    match stored {
        Stored::Duplicate => rep.duplicates += 1,
        Stored::WithEncounter => {
            rep.items_added += 1;
            rep.with_encounter += 1
        }
        Stored::PatientLevel => {
            rep.items_added += 1;
            rep.patient_level += 1
        }
        Stored::Waiting => {
            rep.items_added += 1;
            rep.waiting += 1
        }
    }
}

/// A chunk's items stored (many to a statement: [`crate::ehr::insert_items`]), each counted
/// with its row.
fn store_items(
    tx: &Connection,
    batch: i64,
    items: &mut Vec<(usize, ItemRow, String)>,
    chunk: &mut [(i64, Vec<String>, RowNote)],
    rep: &mut TableReport,
    cache: &mut Cache,
) -> anyhow::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let (at, rows): (Vec<usize>, Vec<(ItemRow, String)>) = items.drain(..).map(|(i, it, s)| (i, (it, s))).unzip();
    for (i, (stored, id)) in at.into_iter().zip(crate::ehr::insert_items(tx, batch, &rows, cache)?) {
        counted(rep, &mut chunk[i].2, stored, id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_from_headers() {
        let h: Vec<String> =
            ["MRN", "Test Name", "Result", "Units", "Collected", "Ref Low", "Ref High", "Abnormal Flag"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let m = guess_mapping(&h);
        assert_eq!(m["mrn"], "MRN");
        assert_eq!(m["name"], "Test Name");
        assert_eq!(m["value"], "Result");
        assert_eq!(m["unit"], "Units");
        assert_eq!(m["time"], "Collected");
        assert_eq!(m["ref_low"], "Ref Low");
        assert_eq!(m["flag"], "Abnormal Flag");
        let h: Vec<String> =
            ["patient_id", "encounter_id", "unit", "bed", "bed_start"].iter().map(|s| s.to_string()).collect();
        let m = guess_mapping(&h);
        assert_eq!((m["care_unit"].as_str(), m["unit"].as_str()), ("unit", ""));
        assert_eq!(guess_category("ecg_index.csv", &[]), "ecg");
        assert_eq!(guess_category("data.csv", &["note_text".into()]), "");
    }
}
