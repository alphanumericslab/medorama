//! The viewer on the synthetic recordings (testdata/): indexing, cache, queries, browsing and
//! the HTTP API (including sign-in).  Parity with the other viewers is checked by
//! tests/parity/viewer_parity.py.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use recording_engine::dataset::Dataset;
use recording_engine::index::{build_index, load_index};
use signal_core::value::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;
use vitalstream_rs::server::{router, App};
use vitalstream_rs::settings::Settings;

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata").join(name)
}

#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB records: a build without the `wfdb` feature refuses them")]
#[test]
fn index_and_query_every_recording() {
    let cache = tempfile::tempdir().unwrap();
    for name in ["collection", "plain"] {
        let key = build_index(&testdata(name), cache.path(), &|_| {}).unwrap();
        let (meta, arrays) = load_index(cache.path(), &key).unwrap();
        let ds = Dataset::new(meta, arrays);
        let info = ds.info();
        let range = info.get("t_range").and_then(Value::as_list).unwrap().clone();
        let (t0, t1) = (range[0].as_f64().unwrap(), range[1].as_f64().unwrap());
        let n_ch = info.get("channels").and_then(Value::as_list).unwrap().len() as i64;
        let chs: Vec<i64> = (0..n_ch).collect();
        for (a, b) in [(t0, t1), (t0, t0 + 2.0), (t0 + 1.0, t0 + 1.2)] {
            let w = Value::from_json(&ds.window_json(&chs, a, b, 800).unwrap()).unwrap();
            assert_eq!(w.get("channels").and_then(Value::as_dict).unwrap().len() as i64, n_ch);
        }
        // samples decoded on demand equal the envelope of the whole piece
        for (p, piece) in ds.pieces.iter().enumerate() {
            let x = ds.samples(p, 0, piece.n).unwrap();
            assert_eq!(x.len() as i64, piece.n, "{name} piece {p}");
        }
        let names: Vec<String> = info
            .get("vitals")
            .and_then(Value::as_list)
            .unwrap()
            .iter()
            .map(|v| v.get("name").and_then(Value::as_str).unwrap().to_string())
            .collect();
        ds.vitals_window(&names, t0, t1, 100);
        ds.vitals_at((t0 + t1) / 2.0);
        ds.markers(&chs, t0, t1);
        if name == "plain" {
            // nothing assumed that a header does not state: ADC units for an uncalibrated signal,
            // no unit when none is stated, and the record without a sampling frequency left out
            let mut units: Vec<String> = info
                .get("channels")
                .and_then(Value::as_list)
                .unwrap()
                .iter()
                .map(|c| c.get("unit").and_then(Value::as_str).unwrap().to_string())
                .collect();
            units.sort();
            assert_eq!(units, ["", "adu", "mV", "mmHg"]);
        }
        if name == "collection" {
            // three stretches; the last, without a start time, after the 0.5 s break stated before it
            let starts: Vec<f64> = ds.pieces.iter().filter(|p| p.ch == 0).map(|p| p.t0).collect();
            assert_eq!(starts, [1709287200.0, 1709287205.5, 1709287209.0]);
            assert_eq!(names, ["HR", "SpO2", "HR [import]"], "the invalid row left out; a second source named");
            let alarms = info.get("alarms").and_then(Value::as_list).unwrap();
            assert_eq!(alarms.len(), 2);
            assert_eq!(alarms[0].get("text").and_then(Value::as_str), Some("HR HIGH 151 > 150"));
            let gaps = info.get("gaps").and_then(Value::as_list).unwrap();
            assert_eq!(gaps.len(), 2);
            let marks = ds.markers(&[0], t0, t1);
            assert!(marks.py_str().contains("R wave") && marks.py_str().contains("'V'"), "{}", marks.py_str());
        }
    }
}

fn app(protected: bool, auth: &Path, cache: &Path) -> Arc<App> {
    app_with(protected, auth, cache, |_| {})
}

fn app_with(protected: bool, auth: &Path, cache: &Path, f: impl FnOnce(&mut Settings)) -> Arc<App> {
    let mut s = Settings::defaults();
    s.auth_file = auth.to_path_buf();
    s.cache_dir = cache.to_path_buf();
    f(&mut s);
    Arc::new(App::new(s.finish().unwrap(), Some(protected)).unwrap())
}

/// A request's answer (a browser's request: to `localhost:8060` unless it names a host).
async fn call(app: &Arc<App>, mut req: Request<Body>) -> (StatusCode, Vec<u8>, Option<String>) {
    if !req.headers().contains_key("host") {
        req.headers_mut().insert("host", "localhost:8060".parse().unwrap());
    }
    let r = router(app.clone()).oneshot(req).await.unwrap();
    let status = r.status();
    let cookie = r.headers().get("set-cookie").map(|v| v.to_str().unwrap().to_string());
    let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec();
    (status, body, cookie)
}

#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB records: a build without the `wfdb` feature refuses them")]
#[tokio::test]
async fn api_open_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let a = app(false, &dir.path().join("auth.json"), &dir.path().join("cache"));
    let (s, body, _) = call(&a, Request::get("/api/v1/config").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(Value::from_json(std::str::from_utf8(&body).unwrap()).unwrap().get("timezone").is_some());
    let path = testdata("collection").canonicalize().unwrap();
    let req = Request::post("/api/v1/open")
        .header("content-type", "application/json")
        .body(Body::from(format!("{{\"path\": {}}}", Value::Str(path.display().to_string()).to_json())))
        .unwrap();
    let (s, body, _) = call(&a, req).await;
    assert_eq!(s, StatusCode::OK);
    let key = Value::from_json(std::str::from_utf8(&body).unwrap()).unwrap().get("key").unwrap().py_str();
    let mut ready = None;
    for _ in 0..300 {
        let (_, body, _) = call(&a, Request::get(format!("/api/v1/datasets/{key}")).body(Body::empty()).unwrap()).await;
        let v = Value::from_json(std::str::from_utf8(&body).unwrap()).unwrap();
        if v.get("state").and_then(Value::as_str) == Some("ready") {
            ready = Some(v);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let info = ready.expect("indexed").get("dataset").cloned().unwrap();
    let t0 = info.get("t_range").and_then(Value::as_list).unwrap()[0].as_f64().unwrap();
    let url = format!("/api/v1/datasets/{key}/window?t0={t0}&t1={}&px=500&ch=0,1", t0 + 5.0);
    let (s, body, _) = call(&a, Request::get(url).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(Value::from_json(std::str::from_utf8(&body).unwrap()).is_ok());
    let (s, _, _) =
        call(&a, Request::get(format!("/api/v1/datasets/{key}/window?t0=5&t1=1")).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, _) = call(&a, Request::get("/static/js/app.js").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
}

#[cfg(feature = "password")] // a build without password support has no sign-in
#[tokio::test]
async fn sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.json");
    vitalstream_rs::auth::set_password(&auth, "correct horse").unwrap();
    let a = app(true, &auth, &dir.path().join("cache"));
    let (s, _, _) = call(&a, Request::get("/api/v1/config").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, body, _) = call(&a, Request::get("/").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(String::from_utf8_lossy(&body).contains("Sign in"));
    let form = |pw: &str| {
        Request::post("/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(format!("password={pw}")))
            .unwrap()
    };
    let (s, _, _) = call(&a, form("wrong")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, cookie) = call(&a, form("correct+horse")).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    let cookie = cookie.unwrap();
    let pair = cookie.split(';').next().unwrap().to_string();
    let (s, _, _) =
        call(&a, Request::get("/api/v1/config").header("cookie", pair.clone()).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    // a body larger than a password is refused before anything is checked
    let (s, _, _) = call(&a, form(&"x".repeat(5000))).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    // logout ends the sign-in on the server: a copy of the cookie no longer works, even after
    // a restart (kept in the auth file)
    let (s, _, gone) =
        call(&a, Request::post("/logout").header("cookie", pair.clone()).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(gone.unwrap().contains("Max-Age=0"));
    for server in [a.clone(), app(true, &auth, &dir.path().join("cache"))] {
        let (s, _, _) =
            call(&server, Request::get("/api/v1/config").header("cookie", pair.clone()).body(Body::empty()).unwrap())
                .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    assert!(vitalstream_rs::auth::load(&auth).is_some(), "the password is kept");
    // behind HTTPS: the cookie is marked Secure
    let secure = app_with(true, &auth, &dir.path().join("cache"), |s| s.secure_cookie = true);
    let (s, _, cookie) = call(&secure, form("correct+horse")).await;
    assert_eq!(s, StatusCode::SEE_OTHER);
    assert!(cookie.unwrap().contains("; Secure"));
}

fn json(body: &[u8]) -> Value {
    Value::from_json(std::str::from_utf8(body).unwrap()).unwrap()
}

fn open_request(path: &Path) -> Request<Body> {
    Request::post("/api/v1/open")
        .header("content-type", "application/json")
        .body(Body::from(format!("{{\"path\": {}}}", Value::Str(path.display().to_string()).to_json())))
        .unwrap()
}

/// Open `path` and wait for its index: the dataset's final state.
async fn opened(a: &Arc<App>, path: &Path) -> (String, Value) {
    let (s, body, _) = call(a, open_request(path)).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let key = json(&body).get("key").unwrap().py_str();
    for _ in 0..400 {
        let (_, body, _) = call(a, Request::get(format!("/api/v1/datasets/{key}")).body(Body::empty()).unwrap()).await;
        let v = json(&body);
        if v.get("state").and_then(Value::as_str) != Some("indexing") {
            return (key, v);
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("still indexing");
}

#[tokio::test]
async fn requests_from_other_sites() {
    let dir = tempfile::tempdir().unwrap();
    let a = app(false, &dir.path().join("auth.json"), &dir.path().join("cache"));
    let get = |host: &str| Request::get("/api/v1/config").header("host", host).body(Body::empty()).unwrap();
    // DNS rebinding: a name this server does not answer for
    for (host, want) in [
        ("localhost:8060", StatusCode::OK),
        ("127.0.0.1:8060", StatusCode::OK),
        ("[::1]:8060", StatusCode::OK),
        ("LOCALHOST", StatusCode::OK),
        ("evil.example:8060", StatusCode::BAD_REQUEST),
        ("localhost.evil.example", StatusCode::BAD_REQUEST),
        ("", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(call(&a, get(host)).await.0, want, "{host}");
    }
    let proxied = app_with(false, &dir.path().join("auth.json"), &dir.path().join("cache"), |s| {
        s.allowed_hosts = vec!["portal.example.org".into()];
    });
    assert_eq!(call(&proxied, get("portal.example.org")).await.0, StatusCode::OK);
    assert_eq!(call(&proxied, get("other.example.org")).await.0, StatusCode::BAD_REQUEST);

    // a request that changes something: JSON, from no other site
    let path = testdata("edf/plain.edf").canonicalize().unwrap();
    let body = format!("{{\"path\": {}}}", Value::Str(path.display().to_string()).to_json());
    let post = |ct: &str, origin: Option<&str>| {
        let mut r = Request::post("/api/v1/open").header("content-type", ct);
        if let Some(o) = origin {
            r = r.header("origin", o);
        }
        r.body(Body::from(body.clone())).unwrap()
    };
    assert_eq!(call(&a, post("text/plain", None)).await.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(call(&a, post("application/x-www-form-urlencoded", None)).await.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(call(&a, post("application/json", Some("http://evil.example"))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&a, post("application/json", Some("null"))).await.0, StatusCode::FORBIDDEN);
    let cross = Request::post("/api/v1/cancel")
        .header("content-type", "application/json")
        .header("sec-fetch-site", "cross-site")
        .body(Body::from("{\"key\":\"0\"}"))
        .unwrap();
    assert_eq!(call(&a, cross).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        call(&a, post("application/json; charset=utf-8", Some("http://localhost:8060"))).await.0,
        StatusCode::OK
    );
    // a field of the wrong type
    let typed = Request::post("/api/v1/open")
        .header("content-type", "application/json")
        .body(Body::from(body.replace('}', ", \"reindex\": \"yes\"}")))
        .unwrap();
    assert_eq!(call(&a, typed).await.0, StatusCode::BAD_REQUEST);
    // bodies: at most 64 KiB
    let big = Request::post("/api/v1/cancel")
        .header("content-type", "application/json")
        .body(Body::from(vec![b' '; 70_000]))
        .unwrap();
    assert_eq!(call(&a, big).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    // a method a known path does not take: 405, in JSON
    let (s, body, _) = call(&a, Request::delete("/api/v1/config").body(Body::empty()).unwrap()).await;
    assert_eq!(
        (s, json(&body).get("detail").map(Value::py_str)),
        (StatusCode::METHOD_NOT_ALLOWED, Some("Method Not Allowed".into()))
    );
    // the page: its Content-Security-Policy, and no inline style or script
    let r = router(a.clone())
        .oneshot(Request::get("/").header("host", "localhost").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csp = r.headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(
        csp.contains("script-src 'self'") && csp.contains("frame-ancestors 'self'") && !csp.contains("unsafe"),
        "{csp}"
    );
    assert!(!vitalstream_rs::server::LOGIN_PAGE.contains("<style"));
    let (s, _, _) = call(&a, Request::get("/static/login.css").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
}

#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB records: a build without the `wfdb` feature refuses them")]
#[tokio::test]
async fn queries_are_checked_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let a = app(false, &dir.path().join("auth.json"), &dir.path().join("cache"));
    let (key, v) = opened(&a, &testdata("collection").canonicalize().unwrap()).await;
    let info = v.get("dataset").unwrap();
    let t0 = info.get("t_range").and_then(Value::as_list).unwrap()[0].as_f64().unwrap();
    let ask = |q: String| Request::get(format!("/api/v1/datasets/{key}/{q}")).body(Body::empty()).unwrap();
    let (t1, t2) = (t0 + 1.0, t0 + 2.0);
    // a channel asked for many times is answered once
    let (s, body, _) = call(&a, ask(format!("window?t0={t1}&t1={t2}&ch={}", vec!["0"; 3000].join(",")))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(json(&body).get("channels").and_then(Value::as_dict).unwrap().len(), 1);
    // bad requests: unknown channels, numbers that are not finite decimals, t1 not after t0,
    // a parameter given twice
    for q in [
        format!("window?t0={t1}&t1={t2}&ch=0,99"),
        format!("window?t0={t1}&t1={t2}&ch=-1"),
        format!("window?t0=-inf&t1={t2}&ch=0"),
        format!("window?t0=nan&t1={t2}&ch=0"),
        format!("window?t0=0x10&t1={t2}&ch=0"),
        format!("window?t0=1_0&t1={t2}&ch=0"),
        format!("window?t0=1e400&t1={t2}&ch=0"),
        format!("window?t0=%20{t1}&t1={t2}&ch=0"),
        format!("window?t0={t1}&t1={t2}&px=1.5&ch=0"),
        format!("window?t0={t1}&t0={t1}&t1={t2}&ch=0"),
        format!("window?t0={t2}&t1={t1}&ch=0"),
        format!("vitals?t0={t2}&t1={t1}&names=HR"),
        format!("vitals?t0={t1}&t1=inf&names=HR"),
        format!("markers?t0={t1}&t1=Infinity&ch=0"),
        "vitals_at?t=-inf".to_string(),
    ] {
        let (s, body, _) = call(&a, ask(q.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{q}: {}", String::from_utf8_lossy(&body));
    }
    // times far outside the recording: no data, no failure
    for q in ["window?t0=-1e308&t1=1e308&ch=0,1", "window?t0=1e300&t1=1.5e300&ch=0", "markers?t0=-1e308&t1=1e308&ch=0"]
    {
        let (s, body, _) = call(&a, ask(q.to_string())).await;
        assert_eq!(s, StatusCode::OK, "{q}: {}", String::from_utf8_lossy(&body));
    }
    let names = vec!["HR"; 2000].join("|");
    let (s, body, _) = call(&a, ask(format!("vitals?t0={t0}&t1={t2}&names={names}"))).await;
    assert_eq!((s, json(&body).as_dict().unwrap().len()), (StatusCode::OK, 1));
}

#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB records: a build without the `wfdb` feature refuses them")]
#[tokio::test]
async fn a_recording_that_cannot_be_indexed_is_an_error() {
    // index.csv without a record column: the state is "error", with a reason (in the home
    // folder, inside the root of a viewer built with `VITALSTREAM_ROOT='~'`, as CI's is)
    let dir = tempfile::tempdir_in(vitalstream_rs::settings::home()).unwrap();
    let c = dir.path().join("c");
    std::fs::create_dir_all(&c).unwrap();
    std::fs::write(c.join("index.csv"), "name\nx\n").unwrap();
    let a = app(false, &dir.path().join("auth.json"), &dir.path().join("cache"));
    let (_, v) = opened(&a, &c).await;
    assert_eq!(v.get("state").and_then(Value::as_str), Some("error"));
    assert!(v.get("error").unwrap().py_str().contains("no record column"));
    // nothing left behind in the index folder
    let left: Vec<_> = std::fs::read_dir(dir.path().join("cache")).unwrap().collect();
    assert!(left.is_empty());
}

#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB records: a build without the `wfdb` feature refuses them")]
#[cfg(unix)]
#[tokio::test]
async fn a_signal_file_outside_the_roots_is_refused() {
    let dir = tempfile::tempdir_in(vitalstream_rs::settings::home()).unwrap();
    let (root, outside) = (dir.path().join("root"), dir.path().join("outside"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.dat"), [1u8, 0, 2, 0]).unwrap();
    std::fs::write(root.join("r.hea"), "r 1 250 2\nr.dat 16 200/mV 16 0 0 3 0 II\n").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.dat"), root.join("r.dat")).unwrap();
    let a = app_with(false, &dir.path().join("auth.json"), &dir.path().join("cache"), |s| {
        s.data_roots = vec![root.to_string_lossy().to_string()];
    });
    let (s, body, _) = call(&a, open_request(&root.join("r.hea"))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{}", String::from_utf8_lossy(&body));
    assert!(!String::from_utf8_lossy(&body).contains("outside/"), "the target is not shown");
}

/// A build without the `wfdb` feature: a WFDB record, a multi-segment record, a folder of
/// records and a collection are listed, and refused when opened, with one message; an EDF file
/// opens as in the default build.
#[cfg(not(feature = "wfdb"))]
#[tokio::test]
async fn without_wfdb_a_wfdb_record_is_refused() {
    use recording_engine::sources::NO_WFDB;
    let dir = tempfile::tempdir().unwrap();
    let a = app(false, &dir.path().join("auth.json"), &dir.path().join("cache"));
    for name in ["plain/rec_a.hea", "session/ms/ms_rec.hea", "plain", "collection", "session"] {
        let (s, body, _) = call(&a, open_request(&testdata(name).canonicalize().unwrap())).await;
        assert_eq!(
            (s, json(&body).get("detail").map(Value::py_str)),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(NO_WFDB.into())),
            "{name}"
        );
        let scan = format!("/api/v1/scan?path={}", testdata(name).canonicalize().unwrap().display());
        let (s, body, _) = call(&a, Request::get(scan).body(Body::empty()).unwrap()).await;
        assert_eq!(
            (s, json(&body).get("detail").map(Value::py_str)),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(NO_WFDB.into())),
            "{name}"
        );
    }
    // still listed as a record, so that opening it says why it is not read
    let browse = format!("/api/v1/browse?path={}", testdata("plain").canonicalize().unwrap().display());
    let (s, body, _) = call(&a, Request::get(browse).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(json(&body).get("entries").and_then(Value::as_list).unwrap().iter().any(|e| e
        .get("type")
        .map(Value::py_str)
        .as_deref()
        == Some("record")));
    let (_, v) = opened(&a, &testdata("edf/psg_plus.edf").canonicalize().unwrap()).await;
    assert_eq!(v.get("state").and_then(Value::as_str), Some("ready"));
}
