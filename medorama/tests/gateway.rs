//! Contract tests of the gateway with a real viewer behind it (docs/architecture.md, "The
//! gateway"): the catalog starts the viewer, passes allowed requests on and refuses the rest.
//!
//! The viewer: `$MEDORAMA_TEST_VIEWER`, else the first built of `target/release/vitalstream-rs`,
//! `viewers/vitalstream/cpp/build/viewer/bin/vitalstream-cpp` and
//! `viewers/vitalstream/cpp/build/release/bin/vitalstream-cpp`.  Without one, the test says so
//! and passes (CI builds the Rust viewer first, so there it always runs).

mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::opts;
use common::{bed_name, wfdb_record, T0};
use http_body_util::BodyExt;
use medorama::db::Db;
use medorama::recordings;
use medorama::server;
use medorama::users::{self, Role, User};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

fn test_viewer() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("MEDORAMA_TEST_VIEWER") {
        return Some(PathBuf::from(v));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let exe = if cfg!(windows) { ".exe" } else { "" };
    [
        root.join("target/release").join(format!("vitalstream-rs{exe}")),
        root.join("viewers/vitalstream/cpp/build/viewer/bin").join(format!("vitalstream-cpp{exe}")),
        root.join("viewers/vitalstream/cpp/build/release/bin").join(format!("vitalstream-cpp{exe}")),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

struct Call {
    status: StatusCode,
    json: serde_json::Value,
    text: String,
    location: Option<String>,
    cookie: Option<String>,
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> Call {
    let mut req = Request::builder().method(method).uri(uri);
    // as the pages do: every changing request carries X-Medorama, the viewer's through the gateway too
    if method != "GET" && (uri.starts_with("/api/") || uri.starts_with("/viewer/api/")) {
        req = req.header("X-Medorama", "1");
    }
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    let req = match body {
        Some(b) => req.header("Content-Type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let location = resp.headers().get("location").map(|v| v.to_str().unwrap().to_string());
    let cookie = resp.headers().get("set-cookie").map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).to_string();
    Call { status, json: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null), text, location, cookie }
}

fn q(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn viewer_behind_the_gateway() {
    let Some(viewer) = test_viewer() else {
        eprintln!("no viewer built (target/release/vitalstream-rs or the C++ viewer): gateway test skipped");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let data = root.join("data");
    let in_catalog = data.join("ward").join(bed_name("ICU7", "12", T0));
    let not_in_catalog = data.join("other").join(bed_name("ICU8", "1", T0));
    let outside = root.join("elsewhere").join(bed_name("ICU9", "2", T0));
    wfdb_record(&in_catalog, 0);
    wfdb_record(&not_in_catalog, 0);
    wfdb_record(&outside, 0);

    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    cfg.data_roots = vec![data.to_string_lossy().to_string()];
    cfg.viewer = viewer.to_string_lossy().to_string();
    let db = Db::open(&cfg, true).unwrap();
    {
        let mut c = db.get().unwrap();
        users::add(&c, "boss", "", Role::Admin, true, "0123456789ab").unwrap();
        // a non-admin with the PHI permission (Privacy Mode off, below), and one without it
        users::add(&c, "vic", "", Role::Viewer, true, "0123456789ab").unwrap();
        users::add(&c, "pat", "", Role::Viewer, false, "0123456789ab").unwrap();
        recordings::import(
            &mut c,
            &cfg,
            &User::local(),
            &[in_catalog.to_string_lossy().to_string()],
            false,
            &opts("test"),
        )
        .unwrap();
    }
    let app = Arc::new(server::App::new(Arc::new(cfg.clone()), Arc::new(db)));
    let router = server::router(app.clone());
    let login = |u: &str| serde_json::json!({"username": u, "password": "0123456789ab"});
    let vic = call(&router, "POST", "/api/v1/login", None, Some(login("vic"))).await.cookie.unwrap();
    // Privacy Mode off for this session (kept by the server, not the browser)
    let off = call(&router, "POST", "/api/v1/privacy", Some(&vic), Some(serde_json::json!({"on": false}))).await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.text);
    let pat = call(&router, "POST", "/api/v1/login", None, Some(login("pat"))).await.cookie.unwrap();
    let boss = call(&router, "POST", "/api/v1/login", None, Some(login("boss"))).await.cookie.unwrap();

    // signed out: the page sends to the catalog's sign-in, the API refuses
    let r = call(&router, "GET", "/viewer/", None, None).await;
    assert_eq!((r.status, r.location.as_deref()), (StatusCode::SEE_OTHER, Some("../")));
    assert_eq!(call(&router, "GET", "/viewer/api/v1/config", None, None).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&router, "GET", "/viewer", Some(&vic), None).await.location.as_deref(), Some("viewer/"));

    // the status, then the page (the viewer starts on first use)
    let s = call(&router, "GET", "/api/v1/viewer", Some(&vic), None).await;
    assert_eq!(s.json["mode"], "embedded", "{:?}", s.json);
    let page = call(&router, "GET", "/viewer/", Some(&vic), None).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    assert!(page.text.contains("static/js/app.js"), "the viewer's page, signed in by the gateway");
    assert!(call(&router, "GET", "/api/v1/viewer", Some(&vic), None).await.json["running"].as_bool().unwrap());
    assert_eq!(call(&router, "GET", "/viewer/static/style.css", Some(&vic), None).await.status, StatusCode::OK);

    // a non-admin's Open dialog shows only the catalog's recordings
    let cfgv = call(&router, "GET", "/viewer/api/v1/config", Some(&vic), None).await.json;
    assert_eq!(cfgv["places"][0]["path"], in_catalog.parent().unwrap().to_string_lossy().as_ref());
    let b =
        call(&router, "GET", &format!("/viewer/api/v1/browse?path={}", q(&data.to_string_lossy())), Some(&vic), None)
            .await;
    assert_eq!(b.status, StatusCode::OK, "{}", b.text);
    let names: Vec<&str> = b.json["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["ward"], "only the folder leading to a catalog recording");
    let other = data.join("other");
    assert_eq!(
        call(&router, "GET", &format!("/viewer/api/v1/browse?path={}", q(&other.to_string_lossy())), Some(&vic), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let comp = call(
        &router,
        "GET",
        &format!("/viewer/api/v1/complete?prefix={}", q(&format!("{}/", data.display()))),
        Some(&vic),
        None,
    )
    .await;
    let items: Vec<String> =
        comp.json["items"].as_array().unwrap().iter().map(|i| i["path"].as_str().unwrap().to_string()).collect();
    assert!(items.iter().all(|p| p.contains("ward")), "{items:?}");

    // opening: catalog recordings only, for a non-admin
    let open = |p: &Path| serde_json::json!({"path": p.to_string_lossy()});
    assert_eq!(
        call(&router, "POST", "/viewer/api/v1/open", Some(&vic), Some(open(&not_in_catalog))).await.status,
        StatusCode::FORBIDDEN
    );
    let o = call(&router, "POST", "/viewer/api/v1/open", Some(&vic), Some(open(&in_catalog))).await;
    assert_eq!(o.status, StatusCode::OK, "{}", o.text);
    let key = o.json["key"].as_str().unwrap().to_string();
    let mut described = serde_json::Value::Null;
    for _ in 0..100 {
        described = call(&router, "GET", &format!("/viewer/api/v1/datasets/{key}"), Some(&vic), None).await.json;
        if described["state"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(described["state"], "ready", "{described:?}");
    // the same index the catalog would use (docs/index-format.md; not the Python viewer)
    if !viewer.to_string_lossy().contains("vitalstream-py") {
        assert_eq!(key, recording_engine::index::cache_key(&in_catalog));
        assert!(cfg.index_dir.join(&key).join("meta.json").is_file());
    }
    let w = call(
        &router,
        "GET",
        &format!("/viewer/api/v1/datasets/{key}/window?t0={T0}&t1={}&ch=0", T0 + 10),
        Some(&vic),
        None,
    )
    .await;
    assert_eq!(w.status, StatusCode::OK, "{}", w.text);
    // datasets only after opening them through the gateway, per user
    assert_eq!(
        call(&router, "GET", &format!("/viewer/api/v1/datasets/{key}"), Some(&boss), None).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, "GET", "/viewer/api/v1/datasets/0123456789abcdef0123", Some(&vic), None).await.status,
        StatusCode::FORBIDDEN
    );

    // Privacy Mode (a user without the PHI permission): no file system; recordings by token;
    // the answers' path and times replaced; the page's (moved) times translated back
    let cfgp = call(&router, "GET", "/viewer/api/v1/config", Some(&pat), None).await.json;
    assert_eq!((cfgp["places"].as_array().map(Vec::len), cfgp["home"].as_str()), (Some(0), Some("")), "{cfgp:?}");
    let b =
        call(&router, "GET", &format!("/viewer/api/v1/browse?path={}", q(&data.to_string_lossy())), Some(&pat), None)
            .await;
    assert_eq!(b.status, StatusCode::FORBIDDEN);
    assert_eq!(
        call(&router, "POST", "/viewer/api/v1/open", Some(&pat), Some(open(&in_catalog))).await.status,
        StatusCode::FORBIDDEN,
        "a path, in Privacy Mode"
    );
    let rec: i64 = app.db.get().unwrap().query_row("SELECT id FROM recordings", [], |r| r.get(0)).unwrap();
    let o = call(
        &router,
        "POST",
        "/viewer/api/v1/open",
        Some(&pat),
        Some(serde_json::json!({"path": format!("mo-rec:{rec}")})),
    )
    .await;
    assert_eq!(o.status, StatusCode::OK, "{}", o.text);
    let pkey = o.json["key"].as_str().unwrap().to_string();
    let mut d = serde_json::Value::Null;
    for _ in 0..100 {
        d = call(&router, "GET", &format!("/viewer/api/v1/datasets/{pkey}"), Some(&pat), None).await.json;
        if d["state"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let ds = &d["dataset"];
    assert_eq!(ds["path"], format!("mo-rec:{rec}"));
    assert!(!d.to_string().contains(&in_catalog.to_string_lossy().to_string()), "{d}");
    let shown_t0 = ds["t_range"][0].as_f64().unwrap();
    assert!(shown_t0 < T0 as f64 - 3.0e9, "moved back: {shown_t0}");
    let w = call(
        &router,
        "GET",
        &format!("/viewer/api/v1/datasets/{pkey}/window?t0={shown_t0}&t1={}&ch=0", shown_t0 + 10.0),
        Some(&pat),
        None,
    )
    .await;
    assert_eq!(w.status, StatusCode::OK, "{}", w.text);
    assert!(
        w.text.contains("\"t0\":") && !w.text.contains(&T0.to_string()[..6]),
        "the samples, at the moved times: {}",
        w.text
    );

    // admins: anything inside data_roots, nothing outside
    assert_eq!(
        call(&router, "POST", "/viewer/api/v1/open", Some(&boss), Some(open(&not_in_catalog))).await.status,
        StatusCode::OK
    );
    assert_eq!(
        call(&router, "POST", "/viewer/api/v1/open", Some(&boss), Some(open(&outside))).await.status,
        StatusCode::FORBIDDEN
    );
    let b =
        call(&router, "GET", &format!("/viewer/api/v1/browse?path={}", q(&data.to_string_lossy())), Some(&boss), None)
            .await;
    assert_eq!(b.json["entries"].as_array().unwrap().len(), 2);

    // default deny: the viewer's own sign-in and anything unknown
    assert_eq!(
        call(&router, "POST", "/viewer/login", Some(&vic), Some(serde_json::json!({}))).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(call(&router, "GET", "/viewer/api/v1/nothing", Some(&vic), None).await.status, StatusCode::NOT_FOUND);
    let plain = Request::builder()
        .method("POST")
        .uri("/viewer/api/v1/open")
        .header("Cookie", &vic)
        .header("X-Medorama", "1")
        .header("Content-Type", "text/plain")
        .body(Body::from(open(&in_catalog).to_string()))
        .unwrap();
    assert_eq!(router.clone().oneshot(plain).await.unwrap().status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    // a change through the gateway carries X-Medorama, as every changing API request does
    let bare = Request::builder()
        .method("POST")
        .uri("/viewer/api/v1/open")
        .header("Cookie", &vic)
        .header("Content-Type", "application/json")
        .body(Body::from(open(&in_catalog).to_string()))
        .unwrap();
    assert_eq!(router.clone().oneshot(bare).await.unwrap().status(), StatusCode::FORBIDDEN);

    // framing by the catalog only
    let r = router
        .clone()
        .oneshot(Request::builder().uri("/viewer/").header("Cookie", &vic).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(r.headers()["x-frame-options"], "SAMEORIGIN");

    // views are audited
    let views: i64 =
        app.db.get().unwrap().query_row("SELECT COUNT(*) FROM audit WHERE event = 'view'", [], |r| r.get(0)).unwrap();
    assert_eq!(views, 3);

    // the viewer stops with the catalog
    app.viewer.stop();
    assert!(!app.viewer.status().running);
}
