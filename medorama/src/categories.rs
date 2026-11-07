//! The categories items fall into: the columns of a patient's journey, in this order, and the
//! groups of the report (docs/design/patient-page.md).  They are the clinical groupings a
//! physician reads a chart by (after HL7 v2's diagnostic service sections and FHIR's resources
//! and observation categories), not the databases the data came from: one database may fill
//! several, and several databases one.  An item's category comes from its row (a `category`
//! column), else the category chosen for its table, else its table's file name, else its
//! domain and name.  The same words find a category in the patient page's search.

/// (key, label, words that name it: in a table's `category` column, a file name, a search).
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

/// The categories shown as journey columns (allergies are shown with the patient instead).
pub fn journey_keys() -> impl Iterator<Item = &'static str> {
    CATEGORIES.iter().map(|c| c.0).filter(|k| *k != "allergy")
}

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
    "rr",
    "resp",
    "respiratory rate",
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

/// LOINC codes of vital signs (OMOP's measurements of them).
const VITAL_LOINC: &[&str] = &[
    "8867-4", "8310-5", "8480-6", "8462-4", "8478-0", "9279-1", "59408-5", "2708-6", "29463-7", "8302-2", "39156-5",
    "8331-1", "8332-9", "8333-7",
];

/// A connected database's entry's category: the one its view gives (a measurement of a vital
/// sign, by its LOINC code or its name, is `vit`), else the one its name says, else `lab`.
pub fn of_entry(category: &str, name: &str, vocabulary: &str, code: &str) -> String {
    let named_as = named(category).unwrap_or("lab");
    let vital = (vocabulary.eq_ignore_ascii_case("loinc") && VITAL_LOINC.contains(&code.trim()))
        || VITALS.contains(&name.trim().to_lowercase().as_str());
    if named_as == "lab" && vital {
        "vit".into()
    } else {
        named_as.into()
    }
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

/// Whether files of a category are recordings, shown in VitalStream (ICU monitoring, Holter,
/// EEG, sleep studies): a WFDB record or a folder of the other categories (a 12-lead ECG, a
/// DICOM series) is attached to its item and shown by that category's viewer.
pub fn is_recording(category: &str) -> bool {
    matches!(category, "icu" | "holter" | "eeg" | "sleep")
}

/// Whether a care unit is an intensive-care unit (its bed stays make the ICU column).
pub fn is_icu_unit(unit: &str) -> bool {
    let u = unit.to_uppercase();
    let words: Vec<&str> = u.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
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
        assert_eq!(resolve("", "measurement", "Lactate", ""), "lab");
        assert_eq!(resolve("", "measurement", "Lactate", "micro"), "micro");
        assert_eq!(resolve("imaging", "observation", "CT", "lab"), "img");
        assert_eq!(resolve("", "drug", "Heparin", ""), "med");
        assert_eq!(label("dx"), "Conditions");
        assert_eq!(of_entry("lab", "Heart rate", "LOINC", "8867-4"), "vit");
        assert_eq!(of_entry("lab", "Pulse rate", "LOINC", "8867-4"), "vit");
        assert_eq!(of_entry("lab", "Lactate", "LOINC", "2524-7"), "lab");
        assert_eq!(of_entry("dx", "Sepsis", "ICD10CM", "A41.9"), "dx");
        assert_eq!(of_entry("", "x", "", ""), "lab");
        for u in ["ICU7", "MICU", "CVICU 2", "CCU", "Neuro ICU", "Intensive Care Unit"] {
            assert!(is_icu_unit(u), "{u}");
        }
        for u in ["Cardiology", "ED", "Medicine 4", "PACU"] {
            assert!(!is_icu_unit(u), "{u}");
        }
    }
}
