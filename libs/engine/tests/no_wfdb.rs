//! A build without WFDB support (`cargo test -p recording-engine --no-default-features`): the
//! WFDB recordings of viewers/vitalstream/testdata (one record, a multi-segment record, a
//! folder of records, a collection) are recognised and refused with one message, neither
//! indexed nor read, while EDF files are read as in the default build.

#![cfg(not(feature = "wfdb"))]

use recording_engine::sources::{self, wfdb_folder, NO_WFDB};
use recording_engine::{dataset::Dataset, index};
use signal_core::dict;
use std::path::{Path, PathBuf};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../viewers/vitalstream/testdata").join(name)
}

#[test]
fn a_wfdb_record_is_refused_and_edf_is_read() {
    let cache = tempfile::tempdir().unwrap();
    // a folder of subfolders of records ("session") is no recording when a tree is searched, but
    // opens as the records below it
    for (name, kind) in [
        ("plain/rec_a.hea", Some("wfdb")),
        ("session/ms/ms_rec.hea", Some("wfdb")),
        ("plain", Some("wfdb")),
        ("collection", Some("wfdb")),
        ("session", None),
    ] {
        let p = testdata(name);
        assert_eq!(sources::kind_of(&p).as_deref(), kind, "{name}: still recognised");
        assert_eq!(sources::unreadable(&p), Some(NO_WFDB), "{name}");
        let e = index::build_index(&p, cache.path(), &|_| {}).unwrap_err();
        assert_eq!(e.to_string(), NO_WFDB, "{name}");
        let e = wfdb_folder::scan(&p, &wfdb_folder::Limits::default()).unwrap_err();
        assert_eq!(e.to_string(), NO_WFDB, "{name}");
    }
    assert_eq!(wfdb_folder::format_label(&testdata("session/ms/ms_rec.hea")), "WFDB multi-segment record");
    // nothing was written into the cache
    assert_eq!(
        std::fs::read_dir(cache.path()).unwrap().filter_map(Result::ok).filter(|e| e.path().is_dir()).count(),
        0
    );
    // the samples of a WFDB index made by a build with WFDB support
    let meta = dict! { "kind" => "wfdb", "path" => testdata("plain").to_string_lossy().to_string() };
    let e = sources::Reader::new(&meta, cache.path()).read(0, 0, 10).unwrap_err();
    assert_eq!(e.to_string(), NO_WFDB);

    // EDF: read as always
    let edf = testdata("edf/psg_plus.edf");
    assert_eq!(sources::kind_of(&edf).as_deref(), Some("edf"));
    assert_eq!(sources::unreadable(&edf), None);
    let key = index::build_index(&edf, cache.path(), &|_| {}).unwrap();
    let (meta, arrays) = index::load_index(cache.path(), &key).unwrap();
    let ds = Dataset::new(meta, arrays);
    let n = ds.pieces[0].n;
    assert!(n > 0);
    assert_eq!(ds.samples(0, 0, n).unwrap().len() as i64, n);
}
