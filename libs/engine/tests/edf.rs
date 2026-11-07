//! EDF, EDF+ and BDF files (viewers/vitalstream/docs/index-format.md, "EDF, EDF+ and BDF") on the
//! synthetic files of viewers/vitalstream/testdata/edf: channels and rates, the start of the
//! first record (EDF+ time keeping), records in runs with breaks (EDF+D), annotations as events,
//! a two-digit year, a record cut short, 24-bit samples, and the patient field never read.

use recording_engine::sources::{self, edf};
use recording_engine::{dataset, index};
use signal_core::value::{Dict, Value};
use std::path::{Path, PathBuf};

const START: f64 = 1709298000.0; // 2024-03-01 13:00:00 UTC

fn file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../viewers/vitalstream/testdata/edf").join(name)
}

fn indexed(name: &str, cache: &Path) -> (String, Dict) {
    let key = index::build_index(&file(name), cache, &|_| {}).unwrap();
    let meta = index::load_index(cache, &key).unwrap().0;
    (key, meta)
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
fn an_edf_plus_file() {
    let cache = tempfile::tempdir().unwrap();
    let p = file("psg_plus.edf");
    assert_eq!(sources::kind_of(&p).as_deref(), Some("edf"));
    assert_eq!(edf::format_label(&p), "EDF+");
    let (key, m) = indexed("psg_plus.edf", cache.path());
    assert_eq!((s(&Value::Dict(m.clone()), "kind"), s(&Value::Dict(m.clone()), "format")), ("edf", "EDF+"));
    let ch: Vec<(&str, &str)> = list(&m, "channels").iter().map(|c| (s(c, "label"), s(c, "unit"))).collect();
    assert_eq!(ch, [("ECG", "mV"), ("Resp", ""), ("SpO2", "%")]);
    let pieces = list(&m, "pieces");
    let got: Vec<(f64, i128, f64)> =
        pieces.iter().map(|p| (f(p, "fs"), p.get("n").and_then(Value::as_int).unwrap(), f(p, "t0") - START)).collect();
    // the first record starts 0.25 s after the header's time (its time-keeping annotation)
    assert_eq!(got, [(250.0, 5000, 0.25), (25.0, 500, 0.25), (1.0, 20, 0.25)]);
    assert!(list(&m, "gaps").is_empty());
    let ev: Vec<(f64, f64, &str, &str)> = list(&m, "alarms")
        .iter()
        .map(|a| (f(a, "t0") - START, f(a, "t1") - START, s(a, "text"), s(a, "kind")))
        .collect();
    assert_eq!(
        ev,
        [
            (0.25, 10.25, "Sleep stage W", "annotation"),
            (0.5, 0.5, "Lights off", "annotation"),
            (4.25, 7.25, "Arousal", "annotation")
        ]
    );
    assert!(list(&m, "alarms").iter().all(|a| a.get("level") == Some(&Value::None)));
    // the patient field (an invented identity here) is in no file of the index
    for e in std::fs::read_dir(index::index_dir(cache.path(), &key)).unwrap() {
        let bytes = std::fs::read(e.unwrap().path()).unwrap();
        assert!(!bytes.windows(9).any(|w| w == b"Synthetic") && !bytes.windows(7).any(|w| w == b"MO-TEST"));
    }
    // samples: the physical values the generator wrote, to within one digital step
    let ds = dataset::Dataset::new(m.clone(), index::load_index(cache.path(), &key).unwrap().1);
    let x = ds.samples(2, 0, 20).unwrap();
    for (k, v) in x.iter().enumerate() {
        let t = 0.25 + k as f64;
        assert!((v - (96.0 + (t / 3.0).sin())).abs() <= 0.1 / 2.0 + 1e-9, "SpO2 {k}: {v}");
    }
    let r = ds.samples(1, 498, 600).unwrap();
    assert_eq!(r.len(), 2);
}

#[test]
fn records_in_runs() {
    let cache = tempfile::tempdir().unwrap();
    let (_, m) = indexed("gappy_plus.edf", cache.path());
    let pieces = list(&m, "pieces");
    let got: Vec<(f64, i128, i128)> = pieces
        .iter()
        .map(|p| {
            (
                f(p, "t0") - START,
                p.get("n").and_then(Value::as_int).unwrap(),
                p.get("data_record").and_then(Value::as_int).unwrap(),
            )
        })
        .collect();
    assert_eq!(got, [(0.0, 375, 0), (3.0, 250, 3), (10.0, 125, 5)]);
    let gaps: Vec<(f64, f64, &str)> =
        list(&m, "gaps").iter().map(|g| (f(g, "t0") - START, f(g, "t1") - START, s(g, "reason"))).collect();
    assert_eq!(gaps, [(1.5, 3.0, "not recorded"), (4.0, 10.0, "not recorded")]);
    let ev: Vec<&str> = list(&m, "alarms").iter().map(|a| s(a, "text")).collect();
    assert_eq!(ev, ["Electrode off"]);
}

#[test]
fn plain_edf_and_bdf() {
    let cache = tempfile::tempdir().unwrap();
    let (_, m) = indexed("plain.edf", cache.path());
    assert_eq!(m.get("format").and_then(Value::as_str), Some("EDF"));
    let pieces = list(&m, "pieces");
    // 1999 (a two-digit year); the number of records is unknown: the 8 whole records of the file
    assert_eq!(f(&pieces[0], "t0"), 920368800.0);
    assert_eq!(pieces.iter().map(|p| p.get("n").and_then(Value::as_int).unwrap()).collect::<Vec<_>>(), [1600, 8]);
    let notes: Vec<String> = list(&m, "notes").iter().map(Value::py_str).collect();
    assert!(notes.iter().any(|n| n.contains("ends inside a data record")), "{notes:?}");
    assert!(list(&m, "alarms").is_empty());

    let (key, m) = indexed("eeg_plus.bdf", cache.path());
    assert_eq!(m.get("format").and_then(Value::as_str), Some("BDF+"));
    let ch: Vec<&str> = list(&m, "channels").iter().map(|c| s(c, "label")).collect();
    assert_eq!(ch, ["Fp1", "Flat"]);
    let ds = dataset::Dataset::new(m.clone(), index::load_index(cache.path(), &key).unwrap().1);
    // a digital range of zero: the digital values themselves
    assert_eq!(ds.samples(1, 0, 12).unwrap(), vec![7.0; 12]);
    let x = ds.samples(0, 0, 768).unwrap();
    let step = (262143.0 + 262144.0) / (8388607.0 + 8388608.0);
    for (k, v) in x.iter().enumerate() {
        let t = k as f64 / 256.0;
        assert!((v - 80.0 * (2.0 * std::f64::consts::PI * 9.0 * t).sin()).abs() <= step, "Fp1 {k}: {v}");
    }
    let ev: Vec<(f64, &str)> = list(&m, "alarms").iter().map(|a| (f(a, "t0") - START, s(a, "text"))).collect();
    assert_eq!(ev, [(1.5, "Eyes closed")]);
}

#[test]
fn not_edf() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.edf");
    std::fs::write(&p, b"not an EDF file at all").unwrap();
    let e = index::build_index(&p, &dir.path().join("cache"), &|_| {}).unwrap_err().to_string();
    assert!(e.contains("not an EDF or BDF file"), "{e}");
}

/// An EDF+D file of `runs` data records of one second, each starting a second after the end
/// of the one before (a break between every two): one signal of `spr` samples per record.
fn discontinuous(path: &Path, runs: usize, spr: usize, duration: &str) {
    let field = |s: &str, w: usize| format!("{s:<w$}");
    let ann = 16usize; // samples of the annotation signal (32 bytes)
    let mut h = String::new();
    h += &field("0", 8);
    h += &field("X X X X", 80);
    h += &field("Startdate 01-MAR-2024 X X X", 80);
    h += "01.03.2413.00.00";
    h += &field(&(256 * 3).to_string(), 8);
    h += &field("EDF+D", 44);
    h += &field(&runs.to_string(), 8);
    h += &field(duration, 8);
    h += &field("2", 4);
    h += &(field("ECG", 16) + &field("EDF Annotations", 16));
    h += &field("", 160);
    h += &(field("mV", 8) + &field("", 8));
    h += &(field("-1", 8) + &field("-1", 8));
    h += &(field("1", 8) + &field("1", 8));
    h += &(field("-32768", 8) + &field("-32768", 8));
    h += &(field("32767", 8) + &field("32767", 8));
    h += &field("", 160);
    h += &(field(&spr.to_string(), 8) + &field(&ann.to_string(), 8));
    h += &field("", 64);
    let mut bytes = h.into_bytes();
    for r in 0..runs {
        bytes.extend((0..spr).flat_map(|i| (i as i16).to_le_bytes()));
        let mut tal = format!("+{}\x14\x14\0", 2 * r).into_bytes();
        tal.resize(2 * ann, 0);
        bytes.extend(tal);
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn many_breaks_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    // more pieces than px in a window: one block per pixel column at most
    let p = dir.path().join("runs.edf");
    discontinuous(&p, 3000, 4, "1");
    let cache = dir.path().join("cache");
    let key = index::build_index(&p, &cache, &|_| {}).unwrap();
    let (meta, arrays) = index::load_index(&cache, &key).unwrap();
    let ds = dataset::Dataset::new(meta, arrays);
    assert_eq!(ds.pieces.len(), 3000);
    let t0 = ds.pieces[0].t0;
    let w = Value::from_json(&ds.window_json(&[0], t0, t0 + 6000.0, 100).unwrap()).unwrap();
    let blocks = w.get("channels").unwrap().get("0").unwrap().as_list().unwrap().clone();
    assert!(blocks.len() <= 100 && blocks.len() >= 90, "{}", blocks.len());
    // a narrow window: every piece in it, in order
    let w = Value::from_json(&ds.window_json(&[0], t0 + 10.0, t0 + 20.0, 400).unwrap()).unwrap();
    let ids: Vec<i128> = w
        .get("channels")
        .unwrap()
        .get("0")
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .map(|b| b.get("piece").and_then(Value::as_int).unwrap())
        .collect();
    assert_eq!(ids, [5, 6, 7, 8, 9, 10]);
    // more pieces than a recording may have: refused
    let p = dir.path().join("too_many.edf");
    discontinuous(&p, recording_engine::common::MAX_PIECES + 1, 1, "1");
    let e = index::build_index(&p, &cache, &|_| {}).unwrap_err().to_string();
    assert!(e.contains("more than 100000 pieces"), "{e}");
}

#[test]
fn a_rate_that_is_not_finite_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("tiny.edf");
    discontinuous(&p, 2, 4, "1e-320");
    let e = edf::header(&p).unwrap_err().to_string();
    assert!(e.contains("no finite sampling rate"), "{e}");
    // the same header read from bytes
    let e = edf::header_from_bytes(&std::fs::read(&p).unwrap()).unwrap_err().to_string();
    assert!(e.contains("no finite sampling rate"), "{e}");
    discontinuous(&p, 2, 4, "1");
    let h = edf::header_from_bytes(&std::fs::read(&p).unwrap()).unwrap();
    assert_eq!((h.records, h.signals.len()), (2, 2));
    assert!(edf::header_from_bytes(&std::fs::read(&p).unwrap()[..300]).is_err());
    assert!(edf::header_from_bytes(b"").is_err());
}
