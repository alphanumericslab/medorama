//! Imports and sources, matching by MRN, the search syntax, identifiers in recordings
//! (docs/administration.md, "Imports and sources", "Matching", "Searching", "Identifiers in
//! recordings").

mod common;
use common::{bed_name, opts, wfdb_record, T0};
use medorama::config::Config;
use medorama::db::Db;
use medorama::imports::{self, Opts};
use medorama::query::{self, Search};
use medorama::users::{self, Role, User};
use medorama::{ehr, identify, linkage, recordings, tables};
use std::path::{Path, PathBuf};

struct Fx {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let data = root.join("data");
    wfdb_record(&data.join("a").join(bed_name("ICU7", "12", T0)), 0);
    wfdb_record(&data.join("a").join(bed_name("ICU7", "14", T0)), 0);
    wfdb_record(&data.join("b").join(bed_name("ICU8", "3", T0)), 0);
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    Fx { _tmp: tmp, data, cfg, db }
}

/// A database imported as a table (`opts`: its name and source).
fn table(f: &Fx, c: &mut medorama::sql::Connection, name: &str, csv: &str, opts: &Opts) -> tables::TableReport {
    tables::import(c, &f.cfg, &admin(), name, csv.as_bytes(), &Default::default(), "", None, opts).unwrap()
}

fn admin() -> User {
    User::local()
}

fn s(q: &str) -> Search {
    Search { q: q.into(), ..Default::default() }
}

fn ids(c: &medorama::sql::Connection, u: &User, q: &str) -> Vec<String> {
    let (_, rows) = query::search(c, u, &s(q), "UTC").unwrap();
    let mut v: Vec<String> = rows.into_iter().map(|e| e.encounter_id).collect();
    v.sort();
    v
}

const LINKAGE_A: &str = "\
patient_id,mrn,last_name,first_name,birth_date,encounter_id,admit_time,discharge_time,unit,bed,bed_start,bed_end
P1,MRN001,Doe,Jane,1950-03-04,E1,2024-01-01 10:00,2024-01-03 10:00,ICU7,12,2024-01-01 11:00,2024-01-02 11:00
P2,MRN002,Roe,Rich,1961-03-05,E2,2024-01-01 08:00,2024-01-01 20:00,ICU7,14,2024-01-01 08:30,2024-01-01 19:00
";

#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn batches_and_sources() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let a = admin();
    let all = f.data.to_string_lossy().to_string();
    let sub = f.data.join("a").to_string_lossy().to_string();

    // a preview stores nothing
    let r = recordings::import(&mut c, &f.cfg, &a, std::slice::from_ref(&all), true, &Opts::new("all", "site-1", true))
        .unwrap();
    assert_eq!((r.added, r.dry_run), (3, true));
    assert_eq!(recordings::list(&c, &a, "", false, 10, 0, "UTC").unwrap().0, 0);

    // two sources share folder a's recordings
    recordings::import(&mut c, &f.cfg, &a, &[all], true, &Opts::new("all", "site-1", false)).unwrap();
    let r = recordings::import(&mut c, &f.cfg, &a, &[sub], true, &Opts::new("folder a", "Pilot B", false)).unwrap();
    assert_eq!((r.added, r.already), (0, 2));
    let srcs = imports::sources(&c).unwrap();
    let get = |n: &str| srcs.iter().find(|x| x.source == n).unwrap();
    assert_eq!((get("site-1").recordings, get("Pilot B").recordings), (3, 2));

    // search by source (case-insensitive, quoted), on recordings
    let (n, _) = recordings::list(&c, &a, r#"source:"pilot b""#, false, 10, 0, "UTC").unwrap();
    assert_eq!(n, 2);

    // removing site-1: folder a's recordings stay (Pilot B has them), b's goes
    let preview = imports::remove_source(&mut c, &f.cfg, &a, "site-1", false, true).unwrap();
    assert_eq!((preview.recordings, preview.kept_recordings), (1, 2));
    assert_eq!(recordings::list(&c, &a, "", false, 10, 0, "UTC").unwrap().0, 3, "a preview removes nothing");
    let done = imports::remove_source(&mut c, &f.cfg, &a, "site-1", false, false).unwrap();
    assert_eq!((done.recordings, done.kept_recordings), (1, 2));
    assert_eq!(recordings::list(&c, &a, "", false, 10, 0, "UTC").unwrap().0, 2);
    let batches = imports::list(&c).unwrap();
    assert_eq!(batches.iter().find(|b| b.name == "all").unwrap().state, "removed");
    // the folders themselves are untouched
    assert!(f.data.join("b").join(bed_name("ICU8", "3", T0)).join("rec.dat").is_file());

    // removing the other batch empties the catalog
    let b = batches.iter().find(|b| b.name == "folder a").unwrap().id;
    assert_eq!(imports::remove_import(&mut c, &f.cfg, &a, b, false, false).unwrap().recordings, 2);
    assert!(imports::remove_import(&mut c, &f.cfg, &a, b, false, false).is_err(), "removed already");
    assert_eq!(recordings::list(&c, &a, "", false, 10, 0, "UTC").unwrap().0, 0);
}

#[test]
fn matching_by_mrn_and_waiting_items() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let doc = {
        let id = users::add(&c, "doc", "", Role::Viewer, true, "0123456789ab").unwrap();
        users::get(&c, id).unwrap().unwrap()
    };
    // EHR first: nothing to match yet, so the items wait
    let ehr_csv = "\
mrn,domain,name,time,value,unit,source
MRN001,measurement,Lactate,2024-01-01 12:00,2.1,mmol/L,
MRN001,measurement,Lactate,2024-02-15 12:00,1.1,mmol/L,
MRN009,measurement,Sodium,2024-01-01 12:00,140,mmol/L,lab-2
,measurement,Sodium,2024-01-01 12:00,140,mmol/L,
";
    let r = table(&f, &mut c, "labs.csv", ehr_csv, &opts("labs"));
    assert_eq!((r.items_added, r.waiting), (3, 3), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("line 5")), "a row without any key is refused");
    assert_eq!(ehr::waiting_count(&c).unwrap(), 3);

    // linkage by MRN only: the catalog names the patient; the waiting items find their places
    let link = "\
mrn,last_name,encounter_id,admit_time,discharge_time,unit,bed,bed_start,bed_end
MRN001,Doe,E1,2024-01-01 10:00,2024-01-03 10:00,ICU7,12,2024-01-01 11:00,2024-01-02 11:00
";
    let r = table(&f, &mut c, "adt.csv", link, &opts("adt"));
    assert_eq!((r.patients_added, r.patients_named, r.ehr_matched), (1, 1, 2));
    let pid: String = c.query_row("SELECT patient_id FROM patients", [], |x| x.get(0)).unwrap();
    assert_eq!(pid, "GEN-000001");
    let e1: i64 = c.query_row("SELECT id FROM encounters WHERE encounter_id = 'E1'", [], |x| x.get(0)).unwrap();
    // in the encounter: the January item; with the patient only: February's
    assert_eq!(ehr::items(&c, e1, false, None, true, 10, 0).unwrap().0, 1);
    assert_eq!(ehr::items(&c, e1, true, None, true, 10, 0).unwrap().0, 1);
    assert_eq!(ehr::waiting_count(&c).unwrap(), 1, "MRN009 still waits");

    // a conflict: patient_id of one patient with the MRN of another
    let more = "\
patient_id,mrn,encounter_id
P7,MRN007,E7
P7,MRN001,E8
";
    let r = table(&f, &mut c, "more.csv", more, &opts("more"));
    assert_eq!(r.conflicts, 1, "{:?}", r.warnings);
    assert_eq!(ids(&c, &doc, ""), ["E1", "E7"]);

    // a per-row source wins over the import's
    let rows = "patient_id,encounter_id,source\nP8,E9,ward-x\n";
    table(&f, &mut c, "rows.csv", rows, &opts("rows"));
    assert_eq!(ids(&c, &doc, "source:ward-x"), ["E9"]);
    let item_sources: Vec<String> = {
        let mut st = c.prepare("SELECT DISTINCT source FROM ehr_items ORDER BY source").unwrap();
        st.query_map([], |x| x.get(0)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(item_sources, ["lab-2", "labs"]);
}

#[test]
fn search_syntax_and_removal_by_search() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let a = admin();
    let (doc, vic) = {
        let d = users::add(&c, "doc", "", Role::Viewer, true, "0123456789ab").unwrap();
        let v = users::add(&c, "vic", "", Role::Viewer, false, "0123456789ab").unwrap();
        (users::get(&c, d).unwrap().unwrap(), users::get(&c, v).unwrap().unwrap())
    };
    table(&f, &mut c, "a.csv", LINKAGE_A, &Opts::new("a", "Doe Pilot", false));
    // "doe" is both a name and a word of a source
    assert_eq!(ids(&c, &doc, "doe"), ["E1", "E2"]);
    assert_eq!(ids(&c, &doc, "name:doe"), ["E1"]);
    assert_eq!(ids(&c, &doc, "NAME:DOE dob:1950"), ["E1"]);
    assert_eq!(ids(&c, &doc, "dob:1961-03"), ["E2"]);
    assert_eq!(ids(&c, &doc, r#"source:"doe pilot" bed:14"#), ["E2"]);
    assert_eq!(ids(&c, &doc, "admit:2024-01-01 unit:icu7"), ["E1", "E2"]);
    // without the PHI permission: names neither found nor searchable by field
    assert_eq!(ids(&c, &vic, "doe"), ["E1", "E2"], "found by the source only");
    assert_eq!(ids(&c, &vic, "jane"), Vec::<String>::new());
    assert!(query::search(&c, &vic, &s("name:doe"), "UTC").unwrap_err().to_string().contains("PHI"));
    assert!(query::search(&c, &doc, &s("colour:red"), "UTC").unwrap_err().to_string().contains("unknown field"));

    // remove what a search matches: preview, then for real
    let hits = query::search_ids(&c, &doc, &s("bed:14"), "UTC").unwrap();
    let p = linkage::remove_encounters(&mut c, &a, &hits, true).unwrap();
    assert_eq!((p.encounters, p.patients, p.bed_stays), (1, 1, 1));
    assert_eq!(ids(&c, &doc, ""), ["E1", "E2"]);
    linkage::remove_encounters(&mut c, &a, &hits, false).unwrap();
    assert_eq!(ids(&c, &doc, ""), ["E1"]);
}

#[test]
fn identifiers_in_recordings() {
    // a recording states its patient's identifiers; its adapter reads them (docs/adapters.md)
    let f = fixture();
    if !common::fake_adapter(&f.data.join("adapters")) {
        return;
    }
    let rec = f.data.join("rec.fake");
    common::fake_recording(&rec, "Doe^Jane", "SYN0001");
    let (found, others) = identify::read(&rec, "fake").unwrap();
    let found = found.unwrap();
    assert_eq!((found.full_name.as_str(), found.patient_id.as_str(), others.len()), ("Doe^Jane", "SYN0001", 0));
    assert_eq!(identify::read(Path::new("."), "wfdb").unwrap().0, None, "WFDB folders hold none");

    // stored, shown to PHI users, compared with the database's
    let mut c = f.db.get().unwrap();
    let a = admin();
    recordings::import(&mut c, &f.cfg, &a, &[rec.to_string_lossy().to_string()], false, &opts("p")).unwrap();
    let id: i64 = c.query_row("SELECT id FROM recordings", [], |x| x.get(0)).unwrap();
    let summary = identify::identify(&c, id, &rec, "fake").unwrap();
    assert_eq!(summary["state"], "found");
    assert!(!summary.to_string().contains("SYN0001"), "no identifier in job summaries");
    let same = identify::in_file(&c, id, Some(("SYN0001", "", "", ""))).unwrap().unwrap();
    assert_eq!(same.mrn_matches, Some(true));
    let other = identify::in_file(&c, id, Some(("XYZ", "", "", ""))).unwrap().unwrap();
    assert_eq!(other.mrn_matches, Some(false));
    // searchable by file-id for PHI users only
    let doc = {
        let d = users::add(&c, "doc", "", Role::Viewer, true, "0123456789ab").unwrap();
        users::get(&c, d).unwrap().unwrap()
    };
    assert_eq!(recordings::list(&c, &doc, "file-id:syn0001", false, 10, 0, "UTC").unwrap().0, 1);
}

#[test]
fn addons_are_installed_listed_and_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let staged = tmp.path().join("download");
    if !common::fake_adapter_in(&staged) {
        return;
    }
    let dir = tmp.path().join("adapters");
    let installed = medorama::addons::install(&dir, &staged.join("fake-adapter")).unwrap();
    assert_eq!(installed["name"], "fake");
    assert!(dir.join("fake-adapter").is_file());
    let listed = medorama::addons::list(&dir);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["formats"][0]["extensions"][0], "fake");
    // installing again replaces it; a program not named <name>-adapter is refused
    medorama::addons::install(&dir, &staged.join("fake-adapter")).unwrap();
    assert_eq!(medorama::addons::list(&dir).len(), 1);
    // the installed copy itself, installed again: still there
    medorama::addons::install(&dir, &dir.join("fake-adapter")).unwrap();
    assert!(dir.join("fake-adapter").is_file());
    assert_eq!(medorama::addons::list(&dir).len(), 1);
    // a program that is no adapter: refused, the installed one as it was, nothing left beside it
    let broken = staged.join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("fake-adapter"), "#!/bin/sh\nexit 3\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(broken.join("fake-adapter"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(medorama::addons::install(&dir, &broken.join("fake-adapter")).is_err());
    assert_eq!(medorama::addons::list(&dir).len(), 1);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::copy(staged.join("fake-adapter"), staged.join("fake")).unwrap();
    assert!(medorama::addons::install(&dir, &staged.join("fake")).is_err());
    medorama::addons::remove(&dir, "fake").unwrap();
    assert!(medorama::addons::list(&dir).is_empty());
    assert!(medorama::addons::remove(&dir, "fake").is_err());
}
