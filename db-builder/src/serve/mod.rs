//! `db-builder serve --root DIR`: the web interface (README.md, "Web interface").  One page
//! (web/: HTML, JavaScript and CSS, built in) and a JSON API under `/api/`, on 127.0.0.1 by
//! default, for one account of its own ([`auth`]).
//!
//! Security:
//! - every API request but `login` needs a signed-in session (a cookie, HttpOnly and
//!   SameSite=Strict); the page itself holds no data;
//! - every request that changes something carries the header `X-DB-Builder: 1` (a page of
//!   another site cannot add it without this server's consent, which it never gives);
//! - every path the page sends is confined to the root ([`crate::root::Root`]): read inside it,
//!   written inside it, never above; files a table's rows name outside it are left out;
//! - API answers are not cached; the page may not be framed.
//!
//! Adding (tables, exports, CLIF folders, folders of recordings, and their previews) runs as a
//! task on a thread of its own, one at a time; the page polls `GET api/tasks/{id}` for its
//! progress and report.

pub mod auth;

use crate::add::{Options, Progress, SourceReport};
use crate::db;
use crate::root::Root;
use axum::extract::{FromRequestParts, Path as UrlPath, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static WEB: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web");

/// The default port (Medorama's is 8090).
pub const DEFAULT_PORT: u16 = 8091;
/// The tables the page shows.
const TABLES: &[&str] = &["persons", "visits", "entries", "recordings", "extras", "sources"];
/// The most rows a page of a table holds.
const MAX_ROWS: usize = 500;
/// The most entries a folder's listing shows, and databases the list shows.
const MAX_LISTED: usize = 5000;
/// The tasks kept (the oldest finished ones are forgotten first).
const MAX_TASKS: usize = 50;

/// How `db-builder serve` runs.
pub struct Settings {
    /// The one folder the page may browse, read and write in.
    pub root: PathBuf,
    /// Its home: the password file and the log of actions.
    pub home: PathBuf,
    /// The key of encrypted databases (`--encrypt`, `$DB_BUILDER_KEY`): every database the page
    /// opens or makes is encrypted with it.
    pub key: Option<String>,
    /// Its port (names its cookie).
    pub port: u16,
    /// Mark the cookie Secure (served over HTTPS); it is also when a proxy says the request
    /// came over HTTPS (`X-Forwarded-Proto: https`).
    pub secure_cookies: bool,
    /// A session unused this long ends.
    pub idle: Duration,
}

/// The default home folder: `~/.local/share/db-builder` (Windows: `%LOCALAPPDATA%\db-builder`).
pub fn default_home() -> PathBuf {
    match std::env::var("LOCALAPPDATA").ok().filter(|_| cfg!(windows)) {
        Some(l) => PathBuf::from(l).join("db-builder"),
        None => PathBuf::from(recording_engine::paths::expand("~/.local/share/db-builder")),
    }
}

/// A task: an add (or its preview) running on its own thread.
struct Task {
    what: String,
    db: String,
    dry_run: bool,
    /// `waiting` (for another task to end), `running`, `done`, `failed`.
    state: &'static str,
    started: Instant,
    ended: Option<Instant>,
    progress: Progress,
    reports: Vec<SourceReport>,
    error: String,
}

pub struct App {
    root: Root,
    home: PathBuf,
    how: db::Opening,
    user: String,
    hash: String,
    cookie: String,
    secure_cookies: bool,
    sessions: auth::Sessions,
    fails: Mutex<auth::Fails>,
    /// Password checks (Argon2: memory and time on purpose) running at once.
    checks: tokio::sync::Semaphore,
    tasks: Mutex<Vec<(u64, Arc<Mutex<Task>>)>>,
    next_task: AtomicU64,
    /// Held while a database is written: one write at a time.
    writing: Arc<Mutex<()>>,
}

pub type Shared = Arc<App>;

impl App {
    /// The server's state; refused without a password set (`db-builder serve --set-password`).
    pub fn new(s: &Settings) -> anyhow::Result<App> {
        let root = Root::new(&s.root).map_err(|e| anyhow::anyhow!("--root: {e}"))?;
        let Some((user, hash)) = auth::read_account(&s.home)? else {
            anyhow::bail!(
                "no password is set for the web interface: set one first with\n  db-builder serve --set-password --home {}",
                s.home.display()
            );
        };
        Ok(App {
            root,
            home: s.home.clone(),
            how: db::Opening { key: s.key.clone() },
            user,
            hash,
            cookie: format!("db_builder_{}", s.port),
            secure_cookies: s.secure_cookies,
            sessions: auth::Sessions::new(s.idle),
            fails: Mutex::new(auth::Fails::default()),
            checks: tokio::sync::Semaphore::new(2),
            tasks: Mutex::new(vec![]),
            next_task: AtomicU64::new(1),
            writing: Arc::new(Mutex::new(())),
        })
    }

    /// The root folder.
    pub fn root(&self) -> &Root {
        &self.root
    }

    /// One line of the log of actions (`<home>/db-builder.log`): what was done, to which
    /// files (paths in the root, never rows).
    fn log(&self, what: &str) {
        use std::io::Write;
        let f = self.home.join("db-builder.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(f) {
            let _ = writeln!(f, "{} {}", db::now(), what.replace(['\r', '\n'], " "));
        }
    }

    /// A database in the root, by its path there.
    fn db_path(&self, given: &str) -> Result<PathBuf, ApiError> {
        let p = self.root.existing(given).map_err(bad)?;
        if !p.is_file() {
            return Err(bad(format!("{}: not a file", given.trim())));
        }
        Ok(p)
    }

    fn open(&self, given: &str) -> Result<(PathBuf, rusqlite::Connection), ApiError> {
        let p = self.db_path(given)?;
        let c = db::open(&p, &self.how).map_err(|e| bad(self.shown_error(&e)))?;
        Ok((p, c))
    }

    /// An error as the page is told it: paths in the root relative to it; an encrypted
    /// database's error says how this server must be started to open it.
    fn shown_error(&self, e: &anyhow::Error) -> String {
        let mut s = format!("{e:#}");
        if s.contains("it is encrypted") && self.how.key.is_none() {
            let file = s.split(": cannot be read").next().unwrap_or("").to_string();
            s = format!(
                "{file}: encrypted (or not a database): this db-builder serve opens encrypted databases when started \
                 with --encrypt, their key in ${}",
                db::KEY_VAR
            );
        }
        let root = format!("{}/", self.root.path().to_string_lossy());
        s.replace(&root, "")
    }

    /// The write lock now, or a refusal when a task holds it.
    fn write_now(&self) -> Result<std::sync::MutexGuard<'_, ()>, ApiError> {
        self.writing.try_lock().map_err(|_| {
            ApiError(StatusCode::CONFLICT, "another task is writing a database: try again when it has ended".into())
        })
    }
}

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.0.is_server_error() {
            eprintln!("db-builder: {} {}", self.0.as_u16(), self.1);
        }
        (self.0, Json(json!({ "detail": self.1 }))).into_response()
    }
}

fn bad(e: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, e.to_string())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

type ApiResult = Result<Response, ApiError>;

fn ok(v: impl serde::Serialize) -> ApiResult {
    Ok(Json(v).into_response())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)?
}

/// A cookie's value.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// A signed-in session (401 without one).
pub struct Signed;

impl FromRequestParts<Shared> for Signed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        // the page's polling of a task does not keep an idle session alive
        let active = parts.headers.get("x-background").is_none_or(|v| v != "1");
        match cookie_value(&parts.headers, &app.cookie) {
            Some(t) if app.sessions.check(&t, active) => Ok(Signed),
            _ => Err(ApiError(StatusCode::UNAUTHORIZED, "sign in required".into())),
        }
    }
}

// ------------------------------------------------------------------ sign-in

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
}

/// `POST api/login`: the account's name and password; a session cookie on success.  Attempts
/// are counted before the password is checked, per address and in all ([`auth::Fails`]).
async fn login(
    State(app): State<Shared>,
    ext: axum::http::Extensions,
    headers: HeaderMap,
    Json(req): Json<Login>,
) -> ApiResult {
    let ip = ext.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip());
    if !app.fails.lock().unwrap().reserve(ip) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            format!("too many failed attempts; try again in {} s", auth::LOCK.as_secs()),
        ));
    }
    let right = {
        let _check = app.checks.acquire().await.map_err(internal)?;
        let a = app.clone();
        // the password checked whatever the name (no shortcut that would tell a name is right)
        blocking(move || {
            let password_right = auth::verify(&req.password, &a.hash);
            Ok(password_right && req.username == a.user)
        })
        .await?
    };
    if !right {
        app.log(&format!("sign-in refused (from {})", ip.map_or("?".into(), |i| i.to_string())));
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Err(ApiError(StatusCode::UNAUTHORIZED, "wrong user name or password".into()));
    }
    app.fails.lock().unwrap().succeeded(ip);
    app.log("signed in");
    let https = headers.get("x-forwarded-proto").is_some_and(|v| v == "https");
    let cookie = format!(
        "{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        app.cookie,
        app.sessions.start(),
        auth::SESSION_MAX.as_secs(),
        if app.secure_cookies || https { "; Secure" } else { "" }
    );
    let mut r = Json(json!({ "user": app.user })).into_response();
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(internal)?);
    Ok(r)
}

async fn logout(State(app): State<Shared>, headers: HeaderMap) -> ApiResult {
    if let Some(t) = cookie_value(&headers, &app.cookie) {
        app.sessions.end(&t);
    }
    let mut r = Json(json!({})).into_response();
    let cookie = format!("{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0", app.cookie);
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(internal)?);
    Ok(r)
}

/// `GET api/me`: who is signed in, the root, whether databases are encrypted.
async fn me(State(app): State<Shared>, _s: Signed) -> ApiResult {
    ok(json!({
        "user": app.user,
        "root": app.root.path().to_string_lossy(),
        "encrypted": app.how.key.is_some(),
        "version": env!("CARGO_PKG_VERSION"),
        "timezone": db::DEFAULT_TIMEZONE,
    }))
}

// ------------------------------------------------------------------ files and databases

#[derive(Deserialize, Default)]
#[serde(default)]
struct DirQuery {
    dir: String,
}

/// `GET api/files?dir=`: a folder of the root (empty: the root), its folders and files.
async fn files(State(app): State<Shared>, _s: Signed, Query(q): Query<DirQuery>) -> ApiResult {
    blocking(move || {
        let dir = app.root.existing(&q.dir).map_err(bad)?;
        if !dir.is_dir() {
            return Err(bad(format!("{}: not a folder", q.dir.trim())));
        }
        let (items, truncated) = app.root.list(&dir, MAX_LISTED).map_err(bad)?;
        let shown = app.root.shown(&dir);
        let parent = (dir != app.root.path()).then(|| dir.parent().map(|p| app.root.shown(p)).unwrap_or_default());
        ok(json!({ "dir": shown, "parent": parent, "items": items, "truncated": truncated }))
    })
    .await
}

/// `GET api/databases`: the SQLite files in the root and its folders.
async fn databases(State(app): State<Shared>, _s: Signed) -> ApiResult {
    blocking(move || {
        let items: Vec<Value> = app
            .root
            .databases(8, MAX_LISTED)
            .iter()
            .map(|p| {
                let m = std::fs::metadata(p).ok();
                json!({ "path": app.root.shown(p), "size": m.as_ref().map_or(0, |m| m.len()) })
            })
            .collect();
        ok(json!({ "items": items }))
    })
    .await
}

#[derive(Deserialize)]
struct NewDb {
    path: String,
    #[serde(default)]
    timezone: String,
}

/// A database's file name: `.sqlite` added when it has neither `.sqlite` nor `.sqlite3`.
fn sqlite_name(given: &str) -> String {
    let g = given.trim();
    let lower = g.to_lowercase();
    if lower.ends_with(".sqlite") || lower.ends_with(".sqlite3") {
        g.to_string()
    } else {
        format!("{g}.sqlite")
    }
}

/// `POST api/databases`: a new, empty database in the root.
async fn database_new(State(app): State<Shared>, _s: Signed, Json(b): Json<NewDb>) -> ApiResult {
    blocking(move || {
        let p = app.root.new_file(&sqlite_name(&b.path)).map_err(bad)?;
        let tz = if b.timezone.trim().is_empty() { db::DEFAULT_TIMEZONE } else { b.timezone.trim() };
        db::create(&p, &app.how, tz).map_err(|e| bad(app.shown_error(&e)))?;
        app.log(&format!("new {} ({tz})", app.root.shown(&p)));
        ok(json!({ "path": app.root.shown(&p) }))
    })
    .await
}

#[derive(Deserialize)]
struct DbQuery {
    db: String,
}

/// `GET api/info?db=`: a database's meta, its tables' rows, its sources.
async fn info(State(app): State<Shared>, _s: Signed, Query(q): Query<DbQuery>) -> ApiResult {
    blocking(move || {
        let (p, c) = app.open(&q.db)?;
        let mut meta = serde_json::Map::new();
        for k in ["schema", "version", "timezone", "created_by"] {
            meta.insert(k.into(), json!(db::meta(&c, k).map_err(internal)?.unwrap_or_default()));
        }
        let mut counts = serde_json::Map::new();
        for t in TABLES {
            let n: i64 = c.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).map_err(internal)?;
            counts.insert(t.to_string(), json!(n));
        }
        let sources: Vec<Value> = c
            .prepare("SELECT id, kind, rows, added_at, file FROM sources ORDER BY id")
            .map_err(internal)?
            .query_map([], |r| {
                Ok(json!({ "id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?, "rows": r.get::<_, i64>(2)?,
                           "added_at": r.get::<_, String>(3)?, "file": r.get::<_, String>(4)? }))
            })
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        let sources: Vec<Value> = sources
            .into_iter()
            .map(|mut s| {
                let f = s["file"].as_str().unwrap_or("").to_string();
                s["file"] = json!(app.shown_path(&f, &p));
                s
            })
            .collect();
        let size = std::fs::metadata(&p).map_or(0, |m| m.len());
        ok(json!({
            "path": app.root.shown(&p), "size": size, "encrypted": app.how.key.is_some(),
            "meta": meta, "counts": counts, "sources": sources,
            "rules_file": app.root.shown(&db::rules_file(&p)),
        }))
    })
    .await
}

impl App {
    /// A path a database stores, as the page shows it: relative to the root when inside it.
    fn shown_path(&self, stored: &str, db_file: &Path) -> String {
        let abs = db::resolve_path(stored, &db::folder_of(db_file));
        if abs.starts_with(self.root.path()) {
            self.root.shown(&abs)
        } else {
            stored.to_string()
        }
    }
}

#[derive(Deserialize)]
struct RowsQuery {
    db: String,
    table: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    q: String,
}

fn cell(v: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(x) => json!(x),
        ValueRef::Real(x) => json!(x),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(format!("({} bytes)", b.len())),
    }
}

/// `GET api/rows?db=&table=&offset=&limit=&q=`: a page of a table's rows (read-only), those
/// with `q` in any column when it is given.
async fn rows(State(app): State<Shared>, _s: Signed, Query(q): Query<RowsQuery>) -> ApiResult {
    let Some(table) = TABLES.iter().find(|t| **t == q.table).copied() else {
        return Err(bad(format!("{:?}: not a table ({})", q.table, TABLES.join(", "))));
    };
    blocking(move || {
        let (_, c) = app.open(&q.db)?;
        let columns: Vec<String> = c
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(internal)?
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        let filter = q.q.trim();
        let (cond, like) = if filter.is_empty() {
            (String::new(), String::new())
        } else {
            let any: Vec<String> =
                columns.iter().map(|k| format!("CAST(\"{k}\" AS TEXT) LIKE ?1 ESCAPE '\\'")).collect();
            let esc = filter.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            (format!(" WHERE {}", any.join(" OR ")), format!("%{esc}%"))
        };
        let args: Vec<&dyn rusqlite::ToSql> = if like.is_empty() { vec![] } else { vec![&like] };
        let total: i64 = c
            .query_row(&format!("SELECT COUNT(*) FROM {table}{cond}"), args.as_slice(), |r| r.get(0))
            .map_err(internal)?;
        let limit = q.limit.unwrap_or(100).clamp(1, MAX_ROWS);
        let mut st = c
            .prepare(&format!("SELECT * FROM {table}{cond} ORDER BY rowid LIMIT {limit} OFFSET {}", q.offset))
            .map_err(internal)?;
        let n = st.column_count();
        let out: Vec<Vec<Value>> = st
            .query_map(args.as_slice(), |r| Ok((0..n).map(|i| r.get_ref(i).map(cell).unwrap_or(Value::Null)).collect()))
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        ok(json!({ "table": table, "columns": columns, "rows": out, "total": total, "offset": q.offset }))
    })
    .await
}

/// `GET api/formats`: the known exports (built in).
async fn formats(_s: Signed) -> ApiResult {
    let items: Vec<Value> = crate::formats::builtin()
        .iter()
        .map(|f| json!({ "key": f.key(), "label": f.label(), "category": f.category() }))
        .collect();
    let categories: Vec<Value> =
        crate::categories::CATEGORIES.iter().map(|(k, label, _)| json!({ "key": k, "label": label })).collect();
    ok(json!({ "items": items, "categories": categories }))
}

// ------------------------------------------------------------------ adding: tasks

/// What the page's forms give `add` and `recordings` (the command's options).
#[derive(Deserialize, Default, Clone)]
#[serde(default)]
struct AddOptions {
    issuer: String,
    mrn_authority: String,
    category: String,
    /// `auto` (or empty), `none`, or a format's key.
    format: String,
    day_first: bool,
    /// A mapping file in the root (TOML or JSON), if any.
    mapping: String,
    /// Declared formats: TOML files or folders of them, in the root.
    formats: Vec<String>,
}

impl App {
    /// The library's options from the page's: files confined to the root, the database's
    /// rules file applied.
    fn options(&self, o: &AddOptions, db_file: &Path, dry_run: bool) -> Result<Options, ApiError> {
        let mapping = if o.mapping.trim().is_empty() {
            Default::default()
        } else {
            let f = self.root.existing(&o.mapping).map_err(bad)?;
            let text = std::fs::read_to_string(&f).map_err(|e| bad(format!("{}: {e}", o.mapping.trim())))?;
            crate::tables::read_mapping(&text).map_err(|e| bad(format!("{}: {e}", o.mapping.trim())))?
        };
        let category = o.category.trim();
        if !category.is_empty() && crate::categories::named(category).is_none() {
            return Err(bad(format!("{category:?}: not a category")));
        }
        let formats = o.formats.iter().map(|f| self.root.existing(f).map_err(bad)).collect::<Result<Vec<_>, _>>()?;
        let rules_file = db::rules_file(db_file);
        let rules = if rules_file.is_file() {
            let f = self.root.existing(&rules_file.to_string_lossy()).map_err(bad)?;
            crate::rules::Rules::read(&f).map_err(|e| bad(format!("{}: {e:#}", self.root.shown(&rules_file))))?
        } else {
            Default::default()
        };
        Ok(Options {
            issuer: o.issuer.trim().into(),
            mrn_authority: o.mrn_authority.trim().into(),
            category: crate::categories::named(category).unwrap_or("").into(),
            mapping,
            format: if o.format.trim().is_empty() { "auto".into() } else { o.format.trim().into() },
            formats,
            day_first: o.day_first,
            rules,
            dry_run,
            progress: Progress::counting(),
        })
    }

    /// Run `work` as a task on its own thread (after any other task that writes): its id.
    fn spawn(
        self: &Arc<Self>,
        what: String,
        db_file: &Path,
        opts: Options,
        work: impl FnOnce(&Options) -> anyhow::Result<Vec<SourceReport>> + Send + 'static,
    ) -> u64 {
        let id = self.next_task.fetch_add(1, Ordering::Relaxed);
        let task = Arc::new(Mutex::new(Task {
            what: what.clone(),
            db: self.root.shown(db_file),
            dry_run: opts.dry_run,
            state: "waiting",
            started: Instant::now(),
            ended: None,
            progress: opts.progress.clone(),
            reports: vec![],
            error: String::new(),
        }));
        {
            let mut tasks = self.tasks.lock().unwrap();
            if tasks.len() >= MAX_TASKS {
                if let Some(i) = tasks.iter().position(|(_, t)| t.lock().unwrap().ended.is_some()) {
                    tasks.remove(i);
                }
            }
            tasks.push((id, task.clone()));
        }
        let app = self.clone();
        std::thread::spawn(move || {
            let lock = app.writing.clone();
            let _one = lock.lock().unwrap_or_else(|e| e.into_inner());
            task.lock().unwrap().state = "running";
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&opts)));
            let mut t = task.lock().unwrap();
            t.ended = Some(Instant::now());
            let label = format!("{}{} in {}", if opts.dry_run { "preview of " } else { "" }, t.what, t.db);
            match r {
                Ok(Ok(reports)) => {
                    t.state = "done";
                    if !opts.dry_run {
                        let added: Vec<String> =
                            reports.iter().filter_map(|r| r.source).map(|s| s.to_string()).collect();
                        app.log(&format!(
                            "{label}: sources {}",
                            if added.is_empty() { "none".into() } else { added.join(", ") }
                        ));
                    }
                    t.reports = reports;
                }
                Ok(Err(e)) => {
                    t.state = "failed";
                    t.error = app.shown_error(&e);
                    if !opts.dry_run {
                        app.log(&format!("{label}: failed"));
                    }
                }
                Err(_) => {
                    t.state = "failed";
                    t.error = "the task stopped on an internal error".into();
                }
            }
        });
        id
    }

    fn task_json(&self, id: u64, t: &Task) -> Value {
        let (file, rows) = t.progress.get();
        let reports: Vec<Value> = t
            .reports
            .iter()
            .map(|r| {
                let mut v = serde_json::to_value(r).unwrap_or(Value::Null);
                v["file"] = json!(self.root.shown(Path::new(&r.file)));
                v
            })
            .collect();
        json!({
            "id": id, "what": t.what, "db": t.db, "dry_run": t.dry_run, "state": t.state,
            "file": if file.is_empty() { String::new() } else { self.root.shown(Path::new(&file)) },
            "rows": rows,
            "elapsed_s": t.ended.unwrap_or_else(Instant::now).duration_since(t.started).as_secs_f64(),
            "reports": reports, "error": t.error,
        })
    }
}

#[derive(Deserialize)]
struct Import {
    db: String,
    inputs: Vec<String>,
    #[serde(default)]
    options: AddOptions,
    #[serde(default)]
    dry_run: bool,
}

/// `POST api/import`: tables, known exports or CLIF folders added (`dry_run`: a preview that
/// writes nothing): a task.
async fn import(State(app): State<Shared>, _s: Signed, Json(b): Json<Import>) -> ApiResult {
    blocking(move || {
        let (db_file, _) = app.open(&b.db)?;
        if b.inputs.is_empty() {
            return Err(bad("choose the tables, exports or CLIF folders to add"));
        }
        let inputs = b.inputs.iter().map(|i| app.root.existing(i).map_err(bad)).collect::<Result<Vec<_>, _>>()?;
        let opts = app.options(&b.options, &db_file, b.dry_run)?;
        let what = format!("add {}", inputs.iter().map(|i| app.root.shown(i)).collect::<Vec<_>>().join(", "));
        let (a, f, dry) = (app.clone(), db_file.clone(), b.dry_run);
        let id = app.spawn(what, &db_file, opts, move |opts| {
            let mut c = db::open(&f, &a.how)?;
            // (as the command line adds them: the indexes a reader's searches need)
            if !dry {
                db::ensure_indexes(&c)?;
            }
            crate::add::add(&mut c, &f, &inputs, opts)
        });
        ok(json!({ "task": id }))
    })
    .await
}

#[derive(Deserialize)]
struct AddRecordings {
    db: String,
    folder: String,
    #[serde(default)]
    patients: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    options: AddOptions,
    #[serde(default)]
    dry_run: bool,
}

/// `POST api/recordings`: a folder of recordings added, with a patients table or a pattern
/// (`dry_run`: a preview): a task.
async fn recordings(State(app): State<Shared>, _s: Signed, Json(b): Json<AddRecordings>) -> ApiResult {
    blocking(move || {
        let (db_file, _) = app.open(&b.db)?;
        let folder = app.root.existing(&b.folder).map_err(bad)?;
        let table =
            if b.patients.trim().is_empty() { None } else { Some(app.root.existing(&b.patients).map_err(bad)?) };
        let pattern = Some(b.pattern.trim().to_string()).filter(|p| !p.is_empty());
        if table.is_none() && pattern.is_none() {
            return Err(bad("say where the recordings' patients are: a patients table, or a pattern of their names"));
        }
        if let Some(p) = &pattern {
            crate::recordings::Pattern::parse(p).map_err(bad)?;
        }
        let opts = app.options(&b.options, &db_file, b.dry_run)?;
        let who = crate::recordings::Patients { table, pattern };
        let what = format!("recordings of {}", app.root.shown(&folder));
        let (a, f, dry) = (app.clone(), db_file.clone(), b.dry_run);
        let id = app.spawn(what, &db_file, opts, move |opts| {
            let mut c = db::open(&f, &a.how)?;
            // (as the command line adds them: the indexes a reader's searches need)
            if !dry {
                db::ensure_indexes(&c)?;
            }
            Ok(vec![crate::recordings::add_folder(&mut c, &f, &folder, &who, opts)?])
        });
        ok(json!({ "task": id }))
    })
    .await
}

/// `GET api/tasks/{id}`: a task's state, progress and reports.
async fn task(State(app): State<Shared>, _s: Signed, UrlPath(id): UrlPath<u64>) -> ApiResult {
    let t = app.tasks.lock().unwrap().iter().find(|(i, _)| *i == id).map(|(_, t)| t.clone());
    let Some(t) = t else { return Err(ApiError(StatusCode::NOT_FOUND, "no such task".into())) };
    let v = app.task_json(id, &t.lock().unwrap());
    ok(v)
}

// ------------------------------------------------------------------ rules, rewrite, check, sources, copy

/// `GET api/rules?db=`: the database's rules file (empty when there is none).
async fn rules_get(State(app): State<Shared>, _s: Signed, Query(q): Query<DbQuery>) -> ApiResult {
    blocking(move || {
        let p = app.db_path(&q.db)?;
        let f = db::rules_file(&p);
        let text = if f.is_file() {
            let real = app.root.existing(&f.to_string_lossy()).map_err(bad)?;
            std::fs::read_to_string(real).map_err(internal)?
        } else {
            String::new()
        };
        ok(json!({ "file": app.root.shown(&f), "exists": f.is_file(), "text": text }))
    })
    .await
}

#[derive(Deserialize)]
struct RulesPut {
    db: String,
    text: String,
}

/// `PUT api/rules`: the rules file, checked first (refused, unchanged, when it is not valid).
async fn rules_put(State(app): State<Shared>, _s: Signed, Json(b): Json<RulesPut>) -> ApiResult {
    blocking(move || {
        let p = app.db_path(&b.db)?;
        let rules = crate::rules::Rules::parse(&b.text).map_err(|e| bad(format!("not saved: {e:#}")))?;
        let f = app.root.new_file(&db::rules_file(&p).to_string_lossy()).map_err(bad)?;
        std::fs::write(&f, &b.text).map_err(internal)?;
        app.log(&format!("rules saved: {}", app.root.shown(&f)));
        ok(json!({ "file": app.root.shown(&f), "paths": rules.has_paths(), "empty": rules.is_empty() }))
    })
    .await
}

#[derive(Deserialize)]
struct Rewrite {
    db: String,
    #[serde(default)]
    dry_run: bool,
}

/// `POST api/rewrite`: the paths a database lists rewritten by its rules file's `[[path]]`
/// rules (`dry_run`: only counted).
async fn rewrite(State(app): State<Shared>, _s: Signed, Json(b): Json<Rewrite>) -> ApiResult {
    blocking(move || {
        let (p, mut c) = app.open(&b.db)?;
        let f = db::rules_file(&p);
        if !f.is_file() {
            return Err(bad("the database has no rules file: write its [[path]] rules in Rules first"));
        }
        let rules = crate::rules::Rules::read(&app.root.existing(&f.to_string_lossy()).map_err(bad)?)
            .map_err(|e| bad(format!("{e:#}")))?;
        if !rules.has_paths() {
            return Err(bad("the rules file has no [[path]] rule"));
        }
        let _one = app.write_now()?;
        let tx = c.transaction().map_err(internal)?;
        let n = db::rewrite_paths(&tx, &rules, !b.dry_run).map_err(internal)?;
        tx.commit().map_err(internal)?;
        if !b.dry_run {
            app.log(&format!(
                "rewrite {}: {} recordings, {} files, {} sources",
                app.root.shown(&p),
                n.recordings,
                n.files,
                n.sources
            ));
        }
        ok(json!({ "dry_run": b.dry_run, "recordings": n.recordings, "files": n.files, "sources": n.sources }))
    })
    .await
}

/// `POST api/check`: what a reader cannot rely on (by key, never by name or MRN).
async fn check(State(app): State<Shared>, _s: Signed, Json(b): Json<DbQuery>) -> ApiResult {
    blocking(move || {
        let (p, c) = app.open(&b.db)?;
        let problems: Vec<Value> = crate::check::check(&c, &p)
            .map_err(|e| bad(app.shown_error(&e)))?
            .into_iter()
            .map(|x| json!({ "what": x.what, "count": x.count, "examples": x.examples }))
            .collect();
        ok(json!({ "problems": problems }))
    })
    .await
}

#[derive(Deserialize)]
struct RemoveSources {
    db: String,
    ids: Vec<i64>,
}

/// `POST api/remove-source`: sources taken out, in one transaction.
async fn remove_source(State(app): State<Shared>, _s: Signed, Json(b): Json<RemoveSources>) -> ApiResult {
    blocking(move || {
        let (p, mut c) = app.open(&b.db)?;
        if b.ids.is_empty() {
            return Err(bad("choose the sources to remove"));
        }
        let _one = app.write_now()?;
        let tx = c.transaction().map_err(internal)?;
        let mut out = vec![];
        for id in &b.ids {
            let r = db::remove_source(&tx, *id).map_err(|e| bad(format!("{e:#}")))?;
            out.push(json!({ "id": id, "entries": r.entries, "recordings": r.recordings, "visits": r.visits, "persons": r.persons }));
        }
        db::bump(&tx).map_err(internal)?;
        tx.commit().map_err(internal)?;
        let ids: Vec<String> = b.ids.iter().map(|i| i.to_string()).collect();
        app.log(&format!("remove sources {} from {}", ids.join(", "), app.root.shown(&p)));
        ok(json!({ "removed": out }))
    })
    .await
}

#[derive(Deserialize)]
struct Copy {
    db: String,
    to: String,
}

/// `POST api/copy`: a copy of the database in a new file of the root (encrypted as it is).
async fn copy(State(app): State<Shared>, _s: Signed, Json(b): Json<Copy>) -> ApiResult {
    blocking(move || {
        let (p, c) = app.open(&b.db)?;
        let to = app.root.new_file(&sqlite_name(&b.to)).map_err(bad)?;
        db::save_copy(&c, &to, &app.how).map_err(|e| bad(app.shown_error(&e)))?;
        app.log(&format!("copy of {} saved as {}", app.root.shown(&p), app.root.shown(&to)));
        ok(json!({ "path": app.root.shown(&to) }))
    })
    .await
}

// ------------------------------------------------------------------ the page

async fn page() -> Response {
    static_response("index.html")
}

async fn static_file(UrlPath(path): UrlPath<String>) -> Response {
    static_response(&path)
}

fn static_response(path: &str) -> Response {
    match WEB.get_file(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], f.contents()).into_response()
        }
        None => ApiError(StatusCode::NOT_FOUND, "not found".into()).into_response(),
    }
}

/// Security headers on every answer; `X-DB-Builder: 1` required on API requests that change
/// something; API answers not cached.
async fn guard(req: Request, next: Next) -> Response {
    let api = req.uri().path().starts_with("/api/");
    let changes = req.method() != Method::GET && req.method() != Method::HEAD;
    if api && changes && req.headers().get("x-db-builder").and_then(|v| v.to_str().ok()) != Some("1") {
        return ApiError(StatusCode::FORBIDDEN, "missing X-DB-Builder header".into()).into_response();
    }
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    r
}

pub fn router(app: Shared) -> Router {
    let api = Router::new()
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/me", get(me))
        .route("/files", get(files))
        .route("/databases", get(databases).post(database_new))
        .route("/info", get(info))
        .route("/rows", get(rows))
        .route("/formats", get(formats))
        .route("/import", post(import))
        .route("/recordings", post(recordings))
        .route("/tasks/{id}", get(task))
        .route("/rules", get(rules_get).put(rules_put))
        .route("/rewrite", post(rewrite))
        .route("/check", post(check))
        .route("/remove-source", post(remove_source))
        .route("/copy", post(copy))
        .fallback(|| async { ApiError(StatusCode::NOT_FOUND, "not found".into()) });
    Router::new()
        .route("/", get(page))
        .route("/static/{*path}", get(static_file))
        .nest("/api", api)
        .layer(middleware::from_fn(guard))
        .with_state(app)
}

/// `db-builder serve`: the server on `host:port`, until Ctrl-C (or a service manager's SIGTERM).
pub fn run(s: Settings, host: &str) -> anyhow::Result<()> {
    let app = Arc::new(App::new(&s)?);
    // the files the recording engine reads (a header, the signal files it names): in the root
    // only, links resolved
    recording_engine::paths::set_allowed_roots(Some(vec![app.root.path().to_string_lossy().to_string()]));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind((host, s.port))
            .await
            .map_err(|e| anyhow::anyhow!("cannot listen on {host}:{}: {e}", s.port))?;
        println!("db-builder: open http://{host}:{}/", s.port);
        println!("  root: {}", app.root.path().display());
        println!("  home: {} (sign in as {})", s.home.display(), app.user);
        if app.how.key.is_some() {
            println!("  databases encrypted with the key in ${}", db::KEY_VAR);
        }
        let loopback = matches!(host, "127.0.0.1" | "localhost" | "::1");
        if !loopback && !s.secure_cookies {
            println!(
                "  WARNING: reachable from other machines without HTTPS: serve it behind a reverse proxy with TLS, with --secure-cookies"
            );
        }
        axum::serve(listener, router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>())
            .with_graceful_shutdown(async {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    match signal(SignalKind::terminate()) {
                        Ok(mut term) => {
                            tokio::select! {
                                _ = tokio::signal::ctrl_c() => {}
                                _ = term.recv() => {}
                            }
                        }
                        Err(_) => {
                            let _ = tokio::signal::ctrl_c().await;
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = tokio::signal::ctrl_c().await;
                }
            })
            .await?;
        anyhow::Ok(())
    })
}
