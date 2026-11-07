# recording-engine

`recording-engine` is the Rust library that reads, indexes and queries physiological recordings
for [Medorama](../../README.md) and the Rust VitalStream viewer. This page is for developers who
use the library or add a reader to it. Its place in the system is described in the
[architecture](../../docs/architecture.md).

| Module | Contents |
|---|---|
| [`sources/`](src/sources) | The readers. [`wfdb_folder.rs`](src/sources/wfdb_folder.rs) reads WFDB records (single- and multi-segment), collections, and folders of records as one session within limits (`Limits`, `scan`) ([WFDB records and collections](../../docs/wfdb-collection.md)); without the feature `wfdb`, they are refused ([Features](#features)). [`edf.rs`](src/sources/edf.rs) reads EDF, EDF+ (continuous and discontinuous) and BDF files ([EDF](../../docs/edf.md)). Every other format is read through an installed adapter. [`mod.rs`](src/sources/mod.rs) lists the readers and explains how to add one ([readers](../../docs/readers.md)). |
| [`adapters.rs`](src/adapters.rs) | The adapter protocol ([adapters](../../docs/adapters.md)): finding adapters, their commands, and their `serve` sessions. |
| [`index.rs`](src/index.rs) | The index: its key, building it atomically, and loading it memory-mapped, as specified in the [index format](../../viewers/vitalstream/docs/index-format.md); `write_arrays` for adapters. |
| [`dataset.rs`](src/dataset.rs) | Queries on an indexed recording: windows of raw samples or min/max envelopes, vital signs and markers. |
| [`common.rs`](src/common.rs) | Channels, pieces, min/max envelope pyramids, vital-sign series and the time-plausibility check. |
| [`paths.rs`](src/paths.rs) | Paths as users write them (`~`, relative paths), the file-system root, and Windows' canonical paths. |

The library is used by the Rust viewer [`vitalstream-rs`](../../viewers/vitalstream/rust), which
is an HTTP layer over it; by [Medorama](../../medorama), which indexes recordings through it; and
by adapters written in Rust. The C++ viewer has the same engine in C++ (`vs_engine`), and the
Python viewer has it in Python. The three write byte-identical indexes, as checked by
[`index_parity.py`](../../viewers/vitalstream/tests/parity/index_parity.py).

The header's patient and recording identification fields of an EDF file are never part of the
index. Only `edf::identification` reads them, for Medorama's `identify` action.

## Features

| Cargo feature | Default | Effect |
|---|---|---|
| `wfdb` | on | WFDB records and folders, read with wfdb-x (the only part of the library that uses it). Without it (`default-features = false`, or `cargo build --no-default-features`), the library contains no wfdb-x code: [`wfdb_none.rs`](src/sources/wfdb_none.rs) takes the place of `wfdb_folder.rs`, with the same functions. WFDB records and folders are still recognised (`kind_of` says `wfdb`), and `build`, `scan` and reading their samples fail with `sources::NO_WFDB`, "this build has no WFDB support (built without the `wfdb` feature)". `sources::unreadable` tells a program to refuse such a recording before it opens or adds it. Every other format is read as with the feature. |

What needs no wfdb-x (which paths are records and WFDB folders, segment tables, format labels,
the limits on a folder, a folder's files for the index key) is in
[`wfdb_files.rs`](src/sources/wfdb_files.rs), shared by both. Medorama and `vitalstream-rs`
depend on the library without its default features and enable `wfdb` through their own feature
of that name; an adapter written in Rust chooses the same way
([docs/building.md](../../docs/building.md#without-wfdb)).

## Example

The following code indexes a recording, loads the index and asks for ten seconds of the first
channel at about 1200 points:

```rust
use std::path::Path;
use recording_engine::{dataset::Dataset, index};

let key = index::build_index(Path::new("recording"), Path::new("index"), &|_fraction| {})?;
let (meta, arrays) = index::load_index(Path::new("index"), &key).expect("just built");
let ds = Dataset::new(meta, arrays);
let t0 = ds.pieces[0].t0;                                        // start of the first piece
let json = ds.window_json(&[ds.pieces[0].ch], t0, t0 + 10.0, 1200)?;  // its channel, 10 s, ~1200 points
```

## Dependencies and tests

The library depends on [`signal-core`](../signal-core),
[wfdb-x](https://github.com/alphanumericslab/wfdb-x) (crate `wfdbx`, for WFDB records, with the
feature `wfdb`), `indexmap`, `memmap2`, `rayon` and `sha2`.

| Test | Checks |
|---|---|
| [`tests/adapters.rs`](tests/adapters.rs) | The adapter protocol end to end, with the mock adapter [`tests/fake_adapter.py`](tests/fake_adapter.py), which every viewer's tests also use. |
| [`tests/edf.rs`](tests/edf.rs) | The EDF, EDF+ and BDF reader. |
| [`tests/wfdb.rs`](tests/wfdb.rs) | The WFDB reader. |
| [`tests/roots.rs`](tests/roots.rs) | Files outside the data roots, never opened. |
| [`tests/no_wfdb.rs`](tests/no_wfdb.rs) | Without the feature `wfdb` (`cargo test -p recording-engine --no-default-features`): WFDB records refused with `NO_WFDB`, EDF files read. `wfdb.rs` and `roots.rs` need the feature. |

The viewers' tests also index and query the synthetic recordings of
[`viewers/vitalstream/testdata`](../../viewers/vitalstream/testdata) through the library. Changes
between versions are listed in [CHANGELOG.md](CHANGELOG.md).
