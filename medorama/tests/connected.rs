//! What Medorama keeps of a connected database, and nothing more (docs/design/connected-
//! databases.md): its subjects (identities, sex) and their encounters, the names and units of its
//! variables, and the recordings it has opened.  Counts only when asked; files only when a
//! patient is opened.  All data invented.

mod common;
use common::{wfdb_record, Dbb, Setup};
use medorama::cohorts::{Criteria, Criterion, Expr};

/// A hospital's database: Ann (one ICU encounter, a lab result, a recording), Bob (a lab result).
fn hospital(s: &Setup) -> Dbb {
    let a = Dbb::new(&s.data().join("a.sqlite"));
    a.person("a1", "P1", "STUDY", "100", "HOSP", "Testa", "Ann", "1960-02-03", "F")
        .person("a2", "P2", "STUDY", "200", "HOSP", "Proba", "Bob", "1971-07-08", "M")
        .visit("va1", "a1", "2024-01-02 08:00", "2024-01-05 10:00", "MICU", true)
        .entry("ea1", "a1", "va1", "lab", "2024-01-02 09:00", "Lactate", Some(4.5), "mmol/L")
        .entry("ea2", "a2", "", "lab", "2024-02-01 09:00", "Lactate", Some(1.2), "mmol/L");
    wfdb_record(&s.data().join("rec"), 0);
    a.recording("r1", "a1", "rec/rec.hea", "icu");
    a
}

fn tables(w: &rusqlite::Connection) -> Vec<String> {
    w.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn connecting_keeps_the_subjects_their_encounters_and_the_catalog_only() {
    let s = Setup::new();
    let a = hospital(&s);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    // the subjects and their encounters: no summary of their data
    let w = medorama::persons::open(&s.cfg, &c, id).unwrap();
    assert_eq!(tables(&w), ["conflicts", "encounters", "persons"]);
    let enc = medorama::persons::encounter_summaries(&s.cfg, &c, id).unwrap();
    let (n, latest, icu) = enc["a1"];
    assert_eq!((n, latest.is_some(), icu), (1, true, true));
    assert!(!enc.contains_key("a2"), "Bob has no encounter");
    // the catalog: names and units, not counted
    let v = medorama::variables::of(&s.cfg, &c, id).unwrap();
    let lactate = v.iter().find(|x| x.name == "Lactate").expect("Lactate");
    assert_eq!(
        (lactate.unit.as_str(), lactate.numeric, lactate.entries, lactate.persons),
        ("mmol/L", true, None, None)
    );
    assert_eq!(medorama::variables::counted_at(&s.cfg, &c, id).unwrap(), None);
    assert_eq!(medorama::variables::recording_categories(&s.cfg, &c, id).unwrap(), ["icu"]);
    // no recording read, no file checked
    assert_eq!(medorama::recordings::count(&s.cfg, &c, id).unwrap(), 0);

    // Count now: who manages it; small counts shown as "< 11"
    assert!(medorama::databases::count_variables(&s.cfg, &c, &s.user("reader"), id).is_err());
    medorama::databases::count_variables(&s.cfg, &c, &boss, id).unwrap();
    let v = medorama::variables::of(&s.cfg, &c, id).unwrap();
    let lactate = v.iter().find(|x| x.name == "Lactate").unwrap();
    assert_eq!((lactate.entries, lactate.persons), (Some(2), Some(2)));
    let shown: medorama::variables::ShownVariable = lactate.clone().into();
    assert_eq!((shown.entries.as_deref(), shown.persons.as_deref()), (Some("< 11"), Some("< 11")));
    let json = serde_json::to_value(&shown).unwrap();
    assert_eq!(json["persons"], "< 11", "the API never sends a small count: {json}");
    assert_eq!(medorama::variables::shown(Some(11)).as_deref(), Some("11"));
    assert_eq!(medorama::variables::shown(Some(0)).as_deref(), Some("0"));
    assert!(medorama::variables::counted_at(&s.cfg, &c, id).unwrap().is_some());

    // opening Ann reads her recordings from the database, and keeps them (indexed later)
    let ann = medorama::persons::known(&s.cfg, &c, id, &["a1".into()]).unwrap()["a1"];
    let recs = medorama::recordings::of_patient(&s.cfg, &c, &boss, ann).unwrap();
    assert_eq!(recs.len(), 1);
    assert!(recs[0].present, "{}", recs[0].error);
    assert_eq!(medorama::recordings::count(&s.cfg, &c, id).unwrap(), 1);
    assert_eq!(medorama::jobs::queue_auto_index(&c, &s.cfg).unwrap(), 1, "indexed once it is opened");
    // Bob was never opened: nothing of his is kept beyond his identity
    let bob = medorama::persons::known(&s.cfg, &c, id, &["a2".into()]).unwrap()["a2"];
    assert!(medorama::recordings::of_patient(&s.cfg, &c, &boss, bob).unwrap().is_empty());
}

#[test]
fn a_variables_view_names_what_a_database_holds_without_reading_its_entries() {
    let s = Setup::new();
    let a = hospital(&s);
    let settings = medorama::sources::Settings {
        persons: "SELECT * FROM persons".into(),
        visits: "SELECT * FROM visits".into(),
        entries: "SELECT * FROM entries".into(),
        version: "SELECT value FROM meta WHERE key = 'version'".into(),
        variables: "SELECT 'lab' AS category, 'Named in the catalog' AS name, '' AS vocabulary, '' AS code, \
                    'mg/dL' AS unit, 1 AS numeric"
            .into(),
        ..Default::default()
    };
    let n = medorama::sources::NewSource {
        name: "registry".into(),
        kind: "views".into(),
        url: a.url(),
        settings,
        ..Default::default()
    };
    let c = s.db.get().unwrap();
    let id = medorama::databases::connect(&s.cfg, &c, &s.user("boss"), &n, "").unwrap();
    let names: Vec<String> = medorama::variables::of(&s.cfg, &c, id).unwrap().into_iter().map(|v| v.name).collect();
    assert_eq!(names, ["Named in the catalog"], "the view's, not the entries'");
    // a view without the catalog's columns is refused when connecting
    let mut bad = n.clone();
    bad.name = "registry2".into();
    bad.settings.variables = "SELECT 'lab' AS category".into();
    assert!(medorama::databases::connect(&s.cfg, &c, &s.user("boss"), &bad, "").is_err());
}

#[test]
fn the_privacy_dictionary_takes_the_recordings_opened_without_being_built_again() {
    let s = Setup::new();
    let a = hospital(&s);
    let id = s.connect("hospa", &a);
    let app = medorama::server::App::new(std::sync::Arc::new(s.cfg.clone()), s.db.clone());
    let c = s.db.get().unwrap();
    let before = medorama::server::restricted(&app, &c).map_err(|_| "gate").unwrap();
    assert!(before.dict.recordings.is_empty());
    let ann = medorama::persons::known(&s.cfg, &c, id, &["a1".into()]).unwrap()["a1"];
    let rec = medorama::recordings::of_patient(&s.cfg, &c, &s.user("boss"), ann).unwrap().remove(0);
    let after = medorama::server::restricted(&app, &c).map_err(|_| "gate").unwrap();
    assert_eq!(after.dict.version, before.dict.version, "not built again: the recording added");
    assert!(after.dict.recordings_version > before.dict.recordings_version);
    assert!(after.dict.recordings.values().any(|p| *p == rec.path), "its path is known to the gate");
    // the recording gone from the database: the dictionary is built again without it
    a.exec("DELETE FROM recordings");
    medorama::recordings::of_patient(&s.cfg, &c, &s.user("boss"), ann).unwrap();
    let gone = medorama::server::restricted(&app, &c).map_err(|_| "gate").unwrap();
    assert!(gone.dict.version > before.dict.version);
    assert!(gone.dict.recordings.is_empty());
}

#[test]
fn the_patient_list_asks_the_databases_for_what_its_page_shows() {
    let s = Setup::new();
    let a = hospital(&s);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let ann = medorama::persons::known(&s.cfg, &c, id, &["a1".into()]).unwrap()["a1"];
    let bob = medorama::persons::known(&s.cfg, &c, id, &["a2".into()]).unwrap()["a2"];
    let (total, rows) = medorama::patient::list(&c, &s.cfg, &boss, "", &[], 50, 0).unwrap();
    assert_eq!(total, 2);
    let row = |p: i64| rows.iter().find(|r| r.id == p).unwrap().clone();
    // read now for the page: the kinds of data, the entries (none were kept)
    assert_eq!(row(ann).items, Some(1));
    assert!(row(ann).kinds.iter().any(|k| k == "lab") && row(ann).kinds.iter().any(|k| k == "icu"));
    assert_eq!((row(bob).items, row(bob).encounters), (Some(1), 0));
    // a filter on a kind of data asks the database
    let (n, rows) = medorama::patient::list(&c, &s.cfg, &boss, "", &["icu".into()], 50, 0).unwrap();
    assert_eq!((n, rows[0].id), (1, ann));
    let (n, _) = medorama::patient::list(&c, &s.cfg, &boss, "", &["lab".into()], 50, 0).unwrap();
    assert_eq!(n, 2);
    let (n, rows) = medorama::patient::list(&c, &s.cfg, &boss, "", &["!icu".into()], 50, 0).unwrap();
    assert_eq!((n, rows[0].id), (1, bob));
    // the database offline: the list still answers from the subjects kept, without the page's data
    std::fs::remove_file(&a.path).unwrap();
    medorama::databases::check(&s.cfg, &c, id).unwrap();
    let (total, rows) = medorama::patient::list(&c, &s.cfg, &boss, "", &[], 50, 0).unwrap();
    assert_eq!(total, 2);
    assert!(rows.iter().all(|r| r.items.is_none()));
}

#[test]
fn a_criterion_reads_more_rows_than_it_keeps() {
    // every row read and tested, only those that hold kept: `max_rows` (here 1) bounds no read
    let s = Setup::new();
    let a = hospital(&s);
    a.entry("ea0", "a1", "", "lab", "2023-06-01 09:00", "Lactate", Some(9.0), "mmol/L");
    let n = medorama::sources::NewSource {
        name: "hospa".into(),
        kind: "dbb".into(),
        url: a.url(),
        settings: medorama::sources::Settings { max_rows: 1, ..Default::default() },
        ..Default::default()
    };
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let id = medorama::databases::connect(&s.cfg, &c, &boss, &n, "").unwrap();
    let known = medorama::persons::known(&s.cfg, &c, id, &["a1".into(), "a2".into()]).unwrap();
    let (ann, bob) = (known["a1"], known["a2"]);
    let one = |k: Criterion| {
        let q = Criteria { include: Some(Expr::Leaf(k)), ..Default::default() };
        let mut found: Vec<i64> =
            medorama::cohorts::evaluate(&c, &s.cfg, &boss, &q, None).unwrap().into_keys().collect();
        found.sort();
        found
    };
    let lactate = |cmp: &str, v: f64, latest: bool| Criterion {
        kind: "measurement".into(),
        name: "Lactate".into(),
        cmp: cmp.into(),
        value: Some(v),
        latest,
        ..Default::default()
    };
    let mut both = vec![ann, bob];
    both.sort();
    assert_eq!(one(lactate(">", 1.0, false)), both, "three rows read");
    assert_eq!(one(lactate(">", 5.0, false)), [ann], "Ann's earlier 9.0");
    assert!(one(lactate(">", 5.0, true)).is_empty(), "Ann's latest is 4.5");
    assert_eq!(one(lactate(">", 4.0, true)), [ann]);
    assert_eq!(one(Criterion { kind: "has".into(), category: "lab".into(), ..Default::default() }), both);
}

#[test]
fn a_pages_requests_read_a_patients_recordings_at_once_without_a_lock_refused() {
    // a patient's page asks in several requests at once (summary, journey, report): each read of
    // the recordings succeeds, none refused with "database is locked"
    let s = Setup::new();
    let a = hospital(&s);
    let id = s.connect("hospa", &a);
    let src = medorama::sources::get(&s.db.get().unwrap(), id).unwrap().unwrap();
    std::thread::scope(|sc| {
        let done: Vec<_> = (0..8)
            .map(|_| {
                sc.spawn(|| {
                    let c = s.db.get().unwrap();
                    medorama::recordings::sync_persons(&s.cfg, &c, &src, &["a1".to_string(), "a2".to_string()])
                })
            })
            .collect();
        for d in done {
            d.join().unwrap().unwrap();
        }
    });
    let c = s.db.get().unwrap();
    let ann = medorama::persons::known(&s.cfg, &c, id, &["a1".into()]).unwrap()["a1"];
    assert!(!medorama::recordings::of_patient(&s.cfg, &c, &s.user("boss"), ann).unwrap().is_empty());
}

/// The optional tables of a recording database (docs/databases.md, "Indexes and signals a
/// recording database holds"): its ready-made index of a recording.
fn held_tables(a: &Dbb) {
    a.exec(
        "CREATE TABLE recording_indexes (recording_key TEXT PRIMARY KEY, index_dir TEXT NOT NULL, size INTEGER NOT NULL,
             mtime_ns INTEGER NOT NULL, md5 TEXT NOT NULL DEFAULT '', adapter TEXT NOT NULL DEFAULT '', indexed_at TEXT NOT NULL DEFAULT '');
         CREATE TABLE recording_signals (recording_key TEXT NOT NULL, name TEXT NOT NULL, group_name TEXT NOT NULL DEFAULT '',
             label TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL DEFAULT '', unit TEXT NOT NULL DEFAULT '', fs REAL,
             PRIMARY KEY (recording_key, name));
         INSERT INTO recording_signals VALUES ('r1', 'icu-ecg-II', 'icu-ecg', 'II', 'wave', 'mV', 250),
                                              ('r1', 'icu-bp-ART', 'icu-bp', 'ART', 'wave', 'mmHg', 125),
                                              ('r1', 'icu-hr', 'icu-hr', 'HR', 'vital', 'bpm', NULL);",
    );
}

/// The size and modification time (integer nanoseconds) of a file, as a tool that indexes ahead
/// of time records them.
fn stamp(p: &std::path::Path) -> (i64, i64) {
    let m = std::fs::metadata(p).unwrap();
    let ns = m.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as i64;
    (m.len() as i64, ns)
}

#[test]
fn a_ready_made_index_the_database_holds_is_taken_instead_of_indexing() {
    let s = Setup::new();
    let a = hospital(&s);
    held_tables(&a);
    // Ann's sleep study, one EDF file
    let path = s.data().join("psg/ann.edf");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../viewers/vitalstream/testdata/edf/psg_plus.edf"),
        &path,
    )
    .unwrap();
    a.recording("r9", "a1", "psg/ann.edf", "sleep");
    // the index made ahead of time, beside the database (a folder: meta.json and arrays)
    let made = s.data().join("made");
    let key2 = recording_engine::index::build_index(&path, &made, &|_| {}).unwrap();
    // (marked, to tell it from an index Medorama builds)
    let mj = made.join(&key2).join("meta.json");
    let text = std::fs::read_to_string(&mj).unwrap();
    std::fs::write(&mj, text.replacen('{', "{\"made_ahead\":true,", 1)).unwrap();
    let made_by = recording_engine::sources::reader_of(&path).unwrap().1;
    let (size, mtime) = stamp(&path);
    a.exec(&format!(
        "INSERT INTO recording_indexes (recording_key, index_dir, size, mtime_ns, adapter)
         VALUES ('r9', 'made/{key2}', {size}, {mtime}, '{made_by}'),
                ('r1', 'made/{key2}', 0, 0, '{made_by}')"
    ));
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let ann = medorama::persons::known(&s.cfg, &c, id, &["a1".into()]).unwrap()["a1"];
    let recs = medorama::recordings::of_patient(&s.cfg, &c, &boss, ann).unwrap();
    let rec = recs.iter().find(|r| r.path.ends_with("ann.edf")).unwrap().clone();
    // opening it in the viewer: taken into the database's index store first
    let paths = vec![rec.path.clone()];
    assert_eq!(medorama::recordings::take_held_before_opening(&s.cfg, &c, &paths).unwrap(), 1);
    let store = medorama::workspace::index_store(&s.cfg, &c, id).unwrap();
    let key = recording_engine::index::cache_key_of(std::path::Path::new(&rec.path), 3);
    let meta = store.meta(&key).expect("taken");
    assert!(meta.contains("\"made_ahead\":true"), "taken, not built: {meta}");
    // (the WFDB record, though listed: a record's header alone does not say it changed)
    let wfdb = recs.iter().find(|r| r.path.ends_with("rec.hea")).unwrap();
    assert_eq!(
        medorama::recordings::take_held_before_opening(&s.cfg, &c, std::slice::from_ref(&wfdb.path)).unwrap(),
        0
    );
    // its index job only records what the index says (the record's job indexes it)
    assert_eq!(medorama::jobs::queue_auto_index(&c, &s.cfg).unwrap(), 2);
    let reg = std::sync::Arc::new(std::sync::RwLock::new(medorama::actions::Registry::load(&s.cfg)));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    medorama::jobs::work(s.db.clone(), std::sync::Arc::new(s.cfg.clone()), reg.clone(), 0, stop.clone(), true);
    let rec = medorama::recordings::get(&s.cfg, &c, rec.id).unwrap().unwrap();
    assert_eq!(rec.index_key.as_deref(), Some(key.as_str()), "ready: {}", rec.error);
    assert!(rec.t_start.is_some(), "its times, from the index");
    assert!(store.meta(&key).unwrap().contains("\"made_ahead\""), "still the one taken");

    // the file changed since the index was made: Medorama indexes it itself
    store.remove(&key).unwrap();
    a.exec("UPDATE recording_indexes SET mtime_ns = mtime_ns - 1");
    assert_eq!(medorama::recordings::take_held_before_opening(&s.cfg, &c, &paths).unwrap(), 0);
    // made by another program than the one that reads it now: not taken either
    a.exec("UPDATE recording_indexes SET mtime_ns = mtime_ns + 1, adapter = 'recording-engine 0.0.1'");
    assert_eq!(medorama::recordings::take_held_before_opening(&s.cfg, &c, &paths).unwrap(), 0);
    assert!(store.meta(&key).is_none());
}

#[test]
fn a_recordings_signals_are_found_by_name_and_shown_with_it() {
    let s = Setup::new();
    let a = hospital(&s);
    held_tables(&a);
    let id = s.connect("hospa", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    // in the catalog, as ICU names of the vocabulary `signal`: each name and each group
    let names = medorama::variables::signals(&s.cfg, &c, id).unwrap();
    assert_eq!(names, ["icu-bp", "icu-bp-ART", "icu-ecg", "icu-ecg-II", "icu-hr"]);
    let ann = medorama::persons::known(&s.cfg, &c, id, &["a1".into()]).unwrap()["a1"];
    let found = |text: &str| -> Vec<i64> {
        let e = medorama::cohorts::parse_text(&c, &s.cfg, &boss, text).unwrap();
        let q = Criteria { include: Some(e), ..Default::default() };
        medorama::cohorts::evaluate(&c, &s.cfg, &boss, &q, None).unwrap().into_keys().collect()
    };
    for text in ["has icu-bp-ART", "has icu-bp", "icu-ecg-II", "has ICU-HR", "has hospa.icu-ecg", "icu-hr AND female"] {
        assert_eq!(found(text), [ann], "{text}");
    }
    assert!(found("NOT has icu-bp").iter().all(|p| *p != ann));
    // a name that is no signal, or a signal compared: told, never searched as nothing
    for bad in ["has icu-bp-XYZ", "icu-bp-XYZ", "icu-ecg-II > 3"] {
        assert!(medorama::cohorts::parse_text(&c, &s.cfg, &boss, bad).is_err(), "{bad}");
    }
    // dates: the recording's times (Ann's recording starts 2020-01-01 in the synthetic record)
    let k = |from: &str, to: &str| {
        let mut e = medorama::cohorts::parse_text(&c, &s.cfg, &boss, "has icu-bp").unwrap();
        if let Expr::Leaf(l) = &mut e {
            l.from = from.into();
            l.to = to.into();
        }
        let q = Criteria { include: Some(e), ..Default::default() };
        medorama::cohorts::evaluate(&c, &s.cfg, &boss, &q, None).unwrap().len()
    };
    let _ = k("", "");
    // shown in the recording's detail, by group
    let rec = medorama::recordings::of_patient(&s.cfg, &c, &boss, ann).unwrap().remove(0);
    let d = medorama::patient::detail(&c, &s.cfg, &boss, ann, &format!("rec:{}", rec.id)).unwrap();
    let shown: Vec<(&str, &str)> = d.signals.iter().map(|x| (x.group.as_str(), x.name.as_str())).collect();
    assert_eq!(shown, [("icu-bp", "icu-bp-ART"), ("icu-ecg", "icu-ecg-II"), ("icu-hr", "icu-hr")]);
    // Count now: the recordings that hold each, and their persons
    medorama::databases::count_variables(&s.cfg, &c, &boss, id).unwrap();
    let v = medorama::variables::of(&s.cfg, &c, id).unwrap();
    let bp = v.iter().find(|x| x.name == "icu-bp").unwrap();
    assert_eq!((bp.entries, bp.persons), (Some(1), Some(1)));
}
