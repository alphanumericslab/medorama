//! Cohorts (docs/user-guide.md, "Cohorts"): exports of large cohorts, units and `latest` in
//! criteria, criteria that keep their meaning, refreshes that do not depend on who runs them,
//! the changes a refresh applies; and the units of a patient's trends.

mod common;
use medorama::cohorts::{self, Criteria, Criterion, Expr, NewCohort};
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::users::User;
use std::collections::HashMap;

struct Fx {
    _tmp: tempfile::TempDir,
    cfg: medorama::config::Config,
    db: Db,
}

fn catalog() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    Fx { _tmp: tmp, cfg, db }
}

fn import(f: &Fx, name: &str, text: &str) {
    let mut c = f.db.get().unwrap();
    medorama::tables::import(
        &mut c,
        &f.cfg,
        &User::local(),
        name,
        text.as_bytes(),
        &HashMap::new(),
        "",
        None,
        &Opts::new(name, "", false),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
}

fn leaf(k: Criterion) -> Criteria {
    Criteria { include: Some(Expr::Leaf(k)), ..Default::default() }
}

#[test]
fn exports_hold_every_member() {
    let f = catalog();
    let mut csv = String::from("Patient ID,Sex\n");
    for i in 0..1205 {
        csv += &format!("P{i:05},F\n");
    }
    import(&f, "patients.csv", &csv);
    let c = f.db.get().unwrap();
    let user = User::local();
    let k = Criterion { kind: "sex".into(), text: "F".into(), ..Default::default() };
    let id =
        cohorts::create(&c, &user, "UTC", &NewCohort { name: "all".into(), criteria: leaf(k), ..Default::default() })
            .unwrap();
    let (_, text) = cohorts::export_csv(&c, &user, id, None).unwrap();
    assert_eq!(text.lines().count(), 1 + 1205, "a line per member, and the header");
    let (_, xlsx) = cohorts::export_xlsx(&c, &user, id, None).unwrap();
    assert!(!xlsx.is_empty());
}

fn measurement(name: &str, cmp: &str, value: f64, unit: &str, latest: bool) -> Criteria {
    leaf(Criterion {
        kind: "measurement".into(),
        name: name.into(),
        cmp: cmp.into(),
        value: Some(value),
        unit: unit.into(),
        latest,
        ..Default::default()
    })
}

fn ids(found: &medorama::cohorts::Found) -> Vec<String> {
    let mut v: Vec<String> = found.items.iter().map(|m| m.patient.patient_id.clone()).collect();
    v.sort();
    v
}

#[test]
fn units_that_do_not_compare_are_left_out_and_said() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\nP2,M\nP3,F\n");
    import(
        &f,
        "labs.csv",
        "Patient ID,Test Name,Result,Units,Collected\nP1,Glucose,100,mg/dL,2024-01-02 06:00\nP2,Glucose,8,mmol/L,2024-01-02 06:00\n\
P3,Glucose,9,,2024-01-02 06:00\n",
    );
    let c = f.db.get().unwrap();
    let u = User::local();
    let search = |q: &Criteria| cohorts::search_page(&c, &u, "UTC", q, None, 100, 0, 50);
    // in mmol/L: the value in mmol/L tested, the others left out, and said
    let r = search(&measurement("Glucose", ">", 7.0, "mmol/L", false)).unwrap();
    assert_eq!(ids(&r), ["P2"], "100 mg/dL is not more than 7 mmol/L by its number");
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(r.warnings[0].contains("mg/dL") && r.warnings[0].contains("no unit"), "{:?}", r.warnings);
    // without a unit, over data in several: which one is meant must be written
    let e = search(&measurement("Glucose", ">", 7.0, "", false)).unwrap_err().to_string();
    assert!(e.contains("write the unit"), "{e}");
    // no value compares: an error, as before
    let e = search(&measurement("Glucose", ">", 7.0, "ms", false)).unwrap_err().to_string();
    assert!(e.contains("does not compare"), "{e}");
}

#[test]
fn latest_is_the_latest_of_the_name_written() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\nP2,M\n");
    // P1's latest QT is short; their QTc, later, is long
    import(
        &f,
        "ecg.csv",
        "Patient ID,Test Name,Result,Units,Collected\nP1,QT,480,ms,2024-01-01 06:00\nP1,QT,400,ms,2024-01-02 06:00\n\
P1,QTc,490,ms,2024-01-03 06:00\nP2,QT,470,ms,2024-01-03 06:00\n",
    );
    let c = f.db.get().unwrap();
    let u = User::local();
    let r = cohorts::search_page(&c, &u, "UTC", &measurement("QT", ">", 450.0, "ms", true), None, 100, 0, 50).unwrap();
    assert_eq!(ids(&r), ["P2"], "P1's latest QT is 400 ms (their QTc is another measurement)");
    let r = cohorts::search_page(&c, &u, "UTC", &measurement("QT", ">", 450.0, "ms", false), None, 100, 0, 50).unwrap();
    assert_eq!(ids(&r), ["P1", "P2"], "any value, ever: names containing it");
}

#[test]
fn trends_keep_their_units_apart() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\n");
    import(
        &f,
        "labs.csv",
        "Patient ID,Test Name,Result,Units,Collected\nP1,Glucose,100,mg/dL,2024-01-02 06:00\nP1,Glucose,5.5,mmol/L,2024-01-03 06:00\n\
P1,Glucose,110,mg/dL,2024-01-04 06:00\n",
    );
    let c = f.db.get().unwrap();
    let p: i64 = c.query_row("SELECT id FROM patients WHERE patient_id = 'P1'", [], |r| r.get(0)).unwrap();
    let t = medorama::patient::trends(&c, &User::local(), p).unwrap();
    let mut got: Vec<(String, usize)> =
        t.iter().filter(|x| x.name == "Glucose").map(|x| (x.unit.clone(), x.n)).collect();
    got.sort();
    assert_eq!(got, [("mg/dL".to_string(), 2), ("mmol/L".to_string(), 1)]);
}

fn person(c: &medorama::sql::Connection, name: &str, phi: bool) -> User {
    let role = medorama::users::Role::Analyst;
    medorama::users::add(c, name, "", role, phi, "0123456789ab").unwrap();
    medorama::users::get_by_name(c, name).unwrap().unwrap()
}

fn pid(c: &medorama::sql::Connection, p: &str) -> i64 {
    c.query_row("SELECT id FROM patients WHERE patient_id = ?1", [p], |r| r.get(0)).unwrap()
}

#[test]
fn saved_text_criteria_keep_what_they_meant() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\nP2,M\n");
    import(&f, "ecg_a.csv", "Patient ID,QT (ms)\nP1,480\n");
    let c = f.db.get().unwrap();
    let u = User::local();
    let k = Criterion { kind: "expression".into(), text: "QT > 450 ms".into(), ..Default::default() };
    let id = cohorts::create(
        &c,
        &u,
        "UTC",
        &NewCohort { name: "long QT".into(), criteria: leaf(k.clone()), ..Default::default() },
    )
    .unwrap();
    // a later database with a column of the same name: the text alone would now be ambiguous
    drop(c);
    import(&f, "ecg_b.csv", "Patient ID,QT (ms)\nP2,500\n");
    let c = f.db.get().unwrap();
    let r = cohorts::refresh(&c, &u, "UTC", id, true).unwrap();
    assert!(r.added.is_empty() && r.removed.is_empty(), "the saved meaning: {r:?}");
    let saved = cohorts::get(&c, &u, id).unwrap().criteria;
    let found = cohorts::search_page(&c, &u, "UTC", &saved, None, 100, 0, 50).unwrap();
    assert!(found.warnings.iter().any(|w| w.contains("would not be read now")), "{:?}", found.warnings);
}

#[test]
fn a_refresh_reads_the_data_as_the_criteria_were_written() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\nP2,M\n");
    import(&f, "notes.csv", "patient_id,time,note_type,note_text\nP1,2024-01-02 15:40,Progress note,septic shock\n");
    let c = f.db.get().unwrap();
    let doc = person(&c, "doc", true);
    let reader = person(&c, "reader", false).in_privacy_mode(false);
    let k = Criterion { kind: "text".into(), text: "septic".into(), ..Default::default() };
    let id =
        cohorts::create(&c, &doc, "UTC", &NewCohort { name: "sepsis".into(), criteria: leaf(k), ..Default::default() })
            .unwrap();
    let shares = vec![cohorts::Share { user_id: reader.id, username: String::new(), can_edit: true }];
    cohorts::change(&c, &doc, "UTC", id, &cohorts::Change { shares: Some(shares), ..Default::default() }).unwrap();
    // the reader does not read notes' text: by the same criteria they would take P1 out
    let e = cohorts::refresh(&c, &reader, "UTC", id, true).unwrap_err().to_string();
    assert!(e.contains("written with the PHI permission"), "{e}");
    let r = cohorts::refresh(&c, &doc, "UTC", id, true).unwrap();
    assert!(r.removed.is_empty(), "{r:?}");
}

#[test]
fn a_subcohort_stops_following_a_parent_no_longer_shared() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\nP2,M\n");
    let c = f.db.get().unwrap();
    let (a, b) = (person(&c, "a", false), person(&c, "b", false));
    let parent = cohorts::create(
        &c,
        &a,
        "UTC",
        &NewCohort { name: "parent".into(), patients: vec![pid(&c, "P1"), pid(&c, "P2")], ..Default::default() },
    )
    .unwrap();
    let share = |to: Vec<cohorts::Share>| {
        cohorts::change(&c, &a, "UTC", parent, &cohorts::Change { shares: Some(to), ..Default::default() }).unwrap()
    };
    share(vec![cohorts::Share { user_id: b.id, username: String::new(), can_edit: false }]);
    let source = cohorts::Source { op: "within".into(), of: vec![parent], ..Default::default() };
    let sub = cohorts::create(
        &c,
        &b,
        "UTC",
        &NewCohort {
            name: "women".into(),
            criteria: leaf(Criterion { kind: "sex".into(), text: "F".into(), ..Default::default() }),
            source: Some(source),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(cohorts::refresh(&c, &b, "UTC", sub, true).is_ok());
    share(vec![]);
    let e = cohorts::refresh(&c, &b, "UTC", sub, true).unwrap_err().to_string();
    assert!(e.contains("no longer shared"), "{e}");
}

#[test]
fn a_refresh_applies_the_changes_chosen() {
    let f = catalog();
    import(&f, "patients.csv", "Patient ID,Sex\nP1,F\nP2,F\nP3,M\n");
    let c = f.db.get().unwrap();
    let u = User::local();
    let women = leaf(Criterion { kind: "sex".into(), text: "F".into(), ..Default::default() });
    let id = cohorts::create(
        &c,
        &u,
        "UTC",
        &NewCohort { name: "women".into(), criteria: women, patients: vec![pid(&c, "P1")], ..Default::default() },
    )
    .unwrap();
    let preview = cohorts::refresh(&c, &u, "UTC", id, true).unwrap();
    assert_eq!(preview.added, [pid(&c, "P2")]);
    // a new patient since the preview: not applied with the changes shown
    drop(c);
    import(&f, "more.csv", "Patient ID,Sex\nP4,F\n");
    let c = f.db.get().unwrap();
    let opts = cohorts::RefreshOpts {
        join: Some(preview.added.clone()),
        remove: Some(preview.removed.clone()),
        ..Default::default()
    };
    let r = cohorts::refresh_with(&c, &u, "UTC", id, &opts).unwrap();
    assert_eq!((r.added.clone(), r.not_chosen), (vec![pid(&c, "P2")], 1), "{r:?}");
    let members = cohorts::member_ids(&c, &u, id).unwrap();
    assert!(!members.contains(&pid(&c, "P4")));
}

#[test]
fn intensive_care_is_one_rule() {
    let f = catalog();
    import(
        &f,
        "encounters.csv",
        "Encounter ID,Patient ID,Admit Time,Care Unit,Bed,Bed Start,Bed End\n\
E1,P1,2024-01-01 10:00,Critical Care,1,2024-01-01 11:00,2024-01-02 11:00\n\
E2,P2,2024-01-01 10:00,ITU 3,2,2024-01-01 11:00,2024-01-02 11:00\n\
E3,P3,2024-01-01 10:00,Cardiology,3,2024-01-01 11:00,2024-01-02 11:00\n",
    );
    let c = f.db.get().unwrap();
    let u = User::local();
    for has in [vec!["icu".to_string()], vec!["!icu".to_string()]] {
        let (_, rows) = medorama::patient::list(&c, &u, "", &has, 100, 0).unwrap();
        let mut got: Vec<String> = rows.into_iter().map(|r| r.patient_id).collect();
        got.sort();
        let want: &[&str] = if has[0] == "icu" { &["P1", "P2"] } else { &["P3"] };
        assert_eq!(got, want, "{has:?}");
    }
    let (_, rows) = medorama::patient::list(&c, &u, "has:icu", &[], 100, 0).unwrap();
    assert_eq!(rows.len(), 2, "the search's has:icu too");
}
