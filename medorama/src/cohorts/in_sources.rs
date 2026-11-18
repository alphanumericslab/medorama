//! A criterion in a connected database (docs/design/databases-only.md, "Searching").

use super::*;
use crate::sources::reads::{Answer, Read};
use crate::sources::{Bounds, Find, Source};

/// What a criterion asks a database for ([`Read`]), the same in every search of it, so that
/// its answer is read once while the data stay the same (sources/reads.rs); none for a
/// criterion a database does not answer (visits, the registry's).  `pushed`: the units whose
/// values the database tests (their threshold is in the query).
pub(super) fn read_of(cx: &Cx, k: &Criterion, s: &Source) -> anyhow::Result<Option<(Read, Vec<String>)>> {
    let exact = k.database == Some(s.id);
    Ok(Some(match k.kind.as_str() {
        "has" => {
            let cat = categories::named(&k.category).ok_or_else(|| anyhow::anyhow!("no category {:?}", k.category))?;
            (Read::Persons(cat.to_string()), vec![])
        }
        "diagnosis" => (
            Read::Entries(Find {
                categories: vec!["dx".into()],
                code_or_name: Some(k.text.trim().into()),
                ..Default::default()
            }),
            vec![],
        ),
        "medication" => (
            Read::Entries(Find {
                categories: vec!["med".into(), "blood".into()],
                name_contains: Some(k.text.trim().into()),
                ..Default::default()
            }),
            vec![],
        ),
        "text" => {
            let t = k.text.trim();
            if t.chars().count() < 3 {
                anyhow::bail!("text: at least 3 characters");
            }
            (Read::Entries(Find { text: Some(t.into()), notes: cx.user.reads_notes(), ..Default::default() }), vec![])
        }
        // `latest` reads every value (each person's latest is tested), and by its name in full
        "measurement" if k.latest => {
            let (f, _) = measurement_find(cx, k, s, true, false)?;
            (Read::Values(f), vec![])
        }
        "measurement" => {
            let (f, pushed) = measurement_find(cx, k, s, exact, true)?;
            (Read::Values(f), pushed)
        }
        _ => return Ok(None),
    }))
}

/// A measurement's filters: its name, as the database's variables spell it, and, `values`
/// asked, the values its threshold leaves, by unit (docs/performance.md, "What a criterion
/// reads"), with the units so pushed.  From the variables only when they were read at the
/// data's version (else the names in any case, every value: as the criterion is written).
///
/// The bounds hold every value the criterion's own test may pass ([`test_value`], run on each
/// row read): a value without a unit is the criterion's unit; a value in a unit the
/// criterion's does not compare with is read whole, to be counted; a value written without a
/// unit, where the data hold several, is not pushed (which unit is meant is asked after, by
/// the units the rows read have).
fn measurement_find(
    cx: &Cx,
    k: &Criterion,
    s: &Source,
    exact: bool,
    values: bool,
) -> anyhow::Result<(Find, Vec<String>)> {
    let written = Find {
        name: exact.then(|| k.name.trim().to_string()),
        name_contains: (!exact).then(|| k.name.trim().to_string()),
        numeric: true,
        ..Default::default()
    };
    if !crate::variables::read_at(cx.c, s.id, &cx.version(s)?)? {
        return Ok((written, vec![]));
    }
    let w = k.name.trim().to_lowercase();
    let held: Vec<crate::variables::Variable> = crate::variables::of(cx.cfg, cx.c, s.id)?
        .into_iter()
        .filter(|v| v.numeric && v.category != "note")
        .filter(|v| if exact { v.name.to_lowercase() == w } else { v.name.to_lowercase().contains(&w) })
        .collect();
    if held.is_empty() {
        return Ok((written, vec![]));
    }
    // (a `variables` view is written apart from the entries: its names in any case, and the
    // entries' other units read too)
    let apart = !s.views().variables.trim().is_empty();
    let mut names: Vec<String> = held.iter().map(|v| v.name.clone()).collect();
    names.sort();
    names.dedup();
    let mut f = Find { names, names_any_case: apart, numeric: true, ..Default::default() };
    let mut units: Vec<String> = held.iter().map(|v| v.unit.clone()).collect();
    units.sort();
    units.dedup();
    if !values || !has_value_test(k) || (k.unit.trim().is_empty() && units.len() > 1) {
        return Ok((f, vec![]));
    }
    let mut pushed = vec![];
    for u in &units {
        let unit = (apart || !k.unit.trim().is_empty() || units.len() > 1).then(|| u.clone());
        match bounds(k, u) {
            Some((lo, hi)) => {
                f.values.push(Bounds { unit, lo, hi });
                pushed.push(u.clone());
            }
            // not comparable: read whole, and counted as left out
            None => f.values.push(Bounds { unit, lo: None, hi: None }),
        }
    }
    f.other_units = apart;
    if f.values.iter().all(|b| b.lo.is_none() && b.hi.is_none()) {
        f.values.clear();
        f.other_units = false;
        pushed.clear();
    }
    Ok((f, pushed))
}

impl Cx<'_> {
    /// A database's answer to a read: found once per evaluation (the reads made before it, in
    /// parallel, included), kept while its data's version stays (sources/reads.rs).
    pub(super) fn read(&self, s: &Source, r: &Read) -> anyhow::Result<Answer> {
        let zone = s.zone(self.timezone);
        let key = s.read_key(r, zone);
        if let Some(a) = self.reads.borrow().get(&key) {
            return Ok(a.clone());
        }
        let a = s.read_at(r, zone, &self.version(s)?)?;
        self.reads.borrow_mut().insert(key, a.clone());
        Ok(a)
    }

    /// A database's data version, read once per evaluation.
    pub(super) fn version(&self, s: &Source) -> anyhow::Result<String> {
        if let Some(v) = self.versions.borrow().get(&s.id) {
            return Ok(v.clone());
        }
        let v = s.version()?;
        self.versions.borrow_mut().insert(s.id, v.clone());
        Ok(v)
    }
}

/// The patients a connected database's data say meet a criterion.  Its filters are run there;
/// values, units, `latest` and dates are tested here.  Sex, age and "data in that database" are
/// the registry's (evaluate.rs): they are answered from the persons Medorama keeps.
pub(super) fn meets_source(cx: &Cx, k: &Criterion, s: &Source) -> anyhow::Result<HashMap<i64, String>> {
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
            if let Some((r, _)) = read_of(cx, k, s)? {
                if let Answer::Persons(keys) = cx.read(s, &r)? {
                    found.extend(keys.iter().map(|p| (p.clone(), why.clone())));
                }
            }
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
            for e in entries_of(cx, k, s)?.iter() {
                found.push((e.person.clone(), format!("{} {}", e.code, e.name).trim().to_string()));
            }
        }
        "measurement" => {
            let Some((r, pushed)) = read_of(cx, k, s)? else { return Ok(HashMap::new()) };
            let Answer::Values(rows) = cx.read(s, &r)? else { return Ok(HashMap::new()) };
            // each row tested (only those the threshold may pass were read: the database tested
            // the others, in their units); the units and the values left out are counted over
            // the dates as read, before each patient's own dates (Privacy Mode) are tested
            let per_patient = dates.per_patient();
            // (the units whose values the database tested are the data's, read or not)
            let mut units: HashSet<String> = pushed.iter().map(|u| u.trim().to_string()).collect();
            let mut skipped = Skipped::default();
            if !pushed.is_empty() {
                skipped.pushed();
            }
            // (person, time, why); one per person when the dates are everyone's
            let mut held: Vec<(String, Option<f64>, String)> = vec![];
            let mut seen: HashSet<String> = HashSet::new();
            {
                let mut test = |person: &str, time: Option<f64>, name: &str, value: Option<f64>, unit: &str| {
                    if !in_range(time) {
                        return Ok(());
                    }
                    units.insert(unit.trim().to_string());
                    let Some(v) = value else { return Ok(()) };
                    if !skipped.compares(k, unit) || (has_value_test(k) && !test_value(k, v, unit)?) {
                        return Ok(());
                    }
                    if !per_patient && !seen.insert(person.to_string()) {
                        return Ok(());
                    }
                    let why = format!("{name} {} {unit}{}", fmt_num(v), on(time)).replace("  ", " ");
                    held.push((person.to_string(), time, why));
                    anyhow::Ok(())
                };
                for e in rows.iter() {
                    test(&e.person, e.time, &e.name, e.value, &e.unit)?;
                }
                for e in result_entries(cx, s, k, exact)? {
                    test(&e.person, e.time, &e.name, e.value_num, &e.unit)?;
                }
            }
            one_unit(k, &units.into_iter().collect::<Vec<_>>())?;
            if per_patient {
                // each patient's own bounds: their persons linked first
                let keys: Vec<String> = held.iter().map(|x| x.0.clone()).collect();
                let linked = crate::persons::patients(cx.cfg, c, s, &keys, Some(user))?;
                held.retain(|(p, t, _)| linked.get(p).is_some_and(|q| dates.holds(*q, *t)));
            }
            found.extend(held.into_iter().map(|(p, _, why)| (p, why)));
            skipped.done(k, cx)?;
        }
        "medication" => {
            for e in entries_of(cx, k, s)?.iter() {
                let end = e.end.or(e.time);
                if from.is_some_and(|f| end.is_none_or(|t| t < f)) || to.is_some_and(|t| e.time.is_none_or(|x| x >= t))
                {
                    continue;
                }
                spans.resize(found.len(), None);
                spans.push(Some((e.time.unwrap_or(f64::NEG_INFINITY), end.unwrap_or(f64::INFINITY))));
                found.push((e.person.clone(), e.name.clone()));
            }
        }
        "text" => {
            let t = k.text.trim();
            for e in entries_of(cx, k, s)?.iter() {
                if deid {
                    texts.insert(found.len(), (e.name.clone(), e.text.clone(), e.value_text.clone()));
                }
                found.push((
                    e.person.clone(),
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

/// The entries a criterion's filter finds (a diagnosis, a medication, a text).
fn entries_of(cx: &Cx, k: &Criterion, s: &Source) -> anyhow::Result<std::sync::Arc<Vec<crate::sources::Entry>>> {
    match read_of(cx, k, s)? {
        Some((r, _)) => match cx.read(s, &r)? {
            Answer::Entries(v) => Ok(v),
            _ => Ok(Default::default()),
        },
        None => Ok(Default::default()),
    }
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
    let (c, user, timezone) = (cx.c, cx.user, cx.timezone);
    let czone = tz(timezone);
    let dates = Dates::of(c, k, czone)?;
    let (from, to) = dates.wide();
    let Some((r, _)) = read_of(cx, k, s)? else { return Ok(vec![]) };
    let Answer::Values(read) = cx.read(s, &r)? else { return Ok(vec![]) };
    // when the dates are everyone's, only each person's latest value is kept (the first read,
    // of the same time), not the millions read
    let per_patient = dates.per_patient();
    let mut rows: Vec<crate::sources::Entry> = vec![];
    let mut latest: HashMap<String, (usize, crate::sources::Entry)> = HashMap::new();
    let mut n = 0usize;
    {
        let mut take = |e: crate::sources::Entry| -> anyhow::Result<()> {
            let t = e.time;
            if e.value_num.is_none()
                || !from.is_none_or(|f| t.is_some_and(|t| t >= f))
                || !to.is_none_or(|x| t.is_some_and(|t| t < x))
            {
                return Ok(());
            }
            n += 1;
            if per_patient {
                rows.push(e);
            } else if latest
                .get(&e.person)
                .is_none_or(|(_, x)| t.unwrap_or(f64::NEG_INFINITY) > x.time.unwrap_or(f64::NEG_INFINITY))
            {
                latest.insert(e.person.clone(), (n, e));
            }
            Ok(())
        };
        for v in read.iter() {
            take(crate::sources::Entry {
                person: v.person.clone(),
                time: v.time,
                name: v.name.to_string(),
                value_num: v.value,
                unit: v.unit.to_string(),
                ..Default::default()
            })?;
        }
        for e in result_entries(cx, s, k, true)? {
            take(e)?;
        }
    }
    if !per_patient {
        let mut kept: Vec<(usize, crate::sources::Entry)> = latest.into_values().collect();
        kept.sort_by_key(|x| x.0);
        rows = kept.into_iter().map(|x| x.1).collect();
    }
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
