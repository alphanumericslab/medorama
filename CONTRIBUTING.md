# Contributing to Medorama

Thank you for helping. This page explains how to propose a change to Medorama or its viewers:
the ground rules, how to set up a clone, and what each kind of change must include. Questions,
bug reports and ideas go in the issue tracker, whose forms ask for what is needed. Security
problems are reported privately instead ([SECURITY.md](SECURITY.md)). Everyone takes part under
the [code of conduct](CODE_OF_CONDUCT.md).

To propose a change, fork the repository, work on a branch, and open a pull request against
`main`. The pull request's checklist follows this page.

## Ground rules

1. **No patient data in git, ever.** Recordings, converted outputs, indexes, screenshots of real
   data and notebook outputs from real data stay in `sample_data/` or
   `viewers/vitalstream/sample_data/` (both ignored by git), or on protected storage. The
   pre-commit hook ([below](#pre-commit-hook)) blocks the common cases, but it does not replace
   care ([PRIVACY.md](PRIVACY.md)).
2. **No licensed material in git.** Formats under a vendor's license are read by add-ons that
   are kept in their own private repositories ([adapters](docs/adapters.md)). Nothing from them
   comes into this repository.
3. **The documents lead.** A change of behavior goes first into the document that specifies it:
   Medorama's rules into [administration](docs/administration.md), its API into the
   [HTTP API](docs/api.md) and `medorama/openapi.yaml`, the adapter protocol into
   [adapters](docs/adapters.md), the WFDB collection into
   [WFDB records and collections](docs/wfdb-collection.md), the EDF reading rules into
   [EDF](docs/edf.md), and VitalStream's index and API into
   [its specifications](viewers/vitalstream/docs).
4. **The three VitalStream viewers agree.** The Python, Rust and C++ viewers write the same
   index byte for byte and answer the same API with the same JSON (`make parity`), so a change
   to one is made to all three. WFDB records are read with
   [wfdb-x](https://github.com/alphanumericslab/wfdb-x) in each language; a change to WFDB
   behavior belongs there.

## Set up a clone

The development installer builds the Python packages, the Rust workspace and the C++ viewer,
and enables the git hooks. Then run the tests:

```bash
tools/install.sh
make test
```

On Windows, run `powershell -ExecutionPolicy Bypass -File tools\install.ps1`. The
[building](docs/building.md) page covers every build situation, and [developing](docs/developing.md)
explains how the code fits together, how to run it on synthetic data, how to find faults and how
to make the common changes.

## What a change must include

### Every change

- One topic per pull request, with tests.
- Updates to the README of every folder you change, and to [`docs/`](docs) when behavior
  changes.
- Screenshots only from the synthetic demo, taken with [`tools/screenshots.py`](tools/screenshots.py).
- A new dependency is recorded in [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md), with its
  license, in the same change (`make audit` checks the Rust ones).

### Code style

| Language | Rules |
|---|---|
| Python | The existing style: the standard library first, NumPy where arrays are involved. No unused imports or undefined names (`make lint-python`: `ruff check --select F`, as CI runs it). |
| Rust | `cargo fmt` and `cargo clippy -- -D warnings` clean (`make lint`). |
| C++ | C++20, the style of the surrounding code (2-space indent, 120 columns), and no warnings with `-Wall -Wextra -Wpedantic` (GCC, Clang) or `/W4` (MSVC): CI builds with warnings as errors. |
| JavaScript (ECG and PACS viewers) | Plain scripts with no dependencies and no build step. The pages must keep working from `file://` without Medorama. |

Program names use the suffixes `-py`, `-rs` and `-cpp` ([naming](viewers/vitalstream/docs/naming.md)).

### The engine and the index

- The C++ engine mirrors the Rust one module for module: `vs_engine`'s `adapters`, `common`,
  `dataset`, `index`, `sources` and `sources_wfdb` correspond to `libs/engine`, and `vs_core`'s
  `pyfmt`, `text` and `value` to `libs/signal-core`. A change to one usually needs the same
  change in the other.
- A new reader for a file format follows [readers and data models](docs/readers.md).
- The *index* is what the viewers and Medorama keep for each recording. A change to its files,
  its `meta.json` or its key goes first into the
  [index format](viewers/vitalstream/docs/index-format.md), raises `INDEX_VERSION` in the three
  engines together (`libs/engine`, the C++ engine and the Python viewer) and in
  `viewers/vitalstream/tests/parity/index_parity.py`, and must pass `make parity`.
- The engine serves both the Rust viewer and Medorama, so run both test suites
  (`make test-rust test-medorama`).

### Medorama

- A change to the gateway's rules goes into
  [administration: the waveform viewer](docs/administration.md#the-waveform-viewer) and
  `medorama/tests/gateway.rs`. The gateway denies by default: a new viewer request passes only
  once it has a rule and a test.
- Database changes are new numbered files in `medorama/src/schema/` (for SQLite and
  PostgreSQL), never edits of files already shipped.

### The ECG and PACS viewers

- `viewers/ecg/dicom.js` is a copy of `viewers/pacs/dicom.js`; `make test-viewers` checks that
  they match.
- Their Python packages (`viewers/ecg/python/muse` and `viewers/pacs/python/dicom`) have tests
  of their own (`make test-python`).

## Pre-commit hook

The command `git config core.hooksPath tools/git-hooks` enables
[`tools/git-hooks/pre-commit`](tools/git-hooks/pre-commit) and `pre-merge-commit`; the
installers run it for you. Both run [`check-files`](tools/git-hooks/check-files) on what is
staged. It refuses data files outside the synthetic test data (recordings, tables, databases,
dumps, archives, DICOM and MUSE XML files by their content), anything under a `sample_data/`
folder, files over 5 MB (the user's manual PDF over 20 MB), notebooks with saved outputs, and real recording names in paths and
contents ([PRIVACY.md](PRIVACY.md#in-this-repository)). CI runs the same check on every commit
of a pull request, so bypassing the hook (`--no-verify`) only moves the refusal there: use it
only when you are certain the commit holds no patient data.

## License

Medorama is licensed under the PolyForm Noncommercial License 1.0.0 ([LICENSE](LICENSE)) and,
by separate agreement, under commercial licenses ([COMMERCIAL.md](COMMERCIAL.md)). Contributions
are accepted under the [Contributor License Agreement](CLA.md), which lets the author offer your
contribution under both while you keep its copyright. Confirm that you accept it in your pull
request (the template has a checkbox).
