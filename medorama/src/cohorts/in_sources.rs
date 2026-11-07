//! A criterion in a connected database (docs/design/sources.md, "Cohorts across sources").

use super::*;

/// The patients a connected database's data say meet a criterion (docs/design/sources.md,
/// "Cohorts across sources").  Its filters are run there; values, units, `latest` and dates are
/// tested here.  Sex, age and "data in that database" are the registry's: the catalog answers
/// them for every linked person.
pub(super) fn meets_source(
    c: &Connection,
    user: &User,
    timezone: &str,
    k: &Criterion,
    s: &crate::sources::Source,
    cache: &Cache,
) -> anyhow::Result<HashMap<i64, String>> {
    use crate::sources::Find;
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
    let exact = k.import == Some(s.import_id);
    let mut found: Vec<(String, String)> = vec![];
    match k.kind.as_str() {
        "has" => {
            let cat = categories::named(&k.category).ok_or_else(|| anyhow::anyhow!("no category {:?}", k.category))?;
            let why = format!("has {}", categories::label(cat));
            let keys = match cat {
                "icu" => s.visit_persons(true)?,
                "enc" => s.visit_persons(false)?,
                // vital signs and lab results are told apart by their names: read the rows
                "lab" | "vit" => {
                    let rows = s.find(&Find { categories: vec![cat.into()], ..Default::default() }, zone)?;
                    rows.into_iter().map(|e| e.person).collect()
                }
                _ => {
                    s.find_persons(&Find { categories: vec![cat.into()], notes: cat == "note", ..Default::default() })?
                }
            };
            found.extend(keys.into_iter().map(|p| (p, why.clone())));
        }
        "encounter" => {
            let t = k.text.trim().to_lowercase();
            for v in s.all_visits(zone)? {
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
                let icu = v.icu || categories::is_icu_unit(&v.unit);
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
            if dates.per_patient() {
                // each patient's own bounds: their persons linked first
                let keys: Vec<String> = rows.iter().map(|e| e.person.clone()).collect();
                let linked = crate::sources::link::patients(c, s, &keys, zone, Some(user))?;
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
            skipped.done(k, cache)?;
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
                found.push((
                    e.person,
                    format!("“{t}” in {} ({})", categories::label(&e.category).to_lowercase(), e.name),
                ));
            }
        }
        "column" if exact => anyhow::bail!("{} is a connected database: it has no columns", s.name),
        _ => {}
    }
    if found.is_empty() {
        return Ok(HashMap::new());
    }
    let keys: Vec<String> = found.iter().map(|f| f.0.clone()).collect();
    let patients = crate::sources::link::patients(c, s, &keys, zone, Some(user))?;
    let mut out = HashMap::new();
    spans.resize(found.len(), None);
    for ((key, why), span) in found.into_iter().zip(spans) {
        if let Some(p) = patients.get(&key) {
            if let (true, Some((a, b))) = (dates.per_patient(), span) {
                if !(dates.after(*p, b) && dates.before(*p, a)) {
                    continue;
                }
            }
            out.entry(*p).or_insert(why);
        }
    }
    Ok(out)
}

/// A measurement's values in a connected database, each (patient, name, value, unit, time),
/// of the name written in full, in the criterion's dates: their persons linked (for `latest`
/// across databases: evaluate.rs).
pub(super) fn source_measurement_rows(
    c: &Connection,
    user: &User,
    timezone: &str,
    k: &Criterion,
    s: &crate::sources::Source,
) -> anyhow::Result<Vec<(i64, String, f64, String, Option<f64>)>> {
    use crate::sources::Find;
    let (czone, zone) = (tz(timezone), s.zone(timezone));
    let dates = Dates::of(c, k, czone)?;
    let (from, to) = dates.wide();
    let f = Find { name: Some(k.name.trim().to_string()), numeric: true, ..Default::default() };
    let rows: Vec<crate::sources::Entry> = s
        .find(&f, zone)?
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
    let linked = crate::sources::link::patients(c, s, &keys, zone, Some(user))?;
    Ok(rows
        .into_iter()
        .filter_map(|e| {
            let p = *linked.get(&e.person)?;
            dates.holds(p, e.time).then(|| (p, e.name, e.value_num.unwrap_or_default(), e.unit, e.time))
        })
        .collect())
}
