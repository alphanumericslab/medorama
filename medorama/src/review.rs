//! The Review tab (docs/administration.md, "Patients and encounters across databases"): what needs a person's decision.
//!
//! - **Held-back rows**: linkage rows in conflict with the catalog; apply (with the fix the
//!   problem calls for: merge, add the MRN, move the encounter) or dismiss.
//! - **Possible duplicate patients** (PHI users): same name and date of birth, names swapped,
//!   same last name and first initial, or day and month of birth swapped; merge, or mark as
//!   different people (never suggested again).
//! - **Recordings whose files disagree** with their linked patient (PHI users): keep the link
//!   (confirmed, not shown again) or unlink (never linked automatically again).
//! - **Waiting EHR items**: grouped by the keys they wait with.
//!
//! Every decision is kept and audited, and outlives re-imports.  A merge cannot be undone
//! from here (removing and re-importing the batches does not split it either: the merged ID
//! keeps pointing to the patient it was merged into).

use crate::db::{audit, now};
use crate::identify::InFile;
use crate::linkage::{apply_row, Force, Outcome};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// The linkage columns without identifiers, shown to users without the PHI permission.
const PSEUDONYMOUS: &[&str] = &[
    "patient_id",
    "encounter_id",
    "sex",
    "admit_time",
    "discharge_time",
    "admit_unit",
    "disposition",
    "unit",
    "bed",
    "bed_start",
    "bed_end",
    "recording_file",
    "source",
];

#[derive(Debug, Serialize)]
pub struct Held {
    pub id: i64,
    pub import_id: i64,
    pub batch: String,
    pub source: String,
    pub line: i64,
    pub problem: String,
    pub detail: String,
    /// What applying does.
    pub apply: String,
    /// The row (identifier columns only with the PHI permission).
    pub row: serde_json::Map<String, serde_json::Value>,
    pub created_at: f64,
}

fn force_for(problem: &str) -> (Force, &'static str) {
    match problem {
        "two_patients" => (Force { merge: true, ..Default::default() }, "Merge the two patients, then apply the row"),
        "mrn_of_other_patient" => (
            Force { merge: true, ..Default::default() },
            "Take the two patient IDs to be one person, then apply the row",
        ),
        "other_mrn" => {
            (Force { other_mrn: true, ..Default::default() }, "Add the MRN to the patient, then apply the row")
        }
        "encounter_of_other_patient" => (
            Force { move_encounter: true, ..Default::default() },
            "Move the encounter to the row's patient, then apply the row",
        ),
        _ => (Force::default(), "Apply the row"),
    }
}

/// The rows held back and not decided yet.
pub fn held(c: &Connection, phi: bool) -> anyhow::Result<Vec<Held>> {
    let mut st = c.prepare(
        "SELECT h.id, h.import_id, i.name, h.source, h.line, h.problem, h.detail, h.data, h.created_at
         FROM held_rows h JOIN imports i ON i.id = h.import_id WHERE h.state = 'open' ORDER BY h.id",
    )?;
    let v = st
        .query_map([], |r| {
            let problem: String = r.get(5)?;
            let data: String = r.get(7)?;
            let mut row: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&data).unwrap_or_default();
            row.retain(|k, v| {
                v.as_str().is_some_and(|s| !s.trim().is_empty()) && (phi || PSEUDONYMOUS.contains(&k.as_str()))
            });
            Ok(Held {
                id: r.get(0)?,
                import_id: r.get(1)?,
                batch: r.get(2)?,
                source: r.get(3)?,
                line: r.get(4)?,
                apply: force_for(&problem).1.to_string(),
                problem,
                detail: r.get(6)?,
                row,
                created_at: r.get(8)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Apply or dismiss a held-back row (admins only).
pub fn decide_held(c: &mut Connection, user: &User, id: i64, apply: bool, timezone: &str) -> anyhow::Result<()> {
    if !user.is_admin() {
        anyhow::bail!("only admins decide held-back rows");
    }
    let row: Option<(i64, String, String, String, String)> = c
        .query_row(
            "SELECT h.import_id, h.source, h.data, h.problem, i.state FROM held_rows h JOIN imports i ON i.id = h.import_id
             WHERE h.id = ?1 AND h.state = 'open'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let Some((import, source, data, problem, state)) = row else { anyhow::bail!("no open held-back row {id}") };
    if state != "active" {
        anyhow::bail!("its import was removed");
    }
    let tx = c.transaction()?;
    if apply {
        let fields: HashMap<String, String> = serde_json::from_str(&data)?;
        // slash dates read as the import read them: in the catalog's date order
        let order = crate::util::date_order_of(&crate::display::load(&tx)?.date);
        let applied = crate::util::with_date_order(order, || {
            apply_row(&tx, import, &source, &fields, crate::util::tz(timezone), force_for(&problem).0, Some(user))
        })?;
        match applied {
            Outcome::Applied { .. } => {}
            Outcome::Held(_, d) => anyhow::bail!("the row is still in conflict: {d}"),
            Outcome::Invalid(e) => anyhow::bail!("the row cannot be applied: {e}"),
        }
    }
    tx.execute(
        "UPDATE held_rows SET state = ?1, decided_by = ?2, decided_at = ?3 WHERE id = ?4",
        params![if apply { "applied" } else { "dismissed" }, (user.id > 0).then_some(user.id), now(), id],
    )?;
    audit(&tx, Some(user), if apply { "apply_held_row" } else { "dismiss_held_row" }, &id.to_string(), &problem);
    tx.commit()?;
    if apply {
        crate::linkage::relink(c, None)?;
        crate::ehr::match_pending(c)?;
    }
    Ok(())
}

/// One side of a possible duplicate (PHI).
#[derive(Debug, Clone, Serialize)]
pub struct Person {
    pub patient_id: String,
    pub last_name: String,
    pub first_name: String,
    pub birth_date: String,
    pub sex: String,
    pub mrns: Vec<String>,
    pub encounters: i64,
}

#[derive(Debug, Serialize)]
pub struct Pair {
    pub a: Person,
    pub b: Person,
    pub reason: String,
}

/// Letters and digits only, lower-cased: what name comparison sees.
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Possible duplicate patients (at most 200 pairs), strongest reasons first.
pub fn duplicates(c: &Connection) -> anyhow::Result<Vec<Pair>> {
    let mut mrns: HashMap<i64, Vec<String>> = HashMap::new();
    {
        let mut st =
            c.prepare("SELECT DISTINCT patient, authority, mrn FROM patient_mrns ORDER BY patient, authority, mrn")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let (p, a, m): (i64, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
            let v = mrns.entry(p).or_default();
            let m = if a.is_empty() { m } else { format!("{a}: {m}") };
            if !v.contains(&m) {
                v.push(m);
            }
        }
    }
    let people: Vec<Person> = {
        let mut st = c.prepare(
            "SELECT p.id, p.patient_id, i.last_name, i.first_name, i.birth_date, p.sex,
                    (SELECT COUNT(*) FROM encounters e WHERE e.patient = p.id)
             FROM patients p JOIN patient_identifiers i ON i.patient = p.id
             WHERE i.last_name != '' AND i.birth_date != ''",
        )?;
        let v = st
            .query_map([], |r| {
                let id: i64 = r.get(0)?;
                Ok(Person {
                    patient_id: r.get(1)?,
                    last_name: r.get(2)?,
                    first_name: r.get(3)?,
                    birth_date: r.get(4)?,
                    sex: r.get(5)?,
                    mrns: mrns.get(&id).cloned().unwrap_or_default(),
                    encounters: r.get(6)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        v
    };
    let distinct: HashSet<(String, String)> = {
        let mut st = c.prepare("SELECT a_patient_id, b_patient_id FROM patient_pairs WHERE decision = 'distinct'")?;

        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    // (rule, reason, key of a person, key it is compared with)
    let swap_dm = |d: &str| {
        if d.len() == 10 && d.is_ascii() {
            format!("{}{}-{}", &d[..5], &d[8..10], &d[5..7])
        } else {
            d.to_string()
        }
    };
    type KeyFn<'a> = Box<dyn Fn(&Person) -> Option<(String, String)> + 'a>;
    let rules: Vec<(&str, KeyFn)> = vec![
        (
            "same name and date of birth",
            Box::new(|p| {
                let k = format!("{}|{}|{}", norm(&p.last_name), norm(&p.first_name), p.birth_date);
                Some((k.clone(), k))
            }),
        ),
        (
            "first and last names swapped, same date of birth",
            Box::new(|p| {
                (norm(&p.last_name) != norm(&p.first_name)).then(|| {
                    (
                        format!("{}|{}|{}", norm(&p.last_name), norm(&p.first_name), p.birth_date),
                        format!("{}|{}|{}", norm(&p.first_name), norm(&p.last_name), p.birth_date),
                    )
                })
            }),
        ),
        (
            "same name, day and month of birth swapped",
            Box::new(move |p| {
                let n = format!("{}|{}", norm(&p.last_name), norm(&p.first_name));
                let s = swap_dm(&p.birth_date);
                (s != p.birth_date).then(|| (format!("{n}|{}", p.birth_date), format!("{n}|{s}")))
            }),
        ),
        (
            "same last name, first initial and date of birth",
            Box::new(|p| {
                let i: String = norm(&p.first_name).chars().take(1).collect();
                let k = format!("{}|{}|{}", norm(&p.last_name), i, p.birth_date);
                Some((k.clone(), k))
            }),
        ),
    ];
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    let mut out = vec![];
    for (reason, key) in &rules {
        let mut index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, p) in people.iter().enumerate() {
            if let Some((own, _)) = key(p) {
                index.entry(own).or_default().push(i);
            }
        }
        for (i, p) in people.iter().enumerate() {
            let Some((_, other)) = key(p) else { continue };
            for &j in index.get(&other).into_iter().flatten() {
                let pair = (i.min(j), i.max(j));
                if i == j || seen.contains(&pair) {
                    continue;
                }
                let (a, b) = (&people[pair.0], &people[pair.1]);
                let ids = if a.patient_id < b.patient_id {
                    (a.patient_id.clone(), b.patient_id.clone())
                } else {
                    (b.patient_id.clone(), a.patient_id.clone())
                };
                seen.insert(pair);
                if distinct.contains(&ids) {
                    continue;
                }
                out.push(Pair { a: a.clone(), b: b.clone(), reason: reason.to_string() });
                if out.len() >= 200 {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

fn patient(c: &Connection, patient_id: &str) -> anyhow::Result<i64> {
    c.query_row("SELECT id FROM patients WHERE patient_id = ?1", [patient_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no patient {patient_id}"))
}

/// Merge patient `from` into `into` (admins with the PHI permission).
pub fn merge(c: &mut Connection, user: &User, from: &str, into: &str) -> anyhow::Result<()> {
    if !(user.is_admin() && user.phi) {
        anyhow::bail!("merging patients needs an admin with the PHI permission");
    }
    let tx = c.transaction()?;
    let (f, i) = (patient(&tx, from)?, patient(&tx, into)?);
    if f == i {
        anyhow::bail!("that is one patient");
    }
    crate::identity::merge(&tx, f, i, Some(user))?;
    tx.commit()?;
    crate::linkage::relink(c, None)?;
    crate::ehr::match_pending(c)?;
    Ok(())
}

/// Record that two patients are different people (admins with the PHI permission).
pub fn distinct(c: &Connection, user: &User, a: &str, b: &str) -> anyhow::Result<()> {
    if !(user.is_admin() && user.phi) {
        anyhow::bail!("deciding on duplicates needs an admin with the PHI permission");
    }
    patient(c, a)?;
    patient(c, b)?;
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    c.execute(
        "INSERT INTO patient_pairs (a_patient_id, b_patient_id, decision, user_id, decided_at) VALUES (?1, ?2, 'distinct', ?3, ?4)
         ON CONFLICT (a_patient_id, b_patient_id) DO UPDATE SET user_id = excluded.user_id, decided_at = excluded.decided_at",
        params![a, b, (user.id > 0).then_some(user.id), now()],
    )?;
    audit(c, Some(user), "distinct_patients", a, b);
    Ok(())
}

/// A recording linked to an encounter whose patient its file disagrees with.
#[derive(Debug, Serialize)]
pub struct Mismatch {
    pub recording: i64,
    pub name: String,
    pub encounter: i64,
    pub encounter_id: String,
    pub patient_id: String,
    pub link: String,
    pub last_name: String,
    pub first_name: String,
    pub mrns: Vec<String>,
    pub birth_date: String,
    pub in_file: InFile,
}

/// Links whose recording's file names another patient, not confirmed by hand (PHI).
pub fn mismatches(c: &Connection) -> anyhow::Result<Vec<Mismatch>> {
    let rows: Vec<(i64, String, i64, String, String, String, i64, String, String, String, String)> = {
        let mut st = c.prepare(
            "SELECT r.id, r.name, e.id, e.encounter_id, p.patient_id, l.method, p.id,
                    COALESCE(i.mrn, ''), COALESCE(i.last_name, ''), COALESCE(i.first_name, ''), COALESCE(i.birth_date, '')
             FROM recording_links l JOIN recordings r ON r.id = l.recording JOIN encounters e ON e.id = l.encounter
             JOIN patients p ON p.id = e.patient LEFT JOIN patient_identifiers i ON i.patient = p.id
             JOIN recording_identifiers f ON f.recording = r.id AND f.state = 'found'
             WHERE l.method != 'manual'
             ORDER BY r.id",
        )?;

        st.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        })?
        .collect::<Result<_, _>>()?
    };
    let mut out = vec![];
    for (rec, name, enc, eid, pid, link, p, mrn, last, first, dob) in rows {
        let Some(mut f) = crate::identify::in_file(c, rec, Some((&mrn, &last, &first, &dob)))? else { continue };
        let mrns: Vec<String> = {
            let mut st = c.prepare("SELECT DISTINCT mrn FROM patient_mrns WHERE patient = ?1 ORDER BY mrn")?;

            st.query_map([p], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        // the file's ID may be any of the patient's MRNs
        if f.mrn_matches == Some(false)
            && mrns.iter().any(|m| {
                m.trim().trim_start_matches('0').eq_ignore_ascii_case(f.patient_id.trim().trim_start_matches('0'))
            })
        {
            f.mrn_matches = Some(true);
        }
        let several = f.others.as_array().is_some_and(|o| !o.is_empty());
        if several || [f.mrn_matches, f.name_matches, f.birth_date_matches].contains(&Some(false)) {
            out.push(Mismatch {
                recording: rec,
                name,
                encounter: enc,
                encounter_id: eid,
                patient_id: pid,
                link,
                last_name: last,
                first_name: first,
                mrns,
                birth_date: dob,
                in_file: f,
            });
        }
    }
    Ok(out)
}

/// EHR items waiting for a patient or encounter, grouped by their keys.
#[derive(Debug, Serialize)]
pub struct Waiting {
    pub encounter_id: String,
    pub patient_id: String,
    /// MRN and encounter number (PHI users only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mrn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csn: Option<String>,
    pub items: i64,
    pub sources: String,
}

pub fn waiting(c: &Connection, phi: bool) -> anyhow::Result<Vec<Waiting>> {
    let mut st = c.prepare(
        "SELECT key_encounter, key_patient, key_mrn, key_authority, key_csn, COUNT(*), group_concat(DISTINCT source)
         FROM ehr_items WHERE encounter IS NULL AND patient IS NULL
         GROUP BY key_encounter, key_patient, key_mrn, key_authority, key_csn ORDER BY COUNT(*) DESC LIMIT 200",
    )?;
    let v = st
        .query_map([], |r| {
            let (mrn, auth): (String, String) = (r.get(2)?, r.get(3)?);
            let mrn = if auth.is_empty() || mrn.is_empty() { mrn } else { format!("{auth}: {mrn}") };
            Ok(Waiting {
                encounter_id: r.get(0)?,
                patient_id: r.get(1)?,
                mrn: phi.then_some(mrn),
                csn: if phi { Some(r.get(4)?) } else { None },
                items: r.get(5)?,
                sources: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// Everything the Review tab shows (admins only; the PHI parts with the PHI permission).
#[derive(Debug, Serialize)]
pub struct Review {
    pub held: Vec<Held>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<Vec<Pair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mismatches: Option<Vec<Mismatch>>,
    pub waiting: Vec<Waiting>,
    /// Items needing a decision (held rows, duplicates, mismatches).
    pub open: usize,
}

pub fn review(c: &Connection, user: &User) -> anyhow::Result<Review> {
    if !user.is_admin() {
        anyhow::bail!("the review list is for admins");
    }
    let held = held(c, user.phi)?;
    let duplicates = if user.phi { Some(duplicates(c)?) } else { None };
    let mismatches = if user.phi { Some(mismatches(c)?) } else { None };
    let open = held.len() + duplicates.as_ref().map_or(0, Vec::len) + mismatches.as_ref().map_or(0, Vec::len);
    Ok(Review { held, duplicates, mismatches, waiting: waiting(c, user.phi)?, open })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_compare_without_case_or_punctuation() {
        assert_eq!(norm("O'Brien-Smith "), "obriensmith");
        assert_eq!(norm("José"), "josé");
    }
}
