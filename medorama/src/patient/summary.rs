//! One patient's summary: identity, encounters and stays, conditions, medications, allergies.

use super::*;

// ------------------------------------------------------------------ one patient

#[derive(Debug, Serialize)]
pub struct Stay {
    pub unit: String,
    pub bed: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Encounter {
    pub id: i64,
    pub encounter_id: String,
    pub admit: Option<f64>,
    pub discharge: Option<f64>,
    pub admit_unit: String,
    pub disposition: String,
    pub stays: Vec<Stay>,
    pub inpatient: bool,
    pub icu: bool,
}

#[derive(Debug, Serialize)]
pub struct Coded {
    pub code: String,
    pub name: String,
    pub detail: String,
    pub time: Option<f64>,
    pub end: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub id: i64,
    pub patient_id: String,
    /// What Privacy Mode names the patient by (pseudonyms.rs).
    pub pseudonym: String,
    pub sex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phi: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<i64>,
    pub encounters: Vec<Encounter>,
    pub problems: Vec<Coded>,
    /// Medications of the latest encounter (or of the latest month of data).
    pub medications: Vec<Coded>,
    /// None: no database loaded holds allergies ("not recorded"); empty: none for this patient.
    pub allergies: Option<Vec<String>>,
    pub first: Option<f64>,
    pub last: Option<f64>,
    /// The stays and visits the connected databases hold.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub visits: Vec<LiveVisit>,
    /// Connected databases linked to the patient that do not answer: their data are missing
    /// from the page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
    /// An invented patient of the demo (demo.rs).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub demo: bool,
}

/// A stay or a visit a connected database holds (its key never leaves).
#[derive(Debug, Serialize)]
pub struct LiveVisit {
    pub source: String,
    pub admit: Option<f64>,
    pub discharge: Option<f64>,
    pub unit: String,
    pub icu: bool,
}

pub(super) fn patient_exists(c: &Connection, id: i64) -> anyhow::Result<(String, String)> {
    // (a patient only an import still loading holds is not there yet)
    c.query_row(&format!("SELECT patient_id, sex FROM patients WHERE id = ?1 AND {}", patient_shown("id")), [id], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .optional()?
    .ok_or_else(|| anyhow::anyhow!("no patient {id}"))
}

pub fn summary(c: &Connection, user: &User, id: i64) -> anyhow::Result<Summary> {
    let (patient_id, sex) = patient_exists(c, id)?;
    let mut encounters = vec![];
    {
        let mut st = c.prepare(&format!(
            "SELECT e.id, e.encounter_id, e.admit_time, e.discharge_time, e.admit_unit, e.disposition,
                    EXISTS (SELECT 1 FROM recording_links l JOIN recordings r ON r.id = l.recording
                            WHERE l.encounter = e.id AND r.category = 'icu' AND {})
             FROM encounters e WHERE e.patient = ?1 AND {} ORDER BY e.admit_time IS NULL, e.admit_time DESC, e.id DESC",
            recording_shown("r.id"),
            encounter_shown("e.id")
        ))?;
        let rows: Vec<(i64, String, Option<f64>, Option<f64>, String, String, bool)> = st
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
            .collect::<Result<_, _>>()?;
        for (eid, encounter_id, admit, discharge, admit_unit, disposition, icu) in rows {
            let mut st = c.prepare(&format!(
                "SELECT unit, bed, start_time, end_time FROM bed_stays WHERE encounter = ?1 AND {} ORDER BY start_time, id",
                stay_shown("id")
            ))?;
            let stays: Vec<Stay> = st
                .query_map([eid], |r| Ok(Stay { unit: r.get(0)?, bed: r.get(1)?, start: r.get(2)?, end: r.get(3)? }))?
                .collect::<Result<_, _>>()?;
            let long = matches!((admit, discharge), (Some(a), Some(b)) if b - a >= 20.0 * 3600.0);
            encounters.push(Encounter {
                id: eid,
                encounter_id,
                admit,
                discharge,
                admit_unit,
                disposition,
                inpatient: long || !stays.is_empty() || icu,
                icu: icu || stays.iter().any(|s| categories::is_icu_unit(&s.unit)),
                stays,
            });
        }
    }
    // (code, name, value, text, unit, first time, last end)
    let coded = |sql: &str, p: &[&dyn crate::sql::ToSql]| -> anyhow::Result<Vec<Coded>> {
        let mut st = c.prepare(sql)?;
        let v = st
            .query_map(p, |r| {
                let (num, text, unit): (Option<f64>, String, String) = (r.get(2)?, r.get(3)?, r.get(4)?);
                let detail = match num {
                    Some(x) => format!("{} {unit}", fmt_num(x)).trim().to_string(),
                    None => text.lines().next().unwrap_or("").to_string(),
                };
                Ok(Coded { code: r.get(0)?, name: r.get(1)?, detail, time: r.get(5)?, end: r.get(6)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(v)
    };
    let shown = item_shown("");
    let mut problems = coded(
        &format!(
            "SELECT MIN(code), MIN(name), NULL, '', '', MIN(time), NULL FROM ehr_items WHERE patient = ?1 AND category = 'dx' AND {shown}
             GROUP BY lower(code), lower(name) ORDER BY MIN(time) DESC, lower(code), lower(name) LIMIT 12"
        ),
        &[&id],
    )?;
    // and those of the connected databases (each once, from when it was first recorded)
    for it in live_items(c, id)?.0.into_iter().filter(|i| i.category == "dx") {
        let same = |p: &&mut Coded| p.code.eq_ignore_ascii_case(&it.code) && p.name.eq_ignore_ascii_case(&it.name);
        match problems.iter_mut().find(|p| same(p)) {
            Some(p) => p.time = [p.time, it.time].into_iter().flatten().reduce(f64::min),
            None => {
                problems.push(Coded { code: it.code, name: it.name, detail: String::new(), time: it.time, end: None })
            }
        }
    }
    problems.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap_or(std::cmp::Ordering::Equal));
    problems.truncate(12);
    // medications of the latest encounter that has any
    let latest: Option<i64> = c
        .query_row(
            &format!(
                "SELECT i.encounter FROM ehr_items i JOIN encounters e ON e.id = i.encounter
                 WHERE i.patient = ?1 AND i.category = 'med' AND {} ORDER BY e.admit_time DESC, e.id DESC LIMIT 1",
                item_shown("i")
            ),
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let medications = match latest {
        Some(e) => coded(
            &format!(
                "SELECT MIN(code), MIN(name), MIN(value_num), MIN(value_text), MIN(unit), MIN(time), MAX(end_time)
                 FROM ehr_items WHERE encounter = ?1 AND category = 'med' AND {shown}
                 GROUP BY lower(name) ORDER BY MAX(COALESCE(end_time, time)) DESC, lower(name) LIMIT 12"
            ),
            &[&e],
        )?,
        None => coded(
            &format!(
                "SELECT MIN(code), MIN(name), MIN(value_num), MIN(value_text), MIN(unit), MIN(time), MAX(end_time)
                 FROM ehr_items WHERE patient = ?1 AND category = 'med' AND {shown}
                   AND time >= (SELECT MAX(time) FROM ehr_items WHERE patient = ?1 AND category = 'med' AND {shown}) - 30 * 86400
                 GROUP BY lower(name) ORDER BY MAX(COALESCE(end_time, time)) DESC, lower(name) LIMIT 12"
            ),
            &[&id],
        )?,
    };
    let any_allergy: bool = c.query_row(
        &format!("SELECT EXISTS (SELECT 1 FROM ehr_items WHERE category = 'allergy' AND {shown})"),
        [],
        |r| r.get(0),
    )?;
    let allergies = if any_allergy {
        let mut st = c.prepare(&format!(
            "SELECT DISTINCT name || CASE WHEN value_text != '' THEN ' (' || value_text || ')' ELSE '' END
             FROM ehr_items WHERE patient = ?1 AND category = 'allergy' AND {shown} ORDER BY 1"
        ))?;
        let v: Vec<String> = st.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Some(v)
    } else {
        None
    };
    let enc = encounter_shown("id");
    let (first, last): (Option<f64>, Option<f64>) = c.query_row(
        &format!(
            "SELECT MIN(t), MAX(t) FROM (
               SELECT time AS t FROM ehr_items WHERE patient = ?1 AND {shown}
               UNION ALL SELECT COALESCE(end_time, time) FROM ehr_items WHERE patient = ?1 AND {shown}
               UNION ALL SELECT admit_time FROM encounters WHERE patient = ?1 AND {enc}
               UNION ALL SELECT discharge_time FROM encounters WHERE patient = ?1 AND {enc}) AS x"
        ),
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (visits, unavailable) = live_visits(c, id)?;
    let ident = if user.phi {
        Some(identity(c, id)?)
    } else if user.privacy {
        Some(placeholder_identity())
    } else {
        None
    };
    if user.phi {
        audit(c, Some(user), "patient_view", &patient_id, "");
    }
    Ok(Summary {
        id,
        patient_id,
        pseudonym: pseudonym_of(c, id)?,
        sex,
        age: shown_age(c, user, id, ident.as_ref())?,
        phi: ident,
        encounters,
        problems,
        medications,
        allergies,
        first: [first, visits.iter().filter_map(|v| v.admit).reduce(f64::min)].into_iter().flatten().reduce(f64::min),
        last: [last, visits.iter().filter_map(|v| v.discharge.or(v.admit)).reduce(f64::max)]
            .into_iter()
            .flatten()
            .reduce(f64::max),
        visits,
        unavailable,
        demo: c.query_row(
            "SELECT EXISTS (SELECT 1 FROM provenance WHERE kind = 'patient' AND row_id = ?1 AND source = ?2)",
            params![id, crate::demo::NAME],
            |r| r.get(0),
        )?,
    })
}
