//! The index store (index format 3): an index packed into it answers as the same index in a
//! folder (format 2) does, across chunk boundaries, encrypted or not; without its key nothing
//! is read; an index is kept, replaced and removed whole.  Synthetic recordings of
//! viewers/vitalstream/testdata.

use recording_engine::dataset::Dataset;
use recording_engine::index;
use recording_engine::store::Store;
use signal_core::value::Value;
use std::path::{Path, PathBuf};

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../viewers/vitalstream/testdata").join(name)
}

/// The same queries on two datasets of one recording give the same answers.
fn same_answers(a: &Dataset, b: &Dataset) {
    // the same description but for the key (another version, another key)
    let info = |d: &Dataset| match d.info() {
        Value::Dict(mut m) => {
            m.shift_remove("key");
            Value::Dict(m).to_json()
        }
        v => v.to_json(),
    };
    assert_eq!(info(a), info(b));
    let t0 = a.info().get("t_range").and_then(Value::as_list).and_then(|l| l.first()?.as_f64()).unwrap_or(0.0);
    let t1 = a.info().get("t_range").and_then(Value::as_list).and_then(|l| l.get(1)?.as_f64()).unwrap_or(t0 + 60.0);
    let channels: Vec<i64> = (0..a.channel_count() as i64).collect();
    // the whole recording, a short window, and a window across a piece's middle
    for (x, y, px) in [(t0, t1, 800), (t0, t0 + 10.0, 1200), (t0 + (t1 - t0) / 3.0, t0 + (t1 - t0) / 2.0, 50)] {
        assert_eq!(a.window_json(&channels, x, y, px).unwrap(), b.window_json(&channels, x, y, px).unwrap());
        assert_eq!(a.markers(&channels, x, y).to_json(), b.markers(&channels, x, y).to_json());
    }
}

#[test]
#[cfg(feature = "wfdb")]
fn a_store_answers_as_a_folder_does() {
    for rec in ["collection", "session"] {
        let dir = tempfile::tempdir().unwrap();
        let path = testdata(rec);
        let key2 = index::build_index(&path, &dir.path().join("cache"), &|_| {}).unwrap();
        let (meta2, arrays2) = index::load_index(&dir.path().join("cache"), &key2).unwrap();
        // a plain store, and an encrypted one, cut into small chunks
        for key in [None, Some([7u8; 32])] {
            let store =
                Store::open_with(&dir.path().join(format!("{key:?}.sqlite").replace(' ', "")), key.as_ref(), 100)
                    .unwrap();
            let key3 = index::build_into_store(&path, &store, &|_| {}, &|| false).unwrap();
            assert_ne!(key3, key2, "another version, another key");
            assert_eq!(key3, index::cache_key_of(&path, 3));
            let (meta3, arrays3) = store.load(&key3).unwrap();
            // the same meta but for the version, the key and the informative fields
            let strip = |m: &signal_core::value::Dict| {
                let mut m = m.clone();
                for k in ["index_version", "key", "engine", "indexed_s"] {
                    m.shift_remove(k);
                }
                Value::Dict(m).to_json()
            };
            assert_eq!(strip(&meta2), strip(&meta3), "{rec}");
            assert_eq!(meta3.get("index_version").and_then(Value::as_int), Some(3));
            let (a, b) = (
                Dataset::new(meta2.clone(), index::load_index(&dir.path().join("cache"), &key2).unwrap().1),
                Dataset::new(meta3, arrays3),
            );
            same_answers(&a, &b);
            // nothing of the build is left next to the store
            assert_eq!(std::fs::read_dir(store.tmp_dir()).unwrap().count(), 0);
        }
        drop(arrays2);
    }
}

#[test]
#[cfg(feature = "wfdb")]
fn without_its_key_a_store_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite");
    let store = Store::open(&file, Some(&[1u8; 32])).unwrap();
    let key = index::build_into_store(&testdata("collection"), &store, &|_| {}, &|| false).unwrap();
    drop(store);
    assert!(Store::open(&file, None).is_err(), "no key");
    assert!(Store::open(&file, Some(&[2u8; 32])).is_err(), "another key");
    // the file holds no readable text of the index
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.windows(key.len()).any(|w| w == key.as_bytes()));
    assert!(!bytes.windows(9).any(|w| w == b"\"pieces\":"));
    assert!(Store::open(&file, Some(&[1u8; 32])).unwrap().load(&key).is_some());
}

#[test]
#[cfg(feature = "wfdb")]
fn an_index_is_kept_replaced_and_removed_whole() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("index.sqlite"), None).unwrap();
    let path = testdata("collection");
    let key = index::build_into_store(&path, &store, &|_| {}, &|| false).unwrap();
    // built again by the same engine: the index there is kept
    let before = store.engine_of(&key);
    assert_eq!(index::build_into_store(&path, &store, &|_| {}, &|| false).unwrap(), key);
    assert_eq!(store.engine_of(&key), before);
    assert_eq!(store.keys(), vec![key.clone()]);
    // a cancelled build leaves nothing
    let other = testdata("plain");
    assert!(index::build_into_store(&other, &store, &|_| {}, &|| true).is_err());
    assert_eq!(store.keys(), vec![key.clone()]);
    assert_eq!(std::fs::read_dir(store.tmp_dir()).unwrap().count(), 0);
    assert!(store.remove(&key).unwrap());
    assert!(store.load(&key).is_none() && store.keys().is_empty());
}

#[test]
fn a_store_of_another_format_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite");
    drop(Store::open(&file, None).unwrap());
    let c = rusqlite::Connection::open(&file).unwrap();
    c.execute("UPDATE store SET value = '4' WHERE name = 'format'", []).unwrap();
    drop(c);
    let e = Store::open(&file, None).err().unwrap().to_string();
    assert!(e.contains("another index store format"), "{e}");
}

#[test]
#[cfg(unix)]
fn what_a_killed_program_left_is_removed_a_day_later() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("index.sqlite");
    let tmp = Store::open(&file, None).unwrap().tmp_dir();
    let (old, fresh) = (tmp.join("k.1.0.tmp"), tmp.join("k.1.1.tmp"));
    for d in [&old, &fresh] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("meta.json"), "{}").unwrap();
    }
    let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
    std::fs::File::open(&old).unwrap().set_modified(two_days_ago).unwrap();
    drop(Store::open(&file, None).unwrap());
    assert!(!old.exists() && fresh.exists());
}
