//! A chunk of a table's rows' patients and encounters, decided together and written as sets:
//! the identity rules of [`crate::linkage::apply_row`], with their answers, in a few statements
//! for thousands of rows rather than a dozen for each (what an import of millions of rows,
//! and a database server a network away, needs).
//!
//! The rows are first grouped: two rows are in the same group when they name the same patient
//! ID, MRN or encounter ID, or the catalog's same patient.  A group is decided in memory, row
//! after row in the table's order, against what the catalog held before the chunk (read for
//! all rows at once) and what the group's earlier rows added.  A group whose rows all follow
//! the plain rules (a new or known patient, their MRN and fields, a new or known encounter of
//! theirs, a bed stay) is written with the others, as sets.  A group with anything else — two
//! patients to merge, a row held back for review, an ID taken over, an MRN beside another —
//! goes row by row through `apply_row` itself, after the others: groups share nothing, so the
//! order between them changes nothing.  What can differ is only which number a new `GEN-…` ID
//! gets.

use crate::db::now;
use crate::identity::{self, mrn_key};
use crate::linkage::{apply_row, row_values, Force, Outcome};
use crate::sql::{Connection, Value};
use crate::users::User;
use std::collections::{HashMap, HashSet};

/// One row's patient and encounter to apply (a linkage row: `linkage::LINKAGE_COLUMNS`).
pub struct Request {
    pub source: String,
    pub row: HashMap<String, String>,
}

/// A patient: one the catalog had, or the chunk's `n`th new one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum P {
    Old(i64),
    New(usize),
}

/// An encounter, the same way.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum E {
    Old(i64),
    New(usize),
}

/// What a row of a plain group does.
struct Act {
    patient: P,
    patient_added: bool,
    patient_named: bool,
    /// (authority, MRN as given)
    mrn: Option<(String, String)>,
    fields: Vec<(&'static str, Value)>,
    recompute: bool,
    encounter: Option<(E, bool)>,
    encounter_fields: Vec<(&'static str, Value)>,
    /// (unit, bed, start, end, recording file)
    bed: Option<(String, String, Option<f64>, Option<f64>, String)>,
}

/// A new patient: its patient ID, or the (authority, MRN key) a `GEN-…` ID is given for.
enum NewPatient {
    Named(String),
    Generated(String, String),
}

/// Find with path halving.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Apply the rows' patients and encounters (batch `batch`), as `apply_row` would one after
/// another; returns each row's outcome, in order.
pub fn apply_rows(
    tx: &Connection,
    batch: i64,
    reqs: &[Request],
    zone: chrono_tz::Tz,
    user: Option<&User>,
) -> anyhow::Result<Vec<Outcome>> {
    // what is read below is decided and written by one transaction at a time (PostgreSQL)
    tx.lock_in_transaction(crate::sql::Lock::Identity)?;
    let n = reqs.len();
    let g = |i: usize, k: &str| reqs[i].row.get(k).map(|s| s.trim()).unwrap_or("").to_string();
    let mut out: Vec<Option<Outcome>> = (0..n).map(|_| None).collect();
    // what each row gives, checked first (as `apply_row` does, before anything is read)
    let mut values: Vec<Option<(Vec<Option<f64>>, String)>> = vec![None; n];
    for i in 0..n {
        match row_values(&reqs[i].row, zone) {
            Ok(v) => values[i] = Some(v),
            Err(e) => out[i] = Some(Outcome::Invalid(e)),
        }
    }
    let live: Vec<usize> = (0..n).filter(|i| out[*i].is_none()).collect();
    let pid: Vec<String> = (0..n).map(|i| g(i, "patient_id")).collect();
    let auth: Vec<String> = (0..n).map(|i| g(i, "mrn_authority")).collect();
    let mrn: Vec<String> = (0..n).map(|i| g(i, "mrn")).collect();
    let mkey: Vec<Option<(String, String)>> =
        (0..n).map(|i| (!mrn[i].is_empty()).then(|| (auth[i].clone(), mrn_key(&mrn[i])))).collect();
    let eid: Vec<String> = (0..n).map(|i| g(i, "encounter_id")).collect();
    // what the catalog has, for all rows at once
    let pids: Vec<String> = live.iter().map(|&i| pid[i].clone()).filter(|p| !p.is_empty()).collect();
    let db_pid = identity::patients_by_id(tx, &pids)?;
    let mkeys: Vec<(String, String)> = live.iter().filter_map(|&i| mkey[i].clone()).collect();
    let db_mrn = identity::patients_by_mrn(tx, &mkeys)?;
    let mut eids: Vec<&str> = live.iter().map(|&i| eid[i].as_str()).filter(|e| !e.is_empty()).collect();
    eids.sort();
    eids.dedup();
    let db_enc: HashMap<String, (i64, i64)> = tx
        .query_in("SELECT encounter_id, id, patient FROM encounters WHERE encounter_id IN ({})", &eids, |r| {
            Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
        })?
        .into_iter()
        .collect();
    let gen_keys: Vec<(String, String)> =
        live.iter().filter(|&&i| pid[i].is_empty()).filter_map(|&i| mkey[i].clone()).collect();
    let db_gen = identity::generated_ids_of(tx, &gen_keys)?;
    let gen_pids: Vec<String> = db_gen.values().cloned().collect();
    let db_gen_patient = identity::patients_by_id(tx, &gen_pids)?;
    // the groups: rows naming the same IDs or the same patient of the catalog
    let mut parent: Vec<usize> = (0..n).collect();
    let mut owner_of: HashMap<String, usize> = HashMap::new();
    for &i in &live {
        let mut keys = vec![];
        if !pid[i].is_empty() {
            keys.push(format!("p\u{1f}{}", pid[i]));
            if let Some(p) = db_pid.get(&pid[i]) {
                keys.push(format!("P\u{1f}{p}"));
            }
        }
        if let Some((a, k)) = &mkey[i] {
            keys.push(format!("m\u{1f}{a}\u{1f}{k}"));
            if let Some(p) = db_mrn.get(&(a.clone(), k.clone())) {
                keys.push(format!("P\u{1f}{p}"));
            }
            if let Some(gp) = db_gen.get(&(a.clone(), k.clone())) {
                keys.push(format!("p\u{1f}{gp}"));
            }
        }
        if !eid[i].is_empty() {
            keys.push(format!("e\u{1f}{}", eid[i]));
            if let Some((_, p)) = db_enc.get(&eid[i]) {
                keys.push(format!("P\u{1f}{p}"));
            }
        }
        for k in keys {
            match owner_of.get(&k) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    parent[a.max(b)] = a.min(b);
                }
                None => {
                    owner_of.insert(k, i);
                }
            }
        }
    }
    // the MRNs the catalog's patients have (the "another MRN" rule)
    let olds: Vec<i64> = db_pid.values().copied().collect();
    let mut mrns_of: HashMap<P, HashSet<(String, String)>> = HashMap::new();
    for (p, a, k) in
        tx.query_in("SELECT patient, authority, mrn_key FROM patient_mrns WHERE patient IN ({})", &olds, |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?
    {
        mrns_of.entry(P::Old(p)).or_default().insert((a, k));
    }
    // each row decided, in order; a group with a row beyond the plain rules goes row by row.
    // Twice: to find those groups, then to decide the others from the start (groups share
    // nothing: the others' decisions are the same both times)
    let mut slow: HashSet<usize> = HashSet::new();
    let catalog_mrns = mrns_of;
    let mut by_pid: HashMap<String, P> = HashMap::new();
    let mut by_mrn: HashMap<(String, String), P> = HashMap::new();
    let mut by_eid: HashMap<String, (E, P)> = HashMap::new();
    let mut new_patients: Vec<NewPatient> = vec![];
    let mut new_encounters: Vec<(String, P)> = vec![];
    let mut acts: Vec<Option<Act>> = (0..n).map(|_| None).collect();
    for pass in 0..2 {
        let found_slow = slow.len();
        let mut mrns_of = catalog_mrns.clone();
        by_pid.clear();
        by_mrn.clear();
        by_eid.clear();
        new_patients.clear();
        new_encounters.clear();
        acts.iter_mut().for_each(|a| *a = None);
        for &i in &live {
            let group = find(&mut parent, i);
            if slow.contains(&group) {
                continue;
            }
            let (times, birth) = values[i].clone().unwrap_or_default();
            let found_pid = (!pid[i].is_empty())
                .then(|| by_pid.get(&pid[i]).copied().or(db_pid.get(&pid[i]).map(|p| P::Old(*p))))
                .flatten();
            let found_mrn = mkey[i].as_ref().and_then(|k| by_mrn.get(k).copied().or(db_mrn.get(k).map(|p| P::Old(*p))));
            let plain = match (found_pid, found_mrn) {
                (Some(a), Some(b)) => a == b,
                // a new patient ID for a patient known by an MRN (taken over, or held back)
                (None, Some(_)) => pid[i].is_empty(),
                // a known patient with another MRN of the same authority
                (Some(a), None) => mkey[i]
                    .as_ref()
                    .is_none_or(|(au, k)| !mrns_of.get(&a).is_some_and(|s| s.iter().any(|(x, y)| x == au && y != k))),
                (None, None) => true,
            };
            let known = found_pid.or(found_mrn);
            let enc = (!eid[i].is_empty())
                .then(|| by_eid.get(&eid[i]).copied().or(db_enc.get(&eid[i]).map(|(e, p)| (E::Old(*e), P::Old(*p)))))
                .flatten();
            // an encounter of another patient (moved, or held back); a generated ID that names a
            // patient already (an MRN removed with its batch)
            let gen_taken = known.is_none()
                && pid[i].is_empty()
                && mkey[i].as_ref().and_then(|k| db_gen.get(k)).is_some_and(|gp| db_gen_patient.contains_key(gp));
            if !plain || enc.is_some_and(|(_, owner)| Some(owner) != known) || gen_taken {
                slow.insert(group);
                continue;
            }
            let (patient, patient_added, patient_named) = match known {
                Some(p) => (p, false, false),
                None => {
                    let p = P::New(new_patients.len());
                    new_patients.push(match &mkey[i] {
                        Some((a, k)) if pid[i].is_empty() => NewPatient::Generated(a.clone(), k.clone()),
                        _ => NewPatient::Named(pid[i].clone()),
                    });
                    (p, true, pid[i].is_empty())
                }
            };
            if !pid[i].is_empty() {
                by_pid.insert(pid[i].clone(), patient);
            } else if let Some(gp) = mkey[i].as_ref().and_then(|k| db_gen.get(k)) {
                // the generated ID given before: a later row may name the patient by it
                by_pid.insert(gp.clone(), patient);
            }
            if let Some(k) = &mkey[i] {
                by_mrn.insert(k.clone(), patient);
                mrns_of.entry(patient).or_default().insert(k.clone());
            }
            let fields: Vec<(&'static str, Value)> = [
                ("sex", g(i, "sex")),
                ("last_name", g(i, "last_name")),
                ("first_name", g(i, "first_name")),
                ("birth_date", birth),
            ]
            .into_iter()
            .map(|(f, v)| (f, Value::Text(v)))
            .collect();
            let gave = !mrn[i].is_empty() || fields.iter().any(|(_, v)| !matches!(v, Value::Text(t) if t.is_empty()));
            let mut act = Act {
                patient,
                patient_added,
                patient_named,
                mrn: (!mrn[i].is_empty()).then(|| (auth[i].clone(), mrn[i].clone())),
                fields,
                recompute: gave || patient_added,
                encounter: None,
                encounter_fields: vec![],
                bed: None,
            };
            if !eid[i].is_empty() {
                let (e, added) = match enc {
                    Some((e, _)) => (e, false),
                    None => {
                        let e = E::New(new_encounters.len());
                        new_encounters.push((eid[i].clone(), patient));
                        by_eid.insert(eid[i].clone(), (e, patient));
                        (e, true)
                    }
                };
                act.encounter = Some((e, added));
                let num = |x: Option<f64>| x.map_or(Value::Null, Value::Real);
                act.encounter_fields = vec![
                    ("admit_time", num(times[0])),
                    ("discharge_time", num(times[1])),
                    ("admit_unit", Value::Text(g(i, "admit_unit"))),
                    ("disposition", Value::Text(g(i, "disposition"))),
                    ("csn", Value::Text(g(i, "csn"))),
                ];
                let (unit, bed, file) = (g(i, "unit"), g(i, "bed"), g(i, "recording_file"));
                if !(unit.is_empty() && bed.is_empty() && times[2].is_none() && file.is_empty()) {
                    act.bed = Some((unit, bed, times[2], times[3], file));
                }
            }
            acts[i] = Some(act);
        }
        if pass == 1 {
            debug_assert_eq!(slow.len(), found_slow, "the second pass finds no other group");
        }
    }
    // the plain groups, written
    let fast: Vec<usize> = live.iter().copied().filter(|&i| !slow.contains(&find(&mut parent, i))).collect();
    let t = now();
    // new patients: their IDs (generated ones in order, passing over IDs the table names)
    let taken: HashSet<String> = pid.iter().filter(|p| !p.is_empty()).cloned().collect();
    let to_generate: Vec<(String, String)> = new_patients
        .iter()
        .filter_map(|np| match np {
            NewPatient::Generated(a, k) if !db_gen.contains_key(&(a.clone(), k.clone())) => {
                Some((a.clone(), k.clone()))
            }
            _ => None,
        })
        .collect();
    let generated: HashMap<(String, String), String> =
        to_generate.iter().cloned().zip(identity::new_generated_ids(tx, &to_generate, &taken)?).collect();
    let patient_ids = tx.next_ids("patients", new_patients.len())?;
    let pseudonyms = crate::pseudonyms::fresh_many(tx, new_patients.len(), false)?;
    let mut rows = vec![];
    for (k, np) in new_patients.iter().enumerate() {
        let id = match np {
            NewPatient::Named(p) => p.clone(),
            NewPatient::Generated(a, m) => {
                let key = (a.clone(), m.clone());
                db_gen.get(&key).or(generated.get(&key)).cloned().unwrap_or_default()
            }
        };
        rows.push(vec![
            Value::Integer(patient_ids[k]),
            Value::Text(id),
            Value::Real(t),
            Value::Text(pseudonyms[k].clone()),
            Value::Real(crate::pseudonyms::new_draw()?),
        ]);
    }
    tx.insert_rows("INSERT INTO patients (id, patient_id, created_at, pseudonym, date_draw)", "", &rows)?;
    let pat = |p: P| match p {
        P::Old(id) => id,
        P::New(k) => patient_ids[k],
    };
    // new encounters
    let encounter_ids = tx.next_ids("encounters", new_encounters.len())?;
    let pseudonyms = crate::pseudonyms::fresh_many(tx, new_encounters.len(), true)?;
    let rows: Vec<Vec<Value>> = new_encounters
        .iter()
        .enumerate()
        .map(|(k, (e, p))| {
            vec![
                Value::Integer(encounter_ids[k]),
                Value::Text(e.clone()),
                Value::Integer(pat(*p)),
                Value::Real(t),
                Value::Text(pseudonyms[k].clone()),
            ]
        })
        .collect();
    tx.insert_rows("INSERT INTO encounters (id, encounter_id, patient, created_at, pseudonym)", "", &rows)?;
    let enc = |e: E| match e {
        E::Old(id) => id,
        E::New(k) => encounter_ids[k],
    };
    // what each row gave: provenance, MRNs, field values (in the rows' order)
    let mut notes: Vec<Vec<Value>> = vec![];
    let mut mrns: Vec<Vec<Value>> = vec![];
    let mut given: Vec<(&str, i64, &str, Value, &str)> = vec![];
    let mut recompute_p = vec![];
    let mut recompute_e = vec![];
    let note = |notes: &mut Vec<Vec<Value>>, source: &str, kind: &str, id: i64| {
        notes.push(vec![
            Value::Integer(batch),
            Value::Text(source.into()),
            Value::Text(kind.into()),
            Value::Integer(id),
        ])
    };
    for &i in &fast {
        let Some(a) = &acts[i] else { continue };
        let source = reqs[i].source.as_str();
        let p = pat(a.patient);
        note(&mut notes, source, "patient", p);
        if let Some((au, m)) = &a.mrn {
            mrns.push(vec![
                Value::Integer(p),
                Value::Text(au.clone()),
                Value::Text(m.clone()),
                Value::Text(mrn_key(m)),
                Value::Integer(batch),
                Value::Text(source.into()),
            ]);
        }
        given.extend(a.fields.iter().map(|(f, v)| ("patient", p, *f, v.clone(), source)));
        if a.recompute {
            recompute_p.push(p);
        }
        if let Some((e, _)) = a.encounter {
            let e = enc(e);
            note(&mut notes, source, "encounter", e);
            given.extend(a.encounter_fields.iter().map(|(f, v)| ("encounter", e, *f, v.clone(), source)));
            recompute_e.push(e);
        }
    }
    tx.insert_rows("INSERT INTO provenance (import_id, source, kind, row_id)", "ON CONFLICT DO NOTHING", &notes)?;
    tx.insert_rows(
        "INSERT OR IGNORE INTO patient_mrns (patient, authority, mrn, mrn_key, import_id, source)",
        "",
        &mrns,
    )?;
    identity::give_many(tx, batch, &given)?;
    identity::recompute_patients(tx, &recompute_p)?;
    identity::recompute_encounters(tx, &recompute_e)?;
    // bed stays: those the catalog has, found; the others added once
    let stays = bed_stays(tx, batch, &fast, &acts, &reqs.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(), &enc)?;
    for &i in &fast {
        let Some(a) = &acts[i] else { continue };
        out[i] = Some(Outcome::Applied {
            patient_added: a.patient_added,
            patient_named: a.patient_named,
            encounter_added: a.encounter.is_some_and(|(_, added)| added),
            bed_stay_added: stays.contains(&i),
        });
    }
    // the other groups, row by row
    for &i in &live {
        if out[i].is_none() {
            out[i] = Some(apply_row(tx, batch, &reqs[i].source, &reqs[i].row, zone, Force::default(), user)?);
        }
    }
    Ok(out.into_iter().map(|o| o.unwrap_or_else(|| Outcome::Invalid("not applied".into()))).collect())
}

/// The plain rows' bed stays: each found among the catalog's, or added (once: a later row
/// naming the same one finds it).  Returns the rows whose bed stay was added.
fn bed_stays(
    tx: &Connection,
    batch: i64,
    fast: &[usize],
    acts: &[Option<Act>],
    sources: &[&str],
    enc: &dyn Fn(E) -> i64,
) -> anyhow::Result<HashSet<usize>> {
    type Stay = (i64, String, String, Option<u64>, Option<u64>, String);
    let rows: Vec<(usize, i64, &(String, String, Option<f64>, Option<f64>, String), bool)> = fast
        .iter()
        .filter_map(|&i| {
            let a = acts[i].as_ref()?;
            let (e, _) = a.encounter?;
            Some((i, enc(e), a.bed.as_ref()?, matches!(e, E::Old(_))))
        })
        .collect();
    // those of known encounters the catalog may have (a few hundred to a query)
    let mut found: HashMap<usize, i64> = HashMap::new();
    let olds: Vec<_> = rows.iter().filter(|r| r.3).collect();
    for chunk in olds.chunks(400) {
        let vals: Vec<Vec<Value>> = chunk
            .iter()
            .map(|(i, e, (u, b, s, en, f), _)| {
                vec![
                    Value::Integer(*i as i64),
                    Value::Integer(*e),
                    Value::Text(u.clone()),
                    Value::Text(b.clone()),
                    s.map_or(Value::Null, Value::Real),
                    en.map_or(Value::Null, Value::Real),
                    Value::Text(f.clone()),
                ]
            })
            .collect();
        let (list, args) =
            crate::sql::values_list(&vals, &["INTEGER", "INTEGER", "TEXT", "TEXT", "REAL", "REAL", "TEXT"]);
        let mut st = tx.prepare(&format!(
            "WITH s (i, e, u, b, st, en, f) AS (VALUES {list})
             SELECT s.i, (SELECT x.id FROM bed_stays x WHERE x.encounter = s.e AND x.unit = s.u AND x.bed = s.b
                AND x.start_time IS NOT DISTINCT FROM s.st AND x.end_time IS NOT DISTINCT FROM s.en
                AND x.recording_file = s.f LIMIT 1)
             FROM s"
        ))?;
        for r in st
            .query_map(crate::sql::params_from_iter(args), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))?
        {
            if let (i, Some(id)) = r? {
                found.insert(i as usize, id);
            }
        }
    }
    let mut first: HashMap<Stay, usize> = HashMap::new();
    let mut new_rows: Vec<usize> = vec![];
    let key = |e: i64, (u, b, s, en, f): &(String, String, Option<f64>, Option<f64>, String)| -> Stay {
        (e, u.clone(), b.clone(), s.map(f64::to_bits), en.map(f64::to_bits), f.clone())
    };
    let mut same: HashMap<usize, usize> = HashMap::new();
    for (i, e, stay, _) in &rows {
        if found.contains_key(i) {
            continue;
        }
        match first.get(&key(*e, stay)) {
            Some(&j) => {
                same.insert(*i, j);
            }
            None => {
                first.insert(key(*e, stay), *i);
                new_rows.push(*i);
            }
        }
    }
    let ids = tx.next_ids("bed_stays", new_rows.len())?;
    let id_of: HashMap<usize, i64> = new_rows.iter().copied().zip(ids.iter().copied()).collect();
    let by_row: HashMap<usize, (i64, &(String, String, Option<f64>, Option<f64>, String))> =
        rows.iter().map(|(i, e, s, _)| (*i, (*e, *s))).collect();
    let inserts: Vec<Vec<Value>> = new_rows
        .iter()
        .map(|i| {
            let (e, (u, b, s, en, f)) = by_row[i];
            vec![
                Value::Integer(id_of[i]),
                Value::Integer(e),
                Value::Text(u.clone()),
                Value::Text(b.clone()),
                s.map_or(Value::Null, Value::Real),
                en.map_or(Value::Null, Value::Real),
                Value::Text(f.clone()),
            ]
        })
        .collect();
    tx.insert_rows(
        "INSERT INTO bed_stays (id, encounter, unit, bed, start_time, end_time, recording_file)",
        "",
        &inserts,
    )?;
    let mut notes = vec![];
    for (i, ..) in &rows {
        let id = found.get(i).copied().or_else(|| id_of.get(i).copied()).or_else(|| same.get(i).map(|j| id_of[j]));
        if let Some(id) = id {
            notes.push(vec![
                Value::Integer(batch),
                Value::Text(sources[*i].into()),
                Value::Text("bed_stay".into()),
                Value::Integer(id),
            ]);
        }
    }
    tx.insert_rows("INSERT INTO provenance (import_id, source, kind, row_id)", "ON CONFLICT DO NOTHING", &notes)?;
    Ok(new_rows.into_iter().collect())
}
