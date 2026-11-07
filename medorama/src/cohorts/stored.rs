//! Stored cohorts: their definitions and sources, who may read and change them, their members.

use super::*;

// ------------------------------------------------------------------ stored cohorts

/// Where a cohort's members come from besides its criteria: one cohort (`within`: a
/// subcohort) or several (`union`: in any; `intersection`: in all; `difference`: in the first
/// and in none of the others).  Set when the cohort is made; the cohorts it names are followed
/// when it is refreshed (their members as they are then).  When one of them is deleted, its
/// last members are kept here (`frozen`) and followed instead.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Source {
    pub op: String,
    pub of: Vec<i64>,
    /// Their names when it was made (shown once they are deleted).
    pub names: BTreeMap<i64, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub frozen: BTreeMap<i64, Vec<i64>>,
}

/// A cohort a source names, as the definition shows it.
#[derive(Debug, Serialize)]
pub struct SourceRef {
    pub id: i64,
    pub name: String,
    pub deleted: bool,
}

/// A cohort's whole definition, in words: where its members come from, its criteria, and the
/// patients added or removed manually (by patient ID).
#[derive(Debug, Default, Serialize)]
pub struct Definition {
    /// `within`, `union`, `intersection`, `difference`; empty: the whole catalog.
    pub op: String,
    pub sources: Vec<SourceRef>,
    /// The source in words: "Subcohort of “A”", "In “A” or “B”" …
    pub from: String,
    pub include: String,
    pub exclude: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// How it was made, when not from scratch: "Cloned from “A”".
    pub origin: String,
}

#[derive(Debug, Serialize)]
pub struct Cohort {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner: Option<i64>,
    pub owner_name: String,
    pub criteria: Criteria,
    pub source: Option<Source>,
    /// A subcohort's parent.
    pub parent: Option<i64>,
    pub definition: Definition,
    pub members: i64,
    pub created_at: f64,
    pub updated_at: f64,
    pub refreshed_at: Option<f64>,
    /// The version of each source when its members were last found: the catalog's data
    /// version, and each connected database's own (docs/design/sources.md, "Cohorts across sources").
    pub source_versions: BTreeMap<String, String>,
    pub can_edit: bool,
    pub is_owner: bool,
    /// The demo's cohort (demo.rs): deleting it removes the demo's patients.
    pub demo: bool,
    /// Who else may see it (the owner only sees this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shares: Option<Vec<Share>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Share {
    pub user_id: i64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub can_edit: bool,
}

/// (may read, may edit, is owner)
pub(super) fn access(c: &Connection, user: &User, id: i64) -> anyhow::Result<(bool, bool, bool)> {
    let owner: Option<Option<i64>> =
        c.query_row("SELECT owner FROM cohorts WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    let Some(owner) = owner else { anyhow::bail!("no cohort {id}") };
    // the local command line (user 0) is the catalog's owner
    let is_owner = user.id == 0 || owner == Some(user.id);
    if is_owner {
        return Ok((true, true, true));
    }
    let share: Option<bool> = c
        .query_row("SELECT can_edit FROM cohort_shares WHERE cohort = ?1 AND user_id = ?2", [id, user.id], |r| r.get(0))
        .optional()?;
    // the demo's cohort: every user sees it
    Ok((share.is_some() || is_demo(c, id)?, share == Some(true), false))
}

/// Whether the cohort is the demo's (demo.rs).
pub fn is_demo(c: &Connection, id: i64) -> anyhow::Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS (SELECT 1 FROM cohorts WHERE id = ?1 AND origin = ?2)",
        params![id, crate::demo::ORIGIN],
        |r| r.get(0),
    )?)
}

pub(super) fn readable(c: &Connection, user: &User, id: i64) -> anyhow::Result<(bool, bool)> {
    let (r, e, o) = access(c, user, id)?;
    if !r {
        anyhow::bail!("no cohort {id}");
    }
    Ok((e, o))
}

pub(super) fn editable(c: &Connection, user: &User, id: i64) -> anyhow::Result<bool> {
    let (_, can_edit, is_owner) = access(c, user, id)?;
    if !can_edit {
        anyhow::bail!("this cohort is shared with you to read only");
    }
    Ok(is_owner)
}

/// Patients' IDs as definitions and the history show them (the catalog's number when a
/// patient has no patient ID), in order.
pub(super) fn patient_ids(c: &Connection, ids: impl IntoIterator<Item = i64>) -> anyhow::Result<Vec<String>> {
    let mut st = c.prepare("SELECT patient_id FROM patients WHERE id = ?1")?;
    let mut out: Vec<String> = vec![];
    for id in ids {
        let pid: Option<String> = st.query_row([id], |r| r.get(0)).optional()?;
        out.push(pid.filter(|p| !p.is_empty()).unwrap_or_else(|| format!("#{id}")));
    }
    out.sort_by(|a, b| natural(a, b));
    Ok(out)
}

/// Text with numbers compared as numbers ("P2" before "P10").
pub(super) fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| {
        let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
        (s.trim_end_matches(|c: char| c.is_ascii_digit()).to_string(), digits.len(), digits)
    };
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

/// "1 patient", "3 patients".
pub(super) fn n_patients(n: usize) -> String {
    format!("{n} patient{}", if n == 1 { "" } else { "s" })
}

/// A list for the history: the first ones, and how many more.
pub(super) fn some(list: &[String]) -> String {
    const SHOWN: usize = 20;
    if list.len() <= SHOWN {
        list.join(", ")
    } else {
        format!("{} and {} more", list[..SHOWN].join(", "), list.len() - SHOWN)
    }
}

/// Names in quotes, joined: “A”, “B” and “C”.
pub(super) fn quoted(names: &[String], last: &str) -> String {
    let q: Vec<String> = names.iter().map(|n| format!("“{n}”")).collect();
    match q.len() {
        0 => String::new(),
        1 => q[0].clone(),
        n => format!("{} {last} {}", q[..n - 1].join(", "), q[n - 1]),
    }
}

/// A source in words, from the names of its cohorts.
pub(super) fn source_words(op: &str, names: &[String]) -> String {
    match op {
        "within" => format!("Subcohort of {}", quoted(names, "and")),
        "union" => format!("In {}", quoted(names, "or")),
        "intersection" => format!("In {}", quoted(names, "and")),
        "difference" if names.len() > 1 => {
            format!("In {} but not in {}", quoted(&names[..1], ""), quoted(&names[1..], "or"))
        }
        _ => String::new(),
    }
}

pub(super) fn definition(
    c: &Connection,
    id: i64,
    criteria: &Criteria,
    source: Option<&Source>,
    origin: &str,
) -> anyhow::Result<Definition> {
    let mut d = Definition { origin: origin.to_string(), ..Default::default() };
    if let Some(s) = source {
        d.op = s.op.clone();
        for x in &s.of {
            let now: Option<String> =
                c.query_row("SELECT name FROM cohorts WHERE id = ?1", [x], |r| r.get(0)).optional()?;
            d.sources.push(match now {
                Some(name) => SourceRef { id: *x, name, deleted: false },
                None => SourceRef {
                    id: *x,
                    name: s.names.get(x).cloned().unwrap_or_else(|| format!("#{x}")),
                    deleted: true,
                },
            });
        }
        let names: Vec<String> = d
            .sources
            .iter()
            .map(|r| if r.deleted { format!("{} (deleted)", r.name) } else { r.name.clone() })
            .collect();
        d.from = source_words(&s.op, &names);
    }
    let words = criteria_words(criteria);
    d.include = words["include"].as_str().unwrap_or("").to_string();
    d.exclude =
        words["exclude"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(String::from).collect();
    let manual: Vec<i64> = c
        .prepare("SELECT patient FROM cohort_members WHERE cohort = ?1 AND how = 'manual'")?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    d.added = patient_ids(c, manual)?;
    let removed: Vec<i64> = c
        .prepare("SELECT patient FROM cohort_removed WHERE cohort = ?1")?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    d.removed = patient_ids(c, removed)?;
    Ok(d)
}

pub(super) fn load(c: &Connection, user: &User, id: i64) -> anyhow::Result<Cohort> {
    let (can_edit, is_owner) = readable(c, user, id)?;
    let (mut co, origin) = c.query_row(
        "SELECT c.id, c.name, c.description, c.owner, COALESCE(u.username, ''), c.criteria, c.created_at, c.updated_at,
                c.refreshed_at, (SELECT COUNT(*) FROM cohort_members m WHERE m.cohort = c.id), c.source, c.origin,
                c.source_versions
         FROM cohorts c LEFT JOIN users u ON u.id = c.owner WHERE c.id = ?1",
        [id],
        |r| {
            let source: Option<Source> = r.get::<_, Option<String>>(10)?.and_then(|s| serde_json::from_str(&s).ok());
            Ok((
                Cohort {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    owner: r.get(3)?,
                    owner_name: r.get(4)?,
                    criteria: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
                    parent: source.as_ref().filter(|s| s.op == "within").and_then(|s| s.of.first().copied()),
                    source,
                    definition: Definition::default(),
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                    refreshed_at: r.get(8)?,
                    source_versions: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
                    members: r.get(9)?,
                    can_edit,
                    is_owner,
                    demo: false,
                    shares: None,
                },
                r.get::<_, String>(11)?,
            ))
        },
    )?;
    co.demo = origin == crate::demo::ORIGIN;
    co.definition = definition(c, id, &co.criteria, co.source.as_ref(), &origin)?;
    if is_owner {
        let mut st = c.prepare(
            "SELECT s.user_id, u.username, s.can_edit FROM cohort_shares s JOIN users u ON u.id = s.user_id
             WHERE s.cohort = ?1 ORDER BY u.username",
        )?;
        co.shares = Some(
            st.query_map([id], |r| Ok(Share { user_id: r.get(0)?, username: r.get(1)?, can_edit: r.get(2)? }))?
                .collect::<Result<_, _>>()?,
        );
    }
    Ok(co)
}

/// The cohorts a user may see: their own, those shared with them and the demo's.
pub fn list(c: &Connection, user: &User) -> anyhow::Result<Vec<Cohort>> {
    let ids: Vec<i64> = c
        .prepare(
            "SELECT id FROM cohorts WHERE ?1 = 0 OR owner = ?1 OR origin = ?2
               OR id IN (SELECT cohort FROM cohort_shares WHERE user_id = ?1) ORDER BY updated_at DESC, id DESC",
        )?
        .query_map(params![user.id, crate::demo::ORIGIN], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    ids.into_iter().map(|id| load(c, user, id)).collect()
}

pub fn get(c: &Connection, user: &User, id: i64) -> anyhow::Result<Cohort> {
    load(c, user, id)
}

/// The members of a cohort, as the patient list shows them, with how and when each was added.
pub fn members(
    c: &Connection,
    user: &User,
    id: i64,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<serde_json::Value>)> {
    members_with(c, user, id, |ids| patient::list_in(c, user, "", &[], Some(ids), limit, offset))
}

/// Every member, as [`members`] lists them (exports: no pages).
pub(super) fn all_members(c: &Connection, user: &User, id: i64) -> anyhow::Result<Vec<serde_json::Value>> {
    Ok(members_with(c, user, id, |ids| {
        let rows = patient::list_all_in(c, user, ids)?;
        Ok((rows.len(), rows))
    })?
    .1)
}

fn members_with(
    c: &Connection,
    user: &User,
    id: i64,
    list: impl FnOnce(&HashSet<i64>) -> anyhow::Result<(usize, Vec<PatientRow>)>,
) -> anyhow::Result<(usize, Vec<serde_json::Value>)> {
    readable(c, user, id)?;
    let mut how: HashMap<i64, (String, f64)> = HashMap::new();
    let mut st = c.prepare("SELECT patient, how, added_at FROM cohort_members WHERE cohort = ?1")?;
    for r in st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?)))? {
        let (p, h, t) = r?;
        how.insert(p, (h, t));
    }
    let ids: HashSet<i64> = how.keys().copied().collect();
    let (total, rows) = list(&ids)?;
    let out = rows
        .into_iter()
        .map(|p| {
            let (h, t) = how.get(&p.id).cloned().unwrap_or_default();
            let mut v = serde_json::to_value(&p).unwrap_or_default();
            v["how"] = h.into();
            v["added_at"] = t.into();
            v
        })
        .collect();
    Ok((total, out))
}

/// The patient ids of a cohort (for job selections and the patient list).
pub fn member_ids(c: &Connection, user: &User, id: i64) -> anyhow::Result<HashSet<i64>> {
    readable(c, user, id)?;
    stored_members(c, id)
}

pub(super) fn stored_members(c: &Connection, id: i64) -> anyhow::Result<HashSet<i64>> {
    Ok(c.prepare("SELECT patient FROM cohort_members WHERE cohort = ?1")?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

/// A source as the user asked for it, checked: a known operation, the right number of cohorts,
/// each one they may read; with the cohorts' names.
pub fn checked_source(c: &Connection, user: &User, s: &Source) -> anyhow::Result<Source> {
    let mut of: Vec<i64> = vec![];
    for x in &s.of {
        if !of.contains(x) {
            of.push(*x);
        }
    }
    match (s.op.as_str(), of.len()) {
        ("within", 1) => {}
        ("within", _) => anyhow::bail!("a subcohort is made within one cohort"),
        ("union" | "intersection" | "difference", n) if n >= 2 => {}
        ("union" | "intersection" | "difference", _) => anyhow::bail!("a merge needs at least two cohorts"),
        (op, _) => anyhow::bail!("unknown source {op:?} (within, union, intersection, difference)"),
    }
    let mut names = BTreeMap::new();
    for x in &of {
        readable(c, user, *x)?;
        names.insert(*x, c.query_row("SELECT name FROM cohorts WHERE id = ?1", [x], |r| r.get::<_, String>(0))?);
    }
    Ok(Source { op: s.op.clone(), of, names, frozen: BTreeMap::new() })
}

/// [`source_members`], for a user who may still read each of its cohorts (a cohort no longer
/// shared with them is not followed: they may not see who is in it).
pub fn readable_source_members(c: &Connection, user: &User, s: &Source) -> anyhow::Result<HashSet<i64>> {
    for x in s.of.iter().filter(|x| !s.frozen.contains_key(x)) {
        if !access(c, user, *x)?.0 {
            let name = s.names.get(x).cloned().unwrap_or_else(|| x.to_string());
            anyhow::bail!(
                "“{name}”, which this cohort is made from, is no longer shared with you: it cannot be followed"
            );
        }
    }
    source_members(c, s)
}

/// The patients a source gives: its cohorts' members now (or as kept when one was deleted).
pub fn source_members(c: &Connection, s: &Source) -> anyhow::Result<HashSet<i64>> {
    let mut sets = vec![];
    for x in &s.of {
        sets.push(match s.frozen.get(x) {
            Some(kept) => kept.iter().copied().collect(),
            None => stored_members(c, *x)?,
        });
    }
    let mut sets = sets.into_iter();
    let first: HashSet<i64> = sets.next().unwrap_or_default();
    Ok(match s.op.as_str() {
        "intersection" => sets.fold(first, |a, b| a.intersection(&b).copied().collect()),
        "difference" => sets.fold(first, |a, b| a.difference(&b).copied().collect()),
        // within (one cohort), union
        _ => sets.fold(first, |mut a, b| {
            a.extend(b);
            a
        }),
    })
}

/// The patients a definition selects before its manual changes: those of its source (the whole
/// catalog without one) who meet its criteria.  Without a source or criteria: nobody (a cohort
/// of patients added manually).
pub(super) fn selected(
    c: &Connection,
    user: &User,
    timezone: &str,
    criteria: &Criteria,
    source: Option<&Source>,
) -> anyhow::Result<HashSet<i64>> {
    let from = source.map(|s| readable_source_members(c, user, s)).transpose()?;
    if criteria.is_empty() {
        return Ok(from.unwrap_or_default());
    }
    let found = evaluate(c, user, timezone, criteria)?;
    Ok(found.into_keys().filter(|p| from.as_ref().is_none_or(|f| f.contains(p))).collect())
}

/// A stored cohort's selection (its criteria and source as saved).
pub(super) fn selected_by(c: &Connection, user: &User, timezone: &str, id: i64) -> anyhow::Result<HashSet<i64>> {
    let (criteria, source): (String, Option<String>) =
        c.query_row("SELECT criteria, source FROM cohorts WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let criteria: Criteria = serde_json::from_str(&criteria).unwrap_or_default();
    let source: Option<Source> = source.and_then(|s| serde_json::from_str(&s).ok());
    criteria.check_reader(user)?;
    selected(c, user, timezone, &criteria, source.as_ref())
}

/// Adds patients who are not members yet: those the definition selects (`sel`) as found by it,
/// the others as added manually.  Patients removed manually before are no longer kept out.
/// Returns those added, by how.
pub(super) fn add_members(
    c: &Connection,
    user: &User,
    id: i64,
    patients: &[i64],
    sel: &HashSet<i64>,
) -> anyhow::Result<(Vec<i64>, Vec<i64>)> {
    let t = now();
    // the patients that exist and are not members yet, then added many to a statement
    let known: HashSet<i64> = c
        .query_in(
            &format!("SELECT id FROM patients WHERE id IN ({{}}) AND {}", patient::patient_shown("id")),
            patients,
            |r| r.get(0),
        )?
        .into_iter()
        .collect();
    let members: HashSet<i64> = c
        .query_in(
            &format!("SELECT patient FROM cohort_members WHERE cohort = {id} AND patient IN ({{}})"),
            patients,
            |r| r.get(0),
        )?
        .into_iter()
        .collect();
    let mut seen = HashSet::new();
    let new: Vec<i64> =
        patients.iter().copied().filter(|p| known.contains(p) && !members.contains(p) && seen.insert(*p)).collect();
    let by = (user.id > 0).then_some(user.id);
    for chunk in new.chunks(400) {
        let mut values: Vec<crate::sql::Value> = vec![];
        let rows: Vec<String> = chunk
            .iter()
            .map(|p| {
                let n = values.len();
                values.extend([
                    crate::sql::Value::Integer(id),
                    crate::sql::Value::Integer(*p),
                    (if sel.contains(p) { "criteria" } else { "manual" }).into(),
                    by.into(),
                    t.into(),
                ]);
                format!("(?{}, ?{}, ?{}, ?{}, ?{})", n + 1, n + 2, n + 3, n + 4, n + 5)
            })
            .collect();
        c.execute(
            &format!(
                "INSERT OR IGNORE INTO cohort_members (cohort, patient, how, added_by, added_at) VALUES {}",
                rows.join(", ")
            ),
            values,
        )?;
    }
    c.query_in(&format!("DELETE FROM cohort_removed WHERE cohort = {id} AND patient IN ({{}})"), &new, |_| Ok(()))?;
    let (by_def, manual): (Vec<i64>, Vec<i64>) = new.into_iter().partition(|p| sel.contains(p));
    Ok((by_def, manual))
}

pub(super) fn store_source(s: Option<&Source>) -> anyhow::Result<Option<String>> {
    Ok(s.map(serde_json::to_string).transpose()?)
}
