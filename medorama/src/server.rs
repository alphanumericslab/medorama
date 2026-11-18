//! HTTP API (`api/v1/...`, docs/api.md) and the web page, plus the
//! background workers (jobs, automatic indexing, periodic re-checks).
//!
//! Security: every API request except `login` and `sign-in` needs a signed-in user
//! (signin.rs: a session cookie, HttpOnly and SameSite=Strict, or an authenticating proxy's
//! header); every request that changes something must carry the header
//! `X-Medorama: 1` (a page on another site cannot add it); roles and the PHI permission are
//! checked on the server, identifiers are never sent to users without the PHI permission,
//! and API responses are not cached.

use crate::actions::Registry;
use crate::config::Config;
use crate::db::{audit, Db};
use crate::jobs::{self, Selection};
use crate::recordings;
use crate::users::{self, Changes, Role, User};
use axum::extract::{FromRequestParts, Path as UrlPath, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use include_dir::{include_dir, Dir};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

static WEB: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/web");
/// A page file of a viewer, built in.
macro_rules! page {
    ($viewer:literal, $file:literal) => {
        ($file, include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../viewers/", $viewer, "/", $file)).as_slice())
    };
}

/// The viewers Medorama opens as tabs (each also usable on its own: viewers/README.md): their
/// pages' files only (the folders also hold examples, tests and Python packages, never served
/// here; a test checks that every page file is listed). VitalStream runs as a program of its
/// own, behind the gateway (/viewer/).
static VIEWERS: [(&str, &[(&str, &[u8])]); 2] = [
    (
        "ecg",
        &[
            page!("ecg", "index.html"),
            page!("ecg", "style.css"),
            page!("ecg", "app.js"),
            page!("ecg", "ecg.js"),
            page!("ecg", "readers.js"),
            page!("ecg", "wfdb.js"),
            page!("ecg", "muse.js"),
            page!("ecg", "dicom.js"),
            page!("ecg", "dicom-ecg.js"),
            page!("ecg", "edf.js"),
            page!("ecg", "paper.js"),
            page!("ecg", "host.js"),
        ],
    ),
    (
        "pacs",
        &[
            page!("pacs", "index.html"),
            page!("pacs", "style.css"),
            page!("pacs", "app.js"),
            page!("pacs", "dicom.js"),
            page!("pacs", "jpeg.js"),
            page!("pacs", "host.js"),
        ],
    ),
];

pub struct App {
    pub cfg: Arc<Config>,
    pub db: Arc<Db>,
    pub reg: Arc<RwLock<Registry>>,
    pub viewer: Arc<crate::viewer::Supervisor>,
    /// db-builder's web interface, run or linked for "Create a database…" (dbbuilder.rs).
    pub dbbuilder: Arc<crate::dbbuilder::DbBuilder>,
    pub gateway: crate::gateway::Gateway,
    /// The name of the sign-in cookie (one per port, so that servers on one host do not mix).
    pub(crate) cookie: String,
    pub(crate) signin: crate::signin::SignIn,
    /// The privacy gate's identifier dictionary, kept while the data do not change.
    dict: Mutex<Option<Arc<crate::gate::Dict>>>,
    /// One dictionary built at a time: the requests that find it outdated wait for one build.
    building: Mutex<()>,
    /// When the background thread last went round (Unix seconds): the health endpoint tells.
    pub(crate) heartbeat: Arc<std::sync::atomic::AtomicU64>,
}

pub type Shared = Arc<App>;

impl App {
    /// The background thread's heartbeat, to set from that thread.
    fn heartbeat_handle(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.heartbeat.clone()
    }
}

impl App {
    pub fn new(cfg: Arc<Config>, db: Arc<Db>) -> App {
        let reg = Arc::new(RwLock::new(Registry::load(&cfg)));
        let viewer = Arc::new(crate::viewer::Supervisor::new(&cfg));
        // the index stores and their keys, written to the viewer at each start (viewer.rs)
        {
            let (cfg, db) = (cfg.clone(), db.clone());
            viewer.set_store_lines(Arc::new(move || {
                let lines = db.get().and_then(|c| crate::viewer::store_lines(&cfg, &c));
                lines.unwrap_or_else(|e| {
                    eprintln!("medorama: the viewer's index stores: {e}");
                    vec![]
                })
            }));
        }
        let dbbuilder = Arc::new(crate::dbbuilder::DbBuilder::new(&cfg));
        App {
            cookie: format!("medorama_{}", cfg.port),
            cfg,
            db,
            reg,
            viewer,
            dbbuilder,
            gateway: crate::gateway::Gateway::default(),
            signin: Default::default(),
            dict: Mutex::new(None),
            building: Mutex::new(()),
            heartbeat: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

pub struct ApiError(pub(crate) StatusCode, pub(crate) String);

/// What a user is told when the control store's one writer is busy (SQLite, during a refresh).
const BUSY: &str = "Medorama is busy saving a database's refresh: try again in a moment.";

impl IntoResponse for ApiError {
    fn into_response(mut self) -> Response {
        if self.1.contains("database is locked") || self.1.contains("database is busy") {
            self = ApiError(StatusCode::SERVICE_UNAVAILABLE, BUSY.into());
            return (self.0, [(header::RETRY_AFTER, "5")], Json(json!({ "detail": self.1 }))).into_response();
        }
        // a fault of the server (not of the request): said on its standard error too, for
        // whoever runs it (`medorama serve`, the service's log)
        if self.0.is_server_error() {
            eprintln!("medorama: {} {}", self.0.as_u16(), self.1);
        }
        (self.0, Json(json!({ "detail": self.1 }))).into_response()
    }
}

fn bad(e: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, e.to_string())
}

fn forbidden(msg: &str) -> ApiError {
    ApiError(StatusCode::FORBIDDEN, msg.into())
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub(crate) type ApiResult = Result<Response, ApiError>;

fn ok(v: impl serde::Serialize) -> ApiResult {
    Ok(Json(v).into_response())
}

/// Run blocking work (database, files) off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)?
}

/// The signed-in user (401 without a valid session), in the session's mode: in Privacy Mode,
/// as without the PHI permission (`User::in_privacy_mode`).
pub struct Auth(pub User);

/// Whether a session is in Privacy Mode: always, for users without the PHI permission; for the
/// others, as they chose in this session (`POST privacy`; kept by the server: signin.rs
/// `set_privacy`), on by default for everyone but admins.
pub fn in_privacy(u: &User, choice: Option<bool>) -> bool {
    if !u.phi_allowed {
        return true;
    }
    choice.unwrap_or(!u.is_admin())
}

/// The request's user, as the privacy layer found it (one database read per request), or why
/// the request has none (the proxy named someone without an account).
#[derive(Clone)]
struct Session(Option<User>, Option<String>);

/// Requests the page makes on its own (its checks for new data, lists refreshing while work
/// runs): they do not keep a session from ending when its user is away (`session_idle_minutes`).
fn background(path: &str, headers: &HeaderMap) -> bool {
    path == "/api/v1/version" || headers.get("x-background").is_some_and(|v| v == "1")
}

/// The address a request came from (the proxy's, behind one), when the server was started with it.
pub(crate) fn peer(ext: &axum::http::Extensions) -> Option<std::net::IpAddr> {
    ext.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0.ip())
}

/// The request's user (signin.rs), in the session's mode; or none, and why when it is not just
/// that nobody signed in.
async fn session_user(
    app: &Shared,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    active: bool,
) -> Result<(Option<User>, Option<String>), ApiError> {
    let a = app.clone();
    let headers = headers.clone();
    blocking(move || {
        let c = a.db.get().map_err(internal)?;
        let who = crate::signin::identify(&a.signin, &a.cfg, &c, &a.cookie, &headers, peer, active).map_err(internal)?;
        let (u, choice) = match who {
            crate::signin::Who::User(u, choice) => (u, choice),
            crate::signin::Who::Nobody => return Ok((None, None)),
            crate::signin::Who::NoAccount(name) => {
                let why = format!(
                    "your institution's sign-in names you {name}, and Medorama has no account of that name: ask an administrator"
                );
                return Ok((None, Some(why)));
            }
        };
        if in_privacy(&u, choice) {
            let notes = !crate::deid::load(&c).map_err(internal)?.hides("notes");
            return Ok((Some(u.in_privacy_mode(notes)), None));
        }
        Ok((Some(u), None))
    })
    .await
}

impl FromRequestParts<Shared> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Self::Rejection> {
        let (user, why) = match parts.extensions.get::<Session>() {
            Some(Session(u, why)) => (u.clone(), why.clone()),
            None => session_user(app, &parts.headers, peer(&parts.extensions), true).await?,
        };
        match (user, why) {
            (Some(u), _) => Ok(Auth(u)),
            (None, Some(why)) => Err(forbidden(&why)),
            (None, None) => Err(ApiError(StatusCode::UNAUTHORIZED, "sign in required".into())),
        }
    }
}

/// Liveness, without a session (a service manager, a load balancer): whether the control store
/// answers and the background thread (checks, sealing, automatic indexing) went round in the
/// last two minutes.  Nothing else: no names, no counts.
async fn health(State(app): State<Shared>) -> Response {
    let beat = app.heartbeat.load(std::sync::atomic::Ordering::Relaxed);
    let alive = beat > 0 && crate::db::now() as u64 - beat < 120;
    let store = {
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            app.db.get().and_then(|c| Ok(c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?)).is_ok()
        })
        .await
        .unwrap_or(false)
    };
    let body = json!({ "ok": alive && store, "control_store": store, "background": alive });
    let code = if alive && store { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (code, Json(body)).into_response()
}

/// The privacy gate for a session in Privacy Mode (gate.rs): its settings, the identifier
/// dictionary (rebuilt when the identifiers it holds change: the `identifiers` version, which
/// triggers move on every change of what it is built from), the gate's secret.
pub fn restricted(app: &App, c: &crate::sql::Connection) -> Result<Arc<crate::gate::Restricted>, ApiError> {
    // a control store whose schema lacks the version: a dictionary built for this answer alone
    let version: Option<(i64, i64)> =
        c.query_row("SELECT identifiers, recordings FROM data_version", [], |r| Ok((r.get(0)?, r.get(1)?))).ok();
    let current = |d: &Arc<crate::gate::Dict>| version.is_some_and(|v| (d.version, d.recordings_version) == v);
    let cached = app.dict.lock().unwrap().clone();
    let dict = match cached {
        Some(d) if current(&d) => d,
        _ => {
            // one build at a time: a request that waited takes the one just built
            let _one = app.building.lock().unwrap_or_else(|e| e.into_inner());
            // (the lock taken and released here: the arm below takes it again)
            let now = app.dict.lock().unwrap().clone();
            match now {
                Some(d) if current(&d) => d,
                // only recordings were kept since: they are added to it
                Some(d) if version.is_some_and(|v| d.version == v.0) => {
                    let v = version.map_or(-1, |v| v.1);
                    let d = Arc::new(d.with_new_recordings(c, &app.cfg, v).map_err(internal)?);
                    *app.dict.lock().unwrap() = Some(d.clone());
                    d
                }
                _ => {
                    let (iv, rv) = version.unwrap_or((-1, -1));
                    let d = Arc::new(crate::gate::Dict::build(c, &app.cfg, iv, rv).map_err(internal)?);
                    if version.is_some() {
                        *app.dict.lock().unwrap() = Some(d.clone());
                    }
                    d
                }
            }
        }
    };
    Ok(Arc::new(crate::gate::Restricted {
        settings: crate::deid::load(c).map_err(internal)?,
        dict,
        secret: crate::gate::secret(c).map_err(internal)?,
    }))
}

/// An answer the gate has cleared for a session in Privacy Mode (the privacy layer passes it).
fn cleared(mut r: Response) -> Response {
    r.headers_mut().insert(crate::gate::CLEARED, HeaderValue::from_static("1"));
    r
}

/// A file whose content the gate cannot inspect (pictures, video, PDF, DICOM pixels): marked,
/// so that the page warns that it may show the patient's identity.
fn may_hold_phi(mut r: Response) -> Response {
    r.headers_mut().insert(crate::gate::MAY_HOLD_PHI, HeaderValue::from_static("1"));
    r
}

/// A JSON answer through the gate when the session is in Privacy Mode (exports made in it, or
/// asked de-identified).
fn gated_json(g: Option<&crate::gate::Restricted>, mut v: Value) -> Value {
    if let Some(g) = g {
        g.json(&mut v);
    }
    v
}

fn admin(u: &User) -> Result<(), ApiError> {
    if u.is_admin() {
        Ok(())
    } else {
        Err(forbidden("only admins may do this"))
    }
}

// ------------------------------------------------------------------ the session (sign-in: signin.rs)

#[derive(Deserialize)]
struct PrivacyChoice {
    on: bool,
}

/// The request's user, as the privacy layer found it (none before it ran, or nobody signed in).
pub(crate) fn session_of(ext: &axum::http::Extensions) -> Option<User> {
    ext.get::<Session>().and_then(|s| s.0.clone())
}

/// The page's Privacy Mode switch: on for anyone; off only with the PHI permission.  The choice
/// is the session's, kept by the server (a new sign-in starts in the user's default), and
/// audited (`privacy_on`, `privacy_off`): the only way a session leaves Privacy Mode.
async fn set_privacy(
    State(app): State<Shared>,
    Auth(u): Auth,
    headers: HeaderMap,
    Json(b): Json<PrivacyChoice>,
) -> ApiResult {
    if !b.on && !u.phi_allowed {
        return Err(forbidden("Privacy Mode is always on for your account: leaving it needs the PHI permission"));
    }
    let a = app.clone();
    blocking(move || {
        let c = a.db.get().map_err(internal)?;
        crate::signin::set_privacy(&a.signin, &a.cfg, &c, &a.cookie, &headers, &u, b.on).map_err(bad)?;
        audit(&c, Some(&u), if b.on { "privacy_on" } else { "privacy_off" }, "", "");
        Ok(())
    })
    .await?;
    ok(json!({ "privacy": b.on }))
}

async fn me(Auth(u): Auth) -> ApiResult {
    ok(json!({ "user": u }))
}

#[derive(Deserialize)]
struct PasswordChange {
    old: String,
    new: String,
}

async fn my_password(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<PasswordChange>) -> ApiResult {
    if app.cfg.sign_in != crate::signin::Method::Password {
        return Err(bad(
            "this Medorama signs users in through the institution's sign-in: Medorama's passwords are not used",
        ));
    }
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        if users::authenticate(&c, &u.username, &req.old).map_err(internal)?.is_none() {
            return Err(forbidden("the current password is wrong"));
        }
        let ch = Changes { display_name: None, role: None, phi: None, disabled: None, password: Some(&req.new) };
        users::update(&mut c, u.id, &ch).map_err(bad)?;
        audit(&c, Some(&u), "password_changed", &u.username, "");
        ok(json!({}))
    })
    .await
}

// ------------------------------------------------------------------ control store

async fn config(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    let c = &app.cfg;
    let path = |p: &std::path::Path| u.is_admin().then(|| p.to_string_lossy().to_string());
    ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        // a demo installation (invented patients only): every page says so
        "demo": app.db.get().ok().is_some_and(|c| crate::installation::is_demo(&c)),
        "viewer_url": c.viewer_url,
        "timezone": c.timezone,
        "auto_index": c.auto_index,
        "workers": c.workers,
        "user": u,
        "sign_in": c.sign_in.name(),
        "workspace_dir": path(&c.workspace_dir),
        "viewer": app.viewer.status(),
        // db-builder's page (its own sign-in: Medorama only links to it), for admins only
        "db_builder": u.is_admin().then(|| app.dbbuilder.status()),
        // the date offset is for admins only: with it, any shown date gives the real one
        "deid": app.db.get().ok().and_then(|c| crate::deid::load(&c).ok()).map(|s| {
            let mut d = crate::deid::describe(&s);
            if !u.is_admin() {
                if let Some(m) = d.as_object_mut() {
                    for k in ["offset", "offset_seconds", "spread_days"] {
                        m.remove(k);
                    }
                }
            }
            d
        }),
        "display": app.db.get().ok().and_then(|c| crate::display::load(&c).ok()).map(|d| crate::display::describe(&d)),
    }))
}

async fn deid_get(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || ok(crate::deid::describe(&crate::deid::load(&*app.db.get().map_err(internal)?).map_err(bad)?)))
        .await
}

async fn deid_put(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<crate::deid::Settings>) -> ApiResult {
    if !u.is_admin() {
        return Err(forbidden("only admins change what the de-identified view hides"));
    }
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        // withholding less is for who may see what it withholds
        if !u.phi_allowed && b.weaker_than(&crate::deid::load(&c).map_err(internal)?) {
            return Err(forbidden(
                "only an admin with the PHI permission may withhold less (untick a choice, fewer days of each patient's own, a smaller offset)",
            ));
        }
        ok(crate::deid::describe(&crate::deid::save(&c, &u, &b).map_err(bad)?))
    })
    .await
}

/// How the page writes dates and times (admins change it; audited).
async fn display_put(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<crate::display::Display>) -> ApiResult {
    if !u.is_admin() {
        return Err(forbidden("only admins change how dates and times are shown"));
    }
    blocking(move || {
        ok(crate::display::describe(&crate::display::save(&*app.db.get().map_err(internal)?, &u, &b).map_err(bad)?))
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RecQuery {
    q: String,
    limit: Option<usize>,
    offset: Option<usize>,
}

/// The recordings of the databases a user may view, matching a search.
async fn recordings_list(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<RecQuery>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        // the recordings this session sees de-identified (Privacy Mode; outside it, those of the
        // databases granted without `phi`)
        let shown = crate::patient::without_phi(&c, &u).map_err(internal)?;
        let g = if shown.is_empty() { None } else { Some(restricted(&app, &c)?) };
        let (total, items) =
            recordings::list_with(&app.cfg, &c, &u, &q.q, limit, q.offset.unwrap_or(0), g.as_deref()).map_err(bad)?;
        if u.privacy && !u.phi && !q.q.trim().is_empty() {
            audit(&c, Some(&u), "privacy_search", "recordings", &q.q);
        }
        let mut items = serde_json::to_value(items).map_err(internal)?;
        if let (false, Some(g)) = (u.privacy, &g) {
            // (in Privacy Mode the last layer gates the whole answer)
            for it in items.as_array_mut().into_iter().flatten() {
                if it["database"].as_i64().is_some_and(|d| shown.contains(&d)) {
                    g.json(it);
                }
            }
        }
        ok(json!({ "total": total, "items": items }))
    })
    .await
}

/// A recording (of a database the user may view) and the results of the actions run on it.
async fn recording(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let Some(r) = recordings::get(&app.cfg, &c, id).map_err(bad)? else { return Err(not_found()) };
        if !crate::grants::can(&c, &u, &format!("db:{}", r.database), "view").map_err(internal)? {
            return Err(not_found());
        }
        let gated = file_restricted(&c, &u, id)?;
        let mut st = c
            .prepare(
                "SELECT id, action, action_version, state, finished_at, summary, error FROM jobs
                 WHERE recording = ?1 AND state IN ('done', 'failed') ORDER BY id DESC LIMIT 100",
            )
            .map_err(internal)?;
        let results: Vec<Value> = st
            .query_map([id], |x| {
                let summary: String = x.get(5)?;
                Ok(json!({
                    "job": x.get::<_, i64>(0)?, "action": x.get::<_, String>(1)?, "version": x.get::<_, String>(2)?,
                    "state": x.get::<_, String>(3)?, "finished_at": x.get::<_, Option<f64>>(4)?,
                    "summary": serde_json::from_str::<Value>(&summary).unwrap_or(Value::Null),
                    "error": x.get::<_, String>(6)?,
                }))
            })
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        let mut v = json!({ "recording": r, "results": results });
        if !gated {
            return ok(v);
        }
        // the whole answer as its patient's (each patient's own offset, the recording's and its
        // results' times alike), the times a plug-in wrote under keys of its own too
        let g = restricted(&app, &c)?;
        let g = match r.patient {
            Some(p) => g.for_patient(p),
            None => g.for_recording(&crate::gate::recording_token(id)),
        };
        for x in v["results"].as_array_mut().into_iter().flatten() {
            crate::deid::shift_unknown_times(&mut x["summary"], &g.settings);
        }
        g.json(&mut v);
        Ok(cleared(Json(v).into_response()))
    })
    .await
}

const OUTSIDE_ROOTS: &str = "the folder is outside the folders data may come from (data_roots)";

/// The extensions of the SQLite database files that may be connected.
const DATABASE_EXTENSIONS: &[&str] = &["sqlite", "sqlite3", "db"];

/// The most entries a folder's listing shows.
const MAX_LISTED: usize = 5000;

#[derive(Deserialize, Default)]
#[serde(default)]
struct FolderQuery {
    /// An absolute folder; empty: the folders data may come from.
    dir: String,
    /// The files listed with the folders: `databases` (SQLite files, the default) or `folders`
    /// (none).
    kind: String,
}

/// A folder on this server, as the page's "Choose on the server" dialog shows it (admins: a
/// SQLite database file to connect, a folder): its folders and the files of the kind asked for,
/// inside `data_roots` (anywhere when it is empty).  Not in Privacy Mode, as the viewer's Open
/// dialog: folders' and files' names may identify.
async fn server_files(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<FolderQuery>) -> ApiResult {
    admin(&u)?;
    let wanted: &'static [&'static str] = match q.kind.as_str() {
        "" | "databases" => DATABASE_EXTENSIONS,
        "folders" => &[],
        other => return Err(bad(format!("unknown kind {other:?} (databases, folders)"))),
    };
    if u.privacy {
        return Err(forbidden(
            "folders are not browsed in Privacy Mode (their names may identify): leave Privacy Mode to choose a file on the server",
        ));
    }
    blocking(move || {
        let roots: Vec<String> = if app.cfg.data_roots.is_empty() {
            let (home, root) = (recording_engine::paths::home(), recording_engine::paths::fs_root());
            if home == root {
                vec![root]
            } else {
                vec![home, root]
            }
        } else {
            app.cfg.data_roots.clone()
        };
        if q.dir.trim().is_empty() {
            let items: Vec<Value> = roots.iter().map(|r| json!({ "name": r, "path": r, "dir": true })).collect();
            return ok(json!({ "dir": "", "parent": null, "items": items, "truncated": false }));
        }
        let dir = crate::config::real(std::path::Path::new(&recording_engine::paths::absolute(q.dir.trim())));
        if !dir.is_dir() {
            return Err(bad(format!("no folder {}", dir.display())));
        }
        if !app.cfg.in_data_roots(&dir) {
            return Err(bad(OUTSIDE_ROOTS));
        }
        let rd = std::fs::read_dir(&dir).map_err(|e| bad(format!("{}: {e}", dir.display())))?;
        let (mut items, mut truncated) = (vec![], false);
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let path = e.path();
            let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
            // a link followed (a broken one is left out), and only when it stays inside a root
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let file = meta.is_file()
                && std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|x| wanted.iter().any(|t| x.eq_ignore_ascii_case(t)));
            if !(meta.is_dir() || file) || (link && !app.cfg.in_data_roots(&path)) {
                continue;
            }
            if items.len() == MAX_LISTED {
                truncated = true;
                break;
            }
            let size = if file { Some(meta.len()) } else { None };
            items.push((meta.is_dir(), name, path.to_string_lossy().to_string(), size));
        }
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        // one level up: the parent while it stays inside a root, else the list of the roots
        let is_root = roots.iter().any(|r| crate::config::real(std::path::Path::new(r)) == dir);
        let parent = match dir.parent() {
            Some(p) if !is_root && app.cfg.in_data_roots(p) => p.to_string_lossy().to_string(),
            _ => String::new(),
        };
        let items: Vec<Value> = items
            .into_iter()
            .map(|(d, name, path, size)| json!({ "name": name, "path": path, "dir": d, "size": size }))
            .collect();
        ok(json!({ "dir": dir.to_string_lossy(), "parent": parent, "items": items, "truncated": truncated }))
    })
    .await
}

async fn addons_list(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    let dir = app.cfg.adapters_dir.clone();
    blocking(move || ok(json!({ "dir": dir.to_string_lossy(), "items": crate::addons::list(&dir) }))).await
}

// ------------------------------------------------------------------ patients

#[derive(Deserialize, Default)]
#[serde(default)]
struct PatientsQuery {
    q: String,
    /// Categories the patients must have, comma-separated.
    has: String,
    /// Only a cohort's members.
    cohort: Option<i64>,
    /// Says, for each patient, whether they are in this cohort (`member`).
    mark: Option<i64>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn patients(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<PatientsQuery>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let has: Vec<String> = q.has.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let only = match q.cohort {
            Some(co) => Some(crate::cohorts::member_ids(&c, &app.cfg, &u, co).map_err(|_| not_found())?),
            None => None,
        };
        let (total, items) = crate::patient::list_in(
            &c,
            &app.cfg,
            &u,
            &q.q,
            &has,
            only.as_ref(),
            q.limit.unwrap_or(100),
            q.offset.unwrap_or(0),
        )
        .map_err(bad)?;
        let items: Vec<serde_json::Value> = match q.mark {
            Some(co) => {
                let members = crate::cohorts::member_ids(&c, &app.cfg, &u, co).map_err(|_| not_found())?;
                items
                    .iter()
                    .map(|p| {
                        let mut v = serde_json::to_value(p).unwrap_or_default();
                        v["member"] = members.contains(&p.id).into();
                        v
                    })
                    .collect()
            }
            None => items.iter().map(|p| serde_json::to_value(p).unwrap_or_default()).collect(),
        };
        let categories: Vec<(&str, &str)> = crate::patient::present(&c, &app.cfg, &u)
            .map_err(bad)?
            .into_iter()
            .filter(|k| *k != "enc")
            .map(|k| (k, crate::categories::label(k)))
            .collect();
        ok(json!({ "total": total, "items": items, "categories": categories }))
    })
    .await
}

// ------------------------------------------------------------------ databases

fn attachment_name(name: &str) -> String {
    let plain: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    let encoded: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("attachment; filename=\"{plain}\"; filename*=UTF-8''{encoded}")
}

async fn data_version(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    let _ = &u;
    blocking(move || {
        let v: i64 = app
            .db
            .get()
            .map_err(internal)?
            .query_row("SELECT version FROM data_version WHERE id = 1", [], |r| r.get(0))
            .map_err(internal)?;
        ok(json!({ "data": v }))
    })
    .await
}

// ------------------------------------------------------------------ cohorts

#[derive(Deserialize, Default)]
#[serde(default)]
struct CohortSearch {
    criteria: crate::cohorts::Criteria,
    /// Only among a source's patients (a new subcohort's parent, cohorts to merge) …
    source: Option<crate::cohorts::Source>,
    /// … or among those of a stored cohort's source (its criteria being edited).
    within_source_of: Option<i64>,
    /// The databases ticked (none: every one the user may search).
    databases: Vec<i64>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn cohort_search(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<CohortSearch>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let source = match (&b.source, b.within_source_of) {
            (Some(s), _) => Some(crate::cohorts::checked_source(&c, &u, s).map_err(bad)?),
            (None, Some(id)) => crate::cohorts::get(&c, &app.cfg, &u, id).map_err(|_| not_found())?.source,
            (None, None) => None,
        };
        // a parent cohort counts only while the user may still read it
        let within =
            source.map(|s| crate::cohorts::readable_source_members(&c, &app.cfg, &u, &s)).transpose().map_err(bad)?;
        // the criteria evaluated once, for the members and for those excluded
        ok(crate::cohorts::search_page(
            &c,
            &app.cfg,
            &u,
            &b.criteria,
            Some(&b.databases),
            within.as_ref(),
            b.limit.unwrap_or(100),
            b.offset.unwrap_or(0),
            50,
        )
        .map_err(bad)?)
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParseText {
    text: String,
}

/// Criteria text as the structured query, and in words (or what is wrong with it).
async fn cohort_parse(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<ParseText>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        match crate::cohorts::parse_text(&c, &app.cfg, &u, &b.text) {
            Ok(e) => ok(json!({ "ok": true, "expr": e, "words": crate::cohorts::describe_expr(&e) })),
            Err(e) => ok(json!({ "ok": false, "error": e.to_string() })),
        }
    })
    .await
}

async fn cohorts_list(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || {
        ok(json!({ "items": crate::cohorts::list(&*app.db.get().map_err(internal)?, &app.cfg, &u).map_err(bad)? }))
    })
    .await
}

async fn cohort_create(
    State(app): State<Shared>,
    Auth(u): Auth,
    Json(b): Json<crate::cohorts::NewCohort>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let id = crate::cohorts::create(&c, &app.cfg, &u, &b).map_err(bad)?;
        ok(crate::cohorts::get(&c, &app.cfg, &u, id).map_err(bad)?)
    })
    .await
}

async fn cohort_get(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        ok(crate::cohorts::get(&*app.db.get().map_err(internal)?, &app.cfg, &u, id).map_err(|_| not_found())?)
    })
    .await
}

async fn cohort_change(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(b): Json<crate::cohorts::Change>,
) -> ApiResult {
    blocking(move || ok(crate::cohorts::change(&*app.db.get().map_err(internal)?, &app.cfg, &u, id, &b).map_err(bad)?))
        .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CloneAs {
    name: String,
}

async fn cohort_clone(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(b): Json<CloneAs>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let new = crate::cohorts::clone(&c, &app.cfg, &u, id, &b.name).map_err(bad)?;
        ok(crate::cohorts::get(&c, &app.cfg, &u, new).map_err(bad)?)
    })
    .await
}

async fn cohort_history(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let items = crate::cohorts::history(&*app.db.get().map_err(internal)?, &u, id).map_err(|_| not_found())?;
        ok(json!({ "items": items }))
    })
    .await
}

async fn cohort_delete(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        crate::cohorts::delete(&c, &app.cfg, &u, id).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Page {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn cohort_members(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(p): Query<Page>,
) -> ApiResult {
    blocking(move || {
        let (total, items) = crate::cohorts::members(
            &*app.db.get().map_err(internal)?,
            &app.cfg,
            &u,
            id,
            p.limit.unwrap_or(100),
            p.offset.unwrap_or(0),
        )
        .map_err(|_| not_found())?;
        ok(json!({ "total": total, "items": items }))
    })
    .await
}

async fn cohort_refresh(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(b): Json<crate::cohorts::RefreshOpts>,
) -> ApiResult {
    blocking(move || {
        ok(crate::cohorts::refresh_with(&*app.db.get().map_err(internal)?, &app.cfg, &u, id, &b).map_err(bad)?)
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ExportWhat {
    /// `members` (CSV, the default), `criteria` (JSON), `data` (the members' data, a long CSV),
    /// `xlsx` (all of it, a sheet per database).
    what: String,
    /// Through the privacy gate, as Privacy Mode shows it (`privacy=1`; `deid=1`, its earlier name).
    privacy: bool,
    deid: bool,
}

impl ExportWhat {
    fn privacy(&self) -> bool {
        self.privacy || self.deid
    }
}

async fn cohort_export(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(q): Query<ExportWhat>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        // through the gate when asked, and always in Privacy Mode
        let g = if q.privacy() || u.privacy { Some(restricted(&app, &c)?) } else { None };
        let deid = g.as_deref();
        let (name, kind, body): (String, &str, Vec<u8>) = match q.what.as_str() {
            "criteria" => {
                let (n, t) = crate::cohorts::export_criteria(&c, &app.cfg, &u, id).map_err(|_| not_found())?;
                let v: Value = serde_json::from_str(&t).map_err(internal)?;
                let t = serde_json::to_string_pretty(&gated_json(deid, v)).map_err(internal)?;
                (n, "application/json; charset=utf-8", t.into_bytes())
            }
            "data" => {
                let (n, t) = crate::cohorts::export_data_csv(&c, &app.cfg, &u, id, deid).map_err(|_| not_found())?;
                (n, "text/csv; charset=utf-8", t.into_bytes())
            }
            "xlsx" => {
                let (n, b) = crate::cohorts::export_xlsx(&c, &app.cfg, &u, id, deid).map_err(|_| not_found())?;
                (n, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", b)
            }
            _ => {
                let (n, t) = crate::cohorts::export_csv(&c, &app.cfg, &u, id, deid).map_err(|_| not_found())?;
                (n, "text/csv; charset=utf-8", t.into_bytes())
            }
        };
        // a demo installation's exports say so in their name
        let name = if crate::installation::is_demo(&c) { format!("demo-{name}") } else { name };
        Ok(cleared(
            ([(header::CONTENT_TYPE, kind.to_string()), (header::CONTENT_DISPOSITION, attachment_name(&name))], body)
                .into_response(),
        ))
    })
    .await
}

/// The users a cohort can be shared with (names only).
async fn cohort_people(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || {
        let people: Vec<serde_json::Value> = crate::users::list(&*app.db.get().map_err(internal)?)
            .map_err(bad)?
            .into_iter()
            .filter(|x| !x.disabled && x.id != u.id)
            .map(|x| json!({ "id": x.id, "username": x.username, "display_name": x.display_name }))
            .collect();
        ok(json!({ "items": people }))
    })
    .await
}

/// An answer about one patient.  In Privacy Mode, their own names and date of birth are also
/// replaced wherever its text names them (notes, reports), besides what the gate's last layer
/// does to every answer.
fn patient_answer(
    c: &crate::sql::Connection,
    cfg: &Config,
    u: &User,
    patient: i64,
    x: impl serde::Serialize,
) -> ApiResult {
    let mut v = serde_json::to_value(x).map_err(internal)?;
    if u.privacy {
        let (names, birth) = crate::gate::Restricted::identity_words(c, cfg, patient);
        crate::gate::scrub_names(&mut v, &names, &birth);
    }
    ok(v)
}

async fn patient_summary(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let summary = crate::patient::summary(&c, &app.cfg, &u, id).map_err(bad)?;
        crate::patient::seen(&c, &u, id).map_err(internal)?;
        patient_answer(&c, &app.cfg, &u, id, summary)
    })
    .await
}

async fn patient_trends(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        patient_answer(&c, &app.cfg, &u, id, crate::patient::trends(&c, &app.cfg, &u, id).map_err(bad)?)
    })
    .await
}

async fn patient_journey(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        patient_answer(&c, &app.cfg, &u, id, crate::patient::journey(&c, &app.cfg, &u, id).map_err(bad)?)
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReportQuery {
    q: String,
}

async fn patient_report(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(q): Query<ReportQuery>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let r = crate::patient::report(&c, &app.cfg, &u, id, &q.q).map_err(bad)?;
        if u.phi && !q.q.trim().is_empty() {
            audit(&c, Some(&u), "phi_search", &format!("patient {id}"), &q.q);
        }
        patient_answer(&c, &app.cfg, &u, id, r)
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DetailQuery {
    key: String,
}

async fn patient_detail(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(q): Query<DetailQuery>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        // in Privacy Mode, the key as it was sent (its time moved back)
        let key = if u.privacy { restricted(&app, &c)?.real_key(&q.key) } else { q.key.clone() };
        patient_answer(&c, &app.cfg, &u, id, crate::patient::detail(&c, &app.cfg, &u, id, &key).map_err(bad)?)
    })
    .await
}

/// A recording's file (an ECG, an image, a PDF), or a recording folder's listing.
/// In Privacy Mode, files are named by their tokens (gate.rs `file_token`) and pass the gate.
async fn patient_file(State(app): State<Shared>, Auth(u): Auth, UrlPath((id, item)): UrlPath<(i64, i64)>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let restricted_file = file_restricted(&c, &u, item)?;
        match crate::patient::attachment(&c, &app.cfg, &u, id, item).map_err(|_| not_found())? {
            // an attached folder (a DICOM series): its files, for a viewer
            crate::patient::Attached::Listing(files) if restricted_file => {
                // only the files the gate would send, by their tokens (numbered as in the folder)
                let dir = crate::patient::item_file(&c, &app.cfg, &u, id, item).map_err(|_| not_found())?;
                let signals = crate::patient::signal_names(&dir);
                let tokens: Vec<String> = files
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| crate::gate::file_kind(&dir.join(n), &signals) != crate::gate::FileKind::Refused)
                    .map(|(i, n)| crate::gate::file_token(i, n))
                    .collect();
                Ok(cleared(Json(json!({ "files": tokens })).into_response()))
            }
            crate::patient::Attached::Listing(files) => ok(json!({ "files": files })),
            crate::patient::Attached::File(path, mime) => attachment_response(&app, &c, &u, id, item, &path, mime),
        }
    })
    .await
}

/// Whether a recording's files leave de-identified, through the gate: in Privacy Mode, and
/// outside it when the user holds no `phi` grant on the recording's database (the PHI
/// permission shows the databases granted `phi` only: patient/restricted.rs).
fn file_restricted(c: &crate::sql::Connection, u: &User, recording: i64) -> Result<bool, ApiError> {
    if u.privacy {
        return Ok(true);
    }
    let (db, _) = recordings::split(recording);
    Ok(!crate::grants::can(c, u, &format!("db:{db}"), "phi").map_err(internal)?)
}

/// A file as the user gets it.  De-identified ([`file_restricted`]) it passes the gate
/// (gate.rs `FileKind`): WFDB headers and MUSE XML de-identified, DICOM headers rewritten
/// (dicom.rs) with the pixels as they are, SVG pictures without their texts and PDF documents
/// without their descriptive fields (deid.rs `svg`, `pdf_info`), pictures, videos and PDF
/// marked, anything else refused.  Shown in a sandbox where it could carry scripts (SVG, XML).
fn attachment_response(
    app: &App,
    c: &crate::sql::Connection,
    u: &User,
    patient: i64,
    item: i64,
    path: &std::path::Path,
    mime: &'static str,
) -> Result<Response, ApiError> {
    use crate::gate::FileKind;
    if !file_restricted(c, u, item)? {
        return file_response(path, mime, None, 0);
    }
    // the patient's own date offset
    let g = restricted(app, c)?.for_patient(patient);
    let text_response = |text: String| {
        cleared(
            (
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    (header::CONTENT_SECURITY_POLICY, "sandbox; default-src 'none'; frame-ancestors 'self'".into()),
                    (header::CACHE_CONTROL, "no-store".into()),
                ],
                text,
            )
                .into_response(),
        )
    };
    let signals = crate::patient::item_file(c, &app.cfg, u, patient, item)
        .map(|p| crate::patient::signal_names(&p))
        .unwrap_or_default();
    match crate::gate::file_kind(path, &signals) {
        FileKind::WfdbHeader => {
            let text = std::fs::read_to_string(path).map_err(|_| not_found())?;
            let names = crate::patient::attachment_names(c, &app.cfg, u, patient, item).map_err(|_| not_found())?;
            let real = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // the header's own number among the attachment's files names the record
            let at = names.iter().position(|n| *n == real).unwrap_or(0);
            let tokens: HashMap<String, String> =
                names.iter().enumerate().map(|(i, n)| (n.clone(), crate::gate::file_token(i, n))).collect();
            let record = crate::gate::file_token(at, "");
            let (who, birth) = crate::gate::Restricted::identity_words(c, &app.cfg, patient);
            Ok(text_response(crate::gate::wfdb_header(&g, &text, &record, &tokens, (&who, &birth))))
        }
        FileKind::Signal => Ok(cleared(file_response(path, mime, None, 0)?)),
        FileKind::MatSignal => {
            let refused = |e: String| forbidden(&format!("not available in Privacy Mode: {e}"));
            let mut head = Vec::with_capacity(512);
            let mut f = std::fs::File::open(path).map_err(|_| not_found())?;
            let size = f.metadata().map_err(|_| not_found())?.len();
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut f, 512), &mut head).map_err(|_| not_found())?;
            let (head, from, until) =
                recording_engine::sources::wfdb_folder::mat_deidentified(&head, size).map_err(refused)?;
            Ok(cleared(file_part_response(path, mime, Some(head), from, Some(until))?))
        }
        FileKind::MuseXml => {
            let text = String::from_utf8_lossy(&std::fs::read(path).map_err(|_| not_found())?).to_string();
            if !text.contains("<RestingECG") {
                return Err(forbidden("this XML file is not one the privacy gate can de-identify (GE MUSE): not available in Privacy Mode"));
            }
            // the patient's own names and date of birth in its texts (statements) first
            let (who, birth) = crate::gate::Restricted::identity_words(c, &app.cfg, patient);
            let text = crate::gate::xml_texts(&text, &|t| crate::gate::own_words(t, &who, &birth));
            let x = crate::deid::muse_xml(&text, &g.settings)
                .map_err(|e| forbidden(&format!("not available in Privacy Mode: {e}")))?;
            // its dates are moved: identifiers only
            Ok(text_response(g.dict.scrub_words(&x).unwrap_or(x)))
        }
        FileKind::Dicom => {
            let pseudonym = g.pseudonym(patient);
            let (names, birth) = crate::gate::Restricted::identity_words(c, &app.cfg, patient);
            let r = crate::dicom::Rewrite {
                pseudonym: &pseudonym,
                offset_s: if g.settings.hides("dates") { g.settings.offset_seconds() } else { 0 },
                cap_ages: g.settings.hides("age_over_89"),
                uid_key: &g.secret,
                text: &|s| g.free_text(s, &names, &birth),
            };
            let w = crate::dicom::deidentify(path, &r)
                .map_err(|e| forbidden(&format!("not available in Privacy Mode: {e}")))?;
            Ok(may_hold_phi(cleared(file_response(path, mime, Some(w.head), w.rest_from)?)))
        }
        FileKind::Edf => {
            let w = edf_deidentified(path, &g.settings)
                .map_err(|e| forbidden(&format!("not available in Privacy Mode: {e}")))?;
            Ok(cleared(file_response(path, mime, Some(w.0), w.1)?))
        }
        FileKind::Opaque if mime.starts_with("image/svg") => {
            // its texts (labels, title, description, metadata) left out: they may name the patient
            let text = String::from_utf8_lossy(&std::fs::read(path).map_err(|_| not_found())?).to_string();
            let mut r = text_response(crate::deid::svg(&text));
            r.headers_mut().insert(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(
                    "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; frame-ancestors 'self'",
                ),
            );
            Ok(may_hold_phi(r))
        }
        FileKind::Opaque
            if mime == "application/pdf" && std::fs::metadata(path).is_ok_and(|m| m.len() <= PDF_IN_MEMORY) =>
        {
            // its descriptive fields (author, title, subject, keywords …) blanked, in place
            let mut b = std::fs::read(path).map_err(|_| not_found())?;
            crate::deid::pdf_info(&mut b);
            Ok(may_hold_phi(cleared(
                ([(header::CONTENT_TYPE, mime.to_string()), (header::CACHE_CONTROL, "no-store".into())], b)
                    .into_response(),
            )))
        }
        FileKind::Opaque => Ok(may_hold_phi(cleared(file_response(path, mime, None, 0)?))),
        FileKind::Refused => Err(forbidden("this kind of file is not available in Privacy Mode")),
    }
}

/// PDF documents whose descriptive fields are blanked in Privacy Mode, at most: larger ones are
/// sent as they are (marked, as every PDF is).
const PDF_IN_MEMORY: u64 = 64 << 20;

/// Files of EDF+ annotations rewritten in memory, at most: larger ones are refused in Privacy
/// Mode (VitalStream shows them, through the gate).
const EDF_IN_MEMORY: u64 = 64 << 20;

/// An EDF or BDF file de-identified (deid.rs `edf_header`, `edf_annotations`): the bytes to send
/// first, and where in the file the rest follows from.  Without annotation signals, the header
/// alone is rewritten and the samples follow as they are; with them, the whole file is
/// rewritten (up to [`EDF_IN_MEMORY`]).
fn edf_deidentified(path: &std::path::Path, s: &crate::deid::Settings) -> anyhow::Result<(Vec<u8>, u64)> {
    use recording_engine::sources::edf;
    use std::io::Read;
    let h = edf::header(path)?;
    let head_len = 256 * (h.signals.len() + 1);
    let mut head = vec![0u8; head_len];
    std::fs::File::open(path)?.read_exact(&mut head)?;
    let head = crate::deid::edf_header(&head, s);
    if !h.signals.iter().any(|x| x.annotations) {
        return Ok((head, head_len as u64));
    }
    let size = std::fs::metadata(path)?.len();
    anyhow::ensure!(size <= EDF_IN_MEMORY, "an EDF+ file of annotations larger than 64 MB (open it in VitalStream)");
    let width = if h.bdf { 3 } else { 2 };
    let record: usize = h.signals.iter().map(|x| x.spr * width).sum();
    // the whole records only: bytes after them could hold part of an annotation
    let mut all = std::fs::read(path)?;
    all.truncate(head_len + h.records as usize * record);
    all[..head_len].copy_from_slice(&head);
    for r in 0..h.records as usize {
        let mut at = head_len + r * record;
        for x in &h.signals {
            let n = x.spr * width;
            if x.annotations {
                crate::deid::edf_annotations(&mut all[at..at + n]);
            }
            at += n;
        }
    }
    Ok((all, size))
}

/// A file of an item's attachment, by name (a WFDB record's signal file, a file of a folder);
/// in Privacy Mode, by its token.
async fn patient_file_part(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath((id, item, name)): UrlPath<(i64, i64, String)>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let name = if file_restricted(&c, &u, item)? {
            let i = crate::gate::token_index(&name).ok_or_else(not_found)?;
            let names = crate::patient::attachment_names(&c, &app.cfg, &u, id, item).map_err(|_| not_found())?;
            names.get(i).filter(|n| crate::gate::file_token(i, n) == name).cloned().ok_or_else(not_found)?
        } else {
            name
        };
        let (path, mime) =
            crate::patient::attachment_part(&c, &app.cfg, &u, id, item, &name).map_err(|_| not_found())?;
        attachment_response(&app, &c, &u, id, item, &path, mime)
    })
    .await
}

/// A file as the answer, streamed (a recording's video or a DICOM loop may be gigabytes: it is
/// never held in memory whole).
/// `head` (a rewritten DICOM header), then the file from byte `from` on.
fn file_response(
    path: &std::path::Path,
    mime: &'static str,
    head: Option<Vec<u8>>,
    from: u64,
) -> Result<Response, ApiError> {
    file_part_response(path, mime, head, from, None)
}

/// [`file_response`] of the file's bytes up to `until` (its end when None).
fn file_part_response(
    path: &std::path::Path,
    mime: &'static str,
    head: Option<Vec<u8>>,
    from: u64,
    until: Option<u64>,
) -> Result<Response, ApiError> {
    {
        use std::io::{Seek, SeekFrom};
        use tokio::io::AsyncReadExt;
        let mut f = std::fs::File::open(path).map_err(|_| not_found())?;
        let size = f.metadata().map_err(|_| not_found())?.len();
        let end = until.map_or(size, |u| u.min(size));
        f.seek(SeekFrom::Start(from.min(end))).map_err(|_| not_found())?;
        let len = end - from.min(end) + head.as_ref().map_or(0, |h| h.len() as u64);
        let first = futures_util::stream::iter(head.map(Ok::<_, std::io::Error>));
        let f = tokio::fs::File::from_std(f).take(end - from.min(end));
        let rest = futures_util::stream::unfold(Some(f), |f| async move {
            let mut f = f?;
            let mut buf = vec![0u8; 256 * 1024];
            match f.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some((Ok::<_, std::io::Error>(buf), Some(f)))
                }
                Err(e) => Some((Err(e), None)),
            }
        });
        let data = axum::body::Body::from_stream(futures_util::StreamExt::chain(first, rest));
        Ok((
            [
                (header::CONTENT_TYPE, mime.to_string()),
                (
                    header::CONTENT_SECURITY_POLICY,
                    if mime.starts_with("image/svg") || mime == "application/xml" {
                        // an SVG or XML file can carry scripts: they must not run with Medorama's rights
                        "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; frame-ancestors 'self'"
                    } else {
                        "frame-ancestors 'self'"
                    }
                    .into(),
                ),
                // patients' files are never kept by the browser (nor by a proxy)
                (header::CACHE_CONTROL, "no-store".into()),
                (header::VARY, "Cookie".into()),
                (header::CONTENT_LENGTH, len.to_string()),
            ],
            data,
        )
            .into_response())
    }
}

async fn check(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let r = recordings::check(&app.cfg, &c).map_err(bad)?;
        audit(&c, Some(&u), "check", "", &serde_json::to_string(&r).unwrap_or_default());
        ok(r)
    })
    .await
}

// ------------------------------------------------------------------ actions and jobs

async fn actions(State(app): State<Shared>, Auth(_u): Auth) -> ApiResult {
    let r = app.reg.read().unwrap().clone();
    ok(json!({ "actions": r.actions, "problems": r.problems }))
}

async fn reload_actions(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    let r = Registry::load(&app.cfg);
    *app.reg.write().unwrap() = r.clone();
    ok(json!({ "actions": r.actions, "problems": r.problems }))
}

#[derive(Deserialize)]
struct RunReq {
    actions: Vec<String>,
    #[serde(default)]
    params: HashMap<String, Value>,
    selection: Selection,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    priority: i64,
}

async fn create_run(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<RunReq>) -> ApiResult {
    if !u.can_run() {
        return Err(forbidden("running actions needs the analyst or admin role"));
    }
    blocking(move || {
        let reg = app.reg.read().unwrap().clone();
        let mut c = app.db.get().map_err(internal)?;
        let p = req.priority.clamp(0, 100);
        ok(jobs::create_run(&mut c, &app.cfg, &reg, &u, &req.actions, &req.params, &req.selection, req.force, p)
            .map_err(bad)?)
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JobQuery {
    state: Option<String>,
    run: Option<i64>,
    limit: Option<i64>,
}

/// The jobs a user may see (jobs.rs `list_for`: an admin's every one; others' those of the
/// databases granted to them, with the errors of their own runs).  A job's results folder is an
/// admin's to know; de-identified (Privacy Mode, or a database granted without `phi`), a failed
/// job's error (whose text may name the recording's files) is only `failed`, and its recording
/// is not named.
async fn jobs_list(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<JobQuery>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let state = q.state.as_deref().filter(|s| !s.is_empty());
        let limit = q.limit.unwrap_or(200).clamp(1, 2000);
        let items = jobs::list_for(&c, &app.cfg, &u, state, q.run, limit).map_err(bad)?;
        let shown = crate::patient::without_phi(&c, &u).map_err(internal)?;
        let mut out = vec![];
        for j in items {
            let deidentified = u.privacy || shown.contains(&j.database);
            let mut it = serde_json::to_value(&j).map_err(internal)?;
            if !u.is_admin() || u.privacy {
                it["output_dir"] = json!("");
            }
            if deidentified && !j.error.is_empty() {
                it["error"] = json!("failed");
            }
            if deidentified && !u.privacy {
                it["recording_name"] = json!(crate::gate::RECORDING);
            }
            out.push(it);
        }
        ok(json!({ "items": out, "counts": jobs::counts_for(&c, &u).map_err(bad)? }))
    })
    .await
}

#[derive(Deserialize)]
struct CancelReq {
    job: Option<i64>,
    run: Option<i64>,
}

async fn cancel(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<CancelReq>) -> ApiResult {
    if !u.can_run() {
        return Err(forbidden("cancelling jobs needs the analyst or admin role"));
    }
    if req.job.is_none() && req.run.is_none() && !u.is_admin() {
        return Err(forbidden("only admins cancel all jobs"));
    }
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "cancelled": jobs::cancel(&c, &u, req.job, req.run).map_err(bad)? }))
    })
    .await
}

// ------------------------------------------------------------------ history

async fn history(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "items": crate::patient::history(&c, &u, 500).map_err(bad)? }))
    })
    .await
}

async fn forget_one(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "removed": crate::patient::forget(&c, &u, Some(id)).map_err(bad)? }))
    })
    .await
}

async fn forget_all(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "removed": crate::patient::forget(&c, &u, None).map_err(bad)? }))
    })
    .await
}

// ------------------------------------------------------------------ users and logs

// ------------------------------------------------------------------ connected databases

/// The connected databases a user is shown (docs/design/databases-only.md): those granted to
/// them, with their status; admins see every one, to grant it.
async fn databases(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "items": crate::databases::shown(&app.cfg, &c, &u).map_err(bad)? }))
    })
    .await
}

#[derive(Deserialize)]
struct NewDatabase {
    name: String,
    kind: String,
    url: String,
    /// Kept wrapped by the master key, never in a file nor sent back.
    #[serde(default)]
    password: String,
    #[serde(default)]
    ca_file: String,
    #[serde(default)]
    settings: crate::sources::Settings,
}

/// Connect a database (admins): checked, then kept with its key and workspace folder, its
/// variables read and its persons linked (`medorama databases connect`).
async fn database_connect(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<NewDatabase>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut settings = b.settings.clone();
        if settings.timezone.trim().is_empty() {
            settings.timezone = app.cfg.timezone.clone();
        }
        let n = crate::sources::NewSource {
            name: b.name.trim().to_string(),
            kind: b.kind.trim().to_string(),
            url: b.url.trim().to_string(),
            password_file: String::new(),
            ca_file: b.ca_file.trim().to_string(),
            settings,
        };
        let c = app.db.get().map_err(internal)?;
        let id = crate::databases::connect(&app.cfg, &c, &u, &n, &b.password).map_err(bad)?;
        // a running viewer gets its index store (the lines it has are given again, unchanged)
        for l in crate::viewer::store_lines(&app.cfg, &c).unwrap_or_default() {
            app.viewer.tell(&l);
        }
        ok(json!({ "id": id }))
    })
    .await
}

/// Check a database now (who may search it): online or offline, and its data read again when
/// its version changed.
async fn database_check(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        if !crate::grants::can(&c, &u, &format!("db:{id}"), "search").map_err(internal)? {
            return Err(not_found());
        }
        let online = crate::databases::check(&app.cfg, &c, id).map_err(bad)?;
        let d = crate::databases::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        ok(json!({ "online": online, "status": d.status, "version": d.data_version, "checked_at": d.checked_at }))
    })
    .await
}

/// Who manages database `id`: 404 for who may not even search it (it is not shown), else 403.
fn require_manage(c: &crate::sql::Connection, u: &User, id: i64) -> Result<(), ApiError> {
    let owner = format!("db:{id}");
    if !crate::grants::can(c, u, &owner, "search").map_err(internal)? {
        return Err(not_found());
    }
    if !crate::grants::can(c, u, &owner, "manage").map_err(internal)? {
        return Err(forbidden("only who manages this database may do this"));
    }
    Ok(())
}

/// Read everything kept of a database again (who manages it).
async fn database_refresh(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        require_manage(&c, &u, id)?;
        crate::databases::refresh(&app.cfg, &c, &u, id).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

/// Count a database's variables (who manages it): one pass over its entries (**Count now**).
async fn database_count(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        require_manage(&c, &u, id)?;
        let n = crate::databases::count_variables(&app.cfg, &c, &u, id).map_err(bad)?;
        ok(json!({ "counted": n }))
    })
    .await
}

/// Read every recording a database lists, check its files, and queue the new or changed ones
/// for indexing (who manages it: **Index all recordings**).
async fn database_index(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        require_manage(&c, &u, id)?;
        let r = crate::databases::index_recordings(&app.cfg, &c, &u, id).map_err(bad)?;
        let queued = crate::jobs::queue_auto_index(&c, &app.cfg).map_err(internal)?;
        ok(json!({ "recordings": r.recordings, "added": r.added, "changed": r.changed, "missing": r.missing,
                   "removed": r.removed, "queued": queued }))
    })
    .await
}

/// Remove a database and every trace of it (who manages it): its key destroyed, its workspace
/// folder, cohort rows and links removed.  The database itself is not touched.
async fn database_remove(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        require_manage(&c, &u, id)?;
        // the viewer forgets its index store first
        app.viewer.tell(&crate::viewer::forget_line(&app.cfg, id));
        crate::databases::remove(&app.cfg, &c, &u, id).map_err(bad)?;
        crate::recordings::sweep_index(&app.cfg, &c).map_err(internal)?;
        ok(json!({}))
    })
    .await
}

/// What a database holds (who may search it): its variables, kept per data version, with their
/// counts once counted (small ones as "< 11"), and when they were counted.
async fn database_variables(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        if !crate::grants::can(&c, &u, &format!("db:{id}"), "search").map_err(internal)? {
            return Err(not_found());
        }
        let items: Vec<crate::variables::ShownVariable> =
            crate::variables::of(&app.cfg, &c, id).map_err(bad)?.into_iter().map(Into::into).collect();
        let counted_at = crate::variables::counted_at(&app.cfg, &c, id).map_err(bad)?;
        ok(json!({ "items": items, "counted_at": counted_at }))
    })
    .await
}

/// A database's identity conflicts (who manages it): persons whose identifiers name two
/// patients, by their database's key (never their identifiers).
async fn database_conflicts(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        require_manage(&c, &u, id)?;
        let items: Vec<Value> = crate::persons::conflicts(&app.cfg, &c, id)
            .map_err(bad)?
            .into_iter()
            .map(|(k, d)| json!({ "person_key": k, "detail": d }))
            .collect();
        ok(json!({ "items": items }))
    })
    .await
}

/// Who may use a database (who manages it).
async fn database_grants(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let o = format!("db:{id}");
        require_manage(&c, &u, id)?;
        let items: Vec<Value> = crate::grants::of_object(&c, &o)
            .map_err(internal)?
            .into_iter()
            .map(|(uid, name, p)| json!({ "user_id": uid, "username": name, "permission": p }))
            .collect();
        ok(json!({ "items": items }))
    })
    .await
}

#[derive(Deserialize)]
struct GrantReq {
    /// None: the user asking (an admin granting themselves).
    #[serde(default)]
    user_id: Option<i64>,
    permission: String,
}

/// Grant a permission on a database (who manages it).  An admin may grant themselves any
/// (audited): admins are not granted every database by being admins.
async fn database_grant(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(b): Json<GrantReq>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let o = format!("db:{id}");
        let to = b.user_id.unwrap_or(u.id);
        // an admin granting themselves a database they do not manage (admins are shown every
        // database: its number tells them nothing); anyone else manages it, else 404 to who may
        // not search it, as if it did not exist
        let own = u.is_admin() && to == u.id && !crate::grants::can(&c, &u, &o, "manage").map_err(internal)?;
        if !own {
            require_manage(&c, &u, id)?;
        }
        crate::sources::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        let by = if own {
            // an admin's own grant: made as the person at the terminal would, and said so
            crate::db::audit(&c, Some(&u), "grant_self", &o, &b.permission);
            User::local()
        } else {
            u.clone()
        };
        crate::grants::grant(&c, &by, to, &o, &b.permission).map_err(|e| forbidden(&e.to_string()))?;
        ok(json!({}))
    })
    .await
}

async fn database_revoke(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(b): Json<GrantReq>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        require_manage(&c, &u, id)?;
        crate::grants::revoke(&c, &u, b.user_id.unwrap_or(u.id), &format!("db:{id}"), &b.permission)
            .map_err(|e| forbidden(&e.to_string()))?;
        ok(json!({}))
    })
    .await
}
async fn users_list(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || ok(json!({ "items": users::list(&*app.db.get().map_err(internal)?).map_err(bad)? }))).await
}

#[derive(Deserialize)]
struct NewUser {
    username: String,
    #[serde(default)]
    display_name: String,
    role: String,
    #[serde(default)]
    phi: bool,
    /// Optional when users sign in through the institution's sign-in (a password nobody knows).
    #[serde(default)]
    password: String,
}

/// Only an admin who holds the PHI permission may give it (to anyone, themselves included).
fn may_grant_phi(u: &User, phi: Option<bool>) -> Result<(), ApiError> {
    if phi == Some(true) && !u.phi_allowed {
        return Err(forbidden("only an admin with the PHI permission may give it"));
    }
    Ok(())
}

async fn user_add(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<NewUser>) -> ApiResult {
    admin(&u)?;
    may_grant_phi(&u, Some(req.phi))?;
    let role = Role::parse(&req.role).ok_or_else(|| bad("role: viewer, analyst or admin"))?;
    let proxy = app.cfg.sign_in != crate::signin::Method::Password;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let password =
            if req.password.is_empty() && proxy { users::unusable_password().map_err(internal)? } else { req.password };
        let id = users::add(&c, &req.username, &req.display_name, role, req.phi, &password).map_err(bad)?;
        audit(&c, Some(&u), "user_added", &req.username, &format!("role {}, phi {}", role.as_str(), req.phi));
        ok(json!({ "id": id }))
    })
    .await
}

#[derive(Deserialize)]
struct UserChange {
    display_name: Option<String>,
    role: Option<String>,
    phi: Option<bool>,
    disabled: Option<bool>,
    password: Option<String>,
    /// End the user's sessions on every device (they sign in again).
    sign_out: Option<bool>,
}

async fn user_change(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(req): Json<UserChange>,
) -> ApiResult {
    admin(&u)?;
    may_grant_phi(&u, req.phi)?;
    let role = match &req.role {
        Some(r) => Some(Role::parse(r).ok_or_else(|| bad("role: viewer, analyst or admin"))?),
        None => None,
    };
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        let target = users::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        // another's password, for a user who may see identifiers: only who may see them too
        // (else an admin without the permission could sign in as them)
        let phi_reset = req.password.is_some() && target.phi_allowed && id != u.id;
        if phi_reset && !u.phi_allowed {
            return Err(forbidden("only an admin with the PHI permission may reset the password of a user who has it"));
        }
        let ch = Changes {
            display_name: req.display_name.as_deref(),
            role,
            phi: req.phi,
            disabled: req.disabled,
            password: req.password.as_deref(),
        };
        // (a new password ends every session of the user: users.rs `update`)
        users::update(&mut c, id, &ch).map_err(bad)?;
        if phi_reset {
            audit(&c, Some(&u), "phi_password_reset", &target.username, "the user's sessions ended");
        }
        let mut detail = vec![];
        if let Some(r) = role {
            detail.push(format!("role {}", r.as_str()));
        }
        if let Some(p) = req.phi {
            detail.push(format!("phi {p}"));
        }
        if let Some(d) = req.disabled {
            detail.push(format!("disabled {d}"));
        }
        if req.password.is_some() {
            detail.push("password reset".into());
        }
        if req.sign_out == Some(true) {
            let n = users::end_sessions(&c, id).map_err(internal)?;
            detail.push(format!("signed out everywhere ({n} session(s))"));
        }
        audit(&c, Some(&u), "user_changed", &id.to_string(), &detail.join(", "));
        ok(json!({}))
    })
    .await
}

async fn user_delete(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        users::delete(&mut c, id).map_err(bad)?;
        audit(&c, Some(&u), "user_deleted", &id.to_string(), "");
        ok(json!({}))
    })
    .await
}

async fn audit_list(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    admin(&u)?;
    let limit: i64 = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(500).clamp(1, 10000);
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let mut st = c
            .prepare("SELECT time, username, event, target, detail FROM audit ORDER BY id DESC LIMIT ?1")
            .map_err(internal)?;
        let v: Vec<Value> = st
            .query_map([limit], |r| {
                // in Privacy Mode: no target or detail (a search's text may name a patient)
                let hide = u.privacy;
                let text = |s: String| if hide && !s.is_empty() { crate::gate::WITHHELD.to_string() } else { s };
                Ok(json!({"time": r.get::<_, f64>(0)?, "username": r.get::<_, String>(1)?, "event": r.get::<_, String>(2)?,
                          "target": text(r.get::<_, String>(3)?), "detail": text(r.get::<_, String>(4)?)}))
            })
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        ok(json!({ "items": v }))
    })
    .await
}

/// Whether the audit log's seals hold (admins): `{seals, entries, unsealed, broken}`.
async fn audit_verify(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(crate::audit::verify_with_export(&c, app.cfg.audit_export.as_deref()).map_err(internal)?)
    })
    .await
}

// ------------------------------------------------------------------ viewer

async fn viewer_status(State(app): State<Shared>, Auth(_u): Auth) -> ApiResult {
    ok(app.viewer.status())
}

/// Start the viewer now (the page asks before showing it, so the first View is quick).
async fn viewer_start(State(app): State<Shared>, Auth(_u): Auth) -> ApiResult {
    if !app.viewer.available() {
        return Err(ApiError(StatusCode::NOT_FOUND, app.viewer.status().reason));
    }
    app.viewer.ensure().await.map_err(|e| ApiError(StatusCode::SERVICE_UNAVAILABLE, e))?;
    ok(app.viewer.status())
}

// ------------------------------------------------------------------ page

fn static_response(path: &str) -> Response {
    if path.split('/').any(|c| c == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    match WEB.get_file(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.to_string()), (header::CACHE_CONTROL, "no-cache".into())],
                f.contents().to_vec(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn page() -> Response {
    static_response("index.html")
}

/// A viewer's page and scripts (not its tests or notes).
async fn viewer_file(UrlPath((name, path)): UrlPath<(String, String)>) -> Response {
    let path = if path.is_empty() || path.ends_with('/') { format!("{path}index.html") } else { path };
    let Some((_, files)) = VIEWERS.iter().find(|(n, _)| *n == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match files.iter().find(|(f, _)| *f == path) {
        Some((_, bytes)) => (
            [
                (header::CONTENT_TYPE, mime_guess::from_path(&path).first_or_octet_stream().to_string()),
                (header::CACHE_CONTROL, "no-cache".into()),
            ],
            bytes.to_vec(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn viewer_index(UrlPath(name): UrlPath<String>) -> Response {
    viewer_file(UrlPath((name, String::new()))).await
}

async fn static_file(UrlPath(path): UrlPath<String>) -> Response {
    static_response(&path)
}

/// The privacy gate's last layer (gate.rs): finds the session's user once (for `Auth`); in
/// Privacy Mode, passes the answers the gate cleared, puts every JSON answer through the gate,
/// and refuses any other answer that carries data.  So an endpoint that forgets the gate
/// cannot leak: its answer is refused (and said on the server's standard error).
async fn privacy(State(app): State<Shared>, mut req: Request, next: Next) -> Response {
    let active = !background(req.uri().path(), req.headers());
    let (user, why) = match session_user(&app, req.headers(), peer(req.extensions()), active).await {
        Ok(u) => u,
        Err(e) => return e.into_response(),
    };
    let in_privacy_mode = user.as_ref().is_some_and(|u| u.privacy);
    req.extensions_mut().insert(Session(user, why));
    let path = req.uri().path().to_string();
    let mut r = next.run(req).await;
    let was_cleared = r.headers_mut().remove(crate::gate::CLEARED).is_some();
    if !in_privacy_mode || was_cleared {
        return r;
    }
    // the pages and their scripts hold no data; redirections neither
    let page = path == "/"
        || path.starts_with("/static/")
        || path.starts_with("/viewers/")
        || path == "/viewer"
        || path == "/viewer/"
        || path.starts_with("/viewer/static/");
    if page || r.status().is_redirection() {
        return r;
    }
    let json = r
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("application/json"));
    if !json {
        if r.status().is_success() {
            eprintln!("medorama: privacy gate: refused an answer to {path} that it did not clear");
            return forbidden("this answer is not available in Privacy Mode").into_response();
        }
        return r; // an error without data
    }
    // the session itself (who is signed in, its mode)
    if ["/api/v1/me", "/api/v1/login", "/api/v1/logout", "/api/v1/privacy", "/api/v1/sign-in", "/api/v1/health"]
        .contains(&path.as_str())
    {
        return r;
    }
    let (mut parts, body) = r.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 256 * 1024 * 1024).await else {
        return internal("the answer could not be read").into_response();
    };
    let Ok(mut v) = serde_json::from_slice::<Value>(&bytes) else {
        eprintln!("medorama: privacy gate: an answer to {path} is not JSON as it says");
        return internal("the answer could not be read").into_response();
    };
    let a = app.clone();
    let g = tokio::task::spawn_blocking(move || a.db.get().map_err(internal).and_then(|c| restricted(&a, &c))).await;
    let g = match g {
        Ok(Ok(g)) => g,
        Ok(Err(ApiError(_, why))) => {
            return internal(format!("the privacy gate's settings could not be read: {why}")).into_response()
        }
        Err(_) => return internal("the privacy gate's settings could not be read").into_response(),
    };
    // a patient's own pages: their dates move by their own offset, whatever the objects name.
    // A fault in the gate refuses the answer (fail closed), never drops the connection
    let gated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match patient_of_path(&path) {
        Some(p) => g.for_patient(p).json(&mut v),
        None => g.json(&mut v),
    }));
    if gated.is_err() {
        eprintln!("medorama: privacy gate: an internal error on an answer to {path}: refused");
        return internal("the privacy gate could not clear this answer").into_response();
    }
    let out = serde_json::to_vec(&v).unwrap_or_default();
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, axum::body::Body::from(out))
}

/// The patient a path is about: `/api/v1/patients/<id>/…`.
fn patient_of_path(path: &str) -> Option<i64> {
    path.strip_prefix("/api/v1/patients/")?.split('/').next()?.parse().ok()
}

/// Security headers on every response; `X-Medorama: 1` required on API requests that change
/// something, the viewer's through the gateway too; API responses are not cached.
async fn guard(req: Request, next: Next) -> Response {
    let api = req.uri().path().starts_with("/api/");
    let viewer = req.uri().path() == "/viewer" || req.uri().path().starts_with("/viewer/");
    let viewers = req.uri().path().starts_with("/viewers/");
    let changes = req.method() != Method::GET && req.method() != Method::HEAD;
    let viewer_api = req.uri().path().starts_with("/viewer/api/");
    if (api || viewer_api) && changes && req.headers().get("x-medorama").and_then(|v| v.to_str().ok()) != Some("1") {
        return forbidden("missing X-Medorama header").into_response();
    }
    let mut r = next.run(req).await;
    let h = r.headers_mut();
    if viewer {
        // the viewer's page, shown in Medorama's VitalStream tab: its own scripts and styles
        // only (it has no inline ones; its icon is a data: picture), framing by Medorama only
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; \
                 frame-ancestors 'self'",
            ),
        );
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return r;
    }
    if viewers {
        // the ECG, echo and imaging viewers: shown in Medorama's tabs; they read the patients'
        // files through the API and show them from blob: addresses (pictures, PDF, video)
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; frame-src 'self' blob:; \
                 object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'",
            ),
        );
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        return r;
    }
    if h.contains_key(header::CONTENT_SECURITY_POLICY) {
        // a response with a policy of its own (patients' files): shown inside Medorama's page
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    } else {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; img-src 'self' data:; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        );
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if api {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    r
}

pub fn router(app: Shared) -> Router {
    let api = Router::new()
        .route("/sign-in", get(crate::signin::method))
        .route("/login", post(crate::signin::login))
        .route("/logout", post(crate::signin::logout))
        .route("/me", get(me))
        .route("/me/password", post(my_password))
        .route("/privacy", post(set_privacy))
        .route("/config", get(config))
        .route("/deid", get(deid_get))
        .route("/admin/deid", axum::routing::put(deid_put))
        .route("/admin/display", axum::routing::put(display_put))
        .route("/recordings", get(recordings_list))
        .route("/recordings/{id}", get(recording))
        .route("/check", post(check))
        .route("/actions", get(actions))
        .route("/actions/reload", post(reload_actions))
        .route("/runs", post(create_run))
        .route("/jobs", get(jobs_list))
        .route("/jobs/cancel", post(cancel))
        .route("/history", get(history).delete(forget_all))
        .route("/history/{id}", delete(forget_one))
        .route("/databases", get(databases).post(database_connect))
        .route("/databases/{id}", axum::routing::delete(database_remove))
        .route("/databases/{id}/check", post(database_check))
        .route("/databases/{id}/refresh", post(database_refresh))
        .route("/databases/{id}/count", post(database_count))
        .route("/databases/{id}/index", post(database_index))
        .route("/databases/{id}/variables", get(database_variables))
        .route("/databases/{id}/conflicts", get(database_conflicts))
        .route("/databases/{id}/grants", get(database_grants).post(database_grant).delete(database_revoke))
        .route("/version", get(data_version))
        .route("/health", get(health))
        .route("/cohorts", get(cohorts_list).post(cohort_create))
        .route("/cohorts/search", post(cohort_search))
        .route("/cohorts/parse", post(cohort_parse))
        .route("/cohorts/people", get(cohort_people))
        .route("/cohorts/{id}", get(cohort_get).patch(cohort_change).delete(cohort_delete))
        .route("/cohorts/{id}/members", get(cohort_members))
        .route("/cohorts/{id}/clone", post(cohort_clone))
        .route("/cohorts/{id}/history", get(cohort_history))
        .route("/cohorts/{id}/refresh", post(cohort_refresh))
        .route("/cohorts/{id}/export", get(cohort_export))
        .route("/users", get(users_list).post(user_add))
        .route("/users/{id}", axum::routing::patch(user_change).delete(user_delete))
        .route("/audit", get(audit_list))
        .route("/audit/verify", get(audit_verify))
        .route("/admin/addons", get(addons_list))
        .route("/server-files", get(server_files))
        .route("/patients", get(patients))
        .route("/patients/{id}", get(patient_summary))
        .route("/patients/{id}/journey", get(patient_journey))
        .route("/patients/{id}/report", get(patient_report))
        .route("/patients/{id}/trends", get(patient_trends))
        .route("/patients/{id}/detail", get(patient_detail))
        .route("/patients/{id}/file/{item}", get(patient_file))
        .route("/patients/{id}/file/{item}/", get(patient_file))
        .route("/patients/{id}/file/{item}/{name}", get(patient_file_part))
        .route("/viewer", get(viewer_status))
        .route("/viewer/start", post(viewer_start))
        .fallback(|| async { not_found() });
    Router::new()
        .route("/", get(page))
        .route("/static/{*path}", get(static_file))
        .route("/viewers/{name}/", get(viewer_index))
        .route("/viewers/{name}/{*path}", get(viewer_file))
        .nest("/api/v1", api)
        .route("/viewer", axum::routing::any(crate::gateway::handle))
        .route("/viewer/", axum::routing::any(crate::gateway::handle))
        .route("/viewer/{*rest}", axum::routing::any(crate::gateway::handle))
        .layer(middleware::from_fn_with_state(app.clone(), privacy))
        .layer(middleware::from_fn(guard))
        .with_state(app)
}

/// How often the server checks each connected database (online, offline, its data version).
const DATABASE_CHECK: Duration = Duration::from_secs(60);

/// Background threads: `workers` job workers, and a scheduler that checks the databases, queues
/// automatic indexing and re-checks the recordings.  They end when `stop` is set.
pub fn start_background(app: &App, stop: Arc<AtomicBool>) -> Vec<std::thread::JoinHandle<()>> {
    let mut threads = vec![];
    for n in 0..app.cfg.workers {
        let (db, cfg, reg, stop) = (app.db.clone(), app.cfg.clone(), app.reg.clone(), stop.clone());
        threads.push(std::thread::spawn(move || jobs::work(db, cfg, reg, n, stop, false)));
    }
    let (db, cfg) = (app.db.clone(), app.cfg.clone());
    let app_heartbeat = app.heartbeat_handle();
    threads.push(std::thread::spawn(move || {
        let app_heartbeat = &*app_heartbeat;
        let mut last_check: Option<Instant> = None;
        let mut last_status: Option<Instant> = None;
        let mut last_queue: Option<Instant> = None;
        let mut last_seal: Option<Instant> = None;
        // each step on its own: a panic in one is reported and the others go on (the health
        // endpoint shows the heartbeat, which a dead thread would stop)
        let step = |what: &str, f: &dyn Fn() -> anyhow::Result<()>| match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(f),
        ) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("medorama: {what}: {e}"),
            Err(_) => eprintln!("medorama: {what}: stopped on an internal error (reported above)"),
        };
        while !stop.load(Ordering::Relaxed) {
            app_heartbeat.store(crate::db::now() as u64, Ordering::Relaxed);
            let interval = Duration::from_secs(cfg.check_interval_s);
            if cfg.check_interval_s > 0 && last_check.is_none_or(|t| t.elapsed() >= interval) {
                step("check", &|| db.get().and_then(|c| recordings::check(&cfg, &c)).map(|_| ()));
                last_check = Some(Instant::now());
            }
            // each database online or offline (and read again when its data changed)
            if last_status.is_none_or(|t| t.elapsed() >= DATABASE_CHECK) {
                step("checking the databases", &|| {
                    db.get().and_then(|c| crate::databases::check_all(&cfg, &c)).map(|_| ())
                });
                last_status = Some(Instant::now());
            }
            if last_seal.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                step("sealing the audit log", &|| {
                    db.get().and_then(|c| crate::audit::seal(&c, cfg.audit_export.as_deref())).map(|_| ())
                });
                last_seal = Some(Instant::now());
            }
            if cfg.auto_index && last_queue.is_none_or(|t| t.elapsed() >= Duration::from_secs(15)) {
                step("automatic indexing", &|| db.get().and_then(|c| jobs::queue_auto_index(&c, &cfg)).map(|_| ()));
                last_queue = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }));
    threads
}

#[cfg(test)]
mod download_names {
    #[test]
    fn names_cannot_break_the_header() {
        let h = super::attachment_name("labs \"2024\"\r\nSet-Cookie: x=1;é.csv");
        assert!(!h.contains(['\r', '\n']) && axum::http::HeaderValue::from_str(&h).is_ok(), "{h}");
        assert!(
            h.starts_with(
                "attachment; filename=\"labs__2024___Set-Cookie__x_1__.csv\"; filename*=UTF-8''labs%20%222024%22"
            ),
            "{h}"
        );
        assert!(h.ends_with("%C3%A9.csv"), "{h}");
    }
}

#[cfg(test)]
mod viewer_pages {
    /// Every file of the ECG and PACS viewers' pages (their folders' `.html`, `.js` and `.css`
    /// files) is built in, and nothing else is.
    #[test]
    fn every_page_file_is_built_in() {
        for (name, files) in super::VIEWERS.iter() {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../viewers").join(name);
            let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
                .filter(|f| [".html", ".js", ".css"].iter().any(|x| f.ends_with(x)))
                .collect();
            on_disk.sort();
            let mut built: Vec<String> = files.iter().map(|(f, _)| f.to_string()).collect();
            built.sort();
            assert_eq!(built, on_disk, "viewers/{name}: the page files built in (server.rs VIEWERS)");
        }
    }
}
