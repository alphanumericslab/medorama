# Changelog: vitalstream-rs

## Unreleased

- Documentation: the speed of the three viewers, measured on WFDB (with `.dat` and `.mat`
  files), EDF, BDF and `.Stp` recordings, indexing and reading, in place of the estimates ("about
  a minute" for the Python viewer was wrong): the VitalStream README's "Speed" and
  [docs/performance.md](../../../docs/performance.md#the-vitalstream-viewers). `tools/bench-viewers.py`
  repeats the measurement.
- WFDB records with MATLAB signal files (`.mat`) are read once checked against their header, and
  refused with the reason otherwise; a signal file (`.dat`, `.mat`) typed as the path to open or
  browse is refused with the header to open in its place (the engine's `check_mat_files` and
  `signal_file_note`; [docs/wfdb-collection.md](../../../docs/wfdb-collection.md#matlab-signal-files)).
- **A build without WFDB support**: the Cargo feature `wfdb` (default features: `password`,
  `wfdb`), so that `--no-default-features --features password` (or `install.sh --no-wfdb`)
  contains no wfdb-x code. Such a viewer lists WFDB records and folders, refuses to open or scan
  them (HTTP 422, "this build has no WFDB support (built without the `wfdb` feature)"), reads every
  other format, and says so when it starts. A build without password support is now
  `--no-default-features --features wfdb` (`install.sh --no-password` passes it). The tests on
  WFDB records are ignored without the feature; `without_wfdb_a_wfdb_record_is_refused` checks the
  refusal, and `requests_from_other_sites` opens an EDF file, so that it runs in both builds.
- Changed: a `vitalstream.toml` in the working directory is no longer read. Name it with
  `--config FILE`, or move it to `~/.config/vitalstream/vitalstream.toml`; the clone's
  `viewers/vitalstream/vitalstream.toml` is read only by a viewer run from the clone it was built
  from. The settings file read is printed on start.
- Changed: Rust 1.88 or later is needed (was 1.85).
- Security: requests must name an allowed host (`localhost`, `127.0.0.1`, `::1`, `host`, the new
  setting `allowed_hosts`), requests that change something must come from no other site and, to
  the API, be JSON; bodies are limited (4 KiB for `login`, 64 KiB for the API); the page and the
  sign-in page carry a Content-Security-Policy (the sign-in page's style is `static/login.css`).
  Passwords are checked one at a time, with a growing delay after wrong ones from one
  address; `logout` ends the sign-in on the server (`revoked` in the auth file); the setting
  `secure_cookie`. A WFDB record whose files lead outside the data roots is refused (`403`).
- Every answer carries `nosniff`, and the pages their Content-Security-Policy, the refusals made
  before a request is routed (`400`, `403`, `415`) included. A body that is not a JSON object (an
  array, a number), or a field of the wrong type (`{"reindex": "yes"}`), is `400`; a failure
  while answering is `500`, not a lost request.
- Queries are checked: a repeated query parameter, a number that is not a finite decimal, an
  unknown channel, more than 64 channels or 512 series, and `t1` not after `t0` are `400`;
  channels given twice are answered once; a known path with another method is `405`; a failure
  while indexing makes the dataset's state `error` instead of leaving it `indexing`.
- A record whose name holds a dot (`rec.v1`) is known by its own header, `rec.v1.hea` (its key
  and the files checked against the data roots), never `rec.hea`.
- The page: requests that change something carry `X-Medorama: 1` (for Medorama's gateway); no
  inline style (the Content-Security-Policy); the Recording panel is written as text only; more
  channel labels are grouped (ventilator and numbered pressures, gases, EEG electrodes);
  times beyond a browser's date range are drawn at its ends; ticks of an hour or more stay on the
  hour across a daylight-saving change, and a time the clocks skip is read with the offset before
  the change.
- `install.sh` and `install.ps1`: relative `--prefix` and `--root` are made absolute (a root
  starting with `~` stays as it is); an option without its value is an error; a program is
  replaced, not written over (on macOS a program changed in place is killed when it starts);
  the standalone viewer is built in `target/standalone` whatever `CARGO_TARGET_DIR` says; the
  installed program is checked before the script ends.
- License: the PolyForm Noncommercial License 1.0.0, in place of MIT or Apache-2.0 (as
  Medorama's: `LICENSE`, `COMMERCIAL.md`).
- EDF, EDF+, BDF and BDF+ files are read directly, through the engine's new EDF reader
  (`libs/engine/src/sources/edf.rs`, docs/edf.md). EDF+ annotations are events of kind
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
- The viewer's root, fixed at build time (`VITALSTREAM_ROOT`): data roots are narrowed to it,
  and one outside it is left out with a warning. A path is checked after its symbolic links are
  resolved: a link inside a data root that leads outside it is refused, and `browse` and
  `complete` do not list it.
- A build without password support (the Cargo feature `password`, on by default;
  `--no-default-features`); the setting `require_password`. `install.sh` builds the standalone
  viewer in `target/standalone`.
- The page: **Open record…** and **Open multiple records…** (`Shift`+`O`), a folder's summary
  before it is opened (**Open as one session**), **Cancel** while indexing, at most 64 waveform
  lanes at once, errors in bold in the status bar (red is for alarms only).
- **Windows:** a viewer built without a root (the whole file system, as Medorama's installer
  builds it) accepts data roots on every drive; it used to keep only those on the home
  folder's drive, and refused, for example, recordings on `D:`.
- A folder's scan stops when `max_scan_s` is reached, not only once it is passed: a limit of
  0 s refuses every folder on Windows too, whose clock ticks every 15 ms.

## 0.1.0

First release: the VitalStream viewer in Rust, viewer API version 1, index format version 1,
WFDB folders and collections, adapters.
