# Changelog: vitalstream_py

## Unreleased

- WFDB records with MATLAB signal files (`.mat`) are read once checked against their header, and
  refused with the reason otherwise; a signal file (`.dat`, `.mat`) typed as the path to open or
  browse is refused with the header to open in its place (`wfdb_files.check_mat_files` and
  `signal_file_note`, with the Rust engine's checks and messages). Tests: `tests/test_wfdb_mat.py`.
- The `[wfdb]` extra installs wfdb-x 1.2.1 (commit `4a91916`, was `a707ba9`), its library under
  the GNU LGPL v2.1 or later.
- **WFDB support is the extra `[wfdb]`**: wfdb-x is no longer a required dependency. The
  installers and CI install `vitalstream_py[wfdb]` (`[test,wfdb]` for development); a plain
  `pip install` has no WFDB support. Without wfdb-x, `sources/wfdb_none.py` takes the place of
  `wfdb_folder.py`: the viewer lists WFDB records and folders, refuses to open or scan them (HTTP
  422, "this build has no WFDB support (wfdb-x is not installed: pip install
  'vitalstream_py[wfdb]')"), reads every other format, and says so when it starts. What needs no
  wfdb-x moved to `sources/wfdb_files.py`; `sources.unreadable`, `sources.NO_WFDB`,
  `wfdb_folder.SUPPORTED`. The tests on WFDB records are skipped without wfdb-x;
  `tests/test_no_wfdb.py` checks the refusal by importing the package afresh with wfdb-x made
  unimportable, and `test_requests_from_other_sites` opens an EDF file.
- Changed: a `vitalstream.toml` in the working directory is no longer read unless `--config`
  names it (or move it to `~/.config/vitalstream/vitalstream.toml`); the clone's copy is read
  only when the package runs from the clone.
- The rules of the Rust engine and viewer, with the same messages
  (docs/index-format.md, docs/viewer-api.md, docs/adapters.md):
  - Settings: the file read is printed on start; every key's type is checked
    (`data_roots = "/"` is an error); new keys `allowed_hosts` and `secure_cookie`.
  - Requests: the `Host` header must name an allowed host (`400`); a request that changes
    something must not come from another site (`Origin`, `Sec-Fetch-Site`: `403`); a `POST` to
    the API must be JSON (`415`); bodies of at most 64 KiB, and 4 KiB for `login` (`413`); body
    fields of the wrong type, and a body that is not a JSON object, are `400`; a parameter given twice, a time that is not a finite
    decimal number, an unknown or repeated channel (at most 64), more than 512 series, or `t1`
    not after `t0` are `400`, never `422`; an unknown method is `405`; answers carry `nosniff`,
    the pages a Content-Security-Policy, and the sign-in page links `static/login.css`.
  - Sign-in: `logout` ends the sign-in on the server (`revoked` in the auth file); the cookie is
    `Secure` with `secure_cookie`; wrong passwords from one address delay its next one (1, 3,
    7, 15 s, at most 30 s); passwords are checked one at a time, off the event loop; a cookie of
    characters other than ASCII is refused, no longer an error (`500`).
  - Indexing: a WFDB record whose files lead outside the data roots is refused (`403`), and left
    out of a folder or collection; a collection skips the records it cannot read or that lie
    outside it (`index.csv has no record column` otherwise); a header that states more samples
    than its files hold is read to the end of its data, a chunk of frames at a time; a rate that
    is not a finite positive number is refused (WFDB and EDF); at most 100,000 pieces and 32,768
    marker names (`(other markers)`); a folder's key counts the files its links lead to and its
    walk is bounded; unique temporary folders, one writer per key, a complete index kept or
    moved aside before it is replaced; an index whose `meta.json` and arrays disagree is not
    used, nor written; a failed index (or a worker that dies, whose pool is made again) is the
    state `error`, with nothing left in the index folder.
  - Queries: a window with more pieces of a channel than pixel columns answers the first piece
    of each column; times far outside the recording find no data instead of failing.
  - Adapters: a cleared environment (an allow-list), standard error drained and never shown,
    time limits on every command, one session per adapter (the others are not held up), a
    session out of step stopped, reads of at most 2^20 samples per request.
  - JSON nested more than 128 levels is refused; EDF numbers take one sign at most.
- A record whose name holds a dot (`rec.v1`) is known by its own header, `rec.v1.hea` (its key
  and the files checked against the data roots), never `rec.hea`, as in the Rust engine.
- The page: requests that change something carry `X-Medorama: 1` (for Medorama's gateway); no
  inline style (the Content-Security-Policy); the Recording panel is written as text only; more
  channel labels are grouped (ventilator and numbered pressures, gases, EEG electrodes);
  times beyond a browser's date range are drawn at its ends; ticks of an hour or more stay on the
  hour across a daylight-saving change, and a time the clocks skip is read with the offset before
  the change.
- `install.sh` and `install.ps1`: relative `--prefix` and `--root` are made absolute (a root
  starting with `~` stays as it is), so the viewer's link does not depend on the folder the
  script ran from; an option without its value is an error; the installed program is checked
  before the script ends.
- Packaging: the package carries the licence (`LICENSE`, `COMMERCIAL.md`, in its
  `.dist-info/licenses/`); its sdist carries the viewer's page, and a wheel builds from the sdist
  alone; building needs setuptools 77 or later.
- License: the PolyForm Noncommercial License 1.0.0, in place of MIT or Apache-2.0 (as
  Medorama's).

- EDF, EDF+, BDF and BDF+ files are read directly (`vitalstream_py/sources/edf.py`,
  docs/edf.md), with indexes byte-identical to the Rust engine's. EDF+ annotations are events of
  kind `annotation`, shown in an **Annotations** lane; the runs of an EDF+D file are pieces with
  breaks between them; `browse` lists EDF files as entries of type `file` and kind `edf`. The
  index stays at version 2 (a new `kind`, `edf`), and the patient and recording fields of the
  header are never read into it.
- `-V`, `--version`: prints `vitalstream-py <version>`, as `vitalstream-rs` and `vitalstream-cpp` do.
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
- The viewer's root, fixed at build time (`VITALSTREAM_ROOT`, written into `_build.py` by
  `setup.py`): data roots are narrowed to it, and one outside it is left out with a warning. A
  path is checked after its symbolic links are resolved: a link inside a data root that leads
  outside it is refused, and `browse` and `complete` do not list it.
- A build without password support (`VITALSTREAM_PASSWORD=0`); the setting `require_password`.
  `install.sh` installs the standalone viewer with `--force-reinstall`, so that its build
  options are written again.
- The page: **Open record…** and **Open multiple records…** (`Shift`+`O`), a folder's summary
  before it is opened (**Open as one session**), **Cancel** while indexing, at most 64 waveform
  lanes at once, errors in bold in the status bar (red is for alarms only).
- Errors of the command line are one line on stderr and exit code 1, as in the Rust viewer.
- **Windows:** a viewer built without a root (the whole file system, as Medorama's installer
  builds it) accepts data roots on every drive; it used to keep only those on the home
  folder's drive, and refused, for example, recordings on `D:`.
- A folder's scan stops when `max_scan_s` is reached, not only once it is passed: a limit of
  0 s refuses every folder on Windows too, whose clock ticks every 15 ms.
- On Windows, the package depends on `tzdata`, the IANA time zones that Windows does not have
  (an unknown time zone such as `America/New_York` used to stop the viewer).
- A wheel builds from the sdist wherever it is unpacked (`setup.py` looked for the repository
  around it).

## 0.1.0

First release: the VitalStream viewer in Python, viewer API version 1, index format version 1,
WFDB folders and collections, adapters.
