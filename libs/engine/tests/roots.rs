//! Files outside the data roots (viewers/vitalstream/docs/index-format.md, "Files outside the
//! data roots"): a signal file that is a link to a file elsewhere, or a header naming one by an
//! absolute path, is never opened, neither while indexing nor when samples are read.  One test
//! only: the allowed roots are the process's.

#![cfg(feature = "wfdb")] // a build without WFDB support: tests/no_wfdb.rs

use recording_engine::{dataset::Dataset, index, paths};
use signal_core::value::Value;
use std::path::Path;

fn record(dir: &Path, name: &str, dat: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{name}.hea")), format!("{name} 1 250 4\n{dat} 16 200/mV 16 0 0 10 0 II\n"))
        .unwrap();
}

#[cfg(unix)]
#[test]
fn files_outside_the_roots_are_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let (root, outside) = (dir.path().join("root"), dir.path().join("outside"));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.dat"), [1u8, 0, 2, 0, 3, 0, 4, 0]).unwrap();
    record(&root, "good", "good.dat");
    std::fs::write(root.join("good.dat"), [1u8, 0, 2, 0, 3, 0, 4, 0]).unwrap();
    // a link to a file outside, and a header naming one by its absolute path
    record(&root, "linked", "linked.dat");
    std::os::unix::fs::symlink(outside.join("secret.dat"), root.join("linked.dat")).unwrap();
    record(&root, "absolute", &outside.join("secret.dat").to_string_lossy());
    let cache = dir.path().join("cache");

    // without roots (a library's default), anything is read
    paths::set_allowed_roots(None);
    assert!(index::build_index(&root.join("linked.hea"), &cache, &|_| {}).is_ok());

    paths::set_allowed_roots(Some(vec![root.to_string_lossy().to_string()]));
    for name in ["linked", "absolute"] {
        let e = index::build_index(&root.join(format!("{name}.hea")), &cache, &|_| {}).unwrap_err().to_string();
        assert!(e.contains("outside the data roots"), "{e}");
    }
    // in a folder: those records are left out, the others read
    let key = index::build_index(&root, &cache, &|_| {}).unwrap();
    let (meta, arrays) = index::load_index(&cache, &key).unwrap();
    let skipped = meta.get("skipped").and_then(Value::as_dict).unwrap();
    assert_eq!(skipped.keys().collect::<Vec<_>>(), ["absolute", "linked"]);
    // reading samples checks again: a signal file replaced by a link afterwards is not read
    let ds = Dataset::new(meta, arrays);
    assert_eq!(ds.samples(0, 0, 4).unwrap().len(), 4);
    std::fs::remove_file(root.join("good.dat")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.dat"), root.join("good.dat")).unwrap();
    assert!(ds.samples(0, 0, 4).unwrap_err().to_string().contains("outside the data roots"));
    paths::set_allowed_roots(None);
}
