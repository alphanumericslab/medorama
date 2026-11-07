//! Who a patient is: identifiers (PHI), placeholders for Privacy Mode, pseudonyms, ages.

use super::*;

/// The journey's time cells: six hours.
pub const BIN_S: f64 = 6.0 * 3600.0;

#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    pub last_name: String,
    pub first_name: String,
    pub birth_date: String,
    /// (authority, MRN); '' = the default authority.
    pub mrns: Vec<(String, String)>,
}

pub(super) fn identity(c: &Connection, patient: i64) -> crate::sql::Result<Identity> {
    let (last_name, first_name, birth_date): (String, String, String) = c
        .query_row(
            "SELECT last_name, first_name, birth_date FROM patient_identifiers WHERE patient = ?1",
            [patient],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .unwrap_or_default();
    let mut st = c.prepare(
        "SELECT authority, mrn FROM patient_mrns WHERE patient = ?1 GROUP BY authority, mrn
         ORDER BY authority != '', authority, mrn",
    )?;
    let mrns = st.query_map([patient], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(Identity { last_name, first_name, birth_date, mrns })
}

/// The identities of many patients at once (as `identity` for each).
pub(super) fn identities(c: &Connection, ids: &[i64]) -> crate::sql::Result<HashMap<i64, Identity>> {
    let mut out: HashMap<i64, Identity> = ids
        .iter()
        .map(|&p| {
            (
                p,
                Identity {
                    last_name: String::new(),
                    first_name: String::new(),
                    birth_date: String::new(),
                    mrns: vec![],
                },
            )
        })
        .collect();
    for (p, last, first, birth) in c.query_in(
        "SELECT patient, last_name, first_name, birth_date FROM patient_identifiers WHERE patient IN ({})",
        ids,
        |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )? {
        if let Some(i) = out.get_mut(&p) {
            (i.last_name, i.first_name, i.birth_date) = (last, first, birth);
        }
    }
    for (p, a, m) in c.query_in(
        "SELECT patient, authority, mrn FROM patient_mrns WHERE patient IN ({}) GROUP BY patient, authority, mrn
         ORDER BY patient, authority != '', authority, mrn",
        ids,
        |r| Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?)),
    )? {
        if let Some(i) = out.get_mut(&p) {
            i.mrns.push((a, m));
        }
    }
    Ok(out)
}

/// A patient's identity as Privacy Mode shows it: placeholders where it would be.
pub fn placeholder_identity() -> Identity {
    Identity {
        last_name: crate::gate::PATIENT_NAME.into(),
        first_name: String::new(),
        birth_date: crate::gate::BIRTH_DATE.into(),
        mrns: vec![(String::new(), crate::gate::MRN.into())],
    }
}

pub(super) fn pseudonym_of(c: &Connection, id: i64) -> anyhow::Result<String> {
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
