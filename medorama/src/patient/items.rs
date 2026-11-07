//! A patient's items: the catalog's and the connected databases', and the report entry each belongs to.

use super::*;

/// A patient's items as the journey and the report use them.
pub(super) struct It {
    pub(super) id: i64,
    pub(super) encounter: Option<i64>,
    pub(super) category: String,
    pub(super) name: String,
    pub(super) code: String,
    pub(super) time: Option<f64>,
    pub(super) end: Option<f64>,
    pub(super) num: Option<f64>,
    pub(super) text: String,
    pub(super) unit: String,
    pub(super) level: u8,
    /// Below its range (else above, when abnormal).
    pub(super) flag_low: bool,
    pub(super) author: String,
    pub(super) service: String,
    pub(super) file: bool,
    pub(super) flag: String,
    pub(super) ref_low: Option<f64>,
    pub(super) ref_high: Option<f64>,
    /// A connected database's entry's vocabulary (the catalog's is read when it is shown).
    pub(super) vocabulary: String,
    /// An entry of a connected database (none: the catalog's item `id`).
    pub(super) live: Option<Live>,
}

/// Where an entry of a connected database comes from, and its token (its key never leaves).
#[derive(Clone)]
pub(super) struct Live {
    pub(super) connection: i64,
    pub(super) source: String,
    pub(super) token: String,
}

/// A patient's items: the catalog's, then the entries of the connected databases linked to
/// them (read now; a source that does not answer is left out: `unavailable` says which).
pub(super) fn items_of(c: &Connection, patient: i64) -> anyhow::Result<Vec<It>> {
    let mut v = catalog_items(c, patient)?;
    v.extend(live_items(c, patient)?.0);
    v.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    Ok(v)
}

/// The entries of the connected databases linked to a patient, as items (with negative ids,
/// for this answer only), and the sources that did not answer.
pub(super) fn live_items(c: &Connection, patient: i64) -> anyhow::Result<(Vec<It>, Vec<String>)> {
    let linked = crate::sources::link::persons_of(c, patient)?;
    if linked.is_empty() {
        return Ok((vec![], vec![]));
    }
    let secret = crate::gate::secret(c)?;
    let (mut out, mut unavailable) = (vec![], vec![]);
    for (conn, keys) in linked {
        let Some(s) = crate::sources::get(c, conn)? else { continue };
        let entries = match s.entries(&keys, s.zone("UTC")) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("medorama: {}: {e}", s.name);
                unavailable.push(s.name.clone());
                continue;
            }
        };
        for e in entries.into_iter().filter(|e| e.category != "allergy") {
            let mut h = sha2::Sha256::new();
            h.update(&secret);
            h.update(format!("{conn}:{}", e.key).as_bytes());
            let token: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
            let note = e.category == "note";
            let text = match (note, e.text.trim().is_empty()) {
                (true, false) => e.text.clone(),
                (false, false) if !e.value_text.is_empty() => format!("{}\n{}", e.value_text, e.text),
                (false, false) => e.text.clone(),
                _ => e.value_text.clone(),
            };
            out.push(It {
                id: -(out.len() as i64) - 1,
                encounter: None,
                level: level(&e.flag, e.value_num, e.ref_low, e.ref_high),
                flag_low: e.flag.trim().to_lowercase().starts_with('l')
                    || matches!((e.value_num, e.ref_low), (Some(v), Some(l)) if v < l),
                category: e.category,
                name: e.name,
                code: e.code,
                time: e.time,
                end: e.end,
                num: e.value_num,
                text,
                unit: e.unit,
                author: e.author,
                service: String::new(),
                file: false,
                flag: e.flag,
                ref_low: e.ref_low,
                ref_high: e.ref_high,
                vocabulary: e.vocabulary,
                live: Some(Live { connection: conn, source: s.name.clone(), token }),
            });
        }
    }
    Ok((out, unavailable))
}

/// The visits the connected databases linked to a patient hold, and the sources that did not
/// answer.
pub(super) fn live_visits(c: &Connection, patient: i64) -> anyhow::Result<(Vec<LiveVisit>, Vec<String>)> {
    let (mut out, mut unavailable) = (vec![], vec![]);
    for (conn, keys) in crate::sources::link::persons_of(c, patient)? {
        let Some(s) = crate::sources::get(c, conn)? else { continue };
        match s.visits(&keys, s.zone("UTC")) {
            Ok(v) => out.extend(v.into_iter().map(|v| LiveVisit {
                source: s.name.clone(),
                admit: v.start,
                discharge: v.end,
                icu: v.icu || categories::is_icu_unit(&v.unit),
                unit: v.unit,
            })),
            Err(e) => {
                eprintln!("medorama: {}: {e}", s.name);
                unavailable.push(s.name.clone());
            }
        }
    }
    out.sort_by(|a, b| b.admit.partial_cmp(&a.admit).unwrap_or(std::cmp::Ordering::Equal));
    Ok((out, unavailable))
}

pub(super) fn catalog_items(c: &Connection, patient: i64) -> anyhow::Result<Vec<It>> {
    let mut st = c.prepare(&format!(
        "SELECT id, encounter, category, name, code, time, end_time, value_num, value_text, unit, flag, ref_low, ref_high,
                author, service, file_path != ''
         FROM ehr_items WHERE patient = ?1 AND category != 'allergy' AND {} ORDER BY time, id",
        item_shown("")
    ))?;
    let v = st
        .query_map([patient], |r| {
            let (num, flag, lo, hi): (Option<f64>, String, Option<f64>, Option<f64>) =
                (r.get(7)?, r.get(10)?, r.get(11)?, r.get(12)?);
            Ok(It {
                id: r.get(0)?,
                encounter: r.get(1)?,
                category: r.get(2)?,
                name: r.get(3)?,
                code: r.get(4)?,
                time: r.get(5)?,
                end: r.get(6)?,
                num,
                text: r.get(8)?,
                unit: r.get(9)?,
                level: level(&flag, num, lo, hi),
                flag_low: flag.trim().to_lowercase().starts_with('l')
                    || matches!((num, lo), (Some(v), Some(l)) if v < l),
                author: r.get(13)?,
                service: r.get(14)?,
                file: r.get(15)?,
                flag,
                ref_low: lo,
                ref_high: hi,
                vocabulary: String::new(),
                live: None,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// The report entry an item belongs to (labs at one moment are one panel; vital signs one
/// entry a day).
pub(super) fn entry_key(it: &It) -> String {
    let enc = it.encounter.unwrap_or(0);
    match (it.category.as_str(), &it.live) {
        ("lab", _) if it.time.is_some() => format!("labs:{enc}:{}", it.time.unwrap_or(0.0)),
        ("vit", _) if it.time.is_some() => format!("vit:{enc}:{}", (it.time.unwrap_or(0.0) / 86400.0).floor() as i64),
        (_, Some(l)) => format!("live:{}:{}", l.connection, l.token),
        _ => format!("item:{}", it.id),
    }
}
