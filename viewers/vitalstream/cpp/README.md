# vitalstream-cpp: the VitalStream viewer in C++

This page describes the C++ implementation of [VitalStream](../README.md): how to build it, its
build options and how its code is organized. It is written for developers and for anyone who
builds the viewer by hand. For installing, running, using and deploying the viewer, see
[VitalStream's README](../README.md).

`vitalstream-cpp` is one self-contained program, with wfdb-x's C++ library (for WFDB records)
and the web page built in. It answers the same API with the same JSON as the Rust and Python
viewers, and writes the same index ([docs/naming.md](../docs/naming.md)).

| | |
|---|---|
| Command | `vitalstream-cpp` (default port 8070) |
| Libraries | `vs_core` (values, Python-compatible formatting, SHA-256, the wfdb-x calls), `vs_engine` (the WFDB and EDF readers, the adapter protocol, the index, the queries) and `vs_viewer` (the HTTP server, settings, the password). |
| Build | C++20 (GCC 12+, Clang 15+, Apple Clang 15+, Visual Studio 2022), CMake 3.24+ and Ninja. Every system, including builds without administrator rights or internet access: [docs/building.md](../../../docs/building.md#the-c-viewer). |
| Install alone | `viewers/vitalstream/install.sh` builds it in `cpp/build/standalone` and installs `~/.local/bin/vitalstream-cpp` ([VitalStream's README](../README.md#install)). |
| Run, settings, services | [VitalStream's README](../README.md#run). |

These commands configure and build the viewer, run its tests and start it:

```bash
cd viewers/vitalstream/cpp
cmake --preset release && cmake --build --preset release      # builds build/release/bin/vitalstream-cpp
ctest --preset release
build/release/bin/vitalstream-cpp                            # opens http://127.0.0.1:8070
```

## How it works

| Folder | Contents |
|---|---|
| [`core/`](core) | `vs_core`, namespace `vitalstream::core`: dynamic values and JSON written exactly as the Rust and Python implementations write them, number formatting, text helpers, paths, SHA-256, memory-mapped files, and wfdb-x behind one lock ([`wfdb_c.cpp`](core/src/wfdb_c.cpp)). |
| [`engine/`](engine) | `vs_engine`, namespace `vitalstream::engine`: the built-in readers, adapters, channels, pieces and envelopes, the index and the queries. [`sources.cpp`](engine/src/sources.cpp) chooses the reader. [`sources_wfdb.cpp`](engine/src/sources_wfdb.cpp) reads WFDB records: one record, a collection, and a folder of records read as one session ([`sources_wfdb_files.cpp`](engine/src/sources_wfdb_files.cpp) holds what needs no wfdb-x; [`sources_wfdb_none.cpp`](engine/src/sources_wfdb_none.cpp) takes the reader's place with `VS_WFDB=OFF`). [`sources_edf.cpp`](engine/src/sources_edf.cpp) reads EDF, EDF+, BDF and BDF+ files ([docs/edf.md](../../../docs/edf.md)). [`adapters.cpp`](engine/src/adapters.cpp) finds adapters, runs their commands and keeps their `serve` sessions. |
| [`vitalstream/`](vitalstream) | The viewer: the HTTP server (cpp-httplib), settings (toml++), the password ([`auth.cpp`](vitalstream/src/auth.cpp), or [`auth_none.cpp`](vitalstream/src/auth_none.cpp) without password support), and the command line (CLI11). |
| [`cmake/`](cmake) | The pinned dependencies (wfdb-x among them), the embedding of [`web/`](../web), the paths fixed at build time ([`build_paths.cmake`](cmake/build_paths.cmake)), and the compiler warnings. |
| [`deps/`](deps/README.md) | Archives of the dependencies, for building without internet access. |

The code follows the Rust engine module by module, with the same data structures and rules, so
that the viewers write the same bytes. It is compiled without floating-point contraction
(`-ffp-contract=off`), so that its arithmetic matches the Rust and Python implementations'.

**WFDB records** are read with the C++ library of
[wfdb-x](https://github.com/alphanumericslab/wfdb-x), fetched at a pinned commit and built as a
static library ([`cmake/deps.cmake`](cmake/deps.cmake)). The library keeps process-wide state,
so every call holds one lock. Records are opened by their full path, and nothing is assumed that
a header does not state (sampling frequency, gain or units).

**EDF files** are read by the engine itself, by the rules of
[docs/edf.md, Reading rules](../../../docs/edf.md#reading-rules-for-implementers), with the same
arithmetic as the Rust and Python readers.

## Build options

CMake options are given when the build folder is configured (for example
`cmake --preset release -DVS_ROOT=~`). The first three correspond to `VITALSTREAM_ROOT` and the
`password` and `wfdb` features of the Rust viewer.

| Option | Default | Effect |
|---|---|---|
| `VS_ROOT` | empty | The viewer's **root**: it never opens anything outside it. Empty means the whole file system; `~` is the home folder of whoever runs the viewer, expanded when it runs. `install.sh` sets it to `~`, or to its `--root DIR`. `tools/install-medorama.sh` leaves it empty, because Medorama applies its own rules. Any folder name works (spaces, quotes, Windows' backslashes): it is written into a generated header, not onto the compiler's command line. |
| `VS_PASSWORD` | `ON` | The password the page can ask for (`--set-password`). With `OFF`, the viewer contains no password code at all: `--set-password` and `--clear-password` fail with "this viewer was built without password support", and the viewer refuses to start where a password is required (`require_password`). |
| `VS_WFDB` | `ON` | WFDB records and folders, read with wfdb-x. With `OFF` (the preset `nowfdb`, into `build/nowfdb`; `install.sh --no-wfdb`), wfdb-x is neither downloaded nor built in: [`sources_wfdb_none.cpp`](engine/src/sources_wfdb_none.cpp) takes the place of the WFDB reader, and the viewer lists WFDB records, refuses to open or scan them with "this build has no WFDB support (built with VS_WFDB=OFF)" (HTTP 422), and reads every other format. `vs_core` then has no `core/wfdb_c.hpp` functions ([docs/building.md](../../../docs/building.md#without-wfdb)). |
| `VS_BUILD_TESTS` | `ON` | Builds the unit tests. |
| `VS_BUILD_VIEWER` | `ON` | Builds the viewer. Set it to `OFF` in a project that only links `vs_engine`. |
| `VS_WARNINGS_AS_ERRORS` | `OFF` | Compiler warnings are errors (`-Werror`, `/WX`); continuous integration sets it. |

The settings are read by [`settings.hpp`](vitalstream/src/settings.hpp), with the same keys as
the Python and Rust viewers ([VitalStream's README, Settings](../README.md#settings)). Among
them:
- `data_roots` are the folders the viewer may open, inside its root. The default is the root; a
  wider folder is narrowed to it, and one outside it is left out with a warning. A path is
  allowed when the file or folder it leads to, with every symbolic link resolved, lies inside a
  data root.
- `max_records`, `max_depth` and `max_scan_s` are the limits on a folder of WFDB records
  (defaults 5000, 20 and 10 s; [docs/wfdb-collection.md, Limits](../../../docs/wfdb-collection.md#limits)).
- `require_password` makes the viewer refuse to start without a password. Medorama sets it for
  the viewer it runs.
- `allowed_hosts` and `secure_cookie`: the host names the viewer answers for besides the local
  ones, and the sign-in cookie over HTTPS only
  ([docs/viewer-api.md, Requests](../docs/viewer-api.md#requests)).

The settings file is `--config FILE`, else `<viewer>/vitalstream.toml` when the program runs from
the clone it was built from (the source folder is fixed at build time), else
`~/.config/vitalstream/vitalstream.toml`; a `vitalstream.toml` in the working directory is never
read on its own. The program says on start which file it read.

**The HTTP server** is [cpp-httplib](https://github.com/yhirose/cpp-httplib), with a request line
of at most 64 KiB, at most 100 headers of 8 KiB each, bodies of at most 64 KiB (4 KiB for
`login`), 30 s to read a request and 60 s to write an answer. Every request goes through the
checks of [docs/viewer-api.md, Requests](../docs/viewer-api.md#requests) before it is routed.
One difference from the Rust viewer, which waits asynchronously: after wrong passwords from one
address, its next password is answered `429` (with `Retry-After`) until the delay has passed,
rather than held, so that guessing never occupies the server's threads. Indexes are built by
`index_workers` threads that take the recordings opened from a queue.

**Adapters written in C++** can build on this project: add it to a CMake project with
`VS_BUILD_VIEWER=OFF` and link `vs_engine`
([docs/adapters.md](../../../docs/adapters.md#writing-an-adapter)).

## Tests

`ctest --preset release` runs the unit tests of [`vitalstream/tests`](vitalstream/tests):
- [`test_viewer.cpp`](vitalstream/tests/test_viewer.cpp): envelopes; settings and the viewer's
  root; the password scheme, with scrypt's test vectors; the synthetic recordings of
  [`testdata/`](../testdata), indexed and queried; WFDB records (one record, a multi-segment
  record, a folder of records read as one session, the limits on a folder, the key of a record,
  cancelling), with the checks and expected values of the Rust engine's `tests/wfdb.rs`; MATLAB
  signal files (`Wfdb.MatSignalFiles`: read as the `.dat` file, refused when transposed, a signal
  file answered with its header), with the messages of `tests/wfdb_mat.rs`; and the adapter
  protocol, with the mock adapter of the engine's tests;
- [`test_edf.cpp`](vitalstream/tests/test_edf.cpp): EDF, EDF+ and BDF files, with the checks of
  the Rust engine's `tests/edf.rs`, the header dates and the annotation lists;
- [`test_limits.cpp`](vitalstream/tests/test_limits.cpp): the engine's limits and checks, as the
  Rust engine's `tests/{wfdb,edf,roots,adapters}.rs`: JSON nesting, numbers in files, marker
  names and pieces bounded, headers that state too much, rates that are not finite, a
  collection that skips what it cannot read, files outside the data roots, writers of one key,
  an index that disagrees with its arrays, many signals, a folder's key with links, the settings
  file, and adapters that misbehave (with `libs/engine/tests/misbehaving_adapter.py`; Linux and
  macOS);
- [`test_server.cpp`](vitalstream/tests/test_server.cpp): the HTTP server, as the Rust viewer's
  `tests/viewer.rs`: requests from other sites, bodies and their limits, the query grammar,
  sign-in (the delay after wrong passwords, logout, `Secure`), a recording that cannot be
  indexed, and a record whose signal file leads outside the data roots;
- [`build_paths/`](vitalstream/tests/build_paths): a root with spaces, quotes and backslashes
  reaches the program as given.

With `VS_WFDB=OFF` (`cmake --preset nowfdb && cmake --build --preset nowfdb && ctest --preset
nowfdb`), the tests on WFDB records are skipped, and `Server.WithoutWfdbAWfdbRecordIsRefused`
checks that WFDB records are refused with their message while an EDF file is read.
