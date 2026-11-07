//! The adapter protocol end to end (docs/adapters.md), with a small adapter written in Python
//! for a made-up `.fake` format: found in its folder, described, chosen by extension, asked to
//! index a recording, to read its samples on demand (`serve`) and its identifiers; and the
//! limits on adapters that misbehave (`misbehaving_adapter.py`). Skipped where no `python3` is
//! installed.

use recording_engine::adapters::{self, Timeouts};
use recording_engine::{dataset::Dataset, index, sources};
use signal_core::value::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ADAPTER: &str = include_str!("fake_adapter.py");
const MISBEHAVING: &str = include_str!("misbehaving_adapter.py");

/// The tests change the engine's adapter folders and time limits: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// `script` installed as the program `name` in `folder`.
fn install(folder: &Path, name: &str, script: &str) -> PathBuf {
    std::fs::create_dir_all(folder).unwrap();
    let program = folder.join(name);
    std::fs::write(&program, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    program
}

fn python() -> bool {
    std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn an_adapter_end_to_end() {
    if !python() || cfg!(windows) {
        eprintln!("no python3: the adapter protocol test is skipped");
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    adapters::set_timeouts(Timeouts::default());
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("adapters");
    install(&folder, "fake-adapter", ADAPTER);
    let samples: Vec<f64> = (0..500).map(|i| (i as f64 / 10.0).sin()).collect();
    let rec = dir.path().join("rec.fake");
    let text = Value::Dict(signal_core::dict! {
        "fs" => 100.0, "t0" => 1700000000.0, "label" => "II", "unit" => "mV", "patient" => "Doe^Jane",
        "samples" => samples.clone(),
    })
    .to_json();
    std::fs::write(&rec, text).unwrap();

    // without the adapter: a file of an unknown format
    adapters::set_folders(vec![dir.path().join("none")]);
    assert_eq!(sources::kind_of(&rec), None);
    assert!(index::build_index(&rec, &dir.path().join("cache"), &|_| {}).is_err());

    adapters::set_folders(vec![folder.clone()]);
    let found = adapters::installed();
    assert_eq!(
        (found.len(), found[0].name.as_str(), found[0].formats[0].label.as_str()),
        (1, "fake", "Fake recording")
    );
    assert_eq!(sources::kind_of(&rec).as_deref(), Some("fake"));

    let progress = std::sync::Mutex::new(vec![]);
    let cache = dir.path().join("cache");
    let key = index::build_index(&rec, &cache, &|f| progress.lock().unwrap().push(f)).unwrap();
    assert!(progress.lock().unwrap().contains(&0.5));
    let (meta, arrays) = index::load_index(&cache, &key).unwrap();
    assert_eq!(meta.get("adapter").and_then(Value::as_str), Some("fake"));
    assert_eq!(meta.get("kind").and_then(Value::as_str), Some("fake"));
    let ds = Dataset::new(meta, arrays);
    // samples read on demand through the adapter's session, twice (the session is kept)
    assert_eq!(ds.samples(0, 10, 20).unwrap(), samples[10..20]);
    assert_eq!(ds.samples(0, 490, 500).unwrap(), samples[490..500]);
    let w = Value::from_json(&ds.window_json(&[0], 1700000000.0, 1700000001.0, 400).unwrap()).unwrap();
    assert!(w.py_str().contains("'raw'"), "{}", w.py_str());

    let ids = found[0].identify(std::slice::from_ref(&rec)).unwrap();
    let first = ids[Path::new(&rec)].get("identifiers").cloned().unwrap();
    assert_eq!(first.get("full_name").and_then(Value::as_str), Some("Doe^Jane"));
}

/// Short time limits for the tests of the limits.
fn short() -> Timeouts {
    Timeouts {
        describe: Duration::from_secs(2),
        identify_per_path: Duration::from_secs(1),
        index_stall: Duration::from_secs(1),
        serve_stall: Duration::from_secs(1),
    }
}

#[test]
fn a_noisy_adapter_does_not_block() {
    if !python() || cfg!(windows) {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    adapters::set_timeouts(Timeouts::default());
    let dir = tempfile::tempdir().unwrap();
    let a = adapters::describe(&install(&dir.path().join("ad"), "noisy-adapter", MISBEHAVING)).unwrap();
    // 300 kB on standard error before its first progress line
    let (rec, out) = (dir.path().join("r.noisy"), dir.path().join("out"));
    std::fs::write(&rec, "fine").unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let seen = Mutex::new(vec![]);
    a.index(&rec, &out, &|f| seen.lock().unwrap().push(f), &|| false).unwrap();
    assert_eq!(*seen.lock().unwrap(), [0.5]);
    // a failure: the message ends with the last line of its standard error, nothing more
    std::fs::write(&rec, "fail").unwrap();
    let e = a.index(&rec, &out, &|_| {}, &|| false).unwrap_err().to_string();
    assert!(e.ends_with(": the last words") && e.len() < 500, "{e}");
}

#[test]
fn a_hanging_adapter_is_stopped() {
    if !python() || cfg!(windows) {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    adapters::set_timeouts(short());
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("ad");
    let t = Instant::now();
    let e = adapters::describe(&install(&folder, "hangdescribe-adapter", MISBEHAVING)).unwrap_err().to_string();
    assert!(e.contains("did not answer within 2 s"), "{e}");
    let a = adapters::describe(&install(&folder, "hang-adapter", MISBEHAVING)).unwrap();
    let (rec, out) = (dir.path().join("r.hang"), dir.path().join("out"));
    std::fs::write(&rec, "x").unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let e = a.index(&rec, &out, &|_| {}, &|| false).unwrap_err().to_string();
    assert!(e.contains("no answer for 1 s"), "{e}");
    let e = a.identify(std::slice::from_ref(&rec)).unwrap_err().to_string();
    assert!(e.contains("did not answer within 1 s"), "{e}");
    // cancelling does not wait for a line from the adapter
    adapters::set_timeouts(Timeouts::default());
    let asked = Instant::now();
    let e = a.index(&rec, &out, &|_| {}, &|| asked.elapsed() > Duration::from_millis(300)).unwrap_err();
    assert_eq!(e.to_string(), "indexing cancelled");
    assert!(asked.elapsed() < Duration::from_secs(5));
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
}

#[test]
fn a_lying_session_is_stopped_and_started_again() {
    if !python() || cfg!(windows) {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    adapters::set_timeouts(short());
    let dir = tempfile::tempdir().unwrap();
    let a = adapters::describe(&install(&dir.path().join("ad"), "liar-adapter", MISBEHAVING)).unwrap();
    let ix = dir.path();
    let want: Vec<f64> = (10..20).map(f64::from).collect();
    assert_eq!(a.read(ix, 0, 10, 20).unwrap(), want);
    // more samples than asked, an absurd count, no JSON: errors, and the session is stopped
    for piece in [1, 2, 4] {
        let e = a.read(ix, piece, 10, 20).unwrap_err().to_string();
        assert!(e.contains("its session was stopped"), "piece {piece}: {e}");
        assert_eq!(a.read(ix, 0, 10, 20).unwrap(), want, "after piece {piece}");
    }
    // an error answered: the session stays in step
    assert_eq!(a.read(ix, 3, 0, 5).unwrap_err().to_string(), "no such piece");
    assert_eq!(a.read(ix, 0, 10, 20).unwrap(), want);
    // no answer: stopped after the stall time
    let e = a.read(ix, 5, 0, 5).unwrap_err().to_string();
    assert!(e.contains("no answer for 1 s"), "{e}");
    assert_eq!(a.read(ix, 0, 10, 20).unwrap(), want);
    // a long read is asked in requests of at most READ_CHUNK samples
    let n = adapters::READ_CHUNK + 3;
    let x = a.read(ix, 0, 0, n).unwrap();
    assert_eq!((x.len() as i64, x[n as usize - 1]), (n, (n - 1) as f64));
}

#[test]
fn an_adapter_gets_a_cleared_environment() {
    if !python() || cfg!(windows) {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    adapters::set_timeouts(Timeouts::default());
    std::env::set_var("MEDORAMA_KEY", "secret");
    let dir = tempfile::tempdir().unwrap();
    let a = adapters::describe(&install(&dir.path().join("ad"), "env-adapter", MISBEHAVING)).unwrap();
    // many paths: passed in several commands, each within the command-line limit
    let paths: Vec<PathBuf> = (0..400).map(|i| dir.path().join(format!("{i:0>100}.env"))).collect();
    let ids = a.identify(&paths).unwrap();
    assert_eq!(ids.len(), 400);
    for p in &paths {
        let first = ids[p].get("identifiers").unwrap();
        assert_eq!(first.get("full_name").and_then(Value::as_str), Some(""), "MEDORAMA_KEY passed on");
        assert_eq!(first.get("patient_id").and_then(Value::as_str), Some("PATH"), "PATH not passed on");
    }
}
