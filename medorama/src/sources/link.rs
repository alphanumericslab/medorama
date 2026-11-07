//! The person registry (docs/design/sources.md, "The person registry"): a connected
//! database's persons linked to the catalog's patients.
//!
//! A person is linked the first time a source names them.  Their identifiers make a linkage
//! row, applied by the rules of an imported table (linkage.rs `apply_row`): by patient ID and
//! its merges, else by MRN of the same authority, else a new patient; a person whose IDs name
//! two patients is held for Review.  What linking adds to the catalog belongs to the
//! connection's batch.

use std::collections::{BTreeSet, HashMap};

use super::Source;
use crate::db::{audit, now};
use crate::identity::{patient_by_id, patient_by_mrn};
use crate::linkage::{apply_row, Force, Outcome};
use crate::sql::{params, Connection};
use crate::users::User;

/// What linking did.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct Linked {
    pub linked: usize,
    pub held: usize,
    pub invalid: usize,
}

/// The linkage row of a person: what an imported table's row would say of them.
fn row_of(s: &Source, p: &super::Person) -> HashMap<String, String> {
    let mut row = HashMap::new();
    let mut put = |k: &str, v: &str| {
        if !v.trim().is_empty() {
            row.insert(k.to_string(), v.trim().to_string());
        }
    };
    // a person known by no identifier is known by the connection's name and their key
    let pid = if p.patient_id.is_empty() && p.mrn.is_empty() {
        format!("{}:{}", s.name, p.key)
    } else {
        p.patient_id.clone()
    };
    put("patient_id", &pid);
    put("mrn", &p.mrn);
    put("mrn_authority", &p.mrn_authority);
    put("last_name", &p.last_name);
    put("first_name", &p.first_name);
    put("birth_date", &p.birth_date);
    put("sex", &p.sex);
    row
}

/// The catalog's patients of these persons of `s`: those linked before, and the others linked
/// now.  Persons held for review, or whose identifiers are not valid, are left out.
pub fn patients(
    c: &Connection,
    s: &Source,
    keys: &[String],
    zone: chrono_tz::Tz,
    user: Option<&User>,
) -> anyhow::Result<HashMap<String, i64>> {
    let keys: Vec<String> = keys.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();
    let mut out: HashMap<String, i64> = c
        .query_in(
            &format!(
                "SELECT person_key, patient FROM source_persons WHERE connection = {} AND person_key IN ({{}})",
                s.id
            ),
            &keys,
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?
        .into_iter()
        .collect();
    let missing: Vec<String> = keys.into_iter().filter(|k| !out.contains_key(k)).collect();
    if missing.is_empty() {
        return Ok(out);
    }
    let persons = s.persons(&missing)?;
    let own = c.is_autocommit();
    let tx = if own { Some(c.unchecked_transaction()?) } else { None };
    // one transaction at a time decides patients (PostgreSQL: several servers)
    c.lock_in_transaction(crate::sql::Lock::Identity)?;
    let mut rep = Linked::default();
    for p in &persons {
        let row = row_of(s, p);
        let data = serde_json::to_string(&row.iter().collect::<std::collections::BTreeMap<_, _>>())?;
        // held before (open, or dismissed by a reviewer): not again
        let held: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM held_rows WHERE import_id = ?1 AND data = ?2 AND state != 'applied')",
            params![s.import_id, data],
            |r| r.get(0),
        )?;
        if held {
            rep.held += 1;
            continue;
        }
        match apply_row(c, s.import_id, &s.name, &row, zone, Force::default(), user)? {
            Outcome::Applied { .. } => {
                let g = |k: &str| row.get(k).map(String::as_str).unwrap_or("");
                let patient = match patient_by_id(c, g("patient_id"))? {
                    Some(p) => Some(p),
                    None => patient_by_mrn(c, g("mrn_authority"), g("mrn"))?,
                };
                if let Some(patient) = patient {
                    c.execute(
                        "INSERT OR IGNORE INTO source_persons (connection, person_key, patient, linked_at) VALUES (?1, ?2, ?3, ?4)",
                        params![s.id, p.key, patient, now()],
                    )?;
                    out.insert(p.key.clone(), patient);
                    rep.linked += 1;
                }
            }
            Outcome::Held(problem, detail) => {
                c.execute(
                    "INSERT INTO held_rows (import_id, source, line, data, problem, detail, created_at) VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6)",
                    params![s.import_id, s.name, data, problem, detail, now()],
                )?;
                rep.held += 1;
            }
            Outcome::Invalid(_) => rep.invalid += 1,
        }
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    if rep.linked + rep.held + rep.invalid > 0 {
        audit(
            c,
            user,
            "link",
            &s.name,
            &format!("{} linked, {} held for review, {} without valid identifiers", rep.linked, rep.held, rep.invalid),
        );
    }
    Ok(out)
}

/// Link every person of `s` (`medorama connections sync`).
pub fn sync(c: &Connection, s: &Source, zone: chrono_tz::Tz, user: &User) -> anyhow::Result<(usize, usize)> {
    let keys: Vec<String> = s.all_persons()?.into_iter().map(|p| p.key).collect();
    let n = keys.len();
    let mut linked = 0;
    for chunk in keys.chunks(2000) {
        linked += patients(c, s, chunk, zone, Some(user))?.len();
    }
    Ok((n, linked))
}

/// The keys of a patient's persons in each source linked to them.
pub fn persons_of(c: &Connection, patient: i64) -> anyhow::Result<Vec<(i64, Vec<String>)>> {
    let mut st = c.prepare(
        "SELECT connection, person_key FROM source_persons WHERE patient = ?1 ORDER BY connection, person_key",
    )?;
    let mut out: Vec<(i64, Vec<String>)> = vec![];
    for r in st.query_map([patient], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (conn, key) = r?;
        match out.last_mut() {
            Some((c0, keys)) if *c0 == conn => keys.push(key),
            _ => out.push((conn, vec![key])),
        }
    }
    Ok(out)
}
