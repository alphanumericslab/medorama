//! Patients, encounters and bed stays from a database's rows (the identity rules), and the
//! links between recordings and encounters (docs/importing.md, "How rows find their patients").
//!
//! A row names its patient by a `patient_id` or an MRN (with the authority that issued it), and
//! may name an encounter (`encounter_id`, its `csn`, admission and discharge) and a bed stay
//! (`care_unit`, `bed`, `bed_start`, `bed_end`, `recording_file`). Each import's values are
//! kept ([`identity`]): a field shows the newest import's value, and removing an import brings
//! back the one before. Empty cells are no value. Rows in conflict with the catalog are held
//! back for review (`held_rows`); a patient known only by an MRN is given a `GEN-…` ID.
//!
//! Recordings are linked to encounters automatically: by `recording_file` when given, else by
//! bed (`UNIT_BED` from the file name = `care_unit` + `_` + `bed`) and time overlap with a bed
//! stay; a recording a table's row names (`file`) with its encounter is linked to that encounter
//! (by `file` too) while the import is there (`import_links`). Links made by hand
//! (`method = manual`) are never changed automatically.

use crate::db::{audit, now};
use crate::identity;
use crate::sql::types::Value;
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use crate::util::{parse_date, parse_time};
use std::collections::HashMap;

/// Column mapping from a TOML text (`[columns] canonical = "header"`, or the same keys at the
/// top level of a file that holds nothing else) or a JSON object
/// (`{"canonical": "header"}`, as the web page sends it).
pub fn read_mapping(text: &str) -> anyhow::Result<HashMap<String, String>> {
    if text.trim_start().starts_with('{') {
        let v: HashMap<String, String> = serde_json::from_str(text)?;
        return Ok(v.into_iter().filter(|(_, h)| !h.is_empty()).collect());
    }
    let t: toml::Table = text.parse()?;
    let mut m = HashMap::new();
    // the columns in a [columns] table, or (a file of nothing else) at the top level
    let (cols, prefix) = match t.get("columns").and_then(|v| v.as_table()) {
        Some(cols) => (cols, "columns."),
        None => (&t, ""),
    };
    for (k, v) in cols {
        m.insert(k.clone(), v.as_str().ok_or_else(|| anyhow::anyhow!("{prefix}{k} must be text"))?.to_string());
    }
    Ok(m)
}

#[cfg(test)]
mod mapping_tests {
    #[test]
    fn a_mapping_in_a_table_or_at_the_top_level() {
        let a = super::read_mapping("[columns]\nmrn = \"Hosp No\"\nname = \"Analyte\"\n").unwrap();
        let b = super::read_mapping("mrn = \"Hosp No\"\nname = \"Analyte\"\n").unwrap();
        assert_eq!(a, b);
        assert_eq!(a["mrn"], "Hosp No");
        assert!(super::read_mapping("mrn = 3\n").is_err());
        assert_eq!(super::read_mapping("{\"mrn\": \"Hosp No\"}").unwrap()["mrn"], "Hosp No");
    }
}

/// What a reviewer allows when applying a held-back row (the Review tab).
#[derive(Debug, Default, Clone, Copy)]
pub struct Force {
    /// The row's patient ID and MRN name two patients: merge the MRN's into the ID's; or the
    /// MRN's patient has another patient ID: take the row's ID to be the same person.
    pub merge: bool,
    /// The patient has another MRN of the same authority: add this one.
    pub other_mrn: bool,
    /// The encounter belongs to another patient: move it to the row's patient.
    pub move_encounter: bool,
}

/// What became of one row.
pub enum Outcome {
    Applied {
        patient_added: bool,
        patient_named: bool,
        encounter_added: bool,
        bed_stay_added: bool,
    },
    /// Held back: (problem, what was found, with pseudonymous IDs only).
    Held(&'static str, String),
    Invalid(String),
}

/// A linkage row's times (admit_time, discharge_time, bed_start, bed_end) and birth date, or
/// why the row cannot be applied.
pub fn row_values(row: &HashMap<String, String>, zone: chrono_tz::Tz) -> Result<(Vec<Option<f64>>, String), String> {
    let g = |k: &str| row.get(k).map(|s| s.trim()).unwrap_or("");
    if g("patient_id").is_empty() && g("mrn").is_empty() {
        return Err("a patient_id or an MRN is needed".into());
    }
    let times: Vec<Option<f64>> = ["admit_time", "discharge_time", "bed_start", "bed_end"]
        .iter()
        .map(|k| parse_time(g(k), zone))
        .collect::<Result<_, _>>()?;
    Ok((times, parse_date(g("birth_date"))?))
}

/// Apply one linkage row, as batch `batch` with source `source`.
pub fn apply_row(
    tx: &Connection,
    batch: i64,
    source: &str,
    row: &HashMap<String, String>,
    zone: chrono_tz::Tz,
    force: Force,
    user: Option<&User>,
) -> anyhow::Result<Outcome> {
    let g = |k: &str| row.get(k).map(|s| s.trim()).unwrap_or("");
    let (pid, mrn, auth, eid) = (g("patient_id"), g("mrn"), g("mrn_authority"), g("encounter_id"));
    // what is read below is decided and written by one transaction at a time (PostgreSQL)
    tx.lock_in_transaction(crate::sql::Lock::Identity)?;
    let (times, birth) = match row_values(row, zone) {
        Ok(v) => v,
        Err(e) => return Ok(Outcome::Invalid(e)),
    };
    let pid_of = |id: i64| -> crate::sql::Result<String> {
        tx.query_row("SELECT patient_id FROM patients WHERE id = ?1", [id], |r| r.get(0))
    };
    // the patient: by patient_id (following merges), else by MRN; checked before anything is written
    let mut merged = false;
    let by_pid = if pid.is_empty() { None } else { identity::patient_by_id(tx, pid)? };
    let mut by_mrn = if mrn.is_empty() { None } else { identity::patient_by_mrn(tx, auth, mrn)? };
    if let (Some(a), Some(b)) = (by_pid, by_mrn) {
        if a != b {
            if !force.merge {
                return Ok(Outcome::Held(
                    "two_patients",
                    format!("patient_id {pid} and the MRN name two patients: {} and {}", pid_of(a)?, pid_of(b)?),
                ));
            }
            identity::merge(tx, b, a, user)?;
            by_mrn = Some(a);
            merged = true;
        }
    }
    // a new patient ID for a patient known by its MRN: it takes the ID if it had only a generated
    // one; else the two IDs are taken to be one person only on a reviewer's word
    if let (None, Some(b), false) = (by_pid, by_mrn, pid.is_empty()) {
        let old = pid_of(b)?;
        if old.starts_with("GEN-") || force.merge {
            merged = true;
            if old.starts_with("GEN-") {
                tx.execute("UPDATE patients SET patient_id = ?1 WHERE id = ?2", params![pid, b])?;
                tx.execute("UPDATE patient_merges SET into_patient_id = ?1 WHERE into_patient_id = ?2", [pid, &old])?;
                // undone when this batch is removed (imports.rs)
                tx.execute(
                    "INSERT OR IGNORE INTO id_takeovers (patient, generated_id, patient_id, import_id) VALUES (?1, ?2, ?3, ?4)",
                    params![b, old, pid, batch],
                )?;
            }
            let (from, into) = if old.starts_with("GEN-") { (old.as_str(), pid) } else { (pid, old.as_str()) };
            tx.execute(
                "INSERT INTO patient_merges (from_patient_id, into_patient_id, user_id, merged_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (from_patient_id) DO UPDATE SET into_patient_id = excluded.into_patient_id",
                params![from, into, user.map(|u| u.id).filter(|i| *i > 0), now()],
            )?;
        } else {
            return Ok(Outcome::Held(
                "mrn_of_other_patient",
                format!("the MRN belongs to patient {old}, the row names patient {pid}"),
            ));
        }
    }
    if let (Some(a), None, false) = (by_pid, by_mrn, mrn.is_empty()) {
        if !force.other_mrn && identity::other_mrns(tx, a, auth, mrn)? {
            return Ok(Outcome::Held(
                "other_mrn",
                format!("patient {} has another MRN{} in the catalog", pid_of(a)?, authority_text(auth)),
            ));
        }
    }
    let known = by_pid.or(by_mrn);
    let enc: Option<(i64, i64)> = if eid.is_empty() {
        None
    } else {
        tx.query_row("SELECT id, patient FROM encounters WHERE encounter_id = ?1", [eid], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
    };
    if let Some((_, owner)) = enc {
        if Some(owner) != known && !force.move_encounter {
            let row_patient = if !pid.is_empty() { pid.to_string() } else { "a new patient".to_string() };
            return Ok(Outcome::Held(
                "encounter_of_other_patient",
                format!("encounter {eid} belongs to patient {}, the row gives it to {row_patient}", pid_of(owner)?),
            ));
        }
    }
    // the patient
    let t = now();
    let (mut patient_added, mut patient_named) = (false, false);
    let patient = match known {
        Some(id) => id,
        None => {
            let new_pid = if pid.is_empty() {
                patient_named = true;
                identity::generated_id(tx, auth, mrn)?
            } else {
                pid.to_string()
            };
            match identity::patient_by_id(tx, &new_pid)? {
                Some(id) => id,
                None => {
                    let id = tx
                        .insert("INSERT INTO patients (patient_id, created_at) VALUES (?1, ?2)", params![new_pid, t])?;
                    crate::pseudonyms::assign_patient(tx, id)?;
                    patient_added = true;
                    id
                }
            }
        }
    };
    crate::imports::note(tx, batch, source, "patient", patient)?;
    if !mrn.is_empty() {
        identity::give_mrn(tx, patient, auth, mrn, batch, source)?;
    }
    let fields: Vec<(&str, Value)> =
        [("sex", g("sex")), ("last_name", g("last_name")), ("first_name", g("first_name")), ("birth_date", &birth)]
            .into_iter()
            .map(|(f, v)| (f, Value::Text(v.to_string())))
            .collect();
    identity::give_all(tx, "patient", patient, &fields, batch, source)?;
    // a known patient the row only names (an encounter's row): their fields are as they were
    let gave = !mrn.is_empty() || fields.iter().any(|(_, v)| !matches!(v, Value::Text(t) if t.is_empty()));
    if gave || patient_added || merged {
        identity::recompute_patient(tx, patient)?;
    }
    if eid.is_empty() {
        // a row about the patient only
        return Ok(Outcome::Applied { patient_added, patient_named, encounter_added: false, bed_stay_added: false });
    }
    // the encounter
    let mut encounter_added = false;
    let enc = match enc {
        Some((id, owner)) => {
            if owner != patient {
                tx.execute("UPDATE encounters SET patient = ?1 WHERE id = ?2", params![patient, id])?;
                tx.execute("UPDATE ehr_items SET patient = ?1 WHERE encounter = ?2", params![patient, id])?;
                audit(tx, user, "move_encounter", eid, &format!("{} -> {}", pid_of(owner)?, pid_of(patient)?));
            }
            id
        }
        None => {
            let id = tx.insert(
                "INSERT INTO encounters (encounter_id, patient, created_at) VALUES (?1, ?2, ?3)",
                params![eid, patient, t],
            )?;
            encounter_added = true;
            crate::pseudonyms::assign_encounter(tx, id)?;
            id
        }
    };
    crate::imports::note(tx, batch, source, "encounter", enc)?;
    let num = |x: Option<f64>| x.map_or(Value::Null, Value::Real);
    let fields = [
        ("admit_time", num(times[0])),
        ("discharge_time", num(times[1])),
        ("admit_unit", Value::Text(g("admit_unit").into())),
        ("disposition", Value::Text(g("disposition").into())),
        ("csn", Value::Text(g("csn").into())),
    ];
    identity::give_all(tx, "encounter", enc, &fields, batch, source)?;
    identity::recompute_encounter(tx, enc)?;
    // the bed stay
    let mut bed_stay_added = false;
    if !(g("unit").is_empty() && g("bed").is_empty() && times[2].is_none() && g("recording_file").is_empty()) {
        let exists: Option<i64> = tx
            .query_row(
                "SELECT id FROM bed_stays WHERE encounter = ?1 AND unit = ?2 AND bed = ?3
                   AND start_time IS ?4 AND end_time IS ?5 AND recording_file = ?6",
                params![enc, g("unit"), g("bed"), times[2], times[3], g("recording_file")],
                |r| r.get(0),
            )
            .optional()?;
        let stay = match exists {
            Some(id) => id,
            None => {
                let id = tx.insert(
                    "INSERT INTO bed_stays (encounter, unit, bed, start_time, end_time, recording_file) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![enc, g("unit"), g("bed"), times[2], times[3], g("recording_file")],
                )?;
                bed_stay_added = true;
                id
            }
        };
        crate::imports::note(tx, batch, source, "bed_stay", stay)?;
    }
    Ok(Outcome::Applied { patient_added, patient_named, encounter_added, bed_stay_added })
}

fn authority_text(auth: &str) -> String {
    if auth.is_empty() {
        String::new()
    } else {
        format!(" of authority {auth}")
    }
}

fn label(unit: &str, bed: &str) -> String {
    if bed.is_empty() {
        unit.to_lowercase()
    } else {
        format!("{unit}_{bed}").to_lowercase()
    }
}

/// Link recordings (all, or the given ones) to encounters; returns how many recordings have
/// at least one automatic link afterwards.  Decisions made by hand (`link_decisions`) win: a
/// recording linked by hand gets exactly those links (when the encounters exist), and a pair
/// marked "never" is never linked automatically.
pub fn relink(c: &mut Connection, only: Option<&[i64]>) -> anyhow::Result<usize> {
    struct Stay {
        enc: i64,
        start: Option<f64>,
        end: Option<f64>,
    }
    let mut by_file: HashMap<String, Vec<i64>> = HashMap::new();
    let mut by_bed: HashMap<String, Vec<Stay>> = HashMap::new();
    {
        let mut st = c.prepare("SELECT encounter, unit, bed, start_time, end_time, recording_file FROM bed_stays")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let (enc, unit, bed, start, end, file): (i64, String, String, Option<f64>, Option<f64>, String) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
            if !file.is_empty() {
                by_file.entry(file.to_lowercase()).or_default().push(enc);
            } else if !unit.is_empty() && start.is_some() {
                by_bed.entry(label(&unit, &bed)).or_default().push(Stay { enc, start, end });
            }
        }
    }
    // decisions: recording -> (encounters to link, encounters never to link)
    let mut manual: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut never: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut decided = std::collections::HashSet::new();
    {
        let mut st = c.prepare(
            "SELECT d.recording, d.decision, e.id FROM link_decisions d LEFT JOIN encounters e ON e.encounter_id = d.encounter_id",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let (rec, decision, enc): (i64, String, Option<i64>) = (r.get(0)?, r.get(1)?, r.get(2)?);
            if decision == "link" {
                decided.insert(rec);
            }
            if let Some(e) = enc {
                if decision == "link" { &mut manual } else { &mut never }.entry(rec).or_default().push(e);
            }
        }
    }
    // the encounters imported rows gave recordings (by `file`, as a bed stay's recording_file)
    let mut by_import: HashMap<i64, Vec<i64>> = HashMap::new();
    {
        let mut st = c.prepare(
            "SELECT DISTINCT l.recording, e.id FROM import_links l JOIN encounters e ON e.encounter_id = l.encounter_id
             JOIN imports i ON i.id = l.import_id AND i.state != 'removed'",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            by_import.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
    }
    let recs: Vec<(i64, String, String, String, String, Option<f64>, Option<f64>, Option<f64>)> = {
        let mut st = c.prepare("SELECT id, name, path, unit, bed, name_time, t_start, t_end FROM recordings")?;

        st.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
        })?
        .collect::<Result<_, _>>()?
    };
    let only: Option<std::collections::HashSet<i64>> = only.map(|o| o.iter().copied().collect());
    let t = now();
    let tx = c.transaction()?;
    let mut linked = 0;
    for (id, name, path, unit, bed, name_time, t_start, t_end) in recs {
        if only.as_ref().is_some_and(|o| !o.contains(&id)) {
            continue;
        }
        let mut found: Vec<(i64, &str)> = vec![];
        if decided.contains(&id) {
            found.extend(manual.get(&id).into_iter().flatten().map(|e| (*e, "manual")));
        } else {
            found.extend(by_import.get(&id).into_iter().flatten().map(|e| (*e, "file")));
            for key in [name.to_lowercase(), path.to_lowercase()] {
                for &e in by_file.get(&key).into_iter().flatten() {
                    found.push((e, "file"));
                }
            }
            if found.is_empty() && !unit.is_empty() {
                let lo = t_start.or(name_time);
                let hi = t_end.or(t_start).or(name_time);
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    for s in by_bed.get(&label(&unit, &bed)).into_iter().flatten() {
                        if s.start.is_some_and(|a| a <= hi) && s.end.is_none_or(|b| b >= lo) {
                            found.push((s.enc, "bed_time"));
                        }
                    }
                }
            }
            let no = never.get(&id);
            found.retain(|(e, _)| !no.is_some_and(|n| n.contains(e)));
        }
        found.sort();
        found.dedup_by_key(|f| f.0);
        tx.execute("DELETE FROM recording_links WHERE recording = ?1", [id])?;
        for (enc, method) in &found {
            tx.execute(
                "INSERT INTO recording_links (recording, encounter, method, linked_at) VALUES (?1, ?2, ?3, ?4)",
                params![id, enc, method, t],
            )?;
        }
        if found.iter().any(|f| f.1 != "manual") {
            linked += 1;
        }
    }
    tx.commit()?;
    Ok(linked)
}

/// Decide by hand how a recording and an encounter relate (admins only): `link` (the recording
/// belongs to it; automatic links of the recording stop), `never` (never link them
/// automatically), or `None` (forget the decision).  Applied at once, and at every relink.
pub fn decide_link(
    c: &mut Connection,
    user: &User,
    recording: i64,
    encounter_id: &str,
    decision: Option<&str>,
) -> anyhow::Result<()> {
    if !user.is_admin() {
        anyhow::bail!("only admins change links");
    }
    match decision {
        Some(d @ ("link" | "never")) => {
            c.execute(
                "INSERT INTO link_decisions (recording, encounter_id, decision, user_id, decided_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (recording, encounter_id) DO UPDATE SET decision = excluded.decision,
                   user_id = excluded.user_id, decided_at = excluded.decided_at",
                params![recording, encounter_id, d, (user.id > 0).then_some(user.id), now()],
            )?;
        }
        None => {
            c.execute(
                "DELETE FROM link_decisions WHERE recording = ?1 AND encounter_id = ?2",
                params![recording, encounter_id],
            )?;
        }
        Some(other) => anyhow::bail!("unknown decision {other:?} (link, never)"),
    }
    audit(c, Some(user), "link_decision", &recording.to_string(), &format!("{encounter_id}: {decision:?}"));
    relink(c, Some(&[recording]))?;
    Ok(())
}

/// Link a recording to an encounter by hand (admins only); `None` forgets every manual link
/// of the recording.
pub fn link_manual(c: &mut Connection, user: &User, recording: i64, encounter: Option<i64>) -> anyhow::Result<()> {
    match encounter {
        Some(e) => {
            let eid: String = c.query_row("SELECT encounter_id FROM encounters WHERE id = ?1", [e], |r| r.get(0))?;
            decide_link(c, user, recording, &eid, Some("link"))
        }
        None => {
            if !user.is_admin() {
                anyhow::bail!("only admins change links");
            }
            c.execute("DELETE FROM link_decisions WHERE recording = ?1 AND decision = 'link'", [recording])?;
            audit(c, Some(user), "link_decision", &recording.to_string(), "manual links forgotten");
            relink(c, Some(&[recording]))?;
            Ok(())
        }
    }
}

/// Remove encounters with their bed stays and EHR items (admins only); a patient left without
/// encounters goes too, with the items kept at patient level.  Recordings stay, unlinked.
/// `dry_run`: count only.
pub fn remove_encounters(
    c: &mut Connection,
    user: &User,
    ids: &[i64],
    dry_run: bool,
) -> anyhow::Result<crate::imports::Removal> {
    if !user.is_admin() {
        anyhow::bail!("only admins remove entries");
    }
    let mut rep = crate::imports::Removal { dry_run, ..Default::default() };
    let tx = c.transaction()?;
    let mut patients = std::collections::HashSet::new();
    for id in ids {
        let row: Option<(String, i64)> = tx
            .query_row("SELECT encounter_id, patient FROM encounters WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((eid, patient)) = row else { continue };
        patients.insert(patient);
        rep.ehr_items += tx.execute("DELETE FROM ehr_items WHERE encounter = ?1", [id])?;
        rep.bed_stays +=
            tx.query_row("SELECT COUNT(*) FROM bed_stays WHERE encounter = ?1", [id], |r| r.get::<_, i64>(0))? as usize;
        rep.encounters += tx.execute("DELETE FROM encounters WHERE id = ?1", [id])?;
        if !dry_run {
            audit(&tx, Some(user), "remove_encounter", &eid, "");
        }
    }
    for p in patients {
        let left: bool =
            tx.query_row("SELECT EXISTS (SELECT 1 FROM encounters WHERE patient = ?1)", [p], |r| r.get(0))?;
        if !left {
            rep.ehr_items += tx.execute("DELETE FROM ehr_items WHERE patient = ?1", [p])?;
            rep.patients += tx.execute("DELETE FROM patients WHERE id = ?1", [p])?;
        }
    }
    crate::imports::prune(&tx)?;
    if dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }
    Ok(rep)
}
