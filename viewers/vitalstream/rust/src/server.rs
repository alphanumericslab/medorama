//! HTTP API (version 1, under `api/v1/`; `docs/viewer-api.md`) and the web page.
//!
//! Access: when a password is set, every API request needs the sign-in cookie that `POST
//! login` sets.  The page uses relative URLs, so it also works behind a reverse proxy that
//! serves it under a path prefix.  Only paths allowed by the settings are served.  Patient
//! names and dates of birth are never read by any source.  Every request must name an allowed
//! host (DNS rebinding), and one that changes something must come from an allowed origin with
//! a JSON body (another site's page cannot send one): docs/viewer-api.md, "Requests".

use crate::auth::{self, Auth};
use crate::settings::{absolute, fs_root, home, is_sep, repo_root, Settings, SEP};
use axum::body::Body;
use axum::extract::{Path as UrlPath, RawQuery, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use indexmap::IndexMap;
use recording_engine::dataset::Dataset;
use recording_engine::index::{build_index_until, cache_key, load_index};
use recording_engine::sources::wfdb_folder;
use recording_engine::sources::{kind_of, unreadable};
use signal_core::dict;
use signal_core::value::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_LOADED: usize = 4;
const MAX_ENTRIES: usize = 5000;
const MAX_COMPLETIONS: usize = 50;
/// The most channels of one `window` or `markers` request, and series of one `vitals` request.
pub const MAX_CHANNELS: usize = 64;
pub const MAX_SERIES: usize = 512;
/// The largest request bodies: `login` (a password), and the API's (`open`, `cancel`).
pub const MAX_LOGIN_BODY: usize = 4 * 1024;
pub const MAX_API_BODY: usize = 64 * 1024;
/// Wrong passwords: the delay before the next attempt from the same address is checked
/// doubles with each (1, 3, 7, 15 s, at most 30 s), for [`FAILURES_KEPT`].
const MAX_LOGIN_DELAY_S: u64 = 30;
const FAILURES_KEPT: Duration = Duration::from_secs(15 * 60);
/// The Content-Security-Policy of the page and the sign-in page.
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
object-src 'none'; base-uri 'none'; frame-ancestors 'self'; form-action 'self'";

pub const LOGIN_PAGE: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>VitalStream</title>
<link rel="stylesheet" href="static/login.css"></head><body>
<form method="post" action="login"><h1>VitalStream</h1>{error}
<input type="password" name="password" placeholder="Password" autofocus autocomplete="current-password">
<button>Sign in</button></form></body></html>"#;

enum Job {
    Indexing { fraction: Arc<Mutex<f64>>, started: Instant, cancel: Arc<AtomicBool> },
    Error(String),
    Cancelled,
    Done,
}

pub struct App {
    pub settings: Settings,
    protected: bool,
    creds: Option<Auth>,
    cookie: String,
    jobs: Mutex<HashMap<String, Job>>,
    loaded: Mutex<IndexMap<String, Arc<Dataset>>>,
    opened: Mutex<HashSet<String>>,
    workers: Arc<tokio::sync::Semaphore>,
    /// one password checked at a time (scrypt takes 16 MiB and tens of milliseconds)
    checking: tokio::sync::Semaphore,
    /// wrong passwords by client address: how many, and the last
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    /// sign-ins ended by `logout` (the cookie's signature, and when it would have expired)
    revoked: Mutex<HashMap<String, u64>>,
}

type Shared = Arc<App>;

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        json_response(self.0, &Value::Dict(dict! { "detail" => self.1 }))
    }
}

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(code, msg.into())
}

fn json_response(code: StatusCode, v: &Value) -> Response {
    (code, [(header::CONTENT_TYPE, "application/json")], recording_engine::common::clean(v.clone()).to_json())
        .into_response()
}

fn ok(v: Value) -> Result<Response, ApiError> {
    Ok(json_response(StatusCode::OK, &v))
}

/// The query parameters (`application/x-www-form-urlencoded`); a parameter given twice is a
/// bad request.
struct Q(HashMap<String, String>);

impl Q {
    fn get(&self, k: &str) -> Option<&String> {
        self.0.get(k)
    }
}

fn query(raw: Option<String>) -> Result<Q, ApiError> {
    let mut out = HashMap::new();
    for kv in raw.as_deref().unwrap_or("").split('&').filter(|kv| !kv.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let k = url_decode(k);
        if out.insert(k.clone(), url_decode(v)).is_some() {
            return Err(err(StatusCode::BAD_REQUEST, format!("query parameter {k} is given more than once")));
        }
    }
    Ok(Q(out))
}

/// A decimal number as the API takes it: `[+-]digits[.digits][e[+-]digits]` (or `.digits`),
/// nothing around it.
fn decimal(v: &str) -> bool {
    let b = v.strip_prefix(['+', '-']).unwrap_or(v).as_bytes();
    let (mant, exp) = match b.iter().position(|&c| c == b'e' || c == b'E') {
        Some(i) => (&b[..i], Some(&b[i + 1..])),
        None => (b, None),
    };
    let digits = |x: &[u8]| !x.is_empty() && x.iter().all(u8::is_ascii_digit);
    let mant_ok = match mant.iter().position(|&c| c == b'.') {
        Some(i) => {
            let (int, frac) = (&mant[..i], &mant[i + 1..]);
            (digits(int) || int.is_empty()) && (digits(frac) || frac.is_empty()) && !(int.is_empty() && frac.is_empty())
        }
        None => digits(mant),
    };
    let exp_ok = exp.is_none_or(|e| digits(e.strip_prefix(b"+").or_else(|| e.strip_prefix(b"-")).unwrap_or(e)));
    mant_ok && exp_ok
}

/// A required time (or other number): a finite decimal number.
fn q_f64(q: &Q, k: &str) -> Result<f64, ApiError> {
    let v = q.get(k).ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("missing query parameter {k}")))?;
    v.parse::<f64>()
        .ok()
        .filter(|x| decimal(v) && x.is_finite())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("{k} must be a finite number")))
}

/// `t0` and `t1`: finite, `t0 < t1`.
fn q_span(q: &Q) -> Result<(f64, f64), ApiError> {
    let (t0, t1) = (q_f64(q, "t0")?, q_f64(q, "t1")?);
    if t1 <= t0 {
        return Err(err(StatusCode::BAD_REQUEST, "t1 must be after t0"));
    }
    Ok((t0, t1))
}

fn q_i64(q: &Q, k: &str, default: i64) -> Result<i64, ApiError> {
    match q.get(k) {
        None => Ok(default),
        Some(v) => v
            .parse::<i64>()
            .ok()
            .filter(|_| v.strip_prefix(['+', '-']).unwrap_or(v).bytes().all(|c| c.is_ascii_digit()))
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, format!("{k} must be an integer"))),
    }
}

/// The channel ids of `ch` (a comma-separated list), each once, in order: ids of channels of the
/// dataset (`0 <= id < channels`), at most [`MAX_CHANNELS`].
fn q_channels(q: &Q, channels: usize) -> Result<Vec<i64>, ApiError> {
    let mut out: Vec<i64> = vec![];
    for c in q.get("ch").map(String::as_str).unwrap_or("").split(',').filter(|c| !c.is_empty()) {
        let id = c
            .parse::<i64>()
            .ok()
            .filter(|_| c.bytes().all(|x| x.is_ascii_digit()))
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "ch must be a comma-separated list of channel ids"))?;
        if id as u64 >= channels as u64 {
            return Err(err(StatusCode::BAD_REQUEST, format!("no channel {id} in this dataset")));
        }
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > MAX_CHANNELS {
        return Err(err(StatusCode::BAD_REQUEST, format!("at most {MAX_CHANNELS} channels at once")));
    }
    Ok(out)
}

fn q_bool(q: &Q, k: &str) -> bool {
    matches!(q.get(k).map(|s| s.to_ascii_lowercase()).as_deref(), Some("true" | "1" | "yes" | "on"))
}

impl App {
    pub fn new(settings: Settings, protected: Option<bool>) -> Result<App, String> {
        let creds = auth::load(&settings.auth_file);
        let settings_auth_file = settings.auth_file.clone();
        let protected = protected.unwrap_or(creds.is_some());
        if protected && creds.is_none() {
            return Err("password protection requested but no password is set".into());
        }
        Ok(App {
            cookie: format!("vitalstream_{}", settings.port),
            workers: Arc::new(tokio::sync::Semaphore::new(settings.index_workers)),
            settings,
            protected,
            creds,
            jobs: Mutex::new(HashMap::new()),
            loaded: Mutex::new(IndexMap::new()),
            opened: Mutex::new(HashSet::new()),
            checking: tokio::sync::Semaphore::new(1),
            failures: Mutex::new(HashMap::new()),
            revoked: Mutex::new(auth::revoked(&settings_auth_file)),
        })
    }

    pub fn protected(&self) -> bool {
        self.protected
    }

    /// The value of this server's sign-in cookie in a request, if any.
    fn cookie_of(&self, headers: &HeaderMap) -> Option<String> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|kv| kv.trim().split_once('='))
            .find(|(k, _)| *k == self.cookie)
            .map(|(_, v)| v.to_string())
    }

    fn logged_in(&self, headers: &HeaderMap) -> bool {
        if !self.protected {
            return true;
        }
        let value = self.cookie_of(headers);
        let signature = value.as_deref().and_then(|v| v.split_once('.')).map(|(_, s)| s);
        if signature.is_some_and(|s| self.revoked.lock().unwrap().contains_key(s)) {
            return false;
        }
        self.creds.as_ref().is_some_and(|c| auth::cookie_ok(c, value.as_deref()))
    }

    /// Whether a request names a host this server answers for (DNS rebinding:
    /// docs/viewer-api.md, "Requests"): `localhost`, `127.0.0.1`, `::1`, the host it listens on,
    /// or one of `allowed_hosts` (`*`: any), with any port.
    fn host_allowed(&self, host: &str) -> bool {
        let name = host_name(host);
        if name.is_empty() {
            return false;
        }
        let configured = host_name(&self.settings.host);
        ["localhost", "127.0.0.1", "::1"].contains(&name.as_str())
            || (name == configured && !["0.0.0.0", "::", ""].contains(&configured.as_str()))
            || self.settings.allowed_hosts.iter().any(|h| h == "*" || host_name(h) == name)
    }

    /// Absolute path as given (links not resolved), if allowed and existing.
    fn check_path(&self, path: &str) -> Result<String, ApiError> {
        let p = absolute(path);
        if !self.settings.allowed(&p) {
            return Err(err(StatusCode::FORBIDDEN, "path outside the configured data roots"));
        }
        if !Path::new(&p).exists() {
            return Err(err(StatusCode::NOT_FOUND, format!("no such file or folder: {p}")));
        }
        Ok(p)
    }

    /// 403 unless every file a WFDB record reads (its header, the signal files it names, its
    /// segments, its `.mrk` file) leads inside the data roots, links resolved.
    fn check_record(&self, hea: &Path) -> Result<(), ApiError> {
        let base = hea.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = hea.file_stem().unwrap_or_default().to_string_lossy().to_string();
        for f in wfdb_folder::files_read(&base, &name) {
            if f.symlink_metadata().is_ok() && !self.settings.allowed(&f.to_string_lossy()) {
                let shown = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                return Err(err(
                    StatusCode::FORBIDDEN,
                    format!("{shown}, a file of this record, lies outside the data roots"),
                ));
            }
        }
        Ok(())
    }

    fn dataset(&self, key: &str) -> Result<Option<Arc<Dataset>>, ApiError> {
        if let Some(ds) = self.loaded.lock().unwrap().get(key).cloned() {
            return Ok(Some(ds));
        }
        let Some((meta, arrays)) = load_index(&self.settings.cache_dir, key) else { return Ok(None) };
        let ds = Arc::new(Dataset::new(meta, arrays));
        let path = ds.meta.get("path").and_then(Value::as_str).unwrap_or("").to_string();
        if !self.opened.lock().unwrap().contains(key) && !self.settings.allowed(&path) {
            return Err(err(StatusCode::FORBIDDEN, "path outside the configured data roots"));
        }
        let mut l = self.loaded.lock().unwrap();
        l.insert(key.to_string(), ds.clone());
        while l.len() > MAX_LOADED {
            l.shift_remove_index(0);
        }
        Ok(Some(ds))
    }

    fn need(&self, key: &str) -> Result<Arc<Dataset>, ApiError> {
        self.dataset(key)?.ok_or_else(|| err(StatusCode::NOT_FOUND, "dataset not indexed"))
    }

    fn start_folder(&self) -> String {
        let h = home();
        if self.settings.allowed(&h) && Path::new(&h).is_dir() {
            h
        } else {
            self.settings.data_roots[0].clone()
        }
    }
}

fn entry(dir: &str, name: &str) -> Option<Value> {
    let full = Path::new(dir).join(name);
    let fs = full.to_string_lossy().to_string();
    let link = full.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if link && !full.exists() {
        return Some(Value::Dict(dict! { "name" => name, "path" => fs, "type" => "broken", "link" => true }));
    }
    if wfdb_folder::is_record(&full) {
        let record = name.strip_suffix(".hea").unwrap_or(name);
        return Some(Value::Dict(dict! {
            "name" => record, "path" => fs, "type" => "record", "kind" => "wfdb",
            "label" => wfdb_folder::format_label(&full), "link" => link,
        }));
    }
    match kind_of(&full).as_deref() {
        Some("wfdb") => Some(Value::Dict(dict! { "name" => name, "path" => fs, "type" => "wfdb", "link" => link })),
        Some(kind) => Some(Value::Dict(dict! {
            "name" => name, "path" => fs, "type" => "file", "kind" => kind, "label" => format_label(&full),
            "size" => full.metadata().map_or(0, |m| m.len()), "link" => link,
        })),
        None if full.is_dir() => {
            Some(Value::Dict(dict! { "name" => name, "path" => fs, "type" => "dir", "link" => link }))
        }
        None => None,
    }
}

/// The label of the format a file is read as (`EDF+`; an adapter's, from its `describe`).
fn format_label(p: &Path) -> String {
    recording_engine::sources::format_label(p)
}

fn sorted_names(dir: &str) -> std::io::Result<Vec<String>> {
    let mut names: Vec<String> =
        std::fs::read_dir(dir)?.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names)
}

/// The name of a `Host` header or an origin's host, lower case, without the port and the
/// brackets of an IPv6 address (`[::1]:8060` -> `::1`).
fn host_name(host: &str) -> String {
    let h = host.trim().to_ascii_lowercase();
    if let Some(rest) = h.strip_prefix('[') {
        return rest.split_once(']').map_or(String::new(), |(a, _)| a.to_string());
    }
    // a bare IPv6 address (the configured host): every colon is part of it
    if h.matches(':').count() > 1 {
        return h;
    }
    h.split(':').next().unwrap_or("").to_string()
}

/// Run `f` on a worker thread; a panic is an internal error (500), not a lost request.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))
}

/// A request body of at most `limit` bytes (413 beyond).
async fn body_of(req: Request, limit: usize) -> Result<axum::body::Bytes, ApiError> {
    axum::body::to_bytes(req.into_body(), limit)
        .await
        .map_err(|_| err(StatusCode::PAYLOAD_TOO_LARGE, format!("the request body is larger than {limit} bytes")))
}

async fn config(State(app): State<Shared>) -> Result<Response, ApiError> {
    let h = home();
    let mut places = vec![];
    if app.settings.allowed(&h) {
        places.push(Value::Dict(dict! { "name" => "Home", "path" => h.clone() }));
    }
    if let Some(r) = repo_root() {
        let sd = r.join("sample_data").to_string_lossy().to_string();
        if app.settings.allowed(&sd) {
            places.push(Value::Dict(dict! { "name" => "sample_data", "path" => sd }));
        }
    }
    for r in &app.settings.data_roots {
        if *r != h && *r != fs_root() {
            places.push(Value::Dict(dict! { "name" => r.clone(), "path" => r.clone() }));
        }
    }
    ok(Value::Dict(dict! {
        "timezone" => app.settings.timezone.clone(), "data_roots" => app.settings.data_roots.clone(),
        "home" => h, "start" => app.start_folder(), "places" => Value::List(places), "sep" => SEP,
        "version" => env!("CARGO_PKG_VERSION"), "qc_verbose" => app.settings.qc_verbose,
        "limits" => Value::Dict(dict! {
            "max_records" => app.settings.limits.max_records, "max_depth" => app.settings.limits.max_depth,
            "max_scan_s" => app.settings.limits.max_scan_s,
        }),
    }))
}

async fn browse(State(app): State<Shared>, RawQuery(raw): RawQuery) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let path = q.get("path").filter(|p| !p.is_empty()).cloned().unwrap_or_else(|| app.start_folder());
    let p = app.check_path(&path)?;
    let hidden = q_bool(&q, "hidden");
    let app2 = app.clone();
    blocking(move || {
        let k = kind_of(Path::new(&p));
        // a recording file (a WFDB record's .hea, or a file an adapter reads)
        if k.is_some() && !Path::new(&p).is_dir() {
            return ok(Value::Dict(dict! { "path" => p, "type" => "file" }));
        }
        if !Path::new(&p).is_dir() {
            let why = wfdb_folder::signal_file_note(Path::new(&p))
                .unwrap_or_else(|| format!("not a folder or a recording this viewer reads: {p}"));
            return Err(err(StatusCode::BAD_REQUEST, why));
        }
        let names = sorted_names(&p).map_err(|_| err(StatusCode::FORBIDDEN, format!("permission denied: {p}")))?;
        // the segments and layout headers of the multi-segment records here: part of those
        // records, listed only with the hidden entries
        let mut segments: HashSet<String> = HashSet::new();
        for n in names.iter().filter(|n| n.ends_with(".hea")) {
            let stem = &n[..n.len() - 4];
            for (seg, _) in wfdb_folder::segment_table(&Path::new(&p).join(stem)).unwrap_or_default() {
                segments.insert(format!("{seg}.hea"));
            }
        }
        let (mut entries, mut truncated) = (vec![], false);
        for name in names {
            if (name.starts_with('.') || segments.contains(&name)) && !hidden {
                continue;
            }
            // a symbolic link that leads outside the data roots is not listed
            if !app2.settings.allowed(&Path::new(&p).join(&name).to_string_lossy()) {
                continue;
            }
            if let Some(e) = entry(&p, &name) {
                entries.push(e);
                if entries.len() >= MAX_ENTRIES {
                    truncated = true;
                    break;
                }
            }
        }
        entries.sort_by_key(|e| {
            (
                matches!(e.get("type").and_then(Value::as_str), Some("file" | "record")),
                e.get("name").map(Value::py_str).unwrap_or_default().to_lowercase(),
            )
        });
        let parent =
            Path::new(&p).parent().map(|x| x.to_string_lossy().to_string()).filter(|x| *x != p && !x.is_empty());
        let parent = parent.filter(|x| app2.settings.allowed(x));
        ok(Value::Dict(dict! {
            "path" => p.clone(), "type" => "dir", "kind" => k, "entries" => Value::List(entries),
            "truncated" => truncated, "parent" => parent,
        }))
    })
    .await?
}

async fn complete(State(app): State<Shared>, RawQuery(raw): RawQuery) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let prefix = q.get("prefix").cloned().unwrap_or_default();
    let text =
        if prefix.is_empty() { format!("{}{SEP}", app.start_folder()) } else { crate::settings::expand(&prefix) };
    let (folder, part) = if text.ends_with(is_sep) {
        (text.clone(), String::new())
    } else {
        match text.rfind(is_sep) {
            Some(i) => (text[..=i].to_string(), text[i + 1..].to_string()),
            None => (String::new(), text.clone()),
        }
    };
    let folder = if folder.is_empty() { fs_root() } else { absolute(&folder) };
    blocking(move || {
        if !app.settings.allowed(&folder) || !Path::new(&folder).is_dir() {
            return ok(Value::Dict(dict! { "items" => Value::List(vec![]) }));
        }
        let Ok(names) = sorted_names(&folder) else { return ok(Value::Dict(dict! { "items" => Value::List(vec![]) })) };
        let mut out = vec![];
        for name in names {
            if !name.to_lowercase().starts_with(&part.to_lowercase())
                || (name.starts_with('.') && !part.starts_with('.'))
            {
                continue;
            }
            if !app.settings.allowed(&Path::new(&folder).join(&name).to_string_lossy()) {
                continue;
            }
            if let Some(e) = entry(&folder, &name) {
                let t = e.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                if t == "broken" {
                    continue;
                }
                let mut p = e.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                if t == "dir" {
                    p.push_str(SEP);
                }
                out.push(Value::Dict(dict! { "path" => p, "type" => t }));
                if out.len() >= MAX_COMPLETIONS {
                    break;
                }
            }
        }
        ok(Value::Dict(dict! { "items" => Value::List(out) }))
    })
    .await?
}

/// What opening a folder of records (or one record) would read, from the headers alone, within
/// the limits (docs/viewer-api.md, `scan`).
async fn scan(State(app): State<Shared>, RawQuery(raw): RawQuery) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let path = q.get("path").cloned().unwrap_or_default();
    let p = app.check_path(&path)?;
    let path = Path::new(&p).to_path_buf();
    if !path.is_dir() && !wfdb_folder::is_record(&path) {
        return Err(err(StatusCode::BAD_REQUEST, format!("not a folder or a WFDB record: {p}")));
    }
    let limits = app.settings.limits;
    match blocking(move || wfdb_folder::scan(&path, &limits)).await? {
        Ok(d) => ok(Value::Dict(d)),
        Err(e) => Err(err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string())),
    }
}

#[derive(serde::Deserialize)]
struct OpenRequest {
    path: String,
    #[serde(default)]
    reindex: bool,
}

/// A request body: a JSON object of the request's fields (an array of them is refused, as the
/// other viewers refuse it; docs/viewer-api.md).
fn object_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    let bad = |e: String| err(StatusCode::BAD_REQUEST, format!("bad request body: {e}"));
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| bad(e.to_string()))?;
    if !v.is_object() {
        return Err(bad("expected a JSON object".into()));
    }
    serde_json::from_value(v).map_err(|e| bad(e.to_string()))
}

async fn open(State(app): State<Shared>, req: Request) -> Result<Response, ApiError> {
    let body = body_of(req, MAX_API_BODY).await?;
    let req: OpenRequest = object_body(&body)?;
    let p = app.check_path(&req.path)?;
    let path = Path::new(&p).to_path_buf();
    if !path.is_dir() && kind_of(&path).is_none() {
        let why =
            wfdb_folder::signal_file_note(&path).unwrap_or_else(|| format!("not a recording this viewer reads: {p}"));
        return Err(err(StatusCode::BAD_REQUEST, why));
    }
    // a WFDB record or folder in a build without WFDB support
    if let Some(why) = unreadable(&path) {
        return Err(err(StatusCode::UNPROCESSABLE_ENTITY, why));
    }
    // one record: every file it reads lies inside the data roots (its signal files may be links)
    if wfdb_folder::is_record(&path) {
        let (a, hea) = (app.clone(), path.clone());
        blocking(move || a.check_record(&hea)).await??;
    }
    // a folder: within the limits on the records it holds, before anything is read
    if path.is_dir() && !path.join("index.csv").is_file() {
        let limits = app.settings.limits;
        if let Err(e) = blocking(move || wfdb_folder::scan(&path, &limits)).await? {
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()));
        }
    }
    let key = {
        let p = p.clone();
        blocking(move || cache_key(Path::new(&p))).await?
    };
    app.opened.lock().unwrap().insert(key.clone());
    if !req.reindex {
        let a = app.clone();
        let k = key.clone();
        if blocking(move || a.dataset(&k)).await??.is_some() {
            return ok(Value::Dict(dict! { "key" => key, "state" => "ready" }));
        }
    }
    {
        let mut jobs = app.jobs.lock().unwrap();
        if !matches!(jobs.get(&key), Some(Job::Indexing { .. })) {
            let fraction = Arc::new(Mutex::new(0.0));
            let cancel = Arc::new(AtomicBool::new(false));
            jobs.insert(
                key.clone(),
                Job::Indexing { fraction: fraction.clone(), started: Instant::now(), cancel: cancel.clone() },
            );
            app.loaded.lock().unwrap().shift_remove(&key);
            let (a, k, workers) = (app.clone(), key.clone(), app.workers.clone());
            tokio::spawn(async move {
                let _permit = workers.acquire_owned().await;
                let cache = a.settings.cache_dir.clone();
                let path = p.clone();
                let f = fraction.clone();
                let stop = cancel.clone();
                let result = tokio::task::spawn_blocking(move || {
                    build_index_until(Path::new(&path), &cache, &|x| *f.lock().unwrap() = x.clamp(0.0, 1.0), &|| {
                        stop.load(Ordering::Relaxed)
                    })
                })
                .await;
                let mut jobs = a.jobs.lock().unwrap();
                jobs.insert(
                    k,
                    match result {
                        Ok(Ok(_)) => Job::Done,
                        Ok(Err(_)) if cancel.load(Ordering::Relaxed) => Job::Cancelled,
                        Ok(Err(e)) => Job::Error(e.to_string()),
                        // a panic while indexing: the index folder is removed on the way out
                        Err(_) => Job::Error("indexing stopped on an internal error".into()),
                    },
                );
            });
        }
    }
    ok(Value::Dict(dict! { "key" => key, "state" => "indexing" }))
}

#[derive(serde::Deserialize)]
struct CancelRequest {
    key: String,
}

/// Stop an index being built (docs/viewer-api.md, `cancel`).
async fn cancel(State(app): State<Shared>, req: Request) -> Result<Response, ApiError> {
    let body = body_of(req, MAX_API_BODY).await?;
    let req: CancelRequest = object_body(&body)?;
    let jobs = app.jobs.lock().unwrap();
    let state = match jobs.get(&req.key) {
        Some(Job::Indexing { cancel, .. }) => {
            cancel.store(true, Ordering::Relaxed);
            "cancelling"
        }
        Some(Job::Cancelled) => "cancelled",
        _ => return Err(err(StatusCode::NOT_FOUND, "no index being built under this key")),
    };
    ok(Value::Dict(dict! { "key" => req.key, "state" => state }))
}

async fn describe(State(app): State<Shared>, UrlPath(key): UrlPath<String>) -> Result<Response, ApiError> {
    {
        let jobs = app.jobs.lock().unwrap();
        match jobs.get(&key) {
            Some(Job::Indexing { fraction, started, .. }) => {
                let fr = *fraction.lock().unwrap();
                return ok(Value::Dict(dict! {
                    "state" => "indexing",
                    "progress" => Value::Dict(dict! { "state" => "indexing", "fraction" => fr, "elapsed_s" => started.elapsed().as_secs_f64() }),
                }));
            }
            Some(Job::Error(e)) => return ok(Value::Dict(dict! { "state" => "error", "error" => e.clone() })),
            Some(Job::Cancelled) => return ok(Value::Dict(dict! { "state" => "cancelled" })),
            _ => {}
        }
    }
    let a = app.clone();
    let ds = blocking(move || a.need(&key)).await??;
    ok(Value::Dict(dict! { "state" => "ready", "dataset" => ds.info() }))
}

async fn window(
    State(app): State<Shared>,
    UrlPath(key): UrlPath<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let (t0, t1) = q_span(&q)?;
    let px = q_i64(&q, "px", 1200)?;
    let text = blocking(move || {
        let ds = app.need(&key)?;
        let ch = q_channels(&q, ds.channel_count())?;
        ds.window_json(&ch, t0, t1, px).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    })
    .await??;
    Ok(([(header::CONTENT_TYPE, "application/json")], text).into_response())
}

async fn vitals(
    State(app): State<Shared>,
    UrlPath(key): UrlPath<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let (t0, t1) = q_span(&q)?;
    let px = q_i64(&q, "px", 1200)?.clamp(16, 8000);
    let mut names: Vec<String> = vec![];
    for n in q.get("names").map(String::as_str).unwrap_or("").split('|').filter(|n| !n.is_empty()) {
        if !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    }
    if names.len() > MAX_SERIES {
        return Err(err(StatusCode::BAD_REQUEST, format!("at most {MAX_SERIES} vital-sign series at once")));
    }
    let v = blocking(move || app.need(&key).map(|ds| ds.vitals_window(&names, t0, t1, px))).await??;
    ok(v)
}

async fn vitals_at(
    State(app): State<Shared>,
    UrlPath(key): UrlPath<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let t = q_f64(&q, "t")?;
    let v = blocking(move || app.need(&key).map(|ds| ds.vitals_at(t))).await??;
    ok(v)
}

async fn markers(
    State(app): State<Shared>,
    UrlPath(key): UrlPath<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let q = query(raw)?;
    let (t0, t1) = q_span(&q)?;
    let v = blocking(move || {
        let ds = app.need(&key)?;
        let ch = q_channels(&q, ds.channel_count())?;
        Ok::<_, ApiError>(ds.markers(&ch, t0, t1))
    })
    .await??;
    ok(v)
}

fn html(code: StatusCode, body: String) -> Response {
    (code, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

async fn index_page(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if app.logged_in(&headers) {
        let body = crate::web::file("index.html").unwrap_or_default();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
            body,
        )
            .into_response();
    }
    html(StatusCode::UNAUTHORIZED, LOGIN_PAGE.replace("{error}", ""))
}

fn redirect_home() -> Response {
    Response::builder().status(StatusCode::SEE_OTHER).header(header::LOCATION, "./").body(Body::empty()).unwrap()
}

fn form_value(body: &[u8], key: &str) -> String {
    let text = String::from_utf8_lossy(body);
    for kv in text.split('&') {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if url_decode(k) == key {
            return url_decode(v);
        }
    }
    String::new()
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The address a request comes from (when the server knows it), for the delay after wrong
/// passwords.
fn client_of(req: &Request) -> String {
    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map_or_else(|| "?".to_string(), |c| c.0.ip().to_string())
}

impl App {
    /// The delay before a password from `client` is checked: none, then 1, 3, 7, 15 s ... (at
    /// most [`MAX_LOGIN_DELAY_S`]) after wrong passwords in the last [`FAILURES_KEPT`].
    fn login_delay(&self, client: &str) -> Duration {
        let mut f = self.failures.lock().unwrap();
        f.retain(|_, (_, last)| last.elapsed() < FAILURES_KEPT);
        let n = f.get(client).map_or(0, |x| x.0);
        Duration::from_secs(((1u64 << n.min(6)) - 1).min(MAX_LOGIN_DELAY_S))
    }

    fn login_failed(&self, client: &str) {
        let mut f = self.failures.lock().unwrap();
        if f.len() >= 10_000 && !f.contains_key(client) {
            f.clear(); // many addresses: start again rather than grow without bound
        }
        let e = f.entry(client.to_string()).or_insert((0, Instant::now()));
        *e = (e.0.saturating_add(1), Instant::now());
    }

    fn cookie_attributes(&self) -> &'static str {
        if self.settings.secure_cookie {
            "HttpOnly; Path=/; SameSite=lax; Secure"
        } else {
            "HttpOnly; Path=/; SameSite=lax"
        }
    }
}

async fn login(State(app): State<Shared>, req: Request) -> Response {
    if !app.protected {
        return redirect_home();
    }
    let client = client_of(&req);
    let body = match body_of(req, MAX_LOGIN_BODY).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let password = form_value(&body, "password");
    tokio::time::sleep(app.login_delay(&client)).await;
    let good = {
        let Ok(_permit) = app.checking.acquire().await else {
            return err(StatusCode::SERVICE_UNAVAILABLE, "").into_response();
        };
        let creds = app.creds.clone().unwrap();
        blocking(move || auth::check_password(&creds, &password)).await.unwrap_or(false)
    };
    if !good {
        app.login_failed(&client);
        tokio::time::sleep(Duration::from_secs(1)).await; // slow down guessing
        return html(StatusCode::UNAUTHORIZED, LOGIN_PAGE.replace("{error}", "<p class=\"err\">Wrong password.</p>"));
    }
    app.failures.lock().unwrap().remove(&client);
    let mut r = redirect_home();
    let cookie = format!(
        "{}={}; Max-Age={}; {}",
        app.cookie,
        auth::make_cookie(app.creds.as_ref().unwrap()),
        auth::LOGIN_DAYS * 86400,
        app.cookie_attributes()
    );
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    r
}

/// Remove the cookie; a valid sign-in presented here is also ended on the server, so that a
/// copy of the cookie no longer works (kept in the auth file: docs/viewer-api.md, "Sign-in").
async fn logout(State(app): State<Shared>, headers: HeaderMap) -> Response {
    if app.protected && app.logged_in(&headers) {
        if let Some((expires, sig)) = app.cookie_of(&headers).and_then(|v| {
            let (e, s) = v.split_once('.')?;
            Some((e.parse::<u64>().ok()?, s.to_string()))
        }) {
            let all = {
                let mut r = app.revoked.lock().unwrap();
                r.insert(sig, expires);
                r.clone()
            };
            let path = app.settings.auth_file.clone();
            let _ = blocking(move || auth::save_revoked(&path, &all)).await;
        }
    }
    let mut r = redirect_home();
    let cookie =
        format!("{}=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; {}", app.cookie, app.cookie_attributes());
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    r
}

async fn static_file(UrlPath(path): UrlPath<String>) -> Response {
    match crate::web::file(&path) {
        Some(body) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.to_string())], body).into_response()
        }
        None => err(StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

/// Deny by default: before sign-in only the page (which then shows the sign-in form),
/// `login`, `logout` and the static files are served.
async fn require_login(State(app): State<Shared>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let open = matches!(path.as_str(), "/" | "/login" | "/logout") || path.starts_with("/static/");
    if !open && !app.logged_in(req.headers()) {
        return err(StatusCode::UNAUTHORIZED, "sign in required").into_response();
    }
    next.run(req).await
}

/// Every request: a `Host` this server answers for (400 otherwise).  A request that changes
/// something (any method but GET and HEAD): no foreign `Origin` and no cross-site fetch (403),
/// and, a POST to the API, a JSON body (415).  Every answer: `nosniff`, and the page's CSP.
async fn guard(State(app): State<Shared>, req: Request, next: Next) -> Response {
    // a refusal carries the same headers as any other answer
    let mut r = match refusal(&app, &req) {
        Some(e) => e.into_response(),
        None => next.run(req).await,
    };
    let h = r.headers_mut();
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if h.get(header::CONTENT_TYPE).is_some_and(|c| c.as_bytes().starts_with(b"text/html")) {
        h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    }
    r
}

/// Why a request is refused before it is answered: another host's name (DNS rebinding), a
/// change asked from another site, an API change whose body is not JSON.
fn refusal(app: &App, req: &Request) -> Option<ApiError> {
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).map(str::to_string);
    let host = host.or_else(|| req.uri().authority().map(|a| a.to_string()));
    if !host.as_deref().is_some_and(|h| app.host_allowed(h)) {
        return Some(err(StatusCode::BAD_REQUEST, "this server does not answer for that host (see allowed_hosts)"));
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let origin = headers.get(header::ORIGIN).map(|o| o.to_str().unwrap_or(""));
        let foreign = origin.is_some_and(|o| {
            let rest = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://"));
            !rest.is_some_and(|h| !h.contains('/') && app.host_allowed(h))
        });
        let cross_site = headers.get("sec-fetch-site").is_some_and(|v| v.as_bytes() == b"cross-site");
        if foreign || cross_site {
            return Some(err(StatusCode::FORBIDDEN, "a request from another site"));
        }
        let json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|c| c.to_str().ok())
            .and_then(|c| c.split(';').next())
            .is_some_and(|c| c.trim().eq_ignore_ascii_case("application/json"));
        if req.method() == Method::POST && req.uri().path().starts_with("/api/") && !json {
            return Some(err(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "the request body must be JSON (Content-Type: application/json)",
            ));
        }
    }
    None
}

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/login", post(login))
        .route("/logout", post(logout))
        .route("/static/{*path}", get(static_file))
        .route("/api/v1/config", get(config))
        .route("/api/v1/browse", get(browse))
        .route("/api/v1/complete", get(complete))
        .route("/api/v1/scan", get(scan))
        .route("/api/v1/open", post(open))
        .route("/api/v1/cancel", post(cancel))
        .route("/api/v1/datasets/{key}", get(describe))
        .route("/api/v1/datasets/{key}/window", get(window))
        .route("/api/v1/datasets/{key}/vitals", get(vitals))
        .route("/api/v1/datasets/{key}/vitals_at", get(vitals_at))
        .route("/api/v1/datasets/{key}/markers", get(markers))
        .fallback(|| async { err(StatusCode::NOT_FOUND, "Not Found") })
        .method_not_allowed_fallback(|| async { err(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed") })
        .layer(middleware::from_fn_with_state(app.clone(), require_login))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_passwords_slow_down_their_sender() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::defaults();
        s.auth_file = dir.path().join("auth.json");
        let app = App::new(s.finish().unwrap(), Some(false)).unwrap();
        let delays: Vec<u64> = (0..8)
            .map(|_| {
                let d = app.login_delay("10.0.0.1").as_secs();
                app.login_failed("10.0.0.1");
                d
            })
            .collect();
        assert_eq!(delays, [0, 1, 3, 7, 15, 30, 30, 30]);
        assert_eq!(app.login_delay("10.0.0.2").as_secs(), 0, "per address");
    }

    #[test]
    fn numbers_and_hosts() {
        for ok in ["1", "-1", "+1.5", ".5", "5.", "1e5", "1E-5", "1709287200.25"] {
            assert!(decimal(ok), "{ok}");
        }
        for bad in ["", ".", "inf", "nan", "0x10", "1_0", " 1", "1 ", "1e", "e5", "--1", "1.2.3", "Infinity"] {
            assert!(!decimal(bad), "{bad}");
        }
        assert_eq!(host_name("[::1]:8060"), "::1");
        assert_eq!(host_name("LocalHost:80"), "localhost");
        assert_eq!(host_name("::1"), "::1");
        assert_eq!(host_name("[::1"), "");
    }
}
