//! The demo (docs/administration.md, "The demo"; demo.rs): loaded as the source DEMO with the
//! cohort DEMO, which every user sees; deleting that cohort (an admin) removes the demo and
//! nothing else; a folder not made by tools/make-demo-data.py is refused.

mod common;
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::users::{self, Role};
use medorama::{cohorts, demo, tables};
use std::collections::HashMap;

/// A small demo folder, as tools/make-demo-data.py lays it out: README.txt and databases/.
fn demo_folder(root: &std::path::Path) -> std::path::PathBuf {
    let dir = root.join("demo");
    std::fs::create_dir_all(dir.join("databases")).unwrap();
    std::fs::write(
        dir.join("README.txt"),
        "Make a synthetic demo data set for Medorama: fake patients, their databases and files.\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("databases").join("patients.csv"),
        "MRN,Last Name,First Name,DOB,Sex,Patient ID\nP900001,Demo,Ada,1950-01-02,F,DEMO-1\nP900002,Demo,Ben,1960-03-04,M,DEMO-2\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("databases").join("labs.csv"),
        "MRN,Test Name,Result,Units,Collected\nP900001,Lactate,2.1,mmol/L,2024-01-01 10:00\nP900002,Lactate,1.2,mmol/L,2024-01-01 11:00\n",
    )
    .unwrap();
    dir
}

#[test]
fn the_demo_comes_and_goes_with_its_cohort() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let admin_id = users::add(&c, "admin", "", Role::Admin, true, "a long password 1").unwrap();
    let viewer_id = users::add(&c, "viewer", "", Role::Viewer, false, "a long password 2").unwrap();
    let analyst_id = users::add(&c, "analyst", "", Role::Analyst, false, "a long password 3").unwrap();
    let admin = users::get(&c, admin_id).unwrap().unwrap();
    let viewer = users::get(&c, viewer_id).unwrap().unwrap();
    let analyst = users::get(&c, analyst_id).unwrap().unwrap();

    // a folder tools/make-demo-data.py did not make is refused
    let other = root.join("other");
    std::fs::create_dir_all(other.join("databases")).unwrap();
    assert!(demo::add(&mut c, &cfg, &admin, &other).is_err());
    // only admins load it
    let dir = demo_folder(&root);
    assert!(demo::add(&mut c, &cfg, &viewer, &dir).is_err());
    assert!(!demo::present(&c).unwrap());

    // another source's patient, before and after: never touched
    let ward = tables::import(
        &mut c,
        &cfg,
        &admin,
        "ward.csv",
        b"MRN,Last Name,First Name,DOB,Sex\nSYN7001,Testcase,Alex,1970-01-02,F\n",
        &HashMap::new(),
        "",
        None,
        &Opts::new("ward.csv", "ward", false),
    )
    .unwrap();
    assert_eq!(ward.patients_added, 1);

    let d = demo::add(&mut c, &cfg, &admin, &dir).unwrap();
    assert_eq!((d.patients, d.tables.len(), d.connected), (2, 2, false));
    assert!(demo::add(&mut c, &cfg, &admin, &dir).is_err(), "loaded once only");
    // every user sees the cohort DEMO, read-only
    for u in [&viewer, &analyst] {
        let list = cohorts::list(&c, u).unwrap();
        assert_eq!(list.len(), 1);
        let co = &list[0];
        assert_eq!((co.name.as_str(), co.members, co.demo, co.can_edit, co.is_owner), ("DEMO", 2, true, false, false));
        assert!(cohorts::delete(&c, u, co.id).is_err(), "only admins delete the demo");
        assert!(demo::remove(&mut c, &cfg, u, false).is_err());
    }
    // a refresh takes in no one else: its definition is the demo's patients
    let r = cohorts::refresh(&c, &admin, "UTC", d.cohort, true).unwrap();
    assert!(r.added.is_empty() && r.removed.is_empty());

    // the dry run says what goes, and changes nothing
    let preview = demo::remove(&mut c, &cfg, &admin, true).unwrap();
    assert_eq!((preview.patients, preview.ehr_items), (2, 2));
    assert!(demo::present(&c).unwrap());

    let gone = demo::remove(&mut c, &cfg, &admin, false).unwrap();
    assert_eq!((gone.patients, gone.ehr_items, gone.dry_run), (2, 2, false));
    assert!(!demo::present(&c).unwrap());
    assert!(cohorts::list(&c, &admin).unwrap().is_empty());
    let left: Vec<String> = c
        .prepare("SELECT patient_id FROM patients ORDER BY patient_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(left.len(), 1, "the ward's patient stays: {left:?}");
    assert!(demo::remove(&mut c, &cfg, &admin, false).is_err(), "nothing left to remove");

    // and it can be loaded again
    let again = demo::add(&mut c, &cfg, &admin, &dir).unwrap();
    assert_eq!(again.patients, 2);
}
