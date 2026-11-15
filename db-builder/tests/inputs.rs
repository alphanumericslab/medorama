//! Each kind of input makes the rows it should: tables (CSV, Excel, OpenDocument, Parquet),
//! known exports (GE MUSE, CLIF, a declared format) and their columns as docs/importing.md
//! says.  Every patient, name and value here is invented.

mod common;

use common::*;
use db_builder::add::Options;

#[test]
fn a_patients_table_encounters_with_bed_stays_and_labs() {
    let f = Fx::new();
    let patients = f.write(
        "patients.csv",
        "MRN,Last Name,First Name,DOB,Sex,Patient ID,Race\nSYN0000001,Testperson,Ada,1950-03-04,Female,P900001,Other\n",
    );
    let encounters = f.write(
        "encounters.csv",
        "Encounter ID,Patient ID,CSN,Admit Time,Discharge Time,Admit Unit,Care Unit,Bed,Bed Start,Bed End\n\
         E9100002,P900001,9000000102,2023-12-28 07:40,2024-01-09 13:00,Cardiology,ICU7,12,2024-01-01 11:00,2024-01-03 11:00\n",
    );
    let labs = f.write(
        "labs.csv",
        "MRN,Encounter ID,Test Name,Result,Units,Ref Low,Ref High,Abnormal Flag,Collected,Code,Code System,Lab Tech\n\
         SYN0000001,E9100002,Lactate,4.1,mmol/L,0.5,2,H,2024-01-02 06:10,2524-7,LOINC,Zed\n\
         SYN0000001,E9100002,Culture,positive,,,,,2024-01-02 07:00,,,\n",
    );
    let reps = f.add(&[patients, encounters, labs], &opts("HOSP-PID"));
    assert_eq!(reps.len(), 3);
    assert_eq!((reps[0].persons_added, reps[1].visits_added, reps[2].entries), (1, 1, 2), "{:?}", reps[2].warnings);
    let c = f.open("UTC");
    assert_eq!(
        rows(&c, "SELECT person_key, patient_id, patient_id_issuer, mrn, mrn_authority, last_name, first_name, birth_date, sex FROM persons"),
        [["P900001", "P900001", "HOSP-PID", "SYN0000001", "HOSP", "Testperson", "Ada", "1950-03-04", "F"]]
    );
    // the encounter: its times in UTC (the database's zone), its unit, an ICU bed stay
    assert_eq!(
        rows(&c, "SELECT visit_key, person_key, start_time, end_time, unit, icu FROM visits"),
        [["E9100002", "P900001", "2023-12-28T07:40:00+00:00", "2024-01-09T13:00:00+00:00", "Cardiology", "1"]]
    );
    assert_eq!(
        rows(&c, "SELECT name, value FROM extras WHERE table_name = 'bed_stays' ORDER BY name"),
        [
            ["bed", "12"],
            ["bed_end", "2024-01-03T11:00:00+00:00"],
            ["bed_start", "2024-01-01T11:00:00+00:00"],
            ["care_unit", "ICU7"],
            ["visit_key", "E9100002"]
        ]
    );
    assert_eq!(
        count(&c, "SELECT COUNT(*) FROM extras WHERE table_name = 'visits' AND name = 'csn' AND value = '9000000102'"),
        1
    );
    // a column no reader knows: kept under its own header, with its row
    assert_eq!(
        rows(&c, "SELECT table_name, row_key, value FROM extras WHERE name = 'Race'"),
        [["persons", "P900001", "Other"]]
    );
    let e = rows(
        &c,
        "SELECT entry_key, person_key, visit_key, category, time, name, code, vocabulary, value_num, value_text, unit, ref_low, ref_high, flag
         FROM entries ORDER BY entry_key",
    );
    assert_eq!(
        e[0],
        [
            "3:1",
            "P900001",
            "E9100002",
            "lab",
            "2024-01-02T06:10:00+00:00",
            "Lactate",
            "2524-7",
            "LOINC",
            "4.1",
            "",
            "mmol/L",
            "0.5",
            "2",
            "H"
        ]
    );
    assert_eq!((e[1][8].as_str(), e[1][9].as_str()), ("NULL", "positive"), "a result in words is text");
    assert_eq!(rows(&c, "SELECT row_key, value FROM extras WHERE name = 'Lab Tech'"), [["3:1", "Zed"]]);
    assert_eq!(rows(&c, "SELECT kind, rows FROM sources ORDER BY id"), [["csv", "1"], ["csv", "1"], ["csv", "2"]]);
}

#[test]
fn tables_in_any_order_and_entries_by_account_number() {
    let f = Fx::new();
    // the labs first: their encounter is not known yet
    let labs = f.write("labs.csv", "csn,test,result,time\n77,Sodium,140,2024-01-02 06:00\n");
    let notes = f.write("notes.csv", "encounter_id,note,time\nE1,Seen on rounds,2024-01-02 09:00\n");
    f.add(&[labs, notes], &opts("SITE"));
    let c = f.open("UTC");
    assert_eq!(count(&c, "SELECT COUNT(*) FROM entries WHERE person_key IS NULL"), 2);
    let enc = f.write("enc.csv", "encounter_id,patient_id,csn\nE1,P1,77\n");
    f.add(&[enc], &opts("SITE"));
    assert_eq!(
        rows(&c, "SELECT name, category, person_key, visit_key FROM entries ORDER BY entry_key"),
        [["Sodium", "lab", "P1", "E1"], ["Notes", "note", "P1", "E1"]]
    );
}

#[test]
fn spreadsheets_and_parquet() {
    let f = Fx::new();
    let x = f.write(
        "meds.xlsx",
        xlsx(&[&["patient_id", "drug", "dose", "units", "start"], &["P2", "Heparin", "5000", "U", "2024-01-02 08:00"]]),
    );
    let o = f.write(
        "vitals.ods",
        ods(&[&["patient_id", "parameter", "value", "time"], &["P2", "HR", "88", "2024-01-02 08:05"]]),
    );
    let p = f.write(
        "obs.parquet",
        parquet(&["patient_id", "name", "value"], &[&["P2", "GCS", "15"]], Some(&[1_704_183_300_000_000])),
    );
    let reps = f.add(&[x, o, p], &opts("SITE"));
    assert_eq!(reps.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(), ["xlsx", "ods", "parquet"]);
    let c = f.open("UTC");
    // the category: the file's name (medications, observations), else a vital sign's name
    assert_eq!(
        rows(&c, "SELECT category, name, value_num, unit, time FROM entries ORDER BY entry_key"),
        [
            ["med", "Heparin", "5000", "U", "2024-01-02T08:00:00+00:00"],
            ["vit", "HR", "88", "", "2024-01-02T08:05:00+00:00"],
            ["obs", "GCS", "15", "", "NULL"]
        ]
    );
    // the Parquet timestamp column, not a recognised one, is kept as it came
    assert_eq!(rows(&c, "SELECT value FROM extras WHERE name = 'ts'"), [["2024-01-02 08:15:00+00:00"]]);
}

#[test]
fn a_mapping_file_and_local_times() {
    let f = Fx::new();
    let t = f.write("lis.csv", "Hosp No,Analyte,Res,When\nM1,Potassium,4.4,2024-03-10 02:30\n");
    let mapping = db_builder::tables::read_mapping(
        "[columns]\nmrn = \"Hosp No\"\nname = \"Analyte\"\nvalue = \"Res\"\ntime = \"When\"\n",
    )
    .unwrap();
    let mut c = db_builder::db::open_or_create(&f.db(), &Default::default(), Some("America/New_York")).unwrap();
    let o = Options { mrn_authority: "HOSP".into(), mapping, ..Default::default() };
    db_builder::add::add(&mut c, &f.db(), &[t], &o).unwrap();
    // 02:30 on 2024-03-10 in New York is skipped by the change to summer time: the moment shown as 01:30
    assert_eq!(
        rows(&c, "SELECT name, value_num, time FROM entries"),
        [["Potassium", "4.4", "2024-03-10T01:30:00-05:00"]]
    );
    assert_eq!(rows(&c, "SELECT person_key, mrn, mrn_authority, patient_id FROM persons"), [["1:1", "M1", "HOSP", ""]]);
    assert_eq!(db_builder::db::meta(&c, "timezone").unwrap().as_deref(), Some("America/New_York"));
}

#[test]
fn rows_that_cannot_be_read_are_left_out() {
    let f = Fx::new();
    let t = f.write(
        "labs.csv",
        "patient_id,name,value,time,ref_low\nP1,Na,140,yesterday,\nP1,K,4,2024-01-01,low\n,Cl,100,2024-01-01,\nP1,Ca,2.3,2024-01-01,2.1\n",
    );
    let r = &f.add(&[t], &opts("SITE"))[0];
    assert_eq!((r.entries, r.left_out), (1, 3), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("labs.csv, line 2") && w.contains("not a time")), "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("line 4") && w.contains("no patient_id")), "{:?}", r.warnings);
    let bad = f.write("semi.csv", "a;b\n1;2\n");
    let mut c = f.open("UTC");
    let e = db_builder::add::add(&mut c, &f.db(), &[bad], &opts("SITE")).unwrap_err();
    assert!(e.to_string().contains("semicolons"), "{e}");
}

#[test]
fn ge_muse_metadata() {
    let f = Fx::new();
    let ecg = f.write("ecg/a.xml", "<RestingECG/>");
    let t = f.write(
        "muse.csv",
        format!(
            "filename,PatientDemographics/PatientID,PatientDemographics/PatientLastName,PatientDemographics/Gender,\
             TestDemographics/AcquisitionDate,TestDemographics/AcquisitionTime,RestingECGMeasurements/QTCorrected,\
             RestingECGMeasurements/PRInterval,TestDemographics/OrderingMDLastName,Diagnosis/DiagnosisStatement/0/StmtText\n\
             {},SYN0001,Testperson,FEMALE,01-02-2024,08:15,438,n/a,Zorbluff,Sinus rhythm\n",
            ecg.display()
        ),
    );
    let r = &f.add(&[t], &opts(""))[0];
    assert_eq!(r.kind, "ge-muse");
    let c = f.open("UTC");
    assert_eq!(
        rows(&c, "SELECT mrn, mrn_authority, last_name, sex FROM persons"),
        [["SYN0001", "HOSP", "Testperson", "F"]]
    );
    assert_eq!(
        rows(&c, "SELECT entry_key, category, name, value_num, unit, text, time FROM entries ORDER BY entry_key"),
        [
            ["1:1", "ecg", "a", "NULL", "", "Sinus rhythm", "2024-01-02T08:15:00+00:00"],
            ["1:1:21", "ecg", "QTc", "438", "ms", "", "2024-01-02T08:15:00+00:00"]
        ]
    );
    assert_eq!(rows(&c, "SELECT value FROM extras WHERE row_key = '1:1' AND name = 'file'"), [["ecg/a.xml"]]);
    let all = rows(&c, "SELECT value FROM extras").concat().join("|");
    assert!(!all.contains("Zorbluff"), "no staff: {all}");
}

#[test]
fn a_clif_folder() {
    let f = Fx::new();
    f.write("clif/patient.csv", "patient_id,sex_category,birth_date,race_category\nC1,Male,1960-05-06,White\n");
    f.write(
        "clif/hospitalization.csv",
        "patient_id,hospitalization_id,admission_dttm,discharge_dttm\nC1,H1,2024-01-01 10:00:00+00:00,2024-01-05 10:00:00+00:00\n",
    );
    f.write(
        "clif/adt.csv",
        "hospitalization_id,in_dttm,out_dttm,location_category\nH1,2024-01-01 12:00:00+00:00,2024-01-03 12:00:00+00:00,icu\n",
    );
    f.write(
        "clif/vitals.csv",
        "hospitalization_id,recorded_dttm,vital_name,vital_category,vital_value\nH1,2024-01-02 08:15:00,Pulse,heart_rate,88\n",
    );
    f.write(
        "clif/microbiology_culture.csv",
        "patient_id,hospitalization_id,organism_id,collect_dttm,fluid_category,method_category,organism_category\n\
         C1,H1,O1,2024-01-02 09:00:00+00:00,blood,culture,escherichia_coli\n",
    );
    f.write(
        "clif/microbiology_susceptibility.csv",
        "organism_id,antimicrobial_category,susceptibility_category\nO1,ceftriaxone,susceptible\n",
    );
    f.write("clif/readme.csv", "a,b\n1,2\n");
    let reps = f.add(&[f.root().join("clif")], &opts("CLIF-SITE"));
    let kinds: Vec<&str> = reps.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "clif-patient",
            "clif-hospitalization",
            "clif-adt",
            "clif-microbiology-culture",
            "clif-vitals",
            "clif-microbiology-susceptibility"
        ]
    );
    let c = f.open("UTC");
    assert_eq!(
        rows(&c, "SELECT person_key, patient_id_issuer, sex, birth_date FROM persons"),
        [["C1", "CLIF-SITE", "M", "1960-05-06"]]
    );
    assert_eq!(rows(&c, "SELECT visit_key, person_key, icu FROM visits"), [["H1", "C1", "1"]]);
    let e = rows(&c, "SELECT category, name, value_num, value_text, unit, person_key, visit_key, time FROM entries ORDER BY category, name");
    assert!(
        e.contains(
            &["vit", "Heart rate", "88", "", "bpm", "C1", "H1", "2024-01-02T08:15:00+00:00"].map(String::from).to_vec()
        ),
        "{e:?}"
    );
    // the susceptibility on its culture's encounter
    assert!(e.iter().any(|r| r[0] == "micro" && r[6] == "H1" && r[3].to_lowercase().contains("susceptible")), "{e:?}");
    assert!(reps[0].warnings.iter().any(|w| w.contains("readme.csv")));
}

#[test]
fn a_declared_format() {
    let f = Fx::new();
    let toml = f.write(
        "formats/acme.toml",
        r#"
key = "acme-ecg"
label = "ACME ECG export"
category = "ecg"
detect = ["PatId", "AcqDate"]
required = ["mrn"]

[[column]]
name = "mrn"
from = "PatId"

[[column]]
name = "time"
from = "AcqDate"
date = "%m-%d-%Y"

[[column]]
name = "QTc (ms)"
from = "QTc"
number = true
"#,
    );
    let t = f.write("acme.csv", "PatId,AcqDate,QTc,Doctor\nM5,01-31-2024,441,Dr X\n,01-31-2024,400,\n");
    let o = Options { mrn_authority: "HOSP".into(), formats: vec![toml], ..Default::default() };
    let r = &f.add(&[t], &o)[0];
    assert_eq!((r.kind.as_str(), r.left_out), ("acme-ecg", 1));
    let c = f.open("UTC");
    assert_eq!(
        rows(&c, "SELECT person_key, category, name, value_num, unit, time FROM entries"),
        [["1:1", "ecg", "QTc", "441", "ms", "2024-01-31T00:00:00+00:00"]]
    );
}
