//! Connected databases: live databases Medorama reads beside its catalog (docs/design/sources.md).
//!
//! A connection names a database (PostgreSQL, or a SQLite file), its kind and settings.  It is
//! read through three views, SQL queries that give the canonical columns: `persons`, `visits`,
//! `entries` ([`omop`] has them built in; a connection of kind `sql` gives its own).  Medorama
//! reads only those columns, opens the database read-only, and never stores its password.
//!
//! | Module | Contents |
//! |---|---|
//! | here | connections: settings, adding, removing, the pools; reading the views; the filters pushed down |
//! | [`omop`] | the OMOP CDM's views |
//! | [`link`] | the person registry: a connection's persons linked to the catalog's patients |

pub mod link;
pub mod omop;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::{audit, now};
use crate::sql::{params, Connection, Dialect, OptionalExtension, Value};
use crate::users::User;

/// A connection's settings (`connections.settings`, JSON).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The schema of the database's tables (OMOP's CDM); empty: the default one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub schema: String,
    /// The schema of OMOP's vocabulary (`concept`), when apart from the CDM's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vocabulary_schema: String,
    /// The time zone of the database's times; empty: the catalog's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    /// What OMOP's `person_source_value` holds: `mrn` (the default), `patient_id` or `none`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub identifier: String,
    /// The authority that issued the database's MRNs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mrn_authority: String,
    /// Each query's time limit, seconds (PostgreSQL); 0: 60.
    #[serde(skip_serializing_if = "is_zero")]
    pub timeout_s: u64,
    /// The rows a criterion may read; 0: 1,000,000.
    #[serde(skip_serializing_if = "is_zero")]
    pub max_rows: u64,
    /// Kind `sql`: the views, as SQL (canonical columns; docs/design/sources.md).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub persons: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub visits: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entries: String,
    /// Kind `sql`: a query whose one value names the data's version (for cohorts).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

fn is_zero(x: &u64) -> bool {
    *x == 0
}

impl Settings {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(if self.timeout_s == 0 { 60 } else { self.timeout_s })
    }
    pub fn max_rows(&self) -> usize {
        if self.max_rows == 0 {
            1_000_000
        } else {
            self.max_rows as usize
        }
    }
}

/// A connected database.
#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Without a password.
    pub url: String,
    pub password_file: String,
    pub ca_file: String,
    pub settings: Settings,
    /// Its batch in the catalog (what it added).
    pub import_id: i64,
    pub created_at: f64,
}

/// The views of a source, in its database's dialect.
#[derive(Debug, Clone)]
pub struct Views {
    pub persons: String,
    pub visits: String,
    pub entries: String,
    pub version: String,
}

const COLS: &str = "id, name, kind, url, password_file, ca_file, settings, import_id, created_at";

fn row_source(r: &crate::sql::Row) -> crate::sql::Result<Source> {
    let settings: String = r.get(6)?;
    Ok(Source {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        url: r.get(3)?,
        password_file: r.get(4)?,
        ca_file: r.get(5)?,
        settings: serde_json::from_str(&settings).unwrap_or_default(),
        import_id: r.get(7)?,
        created_at: r.get(8)?,
    })
}

/// The connected databases, by name.
pub fn list(c: &Connection) -> anyhow::Result<Vec<Source>> {
    let mut st = c.prepare(&format!("SELECT {COLS} FROM connections ORDER BY name"))?;
    Ok(st.query_map([], row_source)?.collect::<Result<_, _>>()?)
}

pub fn get(c: &Connection, id: i64) -> anyhow::Result<Option<Source>> {
    Ok(c.query_row(&format!("SELECT {COLS} FROM connections WHERE id = ?1"), [id], row_source).optional()?)
}

pub fn by_name(c: &Connection, name: &str) -> anyhow::Result<Option<Source>> {
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM connections WHERE lower(name) = lower(?1)",),
        [name.trim()],
        row_source,
    )
    .optional()?)
}

/// The connection whose batch is `import`, if it is one.
pub fn of_import(c: &Connection, import: i64) -> anyhow::Result<Option<Source>> {
    Ok(c.query_row(&format!("SELECT {COLS} FROM connections WHERE import_id = ?1"), [import], row_source).optional()?)
}

/// A new connection.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NewSource {
    pub name: String,
    pub kind: String,
    pub url: String,
    pub password_file: String,
    pub ca_file: String,
    pub settings: Settings,
}

/// Where a connection's password given to Medorama (the page) is kept:
/// `<home>/secrets/<name>.password`, owner-only.
pub fn secret_file(cfg: &crate::config::Config, name: &str) -> std::path::PathBuf {
    cfg.home.join("secrets").join(format!("{name}.password"))
}

/// Connect a database whose password is given (the page): as [`add`], the password kept in
/// [`secret_file`] once the connection is checked and only then (a name already used, or a
/// connection that does not answer, leaves every file as it was).
pub fn add_with_password(
    c: &Connection,
    cfg: &crate::config::Config,
    user: &User,
    n: &NewSource,
    password: &str,
) -> anyhow::Result<i64> {
    if password.is_empty() {
        return add(c, user, n);
    }
    if !user.is_admin() {
        anyhow::bail!("only admins connect databases");
    }
    let name = n.name.trim();
    if !valid_name(name) || by_name(c, name)?.is_some() {
        return add(c, user, n); // its error, before any file is written
    }
    let kept = secret_file(cfg, name);
    let existed = kept.exists();
    let dir = kept.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let given = dir.join(format!(".{name}.password.{}", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    {
        use std::io::Write;
        let mut f = o.open(&given).map_err(|e| anyhow::anyhow!("{}: {e}", given.display()))?;
        f.write_all(password.as_bytes())?;
        f.sync_all()?;
    }
    let mut n = n.clone();
    n.password_file = given.to_string_lossy().to_string();
    let r = add_as(c, user, &n, Some((&given, &kept)));
    if r.is_err() {
        let _ = std::fs::remove_file(&given);
        if c.query_row("SELECT 1 FROM connections WHERE lower(name) = lower(?1)", [name], |r| r.get::<_, i64>(0))
            .optional()?
            .is_none()
            && !existed
            && kept.is_file()
        {
            // renamed before a failure after it: not kept for a connection that is not there
            let _ = std::fs::remove_file(&kept);
        }
    }
    r
}

/// A `sqlite:` URL's file, checked as a connection may name it: absolute, an existing file,
/// inside the folders data may come from (`data_roots`), and never the catalog's own
/// database; returns the URL with its path made absolute.  Other URLs as they are.
pub fn checked_url(cfg: &crate::config::Config, url: &str) -> anyhow::Result<String> {
    let url = url.trim();
    let Some(p) = url.strip_prefix("sqlite://").or_else(|| url.strip_prefix("sqlite:")) else {
        return Ok(url.to_string());
    };
    let path = crate::config::real(std::path::Path::new(&recording_engine::paths::absolute(p)));
    if !path.is_file() {
        anyhow::bail!("no database file {}", path.display());
    }
    if !cfg.import_allowed(&path) {
        anyhow::bail!("the file is outside the folders data may come from (data_roots)");
    }
    if crate::config::real(&cfg.database) == path {
        anyhow::bail!("that is the catalog's own database");
    }
    Ok(format!("sqlite:{}", path.display()))
}

/// A name that criteria text can write before a dot (`omop.lactate`).
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 40
        && n.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        && n.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
}

/// Connect a database (admins): checked first (each view must answer), then kept, with its
/// batch.  Returns its id.
pub fn add(c: &Connection, user: &User, n: &NewSource) -> anyhow::Result<i64> {
    add_as(c, user, n, None)
}

/// `add`; `keep_password`: (the password file written, where it is kept once the connection
/// works: [`add_with_password`]).
fn add_as(
    c: &Connection,
    user: &User,
    n: &NewSource,
    keep_password: Option<(&std::path::Path, &std::path::Path)>,
) -> anyhow::Result<i64> {
    if !user.is_admin() {
        anyhow::bail!("only admins connect databases");
    }
    let name = n.name.trim();
    if !valid_name(name) {
        anyhow::bail!("a connection's name: letters, digits, - and _, starting with a letter (it is written in criteria: {name}.lactate)");
    }
    if by_name(c, name)?.is_some() {
        anyhow::bail!("a connection {name} exists");
    }
    let dup: Option<i64> = c
        .query_row(
            "SELECT id FROM imports WHERE state = 'active' AND lower(CASE WHEN name != '' THEN name ELSE origin END) = lower(?1)",
            [name],
            |r| r.get(0),
        )
        .optional()?;
    if dup.is_some() {
        anyhow::bail!("an imported database is named {name}: choose another name");
    }
    if !matches!(n.kind.as_str(), "omop" | "sql") {
        anyhow::bail!("kind: omop or sql");
    }
    let url = n.url.trim();
    if crate::db::has_password(url) {
        anyhow::bail!("leave the password out of the URL: give the file that holds it (--password-file)");
    }
    for (what, schema) in [("schema", &n.settings.schema), ("vocabulary_schema", &n.settings.vocabulary_schema)] {
        if !omop::valid_schema(schema) {
            anyhow::bail!("{what} {schema:?}: letters, digits and _ only");
        }
    }
    let zone = n.settings.timezone.trim();
    if !zone.is_empty() && zone.parse::<chrono_tz::Tz>().is_err() {
        anyhow::bail!("timezone {zone:?} is not a time zone (an IANA name: Europe/Paris, America/New_York, UTC)");
    }
    if n.kind == "sql" && [&n.settings.persons, &n.settings.entries].iter().any(|v| v.trim().is_empty()) {
        anyhow::bail!("a connection of kind sql needs its views: persons and entries (visits if any)");
    }
    let s = Source {
        id: 0,
        name: name.into(),
        kind: n.kind.clone(),
        url: url.into(),
        password_file: n.password_file.trim().into(),
        ca_file: n.ca_file.trim().into(),
        settings: n.settings.clone(),
        import_id: 0,
        created_at: now(),
    };
    s.check()?;
    // a password given to keep (`add_with_password`): in its place now that it works
    if let Some((given, kept)) = keep_password {
        std::fs::rename(given, kept).map_err(|e| anyhow::anyhow!("{}: {e}", kept.display()))?;
    }
    let stored = match keep_password {
        Some((_, kept)) => kept.to_string_lossy().to_string(),
        None => s.password_file.clone(),
    };
    let import = c.insert(
        "INSERT INTO imports (kind, origin, user_id, time, summary, name, source) VALUES ('connection', ?1, ?2, ?3, '{}', ?4, ?4)",
        params![crate::db::shown_url(url), (user.id > 0).then_some(user.id), now(), name],
    )?;
    let id = c.insert(
        "INSERT INTO connections (name, kind, url, password_file, ca_file, settings, import_id, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            name,
            n.kind,
            url,
            stored,
            s.ca_file,
            serde_json::to_string(&n.settings)?,
            import,
            (user.id > 0).then_some(user.id),
            now()
        ],
    )?;
    audit(c, Some(user), "connect", name, &format!("{} {}", n.kind, crate::db::shown_url(url)));
    Ok(id)
}

/// Disconnect a database (admins): its links and what it added to the catalog go (its batch
/// is removed); the database itself is not touched.
pub fn remove(c: &mut Connection, cfg: &crate::config::Config, user: &User, id: i64) -> anyhow::Result<()> {
    if !user.is_admin() {
        anyhow::bail!("only admins disconnect databases");
    }
    let s = get(c, id)?.ok_or_else(|| anyhow::anyhow!("no connection {id}"))?;
    // (the connection goes with its batch: its pools and the password kept for it too)
    crate::imports::remove_import(c, cfg, user, s.import_id, false, false)?;
    audit(c, Some(user), "disconnect", &s.name, "");
    Ok(())
}

/// What goes with a removed connection on this server: its pooled connections, what was read
/// from it, and the password file kept for it ([`secret_file`]; never a file an admin named).
pub fn removed(cfg: &crate::config::Config, s: &Source) {
    forget(s);
    let f = secret_file(cfg, &s.name);
    if s.password_file == f.to_string_lossy() {
        let _ = std::fs::remove_file(f);
    }
}

// ------------------------------------------------------------------ the databases

/// An open connected database: a few connections to it, reused.
struct Pool {
    idle: Mutex<Vec<Connection>>,
}

/// The pools, by everything that opens a connection (a changed setting opens new ones).
fn pools() -> &'static Mutex<HashMap<String, Arc<Pool>>> {
    static P: OnceLock<Mutex<HashMap<String, Arc<Pool>>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

/// A connection taken from a source's pool; given back when dropped.
pub struct Db {
    pool: Arc<Pool>,
    conn: Option<Connection>,
}

impl std::ops::Deref for Db {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            if c.is_autocommit() && !c.is_closed() {
                let mut idle = self.pool.idle.lock().unwrap();
                if idle.len() < 8 {
                    idle.push(c);
                }
            }
        }
    }
}

/// The environment variable that may hold a connection's password.
pub fn password_var(name: &str) -> String {
    let n: String =
        name.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_uppercase() } else { '_' }).collect();
    format!("MEDORAMA_SOURCE_PASSWORD_{n}")
}

impl Source {
    fn pool_key(&self) -> String {
        format!(
            "{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
            self.id, self.url, self.password_file, self.ca_file, self.settings.timeout_s
        )
    }

    /// The SQLite file of a `sqlite:` URL.
    fn sqlite_path(&self) -> Option<std::path::PathBuf> {
        let u = self.url.trim();
        u.strip_prefix("sqlite://").or_else(|| u.strip_prefix("sqlite:")).map(std::path::PathBuf::from)
    }

    fn open(&self) -> anyhow::Result<Connection> {
        if let Some(path) = self.sqlite_path() {
            if !path.is_file() {
                anyhow::bail!("{}: no such SQLite file", path.display());
            }
            use rusqlite::OpenFlags;
            let c = rusqlite::Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            c.busy_timeout(Duration::from_secs(10))?;
            c.pragma_update(None, "query_only", "ON")?;
            return Ok(Connection::from_sqlite(c));
        }
        let lower = self.url.to_lowercase();
        if !(lower.starts_with("postgres://") || lower.starts_with("postgresql://")) {
            anyhow::bail!("{}: a postgres:// URL or a sqlite: file", self.url);
        }
        let password = match std::env::var(password_var(&self.name)).ok().filter(|p| !p.is_empty()) {
            Some(p) => Some(p),
            None if !self.password_file.is_empty() => Some(
                std::fs::read_to_string(&self.password_file)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.password_file))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        };
        let tls = crate::sql::Tls { ca_file: (!self.ca_file.is_empty()).then(|| self.ca_file.clone().into()) };
        // read only, whatever the account may do; each query within its time limit
        let options =
            format!("-c default_transaction_read_only=on -c statement_timeout={}", self.settings.timeout().as_millis());
        Connection::postgres_with(&self.url, password.as_deref(), &tls, &options)
    }

    /// A connection to the database.
    pub fn db(&self) -> anyhow::Result<Db> {
        let pool = pools()
            .lock()
            .unwrap()
            .entry(self.pool_key())
            .or_insert_with(|| Arc::new(Pool { idle: Mutex::new(vec![]) }))
            .clone();
        let idle = pool.idle.lock().unwrap().pop();
        let conn = match idle {
            Some(c) => c,
            None => self.open().map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?,
        };
        Ok(Db { pool, conn: Some(conn) })
    }

    pub fn dialect(&self) -> Dialect {
        if self.sqlite_path().is_some() {
            Dialect::Sqlite
        } else {
            Dialect::Postgres
        }
    }

    pub fn views(&self) -> Views {
        match self.kind.as_str() {
            "omop" => omop::views(&self.settings, self.dialect()),
            _ => Views {
                persons: self.settings.persons.clone(),
                visits: self.settings.visits.clone(),
                entries: self.settings.entries.clone(),
                version: self.settings.version.clone(),
            },
        }
    }

    pub fn zone(&self, catalog_zone: &str) -> chrono_tz::Tz {
        crate::util::tz(if self.settings.timezone.is_empty() { catalog_zone } else { &self.settings.timezone })
    }

    /// Whether each view answers (one row each) and what the data's version is.
    pub fn check(&self) -> anyhow::Result<Checked> {
        let t = Instant::now();
        let db = self.db()?;
        let v = self.views();
        let mut views = vec![];
        for (name, sql, cols) in [
            ("persons", &v.persons, PERSON_COLS),
            ("visits", &v.visits, VISIT_COLS),
            ("entries", &v.entries, ENTRY_COLS),
        ] {
            if sql.trim().is_empty() {
                continue;
            }
            db.query_row_native(&format!("SELECT COUNT(*) FROM (SELECT {cols} FROM ({sql}) v LIMIT 1) w"), [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|e| anyhow::anyhow!("{}: the view {name}: {e}", self.name))?;
            views.push(name.to_string());
        }
        let version = self.version_of(&db)?;
        Ok(Checked { views, version, ms: t.elapsed().as_millis() as u64 })
    }

    fn version_of(&self, db: &Connection) -> anyhow::Result<String> {
        let sql = self.views().version;
        if sql.trim().is_empty() {
            return Ok(String::new());
        }
        let v: Value = db
            .query_row_native(&sql, [], |r| r.get(0))
            .map_err(|e| anyhow::anyhow!("{}: the version: {e}", self.name))?;
        Ok(text_of(&v).trim().to_string())
    }

    /// The data's version, as the version query says (empty: it has none).
    pub fn version(&self) -> anyhow::Result<String> {
        let db = self.db()?;
        self.version_of(&db)
    }
}

/// What checking a connection found.
#[derive(Debug, Serialize)]
pub struct Checked {
    pub views: Vec<String>,
    pub version: String,
    pub ms: u64,
}

/// Forget a source's pooled connections and what was read from it (removed, or its settings
/// changed).
pub fn forget(s: &Source) {
    let prefix = format!("{}\u{1}", s.id);
    pools().lock().unwrap().retain(|k, _| !k.starts_with(&prefix));
    recent().map.lock().unwrap().retain(|k, _| !k.starts_with(&prefix));
}

/// What was read lately from connected databases, by query: (entries, visits), kept
/// [`Recent::TTL`], at most [`Recent::MAX`] answers (the oldest go first).
struct Recent {
    map: Mutex<HashMap<String, (Instant, (Vec<Entry>, Vec<Visit>))>>,
}

impl Recent {
    const TTL: Duration = Duration::from_secs(30);
    const MAX: usize = 256;

    fn get(&self, key: &str) -> Option<(Vec<Entry>, Vec<Visit>)> {
        let map = self.map.lock().unwrap();
        map.get(key).filter(|(t, _)| t.elapsed() < Self::TTL).map(|(_, v)| v.clone())
    }

    fn put(&self, key: String, v: (Vec<Entry>, Vec<Visit>)) {
        let mut map = self.map.lock().unwrap();
        map.retain(|_, (t, _)| t.elapsed() < Self::TTL);
        if map.len() >= Self::MAX {
            if let Some(oldest) = map.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone()) {
                map.remove(&oldest);
            }
        }
        map.insert(key, (Instant::now(), v));
    }
}

fn recent() -> &'static Recent {
    static R: OnceLock<Recent> = OnceLock::new();
    R.get_or_init(|| Recent { map: Mutex::new(HashMap::new()) })
}

// ------------------------------------------------------------------ reading the views

const PERSON_COLS: &str = "person_key, patient_id, mrn, mrn_authority, last_name, first_name, birth_date, sex";
const VISIT_COLS: &str = "visit_key, person_key, start_time, end_time, unit, icu";
const ENTRY_COLS: &str =
    "entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary, value_num, \
                          value_text, unit, ref_low, ref_high, flag, author, text";

/// A key or a value as text (keys are compared as text in the catalog).
pub fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Integer(i) => i.to_string(),
        Value::Real(x) => {
            if x.fract() == 0.0 && x.abs() < 1e15 {
                format!("{x:.0}")
            } else {
                x.to_string()
            }
        }
        Value::Text(s) => s.clone(),
        Value::Blob(b) => String::from_utf8_lossy(b).to_string(),
    }
}

fn num_of(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Real(x) if x.is_finite() => Some(*x),
        Value::Text(s) => s.trim().parse().ok().filter(|x: &f64| x.is_finite()),
        _ => None,
    }
}

/// A time as the views give it: Unix seconds, or text read in the source's time zone.
pub fn time_of(v: &Value, zone: chrono_tz::Tz) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Real(x) if x.is_finite() => Some(*x),
        Value::Text(s) => {
            let s = s.trim();
            // PostgreSQL's timestamps as text may carry fractions of a second
            let s = match s.split_once('.') {
                Some((a, b)) if a.len() == 19 && b.chars().all(|ch| ch.is_ascii_digit()) => a,
                _ => s,
            };
            crate::util::parse_time(s, zone).ok().flatten()
        }
        _ => None,
    }
}

/// A person of a source, as its view gives them.
#[derive(Debug, Clone, Default)]
pub struct Person {
    pub key: String,
    pub patient_id: String,
    pub mrn: String,
    pub mrn_authority: String,
    pub last_name: String,
    pub first_name: String,
    pub birth_date: String,
    pub sex: String,
}

#[derive(Debug, Clone)]
pub struct Visit {
    pub key: String,
    pub person: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub unit: String,
    pub icu: bool,
}

/// An entry of a source (a result, a drug, a condition, a note …).
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub key: String,
    pub person: String,
    pub visit: String,
    pub category: String,
    pub time: Option<f64>,
    pub end: Option<f64>,
    pub name: String,
    pub code: String,
    pub vocabulary: String,
    pub value_num: Option<f64>,
    pub value_text: String,
    pub unit: String,
    pub ref_low: Option<f64>,
    pub ref_high: Option<f64>,
    pub flag: String,
    pub author: String,
    pub text: String,
}

fn entry_of(r: &crate::sql::Row, zone: chrono_tz::Tz) -> crate::sql::Result<Entry> {
    let g = |i: usize| -> crate::sql::Result<Value> { r.get::<_, Value>(i) };
    let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&g(i)?)) };
    let (category, name, vocabulary, code) = (t(3)?, t(6)?, t(8)?, t(7)?);
    Ok(Entry {
        key: t(0)?,
        person: t(1)?,
        visit: t(2)?,
        category: crate::categories::of_entry(&category, &name, &vocabulary, &code),
        time: time_of(&g(4)?, zone),
        end: time_of(&g(5)?, zone),
        name,
        code,
        vocabulary,
        value_num: num_of(&g(9)?),
        value_text: t(10)?,
        unit: t(11)?,
        ref_low: num_of(&g(12)?),
        ref_high: num_of(&g(13)?),
        flag: t(14)?,
        author: t(15)?,
        text: t(16)?,
    })
}

impl Source {
    /// The persons of these keys.
    pub fn persons(&self, keys: &[String]) -> anyhow::Result<Vec<Person>> {
        let v = self.views().persons;
        let db = self.db()?;
        let rows =
            db.query_in_native(&format!("SELECT {PERSON_COLS} FROM ({v}) p WHERE person_key IN ({{}})"), keys, |r| {
                let t =
                    |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?).trim().to_string()) };
                Ok(Person {
                    key: t(0)?,
                    patient_id: t(1)?,
                    mrn: t(2)?,
                    mrn_authority: t(3)?,
                    last_name: t(4)?,
                    first_name: t(5)?,
                    birth_date: t(6)?,
                    sex: t(7)?,
                })
            })?;
        Ok(rows
            .into_iter()
            .map(|mut p| {
                if p.mrn_authority.is_empty() {
                    p.mrn_authority = self.settings.mrn_authority.clone();
                }
                p
            })
            .collect())
    }

    /// The entries of these persons, in time; kept 30 seconds (a page reads them for its
    /// summary, journey and report at once).
    pub fn entries(&self, keys: &[String], zone: chrono_tz::Tz) -> anyhow::Result<Vec<Entry>> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let key = format!("{}\u{1}{zone}\u{1}{}", self.pool_key(), keys.join("\u{1}"));
        if let Some(v) = recent().get(&key) {
            return Ok(v.0);
        }
        let v = self.entries_now(keys, zone)?;
        recent().put(key, (v.clone(), vec![]));
        Ok(v)
    }

    fn entries_now(&self, keys: &[String], zone: chrono_tz::Tz) -> anyhow::Result<Vec<Entry>> {
        let v = self.views().entries;
        let db = self.db()?;
        let mut out =
            db.query_in_native(&format!("SELECT {ENTRY_COLS} FROM ({v}) e WHERE person_key IN ({{}})"), keys, |r| {
                entry_of(r, zone)
            })?;
        out.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal).then(a.key.cmp(&b.key)));
        Ok(out)
    }

    /// The visits of these persons; kept 30 seconds, as entries are.
    pub fn visits(&self, keys: &[String], zone: chrono_tz::Tz) -> anyhow::Result<Vec<Visit>> {
        let v = self.views().visits;
        if keys.is_empty() || v.trim().is_empty() {
            return Ok(vec![]);
        }
        let key = format!("{}\u{1}visits\u{1}{zone}\u{1}{}", self.pool_key(), keys.join("\u{1}"));
        if let Some(c) = recent().get(&key) {
            return Ok(c.1);
        }
        let db = self.db()?;
        let out =
            db.query_in_native(&format!("SELECT {VISIT_COLS} FROM ({v}) v WHERE person_key IN ({{}})"), keys, |r| {
                visit_of(r, zone)
            })?;
        recent().put(key, (vec![], out.clone()));
        Ok(out)
    }
}

fn visit_of(r: &crate::sql::Row, zone: chrono_tz::Tz) -> crate::sql::Result<Visit> {
    let g = |i: usize| -> crate::sql::Result<Value> { r.get::<_, Value>(i) };
    Ok(Visit {
        key: text_of(&g(0)?),
        person: text_of(&g(1)?),
        start: time_of(&g(2)?, zone),
        end: time_of(&g(3)?, zone),
        unit: text_of(&g(4)?),
        icu: num_of(&g(5)?).is_some_and(|x| x != 0.0),
    })
}

// ------------------------------------------------------------------ the filters pushed down

/// What a criterion asks a source for (cohorts.rs tests the rest: values, units, dates, ages).
#[derive(Debug, Clone, Default)]
pub struct Find {
    /// Entries of these categories (empty: any).
    pub categories: Vec<String>,
    /// Named exactly this (any case).
    pub name: Option<String>,
    /// Whose name contains this.
    pub name_contains: Option<String>,
    /// Whose code starts with this, or whose name contains it (conditions).
    pub code_or_name: Option<String>,
    /// Whose name, value or text contains this.
    pub text: Option<String>,
    /// With a number.
    pub numeric: bool,
    /// Notes too (their text needs the PHI permission).
    pub notes: bool,
}

fn like(s: &str) -> String {
    format!("%{}%", s.trim().to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

impl Source {
    /// The query over the entries for the filters (the vital signs a view calls lab results
    /// included: they are told apart after), and its parameters.
    fn find_sql(&self, f: &Find) -> (String, Vec<Value>) {
        let v = self.views().entries;
        let pg = self.dialect() == Dialect::Postgres;
        let mut wh: Vec<String> = vec![];
        let mut args: Vec<Value> = vec![];
        // (written in the database's own dialect: the views are not translated)
        let arg = |x: Value, args: &mut Vec<Value>| {
            args.push(x);
            if pg {
                format!("${}", args.len())
            } else {
                format!("?{}", args.len())
            }
        };
        // SQLite's LIKE ignores the case of ASCII letters, PostgreSQL's ILIKE of all
        let like_op = if pg { "ILIKE" } else { "LIKE" };
        if !f.categories.is_empty() {
            let mut cats = f.categories.clone();
            if cats.iter().any(|c| c == "vit") && !cats.iter().any(|c| c == "lab") {
                cats.push("lab".into());
            }
            let list: Vec<String> = cats.iter().map(|c| arg(c.clone().into(), &mut args)).collect();
            wh.push(format!("category IN ({})", list.join(", ")));
        }
        if !f.notes {
            wh.push("category != 'note'".into());
        }
        if let Some(n) = &f.name {
            wh.push(format!("lower(name) = lower({})", arg(n.trim().to_string().into(), &mut args)));
        }
        if let Some(n) = &f.name_contains {
            wh.push(format!("lower(name) LIKE {} ESCAPE '\\'", arg(like(n).into(), &mut args)));
        }
        if let Some(t) = &f.code_or_name {
            let prefix = format!("{}%", t.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let a = arg(prefix.into(), &mut args);
            let b = arg(like(t).into(), &mut args);
            wh.push(format!("(code {like_op} {a} ESCAPE '\\' OR lower(name) LIKE {b} ESCAPE '\\')"));
        }
        if let Some(t) = &f.text {
            let a = arg(like(t).into(), &mut args);
            let text = if f.notes { format!(" OR lower(text) LIKE {a} ESCAPE '\\'") } else { String::new() };
            wh.push(format!("(lower(name) LIKE {a} ESCAPE '\\' OR lower(value_text) LIKE {a} ESCAPE '\\'{text})"));
        }
        if f.numeric {
            wh.push("value_num IS NOT NULL".into());
        }
        (
            format!(
                "SELECT {ENTRY_COLS} FROM ({v}) e{}",
                if wh.is_empty() { String::new() } else { format!(" WHERE {}", wh.join(" AND ")) }
            ),
            args,
        )
    }

    /// The entries a criterion's filters find (at most `max_rows`, else an error).
    pub fn find(&self, f: &Find, zone: chrono_tz::Tz) -> anyhow::Result<Vec<Entry>> {
        let (sql, args) = self.find_sql(f);
        let max = self.settings.max_rows();
        let db = self.db()?;
        let mut st = db
            .prepare_native(&format!("{sql} LIMIT {}", max + 1))
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?;
        let rows: Vec<Entry> = st
            .query_map(args, |r| entry_of(r, zone))
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?
            .collect::<Result<_, _>>()
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?;
        if rows.len() > max {
            anyhow::bail!(
                "{}: the criterion reads more than {max} rows: narrow it (a date range, a database)",
                self.name
            );
        }
        Ok(rows.into_iter().filter(|e| f.categories.is_empty() || f.categories.contains(&e.category)).collect())
    }

    /// The persons who have entries the filters find (at most `max_rows`, else an error).
    pub fn find_persons(&self, f: &Find) -> anyhow::Result<Vec<String>> {
        // the filters, as `find` writes them
        let probe = self.find_sql(f);
        let (sql, args) = (format!("SELECT DISTINCT person_key FROM ({}) x", probe.0), probe.1);
        self.keys_of(&sql, args)
    }

    /// The persons who have visits (`icu`: intensive-care ones only).
    pub fn visit_persons(&self, icu: bool) -> anyhow::Result<Vec<String>> {
        let v = self.views().visits;
        if v.trim().is_empty() {
            return Ok(vec![]);
        }
        // (CAST: a view's icu may be a boolean, as PostgreSQL's)
        let sql =
            format!("SELECT DISTINCT person_key FROM ({v}) v{}", if icu { " WHERE CAST(icu AS INT) = 1" } else { "" });
        self.keys_of(&sql, vec![])
    }

    fn keys_of(&self, sql: &str, args: Vec<Value>) -> anyhow::Result<Vec<String>> {
        let max = self.settings.max_rows();
        let db = self.db()?;
        let mut st = db
            .prepare_native(&format!("{sql} LIMIT {}", max + 1))
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?;
        let keys: Vec<String> = st
            .query_map(args, |r| Ok(text_of(&r.get::<_, Value>(0)?)))
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?
            .collect::<Result<_, _>>()
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?;
        if keys.len() > max {
            anyhow::bail!("{}: the criterion finds more than {max} persons: narrow it", self.name);
        }
        Ok(keys)
    }

    /// Every person (with their sex and birth date), at most `max_rows`.
    pub fn all_persons(&self) -> anyhow::Result<Vec<Person>> {
        let v = self.views().persons;
        let max = self.settings.max_rows();
        let db = self.db()?;
        let mut st = db.prepare_native(&format!("SELECT {PERSON_COLS} FROM ({v}) p LIMIT {}", max + 1))?;
        let rows: Vec<Person> = st
            .query_map([], |r| {
                let t =
                    |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?).trim().to_string()) };
                Ok(Person {
                    key: t(0)?,
                    patient_id: t(1)?,
                    mrn: t(2)?,
                    mrn_authority: t(3)?,
                    last_name: t(4)?,
                    first_name: t(5)?,
                    birth_date: t(6)?,
                    sex: t(7)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        if rows.len() > max {
            anyhow::bail!("{}: more than {max} persons: narrow the criteria", self.name);
        }
        Ok(rows)
    }

    /// Every visit, at most `max_rows`.
    pub fn all_visits(&self, zone: chrono_tz::Tz) -> anyhow::Result<Vec<Visit>> {
        let v = self.views().visits;
        if v.trim().is_empty() {
            return Ok(vec![]);
        }
        let max = self.settings.max_rows();
        let db = self.db()?;
        let mut st = db.prepare_native(&format!("SELECT {VISIT_COLS} FROM ({v}) v LIMIT {}", max + 1))?;
        let rows: Vec<Visit> = st.query_map([], |r| visit_of(r, zone))?.collect::<Result<_, _>>()?;
        if rows.len() > max {
            anyhow::bail!("{}: more than {max} visits: narrow the criteria", self.name);
        }
        Ok(rows)
    }

    /// The names of what the source holds, by category, with whether they have numbers: read
    /// once and kept ten minutes (criteria text names them).
    pub fn names(&self) -> anyhow::Result<Arc<Vec<(String, String, bool)>>> {
        type Cached = (Instant, Arc<Vec<(String, String, bool)>>);
        static NAMES: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
        let cache = NAMES.get_or_init(Default::default);
        let key = self.pool_key();
        if let Some((t, v)) = cache.lock().unwrap().get(&key) {
            if t.elapsed() < Duration::from_secs(600) {
                return Ok(v.clone());
            }
        }
        let v = self.views().entries;
        let db = self.db()?;
        let mut st = db.prepare_native(&format!(
            "SELECT category, name, vocabulary, code, MAX(CASE WHEN value_num IS NOT NULL THEN 1 ELSE 0 END)
             FROM ({v}) e WHERE category != 'note' GROUP BY category, name, vocabulary, code"
        ))?;
        let mut names: Vec<(String, String, bool)> = st
            .query_map([], |r| {
                let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?)) };
                let (cat, name, voc, code) = (t(0)?, t(1)?, t(2)?, t(3)?);
                Ok((crate::categories::of_entry(&cat, &name, &voc, &code), name, r.get::<_, i64>(4)? != 0))
            })?
            .collect::<Result<_, _>>()?;
        names.sort();
        names.dedup();
        let names = Arc::new(names);
        cache.lock().unwrap().insert(key, (Instant::now(), names.clone()));
        Ok(names)
    }
}

// ------------------------------------------------------------------ for the pages

/// A connection as admins see it (no secrets: the password file's name only).
#[derive(Debug, Serialize)]
pub struct Shown {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub url: String,
    pub database: &'static str,
    pub import_id: i64,
    pub persons: i64,
    pub created_at: f64,
    pub settings: Settings,
}

pub fn shown(c: &Connection) -> anyhow::Result<Vec<Shown>> {
    let mut out = vec![];
    for s in list(c)? {
        let persons: i64 =
            c.query_row("SELECT COUNT(*) FROM source_persons WHERE connection = ?1", [s.id], |r| r.get(0))?;
        out.push(Shown {
            id: s.id,
            database: if s.dialect() == Dialect::Sqlite { "SQLite" } else { "PostgreSQL" },
            name: s.name,
            kind: s.kind,
            url: s.url,
            import_id: s.import_id,
            persons,
            created_at: s.created_at,
            settings: s.settings,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_times_and_secrets() {
        assert!(valid_name("omop") && valid_name("warehouse_2") && !valid_name("2x") && !valid_name("a.b"));
        assert_eq!(password_var("omop-cdm"), "MEDORAMA_SOURCE_PASSWORD_OMOP_CDM");
        let z = crate::util::tz("UTC");
        assert_eq!(time_of(&Value::Text("2024-01-02 03:04:05.123456".into()), z), Some(1704164645.0));
        assert_eq!(time_of(&Value::Text("2024-01-02".into()), z), Some(1704153600.0));
        assert_eq!(time_of(&Value::Real(5.0), z), Some(5.0));
        assert_eq!(text_of(&Value::Real(42.0)), "42");
    }
}
