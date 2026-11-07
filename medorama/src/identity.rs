//! Patient identity (docs/administration.md, "Patients and encounters across databases"): which patient an identifier
//! names, the values each batch gave, and merging two patients.
//!
//! - **MRNs** are kept per batch with the authority that issued them (`mrn_authority`; empty =
//!   the default one): an MRN names a patient only within its authority, and a patient may
//!   have MRNs of several authorities.
//! - **Patient IDs** follow merges: a row naming a patient ID merged into another one goes to
//!   that one.  IDs the catalog generates (`GEN-…`) are kept per MRN and never given again.
//! - **Field values** (name, date of birth, admission times, …) are kept per batch; a field's
//!   value is the one of the newest batch that gave one, recomputed when a batch is removed.

use crate::db::{audit, now};
use crate::sql::types::Value;
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use std::collections::HashMap;

pub const PATIENT_FIELDS: &[&str] = &["sex", "last_name", "first_name", "birth_date"];
pub const ENCOUNTER_FIELDS: &[&str] = &["admit_time", "discharge_time", "admit_unit", "disposition", "csn"];

/// What MRN matching compares: trimmed, lower-cased.
pub fn mrn_key(mrn: &str) -> String {
    mrn.trim().to_lowercase()
}

/// The patient with this MRN of this authority.
pub fn patient_by_mrn(c: &Connection, authority: &str, mrn: &str) -> crate::sql::Result<Option<i64>> {
    let key = mrn_key(mrn);
    if key.is_empty() {
        return Ok(None);
    }
    c.query_row(
        "SELECT patient FROM patient_mrns WHERE authority = ?1 AND mrn_key = ?2 ORDER BY import_id DESC LIMIT 1",
        params![authority.trim(), key],
        |r| r.get(0),
    )
    .optional()
}

/// [`patient_by_mrn`] for many (authority, MRN key) at once: the same answer for each.
pub fn patients_by_mrn(
    c: &Connection,
    keys: &[(String, String)],
) -> crate::sql::Result<HashMap<(String, String), i64>> {
    let wanted: std::collections::HashSet<&(String, String)> = keys.iter().collect();
    let mut k: Vec<&str> = keys.iter().map(|(_, m)| m.as_str()).filter(|m| !m.is_empty()).collect();
    k.sort();
    k.dedup();
    let rows: Vec<(String, String, i64, Option<i64>)> =
        c.query_in("SELECT authority, mrn_key, patient, import_id FROM patient_mrns WHERE mrn_key IN ({})", &k, |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
    // the newest import's (`ORDER BY import_id DESC`: none counts as the oldest)
    let mut best: HashMap<(String, String), (Option<i64>, i64)> = HashMap::new();
    for (a, m, p, imp) in rows {
        let key = (a, m);
        if !wanted.contains(&key) {
            continue;
        }
        match best.get(&key) {
            Some((bi, _)) if *bi >= imp => {}
            _ => {
                best.insert(key, (imp, p));
            }
        }
    }
    Ok(best.into_iter().map(|(k, (_, p))| (k, p)).collect())
}

/// [`patient_by_id`] for many patient IDs at once.
pub fn patients_by_id(c: &Connection, ids: &[String]) -> crate::sql::Result<HashMap<String, i64>> {
    let mut k: Vec<&str> = ids.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect();
    k.sort();
    k.dedup();
    let mut out: HashMap<String, i64> = c
        .query_in("SELECT patient_id, id FROM patients WHERE patient_id IN ({})", &k, |r| Ok((r.get(0)?, r.get(1)?)))?
        .into_iter()
        .collect();
    let rest: Vec<&str> = k.into_iter().filter(|x| !out.contains_key(*x)).collect();
    let merged: Vec<(String, i64)> = c.query_in(
        "SELECT m.from_patient_id, p.id FROM patient_merges m JOIN patients p ON p.patient_id = m.into_patient_id
         WHERE m.from_patient_id IN ({})",
        &rest,
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    for (from, p) in merged {
        out.entry(from).or_insert(p);
    }
    Ok(out)
}

/// The patient with this patient ID, or the one it was merged into.
pub fn patient_by_id(c: &Connection, patient_id: &str) -> crate::sql::Result<Option<i64>> {
    c.query_row(
        "SELECT id FROM patients WHERE patient_id = ?1
         UNION ALL
         SELECT p.id FROM patient_merges m JOIN patients p ON p.patient_id = m.into_patient_id WHERE m.from_patient_id = ?1
         LIMIT 1",
        [patient_id],
        |r| r.get(0),
    )
    .optional()
}

/// [`generated_id`] for many (authority, MRN) at once, none given an ID before: new
/// `GEN-NNNNNN` IDs, in order, passing over those taken (by a patient, or by `taken`: IDs a
/// table being imported names).
pub fn new_generated_ids(
    c: &Connection,
    keys: &[(String, String)],
    taken: &std::collections::HashSet<String>,
) -> crate::sql::Result<Vec<String>> {
    let mut out: Vec<String> = vec![];
    while out.len() < keys.len() {
        let need = (keys.len() - out.len()) as i64;
        let last: i64 = c.query_row(
            "UPDATE id_counters SET value = value + ?1 WHERE name = 'generated_patient' RETURNING value",
            [need],
            |r| r.get(0),
        )?;
        let drawn: Vec<String> = (last - need + 1..=last).map(|n| format!("GEN-{n:06}")).collect();
        let mut used: std::collections::HashSet<String> = c
            .query_in("SELECT patient_id FROM patients WHERE patient_id IN ({})", &drawn, |r| r.get(0))?
            .into_iter()
            .collect();
        used.extend(c.query_in("SELECT patient_id FROM generated_ids WHERE patient_id IN ({})", &drawn, |r| r.get(0))?);
        out.extend(drawn.into_iter().filter(|d| !used.contains(d) && !taken.contains(d)));
    }
    let t = now();
    let rows: Vec<Vec<Value>> = keys
        .iter()
        .zip(&out)
        .map(|((a, k), id)| {
            vec![Value::Text(a.trim().to_string()), Value::Text(k.clone()), Value::Text(id.clone()), Value::Real(t)]
        })
        .collect();
    c.insert_rows("INSERT INTO generated_ids (authority, mrn_key, patient_id, created_at)", "", &rows)?;
    Ok(out)
}

/// The IDs given before to these (authority, MRN key).
pub fn generated_ids_of(
    c: &Connection,
    keys: &[(String, String)],
) -> crate::sql::Result<HashMap<(String, String), String>> {
    let wanted: std::collections::HashSet<&(String, String)> = keys.iter().collect();
    let mut k: Vec<&str> = keys.iter().map(|(_, m)| m.as_str()).collect();
    k.sort();
    k.dedup();
    Ok(c.query_in("SELECT authority, mrn_key, patient_id FROM generated_ids WHERE mrn_key IN ({})", &k, |r| {
        Ok(((r.get::<_, String>(0)?, r.get::<_, String>(1)?), r.get::<_, String>(2)?))
    })?
    .into_iter()
    .filter(|(key, _)| wanted.contains(key))
    .collect())
}

/// The pseudonymous ID of a patient known only by an MRN: the one given to this MRN before,
/// else a new `GEN-NNNNNN` (never given twice).
pub fn generated_id(c: &Connection, authority: &str, mrn: &str) -> crate::sql::Result<String> {
    let key = mrn_key(mrn);
    if let Some(id) = c
        .query_row(
            "SELECT patient_id FROM generated_ids WHERE authority = ?1 AND mrn_key = ?2",
            params![authority.trim(), key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    // the next number of the kept counter (id_counters); one an imported patient ID already has
    // (a table may name a patient GEN-…) is passed over
    let id = loop {
        let n: i64 = c.query_row(
            "UPDATE id_counters SET value = value + 1 WHERE name = 'generated_patient' RETURNING value",
            [],
            |r| r.get(0),
        )?;
        let id = format!("GEN-{n:06}");
        let taken: bool = c.query_row(
            "SELECT EXISTS (SELECT 1 FROM patients WHERE patient_id = ?1) OR EXISTS (SELECT 1 FROM generated_ids WHERE patient_id = ?1)",
            [&id],
            |r| r.get(0),
        )?;
        if !taken {
            break id;
        }
    };
    c.execute(
        "INSERT INTO generated_ids (authority, mrn_key, patient_id, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![authority.trim(), key, id, now()],
    )?;
    Ok(id)
}

/// Record that a batch gave this MRN to a patient.
pub fn give_mrn(
    c: &Connection,
    patient: i64,
    authority: &str,
    mrn: &str,
    import: i64,
    source: &str,
) -> crate::sql::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO patient_mrns (patient, authority, mrn, mrn_key, import_id, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![patient, authority.trim(), mrn.trim(), mrn_key(mrn), import, source],
    )?;
    Ok(())
}

/// Other MRNs of the same authority the patient has.
pub fn other_mrns(c: &Connection, patient: i64, authority: &str, mrn: &str) -> crate::sql::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM patient_mrns WHERE patient = ?1 AND authority = ?2 AND mrn_key != ?3)",
        params![patient, authority.trim(), mrn_key(mrn)],
        |r| r.get(0),
    )
}

/// Record a value a batch gave a field (empty values are not values).
pub fn give(
    c: &Connection,
    kind: &str,
    row: i64,
    field: &str,
    value: Value,
    import: i64,
    source: &str,
) -> crate::sql::Result<()> {
    if matches!(&value, Value::Null) || matches!(&value, Value::Text(t) if t.is_empty()) {
        return Ok(());
    }
    c.execute(
        "INSERT INTO field_values (kind, row_id, field, value, import_id, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (kind, row_id, field, import_id, source) DO UPDATE SET value = excluded.value",
        params![kind, row, field, value, import, source],
    )?;
    Ok(())
}

/// Record the values a batch gave a row's fields, in one statement ([`give`] for each).
pub fn give_all(
    c: &Connection,
    kind: &str,
    row: i64,
    fields: &[(&str, Value)],
    import: i64,
    source: &str,
) -> crate::sql::Result<()> {
    let given: Vec<&(&str, Value)> = fields
        .iter()
        .filter(|(_, v)| !matches!(v, Value::Null) && !matches!(v, Value::Text(t) if t.is_empty()))
        .collect();
    if given.is_empty() {
        return Ok(());
    }
    // the shared values first (?1 kind, ?2 row, ?3 import, ?4 source), then each field and value
    let mut values =
        vec![Value::Text(kind.into()), Value::Integer(row), Value::Integer(import), Value::Text(source.into())];
    let rows: Vec<String> = given
        .iter()
        .map(|(f, v)| {
            values.push(Value::Text(f.to_string()));
            values.push(v.clone());
            format!("(?1, ?2, ?{}, ?{}, ?3, ?4)", values.len() - 1, values.len())
        })
        .collect();
    c.execute(
        &format!(
            "INSERT INTO field_values (kind, row_id, field, value, import_id, source) VALUES {}
             ON CONFLICT (kind, row_id, field, import_id, source) DO UPDATE SET value = excluded.value",
            rows.join(", ")
        ),
        crate::sql::params_from_iter(values),
    )?;
    Ok(())
}

/// [`give_all`] for the fields of many rows at once, as if given one row after another: a field
/// a batch's source gives twice (two rows about the same patient) keeps the last value, in the
/// place of the first (as an update keeps a row's place).  Each is (kind, row, field, value,
/// source).
pub fn give_many(c: &Connection, import: i64, given: &[(&str, i64, &str, Value, &str)]) -> crate::sql::Result<()> {
    let mut at: HashMap<(&str, i64, &str, &str), usize> = HashMap::new();
    let mut rows: Vec<Vec<Value>> = vec![];
    for (kind, row, field, v, source) in given {
        if matches!(v, Value::Null) || matches!(v, Value::Text(t) if t.is_empty()) {
            continue;
        }
        match at.get(&(*kind, *row, *field, *source)) {
            Some(&i) => rows[i][3] = v.clone(),
            None => {
                at.insert((*kind, *row, *field, *source), rows.len());
                rows.push(vec![
                    Value::Text(kind.to_string()),
                    Value::Integer(*row),
                    Value::Text(field.to_string()),
                    v.clone(),
                    Value::Integer(import),
                    Value::Text(source.to_string()),
                ]);
            }
        }
    }
    c.insert_rows(
        "INSERT INTO field_values (kind, row_id, field, value, import_id, source)",
        "ON CONFLICT (kind, row_id, field, import_id, source) DO UPDATE SET value = excluded.value",
        &rows,
    )
}

/// [`current`] for many rows at once.
fn current_many(c: &Connection, kind: &str, ids: &[i64]) -> crate::sql::Result<HashMap<i64, HashMap<String, Value>>> {
    let mut out: HashMap<i64, HashMap<String, Value>> = HashMap::new();
    let rows: Vec<(i64, String, Value)> = c.query_in(
        &format!(
            "SELECT row_id, field, value FROM field_values WHERE kind = '{kind}' AND row_id IN ({{}})
             ORDER BY row_id, import_id DESC, rowid DESC"
        ),
        ids,
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    for (id, f, v) in rows {
        out.entry(id).or_default().entry(f).or_insert(v);
    }
    Ok(out)
}

/// [`recompute_patient`] for many patients at once.
pub fn recompute_patients(c: &Connection, ids: &[i64]) -> crate::sql::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let now = current_many(c, "patient", &ids)?;
    // each patient's MRN: the default authority's first, then the newest batch's
    let mut mrn: HashMap<i64, (bool, i64, String)> = HashMap::new();
    for (p, m, a, imp) in
        c.query_in("SELECT patient, mrn, authority, import_id FROM patient_mrns WHERE patient IN ({})", &ids, |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?))
        })?
    {
        let rank = (!a.is_empty(), -imp);
        match mrn.get(&p) {
            Some((x, y, _)) if (*x, *y) <= rank => {}
            _ => {
                mrn.insert(p, (rank.0, rank.1, m));
            }
        }
    }
    let empty = HashMap::new();
    let mut sexes = vec![];
    let mut known = vec![];
    let mut none = vec![];
    for &p in &ids {
        let f = |field: &str| now.get(&p).unwrap_or(&empty).get(field).cloned().map(text).unwrap_or_default();
        sexes.push(vec![Value::Integer(p), Value::Text(f("sex"))]);
        let m = mrn.get(&p).map(|x| x.2.clone()).unwrap_or_default();
        let (last, first, birth) = (f("last_name"), f("first_name"), f("birth_date"));
        if [&m, &last, &first, &birth].iter().all(|v| v.is_empty()) {
            none.push(p);
        } else {
            known.push(vec![
                Value::Integer(p),
                Value::Text(m),
                Value::Text(last),
                Value::Text(first),
                Value::Text(birth),
            ]);
        }
    }
    for chunk in sexes.chunks(500) {
        let (list, args) = crate::sql::values_list(chunk, &["INTEGER", "TEXT"]);
        c.execute(
            &format!(
                "WITH v (id, sex) AS (VALUES {list}) UPDATE patients SET sex = v.sex FROM v WHERE patients.id = v.id"
            ),
            crate::sql::params_from_iter(args),
        )?;
    }
    c.query_in("DELETE FROM patient_identifiers WHERE patient IN ({}) RETURNING patient", &none, |r| {
        r.get::<_, i64>(0)
    })?;
    c.insert_rows(
        "INSERT INTO patient_identifiers (patient, mrn, last_name, first_name, birth_date)",
        "ON CONFLICT (patient) DO UPDATE SET mrn = excluded.mrn, last_name = excluded.last_name,
           first_name = excluded.first_name, birth_date = excluded.birth_date",
        &known,
    )
}

/// [`recompute_encounter`] for many encounters at once.
pub fn recompute_encounters(c: &Connection, ids: &[i64]) -> crate::sql::Result<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let now = current_many(c, "encounter", &ids)?;
    let empty = HashMap::new();
    let mut set = vec![];
    let mut csn = vec![];
    let mut none = vec![];
    for &e in &ids {
        let f = |field: &str| now.get(&e).unwrap_or(&empty).get(field).cloned().unwrap_or(Value::Null);
        let num = |v: Value| number(v).map_or(Value::Null, Value::Real);
        set.push(vec![
            Value::Integer(e),
            num(f("admit_time")),
            num(f("discharge_time")),
            Value::Text(text(f("admit_unit"))),
            Value::Text(text(f("disposition"))),
        ]);
        match text(f("csn")) {
            s if s.is_empty() => none.push(e),
            s => csn.push(vec![Value::Integer(e), Value::Text(s)]),
        }
    }
    for chunk in set.chunks(500) {
        let (list, args) = crate::sql::values_list(chunk, &["INTEGER", "REAL", "REAL", "TEXT", "TEXT"]);
        c.execute(
            &format!(
                "WITH v (id, a, d, u, di) AS (VALUES {list})
                 UPDATE encounters SET admit_time = v.a, discharge_time = v.d, admit_unit = v.u, disposition = v.di
                 FROM v WHERE encounters.id = v.id"
            ),
            crate::sql::params_from_iter(args),
        )?;
    }
    c.query_in("DELETE FROM encounter_identifiers WHERE encounter IN ({}) RETURNING encounter", &none, |r| {
        r.get::<_, i64>(0)
    })?;
    c.insert_rows(
        "INSERT INTO encounter_identifiers (encounter, csn)",
        "ON CONFLICT (encounter) DO UPDATE SET csn = excluded.csn",
        &csn,
    )
}

/// A row's fields' values, as its batches gave them (the newest batch wins), in one query.
fn current(c: &Connection, kind: &str, row: i64) -> crate::sql::Result<HashMap<String, Value>> {
    let mut out = HashMap::new();
    let mut st = c.prepare(
        "SELECT field, value FROM field_values WHERE kind = ?1 AND row_id = ?2 ORDER BY import_id DESC, rowid DESC",
    )?;
    for r in st.query_map(params![kind, row], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Value>(1)?)))? {
        let (f, v) = r?;
        out.entry(f).or_insert(v);
    }
    Ok(out)
}

fn text(v: Value) -> String {
    match v {
        Value::Text(t) => t,
        Value::Integer(i) => i.to_string(),
        Value::Real(x) => x.to_string(),
        _ => String::new(),
    }
}

fn number(v: Value) -> Option<f64> {
    match v {
        Value::Real(x) => Some(x),
        Value::Integer(i) => Some(i as f64),
        Value::Text(t) => t.parse().ok(),
        _ => None,
    }
}

/// Set a patient's fields from the values its batches gave (the newest batch wins).
pub fn recompute_patient(c: &Connection, patient: i64) -> crate::sql::Result<()> {
    let now = current(c, "patient", patient)?;
    let f = |field: &str| -> crate::sql::Result<String> { Ok(now.get(field).cloned().map(text).unwrap_or_default()) };
    c.execute("UPDATE patients SET sex = ?1 WHERE id = ?2", params![f("sex")?, patient])?;
    let mrn: String = c
        .query_row(
            "SELECT mrn FROM patient_mrns WHERE patient = ?1 ORDER BY authority != '', import_id DESC LIMIT 1",
            [patient],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let (last, first, birth) = (f("last_name")?, f("first_name")?, f("birth_date")?);
    if [&mrn, &last, &first, &birth].iter().all(|v| v.is_empty()) {
        c.execute("DELETE FROM patient_identifiers WHERE patient = ?1", [patient])?;
    } else {
        c.execute(
            "INSERT INTO patient_identifiers (patient, mrn, last_name, first_name, birth_date) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (patient) DO UPDATE SET mrn = excluded.mrn, last_name = excluded.last_name,
               first_name = excluded.first_name, birth_date = excluded.birth_date",
            params![patient, mrn, last, first, birth],
        )?;
    }
    Ok(())
}

/// Set an encounter's fields from the values its batches gave (the newest batch wins).
pub fn recompute_encounter(c: &Connection, encounter: i64) -> crate::sql::Result<()> {
    let now = current(c, "encounter", encounter)?;
    let f = |field: &str| -> crate::sql::Result<Value> { Ok(now.get(field).cloned().unwrap_or(Value::Null)) };
    c.execute(
        "UPDATE encounters SET admit_time = ?1, discharge_time = ?2, admit_unit = ?3, disposition = ?4 WHERE id = ?5",
        params![
            number(f("admit_time")?),
            number(f("discharge_time")?),
            text(f("admit_unit")?),
            text(f("disposition")?),
            encounter
        ],
    )?;
    let csn = text(f("csn")?);
    if csn.is_empty() {
        c.execute("DELETE FROM encounter_identifiers WHERE encounter = ?1", [encounter])?;
    } else {
        c.execute(
            "INSERT INTO encounter_identifiers (encounter, csn) VALUES (?1, ?2) ON CONFLICT (encounter) DO UPDATE SET csn = excluded.csn",
            params![encounter, csn],
        )?;
    }
    Ok(())
}

fn patient_id_of(c: &Connection, patient: i64) -> crate::sql::Result<String> {
    c.query_row("SELECT patient_id FROM patients WHERE id = ?1", [patient], |r| r.get(0))
}

/// Merge patient `from` into `into`: its encounters, MRNs, values, EHR items, sources and
/// cohorts (memberships, and removals by hand) go to `into`; its patient ID is recorded, so
/// later rows with it go to `into` too.
pub fn merge(c: &Connection, from: i64, into: i64, user: Option<&User>) -> anyhow::Result<()> {
    if from == into {
        return Ok(());
    }
    c.lock_in_transaction(crate::sql::Lock::Identity)?;
    let (from_id, into_id) = (patient_id_of(c, from)?, patient_id_of(c, into)?);
    c.execute("UPDATE encounters SET patient = ?1 WHERE patient = ?2", [into, from])?;
    c.execute("UPDATE ehr_items SET patient = ?1 WHERE patient = ?2", [into, from])?;
    // the persons of connected databases linked to it
    c.execute("UPDATE source_persons SET patient = ?1 WHERE patient = ?2", [into, from])?;
    // rows keyed by the patient: moved, unless `into` has the same one already
    // (table, the patient's column, which rows, the rest of the key)
    for (table, col, kind, key) in [
        ("patient_mrns", "patient", "", &["authority", "mrn_key", "import_id", "source"][..]),
        ("provenance", "row_id", "kind = 'patient' AND ", &["kind", "import_id", "source"][..]),
        ("field_values", "row_id", "kind = 'patient' AND ", &["kind", "field", "import_id", "source"][..]),
    ] {
        let same: Vec<String> = key.iter().map(|k| format!("o.{k} = {table}.{k}")).collect();
        c.execute(
            &format!(
                "UPDATE {table} SET {col} = ?1 WHERE {kind}{col} = ?2
                   AND NOT EXISTS (SELECT 1 FROM {table} o WHERE o.{col} = ?1 AND {})",
                same.join(" AND ")
            ),
            [into, from],
        )?;
        c.execute(&format!("DELETE FROM {table} WHERE {kind}{col} = ?1"), [from])?;
    }
    c.execute("UPDATE patient_merges SET into_patient_id = ?1 WHERE into_patient_id = ?2", [&into_id, &from_id])?;
    c.execute(
        "INSERT INTO patient_merges (from_patient_id, into_patient_id, user_id, merged_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (from_patient_id) DO UPDATE SET into_patient_id = excluded.into_patient_id",
        params![from_id, into_id, user.map(|u| u.id).filter(|i| *i > 0), now()],
    )?;
    // its cohorts: a member stays one (by hand, if either was), a removal by hand stays
    c.execute(
        "INSERT OR IGNORE INTO cohort_members (cohort, patient, how, added_by, added_at)
         SELECT cohort, ?1, how, added_by, added_at FROM cohort_members WHERE patient = ?2",
        [into, from],
    )?;
    c.execute(
        "UPDATE cohort_members SET how = 'manual' WHERE patient = ?1
           AND cohort IN (SELECT cohort FROM cohort_members WHERE patient = ?2 AND how = 'manual')",
        [into, from],
    )?;
    c.execute(
        "INSERT OR IGNORE INTO cohort_removed (cohort, patient, removed_by, removed_at)
         SELECT cohort, ?1, removed_by, removed_at FROM cohort_removed WHERE patient = ?2",
        [into, from],
    )?;
    crate::pseudonyms::alias_on_merge(c, from, into)?;
    c.execute("DELETE FROM patients WHERE id = ?1", [from])?;
    recompute_patient(c, into)?;
    audit(c, user, "merge_patients", &into_id, &format!("{from_id} merged into {into_id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_kept() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE patients (id INTEGER PRIMARY KEY, patient_id TEXT);
             CREATE TABLE generated_ids (authority TEXT, mrn_key TEXT, patient_id TEXT, created_at REAL,
                                         PRIMARY KEY (authority, mrn_key));
             CREATE TABLE id_counters (name TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO id_counters (name, value) VALUES ('generated_patient', 0);
             INSERT INTO patients (patient_id) VALUES ('GEN-000001');",
        )
        .unwrap();
        // a patient a table named GEN-000001 keeps it: the counter passes over it
        assert_eq!(generated_id(&c, "", " MRN1 ").unwrap(), "GEN-000002");
        assert_eq!(generated_id(&c, "", "mrn1").unwrap(), "GEN-000002", "the same MRN keeps its ID");
        assert_eq!(
            generated_id(&c, "other", "mrn1").unwrap(),
            "GEN-000003",
            "another authority's MRN is another patient"
        );
        c.execute("DELETE FROM patients", []).unwrap();
        assert_eq!(generated_id(&c, "", "mrn2").unwrap(), "GEN-000004", "never given twice");
        c.execute("INSERT INTO patients (patient_id) VALUES ('GEN-000005')", []).unwrap();
        assert_eq!(generated_id(&c, "", "mrn3").unwrap(), "GEN-000006");
    }
}
