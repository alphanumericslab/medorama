//! Changing cohorts: making, cloning, changing, sharing, deleting, refreshing them; the
//! versions of the databases they were made from.

use super::*;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NewCohort {
    pub name: String,
    pub description: String,
    pub criteria: Criteria,
    /// The databases it searches (none: every one its user may search).
    pub databases: Vec<i64>,
    /// A subcohort's parent or the cohorts merged (none: every patient of its databases).
    pub source: Option<Source>,
    /// Patients to add now (none: all those the definition selects).
    pub patients: Vec<i64>,
}

/// The databases a cohort is to search: those named that the user may search.
fn checked_databases(c: &Connection, user: &User, dbs: &[i64]) -> anyhow::Result<Vec<i64>> {
    let mut out = vec![];
    for d in dbs {
        if !crate::grants::can(c, user, &format!("db:{d}"), "search")? {
            anyhow::bail!("no database {d} to search");
        }
        if !out.contains(d) {
            out.push(*d);
        }
    }
    out.sort_unstable();
    Ok(out)
}

pub fn create(c: &Connection, cfg: &Config, user: &User, n: &NewCohort) -> anyhow::Result<i64> {
    let name = n.name.trim();
    if name.is_empty() {
        anyhow::bail!("a cohort needs a name");
    }
    let source = n.source.as_ref().map(|s| checked_source(c, user, s)).transpose()?;
    let databases = checked_databases(c, user, &n.databases)?;
    let criteria = n.criteria.as_written_by(c, cfg, user)?;
    let sel = selected(c, cfg, user, &criteria, &databases, source.as_ref())?;
    let t = now();
    let id = c.insert(
        "INSERT INTO cohorts (name, description, owner, criteria, databases, source, created_at, updated_at, refreshed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?7)",
        params![
            name,
            n.description.trim(),
            (user.id > 0).then_some(user.id),
            serde_json::to_string(&criteria)?,
            serde_json::to_string(&databases)?,
            store_source(source.as_ref())?,
            t
        ],
    )?;
    crate::grants::grant_all(c, user, &format!("cohort:{id}"))?;
    c.execute(
        "UPDATE cohorts SET source_versions = ?1 WHERE id = ?2",
        params![serde_json::to_string(&source_versions(c)?)?, id],
    )?;
    let mut patients: Vec<i64> = if n.patients.is_empty() { sel.keys().copied().collect() } else { n.patients.clone() };
    patients.sort();
    let (by_def, manual) = add_members(c, cfg, user, id, &patients, &sel)?;
    let mut detail = format!("made: {}", n_patients(by_def.len() + manual.len()));
    if let Some(s) = &source {
        let names: Vec<String> = s.of.iter().filter_map(|x| s.names.get(x).cloned()).collect();
        detail += &format!("; {}", source_words(&s.op, &names));
    }
    if !manual.is_empty() {
        detail += &format!("; added manually: {}", some(&patient_ids(c, manual)?));
    }
    audit(c, Some(user), "cohort_create", &id.to_string(), &detail);
    Ok(id)
}

/// A copy of a cohort the user may read, theirs: the same definition and members.
pub fn clone(c: &Connection, cfg: &Config, user: &User, id: i64, name: &str) -> anyhow::Result<i64> {
    let co = load(c, cfg, user, id)?;
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("a cohort needs a name");
    }
    let t = now();
    let new = c.insert(
        "INSERT INTO cohorts (name, description, owner, criteria, databases, source, origin, created_at, updated_at,
                              refreshed_at, source_versions)
         SELECT ?1, description, ?2, criteria, databases, source, ?3, ?4, ?4, refreshed_at, source_versions FROM cohorts WHERE id = ?5",
        params![name, (user.id > 0).then_some(user.id), format!("Cloned from “{}”", co.name), t, id],
    )?;
    crate::grants::grant_all(c, user, &format!("cohort:{new}"))?;
    members::copy(cfg, c, id, new)?;
    audit(
        c,
        Some(user),
        "cohort_create",
        &new.to_string(),
        &format!("cloned from “{}”: {}", co.name, n_patients(co.members as usize)),
    );
    Ok(new)
}

/// Patients found by the patient list's search (Patient Catalog).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PatientSearch {
    pub q: String,
    pub has: Vec<String>,
    /// Within a cohort's members.
    pub cohort: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Change {
    pub name: Option<String>,
    pub description: Option<String>,
    pub criteria: Option<Criteria>,
    /// The databases it searches, replacing the list (empty: every one).
    pub databases: Option<Vec<i64>>,
    /// Patients to add.  Those the definition selects count as found by it, the others as
    /// added manually (the definition lists them).
    pub add: Vec<i64>,
    /// And every patient this search finds.
    pub add_search: Option<PatientSearch>,
    /// Members to take out.  Those the definition selected stay out when it is refreshed (the
    /// definition lists them); those added manually are simply no longer added.
    pub remove: Vec<i64>,
    /// Who manages it only: who else may see it, replacing the list.
    pub shares: Option<Vec<Share>>,
}

pub fn change(c: &Connection, cfg: &Config, user: &User, id: i64, ch: &Change) -> anyhow::Result<Cohort> {
    let manages = editable(c, user, id)?;
    let before = load(c, cfg, user, id)?;
    let mut log: Vec<String> = vec![];
    if let Some(n) = &ch.name {
        let n = n.trim();
        if n.is_empty() {
            anyhow::bail!("a cohort needs a name");
        }
        if n != before.name {
            c.execute("UPDATE cohorts SET name = ?1 WHERE id = ?2", params![n, id])?;
            log.push(format!("renamed “{}” → “{n}”", before.name));
        }
    }
    if let Some(d) = &ch.description {
        if d.trim() != before.description {
            c.execute("UPDATE cohorts SET description = ?1 WHERE id = ?2", params![d.trim(), id])?;
            log.push("description changed".into());
        }
    }
    if let Some(q) = &ch.criteria {
        let q = &q.as_written_by(c, cfg, user)?;
        if *q != before.criteria {
            c.execute("UPDATE cohorts SET criteria = ?1 WHERE id = ?2", params![serde_json::to_string(q)?, id])?;
            let w = criteria_words(q);
            let inc = w["include"].as_str().unwrap_or("");
            let exc: Vec<&str> = w["exclude"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
            log.push(match (inc.is_empty(), exc.is_empty()) {
                (true, true) => "criteria removed".into(),
                _ => format!(
                    "criteria: {}{}",
                    if inc.is_empty() { "everyone" } else { inc },
                    if exc.is_empty() { String::new() } else { format!("; never: {}", exc.join("; ")) }
                ),
            });
        }
    }
    if let Some(dbs) = &ch.databases {
        let dbs = checked_databases(c, user, dbs)?;
        if dbs != before.databases {
            c.execute("UPDATE cohorts SET databases = ?1 WHERE id = ?2", params![serde_json::to_string(&dbs)?, id])?;
            log.push(if dbs.is_empty() {
                "searches every database".into()
            } else {
                format!("searches {} databases", dbs.len())
            });
        }
    }
    let mut add = ch.add.clone();
    if let Some(s) = &ch.add_search {
        let only = s.cohort.map(|x| member_ids(c, cfg, user, x)).transpose()?;
        add.extend(patient::matching_ids(c, cfg, user, &s.q, &s.has, only.as_ref())?);
    }
    if !add.is_empty() {
        let current = members::ids(cfg, c, id)?;
        add.retain(|p| !current.contains(p));
        add.sort();
        add.dedup();
    }
    if !add.is_empty() {
        let sel = selected_by(c, cfg, user, id)?;
        let (by_def, manual) = add_members(c, cfg, user, id, &add, &sel)?;
        if !manual.is_empty() {
            log.push(format!("added manually: {}", some(&patient_ids(c, manual)?)));
        }
        if !by_def.is_empty() {
            log.push(format!("added back (the definition selects them): {}", some(&patient_ids(c, by_def)?)));
        }
    }
    if !ch.remove.is_empty() {
        let how = members::how(cfg, c, id)?;
        let by = (user.id > 0).then_some(user.id);
        let (mut out, mut dropped) = (vec![], vec![]);
        for p in &ch.remove {
            match how.get(p).map(|h| h.0.as_str()) {
                Some("criteria") => out.push(*p),
                Some(_) => dropped.push(*p),
                None => {}
            }
        }
        members::remove(cfg, c, id, &out, true, by)?;
        members::remove(cfg, c, id, &dropped, false, by)?;
        if !out.is_empty() {
            log.push(format!("removed manually: {}", some(&patient_ids(c, out)?)));
        }
        if !dropped.is_empty() {
            log.push(format!("no longer added manually: {}", some(&patient_ids(c, dropped)?)));
        }
    }
    if let Some(shares) = &ch.shares {
        if !manages {
            anyhow::bail!("only who manages a cohort shares it");
        }
        let o = format!("cohort:{id}");
        for (uid, _, _) in crate::grants::of_object(c, &o)? {
            if uid != user.id && Some(uid) != before.owner {
                c.execute("DELETE FROM grants WHERE user_id = ?1 AND object = ?2", params![uid, o])?;
            }
        }
        for s in shares.iter().filter(|s| s.user_id != user.id) {
            crate::grants::grant(c, user, s.user_id, &o, if s.can_edit { "edit" } else { "view" })?;
        }
        let now_shared: Vec<String> = load(c, cfg, user, id)?
            .shares
            .unwrap_or_default()
            .into_iter()
            .map(|s| format!("{}{}", s.username, if s.can_edit { " (may edit)" } else { "" }))
            .collect();
        log.push(if now_shared.is_empty() {
            "not shared".into()
        } else {
            format!("shared with {}", now_shared.join(", "))
        });
    }
    if !log.is_empty() {
        c.execute("UPDATE cohorts SET updated_at = ?1 WHERE id = ?2", params![now(), id])?;
        audit(c, Some(user), "cohort_change", &id.to_string(), &log.join("; "));
    }
    load(c, cfg, user, id)
}

/// Deletes a cohort (who manages it): its key destroyed, then its file.  The cohorts made from
/// it keep its last members and follow those.
pub fn delete(c: &Connection, cfg: &Config, user: &User, id: i64) -> anyhow::Result<()> {
    let (_, _, manages) = access(c, user, id)?;
    if !manages {
        anyhow::bail!("only who manages a cohort deletes it");
    }
    let name: String = c.query_row("SELECT name FROM cohorts WHERE id = ?1", [id], |r| r.get(0))?;
    let mut kept: Vec<i64> = members::ids(cfg, c, id)?.into_iter().collect();
    kept.sort();
    let dependents: Vec<(i64, String)> = c
        .prepare("SELECT id, source FROM cohorts WHERE source IS NOT NULL AND id != ?1")?
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (dep, json) in dependents {
        let Ok(mut s) = serde_json::from_str::<Source>(&json) else { continue };
        if !s.of.contains(&id) || s.frozen.contains_key(&id) {
            continue;
        }
        s.frozen.insert(id, kept.clone());
        s.names.insert(id, name.clone());
        c.execute("UPDATE cohorts SET source = ?1 WHERE id = ?2", params![serde_json::to_string(&s)?, dep])?;
        audit(
            c,
            Some(user),
            "cohort_change",
            &dep.to_string(),
            &format!("“{name}”, which it is made from, was deleted: its last members are kept"),
        );
    }
    members::delete(cfg, c, id)?;
    crate::grants::remove_object(c, &format!("cohort:{id}"))?;
    c.execute("DELETE FROM cohorts WHERE id = ?1", [id])?;
    audit(c, Some(user), "cohort_delete", &id.to_string(), &name);
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Refresh {
    /// Patients the definition selects now who are not members (nor removed manually): who
    /// would join (a dry run), or who joined.
    pub added: Vec<i64>,
    /// Members it selected before who it no longer selects (members added manually stay).
    pub removed: Vec<i64>,
    pub dry_run: bool,
    /// A dry run's: the first of those who would join and leave, as the patient list shows
    /// them (for choosing whom to add).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub joining: Vec<crate::patient::PatientRow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub leaving: Vec<crate::patient::PatientRow>,
    /// Applied with the changes chosen (`join`, `remove`): those chosen that the definition no
    /// longer would add or take out (left as they are), and how many changes it would make now
    /// besides those chosen (not applied: a dry run shows them).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_applied: Vec<i64>,
    #[serde(skip_serializing_if = "is_zero")]
    pub not_chosen: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How a refresh is applied.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct RefreshOpts {
    /// Show what would change; change nothing.
    pub dry_run: bool,
    /// Add only these of the patients who would join (none: all of them).
    pub join: Option<Vec<i64>>,
    /// Whether members the definition no longer selects leave (by default they do); without,
    /// only new patients are added.
    pub leave: Option<bool>,
    /// Take out only these of the members who would leave (none: all of them): with `join`,
    /// the changes a dry run showed, applied as they still are.
    pub remove: Option<Vec<i64>>,
}

/// How many of those who would join and leave a dry run lists.
const LISTED: usize = 500;

/// The data version of each connected database as last checked (`offline` when it does not
/// answer).
pub fn source_versions(c: &Connection) -> anyhow::Result<BTreeMap<String, String>> {
    let mut v = BTreeMap::new();
    for d in crate::databases::list(c)? {
        let version = if d.status == "online" { d.data_version.clone() } else { "offline".into() };
        v.insert(d.source.name, if version.is_empty() { "-".into() } else { version });
    }
    Ok(v)
}

/// What the definition selects now, against the members; applied unless `dry_run`.
pub fn refresh(c: &Connection, cfg: &Config, user: &User, id: i64, dry_run: bool) -> anyhow::Result<Refresh> {
    refresh_with(c, cfg, user, id, &RefreshOpts { dry_run, ..Default::default() })
}

/// [`refresh`], as `opts` says: all the changes, or only (some of) the new patients.
pub fn refresh_with(c: &Connection, cfg: &Config, user: &User, id: i64, opts: &RefreshOpts) -> anyhow::Result<Refresh> {
    let dry_run = opts.dry_run;
    let (can_edit, _) = readable(c, user, id)?;
    let sel = selected_by(c, cfg, user, id)?;
    let how = members::how(cfg, c, id)?;
    let out = members::removed(cfg, c, id)?;
    let mut added: Vec<i64> = sel.keys().filter(|p| !how.contains_key(p) && !out.contains(p)).copied().collect();
    let mut removed: Vec<i64> =
        how.iter().filter(|(p, h)| h.0.as_str() == "criteria" && !sel.contains_key(p)).map(|(p, _)| *p).collect();
    added.sort();
    removed.sort();
    if dry_run {
        let rows = |ids: &[i64]| -> anyhow::Result<Vec<crate::patient::PatientRow>> {
            if ids.is_empty() {
                return Ok(vec![]);
            }
            let set: HashSet<i64> = ids.iter().copied().collect();
            Ok(crate::patient::list_in(c, cfg, user, "", &[], Some(&set), LISTED, 0)?.1)
        };
        let (joining, leaving) = (rows(&added)?, rows(&removed)?);
        return Ok(Refresh { added, removed, dry_run, joining, leaving, not_applied: vec![], not_chosen: 0 });
    }
    // the changes chosen, as the definition still makes them
    let (mut not_applied, mut not_chosen) = (vec![], 0);
    let mut only = |now: &mut Vec<i64>, chosen: &Option<Vec<i64>>| {
        if let Some(chosen) = chosen {
            let set: HashSet<i64> = chosen.iter().copied().collect();
            not_applied.extend(chosen.iter().filter(|p| !now.contains(p)).copied());
            not_chosen += now.iter().filter(|p| !set.contains(p)).count();
            now.retain(|p| set.contains(p));
        }
    };
    only(&mut added, &opts.join);
    if opts.leave == Some(false) {
        removed.clear();
    } else {
        only(&mut removed, &opts.remove);
    }
    not_applied.sort();
    not_applied.dedup();
    if !can_edit {
        anyhow::bail!("this cohort is shared with you to read only");
    }
    add_members(c, cfg, user, id, &added, &sel)?;
    members::remove(cfg, c, id, &removed, false, (user.id > 0).then_some(user.id))?;
    let before: BTreeMap<String, String> =
        serde_json::from_str(
            &c.query_row("SELECT source_versions FROM cohorts WHERE id = ?1", [id], |r| r.get::<_, String>(0))?,
        )
        .unwrap_or_default();
    let versions = source_versions(c)?;
    c.execute(
        "UPDATE cohorts SET refreshed_at = ?1, updated_at = ?1, source_versions = ?2 WHERE id = ?3",
        params![now(), serde_json::to_string(&versions)?, id],
    )?;
    let mut detail = format!("refreshed: {} joined, {} left", added.len(), removed.len());
    // the databases whose data changed since
    let changed: Vec<String> = versions
        .iter()
        .filter(|(k, v)| before.get(*k).is_some_and(|b| b != *v))
        .map(|(k, v)| format!("{k} {} → {v}", before[k]))
        .collect();
    if !changed.is_empty() {
        detail += &format!("; new data: {}", changed.join(", "));
    }
    if !added.is_empty() {
        detail += &format!("; joined: {}", some(&patient_ids(c, added.clone())?));
    }
    if !removed.is_empty() {
        detail += &format!("; left: {}", some(&patient_ids(c, removed.clone())?));
    }
    audit(c, Some(user), "cohort_refresh", &id.to_string(), &detail);
    Ok(Refresh { added, removed, dry_run, joining: vec![], leaving: vec![], not_applied, not_chosen })
}

/// A change to a cohort, as its history shows it.
#[derive(Debug, Serialize)]
pub struct Event {
    pub time: f64,
    pub user: String,
    pub what: String,
}

/// A cohort's history, latest first: who made, changed and refreshed it, and how.
pub fn history(c: &Connection, user: &User, id: i64) -> anyhow::Result<Vec<Event>> {
    readable(c, user, id)?;
    let mut st = c.prepare(
        "SELECT time, username, detail FROM audit
         WHERE target = ?1 AND event IN ('cohort_create', 'cohort_change', 'cohort_refresh') ORDER BY id DESC LIMIT 500",
    )?;
    let out = st
        .query_map([id.to_string()], |r| Ok(Event { time: r.get(0)?, user: r.get(1)?, what: r.get(2)? }))?
        .collect::<Result<_, _>>()?;
    Ok(out)
}
