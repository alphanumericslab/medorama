//! The gateway: the viewer under `/viewer/`, behind the catalog's sign-in and rules
//! (docs/architecture.md, "The gateway"; docs/administration.md, "The waveform viewer").
//!
//! Default deny: only these requests of viewer API v1 pass, each with its rule.
//!
//! | Request | Rule |
//! |---|---|
//! | `GET /`, `GET /static/…` | the page (signed-in users) |
//! | `GET api/v1/config` | passes; for non-admins, `places`, `start` and `home` point into the catalog's recordings |
//! | `GET api/v1/browse`, `GET api/v1/complete` | admins: inside `data_roots` (anywhere when empty); others: only folders that lead to recordings in the catalog, showing only those recordings |
//! | `GET api/v1/scan` | what opening a folder of records (or a record) would read: as `open`; refused in Privacy Mode |
//! | `POST api/v1/open` | admins: inside `data_roots`; others: only recordings in the catalog. Audited |
//! | `POST api/v1/cancel` | only indexes of datasets the user opened through the gateway |
//! | `GET api/v1/datasets/{key}…` | only datasets the user opened through the gateway |
//!
//! Requests that change something (`POST`) must be JSON (`Content-Type: application/json`), as
//! the catalog's own API requires (another site's page cannot send one unasked).  The viewer's
//! own sign-in (`login`, `logout`) and anything else are refused.  Each passed
//! request carries the viewer's sign-in cookie, made by the supervisor; the user's catalog
//! cookie is never passed on.
//!
//! **In Privacy Mode** (docs/security.md, "The privacy gate"), the gateway is the gate's part for
//! the viewer: recordings are opened by their tokens (`mo-rec:<id>`, never a path); browsing
//! and completing folders is refused (folder and file names may identify); the times the page
//! sends (`t0`, `t1`, `t`), which it knows moved back, are moved forward again before the
//! request reaches the viewer; a dataset's path and name are replaced, and markers' times
//! (in plain arrays) moved back; the viewer's own error texts (which name files) are replaced by
//! a fixed sentence, and the records of a folder it could not read are named by number with a
//! fixed reason.  Every answer then passes the gate's last layer like any other (server.rs
//! `privacy`): the times in its JSON move back, identifiers are replaced.

// Refusals are HTTP responses, returned as the error side of Results (as axum does).
#![allow(clippy::result_large_err)]

use crate::db::audit;
use crate::server::{Auth, Shared};
use crate::users::User;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Recordings in the catalog, as real paths, and every folder above them (for browsing).
struct Scope {
    built: Instant,
    recordings: HashSet<PathBuf>,
    folders: HashSet<PathBuf>,
    /// Folders holding recordings, most recordings first (the Open dialog's places).
    places: Vec<PathBuf>,
}

/// Gateway state kept by the server.
pub struct Gateway {
    client: Client<hyper_util::client::legacy::connect::HttpConnector, Body>,
    scope: Mutex<Option<Scope>>,
    /// Dataset keys each user has opened through the gateway.
    opened: Mutex<HashMap<i64, HashSet<String>>>,
    /// The token of the recording each dataset key was opened from (Privacy Mode).
    tokens: Mutex<HashMap<String, String>>,
}

impl Default for Gateway {
    fn default() -> Self {
        Gateway {
            client: Client::builder(TokioExecutor::new()).build_http(),
            scope: Mutex::new(None),
            opened: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        }
    }
}

fn deny(code: StatusCode, msg: &str) -> Response {
    (code, axum::Json(json!({ "detail": msg }))).into_response()
}

/// What a session in Privacy Mode is told when the viewer fails: its own texts name files.
const PRIVATE_ERROR: &str = "the viewer could not do this (its reason is not shown in Privacy Mode: it may name files)";
/// … and why a record of a folder was left out.
const PRIVATE_SKIPPED: &str = "could not be read";

/// An answer of the viewer as a session in Privacy Mode gets it: an error's text replaced;
/// in a dataset, the records it left out (`skipped`: name → reason) named by number, with a
/// fixed reason; any `error` text replaced.
fn private_answer(status: StatusCode, v: &mut Value) {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                if let Some(Value::Object(sk)) = m.get_mut("skipped") {
                    let n = sk.len();
                    *sk =
                        (1..=n).map(|i| (format!("{} {i}", crate::gate::RECORDING), json!(PRIVATE_SKIPPED))).collect();
                }
                for (k, x) in m.iter_mut() {
                    match x {
                        Value::String(t) if k == "error" && !t.is_empty() => *t = PRIVATE_ERROR.into(),
                        _ => walk(x),
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    if !status.is_success() {
        *v = json!({ "detail": PRIVATE_ERROR });
        return;
    }
    walk(v);
}

/// The real path (symbolic links resolved) of a path as the user gave it.
fn real(p: &str) -> PathBuf {
    let abs = recording_engine::paths::absolute(p);
    crate::config::real(Path::new(&abs))
}

impl Gateway {
    fn scope<'a>(&self, app: &Shared, guard: &'a mut Option<Scope>) -> &'a Scope {
        let fresh = guard.as_ref().is_some_and(|s| s.built.elapsed() < Duration::from_secs(10));
        if !fresh {
            let paths: Vec<String> = app
                .db
                .get()
                .ok()
                .and_then(|c| {
                    let mut st = c.prepare("SELECT path FROM recordings WHERE present = 1").ok()?;
                    let v = st.query_map([], |r| r.get::<_, String>(0)).ok()?.filter_map(Result::ok).collect();
                    Some(v)
                })
                .unwrap_or_default();
            let mut recordings = HashSet::new();
            let mut folders = HashSet::new();
            let mut counts: HashMap<PathBuf, usize> = HashMap::new();
            for p in paths {
                let p = PathBuf::from(p);
                if let Some(parent) = p.parent() {
                    *counts.entry(parent.to_path_buf()).or_default() += 1;
                }
                for a in p.ancestors().skip(1) {
                    folders.insert(a.to_path_buf());
                }
                recordings.insert(p);
            }
            let mut places: Vec<(PathBuf, usize)> = counts.into_iter().collect();
            places.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            *guard = Some(Scope {
                built: Instant::now(),
                recordings,
                folders,
                places: places.into_iter().take(20).map(|p| p.0).collect(),
            });
        }
        guard.as_ref().unwrap()
    }

    /// May `user` see this path (a folder to browse, or a recording to open)?
    fn allowed(&self, app: &Shared, user: &User, path: &Path, open: bool) -> bool {
        if user.is_admin() {
            let roots = &app.cfg.data_roots;
            return roots.is_empty()
                || roots.iter().any(|r| {
                    let r = crate::config::real(Path::new(r));
                    path.starts_with(&r)
                });
        }
        let mut g = self.scope.lock().unwrap();
        let s = self.scope(app, &mut g);
        s.recordings.contains(path) || (!open && s.folders.contains(path))
    }

    fn places(&self, app: &Shared) -> Vec<PathBuf> {
        let mut g = self.scope.lock().unwrap();
        self.scope(app, &mut g).places.clone()
    }
}

/// Folders a non-admin may start browsing from: the common ancestor of the recordings'
/// folders, else the first place.
fn common_ancestor(places: &[PathBuf]) -> Option<PathBuf> {
    let first = places.first()?;
    let mut c = first.clone();
    for p in &places[1..] {
        while !p.starts_with(&c) {
            if !c.pop() {
                return None;
            }
        }
    }
    Some(c)
}

/// A request passed to the viewer, and its answer.  `private` (Privacy Mode): an answer that is
/// not a success is replaced by [`PRIVATE_ERROR`].
async fn forward(
    app: &Shared,
    private: bool,
    method: Method,
    rest: &str,
    query: Option<&str>,
    body: Bytes,
    ctype: Option<HeaderValue>,
) -> Result<Response, Response> {
    let unavailable = |e: String| deny(StatusCode::SERVICE_UNAVAILABLE, &format!("the viewer is not available: {e}"));
    let port = app.viewer.ensure().await.map_err(unavailable)?;
    let request = |port: u16| {
        let uri = format!("http://127.0.0.1:{port}/{rest}{}", query.map(|q| format!("?{q}")).unwrap_or_default());
        let mut req = axum::http::Request::builder()
            .method(method.clone())
            .uri(uri)
            .header(header::COOKIE, app.viewer.cookie(port));
        if let Some(ct) = &ctype {
            req = req.header(header::CONTENT_TYPE, ct);
        }
        req.body(Body::from(body.clone())).map_err(|e| deny(StatusCode::BAD_GATEWAY, &e.to_string()))
    };
    let resp = match app.gateway.client.request(request(port)?).await {
        // not reached: checked (and started again if it has gone), then asked once more
        Err(e) if e.is_connect() => {
            let port = app.viewer.unreachable(port).await.map_err(unavailable)?;
            app.gateway.client.request(request(port)?).await
        }
        r => r,
    };
    let resp = resp.map_err(|e| deny(StatusCode::BAD_GATEWAY, &format!("viewer: {e}")))?;
    let (parts, incoming) = resp.into_parts();
    if private && !parts.status.is_success() {
        return Ok(deny(parts.status, PRIVATE_ERROR));
    }
    let mut out = Response::builder().status(parts.status);
    for (k, v) in parts.headers.iter() {
        if matches!(
            k.as_str(),
            "set-cookie"
                | "connection"
                | "keep-alive"
                | "transfer-encoding"
                | "x-frame-options"
                | "content-security-policy"
        ) {
            continue;
        }
        out = out.header(k, v);
    }
    out.body(Body::new(incoming)).map_err(|e| deny(StatusCode::BAD_GATEWAY, &e.to_string()))
}

async fn json_of(resp: Response) -> Result<(StatusCode, Value), Response> {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.map_err(|e| deny(StatusCode::BAD_GATEWAY, &e.to_string()))?.to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Ok((status, v))
}

fn json_response(status: StatusCode, v: &Value) -> Response {
    (status, axum::Json(v.clone())).into_response()
}

/// A query with its times (`t0`, `t1`, `t`) moved forward by `offset` seconds: the page of a
/// session in Privacy Mode asks in the times it was shown.
fn shift_query(q: Option<&str>, offset: i64) -> Option<String> {
    let q = q?;
    let out: Vec<String> = q
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match (decode(k).as_str(), decode(v).parse::<f64>()) {
                ("t0" | "t1" | "t", Ok(t)) => format!("{k}={}", t + offset as f64),
                _ => kv.to_string(),
            }
        })
        .collect();
    Some(out.join("&"))
}

fn query_param(q: Option<&str>, key: &str) -> Option<String> {
    url_params(q?).into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn url_params(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match (b[i], hex) {
            (_, Some(v)) => {
                out.push(v);
                i += 2;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/viewer` (redirect) and `/viewer/…`.
pub async fn handle(State(app): State<Shared>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let path = parts.uri.path().to_string();
    if path == "/viewer" {
        return Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header(header::LOCATION, "viewer/")
            .body(Body::empty())
            .unwrap();
    }
    let rest = path.strip_prefix("/viewer/").unwrap_or("").to_string();
    let query = parts.uri.query().map(str::to_string);
    let is_page = rest.is_empty() || rest.starts_with("static/");
    // the catalog's sign-in
    let mut p2 = parts.clone();
    let user = match <Auth as axum::extract::FromRequestParts<Shared>>::from_request_parts(&mut p2, &app).await {
        Ok(Auth(u)) => u,
        Err(_) if is_page => {
            return Response::builder()
                .status(StatusCode::SEE_OTHER)
                .header(header::LOCATION, "../")
                .body(Body::empty())
                .unwrap()
        }
        Err(e) => return e.into_response(),
    };
    if !app.viewer.available() {
        return deny(StatusCode::NOT_FOUND, "no viewer runs under this catalog");
    }
    // Privacy Mode: the gate's settings and dictionary (recording tokens)
    let gate = if user.privacy {
        let a = app.clone();
        match tokio::task::spawn_blocking(move || {
            let c = a.db.get().map_err(|_| ())?;
            crate::server::restricted(&a, &c).map_err(|_| ())
        })
        .await
        {
            Ok(Ok(g)) => Some(g),
            _ => return deny(StatusCode::INTERNAL_SERVER_ERROR, "the privacy gate's settings could not be read"),
        }
    } else {
        None
    };
    let method = parts.method.clone();
    let ctype = parts.headers.get(header::CONTENT_TYPE).cloned();
    // a request that changes something is JSON, which another site's page cannot send without
    // the browser asking first (and the catalog never allows it)
    let json = ctype.as_ref().and_then(|c| c.to_str().ok()).is_some_and(|c| c.starts_with("application/json"));
    if method != Method::GET && method != Method::HEAD && !json {
        return deny(StatusCode::UNSUPPORTED_MEDIA_TYPE, "JSON expected");
    }
    let body = match axum::body::to_bytes(body, 1 << 20).await {
        Ok(b) => b,
        Err(_) => return deny(StatusCode::PAYLOAD_TOO_LARGE, "request too large"),
    };
    let g = &app.gateway;
    let result: Result<Response, Response> = async {
        match (method.as_str(), rest.as_str()) {
            ("GET", "") => forward(&app, gate.is_some(), method, "", None, Bytes::new(), None).await,
            ("GET", r) if r.starts_with("static/") && !r.contains("..") => {
                forward(&app, gate.is_some(), method, r, None, Bytes::new(), None).await
            }
            ("GET", "api/v1/config") => {
                let resp = forward(&app, gate.is_some(), method, "api/v1/config", None, Bytes::new(), None).await?;
                let (status, mut v) = json_of(resp).await?;
                if gate.is_some() && status.is_success() {
                    // nothing of the file system: recordings open from patients' pages
                    for k in ["places", "data_roots"] {
                        v[k] = json!([]);
                    }
                    v["start"] = json!("");
                    v["home"] = json!("");
                } else if !user.is_admin() && status.is_success() {
                    let places = g.places(&app);
                    let start = common_ancestor(&places).or_else(|| places.first().cloned());
                    let name = |p: &Path| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| p.display().to_string())
                    };
                    v["places"] = Value::Array(
                        places.iter().map(|p| json!({"name": name(p), "path": p.display().to_string()})).collect(),
                    );
                    let s = start.map(|p| p.display().to_string()).unwrap_or_default();
                    v["start"] = json!(s);
                    v["home"] = json!(s);
                    v["data_roots"] = json!([]);
                }
                Ok(json_response(status, &v))
            }
            ("GET", "api/v1/browse" | "api/v1/complete") if gate.is_some() => Err(deny(
                StatusCode::FORBIDDEN,
                "folders are not browsed in Privacy Mode (their names may identify): open recordings from a patient's page",
            )),
            ("GET", "api/v1/browse") => {
                let asked = query_param(query.as_deref(), "path").filter(|p| !p.is_empty());
                if let Some(p) = &asked {
                    if !g.allowed(&app, &user, &real(p), false) {
                        return Err(deny(StatusCode::FORBIDDEN, "not a folder you may browse"));
                    }
                } else if !user.is_admin() {
                    return Err(deny(StatusCode::BAD_REQUEST, "path required"));
                }
                let resp = forward(&app, gate.is_some(), method, "api/v1/browse", query.as_deref(), Bytes::new(), None).await?;
                let (status, mut v) = json_of(resp).await?;
                if status.is_success() {
                    if let Some(entries) = v.get_mut("entries").and_then(Value::as_array_mut) {
                        entries.retain(|e| {
                            let p = e.get("path").and_then(Value::as_str).unwrap_or("");
                            let recording = e.get("type").and_then(Value::as_str) != Some("dir");
                            g.allowed(&app, &user, &real(p), recording)
                        });
                    }
                    if let Some(parent) = v.get("parent").and_then(Value::as_str).map(str::to_string) {
                        if !g.allowed(&app, &user, &real(&parent), false) {
                            v["parent"] = Value::Null;
                        }
                    }
                    if v.get("type").and_then(Value::as_str) == Some("file")
                        && !g.allowed(&app, &user, &real(asked.as_deref().unwrap_or("")), true)
                    {
                        return Err(deny(StatusCode::FORBIDDEN, "not a recording you may open"));
                    }
                }
                Ok(json_response(status, &v))
            }
            ("GET", "api/v1/complete") => {
                let resp = forward(&app, gate.is_some(), method, "api/v1/complete", query.as_deref(), Bytes::new(), None).await?;
                let (status, mut v) = json_of(resp).await?;
                if let Some(items) = v.get_mut("items").and_then(Value::as_array_mut) {
                    items.retain(|e| {
                        let p = e.get("path").and_then(Value::as_str).unwrap_or("");
                        let p = p.trim_end_matches(['/', '\\']);
                        let recording = e.get("type").and_then(Value::as_str) != Some("dir");
                        g.allowed(&app, &user, &real(p), recording)
                    });
                }
                Ok(json_response(status, &v))
            }
            ("GET", "api/v1/scan") if gate.is_some() => Err(deny(
                StatusCode::FORBIDDEN,
                "folders are not scanned in Privacy Mode (their names may identify): open recordings from a patient's page",
            )),
            ("GET", "api/v1/scan") => {
                let asked = query_param(query.as_deref(), "path").unwrap_or_default();
                if !g.allowed(&app, &user, &real(&asked), true) {
                    return Err(deny(StatusCode::FORBIDDEN, "not a recording you may open"));
                }
                forward(&app, gate.is_some(), method, "api/v1/scan", query.as_deref(), Bytes::new(), None).await
            }
            ("POST", "api/v1/cancel") => {
                let req: Value = serde_json::from_slice(&body)
                    .map_err(|e| deny(StatusCode::BAD_REQUEST, &format!("bad request body: {e}")))?;
                let key = req.get("key").and_then(Value::as_str).unwrap_or("");
                let ok = g.opened.lock().unwrap().get(&user.id).is_some_and(|s| s.contains(key));
                if !ok {
                    return Err(deny(StatusCode::FORBIDDEN, "open the recording first"));
                }
                forward(&app, gate.is_some(), method, "api/v1/cancel", None, body.clone(), ctype.clone()).await
            }
            ("POST", "api/v1/open") => {
                let req: Value = serde_json::from_slice(&body)
                    .map_err(|e| deny(StatusCode::BAD_REQUEST, &format!("bad request body: {e}")))?;
                let mut req = req;
                let asked = req.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                // in Privacy Mode, a recording's token, never a path
                let p = match &gate {
                    Some(g) => match crate::gate::recording_path(&g.dict, &asked) {
                        Some(path) => path,
                        None => return Err(deny(StatusCode::FORBIDDEN, "in Privacy Mode a recording is opened from a patient's page")),
                    },
                    None => asked.clone(),
                };
                req["path"] = json!(p);
                let body = Bytes::from(serde_json::to_vec(&req).unwrap_or_default());
                let rp = real(&p);
                if !g.allowed(&app, &user, &rp, true) {
                    return Err(deny(StatusCode::FORBIDDEN, "not a recording you may open"));
                }
                let resp = forward(&app, gate.is_some(), method, "api/v1/open", None, body.clone(), ctype.clone()).await?;
                let (status, mut v) = json_of(resp).await?;
                if gate.is_some() {
                    private_answer(status, &mut v);
                }
                if status.is_success() {
                    if let Some(key) = v.get("key").and_then(Value::as_str) {
                        g.opened.lock().unwrap().entry(user.id).or_default().insert(key.to_string());
                        if gate.is_some() {
                            g.tokens.lock().unwrap().insert(key.to_string(), asked.clone());
                        }
                    }
                    if let Ok(c) = app.db.get() {
                        audit(&c, Some(&user), "view", &rp.to_string_lossy(), "");
                    }
                }
                Ok(json_response(status, &v))
            }
            ("GET", r) if r.starts_with("api/v1/datasets/") => {
                let key = r["api/v1/datasets/".len()..].split('/').next().unwrap_or("");
                let ok = g.opened.lock().unwrap().get(&user.id).is_some_and(|s| s.contains(key));
                if !ok {
                    return Err(deny(StatusCode::FORBIDDEN, "open the recording first"));
                }
                let Some(gt) = &gate else {
                    return forward(&app, gate.is_some(), method, r, query.as_deref(), Bytes::new(), None).await;
                };
                // the recording's times move by its patient's own offset
                let token = g.tokens.lock().unwrap().get(key).cloned().unwrap_or_default();
                let gt = &gt.for_recording(&token);
                let offset = if gt.settings.hides("dates") { gt.settings.offset_seconds() } else { 0 };
                let q = shift_query(query.as_deref(), offset);
                let resp = forward(&app, gate.is_some(), method, r, q.as_deref(), Bytes::new(), None).await?;
                let (status, mut v) = json_of(resp).await?;
                if status.is_success() {
                    let token = g.tokens.lock().unwrap().get(key).cloned().unwrap_or_default();
                    if let Some(d) = v.get_mut("dataset") {
                        d["path"] = json!(token);
                        d["name"] = json!(crate::gate::RECORDING);
                        // [first, last] times: a pair the answer's walk would not see as times
                        if let Some(Value::Array(t)) = d.get_mut("t_range") {
                            for x in t.iter_mut() {
                                if let Some(f) = x.as_f64() {
                                    *x = json!(gt.settings.shift(f));
                                }
                            }
                        }
                    }
                    // the recording's free-text events (EDF+ annotations), as its patient's notes
                    // are: withheld when Privacy Mode hides notes, else without the patient's own
                    // names and date of birth (the last layer replaces identifiers, moves dates)
                    if let Some(d) = v.get_mut("dataset") {
                        let (names, birth) = recording_patient_words(&app, &token);
                        gate_annotations(d, gt.settings.hides("notes"), &names, &birth);
                    }
                    private_answer(status, &mut v);
                    // markers: [channel, time, name index]
                    if r.ends_with("/markers") {
                        if let Some(Value::Array(items)) = v.get_mut("items") {
                            for it in items.iter_mut() {
                                if let Some(x) = it.get_mut(1) {
                                    if let Some(f) = x.as_f64() {
                                        *x = json!(gt.settings.shift(f));
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(json_response(status, &v))
            }
            _ => Err(deny(StatusCode::NOT_FOUND, "not available through the catalog")),
        }
    }
    .await;
    result.unwrap_or_else(|r| r)
}

/// A dataset's annotations (`alarms` of kind `annotation`: free text, as EDF+ files hold) in
/// Privacy Mode: withheld (`hide`, when notes are), else with the patient's names and date of
/// birth replaced.
fn gate_annotations(dataset: &mut Value, hide: bool, names: &[String], birth: &str) {
    let Some(Value::Array(events)) = dataset.get_mut("alarms") else { return };
    for e in events.iter_mut().filter(|e| e.get("kind").and_then(Value::as_str) == Some("annotation")) {
        if let Some(t) = e.get_mut("text") {
            if hide {
                *t = json!(crate::gate::ANNOTATION);
            } else {
                crate::gate::scrub_names(t, names, birth);
            }
        }
    }
}

/// The names and date of birth of the patient a recording (its token, `mo-rec:<id>`) belongs to.
fn recording_patient_words(app: &Shared, token: &str) -> (Vec<String>, String) {
    let Some(id) = token.strip_prefix("mo-rec:").and_then(|i| i.parse::<i64>().ok()) else { return Default::default() };
    let Ok(c) = app.db.get() else { return Default::default() };
    let patient: Option<i64> = c
        .query_row(
            "SELECT e.patient FROM recording_links l JOIN encounters e ON e.id = l.encounter WHERE l.recording = ?1 ORDER BY e.id LIMIT 1",
            [id],
            |r| r.get(0),
        )
        .ok();
    patient.map(|p| crate::gate::Restricted::identity_words(&c, p)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_in_privacy_mode() {
        let ds = || {
            json!({"alarms": [
                {"t0": 1.0, "t1": 2.0, "text": "Zyx awake, seen with Quil", "kind": "annotation"},
                {"t0": 3.0, "t1": 3.0, "text": "HR HIGH Zyx", "kind": "alarm"}
            ]})
        };
        let names = vec!["Zyx".to_string(), "Quil".to_string()];
        let mut d = ds();
        gate_annotations(&mut d, false, &names, "");
        assert_eq!(d["alarms"][0]["text"], "[Patient Name] awake, seen with [Patient Name]");
        assert_eq!(d["alarms"][1]["text"], "HR HIGH Zyx", "a device's alarm text is not an annotation");
        let mut d = ds();
        gate_annotations(&mut d, true, &names, "");
        assert_eq!(d["alarms"][0]["text"], crate::gate::ANNOTATION);
    }

    #[test]
    fn helpers() {
        assert_eq!(decode("a%20b+c%2Fd"), "a b c/d");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(query_param(Some("x=1&path=%2Fdata%2Fa"), "path").as_deref(), Some("/data/a"));
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(common_ancestor(&[p("/d/a/x"), p("/d/a/y"), p("/d/b")]), Some(p("/d")));
    }

    #[test]
    fn viewer_texts_in_privacy_mode() {
        let mut v = json!({"dataset": {"skipped": {"Zyx_001": "no such file: /d/Zyx/Zyx_001.dat", "Zyx_002": "bad"},
            "pieces": [{"state": "failed", "error": "/d/Zyx/x.dat: truncated"}]}});
        private_answer(StatusCode::OK, &mut v);
        let t = v.to_string();
        assert!(!t.contains("Zyx"), "{t}");
        assert_eq!(v["dataset"]["skipped"].as_object().unwrap().len(), 2);
        assert_eq!(v["dataset"]["pieces"][0]["error"], PRIVATE_ERROR);
        let mut e = json!({"detail": "no such file or folder: /d/Zyx/rec"});
        private_answer(StatusCode::NOT_FOUND, &mut e);
        assert_eq!(e, json!({"detail": PRIVATE_ERROR}));
    }
}
