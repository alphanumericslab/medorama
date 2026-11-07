//! The Patient Journey: columns, six-hour cells, spans.

use super::*;

// ------------------------------------------------------------------ the journey

/// How far outside its range, or flagged, a value is: 0 normal, 1 abnormal, 2 critical.
pub fn level(flag: &str, value: Option<f64>, low: Option<f64>, high: Option<f64>) -> u8 {
    let f = flag.trim().to_lowercase();
    if ["hh", "ll", "c", "crit", "critical", "panic", "aa", "!"].contains(&f.as_str()) {
        return 2;
    }
    if ["h", "l", "a", "abn", "abnormal", "high", "low", "pos", "positive", "r", "resistant", "detected"]
        .contains(&f.as_str())
    {
        return 1;
    }
    match value {
        Some(v) if low.is_some_and(|l| v < l) || high.is_some_and(|h| v > h) => 1,
        _ => 0,
    }
}

#[derive(Debug, Serialize)]
pub struct Lane {
    pub key: String,
    pub label: String,
    /// Sub-columns (tests, vital signs, drugs); one unnamed for single columns.
    pub subs: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Span {
    pub lane: usize,
    pub sub: usize,
    pub t0: f64,
    pub t1: f64,
    pub level: u8,
    /// The report entry it opens.
    pub key: String,
    /// A background span (an ICU stay), drawn light under what happened in it.
    pub light: bool,
    /// Its end is not known yet (a recording not indexed: an hour is shown).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
}

#[derive(Debug, Serialize)]
pub struct Journey {
    pub lanes: Vec<Lane>,
    pub bin: f64,
    /// (lane, sub, cell start, items, worst level)
    pub cells: Vec<(usize, usize, f64, u32, u8)>,
    pub spans: Vec<Span>,
}

/// Sub-columns of the split categories: their most frequent names, the rest folded into "Other".
pub(super) const SPLIT: &[(&str, usize)] = &[("vit", 5), ("lab", 12), ("med", 8)];

/// The categories some database provides (journey columns, in order).
pub fn present(c: &Connection) -> anyhow::Result<Vec<&'static str>> {
    let mut have: HashSet<String> = {
        let mut st = c.prepare(&format!("SELECT DISTINCT category FROM ehr_items WHERE {}", item_shown("")))?;

        st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    {
        let mut st = c.prepare(&format!("SELECT DISTINCT category FROM recordings WHERE {}", recording_shown("id")))?;
        for k in st.query_map([], |r| r.get::<_, String>(0))? {
            have.insert(k?);
        }
        let mut st = c.prepare(&format!("SELECT DISTINCT unit FROM bed_stays WHERE {}", stay_shown("id")))?;
        for u in st.query_map([], |r| r.get::<_, String>(0))? {
            if categories::is_icu_unit(&u?) {
                have.insert("icu".into());
            }
        }
    }
    let mut out = vec!["enc"];
    out.extend(categories::journey_keys().filter(|k| have.contains(*k)));
    Ok(out)
}

/// The recordings linked to a patient's encounters: (id, start, end, encounter, category).
pub(super) fn recordings_of(
    c: &Connection,
    patient: i64,
) -> anyhow::Result<Vec<(i64, Option<f64>, Option<f64>, i64, String)>> {
    let mut st = c.prepare(&format!(
        "SELECT DISTINCT r.id, COALESCE(r.t_start, r.name_time), r.t_end, l.encounter, r.category FROM recordings r
         JOIN recording_links l ON l.recording = r.id JOIN encounters e ON e.id = l.encounter
         WHERE e.patient = ?1 AND {} AND {}",
        recording_shown("r.id"),
        encounter_shown("e.id")
    ))?;
    let v = st
        .query_map([patient], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// At most about `n` points of a series, evenly spaced, keeping the one at `keep` (the reading
/// the detail shows: its mark must sit on the curve).
pub(super) fn thin(series: Vec<(f64, f64)>, keep: Option<f64>, n: usize) -> Vec<(f64, f64)> {
    if series.len() <= n {
        return series;
    }
    let step = series.len() / n + 1;
    let mut out: Vec<(f64, f64)> = series
        .iter()
        .enumerate()
        .filter(|(i, p)| i % step == 0 || i + 1 == series.len() || Some(p.0) == keep)
        .map(|(_, p)| *p)
        .collect();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// What a recording of a category is called in the report.
pub(super) fn recording_title(category: &str) -> &'static str {
    match category {
        "holter" => "Holter recording",
        "icu" => "ICU monitoring",
        "eeg" => "EEG recording",
        "sleep" => "Sleep study",
        _ => "Recording",
    }
}

pub fn journey(c: &Connection, user: &User, id: i64) -> anyhow::Result<Journey> {
    let _ = user;
    patient_exists(c, id)?;
    let items = items_of(c, id)?;
    // the columns some database provides, and those of this patient's connected data
    let mut keys = present(c)?;
    for k in categories::journey_keys() {
        if !keys.contains(&k) && items.iter().any(|i| i.category == k) {
            keys.push(k);
        }
    }
    let order: Vec<&str> = std::iter::once("enc").chain(categories::journey_keys()).collect();
    keys.sort_by_key(|k| order.iter().position(|o| o == k));
    // sub-columns: the most frequent names of the split categories
    let mut subs: HashMap<&str, Vec<String>> = HashMap::new();
    for (cat, n) in SPLIT {
        let mut count: HashMap<String, usize> = HashMap::new();
        for it in items.iter().filter(|i| i.category == *cat) {
            *count.entry(it.name.clone()).or_default() += 1;
        }
        let mut names: Vec<(String, usize)> = count.into_iter().collect();
        names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut v: Vec<String> = names.iter().take(*n).map(|x| x.0.clone()).collect();
        if names.len() > *n {
            v.push("Other".into());
        }
        if !v.is_empty() {
            subs.insert(cat, v);
        }
    }
    let lanes: Vec<Lane> = keys
        .iter()
        .map(|k| Lane {
            key: k.to_string(),
            label: if *k == "enc" { "Encounters".into() } else { categories::label(k).into() },
            subs: subs.get(k).cloned().unwrap_or_else(|| vec![String::new()]),
        })
        .collect();
    let lane_of = |k: &str| lanes.iter().position(|l| l.key == k);
    let sub_of = |lane: usize, name: &str| -> usize {
        let s = &lanes[lane].subs;
        s.iter().position(|x| x == name).unwrap_or(s.len() - 1)
    };
    let mut cells: BTreeMap<(usize, usize, i64), (u32, u8)> = BTreeMap::new();
    let mut spans = vec![];
    for it in &items {
        let (Some(lane), Some(t)) = (lane_of(&it.category), it.time) else { continue };
        let sub = sub_of(lane, &it.name);
        match it.end {
            Some(e) if e > t => spans.push(Span {
                lane,
                sub,
                t0: t,
                t1: e,
                level: it.level,
                key: entry_key(it),
                light: false,
                estimated: false,
            }),
            _ => {
                let cell = cells.entry((lane, sub, (t / BIN_S).floor() as i64)).or_insert((0, 0));
                cell.0 += 1;
                cell.1 = cell.1.max(it.level);
            }
        }
    }
    if let Some(lane) = lane_of("enc") {
        let mut st = c.prepare(&format!(
            "SELECT id, admit_time, discharge_time FROM encounters WHERE patient = ?1 AND admit_time IS NOT NULL AND {}",
            encounter_shown("id")
        ))?;
        let rows: Vec<(i64, f64, Option<f64>)> =
            st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        for (eid, a, b) in rows {
            spans.push(Span {
                lane,
                sub: 0,
                t0: a,
                t1: b.unwrap_or(a + 3600.0).max(a + 60.0),
                level: 0,
                key: format!("enc:{eid}"),
                light: false,
                estimated: false,
            });
        }
    }
    // ICU stays: the bed stays in intensive-care units, the whole time spent there
    if let Some(lane) = lane_of("icu") {
        let mut st = c.prepare(&format!(
            "SELECT b.encounter, b.unit, b.start_time, COALESCE(b.end_time, e.discharge_time) FROM bed_stays b
             JOIN encounters e ON e.id = b.encounter WHERE e.patient = ?1 AND b.start_time IS NOT NULL AND {} AND {}",
            stay_shown("b.id"),
            encounter_shown("e.id")
        ))?;
        let rows: Vec<(i64, String, f64, Option<f64>)> =
            st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
        for (eid, unit, a, b) in rows {
            if categories::is_icu_unit(&unit) {
                let t1 = b.unwrap_or(a + 86400.0).max(a + 600.0);
                spans.push(Span {
                    lane,
                    sub: 0,
                    t0: a,
                    t1,
                    level: 0,
                    key: format!("enc:{eid}"),
                    light: true,
                    estimated: false,
                });
            }
        }
    }
    // recordings, in their column (ICU monitoring, Holter …), drawn over the stays: from their
    // first sample to their last (known once indexed)
    for (rid, a, b, _, cat) in recordings_of(c, id)? {
        let (Some(lane), Some(a)) = (lane_of(&cat), a) else { continue };
        spans.push(Span {
            lane,
            sub: 0,
            t0: a,
            t1: b.unwrap_or(a + 3600.0).max(a + 60.0),
            level: 0,
            key: format!("rec:{rid}"),
            light: false,
            estimated: b.is_none(),
        });
    }
    Ok(Journey {
        lanes,
        bin: BIN_S,
        cells: cells.into_iter().map(|((l, s, b), (n, lv))| (l, s, b as f64 * BIN_S, n, lv)).collect(),
        spans,
    })
}
