//! Index format 2 (a folder, memory-mapped) against format 3 (a store, plain and encrypted) on
//! one recording: indexing, opening, and the windows the page asks for (docs/performance.md,
//! "Index format 3").
//!
//!     cargo run --release -p recording-engine --example store_bench -- RECORDING WORKDIR
//!
//! WORKDIR (made, then emptied) holds the indexes; nothing is written beside the recording.
//! `$CHUNK_VALUES` sets the stores' chunk size (default: the engine's).

use recording_engine::dataset::Dataset;
use recording_engine::index;
use recording_engine::store::Store;
use signal_core::value::Value;
use std::path::Path;
use std::time::Instant;

/// One format's results: its name, its reads, indexing, opening and first window (s), its size.
type Row = (String, Vec<(&'static str, Vec<f64>)>, f64, f64, f64, u64);

/// Seconds of `f`.
fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed().as_secs_f64())
}

/// Median and 95th percentile, in milliseconds.
fn stats(mut v: Vec<f64>) -> String {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] * 1000.0;
    format!("{:8.2} {:8.2}", at(0.5), at(0.95))
}

fn size_of(p: &Path) -> u64 {
    if p.is_file() {
        return p.metadata().map_or(0, |m| m.len());
    }
    std::fs::read_dir(p).map(|d| d.flatten().map(|e| size_of(&e.path())).sum()).unwrap_or(0)
}

/// The page's reads on one dataset: (name, times) per kind.
fn reads(ds: &Dataset) -> Vec<(&'static str, Vec<f64>)> {
    let info = ds.info();
    let r = info.get("t_range").and_then(Value::as_list).cloned().unwrap_or_default();
    let (t0, t1) = (r[0].as_f64().unwrap(), r[1].as_f64().unwrap());
    let channels: Vec<i64> = (0..ds.channel_count() as i64).collect();
    let mut seed = 12345u64;
    let mut rand = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out = vec![];
    let mut whole = vec![];
    for _ in 0..5 {
        whole.push(time(|| ds.window_json(&channels, t0, t1, 1200).unwrap()).1);
    }
    out.push(("whole recording, 1200 px", whole));
    for (name, span) in [("10 min windows, 1200 px", 600.0), ("1 h windows, 1200 px", 3600.0)] {
        let mut v = vec![];
        for _ in 0..40 {
            let a = t0 + rand() * (t1 - t0 - span).max(0.0);
            v.push(time(|| ds.window_json(&channels, a, a + span, 1200).unwrap()).1);
        }
        out.push((name, v));
    }
    let mut v = vec![];
    for _ in 0..40 {
        let a = t0 + rand() * (t1 - t0 - 10.0).max(0.0);
        v.push(time(|| ds.window_json(&channels, a, a + 10.0, 1200).unwrap()).1);
    }
    out.push(("10 s windows (raw samples)", v));
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (rec, work) = (Path::new(&args[1]), Path::new(&args[2]));
    std::fs::create_dir_all(work).unwrap();
    let mut table: Vec<Row> = vec![];

    // format 2: a folder of memory-mapped files
    let cache = work.join("cache2");
    let (key, build) = time(|| index::build_index(rec, &cache, &|_| {}).unwrap());
    let ((meta, arrays), open) = time(|| index::load_index(&cache, &key).unwrap());
    let ds = Dataset::new(meta, arrays);
    let (_, first) = time(|| ds.window_json(&[0], 0.0, f64::MAX / 4.0, 1200).unwrap());
    table.push(("format 2 (folder)".into(), reads(&ds), build, open, first, size_of(&cache.join(&key))));
    drop(ds);

    // format 3: a store, plain then encrypted
    for (name, k) in [("format 3 (store)", None), ("format 3 (encrypted)", Some([5u8; 32]))] {
        let file = work.join(format!("{}.sqlite", if k.is_some() { "enc" } else { "plain" }));
        let chunk = std::env::var("CHUNK_VALUES").ok().and_then(|v| v.parse().ok());
        let store =
            Store::open_with(&file, k.as_ref(), chunk.unwrap_or(recording_engine::store::CHUNK_VALUES)).unwrap();
        let (key, build) = time(|| index::build_into_store(rec, &store, &|_| {}, &|| false).unwrap());
        drop(store);
        // opened afresh, as a viewer or Medorama would
        let store = Store::open(&file, k.as_ref()).unwrap();
        let ((meta, arrays), open) = time(|| store.load(&key).unwrap());
        let ds = Dataset::new(meta, arrays);
        let (_, first) = time(|| ds.window_json(&[0], 0.0, f64::MAX / 4.0, 1200).unwrap());
        table.push((name.into(), reads(&ds), build, open, first, size_of(&file)));
    }

    println!("{}", rec.display());
    for (name, rows, build, open, first, size) in &table {
        println!(
            "  {name}: index {build:.2} s, open {:.1} ms, first window {:.1} ms, {:.1} MB",
            open * 1e3,
            first * 1e3,
            *size as f64 / 1e6
        );
        for (what, v) in rows {
            println!("    {what:28} median, p95 (ms) {}", stats(v.clone()));
        }
    }
    let _ = std::fs::remove_dir_all(work);
}
