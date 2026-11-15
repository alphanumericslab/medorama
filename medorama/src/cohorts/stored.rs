//! Stored cohorts: their definitions and sources, who may read and change them (grants on
//! `cohort:ID`), their members (in the workspace: members.rs).

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

/// A cohort's whole definition, in words: where its members come from, its criteria, the
/// databases it searches, and the patients added or removed manually (by pseudonym).
#[derive(Debug, Default, Serialize)]
pub struct Definition {
    /// `within`, `union`, `intersection`, `difference`; empty: every database it searches.
    pub op: String,
    pub sources: Vec<SourceRef>,
    /// The source in words: "Subcohort of “A”", "In “A” or “B”" …
    pub from: String,
    pub include: String,
    pub exclude: Vec<String>,
    /// The databases it searches, by name (empty: every one its users may search).
    pub databases: Vec<String>,
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
    /// The databases it searches (ids; empty: every one its user may search).
    pub databases: Vec<i64>,
    pub source: Option<Source>,
    /// A subcohort's parent.
    pub parent: Option<i64>,
    pub definition: Definition,
    pub members: i64,
    pub created_at: f64,
    pub updated_at: f64,
    pub refreshed_at: Option<f64>,
    /// The version of each database when its members were last found.
    pub source_versions: BTreeMap<String, String>,
    pub can_edit: bool,
    pub is_owner: bool,
    /// Who else may see it (who manages it only sees this).
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

fn object(id: i64) -> String {
    format!("cohort:{id}")
}

/// (may read, may edit, may manage: share and delete it)
pub(super) fn access(c: &Connection, user: &User, id: i64) -> anyhow::Result<(bool, bool, bool)> {
    let exists: Option<i64> = c.query_row("SELECT id FROM cohorts WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    if exists.is_none() {
        anyhow::bail!("no cohort {id}");
    }
    let o = object(id);
    Ok((
        crate::grants::can(c, user, &o, "view")?,
        crate::grants::can(c, user, &o, "edit")?,
        crate::grants::can(c, user, &o, "manage")?,
    ))
}

pub(super) fn readable(c: &Connection, user: &User, id: i64) -> anyhow::Result<(bool, bool)> {
    let (r, e, o) = access(c, user, id)?;
    if !r {
        anyhow::bail!("no cohort {id}");
    }
    Ok((e, o))
}

pub(super) fn editable(c: &Connection, user: &User, id: i64) -> anyhow::Result<bool> {
    let (_, can_edit, manages) = access(c, user, id)?;
    if !can_edit {
        anyhow::bail!("this cohort is shared with you to read only");
    }
    Ok(manages)
}

/// Patients as definitions and the history show them: by pseudonym, in order.
pub(super) fn patient_ids(c: &Connection, ids: impl IntoIterator<Item = i64>) -> anyhow::Result<Vec<String>> {
    let mut st = c.prepare("SELECT pseudonym FROM patients WHERE id = ?1")?;
    let mut out: Vec<String> = vec![];
    for id in ids {
        let p: Option<Option<String>> = st.query_row([id], |r| r.get(0)).optional()?;
        out.push(p.flatten().unwrap_or_else(|| format!("#{id}")));
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

#[allow(clippy::too_many_arguments)]
pub(super) fn definition(
    c: &Connection,
    cfg: &Config,
    id: i64,
    criteria: &Criteria,
    databases: &[i64],
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
    for db in databases {
        let name: Option<String> =
            c.query_row("SELECT name FROM databases WHERE id = ?1", [db], |r| r.get(0)).optional()?;
        d.databases.push(name.unwrap_or_else(|| "(removed)".into()));
    }
    d.added = patient_ids(c, members::manual(cfg, c, id)?)?;
    d.removed = patient_ids(c, members::removed(cfg, c, id)?)?;
    Ok(d)
}

pub(super) fn load(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<Cohort> {
    let (can_edit, manages) = readable(c, user, id)?;
    let (mut co, origin) = c.query_row(
        "SELECT c.id, c.name, c.description, c.owner, COALESCE(u.username, ''), c.criteria, c.created_at, c.updated_at,
                c.refreshed_at, c.databases, c.source, c.origin, c.source_versions
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
                    databases: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
                    parent: source.as_ref().filter(|s| s.op == "within").and_then(|s| s.of.first().copied()),
                    source,
                    definition: Definition::default(),
                    created_at: r.get(6)?,
                    updated_at: r.get(7)?,
                    refreshed_at: r.get(8)?,
                    source_versions: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
                    members: 0,
                    can_edit,
                    is_owner: manages,
                    shares: None,
                },
                r.get::<_, String>(11)?,
            ))
        },
    )?;
    co.members = members::count(cfg, c, id)?;
    co.definition = definition(c, cfg, id, &co.criteria, &co.databases, co.source.as_ref(), &origin)?;
    if manages {
        let mut shares: BTreeMap<i64, Share> = BTreeMap::new();
        for (uid, name, perm) in crate::grants::of_object(c, &object(id))? {
            if Some(uid) == co.owner || uid == user.id {
                continue;
            }
            let s = shares.entry(uid).or_insert(Share { user_id: uid, username: name, can_edit: false });
            s.can_edit |= perm != "view";
        }
        co.shares = Some(shares.into_values().collect());
    }
    Ok(co)
}

/// The cohorts a user may see: their own and those shared with them.
pub fn list(c: &Connection, cfg: &Config, user: &User) -> anyhow::Result<Vec<Cohort>> {
    let all: Vec<i64> = c
        .prepare("SELECT id FROM cohorts ORDER BY updated_at DESC, id DESC")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let visible = crate::grants::objects(c, user, "cohort", "view")?;
    all.into_iter()
        .filter(|id| visible.as_ref().is_none_or(|v| v.contains(id)))
        .map(|id| load(c, cfg, user, id))
        .collect()
}

pub fn get(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<Cohort> {
    load(c, cfg, user, id)
}

/// The members of a cohort, as the patient list shows them, with how and when each was added.
pub fn members(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    limit: usize,
    offset: usize,
) -> anyhow::Result<(usize, Vec<serde_json::Value>)> {
    members_with(c, cfg, user, id, |ids| patient::list_in(c, cfg, user, "", &[], Some(ids), limit, offset))
}

/// Every member, as [`members`] lists them (exports: no pages).
pub(super) fn all_members(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
) -> anyhow::Result<Vec<serde_json::Value>> {
    Ok(members_with(c, cfg, user, id, |ids| {
        let rows = patient::list_all_in(c, cfg, user, ids)?;
        Ok((rows.len(), rows))
    })?
    .1)
}

fn members_with(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    list: impl FnOnce(&HashSet<i64>) -> anyhow::Result<(usize, Vec<PatientRow>)>,
) -> anyhow::Result<(usize, Vec<serde_json::Value>)> {
    readable(c, user, id)?;
    let how = members::how(cfg, c, id)?;
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
pub fn member_ids(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<HashSet<i64>> {
    readable(c, user, id)?;
    members::ids(cfg, c, id)
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
pub fn readable_source_members(c: &Connection, cfg: &Config, user: &User, s: &Source) -> anyhow::Result<HashSet<i64>> {
    for x in s.of.iter().filter(|x| !s.frozen.contains_key(x)) {
        if !access(c, user, *x)?.0 {
            let name = s.names.get(x).cloned().unwrap_or_else(|| x.to_string());
            anyhow::bail!(
                "“{name}”, which this cohort is made from, is no longer shared with you: it cannot be followed"
            );
        }
    }
    source_members(c, cfg, s)
}

/// The patients a source gives: its cohorts' members now (or as kept when one was deleted).
pub fn source_members(c: &Connection, cfg: &Config, s: &Source) -> anyhow::Result<HashSet<i64>> {
    let mut sets = vec![];
    for x in &s.of {
        sets.push(match s.frozen.get(x) {
            Some(kept) => kept.iter().copied().collect(),
            None => members::ids(cfg, c, *x)?,
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

/// The patients a definition selects before its manual changes, with the databases each came
/// from: those of its source (every patient of its databases without one) who meet its
/// criteria.  Without criteria: those of its source, or nobody (a cohort of patients added
/// manually).
pub(super) fn selected(
    c: &Connection,
    cfg: &Config,
    user: &User,
    criteria: &Criteria,
    databases: &[i64],
    source: Option<&Source>,
) -> anyhow::Result<Hits> {
    let from = source.map(|s| readable_source_members(c, cfg, user, s)).transpose()?;
    if criteria.is_empty() {
        let Some(from) = from else { return Ok(HashMap::new()) };
        return from
            .into_iter()
            .map(|p| Ok((p, Hit { why: vec![], from: crate::persons::databases_of(c, p)?.into_iter().collect() })))
            .collect();
    }
    let found = evaluate(c, cfg, user, criteria, Some(databases))?;
    Ok(found.into_iter().filter(|(p, _)| from.as_ref().is_none_or(|f| f.contains(p))).collect())
}

/// A stored cohort's selection (its criteria, databases and source as saved).
pub(super) fn selected_by(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<Hits> {
    let (criteria, databases, source): (String, String, Option<String>) =
        c.query_row("SELECT criteria, databases, source FROM cohorts WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    let criteria: Criteria = serde_json::from_str(&criteria).unwrap_or_default();
    let databases: Vec<i64> = serde_json::from_str(&databases).unwrap_or_default();
    let source: Option<Source> = source.and_then(|s| serde_json::from_str(&s).ok());
    criteria.check_reader(user)?;
    selected(c, cfg, user, &criteria, &databases, source.as_ref())
}

/// Adds patients who are not members yet: those the definition selects (`sel`) as found by it,
/// with the databases it found them in, the others as added manually, with the databases that
/// link them now.  Patients removed manually before are no longer kept out.  Returns those
/// added, by how.
pub(super) fn add_members(
    c: &Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    patients: &[i64],
    sel: &Hits,
) -> anyhow::Result<(Vec<i64>, Vec<i64>)> {
    let known: HashSet<i64> =
        c.query_in("SELECT id FROM patients WHERE id IN ({})", patients, |r| r.get(0))?.into_iter().collect();
    let current = members::ids(cfg, c, id)?;
    let mut seen = HashSet::new();
    let mut rows: Vec<(i64, &str, BTreeSet<i64>)> = vec![];
    for p in patients.iter().copied().filter(|p| known.contains(p) && !current.contains(p) && seen.insert(*p)) {
        match sel.get(&p) {
            Some(h) => rows.push((p, "criteria", h.from.clone())),
            None => rows.push((p, "manual", crate::persons::databases_of(c, p)?.into_iter().collect())),
        }
    }
    let added = members::add(cfg, c, id, &rows, (user.id > 0).then_some(user.id))?;
    let (by_def, manual): (Vec<i64>, Vec<i64>) = added.into_iter().partition(|p| sel.contains_key(p));
    Ok((by_def, manual))
}

pub(super) fn store_source(s: Option<&Source>) -> anyhow::Result<Option<String>> {
    Ok(s.map(serde_json::to_string).transpose()?)
}
