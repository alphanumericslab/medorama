//! Tables on the server (Import › Add a database › Tables on this server): the folders listed
//! inside `data_roots` only, and never in Privacy Mode; a table inspected and imported where it
//! is, row by row.

mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use medorama::db::Db;
use medorama::users::Role;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Client {
    app: axum::Router,
}

impl Client {
    async fn send(&self, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut b = Request::builder().method(method).uri(uri).header("Cookie", cookie).header("X-Medorama", "1");
        if body.is_some() {
            b = b.header("Content-Type", "application/json");
        }
        let req = b.body(body.map_or(Body::empty(), |v| Body::from(v.to_string()))).unwrap();
        let r = self.app.clone().oneshot(req).await.unwrap();
        let status = r.status();
        let bytes = r.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn login(&self, user: &str) -> String {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/login")
            .header("X-Medorama", "1")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"username": user, "password": "0123456789ab"}).to_string()))
            .unwrap();
        let r = self.app.clone().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string()
    }
}

fn names(v: &Value) -> Vec<String> {
    v["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn tables_on_the_server() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    let data = root.join("data");
    let outside = root.join("outside");
    std::fs::create_dir_all(data.join("sub")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let mut csv = String::from("patient_id,last_name,first_name\n");
    for i in 0..10_000 {
        csv.push_str(&format!("P{i:05},Last{i},First{i}\n"));
    }
    std::fs::write(data.join("patients.csv"), &csv).unwrap();
    std::fs::write(data.join("notes.txt"), "not a table").unwrap();
    std::fs::write(data.join(".hidden.csv"), "patient_id\nP1\n").unwrap();
    std::fs::write(data.join("omop.sqlite"), "").unwrap();
    std::fs::write(outside.join("other.csv"), "patient_id\nQ1\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, data.join("away")).unwrap();
    cfg.data_roots = vec![data.to_string_lossy().to_string()];
    let db = std::sync::Arc::new(Db::open(&cfg, true).unwrap());
    {
        let c = db.get().unwrap();
        medorama::users::add(&c, "boss", "", Role::Admin, true, "0123456789ab").unwrap();
        medorama::users::add(&c, "reader", "", Role::Analyst, false, "0123456789ab").unwrap();
    }
    let app = medorama::server::router(std::sync::Arc::new(medorama::server::App::new(std::sync::Arc::new(cfg), db)));
    let cl = Client { app };
    let boss = cl.login("boss").await;
    let q = |p: &std::path::Path| urlencode(&p.to_string_lossy());

    // the data roots, then a root's folders and tables: no other file, no hidden one, no link out
    let (st, v) = cl.send("GET", "/api/v1/server-files", &boss, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["items"][0]["path"], data.to_string_lossy().as_ref());
    let (st, v) = cl.send("GET", &format!("/api/v1/server-files?dir={}", q(&data)), &boss, None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(names(&v), ["sub", "patients.csv"]);
    assert_eq!(v["items"][1]["size"], csv.len());
    assert_eq!(v["parent"], "", "above a root: the list of the roots");
    // the other kinds: SQLite files (a database to connect), folders alone (a CLIF folder)
    let list = |kind: &str| format!("/api/v1/server-files?dir={}&kind={kind}", q(&data));
    let (_, v) = cl.send("GET", &list("databases"), &boss, None).await;
    assert_eq!(names(&v), ["sub", "omop.sqlite"]);
    let (_, v) = cl.send("GET", &list("folders"), &boss, None).await;
    assert_eq!(names(&v), ["sub"]);
    let (st, _) = cl.send("GET", &list("pictures"), &boss, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, v) = cl.send("GET", &format!("/api/v1/server-files?dir={}", q(&data.join("sub"))), &boss, None).await;
    assert_eq!(v["parent"], data.to_string_lossy().as_ref());
    for dir in [outside.clone(), data.join("away"), data.join("..")] {
        let (st, _) = cl.send("GET", &format!("/api/v1/server-files?dir={}", q(&dir)), &boss, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{}", dir.display());
    }

    // inspected and imported where it is
    let table = data.join("patients.csv");
    let (st, v) =
        cl.send("POST", "/api/v1/tables/server/inspect", &boss, Some(json!({"path": table.to_string_lossy()}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["rows"], 10_000);
    assert_eq!(v["headers"], json!(["patient_id", "last_name", "first_name"]));
    assert_eq!(v["sample"].as_array().unwrap().len(), 5);
    assert_eq!(v["mapping"]["patient_id"], "patient_id");
    let body = |dry: bool| json!({"paths": [table.to_string_lossy()], "dry_run": dry});
    let (st, v) = cl.send("POST", "/api/v1/tables/server", &boss, Some(body(true))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["dry_run"].as_bool(), v["rows"].as_u64()), (Some(true), Some(10_000)));
    let (st, v) = cl.send("POST", "/api/v1/tables/server", &boss, Some(body(false))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["patients_added"], 10_000);
    let (_, v) = cl.send("GET", "/api/v1/imports", &boss, None).await;
    assert!(v.to_string().contains("patients.csv"), "{v}");

    // outside the roots, several files without a name, not a file: refused
    for b in [
        json!({"paths": [outside.join("other.csv").to_string_lossy()]}),
        json!({"paths": [data.join("away/other.csv").to_string_lossy()]}),
        json!({"paths": [table.to_string_lossy(), table.to_string_lossy()]}),
        json!({"paths": [data.join("sub").to_string_lossy()]}),
        json!({"paths": []}),
    ] {
        let (st, v) = cl.send("POST", "/api/v1/tables/server", &boss, Some(b.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{b}: {v}");
    }
    let (st, _) = cl
        .send(
            "POST",
            "/api/v1/tables/server/inspect",
            &boss,
            Some(json!({"path": outside.join("other.csv").to_string_lossy()})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // admins only; and no folder listed in Privacy Mode
    let reader = cl.login("reader").await;
    let (st, _) = cl.send("GET", &format!("/api/v1/server-files?dir={}", q(&data)), &reader, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, _) = cl.send("POST", "/api/v1/tables/server", &reader, Some(body(true))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    cl.send("POST", "/api/v1/privacy", &boss, Some(json!({"on": true}))).await;
    let (st, v) = cl.send("GET", &format!("/api/v1/server-files?dir={}", q(&data)), &boss, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(!v.to_string().contains("patients.csv"), "{v}");
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
