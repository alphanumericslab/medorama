//! Tables (docs/importing.md is the reference for the recognised columns): a CSV file, a Parquet
//! file, or the first sheet of a spreadsheet (Excel `.xlsx`/`.xls`, OpenDocument `.ods`), read
//! row by row (a table of millions of rows is never held whole; a spreadsheet is read whole, and
//! is small).  A table's columns are mapped once onto the recognised columns ([`GROUPS`]): by
//! their names ([`guess_mapping`]) or by a mapping file ([`read_mapping`]).  Each row may then
//! carry, in any combination, a patient, an encounter (with a bed stay), an entry, and a file
//! ([`crate::add`] writes them).

use crate::categories;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

/// The recognised columns, by group: what a table's columns are mapped onto.
pub const GROUPS: &[(&str, &[&str])] = &[
    (
        "Patient",
        &["patient_id", "patient_id_issuer", "mrn", "mrn_authority", "last_name", "first_name", "birth_date", "sex"],
    ),
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
    ("patient_id_issuer", &["patientidissuer", "issuer", "idissuer"]),
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

/// A cell as text: dates as `YYYY-MM-DD HH:MM:SS` (read in the database's time zone), whole
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

/// The largest spreadsheet read (a spreadsheet is read whole; a larger table is a CSV file).
pub const MAX_SPREADSHEET: u64 = 100 << 20;

/// A sheet's header, and its rows with their line numbers.
type Sheet = (Vec<String>, Vec<(u64, Vec<String>)>);

/// A spreadsheet's first sheet: its header, and its rows with their line numbers.  The reader
/// (calamine) can panic on a damaged file: that is an error of its `add`, never a crash.
fn sheet(data: &[u8]) -> anyhow::Result<Sheet> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sheet_now(data)))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the spreadsheet is damaged (it cannot be read)")))
}

fn sheet_now(data: &[u8]) -> anyhow::Result<Sheet> {
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
/// by commas).  db-builder reads comma-separated UTF-8.
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

/// The mapping a table's headers suggest: recognised column -> header ("" when none).
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

/// A mapping from a TOML text (`[columns] field = "header"`, or the same keys at the top level
/// of a file that holds nothing else) or a JSON object (`{"field": "header"}`).
pub fn read_mapping(text: &str) -> anyhow::Result<HashMap<String, String>> {
    if text.trim_start().starts_with('{') {
        let v: HashMap<String, String> = serde_json::from_str(text)?;
        return Ok(v.into_iter().filter(|(_, h)| !h.is_empty()).collect());
    }
    let t: toml::Table = text.parse()?;
    let mut m = HashMap::new();
    // the columns in a [columns] table, or (a file of nothing else) at the top level
    let (cols, prefix) = match t.get("columns").and_then(|v| v.as_table()) {
        Some(cols) => (cols, "columns."),
        None => (&t, ""),
    };
    for (k, v) in cols {
        m.insert(k.clone(), v.as_str().ok_or_else(|| anyhow::anyhow!("{prefix}{k} must be text"))?.to_string());
    }
    let known = columns();
    if let Some(k) = m.keys().find(|k| !known.contains(&k.as_str())) {
        anyhow::bail!("mapping: unknown column {k:?} (known: {})", known.join(", "));
    }
    Ok(m)
}

/// The recognised column each of a table's columns is read as ("" when none): a mapped header,
/// or a header named as the recognised column.  A recognised column is read from the first
/// column that names it; a second one (`value,value`) goes to `extras`, with a warning.
pub fn column_map(
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
            warnings.push(format!("the column {h} appears twice: the first is read as {k}, this one goes to extras"));
            maps.push("");
        } else {
            maps.push(k);
        }
    }
    (maps, warnings)
}

/// A table's rows as they come, each with its line in its file.
pub type LineRows = Box<dyn Iterator<Item = anyhow::Result<(u64, Vec<String>)>>>;

fn is_spreadsheet(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04") || head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
}

/// What a table file is, by its content (and a spreadsheet by its extension): `csv`,
/// `parquet`, `xlsx`, `xls`, `ods`.
pub fn kind_of(path: &Path) -> anyhow::Result<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 4];
    let n = std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?.read(&mut head)?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    Ok(if crate::parquet_rows::is_parquet(&head[..n]) {
        "parquet"
    } else if is_spreadsheet(&head[..n]) {
        match ext.as_str() {
            "ods" => "ods",
            "xls" => "xls",
            _ => "xlsx",
        }
    } else {
        "csv"
    })
}

/// A file's columns and its rows, read one by one, each with its line in the file: a CSV file
/// row by row; a spreadsheet's first sheet whole; a Parquet file's rows by number.
pub fn rows_of(path: &Path) -> anyhow::Result<(Vec<String>, LineRows)> {
    use std::io::{BufRead, Read};
    let open = || std::fs::File::open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()));
    let mut head = [0u8; 4];
    let n = open()?.read(&mut head)?;
    if is_spreadsheet(&head[..n]) {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size > MAX_SPREADSHEET {
            anyhow::bail!(
                "{}: the spreadsheet is larger than {} MB: a spreadsheet is read whole; save the table as CSV (read row by row)",
                path.display(),
                MAX_SPREADSHEET >> 20
            );
        }
        let (headers, rows) = sheet(&std::fs::read(path)?)?;
        return Ok((headers, Box::new(rows.into_iter().map(Ok))));
    }
    if crate::parquet_rows::is_parquet(&head[..n]) {
        let (headers, rows, _) = crate::parquet_rows::open(path)?;
        // (a row's number, after the header's line, as a CSV file's would be)
        return Ok((headers, Box::new(rows.enumerate().map(|(i, r)| r.map(|cells| (i as u64 + 2, cells))))));
    }
    let mut reader: Box<dyn BufRead> = Box::new(std::io::BufReader::with_capacity(1 << 20, open()?));
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
    Ok((headers, Box::new(rows)))
}

/// Measurements a table (ECG …) may give as columns, with their usual unit.
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

/// A column's measurement name and unit: `QT (ms)`, `Lactate [mmol/L]`, or a known name alone
/// (`QTc`, `R axis`).  Its numbers become entries of their own, in that unit.
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
        let m = guess_mapping(&["Patient ID".to_string(), "Issuer".to_string()]);
        assert_eq!(m["patient_id_issuer"], "Issuer");
    }

    #[test]
    fn measurement_columns() {
        assert_eq!(measure_column("QT (ms)"), Some(("QT".into(), "ms".into())));
        assert_eq!(measure_column("Lactate [mmol/L]"), Some(("Lactate".into(), "mmol/L".into())));
        assert_eq!(measure_column("QTc"), Some(("QTc".into(), "ms".into())));
        assert_eq!(measure_column("race"), None);
    }

    #[test]
    fn a_mapping_in_a_table_or_at_the_top_level() {
        let a = read_mapping("[columns]\nmrn = \"Hosp No\"\nname = \"Analyte\"\n").unwrap();
        let b = read_mapping("mrn = \"Hosp No\"\nname = \"Analyte\"\n").unwrap();
        assert_eq!(a, b);
        assert_eq!(a["mrn"], "Hosp No");
        assert!(read_mapping("mrn = 3\n").is_err());
        assert!(read_mapping("colour = \"x\"\n").is_err());
        assert_eq!(read_mapping("{\"mrn\": \"Hosp No\"}").unwrap()["mrn"], "Hosp No");
    }
}
