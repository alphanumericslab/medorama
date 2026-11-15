//! Clinical reading of connected databases: OMOP's concept 0 and operators, partial dates of
//! birth, Unix-second times, intensive-care stays (OMOP's `visit_detail`, db-builder's bed stays,
//! a view's `stays`), `has icu` and diagnosis codes.  Every row is invented.

mod common;

use common::{Dbb, Setup};
use medorama::sources::{Find, Settings, Source};

fn source(name: &str, kind: &str, path: &std::path::Path, settings: Settings) -> Source {
    Source {
        id: 900 + name.len() as i64,
        name: name.into(),
        kind: kind.into(),
        url: format!("sqlite:{}", path.display()),
        password_file: String::new(),
        password_wrapped: String::new(),
        ca_file: String::new(),
        settings,
        created_at: 0.0,
    }
}

/// A small OMOP CDM in SQLite, with concept 0 where OMOP's tools put it.
fn omop(path: &std::path::Path) {
    let o = rusqlite::Connection::open(path).unwrap();
    o.execute_batch(
        "CREATE TABLE person (person_id INTEGER, gender_concept_id INTEGER, year_of_birth INTEGER, month_of_birth INTEGER,
             day_of_birth INTEGER, birth_datetime TEXT, person_source_value TEXT);
         CREATE TABLE care_site (care_site_id INTEGER, care_site_name TEXT);
         CREATE TABLE visit_occurrence (visit_occurrence_id INTEGER, person_id INTEGER, visit_concept_id INTEGER, visit_start_date TEXT,
             visit_start_datetime TEXT, visit_end_date TEXT, visit_end_datetime TEXT, care_site_id INTEGER);
         CREATE TABLE visit_detail (visit_detail_id INTEGER, person_id INTEGER, visit_detail_concept_id INTEGER, visit_occurrence_id INTEGER,
             visit_detail_start_date TEXT, visit_detail_start_datetime TEXT, visit_detail_end_date TEXT, visit_detail_end_datetime TEXT,
             care_site_id INTEGER);
         CREATE TABLE measurement (measurement_id INTEGER, person_id INTEGER, measurement_concept_id INTEGER, measurement_date,
             measurement_datetime, value_as_number REAL, value_as_concept_id INTEGER, unit_concept_id INTEGER, range_low REAL,
             range_high REAL, visit_occurrence_id INTEGER, measurement_source_value TEXT, unit_source_value TEXT, value_source_value TEXT,
             operator_concept_id INTEGER);
         CREATE TABLE observation (observation_id INTEGER, person_id INTEGER, observation_concept_id INTEGER, observation_date TEXT,
             observation_datetime TEXT, value_as_number REAL, value_as_string TEXT, value_as_concept_id INTEGER, unit_concept_id INTEGER,
             visit_occurrence_id INTEGER, observation_source_value TEXT, unit_source_value TEXT);
         CREATE TABLE condition_occurrence (condition_occurrence_id INTEGER, person_id INTEGER, condition_concept_id INTEGER,
             condition_start_date TEXT, condition_start_datetime TEXT, condition_end_date TEXT, condition_end_datetime TEXT,
             visit_occurrence_id INTEGER, condition_source_value TEXT, condition_source_concept_id INTEGER);
         CREATE TABLE drug_exposure (drug_exposure_id INTEGER, person_id INTEGER, drug_concept_id INTEGER, drug_exposure_start_date TEXT,
             drug_exposure_start_datetime TEXT, drug_exposure_end_date TEXT, drug_exposure_end_datetime TEXT, visit_occurrence_id INTEGER,
             drug_source_value TEXT, dose_unit_source_value TEXT);
         CREATE TABLE procedure_occurrence (procedure_occurrence_id INTEGER, person_id INTEGER, procedure_concept_id INTEGER,
             procedure_date TEXT, procedure_datetime TEXT, visit_occurrence_id INTEGER, procedure_source_value TEXT);
         CREATE TABLE note (note_id INTEGER, person_id INTEGER, note_date TEXT, note_datetime TEXT, note_type_concept_id INTEGER,
             note_title TEXT, note_text TEXT, provider_id INTEGER, visit_occurrence_id INTEGER);
         CREATE TABLE provider (provider_id INTEGER, provider_name TEXT);
         CREATE TABLE concept (concept_id INTEGER, concept_name TEXT, vocabulary_id TEXT, concept_code TEXT);
         CREATE TABLE cdm_source (cdm_source_name TEXT, source_release_date TEXT, cdm_release_date TEXT);
         INSERT INTO concept VALUES (0, 'No matching concept', 'None', 'No matching concept'),
             (3047181, 'Lactate [Moles/volume] in Blood', 'LOINC', '2524-7'), (8753, 'millimole per liter', 'UCUM', 'mmol/L'),
             (4171756, '<', 'Meas Value Operator', '<'), (4172703, '=', 'Meas Value Operator', '='),
             (32037, 'Intensive Care', 'Visit', 'OMOP4822460'), (9201, 'Inpatient Visit', 'Visit', 'IP'),
             (45533176, 'Sepsis, unspecified organism', 'ICD10CM', 'A41.9'), (132797, 'Sepsis', 'SNOMED', '91302008');
         -- a year of birth alone, a year and month, a whole date
         INSERT INTO person VALUES (1, 8532, 1950, NULL, NULL, NULL, 'X1'), (2, 8507, 1960, 3, NULL, NULL, 'X2'),
             (3, 8507, 1970, 5, 6, NULL, 'X3');
         INSERT INTO care_site VALUES (1, 'Medicine 4'), (2, 'MICU');
         INSERT INTO visit_occurrence VALUES (1, 1, 9201, '2024-03-01', '2024-03-01 08:00:00', '2024-03-09', '2024-03-09 13:00:00', 1),
             (2, 2, 0, '2024-04-01', '2024-04-01 08:00:00', '2024-04-05', '2024-04-05 08:00:00', 1),
             (3, 3, 9201, '2024-05-01', '2024-05-01 08:00:00', '2024-05-02', '2024-05-02 08:00:00', 1);
         -- person 1: two days in intensive care (by its concept), then a ward; person 2: a stay in
         -- the MICU (by its unit's name, concept 0)
         INSERT INTO visit_detail VALUES (10, 1, 32037, 1, '2024-03-02', '2024-03-02 11:00:00', '2024-03-04', '2024-03-04 11:00:00', 1),
             (11, 1, 0, 1, '2024-03-04', '2024-03-04 11:00:00', '2024-03-09', '2024-03-09 13:00:00', 1),
             (20, 2, 0, 2, '2024-04-02', '2024-04-02 09:00:00', '2024-04-03', '2024-04-03 09:00:00', 2);
         INSERT INTO measurement VALUES
             (1, 1, 0, '2024-03-02', '2024-03-02 06:10:00', 4.1, 0, 0, 0.5, 2, 1, 'Serum lactate', 'mmol/L', '4.1', 0),
             (2, 1, 3047181, '2024-03-03', '2024-03-03 06:10:00', 5, 0, 8753, NULL, NULL, 1, 'LACT', 'mmol/L', '<5', 4171756),
             (3, 1, 3047181, '2024-03-04', '2024-03-04 06:10:00', 2.2, 0, 8753, NULL, NULL, 1, 'LACT', 'mmol/L', '2.2', 4172703),
             (4, 1, 3047181, '2024-03-05', 1709380800, 1.5, NULL, 8753, NULL, NULL, 1, 'LACT', 'mmol/L', '1.5', NULL),
             (5, 1, 3047181, 1709337600, NULL, 1.4, NULL, 8753, NULL, NULL, 1, 'LACT', 'mmol/L', '1.4', NULL);
         INSERT INTO observation VALUES (1, 1, 0, '2024-03-02', '2024-03-02 07:00:00', 3, NULL, 0, 0, 1, 'Pain score', '');
         INSERT INTO condition_occurrence VALUES (1, 1, 0, '2024-03-01', NULL, NULL, NULL, 1, 'I21.4', 0),
             (2, 2, 132797, '2024-04-01', NULL, NULL, NULL, 2, 'A41.9', 45533176);
         INSERT INTO drug_exposure VALUES (1, 1, 0, '2024-03-01', NULL, '2024-03-03', NULL, 1, 'heparin', 'U');
         INSERT INTO procedure_occurrence VALUES (1, 1, 0, '2024-03-01', NULL, 1, 'Central line');
         INSERT INTO note VALUES (1, 1, '2024-03-09', NULL, 0, '', 'Invented text.', NULL, 1);
         INSERT INTO cdm_source VALUES ('Synthetic', '2024-01-01', '2024-01-02');",
    )
    .unwrap();
}

#[test]
fn omop_concept_zero_operators_partial_births_and_unix_times() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("omop.sqlite");
    omop(&path);
    let s = source("omop", "omop", &path, Settings { timezone: "America/New_York".into(), ..Default::default() });
    s.check().unwrap();
    let zone = s.zone("UTC");
    let keys: Vec<String> = ["1", "2", "3"].iter().map(|k| k.to_string()).collect();
    let e = s.entries(&keys, zone).unwrap();
    let by = |k: &str| e.iter().find(|x| x.key == k).unwrap_or_else(|| panic!("no {k}"));
    // concept 0 names nothing: the source values are read
    let m1 = by("m1");
    assert_eq!(
        (m1.name.as_str(), m1.code.as_str(), m1.unit.as_str(), m1.value_num),
        ("Serum lactate", "", "mmol/L", Some(4.1))
    );
    assert_eq!(
        (by("o1").name.as_str(), by("d1").name.as_str(), by("p1").name.as_str()),
        ("Pain score", "heparin", "Central line")
    );
    assert_eq!((by("c1").name.as_str(), by("c1").code.as_str()), ("I21.4", "I21.4"));
    assert_eq!(by("n1").name, "Note");
    assert!(e.iter().all(|x| !x.name.contains("No matching") && !x.unit.contains("No matching")));
    // an operator other than = makes the value text: <5, not 5
    let (m2, m3) = (by("m2"), by("m3"));
    assert_eq!((m2.value_num, m2.value_text.as_str(), m2.unit.as_str()), (None, "<5", "mmol/L"));
    assert_eq!((m3.value_num, m3.value_text.as_str()), (Some(2.2), ""));
    // Unix seconds: a date and time is that moment (12:00 UTC), a date its local midnight
    assert_eq!(by("m4").time, Some(1_709_380_800.0));
    let ny = medorama::util::tz("America/New_York");
    assert_eq!(by("m5").time, medorama::util::parse_time("2024-03-02", ny).unwrap());
    // local text in the database's time zone
    assert_eq!(by("m1").time, medorama::util::parse_time("2024-03-02 06:10", ny).unwrap());
    // dates of birth: a year alone is its 1 July, a month its 15th
    let mut p = s.persons(&keys).unwrap();
    p.sort_by(|a, b| a.key.cmp(&b.key));
    let births: Vec<&str> = p.iter().map(|p| p.birth_date.as_str()).collect();
    assert_eq!(births, ["1950-07-01", "1960-03-15", "1970-05-06"]);
    // the stays: visit_detail, intensive care by its concept or by its unit's name
    let mut st = s.stays(&keys, zone).unwrap();
    st.sort_by(|a, b| a.key.cmp(&b.key));
    let icu: Vec<(&str, &str, bool)> = st.iter().map(|x| (x.key.as_str(), x.unit.as_str(), x.icu)).collect();
    assert_eq!(icu, [("10", "Medicine 4", true), ("11", "Medicine 4", false), ("20", "MICU", true)]);
    assert_eq!(st[0].start, medorama::util::parse_time("2024-03-02 11:00", ny).unwrap());
    // has icu: person 1 by the visit's detail, person 2 by a stay in the MICU; not person 3
    let mut with_icu = s.visit_persons(true).unwrap();
    with_icu.sort();
    assert_eq!(with_icu, ["1", "2"]);
    assert_eq!(s.visit_persons(false).unwrap().len(), 3);
}

#[test]
fn diagnosis_codes_match_without_their_dots() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("omop.sqlite");
    omop(&path);
    let s = source("omop2", "omop", &path, Settings { timezone: "UTC".into(), ..Default::default() });
    let zone = s.zone("UTC");
    let find = |t: &str| -> Vec<String> {
        let f = Find { categories: vec!["dx".into()], code_or_name: Some(t.into()), ..Default::default() };
        let mut k: Vec<String> = s.find(&f, zone).unwrap().into_iter().map(|e| e.key).collect();
        k.sort();
        k
    };
    // I21.4 is found as I214, i21.4 or I21; A41.9 as A419
    for t in ["I21.4", "I214", "i21.4", "I21"] {
        assert_eq!(find(t), ["c1"], "{t}");
    }
    assert_eq!((find("A41.9"), find("a419")), (vec!["c2".to_string()], vec!["c2".to_string()]));
    // a word is searched in names too; a code is not (I21 is in no name, but 4 is in "I21.4")
    assert_eq!(find("sepsis"), ["c2"]);
    assert!(find("4").is_empty() && find("21.4").is_empty());
    assert!(medorama::sources::names_a_word("heart failure") && medorama::sources::names_a_word("sepsis"));
    for code in ["I21.9", "E11", "410.1", "V45.81", "ab", "12"] {
        assert!(!medorama::sources::names_a_word(code), "{code}");
    }
}

#[test]
fn has_icu_counts_a_unit_named_so_and_a_views_stays() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("dw.sqlite");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch(
        "CREATE TABLE pt (id TEXT); CREATE TABLE adm (id TEXT, pt TEXT, ward TEXT, icu INTEGER);
         CREATE TABLE beds (id TEXT, adm TEXT, pt TEXT, unit TEXT, a TEXT, b TEXT);
         INSERT INTO pt VALUES ('a'), ('b'), ('c'), ('d');
         -- a: its unit is an ICU, though icu says 0; b: flagged; c: a ward, a bed in the CVICU; d: a
         -- step-down unit, which is no ICU
         INSERT INTO adm VALUES ('1', 'a', 'MICU', 0), ('2', 'b', 'Medicine', 1), ('3', 'c', 'Cardiology', 0),
             ('4', 'd', 'ICU step-down', 0);
         INSERT INTO beds VALUES ('s1', '3', 'c', 'CVICU', '2024-01-02 10:00', '2024-01-03 10:00');",
    )
    .unwrap();
    drop(c);
    let views = Settings {
        timezone: "UTC".into(),
        persons: "SELECT id AS person_key, '' AS patient_id, '' AS patient_id_issuer, '' AS mrn, '' AS mrn_authority,
                  '' AS last_name, '' AS first_name, '' AS birth_date, '' AS sex FROM pt"
            .into(),
        visits: "SELECT id AS visit_key, pt AS person_key, NULL AS start_time, NULL AS end_time, ward AS unit, icu FROM adm".into(),
        entries: "SELECT '' AS entry_key, '' AS person_key, '' AS visit_key, '' AS category, NULL AS time, NULL AS end_time,
                  '' AS name, '' AS code, '' AS vocabulary, NULL AS value_num, '' AS value_text, '' AS unit, NULL AS ref_low,
                  NULL AS ref_high, '' AS flag, '' AS author, '' AS text WHERE 1 = 0"
            .into(),
        ..Default::default()
    };
    let s = source("dw", "views", &path, views.clone());
    let mut k = s.visit_persons(true).unwrap();
    k.sort();
    assert_eq!(k, ["a", "b"], "a unit named like an ICU counts, as on the patient's page");
    // with the optional stays view, a stay in the CVICU counts too
    let stays = "SELECT id AS stay_key, pt AS person_key, adm AS visit_key, a AS start_time, b AS end_time, unit, 0 AS icu FROM beds";
    let s = source("dw2", "views", &path, Settings { stays: stays.into(), ..views });
    assert!(s.check().unwrap().views.contains(&"stays".to_string()));
    let mut k = s.visit_persons(true).unwrap();
    k.sort();
    assert_eq!(k, ["a", "b", "c"]);
    let st = s.stays(&["c".into()], s.zone("UTC")).unwrap();
    assert_eq!((st.len(), st[0].icu, st[0].visit.as_str()), (1, true, "3"));
}

#[test]
fn the_icu_column_draws_the_bed_stay_not_the_whole_visit() {
    let t = Setup::new();
    let d = Dbb::new(&t.data().join("beds.sqlite"));
    d.person("p1", "S1", "STUDY", "", "", "", "", "1950-03-04", "F")
        .visit("E1", "p1", "2024-01-01 08:00", "2024-01-09 13:00", "Cardiology", true)
        .entry("e1", "p1", "E1", "vit", "2024-01-02 23:30", "Heart rate", Some(80.0), "/min");
    // the bed stays of the visit, as db-builder keeps them (db-builder/docs/schema.md, extras)
    d.exec(
        "CREATE TABLE extras (table_name TEXT, row_key TEXT, name TEXT, value TEXT, source_id INTEGER);
         INSERT INTO extras VALUES ('bed_stays', '1:1', 'visit_key', 'E1', 1), ('bed_stays', '1:1', 'care_unit', 'ICU7', 1),
             ('bed_stays', '1:1', 'bed_start', '2024-01-01T11:00:00-05:00', 1),
             ('bed_stays', '1:1', 'bed_end', '2024-01-03T11:00:00-05:00', 1),
             ('bed_stays', '1:2', 'visit_key', 'E1', 1), ('bed_stays', '1:2', 'care_unit', 'Cardiology', 1),
             ('bed_stays', '1:2', 'bed_start', '2024-01-03T11:00:00-05:00', 1),
             ('bed_stays', '1:2', 'bed_end', '2024-01-09T13:00:00-05:00', 1);",
    );
    let db = t.connect("beds", &d);
    let c = t.db.get().unwrap();
    let boss = t.user("boss");
    let s = medorama::sources::get(&c, db).unwrap().unwrap();
    let patient = medorama::persons::patients(&t.cfg, &c, &s, &["p1".into()], None).unwrap()["p1"];
    let j = medorama::patient::journey(&c, &t.cfg, &boss, patient).unwrap();
    let icu = j.lanes.iter().position(|l| l.key == "icu").expect("an ICU column");
    let spans: Vec<(f64, f64)> = j.spans.iter().filter(|x| x.lane == icu).map(|x| (x.t0, x.t1)).collect();
    let utc = medorama::util::tz("UTC");
    let at = |x: &str| medorama::util::parse_time(x, utc).unwrap().unwrap();
    assert_eq!(spans, [(at("2024-01-01T16:00:00Z"), at("2024-01-03T16:00:00Z"))], "the ICU stay, not the nine days");
    // a database without bed stays: the whole intensive-care visit
    let d2 = Dbb::new(&t.data().join("nobeds.sqlite"));
    d2.person("q1", "S2", "STUDY", "", "", "", "", "1951-03-04", "M").visit(
        "E2",
        "q1",
        "2024-02-01 08:00",
        "2024-02-03 08:00",
        "MICU",
        true,
    );
    let db2 = t.connect("nobeds", &d2);
    let s2 = medorama::sources::get(&c, db2).unwrap().unwrap();
    let q = medorama::persons::patients(&t.cfg, &c, &s2, &["q1".into()], None).unwrap()["q1"];
    let j = medorama::patient::journey(&c, &t.cfg, &boss, q).unwrap();
    let icu = j.lanes.iter().position(|l| l.key == "icu").unwrap();
    let zone = medorama::util::tz(&t.cfg.timezone);
    let local = |x: &str| medorama::util::parse_time(x, zone).unwrap().unwrap();
    let spans: Vec<(f64, f64)> = j.spans.iter().filter(|x| x.lane == icu).map(|x| (x.t0, x.t1)).collect();
    assert_eq!(spans, [(local("2024-02-01 08:00"), local("2024-02-03 08:00"))]);
}
