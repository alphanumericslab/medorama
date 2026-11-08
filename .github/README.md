# Continuous integration

This folder holds the GitHub Actions workflows that build and test Medorama on every change, the
release workflow, the website build, and the issue and pull-request templates. It is for
contributors and maintainers.

Continuous integration (CI) never sees patient data. Tests that need real recordings skip
themselves, and the checks across implementations run on the synthetic recordings of
[`viewers/vitalstream/testdata`](../viewers/vitalstream/testdata).

## Workflows

| Workflow | What it runs |
|---|---|
| [`checks.yml`](workflows/checks.yml) | On every push and pull request, whatever it changes (so that it can be a required check): the privacy check of every commit and of every file (`tools/git-hooks/check-files`, the same as the pre-commit hook); the tests of `tools/` (the privacy check, the screenshot tool's demo-only guard, the demo's generator, the release archives, the website's link script); Python's lint (`ruff check --select F`: unused imports, undefined names); and the user's manual's static checks. |
| [`rust.yml`](workflows/rust.yml) | The whole Rust workspace: `cargo fmt`, `clippy` and `rustdoc`; the oldest Rust supported (`rust-version` in `Cargo.toml`, 1.88: `cargo check`); the dependencies' advisories, licenses and sources with `cargo deny` (also weekly); the tests of the libraries and the Rust viewer on Linux and macOS, also without WFDB support (the feature `wfdb` left out, with no `wfdbx` in `cargo tree`; `clippy` too); and, on Windows, the installer `tools/install.ps1` with the same tests and the Python viewer's. |
| [`cpp.yml`](workflows/cpp.yml) | The C++ viewer with GCC and Clang on Linux, Clang and GCC on macOS, and MSVC and MinGW on Windows, with its unit tests and with warnings as errors (`-DVS_WARNINGS_AS_ERRORS=ON`), also without WFDB support (the preset `nowfdb`: no wfdb-x fetched or linked); the parity of the three viewers (the same index and the same answers) on Linux, macOS and Windows; the installers `tools/install.ps1 -Cpp` and `viewers/vitalstream/install.sh` (each viewer with and without `--no-wfdb`, which must refuse a WFDB record and index an EDF file); and GCC 13 installed in a home folder by `tools/install-gcc.sh`. |
| [`python.yml`](workflows/python.yml) | The Python viewer and the database tools on Linux, macOS and Windows, with Python 3.9 and 3.12: their tests and command-line entry points; the Python viewer without its `[wfdb]` extra (no wfdb-x) on Linux and Windows; and the packages as users get them: an sdist of each, a wheel built from it alone (with the licence and the viewer's page), installed in a clean environment, where the programs start and `vitalstream-py` serves its page. |
| [`medorama.yml`](workflows/medorama.yml) | Medorama: the API description; the tests on Linux, macOS and Windows, including the gateway with the Rust viewer; the coverage floor; the same tests with the catalog in PostgreSQL 14 and 17 (a service container); the build with database encryption; the build without WFDB support (no `wfdbx` in `cargo tree`, its tests, with the Rust viewer built the same way); the fuzz targets built on every change, and fuzzing (weekly, every target `cargo fuzz list` names); the installer `tools/install-medorama.sh` or `.ps1` with the demo (the C++ viewer on Linux and Windows, the Rust viewer on macOS), where `init` loads the demo and `demo remove` removes it; and the tests of the ECG and PACS viewers and of the VitalStream page's scripts. |
| [`release.yml`](workflows/release.yml) | On a version tag `v*`: the workspace's tests and `cargo deny`, then Medorama, `vitalstream-rs` and `vitalstream-cpp` (with its unit tests) for Linux (x86-64, ARM, and a fully static x86-64 build), macOS (ARM and Intel) and Windows (x86-64), in reproducible archives, attached to a draft GitHub release with `SHA256SUMS` and a signed build provenance ([developing.md](../docs/developing.md#releases)). |

The website, <https://medorama.org/>, needs no workflow: GitHub Pages builds it with Jekyll from
`main` on every push (each folder's README and the documents, with tabs: `_config.yml`,
`_layouts/`, `_includes/`, `assets/`; and the live ECG and PACS viewers). In the repository
settings, **Pages › Source** is **Deploy from a branch**, `main`, `/ (root)`, and the custom
domain is `medorama.org`.

Every workflow reads the repository only (`permissions: contents: read`); a job that must write
(the release) is given that permission alone. Every job has a time
limit. The actions are pinned by commit, with their version in a comment;
[`dependabot.yml`](dependabot.yml) proposes their updates, and the Rust crates', every week.
Workflows with path filters run when the files they build or test change; `checks.yml` always
runs.

## Templates

The issue forms ([`ISSUE_TEMPLATE/`](ISSUE_TEMPLATE)) and the pull-request checklist
([`pull_request_template.md`](pull_request_template.md)) ask for what a report or a change
needs, and remind everyone never to attach patient data. Security problems are reported
privately instead, as described in [SECURITY.md](../SECURITY.md).
