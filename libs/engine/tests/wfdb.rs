//! WFDB records (docs/wfdb-collection.md) on the synthetic recordings of
//! viewers/vitalstream/testdata: one record, a multi-segment record, one of its segments alone,
//! a folder of records in subfolders read as one session, the limits on what a folder may
//! hold, the index key of a record, and cancelling an index being built.

#![cfg(feature = "wfdb")] // a build without WFDB support: tests/no_wfdb.rs

use recording_engine::sources::wfdb_folder::{self, Limits};
use recording_engine::{index, sources};
use signal_core::value::{Dict, Value};
use std::path::{Path, PathBuf};

const T0: f64 = 1709287200.0;

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../viewers/vitalstream/testdata")
}

/// A copy of `src` (a folder) in `dst`.
fn copy(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), &to).unwrap();
        }
    }
}

fn meta_of(path: &Path, cache: &Path) -> Dict {
    let key = index::build_index(path, cache, &|_| {}).unwrap();
    index::load_index(cache, &key).unwrap().0
}

fn list<'a>(m: &'a Dict, k: &str) -> &'a [Value] {
    m.get(k).and_then(Value::as_list).unwrap()
}

fn s<'a>(d: &'a Value, k: &str) -> &'a str {
    d.get(k).and_then(Value::as_str).unwrap_or("")
}

fn f(d: &Value, k: &str) -> f64 {
    d.get(k).and_then(Value::as_f64).unwrap()
}

#[test]
fn a_folder_of_records_is_one_session() {
    let dir = tempfile::tempdir().unwrap();
    let m = meta_of(&testdata().join("session"), dir.path());
    assert_eq!(m.get("format").and_then(Value::as_str), Some("WFDB records"));
    let labels: Vec<&str> = list(&m, "channels").iter().map(|c| s(c, "label")).collect();
    assert_eq!(labels, ["ART", "II", "II (2)", "V"]);
    let pieces = list(&m, "pieces");
    let recs: Vec<&str> = pieces.iter().map(|p| s(p, "record")).collect();
    // subfolders in name order; the segments of the multi-segment records, never on their own;
    // the hidden folder left out
    assert_eq!(
        recs,
        [
            "abp/abp_a",
            "ecg/hr_a",
            "ecg/hr_b",
            "ecg/hr_c",
            "ms/ms_fix_0001",
            "ms/ms_fix_0002",
            "ms/ms_rec_0001",
            "ms/ms_rec_0001",
            "ms/ms_rec_0002"
        ]
    );
    let t: Vec<f64> = pieces.iter().map(|p| f(p, "t0") - T0).collect();
    assert_eq!(t, [7200.0, 7200.0, 7201.0, 7204.0, 7205.0, 7206.0, 7210.0, 7210.0, 7212.5]);
    // the overlapping record is in a second lane
    assert_eq!(pieces[2].get("ch").and_then(Value::as_int), Some(2));
    // without a start time: placed after the data before them
    let timed: Vec<bool> = pieces.iter().map(|p| p.get("timed") == Some(&Value::Bool(true))).collect();
    assert_eq!(timed, [true, true, true, false, false, false, true, true, true]);
    let gaps = list(&m, "gaps");
    assert_eq!(gaps.len(), 1);
    assert_eq!(
        (s(&gaps[0], "reason"), f(&gaps[0], "t0") - T0, f(&gaps[0], "t1") - T0),
        ("null segment", 7212.0, 7212.5)
    );
    assert!(list(&m, "notes").iter().any(|n| n.as_str().is_some_and(|n| n.contains("second lane"))));
}

#[test]
fn one_record() {
    let dir = tempfile::tempdir().unwrap();
    let ms = testdata().join("session/ms");
    assert_eq!(sources::kind_of(&ms.join("ms_rec.hea")).as_deref(), Some("wfdb"));
    assert_eq!(wfdb_folder::format_label(&ms.join("ms_rec.hea")), "WFDB multi-segment record");
    assert_eq!(wfdb_folder::format_label(&ms.join("ms_rec_0001.hea")), "WFDB record");

    // a multi-segment record: its segments from its start, the null segment a break, the
    // markers of the record in the segments they fall in (the one in the null segment left out)
    let m = meta_of(&ms.join("ms_rec.hea"), dir.path());
    assert_eq!(m.get("format").and_then(Value::as_str), Some("WFDB multi-segment record"));
    let pieces = list(&m, "pieces");
    let got: Vec<(&str, i128, f64)> =
        pieces.iter().map(|p| (s(p, "record"), p.get("n").and_then(Value::as_int).unwrap(), f(p, "t0") - T0)).collect();
    assert_eq!(got, [("ms_rec_0001", 500, 7210.0), ("ms_rec_0001", 500, 7210.0), ("ms_rec_0002", 250, 7212.5)]);
    let key = index::build_index(&ms.join("ms_rec.hea"), dir.path(), &|_| {}).unwrap();
    let (_, arrays) = index::load_index(dir.path(), &key).unwrap();
    assert_eq!(arrays.mk_piece.as_slice(), [0, 1, 2]);
    assert_eq!(arrays.mk_sample.as_slice(), [100, 200, 75]);

    // a segment alone: only that record, and its header states no start time
    let m = meta_of(&ms.join("ms_rec_0001.hea"), dir.path());
    let pieces = list(&m, "pieces");
    assert_eq!(pieces.len(), 2);
    assert!(pieces.iter().all(|p| p.get("timed") == Some(&Value::Bool(false))));

    // a record of a folder: that record only
    let m = meta_of(&testdata().join("plain/rec_a.hea"), dir.path());
    let labels: Vec<&str> = list(&m, "channels").iter().map(|c| s(c, "label")).collect();
    assert_eq!(labels, ["II", "ART"]);
}

#[test]
fn the_limits_of_a_folder() {
    let session = testdata().join("session");
    let scan = wfdb_folder::scan(&session, &Limits::default()).unwrap();
    let get = |k: &str| scan.get(k).cloned().unwrap();
    assert_eq!(get("records").as_int(), Some(6));
    assert_eq!(get("segments").as_int(), Some(4));
    assert_eq!(get("untimed").as_int(), Some(2));
    let signals: Vec<String> = get("signals").as_list().unwrap().iter().map(|x| x.py_str()).collect();
    assert_eq!(signals, ["ART", "II", "V"]);
    let r = get("t_range");
    let r = r.as_list().unwrap();
    assert_eq!((r[0].as_f64().unwrap() - T0, r[1].as_f64().unwrap() - T0), (7200.0, 7213.5));

    // over a limit: refused, with what to do instead (header files are counted, segments too)
    let few = Limits { max_records: 5, ..Limits::default() };
    let e = wfdb_folder::scan(&session, &few).unwrap_err().to_string();
    assert!(e.contains("more than 5 WFDB records") && e.contains("index.csv"), "{e}");
    let shallow = Limits { max_depth: 0, ..Limits::default() };
    let e = wfdb_folder::scan(&session, &shallow).unwrap_err().to_string();
    assert!(e.contains("nested more than 0 levels deep"), "{e}");
    let quick = Limits { max_scan_s: 0.0, ..Limits::default() };
    let e = wfdb_folder::scan(&session, &quick).unwrap_err().to_string();
    assert!(e.contains("took more than 0 s"), "{e}");

    // a folder without records
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "no records").unwrap();
    let e = wfdb_folder::scan(dir.path(), &Limits::default()).unwrap_err().to_string();
    assert!(e.contains("no WFDB records"), "{e}");
}

#[test]
fn the_key_of_a_record_follows_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let ms = dir.path().join("ms");
    copy(&testdata().join("session/ms"), &ms);
    let rec = ms.join("ms_rec.hea");
    let k0 = index::cache_key(&rec);
    let names: Vec<String> = wfdb_folder::fingerprint(&rec).into_iter().map(|x| x.0).collect();
    assert_eq!(
        names,
        [
            "ms_rec.hea",
            "ms_rec.mrk",
            "ms_rec_0001.dat",
            "ms_rec_0001.hea",
            "ms_rec_0002.dat",
            "ms_rec_0002.hea",
            "ms_rec_layout.hea"
        ]
    );
    // another record of the folder changes: not this one's key
    std::fs::write(ms.join("ms_fix_0001.dat"), [0u8; 4]).unwrap();
    assert_eq!(index::cache_key(&rec), k0);
    // one of its segments changes: a new key
    std::fs::write(ms.join("ms_rec_0002.dat"), [0u8; 4]).unwrap();
    assert_ne!(index::cache_key(&rec), k0);
}

#[test]
fn cancelling_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let e = index::build_index_until(&testdata().join("session"), &cache, &|_| {}, &|| true).unwrap_err();
    assert!(e.to_string().contains("cancelled"));
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
}

/// A single-signal record `name` in `dir`: its header line `head` and `samples` (16-bit, with
/// their checksum: wfdb-x leaves out a last sample that does not match it).
fn record(dir: &Path, name: &str, head: &str, samples: &[i16]) {
    std::fs::create_dir_all(dir).unwrap();
    let sum = samples.iter().fold(0i16, |a, &b| a.wrapping_add(b));
    std::fs::write(dir.join(format!("{name}.hea")), format!("{head}\n{name}.dat 16 200/mV 16 0 0 {sum} 0 II\n"))
        .unwrap();
    let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(dir.join(format!("{name}.dat")), bytes).unwrap();
}

#[test]
fn a_header_stating_too_much_is_read_to_the_end_of_its_data() {
    let dir = tempfile::tempdir().unwrap();
    record(dir.path(), "r", "r 1 250 1000000000000", &[1, 2, 3, 4, 5, 6, 7, 8]);
    let t = std::time::Instant::now();
    let m = meta_of(&dir.path().join("r.hea"), &dir.path().join("cache"));
    assert!(t.elapsed().as_secs() < 10);
    let p = &list(&m, "pieces")[0];
    assert_eq!(p.get("n").and_then(Value::as_int), Some(8));
    let scan = wfdb_folder::scan(&dir.path().join("r.hea"), &Limits::default()).unwrap();
    assert_eq!(scan.get("duration_s").and_then(Value::as_f64), Some(8.0 / 250.0));
}

#[test]
fn a_rate_that_is_not_a_number_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    record(&dir.path().join("f"), "a", "a 1 -nan 4", &[1, 2, 3, 4]);
    record(&dir.path().join("f"), "b", "b 1 250 4", &[1, 2, 3, 4]);
    let e = index::build_index(&dir.path().join("f/a.hea"), &dir.path().join("cache"), &|_| {}).unwrap_err();
    assert!(e.to_string().contains("no sampling frequency"), "{e}");
    // in a folder: left out, the other record read
    let m = meta_of(&dir.path().join("f"), &dir.path().join("cache"));
    assert_eq!(list(&m, "pieces").len(), 1);
    assert!(m.get("skipped").and_then(Value::as_dict).unwrap().contains_key("a"));
}

#[test]
fn a_collection_skips_what_it_cannot_read() {
    let dir = tempfile::tempdir().unwrap();
    let c = dir.path().join("c");
    record(&c, "good", "good 1 250 4", &[1, 2, 3, 4]);
    record(&dir.path().join("elsewhere"), "far", "far 1 250 4", &[1, 2, 3, 4]);
    // without a record column: a clear error, not a failure of the program
    std::fs::write(c.join("index.csv"), "name,start_unix\ngood,0\n").unwrap();
    let e = index::build_index(&c, &dir.path().join("cache"), &|_| {}).unwrap_err().to_string();
    assert!(e.contains("no record column"), "{e}");
    // records outside the folder (absolute, or with ..) and records that cannot be read are
    // left out with a note, as in a folder of plain records
    let far = dir.path().join("elsewhere/far").to_string_lossy().to_string();
    std::fs::write(c.join("index.csv"), format!("record,start_unix\ngood,0\nmissing,0\n../elsewhere/far,0\n{far},0\n"))
        .unwrap();
    let m = meta_of(&c, &dir.path().join("cache"));
    assert_eq!(list(&m, "pieces").len(), 1);
    let skipped = m.get("skipped").and_then(Value::as_dict).unwrap();
    let names: Vec<&String> = skipped.keys().collect();
    assert_eq!(names, ["missing", "../elsewhere/far", far.as_str()]);
    assert!(skipped["../elsewhere/far"].py_str().contains("outside its folder"));
    assert!(list(&m, "notes").iter().any(|n| n.py_str().contains("3 records could not be read")));
}

#[test]
fn writers_of_one_key_do_not_collide() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let keys: Vec<_> = std::thread::scope(|s| {
        let hs: Vec<_> =
            (0..4).map(|_| s.spawn(|| index::build_index(&testdata().join("session"), &cache, &|_| {}))).collect();
        hs.into_iter().map(|h| h.join().unwrap().unwrap()).collect()
    });
    assert!(keys.iter().all(|k| *k == keys[0]));
    assert!(index::load_index(&cache, &keys[0]).is_some());
    // nothing left but the index
    let names: Vec<String> =
        std::fs::read_dir(&cache).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(names, [keys[0].clone()]);
    // building it again keeps the complete index there
    let before = std::fs::metadata(cache.join(&keys[0]).join("meta.json")).unwrap().modified().unwrap();
    index::build_index(&testdata().join("session"), &cache, &|_| {}).unwrap();
    assert_eq!(std::fs::metadata(cache.join(&keys[0]).join("meta.json")).unwrap().modified().unwrap(), before);
}

#[test]
fn a_failure_while_building_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let r = std::panic::catch_unwind(|| {
        index::build_index_until(&testdata().join("plain"), &cache, &|_| {}, &|| panic!("a reader's bug"))
    });
    assert!(r.is_err());
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
}

#[test]
fn an_index_that_disagrees_with_its_arrays_is_not_used() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let key = index::build_index(&testdata().join("plain"), &cache, &|_| {}).unwrap();
    let folder = cache.join(&key);
    let text = std::fs::read_to_string(folder.join("meta.json")).unwrap();
    let Ok(Value::Dict(meta)) = Value::from_json(&text) else { panic!() };
    let set = |f: &dyn Fn(&mut Dict)| {
        let mut m = meta.clone();
        f(&mut m);
        std::fs::write(folder.join("meta.json"), Value::Dict(m).to_json()).unwrap();
        index::load_index(&cache, &key).is_some()
    };
    assert!(set(&|_| {}));
    let piece = |m: &mut Dict, k: &str, v: Value| {
        let Some(Value::List(ps)) = m.get_mut("pieces") else { panic!() };
        let Value::Dict(p) = &mut ps[0] else { panic!() };
        p.insert(k.into(), v);
    };
    // an envelope beyond its array, a rate of 0, a missing start, an unknown channel
    assert!(!set(&|m| piece(m, "env", Value::List(vec![Value::List(vec![0.into(), 1_000_000.into()])]))));
    assert!(!set(&|m| piece(m, "fs", 0.0.into())));
    assert!(!set(&|m| piece(m, "t0", Value::None)));
    assert!(!set(&|m| piece(m, "ch", 99.into())));
    assert!(!set(&|m| {
        m.insert(
            "vitals".into(),
            Value::List(vec![Value::Dict(signal_core::dict! { "name" => "x", "offset" => 5, "n" => 1 })]),
        );
    }));
    // an envelope level missing (an index half replaced)
    assert!(set(&|_| {}));
    std::fs::remove_file(folder.join("env_max_0.f32")).unwrap();
    assert!(index::load_index(&cache, &key).is_none());
}

#[test]
fn many_signals_are_read_a_chunk_at_a_time() {
    // 300 signals, 5000 frames: decoded in chunks of 4096 frames, every value in place
    let dir = tempfile::tempdir().unwrap();
    let (nsig, n) = (300usize, 5000usize);
    let mut head = format!("w {nsig} 100 {n}\n");
    for k in 0..nsig {
        let sum = (0..n).fold(0i16, |a, i| a.wrapping_add((i + k) as i16));
        head.push_str(&format!("w.dat 16 1/mV 16 0 0 {sum} 0 s{k}\n"));
    }
    std::fs::write(dir.path().join("w.hea"), head).unwrap();
    let bytes: Vec<u8> = (0..n).flat_map(|i| (0..nsig).flat_map(move |k| ((i + k) as i16).to_le_bytes())).collect();
    std::fs::write(dir.path().join("w.dat"), bytes).unwrap();
    let cache = dir.path().join("cache");
    let key = index::build_index(&dir.path().join("w.hea"), &cache, &|_| {}).unwrap();
    let (meta, arrays) = index::load_index(&cache, &key).unwrap();
    let ds = recording_engine::dataset::Dataset::new(meta, arrays);
    assert_eq!(ds.pieces.len(), nsig);
    let x = ds.samples(299, 4090, 4100).unwrap();
    assert_eq!(x, (4090..4100).map(|i| (i + 299) as f64).collect::<Vec<_>>());
    // a window of every channel decodes the record once per request, and answers each
    let t0 = ds.pieces[0].t0;
    let chs: Vec<i64> = (0..64).collect();
    let w = Value::from_json(&ds.window_json(&chs, t0 + 40.0, t0 + 41.0, 400).unwrap()).unwrap();
    let block = &w.get("channels").unwrap().get("63").unwrap().as_list().unwrap()[0];
    assert_eq!(block.get("kind").and_then(Value::as_str), Some("raw"));
    assert_eq!(block.get("y").unwrap().as_list().unwrap()[1].as_f64(), Some((4000 + 63) as f64));
}

#[cfg(unix)]
#[test]
fn a_folders_key_follows_linked_files() {
    // a link to a file counts with its target's size and time (wfdb-x reads through it); links
    // to folders do not
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("f");
    record(&f, "a", "a 1 250 4", &[1, 2, 3, 4]);
    std::fs::write(dir.path().join("target.dat"), [0u8; 8]).unwrap();
    std::os::unix::fs::symlink(dir.path().join("target.dat"), f.join("b.dat")).unwrap();
    std::os::unix::fs::symlink(dir.path(), f.join("up")).unwrap();
    let names: Vec<String> = wfdb_folder::fingerprint(&f).into_iter().map(|x| x.0).collect();
    assert_eq!(names, ["a.dat", "a.hea", "b.dat"]);
    let k0 = index::cache_key(&f);
    std::fs::write(dir.path().join("target.dat"), [0u8; 10]).unwrap();
    assert_ne!(index::cache_key(&f), k0);
}

#[test]
fn a_dotted_record_name_names_its_own_header() {
    // `rec.v1`'s header is `rec.v1.hea`, never `rec.hea`; wfdb-x, as the WFDB library, does not
    // read such a record, so indexing it is a clear error, not another record's data
    let dir = tempfile::tempdir().unwrap();
    let plain = testdata().join("plain");
    let header = std::fs::read_to_string(plain.join("rec_a.hea")).unwrap().replace("rec_a", "rec.v1");
    std::fs::write(dir.path().join("rec.v1.hea"), header).unwrap();
    std::fs::copy(plain.join("rec_a.dat"), dir.path().join("rec.v1.dat")).unwrap();
    std::fs::copy(plain.join("rec_a.hea"), dir.path().join("rec.hea")).unwrap();
    let names: Vec<String> =
        wfdb_folder::fingerprint(&dir.path().join("rec.v1.hea")).into_iter().map(|x| x.0).collect();
    assert_eq!(names, ["rec.v1.hea"]);
    assert!(index::build_index(&dir.path().join("rec.v1.hea"), &dir.path().join("cache"), &|_| {}).is_err());
}
