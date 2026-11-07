//! Privacy Mode's searches (docs/security.md, "The privacy gate"; search.rs): a session in
//! Privacy Mode may search, filter and sort only by what it is shown.  A catalog holds
//! identifiers that cannot appear by chance; a user without the PHI permission searches for each
//! of them, and for the real dates, in every list (encounters, recordings, every database's rows,
//! cohorts): nothing may be found.  The same searches by what the session is shown (pseudonyms,
//! tokens, dates as shown) find the patient: the test would prove nothing otherwise.

mod common;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{bed_name, wfdb_record};
use http_body_util::BodyExt;
use medorama::config::Config;
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::users::{Role, User};
use serde_json::{json, Value};
use std::collections::HashMap;
use tower::ServiceExt;

/// Searched for, and never found, in Privacy Mode.
const CANARIES: &[&str] = &[
    "Zyxwortha",   // last name (in a note)
    "Quilbert",    // first name
    "MRN83651927", // MRN (in a note)
    "PIDX77214",   // imported patient ID
    "ENC55102",    // imported encounter ID
    "CSN99183",    // account number
    "1931",        // the year of birth (in a note)
    "Osei",        // staff
    "2024-01-0",   // real dates (the stay: 2024-01-01 to 2024-01-05)
    "1704110400",  // the recording's real start, as its name holds it
    "icu7_12",     // and its name
    "/icu/",       // and its path
];

struct Fx {
    _tmp: tempfile::TempDir,
    cfg: Config,
    db: Db,
}

fn fixture() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    let data = root.join("data");
    cfg.data_roots = vec![data.to_string_lossy().to_string()];
    std::fs::create_dir_all(data.join("ecg")).unwrap();
    let db = Db::open(&cfg, true).unwrap();
    let mut c = db.get().unwrap();
    let admin = User::local();
    let import = |c: &mut medorama::sql::Connection, name: &str, text: &str, category: &str| {
        medorama::tables::import(
            c,
            &cfg,
            &admin,
            name,
            text.as_bytes(),
            &HashMap::new(),
            category,
            Some(&data),
            &Opts::new(name, "", false),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    std::fs::write(
        data.join("ecg/PIDX77214_ecg.hea"),
        "PIDX77214_ecg 2 500 1000 08:00:00 02/01/2024\nPIDX77214_ecg.dat 16 200/mV 16 0 0 0 0 I\nPIDX77214_ecg.dat 16 200/mV 16 0 0 0 0 II\n",
    )
    .unwrap();
    std::fs::write(data.join("ecg/PIDX77214_ecg.dat"), vec![0u8; 4000]).unwrap();
    let rec = data.join("icu").join(bed_name("ICU7", "12", 1_704_110_400));
    wfdb_record(&rec, 0);
    let rel = rec.strip_prefix(&data).unwrap().to_string_lossy().to_string();
    import(
        &mut c,
        "patients.csv",
        "MRN,Last Name,First Name,DOB,Sex,Patient ID\nMRN83651927,Zyxwortha,Quilbert,1931-05-17,F,PIDX77214\n",
        "",
    );
    import(
        &mut c,
        "encounters.csv",
        "Encounter ID,Patient ID,CSN,Admit Time,Discharge Time,Admit Unit,Care Unit,Bed,Bed Start,Bed End\n\
ENC55102,PIDX77214,CSN99183,2024-01-01 10:00,2024-01-05 10:00,Cardiology,ICU7,12,2024-01-01 11:00,2024-01-03 11:00\n",
        "",
    );
    import(
        &mut c,
        "labs.csv",
        "MRN,Encounter ID,Test Name,Result,Units,Collected\nMRN83651927,ENC55102,Lactate,4.1,mmol/L,2024-01-02 06:10\n",
        "",
    );
    import(
        &mut c,
        "notes.csv",
        "patient_id,encounter_id,time,note_type,author,note_text\n\
PIDX77214,ENC55102,2024-01-02 15:40,ICU progress note,Dr. Kwame Osei,\"Zyxwortha, Quilbert (MRN83651927), born 1931-05-17: septic shock, improving.\"\n",
        "",
    );
    import(
        &mut c,
        "ecg_index.csv",
        "patient_id,encounter_id,time,interpretation,file\nPIDX77214,ENC55102,2024-01-02 08:00,Sinus rhythm,ecg/PIDX77214_ecg.hea\n",
        "ecg",
    );
    import(&mut c, "icu_index.csv", &format!("patient_id,encounter_id,file\nPIDX77214,ENC55102,{rel}\n"), "");
    medorama::users::add(&c, "boss", "", Role::Admin, true, "0123456789ab").unwrap();
    medorama::users::add(&c, "reader", "", Role::Analyst, false, "0123456789ab").unwrap();
    drop(c);
    Fx { _tmp: tmp, cfg, db }
}

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

    async fn get(&self, uri: &str, cookie: &str) -> Value {
        let (st, v) = self.send("GET", &format!("/api/v1/{uri}"), cookie, None).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {v}");
        v
    }

    /// The number of matches a list answers.
    async fn total(&self, uri: &str, cookie: &str) -> i64 {
        self.get(uri, cookie).await["total"].as_i64().unwrap_or_else(|| panic!("{uri}: no total"))
    }

    async fn login(&self, user: &str) -> String {
        let r = Request::builder()
            .method("POST")
            .uri("/api/v1/login")
            .header("X-Medorama", "1")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({"username": user, "password": "0123456789ab"}).to_string()))
            .unwrap();
        let r = self.app.clone().oneshot(r).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string()
    }
}

fn app_of(f: &Fx) -> Client {
    let app = std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(f.cfg.clone()),
        std::sync::Arc::new(Db::open(&f.cfg, false).unwrap()),
    ));
    Client { app: medorama::server::router(app) }
}

fn enc(s: &str) -> String {
    s.replace('%', "%25").replace(' ', "%20").replace(':', "%3A").replace('/', "%2F").replace('"', "%22")
}

/// A time (Unix seconds) as the page writes a date: YYYY-MM-DD, in the catalog's zone (UTC).
fn date(t: f64) -> String {
    chrono::DateTime::from_timestamp(t as i64, 0).unwrap().format("%Y-%m-%d").to_string()
}

#[tokio::test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
async fn searches_find_only_what_is_shown() {
    let f = fixture();
    let cl = app_of(&f);
    let reader = cl.login("reader").await;
    // what the session is shown
    let shown = cl.get("encounters", &reader).await;
    let e = &shown["items"][0];
    let (pseudonym, patient) =
        (e["encounter_id"].as_str().unwrap().to_string(), e["patient_id"].as_str().unwrap().to_string());
    assert!(pseudonym.starts_with("MO-") && patient.starts_with("MO-"), "{e}");
    let admit = e["admit_time"].as_f64().unwrap();
    assert!(admit < 1.6e9, "the admission is shown moved back: {e}");

    // encounters: free text and every field, real values find nothing, shown ones the stay
    for q in CANARIES.iter().map(|c| c.to_string()).chain([
        "encounter:ENC55102".into(),
        "patient:PIDX77214".into(),
        "admit:2024-01-01".into(),
        "discharge:2024".into(),
        "file:1704110400".into(),
    ]) {
        assert_eq!(cl.total(&format!("encounters?q={}", enc(&q)), &reader).await, 0, "encounters q={q}");
    }
    for q in [pseudonym.clone(), format!("patient:{patient}"), format!("admit:{}", date(admit))] {
        assert_eq!(cl.total(&format!("encounters?q={}", enc(&q)), &reader).await, 1, "encounters q={q}");
    }
    // the dates of the page's filter are dates as shown
    let real = (1_704_000_000, 1_704_200_000);
    assert_eq!(cl.total(&format!("encounters?from={}&to={}", real.0, real.1), &reader).await, 0);
    let day = (admit / 86400.0).floor() * 86400.0;
    assert_eq!(cl.total(&format!("encounters?from={}&to={}", day, day + 86400.0), &reader).await, 1);

    // recordings: by their tokens, never their names, paths or real starts
    let recs = cl.get("recordings", &reader).await;
    let r = &recs["items"][0];
    let token = r["path"].as_str().unwrap().to_string();
    assert!(token.starts_with("mo-rec:"), "{r}");
    let start = r["t_start"].as_f64().or(r["name_time"].as_f64()).unwrap();
    for q in CANARIES.iter().map(|c| c.to_string()).chain([
        "start:2024-01-01".into(),
        "encounter:ENC55102".into(),
        "path:icu".into(),
        "file:ICU7".into(),
    ]) {
        assert_eq!(cl.total(&format!("recordings?q={}", enc(&q)), &reader).await, 0, "recordings q={q}");
    }
    for q in [format!("path:{token}"), format!("encounter:{pseudonym}"), format!("start:{}", date(start))] {
        assert_eq!(cl.total(&format!("recordings?q={}", enc(&q)), &reader).await, 1, "recordings q={q}");
    }

    // every database's rows: the search, each column's filter
    let dbs = cl.get("databases", &reader).await;
    let mut searched = 0;
    for d in dbs["items"].as_array().unwrap() {
        let id = d["id"].as_i64().unwrap();
        let page = cl.get(&format!("databases/{id}/rows"), &reader).await;
        let Some(columns) = page["columns"].as_array() else { continue };
        for q in CANARIES {
            assert_eq!(cl.total(&format!("databases/{id}/rows?q={}", enc(q)), &reader).await, 0, "{} q={q}", d["name"]);
            for col in columns {
                let pos = col["pos"].as_i64().unwrap();
                let n = cl.total(&format!("databases/{id}/rows?f{pos}={}", enc(q)), &reader).await;
                assert_eq!(n, 0, "{} f{pos} ({}) = {q}", d["name"], col["header"]);
            }
        }
        // and what a shown cell holds finds its row
        if let Some(cell) = page["rows"][0]["cells"]
            .as_array()
            .and_then(|cs| cs.iter().filter_map(|c| c.as_str()).find(|c| c.starts_with("MO-") || c.starts_with("19")))
        {
            let word = cell.split_whitespace().next().unwrap();
            assert!(
                cl.total(&format!("databases/{id}/rows?q={}", enc(word)), &reader).await >= 1,
                "{} q={word}",
                d["name"]
            );
            searched += 1;
        }
    }
    assert!(searched >= 3, "the shown cells of the databases were searched");

    // cohorts: dates as shown; columns and text as shown
    let search = |include: Value| json!({ "criteria": { "version": 1, "include": include } });
    let found = |v: &Value| v["total"].as_i64().unwrap();
    for (k, want) in [
        (json!({"kind": "encounter", "from": "2024-01-01", "to": "2024-01-05"}), 0),
        (json!({"kind": "encounter", "from": date(admit), "to": date(admit)}), 1),
        (json!({"kind": "measurement", "name": "Lactate", "from": "2024-01-02", "to": "2024-01-02"}), 0),
        (
            json!({"kind": "measurement", "name": "Lactate", "from": date(admit + 86400.0), "to": date(admit + 86400.0)}),
            1,
        ),
        (json!({"kind": "text", "text": "Zyxwortha"}), 0),
        (json!({"kind": "text", "text": "MRN83651927"}), 0),
        (json!({"kind": "text", "text": "Osei"}), 0),
        (json!({"kind": "text", "text": "1931"}), 0),
        (json!({"kind": "text", "text": "septic shock"}), 1),
    ] {
        let (st, v) = cl.send("POST", "/api/v1/cohorts/search", &reader, Some(search(k.clone()))).await;
        assert_eq!(st, StatusCode::OK, "{k}: {v}");
        assert_eq!(found(&v), want, "cohort {k}");
    }
    let ecg = dbs["items"].as_array().unwrap().iter().find(|d| d["name"].as_str().unwrap().contains("ecg")).unwrap();
    let ecg = ecg["id"].as_i64().unwrap();
    for (text, want) in [("ENC55102", 0), ("PIDX77214", 0), (pseudonym.as_str(), 1)] {
        for name in ["encounter_id", "patient_id"] {
            let k = json!({"kind": "column", "import": ecg, "name": name, "text": text});
            let (st, v) = cl.send("POST", "/api/v1/cohorts/search", &reader, Some(search(k.clone()))).await;
            assert_eq!(st, StatusCode::OK, "{k}: {v}");
            let want = if name == "patient_id" { 0 } else { want };
            assert_eq!(found(&v), want, "cohort {k}");
        }
    }

    // and every search of the session is in the audit log
    let n: i64 =
        f.db.get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM audit WHERE event = 'privacy_search' AND username = 'reader'", [], |r| {
                r.get(0)
            })
            .unwrap();
    assert!(n > 20, "Privacy Mode's searches are audited ({n})");
}

#[tokio::test]
async fn lists_are_ordered_by_shown_times() {
    // two patients a day apart; their own days (up to a year) may order them otherwise when shown
    let f = fixture();
    {
        let mut c = f.db.get().unwrap();
        for (i, day) in [(1, "2024-01-01"), (2, "2024-01-02"), (3, "2024-01-03"), (4, "2024-01-04")] {
            medorama::tables::import(
                &mut c,
                &f.cfg,
                &User::local(),
                "more.csv",
                format!("Encounter ID,Patient ID,Admit Time\nENCX{i}0001,PIDXX{i}0001,{day} 10:00\n").as_bytes(),
                &HashMap::new(),
                "",
                None,
                &Opts::new("more.csv", "", false),
            )
            .unwrap();
        }
    }
    let cl = app_of(&f);
    let reader = cl.login("reader").await;
    for (uri, key) in [("encounters", "admit_time"), ("patients", "latest")] {
        let v = cl.get(uri, &reader).await;
        let times: Vec<f64> = v["items"].as_array().unwrap().iter().filter_map(|e| e[key].as_f64()).collect();
        assert!(times.len() >= 5, "{uri}: {v}");
        assert!(times.windows(2).all(|w| w[0] >= w[1]), "{uri}: latest (as shown) first: {times:?}");
    }
}

#[tokio::test]
async fn cohort_dates_written_in_privacy_mode_keep_their_meaning() {
    let f = fixture();
    let cl = app_of(&f);
    let reader = cl.login("reader").await;
    let boss = cl.login("boss").await;
    let admit = cl.get("encounters", &reader).await["items"][0]["admit_time"].as_f64().unwrap();
    let shown = date(admit);
    let k = json!({"kind": "encounter", "from": shown, "to": shown});
    let (st, made) = cl
        .send(
            "POST",
            "/api/v1/cohorts",
            &reader,
            Some(json!({"name": "stay", "criteria": {"version": 1, "include": k}})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{made}");
    let id = made["id"].as_i64().unwrap();
    let boss_id: i64 =
        f.db.get().unwrap().query_row("SELECT id FROM users WHERE username = 'boss'", [], |r| r.get(0)).unwrap();
    let (st, v) = cl
        .send("PATCH", &format!("/api/v1/cohorts/{id}"), &reader, Some(json!({"shares": [{"user_id": boss_id}]})))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // the reader is shown the dates as typed
    let co = cl.get(&format!("cohorts/{id}"), &reader).await;
    assert_eq!(co["criteria"]["include"]["from"], shown, "{co}");
    assert_eq!(co["members"], 1, "{co}");
    // stored moved forward (about the real dates), and marked: whoever refreshes finds the same
    let stored = cl.get(&format!("cohorts/{id}"), &boss).await;
    let from = stored["criteria"]["include"]["from"].as_str().unwrap().to_string();
    assert!(from.starts_with("2023") || from.starts_with("2024"), "{stored}");
    assert_eq!(stored["criteria"]["include"]["privacy_dates"], true, "{stored}");
    for who in [&boss, &reader] {
        let (st, v) =
            cl.send("POST", &format!("/api/v1/cohorts/{id}/refresh"), who, Some(json!({"dry_run": true}))).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!((v["added"].as_array().unwrap().len(), v["removed"].as_array().unwrap().len()), (0, 0), "{v}");
    }
    // saved again unchanged by the reader: the same criteria
    let (st, v) =
        cl.send("PATCH", &format!("/api/v1/cohorts/{id}"), &reader, Some(json!({"criteria": co["criteria"]}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let again = cl.get(&format!("cohorts/{id}"), &boss).await;
    assert_eq!(again["criteria"], stored["criteria"]);
}

#[tokio::test]
async fn the_offset_is_not_sent() {
    let f = fixture();
    let cl = app_of(&f);
    let reader = cl.login("reader").await;
    let (st, _) = cl.send("GET", "/api/v1/deid", &reader, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let cfg = cl.get("config", &reader).await;
    let d = &cfg["deid"];
    assert!(d["fields"].is_array(), "{cfg}");
    assert!(d.get("offset_seconds").is_none() && d.get("offset").is_none() && d.get("spread_days").is_none(), "{d}");
}
