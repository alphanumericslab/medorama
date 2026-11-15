//! The database as a whole: `check` finds each problem, `remove-source` takes a source out, an
//! encrypted database cannot be read without its key, a large table, the same file added twice,
//! and the queries Medorama runs on a db-builder database.

mod common;

use common::*;
use db_builder::{check, db};

fn problems(f: &Fx) -> Vec<(String, usize, Vec<String>)> {
    let c = f.open("UTC");
    check::check(&c, &f.db()).unwrap().into_iter().map(|p| (p.what, p.count, p.examples)).collect()
}

fn has(ps: &[(String, usize, Vec<String>)], what: &str, example: &str) -> bool {
    ps.iter().any(|(w, _, ex)| w.starts_with(what) && ex.iter().any(|e| e.contains(example)))
}

#[test]
fn check_finds_each_problem() {
    let f = Fx::new();
    let clean = f.write("p.csv", "patient_id,mrn,last_name\nP1,M1,Testperson\n");
    f.add(&[clean], &opts("SITE"));
    assert!(problems(&f).is_empty(), "{:?}", problems(&f));
    // a patient ID without its issuer, and an MRN without its authority
    let no_issuer = f.write("q.csv", "patient_id,mrn\nP2,\n,M9\n");
    let mut c = f.open("UTC");
    db_builder::add::add(&mut c, &f.db(), &[no_issuer], &Default::default()).unwrap();
    // one MRN given to two persons by one authority
    let twice = f.write("r.csv", "patient_id,mrn\nP3,M1\n");
    f.add(&[twice], &opts("SITE"));
    // an entry and a recording naming a visit no table names; a recording whose file is gone
    wfdb_record(&f.root().join("icu"), "gone", 0);
    let recs = f.write("s.csv", "encounter_id,file,name\nE404,icu/gone.hea,\nE404,,Lactate\n");
    f.add(&[recs], &opts("SITE"));
    std::fs::remove_file(f.root().join("icu/gone.hea")).unwrap();
    // dangling keys written by another program, and a patient ID given twice by one issuer
    c.execute_batch(
        "INSERT INTO persons (person_key, patient_id, patient_id_issuer) VALUES ('x1', 'P1', 'SITE');
         INSERT INTO visits (visit_key, person_key, source_id) VALUES ('V1', 'nobody', 1);
         INSERT INTO entries (entry_key, person_key, visit_key, source_id) VALUES ('k1', 'nobody', 'V1', 1);
         INSERT INTO visits (visit_key, person_key, source_id) VALUES ('V2', 'P1', 1);
         INSERT INTO entries (entry_key, person_key, visit_key, source_id) VALUES ('k2', 'P3', 'V2', 1);",
    )
    .unwrap();
    let ps = problems(&f);
    assert!(has(&ps, "persons whose patient ID names no issuer", "P2"), "{ps:?}");
    assert!(has(&ps, "persons whose MRN names no authority", "2:2"), "{ps:?}");
    assert!(has(&ps, "patient IDs given to two persons", "P1 + x1"), "{ps:?}");
    assert!(has(&ps, "MRNs given to two persons", "P1 + P3"), "{ps:?}");
    assert!(has(&ps, "visits whose person_key names no person", "V1"), "{ps:?}");
    assert!(has(&ps, "entries whose person_key names no person", "k1"), "{ps:?}");
    assert!(has(&ps, "entries whose visit_key names no visit", "4:2"), "{ps:?}");
    assert!(has(&ps, "entries without a person", "4:2"), "{ps:?}");
    assert!(has(&ps, "entries whose person is not their visit's", "k2"), "{ps:?}");
    assert!(has(&ps, "recordings whose visit_key names no visit", "4:1"), "{ps:?}");
    assert!(has(&ps, "recordings without a person", "4:1"), "{ps:?}");
    assert!(has(&ps, "recordings whose file is missing", "icu/gone.hea"), "{ps:?}");
    // the command says so, and exits with 1
    let (code, out, _) = cli(&["check", &f.db().to_string_lossy()], None);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("recordings whose file is missing"), "{out}");
    let names = out.contains("Testperson") || out.contains("M1");
    assert!(!names, "check names no names and no MRNs: {out}");
}

#[test]
fn remove_source_takes_out_what_only_it_added() {
    let f = Fx::new();
    let p = f.write("patients.csv", "patient_id,last_name\nP1,Testperson\nP2,Otherperson\n");
    let e = f.write("enc.csv", "encounter_id,patient_id,admit_time\nE1,P1,2024-01-01 10:00\n");
    let l = f.write("labs.csv", "encounter_id,test,result\nE1,Na,140\nE1,K,4.1\n");
    f.add(&[p, e, l], &opts("SITE"));
    let c = f.open("UTC");
    // the encounters' source: the visit stays (the labs name it), as the labs' source
    let r = db::remove_source(&c, 2).unwrap();
    assert_eq!((r.entries, r.visits, r.persons), (0, 0, 0));
    assert_eq!(rows(&c, "SELECT visit_key, source_id FROM visits"), [["E1", "3"]]);
    // the patients' source: P2 goes; P1 stays, the visit and its entries still name it
    let r = db::remove_source(&c, 1).unwrap();
    assert_eq!(r.persons, 1);
    assert_eq!(rows(&c, "SELECT person_key FROM persons"), [["P1"]]);
    // the labs: nothing is left
    let r = db::remove_source(&c, 3).unwrap();
    assert_eq!((r.entries, r.visits, r.persons), (2, 1, 1));
    for t in ["persons", "visits", "entries", "extras", "row_sources", "sources"] {
        assert_eq!(count(&c, &format!("SELECT COUNT(*) FROM {t}")), 0, "{t}");
    }
    assert!(db::remove_source(&c, 3).is_err());
    // the command
    let t = f.write("more.csv", "patient_id,test,result\nP9,Na,140\n");
    f.add(&[t], &opts("SITE"));
    let (code, out, err) = cli(&["remove-source", &f.db().to_string_lossy(), "4"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("source 4 removed: 1 entries"), "{out}");
}

#[test]
fn an_encrypted_database_needs_its_key() {
    let f = Fx::new();
    let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let out = f.db().to_string_lossy().to_string();
    let t = f.write("labs.csv", "patient_id,test,result\nP1,Na,140\n");
    assert_eq!(cli(&["--encrypt", "new", &out], None).0, 2, "no key, no encrypted database");
    assert_eq!(cli(&["--encrypt", "new", &out], Some("abc")).0, 2, "a key of 64 hex digits");
    let (code, o, e) = cli(&["--encrypt", "new", &out, "--timezone", "UTC"], Some(key));
    assert_eq!(code, 0, "{o}{e}");
    let (code, o, e) = cli(&["--encrypt", "add", &out, &t.to_string_lossy(), "--issuer", "SITE"], Some(key));
    assert_eq!(code, 0, "{o}{e}");
    // its bytes hold nothing readable, and SQLite (or a wrong key) cannot open it
    let bytes = std::fs::read(f.db()).unwrap();
    assert!(!bytes.starts_with(b"SQLite format 3"));
    assert!(!bytes.windows(4).any(|w| w == b"P1,N" || w == b"SITE"));
    let plain = rusqlite::Connection::open(f.db()).unwrap();
    assert!(plain.query_row("SELECT COUNT(*) FROM persons", [], |r| r.get::<_, i64>(0)).is_err());
    let (code, _, e) = cli(&["info", &out], None);
    assert_eq!(code, 2);
    assert!(e.contains("encrypted"), "{e}");
    let wrong = "f".repeat(64);
    assert_eq!(cli(&["--encrypt", "info", &out], Some(&wrong)).0, 2);
    let (code, o, e) = cli(&["--encrypt", "info", &out], Some(key));
    assert_eq!(code, 0, "{e}");
    assert!(o.contains("entries     1 rows"), "{o}");
    // a copy (the web interface's Save a copy…) is encrypted with the same key
    let how = db::Opening { key: Some(key.into()) };
    let copy = f.root().join("copy.sqlite");
    db::save_copy(&db::open(&f.db(), &how).unwrap(), &copy, &how).unwrap();
    assert!(!std::fs::read(&copy).unwrap().starts_with(b"SQLite format 3"));
    assert!(db::open(&copy, &db::Opening::default()).is_err());
    let n: i64 = db::open(&copy, &how).unwrap().query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    assert!(db::save_copy(&db::open(&f.db(), &how).unwrap(), &copy, &how).is_err(), "not over a file");
}

#[test]
fn a_dry_run_changes_nothing() {
    let f = Fx::new();
    let out = f.db().to_string_lossy().to_string();
    let t = f.write(
        "labs.csv",
        "patient_id,issuer,test,result,units,time,QTc (ms)\nP1,SITE,Na,140,mmol/L,2024-03-02 07:10,412\n",
    );
    // a database not made yet is not made
    let (code, o, e) = cli(&["add", &out, &t.to_string_lossy(), "--dry-run", "--timezone", "UTC"], None);
    assert_eq!(code, 0, "{o}{e}");
    assert!(!f.db().exists());
    assert!(o.contains("would add") && o.contains("1 persons") && o.contains("2 entries"), "{o}");
    assert!(o.contains("column \"QTc (ms)\" -> a measurement: QTc (ms)"), "{o}");
    assert!(o.contains("column \"time\" -> time") && !o.contains("P1,"), "{o}");
    // an existing one keeps its bytes
    assert_eq!(cli(&["new", &out, "--timezone", "UTC"], None).0, 0);
    let before = std::fs::read(f.db()).unwrap();
    let mut opts = opts("SITE");
    opts.dry_run = true;
    let r = f.add(std::slice::from_ref(&t), &opts);
    assert_eq!((r[0].persons_added, r[0].entries, r[0].source.is_some()), (1, 2, true));
    assert_eq!(r[0].sample.len(), 1);
    assert_eq!(std::fs::read(f.db()).unwrap(), before);
    let c = f.open("UTC");
    assert_eq!(c.query_row("SELECT COUNT(*) FROM sources", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    // then added for real: the same counts
    let r = f.add(std::slice::from_ref(&t), &common::opts("SITE"));
    assert_eq!((r[0].persons_added, r[0].entries), (1, 2));
}

#[test]
fn ten_thousand_rows() {
    let f = Fx::new();
    let mut t = String::from("patient_id,encounter_id,test,result,units,time\n");
    for i in 0..10_000 {
        t.push_str(&format!(
            "P{},E{},Lactate,{}.{},mmol/L,2024-01-{:02} 06:{:02}\n",
            i % 500,
            i % 900,
            i % 7,
            i % 10,
            1 + i % 28,
            i % 60
        ));
    }
    let p = f.write("labs.csv", t);
    let start = std::time::Instant::now();
    let r = &f.add(&[p], &opts("SITE"))[0];
    assert_eq!((r.rows, r.entries, r.persons_added, r.visits_added, r.left_out), (10_000, 10_000, 500, 900, 0));
    assert!(start.elapsed().as_secs() < 60, "{:?}", start.elapsed());
    let c = f.open("UTC");
    assert_eq!(
        count(&c, "SELECT COUNT(*) FROM entries WHERE value_num IS NOT NULL AND person_key IS NOT NULL"),
        10_000
    );
}

#[test]
fn the_same_file_twice_is_added_once() {
    let f = Fx::new();
    let t = f.write("labs.csv", "patient_id,test,result\nP1,Na,140\nP1,K,4.1\n");
    let first = f.add(std::slice::from_ref(&t), &opts("SITE"));
    let again = f.add(std::slice::from_ref(&t), &opts("SITE"));
    assert_eq!((first[0].source, again[0].already), (Some(1), Some(1)));
    let c = f.open("UTC");
    assert_eq!(count(&c, "SELECT COUNT(*) FROM entries"), 2);
    assert_eq!(count(&c, "SELECT COUNT(*) FROM sources"), 1);
    // the file changed: its earlier version is replaced
    std::fs::write(&t, "patient_id,test,result\nP1,Na,141\n").unwrap();
    let changed = f.add(&[t], &opts("SITE"));
    assert_eq!(changed[0].replaced, [1]);
    assert_eq!(rows(&c, "SELECT entry_key, value_num FROM entries"), [["2:1", "141"]]);
    // the version changes at every write
    let v = db::meta(&c, "version").unwrap().unwrap();
    assert!(v.ends_with(" 3"), "new, add, add again (nothing written), the change: {v}");
}

/// The queries Medorama runs on a db-builder database (docs/design/databases-only.md, "db-builder").
#[test]
fn medorama_reads_it() {
    let f = Fx::new();
    wfdb_record(&f.root().join("icu"), "bed12", 0);
    let p =
        f.write("patients.csv", "MRN,Last Name,First Name,DOB,Sex,Patient ID\nSYN1,Testperson,Ada,1950-03-04,F,P1\n");
    let e = f.write("enc.csv", "encounter_id,patient_id,admit_time,discharge_time,care_unit,bed\nE1,P1,2024-01-01 10:00,2024-01-05 10:00,MICU,4\n");
    let l = f.write("labs.csv", "encounter_id,test,result,units,ref_low,ref_high,flag,time,code,vocabulary,author,text,end_time\nE1,Lactate,4.1,mmol/L,0.5,2,H,2024-01-02 06:10,2524-7,LOINC,Dr Q,comment,2024-01-02 06:20\n");
    let r = f.write("recs.csv", "patient_id,encounter_id,file\nP1,E1,icu/bed12.hea\n");
    f.add(&[p, e, l, r], &opts("SITE"));
    let c = f.open("UTC");
    assert_eq!(
        rows(&c, "SELECT person_key, patient_id, patient_id_issuer, mrn, mrn_authority, last_name, first_name, birth_date, sex FROM persons"),
        [["P1", "P1", "SITE", "SYN1", "HOSP", "Testperson", "Ada", "1950-03-04", "F"]]
    );
    assert_eq!(
        rows(&c, "SELECT visit_key, person_key, start_time, end_time, unit, icu FROM visits"),
        [["E1", "P1", "2024-01-01T10:00:00+00:00", "2024-01-05T10:00:00+00:00", "MICU", "1"]]
    );
    assert_eq!(
        rows(
            &c,
            "SELECT entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary, value_num, value_text, unit, ref_low, ref_high, flag, author, text FROM entries"
        ),
        [[
            "3:1",
            "P1",
            "E1",
            "lab",
            "2024-01-02T06:10:00+00:00",
            "2024-01-02T06:20:00+00:00",
            "Lactate",
            "2524-7",
            "LOINC",
            "4.1",
            "",
            "mmol/L",
            "0.5",
            "2",
            "H",
            "Dr Q",
            "comment"
        ]]
    );
    let recs = rows(
        &c,
        "SELECT recording_key, person_key, visit_key, file, format, start_time, end_time, label, category, size, fingerprint FROM recordings",
    );
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0][..5], ["4:1", "P1", "E1", "icu/bed12.hea", "WFDB record"]);
    if cfg!(feature = "wfdb") {
        assert_eq!(recs[0][5..7], ["2024-01-01T12:00:00+00:00", "2024-01-01T12:00:20+00:00"]);
    }
    assert_eq!(recs[0][7..9], ["bed12", "icu"]);
    assert_eq!(recs[0][9], std::fs::metadata(f.root().join("icu/bed12.hea")).unwrap().len().to_string());
    assert_eq!(recs[0][10], db::fingerprint(&f.root().join("icu/bed12.hea")).unwrap());
    // meta
    assert_eq!(db::meta(&c, "schema").unwrap().as_deref(), Some("1"));
    assert!(db::meta(&c, "created_by").unwrap().unwrap().starts_with("db-builder "));
    assert_eq!(db::meta(&c, "timezone").unwrap().as_deref(), Some("UTC"));
}

#[test]
fn rules_rewrite_paths_and_identifiers() {
    let f = Fx::new();
    // the data outside the database's folder: their paths are stored whole
    let data = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(data.path()).unwrap();
    let mnt = root.join("mnt");
    wfdb_record(&mnt.join("icu"), "bed12", 0);
    // the table names them where they were before the data were mounted elsewhere
    let t = f
        .write("beds.csv", "patient_id,mrn,encounter_id,file,category\nPT_0042,pt_7,E1,/labs/data/icu/bed12.hea,icu\n");
    let rules = f.write(
        "rules.toml",
        format!(
            "[[path]]\nprefix = '/labs/data'\nto = '{}'\n[patient_id]\nreplace = [{{ regex = '^PT_', to = 'PT' }}]\n\
             [mrn]\ncase = 'upper'\nreplace = [{{ regex = '(?i)^pt_', to = 'PT' }}]\n",
            mnt.display()
        ),
    );
    let mut o = opts("SITE");
    o.rules = db_builder::rules::Rules::read(&rules).unwrap();
    let r = &f.add(&[t], &o)[0];
    assert_eq!(r.recordings, 1, "{:?}", r.warnings);
    let c = f.open("UTC");
    assert_eq!(rows(&c, "SELECT person_key, patient_id, mrn FROM persons"), [["PT0042", "PT0042", "PT7"]]);
    let file = mnt.join("icu/bed12.hea").to_string_lossy().to_string();
    assert_eq!(rows(&c, "SELECT file FROM recordings"), [[file.clone()]]);
    drop(c);
    // the data moved again: `rewrite` changes the paths the database lists
    let vol = root.join("vol");
    std::fs::rename(&mnt, &vol).unwrap();
    assert!(problems(&f).iter().any(|p| p.0.starts_with("recordings whose file is missing")));
    let moved = f.write("moved.toml", format!("[[path]]\nprefix = '{}'\nto = '{}'\n", mnt.display(), vol.display()));
    let db = f.db().to_string_lossy().to_string();
    let (code, out, err) = cli(&["rewrite", &db, "--rules", &moved.to_string_lossy(), "--dry-run"], None);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("would rewrite: 1 recordings"), "{out}");
    let (code, out, err) = cli(&["rewrite", &db, "--rules", &moved.to_string_lossy()], None);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("rewritten: 1 recordings"), "{out}");
    let c = f.open("UTC");
    assert_eq!(rows(&c, "SELECT file FROM recordings"), [[vol.join("icu/bed12.hea").to_string_lossy().to_string()]]);
    drop(c);
    assert!(!problems(&f).iter().any(|p| p.0.starts_with("recordings whose file is missing")));
}
