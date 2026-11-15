//! The categories entries fall into (`entries.category`): the clinical groupings a physician
//! reads a chart by (after HL7 v2's diagnostic service sections and FHIR's resources and
//! observation categories), not the systems the data came from: one table may fill several, and
//! several tables one.  An entry's category comes from its row (a `category` column), else the
//! category chosen for its table (`--category`), else its table's file name or headers, else its
//! domain and name.  The keys are those Medorama's journey shows, in this order.

/// (key, label, words that name it: in a table's `category` column, a file name).
pub const CATEGORIES: &[(&str, &str, &[&str])] = &[
    (
        "icu",
        "ICU",
        &["icu", "intensive care", "critical care", "monitoring", "bedside monitoring", "waveforms", "bedmaster"],
    ),
    ("vit", "Vitals", &["vit", "vital", "vitals", "vital signs", "vital sign", "flowsheet"]),
    (
        "lab",
        "Labs",
        &["lab", "labs", "laboratory", "chemistry", "hematology", "coagulation", "blood gas", "blood gases"],
    ),
    ("micro", "Microbiology", &["micro", "microbiology", "culture", "cultures", "sensitivity", "sensitivities"]),
    ("path", "Pathology", &["path", "pathology", "biopsy", "biopsies", "cytology", "histology"]),
    ("ecg", "ECG", &["ecg", "ekg", "12-lead", "12 lead", "electrocardiogram", "muse"]),
    ("holter", "Holter", &["holter", "ambulatory ecg", "event monitor", "patch monitor"]),
    ("eeg", "EEG", &["eeg", "electroencephalography", "electroencephalogram", "neurophysiology", "ieeg", "seeg"]),
    ("sleep", "Sleep", &["sleep", "sleep study", "sleep studies", "polysomnography", "psg"]),
    ("echo", "Echo", &["echo", "echocardiography", "echocardiogram", "tte", "tee"]),
    (
        "cath",
        "Cath lab",
        &[
            "cath",
            "cath lab",
            "catheterization",
            "cardiac catheterization",
            "angiography",
            "angiogram",
            "coronary angiography",
            "xa",
            "ivus",
            "oct",
            "pci",
        ],
    ),
    ("img", "Imaging", &["imaging", "img", "radiology", "ct", "mri", "x-ray", "xray", "ultrasound", "dicom", "pacs"]),
    ("proc", "Procedures", &["proc", "procedure", "procedures", "surgery", "operation", "operations", "cpt"]),
    ("med", "Medications", &["med", "meds", "medication", "medications", "drug", "drugs", "mar", "pharmacy"]),
    (
        "blood",
        "Transfusions",
        &["blood", "transfusion", "transfusions", "blood product", "blood products", "blood bank"],
    ),
    (
        "support",
        "Life support",
        &["support", "life support", "ventilation", "ventilator", "respiratory support", "dialysis", "crrt", "ecmo"],
    ),
    (
        "dx",
        "Conditions",
        &["dx", "condition", "conditions", "diagnosis", "diagnoses", "problem", "problems", "problem list", "icd"],
    ),
    ("obs", "Assessments", &["obs", "observation", "observations", "assessment", "assessments", "score", "scores"]),
    (
        "note",
        "Notes",
        &["note", "notes", "clinical note", "clinical notes", "document", "documents", "report", "reports"],
    ),
    ("allergy", "Allergies", &["allergy", "allergies", "intolerance", "intolerances"]),
];

pub fn label(key: &str) -> &'static str {
    CATEGORIES.iter().find(|c| c.0 == key).map_or("Other", |c| c.1)
}

/// The category a word names (a key, a label or one of its words), ignoring case.
pub fn named(word: &str) -> Option<&'static str> {
    let w = word.trim().to_lowercase();
    if w.is_empty() {
        return None;
    }
    CATEGORIES.iter().find(|c| c.0 == w || c.1.to_lowercase() == w || c.2.contains(&w.as_str())).map(|c| c.0)
}

/// Vital-sign names a measurement may have (lower case).
const VITALS: &[&str] = &[
    "hr",
    "heart rate",
    "pulse",
    // (not `rr` alone: an ECG's RR interval, in ms, has that name too)
    "resp",
    "resp rate",
    "resp. rate",
    "respiratory rate",
    "rr (resp)",
    "rr resp",
    "rr (/min)",
    "spo2",
    "sao2",
    "temp",
    "temperature",
    "sbp",
    "dbp",
    "map",
    "systolic blood pressure",
    "diastolic blood pressure",
    "mean arterial pressure",
    "nbp",
    "nbp sys",
    "nbp dia",
    "nbp mean",
    "bp",
    "blood pressure",
    "etco2",
    "cvp",
    "weight",
    "height",
];

/// An item's category: the one its row names, else its table's, else from its domain and name.
pub fn resolve(explicit: &str, domain: &str, name: &str, table_default: &str) -> String {
    if let Some(k) = named(explicit).or_else(|| named(table_default)) {
        return k.to_string();
    }
    match domain {
        "drug" => "med",
        "condition" => "dx",
        "procedure" => "proc",
        "note" => "note",
        "observation" => "obs",
        _ if VITALS.contains(&name.trim().to_lowercase().as_str()) => "vit",
        _ => "lab",
    }
    .to_string()
}

/// The domain (OMOP-like) an item of a category has when its row names none.
pub fn domain_of(category: &str) -> &'static str {
    match category {
        "lab" | "vit" => "measurement",
        "med" | "blood" => "drug",
        "dx" => "condition",
        "proc" | "support" => "procedure",
        "note" => "note",
        _ => "observation",
    }
}

/// Whether files of a category are recordings (ICU monitoring, Holter, EEG, sleep studies): a
/// WFDB record of another category (a 12-lead ECG) is a file of its entry instead.
pub fn is_recording(category: &str) -> bool {
    matches!(category, "icu" | "holter" | "eeg" | "sleep")
}

/// Whether a care unit is an intensive-care unit (`visits.icu`).
pub fn is_icu_unit(unit: &str) -> bool {
    let u = unit.to_uppercase();
    let words: Vec<&str> = u.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // a step-down, post-ICU or intermediate-care unit is not one (IMCU, "ICU step-down")
    let not_icu = |w: &&str| {
        w.starts_with("STEP")
            || w.starts_with("POST")
            || w.starts_with("INTERMEDIATE")
            || ["SDU", "IMCU", "IMC"].contains(w)
    };
    if words.iter().any(not_icu) {
        return false;
    }
    u.contains("INTENSIVE")
        || u.contains("CRITICAL CARE")
        || words.iter().any(|w| {
            // ICU, ICU7, MICU, SICU, CVICU, NICU, PICU, CCU, CTICU …
            let w = w.trim_end_matches(|c: char| c.is_ascii_digit());
            w.ends_with("ICU") || w == "CCU" || w == "CICU" || w == "ITU"
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(named("EKG"), Some("ecg"));
        assert_eq!(named("Holter"), Some("holter"));
        assert_eq!((named("polysomnography"), named("EEG")), (Some("sleep"), Some("eeg")));
        assert!(is_recording("sleep") && is_recording("eeg") && !is_recording("ecg"));
        assert_eq!(named("Clinical notes"), Some("note"));
        assert_eq!(named("cultures"), Some("micro"));
        assert_eq!(named("problem list"), Some("dx"));
        assert_eq!(named("blah"), None);
        assert_eq!(resolve("", "measurement", "Heart rate", ""), "vit");
        // a bare RR is an ECG's interval; the respiratory rate is named so
        assert_eq!(resolve("", "measurement", "RR", ""), "lab");
        assert_eq!(resolve("", "measurement", "Resp rate", ""), "vit");
        assert_eq!(resolve("", "measurement", "RR (resp)", ""), "vit");
        assert_eq!(resolve("", "measurement", "Lactate", ""), "lab");
        assert_eq!(resolve("", "measurement", "Lactate", "micro"), "micro");
        assert_eq!(resolve("imaging", "observation", "CT", "lab"), "img");
        assert_eq!(resolve("", "drug", "Heparin", ""), "med");
        assert_eq!(label("dx"), "Conditions");
        for u in ["ICU7", "MICU", "CVICU 2", "CCU", "Neuro ICU", "Intensive Care Unit"] {
            assert!(is_icu_unit(u), "{u}");
        }
        for u in [
            "Cardiology",
            "ED",
            "Medicine 4",
            "PACU",
            "ICU step-down",
            "SICU Stepdown",
            "Post-ICU ward",
            "PostICU",
            "IMCU",
            "Intermediate care",
            "Intermediate ICU",
        ] {
            assert!(!is_icu_unit(u), "{u}");
        }
    }
}
