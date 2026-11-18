//! Connected databases (docs/design/databases-only.md): connecting, status, the patient index,
//! grants, variables, searching by database with provenance, and removal with every trace of
//! it.  All data invented.

mod common;
use common::{Dbb, Setup};
use medorama::cohorts::{Criteria, Criterion, Expr, NewCohort};

fn measurement(name: &str, cmp: &str, value: f64) -> Criteria {
    Criteria {
        include: Some(Expr::Leaf(Criterion {
            kind: "measurement".into(),
            name: name.into(),
            cmp: cmp.into(),
            value: Some(value),
            ..Default::default()
        })),
        ..Default::default()
    }
}

/// Two hospitals' databases: A and B share one patient by MRN (HOSP 100); each has one of its own.
fn two(s: &Setup) -> (Dbb, Dbb) {
    let a = Dbb::new(&s.data().join("a.sqlite"));
    a.person("a1", "P1", "STUDY", "100", "HOSP", "Testa", "Ann", "1960-02-03", "F")
        .person("a2", "P2", "STUDY", "200", "HOSP", "Proba", "Bob", "1971-07-08", "M")
        .visit("va1", "a1", "2024-01-02 08:00", "2024-01-05 10:00", "MICU", true)
        .entry("ea1", "a1", "va1", "lab", "2024-01-02 09:00", "Lactate", Some(4.5), "mmol/L")
        .entry("ea2", "a2", "", "lab", "2024-02-01 09:00", "Lactate", Some(1.2), "mmol/L");
    let b = Dbb::new(&s.data().join("b.sqlite"));
    b.person("b1", "", "", "100", "HOSP", "Testa", "Ann", "1960-02-03", "F")
        .person("b3", "", "", "300", "HOSP", "Mock", "Cy", "1980-01-01", "M")
        .entry("eb1", "b1", "", "lab", "2024-03-01 09:00", "Troponin", Some(150.0), "ng/L")
        .entry("eb3", "b3", "", "lab", "2024-03-02 09:00", "Troponin", Some(20.0), "ng/L");
    (a, b)
}

#[test]
fn equal_identifiers_link_one_patient_exactly() {
    let s = Setup::new();
    let (a, b) = two(&s);
    let ida = s.connect("hospa", &a);
    let idb = s.connect("hospb", &b);
    let c = s.db.get().unwrap();
    let patients: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    assert_eq!(patients, 3, "Ann is one patient in both, Bob and Cy one each");
    let ann_a = medorama::persons::known(&s.cfg, &c, ida, &["a1".into()]).unwrap()["a1"];
    let ann_b = medorama::persons::known(&s.cfg, &c, idb, &["b1".into()]).unwrap()["b1"];
    assert_eq!(ann_a, ann_b);
    assert_eq!(medorama::persons::databases_of(&c, ann_a).unwrap(), vec![ida, idb]);
    // the control store holds no identifier: only keyed hashes
    let hashes: Vec<String> = c
        .prepare("SELECT hash FROM patient_keys")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    // (32 hex digits each, never the MRN as it is; a random hash may hold the digits 100)
    assert!(hashes.iter().all(|h| h.len() == 32 && h.bytes().all(|b| b.is_ascii_hexdigit()) && h != "100"));
    // the same number from another authority is another patient
    let d = Dbb::new(&s.data().join("d.sqlite"));
    d.person("d1", "", "", "100", "OTHER", "", "", "", "");
    s.connect("hospd", &d);
    let patients: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    assert_eq!(patients, 4);
    // identifiers compare in any case, as db-builder matches MRNs: x7 is X7
    let e = Dbb::new(&s.data().join("e.sqlite"));
    e.person("e1", "", "", "x7", "LAB", "", "", "", "").person("e2", "", "", "Y8", "LAB", "", "", "", "");
    let f = Dbb::new(&s.data().join("f.sqlite"));
    f.person("f1", "", "", "X7", "lab", "", "", "", "").person("f2", "", "", "y9", "LAB", "", "", "", "");
    s.connect("hospe", &e);
    s.connect("hospf", &f);
    let patients: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    assert_eq!(patients, 7, "x7 and X7 one patient; Y8 and y9 two");
}

#[test]
fn identifiers_naming_two_patients_are_a_conflict_never_merged() {
    let s = Setup::new();
    let (a, _) = two(&s);
    s.connect("hospa", &a);
    // its study ID is Ann's, its MRN is Bob's
    let e = Dbb::new(&s.data().join("e.sqlite"));
    e.person("e1", "P1", "STUDY", "200", "HOSP", "", "", "", "");
    let id = s.connect("hospe", &e);
    let c = s.db.get().unwrap();
    let conflicts = medorama::persons::conflicts(&s.cfg, &c, id).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert!(conflicts[0].1.contains("2 different patients"));
    assert!(medorama::persons::known(&s.cfg, &c, id, &["e1".into()]).unwrap().is_empty(), "not linked");
    let patients: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    assert_eq!(patients, 2, "nothing merged");
}

#[test]
fn status_follows_the_database() {
    let s = Setup::new();
    let (a, _) = two(&s);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    assert_eq!(medorama::databases::get(&c, id).unwrap().unwrap().status, "online");
    let moved = s.data().join("a.moved");
    std::fs::rename(&a.path, &moved).unwrap();
    assert!(!medorama::databases::check(&s.cfg, &c, id).unwrap());
    let d = medorama::databases::get(&c, id).unwrap().unwrap();
    assert_eq!(d.status, "offline");
    assert!(!d.status_error.is_empty());
    // offline: what Medorama keeps of it still lists its patients
    let (n, _) = medorama::patient::list(&c, &s.cfg, &s.user("boss"), "", &[], 50, 0).unwrap();
    assert_eq!(n, 2);
    std::fs::rename(&moved, &a.path).unwrap();
    assert!(medorama::databases::check(&s.cfg, &c, id).unwrap());
}

#[test]
fn a_new_version_is_read_again() {
    let s = Setup::new();
    let (mut a, _) = two(&s);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    assert!(!medorama::variables::holds(&s.cfg, &c, id, "Sodium", false).unwrap());
    a.entry("ea9", "a1", "", "lab", "2024-04-01 09:00", "Sodium", Some(140.0), "mmol/L");
    a.person("a9", "P9", "STUDY", "", "", "", "", "", "F");
    a.bump();
    medorama::databases::check(&s.cfg, &c, id).unwrap();
    assert!(medorama::variables::holds(&s.cfg, &c, id, "Sodium", false).unwrap());
    assert_eq!(medorama::persons::counts(&s.cfg, &c, id).unwrap().0, 3, "its new person linked");
}

#[test]
fn grants_decide_what_a_user_sees() {
    let s = Setup::new();
    let (a, _) = two(&s);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let mut reader = s.user("reader");
    let none = medorama::databases::shown(&s.cfg, &c, &reader).unwrap();
    assert!(none.is_empty(), "nothing granted, nothing shown");
    assert_eq!(medorama::patient::list(&c, &s.cfg, &reader, "", &[], 50, 0).unwrap().0, 0);
    // searching alone: found by criteria, but no page
    medorama::grants::grant(&c, &s.user("boss"), reader.id, &format!("db:{id}"), "search").unwrap();
    let found = medorama::cohorts::evaluate(&c, &s.cfg, &reader, &measurement("Lactate", ">", 4.0), None).unwrap();
    assert_eq!(found.len(), 1);
    let p = *found.keys().next().unwrap();
    assert!(medorama::patient::summary(&c, &s.cfg, &reader, p).is_err(), "search does not show the page");
    // viewing: the page, without identifiers
    medorama::grants::grant(&c, &s.user("boss"), reader.id, &format!("db:{id}"), "view").unwrap();
    let sum = medorama::patient::summary(&c, &s.cfg, &reader, p).unwrap();
    assert!(sum.phi.is_none() && sum.patient_id.is_empty());
    // identifiers: the user's PHI permission and the database's phi grant, both
    reader.phi = true;
    let sum = medorama::patient::summary(&c, &s.cfg, &reader, p).unwrap();
    assert!(sum.phi.as_ref().is_some_and(|i| i.last_name.is_empty()), "no phi grant: no identifiers");
    medorama::grants::grant(&c, &s.user("boss"), reader.id, &format!("db:{id}"), "phi").unwrap();
    let sum = medorama::patient::summary(&c, &s.cfg, &reader, p).unwrap();
    assert_eq!(sum.phi.unwrap().last_name, "Testa");
    // revoking view takes the stronger ones too
    medorama::grants::revoke(&c, &s.user("boss"), reader.id, &format!("db:{id}"), "view").unwrap();
    assert!(medorama::grants::can(&c, &reader, &format!("db:{id}"), "search").unwrap());
    assert!(!medorama::grants::can(&c, &reader, &format!("db:{id}"), "phi").unwrap());
}

#[test]
fn a_search_names_the_databases_it_left_out() {
    let s = Setup::new();
    let (a, b) = two(&s);
    let ida = s.connect("hospa", &a);
    let idb = s.connect("hospb", &b);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let q = measurement("Troponin", ">", 100.0);
    let ev = medorama::cohorts::evaluate_full(&c, &s.cfg, &boss, &q, None).unwrap();
    assert_eq!(ev.found.len(), 1);
    assert_eq!(ev.found.values().next().unwrap().from.iter().copied().collect::<Vec<_>>(), vec![idb]);
    // B unticked: not asked, and said so
    let ev = medorama::cohorts::evaluate_full(&c, &s.cfg, &boss, &q, Some(&[ida])).unwrap();
    assert!(ev.found.is_empty());
    assert_eq!(
        ev.scope.left_out.iter().map(|l| (l.name.as_str(), l.why.as_str())).collect::<Vec<_>>(),
        vec![("hospb", "not ticked")]
    );
    // B offline: left out, named
    std::fs::remove_file(&b.path).unwrap();
    medorama::databases::check(&s.cfg, &c, idb).unwrap();
    let ev = medorama::cohorts::evaluate_full(&c, &s.cfg, &boss, &q, None).unwrap();
    assert_eq!(ev.scope.left_out.iter().map(|l| l.why.as_str()).collect::<Vec<_>>(), vec!["offline"]);
}

#[test]
fn removing_a_database_leaves_no_trace() {
    let s = Setup::new();
    let (a, b) = two(&s);
    let ida = s.connect("hospa", &a);
    let idb = s.connect("hospb", &b);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    // a cohort: Ann (lactate in A AND troponin in B), Bob (lactate in A only)
    let q = Criteria {
        include: Some(Expr::Group(medorama::cohorts::Group {
            op: "or".into(),
            of: vec![
                Expr::Group(medorama::cohorts::Group {
                    op: "and".into(),
                    of: vec![
                        measurement("Lactate", ">", 4.0).include.unwrap(),
                        measurement("Troponin", ">", 100.0).include.unwrap(),
                    ],
                }),
                measurement("Lactate", "<", 2.0).include.unwrap(),
            ],
        })),
        ..Default::default()
    };
    let co = medorama::cohorts::create(
        &c,
        &s.cfg,
        &boss,
        &NewCohort { name: "Both".into(), criteria: q, ..Default::default() },
    )
    .unwrap();
    let members = medorama::cohorts::member_ids(&c, &s.cfg, &boss, co).unwrap();
    assert_eq!(members.len(), 2);
    let cohort_file = medorama::workspace::cohort_file(&s.cfg, co);
    let cohort_bytes = std::fs::read(&cohort_file).unwrap();
    assert!(!cohort_bytes.starts_with(b"SQLite format 3"), "the cohort's file is encrypted");
    let folder = medorama::workspace::database_dir(&s.cfg, idb);
    assert!(folder.is_dir());
    for f in std::fs::read_dir(&folder).unwrap().flatten() {
        let bytes = std::fs::read(f.path()).unwrap();
        assert!(!bytes.windows(5).any(|w| w == b"Testa"), "{}: encrypted", f.path().display());
    }
    medorama::databases::remove(&s.cfg, &c, &boss, idb).unwrap();
    // its key, folder, links, grants, row: gone
    let count = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(count(&format!("SELECT COUNT(*) FROM keys WHERE owner = 'db:{idb}'")), 0);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM patient_keys WHERE database_id = {idb}")), 0);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM grants WHERE object = 'db:{idb}'")), 0);
    assert_eq!(count(&format!("SELECT COUNT(*) FROM databases WHERE id = {idb}")), 0);
    assert!(!folder.exists());
    // Cy, only in B, is no patient any more; Ann and Bob stay (A links them)
    assert_eq!(count("SELECT COUNT(*) FROM patients"), 2);
    // no cohort keeps a row B contributed to: Ann (found through B) is out, Bob (A only) stays
    let left = medorama::cohorts::member_ids(&c, &s.cfg, &boss, co).unwrap();
    assert_eq!(left.len(), 1);
    let bob = medorama::persons::known(&s.cfg, &c, ida, &["a2".into()]).unwrap()["a2"];
    assert!(left.contains(&bob));
    // the database itself is untouched
    assert!(b.path.is_file());
    let n: i64 = rusqlite::Connection::open(&b.path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM persons", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn a_removal_a_crash_interrupted_finishes() {
    let s = Setup::new();
    let (a, _) = two(&s);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    // as if the removal stopped after its first step
    c.execute("UPDATE databases SET status = 'removing' WHERE id = ?1", [id]).unwrap();
    assert!(medorama::sources::get(&c, id).unwrap().is_none(), "no longer offered");
    assert_eq!(medorama::databases::resume_removals(&s.cfg, &c).unwrap(), 1);
    assert!(!medorama::workspace::database_dir(&s.cfg, id).exists());
    let n: i64 = c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn a_cohort_deleted_takes_its_key() {
    let s = Setup::new();
    let (a, _) = two(&s);
    s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let co = medorama::cohorts::create(
        &c,
        &s.cfg,
        &boss,
        &NewCohort { name: "L".into(), criteria: measurement("Lactate", ">", 1.0), ..Default::default() },
    )
    .unwrap();
    medorama::cohorts::delete(&c, &s.cfg, &boss, co).unwrap();
    let keys: i64 =
        c.query_row(&format!("SELECT COUNT(*) FROM keys WHERE owner = 'cohort:{co}'"), [], |r| r.get(0)).unwrap();
    assert_eq!(keys, 0);
    assert!(!medorama::workspace::cohort_file(&s.cfg, co).exists());
}

#[test]
fn a_database_in_the_workspace_or_outside_the_roots_is_refused() {
    let s = Setup::new();
    let boss = s.user("boss");
    let c = s.db.get().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let d = Dbb::new(&outside.path().join("x.sqlite"));
    d.person("x1", "X", "S", "", "", "", "", "", "");
    let n = medorama::sources::NewSource { name: "x".into(), kind: "dbb".into(), url: d.url(), ..Default::default() };
    let e = medorama::databases::connect(&s.cfg, &c, &boss, &n, "").unwrap_err();
    assert!(e.to_string().contains("data_roots"), "{e}");
    let reader = s.user("reader");
    let n = medorama::sources::NewSource {
        name: "y".into(),
        kind: "dbb".into(),
        url: format!("sqlite:{}", s.data().join("none.sqlite").display()),
        ..Default::default()
    };
    assert!(medorama::databases::connect(&s.cfg, &c, &reader, &n, "").is_err(), "admins only");
}

#[test]
fn an_actions_results_are_found_by_name() {
    let s = Setup::new();
    let (a, _) = two(&s);
    common::wfdb_record(&s.data().join("rec"), 0);
    a.recording("r1", "a1", "rec/rec.hea", "icu");
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    medorama::databases::index_recordings(&s.cfg, &c, &boss, id).unwrap();
    let src = medorama::sources::get(&c, id).unwrap().unwrap();
    let rec = medorama::recordings::of_database(&s.cfg, &c, &src).unwrap().remove(0);
    assert_eq!(medorama::derived::store_result(&s.cfg, &c, &rec, "hrv", r#"{"rmssd": 62.5}"#).unwrap(), 1);
    // `hrv` names no database: the whole name is the result's, quoted or not
    for text in ["hrv.rmssd > 50", "\"hrv.rmssd\" > 50", "hospa.\"hrv.rmssd\" > 50"] {
        let q = Criteria {
            include: Some(medorama::cohorts::parse_text(&c, &s.cfg, &boss, text).unwrap()),
            ..Default::default()
        };
        let found = medorama::cohorts::evaluate(&c, &s.cfg, &boss, &q, None).unwrap();
        assert_eq!(found.len(), 1, "{text}");
    }
    let q = Criteria {
        include: Some(medorama::cohorts::parse_text(&c, &s.cfg, &boss, "hrv.rmssd > 70").unwrap()),
        ..Default::default()
    };
    assert!(medorama::cohorts::evaluate(&c, &s.cfg, &boss, &q, None).unwrap().is_empty());
    // a database's name still comes first: `hosp.` is hospa
    let q = Criteria {
        include: Some(medorama::cohorts::parse_text(&c, &s.cfg, &boss, "hosp.Lactate > 4").unwrap()),
        ..Default::default()
    };
    assert_eq!(medorama::cohorts::evaluate(&c, &s.cfg, &boss, &q, None).unwrap().len(), 1);
}

#[test]
fn a_new_master_key_wraps_every_key_and_password() {
    let s = Setup::new();
    let (a, _) = two(&s);
    // an encrypted database: its key is the password Medorama keeps sealed
    let enc = s.data().join("enc.sqlite");
    a.exec(&format!(
        "ATTACH DATABASE '{}' AS enc KEY 'a password'; SELECT sqlcipher_export('enc'); DETACH DATABASE enc;",
        enc.display()
    ));
    let c = s.db.get().unwrap();
    let url = format!("sqlite:{}", enc.display());
    let n = medorama::sources::NewSource { name: "hospa".into(), kind: "dbb".into(), url, ..Default::default() };
    let id = medorama::databases::connect(&s.cfg, &c, &s.user("boss"), &n, "a password").unwrap();
    let old = medorama::workspace::loaded().unwrap();
    let new = medorama::workspace::master_from_hex(&"ab".repeat(32)).unwrap();
    let owner = format!("password:{id}");
    let sealed = |c: &medorama::sql::Connection| -> String {
        c.query_row("SELECT password_wrapped FROM databases WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    };
    let n = medorama::workspace::rotate(&c, old, &new).unwrap();
    assert!(n >= 2, "the database's data key and its password");
    let keys: Vec<(String, String, String)> = c
        .prepare("SELECT owner, wrapped, master_id FROM keys")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!keys.is_empty());
    for (o, w, m) in &keys {
        assert_eq!(*m, new.id);
        assert!(new.unwrap(o, w).is_ok() && old.unwrap(o, w).is_err());
    }
    assert_eq!(new.open(&owner, &sealed(&c)).unwrap(), b"a password");
    assert!(old.open(&owner, &sealed(&c)).is_err());
    // run again (interrupted): nothing to do, nothing broken
    medorama::workspace::rotate(&c, old, &new).unwrap();
    assert_eq!(new.open(&owner, &sealed(&c)).unwrap(), b"a password");
}

#[test]
fn an_encrypted_database_opens_with_its_key() {
    let s = Setup::new();
    let (a, _) = two(&s);
    // an encrypted copy, as db-builder --encrypt writes one (a raw 256-bit key, 64 hex digits)
    let key = "0f".repeat(32);
    let enc = s.data().join("enc.sqlite");
    a.exec(&format!(
        "ATTACH DATABASE '{}' AS enc KEY \"x'{key}'\"; SELECT sqlcipher_export('enc'); DETACH DATABASE enc;",
        enc.display()
    ));
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let n = |name: &str| medorama::sources::NewSource {
        name: name.into(),
        kind: "dbb".into(),
        url: format!("sqlite:{}", enc.display()),
        ..Default::default()
    };
    assert!(medorama::databases::connect(&s.cfg, &c, &boss, &n("nokey"), "").is_err(), "no key: not read");
    assert!(medorama::databases::connect(&s.cfg, &c, &boss, &n("wrong"), &"1e".repeat(32)).is_err());
    medorama::databases::connect(&s.cfg, &c, &boss, &n("hospa"), &key).unwrap();
    let found = medorama::cohorts::evaluate(&c, &s.cfg, &boss, &measurement("Lactate", ">", 4.0), None).unwrap();
    assert_eq!(found.len(), 1);
}

#[test]
fn indexes_go_into_the_databases_encrypted_store_and_leave_with_it() {
    let s = Setup::new();
    let (a, _) = two(&s);
    common::wfdb_record(&s.data().join("rec"), 0);
    a.recording("r1", "a1", "rec/rec.hea", "icu");
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    // connecting reads no recording; indexing all of them reads them (`medorama databases index`)
    assert_eq!(medorama::recordings::count(&s.cfg, &c, id).unwrap(), 0);
    assert_eq!(medorama::jobs::queue_auto_index(&c, &s.cfg).unwrap(), 0);
    medorama::databases::index_recordings(&s.cfg, &c, &s.user("boss"), id).unwrap();
    assert_eq!(medorama::jobs::queue_auto_index(&c, &s.cfg).unwrap(), 1);
    let reg = std::sync::Arc::new(std::sync::RwLock::new(medorama::actions::Registry::load(&s.cfg)));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    medorama::jobs::work(s.db.clone(), std::sync::Arc::new(s.cfg.clone()), reg, 0, stop, true);
    let src = medorama::sources::get(&c, id).unwrap().unwrap();
    let rec = medorama::recordings::of_database(&s.cfg, &c, &src).unwrap().remove(0);
    assert_eq!(rec.status, medorama::recordings::Status::Ready, "{}", rec.error);
    // index format 3: in the database's own folder, encrypted with its key; no index folder
    let file = medorama::workspace::database_dir(&s.cfg, id).join("index.sqlite");
    let store = medorama::workspace::index_store(&s.cfg, &c, id).unwrap();
    assert_eq!(store.keys(), vec![rec.fingerprint.clone()]);
    assert!(store.load(&rec.fingerprint).is_some());
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.windows(rec.fingerprint.len()).any(|w| w == rec.fingerprint.as_bytes()), "encrypted");
    assert!(recording_engine::store::Store::open(&file, None).is_err());
    assert!(!s.cfg.workspace_dir.join("index").exists(), "no index folder of format 2");
    // the time and format it read are the recording's
    assert_eq!(rec.format, "WFDB record");
    assert_eq!(rec.t_start, Some(common::T0 as f64));
    drop(store);
    // removed with its database: the key first, then the folder
    medorama::databases::remove(&s.cfg, &c, &s.user("boss"), id).unwrap();
    assert!(!file.exists());
}

/// A request through the server's router, with a session cookie: (status, headers, body).
async fn http(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<serde_json::Value>,
) -> (axum::http::StatusCode, std::collections::HashMap<String, String>, Vec<u8>) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut b =
        axum::http::Request::builder().method(method).uri(uri).header("Cookie", cookie).header("X-Medorama", "1");
    if body.is_some() {
        b = b.header("Content-Type", "application/json");
    }
    let req = b.body(body.map_or(axum::body::Body::empty(), |v| axum::body::Body::from(v.to_string()))).unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let headers = r.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    (status, headers, r.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn sign_in(app: &axum::Router, user: &str) -> String {
    let (status, h, _) = http(
        app,
        "POST",
        "/api/v1/login",
        "",
        Some(serde_json::json!({"username": user, "password": common::PASSWORD})),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    h["set-cookie"].split(';').next().unwrap().to_string()
}

/// The PHI permission shows the identifiers of the databases granted `phi` only (docs/security.md,
/// "Privacy Mode and the privacy gate"): a user with it, Privacy Mode off, granted `phi` on B and
/// only `view` on A, gets A's data about the same linked patient de-identified (names, dates,
/// file names, notes, the files' bytes), and B's identified.
#[tokio::test]
#[cfg_attr(not(feature = "wfdb"), ignore = "a WFDB recording: a build without the `wfdb` feature refuses it")]
async fn phi_on_one_database_does_not_identify_another() {
    let s = Setup::new();
    // A: the hospital's notes and ECGs, named after the patient as some sites name files
    std::fs::create_dir_all(s.data().join("ecg")).unwrap();
    std::fs::write(
        s.data().join("ecg/Qwertzia_ecg.hea"),
        "Qwertzia_ecg 1 500 1000 08:00:00 02/01/2024\nQwertzia_ecg.dat 16 200/mV 16 0 0 0 0 I\n\
         # Name: Qwertzia Blumfeld\n# DOB: 1940-05-06\n# seen 2024-01-02 08:00\n",
    )
    .unwrap();
    std::fs::write(s.data().join("ecg/Qwertzia_ecg.dat"), vec![0u8; 2000]).unwrap();
    let a = Dbb::new(&s.data().join("a.sqlite"));
    a.person("pa", "", "", "MRN555777", "HOSP", "Qwertzia", "Blumfeld", "1940-05-06", "F")
        .visit("va", "pa", "2024-01-01 10:00", "2024-01-05 10:00", "MICU", true)
        .entry("la", "pa", "va", "lab", "2024-01-02 06:10", "Lactate", Some(4.1), "mmol/L")
        .note(
            "na",
            "pa",
            "2024-01-02 15:40",
            "Progress note",
            "Qwertzia (MRN555777), born 1940-05-06, call 555-123-4567.",
        )
        .recording("ra", "pa", "ecg/Qwertzia_ecg.hea", "ecg");
    a.exec("UPDATE recordings SET start_time = '2024-01-02 08:00' WHERE recording_key = 'ra'");
    // B: the same patient (by MRN), identified for the user
    let b = Dbb::new(&s.data().join("b.sqlite"));
    b.person("pb", "", "", "MRN555777", "HOSP", "Qwertzia", "Blumfeld", "1940-05-06", "F").entry(
        "lb",
        "pb",
        "",
        "lab",
        "2024-03-01 09:00",
        "Troponin",
        Some(150.0),
        "ng/L",
    );
    let ida = s.connect("hospa", &a);
    let idb = s.connect("hospb", &b);
    let c = s.db.get().unwrap();
    medorama::users::add(&c, "mixed", "", medorama::users::Role::Analyst, true, common::PASSWORD).unwrap();
    let mixed = s.user("mixed");
    let boss = s.user("boss");
    medorama::grants::grant(&c, &boss, mixed.id, &format!("db:{ida}"), "view").unwrap();
    medorama::grants::grant(&c, &boss, mixed.id, &format!("db:{idb}"), "phi").unwrap();
    let p = medorama::persons::known(&s.cfg, &c, ida, &["pa".into()]).unwrap()["pa"];
    assert_eq!(medorama::persons::known(&s.cfg, &c, idb, &["pb".into()]).unwrap()["pb"], p, "one patient");
    let app = s.app();
    let me = sign_in(&app, "mixed").await;
    let (st, _, _) = http(&app, "POST", "/api/v1/privacy", &me, Some(serde_json::json!({"on": false}))).await;
    assert!(st.is_success());
    let api = "/api/v1";
    let get = |uri: String| {
        let (app, me) = (app.clone(), me.clone());
        async move { http(&app, "GET", &uri, &me, None).await }
    };
    let text = |b: &[u8]| String::from_utf8_lossy(b).to_string();
    // A's real times (its stay, 2024-01-01 to 2024-01-05): never sent
    let a_real = 1_704_067_200.0..1_704_499_200.0;
    let times = |v: &serde_json::Value| {
        fn walk(v: &serde_json::Value, out: &mut Vec<f64>) {
            match v {
                serde_json::Value::Number(n) => out.extend(n.as_f64()),
                serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                serde_json::Value::Object(m) => m.values().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = vec![];
        walk(v, &mut out);
        out
    };

    // the summary: the identity B gives (phi on B), B's result at its real time
    let (_, _, sum) = get(format!("{api}/patients/{p}")).await;
    let sum: serde_json::Value = serde_json::from_slice(&sum).unwrap();
    assert_eq!(sum["phi"]["last_name"], "Qwertzia", "identified by B");
    for page in ["", "/journey", "/report", "/trends"] {
        let (_, _, body) = get(format!("{api}/patients/{p}{page}")).await;
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let real: Vec<f64> = times(&v).into_iter().filter(|t| a_real.contains(t)).collect();
        assert!(real.is_empty(), "{page}: A's real times {real:?}");
        if page == "/trends" {
            assert!(
                text(&body).contains("Troponin")
                    && times(&v).iter().any(|t| (1_709_200_000.0..1_709_400_000.0).contains(t)),
                "B's own time: {v}"
            );
        }
    }
    // each of A's entries, in detail: its note's text de-identified, its author withheld
    let (_, _, rep) = get(format!("{api}/patients/{p}/report")).await;
    let rep: serde_json::Value = serde_json::from_slice(&rep).unwrap();
    let mut files = vec![];
    let mut note_seen = false;
    for e in rep["entries"].as_array().unwrap() {
        let key = e["key"].as_str().unwrap();
        let (_, _, d) = get(format!("{api}/patients/{p}/detail?key={}", key.replace(':', "%3A"))).await;
        let body = text(&d);
        let d: serde_json::Value = serde_json::from_slice(&d).unwrap();
        if d["sources"][0] == "hospb" {
            continue;
        }
        for gone in
            ["MRN555777", "1940-05-06", "555-123-4567", "Qwertzia_ecg", "Dr Invented", "2024-01-0", "02/01/2024"]
        {
            assert!(!body.contains(gone), "{key}: {gone}: {body}");
        }
        assert!(!times(&d).iter().any(|t| a_real.contains(t)), "{key}: real times: {body}");
        if d["category"] == "note" {
            let t = d["text"].as_str().unwrap();
            assert!(t.contains("[Patient Name]") && !t.contains("Qwertzia"), "{t}");
            note_seen = true;
        }
        if let Some(att) = d.get("attachment") {
            assert!(att["name"].as_str().unwrap().starts_with("mo-f"), "{att}");
            files.push(att["src"].as_str().unwrap().to_string());
        }
    }
    assert!(note_seen && files.len() == 1, "{rep}");
    // the file: de-identified, by its token; its real name is not accepted
    let (st, _, hea) = get(format!("{api}/{}", files[0])).await;
    assert!(st.is_success(), "{st}");
    let hea = text(&hea);
    for gone in ["Qwertzia", "Blumfeld", "1940-05-06", "02/01/2024", "2024-01-02"] {
        assert!(!hea.contains(gone), "{gone}: {hea}");
    }
    let base = files[0].rsplit_once('/').unwrap().0;
    let (st, _, _) = get(format!("{api}/{base}/Qwertzia_ecg.dat")).await;
    assert_eq!(st, axum::http::StatusCode::NOT_FOUND);
    // the recordings list: A's by its token and moved times
    let (_, _, recs) = get(format!("{api}/recordings")).await;
    assert!(!text(&recs).contains("Qwertzia") && text(&recs).contains("mo-rec:"), "{}", text(&recs));
    let (_, _, recs) = get(format!("{api}/recordings?q=qwertzia")).await;
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&recs).unwrap()["total"], 0, "not found by its file name");
    // exports: A's rows de-identified, B's identified
    let (_, _, co) = http(
        &app,
        "POST",
        &format!("{api}/cohorts"),
        &me,
        Some(serde_json::json!({"name": "Both", "criteria": {"version": 1, "include": {"kind": "has", "category": "lab"}}})),
    )
    .await;
    let co: serde_json::Value = serde_json::from_slice(&co).unwrap();
    let (_, _, csv) = get(format!("{api}/cohorts/{}/export?what=data", co["id"])).await;
    let csv = text(&csv);
    for line in csv.lines().filter(|l| l.starts_with("hospa")) {
        for gone in ["Qwertzia", "MRN555777", "1940-05-06", "555-123-4567", "2024-01-0"] {
            assert!(!line.contains(gone), "{gone}: {line}");
        }
    }
    assert!(csv.lines().any(|l| l.starts_with("hospb") && l.contains("2024-03-01")), "{csv}");
    // with phi on A too, A is identified
    medorama::grants::grant(&c, &boss, mixed.id, &format!("db:{ida}"), "phi").unwrap();
    let (_, _, hea) =
        get(format!("{api}/patients/{p}/file/{}/Qwertzia_ecg.hea", base.rsplit('/').next().unwrap())).await;
    assert!(text(&hea).contains("Qwertzia"), "identified now: {}", text(&hea));
}

/// The gate's secret is kept wrapped by the master key, never in plain text; one an earlier
/// version kept in plain text is wrapped at its first use, the same secret (the patient index's
/// hashes still lead to their patients).
#[test]
fn the_gates_secret_is_wrapped() {
    let s = Setup::new();
    let (a, _) = two(&s);
    s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let plain: i64 = c.query_row("SELECT COUNT(*) FROM settings WHERE key = 'gate_secret'", [], |r| r.get(0)).unwrap();
    assert_eq!(plain, 0, "no secret in plain text");
    let wrapped: String = c.query_row("SELECT wrapped FROM keys WHERE owner = 'gate'", [], |r| r.get(0)).unwrap();
    let secret = medorama::gate::secret(&c).unwrap();
    let hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!wrapped.contains(&hex));
    // an earlier version's, in plain text: wrapped at first use, the same bytes
    c.execute("DELETE FROM keys WHERE owner = 'gate'", []).unwrap();
    c.execute(
        "INSERT INTO settings (key, value, updated_by, updated_at) VALUES ('gate_secret', ?1, NULL, 0)",
        [serde_json::to_string(&hex).unwrap()],
    )
    .unwrap();
    assert_eq!(*medorama::gate::secret(&c).unwrap(), *secret);
    let plain: i64 = c.query_row("SELECT COUNT(*) FROM settings WHERE key = 'gate_secret'", [], |r| r.get(0)).unwrap();
    assert_eq!(plain, 0, "its plain copy deleted");
    assert_eq!(*medorama::gate::secret(&c).unwrap(), *secret);
}

/// Grants' routes answer 404 to who may not search a database, whether it exists or not (its
/// number tells nothing), and 403 to who may search it without managing it; jobs are those of
/// the databases granted.
#[tokio::test]
async fn grants_routes_tell_no_database_apart() {
    let s = Setup::new();
    let (a, _) = two(&s);
    let id = s.connect("hospa", &a);
    let app = s.app();
    let reader = sign_in(&app, "reader").await;
    for db in [id, 999] {
        for (m, p) in [("GET", "grants"), ("POST", "grants"), ("DELETE", "grants"), ("GET", "conflicts")] {
            let body = (m != "GET").then(|| serde_json::json!({"user_id": 1, "permission": "view"}));
            let (st, _, _) = http(&app, m, &format!("/api/v1/databases/{db}/{p}"), &reader, body).await;
            assert_eq!(st, axum::http::StatusCode::NOT_FOUND, "{m} {db}/{p}");
        }
    }
    let c = s.db.get().unwrap();
    medorama::grants::grant(&c, &s.user("boss"), s.user("reader").id, &format!("db:{id}"), "search").unwrap();
    let (st, _, _) = http(&app, "GET", &format!("/api/v1/databases/{id}/grants"), &reader, None).await;
    assert_eq!(st, axum::http::StatusCode::FORBIDDEN);
    // jobs: none of a database not granted for viewing
    c.execute(
        "INSERT INTO jobs (action, action_version, params_hash, database_id, recording, fingerprint, state, created_at, error)
         VALUES ('index', '1', '', ?1, ?2, '', 'failed', 0, 'no such file /data/Qwertzia.hea')",
        [id, id * 1_000_000_000 + 1],
    )
    .unwrap();
    let (_, _, jobs) = http(&app, "GET", "/api/v1/jobs?state=failed", &reader, None).await;
    let v: serde_json::Value = serde_json::from_slice(&jobs).unwrap();
    assert_eq!(v["items"].as_array().unwrap().len(), 0, "{v}");
    medorama::grants::grant(&c, &s.user("boss"), s.user("reader").id, &format!("db:{id}"), "view").unwrap();
    let (_, _, jobs) = http(&app, "GET", "/api/v1/jobs?state=failed", &reader, None).await;
    let v: serde_json::Value = serde_json::from_slice(&jobs).unwrap();
    assert_eq!(v["items"][0]["error"], "failed", "another's error, and in Privacy Mode: {v}");
    assert_eq!(v["counts"]["failed"], 1);
}

/// An encounter criterion "in intensive care" finds a visit with an intensive-care bed stay, as
/// `has icu` does, though the visit itself is on a ward.
#[test]
fn an_encounter_in_intensive_care_counts_its_icu_stays() {
    let s = Setup::new();
    let d = Dbb::new(&s.data().join("beds.sqlite"));
    d.person("p1", "S1", "STUDY", "", "", "", "", "1950-03-04", "F")
        .visit("E1", "p1", "2024-01-01 08:00", "2024-01-09 13:00", "Cardiology", false)
        .person("p2", "S2", "STUDY", "", "", "", "", "1951-03-04", "M")
        .visit("E2", "p2", "2024-01-01 08:00", "2024-01-09 13:00", "Cardiology", false);
    d.exec(
        "CREATE TABLE extras (table_name TEXT, row_key TEXT, name TEXT, value TEXT, source_id INTEGER);
         INSERT INTO extras VALUES ('bed_stays', '1:1', 'visit_key', 'E1', 1), ('bed_stays', '1:1', 'care_unit', 'ICU7', 1),
             ('bed_stays', '1:1', 'bed_start', '2024-01-01T11:00:00Z', 1),
             ('bed_stays', '1:1', 'bed_end', '2024-01-03T11:00:00Z', 1);",
    );
    s.connect("beds", &d);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let crit = |icu: bool| Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "encounter".into(), icu: Some(icu), ..Default::default() })),
        ..Default::default()
    };
    let has = Criteria {
        include: Some(Expr::Leaf(Criterion { kind: "has".into(), category: "icu".into(), ..Default::default() })),
        ..Default::default()
    };
    let in_icu = medorama::cohorts::evaluate(&c, &s.cfg, &boss, &crit(true), None).unwrap();
    let has_icu = medorama::cohorts::evaluate(&c, &s.cfg, &boss, &has, None).unwrap();
    assert_eq!(in_icu.len(), 1, "p1's visit, by its ICU stay");
    assert_eq!(in_icu.keys().collect::<Vec<_>>(), has_icu.keys().collect::<Vec<_>>(), "both agree");
    let not_icu = medorama::cohorts::evaluate(&c, &s.cfg, &boss, &crit(false), None).unwrap();
    assert_eq!(not_icu.len(), 1, "p2 only");
    assert!(!not_icu.contains_key(in_icu.keys().next().unwrap()));
}
