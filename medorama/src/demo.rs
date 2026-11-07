//! The demo (docs/administration.md, "The demo"): invented patients, their databases, recordings
//! and files, made by `tools/make-demo-data.py`, loaded into a catalog as the source `DEMO`, with
//! a cohort `DEMO` that holds them and a connected OMOP database (`demo_omop`).  Deleting that
//! cohort (an admin) removes all of it from the catalog: what another source also has stays.
//! The demo's files are not touched.
//!
//! The installers make it only when asked (`tools/install-medorama.sh --demo`): they write the
//! data set into `.venv/share/medorama/demo` and its folder into `medorama.demo` next to the
//! programs, and `medorama init` then loads it.  `medorama demo add` and `medorama demo remove`
//! do the same for any catalog.

use crate::cohorts::{self, Criteria, Criterion, Expr, NewCohort};
use crate::config::Config;
use crate::db::audit;
use crate::imports::{Opts, Removal};
use crate::sql::{params, Connection, OptionalExtension};
use crate::tables::{self, Part, PartData};
use crate::users::User;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The source label of everything the demo adds, and its cohort's name.
pub const NAME: &str = "DEMO";
/// The demo cohort's origin (shown as how it was made); it marks the cohort as the demo's.
pub const ORIGIN: &str = "the demo: invented patients (deleting this cohort removes them from the catalog)";
/// The demo's connected database (its OMOP file).
pub const CONNECTION: &str = "demo_omop";
/// The file the installers write next to the programs: the demo's folder.
pub const CHOICE_FILE: &str = "medorama.demo";
/// The first line of the README.txt that `tools/make-demo-data.py` writes: a folder made by it.
const SIGNATURE: &str = "Make a synthetic demo data set for Medorama";

/// The demo folder the installer made (`medorama.demo` next to the programs), if any.
pub fn installed() -> Option<PathBuf> {
    crate::viewer::installer_choice(CHOICE_FILE).filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The demo cohort's id, when the demo is in the catalog.
pub fn cohort(c: &Connection) -> anyhow::Result<Option<i64>> {
    Ok(c.query_row("SELECT id FROM cohorts WHERE origin = ?1 ORDER BY id LIMIT 1", [ORIGIN], |r| r.get(0))
        .optional()?)
}

/// Whether the catalog holds any of the demo: its cohort, its batches or its connection.
pub fn present(c: &Connection) -> anyhow::Result<bool> {
    let batches: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM imports WHERE state = 'active' AND (source = ?1 OR name = ?2))",
        params![NAME, CONNECTION],
        |r| r.get(0),
    )?;
    Ok(batches || cohort(c)?.is_some() || crate::sources::by_name(c, CONNECTION)?.is_some())
}

/// What loading the demo added.
#[derive(Debug, Default, Serialize)]
pub struct Loaded {
    /// The tables imported, by file name, with their import's id.
    pub tables: Vec<(String, i64)>,
    pub patients: usize,
    pub encounters: usize,
    pub recordings: usize,
    pub items: usize,
    /// The OMOP database was connected.
    pub connected: bool,
    pub cohort: i64,
    /// The tables' warnings, by file name.
    pub warnings: Vec<String>,
}

/// Load the demo from `dir` (a folder made by `tools/make-demo-data.py`): its tables as the
/// source `DEMO`, its OMOP database, and the cohort `DEMO`, readable by every user.  Admins
/// only; refused when the catalog holds the demo already.  If a table fails, what was added is
/// removed again.
pub fn add(c: &mut Connection, cfg: &Config, user: &User, dir: &Path) -> anyhow::Result<Loaded> {
    if !user.is_admin() {
        anyhow::bail!("only admins add the demo");
    }
    if present(c)? {
        anyhow::bail!("the demo is in this catalog already (the cohort {NAME}): delete that cohort first");
    }
    let dir = PathBuf::from(recording_engine::paths::absolute(&dir.to_string_lossy()));
    let readme = std::fs::read_to_string(dir.join("README.txt")).unwrap_or_default();
    let databases = dir.join("databases");
    if !readme.starts_with(SIGNATURE) || !databases.is_dir() {
        anyhow::bail!(
            "{} is not a demo folder made by tools/make-demo-data.py (it has no README.txt and databases/ from it)",
            dir.display()
        );
    }
    if !cfg.import_allowed(&dir) {
        anyhow::bail!(
            "{} is outside the catalog's data_roots: add it there (medorama.toml) to load the demo",
            dir.display()
        );
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&databases)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    // the patients, then their encounters, then the rest
    let rank = |p: &Path| match p.file_name().map(|n| n.to_string_lossy().to_string()).as_deref() {
        Some("patients.csv") => 0,
        Some("encounters.xlsx") => 1,
        _ => 2,
    };
    files.sort_by_key(|p| (rank(p), p.clone()));
    let mut out = Loaded::default();
    let mut patients_import = None;
    for f in &files {
        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let part = Part { name: name.clone(), data: PartData::File(f.clone()) };
        let r = tables::import_parts(
            c,
            cfg,
            user,
            &[part],
            &HashMap::new(),
            "",
            Some(&databases),
            &Opts::new(&name, NAME, false),
            crate::formats::Choice::Detect,
        );
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                let _ = undo(c, cfg, user);
                anyhow::bail!("the demo's {name}: {e} (nothing of the demo was kept)");
            }
        };
        if name == "patients.csv" {
            patients_import = Some(r.import_id);
        }
        out.tables.push((name.clone(), r.import_id));
        out.patients += r.patients_added;
        out.encounters += r.encounters_added;
        out.recordings += r.recordings_added;
        out.items += r.items_added;
        out.warnings.extend(r.warnings.iter().map(|w| format!("{name}: {w}")));
    }
    let Some(patients_import) = patients_import else {
        let _ = undo(c, cfg, user);
        anyhow::bail!("{} has no patients.csv", databases.display());
    };
    let omop = dir.join("omop").join("omop.sqlite");
    if omop.is_file() {
        let n = crate::sources::NewSource {
            name: CONNECTION.into(),
            kind: "omop".into(),
            url: format!("sqlite:{}", omop.display()),
            settings: crate::sources::Settings { timezone: cfg.timezone.clone(), ..Default::default() },
            ..Default::default()
        };
        if let Err(e) = crate::sources::add(c, user, &n) {
            let _ = undo(c, cfg, user);
            anyhow::bail!("the demo's OMOP database: {e} (nothing of the demo was kept)");
        }
        out.connected = true;
    }
    // the cohort: the patients of the demo's patients.csv (a refresh never takes in others)
    let criteria = Criteria {
        include: Some(Expr::Leaf(Criterion {
            kind: "database".into(),
            import: Some(patients_import),
            ..Default::default()
        })),
        ..Default::default()
    };
    let made = cohorts::create(
        c,
        user,
        &cfg.timezone,
        &NewCohort {
            name: NAME.into(),
            description: "Invented patients for trying Medorama: their encounters, results, notes, ICU \
                          recordings, a Holter, ECGs, imaging and echo, and another hospital's OMOP database. \
                          Deleting this cohort removes all of them from the catalog."
                .into(),
            criteria,
            ..Default::default()
        },
    );
    let id = match made {
        Ok(id) => id,
        Err(e) => {
            let _ = undo(c, cfg, user);
            anyhow::bail!("the demo's cohort: {e} (nothing of the demo was kept)");
        }
    };
    c.execute("UPDATE cohorts SET origin = ?1 WHERE id = ?2", params![ORIGIN, id])?;
    out.cohort = id;
    audit(
        c,
        Some(user),
        "demo_add",
        &dir.to_string_lossy(),
        &format!("{} table(s), {} patient(s), cohort {id}", out.tables.len(), out.patients),
    );
    Ok(out)
}

/// Remove the demo (admins): its connection, everything its batches added (with the catalog's
/// results and indexes of its recordings) and its cohort.  With `dry_run`, only what would be
/// removed.
pub fn remove(c: &mut Connection, cfg: &Config, user: &User, dry_run: bool) -> anyhow::Result<Removal> {
    if !user.is_admin() {
        anyhow::bail!("only admins remove the demo");
    }
    if !present(c)? {
        anyhow::bail!("this catalog holds no demo");
    }
    if dry_run {
        return crate::imports::remove_source(c, cfg, user, NAME, true, true);
    }
    let rep = undo(c, cfg, user)?;
    audit(c, Some(user), "demo_remove", NAME, &serde_json::to_string(&rep).unwrap_or_default());
    Ok(rep)
}

/// Remove what the demo added, whatever is there of it.
fn undo(c: &mut Connection, cfg: &Config, user: &User) -> anyhow::Result<Removal> {
    if let Some(s) = crate::sources::by_name(c, CONNECTION)? {
        crate::sources::remove(c, cfg, user, s.id)?;
    }
    let active: bool =
        c.query_row("SELECT EXISTS (SELECT 1 FROM imports WHERE state = 'active' AND source = ?1)", [NAME], |r| {
            r.get(0)
        })?;
    let rep = if active { crate::imports::remove_source(c, cfg, user, NAME, true, false)? } else { Removal::default() };
    if let Some(id) = cohort(c)? {
        cohorts::delete(c, user, id)?;
    }
    Ok(rep)
}
