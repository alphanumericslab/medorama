//! Privacy Mode end to end (docs/security.md, "The privacy gate"): a catalog seeded with
//! identifiers that cannot appear by chance, in every format the catalog serves (tables, notes,
//! WFDB headers, GE MUSE XML, DICOM, pictures, ICU recordings), then every page's answers,
//! every file and every export, as a user without the PHI permission and as an admin who
//! switched Privacy Mode on.  None of the identifiers may appear in any byte sent; their
//! placeholders and pseudonyms must.  The same answers, identified, hold them (the test would
//! prove nothing otherwise).

mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{bed_name, wfdb_record};
use http_body_util::BodyExt;
use medorama::config::Config;
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::users::{Role, User};
use serde_json::Value;
use std::collections::HashMap;
use tower::ServiceExt;

/// What must never leave in Privacy Mode (compared ignoring case).
const CANARIES: &[&str] = &[
    "Zyxwortha",      // last name
    "Quilbert",       // first name
    "MRN83651927",    // MRN
    "PIDX77214",      // imported patient ID
    "ENC55102",       // imported encounter ID
    "CSN99183",       // account number
    "1931-05-17",     // date of birth
    "19310517",       // the same, as DICOM writes it
    "05-17-1931",     // and as MUSE does
    "Kwame",          // staff
    "Osei",           // staff
    "2024-01-0",      // real dates (the stay: 2024-01-01 to 2024-01-05)
    "20240102",       // as DICOM writes them
    "01-02-2024",     // as MUSE does
    "02/01/2024",     // as WFDB does
    "02.01.24",       // and EDF, in its header
    "02-JAN-2024",    // and EDF+, in its recording field
    "01/02/2024",     // in free text, month first
    "Jan 2, 2024",    // and in words
    "2 January 2024", // and the other way
    "Canary General", // the institution (DICOM)
];

/// The stay's real times (Unix seconds), with a margin: no answer may hold a number in it.
const REAL: std::ops::Range<f64> = 1_704_000_000.0..1_704_600_000.0;

fn dicom(name: &str, id: &str, date: &str) -> Vec<u8> {
    fn el(tag: u32, vr: &[u8; 2], v: &str) -> Vec<u8> {
        let mut v = v.as_bytes().to_vec();
        if v.len() % 2 == 1 {
            v.push(if vr == b"UI" { 0 } else { b' ' });
        }
        let mut out = vec![];
        out.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
        out.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(vr);
        if vr == b"OW" {
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        } else {
            out.extend_from_slice(&(v.len() as u16).to_le_bytes());
        }
        out.extend(v);
        out
    }
    let mut meta = el(0x0002_0002, b"UI", "1.2.840.10008.5.1.4.1.1.7");
    meta.extend(el(0x0002_0003, b"UI", "1.2.3.999.1"));
    meta.extend(el(0x0002_0010, b"UI", "1.2.840.10008.1.2.1"));
    let mut out = vec![0u8; 128];
    out.extend_from_slice(b"DICM");
    out.extend_from_slice(&[0x02, 0, 0, 0, b'U', b'L', 4, 0]);
    out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    out.extend(meta);
    for (tag, vr, v) in [
        (0x0008_0016u32, b"UI", "1.2.840.10008.5.1.4.1.1.7"),
        (0x0008_0018, b"UI", "1.2.3.999.1"),
        (0x0008_0020, b"DA", date),
        (0x0008_0030, b"TM", "080000"),
        (0x0008_0060, b"CS", "OT"),
        (0x0008_0080, b"LO", "Canary General"),
        (0x0008_0090, b"PN", "Osei^Kwame"),
        (0x0008_1030, b"LO", "CT head Zyxwortha"), // Study Description: free text
        (0x0008_103E, b"LO", "Series for Quilbert"), // Series Description
        (0x0010_0010, b"PN", name),
        (0x0010_0020, b"LO", id),
        (0x0010_0030, b"DA", "19310517"),
        (0x0010_0040, b"CS", "F"),
        (0x0020_000D, b"UI", "1.2.3.999"),
        (0x0028_0002, b"US", "\u{1}\u{0}"),
    ] {
        out.extend(el(tag, vr, v));
    }
    out.extend(el(0x7FE0_0010, b"OW", "PIXELS"));
    out
}

/// A DICOM file in implicit VR little endian (Part 10): the VRs are not written, so the rewrite
/// knows the person names by their tags, and finds other text by its content.
fn dicom_implicit() -> Vec<u8> {
    fn el(tag: u32, v: &[u8]) -> Vec<u8> {
        let mut v = v.to_vec();
        if v.len() % 2 == 1 {
            v.push(b' ');
        }
        let mut out = vec![];
        out.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
        out.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        out.extend(v);
        out
    }
    fn explicit(tag: u32, v: &str) -> Vec<u8> {
        let mut v = v.as_bytes().to_vec();
        if v.len() % 2 == 1 {
            v.push(0);
        }
        let mut out = vec![];
        out.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
        out.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(b"UI");
        out.extend_from_slice(&(v.len() as u16).to_le_bytes());
        out.extend(v);
        out
    }
    let mut meta = explicit(0x0002_0003, "1.2.3.999.7");
    meta.extend(explicit(0x0002_0010, "1.2.840.10008.1.2"));
    let mut out = vec![0u8; 128];
    out.extend_from_slice(b"DICM");
    out.extend_from_slice(&[0x02, 0, 0, 0, b'U', b'L', 4, 0]);
    out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    out.extend(meta);
    for (tag, v) in [
        (0x0008_0020u32, b"20240102".as_slice()),
        (0x0008_1070, b"Osei^Kwame"),              // Operators' Name (PN)
        (0x0008_1090, b"Zyxwortha special model"), // Manufacturer's Model Name (LO)
        (0x0010_0010, b"Zyxwortha^Quilbert"),
        (0x0010_0020, b"MRN83651927"),
        (0x0028_0010, &[0x02, 0x00]), // Rows (US)
    ] {
        out.extend(el(tag, v));
    }
    out.extend(el(0x7FE0_0010, b"PIXELS"));
    out
}

/// An EDF+ ECG of two leads, one second, identified as EDF+ writes it (patient and recording
/// fields), with an annotation that names the patient.
fn edf() -> Vec<u8> {
    edf_file("EDF+C")
}

/// An ECG in plain EDF (`EDF`, free text in the patient and recording fields), in BDF (`BDF`),
/// or in EDF+ (`EDF+C`, with annotations).
fn edf_file(kind: &str) -> Vec<u8> {
    let field = |t: &str, w: usize| format!("{t:<w$}").into_bytes();
    let plus = kind == "EDF+C";
    let bdf = kind == "BDF";
    let signals: &[(&str, &str, usize)] = if plus {
        &[("I", "mV", 500usize), ("II", "mV", 500), ("EDF Annotations", "", 30)]
    } else {
        &[("I", "mV", 500usize), ("II", "mV", 500)]
    };
    let mut h = if bdf { [&[0xFFu8][..], b"BIOSEMI"].concat() } else { field("0", 8) };
    if plus {
        h.extend(field("MRN83651927 F 17-MAY-1931 Zyxwortha_Quilbert", 80));
        h.extend(field("Startdate 02-JAN-2024 ENC55102 Osei MAC5500", 80));
    } else {
        h.extend(field("Zyxwortha Quilbert MRN83651927 born 1931-05-17", 80));
        h.extend(field("Recorded by Osei at Canary General on 2024-01-02", 80));
    }
    h.extend(field("02.01.24", 8));
    h.extend(field("08.10.00", 8));
    h.extend(field(&(256 * (signals.len() + 1)).to_string(), 8));
    h.extend(field(
        if bdf {
            "24BIT"
        } else if plus {
            "EDF+C"
        } else {
            ""
        },
        44,
    ));
    h.extend(field("1", 8));
    h.extend(field("1", 8));
    h.extend(field(&signals.len().to_string(), 4));
    let each = |h: &mut Vec<u8>, w: usize, f: &dyn Fn(&(&str, &str, usize)) -> String| {
        for s in signals {
            h.extend(field(&f(s), w));
        }
    };
    each(&mut h, 16, &|s| s.0.into());
    each(&mut h, 80, &|_| String::new());
    each(&mut h, 8, &|s| s.1.into());
    each(&mut h, 8, &|_| "-5".into());
    each(&mut h, 8, &|_| "5".into());
    each(&mut h, 8, &|_| if bdf { "-8388608" } else { "-32768" }.into());
    each(&mut h, 8, &|_| if bdf { "8388607" } else { "32767" }.into());
    each(&mut h, 80, &|_| String::new());
    each(&mut h, 8, &|s| s.2.to_string());
    each(&mut h, 32, &|_| String::new());
    h.extend(vec![0u8; if bdf { 3 } else { 2 } * 500 * 2]);
    if plus {
        let mut notes = b"+0\x14\x14\x00+0.5\x14Zyxwortha awake, seen by Osei\x14\x00".to_vec();
        notes.resize(60, 0);
        h.extend(notes);
    }
    h
}

const MUSE: &str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<RestingECG><MuseInfo><MuseVersion>9.0.9</MuseVersion></MuseInfo>
<PatientDemographics><PatientID>MRN83651927</PatientID><PatientAge>92</PatientAge><AgeUnits>YEARS</AgeUnits>
<DateofBirth>05-17-1931</DateofBirth><Gender>FEMALE</Gender><PatientLastName>ZYXWORTHA</PatientLastName><PatientFirstName>QUILBERT</PatientFirstName></PatientDemographics>
<TestDemographics><AcquisitionDate>01-02-2024</AcquisitionDate><AcquisitionTime>08:00:00</AcquisitionTime>
<OverreaderLastName>OSEI</OverreaderLastName><OverreaderFirstName>KWAME</OverreaderFirstName><EditorLastName>OSEI</EditorLastName></TestDemographics>
<Order><RequestingMDLastName>OSEI</RequestingMDLastName></Order>
<Diagnosis><DiagnosisStatement><StmtText>Sinus rhythm. When compared with ECG of 01-JAN-2024 08:00</StmtText></DiagnosisStatement></Diagnosis>
</RestingECG>"#;

/// GE MUSE XML as other versions write it: attributes in the start tags, and statements that
/// name the patient and write dates in words.
const MUSE_VARIANT: &str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<RestingECG xmlns="urn:x"><PatientDemographics xmlns="urn:x" ><PatientID>MRN83651927</PatientID><PatientAge>92</PatientAge>
<DateofBirth >05-17-1931</DateofBirth><PatientLastName>ZYXWORTHA</PatientLastName></PatientDemographics >
<TestDemographics><AcquisitionDate>01-02-2024</AcquisitionDate></TestDemographics>
<Diagnosis><DiagnosisStatement><StmtText>Quilbert Zyxwortha seen Jan 2, 2024; again on 2 January 2024.</StmtText></DiagnosisStatement></Diagnosis>
</RestingECG>"#;

struct Fx {
    _tmp: tempfile::TempDir,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let data = root.join("data");
    cfg.data_roots = vec![data.to_string_lossy().to_string()];
    std::fs::create_dir_all(data.join("ecg")).unwrap();
    std::fs::create_dir_all(data.join("img/series")).unwrap();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let admin = User::local();
    let import = |c: &mut medorama::sql::Connection, name: &str, text: &str, category: &str| {
        medorama::tables::import(
            c,
            &cfg,
            &admin,
            name,
            text.as_bytes(),
            &HashMap::new(),
            category,
            Some(&data),
            &Opts::new(name, "", false),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    // files, named as some sites name them: after the patient
    let hea = "PIDX77214_ecg 2 500 1000 08:00:00 02/01/2024\n\
PIDX77214_ecg.dat 16 200/mV 16 0 0 0 0 I\nPIDX77214_ecg.dat 16 200/mV 16 0 0 0 0 II\n\
# Patient: Zyxwortha^Quilbert\n# MRN: MRN83651927\n# DOB: 1931-05-17\n# Age: 92\n# Acquired: 2024-01-02 08:00\n\
# Confirmed by: Dr. Kwame Osei\n# Technician: Osei\n\
# Zyxwortha Quilbert, 92 F\n# Name=Zyxwortha\n# Patient name - Quilbert\n# Patient_Name: Zyxwortha\n\
# Birthdate: 1931-05-17\n# seen 01/02/2024 at 08:00, born 1931-05-17\n";
    std::fs::write(data.join("ecg/PIDX77214_ecg.hea"), hea).unwrap();
    std::fs::write(data.join("ecg/PIDX77214_ecg.dat"), vec![0u8; 4000]).unwrap();
    std::fs::write(data.join("ecg/PIDX77214_muse_20240102.xml"), MUSE).unwrap();
    std::fs::write(data.join("ecg/PIDX77214_ecg.edf"), edf()).unwrap();
    std::fs::write(data.join("ecg/PIDX77214_muse_b.xml"), MUSE_VARIANT).unwrap();
    std::fs::write(data.join("ecg/PIDX77214_plain.edf"), edf_file("EDF")).unwrap();
    std::fs::write(data.join("ecg/PIDX77214_plain.bdf"), edf_file("BDF")).unwrap();
    std::fs::write(data.join("img/PIDX77214_implicit.dcm"), dicom_implicit()).unwrap();
    std::fs::write(data.join("img/PIDX77214_ct.dcm"), dicom("Zyxwortha^Quilbert", "MRN83651927", "20240102")).unwrap();
    for i in 1..=2 {
        std::fs::write(
            data.join(format!("img/series/PIDX77214_{i}.dcm")),
            dicom("Zyxwortha^Quilbert", "PIDX77214", "20240102"),
        )
        .unwrap();
    }
    std::fs::write(data.join("img/PIDX77214_photo.png"), b"\x89PNG\r\n\x1a\n not really a picture").unwrap();
    std::fs::write(data.join("img/PIDX77214_letter.txt"), "Dear Dr. Osei, Zyxwortha Quilbert was seen on 2024-01-02.")
        .unwrap();
    let rec = data.join("icu").join(bed_name("ICU7", "12", 1_704_110_400));
    wfdb_record(&rec, 0);
    let rel = rec.strip_prefix(&data).unwrap().to_string_lossy().to_string();
    import(
        &mut c,
        "patients.csv",
        "MRN,Last Name,First Name,DOB,Sex,Patient ID\nMRN83651927,Zyxwortha,Quilbert,1931-05-17,F,PIDX77214\n",
        "",
    );
    import(
        &mut c,
        "encounters.csv",
        "Encounter ID,Patient ID,CSN,Admit Time,Discharge Time,Admit Unit,Care Unit,Bed,Bed Start,Bed End\n\
ENC55102,PIDX77214,CSN99183,2024-01-01 10:00,2024-01-05 10:00,Cardiology,ICU7,12,2024-01-01 11:00,2024-01-03 11:00\n",
        "",
    );
    import(
        &mut c,
        "labs.csv",
        "MRN,Encounter ID,Test Name,Result,Units,Collected\nMRN83651927,ENC55102,Lactate,4.1,mmol/L,2024-01-02 06:10\n",
        "",
    );
    import(
        &mut c,
        "notes.csv",
        "patient_id,encounter_id,time,note_type,author,note_text\n\
PIDX77214,ENC55102,2024-01-02 15:40,ICU progress note,Dr. Kwame Osei,\"Zyxwortha, Quilbert (MRN83651927), born 1931-05-17: septic shock, improving.\"\n",
        "",
    );
    import(
        &mut c,
        "ecg_index.csv",
        "patient_id,encounter_id,time,interpretation,file\nPIDX77214,ENC55102,2024-01-02 08:00,Sinus rhythm,ecg/PIDX77214_ecg.hea\n\
PIDX77214,ENC55102,2024-01-02 08:05,Sinus rhythm,ecg/PIDX77214_muse_20240102.xml\n\
PIDX77214,ENC55102,2024-01-02 08:10,Sinus rhythm,ecg/PIDX77214_ecg.edf\n\
PIDX77214,ENC55102,2024-01-02 08:15,Sinus rhythm,ecg/PIDX77214_muse_b.xml\n\
PIDX77214,ENC55102,2024-01-02 08:20,Sinus rhythm,ecg/PIDX77214_plain.edf\n\
PIDX77214,ENC55102,2024-01-02 08:25,Sinus rhythm,ecg/PIDX77214_plain.bdf\n",
        "ecg",
    );
    import(
        &mut c,
        "imaging_index.csv",
        "patient_id,encounter_id,time,study,file\nPIDX77214,ENC55102,2024-01-02 09:00,CT,img/PIDX77214_ct.dcm\n\
PIDX77214,ENC55102,2024-01-02 09:10,CT series,img/series/\nPIDX77214,ENC55102,2024-01-02 09:20,Photo,img/PIDX77214_photo.png\n\
PIDX77214,ENC55102,2024-01-02 09:30,Letter,img/PIDX77214_letter.txt\n\
PIDX77214,ENC55102,2024-01-02 09:40,CT (implicit VR),img/PIDX77214_implicit.dcm\n",
        "img",
    );
    import(&mut c, "icu_index.csv", &format!("patient_id,encounter_id,file\nPIDX77214,ENC55102,{rel}\n"), "");
    medorama::users::add(&c, "boss", "", Role::Admin, true, "0123456789ab").unwrap();
    medorama::users::add(&c, "reader", "", Role::Analyst, false, "0123456789ab").unwrap();
    drop(c);
    Fx { _tmp: tmp, cfg, db }
}

struct Client {
    app: axum::Router,
}

impl Client {
    async fn send(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, HashMap<String, String>, Vec<u8>) {
        let mut b = Request::builder().method(method).uri(uri).header("Cookie", cookie).header("X-Medorama", "1");
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let req = b.body(body.map_or(Body::empty(), |v| Body::from(v.to_string()))).unwrap();
        let r = self.app.clone().oneshot(req).await.unwrap();
        let status = r.status();
        let headers = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
        (status, headers, r.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    async fn login(&self, user: &str) -> String {
        let (status, h, _) = self
            .send("POST", "/api/v1/login", "", Some(serde_json::json!({"username": user, "password": "0123456789ab"})))
            .await;
        assert_eq!(status, StatusCode::OK);
        h["set-cookie"].split(';').next().unwrap().to_string()
    }
}

fn leaks(body: &[u8]) -> Vec<&'static str> {
    let text = String::from_utf8_lossy(body).to_lowercase();
    CANARIES.iter().filter(|c| text.contains(&c.to_lowercase())).copied().collect()
}

fn real_times(v: &Value, out: &mut Vec<f64>) {
    match v {
        Value::Number(n) => {
            if let Some(x) = n.as_f64().filter(|x| REAL.contains(x)) {
                out.push(x);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| real_times(x, out)),
        Value::Object(m) => m.values().for_each(|x| real_times(x, out)),
        _ => {}
    }
}

#[tokio::test]
async fn privacy_mode_sends_no_identifier() {
    let f = fixture();
    let app = medorama::server::router(std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(f.cfg.clone()),
        std::sync::Arc::new(Db::open(&f.cfg, false).unwrap()),
    )));
    let cl = Client { app };
    let api = "/api/v1";
    let boss = cl.login("boss").await;
    let reader = cl.login("reader").await;
    let id: i64 =
        f.db.get()
            .unwrap()
            .query_row("SELECT id FROM patients WHERE patient_id = 'PIDX77214'", [], |r| r.get(0))
            .unwrap();

    // identified (an admin, Privacy Mode off by default): the identifiers are there
    let (_, _, plain) = cl.send("GET", &format!("{api}/patients/{id}"), &boss, None).await;
    assert!(leaks(&plain).len() > 2, "{}", String::from_utf8_lossy(&plain));

    // a user without the PHI permission cannot leave Privacy Mode
    let (status, _, _) =
        cl.send("POST", &format!("{api}/privacy"), &reader, Some(serde_json::json!({"on": false}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, _, me) = cl.send("GET", &format!("{api}/me"), &reader, None).await;
    assert!(String::from_utf8_lossy(&me).contains(r#""privacy":true"#));

    // the admin switches Privacy Mode on, in a session of their own (the server keeps the choice)
    let boss_private = cl.login("boss").await;
    let (st, _, _) =
        cl.send("POST", &format!("{api}/privacy"), &boss_private, Some(serde_json::json!({"on": true}))).await;
    assert_eq!(st, StatusCode::OK);

    // a cohort to export (made identified, read in Privacy Mode)
    let (_, _, co) = cl
        .send("POST", &format!("{api}/cohorts"), &boss, Some(serde_json::json!({"name": "Canary", "criteria": {"version": 1, "include": {"kind": "has", "category": "ecg"}}})))
        .await;
    let co: Value = serde_json::from_slice(&co).unwrap();
    let cohort = co["id"].as_i64().unwrap();
    cl.send(
        "PATCH",
        &format!("{api}/cohorts/{cohort}"),
        &boss,
        Some(serde_json::json!({"shares": [{"user_id": 2, "can_edit": true}]})),
    )
    .await;

    for (who, cookie) in [("reader", reader.as_str()), ("admin in Privacy Mode", boss_private.as_str())] {
        let mut checked = 0;
        let mut seen_pseudonym = false;
        let mut seen_placeholder = false;
        let mut check = |what: &str, status: StatusCode, headers: &HashMap<String, String>, body: &[u8]| {
            let l = leaks(body);
            assert!(
                l.is_empty(),
                "{who}: {what} sends {l:?}: {}",
                String::from_utf8_lossy(&body[..body.len().min(3000)])
            );
            if headers.get("content-type").is_some_and(|t| t.starts_with("application/json")) {
                let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
                let mut t = vec![];
                real_times(&v, &mut t);
                assert!(t.is_empty(), "{who}: {what} sends real times {t:?}");
            }
            let text = String::from_utf8_lossy(body);
            seen_pseudonym |= text.contains("MO-");
            seen_placeholder |= text.contains("[Patient Name]") || text.contains("[MRN]");
            let _ = status;
            checked += 1;
        };
        // the pages' answers
        let mut uris: Vec<String> = [
            "/me",
            "/config",
            "/patients",
            "/patients?q=MO-",
            "/encounters",
            "/recordings",
            "/databases",
            "/cohorts",
            "/history",
            "/version",
            "/jobs",
        ]
        .iter()
        .map(|p| format!("{api}{p}"))
        .collect();
        for p in ["", "/journey", "/report", "/report?q=lactate", "/trends"] {
            uris.push(format!("{api}/patients/{id}{p}"));
        }
        for p in
            ["", "/members", "/history", "/export", "/export?what=criteria", "/export?what=data", "/export?what=xlsx"]
        {
            uris.push(format!("{api}/cohorts/{cohort}{p}"));
        }
        let (_, _, dbs) = cl.send("GET", &format!("{api}/databases"), cookie, None).await;
        for d in serde_json::from_slice::<Value>(&dbs).unwrap()["items"].as_array().unwrap() {
            uris.push(format!("{api}/databases/{}/rows?limit=500", d["id"]));
            uris.push(format!("{api}/databases/{}/export", d["id"]));
        }
        let (_, _, rep) = cl.send("GET", &format!("{api}/patients/{id}/report"), cookie, None).await;
        let entries: Vec<String> = serde_json::from_slice::<Value>(&rep).unwrap()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["key"].as_str().unwrap().to_string())
            .collect();
        assert!(entries.len() >= 8, "{entries:?}");
        let mut files = vec![];
        for k in &entries {
            let uri = format!("{api}/patients/{id}/detail?key={}", k.replace(':', "%3A"));
            let (_, _, d) = cl.send("GET", &uri, cookie, None).await;
            let d: Value = serde_json::from_slice(&d).unwrap();
            if let Some(a) = d.get("attachment") {
                files.push((
                    a["kind"].as_str().unwrap_or("").to_string(),
                    a["url"].as_str().unwrap().to_string(),
                    a["src"].as_str().unwrap().to_string(),
                ));
            }
            uris.push(uri);
        }
        for uri in &uris {
            let (status, h, body) = cl.send("GET", uri, cookie, None).await;
            // admins' pages may refuse a reader; whatever is sent is checked
            check(uri, status, &h, &body);
        }
        // the files, as the viewers fetch them
        assert!(files.len() >= 5, "{files:?}");
        let mut refused = 0;
        let mut marked = 0;
        let mut edf_sent = 0;
        let mut sent: HashMap<String, usize> = HashMap::new();
        for (kind, url, src) in &files {
            for u in [url, src] {
                let (status, h, body) = cl.send("GET", &format!("{api}/{u}"), cookie, None).await;
                check(u, status, &h, &body);
                refused += (status == StatusCode::FORBIDDEN) as usize;
                if status.is_success() && u == src {
                    *sent.entry(kind.clone()).or_default() += 1;
                }
                marked += h.contains_key("x-medorama-may-contain-phi") as usize;
                if status.is_success() && kind == "folder" && u == src {
                    let names: Value = serde_json::from_slice(&body).unwrap();
                    for n in names["files"].as_array().unwrap() {
                        let n = n.as_str().unwrap();
                        assert!(n.starts_with("mo-f"), "{n}");
                        let (st, h, b) = cl.send("GET", &format!("{api}/{u}{n}"), cookie, None).await;
                        assert!(st.is_success(), "{u}{n}: {st}");
                        check(n, st, &h, &b);
                    }
                }
                if kind == "edf" && u == src {
                    // de-identified, and still an EDF+ file: its time keeping kept, its date moved back
                    assert!(status.is_success(), "{who}: the EDF file is sent de-identified ({status})");
                    assert_eq!(&body[8..16], b"X X X X ");
                    let plus = body[192..].starts_with(b"EDF+");
                    assert!(
                        !plus
                            || body
                                .windows(16)
                                .any(|w| w == b"+0\x14\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00")
                    );
                    edf_sent += 1;
                }
                if status.is_success() && kind == "wfdb" && u == src {
                    // the header names its signal file by its token
                    let text = String::from_utf8_lossy(&body).to_string();
                    let dat = text.lines().nth(1).unwrap().split_whitespace().next().unwrap().to_string();
                    assert!(dat.starts_with("mo-f"), "{text}");
                    let base = src.rsplit_once('/').unwrap().0;
                    let (st, h, b) = cl.send("GET", &format!("{api}/{base}/{dat}"), cookie, None).await;
                    assert!(st.is_success(), "{dat}: {st}");
                    check(&dat, st, &h, &b);
                    // a real file name is not accepted in Privacy Mode
                    let (st, _, _) = cl.send("GET", &format!("{api}/{base}/PIDX77214_ecg.dat"), cookie, None).await;
                    assert_eq!(st, StatusCode::NOT_FOUND);
                }
            }
        }
        assert!(refused >= 1, "{who}: the text file is refused");
        assert_eq!(edf_sent, 3, "{who}: the EDF+, plain EDF and BDF files are among the files");
        // both MUSE files (one with attributes in its tags) and both DICOM files (one in implicit
        // VR) are sent, de-identified
        assert_eq!((sent.get("xml"), sent.get("dicom")), (Some(&2), Some(&2)), "{who}: {sent:?}");
        assert!(marked >= 2, "{who}: pictures and DICOM are marked as possibly identifying");
        // searching identifiers is refused, or finds nobody
        for q in ["name:zyxwortha", "mrn:MRN83651927", "dob:1931"] {
            let (status, _, _) = cl.send("GET", &format!("{api}/patients?q={q}"), cookie, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{q}");
        }
        for q in ["zyxwortha", "PIDX77214", "MRN83651927", "ENC55102"] {
            let (_, _, b) = cl.send("GET", &format!("{api}/patients?q={q}"), cookie, None).await;
            assert_eq!(serde_json::from_slice::<Value>(&b).unwrap()["total"], 0, "{who}: {q} finds a patient");
        }
        assert!(seen_pseudonym && seen_placeholder, "{who}: pseudonyms and placeholders are shown");
        assert!(checked > 40, "{who}: {checked} answers checked");
    }

    // each patient's dates move by their own offset, drawn once and kept: the same for every
    // session and user, on every page, as their pseudonym
    let draw: Option<f64> =
        f.db.get().unwrap().query_row("SELECT date_draw FROM patients WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(draw.is_some_and(|d| (0.0..1.0).contains(&d)), "a draw: {draw:?}");
    for p in ["", "/journey", "/report"] {
        let (_, _, a) = cl.send("GET", &format!("{api}/patients/{id}{p}"), &reader, None).await;
        let (_, _, b) = cl.send("GET", &format!("{api}/patients/{id}{p}"), &boss_private, None).await;
        let reader2 = cl.login("reader").await;
        let (_, _, c) = cl.send("GET", &format!("{api}/patients/{id}{p}"), &reader2, None).await;
        assert!(a == b && a == c, "{p}: the same dates for every session");
    }
    // the catalog's list moves them as the patient's page does
    let (_, _, list) = cl.send("GET", &format!("{api}/patients"), &reader, None).await;
    let (_, _, page) = cl.send("GET", &format!("{api}/patients/{id}"), &reader, None).await;
    let list: Value = serde_json::from_slice(&list).unwrap();
    let page: Value = serde_json::from_slice(&page).unwrap();
    let latest = &list["items"][0]["latest"];
    let admits: Vec<&Value> = page["encounters"].as_array().into_iter().flatten().map(|e| &e["admit"]).collect();
    assert!(latest.is_null() || admits.contains(&latest), "the list's {latest} among the page's {admits:?}");
}

fn app_of(f: &Fx) -> axum::Router {
    medorama::server::router(std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(f.cfg.clone()),
        std::sync::Arc::new(Db::open(&f.cfg, false).unwrap()),
    )))
}

fn id_of(f: &Fx, sql: &str) -> i64 {
    f.db.get().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
}

/// `(path, method)` of every route of the API router (server.rs), as tests/openapi.rs reads them.
fn api_routes() -> Vec<(String, String)> {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/server.rs")).unwrap();
    let start = src.find("let api = Router::new()").expect("the api router");
    let end = start + src[start..].find(";\n").unwrap();
    let mut out = vec![];
    for line in src[start..end].lines().map(str::trim).filter(|l| l.starts_with(".route(\"")) {
        let path = line[8..].split('"').next().unwrap().to_string();
        let handlers = &line[8 + path.len()..];
        for m in ["get", "post", "put", "patch", "delete"] {
            if [format!(" {m}("), format!(".{m}("), format!("::{m}(")].iter().any(|p| handlers.contains(p.as_str())) {
                out.push((path.clone(), m.to_string()));
            }
        }
    }
    out
}

/// Every GET route of the API, in Privacy Mode (a reader, and an admin who switched it on), and
/// the POST routes that answer with data: no identifier in any answer.  A new GET route without
/// a case here fails the test, so that none is forgotten.
#[tokio::test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn every_route_in_privacy_mode() {
    let f = fixture();
    let cl = Client { app: app_of(&f) };
    let api = "/api/v1";
    let reader = cl.login("reader").await;
    let boss = cl.login("boss").await;
    let boss_private = cl.login("boss").await;
    cl.send("POST", &format!("{api}/privacy"), &boss_private, Some(serde_json::json!({"on": true}))).await;
    let (_, _, co) = cl
        .send("POST", &format!("{api}/cohorts"), &boss, Some(serde_json::json!({"name": "Canary", "criteria": {"version": 1, "include": {"kind": "has", "category": "ecg"}}})))
        .await;
    let cohort = serde_json::from_slice::<Value>(&co).unwrap()["id"].as_i64().unwrap();
    cl.send(
        "PATCH",
        &format!("{api}/cohorts/{cohort}"),
        &boss,
        Some(serde_json::json!({"shares": [{"user_id": 2, "can_edit": true}]})),
    )
    .await;
    let patient = id_of(&f, "SELECT id FROM patients WHERE patient_id = 'PIDX77214'");
    let enc = id_of(&f, "SELECT id FROM encounters LIMIT 1");
    let rec = id_of(&f, "SELECT id FROM recordings LIMIT 1");
    let db = id_of(&f, "SELECT MIN(id) FROM imports");
    let item = id_of(&f, "SELECT id FROM ehr_items WHERE file_path LIKE '%_ct.dcm'");
    let folder = id_of(&f, "SELECT id FROM ehr_items WHERE file_path LIKE '%series%'");
    let fill = |path: &str| -> Option<Vec<String>> {
        let p = |s: &str| path.replace("{id}", s);
        Some(match path {
            _ if !path.contains('{') => vec![path.to_string()],
            "/encounters/{id}" => vec![p(&enc.to_string())],
            "/encounters/{id}/ehr" => vec![p(&enc.to_string()), format!("{}?patient_level=true", p(&enc.to_string()))],
            "/recordings/{id}" => vec![p(&rec.to_string())],
            "/databases/{id}/rows" | "/databases/{id}/export" => vec![p(&db.to_string())],
            "/cohorts/{id}" | "/cohorts/{id}/members" | "/cohorts/{id}/history" => vec![p(&cohort.to_string())],
            "/cohorts/{id}/export" => ["", "?what=criteria", "?what=data", "?what=xlsx"]
                .iter()
                .map(|q| format!("{}{q}", p(&cohort.to_string())))
                .collect(),
            "/import-tasks/{id}" => vec![p("1")],
            "/patients/{id}" | "/patients/{id}/journey" | "/patients/{id}/report" | "/patients/{id}/trends" => {
                vec![p(&patient.to_string())]
            }
            "/patients/{id}/detail" => vec![format!("{}?key=item%3A{item}", p(&patient.to_string()))],
            "/patients/{id}/file/{item}" => vec![p(&patient.to_string()).replace("{item}", &item.to_string())],
            "/patients/{id}/file/{item}/" => vec![p(&patient.to_string()).replace("{item}", &folder.to_string())],
            "/patients/{id}/file/{item}/{name}" => {
                vec![p(&patient.to_string()).replace("{item}", &folder.to_string()).replace("{name}", "mo-f0000.dcm")]
            }
            _ => return None,
        })
    };
    let routes = api_routes();
    let gets: Vec<&String> = routes.iter().filter(|r| r.1 == "get").map(|r| &r.0).collect();
    assert!(gets.len() > 30, "{gets:?}");
    let missing: Vec<&&String> = gets.iter().filter(|g| fill(g).is_none()).collect();
    assert!(missing.is_empty(), "GET routes without a case in this test: {missing:?}");
    let posts: Vec<(String, Value)> = vec![
        (
            "/cohorts/search".into(),
            serde_json::json!({"criteria": {"version": 1, "include": {"kind": "has", "category": "ecg"}}}),
        ),
        ("/cohorts/parse".into(), serde_json::json!({"text": "has ecg"})),
        (format!("/cohorts/{cohort}/clone"), serde_json::json!({})),
        (format!("/cohorts/{cohort}/refresh"), serde_json::json!({})),
        ("/check".into(), serde_json::json!({})),
        ("/recordings/remove".into(), serde_json::json!({"ids": [rec], "dry_run": true})),
        ("/encounters/remove".into(), serde_json::json!({"ids": [enc], "dry_run": true})),
        ("/encounters/remove-matching".into(), serde_json::json!({"dry_run": true})),
        ("/recordings/remove-matching".into(), serde_json::json!({"dry_run": true})),
        (format!("/imports/{db}/remove"), serde_json::json!({"dry_run": true})),
        ("/sources/remove".into(), serde_json::json!({"source": "patients.csv", "dry_run": true})),
    ];
    // the same answers, identified, hold identifiers (else this test would prove nothing)
    let (_, _, plain) = cl.send("GET", &format!("{api}/patients/{patient}"), &boss, None).await;
    assert!(!leaks(&plain).is_empty());
    for (who, cookie) in [("reader", &reader), ("admin in Privacy Mode", &boss_private)] {
        let mut answered = 0;
        for g in &gets {
            for uri in fill(g).unwrap() {
                let (st, _, b) = cl.send("GET", &format!("{api}{uri}"), cookie, None).await;
                let l = leaks(&b);
                assert!(
                    l.is_empty(),
                    "{who}: GET {uri} ({st}) sends {l:?}: {}",
                    String::from_utf8_lossy(&b[..b.len().min(2000)])
                );
                answered += st.is_success() as usize;
            }
        }
        assert!(answered > 25, "{who}: {answered} answers");
        for (p, body) in &posts {
            let (st, _, b) = cl.send("POST", &format!("{api}{p}"), cookie, Some(body.clone())).await;
            let l = leaks(&b);
            assert!(
                l.is_empty(),
                "{who}: POST {p} ({st}) sends {l:?}: {}",
                String::from_utf8_lossy(&b[..b.len().min(2000)])
            );
        }
    }
}

/// What reaches a Privacy Mode session around the gate: symbolic links and stray signal files
/// in attached folders, texts that name a patient's folder (an error, a failed job), the
/// results folders of jobs.
#[tokio::test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn around_the_gate() {
    let f = fixture();
    let root = medorama::config::real(f._tmp.path());
    let data = root.join("data");
    let series = data.join("img/series");
    // a symbolic link in an attached folder, to the catalog's own database
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.cfg.database, series.join("zz.png")).unwrap();
    // a signal file no header names, in an attached folder
    std::fs::write(series.join("stray.dat"), b"anything at all: Zyxwortha").unwrap();
    // a WFDB record whose signal file is a symbolic link out of the data
    std::fs::create_dir_all(data.join("linked")).unwrap();
    std::fs::write(data.join("linked/r.hea"), "r 1 250 10\nr.dat 16 200 16 0 0 0 0 I\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&f.cfg.database, data.join("linked/r.dat")).unwrap();
    // a folder of recordings named after the patient, imported
    let pf = data.join("Zyxwortha_Quilbert");
    wfdb_record(&pf.join(bed_name("ICU9", "3", 1_704_200_000)), 0);
    {
        let mut c = f.db.get().unwrap();
        let rows =
            format!("patient_id,encounter_id,file\nPIDX77214,ENC55102,{}\n", pf.strip_prefix(&data).unwrap().display());
        let import = |c: &mut medorama::sql::Connection, name: &str, rows: &str, category: &str| {
            medorama::tables::import(
                c,
                &f.cfg,
                &User::local(),
                name,
                rows.as_bytes(),
                &HashMap::new(),
                category,
                Some(&data),
                &Opts::new(name, "", false),
            )
            .unwrap()
        };
        import(&mut c, "more.csv", &rows, "");
        import(
            &mut c,
            "linked.csv",
            "patient_id,encounter_id,time,file\nPIDX77214,ENC55102,2024-01-02 10:05,linked/r.hea\n",
            "ecg",
        );
    }
    // a job that failed on a recording, its error naming the path (as the engine's errors do)
    let (rid, path): (i64, String) = f
        .db
        .get()
        .unwrap()
        .query_row("SELECT id, path FROM recordings WHERE path LIKE '%Zyxwortha%'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    f.db.get()
        .unwrap()
        .execute(
            "INSERT INTO jobs (action, action_version, params, params_hash, recording, fingerprint, state, priority, created_at, error, output_dir)
             VALUES ('index', '1', '{}', 'x', ?1, 'f', 'failed', 0, 1.0, ?2, ?3)",
            medorama::sql::params![rid, format!("not a recording this program reads: {path}/x.dat"), format!("{path}/results")],
        )
        .unwrap();
    let cl = Client { app: app_of(&f) };
    let api = "/api/v1";
    let reader = cl.login("reader").await;
    let boss = cl.login("boss").await;
    // the texts: the recording's path, its folder, the job's error and results folder
    for u in ["/jobs", "/recordings", "/encounters", "/databases"] {
        let (st, _, b) = cl.send("GET", &format!("{api}{u}"), &reader, None).await;
        assert!(st.is_success(), "{u}: {st}");
        assert!(leaks(&b).is_empty(), "{u}: {:?}: {}", leaks(&b), String::from_utf8_lossy(&b));
        assert!(!String::from_utf8_lossy(&b).contains(&*root.to_string_lossy()), "{u} names a folder");
    }
    let (_, _, jobs) = cl.send("GET", &format!("{api}/jobs"), &reader, None).await;
    let jobs: Value = serde_json::from_slice(&jobs).unwrap();
    let job = jobs["items"].as_array().unwrap().iter().find(|j| j["state"] == "failed").unwrap();
    assert_eq!((job["error"].as_str(), job["output_dir"].as_str()), (Some("failed"), Some("")), "{job}");
    // an admin, identified, sees both
    let (_, _, jobs) = cl.send("GET", &format!("{api}/jobs"), &boss, None).await;
    assert!(String::from_utf8_lossy(&jobs).contains("Zyxwortha_Quilbert/"));
    // the attached folder: neither the link nor the stray signal file is listed, or sent
    let patient = id_of(&f, "SELECT id FROM patients WHERE patient_id = 'PIDX77214'");
    let folder = id_of(&f, "SELECT id FROM ehr_items WHERE file_path LIKE '%series%'");
    let (_, _, b) = cl.send("GET", &format!("{api}/patients/{patient}/file/{folder}/"), &reader, None).await;
    let listed: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(listed["files"].as_array().unwrap().len(), 2, "the two DICOM files only: {listed}");
    for n in ["mo-f0002.dat", "mo-f0002.png", "mo-f0003.png"] {
        let (st, _, b) = cl.send("GET", &format!("{api}/patients/{patient}/file/{folder}/{n}"), &reader, None).await;
        assert!(!st.is_success() && !b.starts_with(b"SQLite format 3"), "{n}: {st}");
    }
    let (_, _, b) = cl.send("GET", &format!("{api}/patients/{patient}/file/{folder}/"), &boss, None).await;
    let listed: Value = serde_json::from_slice(&b).unwrap();
    assert!(!listed.to_string().contains("zz.png"), "a link is not a file of the folder, identified neither: {listed}");
    for n in ["zz.png", "stray.dat"] {
        let (st, _, b) = cl.send("GET", &format!("{api}/patients/{patient}/file/{folder}/{n}"), &boss, None).await;
        if n == "zz.png" {
            assert_eq!(st, StatusCode::NOT_FOUND, "{n}");
        }
        assert!(!b.starts_with(b"SQLite format 3"), "{n}");
    }
    let (st, _, _) = cl.send("GET", &format!("{api}/patients/{patient}/file/{folder}/stray.dat"), &reader, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "a real name in Privacy Mode");
    // the WFDB record's signal file, a link to the catalog's database: refused, both ways
    let hea = id_of(&f, "SELECT id FROM ehr_items WHERE file_path LIKE '%linked/r.hea'");
    for (cookie, name) in [(&boss, "r.dat"), (&reader, "mo-f0001.dat")] {
        let (st, _, b) = cl.send("GET", &format!("{api}/patients/{patient}/file/{hea}/{name}"), cookie, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{name}");
        assert!(!b.starts_with(b"SQLite format 3"));
    }
}

/// A held row applied by an admin adds identifiers (a new encounter ID, an account number):
/// the gate's dictionary knows them at once.
#[tokio::test]
async fn the_dictionary_follows_the_identifiers() {
    let f = fixture();
    let cl = Client { app: app_of(&f) };
    let api = "/api/v1";
    let reader = cl.login("reader").await;
    let boss = cl.login("boss").await;
    // the reader's answers build the dictionary
    cl.send("GET", &format!("{api}/encounters"), &reader, None).await;
    // a row for a known patient with another MRN and a new encounter: held back
    {
        let mut c = f.db.get().unwrap();
        medorama::tables::import(
            &mut c,
            &f.cfg,
            &User::local(),
            "late.csv",
            b"Patient ID,MRN,Encounter ID,CSN,Admit Time\nPIDX77214,MRN44112299,ENC66601,CSN66602,2024-01-03 10:00\n",
            &HashMap::new(),
            "",
            None,
            &Opts::new("late.csv", "", false),
        )
        .unwrap();
    }
    cl.send("GET", &format!("{api}/encounters"), &reader, None).await;
    let (_, _, rv) = cl.send("GET", &format!("{api}/review"), &boss, None).await;
    let rv: Value = serde_json::from_slice(&rv).unwrap();
    let held = rv["held"].as_array().unwrap();
    assert!(!held.is_empty(), "{rv}");
    for h in held {
        let (st, _, b) = cl
            .send(
                "POST",
                &format!("{api}/review/held/{}", h["id"]),
                &boss,
                Some(serde_json::json!({"action": "apply"})),
            )
            .await;
        assert!(st.is_success(), "{st} {}", String::from_utf8_lossy(&b));
    }
    let (_, _, b) = cl.send("GET", &format!("{api}/encounters"), &boss, None).await;
    assert!(String::from_utf8_lossy(&b).contains("ENC66601"), "the encounter was added");
    let patient = id_of(&f, "SELECT id FROM patients WHERE patient_id = 'PIDX77214'");
    for u in ["/encounters".to_string(), "/recordings".into(), format!("/patients/{patient}")] {
        let (_, _, b) = cl.send("GET", &format!("{api}{u}"), &reader, None).await;
        let t = String::from_utf8_lossy(&b);
        for gone in ["ENC66601", "CSN66602", "MRN44112299"] {
            assert!(!t.contains(gone), "{u}: {gone} shown: {t}");
        }
    }
}

/// The Privacy Mode choice is the session's, kept by the server: a cookie cannot make it, a
/// new sign-in starts in the user's default, and leaving Privacy Mode is audited.
#[tokio::test]
async fn the_privacy_choice_is_the_sessions() {
    let f = fixture();
    medorama::users::add(&f.db.get().unwrap(), "clinician", "", Role::Analyst, true, "0123456789ab").unwrap();
    let cl = Client { app: app_of(&f) };
    let api = "/api/v1";
    let privacy = |b: &[u8]| serde_json::from_slice::<Value>(b).unwrap()["user"]["privacy"].clone();
    let c1 = cl.login("clinician").await;
    let (_, _, me) = cl.send("GET", &format!("{api}/me"), &c1, None).await;
    assert_eq!(privacy(&me), true, "a non-admin starts in Privacy Mode");
    // the cookie the page once set (or a hand-made one): no effect
    let (_, _, me) = cl.send("GET", &format!("{api}/me"), &format!("{c1}; medorama_privacy=off"), None).await;
    assert_eq!(privacy(&me), true);
    let (_, _, b) = cl.send("GET", &format!("{api}/patients"), &format!("{c1}; medorama_privacy=off"), None).await;
    assert!(leaks(&b).is_empty());
    // switched off: for this session, audited
    let (st, h, _) = cl.send("POST", &format!("{api}/privacy"), &c1, Some(serde_json::json!({"on": false}))).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!h.contains_key("set-cookie"), "nothing kept in the browser");
    let (_, _, me) = cl.send("GET", &format!("{api}/me"), &c1, None).await;
    assert_eq!(privacy(&me), false);
    let n = id_of(&f, "SELECT COUNT(*) FROM audit WHERE event = 'privacy_off'");
    assert_eq!(n, 1);
    // another sign-in of the same user (the next person at the browser): the default again
    let c2 = cl.login("clinician").await;
    let (_, _, me) = cl.send("GET", &format!("{api}/me"), &c2, None).await;
    assert_eq!(privacy(&me), true);
    let (_, _, b) = cl.send("GET", &format!("{api}/patients"), &c2, None).await;
    assert!(leaks(&b).is_empty());
    // signed out and in again: the default again
    cl.send("POST", &format!("{api}/logout"), &c1, None).await;
    let c3 = cl.login("clinician").await;
    let (_, _, me) = cl.send("GET", &format!("{api}/me"), &c3, None).await;
    assert_eq!(privacy(&me), true);
}

/// The viewer's error texts name files: a Privacy Mode session gets a fixed sentence instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn the_viewers_errors_in_privacy_mode() {
    let root_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let viewer = std::env::var("MEDORAMA_TEST_VIEWER").map(std::path::PathBuf::from).unwrap_or_else(|_| {
        root_dir.join("target/release").join(format!("vitalstream-rs{}", std::env::consts::EXE_SUFFIX))
    });
    if !viewer.is_file() {
        eprintln!("no viewer built (target/release/vitalstream-rs): skipped");
        return;
    }
    let mut f = fixture();
    f.cfg.viewer = viewer.to_string_lossy().to_string();
    let data = medorama::config::real(f._tmp.path()).join("data");
    let pf = data.join("Zyxwortha_Quilbert");
    let rec = pf.join(bed_name("ICU9", "3", 1_704_200_000));
    wfdb_record(&rec, 0);
    {
        let mut c = f.db.get().unwrap();
        let rows = format!(
            "patient_id,encounter_id,file\nPIDX77214,ENC55102,{}\n",
            rec.strip_prefix(&data).unwrap().display()
        );
        medorama::tables::import(
            &mut c,
            &f.cfg,
            &User::local(),
            "more.csv",
            rows.as_bytes(),
            &HashMap::new(),
            "",
            Some(&data),
            &Opts::new("more.csv", "", false),
        )
        .unwrap();
    }
    let id = id_of(&f, "SELECT id FROM recordings WHERE path LIKE '%Zyxwortha%'");
    // the recording goes away after it was imported
    std::fs::remove_dir_all(&pf).unwrap();
    let app = std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(f.cfg.clone()),
        std::sync::Arc::new(Db::open(&f.cfg, false).unwrap()),
    ));
    let cl = Client { app: medorama::server::router(app.clone()) };
    let reader = cl.login("reader").await;
    let (st, _, b) = cl
        .send("POST", "/viewer/api/v1/open", &reader, Some(serde_json::json!({"path": format!("mo-rec:{id}")})))
        .await;
    app.viewer.stop();
    assert!(!st.is_success(), "{st}");
    assert!(leaks(&b).is_empty() && !String::from_utf8_lossy(&b).contains("ICU9"), "{}", String::from_utf8_lossy(&b));
}
