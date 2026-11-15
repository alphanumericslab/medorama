//! The web interface (`db-builder serve`, src/serve/): its sign-in, its CSRF header, its root
//! folder (nothing above it browsed, read or written), the preview that writes nothing, and the
//! commands through the API.  Invented rows only.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use db_builder::serve::{self, auth, Settings};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

const PASSWORD: &str = "a long test password";

struct Fx {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
    app: axum::Router,
}

impl Fx {
    fn new() -> Fx {
        Fx::with_key(None)
    }

    /// A server whose databases are encrypted with `key` (`--encrypt`), or not.
    fn with_key(key: Option<&str>) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let (root, home, outside) = (base.join("root"), base.join("home"), base.join("outside"));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.csv"), "patient_id,last_name\nP1,Outside\n").unwrap();
        auth::set_password(&home, "admin", PASSWORD).unwrap();
        let s = Settings {
            root: root.clone(),
            home,
            key: key.map(str::to_string),
            port: 8091,
            secure_cookies: false,
            idle: std::time::Duration::from_secs(600),
        };
        let app = serve::router(Arc::new(serve::App::new(&s).unwrap()));
        Fx { _tmp: tmp, root, outside, app }
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }

    /// A request: with the session's cookie, and the CSRF header unless `csrf` is false.
    async fn send(
        &self,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        csrf: bool,
    ) -> (StatusCode, Value, Option<String>) {
        let mut b = Request::builder().method(method).uri(uri);
        if csrf {
            b = b.header("X-DB-Builder", "1");
        }
        if let Some(c) = cookie {
            b = b.header("Cookie", c);
        }
        let req = match body {
            Some(v) => b.header("Content-Type", "application/json").body(Body::from(v.to_string())).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let r = self.app.clone().oneshot(req).await.unwrap();
        let status = r.status();
        let cookie = r.headers().get("set-cookie").map(|v| v.to_str().unwrap().to_string());
        let bytes = r.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie)
    }

    async fn call(&self, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (s, v, _) = self.send(method, uri, Some(cookie), body, true).await;
        (s, v)
    }

    /// Signed in: the session's cookie (`name=token`).
    async fn sign_in(&self) -> String {
        let (s, _, c) = self
            .send("POST", "/api/login", None, Some(json!({ "username": "admin", "password": PASSWORD })), true)
            .await;
        assert_eq!(s, StatusCode::OK);
        let c = c.expect("a cookie");
        assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict"), "{c}");
        c.split(';').next().unwrap().to_string()
    }

    /// A task run to its end: its state.
    async fn finish(&self, cookie: &str, task: &Value) -> Value {
        let id = task["task"].as_u64().expect("a task id");
        for _ in 0..600 {
            let (s, t) = self.call("GET", &format!("/api/tasks/{id}"), cookie, None).await;
            assert_eq!(s, StatusCode::OK);
            if t["state"] == "done" || t["state"] == "failed" {
                return t;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the task did not end");
    }

    async fn new_db(&self, cookie: &str, path: &str) -> String {
        let (s, v) =
            self.call("POST", "/api/databases", cookie, Some(json!({ "path": path, "timezone": "UTC" }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["path"].as_str().unwrap().to_string()
    }
}

const PATIENTS: &str = "patient_id,issuer,last_name,first_name,sex,date_of_birth\nP900001,HOSP,Doe,Jane,F,1970-01-02\nP900002,HOSP,Roe,Rick,M,1965-03-04\n";

#[tokio::test]
async fn sign_in_is_required_for_everything_but_the_page() {
    let f = Fx::new();
    let (s, _, _) = f.send("GET", "/", None, None, false).await;
    assert_eq!(s, StatusCode::OK, "the page (it holds no data)");
    let (s, _, _) = f.send("GET", "/static/app.js", None, None, false).await;
    assert_eq!(s, StatusCode::OK);
    for uri in [
        "/api/me",
        "/api/files",
        "/api/databases",
        "/api/info?db=a.sqlite",
        "/api/rows?db=a.sqlite&table=persons",
        "/api/formats",
        "/api/tasks/1",
        "/api/rules?db=a.sqlite",
    ] {
        let (s, _, _) = f.send("GET", uri, None, None, false).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "GET {uri}");
    }
    for uri in [
        "/api/databases",
        "/api/import",
        "/api/recordings",
        "/api/rewrite",
        "/api/check",
        "/api/remove-source",
        "/api/copy",
    ] {
        let (s, _, _) = f.send("POST", uri, None, Some(json!({ "db": "a.sqlite", "path": "a" })), true).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "POST {uri}");
    }
    let (s, _, _) = f.send("PUT", "/api/rules", None, Some(json!({ "db": "a.sqlite", "text": "" })), true).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // a cookie that names no session
    let (s, _, _) = f.send("GET", "/api/me", Some("db_builder_8091=0123"), None, false).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_wrong_password_is_refused_and_sign_out_ends_the_session() {
    let f = Fx::new();
    for (u, p) in [("admin", "not the password"), ("someone", PASSWORD)] {
        let (s, _, c) = f.send("POST", "/api/login", None, Some(json!({ "username": u, "password": p })), true).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{u}");
        assert!(c.is_none());
    }
    let c = f.sign_in().await;
    let (s, me) = f.call("GET", "/api/me", &c, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["user"], "admin");
    assert_eq!(me["root"], f.root.to_string_lossy().as_ref());
    f.call("POST", "/api/logout", &c, None).await;
    assert_eq!(f.call("GET", "/api/me", &c, None).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn changes_need_the_csrf_header() {
    let f = Fx::new();
    let (s, _, _) =
        f.send("POST", "/api/login", None, Some(json!({ "username": "admin", "password": PASSWORD })), false).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "sign-in too");
    let c = f.sign_in().await;
    let (s, v, _) = f.send("POST", "/api/databases", Some(&c), Some(json!({ "path": "x.sqlite" })), false).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(!f.root.join("x.sqlite").exists());
    // reads need none
    let (s, _, _) = f.send("GET", "/api/databases", Some(&c), None, false).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn paths_outside_the_root_are_refused() {
    let f = Fx::new();
    let c = f.sign_in().await;
    let db = f.new_db(&c, "data/study").await;
    assert_eq!(db, "data/study.sqlite");
    let outside = f.outside.join("secret.csv").to_string_lossy().to_string();
    let elsewhere = f.outside.join("x.sqlite").to_string_lossy().to_string();
    for dir in ["..", "../outside", "data/../..", "/", &f.outside.to_string_lossy()] {
        let (s, _) = f.call("GET", &format!("/api/files?dir={}", urlencode(dir)), &c, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "files {dir}");
    }
    for p in ["../evil", &elsewhere, "missing/x.sqlite"] {
        let (s, _) = f.call("POST", "/api/databases", &c, Some(json!({ "path": p }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "new {p}");
    }
    assert!(!f.outside.join("x.sqlite").exists() && !f.root.parent().unwrap().join("evil.sqlite").exists());
    let (s, _) = f.call("GET", &format!("/api/info?db={}", urlencode("../root/data/study.sqlite")), &c, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    for input in ["../outside/secret.csv", outside.as_str()] {
        let (s, v) =
            f.call("POST", "/api/import", &c, Some(json!({ "db": db, "inputs": [input], "dry_run": true }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "import {input}: {v}");
    }
    let (s, _) =
        f.call("POST", "/api/recordings", &c, Some(json!({ "db": db, "folder": "..", "pattern": "{patient}*" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = f
        .call(
            "POST",
            "/api/import",
            &c,
            Some(json!({ "db": db, "inputs": ["data"], "options": { "mapping": outside } })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a mapping file outside");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.outside.join("secret.csv"), f.root.join("data/link.csv")).unwrap();
        std::os::unix::fs::symlink(&f.outside, f.root.join("away")).unwrap();
        for input in ["data/link.csv", "away/secret.csv", "away"] {
            let (s, v) =
                f.call("POST", "/api/import", &c, Some(json!({ "db": db, "inputs": [input], "dry_run": true }))).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "import {input}: {v}");
        }
        let (s, _) = f.call("GET", "/api/files?dir=away", &c, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (_, v) = f.call("GET", "/api/files?dir=data", &c, None).await;
        let names: Vec<&str> = v["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["study.sqlite"], "the link out is not listed");
        let (s, _) = f.call("POST", "/api/copy", &c, Some(json!({ "db": db, "to": "away/copy.sqlite" }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(!f.outside.join("copy.sqlite").exists());
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn a_preview_writes_nothing() {
    let f = Fx::new();
    let c = f.sign_in().await;
    let db = f.new_db(&c, "study.sqlite").await;
    f.write("in/patients.csv", PATIENTS);
    f.write("in/labs.csv", "patient_id,issuer,time,name,value,unit,Lactate [mmol/L],study_arm\nP900001,HOSP,2024-03-02 07:10,Sodium,140,mmol/L,1.9,B\n");
    let before = std::fs::read(f.root.join(&db)).unwrap();
    let t = f
        .call(
            "POST",
            "/api/import",
            &c,
            Some(json!({ "db": db, "inputs": ["in/patients.csv", "in/labs.csv"], "dry_run": true })),
        )
        .await
        .1;
    let t = f.finish(&c, &t).await;
    assert_eq!(t["state"], "done", "{t}");
    assert_eq!(t["dry_run"], true);
    let r = &t["reports"];
    assert_eq!(r[0]["file"], "in/patients.csv");
    assert_eq!(r[0]["rows"], 2);
    assert_eq!(r[0]["persons_added"], 2);
    let cols = r[1]["columns"].as_array().unwrap();
    let how = |h: &str| {
        cols.iter()
            .find(|c| c["header"] == h)
            .map(|c| (c["how"].as_str().unwrap().to_string(), c["field"].as_str().unwrap().to_string()))
    };
    assert_eq!(how("time"), Some(("field".into(), "time".into())));
    assert_eq!(how("Lactate [mmol/L]"), Some(("measure".into(), "Lactate".into())));
    assert_eq!(how("study_arm"), Some(("extra".into(), "".into())));
    assert_eq!(r[1]["sample"][0][0], "P900001");
    assert_eq!(r[1]["entries"], 2);
    assert_eq!(std::fs::read(f.root.join(&db)).unwrap(), before, "the database's bytes are unchanged");
    let (_, info) = f.call("GET", &format!("/api/info?db={db}"), &c, None).await;
    assert_eq!(info["counts"]["persons"], 0);
    assert_eq!(info["sources"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn import_browse_remove_and_copy() {
    let f = Fx::new();
    let c = f.sign_in().await;
    let db = f.new_db(&c, "study.sqlite").await;
    f.write("in/patients.csv", PATIENTS);
    // rules applied to every add from the page: refused when not valid, saved when valid
    let (s, _) =
        f.call("PUT", "/api/rules", &c, Some(json!({ "db": db, "text": "[mrn]\ncase = \"sideways\"\n" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(!f.root.join("study.rules.toml").exists());
    let rules = "[patient_id]\nreplace = [{ regex = '^P9', to = 'Q9' }]\n";
    let (s, v) = f.call("PUT", "/api/rules", &c, Some(json!({ "db": db, "text": rules }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(std::fs::read_to_string(f.root.join("study.rules.toml")).unwrap(), rules);
    let t = f.call("POST", "/api/import", &c, Some(json!({ "db": db, "inputs": ["in/patients.csv"] }))).await.1;
    let t = f.finish(&c, &t).await;
    assert_eq!(t["state"], "done", "{t}");
    assert_eq!(t["reports"][0]["source"], 1);
    // browsed a page at a time, filtered
    let (s, p) = f.call("GET", &format!("/api/rows?db={db}&table=persons"), &c, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["total"], 2);
    let pid = p["columns"].as_array().unwrap().iter().position(|c| c == "patient_id").unwrap();
    assert_eq!(p["rows"][0][pid], "Q900001", "the rules applied");
    let (_, p) = f.call("GET", &format!("/api/rows?db={db}&table=persons&q=roe"), &c, None).await;
    assert_eq!(p["total"], 1);
    let (_, p) = f.call("GET", &format!("/api/rows?db={db}&table=persons&q=%25"), &c, None).await;
    assert_eq!(p["total"], 0, "a % typed is a %");
    let (s, _) = f.call("GET", &format!("/api/rows?db={db}&table=meta"), &c, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "not a table shown");
    let (_, chk) = f.call("POST", "/api/check", &c, Some(json!({ "db": db }))).await;
    assert!(chk["problems"].as_array().unwrap().is_empty(), "{chk}");
    // a copy inside the root; refused outside it, or over a file
    let (s, v) = f.call("POST", "/api/copy", &c, Some(json!({ "db": db, "to": "data/copy" }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["path"], "data/copy.sqlite");
    let (_, p) = f.call("GET", "/api/rows?db=data/copy.sqlite&table=persons", &c, None).await;
    assert_eq!(p["total"], 2);
    for to in ["../copy.sqlite", f.outside.join("copy.sqlite").to_str().unwrap(), "data/copy.sqlite"] {
        let (s, _) = f.call("POST", "/api/copy", &c, Some(json!({ "db": db, "to": to }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "copy to {to}");
    }
    assert!(!f.outside.join("copy.sqlite").exists() && !f.root.parent().unwrap().join("copy.sqlite").exists());
    // the databases of the root
    let (_, l) = f.call("GET", "/api/databases", &c, None).await;
    let paths: Vec<&str> = l["items"].as_array().unwrap().iter().map(|i| i["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["data/copy.sqlite", "study.sqlite"]);
    // a source removed
    let (s, v) = f.call("POST", "/api/remove-source", &c, Some(json!({ "db": db, "ids": [1] }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["removed"][0]["persons"], 2);
    let (_, p) = f.call("GET", &format!("/api/rows?db={db}&table=persons"), &c, None).await;
    assert_eq!(p["total"], 0);
    let (s, _) = f.call("POST", "/api/remove-source", &c, Some(json!({ "db": db, "ids": [1] }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "no such source");
}

#[tokio::test]
async fn recordings_by_pattern_and_rewrite() {
    let f = Fx::new();
    let c = f.sign_in().await;
    let db = f.new_db(&c, "study.sqlite").await;
    f.write("psg/P900001_night.edf", &edf_header());
    let body = json!({ "db": db, "folder": "psg", "pattern": "{patient}_*.edf", "options": { "issuer": "HOSP", "category": "sleep" } });
    let mut preview = body.clone();
    preview["dry_run"] = json!(true);
    let t = f.call("POST", "/api/recordings", &c, Some(preview)).await.1;
    let t = f.finish(&c, &t).await;
    assert_eq!(t["state"], "done", "{t}");
    assert_eq!(t["reports"][0]["rows"], 1);
    assert_eq!(t["reports"][0]["sample"][0], json!(["P900001_night.edf", "P900001", ""]));
    let (_, info) = f.call("GET", &format!("/api/info?db={db}"), &c, None).await;
    assert_eq!(info["counts"]["recordings"], 0, "the preview wrote nothing");
    let t = f.call("POST", "/api/recordings", &c, Some(body)).await.1;
    assert_eq!(f.finish(&c, &t).await["state"], "done");
    let (_, info) = f.call("GET", &format!("/api/info?db={db}"), &c, None).await;
    assert_eq!(info["counts"]["recordings"], 1);
    // the paths rewritten by the rules' [[path]] rules: counted, then written
    let (s, _) = f.call("POST", "/api/rewrite", &c, Some(json!({ "db": db, "dry_run": true }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "no rules file yet");
    let rules = "[[path]]\nprefix = \"psg\"\nto = \"moved\"\n";
    assert_eq!(f.call("PUT", "/api/rules", &c, Some(json!({ "db": db, "text": rules }))).await.0, StatusCode::OK);
    let (s, n) = f.call("POST", "/api/rewrite", &c, Some(json!({ "db": db, "dry_run": true }))).await;
    assert_eq!(s, StatusCode::OK, "{n}");
    assert_eq!(n["recordings"], 1);
    let (_, p) = f.call("GET", &format!("/api/rows?db={db}&table=recordings"), &c, None).await;
    let file = p["columns"].as_array().unwrap().iter().position(|c| c == "file").unwrap();
    assert_eq!(p["rows"][0][file], "psg/P900001_night.edf", "a dry run");
    f.call("POST", "/api/rewrite", &c, Some(json!({ "db": db }))).await;
    let (_, p) = f.call("GET", &format!("/api/rows?db={db}&table=recordings"), &c, None).await;
    assert_eq!(p["rows"][0][file], "moved/P900001_night.edf");
}

/// An EDF file's header, no data records (a recording as a folder of them lists it).
fn edf_header() -> String {
    let field = |s: &str, n: usize| format!("{s:<n$}");
    let mut h = String::new();
    h += &field("0", 8);
    h += &field("X X X X", 80);
    h += &field("Startdate 02-MAR-2024 X X X", 80);
    h += &field("02.03.24", 8);
    h += &field("22.00.00", 8);
    h += &field("512", 8);
    h += &field("", 44);
    h += &field("0", 8);
    h += &field("1", 8);
    h += &field("1", 4);
    h += &field("EEG", 16);
    h += &field("", 80);
    h += &field("uV", 8);
    h += &field("-100", 8);
    h += &field("100", 8);
    h += &field("-2048", 8);
    h += &field("2047", 8);
    h += &field("", 80);
    h += &field("1", 8);
    h += &field("", 32);
    h
}

#[test]
fn serve_refuses_to_start_without_a_password() {
    let tmp = tempfile::tempdir().unwrap();
    let s = Settings {
        root: tmp.path().to_path_buf(),
        home: tmp.path().join("home"),
        key: None,
        port: 8091,
        secure_cookies: false,
        idle: std::time::Duration::from_secs(60),
    };
    let e = serve::App::new(&s).err().expect("refused").to_string();
    assert!(e.contains("--set-password"), "{e}");
}

#[tokio::test]
async fn encrypted_databases_need_the_key() {
    let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let f = Fx::with_key(Some(key));
    let c = f.sign_in().await;
    assert_eq!(f.call("GET", "/api/me", &c, None).await.1["encrypted"], true);
    let db = f.new_db(&c, "secret.sqlite").await;
    assert!(!std::fs::read(f.root.join(&db)).unwrap().starts_with(b"SQLite format 3"), "made encrypted");
    let (s, info) = f.call("GET", &format!("/api/info?db={db}"), &c, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(info["encrypted"], true);
    // a server without the key says how to open it
    let plain = Fx::new();
    std::fs::copy(f.root.join(&db), plain.root.join("secret.sqlite")).unwrap();
    let c = plain.sign_in().await;
    let (s, v) = plain.call("GET", "/api/info?db=secret.sqlite", &c, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let why = v["detail"].as_str().unwrap();
    assert!(
        why.contains("encrypted") && why.contains("--encrypt") && !why.contains(&*plain.root.to_string_lossy()),
        "{why}"
    );
}
