//! End-to-end tests of the catalog on synthetic WFDB recordings in a temporary folder.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use medorama::actions::Registry;
use medorama::config::Config;
use medorama::db::{now, Db};
use medorama::jobs::{self, Selection};
use medorama::query::{self, Search};
use medorama::recordings::{self, Status};
use medorama::users::{self, Role, User};
use medorama::{ehr, linkage, server, tables};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

mod common;
use common::opts;
use common::{bed_name, wfdb_record, T0};

struct Fixture {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let data = root.join("data");
    wfdb_record(&data.join(bed_name("ICU7", "12", T0)), 0);
    wfdb_record(&data.join("sub").join(bed_name("ICU7", "14", T0 + 3600)), 3600);
    wfdb_record(&data.join("unnamed"), 7200);
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    Fixture { _tmp: tmp, data, cfg, db }
}

fn admin() -> User {
    User::local()
}

fn user(c: &medorama::sql::Connection, name: &str, role: Role, phi: bool) -> User {
    let id = users::add(c, name, "", role, phi, "0123456789ab").unwrap();
    users::get(c, id).unwrap().unwrap()
}

fn run_all_jobs(f: &Fixture) {
    let reg = Arc::new(RwLock::new(Registry::load(&f.cfg)));
    let db = Arc::new(Db::open(&f.cfg, false).unwrap());
    jobs::work(db, Arc::new(f.cfg.clone()), reg, 0, Arc::new(AtomicBool::new(false)), true);
}

/// A database imported as a table, named `name`.
fn table(f: &Fixture, c: &mut medorama::sql::Connection, name: &str, csv: &str) -> tables::TableReport {
    tables::import(c, &f.cfg, &admin(), name, csv.as_bytes(), &Default::default(), "", None, &opts(name)).unwrap()
}

const LINKAGE: &str = "\
patient_id,mrn,last_name,first_name,birth_date,sex,encounter_id,csn,admit_time,discharge_time,care_unit,bed,bed_start,bed_end,recording_file
P1,MRN001,Doe,Jane,1950-03-04,F,E1,555001,2024-01-01 10:00,2024-01-03 10:00,ICU7,12,2024-01-01 11:00,2024-01-02 11:00,
P1,MRN001,Doe,Jane,1950-03-04,F,E1,555001,2024-01-01 10:00,2024-01-03 10:00,ICU7,14,2024-01-02 11:00,2024-01-03 09:00,
P2,MRN002,Roe,Rich,03/05/1961,M,E2,555002,2024-01-01 08:00,2024-01-01 20:00,ICU7,14,2024-01-01 08:30,2024-01-01 19:00,
P2,MRN002,Roe,Rich,03/05/1961,M,E2,555002,,,,,,,unnamed
P3,,,,,,E3,,2024-02-01 08:00,,,,,,
";

/// A build without the `wfdb` feature (`cargo test -p medorama --no-default-features`): the
/// WFDB recordings found are not added but listed as skipped, with the reason, while an EDF file
/// beside them is added and indexed as in the default build.
#[cfg(not(feature = "wfdb"))]
#[test]
fn without_wfdb_a_wfdb_recording_is_refused_and_edf_is_added() {
    let f = fixture();
    let testdata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../viewers/vitalstream/testdata");
    std::fs::copy(testdata.join("edf/psg_plus.edf"), f.data.join("psg_plus.edf")).unwrap();
    let mut c = f.db.get().unwrap();
    let a = admin();
    let r =
        recordings::import(&mut c, &f.cfg, &a, &[f.data.to_string_lossy().to_string()], true, &opts("test")).unwrap();
    assert_eq!((r.added, r.already), (1, 0), "{:?}", r.skipped);
    let refused = r.skipped.iter().filter(|(_, why)| why == recording_engine::sources::NO_WFDB).count();
    assert_eq!(refused, 3, "{:?}", r.skipped);
    // a WFDB record named alone, as a database row names one: refused too
    let hea = f.data.join("unnamed/rec.hea").to_string_lossy().to_string();
    let r = recordings::import(&mut c, &f.cfg, &a, &[hea], false, &opts("test")).unwrap();
    assert_eq!((r.added, r.skipped.len(), r.skipped[0].1.as_str()), (0, 1, recording_engine::sources::NO_WFDB));
    let reg = Registry::load(&f.cfg);
    let sel = Selection { all: true, ..Default::default() };
    let run =
        jobs::create_run(&mut c, &reg, &a, &["index".into()], &Default::default(), &sel, false, 0, "UTC").unwrap();
    assert_eq!(run.queued, 1);
    drop(c);
    run_all_jobs(&f);
    let c = f.db.get().unwrap();
    let (n, recs) = recordings::list(&c, &admin(), "", false, 100, 0, "UTC").unwrap();
    assert_eq!((n, recs[0].kind.as_str(), recs[0].status), (1, "edf", Status::Ready), "{}", recs[0].error);
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn import_index_link_search() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let a = admin();

    // import: three recordings, found recursively; again: nothing new
    let r =
        recordings::import(&mut c, &f.cfg, &a, &[f.data.to_string_lossy().to_string()], true, &opts("test")).unwrap();
    assert_eq!((r.added, r.already), (3, 0), "{:?}", r.skipped);
    let r =
        recordings::import(&mut c, &f.cfg, &a, &[f.data.to_string_lossy().to_string()], true, &opts("test")).unwrap();
    assert_eq!((r.added, r.already), (0, 3));
    let (n, recs) = recordings::list(&c, &admin(), "", false, 100, 0, "UTC").unwrap();
    assert_eq!(n, 3);
    assert!(recs.iter().all(|r| r.status == Status::New && r.kind == "wfdb"));
    let named = recs.iter().find(|r| r.bed == "12").unwrap();
    assert_eq!((named.unit.as_str(), named.name_time), ("ICU7", Some(T0 as f64)));

    // only admins add
    let v = user(&c, "vic", Role::Viewer, false);
    assert!(
        recordings::import(&mut c, &f.cfg, &v, &[f.data.to_string_lossy().to_string()], true, &opts("test")).is_err()
    );

    // index everything
    let reg = Registry::load(&f.cfg);
    let run = jobs::create_run(
        &mut c,
        &reg,
        &a,
        &["index".into()],
        &Default::default(),
        &Selection { all: true, ..Default::default() },
        false,
        0,
        "UTC",
    )
    .unwrap();
    assert_eq!(run.queued, 3);
    drop(c);
    run_all_jobs(&f);
    let mut c = f.db.get().unwrap();
    let (_, recs) = recordings::list(&c, &admin(), "", false, 100, 0, "UTC").unwrap();
    for r in &recs {
        assert_eq!(r.status, Status::Ready, "{}: {}", r.name, r.error);
        assert!(f.cfg.index_dir.join(&r.fingerprint).join("meta.json").is_file());
        assert!(r.t_start.is_some() && r.t_end.unwrap() - r.t_start.unwrap() > 19.0);
    }
    // the index key is the viewers' cache key: a viewer pointed at index_dir reuses it
    assert_eq!(recs[0].fingerprint, recording_engine::index::cache_key(Path::new(&recs[0].path)));
    // nothing written into the data folder
    let mut files = vec![];
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                walk(&e.path(), out)
            } else {
                out.push(e.path())
            }
        }
    }
    walk(&f.data, &mut files);
    assert_eq!(files.len(), 6, "{files:?}");
    // a second run does nothing
    let run = jobs::create_run(
        &mut c,
        &reg,
        &a,
        &["index".into()],
        &Default::default(),
        &Selection { all: true, ..Default::default() },
        false,
        0,
        "UTC",
    )
    .unwrap();
    assert_eq!((run.queued, run.done_already), (0, 3));

    // linkage: by bed and time, and by file name
    let rep = table(&f, &mut c, "linkage.csv", LINKAGE);
    assert_eq!((rep.patients_added, rep.encounters_added, rep.bed_stays_added), (3, 3, 4), "{:?}", rep.warnings);
    assert_eq!(rep.recordings_linked, 3);
    let viewer = user(&c, "ann", Role::Analyst, false);
    let doctor = user(&c, "doc", Role::Viewer, true);
    let (total, rows) = query::search(&c, &viewer, &Search::default(), "UTC").unwrap();
    assert_eq!(total, 3);
    assert!(rows.iter().all(|e| e.phi.is_none()));
    let e1 = rows.iter().find(|e| e.encounter_id == "E1").unwrap();
    // ICU7_12 at T0 (11:00-..) -> E1; ICU7_14 at T0+1h (13:00) -> E2 (08:30-19:00), not E1 (from 2024-01-02)
    assert_eq!((e1.recordings, e1.status), (1, Some(Status::Ready)));
    let e2 = rows.iter().find(|e| e.encounter_id == "E2").unwrap();
    assert_eq!(e2.recordings, 2);
    assert_eq!(rows.iter().find(|e| e.encounter_id == "E3").unwrap().status, None);

    // identifiers: searchable and shown only with the PHI permission
    assert_eq!(query::search(&c, &viewer, &Search { q: "doe".into(), ..Default::default() }, "UTC").unwrap().0, 0);
    let (n, rows) = query::search(&c, &doctor, &Search { q: "doe 1950".into(), ..Default::default() }, "UTC").unwrap();
    assert_eq!(n, 1);
    assert_eq!(rows[0].phi.as_ref().unwrap().mrn, "MRN001");
    assert_eq!(
        query::search(&c, &doctor, &Search { q: "1961-03-05".into(), ..Default::default() }, "UTC").unwrap().0,
        1
    );
    assert_eq!(query::search(&c, &doctor, &Search { q: "555002".into(), ..Default::default() }, "UTC").unwrap().0, 1);
    let audited: i64 = c
        .query_row("SELECT COUNT(*) FROM audit WHERE event = 'phi_search' AND username = 'doc'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(audited, 3);
    // status filter, unit filter
    assert_eq!(
        query::search(&c, &viewer, &Search { status: "none".into(), ..Default::default() }, "UTC").unwrap().0,
        1
    );
    assert_eq!(query::search(&c, &viewer, &Search { unit: "icu7".into(), ..Default::default() }, "UTC").unwrap().0, 2);

    // EHR: unknown encounters skipped, duplicates not stored twice, notes only with PHI
    let csv = "encounter_id,domain,name,time,value,unit,vocabulary,code\n\
               E1,measurement,Lactate,2024-01-01 12:00,2.1,mmol/L,LOINC,2524-7\n\
               E1,note,Progress note,2024-01-01 13:00,Text naming someone,,,\n\
               E9,measurement,Lactate,2024-01-01 12:00,2.1,mmol/L,LOINC,2524-7\n";
    let r = table(&f, &mut c, "ehr.csv", csv);
    // E9 is not (yet) in the catalog: its row waits for a database that names it
    assert_eq!((r.items_added, r.waiting, r.with_encounter), (3, 1, 2));
    let r = table(&f, &mut c, "ehr.csv", csv);
    assert_eq!((r.items_added, r.duplicates), (0, 3));
    assert_eq!(ehr::items(&c, e1.id, false, None, false, 100, 0).unwrap().0, 1);
    let (n, items) = ehr::items(&c, e1.id, false, None, true, 100, 0).unwrap();
    assert_eq!(n, 2);
    assert_eq!(items[0].value, serde_json::json!(2.1));
    assert_eq!(items[0].time, Some(T0 as f64));

    // history: per user, removable
    let d = query::encounter(&c, &viewer, e1.id).unwrap().unwrap();
    assert!(d.encounter.phi.is_none());
    query::encounter(&c, &viewer, e2.id).unwrap();
    query::encounter(&c, &viewer, e1.id).unwrap();
    let h = query::history(&c, &viewer, 10).unwrap();
    assert_eq!(h.iter().map(|x| x.encounter.encounter_id.as_str()).collect::<Vec<_>>(), ["E1", "E2"]);
    assert_eq!(h[0].views, 2);
    assert!(query::history(&c, &doctor, 10).unwrap().is_empty());
    query::forget(&c, &viewer, Some(e2.id)).unwrap();
    assert_eq!(query::history(&c, &viewer, 10).unwrap().len(), 1);

    // removal: database only; results purged only inside results_dir
    let named = recs.iter().find(|r| r.bed == "12").unwrap();
    assert!(recordings::remove(&mut c, &f.cfg, &viewer, &[named.id], true).is_err());
    assert_eq!(recordings::remove(&mut c, &f.cfg, &a, &[named.id], true).unwrap(), 1);
    assert!(Path::new(&named.path).join("rec.dat").is_file());
    assert!(!f.cfg.index_dir.join(&named.fingerprint).exists());
    let (total, _) = recordings::list(&c, &admin(), "", false, 100, 0, "UTC").unwrap();
    assert_eq!(total, 2);
    assert_eq!(linkage::remove_encounters(&mut c, &a, &[e2.id], false).unwrap().encounters, 1);
    assert_eq!(query::search(&c, &viewer, &Search::default(), "UTC").unwrap().0, 2);
    let (unlinked, _) = recordings::list(&c, &admin(), "", true, 100, 0, "UTC").unwrap();
    assert_eq!(unlinked, 2);
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn check_stale_missing_and_auto_index() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    recordings::import(&mut c, &f.cfg, &admin(), &[f.data.to_string_lossy().to_string()], true, &opts("test")).unwrap();
    // indexing and reading identifiers, for each of the 3 recordings
    assert_eq!(jobs::queue_auto_index(&c).unwrap(), 6);
    assert_eq!(jobs::queue_auto_index(&c).unwrap(), 0, "queued once");
    drop(c);
    run_all_jobs(&f);
    let c = f.db.get().unwrap();
    // change one recording, delete another
    let one = f.data.join(bed_name("ICU7", "12", T0));
    std::thread::sleep(std::time::Duration::from_millis(20));
    wfdb_record(&one, 60);
    std::fs::remove_dir_all(f.data.join("unnamed")).unwrap();
    let r = recordings::check(&c, &f.cfg).unwrap();
    assert_eq!((r.checked, r.missing, r.changed), (3, 1, 1));
    let (_, recs) = recordings::list(&c, &admin(), "", false, 100, 0, "UTC").unwrap();
    let st = |name: &str| recs.iter().find(|r| r.name == name).unwrap().status;
    assert_eq!(st(&bed_name("ICU7", "12", T0)), Status::Stale);
    assert_eq!(st("unnamed"), Status::Missing);
    // the changed recording: indexed and read again
    assert_eq!(jobs::queue_auto_index(&c).unwrap(), 2);
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn refuses_to_write_into_recording_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let rec = root.join(bed_name("ICU7", "12", T0));
    wfdb_record(&rec, 0);
    // results inside the WFDB folder, or in a folder that holds it: refused
    for results in [rec.join("results"), root.clone()] {
        let cfg = common::config(&root.join("home"), &results);
        let db = Db::open(&cfg, true).unwrap();
        let mut c = db.get().unwrap();
        let r = recordings::import(&mut c, &cfg, &admin(), &[root.to_string_lossy().to_string()], true, &opts("test"))
            .unwrap();
        assert!(r.skipped.iter().any(|(_, why)| why.contains("writes to")), "{results:?}: {r:?}");
    }
}

fn python() -> Option<&'static str> {
    ["python3", "python"]
        .into_iter()
        .find(|p| std::process::Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
}

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn plug_in_actions() {
    let Some(py) = python() else {
        eprintln!("no Python: plug-in test skipped");
        return;
    };
    let mut f = fixture();
    let actions = f.cfg.home.join("actions");
    for (name, script, requires) in [
        ("count", "import json,sys\nj=json.load(sys.stdin)\nprint('progress 0.5')\nopen('out.txt','w').write(j['recording']['name'])\njson.dump({'n': j['params']['n'], 'enc': [e['encounter_id'] for e in j['encounters']], 'idx': j['recording']['index_dir'] is not None}, open('summary.json','w'))\n", "[\"index\"]"),
        ("broken", "import sys\nprint('bad input', file=sys.stderr)\nsys.exit(3)\n", "[]"),
        ("after-broken", "pass\n", "[\"broken\"]"),
    ] {
        let d = actions.join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("run.py"), script).unwrap();
        std::fs::write(
            d.join("action.toml"),
            format!("name = \"{name}\"\nversion = 2\nrequires = {requires}\ncommand = [\"{py}\", \"run.py\"]\n[params]\nn = 1\n"),
        )
        .unwrap();
    }
    let d = actions.join("later");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("action.toml"), "name = \"later\"\nscope = \"encounter\"\ncommand = [\"x\"]\n").unwrap();
    f.cfg.actions_dirs = vec![actions];
    let reg = Registry::load(&f.cfg);
    assert!(reg.problems.is_empty(), "{:?}", reg.problems);
    assert!(!reg.get("later").unwrap().problem.is_empty(), "encounter scope reserved");

    let mut c = f.db.get().unwrap();
    let a = admin();
    recordings::import(&mut c, &f.cfg, &a, &[f.data.to_string_lossy().to_string()], true, &opts("test")).unwrap();
    table(&f, &mut c, "l.csv", LINKAGE);
    let e1: i64 = c.query_row("SELECT id FROM encounters WHERE encounter_id = 'E1'", [], |r| r.get(0)).unwrap();
    let params = [("count".to_string(), serde_json::json!({"n": 5}))].into_iter().collect();
    let sel = Selection { encounters: Some(vec![e1]), ..Default::default() };
    // index is added because count requires it
    let r = jobs::create_run(&mut c, &reg, &a, &["count".into()], &params, &sel, false, 0, "UTC").unwrap();
    assert_eq!(r.queued, 2);
    let viewer = user(&c, "vic", Role::Viewer, false);
    assert!(jobs::create_run(&mut c, &reg, &viewer, &["count".into()], &params, &sel, false, 0, "UTC").is_err());
    assert!(jobs::create_run(&mut c, &reg, &a, &["later".into()], &params, &sel, false, 0, "UTC").is_err());
    jobs::create_run(
        &mut c,
        &reg,
        &a,
        &["after-broken".into()],
        &params,
        &Selection { all: true, ..Default::default() },
        false,
        0,
        "UTC",
    )
    .unwrap();
    drop(c);
    let reg = Arc::new(RwLock::new(reg));
    jobs::work(
        Arc::new(Db::open(&f.cfg, false).unwrap()),
        Arc::new(f.cfg.clone()),
        reg,
        0,
        Arc::new(AtomicBool::new(false)),
        true,
    );
    let c = f.db.get().unwrap();
    let rec: i64 =
        c.query_row("SELECT recording FROM recording_links WHERE encounter = ?1", [e1], |r| r.get(0)).unwrap();
    let res = query::results(&c, rec).unwrap();
    let count = res.iter().find(|x| x.action == "count").unwrap();
    assert_eq!(count.state, "done", "{}", count.error);
    // plug-ins get the encounter's pseudonym, never its imported ID (which may be an account number)
    let pseudonym: String = c.query_row("SELECT pseudonym FROM encounters WHERE id = ?1", [e1], |r| r.get(0)).unwrap();
    assert!(pseudonym.starts_with("MO-E-"));
    assert_eq!(count.summary, serde_json::json!({"n": 5, "enc": [pseudonym], "idx": true}));
    assert!(Path::new(&count.output_dir).join("out.txt").is_file());
    assert!(Path::new(&count.output_dir).starts_with(f.cfg.results_dir.join("actions").join("count")));
    let broken = res.iter().find(|x| x.action == "broken").unwrap();
    assert_eq!(broken.state, "failed");
    assert!(broken.error.contains("bad input"), "{}", broken.error);
    let after = res.iter().find(|x| x.action == "after-broken").unwrap();
    assert_eq!((after.state.as_str(), after.error.as_str()), ("failed", "requires broken, which failed"));
}

#[test]
fn users_and_sessions() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    assert!(users::add(&c, "x", "", Role::Viewer, false, "short").is_err());
    let adm = user(&c, "boss", Role::Admin, true);
    assert!(users::authenticate(&c, "boss", "0123456789ab").unwrap().is_some());
    assert!(users::authenticate(&c, "boss", "wrong-password").unwrap().is_none());
    assert!(users::authenticate(&c, "nobody", "0123456789ab").unwrap().is_none());
    let token = users::new_session(&c, &adm, 1).unwrap();
    assert_eq!(users::session_user(&c, &token, 0, true).unwrap().unwrap().username, "boss");
    // the last admin cannot be demoted, disabled or deleted
    let ch = users::Changes { display_name: None, role: Some(Role::Viewer), phi: None, disabled: None, password: None };
    assert!(users::update(&mut c, adm.id, &ch).is_err());
    assert!(users::delete(&mut c, adm.id).is_err());
    // a new password ends the sessions
    let ch = users::Changes {
        display_name: None,
        role: None,
        phi: None,
        disabled: None,
        password: Some("another-password"),
    };
    users::update(&mut c, adm.id, &ch).unwrap();
    assert!(users::session_user(&c, &token, 0, true).unwrap().is_none());

    // a session unused for longer than the idle limit ends; the page's own checks are not use
    let token = users::new_session(&c, &adm, 12).unwrap();
    let hash: String = c.query_row("SELECT token_hash FROM sessions", [], |r| r.get(0)).unwrap();
    let seen = |c: &medorama::sql::Connection| -> f64 {
        c.query_row("SELECT COALESCE(last_seen, created_at) FROM sessions WHERE token_hash = ?1", [&hash], |r| r.get(0))
            .unwrap()
    };
    let ago = |c: &medorama::sql::Connection, s: f64| {
        c.execute("UPDATE sessions SET last_seen = ?1 WHERE token_hash = ?2", medorama::sql::params![now() - s, hash])
            .unwrap();
    };
    ago(&c, 90.0);
    assert!(users::session_user(&c, &token, 120, false).unwrap().is_some(), "90 s idle, limit 120 s: still on");
    assert!(now() - seen(&c) > 89.0, "a background request does not count as use");
    assert!(users::session_user(&c, &token, 120, true).unwrap().is_some());
    assert!(now() - seen(&c) < 5.0, "the user's request does");
    ago(&c, 130.0);
    assert!(users::session_user(&c, &token, 120, true).unwrap().is_none(), "130 s idle: ended");
    let left: i64 = c.query_row("SELECT COUNT(*) FROM sessions WHERE token_hash = ?1", [&hash], |r| r.get(0)).unwrap();
    assert_eq!(left, 0, "and removed");
    // no idle limit (0): only the session's own end
    let token = users::new_session(&c, &adm, 12).unwrap();
    c.execute("UPDATE sessions SET last_seen = ?1", [now() - 86_000.0]).unwrap();
    assert!(users::session_user(&c, &token, 0, true).unwrap().is_some());
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
    header: bool,
) -> (StatusCode, serde_json::Value, Option<String>) {
    let mut req = Request::builder().method(method).uri(uri);
    if header {
        req = req.header("X-Medorama", "1");
    }
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    let req = match body {
        Some(b) => req.header("Content-Type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let set_cookie =
        resp.headers().get("set-cookie").map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null), set_cookie)
}

#[tokio::test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn http_permissions() {
    let f = fixture();
    {
        let mut c = f.db.get().unwrap();
        user(&c, "boss", Role::Admin, true);
        user(&c, "vic", Role::Viewer, false);
        recordings::import(&mut c, &f.cfg, &admin(), &[f.data.to_string_lossy().to_string()], true, &opts("test"))
            .unwrap();
        table(&f, &mut c, "l.csv", LINKAGE);
    }
    let app =
        server::router(Arc::new(server::App::new(Arc::new(f.cfg.clone()), Arc::new(Db::open(&f.cfg, false).unwrap()))));
    let api = "/api/v1";
    let login = |u: &str| serde_json::json!({"username": u, "password": "0123456789ab"});

    assert_eq!(call(&app, "GET", &format!("{api}/encounters"), None, None, false).await.0, StatusCode::UNAUTHORIZED);
    // changes need the X-Medorama header (cross-site requests cannot add it)
    assert_eq!(
        call(&app, "POST", &format!("{api}/login"), None, Some(login("vic")), false).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _, vic) = call(&app, "POST", &format!("{api}/login"), None, Some(login("vic")), true).await;
    assert_eq!(s, StatusCode::OK);
    let vic = vic.unwrap();
    let (_, _, boss) = call(&app, "POST", &format!("{api}/login"), None, Some(login("boss")), true).await;
    let boss = boss.unwrap();

    let (s, v, _) = call(&app, "GET", &format!("{api}/encounters"), Some(&vic), None, false).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["total"], 3);
    assert!(v["items"].as_array().unwrap().iter().all(|e| e.get("phi").is_none()));
    assert!(!v.to_string().contains("MRN001") && !v.to_string().contains("Doe"));
    let (_, v, _) = call(&app, "GET", &format!("{api}/encounters?q=doe"), Some(&boss), None, false).await;
    assert_eq!(v["items"][0]["phi"]["mrn"], "MRN001");

    // files come in only as rows of a database, with their patient: there is no folder import,
    // not even for an admin
    let imp = serde_json::json!({"paths": [f.data.to_string_lossy()]});
    let (s, _, _) = call(&app, "POST", &format!("{api}/recordings/import"), Some(&boss), Some(imp), true).await;
    assert!(s == StatusCode::NOT_FOUND || s == StatusCode::METHOD_NOT_ALLOWED, "{s}");
    let (s, _, _) = call(&app, "GET", &format!("{api}/browse"), Some(&boss), None, false).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let run = serde_json::json!({"actions": ["index"], "selection": {"all": true}});
    assert_eq!(
        call(&app, "POST", &format!("{api}/runs"), Some(&vic), Some(run.clone()), true).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(call(&app, "GET", &format!("{api}/users"), Some(&vic), None, false).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "GET", &format!("{api}/audit"), Some(&vic), None, false).await.0, StatusCode::FORBIDDEN);
    let (s, v, _) = call(&app, "POST", &format!("{api}/runs"), Some(&boss), Some(run), true).await;
    assert_eq!((s, v["queued"].clone()), (StatusCode::OK, serde_json::json!(3)));

    // history: own, removable
    let (_, v, _) = call(&app, "GET", &format!("{api}/encounters"), Some(&vic), None, false).await;
    let id = v["items"][0]["id"].as_i64().unwrap();
    call(&app, "GET", &format!("{api}/encounters/{id}"), Some(&vic), None, false).await;
    let (_, v, _) = call(&app, "GET", &format!("{api}/history"), Some(&vic), None, false).await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    call(&app, "DELETE", &format!("{api}/history/{id}"), Some(&vic), None, true).await;
    let (_, v, _) = call(&app, "GET", &format!("{api}/history"), Some(&vic), None, false).await;
    assert!(v["items"].as_array().unwrap().is_empty());

    // sign-out ends the session
    call(&app, "POST", &format!("{api}/logout"), Some(&vic), None, true).await;
    assert_eq!(call(&app, "GET", &format!("{api}/me"), Some(&vic), None, false).await.0, StatusCode::UNAUTHORIZED);

    // signing out everywhere ends every session of the user, and only theirs
    let (_, _, a) = call(&app, "POST", &format!("{api}/login"), None, Some(login("vic")), true).await;
    let (_, _, b) = call(&app, "POST", &format!("{api}/login"), None, Some(login("vic")), true).await;
    let (a, b) = (a.unwrap(), b.unwrap());
    let out = serde_json::json!({"everywhere": true});
    assert_eq!(call(&app, "POST", &format!("{api}/logout"), Some(&a), Some(out), true).await.0, StatusCode::OK);
    for s in [&a, &b] {
        assert_eq!(call(&app, "GET", &format!("{api}/me"), Some(s), None, false).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(call(&app, "GET", &format!("{api}/me"), Some(&boss), None, false).await.0, StatusCode::OK);
    // an admin signs a user out everywhere; a viewer cannot
    let (_, _, v2) = call(&app, "POST", &format!("{api}/login"), None, Some(login("vic")), true).await;
    let v2 = v2.unwrap();
    let (_, me, _) = call(&app, "GET", &format!("{api}/me"), Some(&v2), None, false).await;
    let vid = me["user"]["id"].as_i64().or(me["id"].as_i64()).expect("the user's id");
    let off = serde_json::json!({"sign_out": true});
    let (s, _, _) = call(&app, "PATCH", &format!("{api}/users/{vid}"), Some(&v2), Some(off.clone()), true).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(&app, "PATCH", &format!("{api}/users/{vid}"), Some(&boss), Some(off), true).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(call(&app, "GET", &format!("{api}/me"), Some(&v2), None, false).await.0, StatusCode::UNAUTHORIZED);
    let (_, v, _) = call(&app, "GET", &format!("{api}/audit?limit=50"), Some(&boss), None, false).await;
    let audit = v.to_string();
    assert!(audit.contains("signed_out_everywhere") && audit.contains("signed out everywhere"), "{audit}");
}

/// The removal routes do what their fields say: a dry run changes nothing, `purge` deletes the
/// results, `q` and `unlinked` choose the recordings, and a field a route does not read is
/// refused rather than ignored.
#[tokio::test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn http_removals_honour_their_fields() {
    let f = fixture();
    {
        let mut c = f.db.get().unwrap();
        user(&c, "boss", Role::Admin, true);
        user(&c, "ann", Role::Admin, false);
        user(&c, "vic", Role::Viewer, false);
        recordings::import(&mut c, &f.cfg, &admin(), &[f.data.to_string_lossy().to_string()], true, &opts("test"))
            .unwrap();
        table(&f, &mut c, "l.csv", LINKAGE);
        let reg = Registry::load(&f.cfg);
        let sel = Selection { all: true, ..Default::default() };
        jobs::create_run(&mut c, &reg, &admin(), &["index".into()], &Default::default(), &sel, false, 0, "UTC")
            .unwrap();
    }
    run_all_jobs(&f);
    let app =
        server::router(Arc::new(server::App::new(Arc::new(f.cfg.clone()), Arc::new(Db::open(&f.cfg, false).unwrap()))));
    let api = "/api/v1";
    let login = |u: &str| serde_json::json!({"username": u, "password": "0123456789ab"});
    let boss = call(&app, "POST", &format!("{api}/login"), None, Some(login("boss")), true).await.2.unwrap();
    let ann = call(&app, "POST", &format!("{api}/login"), None, Some(login("ann")), true).await.2.unwrap();
    let vic = call(&app, "POST", &format!("{api}/login"), None, Some(login("vic")), true).await.2.unwrap();
    let post = |uri: &str, body: serde_json::Value| {
        let (app, uri, boss) = (app.clone(), format!("{api}/{uri}"), boss.clone());
        async move { call(&app, "POST", &uri, Some(&boss), Some(body), true).await }
    };
    let total = |uri: &str| {
        let (app, uri, boss) = (app.clone(), format!("{api}/{uri}"), boss.clone());
        async move { call(&app, "GET", &uri, Some(&boss), None, false).await.1["total"].as_i64().unwrap() }
    };
    let index_dirs = || -> Vec<PathBuf> {
        let c = f.db.get().unwrap();
        let mut st = c.prepare("SELECT output_dir FROM jobs WHERE action = 'index' AND output_dir != ''").unwrap();
        st.query_map([], |r| r.get::<_, String>(0)).unwrap().map(|d| PathBuf::from(d.unwrap())).collect()
    };
    assert_eq!((total("recordings").await, total("encounters").await), (3, 3));
    assert_eq!(index_dirs().len(), 3);
    assert!(index_dirs().iter().all(|d| d.is_dir()));
    let (_, v, _) = call(&app, "GET", &format!("{api}/recordings?q=unnamed"), Some(&boss), None, false).await;
    let unnamed = v["items"][0]["id"].as_i64().unwrap();

    // recordings/remove: a dry run counts and changes nothing, purge included
    let (s, v, _) =
        post("recordings/remove", serde_json::json!({"ids": [unnamed, unnamed, 9999], "purge": true, "dry_run": true}))
            .await;
    assert_eq!((s, v["dry_run"].clone(), v["removed"].clone()), (StatusCode::OK, true.into(), 1.into()));
    assert_eq!(total("recordings").await, 3);
    assert!(index_dirs().iter().all(|d| d.is_dir()), "a dry run purges nothing");

    // recordings/remove-matching: q and unlinked choose, a dry run changes nothing
    let (_, v, _) = post("recordings/remove-matching", serde_json::json!({"q": "unnamed", "dry_run": true})).await;
    assert_eq!((v["dry_run"].clone(), v["recordings"].clone()), (true.into(), 1.into()));
    let (_, v, _) = post("recordings/remove-matching", serde_json::json!({"unlinked": true, "dry_run": true})).await;
    assert_eq!(v["recordings"], 0, "every recording is linked");
    let (_, v, _) = post("recordings/remove-matching", serde_json::json!({"dry_run": true})).await;
    assert_eq!(v["recordings"], 3);
    assert_eq!(total("recordings").await, 3);
    // a field the route does not read is refused, not ignored
    let (s, _, _) =
        post("recordings/remove-matching", serde_json::json!({"search": {"q": "unnamed"}, "dry_run": true})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // without purge the results stay; with it they go
    let before = index_dirs();
    let (s, v, _) = post("recordings/remove-matching", serde_json::json!({"q": "unnamed"})).await;
    assert_eq!((s, v["recordings"].clone()), (StatusCode::OK, 1.into()));
    assert_eq!(total("recordings").await, 2);
    assert!(before.iter().all(|d| d.is_dir()), "removed without purge: results kept");
    let (_, v, _) = call(&app, "GET", &format!("{api}/recordings"), Some(&boss), None, false).await;
    let rest: Vec<i64> = v["items"].as_array().unwrap().iter().map(|r| r["id"].as_i64().unwrap()).collect();
    let (s, v, _) = post("recordings/remove", serde_json::json!({"ids": [rest[0]], "purge": true})).await;
    assert_eq!((s, v["dry_run"].clone(), v["removed"].clone()), (StatusCode::OK, false.into(), 1.into()));
    assert_eq!(total("recordings").await, 1);
    assert_eq!(before.iter().filter(|d| !d.exists()).count(), 1, "purge deletes the removed recording's results");

    // encounters/remove: a dry run counts and changes nothing; purge is not one of its fields
    let (_, v, _) = call(&app, "GET", &format!("{api}/encounters?q=encounter:E2"), Some(&boss), None, false).await;
    let e2 = v["items"][0]["id"].as_i64().unwrap();
    let (s, v, _) = post("encounters/remove", serde_json::json!({"ids": [e2], "dry_run": true})).await;
    assert_eq!((s, v["removed"].clone(), v["report"]["dry_run"].clone()), (StatusCode::OK, 1.into(), true.into()));
    assert_eq!(total("encounters").await, 3);
    let (s, _, _) = post("encounters/remove", serde_json::json!({"ids": [e2], "purge": true})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(total("encounters").await, 3);

    // encounters/remove-matching: its search chooses; q, unlinked and purge are refused (an
    // ignored q would have removed every encounter)
    let (s, v, _) =
        post("encounters/remove-matching", serde_json::json!({"search": {"q": "encounter:E2"}, "dry_run": true})).await;
    assert_eq!((s, v["encounters"].clone(), v["dry_run"].clone()), (StatusCode::OK, 1.into(), true.into()));
    assert_eq!(total("encounters").await, 3);
    for extra in [
        serde_json::json!({"q": "encounter:E2"}),
        serde_json::json!({"unlinked": true}),
        serde_json::json!({"purge": true}),
    ] {
        let (s, _, _) = post("encounters/remove-matching", extra.clone()).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{extra}");
        assert_eq!(total("encounters").await, 3, "{extra}");
    }
    let (s, v, _) = post("encounters/remove-matching", serde_json::json!({"search": {"q": "encounter:E2"}})).await;
    assert_eq!((s, v["encounters"].clone()), (StatusCode::OK, 1.into()));
    assert_eq!(total("encounters").await, 2);

    // role refusals are 403, not 400
    let put = |uri: &str, who: &str, body: serde_json::Value| {
        let (app, uri, who) = (app.clone(), format!("{api}/{uri}"), who.to_string());
        async move { call(&app, "PUT", &uri, Some(&who), Some(body), true).await.0 }
    };
    // the settings as an admin reads them (others may not: they hold the date offset)
    assert_eq!(call(&app, "GET", &format!("{api}/deid"), Some(&vic), None, false).await.0, StatusCode::FORBIDDEN);
    let deid = call(&app, "GET", &format!("{api}/deid"), Some(&boss), None, false).await.1;
    let settings =
        serde_json::json!({"fields": deid["fields"], "offset": deid["offset"], "spread_days": deid["spread_days"]});
    assert_eq!(put("admin/deid", &vic, settings).await, StatusCode::FORBIDDEN);
    assert_eq!(
        put("admin/display", &vic, serde_json::json!({"date": "ymd", "clock": "24h"})).await,
        StatusCode::FORBIDDEN
    );
    let pair = serde_json::json!({"a": "P1", "b": "P2"});
    for route in ["review/merge", "review/distinct"] {
        for who in [&vic, &ann] {
            let s = call(&app, "POST", &format!("{api}/{route}"), Some(who), Some(pair.clone()), true).await.0;
            assert_eq!(s, StatusCode::FORBIDDEN, "{route}");
        }
    }
    let s = call(&app, "POST", &format!("{api}/jobs/cancel"), Some(&vic), Some(serde_json::json!({"job": 1})), true)
        .await
        .0;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[cfg(feature = "encryption")]
#[test]
fn encrypted_database() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = common::config(&tmp.path().join("home"), &tmp.path().join("results"));
    std::fs::create_dir_all(&cfg.home).unwrap();
    let key = cfg.home.join("db.key");
    std::fs::write(&key, "a long random passphrase\n").unwrap();
    cfg.key_file = Some(key.clone());
    {
        let db = Db::open(&cfg, true).unwrap();
        user(&db.get().unwrap(), "boss", Role::Admin, true);
    }
    // not readable as a plain SQLite file
    let head = std::fs::read(&cfg.database).unwrap();
    assert_ne!(&head[..15], b"SQLite format 3");
    // the right key opens it, a wrong one does not
    assert!(users::get_by_name(&Db::open(&cfg, false).unwrap().get().unwrap(), "boss").unwrap().is_some());
    std::fs::write(&key, "wrong").unwrap();
    assert!(Db::open(&cfg, false).is_err());
}

/// A large table imported by `medorama import-table` in a process of its own, stopped by
/// SIGKILL part way: while it runs, other writes (an audited view) go through between its
/// chunks; after it, nothing of it is left once the catalog starts again.
#[test]
fn an_import_killed_part_way_leaves_nothing() {
    if common::on_postgres() || cfg!(windows) {
        return; // (PostgreSQL imports in one transaction; SIGKILL is Unix's)
    }
    let f = fixture();
    let home = f.cfg.home.clone();
    std::fs::write(home.join(medorama::config::CONFIG_NAME), f.cfg.to_toml()).unwrap();
    let csv = f.data.join("big.csv");
    let mut w = std::io::BufWriter::new(std::fs::File::create(&csv).unwrap());
    use std::io::Write;
    writeln!(w, "patient_id,encounter_id,admit_time,name,value,unit,time").unwrap();
    for i in 0..400_000 {
        writeln!(w, "P{},E{},2024-01-01 00:00,Lactate,{}.1,mmol/L,2024-01-01 0{}:00", i / 20, i / 20, i % 9, i % 10)
            .unwrap();
    }
    drop(w);
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_medorama"))
        .arg("--home")
        .arg(&home)
        .arg("import-table")
        .arg(&csv)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let c = f.db.get().unwrap();
    let count = |sql: &str| -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() };
    // its first chunks committed, the batch still loading
    let until = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while count("SELECT COUNT(*) FROM ehr_items") == 0 {
        assert!(std::time::Instant::now() < until && child.try_wait().unwrap().is_none(), "the import did not start");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(count("SELECT COUNT(*) FROM imports WHERE state = 'loading'"), 1);
    // an audited write meanwhile: written at once, not after the import (nor lost)
    let t = std::time::Instant::now();
    medorama::db::audit(&c, Some(&User::local()), "view", "E1", "");
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "the audit waited {:?}", t.elapsed());
    assert_eq!(count("SELECT COUNT(*) FROM audit WHERE event = 'view' AND target = 'E1'"), 1);
    assert!(child.try_wait().unwrap().is_none(), "the import is still running");
    child.kill().unwrap(); // SIGKILL: a crash
    child.wait().unwrap();
    assert!(count("SELECT COUNT(*) FROM patients") > 0, "chunks were committed");
    // the next start: the import left loading is removed, as a whole
    drop(c);
    let mut c = f.db.get().unwrap();
    assert_eq!(medorama::imports::discard_unfinished(&mut c, &f.cfg, false).unwrap(), 1);
    for t in ["patients", "encounters", "ehr_items", "import_rows", "provenance", "field_values"] {
        let n: i64 = c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "{t}");
    }
    let state: String = c.query_row("SELECT state FROM imports", [], |r| r.get(0)).unwrap();
    assert_eq!(state, "removed");
    assert!(!c.table_exists("dbcol_1").unwrap());
}

/// An import cancelled after its first chunks were committed keeps nothing of it.
#[test]
fn an_import_cancelled_part_way_keeps_nothing() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let n = |c: &medorama::sql::Connection, t: &str| -> i64 {
        c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap()
    };
    let p = medorama::progress::Progress::new();
    let mut opts = common::opts("cancelled");
    opts.progress = Some(p.clone());
    let before = n(&c, "ehr_items");
    let big: String = (0..20_000).fold(String::from("patient_id,encounter_id,name,value\n"), |mut s, i| {
        s += &format!("Q{},F{},Lactate,{i}\n", i / 10, i / 10);
        s
    });
    let p2 = p.clone();
    let watcher = std::thread::spawn(move || {
        while p2.view()["rows"].as_u64().unwrap_or(0) < 6000 {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        p2.cancel();
    });
    let r =
        tables::import(&mut c, &f.cfg, &User::local(), "big.csv", big.as_bytes(), &Default::default(), "", None, &opts);
    watcher.join().unwrap();
    assert!(r.unwrap_err().to_string().contains("nothing of it was kept"));
    assert_eq!(n(&c, "ehr_items"), before, "no item of it");
    assert_eq!(n(&c, "imports WHERE name = 'cancelled' AND state != 'removed'"), 0);
    assert_eq!(n(&c, "patients WHERE patient_id LIKE 'Q%'"), 0);
}

/// A worker stopping (the server or `medorama work` stopping) stops its plug-in and what the
/// plug-in started (its process group), and puts the job back in the queue; run again, the job
/// writes to a new folder.
#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn a_stopping_worker_stops_its_plug_in_and_requeues_the_job() {
    let Some(py) = python() else { return };
    if cfg!(windows) {
        return;
    }
    let mut f = fixture();
    let actions = f.cfg.home.join("actions");
    let d = actions.join("sleepy");
    std::fs::create_dir_all(&d).unwrap();
    let mark = f.cfg.home.join("quick");
    let pids = f.cfg.home.join("pids");
    std::fs::write(
        d.join("run.py"),
        format!(
            "import json,os,subprocess,sys,time\nif os.path.exists({mark:?}):\n    json.dump({{}}, open('summary.json','w')); sys.exit(0)\np = subprocess.Popen(['sleep', '60'])\nopen({pids:?},'w').write(str(p.pid))\ntime.sleep(60)\n"
        ),
    )
    .unwrap();
    std::fs::write(d.join("action.toml"), format!("name = \"sleepy\"\ncommand = [\"{py}\", \"run.py\"]\n")).unwrap();
    f.cfg.actions_dirs = vec![actions];
    let reg = Registry::load(&f.cfg);
    let mut c = f.db.get().unwrap();
    recordings::import(&mut c, &f.cfg, &admin(), &[f.data.to_string_lossy().to_string()], true, &opts("t")).unwrap();
    let rec: i64 = c.query_row("SELECT MIN(id) FROM recordings", [], |r| r.get(0)).unwrap();
    let sel = Selection { recordings: Some(vec![rec]), ..Default::default() };
    jobs::create_run(&mut c, &reg, &admin(), &["sleepy".into()], &Default::default(), &sel, false, 0, "UTC").unwrap();
    let job = jobs::claim(&mut c, &reg, &jobs::worker_name(0)).unwrap().unwrap();
    drop(c);
    let stop = AtomicBool::new(false);
    std::thread::scope(|s| {
        let worker = s.spawn(|| jobs::execute(&f.db, &f.cfg, &reg, &job, &jobs::worker_name(0), &stop));
        let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !pids.is_file() || std::fs::read_to_string(&pids).unwrap().is_empty() {
            assert!(std::time::Instant::now() < until, "the plug-in did not start");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        worker.join().unwrap().unwrap();
    });
    // what the plug-in started is stopped too
    let grandchild = std::fs::read_to_string(&pids).unwrap();
    let alive = std::process::Command::new("kill").args(["-0", grandchild.trim()]).status().unwrap().success();
    assert!(!alive, "the plug-in's own child was left running");
    let c = f.db.get().unwrap();
    let (state, worker): (String, String) =
        c.query_row("SELECT state, worker FROM jobs WHERE id = ?1", [job.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((state.as_str(), worker.as_str()), ("queued", ""), "back in the queue");
    drop(c);
    // run again (quickly now): in a new folder, the first attempt's log kept
    std::fs::write(&mark, "").unwrap();
    run_all_jobs(&f);
    let c = f.db.get().unwrap();
    let (state, out): (String, String) = c
        .query_row("SELECT state, output_dir FROM jobs WHERE id = ?1", [job.id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(state, "done");
    assert!(out.ends_with(&format!("j{}-2", job.id)), "{out}");
    assert!(Path::new(&out).with_file_name(format!("j{}", job.id)).join("log.txt").is_file());
}

/// An index a user cancelled is not started again on its own: the recording shows the
/// cancellation until it is indexed again.
#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn a_cancelled_index_is_not_queued_again() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    recordings::import(&mut c, &f.cfg, &admin(), &[f.data.to_string_lossy().to_string()], true, &opts("test")).unwrap();
    let n = jobs::queue_auto_index(&c).unwrap();
    assert!(n >= 1);
    assert_eq!(jobs::cancel(&c, &admin(), None, None).unwrap(), n);
    let (_, recs) = recordings::list(&c, &admin(), "", false, 100, 0, "UTC").unwrap();
    assert!(recs.iter().all(|r| r.status == Status::Failed && r.error.contains("cancelled")), "{recs:?}");
    jobs::queue_auto_index(&c).unwrap();
    let queued: i64 =
        c.query_row("SELECT COUNT(*) FROM jobs WHERE action = 'index' AND state = 'queued'", [], |r| r.get(0)).unwrap();
    assert_eq!(queued, 0, "the cancellation sticks");
}

/// A recording is added only where it really is inside a data folder: a symbolic link in one,
/// to a recording elsewhere, is not followed out.
#[cfg(unix)]
#[test]
fn a_link_out_of_the_data_roots_is_not_added() {
    let f = fixture();
    let outside = f._tmp.path().join("elsewhere");
    wfdb_record(&outside.join(bed_name("ICU9", "1", T0)), 0);
    std::os::unix::fs::symlink(outside.join(bed_name("ICU9", "1", T0)), f.data.join("link")).unwrap();
    let mut cfg = f.cfg.clone();
    cfg.data_roots = vec![f.data.to_string_lossy().to_string()];
    let mut c = f.db.get().unwrap();
    let rep = recordings::import(&mut c, &cfg, &admin(), &[f.data.to_string_lossy().to_string()], true, &opts("test"))
        .unwrap();
    assert!(
        rep.skipped.iter().any(|(p, why)| p.ends_with("link") && why.contains("symbolic link")),
        "{:?}",
        rep.skipped
    );
    let paths: Vec<String> = c
        .prepare("SELECT path FROM recordings")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(paths.iter().all(|p| !p.contains("elsewhere")), "{paths:?}");
}

/// Under Medorama, the recordings' readers open files inside data_roots only: a record whose
/// signal file is a symbolic link to a file outside them is not indexed (`medorama work`).
#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn a_signal_file_outside_data_roots_is_not_read() {
    if cfg!(windows) || common::on_postgres() {
        return;
    }
    let mut f = fixture();
    let inside = f.data.join("linked").join(bed_name("ICU9", "1", T0));
    wfdb_record(&inside, 0);
    let outside = f.cfg.home.join("outside.dat");
    std::fs::rename(inside.join("rec.dat"), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, inside.join("rec.dat")).unwrap();
    f.cfg.data_roots = vec![f.data.to_string_lossy().to_string()];
    // (the settings as written name no data_roots: given here)
    let toml = f.cfg.to_toml().replace("data_roots = []", &format!("data_roots = [{:?}]", f.cfg.data_roots[0]));
    assert!(toml.contains(&f.cfg.data_roots[0]));
    std::fs::write(f.cfg.home.join(medorama::config::CONFIG_NAME), toml).unwrap();
    let mut c = f.db.get().unwrap();
    recordings::import(&mut c, &f.cfg, &admin(), &[inside.to_string_lossy().to_string()], true, &opts("t")).unwrap();
    let rec: i64 = c.query_row("SELECT id FROM recordings", [], |r| r.get(0)).unwrap();
    let reg = Registry::load(&f.cfg);
    let sel = Selection { recordings: Some(vec![rec]), ..Default::default() };
    jobs::create_run(&mut c, &reg, &admin(), &["index".into()], &Default::default(), &sel, true, 0, "UTC").unwrap();
    drop(c);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_medorama"))
        .arg("--home")
        .arg(&f.cfg.home)
        .arg("work")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let c = f.db.get().unwrap();
    let state: String = c
        .query_row(
            "SELECT state FROM jobs WHERE recording = ?1 AND action = 'index' ORDER BY id DESC LIMIT 1",
            [rec],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "failed", "the file outside data_roots was read: {}", String::from_utf8_lossy(&out.stderr));
}

/// `medorama copy-database`: a SQLite catalog that imported tables, copied into PostgreSQL
/// (MEDORAMA_TEST_POSTGRES): every row, and each database's table of numbers built again there.
#[test]
fn a_catalog_with_databases_is_copied_to_postgres() {
    if !common::on_postgres() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    // the source: SQLite, whatever MEDORAMA_TEST_POSTGRES says
    let mut cfg = Config::new(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let from = Db::open(&cfg, true).unwrap();
    let mut c = from.get().unwrap();
    let mut csv = String::from("mrn,encounter_id,name,value,time\n");
    for i in 0..30_000 {
        csv += &format!("M{},E{},QTc,{},2024-01-01 10:00\n", i / 10, i / 10, 400 + i % 90);
    }
    let r = tables::import(&mut c, &cfg, &admin(), "ecg", csv.as_bytes(), &Default::default(), "", None, &opts("ecg"))
        .unwrap();
    tables::import(
        &mut c,
        &cfg,
        &admin(),
        "ids",
        b"patient_id,mrn\nP1,M1\n",
        &Default::default(),
        "",
        None,
        &opts("ids"),
    )
    .unwrap();
    assert!(c.table_exists(&format!("dbcol_{}", r.import_id)).unwrap());
    drop(c);
    let to_cfg = common::config(&root.join("pg"), &root.join("results"));
    let to = Db::open(&to_cfg, true).unwrap();
    let copied = medorama::db::copy_to_postgres(&from, &to).unwrap();
    assert!(copied.iter().all(|(t, _)| !t.starts_with("dbcol_")));
    let (a, b) = (from.get().unwrap(), to.get().unwrap());
    for t in
        ["patients", "encounters", "ehr_items", "import_rows", "provenance", "id_takeovers", "imports", "id_counters"]
    {
        let n = |c: &medorama::sql::Connection| -> i64 {
            c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap()
        };
        assert_eq!(n(&a), n(&b), "{t}");
    }
    let typed = format!("dbcol_{}", r.import_id);
    assert!(b.table_exists(&typed).unwrap(), "built again in PostgreSQL");
    let n: i64 = b.query_row(&format!("SELECT COUNT(*) FROM {typed}"), [], |r| r.get(0)).unwrap();
    assert_eq!(n, 30_000);
}

/// Several processes opening a new catalog at once: one creates the schema, the others wait
/// for it (no "table exists").
#[test]
fn a_new_catalog_opened_at_once_by_several() {
    if common::on_postgres() {
        return;
    }
    for _ in 0..5 {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::new(&tmp.path().join("home"), &tmp.path().join("results"));
        let opened: Vec<_> = (0..6)
            .map(|_| {
                let cfg = cfg.clone();
                std::thread::spawn(move || Db::open(&cfg, true).map(|_| ()).map_err(|e| e.to_string()))
            })
            .collect();
        for t in opened {
            t.join().unwrap().unwrap();
        }
    }
}

/// `medorama list` with a time no clock writes (one stored before times were checked).
#[test]
fn listing_an_encounter_of_an_absurd_time() {
    if common::on_postgres() {
        return; // (the command reads the settings written here: a SQLite catalog's)
    }
    let f = fixture();
    std::fs::write(f.cfg.home.join(medorama::config::CONFIG_NAME), f.cfg.to_toml()).unwrap();
    let mut c = f.db.get().unwrap();
    table(&f, &mut c, "adt", "patient_id,encounter_id\nP1,E1\n");
    c.execute("UPDATE encounters SET admit_time = 1e20", []).unwrap();
    drop(c);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_medorama"))
        .arg("--home")
        .arg(&f.cfg.home)
        .arg("list")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("E1"));
}

/// The PostgreSQL client's limits (MEDORAMA_TEST_POSTGRES): a statement's time limit unless the
/// URL sets one; a lock taken for some work given back however the work ends, a panic included.
#[test]
fn postgresql_connections_have_limits() {
    if !common::on_postgres() {
        return;
    }
    let f = fixture();
    let c = f.db.get().unwrap();
    let limit: String = c.query_row("SELECT current_setting('statement_timeout')", [], |r| r.get(0)).unwrap();
    assert_eq!(limit, "5min");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = c.exclusive(medorama::sql::Lock::Audit, || panic!("the work fails"));
    }));
    assert!(panicked.is_err());
    // another session takes the lock at once
    let other = f.db.get().unwrap();
    let free: bool =
        other.query_row("SELECT pg_try_advisory_lock(?1)", [medorama::sql::Lock::Audit as i64], |r| r.get(0)).unwrap();
    assert!(free, "the lock was left held");
    let _: bool =
        other.query_row("SELECT pg_advisory_unlock(?1)", [medorama::sql::Lock::Audit as i64], |r| r.get(0)).unwrap();
}
