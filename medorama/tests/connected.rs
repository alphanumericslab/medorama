//! What Medorama keeps of a connected database, and nothing more (docs/design/connected-
//! databases.md): its subjects (identities, sex) and their encounters, the names and units of its
//! variables, and the recordings it has opened.  Counts only when asked; files only when a
//! patient is opened.  All data invented.

mod common;
use common::{wfdb_record, Dbb, Setup};

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
