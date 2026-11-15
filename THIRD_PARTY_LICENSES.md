# Third-party software and licenses

What Medorama and its viewers use from others, per part, with each license. This is an
inventory for the maintainers' licensing decisions, not legal advice.

The project's own code is licensed under the PolyForm Noncommercial License 1.0.0
([LICENSE](LICENSE)), its documentation (the `docs/` folders) under CC BY-NC-ND 4.0 ([docs/LICENSE.md](docs/LICENSE.md))
and its demo and example data under CC BY-NC 4.0; commercial use needs a commercial license
([COMMERCIAL.md](COMMERCIAL.md)). [REUSE.toml](REUSE.toml) gives the license of every file.
Nothing from other parties is vendored in this repository.

## WFDB: wfdb-x

WFDB records, headers and annotation files are read with
[wfdb-x](https://github.com/alphanumericslab/wfdb-x), a reimplementation of PhysioNet's WFDB
Software Package 10.7.0 with identical behaviour in Rust, C++ and Python. Each implementation
takes it from GitHub at one pinned commit (`4a91916`): the crate `wfdbx` (Cargo,
[`Cargo.toml`](Cargo.toml)), the C++ library `wfdbx` (CMake,
[`cpp/cmake/deps.cmake`](viewers/vitalstream/cpp/cmake/deps.cmake), checked by SHA-256) and the
Python package `wfdb-x` (pip, `vitalstream_py`'s `pyproject.toml`). A build without the `wfdb`
feature (Cargo's `--no-default-features`, CMake's `VS_WFDB=OFF`, `vitalstream_py` without its
`[wfdb]` extra) contains no wfdb-x code ([docs/building.md](docs/building.md#without-wfdb)).

| Part | License | Used by |
|---|---|---|
| wfdb-x library (Rust, C++, Python), version 1.2.1 | GNU LGPL v2.1 or later | `recording-engine`, so Medorama and `vitalstream-rs`; `vitalstream-cpp`; `vitalstream_py` |
| wfdb-x command-line tools (each language's `apps`, the `wfdbx` command) | GNU GPL v3 or later | not used: CMake and Cargo build the library only; pip installs the Python package's `wfdbx` command, which no Medorama program runs |

wfdb-x's library is under the GNU Lesser General Public License, version 2.1 or (at your option)
any later version (wfdb-x's [`LICENSE`](https://github.com/alphanumericslab/wfdb-x/blob/4a91916f5282fba44ca3056802fbc7def298af08/LICENSE)),
as the original WFDB C library is under the GNU Library General Public License, version 2 or
later. Its command-line tools, translations of the original tools, stay under the GNU GPL,
version 3 or later (its `LICENSE-GPL`). The crate's and the Python package's metadata name both
licenses (`LGPL-2.1-or-later AND GPL-3.0-or-later`), because each package holds the library and
the tools; Medorama uses the library only (CMake builds it with `WFDBX_BUILD_TOOLS=OFF`, and
Cargo builds no binary of a dependency). wfdb-x's own copies of the original C sources (its
`third_party/`, not used in any build) keep their LGPL / GPL v2 licenses.

**Distributing programs that contain wfdb-x.** wfdb-x is linked statically into the Rust
programs (`medorama`, `vitalstream-rs`) and into `vitalstream-cpp`, and `vitalstream_py` imports
it. The LGPL permits this in programs under other licenses, Medorama's included, on its terms:
the programs carry wfdb-x's license and say that they use it, and users may change wfdb-x and
rebuild the program with their version, from Medorama's source and wfdb-x's source at the pinned
commit (both public). The release workflow (`.github/workflows/release.yml`) puts this file and
wfdb-x's library license (as `LICENSE-wfdb-x`) into every archive it builds. Medorama's own code
stays under the PolyForm Noncommercial License 1.0.0.

## Rust (the workspace: `medorama`, `db-builder`, `vitalstream-rs`, `libs/`)

Fetched by Cargo at the versions in [`Cargo.lock`](Cargo.lock): from crates.io, every crate
under a permissive license that permits its use in Medorama; and wfdb-x (LGPL v2.1 or later) from GitHub (above). `cargo deny check`
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
| Apache-2.0 AND ISC | 1 (`ring`: the cryptography of TLS; OpenSSL serves SQLCipher alone) |
| (Apache-2.0 OR MIT) AND BSD-3-Clause | 1 (`encoding_rs`) |
| MIT AND BSD-3-Clause | 1 (`matchit`) |
| 0BSD OR MIT OR Apache-2.0 | 1 (`adler2`) |
| Apache-2.0 OR BSL-1.0 | 1 (`ryu`) |
| CC0-1.0 | 1 (`tiny-keccak`, through `ahash`) |
| LGPL-2.1-or-later AND GPL-3.0-or-later | 1 (`wfdbx`, wfdb-x: its library is used, under the LGPL; above) |

Direct dependencies:
- **`medorama`:** `axum`, `tokio`, `hyper-util` (the gateway), `rusqlite` (SQLCipher compiled
  in), `tokio-postgres`, `tokio-postgres-rustls`, `rustls`, `rustls-native-certs` (PostgreSQL over
  verified TLS), `argon2`, `hmac`, `sha2`, `getrandom` (passwords, sessions, the audit log's
  seals, the patient index's keyed hashes), `aes-gcm` (the workspace's keys, wrapped by the
  master key), `csv`, `chrono`, `chrono-tz`, `clap`, `serde`, `serde_json`, `toml`,
  `include_dir`, `mime_guess`, `rpassword`, `bytes`, `futures-util`, `http-body-util`, `anyhow`;
  tests: `tempfile`, `calamine`, `tower`, `tungstenite` (the browser test), `zip`.
- **`db-builder`:** `rusqlite` (SQLCipher compiled in, for `--encrypt`), `regex` (rewrite rules), `calamine`
  (spreadsheets), `csv`, `parquet` (CLIF's tables, without Arrow), `chrono`, `chrono-tz`, `clap`,
  `serde`, `serde_json`, `sha2`, `toml`, `bytes`, `anyhow`, `recording-engine`, `signal-core`;
  its web interface (`db-builder serve`): `axum`, `tokio`, `argon2`, `getrandom` (its password
  and sessions), `include_dir`, `mime_guess`, `rpassword`, all of them crates Medorama already
  uses; tests: `tempfile`, `zip`, `tower`, `http-body-util`.
- **`vitalstream-rs`:** `axum`, `tokio`, `scrypt`, `hmac`, `sha2`, `getrandom`, `toml`,
  `chrono-tz`, `clap`, `serde`, `serde_json`, `include_dir`, `mime_guess`, `rpassword`,
  `indexmap`.
- **`recording-engine`:** `wfdbx` (wfdb-x, above), `indexmap`, `memmap2`, `rayon`, `sha2`.
  **`signal-core`:** `indexmap`.

| Compiled in | License | |
|---|---|---|
| SQLite (`rusqlite`, `libsqlite3-sys`; SQLCipher is built on it) | public domain | |
| SQLCipher 4.20 on SQLite 3.54.1 (`rusqlite` 0.42, `libsqlite3-sys`, `sqlcipher-src`: `medorama`, `db-builder`) | BSD-3-Clause; SQLite's public-domain blessing; the crate also carries libtomcrypt's sources (public domain / WTFPL), not compiled | |
| OpenSSL (built from source by `openssl-src`, for SQLCipher) | Apache-2.0 | |

Each release archive of the Rust programs carries this file, wfdb-x's `LICENSE` and
`THIRD_PARTY_CRATES.txt`, the license texts of every crate compiled in (written by `cargo about`
with [`tools/about.toml`](tools/about.toml)).

## C++ (`viewers/vitalstream/cpp/`)

Downloaded at build time, or taken from `cpp/deps/` ([`cpp/deps/README.md`](viewers/vitalstream/cpp/deps/README.md)),
at fixed versions, each checked against the SHA-256 in
[`cpp/cmake/deps.cmake`](viewers/vitalstream/cpp/cmake/deps.cmake):

| Library | Version | License | Used for | In |
|---|---|---|---|---|
| wfdb-x (C++ library) | 1.2.1, commit `4a91916` | LGPL-2.1-or-later | reading WFDB records, headers and annotation files | `vitalstream-cpp` (static; above) |
| CLI11 | 2.4.2 | BSD-3-Clause | the command line | `vitalstream-cpp` |
| toml++ | 3.4.0 | MIT | the settings file | `vitalstream-cpp` |
| SQLCipher (with SQLite 3.54.1), from the crates.io archive `sqlcipher-src` 420.0.0 (its own files MIT) | 4.20.0 | BSD-3-Clause; SQLite's public-domain blessing | the index store (index format 3: encrypted SQLite), built from source | `vitalstream-cpp` (static) |
| libtomcrypt, in the same archive | 1.18.2 | public domain or WTFPL | SQLCipher's crypto provider (AES, SHA, HMAC, PBKDF2, Fortuna): no OpenSSL | `vitalstream-cpp` (static) |
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
vendored, except what the `sqlcipher3` wheels bundle.

| Package | Version | License | Needed by |
|---|---|---|---|
| fastapi | ≥ 0.100 | MIT | `vitalstream_py` |
| uvicorn | ≥ 0.20 | BSD-3-Clause | `vitalstream_py` |
| numpy | ≥ 1.22 | BSD-3-Clause | `vitalstream_py`, `dicom_index_py` |
| wfdb-x | 1.2.1, commit `4a91916` | LGPL-2.1-or-later (the library; the package's tools, GPL-3.0-or-later, are not used) | `vitalstream_py` (reading WFDB records) |
| sqlcipher3 | ≥ 0.6.5 | Zlib (the binding, from pysqlite) | `vitalstream_py` (the index store, index format 3: encrypted and plain) |
| SQLCipher 4.20.0 community on SQLite 3.54.1 (bundled in the `sqlcipher3` wheels) | 0.6.5 | BSD-3-Clause; SQLite's public-domain blessing | `vitalstream_py`, through `sqlcipher3` |
| OpenSSL 3.6.0 (statically linked in the `sqlcipher3` wheels, for SQLCipher) | 0.6.5 | Apache-2.0 | `vitalstream_py`, through `sqlcipher3` |
| tomli | ≥ 1.1 | MIT | `vitalstream_py` on Python < 3.11 |
| tzdata | any | Apache-2.0 | `vitalstream_py` on Windows (the IANA time zones, which Windows does not have) |
| pydicom | ≥ 2.4 | MIT | `dicom_index_py` (`viewers/pacs/python/dicom`: DICOM headers, structured reports, waveforms) |
| pytest, httpx | any | MIT, BSD-3-Clause | tests only |

`muse_reader_py` (`viewers/ecg/python/muse`) has no dependencies. The ECG viewer's MUSE readers
(`viewers/ecg/muse.js`, `viewers/ecg/python/muse`) were written from the format, independently
of GE's GPL `musexmlex.py`. The PACS and ECG viewers' `dicom.js` is written from the DICOM
standard.

## The web pages

`medorama/web/`, `db-builder/web/`, `viewers/vitalstream/web/`, `viewers/ecg/` and `viewers/pacs/` are written for
this project, with no third-party JavaScript or CSS (the PACS viewer's JPEG decoders included).
Medorama's heart-rate detector for Holter recordings follows a published method (Pan and
Tompkins, 1985), implemented here.

## The demo library (`tools/demo/library/`)

The demo library's CT series and chest radiographs ([tools/demo/README.md](tools/demo/README.md))
are adapted from four collections of The Cancer Imaging Archive, as redistributed by Saga IT
(saga-it.com/dicom/samples) from the NCI Imaging Data Commons, under Creative Commons
Attribution licences. They were re-sliced, cropped, resampled, rotated, elastically deformed,
intensity-remapped and noised (the radiographs projected from the chest volumes); every
identifier and date was dropped. Each series the demo writes carries an `ATTRIBUTION.txt` with
its collection and licence. The adaptations are licensed under CC BY-NC 4.0, like the rest of the
library, and the source images stay under their own licences ([REUSE.toml](REUSE.toml) lists
both for each file).

| Collection | Licence | Data citation |
|---|---|---|
| C4KC-KiTS (Kidney Tumor Segmentation Challenge) | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) | Heller, N., Sathianathen, N., Kalapara, A., Walczak, E., Moore, K., Kaluzniak, H., Rosenberg, J., Blake, P., Rengel, Z., Oestreich, M., Dean, J., Tradewell, M., Shah, A., Tejpaul, R., Edgerton, Z., Peterson, M., Raza, S., Regmi, S., Papanikolopoulos, N., Weight, C. (2019). Data from C4KC-KiTS [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/TCIA.2019.IX49E8NX |
| LIDC-IDRI (Lung Image Database Consortium) | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) | Armato III, S. G., McLennan, G., Bidaut, L., McNitt-Gray, M. F., Meyer, C. R., Reeves, A. P., Zhao, B., Aberle, D. R., Henschke, C. I., Hoffman, E. A., Kazerooni, E. A., MacMahon, H., Van Beek, E. J. R., Yankelevitz, D., et al. (2015). Data From LIDC-IDRI [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/K9/TCIA.2015.LO9QL9SX |
| NLST (National Lung Screening Trial) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) | National Lung Screening Trial Research Team. (2013). Data from the National Lung Screening Trial (NLST) [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/TCIA.HMQ8-J677 |
| Pancreas-CT | [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) | Roth, H., Lu, L., Farag, A., Shin H-C, Liu, J., Turkbey EB, Summers RM. Data From Pancreas-CT (Version 2) [Data set]. The Cancer Imaging Archive. https://doi.org/10.7937/K9/TCIA.2016.tNB1kqBU |

The library's heart-beat templates are derived from ECGs and transformed beyond recognition
(blended across sources and with synthetic beats); they carry no third-party licence.

## The website

The website's page layout (`_layouts/default.html`) is adapted from GitHub's Minimal theme
([pages-themes/minimal](https://github.com/pages-themes/minimal), CC0 1.0), whose stylesheet the
site imports (`theme: jekyll-theme-minimal`); the site is built with Jekyll and its plug-ins on
GitHub Pages, none of which is part of the programs.

## Add-ons

Add-ons for licensed formats (such as the STP adapter for `.Stp` files) are separate programs,
not part of this repository, with their own licenses and inventories; Medorama and the viewers
only run them ([docs/adapters.md](docs/adapters.md)).
