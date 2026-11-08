//! WFDB records whose signal file is a MATLAB file (`.mat`, docs/wfdb-collection.md, "MATLAB
//! signal files"): level 4 and level 5 files are read as WFDB reads them, from the header's byte
//! offset, and give the samples of the same record in a `.dat` file; a MAT file that does not
//! match its header (compressed, of doubles, transposed, of another offset or length) is
//! refused; and a signal file opened in place of its header says to open the header.

#![cfg(feature = "wfdb")] // a build without WFDB support reads no record

use recording_engine::index;
use recording_engine::sources::{self, wfdb_folder};
use std::path::Path;

/// Two signals of eight samples each: frame by frame, as a `.dat` file and a MAT matrix of two
/// rows (MATLAB stores a matrix column by column) both hold them.
const SAMPLES: [[i16; 2]; 8] = [[0, 800], [10, 810], [20, 790], [-15, 805], [5, 800], [30, 820], [-40, 780], [0, 800]];

fn frames_le() -> Vec<u8> {
    SAMPLES.iter().flatten().flat_map(|v| v.to_le_bytes()).collect()
}

fn checksums() -> [i16; 2] {
    let mut c = [0i16; 2];
    for f in SAMPLES {
        c[0] = c[0].wrapping_add(f[0]);
        c[1] = c[1].wrapping_add(f[1]);
    }
    c
}

/// A header of record `name`, its two signals in `file` of format `fmt` (with `+offset`), and
/// `nsamp` samples per signal.
fn header(dir: &Path, name: &str, file: &str, fmt: &str, nsamp: usize) {
    let [a, b] = checksums();
    let text = format!(
        "{name} 2 250 {nsamp} 11:00:00 01/03/2024\n{file} {fmt} 200/mV 16 0 0 {a} 0 II\n{file} {fmt} 10/mmHg 16 0 800 {b} 0 ART\n"
    );
    std::fs::write(dir.join(format!("{name}.hea")), text).unwrap();
}

/// A level 4 MAT file: its type (MOPT), rows, columns and the name `val`, then `data`.
fn mat4(ty: u32, rows: u32, cols: u32, data: &[u8], be: bool) -> Vec<u8> {
    let w = |v: u32| if be { v.to_be_bytes() } else { v.to_le_bytes() };
    let mut out = vec![];
    for v in [ty, rows, cols, 0, 4] {
        out.extend(w(v));
    }
    out.extend(b"val\0");
    out.extend(data);
    out
}

/// A level 5 MAT file of one matrix (little-endian): as wfdb2mat writes it (the name in a full
/// data element: the samples start at byte 192), or as MATLAB does (the name in a small one:
/// at byte 184).  `class`: the array's class; `elem`: the type of its elements.
fn mat5(rows: u32, cols: u32, data: &[u8], small_name: bool, class: u8, elem: u32, version: u16) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out[..10].copy_from_slice(b"MATLAB 5.0");
    out[124..126].copy_from_slice(&version.to_le_bytes());
    out[126..128].copy_from_slice(b"IM");
    let pad = (8 - data.len() % 8) % 8;
    let name: Vec<u8> = if small_name {
        let mut n = ((3u32 << 16) | 1).to_le_bytes().to_vec();
        n.extend(b"val\0");
        n
    } else {
        [1u32.to_le_bytes(), 3u32.to_le_bytes()].concat().into_iter().chain(*b"val\0\0\0\0\0").collect()
    };
    let mut body = vec![];
    body.extend([6u32.to_le_bytes(), 8u32.to_le_bytes()].concat());
    body.extend([class, 0, 0, 0, 0, 0, 0, 0]);
    body.extend([5u32.to_le_bytes(), 8u32.to_le_bytes(), rows.to_le_bytes(), cols.to_le_bytes()].concat());
    body.extend(name);
    body.extend([elem.to_le_bytes(), (data.len() as u32).to_le_bytes()].concat());
    body.extend(data);
    body.extend(vec![0u8; pad]);
    out.extend([14u32.to_le_bytes(), (body.len() as u32).to_le_bytes()].concat());
    out.extend(body);
    out
}

/// The index files of a record (all but meta.json, which names its files), to compare two
/// records holding the same samples.
fn arrays_of(hea: &Path, cache: &Path) -> Vec<(String, Vec<u8>)> {
    let key = index::build_index(hea, cache, &|_| {}).unwrap();
    let mut out: Vec<(String, Vec<u8>)> = std::fs::read_dir(cache.join(&key))
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_name() != "meta.json")
        .map(|e| (e.file_name().to_string_lossy().to_string(), std::fs::read(e.path()).unwrap()))
        .collect();
    out.sort();
    out
}

fn refusal(hea: &Path, cache: &Path) -> String {
    index::build_index(hea, cache, &|_| {}).unwrap_err().to_string()
}

#[test]
fn mat_files_are_read_as_the_dat_file() {
    let dir = tempfile::tempdir().unwrap();
    let (d, cache) = (dir.path(), dir.path().join("cache"));
    std::fs::write(d.join("r.dat"), frames_le()).unwrap();
    header(d, "r", "r.dat", "16", 8);
    // level 4 (the PhysioNet Challenge files: "16+24"), level 5 as wfdb2mat writes it ("+192")
    // and as MATLAB does ("+184")
    std::fs::write(d.join("m4.mat"), mat4(30, 2, 8, &frames_le(), false)).unwrap();
    header(d, "m4", "m4.mat", "16+24", 8);
    std::fs::write(d.join("m5.mat"), mat5(2, 8, &frames_le(), false, 10, 3, 0x0100)).unwrap();
    header(d, "m5", "m5.mat", "16+192", 8);
    std::fs::write(d.join("ml.mat"), mat5(2, 8, &frames_le(), true, 10, 3, 0x0100)).unwrap();
    header(d, "ml", "ml.mat", "16+184", 8);
    // big-endian: format 61
    let be: Vec<u8> = SAMPLES.iter().flatten().flat_map(|v| v.to_be_bytes()).collect();
    std::fs::write(d.join("mb.mat"), mat4(1030, 2, 8, &be, true)).unwrap();
    header(d, "mb", "mb.mat", "61+24", 8);
    let dat = arrays_of(&d.join("r.hea"), &cache);
    for name in ["m4", "m5", "ml", "mb"] {
        assert_eq!(arrays_of(&d.join(format!("{name}.hea")), &cache), dat, "{name}");
    }
}

#[test]
fn a_mat_file_that_does_not_match_its_header_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (d, cache) = (dir.path(), dir.path().join("cache"));
    let le = frames_le();
    let doubles: Vec<u8> = SAMPLES.iter().flatten().flat_map(|&v| f64::from(v).to_le_bytes()).collect();
    let mut compressed = mat5(2, 8, &le, false, 10, 3, 0x0100);
    compressed[128..132].copy_from_slice(&15u32.to_le_bytes());
    let cases: Vec<(&str, Vec<u8>, &str, usize, &str)> = vec![
        ("compressed", compressed, "16+192", 8, "it is compressed"),
        ("hdf5", mat5(2, 8, &le, false, 10, 3, 0x0200), "16+192", 8, "MATLAB 7.3 (HDF5)"),
        (
            "doubles",
            mat4(0, 2, 8, &doubles, false),
            "16+24",
            8,
            "it holds little-endian double-precision numbers; format 16 reads little-endian 16-bit integers",
        ),
        ("doubles5", mat5(2, 8, &doubles, false, 6, 9, 0x0100), "16+192", 8, "double-precision"),
        ("transposed", mat4(30, 8, 2, &le, false), "16+24", 8, "it is transposed"),
        ("offset", mat4(30, 2, 8, &le, false), "16+20", 8, "its samples start at byte 24; the header gives +20"),
        ("no_offset", mat4(30, 2, 8, &le, false), "16", 8, "its samples start at byte 24; the header gives +0"),
        ("length", mat4(30, 2, 8, &le, false), "16+24", 10, "it holds 8 samples per signal; the header gives 10"),
        ("short", mat4(30, 2, 8, &le[..20], false), "16+24", 8, "it is shorter than its 2 × 8 matrix"),
        (
            "order",
            mat4(1030, 2, 8, &le, true),
            "16+24",
            8,
            "it holds big-endian 16-bit integers; format 16 reads little-endian",
        ),
        ("format", mat4(30, 2, 8, &le, false), "212+24", 8, "format 212, which no MAT matrix holds"),
        ("text", b"hello, world: not a MAT file at all".to_vec(), "16", 8, "not a MAT file"),
    ];
    for (name, bytes, fmt, nsamp, why) in cases {
        std::fs::write(d.join(format!("{name}.mat")), bytes).unwrap();
        header(d, name, &format!("{name}.mat"), fmt, nsamp);
        let e = refusal(&d.join(format!("{name}.hea")), &cache);
        assert!(
            e.contains(&format!("{name}.mat does not match its header {name}.hea: ")) && e.contains(why),
            "{name}: {e}"
        );
    }
    // in a folder of records: left out with the reason, the others read
    std::fs::write(d.join("good.mat"), mat4(30, 2, 8, &le, false)).unwrap();
    header(d, "good", "good.mat", "16+24", 8);
    let m = index::load_index(&cache, &index::build_index(d, &cache, &|_| {}).unwrap()).unwrap().0;
    let skipped = m.get("skipped").and_then(|v| v.as_dict()).unwrap();
    assert!(skipped.get("transposed").and_then(|v| v.as_str()).unwrap().contains("transposed"), "{skipped:?}");
}

#[test]
fn a_signal_file_says_to_open_its_header() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(d.join("r.dat"), frames_le()).unwrap();
    header(d, "r", "r.dat", "16", 8);
    std::fs::write(d.join("lone.mat"), mat4(30, 2, 8, &frames_le(), false)).unwrap();
    // a header of the same name that names another file
    std::fs::write(d.join("other.dat"), frames_le()).unwrap();
    header(d, "other", "r.dat", "16", 8);
    let note = |f: &str| wfdb_folder::signal_file_note(&d.join(f));
    assert_eq!(note("r.dat").as_deref(), Some("r.dat is a WFDB signal file: open its header, r.hea"));
    assert_eq!(
        note("lone.mat").as_deref(),
        Some("lone.mat is a WFDB signal file: open the header (.hea) that names it")
    );
    assert_eq!(
        note("other.dat").as_deref(),
        Some("other.dat is a WFDB signal file: open the header (.hea) that names it")
    );
    assert_eq!(note("r.hea"), None);
    assert_eq!(note("missing.dat"), None);
    assert_eq!(sources::kind_of(&d.join("r.dat")), None);
    let e = refusal(&d.join("r.dat"), &d.join("cache"));
    assert_eq!(e, "r.dat is a WFDB signal file: open its header, r.hea");
}
