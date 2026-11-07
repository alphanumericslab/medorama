//! Imports and sources (docs/administration.md, "Imports and sources").
//!
//! Every import (recordings, a linkage table, an EHR table) is a **batch** with a name and a
//! **source** label; a table may also name a source per row (`source` column).  `provenance`
//! records which batch and source added each recording, patient, encounter and bed stay, and
//! `ehr_provenance` which ones hold each EHR item; `field_values` and `patient_mrns` keep the
//! values each batch gave.  Removing a batch, or every batch of a source, removes exactly what
//! it added: an entry stays while another batch also has it, and its fields go back to the
//! values the remaining batches gave; the links its rows gave recordings go, and a generated
//! patient ID (`GEN-…`) it replaced comes back.  Files are never touched.
//!
//! A batch is `loading` until its import ends (`active`): a table is committed a chunk of
//! rows at a time on SQLite (so that its one writer's lock is free between chunks), and an
//! import that stops part way (an error, a cancel, a crash) is removed as a whole
//! ([`discard`], and at start-up [`discard_unfinished`]): nothing of it is kept.  Imports run
//! one at a time ([`crate::sql::Connection::import_lock`]).

use crate::config::Config;
use crate::db::{audit, now};
use crate::sql::{params, Connection, OptionalExtension, Transaction};
use crate::users::User;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

/// How an import is labelled, and whether it is only a preview.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// The batch's name (default: the file or folder).
    pub name: String,
    /// The source label (default: the name).  A table's `source` column overrides it per row.
    pub source: String,
    /// Check and count only; store nothing.
    pub dry_run: bool,
    /// Store a table's items one row at a time, as each row comes (the reference the batched
    /// way is tested against), rather than a chunk of rows at a time.
    pub row_by_row: bool,
    /// Where the import says how far it is, and is asked to stop.
    pub progress: Option<std::sync::Arc<crate::progress::Progress>>,
    /// Leave the batch `loading` when it ends: one of several made active together, by
    /// whoever imports them all ([`activate`]: a folder of tables).
    pub keep_loading: bool,
}

impl Opts {
    pub fn new(name: &str, source: &str, dry_run: bool) -> Opts {
        let name = name.trim().to_string();
        let source = if source.trim().is_empty() { name.clone() } else { source.trim().to_string() };
        Opts { name, source, dry_run, ..Default::default() }
    }
}

/// SQL: the batches still loading.  What only they hold is not shown yet: pages, cohorts and
/// exports leave out an item whose `import_id` is one (`i.import_id NOT IN {LOADING}`), and a
/// patient or encounter all of whose provenance is (the fragments: `crate::patient::loading`).
pub const LOADING: &str = "(SELECT id FROM imports WHERE state = 'loading')";

/// Start a batch (`loading` until [`finish`]); returns its id.
pub fn begin(tx: &Transaction, kind: &str, name: &str, source: &str, origin: &str, user: &User) -> anyhow::Result<i64> {
    Ok(tx.insert(
        "INSERT INTO imports (kind, origin, user_id, time, summary, name, source, state)
         VALUES (?1, ?2, ?3, ?4, '{}', ?5, ?6, 'loading')",
        params![kind, origin, (user.id > 0).then_some(user.id), now(), name, source],
    )?)
}

/// Record that batch `import` (source `source`) added row `row_id` of `kind`.
pub fn note(tx: &Connection, import: i64, source: &str, kind: &str, row_id: i64) -> anyhow::Result<()> {
    tx.execute(
        "INSERT INTO provenance (import_id, source, kind, row_id) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![import, source, kind, row_id],
    )?;
    Ok(())
}

/// A batch's summary, and the batch made active (unless it is to stay loading).
pub fn finish(tx: &Transaction, import: i64, summary: &serde_json::Value) -> anyhow::Result<()> {
    finish_as(tx, import, summary, false)
}

pub fn finish_as(tx: &Connection, import: i64, summary: &serde_json::Value, keep_loading: bool) -> anyhow::Result<()> {
    tx.execute(
        "UPDATE imports SET summary = ?1, state = CASE WHEN ?3 = 1 THEN state ELSE 'active' END WHERE id = ?2",
        params![summary.to_string(), import, keep_loading as i64],
    )?;
    Ok(())
}

/// Batches left loading by their import (a folder's tables), made active together.
pub fn activate(c: &Connection, imports: &[i64]) -> anyhow::Result<()> {
    for id in imports {
        c.execute("UPDATE imports SET state = 'active' WHERE id = ?1 AND state = 'loading'", [id])?;
    }
    Ok(())
}

/// An import that stopped part way (an error, a cancel): what its batch committed so far
/// removed, as if it had not run.
pub fn discard(c: &mut Connection, cfg: &Config, import: i64) -> anyhow::Result<()> {
    let loading: bool = c
        .query_row("SELECT state = 'loading' FROM imports WHERE id = ?1", [import], |r| r.get(0))
        .optional()?
        .unwrap_or(false);
    if loading {
        remove(c, cfg, None, Scope::Import(import), false, false)?;
    }
    Ok(())
}

/// The imports a stop or a crash left loading, removed (when no import runs: at start-up, and
/// as each import starts); returns how many.  `held`: the import lock is held by the caller.
pub fn discard_unfinished(c: &mut Connection, cfg: &Config, held: bool) -> anyhow::Result<usize> {
    let _lock = if held {
        None
    } else {
        match c.import_lock_if_free()? {
            Some(l) => Some(l),
            None => return Ok(0), // an import runs: its batch is not unfinished
        }
    };
    let ids: Vec<i64> = c
        .prepare("SELECT id FROM imports WHERE state = 'loading' ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in &ids {
        remove(c, cfg, None, Scope::Import(*id), false, false)?;
        eprintln!("medorama: import {id} had stopped part way: what it had added is removed");
    }
    Ok(ids.len())
}

#[derive(Debug, Serialize)]
pub struct Batch {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub source: String,
    pub origin: String,
    pub username: String,
    pub time: f64,
    pub state: String,
    pub summary: serde_json::Value,
    /// What the batch still contributes: (kind, count), and EHR items.
    pub recordings: i64,
    pub encounters: i64,
    pub ehr_items: i64,
}

pub fn list(c: &Connection) -> anyhow::Result<Vec<Batch>> {
    let mut st = c.prepare(
        "SELECT i.id, i.kind, i.name, i.source, i.origin, COALESCE(u.username, ''), i.time, i.state, i.summary,
                (SELECT COUNT(*) FROM provenance p WHERE p.import_id = i.id AND p.kind = 'recording'),
                (SELECT COUNT(*) FROM provenance p WHERE p.import_id = i.id AND p.kind = 'encounter'),
                (SELECT COUNT(*) FROM ehr_provenance e WHERE e.import_id = i.id)
         FROM imports i LEFT JOIN users u ON u.id = i.user_id ORDER BY i.id DESC",
    )?;
    let v = st
        .query_map([], |r| {
            Ok(Batch {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                source: r.get(3)?,
                origin: r.get(4)?,
                username: r.get(5)?,
                time: r.get(6)?,
                state: r.get(7)?,
                summary: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or(serde_json::Value::Null),
                recordings: r.get(9)?,
                encounters: r.get(10)?,
                ehr_items: r.get(11)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

#[derive(Debug, Serialize)]
pub struct SourceRow {
    pub source: String,
    pub recordings: i64,
    pub patients: i64,
    pub encounters: i64,
    pub bed_stays: i64,
    pub ehr_items: i64,
    pub imports: i64,
}

/// Every source with what it contributes.
pub fn sources(c: &Connection) -> anyhow::Result<Vec<SourceRow>> {
    let mut st = c.prepare(
        "SELECT s.source,
           (SELECT COUNT(DISTINCT row_id) FROM provenance p WHERE p.source = s.source AND p.kind = 'recording'),
           (SELECT COUNT(DISTINCT row_id) FROM provenance p WHERE p.source = s.source AND p.kind = 'patient'),
           (SELECT COUNT(DISTINCT row_id) FROM provenance p WHERE p.source = s.source AND p.kind = 'encounter'),
           (SELECT COUNT(DISTINCT row_id) FROM provenance p WHERE p.source = s.source AND p.kind = 'bed_stay'),
           (SELECT COUNT(DISTINCT item) FROM ehr_provenance e WHERE e.source = s.source),
           (SELECT COUNT(DISTINCT import_id) FROM (SELECT import_id FROM provenance p WHERE p.source = s.source
                                                    UNION SELECT import_id FROM ehr_provenance e WHERE e.source = s.source))
         FROM (SELECT source FROM provenance UNION SELECT source FROM ehr_provenance) s ORDER BY s.source",
    )?;
    let v = st
        .query_map([], |r| {
            Ok(SourceRow {
                source: r.get(0)?,
                recordings: r.get(1)?,
                patients: r.get(2)?,
                encounters: r.get(3)?,
                bed_stays: r.get(4)?,
                ehr_items: r.get(5)?,
                imports: r.get(6)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(v)
}

/// What a removal takes out (or would, in a dry run).
#[derive(Debug, Default, Serialize)]
pub struct Removal {
    pub dry_run: bool,
    pub recordings: usize,
    pub patients: usize,
    pub encounters: usize,
    pub bed_stays: usize,
    pub ehr_items: usize,
    /// Entries the removed batches added that stay, because another batch also added them.
    pub kept_recordings: usize,
    pub kept_encounters: usize,
    pub batches: usize,
    /// Patients given back the generated ID (`GEN-…`) a removed batch had replaced.
    pub ids_restored: usize,
}

enum Scope<'a> {
    Import(i64),
    Source(&'a str),
}

/// Remove one batch.
pub fn remove_import(
    c: &mut Connection,
    cfg: &Config,
    user: &User,
    id: i64,
    purge: bool,
    dry_run: bool,
) -> anyhow::Result<Removal> {
    remove(c, cfg, Some(user), Scope::Import(id), purge, dry_run)
}

/// Remove everything a source contributed, in every batch.
pub fn remove_source(
    c: &mut Connection,
    cfg: &Config,
    user: &User,
    source: &str,
    purge: bool,
    dry_run: bool,
) -> anyhow::Result<Removal> {
    remove(c, cfg, Some(user), Scope::Source(source), purge, dry_run)
}

fn remove(
    c: &mut Connection,
    cfg: &Config,
    user: Option<&User>,
    scope: Scope,
    purge: bool,
    dry_run: bool,
) -> anyhow::Result<Removal> {
    // (no user: an import that did not finish, removed by the import itself)
    if user.is_some_and(|u| !u.is_admin()) {
        anyhow::bail!("only admins remove entries");
    }
    let (filter, arg): (&str, crate::sql::types::Value) = match &scope {
        Scope::Import(id) => ("import_id = ?1", (*id).into()),
        Scope::Source(s) => ("source = ?1", s.to_string().into()),
    };
    if let Scope::Import(id) = scope {
        let state: Option<String> =
            c.query_row("SELECT state FROM imports WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        match state.as_deref() {
            None => anyhow::bail!("no import {id}"),
            Some("removed") => anyhow::bail!("import {id} was removed already"),
            _ => {}
        }
    }
    let mut rep = Removal { dry_run, ..Default::default() };
    let tx = c.transaction()?;
    // the rows the batch(es) touched
    let touched: Vec<(String, i64)> = {
        let mut st = tx.prepare(&format!("SELECT DISTINCT kind, row_id FROM provenance WHERE {filter}"))?;

        st.query_map([&arg], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    let batches: HashSet<i64> = {
        let mut st = tx.prepare(&format!(
            "SELECT import_id FROM provenance WHERE {filter} UNION SELECT import_id FROM ehr_provenance WHERE {filter}
             UNION SELECT import_id FROM field_values WHERE {filter} UNION SELECT import_id FROM held_rows WHERE {filter}"
        ))?;

        st.query_map([&arg], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    // EHR items: removed when no batch holds them any more
    let items: Vec<i64> = {
        let mut st = tx.prepare(&format!("SELECT DISTINCT item FROM ehr_provenance WHERE {filter}"))?;

        st.query_map([&arg], |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    tx.execute(&format!("DELETE FROM ehr_provenance WHERE {filter}"), [&arg])?;
    for item in items {
        let held: Option<(i64, String)> = tx
            .query_row(
                "SELECT import_id, source FROM ehr_provenance WHERE item = ?1 ORDER BY import_id LIMIT 1",
                [item],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match held {
            None => rep.ehr_items += tx.execute("DELETE FROM ehr_items WHERE id = ?1", [item])?,
            Some((i, s)) => {
                tx.execute("UPDATE ehr_items SET import_id = ?1, source = ?2 WHERE id = ?3", params![i, s, item])?;
            }
        }
    }
    for table in ["provenance", "field_values", "patient_mrns", "held_rows"] {
        tx.execute(&format!("DELETE FROM {table} WHERE {filter}"), [&arg])?;
    }
    let orphan = |tx: &Transaction, kind: &str, id: i64| -> crate::sql::Result<bool> {
        tx.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM provenance WHERE kind = ?1 AND row_id = ?2)",
            params![kind, id],
            |r| r.get(0),
        )
    };
    let of = |k: &str| -> Vec<i64> { touched.iter().filter(|(kind, _)| kind == k).map(|(_, id)| *id).collect() };
    for id in of("bed_stay") {
        if orphan(&tx, "bed_stay", id)? {
            rep.bed_stays += tx.execute("DELETE FROM bed_stays WHERE id = ?1", [id])?;
        }
    }
    for id in of("encounter") {
        if orphan(&tx, "encounter", id)? {
            tx.execute("DELETE FROM provenance WHERE kind = 'bed_stay' AND row_id IN (SELECT id FROM bed_stays WHERE encounter = ?1)", [id])?;
            rep.encounters += tx.execute("DELETE FROM encounters WHERE id = ?1", [id])?;
        } else {
            rep.kept_encounters += 1;
            crate::identity::recompute_encounter(&tx, id)?;
        }
    }
    for id in of("patient") {
        let no_encounters: bool =
            tx.query_row("SELECT NOT EXISTS (SELECT 1 FROM encounters WHERE patient = ?1)", [id], |r| r.get(0))?;
        if orphan(&tx, "patient", id)? && no_encounters {
            rep.patients += tx.execute("DELETE FROM patients WHERE id = ?1", [id])?;
        } else {
            crate::identity::recompute_patient(&tx, id)?;
        }
    }
    let mut dirs = vec![];
    for id in of("recording") {
        if orphan(&tx, "recording", id)? {
            if purge {
                let mut st = tx.prepare("SELECT output_dir FROM jobs WHERE recording = ?1 AND output_dir != ''")?;
                dirs.extend(st.query_map([id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?);
            }
            rep.recordings += tx.execute("DELETE FROM recordings WHERE id = ?1", [id])?;
        } else {
            rep.kept_recordings += 1;
        }
    }
    // batches with nothing left are marked removed
    let t = now();
    for b in &batches {
        let left: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM provenance WHERE import_id = ?1) OR EXISTS (SELECT 1 FROM ehr_provenance WHERE import_id = ?1)",
            [b],
            |r| r.get(0),
        )?;
        if !left {
            tx.execute("UPDATE imports SET state = 'removed', removed_at = ?1 WHERE id = ?2", params![t, b])?;
        }
    }
    let mut connection = None;
    if let Scope::Import(id) = scope {
        tx.execute("UPDATE imports SET state = 'removed', removed_at = ?1 WHERE id = ?2", params![t, id])?;
        // a connected database's batch: the connection and its links go with it
        connection = crate::sources::of_import(&tx, id)?;
        tx.execute("DELETE FROM connections WHERE import_id = ?1", [id])?;
    }
    // generated patient IDs the removed batches replaced, back where no other batch names the
    // patient by the ID they gave
    let taken: Vec<(i64, String, String)> = {
        let mut st = tx.prepare(
            "SELECT t.patient, t.generated_id, t.patient_id FROM id_takeovers t JOIN imports i ON i.id = t.import_id
             JOIN patients p ON p.id = t.patient AND p.patient_id = t.patient_id WHERE i.state = 'removed' ORDER BY t.patient",
        )?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?
    };
    let pids: Vec<&str> = taken.iter().map(|t| t.2.as_str()).collect();
    let still: HashSet<String> = tx
        .query_in(
            "SELECT DISTINCT r.key_patient FROM import_rows r JOIN imports i ON i.id = r.import_id
             WHERE i.state != 'removed' AND r.key_patient IN ({})",
            &pids,
            |r| r.get(0),
        )?
        .into_iter()
        .collect();
    for (patient, generated, pid) in &taken {
        if still.contains(pid) {
            continue;
        }
        tx.execute("UPDATE patients SET patient_id = ?1 WHERE id = ?2", params![generated, patient])?;
        tx.execute("DELETE FROM patient_merges WHERE from_patient_id = ?1 AND into_patient_id = ?2", [generated, pid])?;
        tx.execute("UPDATE patient_merges SET into_patient_id = ?1 WHERE into_patient_id = ?2", [generated, pid])?;
        rep.ids_restored += 1;
    }
    // the rows of a removed database (as imported: PHI) go with it, and what its rows did
    tx.execute_batch(
        "DELETE FROM import_rows WHERE import_id IN (SELECT id FROM imports WHERE state = 'removed');
         DELETE FROM import_columns WHERE import_id IN (SELECT id FROM imports WHERE state = 'removed');
         DELETE FROM import_links WHERE import_id IN (SELECT id FROM imports WHERE state = 'removed');
         DELETE FROM id_takeovers WHERE import_id IN (SELECT id FROM imports WHERE state = 'removed');",
    )?;
    crate::typed::drop_removed(&tx)?;
    rep.batches = batches.len().max(matches!(scope, Scope::Import(_)) as usize);
    if dry_run {
        tx.rollback()?;
        return Ok(rep);
    }
    tx.commit()?;
    if let Some(s) = &connection {
        crate::sources::removed(cfg, s);
    }
    for d in dirs {
        crate::recordings::purge_dir(cfg, Path::new(&d));
    }
    crate::linkage::relink(c, None)?;
    crate::ehr::match_pending(c)?;
    let target = match scope {
        Scope::Import(id) => format!("import {id}"),
        Scope::Source(s) => format!("source {s}"),
    };
    let event = if user.is_some() { "remove_import" } else { "discard_import" };
    audit(c, user, event, &target, &serde_json::to_string(&rep).unwrap_or_default());
    Ok(rep)
}

/// Delete provenance rows whose entry no longer exists (after removals by other paths).
pub fn prune(c: &Connection) -> anyhow::Result<()> {
    c.execute_batch(
        "DELETE FROM provenance WHERE kind = 'recording' AND row_id NOT IN (SELECT id FROM recordings);
         DELETE FROM provenance WHERE kind = 'patient' AND row_id NOT IN (SELECT id FROM patients);
         DELETE FROM provenance WHERE kind = 'encounter' AND row_id NOT IN (SELECT id FROM encounters);
         DELETE FROM provenance WHERE kind = 'bed_stay' AND row_id NOT IN (SELECT id FROM bed_stays);
         DELETE FROM field_values WHERE kind = 'patient' AND row_id NOT IN (SELECT id FROM patients);
         DELETE FROM field_values WHERE kind = 'encounter' AND row_id NOT IN (SELECT id FROM encounters);",
    )?;
    Ok(())
}

/// The sources an entry came from.
pub fn sources_of(c: &Connection, kind: &str, row_id: i64) -> anyhow::Result<Vec<String>> {
    let mut st = c.prepare("SELECT DISTINCT source FROM provenance WHERE kind = ?1 AND row_id = ?2 ORDER BY source")?;
    let v = st.query_map(params![kind, row_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(v)
}
