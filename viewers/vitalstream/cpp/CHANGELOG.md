# Changelog: vitalstream-cpp

## 0.3.2 (2026-11-17, pre-release)

No changes; the version follows Medorama's.

## 0.3.1 (2026-11-17, pre-release)

No changes; the version follows Medorama's.

## 0.3.0 (2026-11-17, pre-release)

No changes; the version follows Medorama's.

## 0.2.1 (2026-11-17, pre-release)

No changes; the version follows Medorama's.

## 0.2.0 (2026-11-15, pre-release)

- `viewers/vitalstream/install.sh` and `install.ps1` show the license terms and go on only if
  they are accepted (`y` or `yes`; `--accept-license`, `-AcceptLicense` or
  `MEDORAMA_ACCEPT_LICENSE=yes` in scripts). The documentation (`docs/`) is now under
  CC BY-NC-ND 4.0; the software's license is unchanged.
- **The page** (shared `web/`): a channel's unit as the engine states it: `adu` only for an
  uncalibrated signal, nothing when no unit is stated.
- **Review fixes (index content changes within version 3, noted in index-format.md's Versions
  table; the three engines write the same bytes, checked by `tests/parity/index_parity.py`, which
  now also makes these recordings):**
  - a WFDB header whose record line states no number of samples (or 0) is read to the end of its
    data, as libwfdb reads it (it gave no samples); a compressed file's frames are counted by
    decoding them;
  - a signal of several samples per frame is a piece at its own rate (frame rate times its samples
    per frame) with every sample, read in WFDB's high-resolution mode (it was averaged to the frame
    rate); its markers are at the first sample of their frame; samples are read again by range of
    the signal's own samples;
  - records without a start time before the first record with one end where it starts, with a
    note (they started at 0, 1970-01-01);
  - an EDF/BDF date with a day its month does not have (`31.02.24`) states no start (it became
    March 2).
- **Windows:** with more pieces of a channel than pixel columns, each column keeps the piece that
  ends latest and every piece that reaches past it (it kept the first to start, and could drop a
  long piece) (viewer-api.md).
- **Store:** the envelopes' chunks are read on a connection of their own (read-only), so that
  windows are not held up while an index is packed. `core::wfdbc::read_physical` returns every
  sample of each signal. Tests: `test_limits.cpp`, `test_edf.cpp`, `test_store.cpp`.
- **Index format 3, the index store** (viewers/vitalstream/docs/index-format.md, "Version 3: the
  index store"), as the Rust engine's `store.rs`: `engine/store.hpp` and `engine/src/store.cpp`
  (`Store`: open, with a raw SQLCipher key or plain; `pack` in one transaction, keeping an index of
  the same engine and replacing any other; `load`, `meta`, `keys`, `remove`, `engine_of`; the
  `arrays` table of each array's length; chunks of 4096 values by default, the store's own size
  read from its `store` table; envelopes read by range with a cache of 16 chunks per array, a
  chunk of another length than its array implies refused; markers and vital signs loaded whole;
  entries of `index.tmp/` untouched for a day removed on opening) and `build_into_store` (the
  version 2 files built in `index.tmp/<key>.<pid>.<n>.tmp/`, private, removed on every way out,
  then packed). The key of format 3: `cache_key_of(path, 3)`. `Loaded` now holds `Values<T>` (a
  file mapped, or values held; was `Mapped<T>`) and `Env` (a file, or a store's chunks); an
  adapter's recording loaded from a store reads its samples from a private folder
  `index.tmp/<key>.<pid>.<n>.read/` holding only its `meta.json`, removed with the dataset.
  `build_files` writes the files of one index (formats 2 and 3); the index folders of format 2
  stay for the tests and the benchmark.
- The viewer uses index stores only: its own, `cache_dir/index.sqlite`, encrypted with the key
  in the new setting `index_key_file` (a file of 64 hex digits; empty: plain), and, with
  `--keys-on-stdin` (Medorama), the stores and keys given as JSON lines on standard input
  (`{"store", "key", "own"}`, `{"forget"}`; keys kept in memory only, never logged, all forgotten
  when standard input closes), looked in first (`Stores` in `server.hpp`). `--index PATH`
  indexes into the own store and prints its path and the key.
- New dependency: SQLCipher 4.20.0 (SQLite 3.54.1) with libtomcrypt 1.18.2 as its crypto
  provider, from the crates.io package `sqlcipher-src` 420.0.0 (pinned with its SHA-256 in
  `cmake/deps.cmake`, `deps/sqlcipher-src-420.0.0.tar.gz` for offline builds), built from source
  as `vs_sqlite`: no OpenSSL, no system library.
- Tests: `test_store.cpp` (`Store.*`: round trips plain and encrypted in small chunks, wrong or
  missing key, no plaintext in the file, keep/replace/remove, cancellation, chunk boundaries and
  inconsistent rows, the stale `index.tmp/` entries, the pipe, `index_key_file`, and a store the
  Rust viewer wrote compared row by row, both ways); `Adapters.EndToEnd` through a store;
  `Server.ARecordingThatCannotBeIndexedIsAnError` checks the store.

- Documentation: the speed of the three viewers, measured on WFDB (with `.dat` and `.mat`
  files), EDF, BDF and `.Stp` recordings, indexing and reading, in place of the estimates ("about
  a minute" for the Python viewer was wrong): the VitalStream README's "Speed" and
  [docs/performance.md](../../../docs/performance.md#the-vitalstream-viewers). `tools/bench-viewers.py`
  repeats the measurement.
- WFDB records with MATLAB signal files (`.mat`) are read once checked against their header, and
  refused with the reason otherwise; a signal file (`.dat`, `.mat`) typed as the path to open or
  browse is refused with the header to open in its place (`check_mat_files` and
  `signal_file_note`, with the Rust engine's checks and messages). Test: `Wfdb.MatSignalFiles`.
- wfdb-x 1.2.1 (commit `4a91916`, was `a707ba9`; `cmake/deps.cmake`, with its new SHA-256), its
  library under the GNU LGPL v2.1 or later.
- **A build without WFDB support**: the CMake option `VS_WFDB` (`ON` by default) and the preset
  `nowfdb` (`build/nowfdb`; `install.sh --no-wfdb`). With `OFF`, wfdb-x is neither downloaded
  nor built in (no `core/wfdb_c.cpp`), and `engine/src/sources_wfdb_none.cpp` takes the place of
  `sources_wfdb.cpp`: the viewer lists WFDB records and folders, refuses to open or scan them
  (HTTP 422, "this build has no WFDB support (built with VS_WFDB=OFF)"), reads every other format,
  and says so when it starts. What needs no wfdb-x moved to `engine/src/sources_wfdb_files.cpp`;
  `wfdb_supported()`, `NO_WFDB` and `unreadable()` in `engine/sources.hpp`. The tests on WFDB
  records are skipped with `OFF`; `Server.WithoutWfdbAWfdbRecordIsRefused` checks the refusal, and
  `Server.RequestsFromOtherSites` opens an EDF file, so that it runs in both builds.
- Changed: a `vitalstream.toml` in the working directory is no longer read unless `--config`
  names it (or move it to `~/.config/vitalstream/vitalstream.toml`); the clone's copy is read
  only when the program runs from the clone it was built from.
- The rules of the Rust engine and viewer, with the same statuses and messages
  (docs/index-format.md, docs/viewer-api.md, docs/adapters.md):
  - Settings: the file read is printed on start; new keys `allowed_hosts` and `secure_cookie`
    (strict types, as the others).
  - Requests: the `Host` header must name an allowed host (`400`); a request that changes
    something must not come from another site (`Origin`, `Sec-Fetch-Site`: `403`); a `POST` to
    the API must be JSON (`415`); bodies of at most 64 KiB, and 4 KiB for `login` (`413`); body
    fields of the wrong type, and a body that is not a JSON object, are `400`; a parameter given
    twice, a time that is not a finite decimal number (`0x10` and `-inf` included), an unknown or repeated channel (at most 64),
    more than 512 series, or `t1` not after `t0` are `400`; a known path asked with another
    method is `405`; answers carry `nosniff`, the pages a Content-Security-Policy, and the
    sign-in page links `static/login.css`; an internal error is `500` `internal error`, its text
    never shown.
  - Sign-in: `logout` ends the sign-in on the server (`revoked` in the auth file); the cookie is
    `Secure` with `secure_cookie`; after wrong passwords from one address, its next password is
    refused with `429` (and `Retry-After`) until the delay has passed (1, 3, 7, 15 s, at most
    30 s), so that no worker thread waits; passwords are checked one at a time.
  - Indexing: a WFDB record whose files lead outside the data roots is refused (`403`), and left
    out of a folder or collection; a collection skips the records it cannot read or that lie
    outside it (`index.csv has no record column` otherwise); a header that states more samples
    than its files hold is read to the end of its data, a chunk of frames at a time; a rate that
    is not a finite positive number is refused (WFDB and EDF); at most 100,000 pieces and 32,768
    marker names (`(other markers)`); a folder's key counts the files its links lead to and its
    walk is bounded; unique temporary folders, one writer per key, a complete index kept or
    moved aside before it is replaced (also on Windows, where a folder in use cannot be
    replaced); an index whose `meta.json` and arrays disagree is not used, nor written; a failed
    index is the state `error`, with nothing left in the index folder. Indexes are built by
    `index_workers` threads from a queue, no longer by a thread per request.
  - Queries: a window with more pieces of a channel than pixel columns answers the first piece
    of each column; times far outside the recording find no data (no conversion out of range).
  - Adapters: a cleared environment (an allow-list), standard error drained and never shown,
    time limits on every command, one session per adapter (the others are not held up), a
    session out of step stopped, reads of at most 2^20 samples per request.
  - JSON nested more than 128 levels is refused (no stack overflow); numbers in files take one
    sign at most.
- cpp-httplib 0.65.0 (was 0.18.1, with published advisories), with a request line of at most
  64 KiB, 100 headers of at most 8 KiB, a 30 s read and 60 s write time limit, and an exception
  handler that never echoes the exception's text.
- `VS_ROOT` reaches the program as given (spaces, quotes, Windows backslashes): it is written
  into a generated header (`cmake/build_paths.cmake`). `VS_WARNINGS_AS_ERRORS` (CI).
- A record whose name holds a dot (`rec.v1`) is known by its own header, `rec.v1.hea` (its key
  and the files checked against the data roots), never `rec.hea`, as in the Rust engine.
- The page: requests that change something carry `X-Medorama: 1` (for Medorama's gateway); no
  inline style (the Content-Security-Policy); the Recording panel is written as text only; more
  channel labels are grouped (ventilator and numbered pressures, gases, EEG electrodes);
  times beyond a browser's date range are drawn at its ends; ticks of an hour or more stay on the
  hour across a daylight-saving change, and a time the clocks skip is read with the offset before
  the change.
- `install.sh` and `install.ps1`: relative `--prefix` and `--root` are made absolute (a root
  starting with `~` stays as it is); an option without its value is an error; a program is
  replaced, not written over (on macOS a program changed in place is killed when it starts); the
  installed program is checked before the script ends.
- License: the PolyForm Noncommercial License 1.0.0, in place of MIT or Apache-2.0 (as
  Medorama's: `LICENSE`, `COMMERCIAL.md`).

- EDF, EDF+, BDF and BDF+ files are read directly (`engine/src/sources_edf.cpp`, docs/edf.md),
  with indexes byte-identical to the Rust engine's. EDF+ annotations are events of kind
  `annotation`, shown in an **Annotations** lane; the runs of an EDF+D file are pieces with
  breaks between them; `browse` lists EDF files as entries of type `file` and kind `edf`. The
  index stays at version 2 (a new `kind`, `edf`), and the patient and recording fields of the
  header are never read into it.
- WFDB records: one record (its `.hea` file, single- or multi-segment: segments from the master
  header, null segments as breaks, layout segments, the master's `.mrk` markers in the segments
  they fall in); a folder of records read with its subfolders as one session (in path order,
  hidden entries and symbolic links skipped, segments part of their record); overlapping pieces
  of a channel in lanes of their own (`II (2)`); the records left out listed in `skipped`, with
  the reason.
- Index format version 2: the key of a record follows its files; a folder's key skips hidden
  entries and symbolic links.
- The limits on a folder of records (settings `max_records`, `max_depth`, `max_scan_s`);
  `GET api/v1/scan`, `POST api/v1/cancel` (dataset state `cancelled`), `config.limits`, `browse`
  entries of type `record` (the segments of multi-segment records only with `hidden=true`),
  `422` for a folder over a limit or without records.
- The viewer's root, fixed at build time (CMake `VS_ROOT`): data roots are narrowed to it, and
  one outside it is left out with a warning. A path is checked after its symbolic links are
  resolved: a link inside a data root that leads outside it is refused, and `browse` and
  `complete` do not list it.
- A build without password support (CMake `VS_PASSWORD=OFF`); the setting `require_password`.
  `install.sh` builds the standalone viewer in `build/standalone`.
- The page: **Open record…** and **Open multiple records…** (`Shift`+`O`), a folder's summary
  before it is opened (**Open as one session**), **Cancel** while indexing, at most 64 waveform
  lanes at once, errors in bold in the status bar (red is for alarms only).
- **Windows:** a viewer built without a root (the whole file system, as Medorama's installer
  builds it) accepts data roots on every drive; it used to keep only those on the home
  folder's drive, and refused, for example, recordings on `D:`.
- A folder's scan stops when `max_scan_s` is reached, not only once it is passed: a limit of
  0 s refuses every folder on Windows too, whose clock ticks every 15 ms.
- Builds with warnings as errors on MSVC (`WIN32_LEAN_AND_MEAN` was defined twice) and on
  GCC 12 (its false `-Wrestrict` warning, GCC bug 105329, is turned off there), and with
  Clang on libstdc++ (an include the tests lacked).

## 0.1.0

First release: the VitalStream viewer in C++ (`vs_core`, `vs_engine`, `vs_viewer`), viewer API
version 1, index format version 1, WFDB folders and collections, adapters.
