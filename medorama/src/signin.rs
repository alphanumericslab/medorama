//! Sign-in: how Medorama knows who a request comes from (docs/sign-in.md).  This module is the
//! only code that knows how users sign in; every handler gets the signed-in user as
//! `server::Auth(User)`, and what that user may see and do (roles, the PHI permission, Privacy
//! Mode) is decided elsewhere, the same for every method.
//!
//! | `sign_in` (medorama.toml) | Who the user is |
//! |---|---|
//! | `password` (default) | the account whose name and password were given to `POST login`; a session cookie (HttpOnly, SameSite=Strict, a random token stored hashed) names it afterwards |
//! | `proxy` | the account named by the header (`proxy_user_header`) that the institution's authenticating reverse proxy sets, on requests that come from that proxy (`proxy_addresses`) |
//!
//! With `proxy`, the institution's sign-in (single sign-on through SAML or OpenID Connect,
//! Kerberos, a VPN gateway, multi-factor authentication) is done by the proxy, and Medorama
//! never sees a password: `POST login` is refused, and accounts (their roles and PHI permission)
//! are still kept in Medorama, named as the proxy names their users.  Another method is a new
//! variant of [`Method`], its arm in [`identify`] (and in [`login`] if it signs in through the
//! page), and its settings in `config.rs` (`sign_in_method`); nothing else changes.

use crate::config::Config;
use crate::db::audit;
use crate::server::{blocking, internal, ApiError, ApiResult, Shared};
use crate::sql::Connection;
use crate::users::{self, User};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Failed password sign-ins for a name from one address before that name is locked there for
/// [`LOCK_S`] seconds (from that address only: nobody can lock another user out from elsewhere).
const MAX_FAILS: u32 = 5;
const LOCK_S: u64 = 60;
/// Failed password sign-ins from one address, whatever the names, before it is refused for
/// [`LOCK_S`] seconds.
const MAX_ADDRESS_FAILS: u32 = 30;
/// The most (name, address) and address counts kept: the oldest go first beyond it.
const MAX_KEPT: usize = 10_000;
/// Password checks (Argon2: memory and time on purpose) run at once; the others wait.
const CHECKS_AT_ONCE: usize = 4;

/// How users sign in (the setting `sign_in`).
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    /// Medorama's own accounts and passwords.
    Password,
    /// An authenticating reverse proxy in front of Medorama names the user.
    Proxy {
        /// The header the proxy sets to the user's name (`X-Remote-User`).
        header: String,
        /// The addresses the proxy connects from: the header is believed from these only.
        from: Vec<IpAddr>,
        /// Where **Sign out** leads (the proxy's own sign-out), if anywhere.
        logout_url: String,
    },
}

impl Method {
    pub fn of(cfg: &Config) -> Method {
        cfg.sign_in.clone()
    }

    pub fn name(&self) -> &'static str {
        match self {
            Method::Password => "password",
            Method::Proxy { .. } => "proxy",
        }
    }
}

/// What sign-in keeps between requests: password attempts (per name and address, and per
/// address), the password checks running, when each user was last seen through the proxy (to
/// write one `login` to the audit log per visit, not per request), and proxy users' Privacy
/// Mode choices (password sessions keep theirs in their row: users.rs).
pub struct SignIn {
    fails: Mutex<Fails>,
    checks: tokio::sync::Semaphore,
    seen: Mutex<HashMap<String, Instant>>,
    privacy: Mutex<HashMap<i64, (bool, Instant)>>,
}

impl Default for SignIn {
    fn default() -> Self {
        SignIn {
            fails: Mutex::new(Fails::default()),
            checks: tokio::sync::Semaphore::new(CHECKS_AT_ONCE),
            seen: Mutex::new(HashMap::new()),
            privacy: Mutex::new(HashMap::new()),
        }
    }
}

/// Password attempts, counted before the password is checked (so that attempts made at once
/// count, all of them), with when the last was made.
#[derive(Default)]
struct Fails {
    by_name: HashMap<(String, Option<IpAddr>), (u32, Instant)>,
    by_address: HashMap<Option<IpAddr>, (u32, Instant)>,
}

/// One more attempt on a count, unless it is at `max` within [`LOCK_S`]: false then.
fn reserve<K: std::hash::Hash + Eq + Clone>(m: &mut HashMap<K, (u32, Instant)>, k: &K, max: u32) -> bool {
    let window = Duration::from_secs(LOCK_S);
    if m.len() >= MAX_KEPT {
        m.retain(|_, e| e.1.elapsed() < window);
        while m.len() >= MAX_KEPT {
            let Some(old) = m.iter().min_by_key(|(_, e)| e.1).map(|(k, _)| k.clone()) else { break };
            m.remove(&old);
        }
    }
    let e = m.entry(k.clone()).or_insert((0, Instant::now()));
    if e.1.elapsed() >= window {
        *e = (0, Instant::now());
    }
    if e.0 >= max {
        return false;
    }
    e.0 += 1;
    e.1 = Instant::now();
    true
}

impl Fails {
    /// An attempt for `name` from `ip`, counted now; false when the name or the address is
    /// locked.
    fn reserve(&mut self, name: &str, ip: Option<IpAddr>) -> bool {
        let key = (name.to_lowercase(), ip);
        let locked = self.by_name.get(&key).is_some_and(|e| e.0 >= MAX_FAILS && e.1.elapsed().as_secs() < LOCK_S)
            || self.by_address.get(&ip).is_some_and(|e| e.0 >= MAX_ADDRESS_FAILS && e.1.elapsed().as_secs() < LOCK_S);
        !locked && reserve(&mut self.by_name, &key, MAX_FAILS) && reserve(&mut self.by_address, &ip, MAX_ADDRESS_FAILS)
    }

    /// An attempt under a name no account can have: counted for the address alone.
    fn reserve_address(&mut self, ip: Option<IpAddr>) -> bool {
        reserve(&mut self.by_address, &ip, MAX_ADDRESS_FAILS)
    }

    /// A successful sign-in: its attempt does not count.
    fn succeeded(&mut self, name: &str, ip: Option<IpAddr>) {
        self.by_name.remove(&(name.to_lowercase(), ip));
        if let Some(e) = self.by_address.get_mut(&ip) {
            e.0 = e.0.saturating_sub(1);
        }
    }
}

/// Who a request comes from.
pub enum Who {
    /// A user, with the Privacy Mode choice made in this session (`None`: none, the user's
    /// default: server.rs `in_privacy`).
    User(User, Option<bool>),
    /// Not signed in.
    Nobody,
    /// The proxy named someone who has no (enabled) account here.
    NoAccount(String),
}

/// A cookie's value.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// The user a request comes from.  `cookie`: the name of the session cookie; `peer`: the
/// address the request came from (the proxy's, behind one); `active`: the user's own request,
/// which keeps an idle session alive (not the page's background checks).
pub fn identify(
    si: &SignIn,
    cfg: &Config,
    c: &Connection,
    cookie: &str,
    headers: &HeaderMap,
    peer: Option<IpAddr>,
    active: bool,
) -> anyhow::Result<Who> {
    match &cfg.sign_in {
        Method::Password => {
            let Some(token) = cookie_value(headers, cookie) else { return Ok(Who::Nobody) };
            let s = users::session(c, &token, cfg.session_idle_minutes * 60, active)?;
            Ok(s.map_or(Who::Nobody, |(u, privacy)| Who::User(u, privacy)))
        }
        Method::Proxy { header, from, .. } => {
            // the header counts only on requests from the proxy: anyone else could send it
            // (an IPv4 address as IPv6 writes it, ::ffff:a.b.c.d, is that IPv4 address)
            if !peer.is_some_and(|p| from.iter().any(|f| f.to_canonical() == p.to_canonical())) {
                return Ok(Who::Nobody);
            }
            // one value: a header given twice (the client's and the proxy's) names nobody
            let mut values = headers.get_all(header.as_str()).iter();
            let (Some(value), None) = (values.next(), values.next()) else { return Ok(Who::Nobody) };
            let Some(name) = value.to_str().ok().map(str::trim) else {
                return Ok(Who::Nobody);
            };
            if name.is_empty() {
                return Ok(Who::Nobody);
            }
            let user = users::get_by_login(c, name)?;
            // one audit entry per visit: when the user (or the unknown name) was not seen for
            // the idle time of a session
            let idle = Duration::from_secs((cfg.session_idle_minutes * 60).max(60));
            let first = {
                let mut seen = si.seen.lock().unwrap();
                let key = name.to_lowercase();
                let first = seen.get(&key).is_none_or(|t| t.elapsed() > idle);
                seen.insert(key, Instant::now());
                first
            };
            match user {
                Some(u) => {
                    let mut choices = si.privacy.lock().unwrap();
                    if first {
                        // a new visit starts in the user's default mode
                        choices.remove(&u.id);
                        audit(c, Some(&u), "login", "", "proxy");
                    }
                    let hours = Duration::from_secs(cfg.session_hours.max(1) * 3600);
                    choices.retain(|_, e| e.1.elapsed() < hours);
                    let choice = choices.get(&u.id).map(|e| e.0);
                    Ok(Who::User(u, choice))
                }
                None => {
                    if first {
                        audit(c, None, "login_failed", name, "proxy: no such account");
                    }
                    Ok(Who::NoAccount(name.to_string()))
                }
            }
        }
    }
}

/// Records a session's Privacy Mode choice (`on`), kept by the server, never by the browser: a
/// password session's in its row, until it ends; a proxy user's until their visit ends (one
/// idle time without a request, as their `login` entry) or `session_hours` pass, in this
/// server's memory (with several servers, another one answers in the user's default mode).
pub fn set_privacy(
    si: &SignIn,
    cfg: &Config,
    c: &Connection,
    cookie: &str,
    headers: &HeaderMap,
    user: &User,
    on: bool,
) -> anyhow::Result<()> {
    match &cfg.sign_in {
        Method::Password => {
            let token = cookie_value(headers, cookie).unwrap_or_default();
            if !users::set_session_privacy(c, &token, on)? {
                anyhow::bail!("no session to keep the choice in");
            }
        }
        Method::Proxy { .. } => {
            si.privacy.lock().unwrap().insert(user.id, (on, Instant::now()));
        }
    }
    Ok(())
}

/// `GET sign-in`: how the page signs its users in (asked before anyone is signed in).
pub async fn method(State(app): State<Shared>) -> ApiResult {
    let m = &app.cfg.sign_in;
    let logout = match m {
        Method::Proxy { logout_url, .. } => logout_url.clone(),
        Method::Password => String::new(),
    };
    Ok(Json(json!({ "method": m.name(), "logout_url": logout })).into_response())
}

#[derive(Deserialize)]
pub struct Login {
    username: String,
    password: String,
}

/// A name as the audit log records it: at most 64 characters (a sign-in may give any text).
fn short(name: &str) -> String {
    match name.char_indices().nth(64) {
        Some((at, _)) => format!("{}…", &name[..at]),
        None => name.to_string(),
    }
}

/// `POST login`: a name and password (`password`); a session cookie on success.  Refused with
/// `proxy`: the institution's sign-in names the user.  Attempts are counted before the
/// password is checked: per name and address (`MAX_FAILS`) and per address
/// (`MAX_ADDRESS_FAILS`) within `LOCK_S` seconds; at most `CHECKS_AT_ONCE` passwords are
/// checked at once.
pub async fn login(State(app): State<Shared>, ext: axum::http::Extensions, Json(req): Json<Login>) -> ApiResult {
    if app.cfg.sign_in != Method::Password {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "this catalog signs users in through the institution's sign-in, not with a password".into(),
        ));
    }
    let wrong = || ApiError(StatusCode::UNAUTHORIZED, "wrong user name or password".into());
    let ip = crate::server::peer(&ext);
    // a name no account can have is not looked up, nor kept
    if !users::valid_username(&req.username) {
        if !app.signin.fails.lock().unwrap().reserve_address(ip) {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                format!("too many failed attempts; try again in {LOCK_S} s"),
            ));
        }
        let (db, name) = (app.db.clone(), short(&req.username));
        blocking(move || {
            audit(&*db.get().map_err(internal)?, None, "login_failed", &name, "not a user name");
            Ok(())
        })
        .await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Err(wrong());
    }
    if !app.signin.fails.lock().unwrap().reserve(&req.username, ip) {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            format!("too many failed attempts; try again in {LOCK_S} s"),
        ));
    }
    let a = app.clone();
    let name = req.username.clone();
    let result = {
        let _check = app.signin.checks.acquire().await.map_err(internal)?;
        blocking(move || {
            let c = a.db.get().map_err(internal)?;
            match users::authenticate(&c, &req.username, &req.password).map_err(internal)? {
                Some(u) => {
                    let token = users::new_session(&c, &u, a.cfg.session_hours).map_err(internal)?;
                    audit(&c, Some(&u), "login", "", "");
                    Ok(Some((u, token)))
                }
                None => {
                    audit(&c, None, "login_failed", &req.username, "");
                    Ok(None)
                }
            }
        })
        .await?
    };
    let Some((u, token)) = result else {
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Err(wrong());
    };
    app.signin.fails.lock().unwrap().succeeded(&name, ip);
    let cookie = format!(
        "{}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}{}",
        app.cookie,
        app.cfg.session_hours * 3600,
        if app.cfg.secure_cookies { "; Secure" } else { "" }
    );
    let mut r = Json(json!({ "user": u })).into_response();
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(internal)?);
    Ok(r)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Logout {
    /// Every session of the user, on every device, not only this one.  Audited.
    everywhere: bool,
}

/// `POST logout`: ends the session (`password`; every session of the user with `everywhere`).
/// With `proxy`, the proxy's session is the user's: the answer names where to end it
/// (`logout_url`).  Either way, the Privacy Mode choice is forgotten (it is the session's).
pub async fn logout(
    State(app): State<Shared>,
    ext: axum::http::Extensions,
    headers: HeaderMap,
    body: Option<Json<Logout>>,
) -> ApiResult {
    let everywhere = body.is_some_and(|b| b.everywhere);
    let mut logout_url = String::new();
    match &app.cfg.sign_in {
        Method::Proxy { logout_url: u, .. } => {
            logout_url = u.clone();
            // the visit ends: the next request starts a new one, in the user's default mode
            if let Some(u) = crate::server::session_of(&ext) {
                app.signin.privacy.lock().unwrap().remove(&u.id);
                app.signin.seen.lock().unwrap().remove(&u.username.to_lowercase());
            }
        }
        Method::Password => {
            if let Some(token) = cookie_value(&headers, &app.cookie) {
                let db = app.db.clone();
                let idle = app.cfg.session_idle_minutes * 60;
                blocking(move || {
                    let c = db.get().map_err(internal)?;
                    match users::session_user(&c, &token, idle, false).map_err(internal)? {
                        Some(u) if everywhere => {
                            let n = users::end_sessions(&c, u.id).map_err(internal)?;
                            audit(&c, Some(&u), "signed_out_everywhere", &u.username, &format!("{n} session(s)"));
                        }
                        _ => users::end_session(&c, &token).map_err(internal)?,
                    }
                    Ok(())
                })
                .await?;
            }
        }
    }
    let mut r = Json(json!({ "logout_url": logout_url })).into_response();
    let cookie = format!("{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0", app.cookie);
    r.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(internal)?);
    Ok(r)
}
