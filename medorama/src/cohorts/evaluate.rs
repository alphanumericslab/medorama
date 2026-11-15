//! Evaluating criteria (docs/design/databases-only.md, "Searching"): each criterion in every
//! database of the search's scope that holds such data, then the groups.
//!
//! The scope is the databases the user ticked (none: every one), of those granted to them for
//! searching, and online now; the others are left out and named in the answer.  Each patient
//! found keeps the databases whose answers put them there (their provenance): an `and` keeps
//! the databases of each part, an `or` those of every part the patient meets, a `not` and the
//! registry's criteria (sex, age, a database) the databases of the scope that link the patient.

use super::*;

/// A patient found: why, and the databases whose answers say so.
#[derive(Debug, Clone, Default)]
pub struct Hit {
    pub why: Vec<String>,
    pub from: BTreeSet<i64>,
}

pub type Hits = HashMap<i64, Hit>;

/// A database a search did not ask, and why.
#[derive(Debug, Clone, Serialize)]
pub struct LeftOut {
    pub id: i64,
    pub name: String,
    pub why: String,
}

/// The databases a search asks.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Scope {
    /// (id, name) of each database asked.
    pub searched: Vec<(i64, String)>,
    pub left_out: Vec<LeftOut>,
    #[serde(skip)]
    pub sources: Vec<crate::sources::Source>,
}

impl Scope {
    /// The scope of a search by `user`: the databases `chosen` (none: every one) that they may
    /// search and that answer now.
    pub fn new(c: &Connection, user: &User, chosen: Option<&[i64]>) -> anyhow::Result<Scope> {
        let granted = crate::databases::granted(c, user, "search")?;
        let mut s = Scope::default();
        for d in crate::databases::list(c)? {
            let (id, name) = (d.source.id, d.source.name.clone());
            if !granted.contains(&id) {
                continue;
            }
            if chosen.is_some_and(|ch| !ch.is_empty() && !ch.contains(&id)) {
                s.left_out.push(LeftOut { id, name, why: "not ticked".into() });
            } else if d.status != "online" {
                s.left_out.push(LeftOut { id, name, why: "offline".into() });
            } else {
                s.searched.push((id, name));
                s.sources.push(d.source);
            }
        }
        Ok(s)
    }

    pub fn ids(&self) -> Vec<i64> {
        self.searched.iter().map(|s| s.0).collect()
    }
}

/// An evaluation's context: the control store, the scope, each criterion's patients found once
/// (connected databases are asked once), and what it could not test.
pub(super) struct Cx<'a> {
    pub(super) c: &'a Connection,
    pub(super) cfg: &'a Config,
    pub(super) user: &'a User,
    pub(super) timezone: &'a str,
    pub(super) scope: &'a Scope,
    /// Each criterion's answers.
    sources: std::cell::RefCell<HashMap<String, HashMap<i64, (String, BTreeSet<i64>)>>>,
    /// Each database's persons as kept (the registry's criteria).
    persons: std::cell::RefCell<HashMap<i64, Vec<crate::persons::Known>>>,
    /// Values left out (their unit does not compare), in words, each once.
    pub(super) warnings: std::cell::RefCell<Vec<String>>,
}

impl<'a> Cx<'a> {
    pub(super) fn new(
        c: &'a Connection,
        cfg: &'a Config,
        user: &'a User,
        timezone: &'a str,
        scope: &'a Scope,
    ) -> Cx<'a> {
        Cx {
            c,
            cfg,
            user,
            timezone,
            scope,
            sources: Default::default(),
            persons: Default::default(),
            warnings: Default::default(),
        }
    }

    pub(super) fn warn(&self, w: Option<String>) {
        let mut ws = self.warnings.borrow_mut();
        if let Some(w) = w.filter(|w| !ws.contains(w)) {
            ws.push(w);
        }
    }

    /// A database's persons as kept, read once per evaluation.
    fn persons_of(&self, db: i64) -> anyhow::Result<Vec<crate::persons::Known>> {
        if let Some(v) = self.persons.borrow().get(&db) {
            return Ok(v.clone());
        }
        let v = crate::persons::all_of(self.cfg, self.c, db)?;
        self.persons.borrow_mut().insert(db, v.clone());
        Ok(v)
    }

    /// The registry's answers are read again (patients were linked since).
    fn forget_persons(&self) {
        self.persons.borrow_mut().clear();
    }
}

/// Every patient of the scope, with the databases of the scope that link them.
pub(super) fn everyone(cx: &Cx) -> anyhow::Result<HashMap<i64, BTreeSet<i64>>> {
    let mut out: HashMap<i64, BTreeSet<i64>> = HashMap::new();
    for db in cx.scope.ids() {
        for p in cx.persons_of(db)? {
            out.entry(p.patient).or_default().insert(db);
        }
    }
    Ok(out)
}

/// The patients the registry says meet a criterion of its own: sex, age, a database.
fn meets_registry(cx: &Cx, k: &Criterion) -> anyhow::Result<HashMap<i64, (String, BTreeSet<i64>)>> {
    let mut out: HashMap<i64, (String, BTreeSet<i64>)> = HashMap::new();
    if k.kind == "age" && !cx.user.phi && !cx.user.privacy {
        anyhow::bail!("age comes from the date of birth: it needs the PHI permission");
    }
    for (db, name) in &cx.scope.searched {
        if k.kind == "database" && k.database != Some(*db) {
            continue;
        }
        // an age the session is shown capped (Privacy Mode, or a database granted without
        // `phi`) is tested capped: 90 for every age above 89, so that only "90 or more" finds
        // them, and the reason says "age 90+"
        let capped = k.kind == "age"
            && (cx.user.privacy || !crate::grants::can(cx.c, cx.user, &format!("db:{db}"), "phi")?)
            && crate::deid::load(cx.c)?.hides("age_over_89");
        for p in cx.persons_of(*db)? {
            let why = match k.kind.as_str() {
                "sex" if !p.sex.is_empty() && p.sex.eq_ignore_ascii_case(k.text.trim()) => format!("sex {}", p.sex),
                "age" => match patient::age(&p.birth_date).map(|a| if capped { a.min(90) } else { a }) {
                    Some(a) if test_value(k, a as f64, "")? => {
                        if capped && a >= 90 {
                            "age 90+".to_string()
                        } else {
                            format!("age {a}")
                        }
                    }
                    _ => continue,
                },
                "database" => format!("in {name}"),
                _ => continue,
            };
            let e = out.entry(p.patient).or_insert_with(|| (why, BTreeSet::new()));
            e.1.insert(*db);
        }
    }
    Ok(out)
}

/// Whether a database's variables say it may hold what a criterion asks for: a database that
/// cannot is not asked.
fn may_hold(cx: &Cx, k: &Criterion, s: &crate::sources::Source) -> anyhow::Result<bool> {
    let v = |name: &str, contains: bool| crate::variables::holds(cx.cfg, cx.c, s.id, name, contains);
    let cat = |c: &str| crate::variables::holds_category(cx.cfg, cx.c, s.id, c);
    Ok(match k.kind.as_str() {
        "measurement" => v(&k.name, k.database != Some(s.id))?,
        "diagnosis" => cat("dx")?,
        "medication" => cat("med")? || cat("blood")?,
        _ => true,
    })
}

/// The patients meeting one criterion (before `not`), each with a reason and its databases.
/// Each part is found once per evaluation; a measurement's `latest` is each patient's latest
/// value across the scope.
pub(super) fn meets(cx: &Cx, k: &Criterion) -> anyhow::Result<HashMap<i64, (String, BTreeSet<i64>)>> {
    if matches!(k.kind.as_str(), "sex" | "age" | "database") {
        return meets_registry(cx, k);
    }
    if k.kind == "expression" {
        return meets_expression(cx, k);
    }
    let key = serde_json::to_string(&Criterion { not: false, ..k.clone() })?;
    if let Some(f) = cx.sources.borrow().get(&key) {
        return Ok(f.clone());
    }
    let f = if k.kind == "measurement" && k.latest {
        latest_across(cx, k)?
    } else {
        let mut f: HashMap<i64, (String, BTreeSet<i64>)> = HashMap::new();
        for s in &cx.scope.sources {
            if k.database.is_some_and(|d| d != s.id) || !may_hold(cx, k, s)? {
                continue;
            }
            for (p, why) in meets_source(cx, k, s)? {
                let e = f.entry(p).or_insert_with(|| (why, BTreeSet::new()));
                e.1.insert(s.id);
            }
        }
        f
    };
    cx.sources.borrow_mut().insert(key, f.clone());
    Ok(f)
}

/// Criteria written as text: as saved; the text read again only to say when it now means
/// something else.
fn meets_expression(cx: &Cx, k: &Criterion) -> anyhow::Result<HashMap<i64, (String, BTreeSet<i64>)>> {
    let now = parse_text(cx.c, cx.cfg, cx.user, &k.text);
    let e = match (&k.resolved, now) {
        (Some(saved), Ok(now)) => {
            if **saved != now {
                cx.warn(Some(format!(
                    "“{}” now reads as {}: the criteria keep what it meant when saved ({})",
                    k.text,
                    describe_expr(&now),
                    describe_expr(saved)
                )));
            }
            (**saved).clone()
        }
        (Some(saved), Err(why)) => {
            cx.warn(Some(format!(
                "“{}” would not be read now ({why}): the criteria keep what it meant when saved",
                k.text
            )));
            (**saved).clone()
        }
        (None, now) => now?,
    };
    let all = everyone(cx)?;
    Ok(eval(cx, &e, &all)?.into_iter().map(|(p, h)| (p, (h.why.join(", "), h.from))).collect())
}

/// `latest`, for a measurement: each patient's values in every database of the scope that
/// holds it (or the one named), the latest of them tested.
fn latest_across(cx: &Cx, k: &Criterion) -> anyhow::Result<HashMap<i64, (String, BTreeSet<i64>)>> {
    let zone = tz(cx.timezone);
    // (patient, name, value, unit, time, database)
    let mut rows: Vec<(i64, String, f64, String, Option<f64>, i64)> = vec![];
    for s in &cx.scope.sources {
        if k.database.is_some_and(|d| d != s.id) || !may_hold(cx, k, s)? {
            continue;
        }
        rows.extend(source_measurement_rows(cx, k, s)?.into_iter().map(|(p, n, v, u, t)| (p, n, v, u, t, s.id)));
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
        let (p, name, v, unit, t, db) = &rows[i];
        if !skipped.compares(k, unit) {
            continue;
        }
        if has_value_test(k) && !test_value(k, *v, unit)? {
            continue;
        }
        let when =
            t.map(|t| format!(" on {}", crate::util::fmt_local(t, zone).get(..10).unwrap_or(""))).unwrap_or_default();
        out.insert(*p, (format!("{name} {} {unit}{when}", fmt_num(*v)).replace("  ", " "), BTreeSet::from([*db])));
    }
    skipped.done(k, cx)?;
    Ok(out)
}

pub(super) fn push(why: &mut Vec<String>, more: impl IntoIterator<Item = String>) {
    for w in more {
        if why.len() < MAX_REASONS && !why.contains(&w) {
            why.push(w);
        }
    }
}

/// The patients an expression holds for, each with why and its databases.
pub(super) fn eval(cx: &Cx, e: &Expr, all: &HashMap<i64, BTreeSet<i64>>) -> anyhow::Result<Hits> {
    Ok(match e {
        Expr::Leaf(k) => {
            let found = meets(cx, k)?;
            if k.not {
                all.iter()
                    .filter(|(p, _)| !found.contains_key(p))
                    .map(|(p, dbs)| (*p, Hit { why: vec![format!("no {}", describe(k))], from: dbs.clone() }))
                    .collect()
            } else {
                found.into_iter().map(|(p, (w, from))| (p, Hit { why: vec![w], from })).collect()
            }
        }
        Expr::Group(g) => match g.op.as_str() {
            "and" => {
                let mut keep: Option<Hits> = None;
                for x in &g.of {
                    let f = eval(cx, x, all)?;
                    keep = Some(match keep {
                        None => f,
                        Some(mut k) => {
                            k.retain(|p, _| f.contains_key(p));
                            for (p, hit) in k.iter_mut() {
                                push(&mut hit.why, f[p].why.clone());
                                hit.from.extend(f[p].from.iter().copied());
                            }
                            k
                        }
                    });
                }
                keep.unwrap_or_else(|| {
                    all.iter().map(|(p, dbs)| (*p, Hit { why: vec![], from: dbs.clone() })).collect()
                })
            }
            "or" => {
                let mut out: Hits = HashMap::new();
                for x in &g.of {
                    for (p, hit) in eval(cx, x, all)? {
                        match out.get_mut(&p) {
                            Some(h) => h.from.extend(hit.from),
                            None => {
                                out.insert(p, hit);
                            }
                        }
                    }
                }
                out
            }
            "not" => {
                let mut inner: Hits = HashMap::new();
                for x in &g.of {
                    inner.extend(eval(cx, x, all)?);
                }
                all.iter()
                    .filter(|(p, _)| !inner.contains_key(p))
                    .map(|(p, dbs)| (*p, Hit { why: vec!["not the group".to_string()], from: dbs.clone() }))
                    .collect()
            }
            op => anyhow::bail!("unknown group {op:?} (and, or, not)"),
        },
    })
}

/// A whole evaluation: who the criteria include, who they exclude among those (with why), what
/// could not be tested, and which databases were asked.
#[derive(Debug, Default)]
pub struct Evaluated {
    pub found: Hits,
    pub excluded: Hits,
    pub warnings: Vec<String>,
    pub scope: Scope,
}

/// Evaluate criteria among the databases `chosen` (none: every one the user may search).
/// Audited: searches of identifiers (`phi_search`), and every search in Privacy Mode
/// (`privacy_search`).
pub fn evaluate_full(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &Criteria,
    chosen: Option<&[i64]>,
) -> anyhow::Result<Evaluated> {
    let scope = Scope::new(c, user, chosen)?;
    let cx = Cx::new(c, cfg, user, &cfg.timezone, &scope);
    let (found, excluded) = evaluate_twice(&cx, q)?;
    let phi_criteria =
        q.leaves().iter().any(|k| k.kind == "age" || (k.kind == "text" && user.phi) || k.kind == "expression");
    if user.phi && phi_criteria {
        audit(c, Some(user), "phi_search", "cohorts", &serde_json::to_string(q)?);
    } else if user.privacy && !user.phi && !q.is_empty() {
        audit(c, Some(user), "privacy_search", "cohorts", &serde_json::to_string(q)?);
    }
    let warnings = cx.warnings.into_inner();
    Ok(Evaluated { found, excluded, warnings, scope })
}

/// `evaluate_with`, and again when that linked new patients: a database's persons are linked
/// as their data are found, and every criterion (`not` included) is then applied among all the
/// patients, the databases' answers kept, whatever the order of the conditions.
pub(super) fn evaluate_twice(cx: &Cx, q: &Criteria) -> anyhow::Result<(Hits, Hits)> {
    let count =
        |c: &Connection| -> anyhow::Result<i64> { Ok(c.query_row("SELECT COUNT(*) FROM patients", [], |r| r.get(0))?) };
    let before = count(cx.c)?;
    let first = evaluate_with(cx, q)?;
    if count(cx.c)? == before {
        return Ok(first);
    }
    cx.forget_persons();
    evaluate_with(cx, q)
}

pub(super) fn evaluate_with(cx: &Cx, q: &Criteria) -> anyhow::Result<(Hits, Hits)> {
    let all = everyone(cx)?;
    let mut keep: Hits = all.iter().map(|(p, dbs)| (*p, Hit { why: vec![], from: dbs.clone() })).collect();
    if let Some(inc) = &q.include {
        let f = eval(cx, inc, &all)?;
        keep.retain(|p, _| f.contains_key(p));
        for (p, hit) in keep.iter_mut() {
            // in because of the criteria: their databases, not every one that links the patient
            hit.from = f[p].from.clone();
            push(&mut hit.why, f[p].why.clone());
        }
    }
    let mut excluded: Hits = HashMap::new();
    for x in &q.exclude {
        for (p, hit) in eval(cx, x, &all)? {
            if keep.contains_key(&p) {
                let e = excluded.entry(p).or_default();
                push(&mut e.why, hit.why);
                e.from.extend(hit.from);
            }
        }
    }
    keep.retain(|p, _| !excluded.contains_key(p));
    // an exclusion asked every database of the patient: their place depends on those answers
    if !q.exclude.is_empty() {
        for (p, hit) in keep.iter_mut() {
            if let Some(dbs) = all.get(p) {
                hit.from.extend(dbs.iter().copied());
            }
        }
    }
    Ok((keep, excluded))
}

/// The patients the criteria find, with why (no criteria: everyone of the scope).
pub fn evaluate(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &Criteria,
    chosen: Option<&[i64]>,
) -> anyhow::Result<Hits> {
    Ok(evaluate_full(c, cfg, user, q, chosen)?.found)
}

/// A search as the page shows it: a page of the patients found and a page of those the
/// exclusions take out (`excluded_limit` of them), from one evaluation of the criteria, with
/// the databases asked and those left out.
#[derive(Debug, Serialize)]
pub struct Found {
    pub total: usize,
    pub items: Vec<Match>,
    pub ids: Vec<i64>,
    pub excluded_total: usize,
    pub excluded: Vec<Match>,
    /// What the criteria could not test (values left out: their unit does not compare).
    pub warnings: Vec<String>,
    pub searched: Vec<(i64, String)>,
    pub left_out: Vec<LeftOut>,
}

#[allow(clippy::too_many_arguments)]
pub fn search_page(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &Criteria,
    chosen: Option<&[i64]>,
    within: Option<&HashSet<i64>>,
    limit: usize,
    offset: usize,
    excluded_limit: usize,
) -> anyhow::Result<Found> {
    let ev = evaluate_full(c, cfg, user, &q.as_written_by(c, cfg, user)?, chosen)?;
    let page = |of: &Hits, limit: usize, offset: usize| -> anyhow::Result<(usize, Vec<Match>, Vec<i64>)> {
        let ids: HashSet<i64> = of.keys().copied().filter(|p| within.is_none_or(|w| w.contains(p))).collect();
        let (total, rows) = patient::list_in(c, cfg, user, "", &[], Some(&ids), limit, offset)?;
        let mut all: Vec<i64> = ids.into_iter().collect();
        all.sort();
        let rows = rows
            .into_iter()
            .map(|p| Match { reasons: of.get(&p.id).map(|h| h.why.clone()).unwrap_or_default(), patient: p })
            .collect();
        Ok((total, rows, all))
    };
    let (total, items, ids) = page(&ev.found, limit, offset)?;
    let (excluded_total, excluded, _) =
        if q.exclude.is_empty() { (0, vec![], vec![]) } else { page(&ev.excluded, excluded_limit, 0)? };
    Ok(Found {
        total,
        items,
        ids,
        excluded_total,
        excluded,
        warnings: ev.warnings,
        searched: ev.scope.searched,
        left_out: ev.scope.left_out,
    })
}
