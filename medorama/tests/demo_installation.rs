//! A demo installation holds demo databases only, a site's installation none
//! (docs/design/demo.md): a database says it is the demo's by its marker table.  All data
//! invented.

mod common;
use common::{Dbb, Setup};
use medorama::installation::{self, Mode};

fn marked(s: &Setup, file: &str) -> Dbb {
    let d = Dbb::new(&s.data().join(file));
    d.person("d1", "", "", "SYN1", "DEMO", "Invented", "Ines", "1970-01-01", "F");
    d.exec("CREATE TABLE medorama_demo (note TEXT NOT NULL); INSERT INTO medorama_demo VALUES ('invented');");
    d
}

fn plain(s: &Setup, file: &str) -> Dbb {
    let d = Dbb::new(&s.data().join(file));
    d.person("p1", "", "", "100", "HOSP", "Testa", "Ann", "1960-02-03", "F");
    d
}

fn connect(s: &Setup, name: &str, d: &Dbb) -> anyhow::Result<i64> {
    let n = medorama::sources::NewSource { name: name.into(), kind: "dbb".into(), url: d.url(), ..Default::default() };
    medorama::databases::connect(&s.cfg, &s.db.get().unwrap(), &s.user("boss"), &n, "")
}

#[test]
fn a_sites_installation_refuses_a_demo_database() {
    let s = Setup::new();
    let c = s.db.get().unwrap();
    installation::set(&c, Mode::Real).unwrap();
    let e = connect(&s, "demo", &marked(&s, "demo.sqlite")).unwrap_err();
    assert!(format!("{e:#}").contains("is a demo database"), "{e:#}");
    connect(&s, "hosp", &plain(&s, "hosp.sqlite")).unwrap();
    assert!(!installation::is_demo(&c));
    // once a site's, never the demo's
    assert!(installation::set(&c, Mode::Demo).is_err());
}

#[test]
fn a_demo_installation_connects_demo_databases_only() {
    let s = Setup::new();
    let c = s.db.get().unwrap();
    installation::set(&c, Mode::Demo).unwrap();
    let e = connect(&s, "hosp", &plain(&s, "hosp.sqlite")).unwrap_err();
    assert!(format!("{e:#}").contains("demo installation"), "{e:#}");
    let id = connect(&s, "demo", &marked(&s, "demo.sqlite")).unwrap();
    assert!(medorama::sources::get(&c, id).unwrap().unwrap().settings.demo, "kept as a demo database");
    assert!(installation::is_demo(&c));
    assert_eq!(installation::settle(&c).unwrap(), Mode::Demo);
}

#[test]
fn a_control_store_from_before_decides_by_what_it_connects_and_refuses_both() {
    // demo databases only: the demo's
    let s = Setup::new();
    let c = s.db.get().unwrap();
    installation::set(&c, Mode::Demo).unwrap();
    connect(&s, "demo", &marked(&s, "demo.sqlite")).unwrap();
    c.execute("DELETE FROM settings WHERE key = 'installation'", []).unwrap();
    // (kept without the flag, as before 0.3.1: read again from its marker)
    c.execute("UPDATE databases SET settings = '{}'", []).unwrap();
    assert_eq!(installation::mode(&c).unwrap(), None);
    assert_eq!(installation::settle(&c).unwrap(), Mode::Demo);
    assert_eq!(installation::mode(&c).unwrap(), Some(Mode::Demo));

    // both: refused, with the demo databases named, and decided only once they are removed
    let s = Setup::new();
    let c = s.db.get().unwrap();
    installation::set(&c, Mode::Real).unwrap();
    connect(&s, "hosp", &plain(&s, "hosp.sqlite")).unwrap();
    c.execute("DELETE FROM settings WHERE key = 'installation'", []).unwrap();
    let d = marked(&s, "demo.sqlite");
    let n = medorama::sources::NewSource {
        name: "demo".into(),
        kind: "dbb".into(),
        url: d.url(),
        settings: medorama::sources::Settings { demo: true, ..Default::default() },
        ..Default::default()
    };
    c.execute(
        "INSERT INTO databases (name, kind, url, settings, created_at) VALUES ('demo', 'dbb', ?1, ?2, 0)",
        medorama::sql::params![n.url, serde_json::to_string(&n.settings).unwrap()],
    )
    .unwrap();
    let e = installation::settle(&c).unwrap_err();
    assert!(format!("{e:#}").contains("demo"), "{e:#}");
    assert_eq!(installation::mode(&c).unwrap(), None, "not decided while both are connected");
}
