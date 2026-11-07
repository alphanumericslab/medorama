//! Identifiers a recording holds in its own file (docs/administration.md, "Identifiers in
//! recordings"): the built-in action `identify`, queued for every recording automatically.
//!
//! The adapter that reads a recording's format reads its identifiers too (`identify`,
//! docs/adapters.md): names, patient and encounter IDs, birth date and sex, as the file states
//! them. WFDB folders hold none; EDF and BDF files hold them in their header's patient and
//! recording fields, read here ([`from_edf`]). The values are PHI: they are stored only in
//! `recording_identifiers`, shown only to users with the PHI permission, beside the database's
//! values, to cross-check them. Job summaries never contain them.

use crate::sql::{params, Connection};
use serde::Serialize;
use signal_core::value::{Dict, Value};
use std::path::Path;

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Found {
    pub full_name: String,
    pub last_name: String,
    pub first_name: String,
    pub patient_id: String,
    pub encounter_id: String,
    pub birth_date: String,
    pub sex: String,
}

impl Found {
    fn from_dict(d: &Dict) -> Option<Found> {
        let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").replace('\0', "").trim().to_string();
        let f = Found {
            full_name: s("full_name"),
            last_name: s("last_name"),
            first_name: s("first_name"),
            patient_id: s("patient_id"),
            encounter_id: s("encounter_id"),
            birth_date: s("birth_date"),
            sex: s("sex"),
        };
        (f != Found::default()).then_some(f)
    }
}

/// The identifiers in a recording: the first found, and any different ones after it.
pub fn read(path: &Path, kind: &str) -> Result<(Option<Found>, Vec<Found>), String> {
    if kind == "wfdb" {
        return Ok((None, vec![]));
    }
    if kind == "edf" {
        let (patient, recording) = recording_engine::sources::edf::identification(path).map_err(|e| e.to_string())?;
        return Ok((from_edf(&patient, &recording), vec![]));
    }
    let Some((adapter, _)) = recording_engine::adapters::for_file(path) else {
        return Err(format!("no adapter installed reads {kind} recordings"));
    };
    let answers = adapter.identify(&[path.to_path_buf()]).map_err(|e| e.to_string())?;
    let Some(answer) = answers.into_values().next() else { return Ok((None, vec![])) };
    if let Some(e) = answer.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    let first = answer.get("identifiers").and_then(Value::as_dict).and_then(Found::from_dict);
    let others: Vec<Found> = answer
        .get("others")
        .and_then(Value::as_list)
        .map(|l| l.iter().filter_map(|x| x.as_dict().and_then(Found::from_dict)).take(10).collect())
        .unwrap_or_default();
    Ok((first, others))
}

/// The identifiers of an EDF or BDF header's patient and recording fields.  EDF+ writes them as
/// `code sex birthdate name` (`MCH-0234567 F 02-MAY-1951 Haagse_Harry`, `X` where unknown, `_`
/// for spaces) and `Startdate dd-MMM-yyyy admincode ...` (the admin code: the encounter); a plain
/// EDF patient field is free text, kept as the patient ID.
pub fn from_edf(patient: &str, recording: &str) -> Option<Found> {
    let known = |w: Option<&str>| w.filter(|w| *w != "X").map(|w| w.replace('_', " ")).unwrap_or_default();
    let words: Vec<&str> = patient.split(' ').filter(|w| !w.is_empty()).collect();
    let rec: Vec<&str> = recording.split(' ').filter(|w| !w.is_empty()).collect();
    let plus = words.len() >= 4 && matches!(words[1], "F" | "M" | "X") && (words[2] == "X" || words[2].len() == 11);
    let f = if plus {
        let sex = match words[1] {
            "F" => "female",
            "M" => "male",
            _ => "",
        };
        Found {
            patient_id: known(Some(words[0])),
            sex: sex.into(),
            birth_date: edf_date(words[2]).unwrap_or_default(),
            full_name: known(Some(words[3])),
            encounter_id: if rec.first() == Some(&"Startdate") { known(rec.get(2).copied()) } else { String::new() },
            ..Found::default()
        }
    } else {
        Found { patient_id: patient.trim().to_string(), ..Found::default() }
    };
    (f != Found::default()).then_some(f)
}

/// `02-MAY-1951` as `1951-05-02`.
fn edf_date(d: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    let (day, rest) = d.split_once('-')?;
    let (mon, year) = rest.split_once('-')?;
    let m = MONTHS.iter().position(|x| x.eq_ignore_ascii_case(mon))? + 1;
    (day.len() == 2 && year.len() == 4 && day.bytes().chain(year.bytes()).all(|c| c.is_ascii_digit()))
        .then(|| format!("{year}-{m:02}-{day}"))
}

/// Read and store a recording's identifiers; returns a summary without any identifier.
pub fn identify(c: &Connection, recording: i64, path: &Path, kind: &str) -> Result<serde_json::Value, String> {
    let t = crate::db::now();
    let result = read(path, kind);
    let (state, f, others, error) = match &result {
        Ok((Some(f), o)) => ("found", f.clone(), o.clone(), String::new()),
        Ok((None, _)) => ("none", Found::default(), vec![], String::new()),
        Err(e) => ("error", Found::default(), vec![], e.clone()),
    };
    c.execute(
        "INSERT INTO recording_identifiers (recording, state, full_name, last_name, first_name, patient_id, encounter_id,
                                            birth_date, sex, other_values, error, read_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (recording) DO UPDATE SET state = excluded.state, full_name = excluded.full_name,
           last_name = excluded.last_name, first_name = excluded.first_name, patient_id = excluded.patient_id,
           encounter_id = excluded.encounter_id, birth_date = excluded.birth_date, sex = excluded.sex,
           other_values = excluded.other_values, error = excluded.error, read_at = excluded.read_at",
        params![
            recording,
            state,
            f.full_name,
            f.last_name,
            f.first_name,
            f.patient_id,
            f.encounter_id,
            f.birth_date,
            f.sex,
            if others.is_empty() { String::new() } else { serde_json::to_string(&others).unwrap_or_default() },
            error,
            t
        ],
    )
    .map_err(|e| e.to_string())?;
    result?;
    Ok(serde_json::json!({ "state": state, "several_patients": !others.is_empty() }))
}

/// What a PHI user sees for a recording: the identifiers in its file, and how they compare
/// with its encounter's patient (true: agree, false: differ, null: one side unknown).
#[derive(Debug, Clone, Serialize)]
pub struct InFile {
    pub state: String,
    pub full_name: String,
    pub last_name: String,
    pub first_name: String,
    pub patient_id: String,
    pub encounter_id: String,
    pub birth_date: String,
    pub sex: String,
    pub others: serde_json::Value,
    pub mrn_matches: Option<bool>,
    pub name_matches: Option<bool>,
    pub birth_date_matches: Option<bool>,
}

fn norm(x: &str) -> String {
    x.trim().trim_start_matches('0').to_lowercase()
}

/// The identifiers in a recording's file, compared with a patient's (mrn, last, first, birth).
pub fn in_file(
    c: &Connection,
    recording: i64,
    patient: Option<(&str, &str, &str, &str)>,
) -> anyhow::Result<Option<InFile>> {
    let row = crate::sql::OptionalExtension::optional(c.query_row(
        "SELECT state, full_name, last_name, first_name, patient_id, encounter_id, birth_date, sex, other_values
         FROM recording_identifiers WHERE recording = ?1",
        [recording],
        |r| {
            Ok(InFile {
                state: r.get(0)?,
                full_name: r.get(1)?,
                last_name: r.get(2)?,
                first_name: r.get(3)?,
                patient_id: r.get(4)?,
                encounter_id: r.get(5)?,
                birth_date: r.get(6)?,
                sex: r.get(7)?,
                others: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or(serde_json::Value::Null),
                mrn_matches: None,
                name_matches: None,
                birth_date_matches: None,
            })
        },
    ))?;
    let Some(mut f) = row else { return Ok(None) };
    if let Some((mrn, last, first, dob)) = patient {
        if !f.patient_id.is_empty() && !mrn.trim().is_empty() {
            f.mrn_matches = Some(norm(&f.patient_id) == norm(mrn));
        }
        let file_name = format!("{} {} {}", f.full_name, f.last_name, f.first_name).to_lowercase();
        if !file_name.trim().is_empty() && !last.trim().is_empty() {
            f.name_matches = Some(
                file_name.contains(&last.trim().to_lowercase())
                    && (first.trim().is_empty() || file_name.contains(&first.trim().to_lowercase())),
            );
        }
        if !f.birth_date.is_empty() && !dob.trim().is_empty() {
            f.birth_date_matches = Some(f.birth_date == dob.trim());
        }
    }
    Ok(Some(f))
}

/// What the files of an encounter's recordings say, for its row in the Encounters list (PHI
/// users only): the identities found, and how many files agree with the patient.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FilesSummary {
    /// The distinct identities found, as "name · ID · born date" (at most 5).
    pub found: Vec<String>,
    /// Files that agree with the linkage table (no value differs, one at least compared).
    pub agree: usize,
    /// Files with a value that differs, or naming more than one patient.
    pub differ: usize,
    /// Files with identifiers that could not be compared (the linkage table lacks the values).
    pub unchecked: usize,
    /// Files holding no identifiers (or unreadable).
    pub none: usize,
    /// Files not read yet.
    pub unread: usize,
}

fn identity(f: &InFile) -> String {
    let last_first = [f.last_name.as_str(), f.first_name.as_str()]
        .iter()
        .filter(|x| !x.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let name = if f.full_name.is_empty() { last_first } else { f.full_name.clone() };
    let mut parts = vec![];
    if !name.is_empty() {
        parts.push(name);
    }
    if !f.patient_id.is_empty() {
        parts.push(format!("ID {}", f.patient_id));
    }
    if !f.birth_date.is_empty() {
        parts.push(format!("born {}", f.birth_date));
    }
    parts.join(" · ")
}

/// The summary for an encounter, whose patient is (mrn, last, first, birth).
pub fn files_summary(
    c: &Connection,
    encounter: i64,
    patient: Option<(&str, &str, &str, &str)>,
) -> anyhow::Result<FilesSummary> {
    let ids: Vec<i64> = c
        .prepare("SELECT recording FROM recording_links WHERE encounter = ?1 ORDER BY recording")?
        .query_map([encounter], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut s = FilesSummary::default();
    for id in ids {
        let Some(f) = in_file(c, id, patient)? else {
            s.unread += 1;
            continue;
        };
        if f.state != "found" {
            s.none += 1;
            continue;
        }
        // one identity in two notations (`DOE^JANE`, `Doe, Jane`) is listed once
        let key = |x: &str| {
            x.to_lowercase()
                .split(|ch: char| !ch.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let i = identity(&f);
        if !i.is_empty() && !s.found.iter().any(|x| key(x) == key(&i)) && s.found.len() < 5 {
            s.found.push(i);
        }
        let marks = [f.mrn_matches, f.name_matches, f.birth_date_matches];
        let several = f.others.as_array().is_some_and(|o| !o.is_empty());
        if several || marks.contains(&Some(false)) {
            s.differ += 1;
        } else if marks.contains(&Some(true)) {
            s.agree += 1;
        } else {
            s.unchecked += 1;
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edf_fields() {
        // the EDF+ specification's own example; X where unknown, _ for spaces
        let f =
            from_edf("MCH-0234567 F 02-MAY-1951 Haagse_Harry", "Startdate 02-MAR-2002 EMG561 BK/JOP Sony.").unwrap();
        assert_eq!(
            (
                f.patient_id.as_str(),
                f.sex.as_str(),
                f.birth_date.as_str(),
                f.full_name.as_str(),
                f.encounter_id.as_str()
            ),
            ("MCH-0234567", "female", "1951-05-02", "Haagse Harry", "EMG561")
        );
        assert_eq!(from_edf("X X X X", "Startdate X X X X"), None);
        let f = from_edf("X M X X", "Startdate 02-MAR-2002 X X X").unwrap();
        assert_eq!((f.sex.as_str(), f.patient_id.as_str(), f.full_name.as_str()), ("male", "", ""));
        // plain EDF: free text
        assert_eq!(from_edf("subject 12", "").unwrap().patient_id, "subject 12");
        // the engine reads the fields; its index never holds them (libs/engine/tests/edf.rs)
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../viewers/vitalstream/testdata/edf/psg_plus.edf");
        let (found, others) = read(Path::new(p), "edf").unwrap();
        let f = found.unwrap();
        assert_eq!(
            (f.patient_id.as_str(), f.birth_date.as_str(), f.full_name.as_str()),
            ("MO-TEST-0001", "1970-01-01", "Synthetic Subject")
        );
        assert!(others.is_empty());
    }
}
