# Third-party software and licenses

What Medorama and its viewers use from others, per part, with each license. This is an
inventory for the maintainers' licensing decisions, not legal advice. **Open points** are
marked ⚠️.

The project's own code is licensed under the PolyForm Noncommercial License 1.0.0
([LICENSE](LICENSE)); commercial use needs a commercial license ([COMMERCIAL.md](COMMERCIAL.md)).
Nothing from other parties is vendored in this repository.

## WFDB: wfdb-x

WFDB records, headers and annotation files are read with
[wfdb-x](https://github.com/alphanumericslab/wfdb-x), a reimplementation of PhysioNet's WFDB
Software Package 10.7.0 with identical behaviour in Rust, C++ and Python. Each implementation
takes it from GitHub at one pinned commit (`a707ba9`): the crate `wfdbx` (Cargo,
[`Cargo.toml`](Cargo.toml)), the C++ library `wfdbx` (CMake,
[`cpp/cmake/deps.cmake`](viewers/vitalstream/cpp/cmake/deps.cmake), checked by SHA-256) and the
Python package `wfdb-x` (pip, `vitalstream_py`'s `pyproject.toml`). A build without the `wfdb`
feature (Cargo's `--no-default-features`, CMake's `VS_WFDB=OFF`, `vitalstream_py` without its
`[wfdb]` extra) contains no wfdb-x code ([docs/building.md](docs/building.md#without-wfdb)).

| Part | License | Used by |
|---|---|---|
| wfdb-x (the library and its command-line tools; Rust, C++, Python) | GNU GPL v3 or later | `recording-engine`, so Medorama and `vitalstream-rs`; `vitalstream-cpp`; `vitalstream_py` (the tools are not used) |

wfdb-x's own copies of the original C sources (its `third_party/`, not used in any build) keep
their LGPL / GPL v2 licenses.

⚠️ **Open point: distributing programs that contain wfdb-x.** wfdb-x is linked statically into
the Rust programs (`medorama`, `vitalstream-rs`) and into `vitalstream-cpp`, and `vitalstream_py`
imports it. The repository itself contains no wfdb-x code: the build fetches it. Building
Medorama from source and running it, as the repository is set up to do, combines the two only on
the machine that builds it, which the GPL permits. **Distributing** a built program (a binary
release, an installer, a container image) is different: the GPL v3 requires the whole program
to be offered under the GPL v3, without further restrictions, and the PolyForm Noncommercial
License restricts commercial use, so the two licenses cannot both be met for such a program as
things stand. wfdb-x is a translation of the WFDB library, which its authors (George B. Moody,
Benjamin Moody and contributors) license under the GNU LGPL v2 or later; wfdb-x's own
contributions are by Medorama's author. A resolution is therefore available to the maintainers:
license wfdb-x under the GNU LGPL (version 2.1 or later, or version 3 or later), which permits
linking it into programs under other licenses, provided its source and its license come with them
and users may replace it. Until that is settled, binary releases should not be published; the
release workflow (`.github/workflows/release.yml`) puts this file and wfdb-x's license (as
`LICENSE-wfdb-x`) into every archive it builds.

## Rust (the workspace: `medorama`, `vitalstream-rs`, `libs/`)

Fetched by Cargo at the versions in [`Cargo.lock`](Cargo.lock): from crates.io, every crate
under a permissive license that permits its use in Medorama; and wfdb-x (GPL v3) from GitHub (above). `cargo deny check`
([`deny.toml`](deny.toml), `make audit`) enforces that every crate's license is one of these:

| License | Crates |
|---|---|
| MIT OR Apache-2.0 (and equivalent spellings) | 189 |
| MIT | 48 |
| Apache-2.0 | 6 (`flagset`, `parquet`, `rpassword`, `rtoolbox`, `sync_wrapper`, `zopfli`) |
| BSD-3-Clause | 5 (`snap`, `subtle`, `zstd`, `zstd-safe`, `zstd-sys` with the Zstandard library) |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 4 (`wasi`, `wasip2`, `wit-bindgen`) |
| Zlib OR Apache-2.0 OR MIT, MIT OR Zlib OR Apache-2.0 | 4 (`objc2-core-foundation`, `objc2-system-configuration`, `tinyvec`, `miniz_oxide`) |
| Unlicense OR MIT | 4 (`byteorder`, `memchr`, `csv`, `csv-core`) |
| Zlib | 2 (`foldhash`, `zlib-rs`) |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 2 (`r-efi`; used under MIT / Apache-2.0) |
| Apache-2.0 OR ISC OR MIT | 2 (`rustls`, `rustls-native-certs`) |
| ISC | 2 (`rustls-webpki`, `untrusted`) |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 2 (`unicode-ident`, `unicode-normalization`) |
| Apache-2.0 OR BSL-1.0 OR MIT | 2 (`wasite`, `whoami`) |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 (`zerocopy`, `zerocopy-derive`) |
| Apache-2.0 AND ISC | 1 (`ring`: the cryptography of TLS, with no OpenSSL) |
| (Apache-2.0 OR MIT) AND BSD-3-Clause | 1 (`encoding_rs`) |
| MIT AND BSD-3-Clause | 1 (`matchit`) |
| 0BSD OR MIT OR Apache-2.0 | 1 (`adler2`) |
| Apache-2.0 OR BSL-1.0 | 1 (`ryu`) |
| CC0-1.0 | 1 (`tiny-keccak`, through `ahash`) |
| GPL-3.0-or-later | 1 (`wfdbx`, wfdb-x: above) |

Direct dependencies:
- **`medorama`:** `axum`, `tokio`, `hyper-util` (the gateway), `rusqlite` (SQLite compiled in),
  `tokio-postgres`, `tokio-postgres-rustls`, `rustls`, `rustls-native-certs` (PostgreSQL over
  verified TLS), `argon2`, `hmac`, `sha2`, `getrandom` (passwords, sessions, the audit log's
  seals), `calamine` (spreadsheets), `csv`, `parquet` (CLIF's tables, without Arrow), `chrono`,
  `chrono-tz`, `clap`, `serde`, `serde_json`, `toml`, `include_dir`, `mime_guess`, `rpassword`,
  `bytes`, `futures-util`, `http-body-util`, `anyhow`; tests: `tempfile`, `tower`, `tungstenite`
  (the browser test), `zip`.
- **`vitalstream-rs`:** `axum`, `tokio`, `scrypt`, `hmac`, `sha2`, `getrandom`, `toml`,
  `chrono-tz`, `clap`, `serde`, `serde_json`, `include_dir`, `mime_guess`, `rpassword`,
  `indexmap`.
- **`recording-engine`:** `wfdbx` (wfdb-x, above), `indexmap`, `memmap2`, `rayon`, `sha2`.
  **`signal-core`:** `indexmap`.

| Compiled in | License | |
|---|---|---|
| SQLite (`rusqlite`, `libsqlite3-sys`) | public domain | |
| With `--features encryption`: SQLCipher | BSD-3-Clause | |
| With `--features encryption`: OpenSSL (built from source by `openssl-src`) | Apache-2.0 | |

Each release archive of the Rust programs carries this file, wfdb-x's `LICENSE` and
`THIRD_PARTY_CRATES.txt`, the license texts of every crate compiled in (written by `cargo about`
with [`tools/about.toml`](tools/about.toml)).

## C++ (`viewers/vitalstream/cpp/`)

Downloaded at build time, or taken from `cpp/deps/` ([`cpp/deps/README.md`](viewers/vitalstream/cpp/deps/README.md)),
at fixed versions, each checked against the SHA-256 in
[`cpp/cmake/deps.cmake`](viewers/vitalstream/cpp/cmake/deps.cmake):

| Library | Version | License | Used for | In |
|---|---|---|---|---|
| wfdb-x (C++ library) | commit `a707ba9` | GPL-3.0-or-later | reading WFDB records, headers and annotation files | `vitalstream-cpp` (static; see ⚠️ above) |
| CLI11 | 2.4.2 | BSD-3-Clause | the command line | `vitalstream-cpp` |
| toml++ | 3.4.0 | MIT | the settings file | `vitalstream-cpp` |
| cpp-httplib | 0.65.0 | MIT | the HTTP server | `vitalstream-cpp` |
| GoogleTest | 1.15.2 | BSD-3-Clause | unit tests | tests only |

Build tools (not shipped): CMake (BSD-3-Clause), Ninja (Apache-2.0); the compiler's own runtime
libraries (libstdc++: GPL with the GCC Runtime Library Exception; libc++: Apache-2.0 WITH
LLVM-exception; the MSVC runtime: Microsoft's redistribution terms). SHA-256, scrypt, HMAC and
the IANA time-zone name list are implemented or generated in this repository (the name list
from Python's `zoneinfo` by `tools/gen_timezones.py`; the IANA tz database is in the public
domain).

## Python

Installed by `pip`: from PyPI, except wfdb-x (from GitHub at the pinned commit); nothing is
vendored.

| Package | Version | License | Needed by |
|---|---|---|---|
| fastapi | ≥ 0.100 | MIT | `vitalstream_py` |
| uvicorn | ≥ 0.20 | BSD-3-Clause | `vitalstream_py` |
| numpy | ≥ 1.22 | BSD-3-Clause | `vitalstream_py`, `dicom_index_py` |
| wfdb-x | commit `a707ba9` | GPL-3.0-or-later | `vitalstream_py` (reading WFDB records) |
| tomli | ≥ 1.1 | MIT | `vitalstream_py` on Python < 3.11 |
| tzdata | any | Apache-2.0 | `vitalstream_py` on Windows (the IANA time zones, which Windows does not have) |
| pydicom | ≥ 2.4 | MIT | `dicom_index_py` (`viewers/pacs/python/dicom`: DICOM headers, structured reports, waveforms) |
| pytest, httpx | any | MIT, BSD-3-Clause | tests only |

`muse_reader_py` (`viewers/ecg/python/muse`) has no dependencies. The ECG viewer's MUSE readers
(`viewers/ecg/muse.js`, `viewers/ecg/python/muse`) were written from the format, independently
of GE's GPL `musexmlex.py`. The PACS and ECG viewers' `dicom.js` is written from the DICOM
standard.

## The web pages

`medorama/web/`, `viewers/vitalstream/web/`, `viewers/ecg/` and `viewers/pacs/` are written for
this project, with no third-party JavaScript or CSS (the PACS viewer's JPEG decoders included).
Medorama's heart-rate detector for Holter recordings follows a published method (Pan and
Tompkins, 1985), implemented here.

## Add-ons

Add-ons for licensed formats (such as the STP adapter for `.Stp` files) are separate programs,
not part of this repository, with their own licenses and inventories; Medorama and the viewers
only run them ([docs/adapters.md](docs/adapters.md)).
