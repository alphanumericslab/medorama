//! Patient identity across several databases: values per import, EHR items held by several
//! imports, held-back rows and their review, generated IDs, MRN authorities, merges, duplicates,
//! link decisions (docs/importing.md, "How rows find their patients").

mod common;
use common::{bed_name, opts, wfdb_record, T0};
use medorama::config::Config;
use medorama::db::Db;
use medorama::imports;
use medorama::sql::Connection;
use medorama::users::User;
use medorama::{ehr, linkage, recordings, review, tables};

struct Fx {
    _tmp: tempfile::TempDir,
    data: std::path::PathBuf,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let data = root.join("data");
    wfdb_record(&data.join(bed_name("ICU7", "12", T0)), 0);
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    Fx { _tmp: tmp, data, cfg, db }
}

/// A database imported as a table, named `name`.
fn table(f: &Fx, c: &mut Connection, name: &str, csv: &str) -> tables::TableReport {
    tables::import(c, &f.cfg, &User::local(), name, csv.as_bytes(), &Default::default(), "", None, &opts(name)).unwrap()
}

fn batch(c: &Connection, name: &str) -> i64 {
    c.query_row("SELECT MAX(id) FROM imports WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn remove(f: &Fx, c: &mut Connection, name: &str) -> imports::Removal {
    let id = batch(c, name);
    imports::remove_import(c, &f.cfg, &User::local(), id, false, false).unwrap()
}

fn one<T: medorama::sql::types::FromSql>(c: &Connection, sql: &str) -> T {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn removing_a_table_brings_back_the_earlier_values() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    table(&f, &mut c, "a", "patient_id,mrn,last_name,first_name,encounter_id,admit_unit\nP1,M1,Doe,Jane,E1,ICU7\n");
    table(
        &f,
        &mut c,
        "b",
        "patient_id,mrn,last_name,first_name,encounter_id,admit_unit\nP1,M1,Doe-Smith,Jane,E1,ICU8\n",
    );
    let name = |c: &Connection| -> (String, String) {
        (
            one(c, "SELECT last_name FROM patient_identifiers"),
            one(c, "SELECT admit_unit FROM encounters WHERE encounter_id = 'E1'"),
        )
    };
    assert_eq!(name(&c), ("Doe-Smith".into(), "ICU8".into()), "the newest table wins");
    let r = remove(&f, &mut c, "b");
    assert_eq!((r.patients, r.encounters, r.kept_encounters), (0, 0, 1));
    assert_eq!(name(&c), ("Doe".into(), "ICU7".into()), "removing it brings back the earlier values");
    remove(&f, &mut c, "a");
    assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM patients"), 0);
}

#[test]
fn an_ehr_item_in_two_tables_stays_until_both_are_removed() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    table(&f, &mut c, "adt", "patient_id,encounter_id\nP1,E1\n");
    let row = "encounter_id,domain,name,time,value,unit\nE1,measurement,Lactate,2024-01-01 12:00,2.1,mmol/L\n";
    assert_eq!(table(&f, &mut c, "labs-1", row).items_added, 1);
    let r = table(&f, &mut c, "labs-2", row);
    assert_eq!((r.items_added, r.duplicates), (0, 1));
    let srcs = imports::sources(&c).unwrap();
    assert!(srcs.iter().filter(|s| s.source.starts_with("labs")).all(|s| s.ehr_items == 1), "both sources hold it");
    assert_eq!(remove(&f, &mut c, "labs-1").ehr_items, 0, "labs-2 still holds it");
    let (_, items) = ehr::items(&c, 1, false, None, true, 10, 0).unwrap();
    assert_eq!(items[0].source, "labs-2");
    assert_eq!(remove(&f, &mut c, "labs-2").ehr_items, 1);
}

#[test]
fn generated_ids_and_authorities() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let t = "mrn,mrn_authority,encounter_id\nM1,,E1\nM1,Hospital B,E2\n";
    let r = table(&f, &mut c, "t", t);
    assert_eq!(r.patients_named, 2, "one MRN of two authorities: two patients");
    let ids = |c: &Connection| -> Vec<String> {
        let mut st = c.prepare("SELECT patient_id FROM patients ORDER BY patient_id").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let before = ids(&c);
    assert_eq!(before, ["GEN-000001", "GEN-000002"]);
    remove(&f, &mut c, "t");
    table(&f, &mut c, "t", "mrn,encounter_id\nM9,E9\n");
    table(&f, &mut c, "t", t);
    assert_eq!(ids(&c), ["GEN-000001", "GEN-000002", "GEN-000003"], "an MRN keeps its ID; new MRNs get new ones");

    // a real patient ID arrives for an MRN-only patient: it takes the ID, the old one follows
    table(&f, &mut c, "real", "patient_id,mrn,encounter_id\nP7,M1,E1\n");
    assert_eq!(
        one::<String>(
            &c,
            "SELECT p.patient_id FROM encounters e JOIN patients p ON p.id = e.patient WHERE encounter_id = 'E1'"
        ),
        "P7"
    );
    let r = table(&f, &mut c, "labs", "patient_id,domain,name\nGEN-000001,observation,Weight\n");
    assert_eq!(r.patient_level, 1, "the old generated ID still finds the patient");
}

#[test]
fn held_rows_are_reviewed() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let u = User::local();
    table(
        &f,
        &mut c,
        "a",
        "patient_id,mrn,last_name,first_name,birth_date,encounter_id\nP1,M1,Doe,Jane,1950-03-04,E1\nP2,M2,Doe,Jane,1950-03-04,E2\n",
    );
    // two patients by name and birth date: a possible duplicate
    let rv = review::review(&c, &u).unwrap();
    assert_eq!(rv.duplicates.as_ref().unwrap().len(), 1);
    // conflicts: P1 with P2's MRN; E2 given to P1; P1 with another MRN
    let r = table(&f, &mut c, "b", "patient_id,mrn,encounter_id\nP1,M2,E1\nP1,,E2\nP1,M3,E3\nP9,M1,E9\n");
    assert_eq!(r.conflicts, 4);
    let rv = review::review(&c, &u).unwrap();
    let problems: Vec<&str> = rv.held.iter().map(|h| h.problem.as_str()).collect();
    assert_eq!(problems, ["two_patients", "encounter_of_other_patient", "other_mrn", "mrn_of_other_patient"]);

    // dismiss one, apply the others
    review::decide_held(&mut c, &u, rv.held[1].id, false, "UTC").unwrap();
    review::decide_held(&mut c, &u, rv.held[2].id, true, "UTC").unwrap();
    assert_eq!(one::<i64>(&c, "SELECT COUNT(DISTINCT mrn_key) FROM patient_mrns m JOIN patients p ON p.id = m.patient WHERE p.patient_id = 'P1'"), 2);
    review::decide_held(&mut c, &u, rv.held[0].id, true, "UTC").unwrap();
    assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM patients"), 1, "P2 merged into P1");
    assert_eq!(
        one::<i64>(
            &c,
            "SELECT COUNT(*) FROM encounters e JOIN patients p ON p.id = e.patient WHERE p.patient_id = 'P1'"
        ),
        3
    );
    let rv = review::review(&c, &u).unwrap();
    assert_eq!(rv.held.len(), 1);
    assert!(rv.duplicates.unwrap().is_empty(), "merged: no duplicate left");
    // a later row with the merged ID goes to the patient it was merged into
    table(&f, &mut c, "c", "patient_id,encounter_id\nP2,E5\n");
    assert_eq!(
        one::<String>(
            &c,
            "SELECT p.patient_id FROM encounters e JOIN patients p ON p.id = e.patient WHERE encounter_id = 'E5'"
        ),
        "P1"
    );
    // removing the batch drops its held rows
    remove(&f, &mut c, "b");
    assert!(review::review(&c, &u).unwrap().held.is_empty());
}

#[test]
fn different_people_are_not_suggested_again() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let u = User::local();
    table(
        &f,
        &mut c,
        "a",
        "patient_id,last_name,first_name,birth_date,encounter_id\nP1,Doe,Jane,1950-03-04,E1\nP2,Jane,Doe,1950-03-04,E2\nP3,doe,JANE,1950-04-03,E3\nP4,Roe,Rich,1961-01-01,E4\n",
    );
    let pairs = |c: &Connection| -> Vec<(String, String, String)> {
        let mut v: Vec<_> = review::duplicates(c)
            .unwrap()
            .into_iter()
            .map(|p| {
                let (a, b) = if p.a.patient_id < p.b.patient_id { (p.a, p.b) } else { (p.b, p.a) };
                (a.patient_id, b.patient_id, p.reason)
            })
            .collect();
        v.sort();
        v
    };
    let want = |a: &str, b: &str, r: &str| (a.to_string(), b.to_string(), r.to_string());
    assert_eq!(
        pairs(&c),
        [
            want("P1", "P2", "first and last names swapped, same date of birth"),
            want("P1", "P3", "same name, day and month of birth swapped"),
        ]
    );
    review::distinct(&c, &u, "P2", "P1").unwrap();
    assert_eq!(pairs(&c), [want("P1", "P3", "same name, day and month of birth swapped")]);
    review::merge(&mut c, &u, "P3", "P1").unwrap();
    assert!(pairs(&c).is_empty());
    assert_eq!(
        one::<i64>(
            &c,
            "SELECT COUNT(*) FROM encounters e JOIN patients p ON p.id = e.patient WHERE p.patient_id = 'P1'"
        ),
        2
    );
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn link_decisions_outlive_reimports() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let u = User::local();
    let all = f.data.to_string_lossy().to_string();
    recordings::import(&mut c, &f.cfg, &u, &[all], true, &opts("recs")).unwrap();
    let rec: i64 = one(&c, "SELECT id FROM recordings");
    let t = "patient_id,encounter_id,care_unit,bed,bed_start,bed_end\nP1,E1,ICU7,12,2024-01-01 00:00,2024-01-02 00:00\nP2,E2,ICU9,1,2024-01-01 00:00,2024-01-02 00:00\n";
    table(&f, &mut c, "adt", t);
    let links = |c: &Connection| -> Vec<(String, String)> {
        let mut st = c
            .prepare("SELECT e.encounter_id, l.method FROM recording_links l JOIN encounters e ON e.id = l.encounter ORDER BY 1")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(links(&c), [("E1".to_string(), "bed_time".to_string())]);
    // never E1; link to E2 by hand
    linkage::decide_link(&mut c, &u, rec, "E1", Some("never")).unwrap();
    assert!(links(&c).is_empty());
    linkage::decide_link(&mut c, &u, rec, "E2", Some("link")).unwrap();
    assert_eq!(links(&c), [("E2".to_string(), "manual".to_string())]);
    // remove the linkage table and import it again: the decisions still hold
    remove(&f, &mut c, "adt");
    assert!(links(&c).is_empty());
    table(&f, &mut c, "adt", t);
    assert_eq!(links(&c), [("E2".to_string(), "manual".to_string())]);
    // forget the manual link: automatic linking resumes, still never to E1
    linkage::decide_link(&mut c, &u, rec, "E2", None).unwrap();
    assert!(links(&c).is_empty());
    linkage::decide_link(&mut c, &u, rec, "E1", None).unwrap();
    assert_eq!(links(&c), [("E1".to_string(), "bed_time".to_string())]);
}

#[test]
fn a_merge_keeps_the_cohorts_of_both_patients() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    table(&f, &mut c, "adt", "patient_id,encounter_id\nP1,E1\nP2,E2\nP3,E3\n");
    let id = |c: &Connection, p: &str| -> i64 {
        c.query_row("SELECT id FROM patients WHERE patient_id = ?1", [p], |r| r.get(0)).unwrap()
    };
    let (p1, p2, p3) = (id(&c, "P1"), id(&c, "P2"), id(&c, "P3"));
    c.execute_batch("INSERT INTO cohorts (name, created_at, updated_at) VALUES ('A', 0, 0), ('B', 0, 0)").unwrap();
    // P2 is a member of A by hand; P3 was removed from B by hand; P1 is in B by its criteria
    c.execute("INSERT INTO cohort_members (cohort, patient, how, added_at) VALUES (1, ?1, 'manual', 0)", [p2]).unwrap();
    c.execute("INSERT INTO cohort_members (cohort, patient, how, added_at) VALUES (2, ?1, 'criteria', 0)", [p1])
        .unwrap();
    c.execute("INSERT INTO cohort_removed (cohort, patient, removed_at) VALUES (2, ?1, 0)", [p3]).unwrap();
    {
        let tx = c.transaction().unwrap();
        medorama::identity::merge(&tx, p2, p1, None).unwrap();
        medorama::identity::merge(&tx, p3, p1, None).unwrap();
        tx.commit().unwrap();
    }
    let members: Vec<(i64, i64, String)> = {
        let mut st = c.prepare("SELECT cohort, patient, how FROM cohort_members ORDER BY cohort").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(members, [(1, p1, "manual".to_string()), (2, p1, "criteria".to_string())], "P2's membership moved");
    assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM cohort_removed WHERE cohort = 2"), 1, "P3's removal moved");
    assert_eq!(one::<i64>(&c, "SELECT patient FROM cohort_removed"), p1);
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn links_a_table_gives_go_with_it() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let u = User::local();
    // the recording is in the catalog on its own (its folder imported)
    recordings::import(&mut c, &f.cfg, &u, &[f.data.to_string_lossy().to_string()], true, &opts("folder")).unwrap();
    let rec_dir = f.data.join(bed_name("ICU7", "12", T0));
    let t = format!("patient_id,encounter_id,file\nP1,E1,{}\n", rec_dir.display());
    table(&f, &mut c, "adt", "patient_id,encounter_id\nP1,E1\n");
    table(&f, &mut c, "recs", &t);
    let links = |c: &Connection| -> Vec<(String, String)> {
        let mut st = c
            .prepare("SELECT e.encounter_id, l.method FROM recording_links l JOIN encounters e ON e.id = l.encounter ORDER BY 1")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(links(&c), [("E1".to_string(), "file".to_string())], "linked by the import, not by hand");
    assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM link_decisions"), 0, "no decision is recorded");
    // removing the table removes its link only
    remove(&f, &mut c, "recs");
    assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM recordings"), 1);
    assert!(links(&c).is_empty(), "the recording is no longer linked to E1");
    // imported again: linked again
    table(&f, &mut c, "recs", &t);
    assert_eq!(links(&c), [("E1".to_string(), "file".to_string())]);
    // a decision by hand still wins
    let rec: i64 = one(&c, "SELECT id FROM recordings");
    linkage::decide_link(&mut c, &u, rec, "E1", Some("never")).unwrap();
    assert!(links(&c).is_empty());
}

#[test]
fn removing_the_import_that_named_a_generated_patient_gives_its_id_back() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    table(&f, &mut c, "mrns", "mrn,encounter_id\nM1,E1\nM2,E2\n");
    let pid = |c: &Connection, e: &str| -> String {
        c.query_row(
            "SELECT p.patient_id FROM encounters e JOIN patients p ON p.id = e.patient WHERE encounter_id = ?1",
            [e],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!((pid(&c, "E1"), pid(&c, "E2")), ("GEN-000001".into(), "GEN-000002".into()));
    table(&f, &mut c, "ids", "patient_id,mrn\nP7,M1\nP8,M2\n");
    assert_eq!((pid(&c, "E1"), pid(&c, "E2")), ("P7".into(), "P8".into()), "the IDs taken over");
    // another database names P8 too: it keeps that ID
    table(&f, &mut c, "labs", "patient_id,domain,name\nP8,observation,Weight\n");
    let r = remove(&f, &mut c, "ids");
    assert_eq!(r.ids_restored, 1);
    assert_eq!((pid(&c, "E1"), pid(&c, "E2")), ("GEN-000001".into(), "P8".into()));
    assert_eq!(one::<i64>(&c, "SELECT COUNT(*) FROM patient_merges WHERE into_patient_id = 'P7'"), 0);
    // a later row with the generated ID, or the MRN, still finds the patient
    let r = table(&f, &mut c, "more", "mrn,encounter_id\nM1,E1\n");
    assert_eq!(r.patients_added, 0);
}
