//! GE MUSE ECG metadata: one row per resting 12-lead ECG, the elements of its MUSE XML file as
//! columns (`filename`, `PatientDemographics/PatientID`, `TestDemographics/AcquisitionDate`,
//! `RestingECGMeasurements/QTCorrected`, `Diagnosis/DiagnosisStatement/0/StmtText`, ...): the
//! form a MUSE export's XML files are flattened to.  Tables of one export may differ in their
//! columns (an element present in some files only); each is read by its own header.
//!
//! Kept: the patient (ID, names, date of birth, sex), when (acquisition date and time), the
//! file, the test's status, priority, reason, site, location, device, type, the patient's race,
//! the measurements (edited and confirmed: `RestingECGMeasurements`), the confirmed statements
//! and the computer's, each read as MUSE prints them (fragments joined until one ends the line).
//! Left out: the beat list and sample scale (the ECG viewer reads the XML), the original
//! measurements, the 0/1 flags, and everything that names staff or the visit (physicians,
//! technicians, editors, account and requisition numbers, rooms, beds): Privacy Mode masks
//! patients' identifiers, not those.

use super::{Converter, Format};
use chrono::{NaiveDate, NaiveTime};

/// The format: the patient by MRN (`ge-muse`, recognised by its columns), or by a patient ID
/// (`ge-muse-patient-id`, named).
pub struct Muse {
    by_patient_id: bool,
}

impl Muse {
    pub fn new(by_patient_id: bool) -> Muse {
        Muse { by_patient_id }
    }
}

impl Format for Muse {
    fn key(&self) -> &str {
        if self.by_patient_id {
            "ge-muse-patient-id"
        } else {
            "ge-muse"
        }
    }
    fn label(&self) -> &str {
        if self.by_patient_id {
            "GE MUSE ECG metadata, the patient by a patient ID"
        } else {
            "GE MUSE ECG metadata"
        }
    }
    fn category(&self) -> &str {
        "ecg"
    }
    fn about(&self) -> String {
        format!(
            "One row per resting ECG, its MUSE XML file's elements as columns ({FILE}, {ID}, {DATE}, RestingECGMeasurements/…, \
             Diagnosis/DiagnosisStatement/<n>/StmtText …). Kept: the patient (by {}), the time, the file, the measurements with \
             their units, the confirmed and the computer's statements, the test's status, site, location and device. Left out: \
             staff, visits' numbers, rooms, beds, the beat list.",
            if self.by_patient_id { "patient ID" } else { "MRN" }
        )
    }
    fn detect(&self, h: &[String]) -> bool {
        !self.by_patient_id && is_muse(h)
    }
    fn start(&self, h: &[String]) -> anyhow::Result<Box<dyn Converter>> {
        start(h, if self.by_patient_id { "patient_id" } else { "mrn" })
    }
}

const ID: &str = "PatientDemographics/PatientID";
const FILE: &str = "filename";
const DATE: &str = "TestDemographics/AcquisitionDate";

fn is_muse(h: &[String]) -> bool {
    [FILE, ID, DATE].iter().all(|c| h.iter().any(|x| x == c))
}

/// (MUSE element under RestingECGMeasurements, the column: its unit in its name)
const MEASURES: &[(&str, &str)] = &[
    ("VentricularRate", "Vent. rate (bpm)"),
    ("AtrialRate", "Atrial rate (bpm)"),
    ("PRInterval", "PR (ms)"),
    ("QRSDuration", "QRS (ms)"),
    ("QTInterval", "QT (ms)"),
    ("QTCorrected", "QTc (ms)"),
    ("QTcFrederica", "QTcF (ms)"),
    ("PAxis", "P axis (°)"),
    ("RAxis", "R axis (°)"),
    ("TAxis", "T axis (°)"),
    ("SystolicBP", "Systolic BP (mmHg)"),
    ("DiastolicBP", "Diastolic BP (mmHg)"),
];

/// (element, column): what the test is, where and how it was taken
const TEST: &[(&str, &str)] = &[
    ("TestDemographics/Status", "status"),
    ("TestDemographics/Priority", "priority"),
    ("TestDemographics/TestReason", "test reason"),
    ("TestDemographics/SiteName", "site"),
    ("TestDemographics/LocationName", "location"),
    ("TestDemographics/AcquisitionDevice", "device"),
    ("TestDemographics/DataType", "data type"),
    ("PatientDemographics/Race", "race"),
];

/// A section's statements in order: (the text's column, the flags' columns).
type Statements = Vec<(Option<usize>, Vec<usize>)>;

struct MuseRows {
    key: &'static str,
    at: std::collections::HashMap<String, usize>,
    confirmed: Statements,
    computer: Statements,
}

fn start(h: &[String], key: &'static str) -> anyhow::Result<Box<dyn Converter>> {
    if !is_muse(h) {
        anyhow::bail!("not a GE MUSE metadata table: it needs the columns {FILE}, {ID} and {DATE}");
    }
    Ok(Box::new(MuseRows {
        key,
        at: h.iter().enumerate().map(|(i, c)| (c.trim().to_string(), i)).collect(),
        confirmed: statements(h, "Diagnosis"),
        computer: statements(h, "OriginalDiagnosis"),
    }))
}

/// Where a section's statements are: `<section>/DiagnosisStatement[/<n>]/StmtText` and its
/// `StmtFlag[/<k>]` columns, in statement order (an unnumbered one first).
fn statements(h: &[String], section: &str) -> Statements {
    let prefix = format!("{section}/DiagnosisStatement");
    let mut by_n: std::collections::BTreeMap<i64, (Option<usize>, Vec<usize>)> = Default::default();
    for (i, c) in h.iter().enumerate() {
        let Some(rest) = c.trim().strip_prefix(&prefix) else { continue };
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        let (n, what) = match parts.as_slice() {
            [w] | [w, _] if w.starts_with("Stmt") => (-1, *w),
            [n, w] | [n, w, _] => match n.parse::<i64>() {
                Ok(n) => (n, *w),
                Err(_) => continue,
            },
            _ => continue,
        };
        let e = by_n.entry(n).or_default();
        match what {
            "StmtText" => e.0 = Some(i),
            "StmtFlag" => e.1.push(i),
            _ => {}
        }
    }
    by_n.into_values().collect()
}

impl MuseRows {
    fn get<'a>(&self, r: &'a [String], col: &str) -> &'a str {
        self.at.get(col).and_then(|i| r.get(*i)).map(|s| s.trim()).unwrap_or("")
    }

    /// A section as MUSE prints it: fragments joined until a statement ends the line (a flag
    /// ENDSLINE), lines on lines of their own.
    fn read(st: &Statements, r: &[String]) -> String {
        let (mut lines, mut cur): (Vec<String>, Vec<&str>) = (vec![], vec![]);
        for (text, flags) in st {
            let t = text.and_then(|i| r.get(i)).map(|s| s.trim()).unwrap_or("");
            if !t.is_empty() {
                cur.push(t);
            }
            let ends = flags.iter().any(|i| r.get(*i).is_some_and(|f| f.trim().eq_ignore_ascii_case("ENDSLINE")));
            if ends && !cur.is_empty() {
                lines.push(cur.join(" "));
                cur.clear();
            }
        }
        if !cur.is_empty() {
            lines.push(cur.join(" "));
        }
        lines.join("\n")
    }
}

/// MUSE's MM-DD-YYYY (or MM/DD/YYYY, ISO) as YYYY-MM-DD.
fn date(s: &str) -> Option<String> {
    ["%m-%d-%Y", "%m/%d/%Y", "%Y-%m-%d"]
        .iter()
        .find_map(|f| NaiveDate::parse_from_str(s.trim(), f).ok())
        // a year of four digits only (`03-04-24` is no date: which century?)
        .filter(|d| {
            (1000..=9999).contains(&chrono::Datelike::year(d)) && s.contains(&chrono::Datelike::year(d).to_string())
        })
        .map(|d| d.format("%Y-%m-%d").to_string())
}

fn time(s: &str) -> Option<String> {
    ["%H:%M:%S", "%H:%M"]
        .iter()
        .find_map(|f| NaiveTime::parse_from_str(s.trim(), f).ok())
        .map(|t| t.format("%H:%M:%S").to_string())
}

fn sex(s: &str) -> &'static str {
    match s.trim().to_uppercase().as_str() {
        "MALE" | "M" => "M",
        "FEMALE" | "F" => "F",
        _ => "",
    }
}

/// A measurement, if it is a number (MUSE leaves some empty).
fn number(s: &str) -> String {
    let s = s.trim();
    if s.parse::<f64>().is_ok_and(f64::is_finite) {
        s.to_string()
    } else {
        String::new()
    }
}

impl Converter for MuseRows {
    fn headers(&self) -> Vec<String> {
        let mut h: Vec<String> = [self.key, "last_name", "first_name", "birth_date", "sex", "time", "file"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        h.extend(TEST.iter().map(|(_, c)| c.to_string()));
        h.extend(MEASURES.iter().map(|(_, c)| c.to_string()));
        h.extend(["interpretation".to_string(), "computer interpretation".to_string()]);
        h
    }

    fn row(&mut self, r: &[String]) -> Result<Vec<String>, String> {
        let id = self.get(r, ID);
        if id.is_empty() {
            return Err("rows without a patient ID (PatientDemographics/PatientID)".into());
        }
        let file = self.get(r, FILE);
        if file.is_empty() {
            return Err("rows without a file (filename)".into());
        }
        let when = match (date(self.get(r, DATE)), time(self.get(r, "TestDemographics/AcquisitionTime"))) {
            (Some(d), Some(t)) => format!("{d} {t}"),
            (Some(d), None) => format!("{d} 00:00:00"),
            (None, _) => String::new(),
        };
        let mut out = vec![
            id.to_string(),
            self.get(r, "PatientDemographics/PatientLastName").to_string(),
            self.get(r, "PatientDemographics/PatientFirstName").to_string(),
            date(self.get(r, "PatientDemographics/DateofBirth")).unwrap_or_default(),
            sex(self.get(r, "PatientDemographics/Gender")).to_string(),
            when,
            file.to_string(),
        ];
        out.extend(TEST.iter().map(|(e, _)| self.get(r, e).to_string()));
        out.extend(MEASURES.iter().map(|(m, _)| number(self.get(r, &format!("RestingECGMeasurements/{m}")))));
        out.push(MuseRows::read(&self.confirmed, r));
        out.push(MuseRows::read(&self.computer, r));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(cols: &[&str]) -> Vec<String> {
        cols.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn a_muse_row_converted() {
        let h = table(&[
            "filename",
            "PatientDemographics/PatientID",
            "PatientDemographics/PatientLastName",
            "PatientDemographics/DateofBirth",
            "PatientDemographics/Gender",
            "TestDemographics/AcquisitionDate",
            "TestDemographics/AcquisitionTime",
            "RestingECGMeasurements/QTCorrected",
            "RestingECGMeasurements/PRInterval",
            "TestDemographics/OrderingMDLastName",
            "Diagnosis/DiagnosisStatement/0/StmtText",
            "Diagnosis/DiagnosisStatement/1/StmtText",
            "Diagnosis/DiagnosisStatement/1/StmtFlag/0",
            "Diagnosis/DiagnosisStatement/1/StmtFlag/1",
            "Diagnosis/DiagnosisStatement/2/StmtText",
            "Diagnosis/DiagnosisStatement/2/StmtFlag",
            "OriginalDiagnosis/DiagnosisStatement/StmtText",
            "OriginalDiagnosis/DiagnosisStatement/StmtFlag",
            "QRSTimesTypes/QRS/0/Time",
        ]);
        let by_mrn = Muse::new(false);
        assert!(by_mrn.detect(&h) && !Muse::new(true).detect(&h), "only the default variant is recognised");
        let mut c = by_mrn.start(&h).unwrap();
        let r = table(&[
            "/ecg/a.xml",
            "SYN0001",
            "Testperson",
            "03-04-1950",
            "FEMALE",
            "01-02-2024",
            "08:15",
            "438",
            "n/a",
            "Zorbluff",
            "Sinus rhythm",
            "with 1st degree A-V block",
            "ENDSLINE",
            "USERINSERT",
            "Abnormal ECG",
            "ENDSLINE",
            "Normal sinus rhythm",
            "ENDSLINE",
            "512",
        ]);
        let out = c.row(&r).unwrap();
        let v = |col: &str| out[c.headers().iter().position(|h| h == col).unwrap()].clone();
        assert_eq!(
            (v("mrn"), v("last_name"), v("birth_date"), v("sex")),
            ("SYN0001".into(), "Testperson".into(), "1950-03-04".into(), "F".into())
        );
        assert_eq!(v("time"), "2024-01-02 08:15:00");
        assert_eq!((v("QTc (ms)"), v("PR (ms)")), ("438".into(), String::new()));
        assert_eq!(v("interpretation"), "Sinus rhythm with 1st degree A-V block\nAbnormal ECG");
        assert_eq!(v("computer interpretation"), "Normal sinus rhythm");
        let all = out.join("|");
        assert!(!all.contains("Zorbluff") && !all.contains("512"), "no staff, no beat list: {all}");
        let mut empty = r.clone();
        empty[1] = String::new();
        assert!(c.row(&empty).is_err());
        assert_eq!(Muse::new(true).start(&h).unwrap().headers()[0], "patient_id");
        assert!(by_mrn.start(&table(&["a", "b"])).is_err());
    }
}
