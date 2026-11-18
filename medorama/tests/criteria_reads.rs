//! What a criterion reads (docs/performance.md, "What a criterion reads"): its names and its
//! threshold run in the database, per unit, and the patients found are those Medorama's own
//! test finds over every row; the `has` view; answers kept while the data's version stays.
//! All data invented.

mod common;
use common::{Dbb, Setup};
use medorama::cohorts::{Criteria, Criterion, Expr};

/// QTc values (some in s, most in ms; one exactly at 450 ms) and heart rates, for 60 persons:
/// (entry key, person, time, name, value, unit).
fn measurements() -> Vec<(String, String, String, &'static str, f64, &'static str)> {
    let mut out = vec![];
    for i in 0..60 {
        let p = format!("p{i}");
        for j in 0..3 {
            let ms = 380.0 + ((i * 7 + j * 13) % 140) as f64;
            let (v, unit) = if (i + j) % 9 == 0 { (ms / 1000.0, "s") } else { (ms, "ms") };
            let v = if i == 5 && j == 0 { 450.0 } else { v };
            out.push((format!("q{i}-{j}"), p.clone(), format!("2024-0{}-1{j} 08:00", 1 + j), "QTc", v, unit));
        }
        out.push((format!("h{i}"), p, "2024-01-10 08:00".into(), "HR", 50.0 + i as f64, "bpm"));
    }
    out
}

/// The QTc values of [`measurements`]: (person, value, unit).
fn qtc_rows() -> Vec<(String, f64, &'static str)> {
    measurements().into_iter().filter(|m| m.3 == "QTc").map(|m| (m.1, m.4, m.5)).collect()
}

fn ecgs(s: &Setup, file: &str) -> (Dbb, Vec<(String, f64, &'static str)>) {
    let d = Dbb::new(&s.data().join(file));
    for i in 0..60 {
        d.person(&format!("p{i}"), "", "", &format!("{}", 1000 + i), "HOSP", "Testa", "Ann", "1960-02-03", "F");
    }
    for (key, p, time, name, v, unit) in measurements() {
        d.entry(&key, &p, "", "ecg", &time, name, Some(v), unit);
    }
    (d, qtc_rows())
}

fn qtc(cmp: &str, v: f64, unit: &str) -> Criterion {
    Criterion {
        kind: "measurement".into(),
        name: "QTc".into(),
        cmp: cmp.into(),
        value: Some(v),
        unit: unit.into(),
        ..Default::default()
    }
}

/// The persons Medorama's test passes, over every row (the bounds pushed down must keep them).
fn expected(rows: &[(String, f64, &str)], k: &Criterion) -> Vec<String> {
    let in_ms = |v: f64, u: &str| if u == "s" { v * 1000.0 } else { v };
    let want = |x: f64| if k.unit == "s" { x * 1000.0 } else { x };
    let mut out: Vec<String> = rows
        .iter()
        .filter(|(_, v, u)| {
            let v = in_ms(*v, u);
            let eps = 1e-6;
            match (k.value, k.min, k.max) {
                (Some(x), ..) => {
                    let x = want(x);
                    match k.cmp.as_str() {
                        ">" => v > x + eps,
                        ">=" => v >= x - eps,
                        "<" => v < x - eps,
                        "<=" => v <= x + eps,
                        "!=" => (v - x).abs() > eps,
                        _ => (v - x).abs() <= eps,
                    }
                }
                (None, lo, hi) => lo.is_none_or(|m| v >= want(m) - eps) && hi.is_none_or(|m| v <= want(m) + eps),
            }
        })
        .map(|r| r.0.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The person keys a criterion finds.
fn found(s: &Setup, db: i64, keys: &[String], q: &Criteria) -> anyhow::Result<Vec<String>> {
    let c = s.db.get().unwrap();
    let known = medorama::persons::known(&s.cfg, &c, db, keys).unwrap();
    let hits = medorama::cohorts::evaluate(&c, &s.cfg, &s.user("boss"), q, None)?;
    let mut out: Vec<String> = known.iter().filter(|(_, p)| hits.contains_key(p)).map(|(k, _)| k.clone()).collect();
    out.sort();
    Ok(out)
}

fn one(k: Criterion) -> Criteria {
    Criteria { include: Some(Expr::Leaf(k)), ..Default::default() }
}

fn keys() -> Vec<String> {
    (0..60).map(|i| format!("p{i}")).collect()
}

/// Connected through views, as a site writes them.
fn connect_views(s: &Setup, name: &str, d: &Dbb, settings: medorama::sources::Settings) -> i64 {
    let n = medorama::sources::NewSource {
        name: name.into(),
        kind: "views".into(),
        url: d.url(),
        settings,
        ..Default::default()
    };
    medorama::databases::connect(&s.cfg, &s.db.get().unwrap(), &s.user("boss"), &n, "").unwrap()
}

fn views(entries: &str) -> medorama::sources::Settings {
    medorama::sources::Settings {
        persons: "SELECT * FROM persons".into(),
        entries: entries.into(),
        version: "SELECT value FROM meta WHERE key = 'version'".into(),
        ..Default::default()
    }
}

fn every_threshold() -> Vec<Criterion> {
    let mut ks = vec![];
    for cmp in [">", ">=", "<", "<=", "=", "!="] {
        ks.push(qtc(cmp, 450.0, "ms"));
        ks.push(qtc(cmp, 0.45, "s"));
    }
    ks.push(Criterion { min: Some(440.0), max: Some(470.0), unit: "ms".into(), value: None, ..qtc("", 0.0, "") });
    ks.push(Criterion { min: Some(0.5), value: None, unit: "s".into(), ..qtc("", 0.0, "") });
    ks
}

#[test]
fn a_threshold_run_in_the_database_finds_the_patients_every_row_tested_finds() {
    let s = Setup::new();
    let (d, rows) = ecgs(&s, "ecg.sqlite");
    // numbers as numbers (a db-builder database), as text (a view's CAST), and through a
    // variables view that names one of the two units the entries have
    let a = s.connect("dbb", &d);
    let b = connect_views(
        &s,
        "text",
        &d,
        views(
            "SELECT entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary,
                    CAST(value_num AS TEXT) AS value_num, value_text, unit, ref_low, ref_high, flag, author, text FROM entries",
        ),
    );
    let c = connect_views(
        &s,
        "apart",
        &d,
        medorama::sources::Settings {
            variables:
                "SELECT 'ecg' AS category, 'QTc' AS name, '' AS vocabulary, '' AS code, 'ms' AS unit, 1 AS numeric
                        UNION ALL SELECT 'ecg', 'HR', '', '', 'bpm', 1"
                    .into(),
            ..views("SELECT * FROM entries")
        },
    );
    for db in [a, b, c] {
        for k in every_threshold() {
            let k = Criterion { database: Some(db), ..k };
            assert_eq!(found(&s, db, &keys(), &one(k.clone())).unwrap(), expected(&rows, &k), "{db}: {k:?}");
        }
        // a value without a unit, the data in two: which one is meant is asked, as before
        let e = found(&s, db, &keys(), &one(Criterion { database: Some(db), ..qtc(">", 450.0, "") })).unwrap_err();
        assert!(format!("{e:#}").contains("write the unit"), "{e:#}");
        // one unit: a value without one is in it
        let hr = Criterion { name: "HR".into(), database: Some(db), ..qtc(">", 100.0, "") };
        assert_eq!(
            found(&s, db, &keys(), &one(hr)).unwrap(),
            (51..60).map(|i| format!("p{i}")).collect::<Vec<_>>().tap_sort()
        );
    }
}

trait TapSort {
    fn tap_sort(self) -> Self;
}
impl TapSort for Vec<String> {
    fn tap_sort(mut self) -> Self {
        self.sort();
        self
    }
}

#[test]
fn answers_are_kept_while_the_data_version_stays_and_read_again_when_it_changes() {
    let s = Setup::new();
    let (mut d, rows) = ecgs(&s, "ecg.sqlite");
    let db = s.connect("dbb", &d);
    let k = Criterion { database: Some(db), ..qtc(">", 500.0, "ms") };
    let before = found(&s, db, &keys(), &one(k.clone())).unwrap();
    assert_eq!(before, expected(&rows, &k));
    assert!(!before.contains(&"p0".to_string()));
    // a row added, the version not changed: the answer kept (the database says its data are
    // those it answered for)
    d.entry("late", "p0", "", "ecg", "2024-05-01 08:00", "QTc", Some(600.0), "ms");
    assert_eq!(found(&s, db, &keys(), &one(k.clone())).unwrap(), before);
    // a new version: read again (and its catalog, not read at it yet, pushes nothing down)
    d.bump();
    let after = found(&s, db, &keys(), &one(k)).unwrap();
    assert!(after.contains(&"p0".to_string()));
    assert_eq!(after.len(), before.len() + 1);
}

#[test]
fn a_value_of_a_unit_that_does_not_compare_is_left_out_and_said_so() {
    let s = Setup::new();
    let (d, _) = ecgs(&s, "ecg.sqlite");
    d.entry("mv", "p1", "", "ecg", "2024-01-01 08:00", "QTc", Some(1.2), "mV");
    let db = s.connect("dbb", &d);
    let c = s.db.get().unwrap();
    // no QTc reaches 0.6 s: none read (the database tested them), still none an error
    let q = one(Criterion { database: Some(db), ..qtc(">", 0.6, "s") });
    let ev = medorama::cohorts::evaluate_full(&c, &s.cfg, &s.user("boss"), &q, None).unwrap();
    assert!(ev.found.is_empty());
    assert!(ev.warnings.iter().any(|w| w.contains("1 in mV") && w.contains("left out")), "{:?}", ev.warnings);
}

#[test]
fn several_criteria_read_at_once_combine_as_each_alone() {
    let s = Setup::new();
    let (d, rows) = ecgs(&s, "ecg.sqlite");
    let db = s.connect("dbb", &d);
    let (k1, k2) = (
        Criterion { database: Some(db), ..qtc(">", 480.0, "ms") },
        Criterion { name: "HR".into(), database: Some(db), ..qtc("<", 80.0, "") },
    );
    let both = Criteria {
        include: Some(Expr::Group(medorama::cohorts::Group {
            op: "and".into(),
            of: vec![Expr::Leaf(k1.clone()), Expr::Leaf(k2.clone())],
        })),
        ..Default::default()
    };
    let hr: Vec<String> = (0..30).map(|i| format!("p{i}")).collect();
    let mut want: Vec<String> = expected(&rows, &k1).into_iter().filter(|p| hr.contains(p)).collect();
    want.sort();
    assert_eq!(found(&s, db, &keys(), &both).unwrap(), want);
}

#[test]
fn a_has_view_says_who_has_data_of_a_kind() {
    let s = Setup::new();
    let (d, _) = ecgs(&s, "ecg.sqlite");
    // (the view is believed: it names p0 alone, though every person has ECG entries)
    let db = connect_views(
        &s,
        "muse",
        &d,
        medorama::sources::Settings {
            has: "SELECT 'p0' AS person_key, 'ecg' AS category UNION ALL SELECT 'p1', 'lab'".into(),
            ..views("SELECT * FROM entries")
        },
    );
    let has = |cat: &str| one(Criterion { kind: "has".into(), category: cat.into(), ..Default::default() });
    assert_eq!(found(&s, db, &keys(), &has("ecg")).unwrap(), ["p0"]);
    assert_eq!(found(&s, db, &keys(), &has("lab")).unwrap(), ["p1"]);
    // without it, read from the entries
    let plain = connect_views(&s, "plain", &d, views("SELECT * FROM entries"));
    assert_eq!(found(&s, plain, &keys(), &has("ecg")).unwrap(), keys().tap_sort());
}

#[test]
fn a_threshold_runs_in_postgresql_too_and_a_text_value_is_compared_in_medorama() {
    // (a connected PostgreSQL database: with MEDORAMA_TEST_POSTGRES, as CI runs the tests)
    let Some(url) = std::env::var("MEDORAMA_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) else { return };
    let s = Setup::new();
    let pg = medorama::sql::Connection::postgres(&url, None, &Default::default()).unwrap();
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).unwrap();
    let schema = format!("src_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    let mut sql = format!(
        "CREATE SCHEMA {schema};
         CREATE TABLE {schema}.meta (key TEXT, value TEXT);
         INSERT INTO {schema}.meta VALUES ('version', 'v1');
         CREATE TABLE {schema}.persons (person_key TEXT PRIMARY KEY, patient_id TEXT NOT NULL DEFAULT '',
             patient_id_issuer TEXT NOT NULL DEFAULT '', mrn TEXT NOT NULL DEFAULT '', mrn_authority TEXT NOT NULL DEFAULT '',
             last_name TEXT NOT NULL DEFAULT '', first_name TEXT NOT NULL DEFAULT '', birth_date TEXT NOT NULL DEFAULT '',
             sex TEXT NOT NULL DEFAULT '');
         CREATE TABLE {schema}.entries (entry_key TEXT PRIMARY KEY, person_key TEXT, visit_key TEXT,
             category TEXT NOT NULL DEFAULT '', time TEXT, end_time TEXT, name TEXT NOT NULL DEFAULT '',
             code TEXT NOT NULL DEFAULT '', vocabulary TEXT NOT NULL DEFAULT '', value_num NUMERIC,
             value_text TEXT NOT NULL DEFAULT '', unit TEXT NOT NULL DEFAULT '', ref_low NUMERIC, ref_high NUMERIC,
             flag TEXT NOT NULL DEFAULT '', author TEXT NOT NULL DEFAULT '', text TEXT NOT NULL DEFAULT '');
         CREATE INDEX ON {schema}.entries (name, value_num);"
    );
    for i in 0..60 {
        sql += &format!(
            "INSERT INTO {schema}.persons (person_key, mrn, mrn_authority, last_name, first_name, birth_date, sex)
             VALUES ('p{i}', '{}', 'HOSP', 'Testa', 'Ann', '1960-02-03', 'F');",
            1000 + i
        );
    }
    for (key, p, time, name, v, unit) in measurements() {
        sql += &format!(
            "INSERT INTO {schema}.entries (entry_key, person_key, category, time, name, value_num, unit)
             VALUES ('{key}', '{p}', 'ecg', '{time}', '{name}', {v}, '{unit}');"
        );
    }
    pg.execute_batch_native(&sql).unwrap();
    let rows = qtc_rows();
    let cols = "entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary";
    let rest = "value_text, unit, ref_low, ref_high, flag, author, text";
    for (name, value) in [("numbers", "value_num"), ("texts", "CAST(value_num AS TEXT) AS value_num")] {
        let settings = medorama::sources::Settings {
            persons: format!("SELECT * FROM {schema}.persons"),
            entries: format!("SELECT {cols}, {value}, {rest} FROM {schema}.entries"),
            version: format!("SELECT value FROM {schema}.meta WHERE key = 'version'"),
            ..Default::default()
        };
        let n = medorama::sources::NewSource {
            name: name.into(),
            kind: "views".into(),
            url: url.clone(),
            settings,
            ..Default::default()
        };
        let db = medorama::databases::connect(&s.cfg, &s.db.get().unwrap(), &s.user("boss"), &n, "").unwrap();
        for k in every_threshold() {
            let k = Criterion { database: Some(db), ..k };
            assert_eq!(found(&s, db, &keys(), &one(k.clone())).unwrap(), expected(&rows, &k), "{name}: {k:?}");
        }
    }
    pg.execute_batch_native(&format!("DROP SCHEMA {schema} CASCADE")).unwrap();
}
