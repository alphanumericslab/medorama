//! `db-builder recordings`: a folder's recordings, their patients from a table or a pattern,
//! their start and end from their headers; and recordings named by a table's `file` column.

mod common;

use common::*;
use db_builder::recordings::{add_folder, Patients};

#[test]
fn a_folder_with_a_patients_table() {
    let f = Fx::new();
    let data = f.root().join("data");
    wfdb_record(&data.join("icu"), "rec1", 0);
    wfdb_record(&data.join("icu"), "rec2", 3600);
    f.write("data/sleep/P3_night.edf", edf_file());
    f.write("data/notes.txt", "not a recording");
    let table = f.write(
        "patients.csv",
        "file,patient_id,issuer,encounter_id,category,room\nicu/rec1.hea,P1,SITE,E1,,12\nrec2,P2,SITE,,,\nsleep/P3_night.edf,P3,SITE,,sleep,\nmissing.edf,P4,SITE,,,\n",
    );
    let mut c = f.open("Europe/Paris");
    let who = Patients { table: Some(table.clone()), pattern: None };
    let r = add_folder(&mut c, &f.db(), &data, &who, &Default::default()).unwrap();
    assert_eq!(r.recordings, 3, "{:?}", r.warnings);
    assert!(
        r.warnings.iter().any(|w| w.contains("1 row(s) of the patients table name no recording")),
        "{:?}",
        r.warnings
    );
    let got = rows(
        &c,
        "SELECT person_key, visit_key, file, format, start_time, end_time, label, category, size > 0, length(fingerprint)
         FROM recordings ORDER BY file",
    );
    let wfdb = cfg!(feature = "wfdb");
    assert_eq!(got[0][..4], ["P1", "E1", "data/icu/rec1.hea", "WFDB record"]);
    if wfdb {
        // the header's clock, 12:00, read as UTC (as the engine reads it, so that it agrees with
        // the recording's index), written in the database's zone (Paris, UTC+1 in January)
        assert_eq!(got[0][4..6], ["2024-01-01T13:00:00+01:00", "2024-01-01T13:00:20+01:00"]);
        assert_eq!(got[1][4], "2024-01-01T14:00:00+01:00");
    }
    assert_eq!(got[0][6..], ["rec1", "icu", "1", "64"]);
    assert_eq!(got[1][..3], ["P2", "NULL", "data/icu/rec2.hea"]);
    assert_eq!(got[2][..4], ["P3", "NULL", "data/sleep/P3_night.edf", "EDF"]);
    assert_eq!(got[2][4..6], ["2024-01-02T09:10:00+01:00", "2024-01-02T09:10:01+01:00"]);
    assert_eq!(got[2][7], "sleep");
    assert_eq!(rows(&c, "SELECT row_key, value FROM extras WHERE name = 'room'"), [["1:1", "12"]]);
    assert_eq!(rows(&c, "SELECT patient_id, patient_id_issuer FROM persons ORDER BY 1").len(), 3);
    // the same folder again: nothing done; a file more: the folder's source replaced
    let again = add_folder(&mut c, &f.db(), &data, &who, &Default::default()).unwrap();
    assert_eq!(again.already, Some(1));
    wfdb_record(&data.join("icu"), "rec3", 7200);
    let more = add_folder(&mut c, &f.db(), &data, &who, &Default::default()).unwrap();
    assert_eq!((more.replaced.clone(), more.recordings), (vec![1], 4));
    assert_eq!(count(&c, "SELECT COUNT(*) FROM recordings"), 4);
    assert_eq!(count(&c, "SELECT COUNT(*) FROM recordings WHERE person_key IS NULL"), 1);
}

#[test]
fn a_folder_with_a_pattern() {
    let f = Fx::new();
    let data = f.root().join("psg");
    f.write("psg/S01_night1.edf", edf_file());
    f.write("psg/S02_night1.edf", edf_file());
    f.write("psg/other.edf", edf_file());
    let mut c = f.open("UTC");
    let who = Patients { table: None, pattern: Some("{patient}_*.edf".into()) };
    let o = db_builder::add::Options { issuer: "STUDY".into(), category: "sleep".into(), ..Default::default() };
    let r = add_folder(&mut c, &f.db(), &data, &who, &o).unwrap();
    assert_eq!(r.recordings, 3);
    assert_eq!(
        rows(&c, "SELECT r.person_key, p.patient_id_issuer, r.category FROM recordings r LEFT JOIN persons p USING (person_key) ORDER BY r.file"),
        [["S01", "STUDY", "sleep"], ["S02", "STUDY", "sleep"], ["NULL", "NULL", "sleep"]]
    );
    // the database is never written into the folder it reads
    let inside = data.join("out.sqlite");
    let mut c2 = db_builder::db::open_or_create(&inside, &Default::default(), Some("UTC")).unwrap();
    assert!(add_folder(&mut c2, &inside, &data, &who, &o).unwrap_err().to_string().contains("into the folder"));
}

#[test]
fn recordings_named_by_a_table() {
    let f = Fx::new();
    wfdb_record(&f.root().join("icu"), "bed12", 0);
    let ecg = wfdb_record(&f.root().join("ecg"), "ecg1", 0);
    let t = f.write(
        "index.csv",
        "patient_id,encounter_id,file,category\nP1,E1,icu/bed12.hea,\nP1,E1,ecg/ecg1.hea,ecg\nP1,E1,icu/bed12.dat,\n",
    );
    let r = &f.add(&[t], &opts("SITE"))[0];
    // the 12-lead ECG: an entry, and its file a row of recordings too
    assert_eq!((r.recordings, r.entries, r.left_out), (2, 1, 1), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("WFDB signal file")), "{:?}", r.warnings);
    let c = f.open("UTC");
    assert_eq!(
        rows(&c, "SELECT recording_key, person_key, visit_key, file, category FROM recordings"),
        [["1:1", "P1", "E1", "icu/bed12.hea", "icu"], ["1:2:file", "P1", "E1", "ecg/ecg1.hea", "ecg"]]
    );
    // a 12-lead ECG: an entry, its file with it
    assert_eq!(rows(&c, "SELECT e.category, e.name, x.value FROM entries e JOIN extras x ON x.row_key = e.entry_key AND x.name = 'file' "), [["ecg", "ecg1", "ecg/ecg1.hea"]]);
    assert!(ecg.exists());
}

#[test]
fn a_collection_is_one_recording() {
    let f = Fx::new();
    let data = f.root().join("data");
    // a WFDB collection (docs/wfdb-collection.md): its records are parts of it
    let bed = data.join("collection1");
    wfdb_record(&bed.join("ecg"), "ecg_0001", 0);
    wfdb_record(&bed.join("resp"), "resp_0001", 0);
    f.write("data/collection1/index.csv", "record,modality,start_unix\necg/ecg_0001,ecg,\nresp/resp_0001,resp,\n");
    wfdb_record(&data.join("other"), "rec1", 0);
    let mut c = f.open("UTC");
    let who = Patients { table: None, pattern: Some("{patient}-*".into()) };
    let r = add_folder(&mut c, &f.db(), &data, &who, &Default::default()).unwrap();
    assert_eq!(r.recordings, 2, "{:?}", r.warnings);
    let got = rows(&c, "SELECT file, format, size FROM recordings ORDER BY file");
    assert_eq!(got[0][..2], ["data/collection1", "WFDB collection"]);
    // its size: its files'
    let files: u64 = ["index.csv", "ecg/ecg_0001.hea", "ecg/ecg_0001.dat", "resp/resp_0001.hea", "resp/resp_0001.dat"]
        .iter()
        .map(|n| std::fs::metadata(bed.join(n)).unwrap().len())
        .sum();
    assert_eq!(got[0][2], files.to_string());
    assert_eq!(got[1][..2], ["data/other/rec1.hea", "WFDB record"]);
    // a table's row that names the collection (another database): one recording, the row's
    let g = Fx::new();
    let bed = g.root().join("collection1");
    wfdb_record(&bed.join("ecg"), "ecg_0001", 0);
    g.write("collection1/index.csv", "record,modality,start_unix\necg/ecg_0001,ecg,\n");
    let t = g.write("beds.csv", "patient_id,encounter_id,file\nP1,E1,collection1\n");
    let r = &g.add(&[t], &opts("SITE"))[0];
    assert_eq!(r.recordings, 1, "{:?}", r.warnings);
    assert_eq!(rows(&g.open("UTC"), "SELECT recording_key, format FROM recordings"), [["1:1", "WFDB collection"]]);
}

#[test]
fn indexes_made_ahead_of_time_for_the_recordings_that_are_one_file() {
    let f = Fx::new();
    let data = f.root().join("data");
    wfdb_record(&data.join("icu"), "rec1", 0);
    f.write("data/sleep/P3_night.edf", edf_file());
    let table = f.write("patients.csv", "file,patient_id,issuer\nicu/rec1.hea,P1,SITE\nsleep/P3_night.edf,P3,SITE\n");
    let mut c = f.open("UTC");
    add_folder(&mut c, &f.db(), &data, &Patients { table: Some(table), pattern: None }, &Default::default()).unwrap();
    let dir = f.root().join("indexes");
    let r = db_builder::indexes::make(&mut c, &f.db(), &dir, &[]).unwrap();
    // the EDF file indexed; the WFDB record left to Medorama, which indexes it when opened
    assert_eq!((r.made, r.kept), (1, 0));
    assert!(
        r.skipped.iter().any(|(file, why)| file.ends_with("rec1.hea") && why.contains("WFDB record")),
        "{:?}",
        r.skipped
    );
    let got = rows(&c, "SELECT index_dir, size > 0, mtime_ns > 0, adapter FROM recording_indexes");
    assert_eq!(got.len(), 1);
    assert!(got[0][0].starts_with("indexes/"), "relative to the database's folder: {}", got[0][0]);
    assert_eq!(got[0][1..3], ["1", "1"]);
    assert!(got[0][3].starts_with("recording-engine "), "{}", got[0][3]);
    assert!(f.root().join(&got[0][0]).join("meta.json").is_file());
    // again: kept; the file changed: made again
    assert_eq!(db_builder::indexes::make(&mut c, &f.db(), &dir, &[]).unwrap().kept, 1);
    let edf = data.join("sleep/P3_night.edf");
    std::fs::File::options()
        .write(true)
        .open(&edf)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
        .unwrap();
    assert_eq!(db_builder::indexes::make(&mut c, &f.db(), &dir, &[]).unwrap().made, 1);
    // a category not asked for: nothing
    assert_eq!(db_builder::indexes::make(&mut c, &f.db(), &dir, &["eeg".into()]).unwrap().made, 0);
    // no folder half written left behind
    assert!(std::fs::read_dir(&dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));
}
