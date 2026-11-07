# Names

This page lists the names of the commands, packages, build options, settings and files of the
three VitalStream viewers. It is a reference for users who install more than one viewer and for
developers who work across them. It is part of the [VitalStream](../README.md) documentation.

VitalStream has three viewers: the same page, the same API and the same index, implemented in
three languages. Every name a user meets ends in the implementation's suffix: **`-py`**,
**`-rs`** or **`-cpp`** for commands, and **`_py`** for the Python package, whose name cannot
contain `-`. Without a suffix, a name refers to what the three have in common: "the viewer",
`vitalstream.toml`.

## Per implementation

| | Python | Rust | C++ |
|---|---|---|---|
| Folder | [`python/vitalstream`](../python/vitalstream) | [`rust/`](../rust) | [`cpp/`](../cpp) |
| Command | `vitalstream-py` | `vitalstream-rs` | `vitalstream-cpp` |
| Default port | 8050 | 8060 | 8070 |
| Package or library | the Python package `vitalstream_py` | the crate `vitalstream-rs`, with the engine crate `recording-engine` ([`libs/engine`](../../../libs/engine)) | the CMake targets `vs_core`, `vs_engine` and `vs_viewer` |
| WFDB reader | wfdb-x, Python package `wfdbx` (`sources/wfdb_folder.py`; `wfdb_none.py` without it) | wfdb-x, crate `wfdbx` (`libs/engine/src/sources/wfdb_folder.rs`; `wfdb_none.rs` without it) | wfdb-x, C++ library `wfdbx` (`cpp/engine/src/sources_wfdb.cpp`; `sources_wfdb_none.cpp` without it) |
| EDF reader | `vitalstream_py/sources/edf.py` | `libs/engine/src/sources/edf.rs` | `cpp/engine/src/sources_edf.cpp` |
| The viewer's root, fixed when it is built | `VITALSTREAM_ROOT` when the package is installed | `VITALSTREAM_ROOT` when it is compiled | CMake `VS_ROOT` |
| Built without password support | `VITALSTREAM_PASSWORD=0` | `--no-default-features --features wfdb` (the Cargo feature `password` left out) | CMake `VS_PASSWORD=OFF` |
| Built without WFDB support (no wfdb-x) | installed without the `[wfdb]` extra | `--no-default-features --features password` (the Cargo feature `wfdb` left out) | CMake `VS_WFDB=OFF` (the preset `nowfdb`) |
| Its refusal of a WFDB record | `this build has no WFDB support (wfdb-x is not installed: pip install 'vitalstream_py[wfdb]')` | ``this build has no WFDB support (built without the `wfdb` feature)`` | `this build has no WFDB support (built with VS_WFDB=OFF)` |
| Standalone build folder (`install.sh`) | `~/.local/share/vitalstream/venv` | `target/standalone` | `cpp/build/standalone` |

## Shared by the three

| Item | Name |
|---|---|
| The page | [`web/`](../web). The Rust and C++ viewers have it built in; the Python viewer serves it from its package. |
| Settings file | `vitalstream.toml`: the first found of `--config FILE`, `./vitalstream.toml`, `<viewer>/vitalstream.toml` (in a clone) and `~/.config/vitalstream/vitalstream.toml`. Top-level keys apply to every viewer; the `[py]`, `[rs]` and `[cpp]` sections apply to one ([example](../vitalstream.example.toml)). |
| Password | `~/.config/vitalstream/auth.json`: one password (a salted scrypt hash) and one sign-in key for every viewer. |
| Index folder | `~/.cache/vitalstream` (in a clone, `<viewer>/sample_data/outputs/index`), shared by the three viewers and by Medorama. |
| Adapter folder | `~/.local/share/medorama/adapters` (Windows: `%LOCALAPPDATA%\medorama\adapters`), shared with Medorama. |
| Command-line options | `--config`, `--host`, `--port`, `--timezone`, `--root`, `--cache-dir`, `--adapters-dir`, `--qc-verbose`, `--no-browser`, `--set-password`, `--clear-password`, `--no-auth`, `--index PATH`, `-V` / `--version` |
| Settings keys | `timezone`, `data_roots`, `host`, `port`, `index_workers`, `cache_dir`, `auth_file`, `adapters_dir`, `qc_verbose`, `max_records`, `max_depth`, `max_scan_s`, `require_password` |
| Installers | `install.sh` (`--viewer`, `--prefix`, `--no-rustup`, `--root`, `--no-password`, `--no-wfdb`) and `install.ps1` (`-Viewer`, `-Prefix`, `-Root`, `-NoPassword`, `-NoWfdb`) |
| Tests across the three | [`tests/parity/`](../tests/parity) |

## Two different roots

The installers' `--root` (`-Root`) sets the **viewer's root**, which is fixed in the program
when it is built: the viewer never opens anything outside it. The viewers' own `--root` option
(the setting `data_roots`) names a folder the viewer may open, inside that root, each time it
starts. The first cannot be widened after the build; the second can only narrow it.
