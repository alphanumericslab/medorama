//! The Patient Journey: columns, six-hour cells, spans.

use super::*;

// ------------------------------------------------------------------ the journey

/// How far outside its range, or flagged, a value is: 0 normal, 1 abnormal, 2 critical.  A
/// flag says it first: a value flagged normal (`N`, `WNL`) is normal whatever its range says.
pub fn level(flag: &str, value: Option<f64>, low: Option<f64>, high: Option<f64>) -> u8 {
    let f = flag.trim().to_lowercase();
    if ["n", "nl", "norm", "normal", "wnl", "within normal limits"].contains(&f.as_str()) {
        return 0;
    }
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

/// The categories the databases a user may view provide (journey columns, in order), from
/// their variables and recordings.
pub fn present(c: &Connection, cfg: &Config, user: &User) -> anyhow::Result<Vec<&'static str>> {
    let mut have: HashSet<String> = HashSet::new();
    for db in crate::databases::granted(c, user, "view")? {
        for v in crate::variables::of(cfg, c, db).unwrap_or_default() {
            have.insert(v.category);
        }
        have.extend(crate::variables::recording_categories(cfg, c, db).unwrap_or_default());
    }
    let mut out = vec!["enc"];
    out.extend(categories::journey_keys().filter(|k| have.contains(*k)));
    Ok(out)
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

/// The intensive-care spans of the patient's visits, (start, end, the visit's key): the
/// intensive-care stays their databases give, else, for an intensive-care visit with none, the
/// whole visit.  A stay whose visit is not known is drawn on its own.
pub(super) fn icu_spans(visits: &[Visit], stays: &[Stay]) -> Vec<(f64, f64, String)> {
    let mut out = vec![];
    let icu_stays: Vec<&Stay> = stays.iter().filter(|x| x.icu && x.start.is_some()).collect();
    for v in visits {
        let Some(a) = v.admit else { continue };
        let theirs: Vec<&&Stay> = icu_stays.iter().filter(|x| x.visit == Some(v.id)).collect();
        if theirs.is_empty() {
            if v.icu {
                out.push((a, v.discharge.unwrap_or(a + 86400.0).max(a + 600.0), v.key.clone()));
            }
            continue;
        }
        for x in theirs {
            let b = x.start.unwrap_or(a);
            // a stay not ended yet: until the visit's end, else a day
            let e = x.end.or(v.discharge).unwrap_or(b + 86400.0).max(b + 600.0);
            out.push((b, e, v.key.clone()));
        }
    }
    for x in icu_stays.iter().filter(|x| x.visit.is_none_or(|id| !visits.iter().any(|v| v.id == id))) {
        let b = x.start.unwrap_or(0.0);
        out.push((b, x.end.unwrap_or(b + 86400.0).max(b + 600.0), String::new()));
    }
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

pub fn journey(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<Journey> {
    patient_exists(c, user, id)?;
    let d = load(cfg, c, user, id)?;
    let items = &d.items;
    // the columns the databases provide, and those of this patient's data
    let mut keys = present(c, cfg, user)?;
    for k in categories::journey_keys() {
        let theirs = items.iter().any(|i| i.category == k)
            || d.recordings.iter().any(|r| r.category == k)
            || (k == "icu" && (d.visits.iter().any(|v| v.icu) || d.stays.iter().any(|x| x.icu)));
        if !keys.contains(&k) && theirs {
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
    // an ECG counts once, not once more for each of its measurements
    let mut ecgs: HashMap<&str, Vec<&It>> = HashMap::new();
    for it in items {
        if let Some(k) = &it.ecg {
            ecgs.entry(k.as_str()).or_default().push(it);
        }
    }
    for it in items {
        if it.ecg.as_deref().and_then(|k| ecgs.get(k)).is_some_and(|g| !is_ecg_head(it, g)) {
            continue;
        }
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
    for v in &d.visits {
        let Some(a) = v.admit else { continue };
        if let Some(lane) = lane_of("enc") {
            spans.push(Span {
                lane,
                sub: 0,
                t0: a,
                t1: v.discharge.unwrap_or(a + 3600.0).max(a + 60.0),
                level: 0,
                key: v.key.clone(),
                light: false,
                estimated: false,
            });
        }
    }
    // intensive-care stays: the time spent there (the visit's, when its stays are not known),
    // drawn light under what happened
    if let Some(lane) = lane_of("icu") {
        for (t0, t1, key) in icu_spans(&d.visits, &d.stays) {
            spans.push(Span { lane, sub: 0, t0, t1, level: 0, key, light: true, estimated: false });
        }
    }
    // recordings, in their column (ICU monitoring, Holter …), drawn over the stays: from their
    // first sample to their last (known once indexed)
    // (an entry's file is drawn as its entry)
    for r in d.recordings.iter().filter(|r| !d.attached.contains(&r.id)) {
        let (Some(lane), Some(a)) = (lane_of(&r.category), r.t_start) else { continue };
        spans.push(Span {
            lane,
            sub: 0,
            t0: a,
            t1: r.t_end.unwrap_or(a + 3600.0).max(a + 60.0),
            level: 0,
            key: format!("rec:{}", r.id),
            light: false,
            estimated: r.t_end.is_none(),
        });
    }
    Ok(Journey {
        lanes,
        bin: BIN_S,
        cells: cells.into_iter().map(|((l, s, b), (n, lv))| (l, s, b as f64 * BIN_S, n, lv)).collect(),
        spans,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flag_of_normal_wins_over_the_range() {
        assert_eq!(level("N", Some(9.0), Some(1.0), Some(5.0)), 0);
        assert_eq!(level("WNL", Some(0.1), Some(1.0), Some(5.0)), 0);
        assert_eq!(level("normal", Some(9.0), None, Some(5.0)), 0);
        assert_eq!(level("", Some(9.0), Some(1.0), Some(5.0)), 1);
        assert_eq!(level("H", Some(3.0), Some(1.0), Some(5.0)), 1);
        assert_eq!(level("HH", None, None, None), 2);
    }

    fn visit(id: i64, admit: f64, discharge: Option<f64>, icu: bool) -> Visit {
        Visit {
            id,
            database: 1,
            source: "s".into(),
            key: format!("enc:1:{id}"),
            pseudonym: String::new(),
            visit_key: id.to_string(),
            admit: Some(admit),
            discharge,
            unit: String::new(),
            icu,
        }
    }

    #[test]
    fn the_icu_column_shows_the_stays_in_intensive_care() {
        let h = 3600.0;
        // a visit of nine days, two of them in intensive care
        let visits = [visit(1, 0.0, Some(9.0 * 24.0 * h), true), visit(2, 1e6, Some(1e6 + 48.0 * h), true)];
        let stays = [
            Stay { visit: Some(1), start: Some(3.0 * h), end: Some(51.0 * h), icu: true },
            Stay { visit: Some(1), start: Some(51.0 * h), end: Some(200.0 * h), icu: false },
        ];
        let spans = icu_spans(&visits, &stays);
        // the first visit's ICU stay alone; the second visit (no stays known) whole
        assert_eq!(
            spans,
            vec![(3.0 * h, 51.0 * h, "enc:1:1".to_string()), (1e6, 1e6 + 48.0 * h, "enc:1:2".to_string())]
        );
        // a visit not in intensive care, with no stays: nothing
        assert!(icu_spans(&[visit(3, 0.0, Some(h), false)], &[]).is_empty());
        // a stay not ended: until its visit's end
        let open = [Stay { visit: Some(1), start: Some(5.0 * h), end: None, icu: true }];
        assert_eq!(icu_spans(&visits[..1], &open)[0].1, 9.0 * 24.0 * h);
        // a stay of no visit known: on its own
        let alone = [Stay { visit: None, start: Some(h), end: Some(2.0 * h), icu: true }];
        assert_eq!(icu_spans(&[], &alone), vec![(h, 2.0 * h, String::new())]);
    }
}
