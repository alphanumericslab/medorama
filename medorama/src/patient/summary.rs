//! One patient's summary: identity, visits, conditions, medications, allergies.

use super::*;

// ------------------------------------------------------------------ one patient

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
    /// A patient ID a database gives (with the PHI permission on it), else empty.
    pub patient_id: String,
    /// What Privacy Mode names the patient by (pseudonyms.rs).
    pub pseudonym: String,
    pub sex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phi: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<i64>,
    /// The visits the databases hold, latest first.
    pub encounters: Vec<Visit>,
    pub problems: Vec<Coded>,
    /// Medications of the latest visit (or of the latest month of data).
    pub medications: Vec<Coded>,
    /// None: no database the user may view holds allergies ("not recorded"); empty: none for
    /// this patient.
    pub allergies: Option<Vec<String>>,
    pub first: Option<f64>,
    pub last: Option<f64>,
    /// The databases that link the patient.
    pub sources: Vec<String>,
    /// Databases linked to the patient that do not answer: their data are missing from the page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unavailable: Vec<String>,
}

/// `read_notes`: the session may read free text (detail.rs `is_free_text`).
fn coded(it: &It, read_notes: bool) -> Coded {
    let detail = match it.num {
        Some(x) => format!("{} {}", fmt_num(x), it.unit).trim().to_string(),
        None if read_notes || !is_free_text(it) => it.text.lines().next().unwrap_or("").to_string(),
        None => String::new(),
    };
    Coded { code: it.code.clone(), name: it.name.clone(), detail, time: it.time, end: it.end }
}

pub fn summary(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<Summary> {
    patient_exists(c, user, id)?;
    let d = load(cfg, c, user, id)?;
    let read_notes = user.reads_notes();
    // conditions: each once, from when it was first recorded, latest first
    let mut problems: Vec<Coded> = vec![];
    for it in d.items.iter().filter(|i| i.category == "dx") {
        let same = |p: &&mut Coded| p.code.eq_ignore_ascii_case(&it.code) && p.name.eq_ignore_ascii_case(&it.name);
        match problems.iter_mut().find(|p| same(p)) {
            Some(p) => p.time = [p.time, it.time].into_iter().flatten().reduce(f64::min),
            None => problems.push(Coded { detail: String::new(), end: None, ..coded(it, read_notes) }),
        }
    }
    problems.sort_by(|a, b| b.time.partial_cmp(&a.time).unwrap_or(std::cmp::Ordering::Equal));
    problems.truncate(12);
    // medications of the latest visit that has any, else of the latest month of data
    let meds: Vec<&It> = d.items.iter().filter(|i| i.category == "med").collect();
    let latest_visit = d.visits.iter().find(|v| meds.iter().any(|m| m.encounter == Some(v.id))).map(|v| v.id);
    let last_time = meds.iter().filter_map(|m| m.time).reduce(f64::max);
    let chosen: Vec<&&It> = meds
        .iter()
        .filter(|m| match latest_visit {
            Some(v) => m.encounter == Some(v),
            None => matches!((m.time, last_time), (Some(t), Some(l)) if t >= l - 30.0 * 86400.0),
        })
        .collect();
    let mut medications: Vec<Coded> = vec![];
    for m in chosen {
        if let Some(x) = medications.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&m.name)) {
            x.end = [x.end, m.end.or(m.time)].into_iter().flatten().reduce(f64::max);
            continue;
        }
        medications.push(coded(m, read_notes));
    }
    medications.sort_by(|a, b| b.end.or(b.time).partial_cmp(&a.end.or(a.time)).unwrap_or(std::cmp::Ordering::Equal));
    medications.truncate(12);
    let allergies = d.allergies_recorded.then(|| {
        let mut v: Vec<String> = d
            .allergies
            .iter()
            .map(|a| {
                if a.text.is_empty() || (!read_notes && is_free_text(a)) {
                    a.name.clone()
                } else {
                    format!("{} ({})", a.name, a.text)
                }
            })
            .collect();
        v.sort();
        v.dedup();
        v
    });
    let times = d
        .items
        .iter()
        .flat_map(|i| [i.time, i.end])
        .chain(d.visits.iter().flat_map(|v| [v.admit, v.discharge]))
        .chain(d.recordings.iter().flat_map(|r| [r.t_start, r.t_end]))
        .flatten();
    let (first, last) = times.fold((None, None), |(a, b): (Option<f64>, Option<f64>), t| {
        (Some(a.map_or(t, |a| a.min(t))), Some(b.map_or(t, |b| b.max(t))))
    });
    let records = crate::persons::records_of(cfg, c, id, None)?;
    let real = identity_of(&records);
    let ident = if user.phi {
        Some(identity(cfg, c, user, id)?)
    } else if user.privacy {
        Some(placeholder_identity())
    } else {
        None
    };
    let pseudonym = pseudonym_of(c, id)?;
    if user.phi {
        audit(c, Some(user), "patient_view", &pseudonym, "");
    }
    let mut sources: Vec<String> = vec![];
    for db in crate::persons::databases_of(c, id)? {
        if let Some(s) = crate::sources::get(c, db)? {
            if crate::grants::can(c, user, &format!("db:{db}"), "view")? {
                sources.push(s.name);
            }
        }
    }
    Ok(Summary {
        id,
        patient_id: ident
            .as_ref()
            .filter(|_| user.phi)
            .and_then(|i| i.patient_ids.first().map(|p| p.1.clone()))
            .unwrap_or_default(),
        pseudonym,
        sex: sex_of(&records),
        age: if user.phi {
            ident.as_ref().and_then(|i| age(&i.birth_date))
        } else if user.privacy {
            age(&real.birth_date)
        } else {
            None
        },
        phi: ident,
        encounters: d.visits,
        problems,
        medications,
        allergies,
        first,
        last,
        sources,
        unavailable: d.unavailable,
    })
}
