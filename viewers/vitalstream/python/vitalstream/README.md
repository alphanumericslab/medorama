# vitalstream-py: the VitalStream viewer in Python

This page describes the Python implementation of [VitalStream](../../README.md): how to install
it, its build options and how its code is organized. It is written for developers and for
anyone who installs the viewer by hand. For running, using and deploying the viewer, see
[VitalStream's README](../../README.md).

`vitalstream-py` installs and runs anywhere Python does. It answers the same API with the same
JSON as the Rust and C++ viewers, and writes the same index
([docs/naming.md](../../docs/naming.md)).

| | |
|---|---|
| Command | `vitalstream-py` (default port 8050) |
| Package | `vitalstream_py`, for Python 3.9 or later, with FastAPI, uvicorn, NumPy (and, on Windows, `tzdata` for the time zones) and, with the `[wfdb]` extra, [wfdb-x](https://github.com/alphanumericslab/wfdb-x) (installed from GitHub). |
| Install | `pip install './viewers/vitalstream/python/vitalstream[wfdb]'`, or `viewers/vitalstream/install.sh --viewer py` ([VitalStream's README](../../README.md#install)). |
| Run, settings, services | [VitalStream's README](../../README.md#run). |

Installing the package copies the page ([`web/`](../../web)) into it. An editable install
(`pip install -e …`) serves the page from the clone instead.

## Build options

Two options are fixed when the package is built, as the Rust viewer's are when it is compiled.
They are read from environment variables, which [`setup.py`](setup.py) writes into a generated
module, `vitalstream_py/_build.py`:

| Variable | Effect |
|---|---|
| `VITALSTREAM_ROOT` | The viewer's **root**: it never opens anything outside it, whatever the settings say. `data_roots` inside it are kept, a wider one is narrowed to it, and one outside it is left out with a warning. `~` is the home folder of whoever runs the viewer. Empty or unset means the whole file system. |
| `VITALSTREAM_PASSWORD` | `0` builds the package without password support: no password can be set (`--set-password` and `--clear-password` fail), the page never asks for one, and the viewer refuses to start where one is required (`require_password`, which Medorama sets). Unset, or any other value, keeps password support. |

This command installs a viewer whose root is the home folder, without password support:

```bash
VITALSTREAM_ROOT='~' VITALSTREAM_PASSWORD=0 pip install ./viewers/vitalstream/python/vitalstream
```

Run from the source tree, or from an editable install made without these variables, there is
no `_build.py`, and the defaults apply: the whole file system, with password support. An
editable install made with one of them set writes `_build.py` into the source tree, where git
ignores it. The viewer prints its root when it starts.

**WFDB support** is the `[wfdb]` extra, which installs wfdb-x; the installers always add it,
except with `install.sh --no-wfdb`. Installed without it, in an environment that does not hold
wfdb-x, the package contains no wfdb-x code: [`sources/wfdb_none.py`](vitalstream_py/sources/wfdb_none.py)
takes the place of the WFDB reader, and the viewer lists WFDB records, refuses to open or scan
them with "this build has no WFDB support (wfdb-x is not installed: pip install
'vitalstream_py[wfdb]')" (HTTP 422), and reads every other format
([docs/building.md](../../../../docs/building.md#without-wfdb)):

```bash
pip install ./viewers/vitalstream/python/vitalstream            # without WFDB support
pip install './viewers/vitalstream/python/vitalstream[wfdb]'    # with it
```

## Settings

The settings file has the same keys, types and search order as the Rust and C++ viewers'
([`settings.py`](vitalstream_py/settings.py); [VitalStream's README, Settings](../../README.md#settings)):
`--config FILE`, else `viewers/vitalstream/vitalstream.toml` only when the package runs from a
clone (an editable install), else `~/.config/vitalstream/vitalstream.toml`; never a
`vitalstream.toml` in the working directory. The viewer says on start which file it read, and a
key of the wrong type stops it. Among the keys:

| Key | Default | Effect |
|---|---|---|
| `data_roots` | the viewer's root | The folders the viewer may open, inside its root. A path is allowed when the file or folder it leads to, with every symbolic link resolved, lies inside one. |
| `max_records`, `max_depth`, `max_scan_s` | 5000, 20, 10 | The limits on a folder of WFDB records: header files below it, folder levels, and seconds to count them ([docs/wfdb-collection.md, Limits](../../../../docs/wfdb-collection.md#limits)). |
| `require_password` | false | Refuses to start without a password. |
| `allowed_hosts`, `secure_cookie` | `[]`, false | The host names answered for besides the local ones and `host` ([docs/viewer-api.md, Requests](../../docs/viewer-api.md#requests)); the sign-in cookie marked `Secure`. |

## How it works

| Module | Contents |
|---|---|
| [`__main__.py`](vitalstream_py/__main__.py), [`settings.py`](vitalstream_py/settings.py), [`auth.py`](vitalstream_py/auth.py) | The command line, the settings file, the password. |
| [`server.py`](vitalstream_py/server.py) | The HTTP API (FastAPI; [docs/viewer-api.md](../../docs/viewer-api.md)), with its interactive description at `api/v1/docs`. |
| [`sources/`](vitalstream_py/sources) | The built-in readers, chosen in [`sources/__init__.py`](vitalstream_py/sources/__init__.py). [`wfdb_folder.py`](vitalstream_py/sources/wfdb_folder.py) reads WFDB records: one record, multi-segment records, a collection, a folder of records read as one session, and the limits on a folder ([`wfdb_files.py`](vitalstream_py/sources/wfdb_files.py) holds what needs no wfdb-x; [`wfdb_none.py`](vitalstream_py/sources/wfdb_none.py) takes the reader's place without wfdb-x). [`edf.py`](vitalstream_py/sources/edf.py) reads EDF, EDF+, BDF and BDF+ files ([docs/edf.md](../../../../docs/edf.md)). Other formats are read through [`adapters.py`](vitalstream_py/adapters.py). |
| [`common.py`](vitalstream_py/common.py), [`index.py`](vitalstream_py/index.py), [`dataset.py`](vitalstream_py/dataset.py) | Channels, pieces and envelopes; the index ([docs/index-format.md](../../docs/index-format.md)), memory-mapped; the window, vital-sign and marker queries. |

Indexing runs in worker processes (`index_workers` at a time), which report their progress
through a file beside the index (`<key>.progress.json`). A cancelled index
(`POST api/v1/cancel`) stops at the next block of samples, through a flag file the worker
checks, and leaves nothing in the index folder; so does an index that fails, whose dataset
state becomes `error` with the reason. A worker process that dies (killed, out of memory) makes
the state `error` too, and the next index is built in a new pool of workers. The workers open no
file outside the data roots ([`paths.py`](vitalstream_py/paths.py);
[index-format.md, Files outside the data roots](../../docs/index-format.md#files-outside-the-data-roots)).

The server checks every request as [docs/viewer-api.md, Requests](../../docs/viewer-api.md#requests)
describes, with the statuses and messages of the Rust viewer: the `Host` and `Origin` headers,
JSON bodies of at most 64 KiB (a sign-in form of at most 4 KiB), and query parameters parsed
strictly (a malformed one is `400`, never FastAPI's `422`). Passwords are checked one at a time,
off the event loop, with the delay after wrong ones of the Rust viewer. Adapters run within the
limits of [docs/adapters.md, Limits and environment](../../../../docs/adapters.md#limits-and-environment).

## Tests

These commands install the package with its test dependencies and run the tests:

```bash
pip install -e "./viewers/vitalstream/python/vitalstream[test,wfdb]"
python -m pytest -q viewers/vitalstream/python/vitalstream/tests
```

Without the `[wfdb]` extra, the tests on WFDB records are skipped.
[`test_no_wfdb.py`](tests/test_no_wfdb.py) checks the refusal in either installation: it imports
the package afresh with wfdb-x made unimportable.

- [`test_vitalstream.py`](tests/test_vitalstream.py) indexes and queries the synthetic
  recordings of [`testdata/`](../../testdata). It tests WFDB records with the checks and values
  of the Rust engine's `libs/engine/tests/wfdb.rs` (one record, multi-segment records, a folder
  read as one session, the limits, a record's index key, cancelling); the adapter protocol, with
  the mock adapter of the engine's tests; the viewer's root and the settings keys; the file
  browser (links, typed paths, completion, records and their segments); `scan`, `open` and
  `cancel`; a build without password support; the password; and the page behind a path prefix.
- [`test_wfdb_mat.py`](tests/test_wfdb_mat.py) tests MATLAB signal files with the checks and
  messages of the Rust engine's `libs/engine/tests/wfdb_mat.rs`: read as the `.dat` file,
  refused when they do not match their header, and a signal file opened alone answered with its
  header.
- [`test_edf.py`](tests/test_edf.py) tests EDF, EDF+ and BDF files with the checks of the Rust
  engine's `tests/edf.rs`, the header dates and the annotation lists.
- [`test_limits.py`](tests/test_limits.py) tests the limits and checks shared with the Rust
  engine and viewer (`libs/engine/tests/*.rs`, `viewers/vitalstream/rust/tests/viewer.rs`): the
  pieces and marker names of a recording, headers that state more samples than their files
  hold, rates that are not finite, a collection's records that cannot be read or lie outside
  it, files outside the data roots, a folder's key, writers of one key, the checks on loading
  an index, the requests the server refuses, sign-in (`revoked`, `Secure`, the delay after wrong
  passwords), the settings file, a worker that dies, and adapters held to their limits (with the
  misbehaving adapters of `libs/engine/tests/misbehaving_adapter.py`).
