//! The batched import (`bulk.rs`, `ehr::insert_items`) against the row-by-row one, its
//! reference: the same table, with every identity rule in it, imported both ways into the same
//! catalog, makes the same catalog (compared by what rows mean, not by their ids or random
//! pseudonyms).

mod common;
use medorama::config::Config;
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::sql::Connection;
use medorama::tables;
use medorama::users::User;
use std::collections::HashMap;

const HEAD: &str = "patient_id,mrn,mrn_authority,last_name,first_name,birth_date,sex,encounter_id,csn,admit_time,discharge_time,care_unit,bed,bed_start,bed_end,name,value,unit,time,source\n";

/// What the catalog had before.
const BEFORE: &str = "\
P1,M1,,Doe,Ann,1950-01-02,female,E1,C1,2024-01-01 08:00,2024-01-05 08:00,ICU,3,2024-01-01 08:00,2024-01-02 08:00,,,,,
P2,M2,,Roe,Bob,1960-03-04,male,,,,,,,,,,,,,
,M9,,Poe,Cy,1970-05-06,male,,,,,,,,,,,,,
,M8,,,,,,,,,,,,,,,,,,
";

/// The table imported both ways: plain rows and every exception, in one chunk.
const TABLE: &str = "\
P1,M1,,,,,female,,,,,,,,,heart rate,80,bpm,2024-01-02 10:00,
P1,M1,,,,,male,,,,,,,,,heart rate,82,bpm,2024-01-02 11:00,
P3,M3,,Lee,Dee,1980-07-08,female,E3,C3,2024-02-01 08:00,,ICU,4,2024-02-01 08:00,2024-02-02 08:00,,,,,
P3,M3,,Lee,Dee,1980-07-08,female,E3,C3,2024-02-01 08:00,,ICU,4,2024-02-01 08:00,2024-02-02 08:00,,,,,
P3,,,,,,,E3,,,2024-02-09 08:00,,,,,,,,,other source
P3,,,,,,,,,,,,,,,,,,,other source
,,,,,,,E3,,,,ICU,5,2024-02-02 08:00,2024-02-03 08:00,,,,,
,M4,,Kim,Eve,1990-09-10,female,,,,,,,,,lactate,2.1,mmol/L,2024-03-01 10:00,
,M4,,,,,,,,,,,,,,lactate,2.1,mmol/L,2024-03-01 10:00,
,M4,,,,,,,,,,,,,,lactate,2.4,mmol/L,2024-03-01 11:00,
,m4 ,,,,,,,,,,,,,,,,,,
P2,M1,,,,,,,,,,,,,,,,,,
P10,M9,,,,,,,,,,,,,,,,,,
P1,M5,,,,,,,,,,,,,,,,,,
P1,M5,H2,,,,,,,,,,,,,,,,,
P3,,,,,,,E1,,,,,,,,,,,,
,,,,,,,E404,,,,ICU,6,,,,,,,
P5,,,,,,,,,2024-13-45,,,,,,,,,,
P5,,,,,,,,,,,,,,,,,,,
P5,M8,,,,,,,,,,,,,,,,,,
P6,M6,,,,,,E6,,2024-04-01 08:00,2024-04-03 08:00,,,,,,,,,
,M6,,,,,,,,,,,,,,heart rate,90,bpm,2024-04-02 08:00,
,M6,,,,,,,,,,,,,,heart rate,90,bpm,2024-04-02 08:00,
,M7,,,,,,,,,,,,,,heart rate,70,bpm,2024-04-02 08:00,
,,,,,,,E3,,,,,,,,note,a note,,2024-02-01 12:00,
,,,,,,,E1,,,,,,,,temperature,37.1,C,2024-01-03 08:00,
";

struct Fx {
    _tmp: tempfile::TempDir,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    Fx { _tmp: tmp, cfg, db }
}

fn import(f: &Fx, c: &mut Connection, name: &str, rows: &str, row_by_row: bool) -> tables::TableReport {
    let data = format!("{HEAD}{rows}");
    let mut opts = Opts::new(name, "", false);
    opts.row_by_row = row_by_row;
    tables::import(c, &f.cfg, &User::local(), name, data.as_bytes(), &HashMap::new(), "", None, &opts).unwrap()
}

/// The catalog, as what its rows mean: patients by patient ID (a generated one by the MRN it
/// was given for), encounters by encounter ID; sorted lines.
fn dump(c: &Connection) -> Vec<String> {
    let pkey = |p: &str| {
        format!(
            "(SELECT CASE WHEN x.patient_id LIKE 'GEN-%'
                  THEN 'GEN:' || COALESCE((SELECT g.authority || '/' || g.mrn_key FROM generated_ids g WHERE g.patient_id = x.patient_id), '?')
                  ELSE x.patient_id END FROM patients x WHERE x.id = {p})"
        )
    };
    let ekey = |e: &str| format!("(SELECT y.encounter_id FROM encounters y WHERE y.id = {e})");
    let queries = [
        format!(
            "SELECT 'patient', {}, p.sex, i.mrn, i.last_name, i.first_name, i.birth_date,
                    CAST(p.pseudonym IS NOT NULL AS TEXT), CAST(p.date_draw IS NOT NULL AS TEXT)
             FROM patients p LEFT JOIN patient_identifiers i ON i.patient = p.id",
            pkey("p.id")
        ),
        format!("SELECT 'mrn', {}, authority, mrn, mrn_key, source FROM patient_mrns", pkey("patient")),
        format!(
            "SELECT 'value', kind, CASE WHEN kind = 'patient' THEN {} ELSE {} END, field, CAST(value AS TEXT), source
             FROM field_values",
            pkey("row_id"),
            ekey("row_id")
        ),
        format!(
            "SELECT 'encounter', e.encounter_id, {}, CAST(e.admit_time AS TEXT), CAST(e.discharge_time AS TEXT),
                    e.admit_unit, e.disposition, i.csn, CAST(e.pseudonym IS NOT NULL AS TEXT)
             FROM encounters e LEFT JOIN encounter_identifiers i ON i.encounter = e.id",
            pkey("e.patient")
        ),
        format!(
            "SELECT 'stay', {}, unit, bed, CAST(start_time AS TEXT), CAST(end_time AS TEXT), recording_file FROM bed_stays",
            ekey("encounter")
        ),
        format!(
            "SELECT 'provenance', kind, CASE kind WHEN 'patient' THEN {} WHEN 'encounter' THEN {}
                    ELSE (SELECT {} || ' ' || s.unit || ' ' || s.bed FROM bed_stays s WHERE s.id = row_id) END, source
             FROM provenance",
            pkey("row_id"),
            ekey("row_id"),
            ekey("s.encounter")
        ),
        format!(
            "SELECT 'item', {}, {}, key_encounter, key_csn, key_mrn, key_patient, domain, name, CAST(time AS TEXT),
                    CAST(value_num AS TEXT), value_text, unit, source, category,
                    (SELECT CAST(COUNT(*) AS TEXT) FROM ehr_provenance v WHERE v.item = ehr_items.id),
                    (SELECT CAST(COUNT(*) AS TEXT) FROM item_text t WHERE t.rowid = ehr_items.id)
             FROM ehr_items",
            pkey("patient"),
            ekey("encounter")
        ),
        "SELECT 'held', CAST(line AS TEXT), problem, source FROM held_rows".to_string(),
        "SELECT 'generated', authority, mrn_key FROM generated_ids".to_string(),
        "SELECT 'merge', from_patient_id, into_patient_id FROM patient_merges WHERE from_patient_id NOT LIKE 'GEN-%'"
            .to_string(),
        "SELECT 'row', CAST(import_id AS TEXT), CAST(line AS TEXT), status, CAST(item IS NOT NULL AS TEXT) FROM import_rows"
            .to_string(),
    ];
    let mut out = vec![];
    for q in &queries {
        let mut st = c.prepare(q).unwrap();
        // every column is text (or NULL): read until there is none
        let rows: Vec<String> = st
            .query_map([], |r| {
                Ok((0..)
                    .map_while(|i| r.get::<_, Option<String>>(i).ok())
                    .map(|v| v.unwrap_or_else(|| "∅".into()))
                    .collect::<Vec<_>>())
            })
            .unwrap()
            .map(|r| r.unwrap().join("|"))
            .collect();
        out.extend(rows);
    }
    out.sort();
    out
}

/// The counts both ways agree on (where an item waited is not one: the batched way reads a
/// chunk's patients before its items, and the end of the import matches the others).
fn counts(r: &tables::TableReport) -> Vec<usize> {
    vec![
        r.rows,
        r.patients_added,
        r.patients_named,
        r.encounters_added,
        r.bed_stays_added,
        r.conflicts,
        r.items_added,
        r.duplicates,
    ]
}

#[test]
fn batched_and_row_by_row_make_the_same_catalog() {
    let (a, b) = (fixture(), fixture());
    let (mut ca, mut cb) = (a.db.get().unwrap(), b.db.get().unwrap());
    import(&a, &mut ca, "before", BEFORE, true);
    import(&b, &mut cb, "before", BEFORE, true);
    assert_eq!(dump(&ca), dump(&cb), "the same start");
    let ra = import(&a, &mut ca, "table", TABLE, true);
    let rb = import(&b, &mut cb, "table", TABLE, false);
    let (da, db) = (dump(&ca), dump(&cb));
    for (x, y) in da.iter().zip(&db) {
        assert_eq!(x, y, "row by row | batched");
    }
    assert_eq!(da, db);
    assert_eq!(counts(&ra), counts(&rb));
    assert_eq!(ra.warnings, rb.warnings);
    // the exceptions are there (else the test would test little)
    assert!(ra.conflicts >= 4 && ra.patients_named >= 1 && ra.duplicates >= 2 && ra.bed_stays_added >= 2, "{ra:?}");
    // again: everything already there
    let ra = import(&a, &mut ca, "again", TABLE, true);
    let rb = import(&b, &mut cb, "again", TABLE, false);
    assert_eq!(dump(&ca), dump(&cb));
    assert_eq!(counts(&ra), counts(&rb));
    // and in chunks: rows of one patient across chunk ends
    let many: String = (0..12_000)
        .map(|i| {
            let p = i % 4000;
            format!(
                "Q{p},N{p},,,,,{},V{},,2024-05-01 08:00,,,,,,heart rate,{},bpm,2024-05-01 {:02}:00,\n",
                ["male", "female"][i % 2],
                p,
                60 + i % 50,
                8 + i / 4000
            )
        })
        .collect();
    let ra = import(&a, &mut ca, "many", &many, true);
    let rb = import(&b, &mut cb, "many", &many, false);
    assert_eq!(counts(&ra), counts(&rb));
    assert_eq!(dump(&ca), dump(&cb));
}

#[test]
fn progress_and_cancel() {
    let f = fixture();
    let mut c = f.db.get().unwrap();
    let rows: String =
        (0..12_000).map(|i| format!("R{i},,,,,,,,,,,,,,,heart rate,{},bpm,2024-05-01 08:00,\n", 60 + i % 40)).collect();
    let data = format!("{HEAD}{rows}");
    let mut opts = Opts::new("watched", "", false);
    let p = medorama::progress::Progress::new();
    opts.progress = Some(p.clone());
    let r =
        tables::import(&mut c, &f.cfg, &User::local(), "watched", data.as_bytes(), &HashMap::new(), "", None, &opts)
            .unwrap();
    assert_eq!(r.rows, 12_000);
    let v = p.view();
    assert_eq!(v["rows"], 12_000);
    assert!(v["done"].as_f64().unwrap() > 0.9, "{v}");
    // cancelled: an error, and nothing of it kept
    let before = dump(&c);
    let mut opts = Opts::new("stopped", "", false);
    let p = medorama::progress::Progress::new();
    p.cancel();
    opts.progress = Some(p);
    let err =
        tables::import(&mut c, &f.cfg, &User::local(), "stopped", data.as_bytes(), &HashMap::new(), "", None, &opts)
            .unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err}");
    assert_eq!(dump(&c), before);
    let n: i64 = c.query_row("SELECT COUNT(*) FROM imports", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
}
