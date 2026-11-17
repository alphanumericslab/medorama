//! The list of patients (the Patient Catalog): the patients of the databases a user may view,
//! from what Medorama keeps of each database's persons (persons.rs: their identities and
//! encounters), so that it answers whether a database is online or not.  What else a row shows
//! (the kinds of data, the entries, the latest one) is read from the online databases for the
//! page shown only, and a filter on a kind of data (`has`) asks each online database.

use super::*;

// ------------------------------------------------------------------ the list of patients

#[derive(Debug, Clone, Serialize)]
pub struct PatientRow {
    pub id: i64,
    /// The patient ID a database gives (with the PHI permission on it), else empty.
    pub patient_id: String,
    /// What Privacy Mode names the patient by (pseudonyms.rs).
    pub pseudonym: String,
    pub sex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phi: Option<Identity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<i64>,
    pub encounters: i64,
    pub latest: Option<f64>,
    pub latest_where: String,
    /// Categories of data the patient has (category keys): their encounters, then (the page
    /// shown) what the online databases hold of them.
    pub kinds: Vec<String>,
    /// Their entries in the online databases (the page shown; none: not read).
    pub items: Option<i64>,
    /// The databases that link them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

pub const LIST_FIELDS: &[&str] = &["patient", "sex", "has", "database", "name", "first", "last", "mrn", "dob"];

/// One patient as the list holds them before a page is cut.
struct Gathered {
    row: PatientRow,
    /// Identities by database: (database, what it says), for the PHI permission per database.
    records: Vec<crate::persons::Known>,
}

/// Every patient of the databases `user` may view, gathered from the persons files.
fn gather(cfg: &Config, c: &Connection, user: &User) -> anyhow::Result<HashMap<i64, Gathered>> {
    let viewable = crate::databases::granted(c, user, "view")?;
    // outside Privacy Mode, the times of databases granted without `phi` as Privacy Mode shows
    // them (restricted.rs); in it, the gate moves every time
    let deidentified = if user.privacy { HashSet::new() } else { without_phi(c, user)? };
    let offsets = if deidentified.is_empty() { None } else { Some(crate::search::Offsets::load(c)?) };
    let pseudonyms: HashMap<i64, String> = c
        .prepare("SELECT id, COALESCE(pseudonym, '') FROM patients")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut out: HashMap<i64, Gathered> = HashMap::new();
    for s in crate::sources::list(c)? {
        if !viewable.contains(&s.id) {
            continue;
        }
        let sums = crate::persons::encounter_summaries(cfg, c, s.id)?;
        for k in crate::persons::all_of(cfg, c, s.id)? {
            let g = out.entry(k.patient).or_insert_with(|| Gathered {
                row: PatientRow {
                    id: k.patient,
                    patient_id: String::new(),
                    pseudonym: pseudonyms.get(&k.patient).cloned().unwrap_or_default(),
                    sex: String::new(),
                    phi: None,
                    age: None,
                    encounters: 0,
                    latest: None,
                    latest_where: String::new(),
                    kinds: vec![],
                    items: None,
                    sources: vec![],
                },
                records: vec![],
            });
            if let Some((visits, latest, icu)) = sums.get(&k.person_key) {
                let latest = match (&offsets, deidentified.contains(&s.id)) {
                    (Some(o), true) => latest.map(|t| o.shift(Some(k.patient), t)),
                    _ => *latest,
                };
                g.row.encounters += visits;
                g.row.latest = match (g.row.latest, latest) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                for (x, on) in [("enc", *visits > 0), ("icu", *icu)] {
                    if on && !g.row.kinds.iter().any(|k| k == x) {
                        g.row.kinds.push(x.to_string());
                    }
                }
            }
            if !g.row.sources.contains(&s.name) {
                g.row.sources.push(s.name.clone());
            }
            if g.row.sex.is_empty() {
                g.row.sex = k.sex.clone();
            }
            g.records.push(k);
        }
    }
    let order: Vec<&str> = categories::journey_keys().collect();
    for g in out.values_mut() {
        g.row.kinds.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(99));
    }
    Ok(out)
}

/// The patients who have data of a category in the online databases `user` may view.
fn patients_with(cfg: &Config, c: &Connection, user: &User, category: &str) -> anyhow::Result<HashSet<i64>> {
    let viewable = crate::databases::online(c, &crate::databases::granted(c, user, "view")?)?;
    let mut out = HashSet::new();
    for s in crate::sources::list(c)?.into_iter().filter(|s| viewable.contains(&s.id)) {
        let keys = s.persons_with(category, s.zone(&cfg.timezone))?;
        out.extend(crate::persons::known(cfg, c, s.id, &keys)?.into_values());
    }
    Ok(out)
}

/// A page's rows, with what the online databases hold of them (one grouped query each, over
/// these persons only): the kinds of data, the entries, the latest time.
fn enrich(cfg: &Config, c: &Connection, user: &User, page: &mut [Gathered]) -> anyhow::Result<()> {
    let viewable = crate::databases::online(c, &crate::databases::granted(c, user, "view")?)?;
    let deidentified = if user.privacy { HashSet::new() } else { without_phi(c, user)? };
    let offsets = if deidentified.is_empty() { None } else { Some(crate::search::Offsets::load(c)?) };
    for s in crate::sources::list(c)?.into_iter().filter(|s| viewable.contains(&s.id)) {
        let keys: Vec<String> = page
            .iter()
            .flat_map(|g| g.records.iter().filter(|r| r.database == s.id).map(|r| r.person_key.clone()))
            .collect();
        if keys.is_empty() {
            continue;
        }
        let sums = match s.summaries(&keys, s.zone(&cfg.timezone)) {
            Ok(v) => v,
            // a database that stops answering: the page is shown without it
            Err(e) => {
                eprintln!("medorama: {}: {e}", s.name);
                continue;
            }
        };
        for g in page.iter_mut() {
            for r in g.records.iter().filter(|r| r.database == s.id) {
                let Some((kinds, n, latest)) = sums.get(&r.person_key) else { continue };
                let latest = match (&offsets, deidentified.contains(&s.id)) {
                    (Some(o), true) => latest.map(|t| o.shift(Some(g.row.id), t)),
                    _ => *latest,
                };
                g.row.items = Some(g.row.items.unwrap_or(0) + n);
                g.row.latest = match (g.row.latest, latest) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                for x in kinds {
                    if !g.row.kinds.contains(x) {
                        g.row.kinds.push(x.clone());
                    }
                }
            }
        }
    }
    let order: Vec<&str> = categories::journey_keys().collect();
    for g in page.iter_mut() {
        g.row.kinds.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(99));
    }
    Ok(())
}

/// Patients, latest first; `q` uses the search syntax (fields: [`LIST_FIELDS`]; `has:icu` …),
/// `has` limits to patients with all these categories (`!icu`: without).
pub fn list(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &str,
    has: &[String],
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>)> {
    list_in(c, cfg, user, q, has, None, limit, offset)
}

/// `list`, limited to the patients in `only` (a cohort, a cohort search's matches).
#[allow(clippy::too_many_arguments)]
pub fn list_in(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &str,
    has: &[String],
    only: Option<&HashSet<i64>>,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>)> {
    let (total, rows, _) = list_core(c, cfg, user, q, has, only, limit.clamp(1, 1000), offset)?;
    Ok((total, rows))
}

/// `list_in` without pages: every patient in `only`, latest first (exports).
pub fn list_all_in(c: &Connection, cfg: &Config, user: &User, only: &HashSet<i64>) -> anyhow::Result<Vec<PatientRow>> {
    Ok(list_core(c, cfg, user, "", &[], Some(only), usize::MAX, 0)?.1)
}

/// Every patient the list's search finds (the same search: `q`, `has`, `only`), latest first.
pub fn matching_ids(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &str,
    has: &[String],
    only: Option<&HashSet<i64>>,
) -> anyhow::Result<Vec<i64>> {
    Ok(list_core(c, cfg, user, q, has, only, 0, 0)?.2)
}

/// The search behind `list_in` and `matching_ids`: the total, a page of rows, every id found.
#[allow(clippy::too_many_arguments)]
fn list_core(
    c: &Connection,
    cfg: &Config,
    user: &User,
    q: &str,
    has: &[String],
    only: Option<&HashSet<i64>>,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<PatientRow>, Vec<i64>)> {
    let terms = parse(q);
    crate::search::check(&terms, LIST_FIELDS, user.phi).map_err(anyhow::Error::msg)?;
    let phi_dbs: Vec<i64> = if user.phi { crate::databases::granted(c, user, "phi")? } else { vec![] };
    let mut found: Vec<Gathered> = vec![];
    // `ecg`: has ECGs; `!ecg`: has none (asked of each online database)
    let mut with: Vec<(bool, HashSet<i64>)> = vec![];
    for h in has {
        let (want, name) = h.strip_prefix('!').map_or((true, h.as_str()), |n| (false, n));
        let cat = categories::named(name).ok_or_else(|| anyhow::anyhow!("no category {name:?}"))?;
        with.push((want, patients_with(cfg, c, user, cat)?));
    }
    for (id, g) in gather(cfg, c, user)? {
        if only.is_some_and(|o| !o.contains(&id)) {
            continue;
        }
        if !with.iter().all(|(want, set)| set.contains(&id) == *want) {
            continue;
        }
        let k = &g.row.kinds;
        if !terms.is_empty() {
            let mut d = Doc::default();
            d.add("patient", &g.row.pseudonym);
            d.add("sex", &g.row.sex);
            for x in k {
                d.add("has", x);
                d.add("has", categories::label(x));
            }
            for s in &g.row.sources {
                d.add("database", s);
            }
            // identifiers: with the PHI permission, of the databases granted with it
            for r in g.records.iter().filter(|r| phi_dbs.contains(&r.database)) {
                d.add("patient", &r.patient_id);
                d.add("name", &format!("{}, {}", r.last_name, r.first_name));
                d.add("name", &format!("{} {}", r.first_name, r.last_name));
                d.add("first", &r.first_name);
                d.add("last", &r.last_name);
                d.add("dob", &r.birth_date);
                d.add("mrn", &r.mrn);
            }
            if !d.matches(&terms, user.phi) {
                continue;
            }
        }
        found.push(g);
    }
    if user.phi && !q.trim().is_empty() {
        audit(c, Some(user), "phi_search", "patients", q);
    }
    // latest first, those without a time last, then the newest patient; in Privacy Mode by the
    // times as shown (each patient's own offset): an order by real times would tell them
    let o = if user.privacy && !user.phi { Some(crate::search::Offsets::load(c)?) } else { None };
    let shown = |g: &Gathered| g.row.latest.map(|t| o.as_ref().map_or(t, |o| o.shift(Some(g.row.id), t)));
    found.sort_by(|a, b| {
        shown(b).partial_cmp(&shown(a)).unwrap_or(std::cmp::Ordering::Equal).then(b.row.id.cmp(&a.row.id))
    });
    let total = found.len();
    let ids: Vec<i64> = found.iter().map(|g| g.row.id).collect();
    let mut found: Vec<Gathered> = found.into_iter().skip(offset).take(limit).collect();
    // the page's rows (not an export's every row): what the online databases hold of them
    if limit <= 1000 {
        enrich(cfg, c, user, &mut found)?;
    }
    let page: Vec<PatientRow> = found
        .into_iter()
        .map(|g| {
            let mut row = g.row;
            let mine: Vec<crate::persons::Known> =
                g.records.iter().filter(|r| phi_dbs.contains(&r.database)).cloned().collect();
            if user.phi && !mine.is_empty() {
                let ident = identity_of(&mine);
                row.age = age(&ident.birth_date);
                row.patient_id = ident.patient_ids.first().map(|p| p.1.clone()).unwrap_or_default();
                row.phi = Some(ident);
            } else if user.privacy {
                // the date of birth stays on the server; the gate caps the age at 90+
                row.age = age(&identity_of(&g.records).birth_date);
            }
            row
        })
        .collect();
    Ok((total, page, ids))
}
