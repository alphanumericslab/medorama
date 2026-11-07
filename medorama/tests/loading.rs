//! What an import still loading holds is not shown (docs/administration.md, "Imports and
//! sources"): a table is committed a chunk at a time under a batch that stays `loading` until its
//! last chunk; until then the patient list, the patient pages, encounters, cohorts and the
//! Databases tab leave out the patients, encounters and items only it holds.  An item an active
//! batch also holds stays.  A cancelled import leaves nothing.

mod common;
use medorama::cohorts::{self, Criteria, Criterion, Expr};
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::sql::Connection;
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

fn import(f: &Fx, c: &mut Connection, name: &str, text: &str, opts: &Opts) -> anyhow::Result<i64> {
    let r =
        medorama::tables::import(c, &f.cfg, &User::local(), name, text.as_bytes(), &HashMap::new(), "", None, opts)?;
    Ok(r.import_id)
}

/// The patients the list shows, by their patient IDs, sorted.
fn listed(c: &Connection, q: &str, has: &[&str]) -> Vec<String> {
    let has: Vec<String> = has.iter().map(|h| h.to_string()).collect();
    let (_, rows) = medorama::patient::list(c, &User::local(), q, &has, 1000, 0).unwrap();
    let mut v: Vec<String> = rows.into_iter().map(|r| r.patient_id).collect();
    v.sort();
    v
}

/// The encounters the encounter search shows, sorted.
fn encounters(c: &Connection) -> Vec<String> {
    let (_, rows) = medorama::query::search(c, &User::local(), &Default::default(), "UTC").unwrap();
    let mut v: Vec<String> = rows.into_iter().map(|e| e.encounter_id).collect();
    v.sort();
    v
}

/// The patients a cohort's criterion finds, by their patient IDs, sorted.
fn found(c: &Connection, k: Criterion) -> Vec<String> {
    let q = Criteria { include: Some(Expr::Leaf(k)), ..Default::default() };
    let ids: Vec<i64> = cohorts::evaluate(c, &User::local(), "UTC", &q).unwrap().into_keys().collect();
    let mut v: Vec<String> = ids
        .iter()
        .map(|id| c.query_row("SELECT patient_id FROM patients WHERE id = ?1", [id], |r| r.get(0)).unwrap())
        .collect();
    v.sort();
    v
}

fn lactate_over(v: f64) -> Criterion {
    Criterion {
        kind: "measurement".into(),
        name: "lactate".into(),
        cmp: ">".into(),
        value: Some(v),
        unit: "mmol/L".into(),
        ..Default::default()
    }
}

fn id_of(c: &Connection, pid: &str) -> i64 {
    c.query_row("SELECT id FROM patients WHERE patient_id = ?1", [pid], |r| r.get(0)).unwrap()
}

/// The number of lactate results a patient's trends show.
fn lactates(c: &Connection, pid: &str) -> usize {
    let t = medorama::patient::trends(c, &User::local(), id_of(c, pid)).unwrap();
    t.iter().filter(|t| t.name.eq_ignore_ascii_case("lactate")).map(|t| t.n).sum()
}

fn set_state(c: &Connection, import: i64, state: &str) {
    c.execute("UPDATE imports SET state = ?1 WHERE id = ?2", medorama::sql::params![state, import]).unwrap();
}

const HEAD: &str = "patient_id,encounter_id,admit_time,name,value,unit,time\n";

/// A batch left loading (as between two of its chunks): what only it holds is hidden on every
/// read path; made active, it shows.
#[test]
fn a_loading_batch_is_hidden_until_it_is_active() {
    let f = catalog();
    let mut c = f.db.get().unwrap();
    import(
        &f,
        &mut c,
        "first",
        &format!("{HEAD}P1,E1,2024-01-01 08:00,Lactate,1.0,mmol/L,2024-01-01 09:00\n"),
        &Opts::new("first", "", false),
    )
    .unwrap();
    // the second: P1's result again (held by both), a new result and encounter of P1, a new
    // patient P2
    let second = import(
        &f,
        &mut c,
        "second",
        &format!(
            "{HEAD}P1,E1,2024-01-01 08:00,Lactate,1.0,mmol/L,2024-01-01 09:00
P1,E1,2024-01-01 08:00,Lactate,9.0,mmol/L,2024-01-01 12:00
P1,E2,2024-03-01 08:00,Lactate,8.0,mmol/L,2024-03-01 09:00
P2,E3,2024-02-01 08:00,Lactate,5.0,mmol/L,2024-02-01 09:00
"
        ),
        &Opts::new("second", "", false),
    )
    .unwrap();
    let all = vec!["P1".to_string(), "P2".to_string()];
    assert_eq!(listed(&c, "", &[]), all);
    set_state(&c, second, "loading");

    // the patient list: browsing, a text search, a category
    assert_eq!(listed(&c, "", &[]), ["P1"]);
    assert_eq!(listed(&c, "P2", &[]), Vec::<String>::new());
    assert_eq!(listed(&c, "encounter:E2", &[]), Vec::<String>::new());
    assert_eq!(listed(&c, "", &["lab"]), ["P1"]);
    let (_, rows) = medorama::patient::list(&c, &User::local(), "", &[], 10, 0).unwrap();
    assert_eq!((rows[0].encounters, rows[0].items), (1, 1), "P1's encounter and result of the first batch only");
    // encounters
    assert_eq!(encounters(&c), ["E1"]);
    let e2: i64 = c.query_row("SELECT id FROM encounters WHERE encounter_id = 'E2'", [], |r| r.get(0)).unwrap();
    assert!(medorama::query::encounter(&c, &User::local(), e2).unwrap().is_none());
    // the patient pages: P2 is not there yet; P1 without the second batch's data
    let p2 = id_of(&c, "P2");
    assert!(medorama::patient::summary(&c, &User::local(), p2).is_err());
    assert!(medorama::patient::journey(&c, &User::local(), p2).is_err());
    let s = medorama::patient::summary(&c, &User::local(), id_of(&c, "P1")).unwrap();
    assert_eq!(s.encounters.iter().map(|e| e.encounter_id.as_str()).collect::<Vec<_>>(), ["E1"]);
    assert_eq!(lactates(&c, "P1"), 1, "the result both batches hold stays");
    let j = medorama::patient::journey(&c, &User::local(), id_of(&c, "P1")).unwrap();
    assert_eq!(j.spans.iter().filter(|s| s.key.starts_with("enc:")).count(), 1);
    let r = medorama::patient::report(&c, &User::local(), id_of(&c, "P1"), "").unwrap();
    assert_eq!(r.entries.len(), 1);
    // cohorts
    assert_eq!(found(&c, lactate_over(0.5)), ["P1"]);
    assert_eq!(found(&c, lactate_over(2.0)), Vec::<String>::new());
    assert_eq!(found(&c, Criterion { kind: "has".into(), category: "lab".into(), ..Default::default() }), ["P1"]);
    // the Databases tab: not the loading one
    let dbs = medorama::databases::list(&c).unwrap();
    assert_eq!(dbs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["first"]);
    assert_eq!(medorama::databases::resolve_patient(&c, "P2", "", "").unwrap(), None);

    // made active: everything shows
    set_state(&c, second, "active");
    assert_eq!(listed(&c, "", &[]), all);
    assert_eq!(listed(&c, "encounter:E2", &[]), ["P1"]);
    assert_eq!(encounters(&c), ["E1", "E2", "E3"]);
    assert_eq!(lactates(&c, "P1"), 3);
    assert_eq!(found(&c, lactate_over(2.0)), all);
    assert!(medorama::patient::summary(&c, &User::local(), p2).is_ok());
}

/// A table of `n` rows: patients Q0… with 10 results each, an encounter each.
fn big(n: usize) -> String {
    (0..n).fold(String::from(HEAD), |mut s, i| {
        s += &format!("Q{},F{},2024-01-01 08:00,Lactate,{},mmol/L,2024-01-01 09:00\n", i / 10, i / 10, 1 + i % 10);
        s
    })
}

/// How many patients of the big table the catalog holds (shown or not), and its batches still
/// loading.
fn stored(c: &Connection) -> (i64, i64) {
    let q: i64 = c.query_row("SELECT COUNT(*) FROM patients WHERE patient_id LIKE 'Q%'", [], |r| r.get(0)).unwrap();
    let l: i64 = c.query_row("SELECT COUNT(*) FROM imports WHERE state = 'loading'", [], |r| r.get(0)).unwrap();
    (q, l)
}

/// An import running in another thread: once its first chunks are committed, the pages do not
/// show them; once it completes they do.  Cancelled, it leaves nothing.
#[test]
fn an_import_shows_once_complete_and_a_cancelled_one_never() {
    if common::on_postgres() {
        return; // one transaction there: nothing is committed before the end
    }
    let f = catalog();
    let table = big(15_000);
    for cancel in [false, true] {
        let p = medorama::progress::Progress::new();
        let mut opts = Opts::new(if cancel { "cancelled" } else { "complete" }, "", false);
        opts.progress = Some(p.clone());
        let (result, checked) = std::thread::scope(|s| {
            let importer = s.spawn(|| {
                let mut c = f.db.get().unwrap();
                import(&f, &mut c, "big.csv", &table, &opts)
            });
            // a second chunk read: the first is committed
            let c = f.db.get().unwrap();
            while p.view()["rows"].as_u64().unwrap_or(0) < 10_000 && !importer.is_finished() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let mut checked = false;
            let (q, loading) = stored(&c);
            if q > 0 && loading == 1 {
                assert_eq!(listed(&c, "", &[]), Vec::<String>::new(), "no patient of it is listed");
                assert_eq!(listed(&c, "Q1", &[]), Vec::<String>::new());
                assert_eq!(encounters(&c), Vec::<String>::new());
                assert_eq!(found(&c, lactate_over(0.0)), Vec::<String>::new());
                let q0 = id_of(&c, "Q0");
                assert!(medorama::patient::summary(&c, &User::local(), q0).is_err());
                // (still loading after the checks: they saw it loading)
                checked = stored(&c).1 == 1;
            }
            if cancel {
                p.cancel();
            }
            (importer.join().unwrap(), checked)
        });
        if !checked {
            eprintln!("the import ended before it could be watched loading");
        }
        let c = f.db.get().unwrap();
        match (cancel, result) {
            (false, r) => {
                r.unwrap();
                let u = User::local();
                assert_eq!(medorama::patient::list(&c, &u, "", &[], 10, 0).unwrap().0, 1500);
                assert_eq!(found(&c, lactate_over(9.5)).len(), 1500);
                assert_eq!(medorama::query::search(&c, &u, &Default::default(), "UTC").unwrap().0, 1500);
                // removed again, for the cancelled run
                let id: i64 = c.query_row("SELECT id FROM imports WHERE name = 'complete'", [], |r| r.get(0)).unwrap();
                let mut c = f.db.get().unwrap();
                medorama::imports::remove_import(&mut c, &f.cfg, &User::local(), id, false, false).unwrap();
            }
            (true, Err(e)) => {
                assert!(e.to_string().contains("cancelled"), "{e}");
                assert_eq!(stored(&c), (0, 0), "nothing of it kept");
                assert_eq!(listed(&c, "", &[]), Vec::<String>::new());
            }
            (true, Ok(_)) => eprintln!("the import ended before it could be cancelled"),
        }
    }
}
