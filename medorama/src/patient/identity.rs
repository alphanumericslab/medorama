//! Who a patient is: identifiers (PHI), placeholders for Privacy Mode, pseudonyms, ages.
//!
//! A patient's identifiers are what the databases that link them say, as kept in each one's
//! persons file (persons.rs).  A user sees those of the databases granted to them with `phi`,
//! with the PHI permission and outside Privacy Mode.

use super::*;

/// The journey's time cells: six hours.
pub const BIN_S: f64 = 6.0 * 3600.0;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Identity {
    pub last_name: String,
    pub first_name: String,
    pub birth_date: String,
    /// (authority, MRN); '' = the database's.
    pub mrns: Vec<(String, String)>,
    /// (issuer, patient ID).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub patient_ids: Vec<(String, String)>,
}

/// A patient's identity from what the databases say: the first name and birth date given, every
/// MRN and patient ID.
pub(super) fn identity_of(records: &[crate::persons::Known]) -> Identity {
    let mut i = Identity::default();
    for k in records {
        if i.last_name.is_empty() && !k.last_name.is_empty() {
            (i.last_name, i.first_name) = (k.last_name.clone(), k.first_name.clone());
        }
        if i.birth_date.is_empty() {
            i.birth_date = k.birth_date.clone();
        }
        if !k.mrn.is_empty() && !i.mrns.iter().any(|m| m.0 == k.mrn_authority && m.1 == k.mrn) {
            i.mrns.push((k.mrn_authority.clone(), k.mrn.clone()));
        }
        if !k.patient_id.is_empty() && !i.patient_ids.iter().any(|m| m.0 == k.patient_id_issuer && m.1 == k.patient_id)
        {
            i.patient_ids.push((k.patient_id_issuer.clone(), k.patient_id.clone()));
        }
    }
    i
}

/// A patient's identity from the databases `user` may see the identifiers of.
pub(super) fn identity(cfg: &Config, c: &Connection, user: &User, patient: i64) -> anyhow::Result<Identity> {
    let phi = crate::databases::granted(c, user, "phi")?;
    Ok(identity_of(&crate::persons::records_of(cfg, c, patient, Some(&phi))?))
}

/// The sex the databases give (the first given).
pub(super) fn sex_of(records: &[crate::persons::Known]) -> String {
    records.iter().map(|k| k.sex.clone()).find(|s| !s.is_empty()).unwrap_or_default()
}

/// A patient's identity as Privacy Mode shows it: placeholders where it would be.
pub fn placeholder_identity() -> Identity {
    Identity {
        last_name: crate::gate::PATIENT_NAME.into(),
        first_name: String::new(),
        birth_date: crate::gate::BIRTH_DATE.into(),
        mrns: vec![(String::new(), crate::gate::MRN.into())],
        patient_ids: vec![],
    }
}

pub fn pseudonym_of(c: &Connection, id: i64) -> anyhow::Result<String> {
    Ok(c.query_row("SELECT COALESCE(pseudonym, '') FROM patients WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .unwrap_or_default())
}

/// Whole years from a `YYYY-MM-DD` date to now.
pub fn age(birth: &str) -> Option<i64> {
    let b = chrono::NaiveDate::parse_from_str(birth, "%Y-%m-%d").ok()?;
    let today = chrono::Utc::now().date_naive();
    let mut y = today.year() as i64 - b.year() as i64;
    if (today.month(), today.day()) < (b.month(), b.day()) {
        y -= 1;
    }
    Some(y)
}
use chrono::Datelike;
