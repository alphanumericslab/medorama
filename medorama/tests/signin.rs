//! Sign-in (signin.rs, docs/sign-in.md): passwords, and an institution's authenticating proxy
//! that names the user in a header.  The header counts only on requests from the proxy's
//! addresses; a name without an account is refused with a reason, while the page itself still
//! loads; passwords are refused once the proxy signs users in; each visit is audited once.

mod common;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use medorama::db::Db;
use medorama::signin::Method;
use medorama::users::Role;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

struct Fx {
    _tmp: tempfile::TempDir,
    app: axum::Router,
    db: Arc<Db>,
}

fn fixture(method: Method) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = common::config(&tmp.path().join("home"), &tmp.path().join("results"));
    cfg.sign_in = method;
    let db = Arc::new(Db::open(&cfg, true).unwrap());
    let c = db.get().unwrap();
    medorama::users::add(&c, "alice@hospital.org", "Alice", Role::Analyst, false, "0123456789ab").unwrap();
    medorama::users::add(&c, "admin", "", Role::Admin, true, "0123456789ab").unwrap();
    drop(c);
    let app = medorama::server::router(Arc::new(medorama::server::App::new(Arc::new(cfg), db.clone())));
    Fx { _tmp: tmp, app, db }
}

fn proxy() -> Method {
    Method::Proxy {
        header: "X-Remote-User".into(),
        from: vec!["127.0.0.1".parse().unwrap()],
        logout_url: "/oauth2/sign_out".into(),
    }
}

/// A request from `peer`, with the proxy's header naming `user` (if any).
async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    peer: &str,
    user: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("X-Medorama", "1");
    if let Some(u) = user {
        b = b.header("X-Remote-User", u);
    }
    let mut req = match body {
        Some(v) => b.header("Content-Type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    req.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn through_a_proxy() {
    let f = fixture(proxy());
    let app = &f.app;
    let proxy = "127.0.0.1:50000";
    // the page asks how to sign in, before anyone is signed in
    let (st, v) = send(app, "GET", "/api/v1/sign-in", "10.1.2.3:4000", None, None).await;
    assert_eq!(
        (st, v["method"].as_str(), v["logout_url"].as_str()),
        (StatusCode::OK, Some("proxy"), Some("/oauth2/sign_out"))
    );
    // the proxy names the user (names compare ignoring case)
    let (st, v) = send(app, "GET", "/api/v1/me", proxy, Some("Alice@Hospital.org"), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["user"]["username"], "alice@hospital.org");
    assert_eq!(v["user"]["privacy"], true, "a user without the PHI permission is in Privacy Mode");
    send(app, "GET", "/api/v1/patients", proxy, Some("alice@hospital.org"), None).await;
    // the same header from anywhere else counts for nothing
    let (st, _) = send(app, "GET", "/api/v1/me", "10.1.2.3:4000", Some("admin"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // from the proxy without the header: not signed in
    let (st, _) = send(app, "GET", "/api/v1/me", proxy, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // a name without an account: refused, with the reason; the page itself still loads
    let (st, v) = send(app, "GET", "/api/v1/me", proxy, Some("mallory"), None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(v["detail"].as_str().unwrap().contains("no account"), "{v}");
    let r = app.clone().oneshot(Request::builder().uri("/").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // passwords are not used
    let login = serde_json::json!({"username": "admin", "password": "0123456789ab"});
    let (st, _) = send(app, "POST", "/api/v1/login", proxy, None, Some(login)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let change = serde_json::json!({"old": "0123456789ab", "new": "abcdefghijkl"});
    let (st, _) = send(app, "POST", "/api/v1/me/password", proxy, Some("alice@hospital.org"), Some(change)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // an account for the proxy's users needs no password (it gets one nobody knows)
    let (st, v) = send(
        app,
        "POST",
        "/api/v1/users",
        proxy,
        Some("admin"),
        Some(serde_json::json!({"username": "bob@hospital.org", "role": "viewer"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, _) = send(app, "GET", "/api/v1/me", proxy, Some("bob@hospital.org"), None).await;
    assert_eq!(st, StatusCode::OK);
    // signing out: the proxy's sign-out
    let (st, v) = send(app, "POST", "/api/v1/logout", proxy, Some("alice@hospital.org"), None).await;
    assert_eq!((st, v["logout_url"].as_str()), (StatusCode::OK, Some("/oauth2/sign_out")));
    // the audit log: one sign-in per visit (alice's several requests; admin; bob), one refusal
    let c = f.db.get().unwrap();
    let n = |event: &str| -> i64 {
        c.query_row("SELECT COUNT(*) FROM audit WHERE event = ?1 AND detail LIKE 'proxy%'", [event], |r| r.get(0))
            .unwrap()
    };
    assert_eq!((n("login"), n("login_failed")), (3, 1));
}

#[tokio::test]
async fn with_passwords() {
    let f = fixture(Method::Password);
    let app = &f.app;
    let (_, v) = send(app, "GET", "/api/v1/sign-in", "127.0.0.1:1", None, None).await;
    assert_eq!(v["method"], "password");
    // a proxy's header means nothing here, even from this computer
    let (st, _) = send(app, "GET", "/api/v1/me", "127.0.0.1:1", Some("admin"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let login = serde_json::json!({"username": "admin", "password": "0123456789ab"});
    let (st, v) = send(app, "POST", "/api/v1/login", "127.0.0.1:1", None, Some(login)).await;
    assert_eq!((st, v["user"]["username"].as_str()), (StatusCode::OK, Some("admin")));
}

/// The proxy's header counts once: given twice (the client's and the proxy's), it names nobody.
/// An IPv4 proxy's address as IPv6 writes it (`::ffff:127.0.0.1`) is the same address.  The
/// Privacy Mode choice lasts the visit.
#[tokio::test]
async fn proxy_headers_and_addresses() {
    let f = fixture(proxy());
    let app = &f.app;
    let mut req = Request::builder()
        .uri("/api/v1/me")
        .header("X-Remote-User", "mallory")
        .header("X-Remote-User", "admin")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo("127.0.0.1:5000".parse::<SocketAddr>().unwrap()));
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let (st, v) = send(app, "GET", "/api/v1/me", "[::ffff:127.0.0.1]:5000", Some("admin"), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // an admin's default: Privacy Mode off; switched on, for this visit
    assert_eq!(v["user"]["privacy"], false);
    let (st, _) =
        send(app, "POST", "/api/v1/privacy", "127.0.0.1:1", Some("admin"), Some(serde_json::json!({"on": true}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, v) = send(app, "GET", "/api/v1/me", "127.0.0.1:1", Some("admin"), None).await;
    assert_eq!(v["user"]["privacy"], true);
    // signed out: the next visit starts in the default
    send(app, "POST", "/api/v1/logout", "127.0.0.1:1", Some("admin"), None).await;
    let (_, v) = send(app, "GET", "/api/v1/me", "127.0.0.1:1", Some("admin"), None).await;
    assert_eq!(v["user"]["privacy"], false);
}

/// Wrong passwords tried at once count before they are checked: no more than the limit is
/// checked; the name is locked from that address only; a name no account can have is refused
/// at once, and the audit log keeps 64 characters of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn throttled_sign_in() {
    let f = fixture(Method::Password);
    let guess = |user: String, password: String, peer: &'static str| {
        let app = f.app.clone();
        tokio::spawn(async move {
            let body = serde_json::json!({"username": user, "password": password});
            send(&app, "POST", "/api/v1/login", peer, None, Some(body)).await.0
        })
    };
    let mut tasks = vec![];
    for i in 0..40 {
        tasks.push(guess("admin".into(), format!("wrong-guess-{i}"), "10.0.0.9:1"));
    }
    let mut codes = std::collections::HashMap::new();
    for t in tasks {
        *codes.entry(t.await.unwrap()).or_insert(0) += 1;
    }
    assert_eq!(codes.get(&StatusCode::UNAUTHORIZED), Some(&5), "{codes:?}");
    assert_eq!(codes.get(&StatusCode::TOO_MANY_REQUESTS), Some(&35), "{codes:?}");
    // locked there; the right password from elsewhere still signs in
    assert_eq!(
        guess("admin".into(), "0123456789ab".into(), "10.0.0.9:1").await.unwrap(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(guess("admin".into(), "0123456789ab".into(), "10.0.0.7:1").await.unwrap(), StatusCode::OK);
    // many names from one address: that address is refused beyond its limit
    let mut tasks = vec![];
    for i in 0..40 {
        tasks.push(guess(format!("nobody{i}"), "x".into(), "10.0.0.8:1"));
    }
    let mut locked = 0;
    for t in tasks {
        locked += (t.await.unwrap() == StatusCode::TOO_MANY_REQUESTS) as usize;
    }
    assert_eq!(locked, 10, "30 attempts per address and minute");
    // a name no account can have (too long)
    let long = format!("x{}", "y".repeat(100_000));
    assert_eq!(guess(long, "x".into(), "10.0.0.6:1").await.unwrap(), StatusCode::UNAUTHORIZED);
    let longest: i64 =
        f.db.get()
            .unwrap()
            .query_row("SELECT MAX(LENGTH(target)) FROM audit WHERE event = 'login_failed'", [], |r| r.get(0))
            .unwrap();
    assert!(longest <= 65, "{longest}");
}

/// The PHI permission is given only by an admin who has it: an admin without it cannot give it
/// to themselves, nor to an account they make.
#[tokio::test]
async fn the_phi_permission_is_given_by_those_who_have_it() {
    let f = fixture(Method::Password);
    let app = &f.app;
    medorama::users::add(&f.db.get().unwrap(), "plain", "", Role::Admin, false, "0123456789ab").unwrap();
    let login = |u: &str| {
        let app = app.clone();
        let u = u.to_string();
        async move {
            let body = serde_json::json!({"username": u, "password": "0123456789ab"});
            let req = Request::builder()
                .method("POST")
                .uri("/api/v1/login")
                .header("X-Medorama", "1")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let r = app.oneshot(req).await.unwrap();
            r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string()
        }
    };
    let call = |cookie: String, method: &'static str, uri: String, body: Value| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .header("Cookie", cookie)
                .header("X-Medorama", "1")
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        }
    };
    let plain = login("plain").await;
    let me: i64 =
        f.db.get().unwrap().query_row("SELECT id FROM users WHERE username = 'plain'", [], |r| r.get(0)).unwrap();
    let alice: i64 =
        f.db.get()
            .unwrap()
            .query_row("SELECT id FROM users WHERE username = 'alice@hospital.org'", [], |r| r.get(0))
            .unwrap();
    let phi = serde_json::json!({"phi": true});
    assert_eq!(call(plain.clone(), "PATCH", format!("/api/v1/users/{me}"), phi.clone()).await, StatusCode::FORBIDDEN);
    assert_eq!(
        call(plain.clone(), "PATCH", format!("/api/v1/users/{alice}"), phi.clone()).await,
        StatusCode::FORBIDDEN
    );
    let new = serde_json::json!({"username": "carol", "role": "admin", "phi": true, "password": "0123456789ab"});
    assert_eq!(call(plain.clone(), "POST", "/api/v1/users".into(), new.clone()).await, StatusCode::FORBIDDEN);
    // without it, other changes go through; an admin with it gives it
    let named = serde_json::json!({"display_name": "Plain"});
    assert_eq!(call(plain, "PATCH", format!("/api/v1/users/{me}"), named).await, StatusCode::OK);
    let admin = login("admin").await;
    assert_eq!(call(admin, "PATCH", format!("/api/v1/users/{alice}"), phi).await, StatusCode::OK);
}
