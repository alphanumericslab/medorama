# Changelog: recording-engine

## Unreleased

- wfdb-x 1.2.1 (commit `4a91916`, was `a707ba9`, version 1.0.0), its library under the GNU LGPL
  v2.1 or later.
- **WFDB support is a Cargo feature, `wfdb`** (on by default), the only part of the engine that
  uses wfdb-x, now an optional dependency. Without it (`--no-default-features`), the engine
  contains no wfdb-x code: `sources/wfdb_none.rs` takes the place of `wfdb_folder.rs`, with the
  same functions; WFDB records and folders are still recognised (`kind_of` says `wfdb`), and
  `build`, `scan` and reading their samples fail with `sources::NO_WFDB` ("this build has no WFDB
  support (built without the `wfdb` feature)"); `sources::unreadable` tells a program to refuse
  such a recording before it opens or adds it; every other format reads as before. What needs no
  wfdb-x (`Limits`, `is_record`, `is_wfdb_folder`, `segment_table`, `format_label`, `read_csv`, a
  folder's fingerprint) moved to `sources/wfdb_files.rs`, re-exported by both; `wfdb_folder::SUPPORTED`.
  Tests: `tests/no_wfdb.rs`; `tests/wfdb.rs` and `tests/roots.rs` need the feature. The workspace
  depends on the engine without its default features; Medorama and `vitalstream-rs` enable `wfdb`
  through their own feature of that name.
- **Robustness and limits** (no index of a well-formed recording changes): at most 100,000
  pieces per recording and 32,768 marker names (`MAX_PIECES`, `MAX_MARKER_NAMES`); a WFDB header
  that states more samples than its signal files hold is read to the end of its data; rates that
  are not finite positive numbers are refused (WFDB and EDF); a collection's unreadable records,
  and records outside its folder, are left out with a note; an `index.csv` without a `record`
  column is a clear error; files a record reads outside the data roots are never opened
  (`paths::set_allowed_roots`); a folder's key counts the files its links lead to, and its walk
  is bounded; `edf::header_from_bytes`.
- **The index:** temporary folders are unique per writer and removed on every way out; writers
  of one key wait for each other; a complete index already there is kept, any other moved aside
  before the new one moves in; `load_index` checks `meta.json` against the arrays (`index::check`).
- **Queries:** a window finds its pieces by binary search, answers at most `px` blocks per
  channel, and decodes a WFDB record's signals once per request (`Reader::read_many`); a record
  is decoded a chunk of at most 2^20 samples (and at least 4096 frames) at a time.
- **Adapters:** see docs/adapters.md, "Limits and environment" (timeouts, standard error kept
  out of the log, a cleared environment, sessions dropped after a protocol error, bounded reads).
- **WFDB records:** one record opens by its `.hea` file; multi-segment records are read (the
  segments from the master's start, null segments as breaks of known length, the master's
  markers mapped to the segments); a folder of plain records is read with its subfolders as one
  session (hidden entries and symbolic links skipped, segments part of their record); a signal
  recorded twice at once goes into a second lane of its channel (`II (2)`).
- **Limits** on a folder of records (`Limits`: 5000 header files, 20 levels, 10 s to count
  them; `set_limits`), and `scan`: what a folder holds, from the headers alone.
- **Cancelling** an index being built (`build_index_until`; adapters are stopped at their next
  progress line).
- **Index format version 2:** the key of a record (its files), folders' keys without hidden
  entries and symbolic links, the new `format` labels; indexes of version 1 are built again.
- **Dotted record names:** a record whose name holds a dot (`rec.v1`) is known by its own header,
  `rec.v1.hea`, never `rec.hea`: its key, its `.mrk` file and the files checked against the data
  roots are its own. Such a record is not read (wfdb-x, as the WFDB library, does not read it):
  a clear error, never another record's data.
- **JSON** (`signal-core`, read from `meta.json` and adapters) nested more than 128 levels is
  refused; numbers in files take one sign at most.
- Changed: Rust 1.88 or later is needed (was 1.85).
- License: the PolyForm Noncommercial License 1.0.0, in place of MIT or Apache-2.0 (as
  Medorama's: `LICENSE`, `COMMERCIAL.md`).
- A folder's scan stops when `max_scan_s` is reached, not only once it is passed: a limit of
  0 s refuses every folder on Windows too, whose clock ticks every 15 ms.

## 0.1.0

First release: WFDB folders and collections, adapters (protocol version 1), the index (format
version 1), dataset queries.
