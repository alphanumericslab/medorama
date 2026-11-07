//! EHR data from CSV files: one row per item, in a shape that follows OMOP CDM's clinical
//! tables, so that an OMOP import can later fill the same table.
//!
//! | column | meaning |
//! |---|---|
//! | `encounter_id`, `csn`, `mrn`, `patient_id` | the keys: at least one (see "Matching") |
//! | `mrn_authority` | who issued the MRN; empty = the default one (as in the linkage table) |
//! | `domain`* | `measurement` (labs, vital signs), `drug`, `observation` (scores, findings), `condition` (diagnoses), `procedure`, `note` |
//! | `name`* | e.g. `Lactate`, `Norepinephrine` |
//! | `time`, `end_time` | when (end: infusions, procedures) |
//! | `vocabulary`, `code` | e.g. `LOINC` `2524-7`, `RxNorm` `7512`, `ICD10CM` `A41.9` |
//! | `value`, `unit` | a number is stored as a number, anything else as text |
//! | `source` | the row's source label (else the import's) |
//!
//! **Matching**, by the first key a row has: `encounter_id`; the encounter number `csn`; the
//! patient (`mrn`, else `patient_id`) and the time, which picks the patient's encounter whose
//! admission covers it.  An item of a known patient outside every encounter stays with the
//! patient; an item that matches nothing waits, with its keys, and is matched when a linkage
//! table supplies the patient or encounter ([`match_pending`]).  Tables can thus come in any
//! order.  A row identical to one already stored is not stored twice: the item is recorded as
//! held by both batches (`ehr_provenance`), and stays until the last of them is removed.  Notes (free text, may
//! name people) are shown only to users with the PHI permission.

use crate::identity::{patient_by_id, patient_by_mrn};
use crate::sql::{params, Connection, OptionalExtension, Value};
use serde::Serialize;
use std::collections::HashMap;

pub const DOMAINS: &[&str] = &["measurement", "drug", "observation", "condition", "procedure", "note"];

/// The keys an EHR row came with.
#[derive(Debug, Default, Clone)]
pub struct Keys {
    pub encounter: String,
    pub csn: String,
    pub mrn: String,
    pub authority: String,
    pub patient: String,
}

impl Keys {
    fn cache_key(&self) -> String {
        format!("{}\x1f{}\x1f{}\x1f{}\x1f{}", self.encounter, self.csn, self.mrn, self.authority, self.patient)
    }
}

/// (encounter, patient) for keys and a time; None where unknown.
pub fn resolve(c: &Connection, k: &Keys, time: Option<f64>) -> crate::sql::Result<(Option<i64>, Option<i64>)> {
    if let Some((e, p)) = by_key(c, k)? {
        return Ok((Some(e), Some(p)));
    }
    resolve_by_patient(c, k, time)
}

/// `resolve` once no encounter ID or CSN found one: the patient, and their encounter at the time.
fn resolve_by_patient(c: &Connection, k: &Keys, time: Option<f64>) -> crate::sql::Result<(Option<i64>, Option<i64>)> {
    let patient = if !k.mrn.is_empty() { patient_by_mrn(c, &k.authority, &k.mrn)? } else { None };
    let patient = match patient {
        Some(p) => Some(p),
        None if !k.patient.is_empty() => patient_by_id(c, &k.patient)?,
        None => None,
    };
    let Some(p) = patient else { return Ok((None, None)) };
    let enc = match time {
        Some(t) => c
            .query_row(
                "SELECT id FROM encounters WHERE patient = ?1 AND admit_time IS NOT NULL AND admit_time <= ?2
                   AND (discharge_time IS NULL OR discharge_time >= ?2) ORDER BY admit_time DESC LIMIT 1",
                params![p, t],
                |r| r.get(0),
            )
            .optional()?,
        None => None,
    };
    Ok((enc, Some(p)))
}

/// Match items that have no encounter yet (waiting, or kept with their patient) against the
/// catalog's encounters as they are now; returns how many found an encounter or a patient.
pub fn match_pending(c: &mut Connection) -> anyhow::Result<usize> {
    let rows: Vec<(i64, Option<i64>, Keys, Option<f64>)> = {
        let mut st = c.prepare(
            "SELECT id, patient, key_encounter, key_csn, key_mrn, key_authority, key_patient, time FROM ehr_items
             WHERE encounter IS NULL",
        )?;

        st.query_map([], |r| {
            let keys =
                Keys { encounter: r.get(2)?, csn: r.get(3)?, mrn: r.get(4)?, authority: r.get(5)?, patient: r.get(6)? };
            Ok((r.get(0)?, r.get(1)?, keys, r.get(7)?))
        })?
        .collect::<Result<_, _>>()?
    };
    let tx = c.transaction()?;
    let mut n = 0;
    // a few thousand at a time: their encounters and patients found together (`resolve_many`)
    for chunk in rows.chunks(5000) {
        let mut cache = Cache::new();
        let found = resolve_many(&tx, chunk.iter().map(|(_, _, k, t)| (k, *t)).collect(), &mut cache)?;
        let moved: Vec<Vec<Value>> = chunk
            .iter()
            .zip(found)
            .filter(|((_, patient, ..), f)| (f.0.is_some() || f.1.is_some()) && (f.0.is_some() || f.1 != *patient))
            .map(|((id, ..), f)| {
                vec![
                    Value::Integer(*id),
                    f.0.map_or(Value::Null, Value::Integer),
                    f.1.map_or(Value::Null, Value::Integer),
                ]
            })
            .collect();
        n += moved.len();
        for part in moved.chunks(500) {
            let (list, args) = crate::sql::values_list(part, &["INTEGER", "INTEGER", "INTEGER"]);
            tx.execute(
                &format!(
                    "WITH v (id, e, p) AS (VALUES {list})
                     UPDATE ehr_items SET encounter = v.e, patient = v.p FROM v WHERE ehr_items.id = v.id"
                ),
                crate::sql::params_from_iter(args),
            )?;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// One item to store: keys, what it is, when, its value.
#[derive(Debug, Default, Clone)]
pub struct ItemRow {
    pub keys: Keys,
    pub domain: String,
    pub category: String,
    pub name: String,
    pub time: Option<f64>,
    pub end: Option<f64>,
    pub vocabulary: String,
    pub code: String,
    /// The value as given: a number is stored as a number, anything else as text.
    pub value: String,
    /// Free text: a note, a report, a comment (stored with the value's text).
    pub text: String,
    pub unit: String,
    pub ref_low: Option<f64>,
    pub ref_high: Option<f64>,
    pub flag: String,
    pub author: String,
    pub service: String,
    pub file_path: String,
}

/// What became of an item.
pub enum Stored {
    WithEncounter,
    PatientLevel,
    Waiting,
    Duplicate,
}

/// What `insert_item` keeps across the rows of one table: the encounters and patients keys
/// name, and the items' provenance still to write ([`Cache::flush`], before the table's
/// transaction ends).
#[derive(Default)]
pub struct Cache {
    /// (keys, the time) → (encounter, patient), as `resolve` found them.
    resolved: HashMap<(String, Option<i64>), (Option<i64>, Option<i64>)>,
    /// keys → the encounter (and its patient) the row's encounter ID or CSN names: the same at
    /// any time.  Only found ones: an encounter a later row adds is then found.
    by_key: HashMap<String, (i64, i64)>,
    /// (item, import, source) for `ehr_provenance`.
    provenance: Vec<(i64, i64, String)>,
}

impl Cache {
    pub fn new() -> Cache {
        Cache::default()
    }

    /// The provenance kept, written (many rows to a statement).
    pub fn flush(&mut self, tx: &Connection) -> crate::sql::Result<()> {
        for chunk in self.provenance.chunks(300) {
            let mut values: Vec<Value> = vec![];
            let rows: Vec<String> = chunk
                .iter()
                .map(|(item, batch, source)| {
                    values.extend([Value::Integer(*item), Value::Integer(*batch), Value::Text(source.clone())]);
                    let n = values.len();
                    format!("(?{}, ?{}, ?{})", n - 2, n - 1, n)
                })
                .collect();
            tx.execute(
                &format!("INSERT OR IGNORE INTO ehr_provenance (item, import_id, source) VALUES {}", rows.join(", ")),
                crate::sql::params_from_iter(values),
            )?;
        }
        self.provenance.clear();
        Ok(())
    }
}

/// The most lookups [`Cache`] keeps of each kind.
const CACHE_MAX: usize = 200_000;

/// The encounter an encounter ID or a CSN names (`resolve`'s first ways), and its patient.
fn by_key(c: &Connection, k: &Keys) -> crate::sql::Result<Option<(i64, i64)>> {
    if !k.encounter.is_empty() {
        if let Some(f) = c
            .query_row("SELECT id, patient FROM encounters WHERE encounter_id = ?1", [&k.encounter], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
        {
            return Ok(Some(f));
        }
    }
    if !k.csn.is_empty() {
        return c
            .query_row(
                "SELECT e.id, e.patient FROM encounter_identifiers i JOIN encounters e ON e.id = i.encounter
                 WHERE lower(trim(i.csn)) = lower(trim(?1)) LIMIT 1",
                [&k.csn],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional();
    }
    Ok(None)
}

/// Store one item as part of batch `batch` (source `source`): matched to its encounter or
/// patient, not stored twice (a duplicate is recorded as held by this batch too).
pub fn insert_item(
    tx: &Connection,
    batch: i64,
    source: &str,
    it: &ItemRow,
    cache: &mut Cache,
) -> crate::sql::Result<Stored> {
    insert_item_id(tx, batch, source, it, cache).map(|r| r.0)
}

/// An item's value as stored: a number, or text (a note's text, a result written in words, a
/// comment after it).
pub fn stored_value(it: &ItemRow) -> (Option<f64>, String) {
    let note = it.domain == "note";
    let num = if note { None } else { it.value.trim().parse::<f64>().ok().filter(|x| x.is_finite()) };
    let mut text = if num.is_some() || note { String::new() } else { it.value.trim().to_string() };
    if !it.text.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(it.text.trim());
    } else if note && text.is_empty() {
        text = it.value.trim().to_string();
    }
    (num, text)
}

/// `insert_item`, with the item's id (the one stored before, for a duplicate).
pub fn insert_item_id(
    tx: &Connection,
    batch: i64,
    source: &str,
    it: &ItemRow,
    cache: &mut Cache,
) -> crate::sql::Result<(Stored, i64)> {
    // a cache, not a copy of the table: emptied when it grows large (a table of millions of rows)
    if cache.resolved.len() > CACHE_MAX {
        cache.resolved.clear();
    }
    if cache.by_key.len() > CACHE_MAX {
        cache.by_key.clear();
    }
    let kk = it.keys.cache_key();
    let ck = (kk.clone(), it.time.map(|t| t.to_bits() as i64));
    let (enc, patient) = match (cache.by_key.get(&kk), cache.resolved.get(&ck)) {
        (Some((e, p)), _) => (Some(*e), Some(*p)),
        (None, Some(f)) => *f,
        (None, None) => {
            // `resolve`, its encounter found by ID or CSN kept for every time
            let f = match by_key(tx, &it.keys)? {
                Some((e, p)) => {
                    cache.by_key.insert(kk, (e, p));
                    (Some(e), Some(p))
                }
                None => resolve_by_patient(tx, &it.keys, it.time)?,
            };
            cache.resolved.insert(ck, f);
            f
        }
    };
    let (num, text) = stored_value(it);
    // the same item stored before: looked for among the encounter's items, else the patient's,
    // else those waiting with the same keys (each by an index, not across all items)
    let mut args: Vec<Value> = vec![
        it.name.clone().into(),
        it.time.into(),
        it.domain.clone().into(),
        it.code.clone().into(),
        num.into(),
        text.clone().into(),
        it.unit.clone().into(),
        it.end.into(),
        it.file_path.clone().into(),
    ];
    let keys = "key_encounter = ?11 AND key_csn = ?12 AND key_mrn = ?13 AND key_patient = ?14 AND key_authority = ?15";
    let whose = match (enc, patient) {
        (Some(e), _) => {
            args.push(e.into());
            "encounter = ?10".to_string()
        }
        (None, p) => {
            args.push(p.into());
            args.extend(
                [&it.keys.encounter, &it.keys.csn, &it.keys.mrn, &it.keys.patient, &it.keys.authority]
                    .map(|k| Value::from(k.clone())),
            );
            if p.is_some() {
                format!("patient = ?10 AND {keys}")
            } else {
                format!("patient IS ?10 AND {keys}")
            }
        }
    };
    let dup: Option<i64> = tx
        .query_row(
            &format!(
                "SELECT id FROM ehr_items WHERE {whose} AND name = ?1 AND time IS ?2 AND domain = ?3 AND code = ?4
                   AND value_num IS ?5 AND value_text = ?6 AND unit = ?7 AND end_time IS ?8 AND file_path = ?9 LIMIT 1"
            ),
            args,
            |r| r.get(0),
        )
        .optional()?;
    if let Some(item) = dup {
        cache.provenance.push((item, batch, source.to_string()));
        return Ok((Stored::Duplicate, item));
    }
    let id = tx.insert(
        "INSERT INTO ehr_items (encounter, patient, key_encounter, key_csn, key_mrn, key_authority, key_patient, domain,
                                time, end_time, vocabulary, code, name, value_num, value_text, unit, import_id, source,
                                category, ref_low, ref_high, flag, author, service, file_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        params![
            enc, patient, it.keys.encounter, it.keys.csn, it.keys.mrn, it.keys.authority, it.keys.patient, it.domain, it.time,
            it.end, it.vocabulary, it.code, it.name, num, text, it.unit, batch, source, it.category, it.ref_low, it.ref_high,
            it.flag, it.author, it.service, it.file_path
        ],
    )?;
    cache.provenance.push((id, batch, source.to_string()));
    Ok((
        match (enc, patient) {
            (Some(_), _) => Stored::WithEncounter,
            (None, Some(_)) => Stored::PatientLevel,
            _ => Stored::Waiting,
        },
        id,
    ))
}

/// A number from a cell; None when empty, an error when not a number.
pub fn number(s: &str) -> Result<Option<f64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<f64>().map(Some).map_err(|_| format!("not a number: {s:?}"))
}

#[derive(Debug, Serialize)]
pub struct Item {
    pub id: i64,
    pub domain: String,
    pub time: Option<f64>,
    pub end_time: Option<f64>,
    pub vocabulary: String,
    pub code: String,
    pub name: String,
    pub value: serde_json::Value,
    pub unit: String,
    pub source: String,
}

/// An encounter's items, or (`patient_level`) its patient's items outside every encounter; in
/// time order, notes only with `phi`.
pub fn items(
    c: &Connection,
    encounter: i64,
    patient_level: bool,
    domain: Option<&str>,
    phi: bool,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(i64, Vec<Item>)> {
    let dom = domain.unwrap_or("");
    // what only an import still loading holds is not shown yet (patient::loading)
    let shown = crate::patient::item_shown("");
    let wh = if patient_level {
        format!("encounter IS NULL AND patient = (SELECT patient FROM encounters WHERE id = ?1) AND (?2 = '' OR domain = ?2) AND (?3 = 1 OR domain != 'note') AND {shown}")
    } else {
        format!("encounter = ?1 AND (?2 = '' OR domain = ?2) AND (?3 = 1 OR domain != 'note') AND {shown}")
    };
    let total: i64 =
        c.query_row(&format!("SELECT COUNT(*) FROM ehr_items WHERE {wh}"), params![encounter, dom, phi as i64], |r| {
            r.get(0)
        })?;
    let mut st = c.prepare(&format!(
        "SELECT id, domain, time, end_time, vocabulary, code, name, value_num, value_text, unit,
           COALESCE((SELECT group_concat(source, ', ') FROM (SELECT DISTINCT source FROM ehr_provenance p
                                                           WHERE p.item = ehr_items.id ORDER BY source)), source)
         FROM ehr_items WHERE {wh} ORDER BY time IS NULL, time, id LIMIT ?4 OFFSET ?5"
    ))?;
    let v = st
        .query_map(params![encounter, dom, phi as i64, limit, offset], |r| {
            let num: Option<f64> = r.get(7)?;
            let text: String = r.get(8)?;
            Ok(Item {
                id: r.get(0)?,
                domain: r.get(1)?,
                time: r.get(2)?,
                end_time: r.get(3)?,
                vocabulary: r.get(4)?,
                code: r.get(5)?,
                name: r.get(6)?,
                value: num.map_or(serde_json::Value::String(text), |x| serde_json::json!(x)),
                unit: r.get(9)?,
                source: r.get(10)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok((total, v))
}

/// Item counts per domain (notes only with `phi`).
pub fn counts(c: &Connection, encounter: i64, phi: bool) -> anyhow::Result<Vec<(String, i64)>> {
    let mut st = c.prepare(&format!(
        "SELECT domain, COUNT(*) FROM ehr_items WHERE encounter = ?1 AND (?2 = 1 OR domain != 'note') AND {}
         GROUP BY domain ORDER BY domain",
        crate::patient::item_shown("")
    ))?;
    let v = st.query_map(params![encounter, phi as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    Ok(v)
}

/// Items of the encounter's patient kept outside every encounter.
pub fn patient_level_count(c: &Connection, encounter: i64, phi: bool) -> anyhow::Result<i64> {
    Ok(c.query_row(
        &format!(
            "SELECT COUNT(*) FROM ehr_items WHERE encounter IS NULL AND patient = (SELECT patient FROM encounters WHERE id = ?1)
               AND (?2 = 1 OR domain != 'note') AND {}",
            crate::patient::item_shown("")
        ),
        params![encounter, phi as i64],
        |r| r.get(0),
    )?)
}

/// Items waiting for a linkage table (no patient known).
pub fn waiting_count(c: &Connection) -> anyhow::Result<i64> {
    Ok(c.query_row(
        &format!(
            "SELECT COUNT(*) FROM ehr_items WHERE encounter IS NULL AND patient IS NULL AND {}",
            crate::patient::item_shown("")
        ),
        [],
        |r| r.get(0),
    )?)
}

// ------------------------------------------------------------------ many items at once

/// [`insert_item_id`] for many items at once (an import's rows, a chunk at a time), with the
/// same answers: each item's encounter and patient found as [`resolve`] finds them, but by a few
/// queries for all; the same item stored before found by the database itself, a few hundred
/// items to a query; the new ones written many to a statement, their ids reserved first.  An
/// item that repeats an earlier one of the same call is its duplicate, as it would be one by one.
pub fn insert_items(
    tx: &Connection,
    batch: i64,
    items: &[(ItemRow, String)],
    cache: &mut Cache,
) -> crate::sql::Result<Vec<(Stored, i64)>> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    let found = resolve_many(tx, items.iter().map(|(it, _)| (&it.keys, it.time)).collect(), cache)?;
    let values: Vec<(Option<f64>, String)> = items.iter().map(|(it, _)| stored_value(it)).collect();
    // the same item stored before (one query per few hundred items), by the three ways
    // `insert_item_id` looks: among the encounter's items, the patient's, those waiting
    let mut before: Vec<Option<i64>> = vec![None; items.len()];
    for group in 0..3 {
        let idx: Vec<usize> = (0..items.len())
            .filter(|i| match found[*i] {
                (Some(_), _) => group == 0,
                (None, Some(_)) => group == 1,
                (None, None) => group == 2,
            })
            .collect();
        for chunk in idx.chunks(300) {
            let mut args: Vec<Value> = vec![];
            let mut rows = vec![];
            for &i in chunk {
                let (it, _) = &items[i];
                let (num, text) = &values[i];
                let at = args.len();
                args.extend([
                    Value::Integer(i as i64),
                    found[i].0.or(found[i].1).map_or(Value::Null, Value::Integer),
                    it.name.clone().into(),
                    it.time.into(),
                    it.domain.clone().into(),
                    it.code.clone().into(),
                    (*num).into(),
                    text.clone().into(),
                    it.unit.clone().into(),
                    it.end.into(),
                    it.file_path.clone().into(),
                    it.keys.encounter.clone().into(),
                    it.keys.csn.clone().into(),
                    it.keys.mrn.clone().into(),
                    it.keys.patient.clone().into(),
                    it.keys.authority.clone().into(),
                ]);
                let p = |k: usize| format!("?{}", at + k);
                rows.push(format!(
                    "(CAST({} AS INTEGER), CAST({} AS INTEGER), CAST({} AS TEXT), CAST({} AS REAL), CAST({} AS TEXT), CAST({} AS TEXT),
                      CAST({} AS REAL), CAST({} AS TEXT), CAST({} AS TEXT), CAST({} AS REAL), CAST({} AS TEXT),
                      CAST({} AS TEXT), CAST({} AS TEXT), CAST({} AS TEXT), CAST({} AS TEXT), CAST({} AS TEXT))",
                    p(1), p(2), p(3), p(4), p(5), p(6), p(7), p(8), p(9), p(10), p(11), p(12), p(13), p(14), p(15), p(16)
                ));
            }
            let keys = "x.key_encounter = s.ke AND x.key_csn = s.kc AND x.key_mrn = s.km AND x.key_patient = s.kp AND x.key_authority = s.ka";
            let whose = match group {
                0 => "x.encounter = s.w".to_string(),
                1 => format!("x.patient = s.w AND {keys}"),
                _ => format!("x.patient IS NULL AND {keys}"),
            };
            let sql = format!(
                "WITH s (i, w, n, t, d, c, vn, vt, u, et, f, ke, kc, km, kp, ka) AS (VALUES {})
                 SELECT s.i, (SELECT x.id FROM ehr_items x WHERE {whose} AND x.name = s.n AND x.time IS NOT DISTINCT FROM s.t
                    AND x.domain = s.d AND x.code = s.c AND x.value_num IS NOT DISTINCT FROM s.vn AND x.value_text = s.vt
                    AND x.unit = s.u AND x.end_time IS NOT DISTINCT FROM s.et AND x.file_path = s.f LIMIT 1)
                 FROM s",
                rows.join(", ")
            );
            let mut st = tx.prepare(&sql)?;
            for r in st.query_map(crate::sql::params_from_iter(args), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
            })? {
                let (i, id) = r?;
                before[i as usize] = id;
            }
        }
    }
    // new items: an item repeating an earlier new one of this call is its duplicate
    let mut first: HashMap<String, usize> = HashMap::new();
    let mut dup_of: Vec<Option<usize>> = vec![None; items.len()];
    let mut fresh: Vec<usize> = vec![];
    for i in 0..items.len() {
        if before[i].is_some() {
            continue;
        }
        let (it, _) = &items[i];
        let (num, text) = &values[i];
        let whose = match found[i] {
            (Some(e), _) => format!("e{e}"),
            (None, Some(p)) => format!("p{p}|{}", it.keys.cache_key()),
            (None, None) => format!("w|{}", it.keys.cache_key()),
        };
        let key = format!(
            "{whose}\u{1f}{}\u{1f}{:?}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{}\u{1f}{}\u{1f}{:?}\u{1f}{}",
            it.name,
            it.time.map(f64::to_bits),
            it.domain,
            it.code,
            num.map(f64::to_bits),
            text,
            it.unit,
            it.end.map(f64::to_bits),
            it.file_path
        );
        match first.get(&key) {
            Some(&j) => dup_of[i] = Some(j),
            None => {
                first.insert(key, i);
                fresh.push(i);
            }
        }
    }
    let ids = tx.next_ids("ehr_items", fresh.len())?;
    let mut id_of: HashMap<usize, i64> = HashMap::new();
    for (chunk, idc) in fresh.chunks(200).zip(ids.chunks(200)) {
        let mut args: Vec<Value> = vec![];
        let mut rows = vec![];
        for (&i, &id) in chunk.iter().zip(idc) {
            let (it, source) = &items[i];
            let (num, text) = &values[i];
            let (enc, patient) = found[i];
            id_of.insert(i, id);
            let at = args.len();
            args.extend([
                Value::Integer(id),
                enc.map_or(Value::Null, Value::Integer),
                patient.map_or(Value::Null, Value::Integer),
                it.keys.encounter.clone().into(),
                it.keys.csn.clone().into(),
                it.keys.mrn.clone().into(),
                it.keys.authority.clone().into(),
                it.keys.patient.clone().into(),
                it.domain.clone().into(),
                it.time.into(),
                it.end.into(),
                it.vocabulary.clone().into(),
                it.code.clone().into(),
                it.name.clone().into(),
                (*num).into(),
                text.clone().into(),
                it.unit.clone().into(),
                Value::Integer(batch),
                source.clone().into(),
                it.category.clone().into(),
                it.ref_low.into(),
                it.ref_high.into(),
                it.flag.clone().into(),
                it.author.clone().into(),
                it.service.clone().into(),
                it.file_path.clone().into(),
            ]);
            rows.push(format!("({})", (1..=26).map(|k| format!("?{}", at + k)).collect::<Vec<_>>().join(", ")));
        }
        tx.execute(
            &format!(
                "INSERT INTO ehr_items (id, encounter, patient, key_encounter, key_csn, key_mrn, key_authority, key_patient, domain,
                                        time, end_time, vocabulary, code, name, value_num, value_text, unit, import_id, source,
                                        category, ref_low, ref_high, flag, author, service, file_path)
                 VALUES {}",
                rows.join(", ")
            ),
            crate::sql::params_from_iter(args),
        )?;
    }
    let mut out = Vec::with_capacity(items.len());
    for i in 0..items.len() {
        let (_, source) = &items[i];
        let (stored, id) = match (before[i], dup_of[i]) {
            (Some(id), _) => (Stored::Duplicate, id),
            (None, Some(j)) => (Stored::Duplicate, id_of[&j]),
            (None, None) => (
                match found[i] {
                    (Some(_), _) => Stored::WithEncounter,
                    (None, Some(_)) => Stored::PatientLevel,
                    (None, None) => Stored::Waiting,
                },
                id_of[&i],
            ),
        };
        cache.provenance.push((id, batch, source.clone()));
        out.push((stored, id));
    }
    Ok(out)
}

/// [`resolve`] for many (keys, time) at once, with the same answers: an encounter by its ID or
/// CSN; else the patient by MRN, then by patient ID, and their encounter at the time.  What the
/// cache holds is not asked again.
fn resolve_many(
    tx: &Connection,
    asked: Vec<(&Keys, Option<f64>)>,
    cache: &mut Cache,
) -> crate::sql::Result<Vec<(Option<i64>, Option<i64>)>> {
    // a cache, not a copy of the table (as `insert_item_id`'s)
    if cache.resolved.len() > CACHE_MAX {
        cache.resolved.clear();
    }
    if cache.by_key.len() > CACHE_MAX {
        cache.by_key.clear();
    }
    let at_time = |t: Option<f64>| t.map(|t| t.to_bits() as i64);
    // by encounter ID, then CSN (found ones kept for every time: `Cache::by_key`), asked of what
    // the cache does not answer
    let mut eids: Vec<String> = vec![];
    let mut csns: Vec<String> = vec![];
    for (k, t) in &asked {
        let kk = k.cache_key();
        if cache.by_key.contains_key(&kk) || cache.resolved.contains_key(&(kk, at_time(*t))) {
            continue;
        }
        if !k.encounter.is_empty() {
            eids.push(k.encounter.clone());
        }
        if !k.csn.is_empty() {
            csns.push(k.csn.trim().to_lowercase());
        }
    }
    eids.sort();
    eids.dedup();
    csns.sort();
    csns.dedup();
    let by_eid: HashMap<String, (i64, i64)> = tx
        .query_in("SELECT encounter_id, id, patient FROM encounters WHERE encounter_id IN ({})", &eids, |r| {
            Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
        })?
        .into_iter()
        .collect();
    let mut by_csn: HashMap<String, (i64, i64)> = HashMap::new();
    for (c, e, p) in tx.query_in(
        "SELECT lower(trim(i.csn)), e.id, e.patient FROM encounter_identifiers i JOIN encounters e ON e.id = i.encounter
         WHERE lower(trim(i.csn)) IN ({})",
        &csns,
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
    )? {
        by_csn.entry(c).or_insert((e, p));
    }
    let mut out: Vec<Option<(Option<i64>, Option<i64>)>> = vec![None; asked.len()];
    let mut need_patient: Vec<usize> = vec![];
    for (i, (k, t)) in asked.iter().enumerate() {
        let kk = k.cache_key();
        let ck = (kk.clone(), at_time(*t));
        if let Some((e, p)) = cache.by_key.get(&kk) {
            out[i] = Some((Some(*e), Some(*p)));
            continue;
        }
        if let Some(f) = cache.resolved.get(&ck) {
            out[i] = Some(*f);
            continue;
        }
        let hit = (!k.encounter.is_empty())
            .then(|| by_eid.get(&k.encounter))
            .flatten()
            .or_else(|| (!k.csn.is_empty()).then(|| by_csn.get(&k.csn.trim().to_lowercase())).flatten());
        if let Some(&(e, p)) = hit {
            cache.by_key.insert(kk, (e, p));
            cache.resolved.insert(ck, (Some(e), Some(p)));
            out[i] = Some((Some(e), Some(p)));
        } else {
            need_patient.push(i);
        }
    }
    // the patient: by MRN, then by patient ID
    let mrns: Vec<(String, String)> = need_patient
        .iter()
        .map(|&i| asked[i].0)
        .filter(|k| !k.mrn.is_empty())
        .map(|k| (k.authority.trim().to_string(), crate::identity::mrn_key(&k.mrn)))
        .collect();
    let by_mrn = crate::identity::patients_by_mrn(tx, &mrns)?;
    let pids: Vec<String> =
        need_patient.iter().map(|&i| asked[i].0.patient.clone()).filter(|p| !p.is_empty()).collect();
    let by_pid = crate::identity::patients_by_id(tx, &pids)?;
    let patient_of = |k: &Keys| -> Option<i64> {
        let m = (!k.mrn.is_empty())
            .then(|| by_mrn.get(&(k.authority.trim().to_string(), crate::identity::mrn_key(&k.mrn))).copied())
            .flatten();
        m.or_else(|| (!k.patient.is_empty()).then(|| by_pid.get(&k.patient).copied()).flatten())
    };
    // their encounters, for the times (admitted, not discharged before)
    let mut pats: Vec<i64> =
        need_patient.iter().filter_map(|&i| patient_of(asked[i].0).filter(|_| asked[i].1.is_some())).collect();
    pats.sort();
    pats.dedup();
    let mut stays: HashMap<i64, Vec<(i64, f64, Option<f64>)>> = HashMap::new();
    for (p, e, a, d) in tx.query_in(
        "SELECT patient, id, admit_time, discharge_time FROM encounters WHERE patient IN ({}) AND admit_time IS NOT NULL",
        &pats,
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?, r.get::<_, Option<f64>>(3)?)),
    )? {
        stays.entry(p).or_default().push((e, a, d));
    }
    for v in stays.values_mut() {
        // `ORDER BY admit_time DESC`: the latest admission first (ties by id, newest first)
        v.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal).then(y.0.cmp(&x.0)));
    }
    for i in need_patient {
        let (k, t) = asked[i];
        let ck = (k.cache_key(), at_time(t));
        if let Some(f) = cache.resolved.get(&ck) {
            // an earlier item of this call with the same keys and minute
            out[i] = Some(*f);
            continue;
        }
        let f = match patient_of(k) {
            None => (None, None),
            Some(p) => {
                let enc = t.and_then(|t| {
                    stays
                        .get(&p)
                        .and_then(|v| v.iter().find(|(_, a, d)| *a <= t && d.is_none_or(|d| d >= t)).map(|(e, ..)| *e))
                });
                (enc, Some(p))
            }
        };
        cache.resolved.insert(ck, f);
        out[i] = Some(f);
    }
    Ok(out.into_iter().map(|x| x.unwrap_or((None, None))).collect())
}

/// The text index written once at the end of an import rather than item by item: SQLite's
/// full-text index otherwise flushes and merges its segments at every statement of the
/// import's transaction (most of an import's time).  Within the import's transaction, the
/// trigger that indexes each new item is set aside ([`index_text`] puts it back as it was);
/// a dry run's rollback puts it back too.  PostgreSQL's index takes rows as they come: there,
/// nothing is set aside.
pub fn defer_text_index(tx: &Connection) -> crate::sql::Result<Option<String>> {
    if tx.dialect() != crate::sql::Dialect::Sqlite {
        return Ok(None);
    }
    let sql: Option<String> = tx
        .query_row("SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'item_text_insert'", [], |r| {
            r.get(0)
        })
        .optional()?;
    if sql.is_some() {
        tx.execute_batch("DROP TRIGGER item_text_insert")?;
    }
    Ok(sql)
}

/// The text of the items batch `batch` added after item `after` (in this transaction),
/// indexed all at once, and the trigger [`defer_text_index`] set aside put back.
pub fn index_text(tx: &Connection, batch: i64, after: i64, deferred: Option<String>) -> crate::sql::Result<()> {
    let Some(trigger) = deferred else { return Ok(()) };
    tx.execute(
        "INSERT INTO item_text (rowid, body)
         SELECT id, name || ' ' || code || ' ' || vocabulary || ' ' || value_text || ' ' || unit || ' ' || author || ' ' || service
         FROM ehr_items WHERE import_id = ?1 AND id > ?2",
        [batch, after],
    )?;
    tx.execute_batch(&trigger)?;
    Ok(())
}
