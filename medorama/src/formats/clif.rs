//! CLIF, the Common Longitudinal ICU data Format (data dictionary 2.x:
//! <https://github.com/Common-Longitudinal-ICU-data-Format>): one format per CLIF table, each
//! recognised by its columns, CSV or Parquet ([`crate::parquet_rows`]).  CLIF's times are UTC
//! (`YYYY-MM-DD HH:MM:SS+00:00`), read as such; its standard categories (mCIDE: `vital_category`,
//! `lab_category`, `med_category`, `location_category` ...) name the entries, so that sites'
//! data read alike; the raw names stay as the database's own columns.  Every table of CLIF 2.1
//! is read, and the 3.0 tables marked so (their columns as in clifpy's 3.0 schemas).
//!
//! | Table | Becomes |
//! |---|---|
//! | `patient` | the patient (`patient_id`): sex, date of birth; race, ethnicity, language, death as columns |
//! | `hospitalization` | an encounter (`hospitalization_id`): admission, discharge, disposition |
//! | `adt` | bed stays, by `location_category` (`icu` shows as ICU) |
//! | `vitals` | vital signs, by `vital_category`, in CLIF's units |
//! | `invasive_hemodynamics` | vital signs: CVP, right heart and pulmonary artery pressures, wedge, cardiac output |
//! | `intake_output`; `input`, `output` (3.0) | vital signs (flowsheet): each intake or output (`Intake: …`, `Output: …`), in mL (or occurrences: emesis ...) |
//! | `labs` | results, by `lab_category`, in its reference unit (text results kept as text) |
//! | `respiratory_support` | life support: the device, the mode, and each setting or observation an entry |
//! | `ecmo_mcs` | life support: the device, its blood flow, sweep, FdO2 and work rate (RPM ...) |
//! | `crrt_therapy` | life support: the mode and each rate |
//! | `intermittent_dialysis` (3.0) | life support: each rate (`… (IHD)`) |
//! | `medication_admin_continuous`, `_intermittent` | drugs, by `med_category`, dose and unit; a dose not given (`mar_action_group` `not_administered`: stopped, held) is the event without a dose |
//! | `medication_orders` | drugs ordered (`… order`): dose, route, frequency, from start to end |
//! | `transfusion` | transfusions: the component, its volume, its ISBT 128 product code |
//! | `patient_assessments`, `position`, `code_status` | assessments |
//! | `key_icu_orders`, `therapy_details` | assessments: orders by category and status; therapy sessions' elements |
//! | `provider`, `place_based_index`, `clinical_trial` | assessments: the care team's roles, the area indices (ADI, SVI), trial enrolment |
//! | `microbiology_culture` | cultures: the organism, by fluid and method (its `organism_id` kept) |
//! | `microbiology_nonculture` | microbiology: each organism tested (PCR ...), detected or not |
//! | `microbiology_susceptibility` | microbiology: each antimicrobial's result, on its culture (`organism_id`) |
//! | `patient_procedures`, `hospital_diagnosis` | procedures and conditions, by their codes |
//! | `patient_diagnosis`; `validated_diagnosis` (3.0) | conditions: by their codes, from start to end; validated labels and their status |
//! | `clinical_notes_facts` (3.0) | notes: each revision, by note type (CLIF holds no note text) |
//!
//! A susceptibility row names only its organism: [`import_folder`] reads the folder's cultures
//! first and puts each result on its culture's encounter and time; imported alone, its rows are
//! left out.
//!
//! Left out: `hospitalization`'s geography (ZIP codes, census blocks: identifiers under HIPAA),
//! devices' serial IDs (`device_id`: identifiers under HIPAA), `age_at_admission` (ages come
//! from the date of birth, capped in Privacy Mode), staff IDs (`provider_id`, `author_id`,
//! `cosigner_id`, `reviewer_id`: formats keep no staff) and `lda_id` (lines, drains and
//! airways, not read).

use super::{Converter, Format};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
enum Table {
    Patient,
    Hospitalization,
    Adt,
    Vitals,
    Hemodynamics,
    IntakeOutput,
    Input,
    Output,
    Labs,
    Respiratory,
    Ecmo,
    Crrt,
    Dialysis,
    Meds,
    MedOrders,
    Transfusion,
    Assessments,
    Position,
    CodeStatus,
    IcuOrders,
    Therapy,
    Provider,
    PlaceIndex,
    Trial,
    Micro,
    NonCulture,
    Susceptibility,
    Procedures,
    Diagnosis,
    PatientDiagnosis,
    ValidatedDiagnosis,
    NoteFacts,
}

/// The key of the table linked to the cultures by `organism_id`.
const SUSCEPTIBILITY: &str = "clif-microbiology-susceptibility";

pub struct Clif {
    table: Table,
    key: &'static str,
    label: &'static str,
    category: &'static str,
    detect: &'static [&'static str],
    /// Columns of another table: with one of them, not this one.
    unless: &'static [&'static str],
    about: &'static str,
    /// The cultures a susceptibility table is read against (by `organism_id`; [`import_folder`]).
    link: Option<Arc<HashMap<String, Culture>>>,
}

/// A culture as its susceptibilities are shown with it: whose, when, which, what grew.
#[derive(Clone, Default)]
pub struct Culture {
    patient: String,
    encounter: String,
    time: String,
    name: String,
    organism: String,
}

/// Every CLIF table Medorama reads.
pub fn tables() -> Vec<Clif> {
    use Table::*;
    let t = |table, key, label, category, detect, unless, about| Clif {
        table,
        key,
        label,
        category,
        detect,
        unless,
        about,
        link: None,
    };
    vec![
        t(
            Patient,
            "clif-patient",
            "CLIF patient",
            "",
            &["patient_id", "sex_category", "birth_date"],
            &["hospitalization_id"],
            "CLIF's patients: sex, date of birth; race, ethnicity, language and death as columns.",
        ),
        t(
            Hospitalization,
            "clif-hospitalization",
            "CLIF hospitalization",
            "",
            &["patient_id", "hospitalization_id", "admission_dttm"],
            &[],
            "CLIF's hospitalizations, as encounters: admission, discharge, disposition. Geography left out.",
        ),
        t(
            Adt,
            "clif-adt",
            "CLIF ADT",
            "",
            &["hospitalization_id", "in_dttm", "location_category"],
            &[],
            "CLIF's admission, discharge and transfer events, as bed stays by location category.",
        ),
        t(
            Vitals,
            "clif-vitals",
            "CLIF vitals",
            "vit",
            &["hospitalization_id", "vital_category", "vital_value"],
            &[],
            "CLIF's vital signs, by vital_category, in CLIF's units.",
        ),
        t(
            Hemodynamics,
            "clif-invasive-hemodynamics",
            "CLIF invasive hemodynamics",
            "vit",
            &["hospitalization_id", "measure_category", "measure_value"],
            &[],
            "CLIF's invasive hemodynamic measurements (CVP, PA pressures, wedge, cardiac output), by measure_category.",
        ),
        t(
            IntakeOutput,
            "clif-intake-output",
            "CLIF intake and output",
            "vit",
            &["hospitalization_id", "in_out_flag", "amount"],
            &[],
            "CLIF 2.1's fluid intake and output, each by its fluid, in mL.",
        ),
        t(
            Input,
            "clif-input",
            "CLIF input",
            "vit",
            &["hospitalization_id", "input_category", "input_volume"],
            &[],
            "CLIF 3.0's fluid intake, by input_category, in mL.",
        ),
        t(
            Output,
            "clif-output",
            "CLIF output",
            "vit",
            &["hospitalization_id", "output_category", "output_value"],
            &[],
            "CLIF 3.0's fluid output (urine, drains ...), by output_category, in mL or occurrences.",
        ),
        t(
            Labs,
            "clif-labs",
            "CLIF labs",
            "lab",
            &["hospitalization_id", "lab_category"],
            &[],
            "CLIF's laboratory results, by lab_category, in their reference units.",
        ),
        t(
            Respiratory,
            "clif-respiratory-support",
            "CLIF respiratory support",
            "support",
            &["hospitalization_id", "device_category", "recorded_dttm"],
            &["mcs_group"],
            "CLIF's respiratory support: the device, the mode, each setting and observation an entry.",
        ),
        t(
            Ecmo,
            "clif-ecmo-mcs",
            "CLIF ECMO and MCS",
            "support",
            &["hospitalization_id", "mcs_group"],
            &[],
            "CLIF's ECMO and mechanical circulatory support: the device, its flow, sweep, FdO2 and work rate.",
        ),
        t(
            Crrt,
            "clif-crrt-therapy",
            "CLIF CRRT therapy",
            "support",
            &["hospitalization_id", "crrt_mode_category"],
            &[],
            "CLIF's continuous renal replacement therapy: the mode and each rate.",
        ),
        t(
            Dialysis,
            "clif-intermittent-dialysis",
            "CLIF intermittent dialysis",
            "support",
            &["hospitalization_id", "net_ultrafiltration_out"],
            &[],
            "CLIF 3.0's intermittent hemodialysis: each rate.",
        ),
        t(
            Meds,
            "clif-medication-admin",
            "CLIF medication administration",
            "med",
            &["hospitalization_id", "med_category", "admin_dttm"],
            &[],
            "CLIF's medication administrations (continuous and intermittent), by med_category.",
        ),
        t(
            MedOrders,
            "clif-medication-orders",
            "CLIF medication orders",
            "med",
            &["hospitalization_id", "med_order_id", "order_start_dttm"],
            &[],
            "CLIF's medication orders, by med_category: dose, route, frequency, from start to end.",
        ),
        t(
            Transfusion,
            "clif-transfusion",
            "CLIF transfusion",
            "blood",
            &["hospitalization_id", "component_name", "transfusion_start_dttm"],
            &[],
            "CLIF's transfusions: the blood component, its volume, its ISBT 128 product code.",
        ),
        t(
            Assessments,
            "clif-patient-assessments",
            "CLIF patient assessments",
            "obs",
            &["hospitalization_id", "assessment_category"],
            &[],
            "CLIF's bedside assessments (GCS, RASS, CAM ...), by assessment_category.",
        ),
        t(
            Position,
            "clif-position",
            "CLIF position",
            "obs",
            &["hospitalization_id", "position_category"],
            &[],
            "CLIF's patient positions (prone, not prone).",
        ),
        t(
            CodeStatus,
            "clif-code-status",
            "CLIF code status",
            "obs",
            &["patient_id", "code_status_category"],
            &[],
            "CLIF's code status orders.",
        ),
        t(
            IcuOrders,
            "clif-key-icu-orders",
            "CLIF key ICU orders",
            "obs",
            &["hospitalization_id", "order_category", "order_dttm"],
            &[],
            "CLIF's key ICU orders (physical and occupational therapy ...), by order_category, with their status.",
        ),
        t(
            Therapy,
            "clif-therapy-details",
            "CLIF therapy details",
            "obs",
            &["hospitalization_id", "therapy_element_category"],
            &[],
            "CLIF's therapy sessions: each element and its value.",
        ),
        t(
            Provider,
            "clif-provider",
            "CLIF provider",
            "obs",
            &["hospitalization_id", "provider_id"],
            &[],
            "CLIF's care team: each role, from start to stop. The providers' IDs left out.",
        ),
        t(
            PlaceIndex,
            "clif-place-based-index",
            "CLIF place-based index",
            "obs",
            &["hospitalization_id", "index_name", "index_value"],
            &[],
            "CLIF's place-based indices (Area Deprivation Index, Social Vulnerability Index ...) of each hospitalization.",
        ),
        t(
            Trial,
            "clif-clinical-trial",
            "CLIF clinical trial",
            "obs",
            &["hospitalization_id", "trial_id"],
            &[],
            "CLIF's clinical trial enrolments: the trial, the arm, consent, randomization and withdrawal.",
        ),
        t(
            Micro,
            "clif-microbiology-culture",
            "CLIF microbiology culture",
            "micro",
            &["organism_category", "collect_dttm"],
            &["result_category", "micro_order_name"],
            "CLIF's cultures: the organism, by fluid and method.",
        ),
        t(
            NonCulture,
            "clif-microbiology-nonculture",
            "CLIF microbiology non-culture",
            "micro",
            &["organism_category", "result_category"],
            &[],
            "CLIF's non-culture tests (PCR ...): each organism, detected or not.",
        ),
        t(
            Susceptibility,
            SUSCEPTIBILITY,
            "CLIF microbiology susceptibility",
            "micro",
            &["organism_id", "antimicrobial_category"],
            &[],
            "CLIF's susceptibilities: each antimicrobial's result, on its culture (by organism_id; imported with a CLIF folder's cultures).",
        ),
        t(
            Procedures,
            "clif-patient-procedures",
            "CLIF patient procedures",
            "proc",
            &["hospitalization_id", "procedure_code"],
            &[],
            "CLIF's billed procedures, by their codes.",
        ),
        t(
            Diagnosis,
            "clif-hospital-diagnosis",
            "CLIF hospital diagnosis",
            "dx",
            &["hospitalization_id", "diagnosis_code"],
            &["start_dttm", "source_type"],
            "CLIF's hospital diagnoses, by their codes (primary, present on admission).",
        ),
        t(
            PatientDiagnosis,
            "clif-patient-diagnosis",
            "CLIF patient diagnosis",
            "dx",
            &["patient_id", "diagnosis_code", "start_dttm"],
            &[],
            "CLIF's diagnoses from clinical documentation (problem list, history), by their codes, from start to end.",
        ),
        t(
            ValidatedDiagnosis,
            "clif-validated-diagnosis",
            "CLIF validated diagnosis",
            "dx",
            &["hospitalization_id", "diagnosis_category", "validation_method"],
            &[],
            "CLIF 3.0's clinician-validated diagnoses: confirmed, ruled out or uncertain. The reviewers' IDs left out.",
        ),
        t(
            NoteFacts,
            "clif-clinical-notes-facts",
            "CLIF clinical notes facts",
            "note",
            &["hospitalization_id", "note_id", "note_type_category"],
            &[],
            "CLIF 3.0's clinical notes' facts: each revision, by note type (CLIF holds no note text). Authors' IDs left out.",
        ),
    ]
}

/// The order a CLIF folder's tables are imported in: who and which stay first; the
/// susceptibilities after the cultures they name.
pub fn rank(key: &str) -> usize {
    match key {
        "clif-patient" => 0,
        "clif-hospitalization" => 1,
        "clif-adt" => 2,
        SUSCEPTIBILITY => 4,
        _ => 3,
    }
}

/// What [`import_folder`] did: each table's database (its name, its files, the import), and the
/// files that are no CLIF table.
pub struct Folder {
    pub imported: Vec<(String, usize, crate::tables::TableReport)>,
    pub skipped: Vec<std::path::PathBuf>,
}

/// A CLIF folder imported (admins): its tables (CSV or Parquet files) recognised by their
/// columns, each a database (`clif_vitals` ...; a table in several files, one database), in
/// order: patients, hospitalizations, ADT, then the rest (the susceptibilities last, each on
/// the culture of the folder its `organism_id` names).  The folder is one import: its tables
/// are made active together at the end, and a failure or a cancel part way removes those
/// imported before it (nothing of the folder is kept).
pub fn import_folder(
    c: &mut crate::sql::Connection,
    cfg: &crate::config::Config,
    user: &crate::users::User,
    dir: &std::path::Path,
    source: &str,
    dry_run: bool,
    progress: Option<std::sync::Arc<crate::progress::Progress>>,
) -> anyhow::Result<Folder> {
    if !user.is_admin() {
        anyhow::bail!("only admins import databases");
    }
    // the folder's tables one after another, no other import between them
    let lock = c.import_lock(|| {
        if let Some(p) = &progress {
            p.phase(crate::progress::Phase::Waiting);
            p.check()?;
        }
        Ok(())
    })?;
    if lock.first() {
        crate::imports::discard_unfinished(c, cfg, true)?;
    }
    let mut batches = vec![];
    let r = import_tables(c, cfg, user, dir, source, dry_run, progress, &mut batches);
    match &r {
        Ok(_) if !dry_run => crate::imports::activate(c, &batches)?,
        Ok(_) => {}
        Err(_) => {
            for b in batches.iter().rev() {
                if let Err(e) = crate::imports::discard(c, cfg, *b) {
                    eprintln!("medorama: import {b} (a table of the folder) could not be removed yet ({e}): it is at the next start");
                }
            }
        }
    }
    r
}

#[allow(clippy::too_many_arguments)]
fn import_tables(
    c: &mut crate::sql::Connection,
    cfg: &crate::config::Config,
    user: &crate::users::User,
    dir: &std::path::Path,
    source: &str,
    dry_run: bool,
    progress: Option<std::sync::Arc<crate::progress::Progress>>,
    batches: &mut Vec<i64>,
) -> anyhow::Result<Folder> {
    use crate::tables::{import_parts, Part, PartData};
    let known = super::all(cfg).0;
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("csv") || x.eq_ignore_ascii_case("parquet")))
        .collect();
    files.sort();
    let mut tables: Vec<(String, Vec<std::path::PathBuf>)> = vec![];
    let mut skipped = vec![];
    for f in files {
        let cols = crate::tables::columns_of(&Part { name: String::new(), data: PartData::File(f.clone()) })?;
        match known.iter().find(|x| x.key().starts_with("clif-") && x.detect(&cols)) {
            Some(fmt) => match tables.iter_mut().find(|(k, _)| k == fmt.key()) {
                Some((_, v)) => v.push(f),
                None => tables.push((fmt.key().to_string(), vec![f])),
            },
            None => skipped.push(f),
        }
    }
    if tables.is_empty() {
        anyhow::bail!("{}: no CLIF table (CSV or Parquet) found", dir.display());
    }
    tables.sort_by_key(|(k, _)| (rank(k), k.clone()));
    let culture_files: Vec<std::path::PathBuf> =
        tables.iter().find(|(k, _)| k == "clif-microbiology-culture").map(|(_, ps)| ps.clone()).unwrap_or_default();
    let mut imported = vec![];
    // each table's part of the progress: its files' share of the folder's size
    let size =
        |ps: &[std::path::PathBuf]| ps.iter().map(|f| std::fs::metadata(f).map_or(0, |m| m.len())).sum::<u64>() as f64;
    let whole = tables.iter().map(|(_, ps)| size(ps)).sum::<f64>().max(1.0);
    let mut at = 0.0;
    for (key, paths) in tables {
        let width = size(&paths) / whole;
        if let Some(p) = &progress {
            p.span(at, width);
        }
        at += width;
        let fmt = if key == SUSCEPTIBILITY {
            let mut t = self::tables().into_iter().find(|t| t.key == SUSCEPTIBILITY).expect("a CLIF table");
            t.link = Some(Arc::new(cultures(&culture_files)?));
            super::Choice::Named(Arc::new(t))
        } else {
            super::Choice::parse(&key, &known)?
        };
        let name = key.replace('-', "_");
        let parts: Vec<Part> = paths
            .iter()
            .map(|f| Part {
                name: f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                data: PartData::File(f.clone()),
            })
            .collect();
        let mut opts = crate::imports::Opts::new(&name, source, dry_run);
        opts.progress = progress.clone();
        opts.keep_loading = true;
        let r = import_parts(c, cfg, user, &parts, &Default::default(), "", None, &opts, fmt)
            .map_err(|e| anyhow::anyhow!("{name}: {e} (nothing of the folder was kept)"))?;
        batches.push(r.import_id);
        imported.push((name, paths.len(), r));
    }
    Ok(Folder { imported, skipped })
}

/// The cultures of a folder's `microbiology_culture` files, by `organism_id`: what its
/// susceptibilities are shown with.
fn cultures(paths: &[std::path::PathBuf]) -> anyhow::Result<HashMap<String, Culture>> {
    use crate::tables::{Part, PartData};
    let mut out = HashMap::new();
    for f in paths {
        let part = Part { name: String::new(), data: PartData::File(f.clone()) };
        let (headers, rows) = crate::tables::rows_of(&part)?;
        let read = Rows::new(Table::Micro, "micro", &headers, None);
        for r in rows {
            let r = r?;
            let id = read.get(&r, "organism_id");
            if !id.is_empty() {
                out.insert(id.to_string(), read.culture(&r));
            }
        }
    }
    Ok(out)
}

impl Format for Clif {
    fn key(&self) -> &str {
        self.key
    }
    fn label(&self) -> &str {
        self.label
    }
    fn category(&self) -> &str {
        self.category
    }
    fn about(&self) -> String {
        self.about.into()
    }
    fn detect(&self, h: &[String]) -> bool {
        let has = |c: &str| h.iter().any(|x| x.trim().eq_ignore_ascii_case(c));
        self.detect.iter().all(|c| has(c)) && !self.unless.iter().any(|c| has(c))
    }
    fn start(&self, h: &[String]) -> anyhow::Result<Box<dyn Converter>> {
        if !self.detect.iter().all(|c| h.iter().any(|x| x.trim().eq_ignore_ascii_case(c))) {
            anyhow::bail!("not a {} table: it needs the columns {}", self.label, self.detect.join(", "));
        }
        Ok(Box::new(Rows::new(self.table, self.category, h, self.link.clone())))
    }
}

struct Rows {
    table: Table,
    category: &'static str,
    at: HashMap<String, usize>,
    link: Option<Arc<HashMap<String, Culture>>>,
}

/// A date and time written without an offset or a zone (`2024-01-02 03:04:05`).
fn without_offset(v: &str) -> bool {
    let v = v.trim();
    let Some(time) = v.get(10..).filter(|_| v.len() >= 16 && v.as_bytes()[4] == b'-') else { return false };
    !(time.ends_with('Z')
        || time.ends_with('z')
        || time.contains('+')
        || time.get(1..).is_some_and(|t| t.contains('-')))
}

/// A CLIF category as a name: `heart_rate` → `Heart rate`.
fn label(category: &str) -> String {
    let s = category.trim().replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Vital signs: CLIF's category, its name here, its unit.
const VITALS: &[(&str, &str, &str)] = &[
    ("temp_c", "Temperature", "°C"),
    ("heart_rate", "Heart rate", "bpm"),
    ("sbp", "Systolic BP", "mmHg"),
    ("dbp", "Diastolic BP", "mmHg"),
    ("map", "Mean arterial pressure", "mmHg"),
    ("spo2", "SpO2", "%"),
    ("respiratory_rate", "Respiratory rate", "/min"),
    ("height_cm", "Height", "cm"),
    ("weight_kg", "Weight", "kg"),
];

/// Respiratory support's settings and observations: column, name, unit.
const RESPIRATORY: &[(&str, &str, &str)] = &[
    ("fio2_set", "FiO2 (set)", "fraction"),
    ("lpm_set", "O2 flow (set)", "L/min"),
    ("tidal_volume_set", "Tidal volume (set)", "mL"),
    ("resp_rate_set", "Respiratory rate (set)", "/min"),
    ("pressure_control_set", "Pressure control (set)", "cmH2O"),
    ("pressure_support_set", "Pressure support (set)", "cmH2O"),
    ("peak_inspiratory_pressure_set", "Peak inspiratory pressure (set)", "cmH2O"),
    ("peep_set", "PEEP (set)", "cmH2O"),
    ("flow_rate_set", "Flow rate (set)", "L/min"),
    ("inspiratory_time_set", "Inspiratory time (set)", "s"),
    ("tidal_volume_obs", "Tidal volume", "mL"),
    ("resp_rate_obs", "Respiratory rate (ventilator)", "/min"),
    ("plateau_pressure_obs", "Plateau pressure", "cmH2O"),
    ("peak_inspiratory_pressure_obs", "Peak inspiratory pressure", "cmH2O"),
    ("peep_obs", "PEEP", "cmH2O"),
    ("mean_airway_pressure_obs", "Mean airway pressure", "cmH2O"),
    ("minute_vent_obs", "Minute ventilation", "L/min"),
];

const CRRT: &[(&str, &str, &str)] = &[
    ("blood_flow_rate", "Blood flow rate", "mL/min"),
    ("pre_filter_replacement_fluid_rate", "Pre-filter replacement fluid rate", "mL/h"),
    ("post_filter_replacement_fluid_rate", "Post-filter replacement fluid rate", "mL/h"),
    ("dialysate_flow_rate", "Dialysate flow rate", "mL/h"),
    ("ultrafiltration_out", "Ultrafiltration", "mL/h"),
];

/// Intermittent hemodialysis's rates (clifpy's 3.0 schema gives no units; blood flow is mL/min
/// everywhere).
const DIALYSIS: &[(&str, &str, &str)] = &[
    ("blood_flow_rate", "Blood flow rate (IHD)", "mL/min"),
    ("dialysate_flow_rate", "Dialysate flow rate (IHD)", ""),
    ("net_ultrafiltration_out", "Net ultrafiltration (IHD)", ""),
];

/// ECMO and MCS: the columns (CLIF 2.1's DDL, then clifpy's names), name, unit.
const ECMO: &[(&[&str], &str, &str)] = &[
    (&["flow"], "Blood flow (MCS)", "L/min"),
    (&["sweep_set", "sweep"], "Sweep gas flow (set)", "L/min"),
    (&["fdo2_set", "fdo2"], "FdO2 (set)", "fraction"),
];

/// Invasive hemodynamics: CLIF's category, its name here, its unit.
const HEMODYNAMICS: &[(&str, &str, &str)] = &[
    ("cvp", "CVP", "mmHg"),
    ("ra", "Right atrial pressure", "mmHg"),
    ("rv", "Right ventricular pressure", "mmHg"),
    ("pa_systolic", "PA systolic pressure", "mmHg"),
    ("pa_diastolic", "PA diastolic pressure", "mmHg"),
    ("pa_mean", "PA mean pressure", "mmHg"),
    ("pcwp", "PCWP", "mmHg"),
    ("cardiac_output_thermodilution", "Cardiac output (thermodilution)", "L/min"),
    ("cardiac_output_fick", "Cardiac output (Fick)", "L/min"),
];

fn number(s: &str) -> Option<&str> {
    let s = s.trim();
    s.parse::<f64>().ok().filter(|x| x.is_finite()).map(|_| s)
}

/// CLIF's 0/1 (or a Parquet boolean) as words.
fn yes(s: &str) -> &'static str {
    match s.trim().to_lowercase().as_str() {
        "1" | "true" => "yes",
        "0" | "false" => "no",
        _ => "",
    }
}

/// The first of these that is not empty.
fn first<'a>(xs: &[&'a str]) -> &'a str {
    xs.iter().copied().find(|x| !x.is_empty()).unwrap_or("")
}

fn sex(s: &str) -> &'static str {
    match s.trim().to_lowercase().as_str() {
        "male" | "m" => "M",
        "female" | "f" => "F",
        _ => "",
    }
}

/// The columns of an entry (an item) and of a table's own columns after them.
const ENTRY: &[&str] =
    &["patient_id", "encounter_id", "category", "time", "name", "value", "unit", "code", "vocabulary"];

impl Rows {
    fn new(table: Table, category: &'static str, h: &[String], link: Option<Arc<HashMap<String, Culture>>>) -> Rows {
        let at = h.iter().enumerate().map(|(i, c)| (c.trim().to_lowercase(), i)).collect();
        Rows { table, category, at, link }
    }

    fn get<'a>(&self, r: &'a [String], col: &str) -> &'a str {
        self.at.get(col).and_then(|i| r.get(*i)).map(|s| s.trim()).unwrap_or("")
    }

    /// A culture's row as its entry names it (and its susceptibilities with it).
    fn culture(&self, r: &[String]) -> Culture {
        let g = |c: &str| self.get(r, c);
        let name =
            format!("{} {}", label(g("method_category")), g("fluid_category").replace('_', " ")).trim().to_string();
        Culture {
            patient: g("patient_id").into(),
            encounter: g("hospitalization_id").into(),
            time: g("collect_dttm").into(),
            name: if name.is_empty() { "Culture".into() } else { name },
            organism: label(g("organism_category")),
        }
    }

    /// Its own columns, after the entry's.
    fn own(&self) -> &'static [&'static str] {
        match self.table {
            Table::Vitals => &["vital_category", "vital_name", "site"],
            Table::Labs => &["lab_category", "lab_name", "specimen", "ordered", "resulted"],
            Table::Respiratory => &["device", "mode", "tracheostomy"],
            Table::Meds => &["med_group", "route", "action", "med_name"],
            Table::Assessments => &["assessment_group"],
            Table::Micro => &["fluid", "method", "organism_group", "organism_id"],
            Table::Diagnosis => &["primary", "present on admission"],
            Table::Crrt => &["mode", "machine"],
            Table::Hemodynamics => &["measure_category", "site"],
            Table::IntakeOutput => &["fluid"],
            Table::Input | Table::Output => &["group", "fluid"],
            Table::Ecmo => &["device", "mcs group", "configuration"],
            Table::MedOrders => {
                &["med_group", "route", "status", "frequency", "prn", "ordered", "med_name", "end_time"]
            }
            Table::Transfusion => &["attribute", "end_time"],
            Table::IcuOrders => &["order_category", "order name"],
            Table::Therapy => &["element"],
            Table::Provider => &["role", "end_time"],
            Table::PlaceIndex => &["version"],
            Table::Trial => &["trial", "arm", "consent", "randomized", "end_time"],
            Table::NonCulture => &["fluid", "method", "organism_group", "order name", "ref_low", "ref_high"],
            Table::Susceptibility => &["organism", "culture", "organism_id", "sensitivity"],
            Table::PatientDiagnosis => &["source type", "end_time"],
            Table::ValidatedDiagnosis => &["validation", "diagnosis_name", "reviewed", "end_time"],
            Table::NoteFacts => &["type code", "service", "status", "revision", "note_id", "created", "service date"],
            _ => &[],
        }
    }

    /// An entry: (patient, encounter, time, name, value, unit, code, vocabulary), then its own
    /// columns' values.
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &self,
        pid: &str,
        enc: &str,
        time: &str,
        name: &str,
        value: &str,
        unit: &str,
        code: &str,
        vocab: &str,
        own: &[&str],
    ) -> Vec<String> {
        let mut v: Vec<String> =
            [pid, enc, self.category, time, name, value, unit, code, vocab].iter().map(|s| s.to_string()).collect();
        v.extend(own.iter().map(|s| s.to_string()));
        v
    }
}

impl Converter for Rows {
    fn headers(&self) -> Vec<String> {
        let mut h: Vec<&str> = match self.table {
            Table::Patient => vec!["patient_id", "sex", "birth_date", "race", "ethnicity", "language", "death"],
            Table::Hospitalization => {
                vec!["patient_id", "encounter_id", "admit_time", "discharge_time", "disposition", "admission type"]
            }
            Table::Adt => vec!["encounter_id", "care_unit", "bed_start", "bed_end", "hospital", "location"],
            _ => ENTRY.to_vec(),
        };
        h.extend(self.own());
        h.into_iter().map(String::from).collect()
    }

    fn row(&mut self, r: &[String]) -> Result<Vec<String>, String> {
        let mut v = self.rows(r)?;
        v.pop().ok_or_else(|| "rows holding nothing".into())
    }

    fn rows(&mut self, r: &[String]) -> Result<Vec<Vec<String>>, String> {
        // CLIF's times are UTC: one written without its offset is read as UTC, not in the
        // catalog's zone
        let utc: Vec<String>;
        let r = if self.at.iter().any(|(c, i)| c.ends_with("_dttm") && r.get(*i).is_some_and(|v| without_offset(v))) {
            utc = r
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let dttm = self.at.iter().any(|(c, j)| *j == i && c.ends_with("_dttm"));
                    if dttm && without_offset(v) {
                        format!("{}+00:00", v.trim())
                    } else {
                        v.clone()
                    }
                })
                .collect();
            &utc[..]
        } else {
            r
        };
        let g = |c: &str| self.get(r, c);
        let enc = g("hospitalization_id");
        let pid = g("patient_id");
        let need = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("rows without {what}")) };
        Ok(match self.table {
            Table::Patient => {
                need(!pid.is_empty(), "patient_id")?;
                vec![[
                    pid,
                    sex(g("sex_category")),
                    g("birth_date").get(..10).unwrap_or(""),
                    g("race_category"),
                    g("ethnicity_category"),
                    g("language_category"),
                    g("death_dttm"),
                ]
                .iter()
                .map(|s| s.to_string())
                .collect()]
            }
            Table::Hospitalization => {
                need(!pid.is_empty() && !enc.is_empty(), "patient_id and hospitalization_id")?;
                vec![[
                    pid,
                    enc,
                    g("admission_dttm"),
                    g("discharge_dttm"),
                    g("discharge_category"),
                    g("admission_type_category"),
                ]
                .iter()
                .map(|s| s.to_string())
                .collect()]
            }
            Table::Adt => {
                need(!enc.is_empty() && !g("in_dttm").is_empty(), "hospitalization_id and in_dttm")?;
                vec![[enc, g("location_category"), g("in_dttm"), g("out_dttm"), g("hospital_id"), g("location_name")]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()]
            }
            Table::Vitals => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let cat = g("vital_category");
                let Some(value) = number(g("vital_value")) else { return Ok(vec![]) };
                let (name, unit) = VITALS
                    .iter()
                    .find(|(k, ..)| k.eq_ignore_ascii_case(cat))
                    .map(|(_, n, u)| (n.to_string(), *u))
                    .unwrap_or_else(|| (label(cat), ""));
                vec![self.entry(
                    "",
                    enc,
                    g("recorded_dttm"),
                    &name,
                    value,
                    unit,
                    "",
                    "",
                    &[cat, g("vital_name"), g("meas_site_name")],
                )]
            }
            Table::Labs => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let value = match number(g("lab_value_numeric")) {
                    Some(x) => x,
                    None => g("lab_value"),
                };
                if value.is_empty() {
                    return Ok(vec![]);
                }
                let time = [g("lab_collect_dttm"), g("lab_result_dttm"), g("lab_order_dttm")]
                    .into_iter()
                    .find(|t| !t.is_empty())
                    .unwrap_or("");
                let code = g("lab_loinc_code");
                let own = [
                    g("lab_category"),
                    g("lab_name"),
                    g("lab_specimen_category"),
                    g("lab_order_dttm"),
                    g("lab_result_dttm"),
                ];
                vec![self.entry(
                    "",
                    enc,
                    time,
                    &label(g("lab_category")),
                    value,
                    g("reference_unit"),
                    code,
                    if code.is_empty() { "" } else { "LOINC" },
                    &own,
                )]
            }
            Table::Respiratory => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let t = g("recorded_dttm");
                let own = [g("device_category"), g("mode_category"), g("tracheostomy")];
                let mut out = vec![];
                if !g("device_category").is_empty() {
                    out.push(self.entry("", enc, t, "Respiratory support", g("device_category"), "", "", "", &own));
                }
                for (col, name, unit) in RESPIRATORY {
                    if let Some(x) = number(g(col)) {
                        out.push(self.entry("", enc, t, name, x, unit, "", "", &own));
                    }
                }
                out
            }
            Table::Meds => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let own = [g("med_group"), g("med_route_category"), g("mar_action_category"), g("med_name")];
                // a dose not given (mCIDE's mar_action_group `not_administered`: stopped, paused,
                // held) is the event, without a dose: never counted or searched as one given
                let given = !(g("mar_action_group").eq_ignore_ascii_case("not_administered")
                    || (g("mar_action_group").is_empty() && g("mar_action_category").eq_ignore_ascii_case("stop")));
                let (dose, unit) =
                    if given { (number(g("med_dose")).unwrap_or(""), g("med_dose_unit")) } else { ("", "") };
                vec![self.entry("", enc, g("admin_dttm"), &label(g("med_category")), dose, unit, "", "", &own)]
            }
            Table::Assessments => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let value = [number(g("numerical_value")).unwrap_or(""), g("categorical_value"), g("text_value")]
                    .into_iter()
                    .find(|v| !v.is_empty())
                    .unwrap_or("");
                if value.is_empty() {
                    return Ok(vec![]);
                }
                vec![self.entry(
                    "",
                    enc,
                    g("recorded_dttm"),
                    &label(g("assessment_category")),
                    value,
                    "",
                    "",
                    "",
                    &[g("assessment_group")],
                )]
            }
            Table::Position => {
                need(!enc.is_empty(), "hospitalization_id")?;
                vec![self.entry(
                    "",
                    enc,
                    g("recorded_dttm"),
                    "Position",
                    &label(g("position_category")),
                    "",
                    "",
                    "",
                    &[],
                )]
            }
            Table::Micro => {
                need(!enc.is_empty() || !pid.is_empty(), "hospitalization_id or patient_id")?;
                let culture = self.culture(r);
                let code = g("lab_loinc_code");
                let own = [g("fluid_category"), g("method_category"), g("organism_group"), g("organism_id")];
                vec![self.entry(
                    pid,
                    enc,
                    &culture.time,
                    &culture.name,
                    &culture.organism,
                    "",
                    code,
                    if code.is_empty() { "" } else { "LOINC" },
                    &own,
                )]
            }
            Table::CodeStatus => {
                need(!pid.is_empty(), "patient_id")?;
                vec![self.entry(pid, "", g("start_dttm"), "Code status", g("code_status_category"), "", "", "", &[])]
            }
            Table::Procedures => {
                need(!enc.is_empty() && !g("procedure_code").is_empty(), "hospitalization_id and procedure_code")?;
                let code = g("procedure_code");
                vec![self.entry(
                    "",
                    enc,
                    g("procedure_billed_dttm"),
                    code,
                    "",
                    "",
                    code,
                    g("procedure_code_format"),
                    &[],
                )]
            }
            Table::Diagnosis => {
                need(!enc.is_empty() && !g("diagnosis_code").is_empty(), "hospitalization_id and diagnosis_code")?;
                let code = g("diagnosis_code");
                vec![self.entry(
                    "",
                    enc,
                    "",
                    code,
                    "",
                    "",
                    code,
                    g("diagnosis_code_format"),
                    &[yes(g("diagnosis_primary")), yes(g("poa_present"))],
                )]
            }
            Table::Crrt => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let t = g("recorded_dttm");
                let own = [g("crrt_mode_category"), g("dialysis_machine_name")];
                let mut out = vec![];
                if !g("crrt_mode_category").is_empty() {
                    out.push(self.entry(
                        "",
                        enc,
                        t,
                        "CRRT mode",
                        &g("crrt_mode_category").to_uppercase(),
                        "",
                        "",
                        "",
                        &own,
                    ));
                }
                for (col, name, unit) in CRRT {
                    if let Some(x) = number(g(col)) {
                        out.push(self.entry("", enc, t, name, x, unit, "", "", &own));
                    }
                }
                out
            }
            Table::Dialysis => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let t = g("recorded_dttm");
                DIALYSIS
                    .iter()
                    .filter_map(|(col, name, unit)| {
                        number(g(col)).map(|x| self.entry("", enc, t, name, x, unit, "", "", &[]))
                    })
                    .collect()
            }
            Table::Ecmo => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let t = g("recorded_dttm");
                let own = [g("device_category"), g("mcs_group"), g("ecmo_configuration_category")];
                let mut out = vec![];
                if !g("device_category").is_empty() || !g("mcs_group").is_empty() {
                    let device = first(&[g("device_category"), g("mcs_group")]);
                    out.push(self.entry("", enc, t, "Circulatory support", device, "", "", "", &own));
                }
                for (cols, name, unit) in ECMO {
                    if let Some(x) = cols.iter().find_map(|c| number(g(c))) {
                        out.push(self.entry("", enc, t, name, x, unit, "", "", &own));
                    }
                }
                // the device's work rate (RPM, Impella power ...), by its category
                if let Some(x) = number(g("control_parameter_value")).or_else(|| number(g("device_rate"))) {
                    let what =
                        first(&[g("control_parameter_category"), g("control_parameter_name"), g("device_metric_name")]);
                    let name = if what.is_empty() { "Device rate".to_string() } else { label(what) };
                    out.push(self.entry("", enc, t, &name, x, "", "", "", &own));
                }
                out
            }
            Table::Hemodynamics => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let cat = g("measure_category");
                let Some(value) = number(g("measure_value")) else { return Ok(vec![]) };
                let (name, unit) = HEMODYNAMICS
                    .iter()
                    .find(|(k, ..)| k.eq_ignore_ascii_case(cat))
                    .map(|(_, n, u)| (n.to_string(), *u))
                    .unwrap_or_else(|| (label(cat), ""));
                vec![self.entry("", enc, g("recorded_dttm"), &name, value, unit, "", "", &[cat, g("measure_name")])]
            }
            Table::IntakeOutput => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let way = match g("in_out_flag").parse::<f64>() {
                    Ok(1.0) => "Intake",
                    Ok(0.0) => "Output",
                    _ => return Ok(vec![]),
                };
                let Some(value) = number(g("amount")) else { return Ok(vec![]) };
                let fluid = g("fluid_name");
                let name = if fluid.is_empty() { way.to_string() } else { format!("{way}: {fluid}") };
                vec![self.entry("", enc, g("intake_dttm"), &name, value, "mL", "", "", &[fluid])]
            }
            Table::Input | Table::Output => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let (way, cat, value, unit, group, raw) = if self.table == Table::Input {
                    ("Intake", g("input_category"), g("input_volume"), "mL", g("input_group"), g("input_name"))
                } else {
                    let unit = first(&[g("output_unit"), "mL"]);
                    ("Output", g("output_category"), g("output_value"), unit, g("output_group"), g("output_name"))
                };
                let Some(value) = number(value) else { return Ok(vec![]) };
                let what = if cat.is_empty() { raw.to_string() } else { label(cat).to_lowercase() };
                let name = if what.is_empty() { way.to_string() } else { format!("{way}: {what}") };
                vec![self.entry("", enc, g("recorded_dttm"), &name, value, unit, "", "", &[group, raw])]
            }
            Table::MedOrders => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let med =
                    if g("med_category").is_empty() { g("med_name").to_string() } else { label(g("med_category")) };
                let own = [
                    g("med_group"),
                    g("med_route_name"),
                    first(&[g("med_order_status_category"), g("med_order_status_name")]),
                    g("med_frequency"),
                    yes(g("prn")),
                    g("ordered_dttm"),
                    g("med_name"),
                    g("order_end_dttm"),
                ];
                vec![self.entry(
                    "",
                    enc,
                    first(&[g("order_start_dttm"), g("ordered_dttm")]),
                    format!("{med} order").trim(),
                    number(g("med_dose")).unwrap_or(""),
                    g("med_dose_unit"),
                    "",
                    "",
                    &own,
                )]
            }
            Table::Transfusion => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let code = g("product_code");
                vec![self.entry(
                    "",
                    enc,
                    g("transfusion_start_dttm"),
                    first(&[g("component_name"), "Transfusion"]),
                    number(g("volume_transfused")).unwrap_or(""),
                    g("volume_units"),
                    code,
                    if code.is_empty() { "" } else { "ISBT 128" },
                    &[g("attribute_name"), g("transfusion_end_dttm")],
                )]
            }
            Table::IcuOrders => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let cat = g("order_category");
                let name = if cat.is_empty() { g("order_name").to_string() } else { label(cat) };
                vec![self.entry(
                    "",
                    enc,
                    g("order_dttm"),
                    &name,
                    g("order_status_name"),
                    "",
                    "",
                    "",
                    &[cat, g("order_name")],
                )]
            }
            Table::Therapy => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let value = g("therapy_element_value");
                if value.is_empty() {
                    return Ok(vec![]);
                }
                vec![self.entry(
                    "",
                    enc,
                    g("session_start_dttm"),
                    &label(g("therapy_element_category")),
                    value,
                    "",
                    "",
                    "",
                    &[g("therapy_element_name")],
                )]
            }
            Table::Provider => {
                // the role and when; never who (provider_id)
                need(!enc.is_empty(), "hospitalization_id")?;
                let cat = g("provider_role_category");
                let role = if cat.is_empty() { g("provider_role_name").to_string() } else { label(cat) };
                vec![self.entry(
                    "",
                    enc,
                    g("start_dttm"),
                    "Care team",
                    &role,
                    "",
                    "",
                    "",
                    &[g("provider_role_name"), g("stop_dttm")],
                )]
            }
            Table::PlaceIndex => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let Some(value) = number(g("index_value")) else { return Ok(vec![]) };
                vec![self.entry("", enc, "", g("index_name"), value, "", "", "", &[g("index_version")])]
            }
            Table::Trial => {
                need(!enc.is_empty(), "hospitalization_id")?;
                let own = [g("trial_id"), g("arm_id"), g("consent_dttm"), g("randomized_dttm"), g("withdrawal_dttm")];
                vec![self.entry(
                    "",
                    enc,
                    first(&[g("randomized_dttm"), g("consent_dttm")]),
                    "Clinical trial",
                    first(&[g("trial_name"), g("trial_id")]),
                    "",
                    "",
                    "",
                    &own,
                )]
            }
            Table::NonCulture => {
                need(!enc.is_empty() || !pid.is_empty(), "hospitalization_id or patient_id")?;
                let organism = label(g("organism_category"));
                let name = first(&[&organism, g("micro_order_name"), "Non-culture test"]);
                let result = label(g("result_category"));
                let code = g("lab_loinc_code");
                let own = [
                    g("fluid_category"),
                    g("method_category"),
                    g("organism_group"),
                    g("micro_order_name"),
                    number(g("reference_low")).unwrap_or(""),
                    number(g("reference_high")).unwrap_or(""),
                ];
                vec![self.entry(
                    pid,
                    enc,
                    first(&[g("collect_dttm"), g("result_dttm"), g("order_dttm")]),
                    name,
                    first(&[&result, g("result_name")]),
                    g("result_units"),
                    code,
                    if code.is_empty() { "" } else { "LOINC" },
                    &own,
                )]
            }
            Table::Susceptibility => {
                // on its culture: its patient, encounter and time, by organism_id
                let id = g("organism_id");
                let Some(c) = self.link.as_ref().and_then(|l| l.get(id)) else {
                    return Err(
                        "rows without their culture (a CLIF folder's microbiology_culture, by organism_id)".into()
                    );
                };
                let drug = if g("antimicrobial_category").is_empty() {
                    g("antimicrobial_name").to_string()
                } else {
                    label(g("antimicrobial_category"))
                };
                let result = label(g("susceptibility_category"));
                let own = [c.organism.as_str(), &c.name, id, g("sensitivity_name")];
                vec![self.entry(
                    &c.patient,
                    &c.encounter,
                    &c.time,
                    &format!("{}: {drug}", first(&[&c.organism, "Organism"])),
                    first(&[&result, g("susceptibility_name")]),
                    "",
                    "",
                    "",
                    &own,
                )]
            }
            Table::PatientDiagnosis => {
                let code = g("diagnosis_code");
                need((!enc.is_empty() || !pid.is_empty()) && !code.is_empty(), "patient_id and diagnosis_code")?;
                vec![self.entry(
                    pid,
                    enc,
                    g("start_dttm"),
                    code,
                    "",
                    "",
                    code,
                    g("diagnosis_code_format"),
                    &[g("source_type"), g("end_dttm")],
                )]
            }
            Table::ValidatedDiagnosis => {
                // the label and its status; never who reviewed it (reviewer_id)
                need(!enc.is_empty(), "hospitalization_id")?;
                let own = [g("validation_method"), g("diagnosis_name"), g("review_timestamp"), g("diagnosis_end_dttm")];
                vec![self.entry(
                    "",
                    enc,
                    g("diagnosis_start_dttm"),
                    &label(g("diagnosis_category")),
                    &label(g("diagnosis_status")),
                    "",
                    "",
                    "",
                    &own,
                )]
            }
            Table::NoteFacts => {
                // a note's facts: its type, when, its revision; CLIF keeps no text, and the authors'
                // IDs are not read
                need(!enc.is_empty(), "hospitalization_id")?;
                let cat = g("note_type_category");
                let own = [
                    g("note_type"),
                    g("note_author_specialty"),
                    g("note_status"),
                    g("revision_id"),
                    g("note_id"),
                    g("creation_dttm"),
                    g("service_date"),
                ];
                vec![self.entry(
                    "",
                    enc,
                    first(&[g("revision_dttm"), g("creation_dttm")]),
                    &if cat.is_empty() { "Note".to_string() } else { label(cat) },
                    "",
                    "",
                    "",
                    "",
                    &own,
                )]
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(cols: &[&str]) -> Vec<String> {
        cols.iter().map(|c| c.to_string()).collect()
    }

    fn find(key: &str) -> Clif {
        tables().into_iter().find(|t| t.key == key).unwrap()
    }

    #[test]
    fn each_table_is_recognised_by_its_columns() {
        let cases: &[(&[&str], &str)] = &[
            (&["patient_id", "race_name", "race_category", "sex_category", "birth_date"], "clif-patient"),
            (&["patient_id", "hospitalization_id", "admission_dttm", "discharge_dttm"], "clif-hospitalization"),
            (&["hospitalization_id", "in_dttm", "out_dttm", "location_category"], "clif-adt"),
            (&["hospitalization_id", "recorded_dttm", "vital_category", "vital_value"], "clif-vitals"),
            (&["hospitalization_id", "lab_category", "lab_value"], "clif-labs"),
            (&["hospitalization_id", "recorded_dttm", "device_category", "fio2_set"], "clif-respiratory-support"),
            (&["hospitalization_id", "admin_dttm", "med_category", "med_dose"], "clif-medication-admin"),
            (&["hospitalization_id", "assessment_category", "numerical_value"], "clif-patient-assessments"),
            (&["hospitalization_id", "procedure_code", "procedure_code_format"], "clif-patient-procedures"),
            // tables sharing another's columns, told apart by their own
            (&["hospitalization_id", "recorded_dttm", "device_category", "mcs_group", "flow"], "clif-ecmo-mcs"),
            (
                &["hospitalization_id", "organism_category", "collect_dttm", "result_category"],
                "clif-microbiology-nonculture",
            ),
            (&["patient_id", "hospitalization_id", "diagnosis_code", "start_dttm"], "clif-patient-diagnosis"),
            (&["organism_id", "antimicrobial_category", "susceptibility_category"], "clif-microbiology-susceptibility"),
        ];
        let all = tables();
        for (cols, key) in cases {
            let found: Vec<&str> = all.iter().filter(|t| t.detect(&h(cols))).map(|t| t.key).collect();
            assert_eq!(found, [*key], "{cols:?}");
        }
        assert!(all.iter().all(|t| !t.detect(&h(&["MRN", "Test Name"]))));
    }

    #[test]
    fn rows_become_entries() {
        let vit = h(&[
            "hospitalization_id",
            "recorded_dttm",
            "vital_name",
            "vital_category",
            "vital_value",
            "meas_site_name",
        ]);
        let mut c = find("clif-vitals").start(&vit).unwrap();
        let row = c.row(&h(&["H1", "2024-01-02 08:15:00+00:00", "Pulse", "heart_rate", "88", ""])).unwrap();
        let at = |r: &[String], col: &str| r[c.headers().iter().position(|x| x == col).unwrap()].clone();
        assert_eq!(
            (at(&row, "encounter_id"), at(&row, "name"), at(&row, "value"), at(&row, "unit"), at(&row, "category")),
            ("H1".into(), "Heart rate".into(), "88".into(), "bpm".into(), "vit".into())
        );
        assert!(
            c.rows(&h(&["H1", "2024-01-02 08:15:00+00:00", "Pulse", "heart_rate", "", ""])).unwrap().is_empty(),
            "no value, no entry"
        );
        // a ventilator's row: the device and each setting an entry
        let resp = h(&[
            "hospitalization_id",
            "recorded_dttm",
            "device_category",
            "mode_category",
            "fio2_set",
            "peep_set",
            "tidal_volume_obs",
        ]);
        let mut c = find("clif-respiratory-support").start(&resp).unwrap();
        let rows = c
            .rows(&h(&["H1", "2024-01-02 09:00:00+00:00", "IMV", "Assist Control-Volume Control", "0.5", "8", ""]))
            .unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r[4].as_str()).collect();
        assert_eq!(names, ["Respiratory support", "FiO2 (set)", "PEEP (set)"]);
        // the patient: sex, date of birth; geography never kept
        let pat = h(&["patient_id", "sex_category", "birth_date", "race_category", "zipcode_five_digit"]);
        let mut c = find("clif-patient").start(&pat).unwrap();
        let r = c.row(&h(&["P1", "Female", "1950-03-04", "White", "30322"])).unwrap();
        assert_eq!(&r[..3], ["P1", "F", "1950-03-04"]);
        assert!(!r.join("|").contains("30322"));
        assert!(c.row(&h(&["", "Female", "1950-03-04", "", ""])).is_err());
    }
}
