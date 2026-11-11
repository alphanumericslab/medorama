//! The web page in a real browser (headless Chrome, driven through the DevTools protocol):
//! signing in, every tab drawn without an error, a patient's page and an entry's detail, a
//! cohort search, a connected OMOP database, Privacy Mode on screen, and signing out (the form
//! must keep what is typed); on each, an accessibility check in both themes (tests/a11y.js:
//! names of controls, alt texts, unique ids, WCAG AA contrast).  Synthetic data only.
//!
//! Also what people do on the page (a cohort, an import, a user, Privacy Mode's settings) by its
//! controls, and the page by the keyboard only (the focus never lost, always shown).
//!
//! It needs Chrome or Chromium: `$MEDORAMA_TEST_CHROME`, else the usual places; without one it
//! says so and passes (as the gateway test does without a viewer), unless
//! `$MEDORAMA_TEST_CHROME_REQUIRED` is set (CI's coverage job).

mod common;
use medorama::db::Db;
use medorama::imports::Opts;
use medorama::users::{Role, User};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// What the synthetic patients hold that Privacy Mode must not show.
const CANARIES: &[&str] = &["Wexlorne", "Thandiwe", "SYN4400001"];

fn chrome() -> Option<String> {
    if let Some(c) = std::env::var("MEDORAMA_TEST_CHROME").ok().filter(|c| !c.is_empty()) {
        return Some(c);
    }
    let known = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    ];
    if let Some(k) = known.iter().find(|k| std::path::Path::new(k).is_file()) {
        return Some(k.to_string());
    }
    ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].iter().find_map(|n| {
        std::env::var_os("PATH")?
            .to_str()?
            .split(if cfg!(windows) { ';' } else { ':' })
            .map(|d| std::path::Path::new(d).join(n))
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().to_string())
    })
}

/// The VitalStream viewer, as the gateway's test finds it (`$MEDORAMA_TEST_VIEWER`, else one
/// built in the workspace); without one, the viewer's part is skipped.
fn viewer() -> Option<std::path::PathBuf> {
    if let Some(v) = std::env::var_os("MEDORAMA_TEST_VIEWER") {
        return Some(v.into());
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let exe = if cfg!(windows) { ".exe" } else { "" };
    [
        format!("target/release/vitalstream-rs{exe}"),
        format!("viewers/vitalstream/cpp/build/viewer/bin/vitalstream-cpp{exe}"),
    ]
    .iter()
    .map(|p| root.join(p))
    .find(|p| p.is_file())
}

/// Base64 (DevTools' screenshots) decoded.
fn unbase64(s: &str) -> Vec<u8> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        _ => 63,
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for ch in bytes.chunks(4) {
        let v: Vec<u32> = ch.iter().map(|c| val(*c) as u32).collect();
        let n = v.iter().enumerate().fold(0u32, |a, (i, x)| a | (x << (18 - 6 * i)));
        out.extend(&n.to_be_bytes()[1..ch.len()]);
    }
    out
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A plain HTTP GET (Chrome's /json endpoints).
fn http_get(port: u16, path: &str) -> Option<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes()).ok()?;
    // until the end, or until it stops sending (some keep the connection open)
    let mut buf = vec![];
    let mut chunk = [0u8; 8192];
    while let Ok(n) = s.read(&mut chunk) {
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head.lines().find_map(|l| {
                l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
            });
            if len.is_some_and(|n| body.len() >= n) {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    text.split_once("\r\n\r\n").map(|(_, b)| b.to_string())
}

/// The browser: a headless Chrome and the DevTools connection to its page.
struct Browser {
    child: std::process::Child,
    ws: tungstenite::WebSocket<TcpStream>,
    next: u64,
    /// Uncaught exceptions and console errors seen so far.
    errors: Vec<String>,
    /// The text to type into the next `prompt()` (dialogs are accepted as they open).
    prompt: String,
    /// The dialogs' messages, as they opened.
    dialogs: Vec<String>,
    _profile: tempfile::TempDir,
}

impl Browser {
    fn launch(path: &str) -> Browser {
        let port = free_port();
        let profile = tempfile::tempdir().unwrap();
        let child = std::process::Command::new(path)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--password-store=basic",
                "--window-size=1400,900",
                &format!("--remote-debugging-port={port}"),
                &format!("--user-data-dir={}", profile.path().display()),
                "about:blank",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("chrome starts");
        let deadline = Instant::now() + Duration::from_secs(30);
        let url = loop {
            if let Some(body) = http_get(port, "/json/list") {
                let tabs: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                if let Some(u) = tabs.as_array().and_then(|t| {
                    t.iter()
                        .find(|t| t["type"] == "page")
                        .and_then(|t| t["webSocketDebuggerUrl"].as_str().map(String::from))
                }) {
                    break u;
                }
            }
            assert!(Instant::now() < deadline, "chrome did not answer");
            std::thread::sleep(Duration::from_millis(200));
        };
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let (ws, _) = tungstenite::client::client(url.as_str(), stream).expect("devtools");
        let mut b =
            Browser { child, ws, next: 0, errors: vec![], prompt: String::new(), dialogs: vec![], _profile: profile };
        b.call("Runtime.enable", json!({}));
        b.call("Page.enable", json!({}));
        b
    }

    /// Send a command and wait for its answer, noting errors seen meanwhile.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.ws
            .send(tungstenite::Message::text(json!({"id": id, "method": method, "params": params}).to_string()))
            .unwrap();
        loop {
            let msg = self.ws.read().expect("devtools message");
            let Ok(text) = msg.to_text() else { continue };
            let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
            match v["method"].as_str() {
                Some("Runtime.exceptionThrown") => {
                    self.errors.push(v["params"]["exceptionDetails"]["exception"]["description"].to_string())
                }
                Some("Page.javascriptDialogOpening") => {
                    // a confirm() or prompt(): accepted (with `prompt` typed), as a user would
                    self.dialogs.push(v["params"]["message"].as_str().unwrap_or("").to_string());
                    self.next += 1;
                    let answer = json!({"id": self.next, "method": "Page.handleJavaScriptDialog",
                        "params": {"accept": true, "promptText": self.prompt}});
                    self.ws.send(tungstenite::Message::text(answer.to_string())).unwrap();
                }
                Some("Runtime.consoleAPICalled") if v["params"]["type"] == "error" => {
                    let args = v["params"]["args"].to_string();
                    // a refused request in the network panel is not a page error
                    if !args.contains("Failed to load resource") {
                        self.errors.push(args)
                    }
                }
                _ => {}
            }
            if v["id"] == id {
                return v;
            }
        }
    }

    /// The value of a JavaScript expression (awaited when it is a promise).
    fn eval(&mut self, js: &str) -> Value {
        let r = self.call("Runtime.evaluate", json!({"expression": js, "awaitPromise": true, "returnByValue": true}));
        if let Some(e) = r["result"]["exceptionDetails"].as_object() {
            panic!("{js}: {e:?}");
        }
        r["result"]["result"]["value"].clone()
    }

    /// The accessibility check (tests/a11y.js) of what the page shows, in the light and the dark
    /// theme: its problems, each named after `what`.
    fn accessibility(&mut self, what: &str) -> Vec<String> {
        let mut out = vec![];
        for theme in ["light", "dark"] {
            self.eval(&format!("document.documentElement.dataset.theme = '{theme}'; true"));
            std::thread::sleep(Duration::from_millis(100));
            let found = self.eval(include_str!("a11y.js"));
            out.extend(
                found
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|p| format!("{what} ({theme}): {p}")),
            );
        }
        out
    }

    /// Wait until an expression is true.
    fn until(&mut self, js: &str, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.eval(js) != Value::Bool(true) {
            if Instant::now() >= deadline {
                // what the page said (an error is shown in its notice, not thrown)
                let said = self.eval("(document.querySelector('#toast') || {}).innerText || ''");
                panic!("timed out waiting for {what}; the page says {said}; errors: {:?}", self.errors);
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// A click with the mouse, in the middle of the element (scrolled into view first; waited
    /// for while the page draws it).
    fn click(&mut self, selector: &str) {
        self.until(&format!("!!document.querySelector({selector:?})"), selector);
        let at = self.eval(&format!(
            "(() => {{ const e = document.querySelector({selector:?}); if (!e) return null;
               e.scrollIntoView({{block: 'center'}}); const r = e.getBoundingClientRect();
               return [r.left + r.width / 2, r.top + r.height / 2]; }})()"
        ));
        let (x, y) = (at[0].as_f64().unwrap_or_else(|| panic!("no {selector} to click")), at[1].as_f64().unwrap());
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
            );
        }
    }

    /// A key pressed and released (`Tab`, `Enter`, `Escape`, ` `, `ArrowDown` …), with Shift.
    fn key_with(&mut self, key: &str, shift: bool) {
        let (code, vk) = match key {
            "Tab" => ("Tab", 9),
            "Enter" => ("Enter", 13),
            "Escape" => ("Escape", 27),
            " " => ("Space", 32),
            "ArrowDown" => ("ArrowDown", 40),
            "ArrowUp" => ("ArrowUp", 38),
            _ => panic!("no key {key}"),
        };
        let text = match key {
            "Enter" => "\r",
            " " => " ",
            _ => "",
        };
        let modifiers = if shift { 8 } else { 0 };
        self.call(
            "Input.dispatchKeyEvent",
            json!({"type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code,
                   "windowsVirtualKeyCode": vk, "text": text, "modifiers": modifiers}),
        );
        self.call(
            "Input.dispatchKeyEvent",
            json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "modifiers": modifiers}),
        );
    }

    fn key(&mut self, key: &str) {
        self.key_with(key, false)
    }

    /// Text typed into what has the focus.
    fn type_text(&mut self, text: &str) {
        self.call("Input.insertText", json!({"text": text}));
    }

    /// Text typed into a field, after a click on it.
    fn fill(&mut self, selector: &str, text: &str) {
        self.click(selector);
        self.eval(&format!("document.querySelector({selector:?}).select?.(); true"));
        self.type_text(text);
    }

    /// A select's option chosen (as a user's choice: the change event follows).
    fn choose(&mut self, selector: &str, value: &str) {
        self.eval(&format!(
            "(() => {{ const e = document.querySelector({selector:?}); e.focus(); e.value = {value:?};
               e.dispatchEvent(new Event('change', {{bubbles: true}})); return true; }})()"
        ));
    }

    /// Files chosen in a file input, as the file dialog chooses them.
    fn choose_files(&mut self, selector: &str, files: &[&std::path::Path]) {
        let doc = self.call("DOM.getDocument", json!({}));
        let root = doc["result"]["root"]["nodeId"].clone();
        let node = self.call("DOM.querySelector", json!({"nodeId": root, "selector": selector}));
        let files: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
        let r = self.call("DOM.setFileInputFiles", json!({"nodeId": node["result"]["nodeId"], "files": files}));
        assert!(r.get("error").is_none(), "{selector}: {r}");
    }

    /// The element that has the focus, described (`tag#id.class "text"`), or `lost: …` when the
    /// focus is on nothing a user can see or act on (the page's body, a removed or hidden
    /// element).
    fn focused(&mut self) -> String {
        self.eval(
            "(() => { const e = document.activeElement;
               if (!e || e === document.body || e === document.documentElement) return 'lost: on the page itself';
               if (!e.isConnected) return 'lost: on a removed element';
               const r = e.getBoundingClientRect();
               if ((r.width === 0 && r.height === 0) || e.closest('.hidden, [hidden]')) return `lost: on a hidden ${e.tagName.toLowerCase()}#${e.id}`;
               const t = (e.getAttribute('aria-label') || e.innerText || e.value || '').trim().slice(0, 30);
               return `${e.tagName.toLowerCase()}${e.id ? '#' + e.id : ''}${typeof e.className === 'string' && e.className ? '.' + e.className.split(' ')[0] : ''} \"${t}\"`; })()",
        )
        .as_str()
        .unwrap_or("")
        .to_string()
    }

    /// Whether the focused element shows that it has the focus (an outline, a ring, a border).
    fn focus_shows(&mut self) -> bool {
        self.eval(
            "(() => { const e = document.activeElement; const s = getComputedStyle(e);
               const own = s.outlineStyle !== 'none' && parseFloat(s.outlineWidth) > 0 || s.boxShadow !== 'none';
               const box = e.closest('.sbox'); const b = box && getComputedStyle(box);
               return own || !!(b && b.outlineStyle !== 'none' && parseFloat(b.outlineWidth) > 0); })()",
        ) == Value::Bool(true)
    }

    /// One Tab (or Shift+Tab), checked: the focus is not lost, and shows.  Past the page's last
    /// (or first) control the focus leaves the page, as in any browser (to its address bar):
    /// that stop is `(out of the page)`, and the next Tab comes back to the first control.
    fn tab(&mut self, back: bool, stops: &[String]) -> String {
        let edge = self.eval(&format!(
            "(() => {{ const ok = (e) => {{ const r = e.getBoundingClientRect(); return e.tabIndex >= 0 && !e.disabled
                 && (r.width || r.height) && !e.closest('.hidden, [hidden], [inert]') && getComputedStyle(e).visibility !== 'hidden'; }};
               const all = [...document.querySelectorAll('a[href], button, input:not([type=hidden]), select, textarea, summary, [tabindex]')].filter(ok);
               return document.activeElement === all[{}]; }})()",
            if back { "0" } else { "all.length - 1" }
        )) == Value::Bool(true);
        self.key_with("Tab", back);
        let f = self.focused();
        if edge && f == "lost: on the page itself" {
            return "(out of the page)".into();
        }
        assert!(!f.starts_with("lost"), "focus {f} after {stops:?}");
        assert!(self.focus_shows(), "{f}: the focus does not show");
        f
    }

    /// Tab (or Shift+Tab) until `js` is true of the focused element (`e`), at most `max` times;
    /// each step checked ([`Browser::tab`]).  The stops, in order.
    fn tab_to(&mut self, js: &str, max: usize, back: bool) -> Vec<String> {
        let mut stops = vec![];
        for _ in 0..max {
            let f = self.tab(back, &stops);
            stops.push(f);
            if self.eval(&format!("(e => !!({js}))(document.activeElement)")) == Value::Bool(true) {
                return stops;
            }
        }
        panic!("{js}: not reached by {max} tabs: {stops:?}");
    }

    /// `n` presses of Tab, each checked as [`Browser::tab_to`] checks them.
    fn tab_through(&mut self, n: usize) -> Vec<String> {
        let mut stops = vec![];
        for _ in 0..n {
            let f = self.tab(false, &stops);
            stops.push(f);
        }
        stops
    }

    /// With `$MEDORAMA_TEST_SCREENSHOTS` (a folder): the page as it is drawn, `<name>.png`, in the
    /// light and the dark theme (for looking at the design; never compared).
    fn screenshot(&mut self, name: &str) {
        let Some(dir) = std::env::var_os("MEDORAMA_TEST_SCREENSHOTS") else { return };
        for theme in ["light", "dark"] {
            self.eval(&format!("document.documentElement.dataset.theme = '{theme}'; window.scrollTo(0, 0); true"));
            std::thread::sleep(Duration::from_millis(300));
            let r = self.call("Page.captureScreenshot", json!({"format": "png", "captureBeyondViewport": true}));
            if let Some(data) = r["result"]["data"].as_str() {
                std::fs::write(std::path::Path::new(&dir).join(format!("{name}-{theme}.png")), unbase64(data)).unwrap();
            }
        }
    }

    fn goto(&mut self, url: &str) {
        self.call("Page.navigate", json!({"url": url}));
    }

    fn text(&mut self) -> String {
        self.eval("document.body.innerText").as_str().unwrap_or("").to_string()
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A catalog with synthetic patients, labs and a note, and a connected OMOP database.
fn fixture() -> (tempfile::TempDir, medorama::config::Config) {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.timezone = "UTC".into();
    // a recording (synthetic WFDB), and the VitalStream viewer when one is built
    let data = root.join("data");
    cfg.data_roots = vec![data.to_string_lossy().to_string()];
    if let Some(v) = viewer() {
        cfg.viewer = v.to_string_lossy().to_string();
    }
    let rec = data.join("icu").join(common::bed_name("ICU7", "12", common::T0 + 86_400));
    common::wfdb_record(&rec, 86_400);
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
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    };
    import(
        &mut c,
        "patients.csv",
        "MRN,Last Name,First Name,DOB,Sex,Patient ID\nSYN4400001,Wexlorne,Thandiwe,1952-02-03,F,P440001\nSYN4400002,Quarrel,Bram,1964-05-06,M,P440002\n",
        "",
    );
    import(
        &mut c,
        "encounters.csv",
        "Encounter ID,Patient ID,Admit Time,Discharge Time,Admit Unit\nE4400001,P440001,2024-01-02 08:00,2024-01-09 12:00,Cardiology\n",
        "",
    );
    import(
        &mut c,
        "labs.csv",
        "Patient ID,Encounter ID,Test Name,Result,Units,Ref Low,Ref High,Collected\nP440001,E4400001,Lactate,4.6,mmol/L,0.5,2,2024-01-03 06:00\nP440001,E4400001,Creatinine,1.9,mg/dL,0.6,1.2,2024-01-03 06:00\nP440002,,Lactate,1.1,mmol/L,0.5,2,2024-01-04 06:00\n",
        "lab",
    );
    import(
        &mut c,
        "notes.csv",
        "Patient ID,Encounter ID,Note Type,Time,Text\nP440001,E4400001,Progress note,2024-01-04 10:00,Thandiwe Wexlorne is improving.\n",
        "note",
    );
    let rel = rec.strip_prefix(&data).unwrap().to_string_lossy().to_string();
    import(&mut c, "icu_index.csv", &format!("patient_id,encounter_id,file\nP440001,E4400001,{rel}\n"), "");
    // a row held back for review: an MRN another patient has
    import(
        &mut c,
        "late.csv",
        "MRN,Last Name,First Name,DOB,Sex,Patient ID\nSYN4400001,Quill,Ann,1970-01-01,F,P440009\n",
        "",
    );
    medorama::users::add(&c, "boss", "", Role::Admin, true, "0123456789ab").unwrap();
    // a connected OMOP database: one person, the second patient (the same MRN)
    let omop = root.join("omop.sqlite");
    let o = rusqlite::Connection::open(&omop).unwrap();
    o.execute_batch(
        "CREATE TABLE person (person_id INTEGER, gender_concept_id INTEGER, year_of_birth INTEGER, month_of_birth INTEGER,
             day_of_birth INTEGER, birth_datetime TEXT, person_source_value TEXT);
         CREATE TABLE care_site (care_site_id INTEGER, care_site_name TEXT);
         CREATE TABLE visit_occurrence (visit_occurrence_id INTEGER, person_id INTEGER, visit_concept_id INTEGER, visit_start_date TEXT,
             visit_start_datetime TEXT, visit_end_date TEXT, visit_end_datetime TEXT, care_site_id INTEGER);
         CREATE TABLE visit_detail (visit_detail_id INTEGER, person_id INTEGER, visit_detail_concept_id INTEGER, visit_occurrence_id INTEGER);
         CREATE TABLE measurement (measurement_id INTEGER, person_id INTEGER, measurement_concept_id INTEGER, measurement_date TEXT,
             measurement_datetime TEXT, value_as_number REAL, value_as_concept_id INTEGER, unit_concept_id INTEGER, range_low REAL,
             range_high REAL, visit_occurrence_id INTEGER, measurement_source_value TEXT, unit_source_value TEXT, value_source_value TEXT);
         CREATE TABLE observation (observation_id INTEGER, person_id INTEGER, observation_concept_id INTEGER, observation_date TEXT,
             observation_datetime TEXT, value_as_number REAL, value_as_string TEXT, value_as_concept_id INTEGER, unit_concept_id INTEGER,
             visit_occurrence_id INTEGER, observation_source_value TEXT, unit_source_value TEXT);
         CREATE TABLE condition_occurrence (condition_occurrence_id INTEGER, person_id INTEGER, condition_concept_id INTEGER,
             condition_start_date TEXT, condition_start_datetime TEXT, condition_end_date TEXT, condition_end_datetime TEXT,
             visit_occurrence_id INTEGER, condition_source_value TEXT, condition_source_concept_id INTEGER);
         CREATE TABLE drug_exposure (drug_exposure_id INTEGER, person_id INTEGER, drug_concept_id INTEGER, drug_exposure_start_date TEXT,
             drug_exposure_start_datetime TEXT, drug_exposure_end_date TEXT, drug_exposure_end_datetime TEXT, visit_occurrence_id INTEGER,
             drug_source_value TEXT, dose_unit_source_value TEXT);
         CREATE TABLE procedure_occurrence (procedure_occurrence_id INTEGER, person_id INTEGER, procedure_concept_id INTEGER,
             procedure_date TEXT, procedure_datetime TEXT, visit_occurrence_id INTEGER, procedure_source_value TEXT);
         CREATE TABLE note (note_id INTEGER, person_id INTEGER, note_date TEXT, note_datetime TEXT, note_type_concept_id INTEGER,
             note_title TEXT, note_text TEXT, provider_id INTEGER, visit_occurrence_id INTEGER);
         CREATE TABLE provider (provider_id INTEGER, provider_name TEXT);
         CREATE TABLE concept (concept_id INTEGER, concept_name TEXT, vocabulary_id TEXT, concept_code TEXT);
         CREATE TABLE cdm_source (cdm_source_name TEXT, source_release_date TEXT, cdm_release_date TEXT);
         INSERT INTO concept VALUES (3019550, 'Troponin T.cardiac [Mass/volume] in Serum or Plasma', 'LOINC', '6598-7'), (8842, 'nanogram per liter', 'UCUM', 'ng/L');
         INSERT INTO person VALUES (1, 8507, 1964, 5, 6, NULL, 'SYN4400002');
         INSERT INTO measurement VALUES (1, 1, 3019550, '2023-11-02', '2023-11-02 09:00:00', 212, NULL, 8842, 0, 14, NULL, 'TNT', 'ng/L', '212');
         INSERT INTO cdm_source VALUES ('Synthetic', '2024-01-01', '2024-01-02');",
    )
    .unwrap();
    drop(o);
    let n = medorama::sources::NewSource {
        name: "omop".into(),
        kind: "omop".into(),
        url: format!("sqlite:{}", omop.display()),
        settings: medorama::sources::Settings { timezone: "UTC".into(), ..Default::default() },
        ..Default::default()
    };
    medorama::sources::add(&c, &admin, &n).unwrap();
    drop(c);
    (tmp, cfg)
}

/// The fixture's server on a free port, and a browser on its page; `None` without Chrome.
struct Run {
    b: Browser,
    base: String,
    cfg: medorama::config::Config,
    _rt: tokio::runtime::Runtime,
    _tmp: tempfile::TempDir,
}

fn start() -> Option<Run> {
    let Some(path) = chrome() else {
        // where the browser tests are required (CI's coverage), a missing Chrome is a failure
        assert!(
            std::env::var_os("MEDORAMA_TEST_CHROME_REQUIRED").is_none(),
            "no Chrome or Chromium found, and MEDORAMA_TEST_CHROME_REQUIRED is set"
        );
        eprintln!("no Chrome or Chromium found (set MEDORAMA_TEST_CHROME): the browser test is skipped");
        return None;
    };
    let (tmp, cfg) = fixture();
    let port = free_port();
    let app = std::sync::Arc::new(medorama::server::App::new(
        std::sync::Arc::new(cfg.clone()),
        std::sync::Arc::new(Db::open(&cfg, false).unwrap()),
    ));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.spawn(async move {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        axum::serve(listener, medorama::server::router(app)).await.unwrap();
    });
    let base = format!("http://127.0.0.1:{port}/");
    let mut b = Browser::launch(&path);
    b.goto(&base);
    b.until(
        "!!document.querySelector('#login') && !document.querySelector('#login').classList.contains('hidden')",
        "the sign-in form",
    );
    Some(Run { b, base, cfg, _rt: rt, _tmp: tmp })
}

/// Signs in as `boss` with the mouse and the keyboard's letters.
fn sign_in(b: &mut Browser) {
    b.fill("#login-user", "boss");
    b.fill("#login-pass", "0123456789ab");
    b.click("#login-form button[type=submit], #login-form button");
    b.until("!document.querySelector('#app').classList.contains('hidden')", "the page after signing in");
}

/// An API answer, read by the page's session.
fn api(b: &mut Browser, path: &str) -> Value {
    b.eval(&format!("fetch('/api/v1/{path}').then(r => r.json())"))
}

#[test]
fn the_page_in_a_browser() {
    let Some(Run { mut b, base, _rt, _tmp, .. }) = start() else { return };

    // signing in
    let mut problems: Vec<String> = b.accessibility("the sign-in form");
    b.eval(
        "document.querySelector('#login-user').value = 'boss';
         document.querySelector('#login-pass').value = '0123456789ab';
         document.querySelector('#login-form').requestSubmit(); true",
    );
    b.until("!document.querySelector('#app').classList.contains('hidden')", "the page after signing in");
    assert_eq!(b.eval("location.hash"), json!("#/cohorts"), "the page opens on Cohorts");

    // every tab draws without an error, and passes the accessibility check (tests/a11y.js)
    for (hash, view) in [
        ("#/cohorts", "cohorts"),
        ("#/patients", "patients"),
        ("#/databases", "databases"),
        ("#/import", "sources"),
        ("#/review", "review"),
        ("#/jobs", "jobs"),
        ("#/actions", "actions"),
        ("#/history", "history"),
        ("#/admin", "admin"),
    ] {
        b.eval(&format!("location.hash = '{hash}'; true"));
        b.until(&format!("!document.querySelector('#view-{view}').classList.contains('hidden')"), hash);
        std::thread::sleep(Duration::from_millis(600));
        assert!(b.errors.is_empty(), "{hash}: {:?}", b.errors);
        problems.extend(b.accessibility(hash));
    }

    // the Patient Catalog lists the patients; the connected database is named
    b.eval("location.hash = '#/patients'; true");
    b.until("document.querySelector('#view-patients').innerText.includes('P440001')", "the list");
    b.eval("location.hash = '#/databases'; true");
    b.until("document.querySelector('#db-list').innerText.includes('connected')", "the connected database");

    // a cohort search across the catalog and OMOP (its patient linked as found)
    let found = b.eval(&format!(
        "fetch('{base}api/v1/cohorts/search', {{method: 'POST', headers: {{'X-Medorama': '1', 'Content-Type': 'application/json'}},
           body: JSON.stringify({{criteria: {{version: 1, include: {{kind: 'expression', text: 'labs.lactate > 4 OR omop.troponin > 100'}}}}}})}})
         .then(r => r.json()).then(d => d.items.map(i => i.patient_id).sort().join(','))"
    ));
    assert_eq!(found, json!("P440001,P440002"));

    // a patient's page: the journey, the report, an entry's detail
    let id = b.eval(&format!("fetch('{base}api/v1/patients?q=P440001').then(r => r.json()).then(d => d.items[0].id)"));
    b.eval(&format!("location.hash = '#/patient/{id}'; true"));
    b.until("document.querySelector('#pp-bar').innerText.includes('Wexlorne')", "the patient's banner");
    b.until("document.querySelector('#view-patient').innerText.includes('Lactate')", "the report");
    b.eval(
        "const e = [...document.querySelectorAll('#view-patient [data-key]')].find(x => x.innerText.includes('Lactate'));
         e && e.click(); true",
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(b.errors.is_empty(), "the patient's page: {:?}", b.errors);
    problems.extend(b.accessibility("a patient's page"));

    // Privacy Mode on screen: no identifier, a pseudonym
    b.eval("document.querySelector('#btn-privacy').click(); true");
    b.until("document.body.classList.contains('privacy')", "Privacy Mode");
    b.until(
        "document.querySelector('#pp-bar') && document.querySelector('#pp-bar').innerText.includes('MO-')",
        "the pseudonym",
    );
    std::thread::sleep(Duration::from_millis(800));
    let text = b.text();
    for k in CANARIES {
        assert!(!text.contains(k), "Privacy Mode shows {k}");
    }
    problems.extend(b.accessibility("a patient's page in Privacy Mode"));
    b.eval("location.hash = '#/patients'; true");
    b.until("document.querySelector('#view-patients').innerText.includes('MO-')", "the list in Privacy Mode");
    let text = b.text();
    for k in CANARIES {
        assert!(!text.contains(k), "the list in Privacy Mode shows {k}");
    }

    // signing out: the form keeps what is typed while the page stops asking for changes
    b.eval("document.querySelector('#btn-logout').click(); true");
    b.until("!document.querySelector('#login').classList.contains('hidden')", "the sign-in form again");
    b.eval(
        "document.querySelector('#login-pass').focus(); document.querySelector('#login-pass').value = 'typing'; true",
    );
    std::thread::sleep(Duration::from_secs(7));
    assert_eq!(b.eval("document.activeElement.id"), json!("login-pass"));
    assert_eq!(b.eval("document.querySelector('#login-pass').value"), json!("typing"));
    assert!(b.errors.is_empty(), "{:?}", b.errors);

    problems.sort();
    problems.dedup();
    assert!(problems.is_empty(), "accessibility, {} problems:\n{}", problems.len(), problems.join("\n"));
}

/// The next person at the browser: after signing out, or after the session ended, the page is
/// loaded again, so that a user without the PHI permission who signs in next sees nothing of
/// the patient the last one looked at.
#[test]
fn the_next_user_sees_nothing_of_the_last() {
    let Some(Run { mut b, base, _rt, _tmp, .. }) = start() else { return };
    let made = b.eval(&format!(
        "fetch('{base}api/v1/login', {{method: 'POST', headers: {{'X-Medorama': '1', 'Content-Type': 'application/json'}},
           body: JSON.stringify({{username: 'boss', password: '0123456789ab'}})}})
         .then(() => fetch('{base}api/v1/users', {{method: 'POST', headers: {{'X-Medorama': '1', 'Content-Type': 'application/json'}},
           body: JSON.stringify({{username: 'reader', role: 'viewer', phi: false, password: '0123456789ab'}})}}))
         .then(r => r.status)"
    ));
    assert_eq!(made, json!(200));
    b.goto(&base);
    b.until("!document.querySelector('#app').classList.contains('hidden')", "the page, signed in");
    let open_patient = |b: &mut Browser| {
        let id = api(b, "patients?q=P440001")["items"][0]["id"].clone();
        b.eval(&format!("location.hash = '#/patient/{id}'; true"));
        b.until(
            "document.querySelector('#pp-bar') && document.querySelector('#pp-bar').innerText.includes('Wexlorne')",
            "the patient, identified",
        );
    };
    let reader_signs_in = |b: &mut Browser, how: &str| {
        b.until("!document.querySelector('#login').classList.contains('hidden')", &format!("the sign-in form ({how})"));
        b.fill("#login-user", "reader");
        b.fill("#login-pass", "0123456789ab");
        b.click("#login-form button[type=submit], #login-form button");
        b.until("!document.querySelector('#app').classList.contains('hidden')", "the page, the reader signed in");
        std::thread::sleep(Duration::from_millis(1500));
        let text = b.text();
        for k in CANARIES {
            assert!(!text.contains(k), "after {how}, the next user sees {k}");
        }
    };
    // signed out by the button
    open_patient(&mut b);
    b.eval("document.querySelector('#btn-logout').click(); true");
    reader_signs_in(&mut b, "signing out");
    // the session ended elsewhere (expired, or signed out everywhere): the next request is refused
    b.eval("document.querySelector('#btn-logout').click(); true");
    b.until("!document.querySelector('#login').classList.contains('hidden')", "the sign-in form");
    sign_in(&mut b);
    open_patient(&mut b);
    b.eval(&format!(
        "fetch('{base}api/v1/logout', {{method: 'POST', headers: {{'X-Medorama': '1'}}}}).then(() => true)"
    ));
    b.eval("location.hash = '#/patients'; true");
    reader_signs_in(&mut b, "the session ended");
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}

/// What people do on the page, by its controls (the mouse, the keyboard's letters, the file
/// dialog, the confirmations): make a cohort, import a table, add and edit a user, change what
/// Privacy Mode does.  Each checked through the API afterwards.
#[test]
fn the_pages_workflows() {
    let Some(Run { mut b, cfg, _rt, _tmp, .. }) = start() else { return };
    sign_in(&mut b);

    // a cohort: an expression, its patients, saved under a name
    b.eval("location.hash = '#/cohorts'; true");
    b.until("!document.querySelector('#view-cohorts').classList.contains('hidden')", "the cohorts");
    b.click("#co-addexpr");
    b.until("!!document.querySelector('#co-criteria input[data-k=\"text\"]')", "the expression's field");
    b.fill("#co-criteria input[data-k=\"text\"]", "female");
    b.until("document.querySelector('#co-count').innerText.startsWith('1 patient')", "one patient found");
    b.fill("#co-name", "Women in the catalog");
    b.click("#co-create");
    b.until("/^#\\/cohorts\\/\\d+/.test(location.hash)", "the new cohort's page");
    let cohorts = api(&mut b, "cohorts");
    let made = cohorts["items"].as_array().unwrap().iter().find(|c| c["name"] == "Women in the catalog").cloned();
    assert_eq!(made.map(|c| c["members"].clone()), Some(json!(1)), "{cohorts}");

    // a table, chosen in the file dialog, previewed, imported
    let file = cfg.home.join("troponin.csv");
    std::fs::write(&file, "Patient ID,Test Name,Result,Units,Collected\nP440002,Troponin T,35,ng/L,2024-02-01 09:00\n")
        .unwrap();
    b.eval("location.hash = '#/import'; true");
    b.until("!document.querySelector('#view-sources').classList.contains('hidden')", "the import page");
    b.choose_files("#tb-files", &[&file]);
    b.until("document.querySelector('#tb-list').innerText.includes('1 rows')", "the file's columns");
    b.click("#tb-preview");
    b.until("document.querySelector('#tb-list').innerText.includes('Preview:')", "the preview");
    b.click("#tb-import");
    b.until("document.querySelector('#imports-table').innerText.includes('troponin.csv')", "the import listed");
    let id = api(&mut b, "patients?q=P440002")["items"][0]["id"].clone();
    let report = api(&mut b, &format!("patients/{id}/report")).to_string();
    assert!(report.contains("Troponin T"), "the imported result on the patient's page: {report}");

    // a table on the server, chosen in the page's own dialog (the server lists its folders),
    // imported where it is
    let served = std::path::Path::new(&cfg.data_roots[0]).join("creatinine.csv");
    std::fs::write(
        &served,
        "Patient ID,Test Name,Result,Units,Collected\nP440002,Creatinine,80,umol/L,2024-02-01 09:00\n",
    )
    .unwrap();
    let pick = "#sf-list input[data-path$=\"creatinine.csv\"]";
    b.click("#tb-server");
    b.until("document.querySelector('#sf').open && !!document.querySelector('#sf-list [data-dir]')", "the data roots");
    b.click("#sf-list [data-dir]");
    b.until(&format!("!!document.querySelector({pick:?})"), "the table on the server");
    let problems = b.accessibility("Choose on the server");
    assert!(problems.is_empty(), "{problems:#?}");
    b.click(pick);
    b.click("#sf-ok");
    b.until("document.querySelector('#tb-list').innerText.includes('1 rows')", "the server table's columns");
    b.click("#tb-import");
    b.until(
        "document.querySelector('#imports-table').innerText.includes('creatinine.csv')",
        "the server table imported",
    );
    let report = api(&mut b, &format!("patients/{id}/report")).to_string();
    assert!(report.contains("Creatinine"), "the result imported from the server: {report}");

    // a user: added, their role and PHI permission changed, their password reset, deleted
    b.eval("location.hash = '#/admin'; true");
    b.until("!document.querySelector('#view-admin').classList.contains('hidden')", "the admin page");
    // the users' table above the form is drawn first: the form's button moves down when it is
    b.until("document.querySelector('#usr-table').innerText.includes('boss')", "the users");
    b.fill("#usr-name", "carol");
    b.fill("#usr-display", "Carol Tester");
    b.choose("#usr-role", "analyst");
    b.fill("#usr-pass", "carolpassword1");
    b.click("#usr-add button");
    b.until("document.querySelector('#usr-table').innerText.includes('carol')", "the new user");
    let user = |b: &mut Browser| {
        api(b, "users")["items"].as_array().unwrap().iter().find(|u| u["username"] == "carol").cloned()
    };
    let carol = user(&mut b).expect("carol");
    let uid = carol["id"].as_i64().unwrap();
    assert_eq!((carol["role"].as_str(), carol["phi"].as_bool()), (Some("analyst"), Some(false)));
    b.choose(&format!("[data-user-role=\"{uid}\"]"), "viewer");
    b.until(
        "fetch('/api/v1/users').then(r => r.json()).then(d => d.items.some(u => u.username === 'carol' && u.role === 'viewer'))",
        "the role changed",
    );
    b.until(&format!("!!document.querySelector('[data-user-phi=\"{uid}\"]')"), "the table again");
    b.click(&format!("[data-user-phi=\"{uid}\"]"));
    b.until(
        "fetch('/api/v1/users').then(r => r.json()).then(d => d.items.some(u => u.username === 'carol' && u.phi))",
        "the PHI permission given",
    );
    b.prompt = "carolnewpass12".into();
    b.click(&format!("[data-user-pass=\"{uid}\"]"));
    let login = |b: &mut Browser, pw: &str| {
        b.eval(&format!(
            "fetch('/api/v1/login', {{method: 'POST', credentials: 'omit', headers: {{'X-Medorama': '1', 'Content-Type': 'application/json'}},
               body: JSON.stringify({{username: 'carol', password: '{pw}'}})}}).then(r => r.status)"
        ))
    };
    b.until(
        "fetch('/api/v1/login', {method: 'POST', credentials: 'omit', headers: {'X-Medorama': '1', 'Content-Type': 'application/json'},
           body: JSON.stringify({username: 'carol', password: 'carolnewpass12'})}).then(r => r.status === 200)",
        "the new password",
    );
    assert_ne!(login(&mut b, "carolpassword1"), json!(200), "the old password no longer works");
    b.click(&format!("[data-user-del=\"{uid}\"]"));
    b.until("!document.querySelector('#usr-table').innerText.includes('Carol Tester')", "the user deleted");
    assert!(user(&mut b).is_none());
    assert!(b.dialogs.iter().any(|d| d.contains("Delete the user carol")), "{:?}", b.dialogs);

    // Privacy Mode's settings: the offset and each patient's days, saved; Defaults only shows them
    b.fill("#deid-days", "1000");
    b.fill("#deid-spread", "30");
    b.until("document.querySelector('#deid-sum').innerText.includes('up to 30 more')", "the sum");
    b.click("#deid-form button.primary");
    b.until(
        "fetch('/api/v1/deid').then(r => r.json()).then(d => d.spread_days === 30 && d.offset.days === 1000)",
        "the settings saved",
    );
    b.click("#deid-hipaa");
    b.until("document.querySelector('#deid-spread').value === '365'", "the defaults shown");
    let d = api(&mut b, "deid");
    assert_eq!(
        (d["spread_days"].clone(), d["offset"]["days"].clone()),
        (json!(30), json!(1000)),
        "not saved by Defaults"
    );

    // dates and times, as an admin chooses them: day first, a 12-hour clock
    b.choose("#disp-date", "dmy");
    b.choose("#disp-clock", "12h");
    b.until(
        r"/^\d{2}\/\d{2}\/\d{4} \d{1,2}:\d{2} (AM|PM)$/.test(document.querySelector('#disp-now').innerText)",
        "the preview",
    );
    b.click("#disp-form button.primary");
    b.until(
        "fetch('/api/v1/config').then(r => r.json()).then(d => d.display.date === 'dmy' && d.display.clock === '12h')",
        "the choice saved",
    );
    // the audit log's times, written that way
    b.until(
        r"/\d{2}\/\d{2}\/\d{4} \d{1,2}:\d{2}(:\d{2})? (AM|PM)/.test(document.querySelector('#audit-table').innerText)",
        "the page's times in the chosen form",
    );
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}

/// The page with the keyboard only: signing in, a cohort search, a patient's page and an entry,
/// by Tab, Shift+Tab, Enter and typing.  The focus is never lost (on the page itself, or on an
/// element that was drawn again) and always shows.
#[test]
fn the_page_by_keyboard_only() {
    let Some(Run { mut b, _rt, _tmp, .. }) = start() else { return };

    // signing in: the user name has the focus, Tab to the password, Enter
    b.until("document.activeElement && document.activeElement.id === 'login-user'", "the focus on the user name");
    b.type_text("boss");
    b.key("Tab");
    assert!(b.focused().starts_with("input#login-pass"), "{}", b.focused());
    b.type_text("0123456789ab");
    b.key("Enter");
    b.until("!document.querySelector('#app').classList.contains('hidden')", "the page after signing in");
    b.until("location.hash === '#/cohorts'", "the cohorts");
    std::thread::sleep(Duration::from_millis(500));
    let f = b.focused();
    assert!(!f.starts_with("lost"), "after signing in, the focus is {f}");

    // a cohort search: the expression button, the expression typed, a patient opened
    b.tab_to("e.id === 'co-addexpr'", 80, false);
    b.key("Enter");
    b.until("!!document.querySelector('#co-criteria input[data-k=\"text\"]')", "the expression's field");
    let f = b.focused();
    assert!(f.starts_with("input") && f.contains("Expression"), "the new expression has the focus: {f}");
    b.type_text("female");
    b.until("document.querySelector('#co-count').innerText.startsWith('1 patient')", "one patient found");
    let f = b.focused();
    assert!(f.contains("Expression"), "the results drawn, the focus stays in the expression: {f}");
    b.tab_to("e.matches('#co-results a[href^=\"#/patient/\"]')", 40, false);
    b.key("Enter");
    b.until(
        "document.querySelector('#pp-bar') && document.querySelector('#pp-bar').innerText.includes('Wexlorne')",
        "the patient's page",
    );
    b.until("document.querySelector('#view-patient').innerText.includes('Lactate')", "the report");
    std::thread::sleep(Duration::from_millis(500));
    let f = b.focused();
    assert!(!f.starts_with("lost"), "on the patient's page, the focus is {f}");

    // the patient's page: to an entry of the report, opened by Enter; the focus stays on it
    b.tab_to("e.matches('#pp-list [data-key]') && e.innerText.includes('Lactate')", 120, false);
    let key = b.eval("document.activeElement.dataset.key");
    b.key("Enter");
    b.until("document.querySelector('#pp-detail').innerText.includes('Lactate')", "the entry's detail");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(b.eval("document.activeElement.dataset.key"), key, "the entry opened keeps the focus: {}", b.focused());
    assert_eq!(b.eval("document.activeElement.getAttribute('aria-pressed')"), json!("true"), "and says it is chosen");
    // back by Shift+Tab to another entry (the note), opened by Space
    b.tab_to(&format!("e.matches('#pp-list [data-key]') && e.dataset.key !== {key}"), 20, true);
    let other = b.eval("document.activeElement.dataset.key");
    b.key(" ");
    b.until("document.querySelector('#pp-detail').innerText.includes('improving')", "the note's detail");
    assert_eq!(b.eval("document.activeElement.dataset.key"), other, "{}", b.focused());
    // and on through the whole page, round and back: no trap, every stop shown
    let stops = b.tab_through(80);
    assert!(stops.iter().any(|s| s == "(out of the page)"), "the Tab order goes round the page: {stops:?}");
    assert!(stops.iter().any(|s| s.contains("Cohorts")), "and comes back to the top bar: {stops:?}");
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}

/// The other screens, by their controls: the Review queue (a held-back row dismissed), Actions
/// (the index action run on every recording) and Jobs (the job done), and the VitalStream viewer
/// in its tab (the recording opened from the patient's page), when a viewer is built.
#[test]
#[cfg_attr(not(feature = "wfdb"), ignore = "WFDB recordings: a build without the `wfdb` feature refuses them")]
fn the_other_screens() {
    let Some(Run { mut b, cfg, _rt, _tmp, .. }) = start() else { return };
    sign_in(&mut b);

    // Review: the held-back row, dismissed (its confirmation accepted)
    b.eval("location.hash = '#/review'; true");
    b.until("document.querySelector('#view-review').innerText.includes('P440009')", "the held-back row");
    b.click("[data-held][data-act='dismiss']");
    b.until("!document.querySelector('#view-review').innerText.includes('P440009')", "the row dismissed");
    assert!(b.dialogs.iter().any(|d| d.contains("Dismiss this row")), "{:?}", b.dialogs);
    let review = api(&mut b, "review");
    assert!(review["held"].as_array().is_some_and(|h| h.is_empty()), "{review}");

    // Actions: the index action on every recording; the worker runs it; Jobs shows it done
    b.eval("location.hash = '#/actions'; true");
    b.until("document.querySelector('#act-table').innerText.includes('index')", "the actions");
    b.eval(
        "const i = document.querySelector('#act-table input[data-action=\"index\"]'); if (!i.checked) i.click(); true",
    );
    b.click("input[name='act-scope'][value='all']");
    b.click("#act-run");
    b.until("document.querySelector('#act-report').innerText.trim() !== ''", "the run's report");
    assert!(b.dialogs.iter().any(|d| d.contains("every recording")), "{:?}", b.dialogs);
    let jobs = api(&mut b, "jobs");
    assert_eq!(jobs["items"].as_array().map(|j| j.len()), Some(1), "{jobs}");
    let reg = std::sync::Arc::new(std::sync::RwLock::new(medorama::actions::Registry::load(&cfg)));
    let db = std::sync::Arc::new(Db::open(&cfg, false).unwrap());
    medorama::jobs::work(
        db,
        std::sync::Arc::new(cfg.clone()),
        reg,
        0,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        true,
    );
    b.eval("location.hash = '#/jobs'; true");
    b.until("document.querySelector('#view-jobs').innerText.toLowerCase().includes('done')", "the job done");
    assert!(b.errors.is_empty(), "{:?}", b.errors);

    // the viewer: the recording, from the patient's page, in the VitalStream tab
    if viewer().is_none() {
        eprintln!("no VitalStream viewer built: its part of the browser test is skipped");
        return;
    }
    let id = api(&mut b, "patients?q=P440001")["items"][0]["id"].clone();
    b.eval(&format!("location.hash = '#/patient/{id}'; true"));
    b.until("document.querySelector('#view-patient').innerText.includes('ICU')", "the patient's page");
    let rec = api(&mut b, "recordings")["items"][0]["path"].as_str().unwrap_or("").to_string();
    b.eval(&format!("location.hash = '#/viewer?path=' + encodeURIComponent({rec:?}); true"));
    b.until(
        "(() => { const f = document.querySelector('#viewer-frame'); const d = f && f.contentDocument;
           return !!(d && d.querySelector('canvas') && d.body.innerText.includes('II')); })()",
        "the recording drawn in the viewer",
    );
    let running = api(&mut b, "viewer");
    assert_eq!(running["running"], json!(true), "{running}");
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}

/// Add a database, each kind by the page: several files as one database; a known export
/// recognised (GE MUSE) and a site's own format listed; a database file connected (one outside
/// the folders data may come from refused), its persons linked, then disconnected.
#[test]
fn the_database_kinds() {
    let Some(Run { mut b, cfg, _rt, _tmp, .. }) = start() else { return };
    // a site's format, described in a file
    std::fs::create_dir_all(cfg.home.join("formats")).unwrap();
    std::fs::write(
        cfg.home.join("formats/acme-lab.toml"),
        "key = \"acme-lab\"\nlabel = \"ACME lab export\"\ncategory = \"lab\"\ndetect = [\"PatNo\", \"Analyte\"]\n\
         [[column]]\nname = \"patient_id\"\nfrom = \"PatNo\"\n[[column]]\nname = \"name\"\nfrom = \"Analyte\"\n\
         [[column]]\nname = \"value\"\nfrom = \"Res\"\nnumber = true\n[[column]]\nname = \"time\"\nfrom = \"When\"\ndate = \"%d.%m.%Y %H:%M\"\n",
    )
    .unwrap();
    sign_in(&mut b);
    b.eval("location.hash = '#/import'; true");
    b.until("!document.querySelector('#view-sources').classList.contains('hidden')", "the import page");
    b.until("document.querySelector('#fmt-table').innerText.includes('ACME lab export')", "the site's format listed");
    b.screenshot("import");
    assert!(b.text().contains("GE MUSE ECG metadata"), "the built-in format listed");

    // two files, one database (the site's format recognised in both)
    let dir = cfg.home.join("up");
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("acme_1.csv");
    let c2 = dir.join("acme_2.csv");
    std::fs::write(&a, "PatNo,Analyte,Res,When,Lab Tech\nP440001,Sodium,140,02.01.2024 08:00,Zorbluff\n").unwrap();
    std::fs::write(&c2, "PatNo,Analyte,Res,When,Lab Tech\nP440002,Sodium,133,03.01.2024 09:30,Zorbluff\n").unwrap();
    b.choose_files("#tb-files", &[&a, &c2]);
    b.until("document.querySelectorAll('#tb-list .tbl').length === 2", "both files read");
    assert!(b.eval("document.querySelector('#tb-list').innerText.includes('ACME lab export')") == json!(true));
    b.click("input[name='tb-mode'][value='one']");
    b.until("!document.querySelector('#tb-dbname').classList.contains('hidden')", "the database's name");
    b.fill("#tb-dbname", "acme_labs");
    b.click("#tb-preview");
    b.until("document.querySelector('#tb-list').innerText.includes('Preview: 2 rows')", "the preview of both files");
    b.click("#tb-import");
    b.until("document.querySelector('#imports-table').innerText.includes('acme_labs')", "one database imported");
    let dbs = api(&mut b, "databases");
    let d = dbs["items"].as_array().unwrap().iter().find(|d| d["name"] == "acme_labs").cloned().expect("acme_labs");
    assert_eq!((d["rows"].clone(), d["categories"].clone()), (json!(2), json!(["lab"])), "{d}");
    let id = d["id"].as_i64().unwrap();
    let rows = api(&mut b, &format!("databases/{id}/rows?limit=10")).to_string();
    assert!(
        rows.contains("2024-01-02 08:00:00") && !rows.contains("Zorbluff"),
        "converted, the undescribed column left out: {rows}"
    );

    // a CLIF folder on the server (inside data_roots): previewed, imported, each table a database
    let clif = cfg.home.parent().unwrap().join("data/clif");
    std::fs::create_dir_all(&clif).unwrap();
    std::fs::write(clif.join("patient.csv"), "patient_id,sex_category,birth_date\nCL01,Female,1960-01-01\n").unwrap();
    std::fs::write(
        clif.join("hospitalization.csv"),
        "patient_id,hospitalization_id,admission_dttm,discharge_dttm\nCL01,CLH01,2024-02-01 08:00:00+00:00,2024-02-05 08:00:00+00:00\n",
    )
    .unwrap();
    std::fs::write(
        clif.join("vitals.csv"),
        "hospitalization_id,recorded_dttm,vital_name,vital_category,vital_value\nCLH01,2024-02-01 09:00:00+00:00,HR,heart_rate,101\n",
    )
    .unwrap();
    b.eval("document.querySelector('.clifbox').open = true; true");
    b.fill("#clif-dir", &clif.to_string_lossy());
    b.click("#clif-preview");
    b.until("document.querySelector('#clif-report').innerText.includes('clif_vitals')", "the CLIF preview");
    b.click("#clif-import");
    b.until("document.querySelector('#imports-table').innerText.includes('clif_vitals')", "the CLIF tables imported");
    let names: Vec<String> = api(&mut b, "databases")["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["name"].as_str().map(String::from))
        .collect();
    for n in ["clif_patient", "clif_hospitalization", "clif_vitals"] {
        assert!(names.iter().any(|x| x == n), "{n} not among {names:?}");
    }

    // a database file: refused outside data_roots, connected inside, its persons linked, disconnected
    b.click("input[name='db-kind'][value='file']");
    b.until("!document.querySelector('#db-connect').classList.contains('hidden')", "the connection form");
    b.screenshot("connect");
    let outside = cfg.home.parent().unwrap().join("omop.sqlite");
    b.fill("#cn-name", "omop2");
    b.fill("#cn-path", &outside.to_string_lossy());
    b.click("#db-connect button.primary");
    b.until("document.querySelector('#toast').innerText.includes('outside the folders')", "refused outside data_roots");
    let inside = cfg.home.parent().unwrap().join("data/omop2.sqlite");
    std::fs::copy(&outside, &inside).unwrap();
    b.fill("#cn-path", &inside.to_string_lossy());
    b.click("#db-connect button.primary");
    b.until("document.querySelector('#cn-table').innerText.includes('omop2')", "connected and listed");
    assert!(b.eval("document.querySelector('#cn-result').innerText.startsWith('Connected')") == json!(true));
    let cid = api(&mut b, "connections")["items"].as_array().unwrap().iter().find(|c| c["name"] == "omop2").unwrap()
        ["id"]
        .clone();
    b.click(&format!("[data-cn-sync=\"{cid}\"]"));
    b.until("document.querySelector('#toast').innerText.includes('linked')", "its persons linked");
    b.click(&format!("[data-cn-rm=\"{cid}\"]"));
    b.until("!document.querySelector('#cn-table').innerText.includes('omop2')", "disconnected");
    assert!(b.dialogs.iter().any(|d| d.contains("Disconnect omop2")), "{:?}", b.dialogs);
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}

/// A cohort after new data: the page says so; its refresh lists the new patients, ticked; only
/// those still ticked are added, and nobody leaves.
#[test]
fn a_cohort_takes_its_new_patients() {
    let Some(Run { mut b, _rt, _tmp, .. }) = start() else { return };
    sign_in(&mut b);
    let post = |b: &mut Browser, path: &str, body: &str, kind: &str| {
        b.eval(&format!(
            "fetch('/api/v1/{path}', {{method: 'POST', headers: {{'X-Medorama': '1', 'Content-Type': '{kind}'}}, body: {body}}}).then(r => r.json())"
        ))
    };
    let co = post(
        &mut b,
        "cohorts",
        "JSON.stringify({name: 'Women', criteria: {include: {kind: 'expression', text: 'female'}}})",
        "application/json",
    );
    let id = co["id"].as_i64().unwrap_or_else(|| panic!("{co}"));
    let before = api(&mut b, &format!("cohorts/{id}"))["members"].as_i64().unwrap();
    let rows: String = (0..14).map(|i| format!("\\nPN{i},Newmark,Person{i},F")).collect();
    let r = post(&mut b, "tables?name=new.csv", &format!("'Patient ID,Last Name,First Name,Sex{rows}'"), "text/csv");
    assert_eq!(r["patients_added"], 14, "{r}");
    b.eval(&format!("location.hash = '#/cohorts/{id}'; true"));
    b.until("!!document.querySelector('#co-one .notice')", "the cohort says there are new data");
    b.screenshot("cohort-new-data");
    b.click("#co-one .notice [data-act=refresh]");
    b.until("!!document.querySelector('#co-add-new')", "who would join");
    assert_eq!(b.eval("document.querySelectorAll('#co-panel [data-join]:checked').length"), json!(14));
    assert_eq!(b.eval("document.querySelector('#co-add-new').textContent"), json!("Add the 14 new patients"));
    // one left out
    b.eval("document.querySelector('#co-panel [data-join]').click(); true");
    assert_eq!(b.eval("document.querySelector('#co-add-new').textContent"), json!("Add 13 chosen patients"));
    assert_eq!(b.eval("document.querySelector('#co-join-all').indeterminate"), json!(true));
    b.screenshot("cohort-new-patients");
    let problems = b.accessibility("a cohort's new patients");
    assert!(problems.is_empty(), "accessibility:\n{}", problems.join("\n"));
    b.eval("document.querySelector('#co-add-new').click(); true");
    b.until("!document.querySelector('#co-add-new') && !document.querySelector('#co-one .notice')", "added");
    assert_eq!(api(&mut b, &format!("cohorts/{id}"))["members"].as_i64().unwrap(), before + 13);
    // the one left out is still offered
    let r = post(&mut b, &format!("cohorts/{id}/refresh"), "JSON.stringify({dry_run: true})", "application/json");
    assert_eq!(r["added"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}

#[test]
fn the_demo_and_its_cohort() {
    let Some(Run { mut b, cfg, _rt, _tmp, .. }) = start() else { return };
    sign_in(&mut b);
    // a small demo, as tools/make-demo-data.py lays it out, inside the catalog's data_roots
    let dir = std::path::Path::new(&cfg.data_roots[0]).join("demo");
    std::fs::create_dir_all(dir.join("databases")).unwrap();
    std::fs::write(dir.join("README.txt"), "Make a synthetic demo data set for Medorama: fake patients.\n").unwrap();
    std::fs::write(
        dir.join("databases").join("patients.csv"),
        "MRN,Last Name,First Name,DOB,Sex,Patient ID\nP900001,Demoe,Ada,1950-01-02,F,DEMO-1\nP900002,Demoe,Ben,1960-03-04,M,DEMO-2\n",
    )
    .unwrap();
    let db = Db::open(&cfg, false).unwrap();
    let mut c = db.get().unwrap();
    let boss = medorama::users::get_by_name(&c, "boss").unwrap().unwrap();
    let d = medorama::demo::add(&mut c, &cfg, &boss, &dir).unwrap();
    drop(c);
    let id = d.cohort;
    // its patients' pages say DEMO
    let members = api(&mut b, &format!("cohorts/{id}/members"));
    let pid = members["items"][0]["id"].as_i64().unwrap_or_else(|| panic!("{members}"));
    b.eval(&format!("location.hash = '#/patient/{pid}'; true"));
    b.until(
        "document.querySelector('#pp-bar') && document.querySelector('#pp-bar').innerText.includes('DEMO')",
        "the demo patient's banner",
    );
    // the cohort: the demo's, for every user; no sharing; an admin deletes it, which says what goes
    b.eval(&format!("location.hash = '#/cohorts/{id}'; true"));
    b.until(
        "document.querySelector('#co-head') && document.querySelector('#co-head').innerText.includes('the demo, for every user')",
        "the demo's cohort",
    );
    assert!(b.eval("document.querySelector('#co-list').innerText.includes('the demo')").as_bool().unwrap());
    assert!(b.eval("!document.querySelector('.coacts [data-act=share]')").as_bool().unwrap());
    b.click(".coacts [data-act=delete]");
    b.until("!!document.querySelector('#co-delete-go')", "the demo's delete panel");
    let text = b.eval("document.querySelector('#co-panel').innerText").as_str().unwrap_or_default().to_string();
    assert!(text.contains("Delete the demo") && text.contains("2 patients"), "{text}");
    b.screenshot("demo-delete");
    let problems = b.accessibility("deleting the demo");
    assert!(problems.is_empty(), "accessibility:\n{}", problems.join("\n"));
    b.click("#co-delete-go");
    b.until("location.hash === '#/cohorts'", "back to the cohorts");
    assert!(api(&mut b, "cohorts")["items"].as_array().unwrap().iter().all(|c| c["name"] != "DEMO"));
    assert!(!medorama::demo::present(&db.get().unwrap()).unwrap());
    assert!(b.errors.is_empty(), "{:?}", b.errors);
}
