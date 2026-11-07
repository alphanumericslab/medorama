//! Searching and describing encounters, and each user's history.
//!
//! Identifiers (MRN, names, date of birth, encounter number) are read, searched and returned
//! only for users with the PHI permission; for everyone else the queries never touch the
//! identifier tables.  Searches and views by PHI users are written to the audit log.

use crate::db::{audit, now};
use crate::recordings::{row_recording, summary, Recording, Status, REC_SELECT};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Search {
    /// The search text (search.rs): words, and `field:value` terms.
    pub q: String,
    pub unit: String,
    /// Stays overlapping [from, to] (Unix seconds).
    pub from: Option<f64>,
    pub to: Option<f64>,
    /// Summary status of the encounter's recordings (`ready`, `new`, ... or `none`).
    pub status: String,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Identifiers {
    pub mrn: String,
    pub last_name: String,
    pub first_name: String,
    pub birth_date: String,
    pub csn: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EncounterRow {
    pub id: i64,
    /// The patient's row (for the patient page).
    pub patient: i64,
    pub encounter_id: String,
    pub patient_id: String,
    pub sex: String,
    pub admit_time: Option<f64>,
    pub discharge_time: Option<f64>,
    pub admit_unit: String,
    pub disposition: String,
    pub units: Vec<String>,
    pub recordings: usize,
    /// Most urgent status of its recordings; None without recordings.
    pub status: Option<Status>,
    pub status_counts: HashMap<&'static str, usize>,
    /// The sources it came from.
    pub sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phi: Option<Identifiers>,
    /// What its recordings' own files say (PHI users only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_files: Option<crate::identify::FilesSummary>,
}

/// Status of every recording and the recordings of every encounter.
fn statuses(c: &Connection) -> anyhow::Result<(HashMap<i64, Status>, HashMap<i64, Vec<i64>>)> {
    let mut st = c.prepare(REC_SELECT)?;
    let s: HashMap<i64, Status> =
        st.query_map([], row_recording)?.map(|r| r.map(|r| (r.id, r.status))).collect::<Result<_, _>>()?;
    let mut links: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut st = c.prepare("SELECT encounter, recording FROM recording_links ORDER BY recording")?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        links.entry(r.get(0)?).or_default().push(r.get(1)?);
    }
    Ok((s, links))
}

fn units(c: &Connection) -> anyhow::Result<HashMap<i64, Vec<String>>> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let mut st = c.prepare(&format!(
        "SELECT encounter, unit FROM bed_stays WHERE unit != '' AND {} ORDER BY encounter, start_time",
        crate::patient::stay_shown("id")
    ))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let (e, u): (i64, String) = (r.get(0)?, r.get(1)?);
        let v = out.entry(e).or_default();
        if !v.contains(&u) {
            v.push(u);
        }
    }
    Ok(out)
}

const ENC_COLS: &str =
    "e.id, e.encounter_id, p.patient_id, p.sex, e.admit_time, e.discharge_time, e.admit_unit, e.disposition, p.id";
const PHI_COLS: &str = ", COALESCE(pi.mrn, ''), COALESCE(pi.last_name, ''), COALESCE(pi.first_name, ''), COALESCE(pi.birth_date, ''), COALESCE(ei.csn, '')";
const PHI_JOINS: &str =
    " LEFT JOIN patient_identifiers pi ON pi.patient = p.id LEFT JOIN encounter_identifiers ei ON ei.encounter = e.id";

/// SQL condition: the encounter `e` and its patient `p` are shown (not only an import still
/// loading holds them).
fn shown() -> String {
    format!("{} AND {}", crate::patient::encounter_shown("e.id"), crate::patient::patient_shown("p.id"))
}

fn row_encounter(r: &crate::sql::Row, phi: bool) -> crate::sql::Result<EncounterRow> {
    Ok(EncounterRow {
        id: r.get(0)?,
        encounter_id: r.get(1)?,
        patient_id: r.get(2)?,
        sex: r.get(3)?,
        admit_time: r.get(4)?,
        discharge_time: r.get(5)?,
        admit_unit: r.get(6)?,
        disposition: r.get(7)?,
        patient: r.get(8)?,
        units: vec![],
        recordings: 0,
        status: None,
        status_counts: HashMap::new(),
        sources: vec![],
        phi: if phi {
            Some(Identifiers {
                mrn: r.get(9)?,
                last_name: r.get(10)?,
                first_name: r.get(11)?,
                birth_date: r.get(12)?,
                csn: r.get(13)?,
            })
        } else {
            None
        },
        in_files: None,
    })
}

/// The patient of a row as `identify` compares it: (mrn, last, first, birth).
fn patient_of(e: &EncounterRow) -> Option<(&str, &str, &str, &str)> {
    e.phi.as_ref().map(|p| (p.mrn.as_str(), p.last_name.as_str(), p.first_name.as_str(), p.birth_date.as_str()))
}

fn fill(
    rows: &mut [EncounterRow],
    st: &HashMap<i64, Status>,
    links: &HashMap<i64, Vec<i64>>,
    units: &HashMap<i64, Vec<String>>,
) {
    for e in rows.iter_mut() {
        let recs = links.get(&e.id).map(Vec::as_slice).unwrap_or(&[]);
        let s: Vec<Status> = recs.iter().filter_map(|r| st.get(r).copied()).collect();
        e.recordings = recs.len();
        e.status = summary(&s);
        for x in s {
            *e.status_counts.entry(x.as_str()).or_default() += 1;
        }
        e.units = units.get(&e.id).cloned().unwrap_or_default();
        if e.units.is_empty() && !e.admit_unit.is_empty() {
            e.units.push(e.admit_unit.clone());
        }
    }
}

/// Encounter IDs matching a search (all pages), most recent first.
pub fn search_ids(c: &Connection, user: &User, s: &Search, timezone: &str) -> anyhow::Result<Vec<i64>> {
    search_ids_in(c, user, None, s, timezone)
}

/// [`search_ids`] through a session's gate (Privacy Mode: built here when not given).
pub fn search_ids_in(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    s: &Search,
    timezone: &str,
) -> anyhow::Result<Vec<i64>> {
    let g = crate::search::session_gate(c, user, gate)?;
    Ok(search_rows(c, user, g.get(), s, timezone)?.into_iter().map(|e| e.id).collect())
}

/// Lists of strings per id, from a query returning (id, text).
fn grouped(c: &Connection, sql: &str) -> anyhow::Result<HashMap<i64, Vec<String>>> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    let mut st = c.prepare(sql)?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let (id, t): (i64, String) = (r.get(0)?, r.get(1)?);
        let v = out.entry(id).or_default();
        if !t.is_empty() && !v.contains(&t) {
            v.push(t);
        }
    }
    Ok(out)
}

/// What the recordings' own files hold, per recording: (names, ids, dates of birth).
fn file_identifiers(c: &Connection) -> anyhow::Result<HashMap<i64, [String; 3]>> {
    let mut st = c.prepare(
        "SELECT recording, trim(full_name || ' ' || last_name || ' ' || first_name), patient_id, birth_date
         FROM recording_identifiers WHERE state = 'found'",
    )?;
    let v = st
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, [r.get(1)?, r.get(2)?, r.get(3)?])))?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(v)
}

/// The encounters a search finds.  In Privacy Mode (`gate`), it finds, filters and orders by
/// what the session is shown (search.rs): pseudonyms, recordings' tokens, each patient's times
/// moved back; `from` and `to` are times as shown.
fn search_rows(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    s: &Search,
    timezone: &str,
) -> anyhow::Result<Vec<EncounterRow>> {
    let phi = user.phi;
    let terms = crate::search::parse(&s.q);
    crate::search::check(&terms, crate::search::ENCOUNTER_FIELDS, phi).map_err(anyhow::Error::msg)?;
    let offsets = gate.map(crate::search::Offsets::of_gate);
    let shift = |p: i64, t: Option<f64>| match &offsets {
        Some(o) => t.map(|t| o.shift(Some(p), t)),
        None => t,
    };
    let sql = format!(
        "SELECT {ENC_COLS}{} FROM encounters e JOIN patients p ON p.id = e.patient{} WHERE {}
         ORDER BY e.admit_time IS NULL, e.admit_time DESC, e.id DESC",
        if phi { PHI_COLS } else { "" },
        if phi { PHI_JOINS } else { "" },
        shown(),
    );
    let mut st = c.prepare(&sql)?;
    let mut rows: Vec<EncounterRow> = st.query_map([], |r| row_encounter(r, phi))?.collect::<Result<_, _>>()?;
    let (stat, links) = statuses(c)?;
    let u = units(c)?;
    fill(&mut rows, &stat, &links, &u);
    let sources = grouped(c, "SELECT row_id, source FROM provenance WHERE kind = 'encounter' ORDER BY source")?;
    for e in rows.iter_mut() {
        e.sources = sources.get(&e.id).cloned().unwrap_or_default();
    }
    // filters of the page (unit, dates, status)
    let unit = s.unit.trim().to_lowercase();
    rows.retain(|e| {
        (unit.is_empty() || e.units.iter().any(|x| x.to_lowercase() == unit) || e.admit_unit.to_lowercase() == unit)
            && s.from.is_none_or(|f| shift(e.patient, e.discharge_time).is_none_or(|d| d >= f))
            && s.to.is_none_or(|t| shift(e.patient, e.admit_time).is_none_or(|a| a <= t))
            && (s.status.is_empty()
                || match e.status {
                    None => s.status == "none",
                    Some(x) => x.as_str() == s.status,
                })
    });
    if offsets.is_some() {
        // in the order shown: the latest admission (as shown) first
        rows.sort_by(|a, b| {
            let (x, y) = (shift(a.patient, a.admit_time), shift(b.patient, b.admit_time));
            y.is_some()
                .cmp(&x.is_some())
                .then(y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal))
                .then(b.id.cmp(&a.id))
        });
    }
    if terms.is_empty() {
        return Ok(rows);
    }
    if let Some(g) = gate {
        return search_shown(c, g, rows, &links, &terms, timezone);
    }
    let beds = grouped(c, "SELECT encounter, bed FROM bed_stays")?;
    let names = grouped(c, "SELECT id, name FROM recordings")?;
    let files = if phi { file_identifiers(c)? } else { HashMap::new() };
    let mrns = if phi {
        grouped(c, "SELECT DISTINCT e.id, m.mrn FROM encounters e JOIN patient_mrns m ON m.patient = e.patient")?
    } else {
        HashMap::new()
    };
    let zone = crate::util::tz(timezone);
    let t = |x: Option<f64>| x.map(|x| crate::util::fmt_local(x, zone)).unwrap_or_default();
    rows.retain(|e| {
        let mut d = crate::search::Doc::default();
        d.add("encounter", &e.encounter_id);
        d.add("patient", &e.patient_id);
        for x in &e.sources {
            d.add("source", x);
        }
        d.add("unit", &e.admit_unit);
        for x in &e.units {
            d.add("unit", x);
        }
        for x in beds.get(&e.id).into_iter().flatten() {
            d.add("bed", x);
        }
        d.add("admit", &t(e.admit_time));
        d.add("discharge", &t(e.discharge_time));
        d.add("disposition", &e.disposition);
        d.add("sex", &e.sex);
        d.add("status", e.status.map_or("none", |x| x.as_str()));
        for r in links.get(&e.id).into_iter().flatten() {
            for n in names.get(r).into_iter().flatten() {
                d.add("file", n);
            }
            if let Some([name, id, dob]) = files.get(r) {
                d.add("file-name", name);
                d.add("file-id", id);
                d.add("file-dob", dob);
            }
        }
        if let Some(p) = &e.phi {
            d.add("name", &format!("{}, {}", p.last_name, p.first_name));
            d.add("name", &format!("{} {}", p.first_name, p.last_name));
            d.add("first", &p.first_name);
            d.add("last", &p.last_name);
            d.add("dob", &p.birth_date);
            d.add("mrn", &p.mrn);
            for m in mrns.get(&e.id).into_iter().flatten() {
                d.add("mrn", m);
            }
            d.add("csn", &p.csn);
        }
        d.matches(&terms, phi)
    });
    Ok(rows)
}

/// The encounters whose values, as a session in Privacy Mode is shown them, match the terms:
/// each row through the gate (pseudonyms, each patient's times moved back), its recordings by
/// their tokens.
fn search_shown(
    c: &Connection,
    g: &crate::gate::Restricted,
    rows: Vec<EncounterRow>,
    links: &HashMap<i64, Vec<i64>>,
    terms: &[crate::search::Term],
    timezone: &str,
) -> anyhow::Result<Vec<EncounterRow>> {
    let beds = grouped(c, "SELECT encounter, bed FROM bed_stays")?;
    let zone = crate::util::tz(timezone);
    let t = |v: &serde_json::Value| v.as_f64().map(|x| crate::util::fmt_local(x, zone)).unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter(|e| {
            let v = crate::search::shown(g, e);
            let mut d = crate::search::Doc::default();
            let mut add = |field: &'static str, x: &serde_json::Value| {
                for s in crate::search::strings(x) {
                    d.add(field, &s);
                }
            };
            add("encounter", &v["encounter_id"]);
            add("patient", &v["patient_id"]);
            add("source", &v["sources"]);
            add("unit", &v["admit_unit"]);
            add("unit", &v["units"]);
            add("disposition", &v["disposition"]);
            add("sex", &v["sex"]);
            add("status", &v["status"]);
            if v["status"].is_null() {
                d.add("status", "none");
            }
            let own = g.for_patient(e.patient);
            for x in beds.get(&e.id).into_iter().flatten() {
                d.add("bed", &own.text(x));
            }
            d.add("admit", &t(&v["admit_time"]));
            d.add("discharge", &t(&v["discharge_time"]));
            for r in links.get(&e.id).into_iter().flatten() {
                d.add("file", &crate::gate::recording_token(*r));
            }
            d.matches(terms, false)
        })
        .collect())
}

/// One page of encounters matching a search, and the number of matches.
pub fn search(c: &Connection, user: &User, s: &Search, timezone: &str) -> anyhow::Result<(usize, Vec<EncounterRow>)> {
    search_in(c, user, None, s, timezone)
}

/// [`search`] through a session's gate (Privacy Mode: built here when not given).  Searches in
/// Privacy Mode are audited (`privacy_search`), as those of identifiers are (`phi_search`).
pub fn search_in(
    c: &Connection,
    user: &User,
    gate: Option<&crate::gate::Restricted>,
    s: &Search,
    timezone: &str,
) -> anyhow::Result<(usize, Vec<EncounterRow>)> {
    let g = crate::search::session_gate(c, user, gate)?;
    let rows = search_rows(c, user, g.get(), s, timezone)?;
    if !s.q.trim().is_empty() {
        if user.phi {
            audit(c, Some(user), "phi_search", "", &s.q);
        } else if user.privacy {
            audit(c, Some(user), "privacy_search", "encounters", &s.q);
        }
    }
    let total = rows.len();
    let offset = s.offset.unwrap_or(0).max(0) as usize;
    let limit = s.limit.unwrap_or(100).clamp(1, 1000) as usize;
    let mut page: Vec<EncounterRow> = rows.into_iter().skip(offset).take(limit).collect();
    if user.phi {
        for e in page.iter_mut() {
            e.in_files = Some(crate::identify::files_summary(c, e.id, patient_of(e))?);
        }
    }
    Ok((total, page))
}

#[derive(Debug, Serialize)]
pub struct BedStay {
    pub unit: String,
    pub bed: String,
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
    pub recording_file: String,
}

#[derive(Debug, Serialize)]
pub struct ResultView {
    pub job_id: i64,
    pub action: String,
    pub version: String,
    pub state: String,
    pub finished_at: Option<f64>,
    pub output_dir: String,
    pub summary: serde_json::Value,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct LinkedRecording {
    #[serde(flatten)]
    pub recording: Recording,
    pub link: String,
    pub results: Vec<ResultView>,
    pub sources: Vec<String>,
    /// The identifiers in the recording's own file, compared with the patient's (PHI users
    /// only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_file: Option<crate::identify::InFile>,
}

#[derive(Debug, Serialize)]
pub struct EncounterDetail {
    #[serde(flatten)]
    pub encounter: EncounterRow,
    pub bed_stays: Vec<BedStay>,
    pub recording_list: Vec<LinkedRecording>,
    pub ehr_counts: Vec<(String, i64)>,
    /// EHR items of the patient outside every encounter.
    pub patient_level_items: i64,
    /// The patient's other encounters: (id, encounter_id, admit_time).
    pub other_encounters: Vec<(i64, String, Option<f64>)>,
}

/// The newest job of each action on a recording (index excluded).
pub fn results(c: &Connection, recording: i64) -> anyhow::Result<Vec<ResultView>> {
    let mut st = c.prepare(
        "SELECT id, action, action_version, state, finished_at, output_dir, summary, error FROM jobs
         WHERE id IN (SELECT MAX(id) FROM jobs WHERE recording = ?1 AND action NOT IN ('index', 'identify') GROUP BY action)
         ORDER BY action",
    )?;
    let v = st
        .query_map([recording], |r| {
            let s: String = r.get(6)?;
            Ok(ResultView {
                job_id: r.get(0)?,
                action: r.get(1)?,
                version: r.get(2)?,
                state: r.get(3)?,
                finished_at: r.get(4)?,
                output_dir: r.get(5)?,
                summary: serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
                error: r.get(7)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// An encounter with its bed stays, recordings and results; adds it to the user's history.
pub fn encounter(c: &Connection, user: &User, id: i64) -> anyhow::Result<Option<EncounterDetail>> {
    let phi = user.phi;
    let sql = format!(
        "SELECT {ENC_COLS}{} FROM encounters e JOIN patients p ON p.id = e.patient{} WHERE e.id = ?1 AND {}",
        if phi { PHI_COLS } else { "" },
        if phi { PHI_JOINS } else { "" },
        shown(),
    );
    let Some(mut row) = c.query_row(&sql, [id], |r| row_encounter(r, phi)).optional()? else { return Ok(None) };
    let mut st = c.prepare(&format!("{REC_SELECT} JOIN recording_links l ON l.recording = r.id WHERE l.encounter = ?1 ORDER BY COALESCE(r.t_start, r.name_time), r.id"))?;
    let recs: Vec<Recording> = st.query_map([id], row_recording)?.collect::<Result<_, _>>()?;
    let mut list = vec![];
    for r in recs {
        let link: String = c.query_row(
            "SELECT method FROM recording_links WHERE recording = ?1 AND encounter = ?2",
            params![r.id, id],
            |x| x.get(0),
        )?;
        let in_file = if phi { crate::identify::in_file(c, r.id, patient_of(&row))? } else { None };
        let sources = crate::imports::sources_of(c, "recording", r.id)?;
        list.push(LinkedRecording { results: results(c, r.id)?, recording: r, link, sources, in_file });
    }
    let s: Vec<Status> = list.iter().map(|l| l.recording.status).collect();
    row.recordings = list.len();
    if phi {
        row.in_files = Some(crate::identify::files_summary(c, id, patient_of(&row))?);
    }
    row.status = summary(&s);
    for x in s {
        *row.status_counts.entry(x.as_str()).or_default() += 1;
    }
    let mut st = c.prepare(&format!(
        "SELECT unit, bed, start_time, end_time, recording_file FROM bed_stays WHERE encounter = ?1 AND {} ORDER BY start_time, id",
        crate::patient::stay_shown("id")
    ))?;
    let stays: Vec<BedStay> = st
        .query_map([id], |r| {
            Ok(BedStay {
                unit: r.get(0)?,
                bed: r.get(1)?,
                start_time: r.get(2)?,
                end_time: r.get(3)?,
                recording_file: r.get(4)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    for s in &stays {
        if !s.unit.is_empty() && !row.units.contains(&s.unit) {
            row.units.push(s.unit.clone());
        }
    }
    let mut st = c.prepare(&format!(
        "SELECT e2.id, e2.encounter_id, e2.admit_time FROM encounters e2 JOIN encounters e ON e.patient = e2.patient
         WHERE e.id = ?1 AND e2.id != ?1 AND {} ORDER BY e2.admit_time",
        crate::patient::encounter_shown("e2.id")
    ))?;
    let others = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    row.sources = crate::imports::sources_of(c, "encounter", id)?;
    let detail = EncounterDetail {
        ehr_counts: crate::ehr::counts(c, id, phi)?,
        patient_level_items: crate::ehr::patient_level_count(c, id, phi)?,
        encounter: row,
        bed_stays: stays,
        recording_list: list,
        other_encounters: others,
    };
    remember(c, user, id)?;
    if phi {
        audit(c, Some(user), "phi_view", &detail.encounter.encounter_id, "");
    }
    Ok(Some(detail))
}

/// Add an encounter to the user's history (or update its time).
pub fn remember(c: &Connection, user: &User, encounter: i64) -> anyhow::Result<()> {
    if user.id <= 0 {
        return Ok(());
    }
    let t = now();
    c.execute(
        "INSERT INTO history (user_id, encounter, first_seen, last_seen, views) VALUES (?1, ?2, ?3, ?3, 1)
         ON CONFLICT (user_id, encounter) DO UPDATE SET last_seen = excluded.last_seen, views = history.views + 1",
        params![user.id, encounter, t],
    )?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct HistoryRow {
    #[serde(flatten)]
    pub encounter: EncounterRow,
    pub first_seen: f64,
    pub last_seen: f64,
    pub views: i64,
}

/// The user's history, most recent first.
pub fn history(c: &Connection, user: &User, limit: i64) -> anyhow::Result<Vec<HistoryRow>> {
    let phi = user.phi;
    let sql = format!(
        "SELECT {ENC_COLS}{}, h.first_seen, h.last_seen, h.views FROM history h
         JOIN encounters e ON e.id = h.encounter JOIN patients p ON p.id = e.patient{}
         WHERE h.user_id = ?1 AND {} ORDER BY h.last_seen DESC LIMIT ?2",
        if phi { PHI_COLS } else { "" },
        if phi { PHI_JOINS } else { "" },
        shown(),
    );
    let k = if phi { 14 } else { 9 };
    let mut st = c.prepare(&sql)?;
    let mut rows: Vec<HistoryRow> = st
        .query_map(params![user.id, limit], |r| {
            Ok(HistoryRow {
                encounter: row_encounter(r, phi)?,
                first_seen: r.get(k)?,
                last_seen: r.get(k + 1)?,
                views: r.get(k + 2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let (stat, links) = statuses(c)?;
    let u = units(c)?;
    let mut encs: Vec<EncounterRow> = rows.iter().map(|h| h.encounter.clone()).collect();
    fill(&mut encs, &stat, &links, &u);
    for (h, e) in rows.iter_mut().zip(encs) {
        h.encounter = e;
    }
    Ok(rows)
}

/// Remove encounters from the user's history (all of them with `None`).
pub fn forget(c: &Connection, user: &User, encounter: Option<i64>) -> anyhow::Result<usize> {
    Ok(match encounter {
        Some(e) => c.execute("DELETE FROM history WHERE user_id = ?1 AND encounter = ?2", params![user.id, e])?,
        None => c.execute("DELETE FROM history WHERE user_id = ?1", [user.id])?,
    })
}
