//! A synthetic CLIF folder (the Common Longitudinal ICU data Format: CSV tables, the vitals as
//! Parquet; every value invented) imported as Medorama reads CLIF (formats/clif.rs): the patients,
//! their hospitalizations and ICU stays, vital signs, results, ventilator settings, drugs,
//! assessments and diagnoses on their pages; cohorts on CLIF's categories; Privacy Mode.

mod common;
use medorama::cohorts::{self, Criteria, Criterion, Expr};
use medorama::db::Db;
use medorama::users::User;
use std::path::Path;
use std::sync::Arc;

/// The vitals as Parquet: timestamps (UTC instants), categories, values.
fn vitals_parquet(path: &Path) {
    use parquet::data_type::{ByteArray, ByteArrayType, DoubleType, Int64Type};
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    let schema = Arc::new(
        parquet::schema::parser::parse_message_type(
            "message schema {
               REQUIRED BYTE_ARRAY hospitalization_id (UTF8);
               REQUIRED INT64 recorded_dttm (TIMESTAMP(MICROS,true));
               REQUIRED BYTE_ARRAY vital_name (UTF8);
               REQUIRED BYTE_ARRAY vital_category (UTF8);
               OPTIONAL DOUBLE vital_value;
             }",
        )
        .unwrap(),
    );
    let t0: i64 = 1_704_186_000_000_000; // 2024-01-02 09:00:00 UTC
    let hosp = ["H1", "H1", "H1", "H2", "H2"];
    let times = [t0, t0 + 3_600_000_000, t0 + 7_200_000_000, t0, t0 + 3_600_000_000];
    let names = ["Pulse", "SpO2 (%)", "Temp", "Pulse", "Pulse"];
    let cats = ["heart_rate", "spo2", "temp_c", "heart_rate", "heart_rate"];
    let values = [118.0, 91.0, 38.6, 72.0]; // the fifth: no value (null)
    let file = std::fs::File::create(path).unwrap();
    let mut w = SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build())).unwrap();
    let mut g = w.next_row_group().unwrap();
    let text = |xs: &[&str]| xs.iter().map(|s| ByteArray::from(*s)).collect::<Vec<_>>();
    let mut col = 0;
    while let Some(mut c) = g.next_column().unwrap() {
        match col {
            0 => {
                c.typed::<ByteArrayType>().write_batch(&text(&hosp), None, None).unwrap();
            }
            1 => {
                c.typed::<Int64Type>().write_batch(&times, None, None).unwrap();
            }
            2 => {
                c.typed::<ByteArrayType>().write_batch(&text(&names), None, None).unwrap();
            }
            3 => {
                c.typed::<ByteArrayType>().write_batch(&text(&cats), None, None).unwrap();
            }
            _ => {
                c.typed::<DoubleType>().write_batch(&values, Some(&[1, 1, 1, 1, 0]), None).unwrap();
            }
        }
        c.close().unwrap();
        col += 1;
    }
    g.close().unwrap();
    w.close().unwrap();
}

/// A CLIF folder: two patients, their stays (one in the ICU, on a ventilator), and a file that
/// is not CLIF.
fn clif_folder(dir: &Path) {
    let w = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    w(
        "clif_patient.csv",
        "patient_id,race_name,race_category,ethnicity_category,sex_name,sex_category,birth_date,death_dttm,language_category,zipcode_five_digit\n\
         C001,W,White,Non-Hispanic,F,Female,1950-03-04,,English,30322\n\
         C002,B,Black or African American,Non-Hispanic,M,Male,1981-07-09,,English,30307\n",
    );
    w(
        "clif_hospitalization.csv",
        "patient_id,hospitalization_id,admission_dttm,discharge_dttm,age_at_admission,admission_type_category,discharge_category,zipcode_nine_digit,census_tract\n\
         C001,H1,2024-01-01 20:00:00+00:00,2024-01-08 15:00:00+00:00,73,ed,Home,303221234,13121001200\n\
         C002,H2,2024-01-02 06:00:00+00:00,2024-01-04 12:00:00+00:00,42,elective,Home,303071234,13121001300\n",
    );
    w(
        "clif_adt.csv",
        "hospitalization_id,hospital_id,in_dttm,out_dttm,location_name,location_category\n\
         H1,EUH,2024-01-01 20:00:00+00:00,2024-01-02 04:00:00+00:00,ED POD A,ed\n\
         H1,EUH,2024-01-02 04:00:00+00:00,2024-01-05 10:00:00+00:00,MICU 7 East,icu\n\
         H1,EUH,2024-01-05 10:00:00+00:00,2024-01-08 15:00:00+00:00,7 West,ward\n\
         H2,EUH,2024-01-02 06:00:00+00:00,2024-01-04 12:00:00+00:00,4 North,ward\n",
    );
    vitals_parquet(&dir.join("clif_vitals.parquet"));
    w(
        "clif_labs.csv",
        "hospitalization_id,lab_order_dttm,lab_collect_dttm,lab_result_dttm,lab_order_name,lab_name,lab_category,lab_value,lab_value_numeric,reference_unit,lab_specimen_category,lab_loinc_code\n\
         H1,2024-01-02 05:00:00+00:00,2024-01-02 05:10:00+00:00,2024-01-02 05:40:00+00:00,BMP,LACTATE,lactate,4.8,4.8,mmol/L,blood/plasma/serum,2524-7\n\
         H1,2024-01-02 05:00:00+00:00,2024-01-02 05:10:00+00:00,2024-01-02 06:30:00+00:00,UA,UA COLOR,urine_color,Yellow,,,urine,\n\
         H2,2024-01-02 07:00:00+00:00,2024-01-02 07:05:00+00:00,2024-01-02 07:45:00+00:00,BMP,LACTATE,lactate,1.1,1.1,mmol/L,blood/plasma/serum,2524-7\n",
    );
    w(
        "clif_respiratory_support.csv",
        "hospitalization_id,recorded_dttm,device_name,device_category,mode_name,mode_category,tracheostomy,fio2_set,lpm_set,peep_set,tidal_volume_obs\n\
         H1,2024-01-02 06:00:00+00:00,VENT,IMV,AC/VC,Assist Control-Volume Control,0,0.6,,10,430\n\
         H1,2024-01-03 06:00:00+00:00,VENT,IMV,AC/VC,Assist Control-Volume Control,0,0.4,,8,\n",
    );
    w(
        "clif_medication_admin_continuous.csv",
        "hospitalization_id,med_order_id,admin_dttm,med_name,med_category,med_group,med_route_category,med_dose,med_dose_unit,mar_action_category\n\
         H1,O1,2024-01-02 06:30:00+00:00,norepinephrine 8 mg/250 mL,norepinephrine,vasoactives,iv,0.08,mcg/kg/min,new_bag\n\
         H1,O1,2024-01-02 12:00:00+00:00,norepinephrine 8 mg/250 mL,norepinephrine,vasoactives,iv,0.08,mcg/kg/min,stop\n",
    );
    w(
        "clif_patient_assessments.csv",
        "hospitalization_id,recorded_dttm,assessment_name,assessment_category,assessment_group,numerical_value,categorical_value,text_value\n\
         H1,2024-01-02 08:00:00+00:00,GCS,gcs_total,Neurological,9,,\n",
    );
    w(
        "clif_hospital_diagnosis.csv",
        "hospitalization_id,diagnosis_code,diagnosis_code_format,diagnosis_primary,poa_present\nH1,A41.9,ICD10CM,1,1\n",
    );
    w("notes.csv", "a,b\n1,2\n");
}

#[test]
fn a_clif_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let dir = root.join("clif");
    std::fs::create_dir_all(&dir).unwrap();
    clif_folder(&dir);
    // (a seed for the fuzzer's tables target: MEDORAMA_PARQUET_SEED=<file>)
    if let Some(out) = std::env::var_os("MEDORAMA_PARQUET_SEED") {
        std::fs::copy(dir.join("clif_vitals.parquet"), out).unwrap();
    }
    let admin = User::local();
    let done = medorama::formats::clif::import_folder(&mut c, &cfg, &admin, &dir, "clif", false, None).unwrap();
    let names: Vec<&str> = done.imported.iter().map(|(n, ..)| n.as_str()).collect();
    assert_eq!(&names[..3], ["clif_patient", "clif_hospitalization", "clif_adt"], "who and which stay first");
    assert_eq!(names.len(), 9, "{names:?}");
    assert_eq!(done.skipped.len(), 1, "the file that is not CLIF");
    let report = |name: &str| &done.imported.iter().find(|(n, ..)| n == name).unwrap().2;
    assert_eq!(report("clif_patient").patients_added, 2);
    assert_eq!(report("clif_hospitalization").encounters_added, 2);
    assert_eq!(report("clif_adt").bed_stays_added, 4, "{:?}", report("clif_adt").warnings);
    assert_eq!(report("clif_vitals").items_added, 4, "the value-less row left out");
    assert_eq!(report("clif_respiratory_support").items_added, 2 + 4 + 2 + 2 - 3, "a device, then each value given");
    // geography and the age at admission are never kept
    let stored: String = c.query_row("SELECT group_concat(cells, '|') FROM import_rows", [], |r| r.get(0)).unwrap();
    for gone in ["30322", "303221234", "13121001200"] {
        assert!(!stored.contains(gone), "{gone} kept");
    }
    // the patients, their stays, the ICU
    let (sex, birth): (String, String) = c
        .query_row(
            "SELECT p.sex, i.birth_date FROM patients p JOIN patient_identifiers i ON i.patient = p.id WHERE p.patient_id = 'C001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((sex.as_str(), birth.as_str()), ("F", "1950-03-04"));
    let icu: i64 = c.query_row("SELECT COUNT(*) FROM bed_stays WHERE upper(unit) = 'ICU'", [], |r| r.get(0)).unwrap();
    assert_eq!(icu, 1);
    // the patient's page: entries of every kind, the ICU among her data
    let p: i64 = c.query_row("SELECT id FROM patients WHERE patient_id = 'C001'", [], |r| r.get(0)).unwrap();
    let report = serde_json::to_string(&medorama::patient::report(&c, &admin, p, "").unwrap()).unwrap();
    for x in [
        "Heart rate",
        "SpO2",
        "Temperature",
        "Lactate",
        "Urine color",
        "Yellow",
        "FiO2 (set)",
        "PEEP (set)",
        "Norepinephrine",
        "Gcs total",
        "A41.9",
    ] {
        assert!(report.contains(x), "{x} missing from the report");
    }
    // the infusion stopped (mCIDE: not administered) is an event without a dose
    let doses: Vec<Option<f64>> = c
        .prepare("SELECT value_num FROM ehr_items WHERE name = 'Norepinephrine' ORDER BY time")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(doses, [Some(0.08), None]);
    let kinds = medorama::patient::list(&c, &admin, "", &[], 10, 0).unwrap().1;
    let k = kinds.iter().find(|r| r.patient_id == "C001").unwrap();
    for cat in ["icu", "vit", "lab", "support", "med", "obs", "dx"] {
        assert!(k.kinds.iter().any(|x| x == cat), "{cat} not among {:?}", k.kinds);
    }
    // cohorts on CLIF's categories (by their names here) and its columns
    let ids = |text: &str| {
        let q = Criteria {
            include: Some(Expr::Leaf(Criterion { kind: "expression".into(), text: text.into(), ..Default::default() })),
            ..Default::default()
        };
        let mut v: Vec<String> = cohorts::search(&c, &admin, "UTC", &q, None, 100, 0)
            .unwrap()
            .1
            .into_iter()
            .map(|m| m.patient.patient_id)
            .collect();
        v.sort();
        v
    };
    assert_eq!(ids("lactate > 4"), ["C001"]);
    assert_eq!(ids("\"heart rate\" > 100"), ["C001"]);
    assert_eq!(ids("has icu"), ["C001"]);
    assert_eq!(ids("has support AND female"), ["C001"]);
    assert_eq!(ids("clif_respiratory_support.device = \"IMV\""), ["C001"]);
    // a second import of the same folder adds nothing new
    let again = medorama::formats::clif::import_folder(&mut c, &cfg, &admin, &dir, "clif", true, None).unwrap();
    assert!(again.imported.iter().all(|(_, _, r)| r.items_added == 0 && r.patients_added == 0), "dry run, all known");
    // in Privacy Mode: no CLIF identifier (the IDs are long at a site: here made so), no date of
    // birth, no real date
    let rename = "UPDATE patients SET patient_id = 'CLIFPT0000001' WHERE patient_id = 'C001'";
    c.execute(rename, []).unwrap();
    c.execute("UPDATE encounters SET encounter_id = 'CLIFHOSP000001' WHERE encounter_id = 'H1'", []).unwrap();
    let g = medorama::gate::Restricted {
        settings: medorama::deid::Settings::default(),
        dict: Arc::new(medorama::gate::Dict::build(&c, 0).unwrap()),
        secret: vec![7; 32],
    }
    .for_patient(p);
    let mut shown = serde_json::to_value(medorama::patient::report(&c, &admin, p, "").unwrap()).unwrap();
    let mut summary = serde_json::to_value(medorama::patient::summary(&c, &admin, p).unwrap()).unwrap();
    g.json(&mut shown);
    g.json(&mut summary);
    let text = format!("{shown}{summary}");
    for leak in ["CLIFPT0000001", "CLIFHOSP000001", "1950-03-04", "2024-01-0"] {
        assert!(!text.contains(leak), "Privacy Mode shows {leak}");
    }
    assert!(text.contains("MO-"), "the pseudonyms instead");
}

/// A table of text columns as Parquet (empty cells null): CLIF's tables as sites write them.
fn text_parquet(path: &Path, csv: &str) {
    use parquet::data_type::{ByteArray, ByteArrayType};
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    let fields: Vec<String> = header.iter().map(|h| format!("OPTIONAL BYTE_ARRAY {h} (UTF8);")).collect();
    let schema = format!("message schema {{ {} }}", fields.join(" "));
    let schema = Arc::new(parquet::schema::parser::parse_message_type(&schema).unwrap());
    let file = std::fs::File::create(path).unwrap();
    let mut w = SerializedFileWriter::new(file, schema, Arc::new(WriterProperties::builder().build())).unwrap();
    let mut g = w.next_row_group().unwrap();
    let mut col = 0;
    while let Some(mut c) = g.next_column().unwrap() {
        let cells: Vec<&str> = rows.iter().map(|r| r[col]).collect();
        let values: Vec<ByteArray> = cells.iter().filter(|s| !s.is_empty()).map(|s| ByteArray::from(*s)).collect();
        let defs: Vec<i16> = cells.iter().map(|s| i16::from(!s.is_empty())).collect();
        c.typed::<ByteArrayType>().write_batch(&values, Some(&defs), None).unwrap();
        c.close().unwrap();
        col += 1;
    }
    g.close().unwrap();
    w.close().unwrap();
}

/// Every CLIF table's columns (CLIF 2.1's DDL; clifpy's names for ECMO; the 3.0 tables Medorama
/// reads, from clifpy's 3.0 schemas), and the format each is.
const SCHEMAS: &[(&str, &str)] = &[
    ("clif-adt", "hospitalization_id,hospital_id,hospital_type,in_dttm,out_dttm,location_name,location_category,location_type"),
    ("clif-code-status", "patient_id,start_dttm,code_status_name,code_status_category"),
    ("clif-crrt-therapy", "hospitalization_id,device_id,recorded_dttm,crrt_mode_name,crrt_mode_category,dialysis_machine_name,blood_flow_rate,pre_filter_replacement_fluid_rate,post_filter_replacement_fluid_rate,dialysate_flow_rate,ultrafiltration_out"),
    ("clif-ecmo-mcs", "hospitalization_id,recorded_dttm,device_name,device_category,mcs_group,ecmo_configuration_category,control_parameter_name,control_parameter_category,control_parameter_value,flow,sweep_set,fdO2_set"),
    ("clif-ecmo-mcs", "hospitalization_id,recorded_dttm,device_name,device_category,mcs_group,device_metric_name,device_rate,sweep,flow,fdO2"),
    ("clif-hospitalization", "patient_id,hospitalization_id,hospitalization_joined_id,admission_dttm,discharge_dttm,age_at_admission,admission_type_name,admission_type_category,discharge_name,discharge_category,zipcode_nine_digit,zipcode_five_digit,census_block_code,census_block_group_code,census_tract,state_code,county_code,fips_version"),
    ("clif-hospital-diagnosis", "hospitalization_id,diagnosis_code,diagnosis_code_format,diagnosis_primary,poa_present"),
    ("clif-labs", "hospitalization_id,lab_order_dttm,lab_collect_dttm,lab_result_dttm,lab_order_name,lab_order_category,lab_name,lab_category,lab_value,lab_value_numeric,reference_unit,lab_specimen_name,lab_specimen_category,lab_loinc_code"),
    ("clif-medication-admin", "hospitalization_id,med_order_id,admin_dttm,med_name,med_category,med_group,med_route_name,med_route_category,med_dose,med_dose_unit,mar_action_name,mar_action_category,mar_action_group"),
    ("clif-microbiology-culture", "patient_id,hospitalization_id,organism_id,order_dttm,collect_dttm,result_dttm,fluid_name,fluid_category,method_name,method_category,organism_name,organism_category,organism_group,lab_loinc_code"),
    ("clif-microbiology-nonculture", "patient_id,hospitalization_id,result_dttm,collect_dttm,order_dttm,fluid_name,fluid_category,method_name,method_category,micro_order_name,organism_category,organism_group,result_name,result_category,reference_low,reference_high,result_units,lab_loinc_code"),
    ("clif-patient-assessments", "hospitalization_id,recorded_dttm,assessment_name,assessment_category,assessment_group,numerical_value,categorical_value,text_value"),
    ("clif-patient", "patient_id,race_name,race_category,ethnicity_name,ethnicity_category,sex_name,sex_category,birth_date,death_dttm,language_name,language_category"),
    ("clif-position", "hospitalization_id,recorded_dttm,position_name,position_category"),
    ("clif-respiratory-support", "hospitalization_id,recorded_dttm,device_name,device_id,device_category,vent_brand_name,mode_name,mode_category,tracheostomy,fio2_set,lpm_set,tidal_volume_set,resp_rate_set,pressure_control_set,pressure_support_set,flow_rate_set,peak_inspiratory_pressure_set,inspiratory_time_set,peep_set,tidal_volume_obs,resp_rate_obs,plateau_pressure_obs,peak_inspiratory_pressure_obs,peep_obs,minute_vent_obs,mean_airway_pressure_obs"),
    ("clif-vitals", "hospitalization_id,recorded_dttm,vital_name,vital_category,vital_value,meas_site_name"),
    ("clif-intake-output", "hospitalization_id,intake_dttm,fluid_name,amount,in_out_flag"),
    ("clif-invasive-hemodynamics", "hospitalization_id,recorded_dttm,measure_name,measure_category,measure_value"),
    ("clif-key-icu-orders", "hospitalization_id,order_dttm,order_name,order_category,order_status_name"),
    ("clif-medication-admin", "hospitalization_id,med_order_id,admin_dttm,med_name,med_category,med_group,med_route_name,med_route_category,med_dose,med_dose_unit,mar_action_name,mar_action_category,mar_action_group"),
    ("clif-medication-orders", "hospitalization_id,med_order_id,order_start_dttm,order_end_dttm,ordered_dttm,med_name,med_category,med_group,med_order_status_name,med_order_status_category,med_route_name,med_dose,med_dose_unit,med_frequency,prn"),
    ("clif-patient-diagnosis", "patient_id,hospitalization_id,diagnosis_code,diagnosis_code_format,source_type,start_dttm,end_dttm"),
    ("clif-patient-procedures", "hospitalization_id,billing_provider_id,performing_provider_id,procedure_code,procedure_code_format,procedure_billed_dttm"),
    ("clif-place-based-index", "hospitalization_id,index_name,index_value,index_version"),
    ("clif-provider", "hospitalization_id,provider_id,start_dttm,stop_dttm,provider_role_name,provider_role_category"),
    ("clif-microbiology-susceptibility", "organism_id,antimicrobial_name,antimicrobial_category,sensitivity_name,susceptibility_name,susceptibility_category"),
    ("clif-therapy-details", "hospitalization_id,session_start_dttm,therapy_element_name,therapy_element_category,therapy_element_value"),
    ("clif-transfusion", "hospitalization_id,transfusion_start_dttm,transfusion_end_dttm,component_name,attribute_name,volume_transfused,volume_units,product_code"),
    ("clif-clinical-trial", "hospitalization_id,trial_id,trial_name,arm_id,consent_dttm,randomized_dttm,withdrawal_dttm"),
    ("clif-clinical-notes-facts", "hospitalization_id,author_id,note_id,note_type,note_type_category,note_status,cosigner_id,cosigner_specialty,note_author_specialty,revision_id,creation_dttm,revision_dttm,service_date"),
    ("clif-input", "hospitalization_id,recorded_dttm,input_name,input_category,input_group,input_volume,lda_id"),
    ("clif-output", "hospitalization_id,recorded_dttm,output_name,output_category,output_group,output_value,output_unit,lda_id"),
    ("clif-intermittent-dialysis", "hospitalization_id,device_id,recorded_dttm,blood_flow_rate,dialysate_flow_rate,net_ultrafiltration_out"),
    ("clif-validated-diagnosis", "hospitalization_id,diagnosis_name,diagnosis_category,diagnosis_start_dttm,diagnosis_end_dttm,validation_method,diagnosis_status,reviewer_id,review_timestamp"),
];

#[test]
fn every_clif_table_is_recognised_by_its_columns_alone() {
    use medorama::formats::Format;
    let all = medorama::formats::builtin();
    for (key, cols) in SCHEMAS {
        let h: Vec<String> = cols.split(',').map(String::from).collect();
        let found: Vec<&str> = all.iter().filter(|f| f.detect(&h)).map(|f| f.key()).collect();
        assert_eq!(found, [*key], "{cols}");
        // the columns a table must have: none of them taken, it is no other table either
        let t = medorama::formats::clif::tables().into_iter().find(|t| t.key() == *key).unwrap();
        assert!(t.start(&h).is_ok(), "{key}");
    }
    // every format is one of CLIF's tables
    for t in medorama::formats::clif::tables() {
        assert!(SCHEMAS.iter().any(|(k, _)| *k == t.key()), "{} has no schema here", t.key());
    }
}

/// The rest of CLIF: its other tables, two patients' worth (the cultures as Parquet).
fn clif_rest(dir: &Path) {
    let w = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    w("patient.csv", "patient_id,sex_category,birth_date\nC001,Female,1950-03-04\nC002,Male,1981-07-09\n");
    w(
        "hospitalization.csv",
        "patient_id,hospitalization_id,admission_dttm,discharge_dttm\n\
         C001,H1,2024-01-01 20:00:00+00:00,2024-01-08 15:00:00+00:00\n\
         C002,H2,2024-01-02 06:00:00+00:00,2024-01-04 12:00:00+00:00\n",
    );
    text_parquet(
        &dir.join("microbiology_culture.parquet"),
        "patient_id,hospitalization_id,organism_id,order_dttm,collect_dttm,result_dttm,fluid_name,fluid_category,method_name,method_category,organism_name,organism_category,organism_group,lab_loinc_code\n\
         C001,H1,ORG1,2024-01-02 05:00:00+00:00,2024-01-02 05:30:00+00:00,2024-01-04 09:00:00+00:00,BLOOD,blood,CULTURE,culture,E COLI,escherichia_coli,escherichia,600-7\n\
         C001,H1,ORG2,2024-01-02 05:00:00+00:00,2024-01-02 05:30:00+00:00,2024-01-04 09:00:00+00:00,BLOOD,blood,CULTURE,culture,K PNEUMONIAE,klebsiella_pneumoniae,klebsiella,600-7\n\
         C002,H2,ORG3,2024-01-02 07:00:00+00:00,2024-01-02 07:10:00+00:00,2024-01-03 10:00:00+00:00,URINE,urine,CULTURE,culture,E FAECALIS,enterococcus_faecalis,enterococcus,630-4",
    );
    w(
        "microbiology_susceptibility.csv",
        "organism_id,antimicrobial_name,antimicrobial_category,sensitivity_name,susceptibility_name,susceptibility_category\n\
         ORG1,CEFTRIAXONE,ceftriaxone,<=1,S,susceptible\n\
         ORG1,AMPICILLIN,ampicillin,>=32,R,non_susceptible\n\
         ORG2,CEFTRIAXONE,ceftriaxone,<=1,S,susceptible\n\
         ORG3,VANCOMYCIN,vancomycin,1,S,susceptible\n\
         ORG9,MEROPENEM,meropenem,,S,susceptible\n",
    );
    w(
        "microbiology_nonculture.csv",
        "patient_id,hospitalization_id,result_dttm,collect_dttm,order_dttm,fluid_name,fluid_category,method_name,method_category,micro_order_name,organism_category,organism_group,result_name,result_category,reference_low,reference_high,result_units,lab_loinc_code\n\
         C001,H1,2024-01-02 09:00:00+00:00,2024-01-02 06:00:00+00:00,2024-01-02 05:50:00+00:00,NP SWAB,respiratory_tract,PCR,pcr,RESP PANEL,influenza_a,influenza,Not Detected,not_detected,,,,\n\
         C001,H1,2024-01-02 09:00:00+00:00,2024-01-02 06:00:00+00:00,2024-01-02 05:50:00+00:00,NP SWAB,respiratory_tract,PCR,pcr,RESP PANEL,sars_cov_2,coronavirus,Detected,detected,,,,\n",
    );
    w(
        "ecmo_mcs.csv",
        "hospitalization_id,recorded_dttm,device_name,device_category,mcs_group,ecmo_configuration_category,control_parameter_name,control_parameter_category,control_parameter_value,flow,sweep_set,fdO2_set\n\
         H1,2024-01-03 08:00:00+00:00,CARDIOHELP,VA_ECMO,ECMO,va,Pump speed,rpm,3500,4.2,3.0,0.6\n",
    );
    w(
        "intake_output.csv",
        "hospitalization_id,intake_dttm,fluid_name,amount,in_out_flag\n\
         H1,2024-01-02 10:00:00+00:00,Normal Saline,1000,1\n\
         H1,2024-01-02 11:00:00+00:00,Urine,450,0\n\
         H1,2024-01-02 12:00:00+00:00,Albumin,,1\n",
    );
    w(
        "input.csv",
        "hospitalization_id,recorded_dttm,input_name,input_category,input_group,input_volume,lda_id\n\
         H1,2024-01-02 13:00:00+00:00,LR BOLUS,lactated_ringers,iv_fluids,500,LDA4242\n",
    );
    w(
        "output.csv",
        "hospitalization_id,recorded_dttm,output_name,output_category,output_group,output_value,output_unit,lda_id\n\
         H1,2024-01-02 14:00:00+00:00,FOLEY,indwelling_urinary_catheter,urine,300,mL,LDA4243\n\
         H1,2024-01-02 15:00:00+00:00,EMESIS,emesis,gi,2,occurrences,\n",
    );
    w(
        "invasive_hemodynamics.csv",
        "hospitalization_id,recorded_dttm,measure_name,measure_category,measure_value\n\
         H1,2024-01-02 16:00:00+00:00,Right Atrium,cvp,12\n\
         H1,2024-01-02 16:00:00+00:00,PA catheter,pcwp,18\n",
    );
    w(
        "key_icu_orders.csv",
        "hospitalization_id,order_dttm,order_name,order_category,order_status_name\n\
         H1,2024-01-04 09:00:00+00:00,PT EVAL AND TREAT,pt_evaluation,completed\n",
    );
    w(
        "medication_orders.csv",
        "hospitalization_id,med_order_id,order_start_dttm,order_end_dttm,ordered_dttm,med_name,med_category,med_group,med_order_status_name,med_order_status_category,med_route_name,med_dose,med_dose_unit,med_frequency,prn\n\
         H1,MO1,2024-01-02 06:00:00+00:00,2024-01-06 06:00:00+00:00,2024-01-02 05:45:00+00:00,vancomycin 1 g,vancomycin,antibiotics,Active,active,Intravenous,1000,mg,Every 12 hours,0\n",
    );
    w(
        "patient_diagnosis.csv",
        "patient_id,hospitalization_id,diagnosis_code,diagnosis_code_format,source_type,start_dttm,end_dttm\n\
         C001,H1,I10,ICD10CM,problem_list,2020-05-01 00:00:00+00:00,\n",
    );
    w(
        "place_based_index.csv",
        "hospitalization_id,index_name,index_value,index_version\nH1,Area Deprivation Index,73,ADI 2019\n",
    );
    w(
        "provider.csv",
        "hospitalization_id,provider_id,start_dttm,stop_dttm,provider_role_name,provider_role_category\n\
         H1,PRV9071,2024-01-02 04:00:00+00:00,2024-01-05 10:00:00+00:00,Attending Physician,attending\n",
    );
    w(
        "therapy_details.csv",
        "hospitalization_id,session_start_dttm,therapy_element_name,therapy_element_category,therapy_element_value\n\
         H1,2024-01-04 10:00:00+00:00,Distance walked,ambulation_distance_ft,150\n",
    );
    w(
        "transfusion.csv",
        "hospitalization_id,transfusion_start_dttm,transfusion_end_dttm,component_name,attribute_name,volume_transfused,volume_units,product_code\n\
         H1,2024-01-02 18:00:00+00:00,2024-01-02 20:00:00+00:00,Red Blood Cells,Leukocyte Reduced,300,mL,E0382\n",
    );
    w(
        "clinical_trial.csv",
        "hospitalization_id,trial_id,trial_name,arm_id,consent_dttm,randomized_dttm,withdrawal_dttm\n\
         H1,NCT00000001,Synthetic sedation trial,B,2024-01-02 07:00:00+00:00,2024-01-02 08:00:00+00:00,\n",
    );
    w(
        "clinical_notes_facts.csv",
        "hospitalization_id,author_id,note_id,note_type,note_type_category,note_status,cosigner_id,cosigner_specialty,note_author_specialty,revision_id,creation_dttm,revision_dttm,service_date\n\
         H1,AUTH5521,N1,PN,progress_note,signed,AUTH5522,Critical Care,Critical Care,1,2024-01-03 09:00:00+00:00,2024-01-03 09:30:00+00:00,2024-01-03\n\
         H1,AUTH5521,N1,PN,progress_note,addended,AUTH5522,Critical Care,Critical Care,2,2024-01-03 09:00:00+00:00,2024-01-03 17:00:00+00:00,2024-01-03\n",
    );
    w(
        "intermittent_dialysis.csv",
        "hospitalization_id,device_id,recorded_dttm,blood_flow_rate,dialysate_flow_rate,net_ultrafiltration_out\n\
         H2,DEV7781,2024-01-03 09:00:00+00:00,350,500,2000\n",
    );
    w(
        "validated_diagnosis.csv",
        "hospitalization_id,diagnosis_name,diagnosis_category,diagnosis_start_dttm,diagnosis_end_dttm,validation_method,diagnosis_status,reviewer_id,review_timestamp\n\
         H1,Sepsis,sepsis,2024-01-02 05:00:00+00:00,,manual_chart_review,confirmed,REV3310,2024-02-01 12:00:00+00:00\n",
    );
}

#[test]
fn the_rest_of_clif() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let dir = root.join("clif");
    std::fs::create_dir_all(&dir).unwrap();
    clif_rest(&dir);
    let admin = User::local();
    let done = medorama::formats::clif::import_folder(&mut c, &cfg, &admin, &dir, "clif", false, None).unwrap();
    let names: Vec<&str> = done.imported.iter().map(|(n, ..)| n.as_str()).collect();
    assert_eq!(&names[..2], ["clif_patient", "clif_hospitalization"], "who and which stay first");
    assert_eq!(names.last(), Some(&"clif_microbiology_susceptibility"), "after the cultures");
    assert_eq!(names.len(), 21, "{names:?}");
    assert!(done.skipped.is_empty());
    let report = |name: &str| &done.imported.iter().find(|(n, ..)| n == name).unwrap().2;
    for (name, items) in [
        ("clif_microbiology_culture", 3),
        ("clif_microbiology_susceptibility", 4),
        ("clif_microbiology_nonculture", 2),
        ("clif_ecmo_mcs", 5),
        ("clif_intake_output", 2),
        ("clif_input", 1),
        ("clif_output", 2),
        ("clif_invasive_hemodynamics", 2),
        ("clif_key_icu_orders", 1),
        ("clif_medication_orders", 1),
        ("clif_patient_diagnosis", 1),
        ("clif_place_based_index", 1),
        ("clif_provider", 1),
        ("clif_therapy_details", 1),
        ("clif_transfusion", 1),
        ("clif_clinical_trial", 1),
        ("clif_clinical_notes_facts", 2),
        ("clif_intermittent_dialysis", 3),
        ("clif_validated_diagnosis", 1),
    ] {
        assert_eq!(report(name).items_added, items, "{name}: {:?}", report(name).warnings);
    }
    let w = report("clif_microbiology_susceptibility").warnings.join("|");
    assert!(w.contains("1 rows without their culture"), "{w}");
    // staff and devices' IDs, links to tables not read: never kept
    let stored: String = c.query_row("SELECT group_concat(cells, '|') FROM import_rows", [], |r| r.get(0)).unwrap();
    for gone in ["PRV9071", "AUTH5521", "AUTH5522", "REV3310", "DEV7781", "LDA4242", "LDA4243"] {
        assert!(!stored.contains(gone), "{gone} kept");
    }
    // a susceptibility on its culture: the culture's patient, encounter and time
    let on: Vec<(String, String, String)> = {
        let mut st = c
            .prepare(
                "SELECT i.name, i.value_text, e.encounter_id FROM ehr_items i JOIN encounters e ON e.id = i.encounter
                 WHERE i.name LIKE '%: %' AND i.category = 'micro' ORDER BY i.name",
            )
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    let on: Vec<(&str, &str, &str)> = on.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str())).collect();
    assert_eq!(
        on,
        [
            ("Enterococcus faecalis: Vancomycin", "Susceptible", "H2"),
            ("Escherichia coli: Ampicillin", "Non susceptible", "H1"),
            ("Escherichia coli: Ceftriaxone", "Susceptible", "H1"),
            ("Klebsiella pneumoniae: Ceftriaxone", "Susceptible", "H1"),
        ]
    );
    let same_time: i64 = c
        .query_row(
            "SELECT COUNT(DISTINCT time) FROM ehr_items WHERE encounter = (SELECT id FROM encounters WHERE encounter_id = 'H1')
               AND category = 'micro' AND (name LIKE '%: %' OR name = 'Culture blood')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(same_time, 1, "the cultures' collection time");
    // the patient's page: every new table's entries
    let p: i64 = c.query_row("SELECT id FROM patients WHERE patient_id = 'C001'", [], |r| r.get(0)).unwrap();
    let page = serde_json::to_string(&medorama::patient::report(&c, &admin, p, "").unwrap()).unwrap();
    for x in [
        "Culture blood",
        "Escherichia coli: Ceftriaxone",
        "Influenza a",
        "Not detected",
        "Circulatory support",
        "VA_ECMO",
        "Sweep gas flow (set)",
        "Rpm",
        "Intake: Normal Saline",
        "Pt evaluation",
        "Vancomycin order",
        "I10",
        "Area Deprivation Index",
        "Care team",
        "Attending",
        "Ambulation distance ft",
        "Red Blood Cells",
        "Clinical trial",
        "Synthetic sedation trial",
        "Progress note",
        "Sepsis",
        "Confirmed",
    ] {
        assert!(page.contains(x), "{x} missing from the report");
    }
    // codes and the ends of what lasts
    let blood: (String, String, f64, String) = c
        .query_row(
            "SELECT code, vocabulary, value_num, unit FROM ehr_items WHERE category = 'blood' AND end_time IS NOT NULL",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(blood, ("E0382".into(), "ISBT 128".into(), 300.0, "mL".into()));
    let lasting: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM ehr_items WHERE end_time IS NOT NULL AND name IN ('Vancomycin order', 'Care team')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lasting, 2);
    // a day's vital signs are summed up on the page: the flowsheet's entries, by their names
    let vit: Vec<(String, String)> = {
        let mut st = c
            .prepare("SELECT name, unit FROM ehr_items WHERE category = 'vit' AND patient = ?1 ORDER BY name")
            .unwrap();
        st.query_map([p], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };
    let vit: Vec<(&str, &str)> = vit.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(
        vit,
        [
            ("CVP", "mmHg"),
            ("Intake: Normal Saline", "mL"),
            ("Intake: lactated ringers", "mL"),
            ("Output: Urine", "mL"),
            ("Output: emesis", "occurrences"),
            ("Output: indwelling urinary catheter", "mL"),
            ("PCWP", "mmHg"),
        ]
    );
    assert!(!page.contains("Enterococcus"), "another patient's culture");
    let kinds = medorama::patient::list(&c, &admin, "", &[], 10, 0).unwrap().1;
    let k = kinds.iter().find(|r| r.patient_id == "C001").unwrap();
    for cat in ["vit", "support", "med", "blood", "obs", "micro", "dx", "note"] {
        assert!(k.kinds.iter().any(|x| x == cat), "{cat} not among {:?}", k.kinds);
    }
    let p2: i64 = c.query_row("SELECT id FROM patients WHERE patient_id = 'C002'", [], |r| r.get(0)).unwrap();
    let page2 = serde_json::to_string(&medorama::patient::report(&c, &admin, p2, "").unwrap()).unwrap();
    for x in ["Blood flow rate (IHD)", "Net ultrafiltration (IHD)", "Enterococcus faecalis: Vancomycin"] {
        assert!(page2.contains(x), "{x} missing from the second report");
    }
    // cohorts on the new entries
    let ids = |text: &str| {
        let q = Criteria {
            include: Some(Expr::Leaf(Criterion { kind: "expression".into(), text: text.into(), ..Default::default() })),
            ..Default::default()
        };
        let mut v: Vec<String> = cohorts::search(&c, &admin, "UTC", &q, None, 100, 0)
            .unwrap()
            .1
            .into_iter()
            .map(|m| m.patient.patient_id)
            .collect();
        v.sort();
        v
    };
    assert_eq!(ids("has blood"), ["C001"]);
    assert_eq!(ids("has micro"), ["C001", "C002"]);
    assert_eq!(ids("pcwp > 15"), ["C001"]);
    // in Privacy Mode: no real date, no culture's or trial's identifier
    let g = medorama::gate::Restricted {
        settings: medorama::deid::Settings::default(),
        dict: Arc::new(medorama::gate::Dict::build(&c, 0).unwrap()),
        secret: vec![7; 32],
    }
    .for_patient(p);
    let mut shown = serde_json::to_value(medorama::patient::report(&c, &admin, p, "").unwrap()).unwrap();
    g.json(&mut shown);
    let text = shown.to_string();
    for leak in ["2024-01-0", "ORG1", "NCT00000001", "1950-03-04"] {
        assert!(!text.contains(leak), "Privacy Mode shows {leak}");
    }
    assert!(text.contains("Escherichia coli: Ceftriaxone"), "the entries still shown");
    // susceptibilities alone (no cultures in the folder): left out, said so
    let alone = root.join("alone");
    std::fs::create_dir_all(&alone).unwrap();
    std::fs::copy(dir.join("microbiology_susceptibility.csv"), alone.join("microbiology_susceptibility.csv")).unwrap();
    let done = medorama::formats::clif::import_folder(&mut c, &cfg, &admin, &alone, "clif", true, None).unwrap();
    let (_, _, r) = &done.imported[0];
    assert_eq!(r.items_added, 0);
    assert!(r.warnings.join("|").contains("5 rows without their culture"), "{:?}", r.warnings);
}

/// CLIF's times are UTC: written without their offset, they are still read as UTC (not in the
/// catalog's zone).  A folder is one import: a table that fails part way removes the tables
/// imported before it.
#[test]
fn clif_times_are_utc_and_a_folder_is_one_import() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "America/New_York".into();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let dir = root.join("clif");
    std::fs::create_dir_all(&dir).unwrap();
    let w = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    w("clif_patient.csv", "patient_id,sex_category,birth_date\nC001,Female,1950-03-04\n");
    w(
        "clif_hospitalization.csv",
        "patient_id,hospitalization_id,admission_dttm,discharge_dttm,discharge_category\n\
         C001,H1,2024-01-01 20:00:00,2024-01-08T15:00:00Z,Home\n",
    );
    let admin = User::local();
    medorama::formats::clif::import_folder(&mut c, &cfg, &admin, &dir, "clif", false, None).unwrap();
    let (admit, discharge): (f64, f64) = c
        .query_row("SELECT admit_time, discharge_time FROM encounters WHERE encounter_id = 'H1'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(admit, 1_704_139_200.0, "2024-01-01 20:00 UTC, not New York's");
    assert_eq!(discharge, 1_704_726_000.0);
    // a second folder whose last table cannot be read: none of its tables is kept
    let dir2 = root.join("clif2");
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("clif_patient.csv"), "patient_id,sex_category,birth_date\nC009,Male,1960-01-01\n")
        .unwrap();
    std::fs::write(
        dir2.join("clif_hospitalization.csv"),
        "patient_id,hospitalization_id,admission_dttm,discharge_dttm\nC009,H9,2024-01-01 20:00:00,\"unclosed\n",
    )
    .unwrap();
    std::fs::write(
        dir2.join("clif_vitals.csv"),
        b"hospitalization_id,recorded_dttm,vital_category,vital_value\nH9,2024-01-02 10:00:00,heart_rate,\xff\n",
    )
    .unwrap();
    let Err(e) = medorama::formats::clif::import_folder(&mut c, &cfg, &admin, &dir2, "clif2", false, None) else {
        panic!("the folder imported")
    };
    assert!(e.to_string().contains("nothing of the folder was kept"), "{e}");
    let n: i64 = c.query_row("SELECT COUNT(*) FROM patients WHERE patient_id = 'C009'", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0, "the folder's first tables are removed too");
    let loading: i64 = c.query_row("SELECT COUNT(*) FROM imports WHERE state = 'loading'", [], |r| r.get(0)).unwrap();
    assert_eq!(loading, 0);
}
