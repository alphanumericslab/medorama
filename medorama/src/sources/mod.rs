//! Connected databases as Medorama reads them (docs/design/databases-only.md, "Kinds of
//! databases"): the databases Medorama reads in place, read-only, never writing them.
//!
//! A database (PostgreSQL, or a SQLite file) is read through views, SQL queries that give the
//! canonical columns: `persons`, `visits`, `entries`, and `recordings` for a recording database;
//! `stays` (a visit's stays in its units: the intensive-care ones) when the database has them.
//! [`omop`] has them built in, a db-builder database (kind `dbb`) has them as tables, and a
//! database of kind `views` gives its own.  Medorama reads only those columns.  Connecting,
//! status, grants and removal are in `databases.rs`.
//!
//! | Module | Contents |
//! |---|---|
//! | here | settings, the pools; reading the views; the filters pushed down |
//! | [`omop`] | the OMOP CDM's views |

pub mod omop;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::db::now;
use crate::sql::{Connection, Dialect, OptionalExtension, Value};

/// A database's settings (`databases.settings`, JSON).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The schema of the database's tables (OMOP's CDM); empty: the default one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub schema: String,
    /// The schema of OMOP's vocabulary (`concept`), when apart from the CDM's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub vocabulary_schema: String,
    /// The time zone of the database's times; empty: Medorama's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timezone: String,
    /// What OMOP's `person_source_value` holds: `mrn` (the default), `patient_id` or `none`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub identifier: String,
    /// The authority that issued the database's MRNs (when a row names none).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mrn_authority: String,
    /// Who issued the database's patient IDs (when a row names none).  Empty: the database
    /// itself, so that its patient IDs link only its own persons.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub issuer: String,
    /// The folder a recording's relative `file` is read from; empty: a SQLite database's own
    /// folder.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub files_root: String,
    /// Each query's time limit, seconds (PostgreSQL); 0: 60.
    #[serde(skip_serializing_if = "is_zero")]
    pub timeout_s: u64,
    /// The rows a criterion may read; 0: 1,000,000.
    #[serde(skip_serializing_if = "is_zero")]
    pub max_rows: u64,
    /// Kind `views`: the views, as SQL (canonical columns; docs/databases.md).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub persons: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub visits: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub entries: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub recordings: String,
    /// Kind `views`, optional: a visit's stays in its units (bed stays, transfers).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stays: String,
    /// Kind `views`: a query whose one value names the data's version (for cohorts).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// Kind `views`, optional: what the database holds, one row per variable (category, name,
    /// vocabulary, code, unit, numeric), so that connecting reads no entry; empty: the names
    /// and units are read from the entries.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub variables: String,
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
    /// The password given to Medorama, wrapped by the master key (workspace.rs); never shown.
    #[serde(skip)]
    pub password_wrapped: String,
    pub ca_file: String,
    pub settings: Settings,
    pub created_at: f64,
}

/// The views of a source, in its database's dialect.
#[derive(Debug, Clone)]
pub struct Views {
    pub persons: String,
    pub visits: String,
    pub entries: String,
    pub recordings: String,
    /// A visit's stays in its units (optional: empty when the database has none).
    pub stays: String,
    pub version: String,
    /// What the database holds, one row per variable (optional: empty, read from the entries).
    pub variables: String,
}

const COLS: &str = "id, name, kind, url, password_file, password_wrapped, ca_file, settings, created_at";

fn row_source(r: &crate::sql::Row) -> crate::sql::Result<Source> {
    let settings: String = r.get(7)?;
    Ok(Source {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        url: r.get(3)?,
        password_file: r.get(4)?,
        password_wrapped: r.get(5)?,
        ca_file: r.get(6)?,
        settings: serde_json::from_str(&settings).unwrap_or_default(),
        created_at: r.get(8)?,
    })
}

/// The connected databases, by name (those being removed left out).
pub fn list(c: &Connection) -> anyhow::Result<Vec<Source>> {
    let mut st = c.prepare(&format!("SELECT {COLS} FROM databases WHERE status != 'removing' ORDER BY name"))?;
    Ok(st.query_map([], row_source)?.collect::<Result<_, _>>()?)
}

pub fn get(c: &Connection, id: i64) -> anyhow::Result<Option<Source>> {
    Ok(c.query_row(&format!("SELECT {COLS} FROM databases WHERE id = ?1 AND status != 'removing'"), [id], row_source)
        .optional()?)
}

/// A database by id, also while it is being removed.
pub fn get_any(c: &Connection, id: i64) -> anyhow::Result<Option<Source>> {
    Ok(c.query_row(&format!("SELECT {COLS} FROM databases WHERE id = ?1"), [id], row_source).optional()?)
}

pub fn by_name(c: &Connection, name: &str) -> anyhow::Result<Option<Source>> {
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM databases WHERE lower(name) = lower(?1) AND status != 'removing'",),
        [name.trim()],
        row_source,
    )
    .optional()?)
}

/// A database to connect.
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

impl NewSource {
    /// The settings' rules, before anything is read.
    pub fn validate(&self) -> anyhow::Result<()> {
        let name = self.name.trim();
        if !valid_name(name) {
            anyhow::bail!("a database's name: letters, digits, - and _, starting with a letter (it is written in criteria: {name}.lactate)");
        }
        if !matches!(self.kind.as_str(), "omop" | "views" | "dbb") {
            anyhow::bail!("kind: omop, views or dbb (a db-builder database)");
        }
        if crate::db::has_password(self.url.trim()) {
            anyhow::bail!("leave the password out of the URL: give it apart (the page, --password-file)");
        }
        for (what, schema) in
            [("schema", &self.settings.schema), ("vocabulary_schema", &self.settings.vocabulary_schema)]
        {
            if !omop::valid_schema(schema) {
                anyhow::bail!("{what} {schema:?}: letters, digits and _ only");
            }
        }
        let zone = self.settings.timezone.trim();
        if !zone.is_empty() && zone.parse::<chrono_tz::Tz>().is_err() {
            anyhow::bail!("timezone {zone:?} is not a time zone (an IANA name: Europe/Paris, America/New_York, UTC)");
        }
        if self.kind == "views" && [&self.settings.persons, &self.settings.entries].iter().any(|v| v.trim().is_empty())
        {
            anyhow::bail!("a database of kind views needs its views: persons and entries (visits, recordings if any)");
        }
        if self.kind == "dbb" && !self.url.trim().starts_with("sqlite:") {
            anyhow::bail!("a db-builder database is a SQLite file: sqlite:/path/to/file.sqlite");
        }
        Ok(())
    }

    /// As a source, to check before it is kept.
    pub fn source(&self, password_wrapped: String) -> Source {
        Source {
            id: 0,
            name: self.name.trim().into(),
            kind: self.kind.clone(),
            url: self.url.trim().into(),
            password_file: self.password_file.trim().into(),
            password_wrapped,
            ca_file: self.ca_file.trim().into(),
            settings: self.settings.clone(),
            created_at: now(),
        }
    }
}

/// A `sqlite:` URL's file, checked as a database may name it: absolute, an existing file,
/// inside the folders data may come from (`data_roots`), and never Medorama's own control store
/// or a file of its workspace; returns the URL with its path made absolute.  Other URLs as
/// they are.
pub fn checked_url(cfg: &crate::config::Config, url: &str) -> anyhow::Result<String> {
    let url = url.trim();
    let Some(p) = url.strip_prefix("sqlite://").or_else(|| url.strip_prefix("sqlite:")) else {
        return Ok(url.to_string());
    };
    let path = crate::config::real(std::path::Path::new(&recording_engine::paths::absolute(p)));
    if !path.is_file() {
        anyhow::bail!("no database file {}", path.display());
    }
    if !cfg.in_data_roots(&path) {
        anyhow::bail!("the file is outside the folders data may come from (data_roots)");
    }
    if crate::config::real(&cfg.database) == path {
        anyhow::bail!("that is Medorama's own control store");
    }
    if crate::config::within(&path, &crate::config::real(&cfg.workspace_dir)) {
        anyhow::bail!("that file is in Medorama's workspace");
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
    /// What a pooled connection was opened with: another password (or key) is another pool.
    fn pool_key(&self) -> String {
        format!(
            "{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
            self.id, self.url, self.password_file, self.password_wrapped, self.ca_file, self.settings.timeout_s
        )
    }

    /// The SQLite file of a `sqlite:` URL.
    fn sqlite_path(&self) -> Option<std::path::PathBuf> {
        let u = self.url.trim();
        u.strip_prefix("sqlite://").or_else(|| u.strip_prefix("sqlite:")).map(std::path::PathBuf::from)
    }

    /// The database's password: from `$MEDORAMA_SOURCE_PASSWORD_<NAME>`, else the one given to
    /// Medorama (kept sealed), else its password file.
    fn password(&self) -> anyhow::Result<Option<String>> {
        Ok(match std::env::var(password_var(&self.name)).ok().filter(|p| !p.is_empty()) {
            Some(p) => Some(p),
            None if !self.password_wrapped.is_empty() => {
                Some(crate::workspace::unwrap_secret(&format!("password:{}", self.id), &self.password_wrapped)?)
            }
            None if !self.password_file.is_empty() => Some(
                std::fs::read_to_string(&self.password_file)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", self.password_file))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        })
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
            // a SQLite database encrypted with SQLCipher (db-builder's --encrypt): its password
            // is its key, 64 hex digits a raw key (as db-builder writes it), else a passphrase
            if let Some(k) = self.password()? {
                if k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit()) {
                    c.execute_batch(&format!("PRAGMA key = \"x'{k}'\";"))?;
                } else {
                    c.pragma_update(None, "key", &k)?;
                }
                c.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(()))
                    .map_err(|_| anyhow::anyhow!("{}: cannot be read with this key", path.display()))?;
            }
            c.busy_timeout(Duration::from_secs(10))?;
            c.pragma_update(None, "query_only", "ON")?;
            return Ok(Connection::from_sqlite(c));
        }
        let lower = self.url.to_lowercase();
        if !(lower.starts_with("postgres://") || lower.starts_with("postgresql://")) {
            anyhow::bail!("{}: a postgres:// URL or a sqlite: file", self.url);
        }
        let password = self.password()?;
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
            // a db-builder database: its tables are the views (db-builder/docs/schema.md)
            "dbb" => Views {
                persons: format!("SELECT {PERSON_COLS} FROM persons"),
                visits: format!("SELECT {VISIT_COLS} FROM visits"),
                entries: format!("SELECT {ENTRY_COLS} FROM entries"),
                recordings: format!("SELECT {RECORDING_COLS} FROM recordings"),
                stays: DBB_STAYS.into(),
                version: "SELECT value FROM meta WHERE key = 'version'".into(),
                variables: String::new(),
            },
            _ => Views {
                persons: self.settings.persons.clone(),
                visits: self.settings.visits.clone(),
                entries: self.settings.entries.clone(),
                recordings: self.settings.recordings.clone(),
                stays: self.settings.stays.clone(),
                version: self.settings.version.clone(),
                variables: self.settings.variables.clone(),
            },
        }
    }

    /// A recording's file: as the database names it, or read from `files_root` (else a SQLite
    /// database's folder) when relative.
    pub fn recording_path(&self, file: &str) -> std::path::PathBuf {
        let f = std::path::Path::new(file);
        if f.is_absolute() {
            return f.to_path_buf();
        }
        let root = if !self.settings.files_root.is_empty() {
            std::path::PathBuf::from(&self.settings.files_root)
        } else {
            match self.sqlite_path().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
                Some(d) => d,
                None => std::path::PathBuf::from("."),
            }
        };
        root.join(f)
    }

    pub fn zone(&self, medorama_zone: &str) -> chrono_tz::Tz {
        crate::util::tz(if self.settings.timezone.is_empty() { medorama_zone } else { &self.settings.timezone })
    }

    /// Whether each view answers (one row each) and what the data's version is.
    pub fn check(&self) -> anyhow::Result<Checked> {
        let t = Instant::now();
        // a SQLite file moved or deleted: a pooled connection would still read it
        if let Some(p) = self.sqlite_path() {
            if !p.is_file() {
                forget(self);
                anyhow::bail!("{}: no such SQLite file", p.display());
            }
        }
        let db = self.db()?;
        let v = self.views();
        let none = String::new();
        let mut views = vec![];
        for (name, sql, cols) in [
            ("persons", &v.persons, PERSON_COLS),
            ("visits", &v.visits, VISIT_COLS),
            ("entries", &v.entries, ENTRY_COLS),
            ("recordings", &v.recordings, RECORDING_COLS),
            // (stays are optional: only a site's own view is checked)
            ("stays", if self.kind == "views" { &v.stays } else { &none }, STAY_COLS),
            ("variables", &v.variables, VARIABLE_COLS),
        ] {
            // a db-builder database without recordings, or recordings without entries
            if self.kind == "dbb" && !self.has_table(&db, name)? {
                continue;
            }
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

impl Source {
    /// Whether a SQLite database has a table (a db-builder database may hold no recordings).
    fn has_table(&self, db: &Connection, table: &str) -> anyhow::Result<bool> {
        Ok(db
            .query_row_native("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?
            > 0)
    }

    /// Whether it is a recording database (it has the `recordings` view).
    pub fn has_recordings(&self) -> bool {
        if self.kind == "dbb" {
            return self.db().and_then(|db| self.has_table(&db, "recordings")).unwrap_or(false);
        }
        !self.views().recordings.trim().is_empty()
    }
}

/// What checking a database found.
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

/// What was read lately from connected databases, by query: (entries, visits, stays), kept
/// [`Recent::TTL`], at most [`Recent::MAX`] answers (the oldest go first).
struct Recent {
    map: Mutex<HashMap<String, (Instant, (Vec<Entry>, Vec<Visit>, Vec<Stay>))>>,
}

impl Recent {
    const TTL: Duration = Duration::from_secs(30);
    const MAX: usize = 256;

    fn get(&self, key: &str) -> Option<(Vec<Entry>, Vec<Visit>, Vec<Stay>)> {
        let map = self.map.lock().unwrap();
        map.get(key).filter(|(t, _)| t.elapsed() < Self::TTL).map(|(_, v)| v.clone())
    }

    fn put(&self, key: String, v: (Vec<Entry>, Vec<Visit>, Vec<Stay>)) {
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

const PERSON_COLS: &str =
    "person_key, patient_id, patient_id_issuer, mrn, mrn_authority, last_name, first_name, birth_date, sex";
const VISIT_COLS: &str = "visit_key, person_key, start_time, end_time, unit, icu";
const ENTRY_COLS: &str =
    "entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary, value_num, \
                          value_text, unit, ref_low, ref_high, flag, author, text";
const STAY_COLS: &str = "stay_key, person_key, visit_key, start_time, end_time, unit, icu";
/// A db-builder database's stays: its bed stays, kept in `extras` (db-builder/docs/schema.md,
/// "extras"), with their visits' persons.
const DBB_STAYS: &str = "SELECT b.stay_key, v.person_key, b.visit_key, b.start_time, b.end_time, b.unit, 0 AS icu
     FROM (SELECT row_key AS stay_key, MAX(CASE WHEN name = 'visit_key' THEN value END) AS visit_key,
                  MAX(CASE WHEN name = 'bed_start' THEN value END) AS start_time,
                  MAX(CASE WHEN name = 'bed_end' THEN value END) AS end_time,
                  COALESCE(MAX(CASE WHEN name = 'care_unit' THEN value END), '') AS unit
           FROM extras WHERE table_name = 'bed_stays' GROUP BY row_key) b
     JOIN visits v ON v.visit_key = b.visit_key";
/// The optional `variables` view: one row per variable the database holds.
pub const VARIABLE_COLS: &str = "category, name, vocabulary, code, unit, numeric";
const RECORDING_COLS: &str =
    "recording_key, person_key, visit_key, file, format, start_time, end_time, label, category, size, fingerprint";

/// A key or a value as text (keys are compared as text in Medorama).
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
    pub patient_id_issuer: String,
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

/// A stay in a unit during a visit (a bed stay, OMOP's `visit_detail`).
#[derive(Debug, Clone)]
pub struct Stay {
    pub key: String,
    pub person: String,
    pub visit: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub unit: String,
    /// In an intensive-care unit: by the database's say, or by the unit's name.
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
                    patient_id_issuer: t(2)?,
                    mrn: t(3)?,
                    mrn_authority: t(4)?,
                    last_name: t(5)?,
                    first_name: t(6)?,
                    birth_date: t(7)?,
                    sex: t(8)?,
                })
            })?;
        Ok(rows.into_iter().map(|p| self.with_issuers(p)).collect())
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
        recent().put(key, (v.clone(), vec![], vec![]));
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
        recent().put(key, (vec![], out.clone(), vec![]));
        Ok(out)
    }

    /// The stays of these persons in their visits' units (empty when the database gives none);
    /// kept 30 seconds, as entries are.
    pub fn stays(&self, keys: &[String], zone: chrono_tz::Tz) -> anyhow::Result<Vec<Stay>> {
        let v = self.views().stays;
        if keys.is_empty() || v.trim().is_empty() {
            return Ok(vec![]);
        }
        let key = format!("{}\u{1}stays\u{1}{zone}\u{1}{}", self.pool_key(), keys.join("\u{1}"));
        if let Some(c) = recent().get(&key) {
            return Ok(c.2);
        }
        let db = self.db()?;
        // a db-builder database made before bed stays were kept has no extras
        if self.kind == "dbb" && !self.has_table(&db, "extras")? {
            return Ok(vec![]);
        }
        let out =
            db.query_in_native(&format!("SELECT {STAY_COLS} FROM ({v}) s WHERE person_key IN ({{}})"), keys, |r| {
                stay_of(r, zone)
            })?;
        recent().put(key, (vec![], vec![], out.clone()));
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

fn stay_of(r: &crate::sql::Row, zone: chrono_tz::Tz) -> crate::sql::Result<Stay> {
    let g = |i: usize| -> crate::sql::Result<Value> { r.get::<_, Value>(i) };
    let unit = text_of(&g(5)?);
    Ok(Stay {
        key: text_of(&g(0)?),
        person: text_of(&g(1)?),
        visit: text_of(&g(2)?),
        start: time_of(&g(3)?, zone),
        end: time_of(&g(4)?, zone),
        icu: num_of(&g(6)?).is_some_and(|x| x != 0.0) || crate::categories::is_icu_unit(&unit),
        unit,
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

/// Whether a diagnosis's text is a word to find in names (`sepsis`, `heart failure`): it has a
/// letter and three characters, and is not shaped like a code (ICD-10's `I21.9`, `E11`;
/// ICD-9's `410.1`, `V45.81`).
pub fn names_a_word(text: &str) -> bool {
    let t = text.trim();
    let cs: Vec<char> = t.chars().collect();
    let code_like = !cs.is_empty()
        && cs.iter().all(|c| c.is_ascii_alphanumeric() || *c == '.')
        && (cs[0].is_ascii_digit() || (cs[0].is_ascii_alphabetic() && cs.get(1).is_some_and(|c| c.is_ascii_digit())))
        && cs.len() <= 8;
    cs.len() >= 3 && cs.iter().any(|c| c.is_alphabetic()) && !code_like
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
            // codes compared without their dots (I21.9 is I219), the text's prefix in upper case;
            // names only for words (`sepsis`), not for codes (`I21`, `410.1`)
            let dotless: String = t.trim().replace('.', "").to_uppercase();
            let prefix = format!("{}%", dotless.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let a = arg(prefix.into(), &mut args);
            let code = format!("upper(replace(code, '.', '')) LIKE {a} ESCAPE '\\'");
            if names_a_word(t) {
                let b = arg(like(t).into(), &mut args);
                wh.push(format!("({code} OR lower(name) LIKE {b} ESCAPE '\\')"));
            } else {
                wh.push(code);
            }
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

    /// The persons who have visits (`icu`: intensive-care ones only: a visit the database says
    /// is, or in a unit named like an intensive-care unit, as the encounter criterion and the
    /// patient's page read them; or with a stay in one).
    pub fn visit_persons(&self, icu: bool) -> anyhow::Result<Vec<String>> {
        let views = self.views();
        let v = views.visits;
        if v.trim().is_empty() {
            return Ok(vec![]);
        }
        if !icu {
            return self.keys_of(&format!("SELECT DISTINCT person_key FROM ({v}) v"), vec![]);
        }
        // (CAST: a view's icu may be a boolean, as PostgreSQL's; the units' names are tested here)
        let mut keys = self.icu_keys(&format!("SELECT DISTINCT person_key, unit, CAST(icu AS INT) FROM ({v}) v"))?;
        let stays = views.stays;
        if !stays.trim().is_empty() {
            let sql = format!("SELECT DISTINCT person_key, unit, CAST(icu AS INT) FROM ({stays}) s");
            match self.icu_keys(&sql) {
                Ok(k) => keys.extend(k),
                // (stays are optional: a database without them is read without)
                Err(e) if self.kind != "views" => eprintln!("medorama: {}: stays: {e}", self.name),
                Err(e) => return Err(e),
            }
        }
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// The persons of rows (person_key, unit, icu) in intensive care.
    fn icu_keys(&self, sql: &str) -> anyhow::Result<Vec<String>> {
        let max = self.settings.max_rows();
        let db = self.db()?;
        if self.kind == "dbb" && sql.contains("FROM extras") && !self.has_table(&db, "extras")? {
            return Ok(vec![]);
        }
        let mut st = db
            .prepare_native(&format!("{sql} LIMIT {}", max + 1))
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?;
        let rows: Vec<(String, String, bool)> = st
            .query_map(Vec::<Value>::new(), |r| {
                let g = |i: usize| -> crate::sql::Result<Value> { r.get::<_, Value>(i) };
                Ok((text_of(&g(0)?), text_of(&g(1)?), num_of(&g(2)?).is_some_and(|x| x != 0.0)))
            })
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?
            .collect::<Result<_, _>>()
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.name))?;
        if rows.len() > max {
            anyhow::bail!("{}: the criterion finds more than {max} persons: narrow it", self.name);
        }
        Ok(rows
            .into_iter()
            .filter(|(p, unit, icu)| !p.is_empty() && (*icu || crate::categories::is_icu_unit(unit)))
            .map(|(p, ..)| p)
            .collect())
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
        let max = self.settings.max_rows();
        let rows = self.persons_limited(Some(max + 1))?;
        if rows.len() > max {
            anyhow::bail!("{}: more than {max} persons: narrow the criteria", self.name);
        }
        Ok(rows)
    }

    /// Every person, however many: the subject index (persons.rs), which no criterion bounds.
    pub fn every_person(&self) -> anyhow::Result<Vec<Person>> {
        self.persons_limited(None)
    }

    fn persons_limited(&self, limit: Option<usize>) -> anyhow::Result<Vec<Person>> {
        let v = self.views().persons;
        let db = self.db()?;
        let limit = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
        let mut st = db.prepare_native(&format!("SELECT {PERSON_COLS} FROM ({v}) p{limit}"))?;
        let rows: Vec<Person> = st
            .query_map([], |r| {
                let t =
                    |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?).trim().to_string()) };
                Ok(Person {
                    key: t(0)?,
                    patient_id: t(1)?,
                    patient_id_issuer: t(2)?,
                    mrn: t(3)?,
                    mrn_authority: t(4)?,
                    last_name: t(5)?,
                    first_name: t(6)?,
                    birth_date: t(7)?,
                    sex: t(8)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows.into_iter().map(|p| self.with_issuers(p)).collect())
    }

    /// Every visit, at most `max_rows`.
    pub fn all_visits(&self, zone: chrono_tz::Tz) -> anyhow::Result<Vec<Visit>> {
        let max = self.settings.max_rows();
        let rows = self.visits_limited(zone, Some(max + 1))?;
        if rows.len() > max {
            anyhow::bail!("{}: more than {max} visits: narrow the criteria", self.name);
        }
        Ok(rows)
    }

    /// Every visit, however many: the subject index's encounters (persons.rs).
    pub fn every_visit(&self, zone: chrono_tz::Tz) -> anyhow::Result<Vec<Visit>> {
        self.visits_limited(zone, None)
    }

    fn visits_limited(&self, zone: chrono_tz::Tz, limit: Option<usize>) -> anyhow::Result<Vec<Visit>> {
        let v = self.views().visits;
        if v.trim().is_empty() {
            return Ok(vec![]);
        }
        let db = self.db()?;
        let limit = limit.map(|n| format!(" LIMIT {n}")).unwrap_or_default();
        let mut st = db.prepare_native(&format!("SELECT {VISIT_COLS} FROM ({v}) v{limit}"))?;
        Ok(st.query_map([], |r| visit_of(r, zone))?.collect::<Result<_, _>>()?)
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

// ------------------------------------------------------------------ recordings

/// A recording of a recording database, as its view gives it.
#[derive(Debug, Clone, Default)]
pub struct Recording {
    pub key: String,
    pub person: String,
    pub visit: String,
    pub file: String,
    pub format: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub label: String,
    pub category: String,
    pub size: Option<f64>,
    pub fingerprint: String,
}

fn recording_of(r: &crate::sql::Row, zone: chrono_tz::Tz) -> crate::sql::Result<Recording> {
    let g = |i: usize| -> crate::sql::Result<Value> { r.get::<_, Value>(i) };
    let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&g(i)?).trim().to_string()) };
    Ok(Recording {
        key: t(0)?,
        person: t(1)?,
        visit: t(2)?,
        file: t(3)?,
        format: t(4)?,
        start: time_of(&g(5)?, zone),
        end: time_of(&g(6)?, zone),
        label: t(7)?,
        category: t(8)?,
        size: num_of(&g(9)?),
        fingerprint: t(10)?,
    })
}

impl Source {
    /// The issuers a row leaves empty: the database's settings; a patient ID with no issuer at
    /// all is the database's own (it links only its own persons).
    fn with_issuers(&self, mut p: Person) -> Person {
        if p.mrn_authority.is_empty() {
            p.mrn_authority = self.settings.mrn_authority.clone();
        }
        if p.patient_id_issuer.is_empty() {
            p.patient_id_issuer = if self.settings.issuer.is_empty() {
                format!("db:{}", self.name)
            } else {
                self.settings.issuer.clone()
            };
        }
        p
    }

    /// Every recording (a recording database), at most `max_rows`.
    pub fn all_recordings(&self, zone: chrono_tz::Tz) -> anyhow::Result<Vec<Recording>> {
        let v = self.views().recordings;
        if v.trim().is_empty() || !self.has_recordings() {
            return Ok(vec![]);
        }
        let max = self.settings.max_rows();
        let db = self.db()?;
        let mut st = db.prepare_native(&format!("SELECT {RECORDING_COLS} FROM ({v}) r LIMIT {}", max + 1))?;
        let rows: Vec<Recording> = st.query_map([], |r| recording_of(r, zone))?.collect::<Result<_, _>>()?;
        if rows.len() > max {
            anyhow::bail!("{}: more than {max} recordings", self.name);
        }
        Ok(rows)
    }

    /// The recordings of these persons.
    pub fn recordings(&self, keys: &[String], zone: chrono_tz::Tz) -> anyhow::Result<Vec<Recording>> {
        let v = self.views().recordings;
        if keys.is_empty() || v.trim().is_empty() || !self.has_recordings() {
            return Ok(vec![]);
        }
        let db = self.db()?;
        let mut out = db.query_in_native(
            &format!("SELECT {RECORDING_COLS} FROM ({v}) r WHERE person_key IN ({{}})"),
            keys,
            |r| recording_of(r, zone),
        )?;
        out.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal).then(a.key.cmp(&b.key)));
        Ok(out)
    }

    /// The persons with a recording of this category (cohorts' `has`), read from the view.
    pub fn recording_persons(&self, category: &str) -> anyhow::Result<Vec<String>> {
        let v = self.views().recordings;
        if v.trim().is_empty() || !self.has_recordings() {
            return Ok(vec![]);
        }
        let db = self.db()?;
        let mut st = db.prepare_native(&format!(
            "SELECT DISTINCT person_key, category FROM ({v}) r WHERE person_key IS NOT NULL"
        ))?;
        let rows: Vec<(String, String)> = st
            .query_map([], |r| Ok((text_of(&r.get::<_, Value>(0)?), text_of(&r.get::<_, Value>(1)?))))?
            .collect::<Result<_, _>>()?;
        Ok(rows
            .into_iter()
            .filter(|(_, c)| {
                let c = if c.trim().is_empty() { "icu" } else { crate::categories::named(c).unwrap_or("icu") };
                c == category
            })
            .map(|(p, _)| p)
            .collect())
    }

    /// The persons who have data of a category (`has`: cohorts, the patient list's filters):
    /// visits for `enc` and `icu`, the entries of the category (lab results and vital signs told
    /// apart by their names, so read as rows), and the recordings of the category.
    pub fn persons_with(&self, category: &str, zone: chrono_tz::Tz) -> anyhow::Result<Vec<String>> {
        let mut keys = match category {
            "icu" => self.visit_persons(true)?,
            "enc" => self.visit_persons(false)?,
            "lab" | "vit" => self
                .find(&Find { categories: vec![category.into()], ..Default::default() }, zone)?
                .into_iter()
                .map(|e| e.person)
                .collect(),
            _ => self.find_persons(&Find {
                categories: vec![category.into()],
                notes: category == "note",
                ..Default::default()
            })?,
        };
        keys.extend(self.recording_persons(category)?);
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /// What the patient list shows of these persons (one page of it), read now: person key →
    /// (the categories they have data of, their entries, the latest time of an entry).
    pub fn summaries(
        &self,
        keys: &[String],
        zone: chrono_tz::Tz,
    ) -> anyhow::Result<HashMap<String, (std::collections::BTreeSet<String>, i64, Option<f64>)>> {
        let mut out: HashMap<String, (std::collections::BTreeSet<String>, i64, Option<f64>)> = HashMap::new();
        if keys.is_empty() {
            return Ok(out);
        }
        let db = self.db()?;
        let v = self.views().entries;
        let rows = db.query_in_native(
            &format!(
                "SELECT person_key, category, name, vocabulary, code, COUNT(*), MAX(time) FROM ({v}) e
                 WHERE person_key IN ({{}}) GROUP BY person_key, category, name, vocabulary, code"
            ),
            keys,
            |r| {
                let t = |i: usize| -> crate::sql::Result<String> { Ok(text_of(&r.get::<_, Value>(i)?)) };
                let n = match r.get::<_, Value>(5)? {
                    Value::Integer(x) => x,
                    Value::Real(x) => x as i64,
                    other => text_of(&other).parse().unwrap_or(0),
                };
                Ok((
                    t(0)?,
                    crate::categories::of_entry(&t(1)?, &t(2)?, &t(3)?, &t(4)?),
                    n,
                    time_of(&r.get::<_, Value>(6)?, zone),
                ))
            },
        )?;
        for (k, cat, n, at) in rows {
            let e = out.entry(k).or_default();
            if cat != "allergy" {
                e.0.insert(cat);
            }
            e.1 += n;
            e.2 = match (e.2, at) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
        }
        for r in self.recordings(keys, zone)? {
            let cat = if r.category.trim().is_empty() {
                "icu"
            } else {
                crate::categories::named(&r.category).unwrap_or("icu")
            };
            let e = out.entry(r.person).or_default();
            e.0.insert(cat.to_string());
            e.2 = match (e.2, r.start) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
        }
        Ok(out)
    }

    /// The categories of the recordings the database lists (one grouped query).
    pub fn recording_categories(&self) -> anyhow::Result<Vec<String>> {
        let v = self.views().recordings;
        if v.trim().is_empty() || !self.has_recordings() {
            return Ok(vec![]);
        }
        let db = self.db()?;
        let mut st = db.prepare_native(&format!("SELECT DISTINCT category FROM ({v}) r"))?;
        let mut out: Vec<String> = st
            .query_map([], |r| Ok(text_of(&r.get::<_, Value>(0)?)))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|c| {
                if c.trim().is_empty() { "icu" } else { crate::categories::named(&c).unwrap_or("icu") }.to_string()
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// One recording, by its key.
    pub fn recording(&self, key: &str, zone: chrono_tz::Tz) -> anyhow::Result<Option<Recording>> {
        let v = self.views().recordings;
        if v.trim().is_empty() || !self.has_recordings() {
            return Ok(None);
        }
        let db = self.db()?;
        Ok(db
            .query_in_native(
                &format!("SELECT {RECORDING_COLS} FROM ({v}) r WHERE recording_key IN ({{}})"),
                &[key.to_string()],
                |r| recording_of(r, zone),
            )?
            .into_iter()
            .next())
    }
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
