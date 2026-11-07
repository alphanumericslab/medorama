//! The audit log, tamper-evident (docs/security.md, "Audit logging").
//!
//! Entries are written by [`crate::db::audit`].  Every minute the server **seals** the entries
//! not sealed yet: a seal is the SHA-256 chain of those entries, in order, starting from the
//! previous seal's hash, and each entry records the seal that covers it.  [`verify`] computes
//! every chain again: an entry changed, deleted or slipped into a sealed batch, or a seal
//! changed, shows as the first seal whose hash differs.  Entries committed late (a long
//! transaction) are simply sealed by the next seal.  Entries and seals are numbered by the
//! database, without reuse: [`verify`] also counts the numbers the database gave against those
//! left, so that entries or seals deleted at the end of the log (the last seal with its
//! entries, or entries not sealed yet) show too.
//!
//! Whoever controls the database could rewrite the entries and their seals together (and the
//! database's counters with them); so each seal and its entries can also be appended, as JSON
//! lines, to a file outside the database (`audit_export` in medorama.toml): put it on storage
//! the database's administrators cannot change (a write-once volume, a log collector).
//! [`verify_with_export`] compares the database's seals with it: the end of the log is
//! protected by the export, up to its last seal.

use std::io::Write;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::now;
use crate::sql::{params, Connection, Dialect, Lock, OptionalExtension, Value};

/// The chain's start.
const GENESIS: &str = "medorama-audit-v1";

/// One entry as it is hashed: every column, in order, unambiguously.
fn entry_bytes(
    id: i64,
    time: f64,
    user: Option<i64>,
    username: &str,
    event: &str,
    target: &str,
    detail: &str,
) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!([id, format!("{time:?}"), user, username, event, target, detail]))
        .unwrap_or_default()
}

fn link(prev: &str, entry: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update([0u8]);
    h.update(entry);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

type Row = (i64, f64, Option<i64>, String, String, String, String);

fn rows(c: &Connection, sql: &str, p: Vec<Value>) -> anyhow::Result<Vec<Row>> {
    let mut st = c.prepare(sql)?;
    Ok(st
        .query_map(p, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .collect::<Result<_, _>>()?)
}

const COLS: &str = "id, time, user_id, username, event, target, detail";

/// What a seal covered, as shipped.
#[derive(Debug, Serialize)]
pub struct Sealed {
    pub seal: i64,
    pub entries: usize,
    pub hash: String,
}

/// Seal the entries not sealed yet (none: nothing done).  One server at a time.
pub fn seal(c: &Connection, export: Option<&std::path::Path>) -> anyhow::Result<Option<Sealed>> {
    // entries the database could not take at once (db.rs `audit`) go in first, in order
    crate::db::flush_audit(c);
    c.exclusive(Lock::Audit, || seal_now(c, export))?
}

fn seal_now(c: &Connection, export: Option<&std::path::Path>) -> anyhow::Result<Option<Sealed>> {
    let todo = rows(c, &format!("SELECT {COLS} FROM audit WHERE sealed_in IS NULL ORDER BY id LIMIT 50000"), vec![])?;
    if todo.is_empty() {
        return Ok(None);
    }
    let prev: String = c
        .query_row("SELECT hash FROM audit_seals ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
        .optional()?
        .unwrap_or_else(|| GENESIS.to_string());
    let mut hash = prev;
    for (id, time, user, username, event, target, detail) in &todo {
        hash = link(&hash, &entry_bytes(*id, *time, *user, username, event, target, detail));
    }
    let tx = c.unchecked_transaction()?;
    let seal = tx.insert(
        "INSERT INTO audit_seals (time, entries, hash) VALUES (?1, ?2, ?3)",
        params![now(), todo.len() as i64, hash],
    )?;
    let ids: Vec<i64> = todo.iter().map(|r| r.0).collect();
    for chunk in ids.chunks(500) {
        let list: Vec<String> = (2..chunk.len() + 2).map(|i| format!("?{i}")).collect();
        let mut v: Vec<Value> = vec![seal.into()];
        v.extend(chunk.iter().map(|i| Value::Integer(*i)));
        tx.execute(&format!("UPDATE audit SET sealed_in = ?1 WHERE id IN ({})", list.join(", ")), v)?;
    }
    tx.commit()?;
    if let Some(path) = export {
        ship(path, seal, &hash, &todo)?;
    }
    Ok(Some(Sealed { seal, entries: todo.len(), hash }))
}

/// Append a seal and its entries to the export file (JSON lines; created owner-only).
fn ship(path: &std::path::Path, seal: i64, hash: &str, entries: &[Row]) -> anyhow::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f = o.open(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let mut out = String::new();
    for (id, time, user, username, event, target, detail) in entries {
        out += &serde_json::json!({"id": id, "time": time, "user_id": user, "username": username, "event": event,
                                   "target": target, "detail": detail, "seal": seal})
        .to_string();
        out.push('\n');
    }
    out += &serde_json::json!({"seal": seal, "entries": entries.len(), "hash": hash, "time": now()}).to_string();
    out.push('\n');
    f.write_all(out.as_bytes())?;
    f.sync_data()?;
    Ok(())
}

/// What verifying found.
#[derive(Debug, Serialize)]
pub struct Verified {
    pub seals: usize,
    pub entries: usize,
    /// Entries written since the last seal (not covered yet).
    pub unsealed: usize,
    /// The first seal whose chain differs, or what is missing, and how.
    pub broken: Option<String>,
    /// Seals found in the export file (`audit_export`), when it was compared.
    pub exported_seals: Option<usize>,
}

/// The highest number the database gave in a table's `id` (none given yet: `None`).
fn issued(c: &Connection, table: &str) -> anyhow::Result<Option<i64>> {
    Ok(match c.dialect() {
        Dialect::Sqlite => {
            c.query_row("SELECT seq FROM sqlite_sequence WHERE name = ?1", [table], |r| r.get(0)).optional()?
        }
        Dialect::Postgres => {
            c.query_row(&format!("SELECT pg_sequence_last_value(pg_get_serial_sequence('{table}', 'id'))"), [], |r| {
                r.get::<_, Option<i64>>(0)
            })?
        }
    })
}

/// Rows missing from a table numbered without reuse: those the database numbered and that are
/// not there (deleted; on PostgreSQL also those of a transaction that failed).
fn missing(c: &Connection, table: &str) -> anyhow::Result<i64> {
    let (n, max): (i64, Option<i64>) =
        c.query_row(&format!("SELECT COUNT(*), MAX(id) FROM {table}"), [], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let top = issued(c, table)?.max(max).unwrap_or(0);
    Ok(top - n)
}

/// Compute every seal's chain again.
pub fn verify(c: &Connection) -> anyhow::Result<Verified> {
    let mut st = c.prepare("SELECT id, entries, hash FROM audit_seals ORDER BY id")?;
    let seals: Vec<(i64, i64, String)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    let mut prev = GENESIS.to_string();
    let mut total = 0;
    for (id, n, hash) in &seals {
        let entries =
            rows(c, &format!("SELECT {COLS} FROM audit WHERE sealed_in = ?1 ORDER BY id"), vec![(*id).into()])?;
        total += entries.len();
        let mut h = prev.clone();
        for (eid, time, user, username, event, target, detail) in &entries {
            h = link(&h, &entry_bytes(*eid, *time, *user, username, event, target, detail));
        }
        let broken = if entries.len() as i64 != *n {
            Some(format!("seal {id}: {} entries, the seal says {n} (entries deleted or added)", entries.len()))
        } else if h != *hash {
            Some(format!("seal {id}: its entries do not give its hash (an entry or a seal was changed)"))
        } else {
            None
        };
        if broken.is_some() {
            let unsealed: i64 = c.query_row("SELECT COUNT(*) FROM audit WHERE sealed_in IS NULL", [], |r| r.get(0))?;
            return Ok(Verified {
                seals: seals.len(),
                entries: total,
                unsealed: unsealed as usize,
                broken,
                exported_seals: None,
            });
        }
        prev = hash.clone();
    }
    let unsealed: i64 = c.query_row("SELECT COUNT(*) FROM audit WHERE sealed_in IS NULL", [], |r| r.get(0))?;
    // an entry pointing at a seal that does not exist was moved out of the chain
    let orphans: i64 = c.query_row(
        "SELECT COUNT(*) FROM audit WHERE sealed_in IS NOT NULL AND sealed_in NOT IN (SELECT id FROM audit_seals)",
        [],
        |r| r.get(0),
    )?;
    let last_seal: Option<f64> = c.query_row("SELECT MAX(time) FROM audit_seals", [], |r| r.get(0))?;
    let since = last_seal.map_or("no seal left".to_string(), |t| {
        format!("the last seal left was made at {} UTC", crate::util::fmt_local(t, crate::util::tz("UTC")))
    });
    let (gone_seals, gone_entries) = (missing(c, "audit_seals")?, missing(c, "audit")?);
    let broken = if orphans > 0 {
        Some(format!("{orphans} entries name a seal that does not exist (a seal was deleted)"))
    } else if gone_seals > 0 {
        Some(format!("{gone_seals} seal(s) missing: the database numbered more than it holds (deleted; {since})"))
    } else if gone_entries > 0 {
        Some(format!(
            "{gone_entries} entries missing: the database numbered more than it holds (deleted, or lost with a transaction that failed; {since})"
        ))
    } else {
        None
    };
    Ok(Verified { seals: seals.len(), entries: total, unsealed: unsealed as usize, broken, exported_seals: None })
}

/// [`verify`], and the seals of the export file (`audit_export`) compared with the database's:
/// each must be there, with the same hash and number of entries.  A seal the database lacks
/// (the end of the log cut, with its counters) shows here.
pub fn verify_with_export(c: &Connection, export: Option<&std::path::Path>) -> anyhow::Result<Verified> {
    let mut v = verify(c)?;
    let Some(path) = export else { return Ok(v) };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        // nothing sealed yet: no file yet
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && v.seals == 0 => String::new(),
        Err(e) => anyhow::bail!("{}: {e}", path.display()),
    };
    let mut st = c.prepare("SELECT id, entries, hash FROM audit_seals")?;
    let held: std::collections::HashMap<i64, (i64, String)> =
        st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?;
    let mut n = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(j) = serde_json::from_str::<serde_json::Value>(line) else {
            v.broken.get_or_insert_with(|| format!("{}: a line is not JSON", path.display()));
            continue;
        };
        let (Some(seal), Some(hash)) = (j["seal"].as_i64(), j["hash"].as_str()) else { continue };
        n += 1;
        let entries = j["entries"].as_i64().unwrap_or(-1);
        match held.get(&seal) {
            Some((e, h)) if *h == hash && *e == entries => {}
            Some(_) => {
                v.broken.get_or_insert_with(|| format!("seal {seal} differs from the export's (it was changed)"));
            }
            None => {
                v.broken.get_or_insert_with(|| {
                    format!("seal {seal} is in the export ({}) but not in the database (deleted)", path.display())
                });
            }
        }
    }
    v.exported_seals = Some(n);
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> (tempfile::TempDir, crate::db::Db) {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config::new(&tmp.path().join("home"), &tmp.path().join("results"));
        let db = crate::db::Db::open(&cfg, true).unwrap();
        (tmp, db)
    }

    #[test]
    fn sealed_entries_cannot_change() {
        let (tmp, db) = catalog();
        let c = db.get().unwrap();
        let export = tmp.path().join("audit.jsonl");
        for i in 0..5 {
            crate::db::audit(&c, None, "test", &format!("t{i}"), "");
        }
        let s = seal(&c, Some(&export)).unwrap().unwrap();
        assert_eq!(s.entries, 5);
        assert!(seal(&c, Some(&export)).unwrap().is_none(), "nothing new");
        crate::db::audit(&c, None, "test", "t5", "");
        seal(&c, Some(&export)).unwrap().unwrap();
        let v = verify(&c).unwrap();
        assert_eq!((v.seals, v.entries, v.unsealed, v.broken.is_none()), (2, 6, 0, true));
        assert_eq!(std::fs::read_to_string(&export).unwrap().lines().count(), 6 + 2);
        // an entry changed
        c.execute("UPDATE audit SET detail = 'nothing to see' WHERE target = 't2'", []).unwrap();
        assert!(verify(&c).unwrap().broken.unwrap().contains("seal 1"));
        c.execute("UPDATE audit SET detail = '' WHERE target = 't2'", []).unwrap();
        assert!(verify(&c).unwrap().broken.is_none());
        // an entry deleted
        c.execute("DELETE FROM audit WHERE target = 't5'", []).unwrap();
        assert!(verify(&c).unwrap().broken.unwrap().contains("seal 2"));
    }

    #[test]
    fn the_end_of_the_log_cannot_be_cut() {
        let (tmp, db) = catalog();
        let c = db.get().unwrap();
        let export = tmp.path().join("audit.jsonl");
        crate::db::audit(&c, None, "test", "t0", "");
        seal(&c, Some(&export)).unwrap().unwrap();
        for i in 1..4 {
            crate::db::audit(&c, None, "phi_search", "", &format!("secret search {i}"));
        }
        let s = seal(&c, Some(&export)).unwrap().unwrap();
        let v = verify_with_export(&c, Some(&export)).unwrap();
        assert!(v.broken.is_none() && v.exported_seals == Some(2), "{v:?}");
        // the last seal and its entries deleted
        c.execute("DELETE FROM audit WHERE sealed_in = ?1", [s.seal]).unwrap();
        c.execute("DELETE FROM audit_seals WHERE id = ?1", [s.seal]).unwrap();
        assert!(verify(&c).unwrap().broken.unwrap().contains("missing"));
        // … and the database's counters set back: the export still knows the seal
        c.execute("UPDATE sqlite_sequence SET seq = 1 WHERE name IN ('audit', 'audit_seals')", []).unwrap();
        assert!(verify(&c).unwrap().broken.is_none(), "the database alone cannot tell");
        assert!(verify_with_export(&c, Some(&export)).unwrap().broken.unwrap().contains("not in the database"));
        // entries not sealed yet, deleted
        let (_tmp2, db2) = catalog();
        let c2 = db2.get().unwrap();
        crate::db::audit(&c2, None, "test", "t0", "");
        seal(&c2, None).unwrap();
        crate::db::audit(&c2, None, "phi_search", "", "unsealed");
        assert!(verify(&c2).unwrap().broken.is_none());
        c2.execute("DELETE FROM audit WHERE sealed_in IS NULL", []).unwrap();
        assert!(verify(&c2).unwrap().broken.unwrap().contains("1 entries missing"));
    }
}
