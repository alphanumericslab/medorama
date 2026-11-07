# vitalstream-rs: the VitalStream viewer in Rust

This page describes the Rust implementation of [VitalStream](../README.md): how to build it, its
build options and how its code is organized. It is written for developers and for anyone who
builds the viewer by hand. For installing, running, using and deploying the viewer, see
[VitalStream's README](../README.md).

`vitalstream-rs` is one self-contained program, with the engine, the web server and the web page
built in. It answers the same API with the same JSON as the C++ and Python viewers, and writes
the same index ([docs/naming.md](../docs/naming.md)).

| | |
|---|---|
| Command | `vitalstream-rs` (default port 8060) |
| Engine | The crate `recording-engine` ([`libs/engine`](../../../libs/engine)), shared with Medorama: the WFDB and EDF readers, the adapter protocol, the index and the queries. |
| Build | `cargo build --release --locked -p vitalstream-rs` from the repository root (the crate is a member of its Cargo workspace) builds `target/release/vitalstream-rs`. Every system: [docs/building.md](../../../docs/building.md). |
| Install alone | `viewers/vitalstream/install.sh --viewer rs` builds it in `target/standalone` and installs `~/.local/bin/vitalstream-rs` ([VitalStream's README](../README.md#install)). |
| Run, settings, services | [VitalStream's README](../README.md#run). |

These commands build the viewer, start it, and optionally install it into `~/.cargo/bin`:

```bash
cargo build --release --locked -p vitalstream-rs
target/release/vitalstream-rs                              # opens http://127.0.0.1:8060
cargo install --path viewers/vitalstream/rust --locked     # optional
```

## Build options

Three options are fixed when the viewer is compiled:

| Option | Default | Effect |
|---|---|---|
| `VITALSTREAM_ROOT` (an environment variable at build time) | unset | The viewer's **root**: it never opens anything outside it. Unset or empty means the whole file system; `~` is the home folder of whoever runs the viewer. `install.sh` sets it to `~`, or to its `--root DIR`. `tools/install-medorama.sh` leaves it unset, because Medorama applies its own rules. |
| The Cargo feature `password` | on | The password the page can ask for (`--set-password`). Without it (`--no-default-features --features wfdb`, or `install.sh --no-password`), the viewer contains no password code at all ([`auth_none.rs`](src/auth_none.rs)). `--set-password` and `--clear-password` then fail with "this viewer was built without password support", and the viewer refuses to start where a password is required (`require_password`). |
| The Cargo feature `wfdb` | on | WFDB records and folders, read with wfdb-x (the engine's feature `wfdb`). Without it (`--no-default-features --features password`, or `install.sh --no-wfdb`), the viewer contains no wfdb-x code: it lists WFDB records, refuses to open or scan them with "this build has no WFDB support (built without the `wfdb` feature)" (HTTP 422), and reads every other format ([docs/building.md](../../../docs/building.md#without-wfdb)). |

These commands build a standalone viewer whose root is the home folder, one without password
support, and one without WFDB support:

```bash
VITALSTREAM_ROOT='~' cargo build --release --locked -p vitalstream-rs --target-dir target/standalone
cargo build --release --locked -p vitalstream-rs --target-dir target/standalone --no-default-features --features wfdb
cargo build --release --locked -p vitalstream-rs --target-dir target/standalone --no-default-features --features password
```

`install.sh` builds in `target/standalone`, so that `target/release` (Medorama's viewer, built
by `tools/install-medorama.sh`) keeps its own options. The viewer prints its root when it
starts.

The settings are read by [`settings.rs`](src/settings.rs), with the same keys as the Python and
C++ viewers ([VitalStream's README, Settings](../README.md#settings)). Among them:
- `data_roots` are the folders the viewer may open, inside its root. The default is the root; a
  wider folder is narrowed to it, and one outside it is left out with a warning. A path is
  allowed when the file or folder it leads to, with every symbolic link resolved, lies inside a
  data root.
- `max_records`, `max_depth` and `max_scan_s` are the limits on a folder of WFDB records
  (defaults 5000, 20 and 10 s; [docs/wfdb-collection.md, Limits](../../../docs/wfdb-collection.md#limits)).
- `require_password` makes the viewer refuse to start without a password. Medorama sets it for
  the viewer it runs.
- `allowed_hosts` lists the host names the viewer answers for besides the local ones and `host`,
  and `secure_cookie` marks the sign-in cookie `Secure` ([docs/viewer-api.md, Requests](../docs/viewer-api.md#requests)).

A `vitalstream.toml` in the working directory is never read unless `--config` names it; the
viewer prints the settings file it read when it starts.

## How it works

The viewer is a thin HTTP layer over the engine ([docs/architecture.md](../../../docs/architecture.md)).

| Module | Contents |
|---|---|
| [`server.rs`](src/server.rs) | The HTTP API (version 1, [docs/viewer-api.md](../docs/viewer-api.md)): browsing, `scan`, `open`, `cancel` and the queries; sign-in. |
| [`settings.rs`](src/settings.rs), [`auth.rs`](src/auth.rs) | The settings file and the viewer's root; the password ([`auth_none.rs`](src/auth_none.rs) without the `password` feature). |
| [`web.rs`](src/web.rs), [`build.rs`](build.rs) | The page from [`web/`](../web), built into the program. |
| Engine: [`sources/`](../../../libs/engine/src/sources) | The built-in readers, listed in [`sources/mod.rs`](../../../libs/engine/src/sources/mod.rs). `wfdb_folder.rs` reads WFDB records: one record, multi-segment records, a collection, a folder of records read as one session, and the limits on a folder (`wfdb_files.rs` holds what needs no wfdb-x; `wfdb_none.rs` takes the reader's place without the feature `wfdb`). `edf.rs` reads EDF, EDF+, BDF and BDF+ files ([docs/edf.md](../../../docs/edf.md)). Other formats are read through adapters ([`adapters.rs`](../../../libs/engine/src/adapters.rs)). |
| Engine: [`common.rs`](../../../libs/engine/src/common.rs) | Channels, pieces, minimum and maximum envelope pyramids, vital-sign series. |
| Engine: [`index.rs`](../../../libs/engine/src/index.rs), [`dataset.rs`](../../../libs/engine/src/dataset.rs) | The index ([docs/index-format.md](../docs/index-format.md), version 2); the window, vital-sign and marker queries. |

Indexing runs in background threads, `index_workers` at a time. `POST api/v1/cancel` stops one,
and nothing of it is kept. When a view zooms in, raw samples are read from the recording: WFDB
records through wfdb-x (the crate `wfdbx`; refused without the feature `wfdb`), EDF files by
the engine's EDF reader, and other formats through the adapter's `serve` session.

## Tests

The tests cover the viewer and its engine:
- [`tests/viewer.rs`](tests/viewer.rs) indexes and queries the synthetic recordings of
  [`testdata/`](../testdata), and tests the HTTP API (the checks on hosts, origins, bodies and
  query parameters, a record whose signal file leads outside the data roots, a recording that
  cannot be indexed) and sign-in (logout ending a sign-in, the `Secure` cookie);
- the unit tests of [`settings.rs`](src/settings.rs) test the viewer's root and its symbolic
  links, and the settings files (none read from the working directory, wrong types refused);
  those of [`server.rs`](src/server.rs) the delay after wrong passwords and the number grammar;
- the engine's [`tests/wfdb.rs`](../../../libs/engine/tests/wfdb.rs) tests WFDB records (one
  record, a folder read as one session, the limits on a folder, a record's index key,
  cancelling, a header stating too many samples, rates that are not numbers, a collection's
  records that cannot be read, writers of one key at once, an index that disagrees with its
  arrays, many signals) and [`tests/roots.rs`](../../../libs/engine/tests/roots.rs) files
  outside the data roots;
- the engine's [`tests/edf.rs`](../../../libs/engine/tests/edf.rs) tests EDF, EDF+ and BDF files
  (channels and rates, runs and breaks, annotations, start times, a record cut short, 24-bit
  samples, the patient field never read, the limit on pieces, rates that are not finite);
- the engine's [`tests/adapters.rs`](../../../libs/engine/tests/adapters.rs) tests the adapter
  protocol end to end;
- without the feature `wfdb`, the tests on WFDB records are ignored, and
  `without_wfdb_a_wfdb_record_is_refused` (in `tests/viewer.rs`) and the engine's
  [`tests/no_wfdb.rs`](../../../libs/engine/tests/no_wfdb.rs) check that WFDB records are
  refused with their message while EDF files are read.

Run them from the repository root:

```bash
cargo test --release --locked -p vitalstream-rs -p recording-engine
cargo test --release --locked -p vitalstream-rs -p recording-engine --no-default-features --features vitalstream-rs/password --target-dir target/nowfdb   # without WFDB support
```
