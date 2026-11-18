//! The connected databases (docs/design/connected-databases.md): connecting one, its status,
//! refreshing what Medorama keeps of it, and removing it with every trace of it.
//!
//! Connecting checks the database, makes its data key and its workspace folder, grants it to
//! the admin who connected it, reads its catalog (the names and units of its variables) and its
//! subject index (its persons, linked, and their encounters): one pass over its persons and one
//! over its visits; no entry is counted and no file is read.  A check (at start, every minute)
//! marks it online or offline; a changed data version reads its catalog and subjects again.  Removing it is six steps
//! (`remove`), each one repeatable, so that a removal a crash interrupts finishes when Medorama
//! starts again ([`resume_removals`]).

use std::collections::HashMap;

use crate::config::Config;
use crate::db::{audit, now};
use crate::sources::{NewSource, Source};
use crate::sql::{params, Connection, OptionalExtension};
use crate::users::User;

/// A connected database with its status.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Database {
    #[serde(flatten)]
    pub source: Source,
    pub status: String,
    pub status_error: String,
    pub checked_at: Option<f64>,
    pub data_version: String,
    pub variables_version: String,
}

fn status_of(c: &Connection, s: Source) -> anyhow::Result<Database> {
    let (status, status_error, checked_at, data_version, variables_version) = c.query_row(
        "SELECT status, status_error, checked_at, data_version, variables_version FROM databases WHERE id = ?1",
        [s.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )?;
    Ok(Database { source: s, status, status_error, checked_at, data_version, variables_version })
}

/// Every connected database (those being removed left out), by name.
pub fn list(c: &Connection) -> anyhow::Result<Vec<Database>> {
    crate::sources::list(c)?.into_iter().map(|s| status_of(c, s)).collect()
}

pub fn get(c: &Connection, id: i64) -> anyhow::Result<Option<Database>> {
    crate::sources::get(c, id)?.map(|s| status_of(c, s)).transpose()
}

/// The ids of the databases a user may `permission` (search, view …), online or not.
pub fn granted(c: &Connection, user: &User, permission: &str) -> anyhow::Result<Vec<i64>> {
    let all: Vec<i64> = crate::sources::list(c)?.into_iter().map(|s| s.id).collect();
    crate::grants::databases(c, user, permission, &all)
}

/// The online databases of those given.
pub fn online(c: &Connection, of: &[i64]) -> anyhow::Result<Vec<i64>> {
    let mut out = vec![];
    for id in of {
        let s: Option<String> =
            c.query_row("SELECT status FROM databases WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        if s.as_deref() == Some("online") {
            out.push(*id);
        }
    }
    Ok(out)
}

/// The version a database says its data are at: its version query's answer, else a count of
/// its persons and recordings (so that a change is still noticed; its entries are not counted,
/// which on a large database would read them all at every check).
fn version_of(s: &Source, said: &str) -> anyhow::Result<String> {
    if !said.is_empty() {
        return Ok(said.to_string());
    }
    let db = s.db()?;
    let v = s.views();
    let count = |sql: &str| -> anyhow::Result<i64> {
        if sql.trim().is_empty() {
            return Ok(0);
        }
        db.query_row_native(&format!("SELECT COUNT(*) FROM ({sql}) v"), [], |r| r.get::<_, i64>(0))
            .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))
    };
    let recordings = if s.has_recordings() { count(&v.recordings)? } else { 0 };
    Ok(format!("count:{}:{recordings}", count(&v.persons)?))
}

/// Connect a database (admins): checked first (each view must answer), then kept with its data
/// key and workspace folder, granted to the admin who connected it, its variables read and its
/// persons linked.  A password given here is kept wrapped by the master key, never in a file.
pub fn connect(cfg: &Config, c: &Connection, user: &User, n: &NewSource, password: &str) -> anyhow::Result<i64> {
    if !user.is_admin() {
        anyhow::bail!("only admins connect databases");
    }
    n.validate()?;
    let mut n = n.clone();
    n.url = crate::sources::checked_url(cfg, &n.url)?;
    let name = n.name.trim().to_string();
    let taken: Option<i64> =
        c.query_row("SELECT id FROM databases WHERE lower(name) = lower(?1)", [&name], |r| r.get(0)).optional()?;
    if taken.is_some() {
        anyhow::bail!("a database named {name} is connected");
    }
    // checked with the password sealed for this check alone, then sealed for its id
    let trial =
        if password.is_empty() { String::new() } else { crate::workspace::seal_secret("password:0", password)? };
    let mut s = n.source(trial);
    let checked = s.check()?;
    let version = version_of(&s, &checked.version)?;
    // a demo database to a demo installation only, a site's to a site's (installation.rs)
    n.settings.demo = s.is_demo();
    crate::installation::admits(c, &name, n.settings.demo)?;
    crate::sources::forget(&s);
    let id = c.insert(
        "INSERT INTO databases (name, kind, url, password_file, ca_file, settings, status, checked_at, data_version,
                                created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'online', ?7, ?8, ?9, ?10)",
        params![
            name,
            n.kind,
            s.url,
            s.password_file,
            s.ca_file,
            serde_json::to_string(&n.settings)?,
            now(),
            version,
            (user.id > 0).then_some(user.id),
            now()
        ],
    )?;
    s.id = id;
    if !password.is_empty() {
        s.password_wrapped = crate::workspace::seal_secret(&format!("password:{id}"), password)?;
        c.execute("UPDATE databases SET password_wrapped = ?1 WHERE id = ?2", params![s.password_wrapped, id])?;
    }
    crate::workspace::create_key(c, crate::workspace::loaded()?, &crate::workspace::database_owner(id))?;
    crate::workspace::make_dir(&crate::workspace::database_dir(cfg, id))?;
    crate::grants::grant_all(c, user, &format!("db:{id}"))?;
    audit(c, Some(user), "connect", &name, &format!("{} {}", n.kind, crate::db::shown_url(&s.url)));
    if let Err(e) = refresh_content(cfg, c, &s, &version, Some(user)) {
        // connected, but what Medorama keeps of it is not read yet: a refresh reads it
        audit(c, Some(user), "refresh_failed", &name, &e.to_string());
        return Err(e.context(format!("{name} is connected, but reading it failed (refresh it)")));
    }
    Ok(id)
}

/// Read again what Medorama keeps of a database: its catalog, its subjects and their encounters.
/// Its recordings are read when a patient is opened (recordings.rs), never here.
fn refresh_content(cfg: &Config, c: &Connection, s: &Source, version: &str, user: Option<&User>) -> anyhow::Result<()> {
    crate::variables::read(cfg, c, s, version)?;
    crate::persons::sync(cfg, c, s, user)?;
    // what the privacy gate's dictionary is built from changed: it is built again
    c.execute("UPDATE data_version SET version = version + 1, identifiers = identifiers + 1", [])?;
    Ok(())
}

/// Check a database now: online or offline, and its data version.  A changed version reads its
/// catalog and subjects again.  Returns whether it is online.
pub fn check(cfg: &Config, c: &Connection, id: i64) -> anyhow::Result<bool> {
    let Some(d) = get(c, id)? else { anyhow::bail!("no database {id}") };
    let s = d.source;
    match s.check().and_then(|k| version_of(&s, &k.version)) {
        Ok(version) => {
            c.execute(
                "UPDATE databases SET status = 'online', status_error = '', checked_at = ?1, data_version = ?2
                 WHERE id = ?3 AND status != 'removing'",
                params![now(), version, id],
            )?;
            if version != d.variables_version && refresh_due(cfg, id, &version) {
                refresh_content(cfg, c, &s, &version, None)?;
            }
            Ok(true)
        }
        Err(e) => {
            crate::sources::forget(&s);
            c.execute(
                "UPDATE databases SET status = 'offline', status_error = ?1, checked_at = ?2
                 WHERE id = ?3 AND status != 'removing'",
                params![e.to_string(), now(), id],
            )?;
            Ok(false)
        }
    }
}

/// Whether a changed version is refreshed now: always when the database states a version;
/// when the version is a count of its rows (no version query), which a live database changes
/// at every check, at most once per `check_interval_s` (refreshing re-links every person).
fn refresh_due(cfg: &Config, id: i64, version: &str) -> bool {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static LAST: OnceLock<Mutex<HashMap<i64, Instant>>> = OnceLock::new();
    if !version.starts_with("count:") {
        return true;
    }
    let mut last = LAST.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let interval = Duration::from_secs(cfg.check_interval_s.max(60));
    if last.get(&id).is_some_and(|t| t.elapsed() < interval) {
        return false;
    }
    last.insert(id, Instant::now());
    true
}

/// Check every database (the server, every minute): how many are online, offline.
pub fn check_all(cfg: &Config, c: &Connection) -> anyhow::Result<(usize, usize)> {
    let (mut on, mut off) = (0, 0);
    for d in list(c)? {
        match check(cfg, c, d.source.id) {
            Ok(true) => on += 1,
            Ok(false) => off += 1,
            Err(e) => {
                eprintln!("medorama: {}: {e}", d.source.name);
                off += 1
            }
        }
    }
    Ok((on, off))
}

/// Refresh a database on demand (who manages it): checked, and its catalog and subjects read again.
pub fn refresh(cfg: &Config, c: &Connection, user: &User, id: i64) -> anyhow::Result<()> {
    crate::grants::require(c, user, &format!("db:{id}"), "manage")?;
    let Some(d) = get(c, id)? else { anyhow::bail!("no database {id}") };
    if !check(cfg, c, id)? {
        let why = get(c, id)?.map(|d| d.status_error).unwrap_or_default();
        anyhow::bail!("{} does not answer: {why}", d.source.name);
    }
    let version: String = c.query_row("SELECT data_version FROM databases WHERE id = ?1", [id], |r| r.get(0))?;
    refresh_content(cfg, c, &d.source, &version, Some(user))?;
    audit(c, Some(user), "refresh", &d.source.name, "");
    Ok(())
}

/// Replace a connected database's views (kind `views`; an admin who manages it): checked, kept,
/// and the database read again with them.  Its patients keep their numbers: a person is linked
/// by the same identifiers, whatever view gives them.
pub fn set_views(
    cfg: &Config,
    c: &Connection,
    user: &User,
    id: i64,
    views: &crate::sources::Settings,
) -> anyhow::Result<()> {
    if !user.is_admin() {
        anyhow::bail!("only admins change a database's views");
    }
    crate::grants::require(c, user, &format!("db:{id}"), "manage")?;
    let Some(d) = get(c, id)? else { anyhow::bail!("no database {id}") };
    let mut s = d.source;
    if s.kind != "views" {
        anyhow::bail!("{} is a database of kind {}: only a database of kind views has views", s.name, s.kind);
    }
    let v = views.clone();
    (s.settings.persons, s.settings.visits, s.settings.entries, s.settings.recordings, s.settings.version) =
        (v.persons, v.visits, v.entries, v.recordings, v.version);
    (s.settings.stays, s.settings.variables, s.settings.has) = (v.stays, v.variables, v.has);
    crate::sources::forget(&s);
    let checked = s.check()?;
    let version = version_of(&s, &checked.version)?;
    c.execute(
        "UPDATE databases SET settings = ?1, status = 'online', status_error = '', checked_at = ?2, data_version = ?3
         WHERE id = ?4",
        params![serde_json::to_string(&s.settings)?, now(), version, id],
    )?;
    audit(c, Some(user), "views", &s.name, "");
    refresh_content(cfg, c, &s, &version, Some(user))
}

/// Count a database's variables (who manages it): how many entries and persons hold each, one
/// grouped query over its entries (the Databases tab's **Count now**; never done when connecting).
pub fn count_variables(cfg: &Config, c: &Connection, user: &User, id: i64) -> anyhow::Result<usize> {
    crate::grants::require(c, user, &format!("db:{id}"), "manage")?;
    let Some(d) = get(c, id)? else { anyhow::bail!("no database {id}") };
    if d.status != "online" {
        anyhow::bail!("{} is offline", d.source.name);
    }
    let n = crate::variables::count(cfg, c, &d.source)?;
    audit(c, Some(user), "count", &d.source.name, &format!("{n} variables counted"));
    Ok(n)
}

/// Read every recording a database lists, check their files and queue the new or changed ones
/// for indexing (who manages it; `medorama databases index`): what connecting no longer does.
pub fn index_recordings(
    cfg: &Config,
    c: &Connection,
    user: &User,
    id: i64,
) -> anyhow::Result<crate::recordings::SyncReport> {
    crate::grants::require(c, user, &format!("db:{id}"), "manage")?;
    let Some(d) = get(c, id)? else { anyhow::bail!("no database {id}") };
    if d.status != "online" {
        anyhow::bail!("{} is offline", d.source.name);
    }
    let rep = crate::recordings::sync(cfg, c, &d.source)?;
    audit(
        c,
        Some(user),
        "index_all",
        &d.source.name,
        &format!("{} recordings read, {} missing", rep.recordings, rep.missing),
    );
    Ok(rep)
}

/// Remove a database (who manages it) and every trace of it (docs/design/databases-only.md,
/// "Removing a database").  The database itself is never touched.
pub fn remove(cfg: &Config, c: &Connection, user: &User, id: i64) -> anyhow::Result<()> {
    crate::grants::require(c, user, &format!("db:{id}"), "manage")?;
    let name: String = c
        .query_row("SELECT name FROM databases WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("no database {id}"))?;
    // 1. no longer offered, searched or opened
    c.execute("UPDATE databases SET status = 'removing' WHERE id = ?1", [id])?;
    finish_removal(cfg, c, id)?;
    audit(c, Some(user), "remove", &name, "its key destroyed; its workspace folder, cohort rows and links removed");
    Ok(())
}

/// Steps 2 to 6 of a removal; each can be repeated.
fn finish_removal(cfg: &Config, c: &Connection, id: i64) -> anyhow::Result<()> {
    let derived: Vec<i64> = c
        .prepare("SELECT derived_id FROM derived_sources WHERE database_id = ?1")?
        .query_map([id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    // 2. the keys first: from now on nothing of it can be read
    crate::workspace::shred(c, &crate::workspace::database_owner(id))?;
    for d in &derived {
        crate::workspace::shred(c, &crate::workspace::derived_owner(*d))?;
    }
    // 3. its workspace folder, and those of what was derived from it
    crate::workspace::remove_dir(cfg, &crate::workspace::database_dir(cfg, id))?;
    for d in &derived {
        crate::workspace::remove_dir(cfg, &crate::workspace::derived_dir(cfg, *d))?;
    }
    // 4. every cohort row it (or what was derived from it) contributed to
    crate::cohorts::purge_database(cfg, c, id, &derived)?;
    // 5. its patient links, and the patients no database links any more
    c.execute("DELETE FROM patient_keys WHERE database_id = ?1", [id])?;
    crate::persons::sweep(c)?;
    crate::cohorts::forget_unlinked(cfg, c)?;
    // 6. its row, grants, jobs and visit pseudonyms; what this server read of it
    c.execute("DELETE FROM jobs WHERE database_id = ?1", [id])?;
    c.execute("DELETE FROM visit_pseudonyms WHERE database_id = ?1", [id])?;
    for d in &derived {
        c.execute("DELETE FROM derived WHERE id = ?1", [d])?;
    }
    crate::grants::remove_object(c, &format!("db:{id}"))?;
    if let Some(s) = crate::sources::get_any(c, id)? {
        crate::sources::forget(&s);
    }
    c.execute("DELETE FROM databases WHERE id = ?1", [id])?;
    Ok(())
}

/// Finish the removals a crash interrupted (the server and every command, when they start).
pub fn resume_removals(cfg: &Config, c: &Connection) -> anyhow::Result<usize> {
    let ids: Vec<i64> = c
        .prepare("SELECT id FROM databases WHERE status = 'removing'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in &ids {
        finish_removal(cfg, c, *id)?;
        audit(c, None, "remove", &format!("database {id}"), "finished after an interruption");
    }
    Ok(ids.len())
}

/// A database as a user sees it in the Databases tab: no secrets, and its counts only for those
/// granted it.
#[derive(Debug, serde::Serialize)]
pub struct Shown {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub engine: &'static str,
    /// The URL without its password (who manages it), else empty.
    pub url: String,
    pub status: String,
    pub status_error: String,
    pub checked_at: Option<f64>,
    pub version: String,
    /// The recordings Medorama keeps of it (those opened or indexed).
    pub recordings: i64,
    /// Whether it lists recordings (a recording database).
    pub has_recordings: bool,
    /// The user's strongest permission on it, if any.
    pub permission: Option<&'static str>,
    pub persons: Option<i64>,
    pub patients: Option<i64>,
    pub conflicts: Option<i64>,
    pub variables: Option<usize>,
    pub created_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<crate::sources::Settings>,
}

/// The databases a user is shown: those granted to them (admins: every one, to grant them),
/// online ones first.
pub fn shown(cfg: &Config, c: &Connection, user: &User) -> anyhow::Result<Vec<Shown>> {
    let mut out = vec![];
    for d in list(c)? {
        let s = &d.source;
        let object = format!("db:{}", s.id);
        let permission = crate::grants::strongest(c, user, &object)?;
        if permission.is_none() && !user.is_admin() {
            continue;
        }
        let manage = permission == Some("manage");
        let (persons, patients, conflicts, variables) = if permission.is_some() {
            let (a, b, k) = crate::persons::counts(cfg, c, s.id).unwrap_or((0, 0, 0));
            let v = crate::variables::of(cfg, c, s.id).map(|v| v.len()).unwrap_or(0);
            (Some(a), Some(b), Some(k), Some(v))
        } else {
            (None, None, None, None)
        };
        out.push(Shown {
            id: s.id,
            name: s.name.clone(),
            kind: s.kind.clone(),
            engine: if s.dialect() == crate::sql::Dialect::Sqlite { "SQLite" } else { "PostgreSQL" },
            url: if manage { crate::db::shown_url(&s.url) } else { String::new() },
            status: d.status.clone(),
            status_error: if manage { d.status_error.clone() } else { String::new() },
            checked_at: d.checked_at,
            version: d.data_version.clone(),
            recordings: if permission.is_some() { crate::recordings::count(cfg, c, s.id).unwrap_or(0) } else { 0 },
            has_recordings: permission.is_some() && !s.views().recordings.trim().is_empty(),
            permission,
            persons,
            patients,
            conflicts,
            variables,
            created_at: s.created_at,
            settings: manage.then(|| s.settings.clone()),
        });
    }
    out.sort_by(|a, b| (a.status != "online").cmp(&(b.status != "online")).then(a.name.cmp(&b.name)));
    Ok(out)
}

/// A CSV cell, made safe for spreadsheets: text a spreadsheet would read as a formula (`=`,
/// `+`, `-`, `@` …) is prefixed with `'`, numbers stay as they are.
pub(crate) fn csv_cell(v: &str) -> std::borrow::Cow<'_, str> {
    let formula = v.starts_with(['=', '+', '-', '@', '\t', '\r']);
    if formula && v.trim().parse::<f64>().is_err() {
        std::borrow::Cow::Owned(format!("'{v}"))
    } else {
        std::borrow::Cow::Borrowed(v)
    }
}
