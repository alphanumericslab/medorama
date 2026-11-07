//! Helpers shared by the tests: synthetic WFDB recordings.
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

/// A bedside-monitor recording name UNIT_BED-<time> (built here: such names are kept out of the sources).
pub fn bed_name(unit: &str, bed: &str, t: i64) -> String {
    format!("{unit}_{bed}-{t}")
}

/// Batch options for a test import (name and source).
pub fn opts(name: &str) -> medorama::imports::Opts {
    medorama::imports::Opts::new(name, name, false)
}

/// Settings for a test catalog in `home`, results in `results`.  With
/// `MEDORAMA_TEST_POSTGRES=postgres://…` set, the catalog is in that PostgreSQL database, in a
/// schema of its own (so tests run side by side); else in SQLite, as by default.
pub fn config(home: &Path, results: &Path) -> medorama::config::Config {
    let mut cfg = medorama::config::Config::new(home, results);
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
