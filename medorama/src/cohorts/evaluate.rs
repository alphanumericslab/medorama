//! Evaluating criteria: each criterion in the catalog and in every connected database that holds such data, then the groups; again when new patients were linked.

use super::*;

pub(super) fn like(s: &str) -> String {
    format!("%{}%", s.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

/// The patients meeting one criterion (before `not`), each with a reason: in the catalog, and
/// in each connected database that holds such data (all of them, or the one `import` names).
/// Each part is found once per evaluation (`cache`); a measurement's `latest` is each
/// patient's latest value across all of them.
pub(super) fn meets(
    c: &Connection,
    user: &User,
    timezone: &str,
    k: &Criterion,
    cache: &Cache,
) -> anyhow::Result<HashMap<i64, String>> {
    let key = serde_json::to_string(&Criterion { not: false, ..k.clone() })?;
    if k.kind == "measurement" && k.latest {
        if let Some(f) = cache.sources.borrow().get(&key) {
            return Ok(f.clone());
        }
        let f = latest_across(c, user, timezone, k, cache)?;
        cache.sources.borrow_mut().insert(key, f.clone());
        return Ok(f);
    }
    let cached = cache.catalog.borrow().get(&key).cloned();
    let mut out = match cached {
        Some(f) => f,
        None => {
            let f = meets_catalog(c, user, timezone, k, cache)?;
            cache.catalog.borrow_mut().insert(key.clone(), f.clone());
            f
        }
    };
    let cached = cache.sources.borrow().get(&key).cloned();
    let theirs = match cached {
        Some(f) => f,
        None => {
            let mut f: HashMap<i64, String> = HashMap::new();
            for s in crate::sources::list(c)? {
                if k.import.is_some_and(|i| i != s.import_id) {
                    continue;
                }
                for (p, why) in meets_source(c, user, timezone, k, &s, cache)? {
                    f.entry(p).or_insert(why);
                }
            }
            cache.sources.borrow_mut().insert(key, f.clone());
            f
        }
    };
    for (p, why) in theirs {
        out.entry(p).or_insert(why);
    }
    Ok(out)
}

/// `latest`, for a measurement: each patient's values in the catalog and in every connected
/// database that holds it (or the one `import` names), the latest of them tested.
fn latest_across(
    c: &Connection,
    user: &User,
    timezone: &str,
    k: &Criterion,
    cache: &Cache,
) -> anyhow::Result<HashMap<i64, String>> {
    let zone = tz(timezone);
    let mut rows = measurement_rows(c, k, &Dates::of(c, k, zone)?)?;
    for s in crate::sources::list(c)? {
        if k.import.is_some_and(|i| i != s.import_id) {
            continue;
        }
        rows.extend(source_measurement_rows(c, user, timezone, k, &s)?);
    }
    // the latest of each patient (the first given, of the same time)
    let mut last: HashMap<i64, usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        let newer =
            last.get(&r.0).is_none_or(|j| r.4.unwrap_or(f64::NEG_INFINITY) > rows[*j].4.unwrap_or(f64::NEG_INFINITY));
        if newer {
            last.insert(r.0, i);
        }
    }
    let units: Vec<String> = last.values().map(|i| rows[*i].3.clone()).collect();
    one_unit(k, &units)?;
    let mut out = HashMap::new();
    let mut skipped = Skipped::default();
    for i in last.into_values() {
        let (p, name, v, unit, t) = &rows[i];
        if !skipped.compares(k, unit) {
            continue;
        }
        if has_value_test(k) && !test_value(k, *v, unit)? {
            continue;
        }
        let when =
            t.map(|t| format!(" on {}", crate::util::fmt_local(t, zone).get(..10).unwrap_or(""))).unwrap_or_default();
        out.insert(*p, format!("{name} {} {unit}{when}", fmt_num(*v)).replace("  ", " "));
    }
    skipped.done(k, cache)?;
    Ok(out)
}

/// Every patient a cohort may hold: not those only an import still loading holds.
pub(super) fn everyone(c: &Connection) -> anyhow::Result<HashSet<i64>> {
    Ok(c.prepare(&format!("SELECT id FROM patients WHERE {}", patient::patient_shown("id")))?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

pub(super) fn push(why: &mut Vec<String>, more: impl IntoIterator<Item = String>) {
    for w in more {
        if why.len() < MAX_REASONS && !why.contains(&w) {
            why.push(w);
        }
    }
}

/// An evaluation's context: each criterion's patients, found once (connected databases are
/// asked once), and what it could not test.
#[derive(Default)]
pub(super) struct Cache {
    /// In the catalog: found again after connected databases linked new patients …
    catalog: std::cell::RefCell<HashMap<String, HashMap<i64, String>>>,
    /// … and in the connected databases.
    sources: std::cell::RefCell<HashMap<String, HashMap<i64, String>>>,
    /// Values left out (their unit does not compare), in words, each once.
    pub(super) warnings: std::cell::RefCell<Vec<String>>,
}

impl Cache {
    pub(super) fn warn(&self, w: Option<String>) {
        let mut ws = self.warnings.borrow_mut();
        if let Some(w) = w.filter(|w| !ws.contains(w)) {
            ws.push(w);
        }
    }

    /// The catalog's answers forgotten: patients were linked since (their data are the
    /// registry's: sex, age, …).
    fn forget_catalog(&self) {
        self.catalog.borrow_mut().clear();
    }
}

/// The patients an expression holds for, each with why.
pub(super) fn eval(
    c: &Connection,
    user: &User,
    timezone: &str,
    e: &Expr,
    all: &HashSet<i64>,
    cache: &Cache,
) -> anyhow::Result<HashMap<i64, Vec<String>>> {
    Ok(match e {
        Expr::Leaf(k) => {
            let found = meets(c, user, timezone, k, cache)?;
            if k.not {
                all.iter()
                    .filter(|p| !found.contains_key(p))
                    .map(|p| (*p, vec![format!("no {}", describe(k))]))
                    .collect()
            } else {
                found.into_iter().map(|(p, w)| (p, vec![w])).collect()
            }
        }
        Expr::Group(g) => match g.op.as_str() {
            "and" => {
                let mut keep: Option<HashMap<i64, Vec<String>>> = None;
                for x in &g.of {
                    let f = eval(c, user, timezone, x, all, cache)?;
                    keep = Some(match keep {
                        None => f,
                        Some(mut k) => {
                            k.retain(|p, _| f.contains_key(p));
                            for (p, why) in k.iter_mut() {
                                push(why, f[p].clone());
                            }
                            k
                        }
                    });
                }
                keep.unwrap_or_else(|| all.iter().map(|p| (*p, vec![])).collect())
            }
            "or" => {
                let mut out: HashMap<i64, Vec<String>> = HashMap::new();
                for x in &g.of {
                    for (p, why) in eval(c, user, timezone, x, all, cache)? {
                        out.entry(p).or_insert(why);
                    }
                }
                out
            }
            "not" => {
                let mut inner: HashMap<i64, Vec<String>> = HashMap::new();
                for x in &g.of {
                    inner.extend(eval(c, user, timezone, x, all, cache)?);
                }
                all.iter().filter(|p| !inner.contains_key(p)).map(|p| (*p, vec!["not the group".to_string()])).collect()
            }
            op => anyhow::bail!("unknown group {op:?} (and, or, not)"),
        },
    })
}

/// Who the criteria include, and who they exclude among those (with why each was excluded).
#[allow(clippy::type_complexity)]
pub fn evaluate_both(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
) -> anyhow::Result<(HashMap<i64, Vec<String>>, HashMap<i64, Vec<String>>)> {
    let (found, excluded, _) = evaluate_full(c, user, timezone, q)?;
    Ok((found, excluded))
}

/// [`evaluate_both`], with what the evaluation could not test (values in units that do not
/// compare with the criteria's), in words.  Audited: searches of identifiers (`phi_search`),
/// and every search in Privacy Mode (`privacy_search`).
#[allow(clippy::type_complexity)]
pub fn evaluate_full(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
) -> anyhow::Result<(HashMap<i64, Vec<String>>, HashMap<i64, Vec<String>>, Vec<String>)> {
    let cache = Cache::default();
    let (found, excluded) = evaluate_twice(c, user, timezone, q, &cache)?;
    let phi_criteria =
        q.leaves().iter().any(|k| k.kind == "age" || (k.kind == "text" && user.phi) || k.kind == "expression");
    if user.phi && phi_criteria {
        audit(c, Some(user), "phi_search", "cohorts", &serde_json::to_string(q)?);
    } else if user.privacy && !user.phi && !q.is_empty() {
        audit(c, Some(user), "privacy_search", "cohorts", &serde_json::to_string(q)?);
    }
    Ok((found, excluded, cache.warnings.into_inner()))
}

/// `evaluate_with`, and again when that brought new patients: connected databases' persons
/// are linked as their data are found, and every criterion (`not` included) is then applied
/// among all the patients: the connected databases' answers kept (`cache`), the catalog's
/// found again (the linked patients are the catalog's too), whatever the order of the
/// conditions.
#[allow(clippy::type_complexity)]
pub(super) fn evaluate_twice(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
    cache: &Cache,
) -> anyhow::Result<(HashMap<i64, Vec<String>>, HashMap<i64, Vec<String>>)> {
    let count =
        |c: &Connection| -> anyhow::Result<i64> { Ok(c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0))?) };
    let before = count(c)?;
    let first = evaluate_with(c, user, timezone, q, cache)?;
    if count(c)? == before {
        return Ok(first);
    }
    cache.forget_catalog();
    evaluate_with(c, user, timezone, q, cache)
}

#[allow(clippy::type_complexity)]
pub(super) fn evaluate_with(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
    cache: &Cache,
) -> anyhow::Result<(HashMap<i64, Vec<String>>, HashMap<i64, Vec<String>>)> {
    let all = everyone(c)?;
    let mut keep: HashMap<i64, Vec<String>> = all.iter().map(|p| (*p, vec![])).collect();
    if let Some(inc) = &q.include {
        let f = eval(c, user, timezone, inc, &all, cache)?;
        keep.retain(|p, _| f.contains_key(p));
        for (p, why) in keep.iter_mut() {
            push(why, f[p].clone());
        }
    }
    let mut excluded: HashMap<i64, Vec<String>> = HashMap::new();
    for x in &q.exclude {
        for (p, why) in eval(c, user, timezone, x, &all, cache)? {
            if keep.contains_key(&p) {
                push(excluded.entry(p).or_default(), why);
            }
        }
    }
    keep.retain(|p, _| !excluded.contains_key(p));
    Ok((keep, excluded))
}

/// The patients the criteria find, with why (no criteria: everyone).
pub fn evaluate(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
) -> anyhow::Result<HashMap<i64, Vec<String>>> {
    Ok(evaluate_both(c, user, timezone, q)?.0)
}

/// A search: the matching patients as the patient list shows them, latest first; only among
/// `within` when given (a subcohort's parent, the cohorts merged).
pub fn search(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
    within: Option<&HashSet<i64>>,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<Match>, Vec<i64>)> {
    let (found, _) = evaluate_both(c, user, timezone, &q.as_written_by(c, user)?)?;
    let ids: HashSet<i64> = found.keys().copied().filter(|p| within.is_none_or(|w| w.contains(p))).collect();
    let (total, rows) = patient::list_in(c, user, "", &[], Some(&ids), limit, offset)?;
    let mut all: Vec<i64> = ids.into_iter().collect();
    all.sort();
    let out = rows
        .into_iter()
        .map(|p| Match { reasons: found.get(&p.id).cloned().unwrap_or_default(), patient: p })
        .collect();
    Ok((total, out, all))
}

/// A search as the page shows it: a page of the patients found and a page of those the
/// exclusions take out (`excluded_limit` of them), from one evaluation of the criteria.
#[derive(Debug, Serialize)]
pub struct Found {
    pub total: usize,
    pub items: Vec<Match>,
    pub ids: Vec<i64>,
    pub excluded_total: usize,
    pub excluded: Vec<Match>,
    /// What the criteria could not test (values left out: their unit does not compare).
    pub warnings: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn search_page(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
    within: Option<&HashSet<i64>>,
    limit: usize,
    offset: usize,
    excluded_limit: usize,
) -> anyhow::Result<Found> {
    let (found, out, warnings) = evaluate_full(c, user, timezone, &q.as_written_by(c, user)?)?;
    let page = |of: &HashMap<i64, Vec<String>>,
                limit: usize,
                offset: usize|
     -> anyhow::Result<(usize, Vec<Match>, Vec<i64>)> {
        let ids: HashSet<i64> = of.keys().copied().filter(|p| within.is_none_or(|w| w.contains(p))).collect();
        let (total, rows) = patient::list_in(c, user, "", &[], Some(&ids), limit, offset)?;
        let mut all: Vec<i64> = ids.into_iter().collect();
        all.sort();
        let rows = rows
            .into_iter()
            .map(|p| Match { reasons: of.get(&p.id).cloned().unwrap_or_default(), patient: p })
            .collect();
        Ok((total, rows, all))
    };
    let (total, items, ids) = page(&found, limit, offset)?;
    let (excluded_total, excluded, _) =
        if q.exclude.is_empty() { (0, vec![], vec![]) } else { page(&out, excluded_limit, 0)? };
    Ok(Found { total, items, ids, excluded_total, excluded, warnings })
}

/// The patients the exclusions take out (those the inclusion found), with why: a page.
pub fn excluded(
    c: &Connection,
    user: &User,
    timezone: &str,
    q: &Criteria,
    within: Option<&HashSet<i64>>,
    limit: usize,
) -> anyhow::Result<(usize, Vec<Match>)> {
    let (_, out) = evaluate_both(c, user, timezone, &q.as_written_by(c, user)?)?;
    let ids: HashSet<i64> = out.keys().copied().filter(|p| within.is_none_or(|w| w.contains(p))).collect();
    let (total, rows) = patient::list_in(c, user, "", &[], Some(&ids), limit, 0)?;
    Ok((
        total,
        rows.into_iter().map(|p| Match { reasons: out.get(&p.id).cloned().unwrap_or_default(), patient: p }).collect(),
    ))
}
