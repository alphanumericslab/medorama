//! Connected databases (docs/design/sources.md): a synthetic OMOP CDM database beside the
//! catalog, one of its persons also in an imported table (the same MRN).  Connecting it,
//! cohorts across both, the person registry, the patient page with both sources' data,
//! Privacy Mode over the connected data, disconnecting.  The OMOP database is a SQLite file,
//! and also a PostgreSQL schema when the tests run on PostgreSQL (`MEDORAMA_TEST_POSTGRES`).

mod common;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use medorama::cohorts::{self, Criteria};
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::sources::{self, NewSource, Settings};
use medorama::users::{Role, User};
use std::collections::HashMap;
use tower::ServiceExt;

/// What the connected data hold that Privacy Mode must never send.
const CANARIES: &[&str] = &["Zyxdoe", "Janelle", "SYN7000001", "OMOPMRN4471", "Pombert", "Qwyl", "1950-03-04"];

/// The real times of the OMOP data (Unix seconds, January 2024), with a margin.
const REAL: std::ops::Range<f64> = 1_703_900_000.0..1_706_000_000.0;

/// The OMOP tables (the columns Medorama reads) and their rows; `ts`, `date`, `num`: the
/// database's types; `s`: the schema's prefix.
fn omop(ts: &str, date: &str, num: &str, s: &str) -> String {
    format!(
        "CREATE TABLE {s}person (person_id BIGINT, gender_concept_id BIGINT, year_of_birth INTEGER, month_of_birth INTEGER,
             day_of_birth INTEGER, birth_datetime {ts}, person_source_value TEXT);
         CREATE TABLE {s}care_site (care_site_id BIGINT, care_site_name TEXT);
         CREATE TABLE {s}visit_occurrence (visit_occurrence_id BIGINT, person_id BIGINT, visit_concept_id BIGINT,
             visit_start_date {date}, visit_start_datetime {ts}, visit_end_date {date}, visit_end_datetime {ts}, care_site_id BIGINT);
         CREATE TABLE {s}visit_detail (visit_detail_id BIGINT, person_id BIGINT, visit_detail_concept_id BIGINT, visit_occurrence_id BIGINT);
         CREATE TABLE {s}measurement (measurement_id BIGINT, person_id BIGINT, measurement_concept_id BIGINT, measurement_date {date},
             measurement_datetime {ts}, value_as_number {num}, value_as_concept_id BIGINT, unit_concept_id BIGINT, range_low {num},
             range_high {num}, visit_occurrence_id BIGINT, measurement_source_value TEXT, unit_source_value TEXT, value_source_value TEXT);
         CREATE TABLE {s}observation (observation_id BIGINT, person_id BIGINT, observation_concept_id BIGINT, observation_date {date},
             observation_datetime {ts}, value_as_number {num}, value_as_string TEXT, value_as_concept_id BIGINT, unit_concept_id BIGINT,
             visit_occurrence_id BIGINT, observation_source_value TEXT, unit_source_value TEXT);
         CREATE TABLE {s}condition_occurrence (condition_occurrence_id BIGINT, person_id BIGINT, condition_concept_id BIGINT,
             condition_start_date {date}, condition_start_datetime {ts}, condition_end_date {date}, condition_end_datetime {ts},
             visit_occurrence_id BIGINT, condition_source_value TEXT, condition_source_concept_id BIGINT);
         CREATE TABLE {s}drug_exposure (drug_exposure_id BIGINT, person_id BIGINT, drug_concept_id BIGINT, drug_exposure_start_date {date},
             drug_exposure_start_datetime {ts}, drug_exposure_end_date {date}, drug_exposure_end_datetime {ts}, visit_occurrence_id BIGINT,
             drug_source_value TEXT, dose_unit_source_value TEXT);
         CREATE TABLE {s}procedure_occurrence (procedure_occurrence_id BIGINT, person_id BIGINT, procedure_concept_id BIGINT,
             procedure_date {date}, procedure_datetime {ts}, visit_occurrence_id BIGINT, procedure_source_value TEXT);
         CREATE TABLE {s}note (note_id BIGINT, person_id BIGINT, note_date {date}, note_datetime {ts}, note_type_concept_id BIGINT,
             note_title TEXT, note_text TEXT, provider_id BIGINT, visit_occurrence_id BIGINT);
         CREATE TABLE {s}provider (provider_id BIGINT, provider_name TEXT);
         CREATE TABLE {s}concept (concept_id BIGINT, concept_name TEXT, vocabulary_id TEXT, concept_code TEXT);
         CREATE TABLE {s}cdm_source (cdm_source_name TEXT, source_release_date {date}, cdm_release_date {date});

         INSERT INTO {s}concept VALUES (3047181, 'Lactate [Moles/volume] in Blood', 'LOINC', '2524-7'),
             (3027018, 'Heart rate', 'LOINC', '8867-4'), (8753, 'millimole per liter', 'UCUM', 'mmol/L'),
             (8541, 'per minute', 'UCUM', '/min'), (3019550, 'Troponin T.cardiac [Mass/volume] in Serum or Plasma', 'LOINC', '6598-7'),
             (8842, 'nanogram per liter', 'UCUM', 'ng/L'), (4329847, 'Myocardial infarction', 'SNOMED', '22298006'),
             (45576876, 'Subsequent non-ST elevation myocardial infarction', 'ICD10CM', 'I21.4'),
             (1367571, 'heparin', 'RxNorm', '5224'), (9201, 'Inpatient Visit', 'Visit', 'IP'), (44814637, 'Discharge summary', 'Type Concept', 'X');
         INSERT INTO {s}person VALUES (101, 8532, 1950, 3, 4, NULL, 'SYN7000001'), (102, 8507, 1960, 7, 8, NULL, 'OMOPMRN4471'),
             (103, 8532, 1970, NULL, NULL, NULL, 'OMOP3');
         INSERT INTO {s}care_site VALUES (1, 'Cardiology'), (2, 'MICU');
         INSERT INTO {s}visit_occurrence VALUES (1, 101, 9201, '2024-01-01', '2024-01-01 08:00:00', '2024-01-06', '2024-01-06 12:00:00', 1),
             (2, 102, 9201, '2024-01-10', '2024-01-10 09:00:00', '2024-01-15', '2024-01-15 10:00:00', 2);
         INSERT INTO {s}visit_detail VALUES (21, 102, 32037, 2);
         INSERT INTO {s}measurement VALUES
             (1, 101, 3047181, '2024-01-02', '2024-01-02 06:10:00', 4.6, NULL, 8753, 0.5, 2.0, 1, 'LACT', 'mmol/L', '4.6'),
             (2, 101, 3027018, '2024-01-02', '2024-01-02 06:00:00', 112, NULL, 8541, 60, 100, 1, 'HR', '/min', '112'),
             (3, 102, 3047181, '2024-01-11', '2024-01-11 07:00:00', 1.2, NULL, 8753, 0.5, 2.0, 2, 'LACT', 'mmol/L', '1.2'),
             (4, 103, 3019550, '2024-01-20', '2024-01-20 10:00:00', 183, NULL, 8842, 0, 14, NULL, 'TNT', 'ng/L', '183');
         INSERT INTO {s}condition_occurrence VALUES (1, 101, 4329847, '2024-01-01', '2024-01-01 09:00:00', NULL, NULL, 1, 'I21.4', 45576876);
         INSERT INTO {s}drug_exposure VALUES (1, 101, 1367571, '2024-01-02', '2024-01-02 08:00:00', '2024-01-04', '2024-01-04 08:00:00', 1, 'heparin', 'U');
         INSERT INTO {s}provider VALUES (7, 'Pombert Qwyl');
         INSERT INTO {s}note VALUES (1, 101, '2024-01-03', '2024-01-03 15:00:00', 44814637, 'Discharge summary',
             'Janelle Zyxdoe (MRN SYN7000001) was admitted with chest pain; lactate 4.6.', 7, 1);
         INSERT INTO {s}cdm_source VALUES ('Synthetic', '2024-02-01', '2024-02-02');"
    )
}

struct Fx {
    _tmp: tempfile::TempDir,
    cfg: medorama::config::Config,
    db: Db,
    patient: i64,
}

/// A catalog with one imported table of patients: P900001 has the MRN OMOP's person 101 has.
fn catalog() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let table = "MRN,Last Name,First Name,DOB,Sex,Patient ID\nSYN7000001,Zyxdoe,Janelle,1950-03-04,F,P900001\n\
                 SYN7000002,Other,Person,1980-01-01,M,P900002\n";
    medorama::tables::import(
        &mut c,
        &cfg,
        &User::local(),
        "patients.csv",
        table.as_bytes(),
        &HashMap::new(),
        "",
        None,
        &Opts::new("patients", "", false),
    )
    .unwrap();
    let labs = "MRN,Test Name,Result,Units,Collected\nSYN7000002,Lactate,5.1,mmol/L,2024-01-05 10:00\n";
    medorama::tables::import(
        &mut c,
        &cfg,
        &User::local(),
        "labs.csv",
        labs.as_bytes(),
        &HashMap::new(),
        "lab",
        None,
        &Opts::new("labs", "", false),
    )
    .unwrap();
    medorama::users::add(&c, "boss", "", Role::Admin, true, "0123456789ab").unwrap();
    medorama::users::add(&c, "reader", "", Role::Analyst, false, "0123456789ab").unwrap();
    let patient: i64 = c.query_row("SELECT id FROM patients WHERE patient_id = 'P900001'", [], |r| r.get(0)).unwrap();
    drop(c);
    Fx { _tmp: tmp, cfg, db, patient }
}

fn search(c: &medorama::sql::Connection, u: &User, text: &str) -> anyhow::Result<Vec<String>> {
    let e = cohorts::parse_text(c, u, text)?;
    let q = Criteria { include: Some(e), ..Default::default() };
    let (_, rows, _) = cohorts::search(c, u, "UTC", &q, None, 100, 0)?;
    let mut v: Vec<String> = rows.into_iter().map(|m| m.patient.patient_id).collect();
    v.sort();
    Ok(v)
}

/// An answer of the API: its body, and the cookie it sets (if any).
async fn get(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (Vec<u8>, Option<String>) {
    let mut b =
        Request::builder().method(if body.is_some() { "POST" } else { "GET" }).uri(uri).header("X-Medorama", "1");
    if !cookie.is_empty() {
        b = b.header("Cookie", cookie);
    }
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    let r =
        app.clone().oneshot(b.body(body.map_or(Body::empty(), |v| Body::from(v.to_string()))).unwrap()).await.unwrap();
    let (parts, body) = r.into_parts();
    let set = parts.headers.get("set-cookie").map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string());
    (body.collect().await.unwrap().to_bytes().to_vec(), set)
}

fn real_times(v: &serde_json::Value, out: &mut Vec<f64>) {
    match v {
        serde_json::Value::Number(n) => out.extend(n.as_f64().filter(|x| REAL.contains(x))),
        serde_json::Value::Array(a) => a.iter().for_each(|x| real_times(x, out)),
        serde_json::Value::Object(m) => m.values().for_each(|x| real_times(x, out)),
        _ => {}
    }
}

/// Everything, over the OMOP database at `url` (`schema`: its tables' schema).
async fn exercise(f: &Fx, url: &str, schema: &str) {
    let admin = User::local();
    let mut c = f.db.get().unwrap();
    // connecting checks the views first
    let bad =
        NewSource { name: "omop".into(), kind: "omop".into(), url: format!("{url}-missing"), ..Default::default() };
    assert!(sources::add(&c, &admin, &bad).is_err());
    let n = NewSource {
        name: "omop".into(),
        kind: "omop".into(),
        url: url.into(),
        settings: Settings { schema: schema.into(), timezone: "UTC".into(), ..Default::default() },
        ..Default::default()
    };
    let id = sources::add(&c, &admin, &n).unwrap();
    assert!(sources::add(&c, &admin, &n).unwrap_err().to_string().contains("exists"));
    let s = sources::get(&c, id).unwrap().unwrap();
    let ck = s.check().unwrap();
    assert_eq!(ck.views, vec!["persons", "visits", "entries"]);
    assert_eq!(ck.version, "2024-02-01 / 2024-02-02");

    // criteria across the catalog and OMOP: a name in both is asked about
    let err = search(&c, &admin, "lactate > 4").unwrap_err().to_string();
    assert!(err.contains("several databases") && err.contains("omop"), "{err}");
    // OMOP's lactate: its person 101 is the catalog's P900001 (the same MRN), linked now
    assert_eq!(search(&c, &admin, "omop.lactate > 4").unwrap(), vec!["P900001"]);
    assert_eq!(search(&c, &admin, "labs.lactate > 4").unwrap(), vec!["P900002"]);
    assert_eq!(search(&c, &admin, "labs.lactate > 4 OR omop.lactate > 4").unwrap(), vec!["P900001", "P900002"]);
    // criteria text as the page sends it (a criterion of kind expression): a person not linked
    // yet is found the first time, with a NOT among all the patients, those linked now included
    let page = |text: &str| -> Vec<String> {
        let k = cohorts::Criterion { kind: "expression".into(), text: text.into(), ..Default::default() };
        let q = Criteria { include: Some(cohorts::Expr::Leaf(k)), ..Default::default() };
        let (_, rows, _) = cohorts::search(&c, &admin, "UTC", &q, None, 100, 0).unwrap();
        let mut v: Vec<String> = rows.into_iter().map(|m| m.patient.patient_id).collect();
        v.sort();
        v
    };
    let found = page("omop.lactate < 2 AND NOT female");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("GEN-"), "{found:?}");
    // a long concept name written in full; units converted (1.2 mmol/L is 10.8 mg/dL)
    assert_eq!(search(&c, &admin, "omop.\"Lactate [Moles/volume] in Blood\" < 2").unwrap().len(), 1);
    let linked: i64 = c.query_row("SELECT COUNT(*) FROM source_persons", [], |r| r.get(0)).unwrap();
    assert_eq!(linked, 2, "persons are linked as searches find them");
    // a person only OMOP has becomes a patient of the registry (known by their MRN)
    let new: String = c
        .query_row(
            "SELECT p.patient_id FROM source_persons s JOIN patients p ON p.id = s.patient WHERE s.person_key = '102'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(new.starts_with("GEN-"), "{new}");
    // conditions by their ICD-10 code, drugs, stays, text, vital signs
    let has = |t: &str| search(&c, &admin, t).unwrap();
    assert_eq!(has("diagnosis I21"), vec!["P900001"]);
    assert_eq!(has("medication heparin"), vec!["P900001"]);
    assert_eq!(has("has icu"), vec![new.clone()]);
    assert_eq!(has("has vit"), vec!["P900001"]);
    assert_eq!(has("female AND diagnosis I21"), vec!["P900001"]);
    assert!(has("text \"chest pain\"").contains(&"P900001".to_string()), "notes are searched with the PHI permission");
    let reader = medorama::users::get_by_name(&c, "reader").unwrap().unwrap();
    assert!(!search(&c, &reader, "text \"chest pain\"").unwrap().contains(&"P900001".to_string()));
    // a criterion naming a missing measurement says so
    assert!(search(&c, &admin, "omop.potassium > 5").unwrap_err().to_string().contains("nothing is named"));

    // a cohort records the version of the data it was made from
    let q =
        Criteria { include: Some(cohorts::parse_text(&c, &admin, "omop.lactate > 4").unwrap()), ..Default::default() };
    let co = cohorts::create(
        &c,
        &admin,
        "UTC",
        &cohorts::NewCohort { name: "OMOP lactate".into(), criteria: q, ..Default::default() },
    )
    .unwrap();
    let got = cohorts::get(&c, &admin, co).unwrap();
    assert_eq!(got.source_versions.get("omop").map(String::as_str), Some("2024-02-01 / 2024-02-02"));
    assert!(got.source_versions.contains_key("catalog"));

    // linking every person: 103 has a year of birth only, and is linked by their MRN
    let (n, all) = sources::link::sync(&c, &s, s.zone("UTC"), &admin).unwrap();
    assert_eq!((n, all), (3, 3));

    // the patient page: the catalog's data and OMOP's, in the same columns
    let boss = medorama::users::get_by_name(&c, "boss").unwrap().unwrap();
    let j = medorama::patient::journey(&c, &boss, f.patient).unwrap();
    let lanes: Vec<&str> = j.lanes.iter().map(|l| l.key.as_str()).collect();
    for k in ["vit", "lab", "med", "dx", "note"] {
        assert!(lanes.contains(&k), "{lanes:?}");
    }
    assert!(j.spans.iter().any(|s| s.key.starts_with("live:")), "the heparin's span");
    let report = medorama::patient::report(&c, &boss, f.patient, "").unwrap();
    let keys: Vec<&str> = report.entries.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.iter().any(|k| k.starts_with("labs:0:")) && keys.iter().any(|k| k.starts_with("live:")), "{keys:?}");
    let text = serde_json::to_string(&report).unwrap();
    assert!(
        text.contains("Lactate [Moles/volume] in Blood")
            && text.contains("heparin")
            && text.contains("Discharge summary"),
        "{text}"
    );
    let found = medorama::patient::report(&c, &boss, f.patient, "chest").unwrap();
    assert_eq!(found.entries.len(), 1, "the note's text is searched");
    let note = found.entries[0].key.clone();
    assert!(note.starts_with("live:"), "{note}");
    let d = medorama::patient::detail(&c, &f.cfg, &boss, f.patient, &note).unwrap();
    assert_eq!(d.sources, vec!["omop"]);
    assert!(serde_json::to_string(&d).unwrap().contains("chest pain"));
    let sum = medorama::patient::summary(&c, &boss, f.patient).unwrap();
    assert_eq!(sum.visits.len(), 1, "OMOP's stay");
    assert_eq!((sum.visits[0].source.as_str(), sum.visits[0].unit.as_str()), ("omop", "Cardiology"));
    assert!(sum.problems.iter().any(|p| p.code == "I21.4"), "OMOP's condition, by its ICD-10 code");
    assert!(sum.unavailable.is_empty());
    let trends = medorama::patient::trends(&c, &boss, f.patient).unwrap();
    assert!(trends.iter().any(|t| t.name == "Heart rate" && t.category == "vit"));

    // Privacy Mode, over the connected data: no identifier, no real date
    let app = medorama::server::router(std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(f.cfg.clone()),
        std::sync::Arc::new(Db::open(&f.cfg, false).unwrap()),
    )));
    let api = "/api/v1";
    let (_, cookie) = get(
        &app,
        "",
        &format!("{api}/login"),
        Some(serde_json::json!({"username": "reader", "password": "0123456789ab"})),
    )
    .await;
    let cookie = cookie.unwrap();
    let p = f.patient;
    let mut seen = 0;
    for uri in [
        format!("{api}/patients/{p}"),
        format!("{api}/patients/{p}/journey"),
        format!("{api}/patients/{p}/report"),
        format!("{api}/patients/{p}/report?q=chest"),
        format!("{api}/patients/{p}/trends"),
        format!("{api}/patients/{p}/detail?key={note}"),
        format!("{api}/patients"),
    ] {
        let (body, _) = get(&app, &cookie, &uri, None).await;
        let t = String::from_utf8_lossy(&body).to_lowercase();
        for k in CANARIES {
            assert!(!t.contains(&k.to_lowercase()), "{uri} sends {k}: {}", &t[..t.len().min(2000)]);
        }
        let mut times = vec![];
        real_times(&serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null), &mut times);
        assert!(times.is_empty(), "{uri} sends real times {times:?}");
        seen += t.contains("lactate") as usize;
    }
    assert!(seen >= 2, "the connected data are there");
    let (body, _) = get(&app, &cookie, &format!("{api}/cohorts/search"), Some(serde_json::json!({"criteria": {"version": 1, "include": {"kind": "expression", "text": "omop.lactate > 4"}}}))).await;
    let t = String::from_utf8_lossy(&body);
    assert!(t.contains("MO-") && !CANARIES.iter().any(|k| t.contains(k)), "{t}");

    // disconnecting: the links go, and the patients only OMOP had
    let before: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    sources::remove(&mut c, &f.cfg, &admin, id).unwrap();
    let after: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    assert_eq!(before - after, 2, "102 and 103 were only in OMOP");
    let left: i64 = c.query_row("SELECT COUNT(*) FROM source_persons", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
    assert_eq!(search(&c, &admin, "lactate > 4").unwrap(), vec!["P900002"]);
    let j = serde_json::to_string(&medorama::patient::journey(&c, &boss, f.patient).unwrap()).unwrap();
    assert!(!j.contains("live:"));
    assert!(cohorts::parse_text(&c, &admin, "omop.lactate > 4").is_err());
}

#[tokio::test]
async fn omop_in_a_sqlite_file() {
    let f = catalog();
    let file = f._tmp.path().join("omop.sqlite");
    let o = rusqlite::Connection::open(&file).unwrap();
    o.execute_batch(&omop("TEXT", "TEXT", "REAL", "")).unwrap();
    drop(o);
    exercise(&f, &format!("sqlite:{}", file.display()), "").await;
}

#[tokio::test]
async fn omop_in_postgresql() {
    let Some(url) = std::env::var("MEDORAMA_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) else {
        return; // runs with the PostgreSQL tests
    };
    let f = catalog();
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).unwrap();
    let schema = format!("omop_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    let c = medorama::sql::Connection::postgres(&url, None, &Default::default()).unwrap();
    // a connection's URL holds no password: it goes where Medorama looks for it
    let url = match (url.find("://"), url.rfind('@')) {
        (Some(a), Some(at)) if url[a + 3..at].contains(':') => {
            let (user, pw) = url[a + 3..at].split_once(':').unwrap();
            std::env::set_var(sources::password_var("omop"), pw);
            format!("{}{user}{}", &url[..a + 3], &url[at..])
        }
        _ => url,
    };
    c.execute_batch_native(&format!(
        "CREATE SCHEMA {schema}; {}",
        omop("TIMESTAMP", "DATE", "NUMERIC", &format!("{schema}."))
    ))
    .unwrap();
    exercise(&f, &url, &schema).await;
    c.execute_batch_native(&format!("DROP SCHEMA {schema} CASCADE")).unwrap();
}

/// Connecting from the page (admins): checked, listed, its persons linked, disconnected; a
/// password given there kept in a file only the catalog reads (0600), never in an answer, gone
/// with the connection; everyone else refused.
#[tokio::test]
async fn connections_from_the_page() {
    let f = catalog();
    let file = f._tmp.path().join("omop.sqlite");
    let o = rusqlite::Connection::open(&file).unwrap();
    o.execute_batch(&omop("TEXT", "TEXT", "REAL", "")).unwrap();
    drop(o);
    let app = medorama::server::router(std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(f.cfg.clone()),
        std::sync::Arc::new(Db::open(&f.cfg, false).unwrap()),
    )));
    let login = |u: &str| serde_json::json!({"username": u, "password": "0123456789ab"});
    let boss = get(&app, "", "/api/v1/login", Some(login("boss"))).await.1.unwrap();
    let reader = get(&app, "", "/api/v1/login", Some(login("reader"))).await.1.unwrap();
    let send = |method: &'static str, uri: String, cookie: String, body: Option<serde_json::Value>| {
        let app = app.clone();
        async move {
            let mut b = Request::builder().method(method).uri(uri).header("X-Medorama", "1").header("Cookie", cookie);
            if body.is_some() {
                b = b.header("Content-Type", "application/json");
            }
            let r =
                app.oneshot(b.body(body.map_or(Body::empty(), |v| Body::from(v.to_string()))).unwrap()).await.unwrap();
            let status = r.status();
            (status, r.into_body().collect().await.unwrap().to_bytes().to_vec())
        }
    };
    let new = serde_json::json!({"name": "omop", "kind": "omop", "url": format!("sqlite:{}", file.display()),
        "password": "s3cret-pass-421", "settings": {"identifier": "mrn"}});
    let api = "/api/v1";
    assert_eq!(send("POST", format!("{api}/connections"), reader.clone(), Some(new.clone())).await.0, 403);
    let (status, body) = send("POST", format!("{api}/connections"), boss.clone(), Some(new)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let r: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = r["id"].as_i64().unwrap();
    assert!(r["views"].as_array().is_some_and(|v| !v.is_empty()), "{r}");
    // the password: in a private file, in no answer
    let secret = f.cfg.home.join("secrets/omop.password");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "s3cret-pass-421");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(secret.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let (_, list) = send("GET", format!("{api}/connections"), boss.clone(), None).await;
    assert!(!String::from_utf8_lossy(&list).contains("s3cret"), "the password is never sent back");
    assert_eq!(send("GET", format!("{api}/connections"), reader.clone(), None).await.0, 403);
    // its persons linked
    assert_eq!(send("POST", format!("{api}/connections/{id}/sync"), reader.clone(), None).await.0, 403);
    let (status, body) = send("POST", format!("{api}/connections/{id}/sync"), boss.clone(), None).await;
    assert_eq!(status, 200);
    let s: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(s["linked"].as_i64().unwrap() >= 1, "{s}");
    // the catalog's own database is not a database to connect
    if let Some(own) = f.cfg.database.to_str().filter(|_| !common::on_postgres()) {
        let again = serde_json::json!({"name": "self", "kind": "omop", "url": format!("sqlite:{own}")});
        let (status, body) = send("POST", format!("{api}/connections"), boss.clone(), Some(again)).await;
        assert_eq!(status, 400);
        assert!(String::from_utf8_lossy(&body).contains("catalog's own database"));
    }
    // disconnected: the connection and its password file go
    assert_eq!(send("DELETE", format!("{api}/connections/{id}"), reader.clone(), None).await.0, 403);
    assert_eq!(send("DELETE", format!("{api}/connections/{id}"), boss.clone(), None).await.0, 200);
    assert!(!secret.exists());
    let (_, list) = send("GET", format!("{api}/connections"), boss, None).await;
    assert!(!String::from_utf8_lossy(&list).contains("\"omop\""));
}

/// Cohorts across the catalog and a connected database do not depend on the order of their
/// conditions: the first search on a fresh catalog finds a person only the connected database
/// has, whichever condition comes first.
#[tokio::test]
async fn first_search_finds_a_connected_person_in_any_order() {
    for text in [
        "female AND omop.\"Troponin T.cardiac [Mass/volume] in Serum or Plasma\" > 100",
        "omop.\"Troponin T.cardiac [Mass/volume] in Serum or Plasma\" > 100 AND female",
    ] {
        let f = catalog();
        let file = f._tmp.path().join("omop.sqlite");
        let o = rusqlite::Connection::open(&file).unwrap();
        o.execute_batch(&omop("TEXT", "TEXT", "REAL", "")).unwrap();
        drop(o);
        let c = f.db.get().unwrap();
        let admin = User::local();
        let n = NewSource {
            name: "omop".into(),
            kind: "omop".into(),
            url: format!("sqlite:{}", file.display()),
            settings: Settings { timezone: "UTC".into(), ..Default::default() },
            ..Default::default()
        };
        sources::add(&c, &admin, &n).unwrap();
        let found = search(&c, &admin, text).unwrap();
        assert_eq!(found.len(), 1, "{text}: {found:?}");
        assert!(found[0].starts_with("GEN-"), "{text}: {found:?}");
    }
}

/// `latest` is each patient's latest value across the catalog and the connected databases, not
/// each database's latest.
#[tokio::test]
async fn latest_across_databases() {
    let f = catalog();
    let file = f._tmp.path().join("omop.sqlite");
    let o = rusqlite::Connection::open(&file).unwrap();
    o.execute_batch(&omop("TEXT", "TEXT", "REAL", "")).unwrap();
    drop(o);
    let mut c = f.db.get().unwrap();
    let admin = User::local();
    // P900001 (OMOP's person 101: lactate 4.6 on 2024-01-02) has a later, normal one in the catalog
    let labs = "MRN,Test Name,Result,Units,Collected\nSYN7000001,Lactate [Moles/volume] in Blood,1.0,mmol/L,2024-01-05 10:00\n";
    medorama::tables::import(
        &mut c,
        &f.cfg,
        &admin,
        "late.csv",
        labs.as_bytes(),
        &HashMap::new(),
        "lab",
        None,
        &Opts::new("late", "", false),
    )
    .unwrap();
    let n = NewSource {
        name: "omop".into(),
        kind: "omop".into(),
        url: format!("sqlite:{}", file.display()),
        settings: Settings { timezone: "UTC".into(), ..Default::default() },
        ..Default::default()
    };
    sources::add(&c, &admin, &n).unwrap();
    let k = |latest: bool| cohorts::Criterion {
        kind: "measurement".into(),
        name: "Lactate [Moles/volume] in Blood".into(),
        cmp: ">".into(),
        value: Some(4.0),
        unit: "mmol/L".into(),
        latest,
        ..Default::default()
    };
    let found = |latest: bool| -> Vec<String> {
        let q = Criteria { include: Some(cohorts::Expr::Leaf(k(latest))), ..Default::default() };
        let (_, rows, _) = cohorts::search(&c, &admin, "UTC", &q, None, 100, 0).unwrap();
        rows.into_iter().map(|m| m.patient.patient_id).collect()
    };
    assert_eq!(found(false), vec!["P900001"], "a value above 4, ever (in OMOP)");
    assert!(found(true).is_empty(), "their latest (in the catalog) is 1.0");
}

/// A password given for a connection is kept only once the connection works, and never over
/// another connection's: a name already used leaves the first one's password file as it was.
/// Removing the connection's import removes the connection, its pools and its password file.
/// Settings that cannot be right (a time zone, a schema's name) are refused.
#[test]
fn a_connections_password_and_settings() {
    let f = catalog();
    let file = f._tmp.path().join("omop.sqlite");
    let o = rusqlite::Connection::open(&file).unwrap();
    o.execute_batch(&omop("TEXT", "TEXT", "REAL", "")).unwrap();
    drop(o);
    let c = f.db.get().unwrap();
    let admin = User::local();
    let n = NewSource {
        name: "omop".into(),
        kind: "omop".into(),
        url: format!("sqlite:{}", file.display()),
        settings: Settings { timezone: "UTC".into(), ..Default::default() },
        ..Default::default()
    };
    let id = sources::add_with_password(&c, &f.cfg, &admin, &n, "first-secret").unwrap();
    let secret = sources::secret_file(&f.cfg, "omop");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "first-secret");
    let e = sources::add_with_password(&c, &f.cfg, &admin, &n, "second-secret").unwrap_err();
    assert!(e.to_string().contains("exists"), "{e}");
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "first-secret", "the first connection's password stays");
    // a connection that does not answer: no password kept
    let bad = NewSource { name: "other".into(), url: "sqlite:/no/such/file.sqlite".into(), ..n.clone() };
    assert!(sources::add_with_password(&c, &f.cfg, &admin, &bad, "x").is_err());
    assert!(!sources::secret_file(&f.cfg, "other").exists());
    assert_eq!(std::fs::read_dir(secret.parent().unwrap()).unwrap().count(), 1, "no file left over");
    // settings that cannot be right
    let zone = NewSource {
        name: "zoned".into(),
        settings: Settings { timezone: "Europe/Pariss".into(), ..Default::default() },
        ..n.clone()
    };
    assert!(sources::add(&c, &admin, &zone).unwrap_err().to_string().contains("not a time zone"));
    let schema = NewSource {
        name: "schemed".into(),
        settings: Settings { schema: "cdm\"; --".into(), ..Default::default() },
        ..n.clone()
    };
    assert!(sources::add(&c, &admin, &schema).unwrap_err().to_string().contains("letters, digits"));
    let pw = NewSource { name: "pw".into(), url: "postgres://u@db.example/x?password=p".into(), ..n.clone() };
    assert!(sources::add(&c, &admin, &pw).unwrap_err().to_string().contains("password out of the URL"));
    // a connection's import removed: the connection and its password go with it
    let s = sources::get(&c, id).unwrap().unwrap();
    drop(c);
    let mut c = f.db.get().unwrap();
    medorama::imports::remove_import(&mut c, &f.cfg, &admin, s.import_id, false, false).unwrap();
    assert!(sources::get(&c, id).unwrap().is_none());
    assert!(!secret.exists());
    // a SQLite file named by a command: inside data_roots, never the catalog's own database
    let mut cfg = f.cfg.clone();
    cfg.data_roots = vec![f._tmp.path().join("elsewhere").to_string_lossy().to_string()];
    assert!(sources::checked_url(&cfg, &format!("sqlite:{}", file.display()))
        .unwrap_err()
        .to_string()
        .contains("data_roots"));
    if !common::on_postgres() {
        let own = format!("sqlite:{}", f.cfg.database.display());
        assert!(sources::checked_url(&f.cfg, &own).unwrap_err().to_string().contains("catalog's own"));
    }
}

/// A connected PostgreSQL database's views are its own SQL, run as written (not translated
/// from SQLite's dialect): `?` operators, `E''` strings, dollar quotes, comments, `LIKE`; a
/// boolean `icu`; a `timestamp` (a local time) read in the connection's time zone.
#[test]
fn postgresql_views_run_as_written() {
    let Some(url) = std::env::var("MEDORAMA_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) else {
        return; // runs with the PostgreSQL tests
    };
    let f = catalog();
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).unwrap();
    let schema = format!("views_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    let pg = medorama::sql::Connection::postgres(&url, None, &Default::default()).unwrap();
    pg.execute_batch_native(&format!(
        "CREATE SCHEMA {schema};
         CREATE TABLE {schema}.p (id BIGINT, doc JSONB, born DATE);
         INSERT INTO {schema}.p VALUES (1, '{{\"mrn\": \"SYN7000001\"}}', '1950-03-04'), (2, '{{}}', NULL);
         CREATE TABLE {schema}.v (id BIGINT, person BIGINT, start_at TIMESTAMP, intensive BOOLEAN);
         INSERT INTO {schema}.v VALUES (10, 1, '2024-01-02 03:00:00', true), (11, 2, '2024-01-03 03:00:00', false);"
    ))
    .unwrap();
    let url = match (url.find("://"), url.rfind('@')) {
        (Some(a), Some(at)) if url[a + 3..at].contains(':') => {
            let (user, pw) = url[a + 3..at].split_once(':').unwrap();
            std::env::set_var(sources::password_var("pgviews"), pw);
            format!("{}{user}{}", &url[..a + 3], &url[at..])
        }
        _ => url,
    };
    let persons = format!(
        "SELECT id AS person_key, NULL AS patient_id, doc->>'mrn' AS mrn, '' AS mrn_authority, '' AS last_name,
                E'it\\'s' AS first_name, CAST(born AS TEXT) AS birth_date, $$F$$ AS sex /* ? LIKE */
         FROM {schema}.p WHERE doc ? 'mrn' OR id = 2"
    );
    let visits = format!(
        "SELECT id AS visit_key, person AS person_key, start_at AS start_time, NULL AS end_time, 'ICU' AS unit,
                intensive AS icu FROM {schema}.v"
    );
    let entries = format!(
        "SELECT 'e' || id AS entry_key, person AS person_key, id AS visit_key, 'lab' AS category, start_at AS time,
                NULL AS end_time, 'Lactate' AS name, '' AS code, '' AS vocabulary, 4.5 AS value_num, '' AS value_text,
                'mmol/L' AS unit, NULL AS ref_low, NULL AS ref_high, '' AS flag, '' AS author, '' AS text FROM {schema}.v"
    );
    let n = NewSource {
        name: "pgviews".into(),
        kind: "sql".into(),
        url,
        settings: Settings { timezone: "America/New_York".into(), persons, visits, entries, ..Default::default() },
        ..Default::default()
    };
    let c = f.db.get().unwrap();
    let id = sources::add(&c, &User::local(), &n).unwrap();
    let s = sources::get(&c, id).unwrap().unwrap();
    let p = s.persons(&["1".into(), "2".into()]).unwrap();
    assert_eq!(p.len(), 2);
    assert!(p.iter().any(|p| p.mrn == "SYN7000001" && p.first_name == "it's" && p.sex == "F"));
    assert_eq!(s.visit_persons(true).unwrap(), vec!["1".to_string()], "a boolean icu");
    let zone = s.zone("UTC");
    let v = s.visits(&["1".into()], zone).unwrap();
    // 03:00 in New York, not in UTC
    assert_eq!(v[0].start, Some(1_704_182_400.0));
    let e = s.find(&sources::Find { name_contains: Some("lact".into()), ..Default::default() }, zone).unwrap();
    assert_eq!(e.len(), 2);
    pg.execute_batch_native(&format!("DROP SCHEMA {schema} CASCADE")).unwrap();
}
