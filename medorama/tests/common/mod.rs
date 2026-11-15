//! Helpers shared by the tests: Medorama set up in a folder, db-builder databases and synthetic
//! WFDB recordings, all invented.
#![allow(dead_code)]

use std::path::Path;

/// 2024-01-01 12:00:00 UTC
pub const T0: i64 = 1_704_110_400;

/// A WFDB record (one ECG lead, 250 Hz, 20 s) in folder `dir`, starting at T0 + `offset_s`.
pub fn wfdb_record(dir: &Path, offset_s: i64) {
    std::fs::create_dir_all(dir).unwrap();
    let t = chrono::DateTime::from_timestamp(T0 + offset_s, 0).unwrap();
    let n = 5000;
    let samples: Vec<i16> = (0..n).map(|i| (100.0 * (i as f64 / 25.0).sin()) as i16).collect();
    // WFDB's checksum: the 16-bit sum of the samples (WFDB readers check it)
    let checksum = samples.iter().fold(0i16, |a, &x| a.wrapping_add(x));
    std::fs::write(
        dir.join("rec.hea"),
        format!(
            "rec 1 250 {n} {}\nrec.dat 16 200 12 0 {} {checksum} 0 II\n",
            t.format("%H:%M:%S %d/%m/%Y"),
            samples[0]
        ),
    )
    .unwrap();
    let mut b = Vec::with_capacity(n * 2);
    for x in &samples {
        b.extend(x.to_le_bytes());
    }
    std::fs::write(dir.join("rec.dat"), b).unwrap();
}

/// Settings for a test setup in `home`, its workspace `ws`, with its master key (one per test
/// process: the first loaded).  With
/// `MEDORAMA_TEST_POSTGRES=postgres://…` set, the catalog is in that PostgreSQL database, in a
/// schema of its own (so tests run side by side); else in SQLite, as by default.
pub fn config(home: &Path, ws: &Path) -> medorama::config::Config {
    let mut cfg = medorama::config::Config::new(home, ws);
    std::fs::create_dir_all(home).unwrap();
    std::fs::create_dir_all(ws).unwrap();
    medorama::workspace::make_master_file(&cfg).unwrap();
    medorama::workspace::load(&cfg).unwrap();
    if let Some(url) = std::env::var("MEDORAMA_TEST_POSTGRES").ok().filter(|u| !u.is_empty()) {
        let mut b = [0u8; 8];
        getrandom::fill(&mut b).unwrap();
        let schema = format!("t_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
        let c = medorama::sql::Connection::postgres(&url, None, &Default::default()).unwrap();
        c.execute_batch_native(&format!("CREATE SCHEMA {schema}")).unwrap();
        let sep = if url.contains('?') { '&' } else { '?' };
        cfg.database_url = format!("{url}{sep}options=-csearch_path%3D{schema},public");
    }
    cfg
}

/// Whether the tests run on PostgreSQL.
pub fn on_postgres() -> bool {
    std::env::var("MEDORAMA_TEST_POSTGRES").is_ok_and(|u| !u.is_empty())
}

/// Installs the tests' adapter for a made-up `.fake` recording format
/// (libs/engine/tests/fake_adapter.py) in `dir`, and makes it the adapter folder. False where no
/// `python3` runs it (the test is then skipped).
pub fn fake_adapter(dir: &Path) -> bool {
    if !fake_adapter_in(dir) {
        return false;
    }
    recording_engine::adapters::set_folders(vec![dir.to_path_buf()]);
    true
}

/// The mock adapter's program written into `dir`, without making it the adapter folder.
#[allow(dead_code)]
pub fn fake_adapter_in(dir: &Path) -> bool {
    let python = std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success());
    if !python || cfg!(windows) {
        eprintln!("no python3: the adapter test is skipped");
        return false;
    }
    std::fs::create_dir_all(dir).unwrap();
    let program = dir.join("fake-adapter");
    std::fs::write(&program, include_str!("../../../libs/engine/tests/fake_adapter.py")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    true
}

/// A `.fake` recording (read by [`fake_adapter`]) stating these identifiers.
pub fn fake_recording(path: &Path, patient: &str, patient_id: &str) {
    let samples: Vec<String> = (0..200).map(|i| format!("{:.3}", (i as f64 / 10.0).sin())).collect();
    std::fs::write(
        path,
        format!(
            r#"{{"fs": 100.0, "t0": {T0}.0, "label": "II", "unit": "mV", "patient": "{patient}", "patient_id": "{patient_id}", "samples": [{}]}}"#,
            samples.join(", ")
        ),
    )
    .unwrap();
}

/// A db-builder database (db-builder/docs/schema.md), written here: the tables Medorama reads.
pub struct Dbb {
    pub path: std::path::PathBuf,
    c: rusqlite::Connection,
    version: u32,
}

impl Dbb {
    pub fn new(path: &Path) -> Dbb {
        let _ = std::fs::remove_file(path);
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE persons (person_key TEXT PRIMARY KEY, patient_id TEXT NOT NULL DEFAULT '', patient_id_issuer TEXT NOT NULL DEFAULT '',
                 mrn TEXT NOT NULL DEFAULT '', mrn_authority TEXT NOT NULL DEFAULT '', last_name TEXT NOT NULL DEFAULT '',
                 first_name TEXT NOT NULL DEFAULT '', birth_date TEXT NOT NULL DEFAULT '', sex TEXT NOT NULL DEFAULT '');
             CREATE TABLE visits (visit_key TEXT PRIMARY KEY, person_key TEXT, start_time TEXT, end_time TEXT,
                 unit TEXT NOT NULL DEFAULT '', icu INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE entries (entry_key TEXT PRIMARY KEY, person_key TEXT, visit_key TEXT, category TEXT NOT NULL DEFAULT '',
                 time TEXT, end_time TEXT, name TEXT NOT NULL DEFAULT '', code TEXT NOT NULL DEFAULT '', vocabulary TEXT NOT NULL DEFAULT '',
                 value_num REAL, value_text TEXT NOT NULL DEFAULT '', unit TEXT NOT NULL DEFAULT '', ref_low REAL, ref_high REAL,
                 flag TEXT NOT NULL DEFAULT '', author TEXT NOT NULL DEFAULT '', text TEXT NOT NULL DEFAULT '');
             CREATE TABLE recordings (recording_key TEXT PRIMARY KEY, person_key TEXT, visit_key TEXT, file TEXT NOT NULL,
                 format TEXT NOT NULL DEFAULT '', start_time TEXT, end_time TEXT, label TEXT NOT NULL DEFAULT '',
                 category TEXT NOT NULL DEFAULT '', size INTEGER, fingerprint TEXT NOT NULL DEFAULT '');
             INSERT INTO meta VALUES ('schema', '1'), ('version', 'v1'), ('timezone', 'UTC');",
        )
        .unwrap();
        Dbb { path: path.to_path_buf(), c, version: 1 }
    }

    pub fn url(&self) -> String {
        format!("sqlite:{}", self.path.display())
    }

    /// A person: key, patient ID and its issuer, MRN and its authority, names, birth date, sex.
    #[allow(clippy::too_many_arguments)]
    pub fn person(
        &self,
        key: &str,
        pid: &str,
        issuer: &str,
        mrn: &str,
        authority: &str,
        last: &str,
        first: &str,
        dob: &str,
        sex: &str,
    ) -> &Self {
        self.c
            .execute(
                "INSERT INTO persons VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![key, pid, issuer, mrn, authority, last, first, dob, sex],
            )
            .unwrap();
        self
    }

    pub fn visit(&self, key: &str, person: &str, start: &str, end: &str, unit: &str, icu: bool) -> &Self {
        self.c
            .execute(
                "INSERT INTO visits VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![key, person, start, end, unit, icu as i64],
            )
            .unwrap();
        self
    }

    /// A result: key, person, visit, category, time, name, value, unit.
    #[allow(clippy::too_many_arguments)]
    pub fn entry(
        &self,
        key: &str,
        person: &str,
        visit: &str,
        category: &str,
        time: &str,
        name: &str,
        value: Option<f64>,
        unit: &str,
    ) -> &Self {
        self.c
            .execute(
                "INSERT INTO entries (entry_key, person_key, visit_key, category, time, name, value_num, unit) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![key, person, visit, category, time, name, value, unit],
            )
            .unwrap();
        self
    }

    /// A note, with its text.
    pub fn note(&self, key: &str, person: &str, time: &str, name: &str, text: &str) -> &Self {
        self.c
            .execute(
                "INSERT INTO entries (entry_key, person_key, category, time, name, text, author) VALUES (?1, ?2, 'note', ?3, ?4, ?5, 'Dr Invented')",
                rusqlite::params![key, person, time, name, text],
            )
            .unwrap();
        self
    }

    /// A recording: key, person, its file (relative to this database's folder), category.
    pub fn recording(&self, key: &str, person: &str, file: &str, category: &str) -> &Self {
        self.c
            .execute(
                "INSERT INTO recordings (recording_key, person_key, file, category) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![key, person, file, category],
            )
            .unwrap();
        self
    }

    /// The data changed: a new version.
    pub fn bump(&mut self) {
        self.version += 1;
        self.c.execute("UPDATE meta SET value = ?1 WHERE key = 'version'", [format!("v{}", self.version)]).unwrap();
    }

    pub fn exec(&self, sql: &str) {
        self.c.execute_batch(sql).unwrap();
    }
}

/// Medorama set up in a temporary folder: its settings, control store and workspace, an admin
/// (`boss`, PHI) and an analyst without the PHI permission (`reader`), both with the password
/// `0123456789ab`.
pub struct Setup {
    pub tmp: tempfile::TempDir,
    pub cfg: medorama::config::Config,
    pub db: std::sync::Arc<medorama::db::Db>,
}

pub const PASSWORD: &str = "0123456789ab";

impl Setup {
    pub fn new() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let root = medorama::config::real(tmp.path());
        let mut cfg = config(&root.join("home"), &root.join("ws"));
        cfg.data_roots = vec![root.join("data").to_string_lossy().to_string()];
        std::fs::create_dir_all(root.join("data")).unwrap();
        let db = std::sync::Arc::new(medorama::db::Db::open(&cfg, true).unwrap());
        {
            let c = db.get().unwrap();
            medorama::users::add(&c, "boss", "", medorama::users::Role::Admin, true, PASSWORD).unwrap();
            medorama::users::add(&c, "reader", "", medorama::users::Role::Analyst, false, PASSWORD).unwrap();
        }
        Setup { tmp, cfg, db }
    }

    /// The data folder (inside data_roots).
    pub fn data(&self) -> std::path::PathBuf {
        medorama::config::real(self.tmp.path()).join("data")
    }

    pub fn user(&self, name: &str) -> medorama::users::User {
        medorama::users::get_by_name(&self.db.get().unwrap(), name).unwrap().unwrap()
    }

    /// Connect a db-builder database, as `boss`.
    pub fn connect(&self, name: &str, d: &Dbb) -> i64 {
        let n =
            medorama::sources::NewSource { name: name.into(), kind: "dbb".into(), url: d.url(), ..Default::default() };
        medorama::databases::connect(&self.cfg, &self.db.get().unwrap(), &self.user("boss"), &n, "").unwrap()
    }

    pub fn app(&self) -> axum::Router {
        medorama::server::router(std::sync::Arc::new(medorama::server::App::new(
            std::sync::Arc::new(self.cfg.clone()),
            self.db.clone(),
        )))
    }
}
