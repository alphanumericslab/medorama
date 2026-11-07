//! Databases and the patient page (docs/administration.md, "Databases"; docs/design/patient-page.md):
//! one import for every table (CSV or spreadsheet), rows carrying patients, encounters, items,
//! recordings and attached files; the patient list, summary, journey, report, search and
//! detail; what users without the PHI permission see; removing a database everywhere.

mod common;
use common::{bed_name, wfdb_record, T0};
use medorama::config::Config;
use medorama::db::Db;
use medorama::imports::{self, Opts};
use medorama::sql::Connection;
use medorama::users::{Role, User};
use medorama::{patient, tables};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    cfg.data_roots = vec![root.join("data").to_string_lossy().to_string()];
    std::fs::create_dir_all(root.join("data")).unwrap();
    let db = Db::open(&cfg, true).unwrap();
    Fx { _tmp: tmp, root, cfg, db }
}

fn admin() -> User {
    User::local()
}

fn reader() -> User {
    User {
        id: 0,
        username: "reader".into(),
        display_name: String::new(),
        role: Role::Viewer,
        phi: false,
        disabled: false,
        phi_allowed: false,
        privacy: false,
        privacy_notes: false,
    }
}

fn import(f: &Fx, c: &mut Connection, name: &str, data: &[u8], category: &str) -> tables::TableReport {
    let base = f.root.join("data");
    tables::import(c, &f.cfg, &admin(), name, data, &HashMap::new(), category, Some(&base), &Opts::new(name, "", false))
        .unwrap()
}

/// A one-sheet spreadsheet of text cells (inline strings).
fn xlsx(rows: &[&[&str]]) -> Vec<u8> {
    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    for (r, row) in rows.iter().enumerate() {
        sheet.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (i, v) in row.iter().enumerate() {
            sheet.push_str(&format!(
                r#"<c r="{}{}" t="inlineStr"><is><t>{v}</t></is></c>"#,
                (b'A' + i as u8) as char,
                r + 1
            ));
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");
    let files = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
        ("xl/workbook.xml", r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string()),
        ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string()),
        ("xl/worksheets/sheet1.xml", sheet),
    ];
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut out);
        for (name, text) in files {
            z.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(text.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    out.into_inner()
}

/// The demo patient: identity, an encounter with an ICU stay, labs, notes, an ECG picture, a
/// recording linked by a table.
fn load(f: &Fx, c: &mut Connection) -> i64 {
    let data = f.root.join("data");
    std::fs::create_dir_all(data.join("ecg")).unwrap();
    std::fs::write(data.join("ecg/one.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#)
        .unwrap();
    let rec = data.join("icu").join(bed_name("ICU7", "12", T0));
    wfdb_record(&rec, 0);
    let r = import(
        f,
        c,
        "patients.csv",
        b"MRN,Last Name,First Name,DOB,Sex,Patient ID\nSYN1,Doe,Jane,1950-03-04,F,P1\n",
        "",
    );
    assert_eq!((r.patients_added, r.items_added), (1, 0));
    let r = import(
        f,
        c,
        "encounters.xlsx",
        &xlsx(&[
            &[
                "Encounter ID",
                "Patient ID",
                "Admit Time",
                "Discharge Time",
                "Admit Unit",
                "Care Unit",
                "Bed",
                "Bed Start",
                "Bed End",
            ],
            &[
                "E1",
                "P1",
                "2024-01-01 10:00",
                "2024-01-05 10:00",
                "Cardiology",
                "ICU7",
                "12",
                "2024-01-01 11:00",
                "2024-01-03 11:00",
            ],
        ]),
        "",
    );
    assert_eq!((r.encounters_added, r.bed_stays_added), (1, 1), "{:?}", r.warnings);
    let r = import(
        f,
        c,
        "labs.csv",
        b"MRN,Encounter ID,Test Name,Result,Units,Ref Low,Ref High,Collected\n\
SYN1,E1,Lactate,4.1,mmol/L,0.5,2,2024-01-02 06:10\nSYN1,E1,Sodium,139,mmol/L,135,145,2024-01-02 06:10\n\
SYN1,E1,Lactate,1.1,mmol/L,0.5,2,2024-01-04 06:10\n",
        "",
    );
    assert_eq!((r.items_added, r.with_encounter), (3, 3));
    let r = import(
        f,
        c,
        "notes.csv",
        b"patient_id,encounter_id,time,note_type,author,note_text\nP1,E1,2024-01-02 15:40,ICU progress note,Dr. K. Osei,\"Assessment: septic shock, improving.\nPlan: continue.\"\n",
        "",
    );
    assert_eq!(r.items_added, 1);
    let r = import(f, c, "ecg_index.csv", b"patient_id,encounter_id,time,value,unit,interpretation,file\nP1,E1,2024-01-02 08:00,112,bpm,Sinus tachycardia,ecg/one.svg\n", "");
    assert_eq!(r.attachments, 1);
    let rel = rec.strip_prefix(&data).unwrap().to_string_lossy().to_string();
    let r = import(f, c, "icu_index.csv", format!("patient_id,encounter_id,file\nP1,E1,{rel}\n").as_bytes(), "");
    assert_eq!(r.recordings_added, 1, "{:?}", r.warnings);
    c.query_row("SELECT id FROM patients WHERE patient_id = 'P1'", [], |r| r.get(0)).unwrap()
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn a_patient_from_several_databases() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let id = load(&f, &mut c);
    let a = admin();
    // categories come from the file names
    let cats: Vec<(String, i64)> = c
        .prepare("SELECT category, COUNT(*) FROM ehr_items GROUP BY 1 ORDER BY 1")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(cats, [("ecg".to_string(), 1), ("lab".to_string(), 3), ("note".to_string(), 1)]);
    // the list
    let (n, rows) = patient::list(&c, &a, "doe", &[], 10, 0).unwrap();
    assert_eq!(n, 1);
    assert_eq!(rows[0].kinds, ["icu", "lab", "ecg", "note"]);
    assert_eq!(patient::list(&c, &a, "", &["ecg".into()], 10, 0).unwrap().0, 1);
    assert_eq!(patient::list(&c, &a, "", &["micro".into()], 10, 0).unwrap().0, 0);
    // the summary: no database holds allergies
    let s = patient::summary(&c, &a, id).unwrap();
    assert!(s.allergies.is_none());
    assert_eq!(s.encounters.len(), 1);
    assert!(s.encounters[0].inpatient && s.encounters[0].icu);
    // the journey: the categories some database provides
    let j = patient::journey(&c, &a, id).unwrap();
    let keys: Vec<&str> = j.lanes.iter().map(|l| l.key.as_str()).collect();
    assert_eq!(keys, ["enc", "icu", "lab", "ecg", "note"]);
    assert!(j.cells.iter().any(|c| c.4 == 1), "the high lactate is abnormal");
    assert!(j.spans.iter().any(|s| s.key.starts_with("rec:")));
    // the report: labs of one moment are one panel
    let r = patient::report(&c, &a, id, "").unwrap();
    let panel = r.entries.iter().find(|e| e.key.starts_with("labs:") && e.abnormal == 1).unwrap();
    assert_eq!(panel.title, "Laboratory · 2 tests");
    let d = patient::detail(&c, &f.cfg, &a, id, &panel.key).unwrap();
    let lact = d.rows.iter().find(|r| r.name == "Lactate").unwrap();
    assert_eq!((lact.level, lact.series.len()), (1, 2));
    assert_eq!(d.notes.len(), 1, "the note of that day");
    // search: words in notes, categories, values, fuzzy
    let r = patient::report(&c, &a, id, "septic").unwrap();
    assert_eq!(r.entries.len(), 1);
    assert!(r.entries[0].snippet.as_deref().unwrap().contains("septic shock"));
    let r = patient::report(&c, &a, id, "ecg icu").unwrap();
    assert_eq!(r.counts, [("icu".to_string(), 1), ("ecg".to_string(), 1)]);
    assert_eq!(patient::report(&c, &a, id, "lactate > 3").unwrap().entries.len(), 1);
    let r = patient::report(&c, &a, id, "lactat").unwrap();
    assert_eq!(r.entries.len(), 2, "a part of a word matches");
    let r = patient::report(&c, &a, id, "improvng").unwrap();
    assert_eq!(r.fuzzy, [("improvng".to_string(), "improving".to_string())]);
    // the ECG's picture: the patient's, inside the data roots
    let ecg = patient::report(&c, &a, id, "ecg").unwrap().entries.into_iter().find(|e| e.category == "ecg").unwrap();
    let d = patient::detail(&c, &f.cfg, &a, id, &ecg.key).unwrap();
    assert_eq!(d.attachment.as_ref().unwrap().kind, "image");
    let item: i64 = ecg.key[5..].parse().unwrap();
    assert!(patient::attachment(&c, &f.cfg, id, item).is_ok());
    assert!(patient::attachment(&c, &f.cfg, id + 1, item).is_err(), "not another patient's");
    // the recording a table linked: shown in the ICU column, with its notes
    let rec = patient::report(&c, &a, id, "").unwrap().entries.into_iter().find(|e| e.key.starts_with("rec:")).unwrap();
    let d = patient::detail(&c, &f.cfg, &a, id, &rec.key).unwrap();
    assert!(d.monitoring.is_some());
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn without_the_phi_permission() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let id = load(&f, &mut c);
    let u = reader();
    let (_, rows) = patient::list(&c, &u, "", &[], 10, 0).unwrap();
    assert!(rows[0].phi.is_none() && rows[0].age.is_none());
    assert!(patient::list(&c, &u, "name:doe", &[], 10, 0).is_err(), "names cannot be searched");
    assert!(patient::summary(&c, &u, id).unwrap().phi.is_none());
    // notes: shown to exist, not their text, and not searchable
    assert!(patient::report(&c, &u, id, "septic").unwrap().entries.is_empty());
    let r = patient::report(&c, &u, id, "").unwrap();
    let note = r.entries.iter().find(|e| e.category == "note").unwrap();
    assert!(note.summary.is_empty() || !note.summary.contains("septic"));
    let d = patient::detail(&c, &f.cfg, &u, id, &note.key).unwrap();
    assert!(d.text.is_none());
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn removing_a_database_removes_it_everywhere() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let id = load(&f, &mut c);
    let a = admin();
    let batch: i64 = c.query_row("SELECT id FROM imports WHERE name = 'notes.csv'", [], |r| r.get(0)).unwrap();
    assert!(medorama::typed::exists(&c, batch).unwrap());
    imports::remove_import(&mut c, &f.cfg, &a, batch, false, false).unwrap();
    assert!(!medorama::typed::exists(&c, batch).unwrap(), "its table of numbers goes with it");
    assert!(patient::report(&c, &a, id, "septic").unwrap().entries.is_empty(), "out of the search index too");
    let j = patient::journey(&c, &a, id).unwrap();
    assert!(!j.lanes.iter().any(|l| l.key == "note"), "no database provides notes any more");
    let n: i64 = c.query_row("SELECT COUNT(*) FROM item_text", [], |r| r.get(0)).unwrap();
    let m: i64 = c.query_row("SELECT COUNT(*) FROM ehr_items", [], |r| r.get(0)).unwrap();
    assert_eq!(n, m);
    // the encounters table: its encounter goes, its items wait, the recording is unlinked
    let batch: i64 = c.query_row("SELECT id FROM imports WHERE name = 'encounters.xlsx'", [], |r| r.get(0)).unwrap();
    imports::remove_import(&mut c, &f.cfg, &a, batch, false, false).unwrap();
    let s = patient::summary(&c, &a, id).unwrap();
    assert!(s.encounters.is_empty());
    assert!(patient::journey(&c, &a, id).unwrap().spans.iter().all(|s| !s.key.starts_with("rec:")));
}

/// A WFDB record of `leads` ECG leads (format 16, 1000 adu/mV) with narrow R waves at `bpm`.
fn ecg_record(dir: &std::path::Path, name: &str, leads: &[&str], fs: usize, seconds: usize, bpm: f64) {
    std::fs::create_dir_all(dir).unwrap();
    let n = fs * seconds;
    let mut cols: Vec<Vec<i16>> = vec![];
    for (j, _) in leads.iter().enumerate() {
        cols.push(
            (0..n)
                .map(|i| {
                    let ph = (i as f64 / fs as f64 * bpm / 60.0).fract();
                    let v = (-((ph - 0.3) / 0.012).powi(2)).exp() * 1.2 + (-((ph - 0.6) / 0.05).powi(2)).exp() * 0.3;
                    (v * 1000.0 * (1.0 - 0.05 * j as f64)) as i16
                })
                .collect(),
        );
    }
    let mut head = format!("{name} {} {fs} {n}\n", leads.len());
    let mut dat = Vec::with_capacity(n * leads.len() * 2);
    for i in 0..n {
        for c in &cols {
            dat.extend(c[i].to_le_bytes());
        }
    }
    for (j, lead) in leads.iter().enumerate() {
        let sum = cols[j].iter().fold(0i16, |a, &x| a.wrapping_add(x));
        head += &format!("{name}.dat 16 1000/mV 16 0 {} {sum} 0 {lead}\n", cols[j][0]);
    }
    std::fs::write(dir.join(format!("{name}.hea")), head).unwrap();
    std::fs::write(dir.join(format!("{name}.dat")), dat).unwrap();
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn ecgs_images_and_a_holters_heart_rate() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let id = load(&f, &mut c);
    let a = admin();
    let data = f.root.join("data");
    // a 12-lead ECG as WFDB: attached to its item (shown by the ECG viewer), not a recording
    let twelve = ["I", "II", "III", "aVR", "aVL", "aVF", "V1", "V2", "V3", "V4", "V5", "V6"];
    ecg_record(&data.join("carts"), "e1", &twelve, 500, 10, 70.0);
    ecg_record(&data.join("carts"), "other", &twelve, 500, 10, 90.0); // another patient's, beside it
    let r = import(
        &f,
        &mut c,
        "muse_export.csv",
        b"patient_id,encounter_id,time,value,unit,file\nP1,E1,2024-01-03 09:00,70,bpm,carts/e1.hea\n",
        "ecg",
    );
    assert_eq!((r.attachments, r.recordings_added), (1, 0), "{:?}", r.warnings);
    let item: i64 = c.query_row("SELECT id FROM ehr_items WHERE file_path LIKE '%e1.hea'", [], |r| r.get(0)).unwrap();
    let d = patient::detail(&c, &f.cfg, &a, id, &format!("item:{item}")).unwrap();
    let att = d.attachment.unwrap();
    assert_eq!((att.kind.as_str(), att.viewer.as_deref()), ("wfdb", Some("ecg")));
    assert!(att.src.ends_with(&format!("file/{item}/e1.hea")), "{}", att.src);
    // an attachment's own files only: the header, the signal file it names; not the neighbours
    for (name, ok) in [
        ("e1.hea", true),
        ("e1.dat", true),
        ("other.dat", false),
        ("other.hea", false),
        ("../carts/e1.dat", false),
        (".hidden", false),
    ] {
        assert_eq!(patient::attachment_part(&c, &f.cfg, id, item, name).is_ok(), ok, "{name}");
    }
    assert!(patient::attachment_part(&c, &f.cfg, id + 1, item, "e1.dat").is_err(), "another patient's item");
    // a DICOM series: an attached folder, listed, its files served by name
    std::fs::create_dir_all(data.join("ct/s1")).unwrap();
    for n in ["IM0001.dcm", "IM0002.dcm"] {
        std::fs::write(data.join("ct/s1").join(n), b"DICM").unwrap();
    }
    let r = import(
        &f,
        &mut c,
        "imaging_index.csv",
        b"patient_id,encounter_id,time,study,file\nP1,E1,2024-01-03 11:00,CT chest,ct/s1/\n",
        "imaging",
    );
    assert_eq!(r.attachments, 1, "{:?}", r.warnings);
    let ct: i64 = c.query_row("SELECT id FROM ehr_items WHERE category = 'img'", [], |r| r.get(0)).unwrap();
    match patient::attachment(&c, &f.cfg, id, ct).unwrap() {
        patient::Attached::Listing(files) => assert_eq!(files, ["IM0001.dcm", "IM0002.dcm"]),
        patient::Attached::File(..) => panic!("a folder is listed"),
    }
    assert!(patient::attachment_part(&c, &f.cfg, id, ct, "IM0002.dcm").is_ok());
    let d = patient::detail(&c, &f.cfg, &a, id, &format!("item:{ct}")).unwrap();
    assert_eq!(d.attachment.unwrap().viewer.as_deref(), Some("pacs"));
    // a Holter: a recording (category from the table), indexed, its heart rate found from its ECG
    ecg_record(&data.join("holter/h1"), "h1", &["II", "V5"], 250, 180, 75.0);
    let r = import(&f, &mut c, "holter_index.csv", b"patient_id,encounter_id,file\nP1,E1,holter/h1/h1.hea\n", "");
    assert_eq!(r.recordings_added, 1, "{:?}", r.warnings);
    let (rid, path): (i64, String) = c
        .query_row("SELECT id, path FROM recordings WHERE category = 'holter'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    let key = recording_engine::index::build_index(std::path::Path::new(&path), &f.cfg.index_dir, &|_| {}).unwrap();
    let series = medorama::derived::derive(&f.cfg.index_dir, &key, &|_| {}).unwrap();
    assert_eq!(series.len(), 1);
    let hr = &series[0];
    assert_eq!(hr.name, medorama::derived::HEART_RATE);
    assert!(hr.t.len() >= 2 && hr.v.iter().all(|v| (v - 75.0).abs() < 2.0), "{:?}", hr.v);
    medorama::derived::store(&c, rid, &series).unwrap();
    assert_eq!(medorama::derived::load(&c, rid).unwrap(), series);
    // an ICU recording has a heart rate of its own: nothing to derive
    let icu: String = c.query_row("SELECT path FROM recordings WHERE category = 'icu'", [], |r| r.get(0)).unwrap();
    let key = recording_engine::index::build_index(std::path::Path::new(&icu), &f.cfg.index_dir, &|_| {}).unwrap();
    let _ = medorama::derived::derive(&f.cfg.index_dir, &key, &|_| {}).unwrap();
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn databases_as_tables_and_their_privacy() {
    use medorama::databases;
    let f = fixture();
    let mut c = f.db.get().unwrap();
    load(&f, &mut c);
    let a = admin();
    let list = databases::list(&c).unwrap();
    let find = |name: &str| list.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("{name}"));
    let pats = find("patients.csv");
    assert!(pats.viewable);
    assert_eq!((pats.rows, pats.patients), (1, 1));
    assert_eq!(find("labs.csv").categories, ["lab"]);
    let q = |text: &str| databases::Query { q: text.into(), limit: 50, ..Default::default() };
    // with the PHI permission: every column, as imported
    let p = databases::rows(&c, &a, None, pats.id, &q("")).unwrap();
    let heads: Vec<&str> = p.columns.iter().map(|c| c.header.as_str()).collect();
    assert_eq!(heads, ["MRN", "Last Name", "First Name", "DOB", "Sex", "Patient ID"]);
    assert_eq!(p.rows[0].cells[1], "Doe");
    assert_eq!(p.rows[0].patient_id, "P1");
    // without it: no identifying column, and no search through them
    let r = reader();
    let p = databases::rows(&c, &r, None, pats.id, &q("")).unwrap();
    let heads: Vec<&str> = p.columns.iter().map(|c| c.header.as_str()).collect();
    assert_eq!(heads, ["Sex", "Patient ID"]);
    assert_eq!(p.hidden, ["MRN", "Last Name", "First Name", "DOB"]);
    assert_eq!(databases::rows(&c, &r, None, pats.id, &q("doe")).unwrap().total, 0, "a hidden column is not searched");
    assert_eq!(databases::rows(&c, &a, None, pats.id, &q("doe")).unwrap().total, 1);
    // notes: their text needs the PHI permission, and is not searched without it
    let notes = find("notes.csv").id;
    let n = databases::rows(&c, &r, None, notes, &q("")).unwrap();
    let text = n.columns.iter().position(|c| c.maps_to == "text").unwrap();
    assert!(!n.rows[0].cells[text].contains("septic"));
    assert_eq!(databases::rows(&c, &r, None, notes, &q("septic")).unwrap().total, 0);
    assert_eq!(databases::rows(&c, &a, None, notes, &q("septic")).unwrap().total, 1);
    // values sort as numbers, filters per column
    let labs = find("labs.csv").id;
    let value =
        databases::rows(&c, &a, None, labs, &q("")).unwrap().columns.iter().find(|c| c.maps_to == "value").unwrap().pos;
    let s = databases::rows(
        &c,
        &a,
        None,
        labs,
        &databases::Query { sort: Some(value), desc: true, limit: 5, ..Default::default() },
    )
    .unwrap();
    assert_eq!(s.rows[0].cells[value], "139");
    let name = s.columns.iter().find(|c| c.maps_to == "name").unwrap().pos;
    let lact = databases::Query { filters: vec![(name, "lact".into())], limit: 5, ..Default::default() };
    assert_eq!(databases::rows(&c, &a, None, labs, &lact).unwrap().total, 2);
    // a file links to its viewer; a row to its patient page entry
    let ecg = databases::rows(&c, &a, None, find("ecg_index.csv").id, &q("")).unwrap();
    let link = ecg.rows[0].link.as_ref().unwrap();
    assert_eq!((link.viewer.as_str(), link.kind.as_str()), ("ecg", "image"));
    assert!(ecg.rows[0].entry.starts_with("item:"));
    let icu = databases::rows(&c, &a, None, find("icu_index.csv").id, &q("")).unwrap();
    assert_eq!(icu.rows[0].link.as_ref().unwrap().viewer, "vitalstream");
    // removing a database removes its rows as imported (PHI) too
    imports::remove_import(&mut c, &f.cfg, &a, pats.id, false, false).unwrap();
    let left: i64 =
        c.query_row("SELECT COUNT(*) FROM import_rows WHERE import_id = ?1", [pats.id], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
    assert!(databases::list(&c).unwrap().iter().all(|d| d.id != pats.id));
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn cohorts_by_criteria() {
    use medorama::cohorts::{self, Change, Criteria, Criterion, Expr, Group, NewCohort, Share};
    use medorama::users;
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let id = load(&f, &mut c);
    let k = |kind: &str| Criterion { kind: kind.into(), ..Default::default() };
    // every criterion must be met
    let q = |all: Vec<Criterion>| Criteria {
        include: Some(Expr::Group(Group { op: "and".into(), of: all.into_iter().map(Expr::Leaf).collect() })),
        ..Default::default()
    };
    let owner_id = users::add(&c, "owner", "", Role::Analyst, true, "a long password 1").unwrap();
    let other_id = users::add(&c, "other", "", Role::Viewer, false, "a long password 2").unwrap();
    let owner = users::get(&c, owner_id).unwrap().unwrap();
    let other = users::get(&c, other_id).unwrap().unwrap();
    // a lactate of 4 or more, with an ECG: P1, with why
    let lact = Criterion { name: "lactate".into(), min: Some(4.0), ..k("measurement") };
    let has_ecg = Criterion { category: "ecg".into(), ..k("has") };
    let found = cohorts::evaluate(&c, &owner, "UTC", &q(vec![lact.clone(), has_ecg.clone()])).unwrap();
    assert_eq!(found.keys().copied().collect::<Vec<_>>(), [id]);
    assert_eq!(found[&id], ["Lactate 4.1 mmol/L on 2024-01-02", "has ECG"]);
    assert!(cohorts::evaluate(&c, &owner, "UTC", &q(vec![Criterion { not: true, ..has_ecg.clone() }]))
        .unwrap()
        .is_empty());
    let late = Criterion { from: "2024-01-03".into(), ..lact.clone() };
    assert!(cohorts::evaluate(&c, &owner, "UTC", &q(vec![late])).unwrap().is_empty(), "the dates count");
    for (crit, n) in [
        (Criterion { text: "F".into(), ..k("sex") }, 1),
        (Criterion { text: "septic".into(), ..k("text") }, 1),
        (Criterion { text: "icu7".into(), icu: Some(true), ..k("encounter") }, 1),
        (Criterion { min: Some(70.0), ..k("age") }, 1),
    ] {
        assert_eq!(cohorts::evaluate(&c, &owner, "UTC", &q(vec![crit.clone()])).unwrap().len(), n, "{crit:?}");
    }
    // without the PHI permission: no age (it comes from the date of birth), no notes' text
    assert!(cohorts::evaluate(&c, &other, "UTC", &q(vec![Criterion { min: Some(70.0), ..k("age") }])).is_err());
    assert!(cohorts::evaluate(&c, &other, "UTC", &q(vec![Criterion { text: "septic".into(), ..k("text") }]))
        .unwrap()
        .is_empty());
    // a cohort: private until shared; read-only shares cannot change it
    let co = cohorts::create(
        &c,
        &owner,
        "UTC",
        &NewCohort { name: "Lactate".into(), criteria: q(vec![lact]), patients: vec![id], ..Default::default() },
    )
    .unwrap();
    assert_eq!(cohorts::get(&c, &owner, co).unwrap().members, 1);
    assert!(cohorts::get(&c, &other, co).is_err());
    assert!(cohorts::list(&c, &other).unwrap().is_empty());
    let share = Change {
        shares: Some(vec![Share { user_id: other_id, username: String::new(), can_edit: false }]),
        ..Default::default()
    };
    cohorts::change(&c, &owner, "UTC", co, &share).unwrap();
    assert_eq!(cohorts::list(&c, &other).unwrap().len(), 1);
    assert!(cohorts::change(&c, &other, "UTC", co, &Change { remove: vec![id], ..Default::default() }).is_err());
    assert!(cohorts::delete(&c, &other, co).is_err());
    // refresh: members found by the criteria leave when they no longer meet them; manual ones stay
    let stricter = Criterion { name: "lactate".into(), min: Some(9.0), ..k("measurement") };
    cohorts::change(&c, &owner, "UTC", co, &Change { criteria: Some(q(vec![stricter])), ..Default::default() })
        .unwrap();
    let r = cohorts::refresh(&c, &owner, "UTC", co, true).unwrap();
    assert_eq!((r.added.len(), r.removed.as_slice()), (0, [id].as_slice()));
    assert_eq!(cohorts::get(&c, &owner, co).unwrap().members, 1, "a preview changes nothing");
    cohorts::refresh(&c, &owner, "UTC", co, false).unwrap();
    assert_eq!(cohorts::get(&c, &owner, co).unwrap().members, 0);
    cohorts::change(&c, &owner, "UTC", co, &Change { add: vec![id], ..Default::default() }).unwrap();
    cohorts::refresh(&c, &owner, "UTC", co, false).unwrap();
    assert_eq!(cohorts::get(&c, &owner, co).unwrap().members, 1, "added manually: kept");
    // export: identifiers only for users with the PHI permission
    let (_, csv) = cohorts::export_csv(&c, &owner, co, None).unwrap();
    assert!(csv.contains("Doe") && csv.starts_with("patient_id,"));
    let (_, csv) = cohorts::export_csv(&c, &other, co, None).unwrap();
    assert!(!csv.contains("Doe") && csv.contains("P1"));
    cohorts::delete(&c, &owner, co).unwrap();
    assert!(cohorts::list(&c, &owner).unwrap().is_empty());
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn cohort_definitions_clones_merges_and_subcohorts() {
    use medorama::cohorts::{self, Change, Criteria, Criterion, Expr, NewCohort, PatientSearch, Share, Source};
    use medorama::users;
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let p1 = load(&f, &mut c);
    import(
        &f,
        &mut c,
        "patients2.csv",
        b"MRN,Last Name,First Name,DOB,Sex,Patient ID\nSYN2,Roe,Rick,1961-03-05,M,P2\nSYN3,Major,Ana,1977-08-19,F,P3\n",
        "",
    );
    let pid = |c: &Connection, p: &str| -> i64 {
        c.query_row("SELECT id FROM patients WHERE patient_id = ?1", [p], |r| r.get(0)).unwrap()
    };
    let (p2, p3) = (pid(&c, "P2"), pid(&c, "P3"));
    let owner_id = users::add(&c, "owner", "", Role::Analyst, true, "a long password 1").unwrap();
    let other_id = users::add(&c, "other", "", Role::Analyst, false, "a long password 2").unwrap();
    let owner = users::get(&c, owner_id).unwrap().unwrap();
    let other = users::get(&c, other_id).unwrap().unwrap();
    let expr = |text: &str| Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "expression".into(), text: text.into(), ..Default::default() })),
        ..Default::default()
    };
    let members = |c: &Connection, co: i64| {
        let mut v: Vec<i64> = cohorts::member_ids(c, &owner, co).unwrap().into_iter().collect();
        v.sort();
        v
    };
    let change = |c: &Connection, co: i64, ch: Change| cohorts::change(c, &owner, "UTC", co, &ch).unwrap();
    let new = |c: &Connection, name: &str, criteria: Criteria, source: Option<Source>| {
        cohorts::create(c, &owner, "UTC", &NewCohort { name: name.into(), criteria, source, ..Default::default() })
            .unwrap()
    };

    // made without a list of patients: all those the definition selects
    let female = new(&c, "Female", expr("female"), None);
    assert_eq!(members(&c, female), [p1, p3]);
    let d = cohorts::get(&c, &owner, female).unwrap().definition;
    assert_eq!((d.include.as_str(), d.from.as_str(), d.added.len(), d.removed.len()), ("female", "", 0, 0));

    // added manually (outside the criteria): in the definition, and kept by a refresh
    let co = change(&c, female, Change { add: vec![p2], ..Default::default() });
    assert_eq!(co.definition.added, ["P2"]);
    // removed manually (selected by the criteria): in the definition, and kept out by a refresh
    let co = change(&c, female, Change { remove: vec![p3], ..Default::default() });
    assert_eq!(co.definition.added, ["P2"]);
    assert_eq!(co.definition.removed, ["P3"]);
    let r = cohorts::refresh(&c, &owner, "UTC", female, false).unwrap();
    assert!(r.added.is_empty() && r.removed.is_empty(), "{r:?}");
    assert_eq!(members(&c, female), [p1, p2]);
    // a member added manually, removed: no longer in the definition at all
    let co = change(&c, female, Change { remove: vec![p2], ..Default::default() });
    assert!(co.definition.added.is_empty());
    // added again, a patient removed manually: found by the definition again
    let co = change(&c, female, Change { add: vec![p3], ..Default::default() });
    assert!(co.definition.removed.is_empty() && co.definition.added.is_empty());
    assert_eq!(members(&c, female), [p1, p3]);
    // by the Patient Catalog's search
    let co = change(
        &c,
        female,
        Change { add_search: Some(PatientSearch { q: "P2".into(), ..Default::default() }), ..Default::default() },
    );
    assert_eq!(co.members, 3);
    assert_eq!(co.definition.added, ["P2"]);
    change(&c, female, Change { remove: vec![p2], ..Default::default() });

    // a subcohort: the parent's members who meet its criteria; the search sees only them
    let everyone = new(&c, "Everyone", expr("female or male"), None);
    let within = |of: Vec<i64>| Some(Source { op: "within".into(), of, ..Default::default() });
    let parent = cohorts::checked_source(&c, &owner, &within(vec![female]).unwrap()).unwrap();
    let (total, _, ids) = cohorts::search(
        &c,
        &owner,
        "UTC",
        &expr("has ecg"),
        Some(&cohorts::source_members(&c, &parent).unwrap()),
        10,
        0,
    )
    .unwrap();
    assert_eq!((total, ids), (1, vec![p1]));
    let sub = new(&c, "Female, with an ECG", expr("has ecg"), within(vec![female]));
    let co = cohorts::get(&c, &owner, sub).unwrap();
    assert_eq!((co.parent, co.definition.from.as_str()), (Some(female), "Subcohort of “Female”"));
    assert_eq!(members(&c, sub), [p1]);

    // merges: union, intersection, difference (in the first, in none of the others)
    let merge = |op: &str, of: Vec<i64>| Some(Source { op: op.into(), of, ..Default::default() });
    let male = new(&c, "Male", expr("male"), None);
    let u = new(&c, "Both", Criteria::default(), merge("union", vec![female, male]));
    assert_eq!(members(&c, u), [p1, p2, p3]);
    assert_eq!(cohorts::get(&c, &owner, u).unwrap().definition.from, "In “Female” or “Male”");
    let i = new(&c, "Female, all", Criteria::default(), merge("intersection", vec![everyone, female]));
    assert_eq!(members(&c, i), [p1, p3]);
    let diff = new(&c, "Not female", Criteria::default(), merge("difference", vec![everyone, female]));
    assert_eq!(members(&c, diff), [p2]);
    assert_eq!(cohorts::get(&c, &owner, diff).unwrap().definition.from, "In “Everyone” but not in “Female”");
    // a merge follows its cohorts when refreshed
    change(&c, female, Change { remove: vec![p3], ..Default::default() });
    let r = cohorts::refresh(&c, &owner, "UTC", diff, false).unwrap();
    assert_eq!((r.added.as_slice(), r.removed.as_slice()), ([p3].as_slice(), [].as_slice()));
    // sources are checked
    for bad in [within(vec![female, male]), merge("union", vec![female]), merge("xor", vec![female, male])] {
        assert!(cohorts::create(&c, &owner, "UTC", &NewCohort { name: "x".into(), source: bad, ..Default::default() })
            .is_err());
    }
    assert!(
        cohorts::checked_source(&c, &other, &merge("union", vec![female, male]).unwrap()).is_err(),
        "not shared with them"
    );

    // a clone: the same definition and members, its own owner; made by whoever may read it
    change(
        &c,
        female,
        Change {
            shares: Some(vec![Share { user_id: other_id, username: String::new(), can_edit: false }]),
            ..Default::default()
        },
    );
    let copy = cohorts::clone(&c, &other, female, "My female cohort").unwrap();
    let co = cohorts::get(&c, &other, copy).unwrap();
    assert!(co.is_owner && co.can_edit);
    assert_eq!(co.definition.origin, "Cloned from “Female”");
    assert_eq!(co.definition.removed, ["P3"]);
    assert_eq!(cohorts::member_ids(&c, &other, copy).unwrap(), cohorts::member_ids(&c, &owner, female).unwrap());

    // deleting a cohort others are made from: they keep its last members
    cohorts::delete(&c, &owner, female).unwrap();
    let co = cohorts::get(&c, &owner, diff).unwrap();
    assert!(
        co.definition.sources[1].deleted && co.definition.from.ends_with("“Female (deleted)”"),
        "{:?}",
        co.definition
    );
    let r = cohorts::refresh(&c, &owner, "UTC", diff, true).unwrap();
    assert!(r.added.is_empty() && r.removed.is_empty(), "{r:?}");
    assert_eq!(members(&c, sub), [p1], "a subcohort keeps its parent's last members");

    // the history: each change, in words
    let h: Vec<String> = cohorts::history(&c, &owner, diff).unwrap().into_iter().map(|e| e.what).collect();
    assert!(h[0].contains("“Female”, which it is made from, was deleted"), "{h:?}");
    assert!(h.iter().any(|w| w.starts_with("refreshed: 1 joined, 0 left; joined: P3")), "{h:?}");
    assert!(h.last().unwrap().starts_with("made: 1 patient; In “Everyone” but not in “Female”"), "{h:?}");
    let h: Vec<String> = cohorts::history(&c, &other, copy).unwrap().into_iter().map(|e| e.what).collect();
    assert_eq!(h, ["cloned from “Female”: 1 patient"]);
    assert!(cohorts::history(&c, &other, male).is_err(), "not shared with them");
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn export_delete_and_the_data_version() {
    use medorama::cohorts::{self, Criteria, Criterion, Expr};
    use medorama::databases;
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let id = load(&f, &mut c);
    let a = admin();
    let version = |c: &medorama::sql::Connection| -> i64 {
        c.query_row("SELECT version FROM data_version", [], |r| r.get(0)).unwrap()
    };
    let pats = databases::list(&c).unwrap().into_iter().find(|d| d.name == "patients.csv").unwrap();
    // export: as imported; without the PHI permission, only the columns one may see
    let (name, csv) = databases::export_csv(&c, &a, pats.id, None).unwrap();
    assert_eq!(name, "patients.csv");
    assert!(csv.starts_with("MRN,Last Name,First Name,DOB,Sex,Patient ID") && csv.contains("Doe"));
    let (_, csv) = databases::export_csv(&c, &reader(), pats.id, None).unwrap();
    assert_eq!(csv.lines().next().unwrap(), "Sex,Patient ID");
    assert!(!csv.contains("Doe"));
    // a cohort criterion naming a database: once deleted, it finds nobody (no error)
    let labs = databases::list(&c).unwrap().into_iter().find(|d| d.name == "labs.csv").unwrap();
    let q = Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "database".into(), import: Some(labs.id), ..Default::default() })),
        ..Default::default()
    };
    assert_eq!(cohorts::evaluate(&c, &a, "UTC", &q).unwrap().keys().copied().collect::<Vec<_>>(), [id]);
    // every import and deletion moves the version the open pages watch
    let v0 = version(&c);
    imports::remove_import(&mut c, &f.cfg, &a, labs.id, false, false).unwrap();
    let v1 = version(&c);
    assert!(v1 > v0);
    assert!(cohorts::evaluate(&c, &a, "UTC", &q).unwrap().is_empty());
    import(&f, &mut c, "more.csv", b"patient_id,name,value,time\nP1,Sodium,140,2024-01-03 06:00\n", "lab");
    assert!(version(&c) > v1);
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn cohorts_v2_columns_expressions_and_exports() {
    use medorama::cohorts::{self, Criteria, Criterion, Expr, NewCohort};
    use medorama::deid;
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let p1 = load(&f, &mut c);
    import(
        &f,
        &mut c,
        "patients2.csv",
        // a "Gender" column, as some exports name it, is read as the patient's sex
        b"MRN,Last Name,First Name,DOB,Gender,Patient ID\nSYN2,Old,Al,1930-01-01,M,P2\n",
        "",
    );
    let p2: i64 = c.query_row("SELECT id FROM patients WHERE patient_id = 'P2'", [], |r| r.get(0)).unwrap();
    // two ECG databases with a QT column each (never merged), measurements as "Name (unit)"
    let r = import(
        &f,
        &mut c,
        "ecg_measures.csv",
        "patient_id,time,QT (ms),R axis,interpretation,Comment\nP1,2024-01-02 08:00,410,60,Sinus rhythm,Jane's first\n\
P1,2024-01-04 08:00,390,62,Atrial fibrillation,\nP2,2024-02-01 09:00,380,-40,Left axis deviation,\n"
            .as_bytes(),
        "",
    );
    assert_eq!(r.items_added, 3, "{:?}", r.warnings);
    assert!(r.ignored_columns.contains(&"QT (ms)".to_string()), "kept as the database's own column");
    import(&f, &mut c, "holter_qt.csv", b"patient_id,time,QT (ms)\nP1,2024-01-03 00:00,520\n", "");
    let a = admin();
    let ids = |found: std::collections::HashMap<i64, Vec<String>>| {
        let mut v: Vec<i64> = found.into_keys().collect();
        v.sort();
        v
    };
    let expr = |text: &str| Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "expression".into(), text: text.into(), ..Default::default() })),
        ..Default::default()
    };
    let err = cohorts::parse_text(&c, &a, "QT > 400ms").unwrap_err().to_string();
    assert!(err.contains("several databases") && err.contains("ecg_measures") && err.contains("holter_qt"), "{err}");
    assert_eq!(ids(cohorts::evaluate(&c, &a, "UTC", &expr("ecg_measures.QT > 0.4 s")).unwrap()), [p1]);
    assert_eq!(
        ids(cohorts::evaluate(&c, &a, "UTC", &expr("latest ecg_measures.QT > 400")).unwrap()),
        Vec::<i64>::new()
    );
    assert_eq!(ids(cohorts::evaluate(&c, &a, "UTC", &expr("holter_qt.QT >= 520 ms and female")).unwrap()), [p1]);
    assert_eq!(ids(cohorts::evaluate(&c, &a, "UTC", &expr("\"R axis\" < -30 or lactate > 4")).unwrap()), [p1, p2]);
    assert!(cohorts::evaluate(&c, &a, "UTC", &expr("ecg_measures.QT > 4 mV")).is_err(), "units must compare");
    // the same answers from the cells as from the databases' tables of numbers (typed.rs): a
    // database imported before them, then given its table
    let asked = ["ecg_measures.QT > 0.4 s", "holter_qt.QT >= 520 ms", "\"R axis\" < -30", "ecg_measures.QT < 395"];
    let answers = |c: &Connection| -> Vec<Vec<i64>> {
        asked.iter().map(|q| ids(cohorts::evaluate(c, &a, "UTC", &expr(q)).unwrap())).collect()
    };
    let typed = answers(&c);
    let dbs: Vec<i64> = c
        .prepare("SELECT id FROM imports WHERE kind = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(dbs.iter().all(|d| medorama::typed::exists(&c, *d).unwrap()));
    for d in &dbs {
        c.execute_batch_native(&format!("DROP TABLE {}", medorama::typed::name(*d))).unwrap();
    }
    assert_eq!(answers(&c), typed, "from the cells");
    assert_eq!(medorama::typed::build_missing(&mut c).unwrap(), dbs.len());
    assert!(dbs.iter().all(|d| medorama::typed::exists(&c, *d).unwrap()));
    assert_eq!(answers(&c), typed, "from the tables made afterwards");
    assert_eq!(typed[3], [p1, p2]);
    // inclusion and exclusion; exclusion looks at the whole history
    let q = Criteria {
        include: expr("female or male").include,
        exclude: vec![Expr::Leaf(Criterion {
            kind: "column".into(),
            import: cohorts::parse_text(&c, &a, "in ecg_measures").ok().and_then(|e| match e {
                Expr::Leaf(k) => k.import,
                _ => None,
            }),
            name: "interpretation".into(),
            text: "fibrillation".into(),
            ..Default::default()
        })],
        ..Default::default()
    };
    let (inc, exc) = cohorts::evaluate_both(&c, &a, "UTC", &q).unwrap();
    assert_eq!((ids(inc), exc.keys().copied().collect::<Vec<_>>()), (vec![p2], vec![p1]));
    assert!(exc[&p1][0].contains("Atrial fibrillation"), "{:?}", exc[&p1]);
    // without the PHI permission: measurement columns yes, other unmapped columns no
    let r = reader();
    assert_eq!(ids(cohorts::evaluate(&c, &r, "UTC", &expr("ecg_measures.QT > 400")).unwrap()), [p1]);
    assert!(cohorts::parse_text(&c, &r, "ecg_measures.Comment contains \"Jane\"").is_err());
    // exports: members, data, workbook; de-identified as the view is
    let co = cohorts::create(
        &c,
        &a,
        "UTC",
        &NewCohort {
            name: "QT".into(),
            criteria: expr("female or male"),
            patients: vec![p1, p2],
            ..Default::default()
        },
    )
    .unwrap();
    let (_, csv) = cohorts::export_csv(&c, &a, co, None).unwrap();
    assert!(csv.contains("Doe") && csv.contains("1930-01-01"));
    let s = deid::Settings::default();
    // through the privacy gate, as Privacy Mode exports it
    let g = medorama::gate::Restricted {
        settings: s.clone(),
        dict: std::sync::Arc::new(medorama::gate::Dict::build(&c, 0).unwrap()),
        secret: vec![7; 32],
    };
    let (name, csv) = cohorts::export_csv(&c, &a, co, Some(&g)).unwrap();
    assert!(
        name.ends_with("-privacy.csv") && !csv.contains("Doe") && !csv.contains("SYN") && !csv.contains("1930"),
        "{csv}"
    );
    assert!(csv.contains("[Patient Name]") && csv.contains("MO-"), "{csv}");
    assert!(csv.lines().next().unwrap().ends_with(",age") && csv.contains(",90+"), "{csv}");
    let (_, data) = cohorts::export_data_csv(&c, &a, co, None).unwrap();
    assert!(data.contains("ecg_measures,2,QT (ms),410") && data.contains("Jane's first"), "{data}");
    let (_, data) = cohorts::export_data_csv(&c, &a, co, Some(&g)).unwrap();
    // each patient's dates by their own offset: the catalog's, and their own days
    let ids: Vec<i64> = c
        .prepare("SELECT id FROM patients")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let moved: Vec<String> =
        ids.iter().map(|p| deid::shift_text("2024-01-02 08:00", g.for_patient(*p).settings.offset_seconds())).collect();
    // 36,570 days back, and up to 365 more: late 1922 or 1923
    assert!(
        moved.iter().any(|m| (m.starts_with("1922-") || m.starts_with("1923-")) && data.contains(m)),
        "{moved:?}\n{data}"
    );
    assert!(!data.contains("2024-01-02") && !data.contains("Jane's first") && !data.contains("Doe"), "{data}");
    let (_, book) = cohorts::export_xlsx(&c, &a, co, Some(&g)).unwrap();
    use calamine::Reader;
    let wb: calamine::Xlsx<_> = calamine::Xlsx::new(std::io::Cursor::new(book)).unwrap();
    let sheets = wb.sheet_names();
    assert!(
        sheets.starts_with(&["Members".to_string(), "Definition".to_string()])
            && sheets.contains(&"ecg_measures".to_string()),
        "{sheets:?}"
    );
    let (_, crit) = cohorts::export_criteria(&c, &a, co).unwrap();
    assert!(crit.contains("female or male") && crit.contains("\"in_words\""));
}

/// GE MUSE ECG metadata tables (the columns of a real export, tests/fixtures; every value
/// invented): recognised, converted, several files one database, read row by row; the patients
/// made from them, the ECGs on their pages, cohorts on their columns, nothing of staff or visits
/// kept, the identifiers masked without the PHI permission.
#[test]
fn muse_metadata_tables_make_one_database() {
    use medorama::cohorts::{self, Criteria, Criterion, Expr};
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let header: Vec<String> =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/muse_metadata_header.txt"))
            .unwrap()
            .split_whitespace()
            .map(String::from)
            .collect();
    let ecgs = f.root.join("data/ecg");
    std::fs::create_dir_all(&ecgs).unwrap();
    let row = |v: &[(&str, &str)]| -> Vec<String> {
        header
            .iter()
            .map(|h| {
                if h.ends_with("MDLastName") || h.ends_with("OverreaderLastName") {
                    return "Zorbluff".to_string(); // a staff surname: never kept
                }
                match h.as_str() {
                    "Order/HISAccountNumber" => "ACCT777001".into(),
                    "TestDemographics/Status" => "CONFIRMED".into(),
                    "TestDemographics/LocationName" => "CARDIOLOGY".into(),
                    _ => v.iter().find(|(k, _)| k == h).map(|(_, x)| x.to_string()).unwrap_or_default(),
                }
            })
            .collect()
    };
    let ecg = |file: &str, id: &str, sex: &str, date: &str, qtc: &str, read: &str| {
        std::fs::write(ecgs.join(file), "<RestingECG/>").unwrap();
        row(&[
            ("filename", &ecgs.join(file).to_string_lossy()),
            ("PatientDemographics/PatientID", id),
            ("PatientDemographics/PatientLastName", "Testperson"),
            ("PatientDemographics/PatientFirstName", "Ada"),
            ("PatientDemographics/DateofBirth", "03-04-1950"),
            ("PatientDemographics/Gender", sex),
            ("TestDemographics/AcquisitionDate", date),
            ("TestDemographics/AcquisitionTime", "08:15:30"),
            ("RestingECGMeasurements/QTCorrected", qtc),
            ("RestingECGMeasurements/VentricularRate", "72"),
            ("Diagnosis/DiagnosisStatement/0/StmtText", read),
            ("Diagnosis/DiagnosisStatement/0/StmtFlag", "ENDSLINE"),
            ("Diagnosis/DiagnosisStatement/1/StmtText", "Abnormal ECG"),
        ])
    };
    let write = |name: &str, rows: Vec<Vec<String>>| {
        let p = f.root.join(name);
        let mut w = csv::Writer::from_path(&p).unwrap();
        w.write_record(&header).unwrap();
        for r in rows {
            w.write_record(&r).unwrap();
        }
        w.flush().unwrap();
        p
    };
    let a = write(
        "muse_1.csv",
        vec![
            ecg("e1.xml", "SYN7001", "FEMALE", "01-02-2024", "490", "Atrial fibrillation"),
            ecg("e2.xml", "SYN7001", "FEMALE", "02-02-2024", "440", "Sinus rhythm"),
        ],
    );
    let mut no_id = ecg("e9.xml", "", "MALE", "01-02-2024", "400", "Sinus rhythm");
    no_id[1] = String::new();
    let b = write("muse_2.csv", vec![ecg("e3.xml", "SYN7002", "MALE", "12-31-2023", "410", "Sinus rhythm"), no_id]);
    let part = |p: &PathBuf| tables::Part {
        name: p.file_name().unwrap().to_string_lossy().to_string(),
        data: tables::PartData::File(p.clone()),
    };
    let r = tables::import_parts(
        &mut c,
        &f.cfg,
        &admin(),
        &[part(&a), part(&b)],
        &HashMap::new(),
        "",
        None,
        &Opts::new("muse_ecg", "muse", false),
        medorama::formats::Choice::Detect,
    )
    .unwrap();
    assert_eq!((r.rows, r.patients_added, r.items_added, r.conflicts), (3, 2, 3, 0), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.starts_with("1 rows without a patient ID")), "{:?}", r.warnings);
    // one database, its ECGs
    let list = medorama::databases::list(&c).unwrap();
    let d = list.iter().find(|d| d.name == "muse_ecg").expect("one database of the two files");
    assert_eq!((d.rows, d.patients, d.categories.clone()), (3, 2, vec!["ecg".to_string()]));
    let stored: String = c
        .query_row("SELECT group_concat(cells, '|') FROM import_rows WHERE import_id = ?1", [r.import_id], |r| r.get(0))
        .unwrap();
    for gone in ["Zorbluff", "ACCT777001"] {
        assert!(!stored.contains(gone), "{gone} kept");
    }
    assert!(stored.contains("2024-01-02 08:15:30") && stored.contains("1950-03-04"), "converted: {stored}");
    // the patients, made from the rows: sex, date of birth; an ECG with its file
    let (sex, birth): (String, String) = c
        .query_row(
            "SELECT p.sex, i.birth_date FROM patients p JOIN patient_identifiers i ON i.patient = p.id
             JOIN patient_mrns m ON m.patient = p.id WHERE m.mrn = 'SYN7001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((sex.as_str(), birth.as_str()), ("F", "1950-03-04"));
    let files: i64 = c
        .query_row("SELECT COUNT(*) FROM ehr_items WHERE category = 'ecg' AND file_path LIKE '%.xml'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 3);
    // cohorts on its columns
    let ad = admin();
    let expr = |text: &str| Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "expression".into(), text: text.into(), ..Default::default() })),
        ..Default::default()
    };
    let found = |text: &str| cohorts::evaluate(&c, &ad, "UTC", &expr(text)).unwrap().len();
    assert_eq!(found("muse_ecg.QTc >= 470"), 1);
    assert_eq!(found("latest muse_ecg.QTc >= 470"), 0, "her latest ECG's QTc is 440");
    assert_eq!(found("muse_ecg.interpretation contains \"fibrillation\""), 1);
    assert_eq!(found("has ecg"), 2);
    // without the PHI permission: the patients' names and dates of birth are not shown
    let p = medorama::databases::rows(
        &c,
        &reader(),
        None,
        d.id,
        &medorama::databases::Query { limit: 10, ..Default::default() },
    )
    .unwrap();
    let shown = serde_json::to_string(&p.rows).unwrap();
    assert!(!shown.contains("Testperson") && !shown.contains("1950-03-04") && !shown.contains("SYN7001"), "{shown}");
    // a file of other columns is not part of the same database
    let other = f.root.join("other.csv");
    std::fs::write(&other, "MRN,QT (ms)\nSYN7003,400\n").unwrap();
    let err = tables::import_parts(
        &mut c,
        &f.cfg,
        &admin(),
        &[part(&a), part(&other)],
        &HashMap::new(),
        "",
        None,
        &Opts::new("mixed", "", true),
        medorama::formats::Choice::Detect,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("other.csv: not a GE MUSE metadata table"), "{err}");
    // plain tables of different columns neither
    let third = f.root.join("third.csv");
    std::fs::write(&third, "MRN,PR (ms)\nSYN7004,160\n").unwrap();
    let err = tables::import_parts(
        &mut c,
        &f.cfg,
        &admin(),
        &[part(&other), part(&third)],
        &HashMap::new(),
        "",
        None,
        &Opts::new("mixed", "", true),
        medorama::formats::Choice::Detect,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("third.csv: its columns are not those of other.csv"), "{err}");
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn a_cohort_takes_only_its_new_patients() {
    use medorama::cohorts::{self, Criteria, Criterion, Expr, NewCohort, RefreshOpts};
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let p1 = load(&f, &mut c);
    let owner = admin();
    let female = Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "expression".into(), text: "female".into(), ..Default::default() })),
        ..Default::default()
    };
    let co = cohorts::create(
        &c,
        &owner,
        "UTC",
        &NewCohort { name: "Female".into(), criteria: female, ..Default::default() },
    )
    .unwrap();
    let members = |c: &Connection| {
        let mut v: Vec<i64> = cohorts::member_ids(c, &owner, co).unwrap().into_iter().collect();
        v.sort();
        v
    };
    assert_eq!(members(&c), [p1]);
    // new data: two new women; the first member's sex corrected
    import(&f, &mut c, "more.csv", b"Patient ID,Sex\nP4,F\nP5,F\nP1,M\n", "");
    let pid = |c: &Connection, p: &str| -> i64 {
        c.query_row("SELECT id FROM patients WHERE patient_id = ?1", [p], |r| r.get(0)).unwrap()
    };
    let (p4, p5) = (pid(&c, "P4"), pid(&c, "P5"));
    let opts = |dry_run: bool, join: Option<Vec<i64>>, leave: Option<bool>| RefreshOpts {
        dry_run,
        join,
        leave,
        ..Default::default()
    };
    let r = cohorts::refresh_with(&c, &owner, "UTC", co, &opts(true, None, None)).unwrap();
    assert_eq!((r.added.as_slice(), r.removed.as_slice()), ([p4, p5].as_slice(), [p1].as_slice()));
    assert_eq!((r.joining.len(), r.leaving.len()), (2, 1), "the preview lists them");
    let mut listed: Vec<String> = r
        .joining
        .iter()
        .map(|p| serde_json::to_value(p).unwrap()["patient_id"].as_str().unwrap().to_string())
        .collect();
    listed.sort();
    assert_eq!(listed, ["P4", "P5"]);
    // only the one chosen of the new ones; nobody leaves
    let r = cohorts::refresh_with(&c, &owner, "UTC", co, &opts(false, Some(vec![p4]), Some(false))).unwrap();
    assert_eq!((r.added.as_slice(), r.removed.len()), ([p4].as_slice(), 0));
    assert_eq!(members(&c), [p1, p4]);
    // the other new one, still to join; then all the new ones only
    let r = cohorts::refresh_with(&c, &owner, "UTC", co, &opts(true, None, None)).unwrap();
    assert_eq!((r.added.as_slice(), r.removed.as_slice()), ([p5].as_slice(), [p1].as_slice()));
    cohorts::refresh_with(&c, &owner, "UTC", co, &opts(false, None, Some(false))).unwrap();
    assert_eq!(members(&c), [p1, p4, p5]);
    // every change
    cohorts::refresh(&c, &owner, "UTC", co, false).unwrap();
    assert_eq!(members(&c), [p4, p5]);
    // a choice outside those who would join adds nobody
    let r = cohorts::refresh_with(&c, &owner, "UTC", co, &opts(false, Some(vec![p1]), None)).unwrap();
    assert!(r.added.is_empty());
    assert_eq!(members(&c), [p4, p5]);
}

/// What a table's rows say wrongly is said, at the row's line in its file: a column twice, a
/// year of two digits, milliseconds for seconds, a date of birth in the future; a table of
/// thousands of columns still imports (its criteria read the cells).
#[test]
fn rows_are_named_by_their_line_and_file() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    // a blank line, and a cell over two lines, before the bad rows
    let a = f.root.join("data").join("a.csv");
    std::fs::write(
        &a,
        "patient_id,encounter_id,domain,name,value,value,time\n\
         P1,E1,observation,Note,\"two\nlines\",x,2024-01-01 10:00\n\
         \n\
         P1,E1,observation,Weight,70,y,03/04/24\n\
         P1,E1,observation,Weight,71,z,1709380800000\n",
    )
    .unwrap();
    let b = f.root.join("data").join("b.csv");
    std::fs::write(
        &b,
        "patient_id,encounter_id,domain,name,value,value,time\nP1,E1,observation,Weight,72,w,2300-01-01\n",
    )
    .unwrap();
    import(&f, &mut c, "adt", b"patient_id,encounter_id,birth_date\nP1,E1,1950-01-01\nP2,E2,2999-01-01\n", "");
    let part = |p: &PathBuf| tables::Part {
        name: p.file_name().unwrap().to_string_lossy().to_string(),
        data: tables::PartData::File(p.clone()),
    };
    let r = tables::import_parts(
        &mut c,
        &f.cfg,
        &admin(),
        &[part(&a), part(&b)],
        &HashMap::new(),
        "",
        None,
        &Opts::new("obs", "", false),
        medorama::formats::Choice::Detect,
    )
    .unwrap();
    let w = r.warnings.join("\n");
    assert!(w.contains("the column value appears twice: the first is read as value"), "{w}");
    assert!(w.contains("a.csv, line 5: not a time: \"03/04/24\" (write the year with its four digits)"), "{w}");
    assert!(w.contains("a.csv, line 6: 1709380800000 is too large for Unix seconds"), "{w}");
    assert!(w.contains("b.csv, line 2: not a time: \"2300-01-01\""), "{w}");
    assert_eq!(r.items_added, 1, "{w}");
    // the first `value` column is the value
    let v: String = c.query_row("SELECT value_text FROM ehr_items WHERE name = 'Note'", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "two\nlines");
    // a date of birth in the future: the row says so
    let adt = imports::list(&c).unwrap().into_iter().find(|b| b.name == "adt").unwrap();
    assert_eq!(adt.summary["warnings"], 1);

    // thousands of columns: no table of numbers (too wide for one), the rows still imported
    let mut wide = (0..2100).map(|i| format!("c{i}")).collect::<Vec<_>>().join(",");
    wide = format!("patient_id,{wide}\nP9,{}\n", (0..2100).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    let r = import(&f, &mut c, "wide", wide.as_bytes(), "");
    assert!(!c.table_exists(&format!("dbcol_{}", r.import_id)).unwrap());
    let rows: i64 =
        c.query_row("SELECT COUNT(*) FROM import_rows WHERE import_id = ?1", [r.import_id], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1);
}

/// A recording a row names with a patient but no encounter: added, and said to be linked to
/// no encounter by the row.
#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn a_recording_without_its_encounter_is_said() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let rec = f.root.join("data").join(bed_name("ICU7", "12", T0));
    wfdb_record(&rec, 0);
    let t = format!("patient_id,file\nP1,{}\n", rec.display());
    let r = import(&f, &mut c, "recs", t.as_bytes(), "");
    assert_eq!(r.recordings_added, 1);
    assert!(r.warnings.iter().any(|w| w.contains("names no encounter_id")), "{:?}", r.warnings);
}
