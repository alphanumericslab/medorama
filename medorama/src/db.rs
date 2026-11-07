//! The database: SQLite (one file, the default) or PostgreSQL (`database_url`, for several
//! servers and many users), through one interface ([`crate::sql`]).
//!
//! SQLite runs in write-ahead-log mode (one writer at a time, readers never blocked), so the
//! server, `medorama` commands and job workers can use it together.  PostgreSQL lets several
//! Medorama servers share one catalog: sessions, cohorts, pseudonyms and the job queue are all in
//! the database, so any server answers any request.
//!
//! The schema is `src/schema/sqlite/001_schema.sql` and `src/schema/postgres/001_schema.sql`
//! (the same tables, one file per database). A later change to it comes as the next numbered
//! file in each folder, applied when a catalog is opened; the catalog records the versions it
//! has (`schema_migrations`).

use crate::config::Config;
use crate::sql::{Connection, Dialect, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// (version, SQL), in order, per database.  Never edit one that has been released; add the next.
const SQLITE_MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("schema/sqlite/001_schema.sql")),
    (2, include_str!("schema/sqlite/002_import_links.sql")),
    (3, include_str!("schema/sqlite/003_privacy_gate.sql")),
];
const PG_MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("schema/postgres/001_schema.sql")),
    (2, include_str!("schema/postgres/002_import_links.sql")),
    (3, include_str!("schema/postgres/003_privacy_gate.sql")),
];

/// The most connections to a PostgreSQL database a server opens at once (a request waits for
/// one beyond that); idle ones kept, and how long one may be idle before it is checked.
const PG_MAX_CONNECTIONS: usize = 32;
const IDLE_KEPT: usize = 16;
const IDLE_CHECK: std::time::Duration = std::time::Duration::from_secs(30);

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

enum Kind {
    Sqlite { path: PathBuf, key: Option<String> },
    Postgres { url: String, password: Option<String>, tls: crate::sql::Tls },
}

/// A pool of connections to the database.
pub struct Db {
    kind: Kind,
    /// Idle connections, with when each was last given back.
    pool: Mutex<Vec<(Connection, std::time::Instant)>>,
    /// How many PostgreSQL connections are open (idle or in use), and a wait for one to close.
    open: (Mutex<usize>, std::sync::Condvar),
}

pub struct Conn<'a> {
    db: &'a Db,
    conn: Option<Connection>,
}

impl std::ops::Deref for Conn<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl std::ops::DerefMut for Conn<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.conn.as_mut().unwrap()
    }
}

impl Drop for Conn<'_> {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            if c.is_autocommit() && !c.is_closed() {
                let mut p = self.db.pool.lock().unwrap_or_else(|e| e.into_inner());
                if p.len() < IDLE_KEPT {
                    p.push((c, std::time::Instant::now()));
                    return;
                }
            }
            self.db.closed(c);
        }
    }
}

fn database_key(cfg: &Config) -> anyhow::Result<Option<String>> {
    let env = std::env::var("MEDORAMA_KEY").ok().filter(|k| !k.is_empty());
    let key = match env {
        Some(k) => Some(k),
        _ => match &cfg.key_file {
            Some(f) => Some(read_secret(f)?),
            None => None,
        },
    };
    if key.is_some() && !cfg!(feature = "encryption") {
        anyhow::bail!(
            "a database key is configured, but this build has no encryption (build with --features encryption)"
        );
    }
    Ok(key)
}

fn read_secret(f: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(f).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?.trim().to_string())
}

/// The PostgreSQL password: `$MEDORAMA_DATABASE_PASSWORD`, else `database_password_file`, else
/// none (the URL's, if it has one).
fn database_password(cfg: &Config) -> anyhow::Result<Option<String>> {
    if let Some(p) = std::env::var("MEDORAMA_DATABASE_PASSWORD").ok().filter(|p| !p.is_empty()) {
        return Ok(Some(p));
    }
    cfg.database_password_file.as_deref().map(read_secret).transpose()
}

fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let p = PathBuf::from(format!("{}{suffix}", path.display()));
            if p.exists() {
                let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// A database URL without its password, to show: the password of `user:password@`, of a
/// `password=` parameter (`?password=…`), or of libpq's `key=value` form (`host=… password=…`).
/// A URL holds a password when this differs from it.
pub fn shown_url(url: &str) -> String {
    let Some(s) = url.find("://") else {
        // libpq's form: key=value pairs, apart by spaces
        return url
            .split(' ')
            .map(|kv| match kv.split_once('=') {
                Some((k, _)) if k.trim().eq_ignore_ascii_case("password") => format!("{k}=…"),
                _ => kv.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" ");
    };
    let rest = &url[s + 3..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let authority = match authority.rfind('@') {
        Some(at) => match authority[..at].split_once(':') {
            Some((user, _)) => format!("{user}:…{}", &authority[at..]),
            None => authority.to_string(),
        },
        None => authority.to_string(),
    };
    let path = match path.split_once('?') {
        Some((p, query)) => {
            let query: Vec<String> = query
                .split('&')
                .map(|kv| match kv.split_once('=') {
                    Some((k, _)) if k.eq_ignore_ascii_case("password") => format!("{k}=…"),
                    _ => kv.to_string(),
                })
                .collect();
            format!("{p}?{}", query.join("&"))
        }
        None => path.to_string(),
    };
    format!("{}{authority}{path}", &url[..s + 3])
}

/// Whether a database URL holds a password (it must not: the password comes apart).
pub fn has_password(url: &str) -> bool {
    shown_url(url) != url
}

impl Db {
    /// Open (creating it if `create`) and bring the schema up to date.
    pub fn open(cfg: &Config, create: bool) -> anyhow::Result<Db> {
        let url = cfg.database_url.clone();
        let db = if url.is_empty() {
            if !create && !cfg.database.exists() {
                anyhow::bail!(
                    "{} is missing, although {} is there.\n  Either `init` stopped before creating the database: delete {} and run `medorama init` again;\n  or the database was moved or deleted: put it back, or set `database` in medorama.toml.",
                    cfg.database.display(),
                    cfg.home.join(crate::config::CONFIG_NAME).display(),
                    cfg.home.join(crate::config::CONFIG_NAME).display()
                );
            }
            if let Some(dir) = cfg.database.parent() {
                std::fs::create_dir_all(dir)?;
            }
            Db {
                kind: Kind::Sqlite { path: cfg.database.clone(), key: database_key(cfg)? },
                pool: Mutex::new(vec![]),
                open: Default::default(),
            }
        } else {
            if cfg.key_file.is_some() {
                anyhow::bail!(
                    "key_file encrypts a SQLite database; a PostgreSQL database is encrypted by its server (docs/security.md)"
                );
            }
            Db {
                kind: Kind::Postgres {
                    url: url.clone(),
                    password: database_password(cfg)?,
                    tls: crate::sql::Tls { ca_file: cfg.database_ca_file.clone() },
                },
                pool: Mutex::new(vec![]),
                open: Default::default(),
            }
        };
        set_audit_fallback(cfg.audit_export.clone());
        {
            let mut c = db.get()?;
            if !create && c.dialect() == Dialect::Postgres && schema_version(&c)? == 0 {
                anyhow::bail!(
                    "the PostgreSQL database {} holds no catalog yet: create it with `medorama init`",
                    shown_url(&url)
                );
            }
            migrate(&mut c)?;
        }
        if let Kind::Sqlite { path, .. } = &db.kind {
            private(path);
        }
        Ok(db)
    }

    pub fn dialect(&self) -> Dialect {
        match self.kind {
            Kind::Sqlite { .. } => Dialect::Sqlite,
            Kind::Postgres { .. } => Dialect::Postgres,
        }
    }

    /// Where the database is, to show (no password).
    pub fn describe(&self) -> String {
        match &self.kind {
            Kind::Sqlite { path, .. } => path.display().to_string(),
            Kind::Postgres { url, .. } => shown_url(url),
        }
    }

    pub fn get(&self) -> anyhow::Result<Conn<'_>> {
        loop {
            let idle = self.pool.lock().unwrap_or_else(|e| e.into_inner()).pop();
            let Some((c, since)) = idle else { break };
            // a connection idle a while may have been closed by the server, or the network
            if c.is_closed() || (since.elapsed() > IDLE_CHECK && c.execute_batch_native("SELECT 1").is_err()) {
                self.closed(c);
                continue;
            }
            return Ok(Conn { db: self, conn: Some(c) });
        }
        if let Kind::Postgres { .. } = self.kind {
            let (n, cv) = &self.open;
            let mut n = n.lock().unwrap_or_else(|e| e.into_inner());
            let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while *n >= PG_MAX_CONNECTIONS {
                let left = until.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    anyhow::bail!("all {PG_MAX_CONNECTIONS} connections to the database are busy: try again");
                }
                n = cv.wait_timeout(n, left).unwrap_or_else(|e| e.into_inner()).0;
                if let Some((c, _)) = self.pool.lock().unwrap_or_else(|e| e.into_inner()).pop() {
                    drop(n);
                    if c.is_closed() {
                        self.closed(c);
                        return self.get();
                    }
                    return Ok(Conn { db: self, conn: Some(c) });
                }
            }
            *n += 1;
        }
        let c = self.connect();
        if c.is_err() {
            self.forget_one();
        }
        Ok(Conn { db: self, conn: Some(c?) })
    }

    /// A connection given up (closed, or not kept idle).
    fn closed(&self, c: Connection) {
        drop(c);
        self.forget_one();
    }

    fn forget_one(&self) {
        if let Kind::Postgres { .. } = self.kind {
            let (n, cv) = &self.open;
            let mut n = n.lock().unwrap_or_else(|e| e.into_inner());
            *n = n.saturating_sub(1);
            cv.notify_one();
        }
    }

    fn connect(&self) -> anyhow::Result<Connection> {
        Ok(match &self.kind {
            Kind::Sqlite { path, key } => {
                use rusqlite::OpenFlags;
                let c = rusqlite::Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                if let Some(k) = key {
                    c.pragma_update(None, "key", k)?;
                }
                c.busy_timeout(std::time::Duration::from_secs(30))?;
                c.pragma_update(None, "foreign_keys", "ON")?;
                // the mode is kept in the file, set by the first connection to a new catalog; one
                // that sets it while another does is told "locked" at once, without the busy
                // timeout, so it tries again
                let since = std::time::Instant::now();
                loop {
                    match c.pragma_update(None, "journal_mode", "WAL") {
                        Err(rusqlite::Error::SqliteFailure(e, _))
                            if e.code == rusqlite::ErrorCode::DatabaseBusy
                                && since.elapsed() < std::time::Duration::from_secs(30) =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(10))
                        }
                        r => break r?,
                    }
                }
                c.pragma_update(None, "synchronous", "NORMAL")?;
                // reads straight from the file's memory map: with many users at once, about 20
                // times the requests a second of reading through each connection's small page
                // cache (docs/performance.md); address space only, the pages are the system's
                // file cache.  (SQLCipher reads through its cache, which decrypts.)
                c.pragma_update(None, "mmap_size", 1i64 << 30)?;
                Connection::from_sqlite(c)
            }
            Kind::Postgres { url, password, tls } => Connection::postgres(url, password.as_deref(), tls)?,
        })
    }
}

fn schema_version(c: &Connection) -> anyhow::Result<i64> {
    let exists: bool = match c.dialect() {
        Dialect::Sqlite => c.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |r| r.get(0),
        )?,
        Dialect::Postgres => c.query_row("SELECT to_regclass('schema_migrations') IS NOT NULL", [], |r| r.get(0))?,
    };
    if !exists {
        return Ok(0);
    }
    Ok(c.query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |r| r.get(0))?)
}

fn migrate(c: &mut Connection) -> anyhow::Result<()> {
    let pg = c.dialect() == Dialect::Postgres;
    if !pg {
        return migrate_locked(c, pg);
    }
    // several servers may start at once: one migrates, the others wait for it
    let lock = crate::sql::Lock::Migrate as i64;
    c.execute_batch_native(&format!("SELECT pg_advisory_lock({lock})"))?;
    let r = migrate_locked(c, pg);
    c.execute_batch_native(&format!("SELECT pg_advisory_unlock({lock})"))?;
    r
}

fn migrate_locked(c: &mut Connection, pg: bool) -> anyhow::Result<()> {
    c.execute_batch_native(if pg {
        "CREATE TABLE IF NOT EXISTS schema_migrations (version BIGINT PRIMARY KEY, applied_at DOUBLE PRECISION NOT NULL)"
    } else {
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at REAL NOT NULL)"
    })?;
    let list = if pg { PG_MIGRATIONS } else { SQLITE_MIGRATIONS };
    let newest = list.last().map_or(0, |m| m.0);
    for (v, sql) in list {
        // the version read with the write lock held (SQLite: several processes may open a new
        // catalog at once; PostgreSQL: the advisory lock above): one of them applies each
        let tx = c.transaction_with_behavior(crate::sql::TransactionBehavior::Immediate)?;
        let current = schema_version(&tx)?;
        if current > newest {
            anyhow::bail!(
                "the database has schema version {current}, newer than this program knows ({newest}); update medorama"
            );
        }
        if *v <= current {
            continue;
        }
        tx.execute_batch_native(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            crate::sql::params![v, now()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// What `copy_to_postgres` copied: (table, rows), in the order copied.
pub type Copied = Vec<(String, usize)>;

/// Rows read from the source at a time (by id), and written to a statement.
const COPY_RANGE: i64 = 20_000;

/// Copy a SQLite catalog, every table and row, into an empty PostgreSQL catalog (`to`, opened
/// with `create`): the way from one to the other.  Both are at the same schema version; the
/// source is only read.  Tables are copied parents first, in one transaction: an error leaves
/// the target as it was.  Each table is read a range of rows at a time.  A database's table of
/// numbers (`dbcol_<import>`, built from its rows: [`crate::typed`]) is not copied but built
/// again in the target.
pub fn copy_to_postgres(from: &Db, to: &Db) -> anyhow::Result<Copied> {
    if from.dialect() != Dialect::Sqlite || to.dialect() != Dialect::Postgres {
        anyhow::bail!("copies a SQLite catalog into a PostgreSQL one");
    }
    let (src, mut dst) = (from.get()?, to.get()?);
    let (v1, v2) = (schema_version(&src)?, schema_version(&dst)?);
    if v1 != v2 {
        anyhow::bail!("the catalogs have schema versions {v1} and {v2}: open both with this version of medorama first");
    }
    let users: i64 = dst.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
    if users > 0 {
        anyhow::bail!("the PostgreSQL catalog is not empty (it has users)");
    }
    // the tables both have, and their common columns
    let mut st = src.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
           AND name NOT LIKE 'item_text%' AND name NOT LIKE 'dbcol_%' AND name != 'schema_migrations' ORDER BY name",
    )?;
    let names: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let pg_cols = |t: &str| -> anyhow::Result<Vec<String>> {
        let mut st = dst.prepare(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = current_schema()
               AND table_name = ?1 AND is_generated = 'NEVER' AND (is_identity = 'NO' OR identity_generation = 'BY DEFAULT')
             ORDER BY ordinal_position",
        )?;
        Ok(st.query_map([t], |r| r.get(0))?.collect::<Result<_, _>>()?)
    };
    let mut tables: Vec<(String, Vec<String>, Vec<String>)> = vec![]; // (table, columns, parents)
    for t in names {
        let target = pg_cols(&t)?;
        if target.is_empty() {
            anyhow::bail!("the PostgreSQL catalog has no table {t}");
        }
        let mut st = src.prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))?;
        let mine: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let cols: Vec<String> = mine.into_iter().filter(|c| target.contains(c)).collect();
        let mut st = src.prepare(&format!("SELECT DISTINCT \"table\" FROM pragma_foreign_key_list('{t}')"))?;
        let parents: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let parents = parents.into_iter().filter(|p| *p != t).collect();
        tables.push((t, cols, parents));
    }
    // parents first
    let mut order: Vec<(String, Vec<String>)> = vec![];
    while !tables.is_empty() {
        let done: Vec<&str> = order.iter().map(|o| o.0.as_str()).collect();
        let Some(i) = tables.iter().position(|(_, _, p)| p.iter().all(|p| done.contains(&p.as_str()))) else {
            anyhow::bail!(
                "the tables' references form a cycle: {}",
                tables.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(", ")
            );
        };
        let (t, cols, _) = tables.remove(i);
        order.push((t, cols));
    }
    let tx = dst.transaction()?;
    // the rows the schema itself puts in a new catalog: the source's own take their place
    tx.execute("DELETE FROM data_version", [])?;
    tx.execute("DELETE FROM id_counters", [])?;
    let mut out = vec![];
    for (t, cols) in &order {
        let list = cols.join(", ");
        // read in ranges of the table's id (its rowid when it has none), never whole
        let key = if cols.iter().any(|c| c == "id") { "id" } else { "rowid" };
        let (lo, hi): (Option<i64>, Option<i64>) =
            src.query_row(&format!("SELECT MIN({key}), MAX({key}) FROM {t}"), [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut copied = 0;
        let mut st = src.prepare(&format!("SELECT {list} FROM {t} WHERE {key} >= ?1 AND {key} < ?2 ORDER BY {key}"))?;
        let mut at = lo.unwrap_or(0);
        while let Some(hi) = hi.filter(|h| at <= *h) {
            let rows: Vec<Vec<Value>> = st
                .query_map([at, at.saturating_add(COPY_RANGE).min(hi.saturating_add(1))], |r| {
                    (0..cols.len()).map(|i| r.get::<_, Value>(i)).collect::<crate::sql::Result<Vec<_>>>()
                })?
                .collect::<Result<_, _>>()?;
            tx.insert_rows(&format!("INSERT INTO {t} ({list})"), "", &rows)?;
            copied += rows.len();
            at = at.saturating_add(COPY_RANGE);
        }
        if cols.iter().any(|c| c == "id") {
            tx.execute_batch_native(&format!(
                "SELECT setval(pg_get_serial_sequence('{t}', 'id'), COALESCE((SELECT MAX(id) FROM {t}), 0) + 1, false)"
            ))?;
        }
        out.push((t.clone(), copied));
    }
    tx.commit()?;
    // the databases' tables of numbers, built from their rows
    crate::typed::build_missing(&mut dst)?;
    Ok(out)
}

/// An audit entry waiting to be written.
#[derive(Debug, Clone)]
struct Pending {
    time: f64,
    user_id: Option<i64>,
    username: String,
    event: String,
    target: String,
    detail: String,
}

/// Audit entries the database refused (busy past its wait, or failing), in order: written
/// before the next entry is, or by [`flush_audit`] (the sealer, the server as it stops).
static AUDIT_QUEUE: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
/// Where those are also appended at once (the settings' `audit_export`).
static AUDIT_FALLBACK: Mutex<Option<PathBuf>> = Mutex::new(None);

fn set_audit_fallback(p: Option<PathBuf>) {
    *AUDIT_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = p;
}

fn write_audit(c: &Connection, e: &Pending) -> crate::sql::Result<usize> {
    c.execute(
        "INSERT INTO audit (time, user_id, username, event, target, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        crate::sql::params![e.time, e.user_id, e.username, e.event, e.target, e.detail],
    )
}

/// Write the queued entries, in order, as far as the database takes them; returns how many
/// are left.  Not within an open transaction (its rollback would lose them).
fn flush_queue(c: &Connection, q: &mut Vec<Pending>) -> usize {
    if !c.is_autocommit() {
        return q.len();
    }
    let mut done = 0;
    for e in q.iter() {
        if write_audit(c, e).is_err() {
            break;
        }
        done += 1;
    }
    q.drain(..done);
    q.len()
}

/// Write the audit entries that waited (see [`audit`]); returns how many still wait.
pub fn flush_audit(c: &Connection) -> usize {
    let mut q = AUDIT_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_empty() {
        return 0;
    }
    let left = flush_queue(c, &mut q);
    if left == 0 {
        eprintln!("medorama: the audit entries that waited are written");
    }
    left
}

/// An entry the database did not take: queued, appended to the export file when one is set,
/// and said on the server's output (its event and target only).
fn keep_for_later(e: Pending, why: &str) {
    let export = AUDIT_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut also = String::new();
    if let Some(path) = export {
        let line = serde_json::json!({"pending": true, "time": e.time, "user_id": e.user_id, "username": e.username,
                                      "event": e.event, "target": e.target, "detail": e.detail});
        let mut o = std::fs::OpenOptions::new();
        o.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
        match o.open(&path).and_then(|mut f| {
            use std::io::Write;
            f.write_all(format!("{line}\n").as_bytes())?;
            f.sync_data()
        }) {
            Ok(()) => also = format!(", and appended to {}", path.display()),
            Err(err) => also = format!(" ({}: {err})", path.display()),
        }
    }
    eprintln!(
        "medorama: the audit log could not record {} {} now ({why}): kept, to be written when the database takes it{also}",
        e.event, e.target
    );
    AUDIT_QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(e);
}

/// Append to the audit log.  Never lost: an entry the database refuses (busy past its wait, or
/// failing) is kept in order and written before the next one, or by the sealer
/// ([`flush_audit`]); it is appended at once to `audit_export` when that is set, and the
/// server's output says so.
pub fn audit(c: &Connection, user: Option<&crate::users::User>, event: &str, target: &str, detail: &str) {
    let e = Pending {
        time: now(),
        user_id: user.map(|u| u.id),
        username: user.map_or("", |u| u.username.as_str()).to_string(),
        event: event.into(),
        target: target.into(),
        detail: detail.into(),
    };
    {
        let mut q = AUDIT_QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        if !q.is_empty() {
            // the entries before it first (without waiting: they wait for the next chance)
            let left = c.without_waiting(|c| Ok(flush_queue(c, &mut q))).unwrap_or(q.len());
            if left > 0 {
                drop(q);
                keep_for_later(e, "earlier entries still wait");
                return;
            }
        }
    }
    if let Err(err) = write_audit(c, &e) {
        keep_for_later(e, &err.to_string());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn urls_are_shown_without_passwords() {
        assert_eq!(
            super::shown_url("postgres://medorama:secret@db.example/medorama"),
            "postgres://medorama:…@db.example/medorama"
        );
        assert_eq!(
            super::shown_url("postgres://medorama@db.example/medorama"),
            "postgres://medorama@db.example/medorama"
        );
        assert_eq!(super::shown_url("postgres://db.example/medorama"), "postgres://db.example/medorama");
        // a password as a parameter, or in libpq's form
        assert_eq!(
            super::shown_url("postgres://medorama@db.example/medorama?sslmode=require&password=secret"),
            "postgres://medorama@db.example/medorama?sslmode=require&password=…"
        );
        assert_eq!(super::shown_url("host=db user=m password=secret"), "host=db user=m password=…");
        // an @ in a parameter is not the user's
        assert_eq!(
            super::shown_url("postgres://db.example/m?application_name=a@b"),
            "postgres://db.example/m?application_name=a@b"
        );
        assert!(super::has_password("postgres://u:p@h/d") && super::has_password("postgres://h/d?PassWord=x"));
        assert!(!super::has_password("postgres://u@h/d?sslmode=verify-full"));
    }
}
