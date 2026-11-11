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
use crate::query::{self, Search};
use crate::users::{self, Changes, Role, User};
use crate::{ehr, linkage, recordings};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Path as UrlPath, Query, Request, State};
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

/// Largest CSV file accepted by the import endpoints.
const MAX_UPLOAD: usize = 1 << 30;

pub struct App {
    pub cfg: Arc<Config>,
    pub db: Arc<Db>,
    pub reg: Arc<RwLock<Registry>>,
    pub viewer: Arc<crate::viewer::Supervisor>,
    pub gateway: crate::gateway::Gateway,
    /// The name of the sign-in cookie (one per port, so that catalogs on one host do not mix).
    pub(crate) cookie: String,
    pub(crate) signin: crate::signin::SignIn,
    /// The privacy gate's identifier dictionary, kept while the data do not change.
    dict: Mutex<Option<Arc<crate::gate::Dict>>>,
    /// Imports running in the background (and those just ended).
    pub tasks: crate::progress::Tasks,
}

pub type Shared = Arc<App>;

impl App {
    pub fn new(cfg: Arc<Config>, db: Arc<Db>) -> App {
        let reg = Arc::new(RwLock::new(Registry::load(&cfg)));
        let viewer = Arc::new(crate::viewer::Supervisor::new(&cfg));
        App {
            cookie: format!("medorama_{}", cfg.port),
            cfg,
            db,
            reg,
            viewer,
            gateway: crate::gateway::Gateway::default(),
            signin: Default::default(),
            dict: Mutex::new(None),
            tasks: Default::default(),
        }
    }
}

pub struct ApiError(pub(crate) StatusCode, pub(crate) String);

/// What a user is told when the catalog's one writer is busy (SQLite, during an import).
const BUSY: &str = "The catalog is busy saving an import: try again when it is done (Databases shows its progress).";

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

/// The privacy gate for a session in Privacy Mode (gate.rs): its settings, the identifier
/// dictionary (rebuilt when the identifiers it holds change: the `identifiers` version, which
/// triggers move on every change of the tables it is built from), the catalog's secret.
pub fn restricted(app: &App, c: &crate::sql::Connection) -> Result<Arc<crate::gate::Restricted>, ApiError> {
    // a catalog whose schema lacks the version: a dictionary built for this answer alone
    let version: Option<i64> = c.query_row("SELECT identifiers FROM data_version", [], |r| r.get(0)).ok();
    let cached = app.dict.lock().unwrap().clone();
    let dict = match (cached, version) {
        (Some(d), Some(v)) if d.version == v => d,
        (_, v) => {
            let d = Arc::new(crate::gate::Dict::build(c, v.unwrap_or(-1)).map_err(internal)?);
            if v.is_some() {
                *app.dict.lock().unwrap() = Some(d.clone());
            }
            d
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
            "this catalog signs users in through the institution's sign-in: Medorama's passwords are not used",
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

// ------------------------------------------------------------------ catalog

async fn config(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    let c = &app.cfg;
    let path = |p: &std::path::Path| u.is_admin().then(|| p.to_string_lossy().to_string());
    ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "viewer_url": c.viewer_url,
        "timezone": c.timezone,
        "auto_index": c.auto_index,
        "workers": c.workers,
        "user": u,
        "sign_in": c.sign_in.name(),
        "results_dir": path(&c.results_dir),
        "index_dir": path(&c.index_dir),
        "viewer": app.viewer.status(),
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
        ok(crate::deid::describe(&crate::deid::save(&*app.db.get().map_err(internal)?, &u, &b).map_err(bad)?))
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

async fn encounters(State(app): State<Shared>, Auth(u): Auth, Query(s): Query<Search>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        // in Privacy Mode, searched, filtered and sorted on what the session is shown
        let g = if u.privacy { Some(restricted(&app, &c)?) } else { None };
        let (total, rows) = query::search_in(&c, &u, g.as_deref(), &s, &app.cfg.timezone).map_err(bad)?;
        ok(json!({ "total": total, "items": rows }))
    })
    .await
}

async fn encounter(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        match query::encounter(&c, &u, id).map_err(bad)? {
            Some(d) => ok(d),
            None => Err(not_found()),
        }
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EhrQuery {
    domain: Option<String>,
    /// The patient's items outside every encounter, instead of the encounter's.
    patient_level: bool,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn encounter_ehr(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(q): Query<EhrQuery>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let (total, items) = ehr::items(
            &c,
            id,
            q.patient_level,
            q.domain.as_deref().filter(|d| !d.is_empty()),
            u.phi,
            q.limit.unwrap_or(500).clamp(1, 5000),
            q.offset.unwrap_or(0).max(0),
        )
        .map_err(bad)?;
        ok(json!({ "total": total, "items": items }))
    })
    .await
}

/// Recordings to remove: `purge` also deletes their results; `dry_run` says what would go and
/// changes nothing.  Unknown fields are refused, so that a misspelt one is never ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ids {
    ids: Vec<i64>,
    #[serde(default)]
    purge: bool,
    #[serde(default)]
    dry_run: bool,
}

/// Encounters to remove (they hold no files or results: nothing to purge).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EncounterIds {
    ids: Vec<i64>,
    #[serde(default)]
    dry_run: bool,
}

async fn remove_encounters(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<EncounterIds>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        let r = linkage::remove_encounters(&mut c, &u, &req.ids, req.dry_run).map_err(bad)?;
        ok(json!({ "removed": r.encounters, "report": r }))
    })
    .await
}

/// The encounters a search finds (`search`: as `GET encounters` takes it).  Unknown fields are
/// refused: a field this route does not read (`q`, say) must not widen the removal to every
/// encounter.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveMatchingEncounters {
    #[serde(default)]
    search: Search,
    #[serde(default)]
    dry_run: bool,
}

/// The recordings a search finds (`q`, `unlinked`: as `GET recordings` takes them).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveMatchingRecordings {
    #[serde(default)]
    q: String,
    #[serde(default)]
    unlinked: bool,
    #[serde(default)]
    purge: bool,
    #[serde(default)]
    dry_run: bool,
}

/// The encounters matching a search, with their bed stays and EHR items (recordings stay).
async fn remove_matching_encounters(
    State(app): State<Shared>,
    Auth(u): Auth,
    Json(req): Json<RemoveMatchingEncounters>,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        let g = if u.privacy { Some(restricted(&app, &c)?) } else { None };
        let ids = query::search_ids_in(&c, &u, g.as_deref(), &req.search, &app.cfg.timezone).map_err(bad)?;
        let r = linkage::remove_encounters(&mut c, &u, &ids, req.dry_run).map_err(bad)?;
        if !req.dry_run {
            audit(
                &c,
                Some(&u),
                "remove_matching",
                &format!("encounters: {}", req.search.q),
                &serde_json::to_string(&r).unwrap_or_default(),
            );
        }
        ok(r)
    })
    .await
}

/// The recordings matching a search.
async fn remove_matching_recordings(
    State(app): State<Shared>,
    Auth(u): Auth,
    Json(req): Json<RemoveMatchingRecordings>,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        let g = if u.privacy { Some(restricted(&app, &c)?) } else { None };
        let ids =
            recordings::search_ids_in(&c, &u, g.as_deref(), &req.q, req.unlinked, &app.cfg.timezone).map_err(bad)?;
        let n = if req.dry_run {
            ids.len()
        } else {
            recordings::remove(&mut c, &app.cfg, &u, &ids, req.purge).map_err(bad)?
        };
        if !req.dry_run {
            audit(&c, Some(&u), "remove_matching", &format!("recordings: {}", req.q), &n.to_string());
        }
        ok(json!({ "dry_run": req.dry_run, "recordings": n }))
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RecQuery {
    q: String,
    unlinked: bool,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn recordings_list(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<RecQuery>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let limit = q.limit.unwrap_or(100).clamp(1, 1000);
        let g = if u.privacy { Some(restricted(&app, &c)?) } else { None };
        let (total, items) = recordings::list_in(
            &c,
            &u,
            g.as_deref(),
            &q.q,
            q.unlinked,
            limit,
            q.offset.unwrap_or(0).max(0),
            &app.cfg.timezone,
        )
        .map_err(bad)?;
        ok(json!({ "total": total, "items": items }))
    })
    .await
}

async fn recording(State(app): State<Shared>, Auth(_u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let Some(r) = recordings::get(&c, id).map_err(bad)? else { return Err(not_found()) };
        let mut st = c
            .prepare(
                "SELECT e.id, e.encounter_id, l.method FROM recording_links l JOIN encounters e ON e.id = l.encounter
                 WHERE l.recording = ?1 ORDER BY e.admit_time",
            )
            .map_err(internal)?;
        let links: Vec<Value> = st
            .query_map([id], |x| {
                Ok(json!({"id": x.get::<_, i64>(0)?, "encounter_id": x.get::<_, String>(1)?, "method": x.get::<_, String>(2)?}))
            })
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        ok(json!({ "recording": r, "encounters": links, "results": query::results(&c, id).map_err(bad)? }))
    })
    .await
}

async fn remove_recordings(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<Ids>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        let n = if req.dry_run {
            recordings::existing(&c, &req.ids).map_err(bad)?
        } else {
            recordings::remove(&mut c, &app.cfg, &u, &req.ids, req.purge).map_err(bad)?
        };
        ok(json!({ "dry_run": req.dry_run, "removed": n }))
    })
    .await
}

#[derive(Deserialize)]
struct LinkReq {
    encounter: Option<i64>,
}

async fn link_recording(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(req): Json<LinkReq>,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        linkage::link_manual(&mut c, &u, id, req.encounter).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

#[derive(Deserialize)]
struct DecisionReq {
    encounter_id: String,
    /// `link`, `never`, or null (forget the decision).
    decision: Option<String>,
}

async fn link_decision(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(req): Json<DecisionReq>,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        linkage::decide_link(&mut c, &u, id, &req.encounter_id, req.decision.as_deref()).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

async fn review(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(crate::review::review(&c, &u).map_err(bad)?)
    })
    .await
}

#[derive(Deserialize)]
struct HeldReq {
    /// `apply` or `dismiss`.
    action: String,
}

async fn decide_held(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(req): Json<HeldReq>,
) -> ApiResult {
    admin(&u)?;
    let apply = match req.action.as_str() {
        "apply" => true,
        "dismiss" => false,
        other => return Err(bad(format!("unknown action {other:?} (apply, dismiss)"))),
    };
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        crate::review::decide_held(&mut c, &u, id, apply, &app.cfg.timezone).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

#[derive(Deserialize)]
struct PairReq {
    a: String,
    b: String,
}

/// Merge patient `a` into patient `b`.
async fn merge_patients(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<PairReq>) -> ApiResult {
    admin(&u)?;
    if !u.phi {
        return Err(forbidden("merging patients needs an admin with the PHI permission"));
    }
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        crate::review::merge(&mut c, &u, &req.a, &req.b).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

async fn distinct_patients(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<PairReq>) -> ApiResult {
    admin(&u)?;
    if !u.phi {
        return Err(forbidden("deciding on duplicates needs an admin with the PHI permission"));
    }
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        crate::review::distinct(&c, &u, &req.a, &req.b).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UploadQuery {
    /// The file's name, for the import log (and the default batch name and source).
    name: String,
    /// The batch's name and source label (default: the file's name).
    batch: String,
    source: String,
    /// A column mapping: JSON object or TOML text (see linkage.rs).
    mapping: String,
    /// Databases: the category of the table's items when it has no category column.
    category: String,
    /// Databases: a known export to convert (`auto`, `none`, or a format's key).
    format: String,
    dry_run: bool,
    /// Run in the background: the answer is the import's task (202), its progress asked for
    /// at `import-tasks/{id}` until it is done.
    background: bool,
}

impl UploadQuery {
    fn opts(&self) -> crate::imports::Opts {
        crate::imports::Opts::new(
            if self.batch.is_empty() { &self.name } else { &self.batch },
            &self.source,
            self.dry_run,
        )
    }
}

/// Headers, the kind it looks like and the suggested mapping of an uploaded table.
#[derive(Deserialize, Default)]
#[serde(default)]
struct InspectQuery {
    name: String,
    /// `auto` (default), `none`, or a format's key.
    format: String,
}

async fn inspect_table(
    State(app): State<Shared>,
    Auth(u): Auth,
    Query(q): Query<InspectQuery>,
    body: axum::body::Bytes,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let known = crate::formats::all(&app.cfg).0;
        let choice = crate::formats::Choice::parse(&q.format, &known).map_err(bad)?;
        ok(crate::tables::inspect(&app.cfg, &q.name, &body, u.phi, &choice).map_err(bad)?)
    })
    .await
}

async fn import_table(
    State(app): State<Shared>,
    Auth(u): Auth,
    Query(q): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let map = if q.mapping.trim().is_empty() {
            Default::default()
        } else {
            linkage::read_mapping(&q.mapping).map_err(bad)?
        };
        let known = crate::formats::all(&app.cfg).0;
        let format = crate::formats::Choice::parse(&q.format, &known).map_err(bad)?;
        let parts = [crate::tables::Part { name: q.name.clone(), data: crate::tables::PartData::Bytes(&body) }];
        let mut c = app.db.get().map_err(internal)?;
        ok(crate::tables::import_parts(&mut c, &app.cfg, &u, &parts, &map, &q.category, None, &q.opts(), format)
            .map_err(bad)?)
    })
    .await
}

/// Several files with the same columns, one database (a multipart upload: each `file` part; the
/// query as `POST tables`, `batch` the database's name).  Held in memory, never written to disk.
async fn import_table_files(
    State(app): State<Shared>,
    Auth(u): Auth,
    Query(q): Query<UploadQuery>,
    mut form: axum::extract::Multipart,
) -> ApiResult {
    admin(&u)?;
    if q.batch.trim().is_empty() {
        return Err(bad("several files make one database: give it a name"));
    }
    let (label, background_wanted, u2) = (q.batch.trim().to_string(), q.background, u.clone());
    let mut files: Vec<(String, axum::body::Bytes)> = vec![];
    while let Some(field) = form.next_field().await.map_err(|e| bad(format!("the upload: {e}")))? {
        if field.name() != Some("file") {
            continue;
        }
        let name = field.file_name().unwrap_or("table.csv").to_string();
        let data = field.bytes().await.map_err(|e| bad(format!("{name}: {e}")))?;
        files.push((name, data));
    }
    if files.is_empty() {
        return Err(bad("no file"));
    }
    let run = move |app: Shared, progress: Option<Arc<crate::progress::Progress>>| -> Result<Value, ApiError> {
        let map = if q.mapping.trim().is_empty() {
            Default::default()
        } else {
            linkage::read_mapping(&q.mapping).map_err(bad)?
        };
        let known = crate::formats::all(&app.cfg).0;
        let format = crate::formats::Choice::parse(&q.format, &known).map_err(bad)?;
        let parts: Vec<crate::tables::Part> = files
            .iter()
            .map(|(name, data)| crate::tables::Part { name: name.clone(), data: crate::tables::PartData::Bytes(data) })
            .collect();
        let mut c = app.db.get().map_err(internal)?;
        let mut opts = q.opts();
        opts.progress = progress;
        let r = crate::tables::import_parts(&mut c, &app.cfg, &u, &parts, &map, &q.category, None, &opts, format)
            .map_err(bad)?;
        serde_json::to_value(r).map_err(internal)
    };
    if background_wanted {
        return in_background(&app, &u2, &label, move |a, p| run(a, Some(p)));
    }
    blocking(move || run(app, None).and_then(ok)).await
}

/// Run an import in the background (a thread of its own): the answer is its task, at once.
fn in_background(
    app: &Shared,
    user: &User,
    label: &str,
    f: impl FnOnce(Shared, Arc<crate::progress::Progress>) -> Result<Value, ApiError> + Send + 'static,
) -> ApiResult {
    let task = app.tasks.start(user.id, label).map_err(internal)?;
    let (t, a, u, label) = (task.clone(), app.clone(), user.clone(), label.to_string());
    std::thread::Builder::new()
        .name("import".into())
        .spawn(move || {
            let p = t.progress.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(a.clone(), p)))
                .unwrap_or_else(|_| Err(internal("the import stopped on an internal error")));
            // a cancelled import, rolled back: said in the audit log (now that it can be written)
            if t.progress.cancelled() && r.is_err() {
                if let Ok(c) = a.db.get() {
                    audit(&c, Some(&u), "cancel_import", &label, "");
                }
            }
            t.finish(r.map_err(|e| busy_text(&e.1)));
        })
        .map_err(internal)?;
    Ok((StatusCode::ACCEPTED, Json(task.view())).into_response())
}

/// A message for a person: the catalog's being busy said plainly.
fn busy_text(e: &str) -> String {
    if e.contains("database is locked") || e.contains("database is busy") {
        BUSY.into()
    } else {
        e.to_string()
    }
}

/// The imports running in the background, and those ended within the hour (admins).
async fn import_tasks(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    let items: Vec<Value> = app.tasks.list().iter().map(|t| t.view()).collect();
    ok(json!({ "items": items }))
}

/// One import's progress, or its end (admins).
async fn import_task(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<String>) -> ApiResult {
    admin(&u)?;
    let t = app.tasks.get(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such import running".into()))?;
    ok(t.view())
}

/// Stop an import (admins): at its next chunk of rows; nothing of it is kept.
async fn import_task_cancel(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<String>) -> ApiResult {
    admin(&u)?;
    let t = app.tasks.get(&id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such import running".into()))?;
    t.progress.cancel();
    ok(t.view())
}

#[derive(Deserialize)]
struct ClifFolder {
    dir: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    dry_run: bool,
    /// Run in the background (as `tables/files`).
    #[serde(default)]
    background: bool,
}

/// A CLIF folder on this server imported (admins): inside `data_roots`; each table a database.
async fn import_clif(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<ClifFolder>) -> ApiResult {
    admin(&u)?;
    let (background_wanted, u2) = (b.background, u.clone());
    let run = move |app: Shared, progress: Option<Arc<crate::progress::Progress>>| -> Result<Value, ApiError> {
        let dir = crate::config::real(std::path::Path::new(&recording_engine::paths::absolute(b.dir.trim())));
        if !dir.is_dir() {
            return Err(bad(format!("no folder {}", dir.display())));
        }
        if !app.cfg.import_allowed(&dir) {
            return Err(bad(OUTSIDE_ROOTS));
        }
        let source = if b.source.trim().is_empty() { "clif" } else { b.source.trim() };
        let mut c = app.db.get().map_err(internal)?;
        let done = crate::formats::clif::import_folder(&mut c, &app.cfg, &u, &dir, source, b.dry_run, progress)
            .map_err(bad)?;
        let tables: Vec<Value> =
            done.imported.iter().map(|(name, files, r)| json!({ "name": name, "files": files, "report": r })).collect();
        let skipped: Vec<String> = done
            .skipped
            .iter()
            .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
            .collect();
        Ok(json!({ "tables": tables, "skipped": skipped }))
    };
    if background_wanted {
        return in_background(&app, &u2, "CLIF folder", move |a, p| run(a, Some(p)));
    }
    blocking(move || run(app, None).and_then(ok)).await
}

/// What an import is told of a folder or file outside `data_roots`.
const OUTSIDE_ROOTS: &str = "the folder is outside the folders data may come from (data_roots)";

/// The extensions of the tables the page imports (as its upload accepts).
const TABLE_EXTENSIONS: &[&str] = &["csv", "xlsx", "xls", "ods", "parquet"];

/// The extensions of the SQLite database files that may be connected.
const DATABASE_EXTENSIONS: &[&str] = &["sqlite", "sqlite3", "db"];

/// The most entries a folder's listing shows.
const MAX_LISTED: usize = 5000;

#[derive(Deserialize, Default)]
#[serde(default)]
struct FolderQuery {
    /// An absolute folder; empty: the folders data may come from.
    dir: String,
    /// The files listed with the folders: `tables` (default), `databases` (SQLite files) or
    /// `folders` (none).
    kind: String,
}

/// A folder on this server, as the page's "Choose on the server" dialog shows it (admins: the
/// tables to import, a CLIF folder, a database file): its folders and the files of the kind
/// asked for, inside `data_roots` (anywhere when it is empty).  Not in Privacy Mode, as the
/// viewer's Open dialog: folders' and files' names may identify.
async fn server_files(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<FolderQuery>) -> ApiResult {
    admin(&u)?;
    let wanted: &'static [&'static str] = match q.kind.as_str() {
        "" | "tables" => TABLE_EXTENSIONS,
        "databases" => DATABASE_EXTENSIONS,
        "folders" => &[],
        other => return Err(bad(format!("unknown kind {other:?} (tables, databases, folders)"))),
    };
    if u.privacy {
        return Err(forbidden(
            "folders are not browsed in Privacy Mode (their names may identify): leave Privacy Mode to choose tables on the server",
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
        if !app.cfg.import_allowed(&dir) {
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
            if !(meta.is_dir() || file) || (link && !app.cfg.import_allowed(&path)) {
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
            Some(p) if !is_root && app.cfg.import_allowed(p) => p.to_string_lossy().to_string(),
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

/// A table file on this server that may be imported: it exists, inside `data_roots`.
fn server_table(cfg: &crate::config::Config, path: &str) -> Result<std::path::PathBuf, ApiError> {
    let p = crate::config::real(std::path::Path::new(&recording_engine::paths::absolute(path.trim())));
    if !p.is_file() {
        return Err(bad(format!("no file {}", p.display())));
    }
    if !cfg.import_allowed(&p) {
        return Err(bad(OUTSIDE_ROOTS));
    }
    Ok(p)
}

fn file_name_of(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

#[derive(Deserialize)]
struct ServerTable {
    path: String,
    /// `auto` (default), `none`, or a format's key.
    #[serde(default)]
    format: String,
}

/// As `tables/inspect`, a table on this server, read where it is, row by row (any size).
async fn inspect_server_table(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<ServerTable>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let path = server_table(&app.cfg, &b.path)?;
        let known = crate::formats::all(&app.cfg).0;
        let choice = crate::formats::Choice::parse(&b.format, &known).map_err(bad)?;
        let part = crate::tables::Part { name: file_name_of(&path), data: crate::tables::PartData::File(path) };
        ok(crate::tables::inspect_part(&app.cfg, &part, u.phi, &choice).map_err(bad)?)
    })
    .await
}

#[derive(Deserialize)]
struct ServerTables {
    /// One table, or several with the same columns (one database, named by `batch`).
    paths: Vec<String>,
    /// The rest as `POST tables` (`name`: by default the first file's name).
    #[serde(flatten)]
    q: UploadQuery,
}

/// Tables on this server imported where they are, as `medorama import-table` does (admins):
/// read row by row, so of any size; a relative path in a `file` column is read from the first
/// table's folder.
async fn import_server_tables(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<ServerTables>) -> ApiResult {
    admin(&u)?;
    let ServerTables { paths, mut q } = b;
    if paths.is_empty() {
        return Err(bad("no file"));
    }
    if paths.len() > 1 && q.batch.trim().is_empty() {
        return Err(bad("several files make one database: give it a name"));
    }
    if q.name.is_empty() {
        q.name = file_name_of(std::path::Path::new(paths[0].trim()));
    }
    let label = if q.batch.trim().is_empty() { q.name.clone() } else { q.batch.trim().to_string() };
    let (background_wanted, u2) = (q.background, u.clone());
    let run = move |app: Shared, progress: Option<Arc<crate::progress::Progress>>| -> Result<Value, ApiError> {
        let files = paths.iter().map(|p| server_table(&app.cfg, p)).collect::<Result<Vec<_>, _>>()?;
        let map = if q.mapping.trim().is_empty() {
            Default::default()
        } else {
            linkage::read_mapping(&q.mapping).map_err(bad)?
        };
        let known = crate::formats::all(&app.cfg).0;
        let format = crate::formats::Choice::parse(&q.format, &known).map_err(bad)?;
        let parts: Vec<crate::tables::Part> = files
            .iter()
            .map(|f| crate::tables::Part { name: file_name_of(f), data: crate::tables::PartData::File(f.clone()) })
            .collect();
        let base = files[0].parent().map(|p| p.to_path_buf());
        let mut c = app.db.get().map_err(internal)?;
        let mut opts = q.opts();
        opts.progress = progress;
        let r = crate::tables::import_parts(
            &mut c,
            &app.cfg,
            &u,
            &parts,
            &map,
            &q.category,
            base.as_deref(),
            &opts,
            format,
        )
        .map_err(bad)?;
        serde_json::to_value(r).map_err(internal)
    };
    if background_wanted {
        return in_background(&app, &u2, &label, move |a, p| run(a, Some(p)));
    }
    blocking(move || run(app, None).and_then(ok)).await
}

/// The import formats the catalog knows (admins): built in and described in `formats_dirs`,
/// and the files that could not be read.
async fn formats_list(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    let (known, problems) = crate::formats::all(&app.cfg);
    let categories: Vec<Value> =
        crate::categories::CATEGORIES.iter().map(|c| json!({"key": c.0, "label": c.1})).collect();
    ok(json!({ "items": crate::formats::describe(&known), "problems": problems, "categories": categories }))
}

/// The installed adapters (Admin › Add-ons). Installing is done on the server only:
/// `medorama addons install` (crate::addons).
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
            Some(co) => Some(crate::cohorts::member_ids(&c, &u, co).map_err(|_| not_found())?),
            None => None,
        };
        let (total, items) =
            crate::patient::list_in(&c, &u, &q.q, &has, only.as_ref(), q.limit.unwrap_or(100), q.offset.unwrap_or(0))
                .map_err(bad)?;
        let items: Vec<serde_json::Value> = match q.mark {
            Some(co) => {
                let members = crate::cohorts::member_ids(&c, &u, co).map_err(|_| not_found())?;
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
        let categories: Vec<(&str, &str)> = crate::patient::present(&c)
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

async fn databases(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    let _ = &u;
    blocking(move || ok(json!({ "items": crate::databases::list(&*app.db.get().map_err(internal)?).map_err(bad)? })))
        .await
}

/// `?q&sort&desc&limit&offset`, and `f<pos>=text` per column.
async fn database_rows(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(p): Query<HashMap<String, String>>,
) -> ApiResult {
    blocking(move || {
        let num = |k: &str| p.get(k).and_then(|v| v.parse::<usize>().ok());
        let q = crate::databases::Query {
            q: p.get("q").cloned().unwrap_or_default(),
            filters: p
                .iter()
                .filter_map(|(k, v)| k.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()).map(|n| (n, v.clone())))
                .collect(),
            sort: num("sort"),
            desc: p.get("desc").is_some_and(|v| v == "1" || v == "true"),
            limit: num("limit").unwrap_or(100),
            offset: num("offset").unwrap_or(0),
        };
        let c = app.db.get().map_err(internal)?;
        let g = if u.privacy { Some(restricted(&app, &c)?) } else { None };
        ok(crate::databases::rows(&c, &u, g.as_deref(), id, &q).map_err(bad)?)
    })
    .await
}

/// A database as CSV (admins).
/// A download's `Content-Disposition` (RFC 6266): the name as plain ASCII (letters, digits,
/// `-`, `_`, `.`; anything else `_`, so that no quote or line break reaches the header), and
/// as written, percent-encoded UTF-8 (`filename*`).
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

async fn database_export(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Query(q): Query<ExportWhat>,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        // through the gate when asked, and always in Privacy Mode
        let g = if q.privacy() || u.privacy { Some(restricted(&app, &c)?) } else { None };
        let (name, text) = crate::databases::export_csv(&c, &u, id, g.as_deref()).map_err(bad)?;
        Ok(cleared(
            (
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
                    (header::CONTENT_DISPOSITION, attachment_name(&name)),
                ],
                text,
            )
                .into_response(),
        ))
    })
    .await
}

/// The data's version: it moves when databases, patients, encounters or recordings change, so
/// open pages know to redraw.
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
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn cohort_search(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<CohortSearch>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let source = match (&b.source, b.within_source_of) {
            (Some(s), _) => Some(crate::cohorts::checked_source(&c, &u, s).map_err(bad)?),
            (None, Some(id)) => crate::cohorts::get(&c, &u, id).map_err(|_| not_found())?.source,
            (None, None) => None,
        };
        // a parent cohort counts only while the user may still read it
        let within = source.map(|s| crate::cohorts::readable_source_members(&c, &u, &s)).transpose().map_err(bad)?;
        // the criteria evaluated once, for the members and for those excluded
        ok(crate::cohorts::search_page(
            &c,
            &u,
            &app.cfg.timezone,
            &b.criteria,
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
        match crate::cohorts::parse_text(&c, &u, &b.text) {
            Ok(e) => ok(json!({ "ok": true, "expr": e, "words": crate::cohorts::describe_expr(&e) })),
            Err(e) => ok(json!({ "ok": false, "error": e.to_string() })),
        }
    })
    .await
}

async fn cohorts_list(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || ok(json!({ "items": crate::cohorts::list(&*app.db.get().map_err(internal)?, &u).map_err(bad)? })))
        .await
}

async fn cohort_create(
    State(app): State<Shared>,
    Auth(u): Auth,
    Json(b): Json<crate::cohorts::NewCohort>,
) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let id = crate::cohorts::create(&c, &u, &app.cfg.timezone, &b).map_err(bad)?;
        ok(crate::cohorts::get(&c, &u, id).map_err(bad)?)
    })
    .await
}

async fn cohort_get(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || ok(crate::cohorts::get(&*app.db.get().map_err(internal)?, &u, id).map_err(|_| not_found())?)).await
}

async fn cohort_change(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(b): Json<crate::cohorts::Change>,
) -> ApiResult {
    blocking(move || {
        ok(crate::cohorts::change(&*app.db.get().map_err(internal)?, &u, &app.cfg.timezone, id, &b).map_err(bad)?)
    })
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
        let new = crate::cohorts::clone(&c, &u, id, &b.name).map_err(bad)?;
        ok(crate::cohorts::get(&c, &u, new).map_err(bad)?)
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
        let mut c = app.db.get().map_err(internal)?;
        // the demo's cohort: the demo goes with it (admins)
        if crate::cohorts::is_demo(&c, id).map_err(internal)? {
            admin(&u)?;
            let r = crate::demo::remove(&mut c, &app.cfg, &u, false).map_err(bad)?;
            return ok(json!({ "removed": r }));
        }
        crate::cohorts::delete(&c, &u, id).map_err(bad)?;
        ok(json!({}))
    })
    .await
}

/// What deleting the demo's cohort would remove (admins): a dry run.
async fn demo_removal(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        ok(crate::demo::remove(&mut c, &app.cfg, &u, true).map_err(bad)?)
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
        ok(crate::cohorts::refresh_with(&*app.db.get().map_err(internal)?, &u, &app.cfg.timezone, id, &b)
            .map_err(bad)?)
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
                let (n, t) = crate::cohorts::export_criteria(&c, &u, id).map_err(|_| not_found())?;
                let v: Value = serde_json::from_str(&t).map_err(internal)?;
                let t = serde_json::to_string_pretty(&gated_json(deid, v)).map_err(internal)?;
                (n, "application/json; charset=utf-8", t.into_bytes())
            }
            "data" => {
                let (n, t) = crate::cohorts::export_data_csv(&c, &u, id, deid).map_err(|_| not_found())?;
                (n, "text/csv; charset=utf-8", t.into_bytes())
            }
            "xlsx" => {
                let (n, b) = crate::cohorts::export_xlsx(&c, &u, id, deid).map_err(|_| not_found())?;
                (n, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", b)
            }
            _ => {
                let (n, t) = crate::cohorts::export_csv(&c, &u, id, deid).map_err(|_| not_found())?;
                (n, "text/csv; charset=utf-8", t.into_bytes())
            }
        };
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
fn patient_answer(c: &crate::sql::Connection, u: &User, patient: i64, x: impl serde::Serialize) -> ApiResult {
    let mut v = serde_json::to_value(x).map_err(internal)?;
    if u.privacy {
        let (names, birth) = crate::gate::Restricted::identity_words(c, patient);
        crate::gate::scrub_names(&mut v, &names, &birth);
    }
    ok(v)
}

async fn patient_summary(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        patient_answer(&c, &u, id, crate::patient::summary(&c, &u, id).map_err(bad)?)
    })
    .await
}

async fn patient_trends(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        patient_answer(&c, &u, id, crate::patient::trends(&c, &u, id).map_err(bad)?)
    })
    .await
}

async fn patient_journey(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        patient_answer(&c, &u, id, crate::patient::journey(&c, &u, id).map_err(bad)?)
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
        let r = crate::patient::report(&c, &u, id, &q.q).map_err(bad)?;
        if u.phi && !q.q.trim().is_empty() {
            audit(&c, Some(&u), "phi_search", &format!("patient {id}"), &q.q);
        }
        patient_answer(&c, &u, id, r)
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
        patient_answer(&c, &u, id, crate::patient::detail(&c, &app.cfg, &u, id, &key).map_err(bad)?)
    })
    .await
}

/// An item's attached file (an image, a PDF, a text file), or an attached folder's listing.
/// In Privacy Mode, files are named by their tokens (gate.rs `file_token`) and pass the gate.
async fn patient_file(State(app): State<Shared>, Auth(u): Auth, UrlPath((id, item)): UrlPath<(i64, i64)>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        match crate::patient::attachment(&c, &app.cfg, id, item).map_err(|_| not_found())? {
            // an attached folder (a DICOM series): its files, for a viewer
            crate::patient::Attached::Listing(files) if u.privacy => {
                // only the files the gate would send, by their tokens (numbered as in the folder)
                let dir = crate::patient::item_file(&c, &app.cfg, id, item).map_err(|_| not_found())?;
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

/// A file as the user gets it.  In Privacy Mode it passes the gate (gate.rs `FileKind`): WFDB
/// headers and MUSE XML de-identified, DICOM headers rewritten (dicom.rs) with the pixels as
/// they are, pictures, videos and PDF as they are but marked, anything else refused.  Shown in a
/// sandbox where it could carry scripts (SVG, XML).
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
    if !u.privacy {
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
    let signals = crate::patient::item_file(c, &app.cfg, patient, item)
        .map(|p| crate::patient::signal_names(&p))
        .unwrap_or_default();
    match crate::gate::file_kind(path, &signals) {
        FileKind::WfdbHeader => {
            let text = std::fs::read_to_string(path).map_err(|_| not_found())?;
            let names = crate::patient::attachment_names(c, &app.cfg, patient, item).map_err(|_| not_found())?;
            let real = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // the header's own number among the attachment's files names the record
            let at = names.iter().position(|n| *n == real).unwrap_or(0);
            let tokens: HashMap<String, String> =
                names.iter().enumerate().map(|(i, n)| (n.clone(), crate::gate::file_token(i, n))).collect();
            let record = crate::gate::file_token(at, "");
            let (who, birth) = crate::gate::Restricted::identity_words(c, patient);
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
            let (who, birth) = crate::gate::Restricted::identity_words(c, patient);
            let text = crate::gate::xml_texts(&text, &|t| crate::gate::own_words(t, &who, &birth));
            let x = crate::deid::muse_xml(&text, &g.settings)
                .map_err(|e| forbidden(&format!("not available in Privacy Mode: {e}")))?;
            // its dates are moved: identifiers only
            Ok(text_response(g.dict.scrub_words(&x).unwrap_or(x)))
        }
        FileKind::Dicom => {
            let pseudonym = g.pseudonym(patient);
            let (names, birth) = crate::gate::Restricted::identity_words(c, patient);
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
        FileKind::Opaque => Ok(may_hold_phi(cleared(file_response(path, mime, None, 0)?))),
        FileKind::Refused => Err(forbidden("this kind of file is not available in Privacy Mode")),
    }
}

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
        let name = if u.privacy {
            let i = crate::gate::token_index(&name).ok_or_else(not_found)?;
            let names = crate::patient::attachment_names(&c, &app.cfg, id, item).map_err(|_| not_found())?;
            names.get(i).filter(|n| crate::gate::file_token(i, n) == name).cloned().ok_or_else(not_found)?
        } else {
            name
        };
        let (path, mime) = crate::patient::attachment_part(&c, &app.cfg, id, item, &name).map_err(|_| not_found())?;
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
                        // an SVG or XML file can carry scripts: they must not run with the catalog's rights
                        "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'; frame-ancestors 'self'"
                    } else {
                        "frame-ancestors 'self'"
                    }
                    .into(),
                ),
                (header::CACHE_CONTROL, "private, max-age=300".into()),
                // cached per mode: the same address answers differently in Privacy Mode
                (header::VARY, "Cookie".into()),
                (header::CONTENT_LENGTH, len.to_string()),
            ],
            data,
        )
            .into_response())
    }
}

async fn batches(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || ok(json!({ "items": crate::imports::list(&*app.db.get().map_err(internal)?).map_err(bad)? })))
        .await
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RemoveReq {
    source: String,
    purge: bool,
    dry_run: bool,
}

async fn remove_batch(
    State(app): State<Shared>,
    Auth(u): Auth,
    UrlPath(id): UrlPath<i64>,
    Json(req): Json<RemoveReq>,
) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        ok(crate::imports::remove_import(&mut c, &app.cfg, &u, id, req.purge, req.dry_run).map_err(bad)?)
    })
    .await
}

async fn sources(State(app): State<Shared>, Auth(_u): Auth) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let items = crate::imports::sources(&c).map_err(bad)?;
        ok(json!({ "items": items, "waiting": ehr::waiting_count(&c).map_err(bad)? }))
    })
    .await
}

async fn remove_source(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<RemoveReq>) -> ApiResult {
    admin(&u)?;
    if req.source.is_empty() {
        return Err(bad("source required"));
    }
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        ok(crate::imports::remove_source(&mut c, &app.cfg, &u, &req.source, req.purge, req.dry_run).map_err(bad)?)
    })
    .await
}

async fn mappings(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let mut st = c.prepare("SELECT name, kind, columns FROM mappings ORDER BY name").map_err(internal)?;
        let v: Vec<Value> = st
            .query_map([], |r| {
                Ok(json!({"name": r.get::<_, String>(0)?, "kind": r.get::<_, String>(1)?,
                          "columns": serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap_or(Value::Null)}))
            })
            .map_err(internal)?
            .collect::<Result<_, _>>()
            .map_err(internal)?;
        ok(json!({ "items": v }))
    })
    .await
}

#[derive(Deserialize)]
struct MappingReq {
    name: String,
    kind: String,
    columns: HashMap<String, String>,
}

async fn save_mapping(State(app): State<Shared>, Auth(u): Auth, Json(req): Json<MappingReq>) -> ApiResult {
    admin(&u)?;
    if req.name.trim().is_empty() || req.kind != "table" {
        return Err(bad("a name, and kind table"));
    }
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        c.execute(
            "INSERT INTO mappings (name, kind, columns, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (name) DO UPDATE SET kind = excluded.kind, columns = excluded.columns, updated_at = excluded.updated_at",
            crate::sql::params![req.name.trim(), req.kind, serde_json::to_string(&req.columns).unwrap_or_default(), crate::db::now()],
        )
        .map_err(internal)?;
        ok(json!({}))
    })
    .await
}

async fn delete_mapping(State(app): State<Shared>, Auth(u): Auth, UrlPath(name): UrlPath<String>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        c.execute("DELETE FROM mappings WHERE name = ?1", [name]).map_err(internal)?;
        ok(json!({}))
    })
    .await
}

async fn check(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let r = recordings::check(&c, &app.cfg).map_err(bad)?;
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
        ok(jobs::create_run(
            &mut c,
            &reg,
            &u,
            &req.actions,
            &req.params,
            &req.selection,
            req.force,
            p,
            &app.cfg.timezone,
        )
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

/// The jobs.  A job's results folder is an admin's to know; in Privacy Mode, a failed job's error
/// (whose text may name the recording's files) is only `failed`.
async fn jobs_list(State(app): State<Shared>, Auth(u): Auth, Query(q): Query<JobQuery>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let state = q.state.as_deref().filter(|s| !s.is_empty());
        let items = jobs::list(&c, state, q.run, q.limit.unwrap_or(200).clamp(1, 2000)).map_err(bad)?;
        let mut items = serde_json::to_value(items).map_err(internal)?;
        for it in items.as_array_mut().into_iter().flatten() {
            if !u.is_admin() || u.privacy {
                it["output_dir"] = json!("");
            }
            if u.privacy && it["error"].as_str().is_some_and(|e| !e.is_empty()) {
                it["error"] = json!("failed");
            }
        }
        ok(json!({ "items": items, "counts": jobs::counts(&c).map_err(bad)? }))
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
        ok(json!({ "items": query::history(&c, &u, 500).map_err(bad)? }))
    })
    .await
}

async fn forget_one(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "removed": query::forget(&c, &u, Some(id)).map_err(bad)? }))
    })
    .await
}

async fn forget_all(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        ok(json!({ "removed": query::forget(&c, &u, None).map_err(bad)? }))
    })
    .await
}

// ------------------------------------------------------------------ users and logs

// ------------------------------------------------------------------ connected databases

/// The connected databases (admins): no secrets, the password file's name only.
async fn connections_list(State(app): State<Shared>, Auth(u): Auth) -> ApiResult {
    admin(&u)?;
    blocking(move || ok(json!({ "items": crate::sources::shown(&*app.db.get().map_err(internal)?).map_err(bad)? })))
        .await
}

/// Whether a connected database answers: each view, its version, how long it took (admins).
async fn connection_check(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let s = crate::sources::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        match s.check() {
            Ok(ck) => ok(json!({ "ok": true, "views": ck.views, "version": ck.version, "ms": ck.ms })),
            Err(e) => ok(json!({ "ok": false, "error": e.to_string() })),
        }
    })
    .await
}

#[derive(Deserialize)]
struct NewConnection {
    name: String,
    kind: String,
    url: String,
    /// Kept in `<home>/secrets/<name>.password` (0600), never in the database nor sent back.
    #[serde(default)]
    password: String,
    #[serde(default)]
    settings: crate::sources::Settings,
}

/// Connect a database from the page (admins): checked first, as `medorama connections add`.
async fn connection_add(State(app): State<Shared>, Auth(u): Auth, Json(b): Json<NewConnection>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let name = b.name.trim().to_string();
        if !crate::sources::valid_name(&name) {
            return Err(bad("a connection's name: letters, digits, - and _, starting with a letter (criteria write it: name.lactate)"));
        }
        // a database file on this server: inside the folders data may come from, never the catalog
        let url = crate::sources::checked_url(&app.cfg, &b.url).map_err(bad)?;
        let mut settings = b.settings.clone();
        if settings.timezone.trim().is_empty() {
            settings.timezone = app.cfg.timezone.clone();
        }
        let n = crate::sources::NewSource {
            name: name.clone(),
            kind: b.kind.trim().to_string(),
            url,
            password_file: String::new(),
            ca_file: String::new(),
            settings,
        };
        let c = app.db.get().map_err(internal)?;
        // the password is written only once the connection is checked: a name already used, or
        // a database that does not answer, leaves every file as it was
        let id = crate::sources::add_with_password(&c, &app.cfg, &u, &n, &b.password).map_err(bad)?;
        let s = crate::sources::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        let ck = s.check().map_err(bad)?;
        ok(json!({ "id": id, "name": s.name, "views": ck.views, "version": ck.version }))
    })
    .await
}

/// Link every person of a connected database to the catalog's patients (admins).
async fn connection_sync(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let c = app.db.get().map_err(internal)?;
        let s = crate::sources::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        let (persons, linked) = crate::sources::link::sync(&c, &s, s.zone(&app.cfg.timezone), &u).map_err(bad)?;
        ok(json!({ "persons": persons, "linked": linked }))
    })
    .await
}

/// Disconnect a database (admins): its links and what it added go; its password file too.
async fn connection_remove(State(app): State<Shared>, Auth(u): Auth, UrlPath(id): UrlPath<i64>) -> ApiResult {
    admin(&u)?;
    blocking(move || {
        let mut c = app.db.get().map_err(internal)?;
        crate::sources::get(&c, id).map_err(internal)?.ok_or_else(not_found)?;
        // with its pools and the password file the page made (sources::removed)
        crate::sources::remove(&mut c, &app.cfg, &u, id).map_err(bad)?;
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
        let ch = Changes {
            display_name: req.display_name.as_deref(),
            role,
            phi: req.phi,
            disabled: req.disabled,
            password: req.password.as_deref(),
        };
        users::update(&mut c, id, &ch).map_err(bad)?;
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
    if ["/api/v1/me", "/api/v1/login", "/api/v1/logout", "/api/v1/privacy", "/api/v1/sign-in"].contains(&path.as_str())
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
    let Ok(Ok(g)) = g else {
        return internal("the privacy gate's settings could not be read").into_response();
    };
    // a patient's own pages: their dates move by their own offset, whatever the objects name
    match patient_of_path(&path) {
        Some(p) => g.for_patient(p).json(&mut v),
        None => g.json(&mut v),
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
        // the viewer's page, shown in the catalog's VitalStream tab: its own scripts and styles
        // only (it has no inline ones; its icon is a data: picture), framing by the catalog only
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
        // the ECG, echo and imaging viewers: shown in the catalog's tabs; they read the patients'
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
        // a response with a policy of its own (patients' files): shown inside the catalog's page
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
        .route("/encounters", get(encounters))
        .route("/encounters/remove", post(remove_encounters))
        .route("/encounters/remove-matching", post(remove_matching_encounters))
        .route("/encounters/{id}", get(encounter))
        .route("/encounters/{id}/ehr", get(encounter_ehr))
        .route("/recordings", get(recordings_list))
        .route("/recordings/remove", post(remove_recordings))
        .route("/recordings/remove-matching", post(remove_matching_recordings))
        .route("/recordings/{id}", get(recording))
        .route("/recordings/{id}/link", post(link_recording))
        .route("/recordings/{id}/decision", post(link_decision))
        .route("/review", get(review))
        .route("/review/held/{id}", post(decide_held))
        .route("/review/merge", post(merge_patients))
        .route("/review/distinct", post(distinct_patients))
        .route("/check", post(check))
        .route("/actions", get(actions))
        .route("/actions/reload", post(reload_actions))
        .route("/runs", post(create_run))
        .route("/jobs", get(jobs_list))
        .route("/jobs/cancel", post(cancel))
        .route("/history", get(history).delete(forget_all))
        .route("/history/{id}", delete(forget_one))
        .route("/databases", get(databases))
        .route("/databases/{id}/rows", get(database_rows))
        .route("/databases/{id}/export", get(database_export))
        .route("/version", get(data_version))
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
        .route("/connections", get(connections_list).post(connection_add))
        .route("/connections/{id}/check", post(connection_check))
        .route("/connections/{id}/sync", post(connection_sync))
        .route("/connections/{id}", axum::routing::delete(connection_remove))
        .route("/users", get(users_list).post(user_add))
        .route("/users/{id}", axum::routing::patch(user_change).delete(user_delete))
        .route("/audit", get(audit_list))
        .route("/audit/verify", get(audit_verify))
        .route("/imports", get(batches))
        .route("/import-tasks", get(import_tasks))
        .route("/import-tasks/{id}", get(import_task))
        .route("/import-tasks/{id}/cancel", post(import_task_cancel))
        .route("/imports/{id}/remove", post(remove_batch))
        .route("/sources", get(sources))
        .route("/sources/remove", post(remove_source))
        .route("/demo/removal", get(demo_removal))
        .route("/tables/inspect", post(inspect_table).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/tables", post(import_table).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/tables/files", post(import_table_files).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/formats", get(formats_list))
        .route("/admin/addons", get(addons_list))
        .route("/tables/clif", post(import_clif))
        .route("/tables/server", post(import_server_tables))
        .route("/tables/server/inspect", post(inspect_server_table))
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
        .route("/mappings", get(mappings).post(save_mapping))
        .route("/mappings/{name}", delete(delete_mapping))
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

/// Background threads: `workers` job workers, and a scheduler that queues automatic indexing
/// and re-checks the recordings.  They end when `stop` is set.
pub fn start_background(app: &App, stop: Arc<AtomicBool>) -> Vec<std::thread::JoinHandle<()>> {
    let mut threads = vec![];
    for n in 0..app.cfg.workers {
        let (db, cfg, reg, stop) = (app.db.clone(), app.cfg.clone(), app.reg.clone(), stop.clone());
        threads.push(std::thread::spawn(move || jobs::work(db, cfg, reg, n, stop, false)));
    }
    // databases imported before they had their table of numbers (typed.rs): given one, once
    let db = app.db.clone();
    threads.push(std::thread::spawn(move || match db.get().and_then(|mut c| crate::typed::build_missing(&mut c)) {
        Ok(0) => {}
        Ok(n) => eprintln!("medorama: {n} database(s) given their table of numbers (for criteria on their columns)"),
        Err(e) => eprintln!("medorama: the databases' tables of numbers: {e}"),
    }));
    let (db, cfg) = (app.db.clone(), app.cfg.clone());
    threads.push(std::thread::spawn(move || {
        let mut last_check: Option<Instant> = None;
        let mut last_queue: Option<Instant> = None;
        let mut last_seal: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            let interval = Duration::from_secs(cfg.check_interval_s);
            if cfg.check_interval_s > 0 && last_check.is_none_or(|t| t.elapsed() >= interval) {
                if let Err(e) = db.get().and_then(|c| recordings::check(&c, &cfg)) {
                    eprintln!("medorama: check: {e}");
                }
                last_check = Some(Instant::now());
            }
            if last_seal.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                if let Err(e) = db.get().and_then(|c| crate::audit::seal(&c, cfg.audit_export.as_deref())) {
                    eprintln!("medorama: sealing the audit log: {e}");
                }
                last_seal = Some(Instant::now());
            }
            if cfg.auto_index && last_queue.is_none_or(|t| t.elapsed() >= Duration::from_secs(15)) {
                if let Err(e) = db.get().and_then(|c| jobs::queue_auto_index(&c)) {
                    eprintln!("medorama: automatic indexing: {e}");
                }
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
