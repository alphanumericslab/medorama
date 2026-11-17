//! A criterion in a connected database (docs/design/databases-only.md, "Searching").

use super::*;

/// The patients a connected database's data say meet a criterion.  Its filters are run there;
/// values, units, `latest` and dates are tested here.  Sex, age and "data in that database" are
/// the registry's (evaluate.rs): they are answered from the persons Medorama keeps.
pub(super) fn meets_source(cx: &Cx, k: &Criterion, s: &crate::sources::Source) -> anyhow::Result<HashMap<i64, String>> {
    use crate::sources::Find;
    let (c, user, timezone) = (cx.c, cx.user, cx.timezone);
    let (czone, zone) = (tz(timezone), s.zone(timezone));
    // the bounds on real times (widened for dates as shown, which each patient's own offset
    // moves: tested exactly once the persons are linked, below)
    let dates = Dates::of(c, k, czone)?;
    let (from, to) = dates.wide();
    let in_range =
        |t: Option<f64>| from.is_none_or(|f| t.is_some_and(|t| t >= f)) && to.is_none_or(|e| t.is_some_and(|t| t < e));
    // each found row's times, for dates as shown: (start, end), tested for the person's patient
    let mut spans: Vec<Option<(f64, f64)>> = vec![];
    let on = |t: Option<f64>| {
        t.map(|t| format!(" on {}", crate::util::fmt_local(t, czone).get(..10).unwrap_or(""))).unwrap_or_default()
    };
    let exact = k.database == Some(s.id);
    let mut found: Vec<(String, String)> = vec![];
    // the database's data are shown de-identified (Privacy Mode, or granted without `phi`):
    // what a criterion finds in text is tested in the text as shown, and its reasons are as shown
    let deid = user.privacy || !crate::grants::can(c, user, &format!("db:{}", s.id), "phi")?;
    // (found row → its name and texts, for a text criterion)
    let mut texts: HashMap<usize, (String, String, String)> = HashMap::new();
    match k.kind.as_str() {
        "has" => {
            let cat = categories::named(&k.category).ok_or_else(|| anyhow::anyhow!("no category {:?}", k.category))?;
            let why = format!("has {}", categories::label(cat));
            // visits, entries, and the recordings the database lists of that category
            let keys = s.persons_with(cat, zone)?;
            found.extend(keys.into_iter().map(|p| (p, why.clone())));
        }
        "encounter" => {
            let t = k.text.trim().to_lowercase();
            let visits = s.all_visits(zone)?;
            // the visits with an intensive-care stay (a bed stay in an ICU), as `has icu` counts them
            let with_icu_stay: HashSet<(String, String)> = if k.icu.is_some() {
                let mut persons: Vec<String> = visits.iter().map(|v| v.person.clone()).collect();
                persons.sort();
                persons.dedup();
                s.stays(&persons, zone)?
                    .into_iter()
                    .filter(|st| st.icu || categories::is_icu_unit(&st.unit))
                    .map(|st| (st.person, st.visit))
                    .collect()
            } else {
                HashSet::new()
            };
            for v in visits {
                // a visit without an end goes on: until now
                let (a, b) = (
                    v.start.unwrap_or(f64::NEG_INFINITY),
                    v.end.unwrap_or_else(|| v.start.map_or(f64::INFINITY, |a| now().max(a))),
                );
                if from.is_some_and(|f| b < f) || to.is_some_and(|e| a >= e) {
                    continue;
                }
                if !t.is_empty() && !v.unit.to_lowercase().contains(&t) {
                    continue;
                }
                let icu = v.icu
                    || categories::is_icu_unit(&v.unit)
                    || with_icu_stay.contains(&(v.person.clone(), v.key.clone()));
                if k.icu.is_some_and(|want| want != icu) {
                    continue;
                }
                let unit = if v.unit.is_empty() { String::new() } else { format!(" ({})", v.unit) };
                spans.resize(found.len(), None);
                spans.push(Some((a, b)));
                found.push((v.person, format!("encounter in {}{unit}", s.name)));
            }
        }
        "diagnosis" => {
            let f =
                Find { categories: vec!["dx".into()], code_or_name: Some(k.text.trim().into()), ..Default::default() };
            for e in s.find(&f, zone)? {
                found.push((e.person, format!("{} {}", e.code, e.name).trim().to_string()));
            }
        }
        "measurement" => {
            let f = Find {
                name: exact.then(|| k.name.trim().to_string()),
                name_contains: (!exact).then(|| k.name.trim().to_string()),
                numeric: true,
                ..Default::default()
            };
            let mut rows: Vec<crate::sources::Entry> =
                s.find(&f, zone)?.into_iter().filter(|e| in_range(e.time)).collect();
            rows.extend(result_entries(cx, s, k, exact)?.into_iter().filter(|e| in_range(e.time)));
            if dates.per_patient() {
                // each patient's own bounds: their persons linked first
                let keys: Vec<String> = rows.iter().map(|e| e.person.clone()).collect();
                let linked = crate::persons::patients(cx.cfg, c, s, &keys, Some(user))?;
                rows.retain(|e| linked.get(&e.person).is_some_and(|p| dates.holds(*p, e.time)));
            }
            let units: Vec<String> = rows.iter().map(|e| e.unit.clone()).collect();
            one_unit(k, &units)?;
            let mut skipped = Skipped::default();
            for e in rows {
                let Some(v) = e.value_num else { continue };
                if !skipped.compares(k, &e.unit) {
                    continue;
                }
                if has_value_test(k) && !test_value(k, v, &e.unit)? {
                    continue;
                }
                found
                    .push((e.person, format!("{} {} {}{}", e.name, fmt_num(v), e.unit, on(e.time)).replace("  ", " ")));
            }
            skipped.done(k, cx)?;
        }
        "medication" => {
            let f = Find {
                categories: vec!["med".into(), "blood".into()],
                name_contains: Some(k.text.trim().into()),
                ..Default::default()
            };
            for e in s.find(&f, zone)? {
                let end = e.end.or(e.time);
                if from.is_some_and(|f| end.is_none_or(|t| t < f)) || to.is_some_and(|t| e.time.is_none_or(|x| x >= t))
                {
                    continue;
                }
                spans.resize(found.len(), None);
                spans.push(Some((e.time.unwrap_or(f64::NEG_INFINITY), end.unwrap_or(f64::INFINITY))));
                found.push((e.person, e.name));
            }
        }
        "text" => {
            let t = k.text.trim();
            if t.chars().count() < 3 {
                anyhow::bail!("text: at least 3 characters");
            }
            let f = Find { text: Some(t.into()), notes: user.reads_notes(), ..Default::default() };
            for e in s.find(&f, zone)? {
                if deid {
                    texts.insert(found.len(), (e.name.clone(), e.text.clone(), e.value_text.clone()));
                }
                found.push((
                    e.person,
                    format!("“{t}” in {} ({})", categories::label(&e.category).to_lowercase(), e.name),
                ));
            }
        }
        _ => {}
    }
    if found.is_empty() {
        return Ok(HashMap::new());
    }
    let keys: Vec<String> = found.iter().map(|f| f.0.clone()).collect();
    let patients = crate::persons::patients(cx.cfg, c, s, &keys, Some(user))?;
    let mut out = HashMap::new();
    spans.resize(found.len(), None);
    let gate =
        if deid && (!texts.is_empty() || !user.privacy) { Some(crate::search::build_gate(c, cx.cfg)?) } else { None };
    let wanted = k.text.trim().to_lowercase();
    for (n, ((key, why), span)) in found.into_iter().zip(spans).enumerate() {
        if let Some(p) = patients.get(&key) {
            if let (true, Some((a, b))) = (dates.per_patient(), span) {
                if !(dates.after(*p, b) && dates.before(*p, a)) {
                    continue;
                }
            }
            let mut why = why;
            if let Some(g) = &gate {
                let g = g.for_patient(*p);
                let (names, birth) = crate::gate::Restricted::identity_words(c, cx.cfg, *p);
                // found in the text as shown: not by a name, an identifier or a date it withholds
                if let Some((name, text, value)) = texts.get(&n) {
                    let shown = shown_text(&g, name, text, value, &names, &birth, user);
                    if !shown.to_lowercase().contains(&wanted) {
                        continue;
                    }
                }
                if !user.privacy {
                    // (in Privacy Mode the answer's gate does this)
                    let w = g.note(&why, &names, &birth);
                    why = g.dict.scrub_any_names(&w).unwrap_or(w);
                }
            }
            out.entry(*p).or_insert(why);
        }
    }
    Ok(out)
}

/// An entry's name and texts as a de-identified session is shown them (its free text withheld
/// when Privacy Mode withholds it), as one text.
fn shown_text(
    g: &crate::gate::Restricted,
    name: &str,
    text: &str,
    value: &str,
    names: &[String],
    birth: &str,
    user: &User,
) -> String {
    let free = |t: &str| {
        if user.privacy && !user.reads_notes() && !t.trim().is_empty() {
            crate::gate::WITHHELD.to_string()
        } else {
            g.entry_text(t, names, birth)
        }
    };
    let value = if crate::cohorts::is_long_text(value) { free(value) } else { g.note(value, names, birth) };
    [g.note(name, names, birth), value, free(text)].join(" ")
}

/// A measurement's values in a connected database, each (patient, name, value, unit, time),
/// of the name written in full, in the criterion's dates: their persons linked (for `latest`
/// across databases: evaluate.rs).
pub(super) fn source_measurement_rows(
    cx: &Cx,
    k: &Criterion,
    s: &crate::sources::Source,
) -> anyhow::Result<Vec<(i64, String, f64, String, Option<f64>)>> {
    use crate::sources::Find;
    let (c, user, timezone) = (cx.c, cx.user, cx.timezone);
    let (czone, zone) = (tz(timezone), s.zone(timezone));
    let dates = Dates::of(c, k, czone)?;
    let (from, to) = dates.wide();
    let f = Find { name: Some(k.name.trim().to_string()), numeric: true, ..Default::default() };
    let mut found = s.find(&f, zone)?;
    found.extend(result_entries(cx, s, k, true)?);
    let rows: Vec<crate::sources::Entry> = found
        .into_iter()
        .filter(|e| {
            from.is_none_or(|f| e.time.is_some_and(|t| t >= f)) && to.is_none_or(|x| e.time.is_some_and(|t| t < x))
        })
        .filter(|e| e.value_num.is_some())
        .collect();
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let keys: Vec<String> = rows.iter().map(|e| e.person.clone()).collect();
    let linked = crate::persons::patients(cx.cfg, c, s, &keys, Some(user))?;
    Ok(rows
        .into_iter()
        .filter_map(|e| {
            let p = *linked.get(&e.person)?;
            dates.holds(p, e.time).then(|| (p, e.name, e.value_num.unwrap_or_default(), e.unit, e.time))
        })
        .collect())
}

/// The results of actions run on a database's recordings, named like a criterion's measurement,
/// as its entries (derived.rs).
fn result_entries(
    cx: &Cx,
    s: &crate::sources::Source,
    k: &Criterion,
    exact: bool,
) -> anyhow::Result<Vec<crate::sources::Entry>> {
    Ok(crate::derived::results_like(cx.cfg, cx.c, s.id, &k.name, exact)?
        .into_iter()
        .map(|(person, name, v, at)| crate::sources::Entry {
            person,
            name,
            category: "result".into(),
            value_num: Some(v),
            time: at,
            ..Default::default()
        })
        .collect())
}
