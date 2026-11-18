//! A patient's page as the report and the detail show it (docs/design/patient-page.md): one
//! report entry per thing done, named only; an ECG with its measurements as one, its detail
//! comparing it with the patient's previous ECG.  And the dates of a patient's data, which an
//! ECG database without visits gives in its persons view.  All data invented.

mod common;
use common::{Dbb, Setup};

/// An ECG database: Ann's two ECGs (each with its statements and two measurements), Bob's one.
fn ecgs(s: &Setup) -> Dbb {
    let a = Dbb::new(&s.data().join("ecg.sqlite"));
    a.person("p1", "", "", "100", "HOSP", "Testa", "Ann", "1960-02-03", "F").person(
        "p2",
        "",
        "",
        "200",
        "HOSP",
        "Proba",
        "Bob",
        "1971-07-08",
        "M",
    );
    a.exec(
        "INSERT INTO entries (entry_key, person_key, category, time, name, value_num, unit, text) VALUES
           ('e1', 'p1', 'ecg', '2024-01-01 08:00', '12-lead ECG', NULL, '', 'Sinus rhythm\nProlonged QT'),
           ('e1:qtc', 'p1', 'ecg', '2024-01-01 08:00', 'QTc', 478, 'ms', ''),
           ('e1:qrs', 'p1', 'ecg', '2024-01-01 08:00', 'QRS', 104, 'ms', ''),
           ('e2', 'p1', 'ecg', '2024-03-05 08:22', '12-lead ECG', NULL, '', 'Atrial fibrillation\nProlonged QT'),
           ('e2:qtc', 'p1', 'ecg', '2024-03-05 08:22', 'QTc', 503, 'ms', ''),
           ('e2:qrs', 'p1', 'ecg', '2024-03-05 08:22', 'QRS', 104, 'ms', ''),
           ('e3', 'p2', 'ecg', '2023-05-01 10:00', '12-lead ECG', NULL, '', 'Normal ECG'),
           ('e3:qtc', 'p2', 'ecg', '2023-05-01 10:00', 'QTc', 410, 'ms', '');",
    );
    a
}

/// Connected through views: the persons view gives the dates of each person's data.
fn connect_views(s: &Setup, a: &Dbb, persons: &str) -> i64 {
    let n = medorama::sources::NewSource {
        name: "muse".into(),
        kind: "views".into(),
        url: a.url(),
        settings: views(persons),
        ..Default::default()
    };
    medorama::databases::connect(&s.cfg, &s.db.get().unwrap(), &s.user("boss"), &n, "").unwrap()
}

fn views(persons: &str) -> medorama::sources::Settings {
    medorama::sources::Settings {
        persons: persons.into(),
        entries: "SELECT * FROM entries".into(),
        version: "SELECT value FROM meta WHERE key = 'version'".into(),
        ..Default::default()
    }
}

const WITH_DATES: &str = "SELECT p.*, MIN(e.time) AS earliest, MAX(e.time) AS latest
                          FROM persons p JOIN entries e ON e.person_key = p.person_key GROUP BY p.person_key";

#[test]
fn an_ecg_and_its_measurements_are_one_entry_compared_with_the_previous_ecg() {
    let s = Setup::new();
    let a = ecgs(&s);
    let id = s.connect("muse", &a);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let ann = medorama::persons::known(&s.cfg, &c, id, &["p1".into()]).unwrap()["p1"];
    // the report: two ECGs, named; no entry for a measurement, and no value
    let r = medorama::patient::report(&c, &s.cfg, &boss, ann, "").unwrap();
    assert_eq!(r.entries.len(), 2, "{:?}", r.entries.iter().map(|e| &e.title).collect::<Vec<_>>());
    assert!(r.entries.iter().all(|e| e.title == "12-lead ECG" && e.key.starts_with("ecg:")));
    assert!(r.entries[0].names.contains(&"QTc".to_string()), "the journey finds it by its measurements");
    let json = serde_json::to_string(&r.entries).unwrap();
    assert!(!json.contains("503"), "no value in the report: {json}");
    // the newest ECG's detail: each measurement once, its change since the previous ECG and its
    // trend; its statements against the previous ECG's
    let d = medorama::patient::detail(&c, &s.cfg, &boss, ann, &r.entries[0].key).unwrap();
    assert_eq!(d.title, "12-lead ECG");
    let e = d.ecg.expect("an ECG's detail");
    let qtc = e.measures.iter().find(|m| m.name == "QTc").unwrap();
    assert_eq!((qtc.value, qtc.change, qtc.series.len()), (503.0, Some(25.0), 2));
    assert_eq!(e.measures.iter().filter(|m| m.name == "QTc").count(), 1);
    assert_eq!(e.measures.iter().find(|m| m.name == "QRS").unwrap().change, Some(0.0));
    assert_eq!(e.statements, [("Atrial fibrillation".to_string(), true), ("Prolonged QT".to_string(), false)]);
    assert_eq!(e.gone, ["Sinus rhythm"]);
    assert!(e.previous.is_some());
    // the first ECG: nothing to compare with
    let first = medorama::patient::detail(&c, &s.cfg, &boss, ann, &r.entries[1].key).unwrap().ecg.unwrap();
    assert!(first.previous.is_none() && first.gone.is_empty());
    assert!(first.measures.iter().all(|m| m.change.is_none()));
    assert!(first.statements.iter().all(|(_, new)| !new));
}

#[test]
fn a_database_without_visits_dates_its_patients_by_its_persons_view() {
    let s = Setup::new();
    let a = ecgs(&s);
    let id = connect_views(&s, &a, WITH_DATES);
    let c = s.db.get().unwrap();
    let boss = s.user("boss");
    let known = medorama::persons::known(&s.cfg, &c, id, &["p1".into(), "p2".into()]).unwrap();
    let (ann, bob) = (known["p1"], known["p2"]);
    // the list: latest first, by the dates the view gives (Ann's latest ECG is the newer)
    let (total, rows) = medorama::patient::list(&c, &s.cfg, &boss, "", &[], 50, 0).unwrap();
    assert_eq!(total, 2);
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [ann, bob]);
    assert!(rows.iter().all(|r| r.latest.is_some() && r.encounters == 0));
    let rec = medorama::persons::records_of(&s.cfg, &c, bob, None).unwrap();
    assert!(rec[0].earliest.is_some() && rec[0].earliest == rec[0].latest, "Bob's one ECG");

    // the views replaced (persons without an MRN of digits left out): read again, the patients
    // kept keep their numbers
    a.exec("UPDATE persons SET mrn = 'test' WHERE person_key = 'p2'");
    let filtered = format!("SELECT * FROM ({WITH_DATES}) WHERE mrn NOT GLOB '*[^0-9]*'");
    medorama::databases::set_views(&s.cfg, &c, &boss, id, &views(&filtered)).unwrap();
    let (persons, patients, _) = medorama::persons::counts(&s.cfg, &c, id).unwrap();
    assert_eq!((persons, patients), (1, 1));
    assert_eq!(medorama::persons::known(&s.cfg, &c, id, &["p1".into()]).unwrap()["p1"], ann);
    // who may not: not an admin; not a database of views
    assert!(medorama::databases::set_views(&s.cfg, &c, &s.user("reader"), id, &views(&filtered)).is_err());
    let dbb = s.connect("hosp", &common::Dbb::new(&s.data().join("h.sqlite")));
    assert!(medorama::databases::set_views(&s.cfg, &c, &boss, dbb, &views(&filtered)).is_err());
}
