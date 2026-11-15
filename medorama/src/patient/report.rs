//! The report: entries, their summaries, the search with highlights and fuzzy matches.

use super::*;

// ------------------------------------------------------------------ the report

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub key: String,
    pub time: Option<f64>,
    pub end: Option<f64>,
    pub category: String,
    pub title: String,
    pub summary: String,
    /// Abnormal values in it.
    pub abnormal: u32,
    pub encounter: Option<i64>,
    /// With a search: the text around the first match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Opens a viewer or shows a file.
    pub attachment: bool,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub entries: Vec<Entry>,
    /// With a search: entries per category, and the words highlighted.
    pub counts: Vec<(String, usize)>,
    pub terms: Vec<String>,
    /// Words a fuzzy match found for search words with no exact match.
    pub fuzzy: Vec<(String, String)>,
}

pub(super) fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

pub(super) fn value_text(it: &It) -> String {
    let first: String = it.text.lines().next().unwrap_or("").chars().take(120).collect();
    match it.num {
        Some(v) if first.is_empty() => format!("{} {}", fmt_num(v), it.unit).trim().to_string(),
        Some(v) => format!("{} {} · {first}", fmt_num(v), it.unit).replace("  ", " "),
        None => first,
    }
}

/// [`value_text`] as a session may read it: without the text when it is free text and the
/// session may not read free text (Privacy Mode withholding the text of notes and reports).
pub(super) fn shown_value(it: &It, read_notes: bool) -> String {
    if read_notes || !is_free_text(it) {
        return value_text(it);
    }
    match it.num {
        Some(v) => format!("{} {}", fmt_num(v), it.unit).trim().to_string(),
        None => String::new(),
    }
}

/// H, L or critical after a value outside its range.
pub(super) fn flag_mark(it: &It) -> &'static str {
    match it.level {
        2 => " critical",
        1 if it.num.is_some() && it.flag_low => " L",
        1 if it.num.is_some() => " H",
        1 => " abnormal",
        _ => "",
    }
}

/// Build the report's entries from items and recordings; `keep`: only those items.
pub(super) fn entries(
    d: &Data,
    items: &[It],
    read_notes: bool,
    privacy: bool,
    keep: Option<&HashSet<i64>>,
    recs: Option<&HashSet<i64>>,
) -> anyhow::Result<Vec<Entry>> {
    let mut groups: BTreeMap<String, Vec<&It>> = BTreeMap::new();
    for it in items {
        if keep.is_some_and(|k| !k.contains(&it.id)) {
            continue;
        }
        groups.entry(entry_key(it)).or_default().push(it);
    }
    let mut out = vec![];
    for (key, its) in groups {
        let first = its[0];
        let abnormal = its.iter().filter(|i| i.level > 0).count() as u32;
        let (title, summary) = if key.starts_with("labs:") {
            let names: Vec<&str> = its.iter().map(|i| i.name.as_str()).collect();
            let title =
                if names.len() == 1 { names[0].to_string() } else { format!("Laboratory · {} tests", names.len()) };
            let mut shown: Vec<&&It> = its.iter().filter(|i| i.level > 0).collect();
            if shown.is_empty() {
                shown = its.iter().collect();
            }
            let summary = shown
                .iter()
                .take(4)
                .map(|i| format!("{} {}{}", i.name, shown_value(i, read_notes), flag_mark(i)))
                .collect::<Vec<_>>()
                .join(" · ");
            (title, summary)
        } else if key.starts_with("vit:") {
            let mut by: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
            for i in &its {
                if let Some(v) = i.num {
                    let e = by.entry(i.name.as_str()).or_insert((v, v));
                    e.0 = e.0.min(v);
                    e.1 = e.1.max(v);
                }
            }
            let summary = by
                .iter()
                .take(4)
                .map(|(n, (a, b))| {
                    if a == b {
                        format!("{n} {}", fmt_num(*a))
                    } else {
                        format!("{n} {}–{}", fmt_num(*a), fmt_num(*b))
                    }
                })
                .collect::<Vec<_>>()
                .join(" · ");
            (format!("Vital signs · {} readings", its.len()), summary)
        } else if first.category == "note" {
            // staff names are withheld in Privacy Mode
            let author =
                if privacy && !first.author.is_empty() { crate::gate::STAFF_NAME } else { first.author.as_str() };
            let who = [author, first.service.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" · ");
            (first.name.clone(), who)
        } else {
            let title = if first.code.is_empty() || first.category != "dx" {
                first.name.clone()
            } else {
                format!("{} · {}", first.name, first.code)
            };
            (title, shown_value(first, read_notes))
        };
        let times: Vec<f64> = its.iter().filter_map(|i| i.time).collect();
        out.push(Entry {
            key,
            time: times.iter().copied().fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.min(t)))),
            end: its.iter().filter_map(|i| i.end).fold(None, |a: Option<f64>, t| Some(a.map_or(t, |a| a.max(t)))),
            category: first.category.clone(),
            title,
            summary,
            abnormal,
            encounter: first.encounter,
            snippet: None,
            attachment: its.iter().any(|i| i.file.is_some()),
        });
    }
    for r in &d.recordings {
        if d.attached.contains(&r.id) || recs.is_some_and(|x| !x.contains(&r.id)) {
            continue;
        }
        let (a, b) = (r.t_start, r.t_end);
        let dur = match (a, b) {
            (Some(a), Some(b)) => duration(b - a),
            _ => String::new(),
        };
        out.push(Entry {
            key: format!("rec:{}", r.id),
            time: a,
            end: b,
            category: r.category.clone(),
            title: recording_title(&r.category).into(),
            summary: [r.label.clone(), dur, r.format.clone()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            abnormal: 0,
            encounter: visit_of_recording(d, r),
            snippet: None,
            attachment: true,
        });
    }
    out.sort_by(|a, b| match (a.time, b.time) {
        (Some(x), Some(y)) => y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal).then(a.key.cmp(&b.key)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => a.key.cmp(&b.key),
    });
    Ok(out)
}

/// The words of a search: quoted phrases kept together; `name > 4` comparisons kept apart.
pub(super) fn search_words(q: &str) -> (Vec<String>, Vec<(String, String, f64)>) {
    let terms: Vec<Term> = parse(q);
    let mut words: Vec<String> = terms
        .iter()
        .map(|t| match &t.field {
            Some(f) => format!("{f}:{}", t.value),
            None => t.value.clone(),
        })
        .collect();
    let mut cmp = vec![];
    let mut i = 0;
    while i + 2 < words.len() {
        let op = words[i + 1].as_str();
        if ["<", ">", "<=", ">=", "="].contains(&op) {
            if let Ok(v) = words[i + 2].parse::<f64>() {
                cmp.push((words[i].clone(), op.to_string(), v));
                words.drain(i..i + 3);
                continue;
            }
        }
        i += 1;
    }
    // `lactate>4` written without spaces
    words.retain(|w| {
        for op in [">=", "<=", ">", "<", "="] {
            if let Some((n, v)) = w.split_once(op) {
                if let (false, Ok(v)) = (n.is_empty(), v.parse::<f64>()) {
                    cmp.push((n.to_string(), op.to_string(), v));
                    return false;
                }
            }
        }
        true
    });
    (words, cmp)
}

/// Edit distance, for fuzzy matching.
pub(super) fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The text around the first place a word occurs (case ignored), up to 170 characters.
pub(super) fn snippet(text: &str, words: &[String]) -> Option<String> {
    let lower = text.to_lowercase();
    let at = words.iter().filter_map(|w| lower.find(w.as_str())).min()?;
    // character positions, so that multi-byte text is cut at a character boundary
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let ci = chars.iter().position(|(b, _)| *b >= at).unwrap_or(0);
    let mut from = ci.saturating_sub(60);
    if from > 0 {
        while from < ci && !chars[from - 1].1.is_whitespace() {
            from += 1;
        }
    }
    let to = (ci + 110).min(chars.len());
    let mut s: String = chars[from..to].iter().map(|(_, c)| *c).collect();
    s = s.replace('\n', " ");
    if from > 0 {
        s = format!("…{}", s.trim_start());
    }
    if to < chars.len() {
        s = format!("{}…", s.trim_end());
    }
    Some(s)
}

/// A length of time in words: minutes, hours and minutes, days and hours; rounded once, then
/// split (never "1 h 60 min" or "1 d 24 h").
pub(super) fn duration(seconds: f64) -> String {
    let min = (seconds / 60.0).round().max(0.0) as i64;
    if min >= 24 * 60 {
        let h = (seconds / 3600.0).round() as i64;
        format!("{} d {} h", h / 24, h % 24)
    } else if min >= 60 {
        format!("{} h {} min", min / 60, min % 60)
    } else {
        format!("{min} min")
    }
}

pub fn report(c: &Connection, cfg: &Config, user: &User, id: i64, q: &str) -> anyhow::Result<Report> {
    patient_exists(c, user, id)?;
    // notes' text: with the PHI permission, or in a de-identified session that lets notes through
    let read_notes = user.reads_notes();
    let mut d = load(cfg, c, user, id)?;
    let items = std::mem::take(&mut d.items);
    // free text only for who may read it: else entries are found by name only
    let visible = |it: &It| read_notes || !is_free_text(it);
    if q.trim().is_empty() {
        let mut e = entries(&d, &items, read_notes, user.privacy, None, None)?;
        if !read_notes {
            for x in e.iter_mut().filter(|x| x.category == "note") {
                x.summary.clear();
            }
        }
        return Ok(Report { entries: e, counts: vec![], terms: vec![], fuzzy: vec![] });
    }
    let (words, cmps) = search_words(q);
    let shown_ids: HashSet<i64> = items.iter().filter(|i| visible(i)).map(|i| i.id).collect();
    // in Privacy Mode, an entry is found by its text as the session is shown it (gate.rs): not by
    // the patient's names, identifiers or staff it does not show
    let gate = if user.privacy && !user.phi { Some(crate::search::build_gate(c, cfg)?.for_patient(id)) } else { None };
    let identity = gate.as_ref().map(|_| crate::gate::Restricted::identity_words(c, cfg, id));
    // (name, text) of each entry as shown, and all its fields as one, lower case
    let shown: HashMap<i64, (String, String, String)> = match (&gate, &identity) {
        (Some(g), Some((names, birth))) => items
            .iter()
            .map(|it| {
                let name = g.note(&it.name, names, birth);
                let text = if visible(it) { g.note(&it.text, names, birth) } else { String::new() };
                let all =
                    [name.clone(), g.text(&it.code), text.clone(), g.text(&it.unit), g.text(&it.service)].join(" ");
                (it.id, (name, text, all.to_lowercase()))
            })
            .collect(),
        _ => HashMap::new(),
    };
    // what a search reads of an entry: as shown in Privacy Mode, else as it is
    let name_of = |it: &It| shown.get(&it.id).map_or(it.name.clone(), |s| s.0.clone());
    let text_of = |it: &It| shown.get(&it.id).map_or(it.text.clone(), |s| s.1.clone());
    let mut keep: HashSet<i64> = HashSet::new();
    let mut recs: HashSet<i64> = HashSet::new();
    let mut highlight: Vec<String> = vec![];
    let mut fuzzy = vec![];
    let patient_recs = &d.recordings;
    for w in &words {
        let w = w.to_lowercase();
        if let Some(cat) = categories::named(&w) {
            keep.extend(items.iter().filter(|i| i.category == cat).map(|i| i.id));
            recs.extend(patient_recs.iter().filter(|r| r.category == cat).map(|r| r.id));
            continue;
        }
        let mut hits: HashSet<i64> = HashSet::new();
        for it in &items {
            if [&it.name, &it.code, &it.text, &it.unit].iter().any(|f| f.to_lowercase().contains(&w)) {
                hits.insert(it.id);
            }
        }
        hits.retain(|h| shown_ids.contains(h));
        if gate.is_some() {
            hits.retain(|h| shown.get(h).is_some_and(|t| t.2.contains(&w)));
        }
        // recordings by their label and format (not by file name: it may identify)
        for r in patient_recs.iter() {
            if [&r.label, &r.format].iter().any(|f| f.to_lowercase().contains(&w)) {
                recs.insert(r.id);
            }
        }
        if hits.is_empty() && w.chars().count() >= 5 {
            // fuzzy: a word of this patient's text, as the session is shown it, within one or
            // two edits (in Privacy Mode, never a word the gate replaced: it would go back)
            let max = if w.chars().count() >= 8 { 2 } else { 1 };
            let mut vocab: HashSet<String> = HashSet::new();
            for it in items.iter().filter(|i| visible(i)) {
                for t in [name_of(it), text_of(it)] {
                    for x in t.split(|ch: char| !ch.is_alphanumeric()).filter(|x| x.chars().count() >= 4) {
                        vocab.insert(x.to_lowercase());
                    }
                }
            }
            if let Some(best) = vocab.iter().filter(|v| distance(v, &w) <= max).min_by_key(|v| distance(v, &w)) {
                fuzzy.push((w.clone(), best.clone()));
                for it in items.iter().filter(|i| visible(i)) {
                    if name_of(it).to_lowercase().contains(best.as_str())
                        || text_of(it).to_lowercase().contains(best.as_str())
                    {
                        hits.insert(it.id);
                    }
                }
                highlight.push(best.clone());
            }
        }
        highlight.push(w.clone());
        keep.extend(hits);
    }
    for (name, op, v) in &cmps {
        let n = name.to_lowercase();
        for it in &items {
            let Some(x) = it.num else { continue };
            let ok = match op.as_str() {
                ">" => x > *v,
                "<" => x < *v,
                ">=" => x >= *v,
                "<=" => x <= *v,
                _ => (x - v).abs() < 1e-9,
            };
            if ok && it.name.to_lowercase().contains(&n) {
                keep.insert(it.id);
            }
        }
        highlight.push(n);
    }
    let matched: Vec<It> = items.into_iter().filter(|i| keep.contains(&i.id)).collect();
    let mut e = entries(&d, &matched, read_notes, user.privacy, None, Some(&recs))?;
    for x in e.iter_mut() {
        if x.key.starts_with("live:") {
            if let Some(it) = matched.iter().find(|i| entry_key(i) == x.key) {
                if (it.text.chars().count() > 60 || it.category == "note") && visible(it) {
                    // in Privacy Mode, around the match in the text as shown
                    x.snippet = snippet(&text_of(it), &highlight);
                }
            }
        }
        if !read_notes && x.category == "note" {
            x.summary.clear();
        }
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for x in &e {
        *counts.entry(x.category.clone()).or_default() += 1;
    }
    let order: Vec<&str> = categories::journey_keys().collect();
    let mut counts: Vec<(String, usize)> = counts.into_iter().collect();
    counts.sort_by_key(|(k, _)| order.iter().position(|o| o == k).unwrap_or(99));
    highlight.sort();
    highlight.dedup();
    Ok(Report { entries: e, counts, terms: highlight, fuzzy })
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations_are_rounded_once() {
        assert_eq!(super::duration(3599.0), "1 h 0 min");
        assert_eq!(super::duration(2.0 * 86400.0 - 60.0), "2 d 0 h");
        assert_eq!(super::duration(90.0 * 60.0), "1 h 30 min");
        assert_eq!(super::duration(125.0), "2 min");
    }
}
