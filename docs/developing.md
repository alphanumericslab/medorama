# Developing Medorama and its viewers

This page is for contributors. It explains how the code fits together, how a request travels
through it, how to run Medorama while you work on it, how to find a fault, and where to make the
most common changes. What the system does is described in the [user's guide](user-guide.md), the
[administrator's guide](administration.md) and the [HTTP API](api.md); how its parts and
contracts fit together is in [architecture.md](architecture.md). The waveform viewer VitalStream
has its own guide: [viewers/vitalstream/README.md](../viewers/vitalstream/README.md). Work that is
planned but not built is listed, with designs, in [todo.md](todo.md).

## Contents

- [The code](#the-code)
- [A request, end to end](#a-request-end-to-end)
- [Running it while working on it](#running-it-while-working-on-it)
- [Finding a fault](#finding-a-fault)
- [Common changes](#common-changes)
- [Add a sign-in method](#add-a-sign-in-method)
- [Add a reader for a new file format](#add-a-reader-for-a-new-file-format)
- [Tests](#tests)
- [Releases](#releases)

## The code

[![Medorama](https://github.com/alphanumericslab/medorama/actions/workflows/medorama.yml/badge.svg)](https://github.com/alphanumericslab/medorama/actions/workflows/medorama.yml)
[![Rust](https://github.com/alphanumericslab/medorama/actions/workflows/rust.yml/badge.svg)](https://github.com/alphanumericslab/medorama/actions/workflows/rust.yml)
[![C++](https://github.com/alphanumericslab/medorama/actions/workflows/cpp.yml/badge.svg)](https://github.com/alphanumericslab/medorama/actions/workflows/cpp.yml)
[![Python](https://github.com/alphanumericslab/medorama/actions/workflows/python.yml/badge.svg)](https://github.com/alphanumericslab/medorama/actions/workflows/python.yml)

The source code is at [github.com/alphanumericslab/medorama](https://github.com/alphanumericslab/medorama).
Each part is a program of its own, in its own folder:

```
├── README.md, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, PRIVACY.md, LICENSE,
│   COMMERCIAL.md, CLA.md, THIRD_PARTY_LICENSES.md, CITATION.cff, Cargo.toml (the Rust workspace), deny.toml, Makefile
├── medorama/             the server, its web page and its tests          → medorama/README.md
├── viewers/              the viewers, each usable on its own             → viewers/README.md
│   ├── vitalstream/      waveforms: the Python, Rust and C++ viewers
│   ├── ecg/              12-lead ECGs on paper; muse-py
│   └── pacs/             DICOM studies; dicom-py
├── libs/                 engine and signal-core (Rust)                   → libs/README.md
├── docs/                 guides, security, architecture, specifications, the user's manual
├── tools/                installers, the demo data, screenshots, git hooks → tools/README.md
├── .github/              CI, releases, issue and pull-request templates  → .github/README.md
└── sample_data/          local data and outputs (never committed)
```

| Where | What | Language |
|---|---|---|
| `medorama/src/` | The server: the catalog, imports, cohorts, jobs and the HTTP API. It is one crate, and `lib.rs` lists its modules. | Rust |
| `medorama/src/server.rs` | The routes, the `guard` and `privacy` middleware, and the file responses. | Rust |
| `medorama/src/signin.rs` | Sign-in: the only code that knows how users sign in ([sign-in.md](sign-in.md)). | Rust |
| `medorama/src/gate.rs`, `deid.rs`, `dicom.rs`, `pseudonyms.rs` | The privacy gate and the de-identifiers it uses ([protecting-patient-data.md](protecting-patient-data.md)). | Rust |
| `medorama/src/formats/` | Import formats: converters written in Rust, and formats described in TOML files (`declared.rs`). | Rust |
| `medorama/src/sources/` | Connected databases: the canonical views, and OMOP's (`omop.rs`). | Rust |
| `medorama/src/schema/` | The database schema: `sqlite/` and `postgres/`, each with `001_schema.sql` (the same tables) and then one numbered file per change, which `db.rs` applies in order. | SQL |
| `medorama/web/` | The page: `index.html`, `style.css` and `js/` (ES modules, no build step). | JavaScript |
| `libs/engine/` | The recording engine: readers of recordings (`src/sources/`: WFDB, EDF and BDF, and adapters) and the index the viewers draw from. | Rust |
| `viewers/vitalstream/` | The waveform viewer, in three implementations that produce byte-identical indexes, and their shared web page. | Python, Rust, C++ |
| `viewers/ecg/`, `viewers/pacs/` | The ECG and PACS viewers: static pages with plain scripts, usable on their own. The ECG viewer's readers are registered in `readers.js`. | JavaScript |
| `viewers/ecg/python/muse/`, `viewers/pacs/python/dicom/` | `muse-py` and `dicom-py`: tools that build databases to import, and serve the viewers on their own. | Python |
| `tools/make-demo-data.py` | The synthetic demo data set: patients, databases and files. | Python |

The server embeds the page and the ECG and PACS viewers when it is built (`include_dir!` in
`server.rs`). **Rebuild after changing them** (`cargo build`), then reload the browser.

## A request, end to end

This is what happens when the page asks for a patient's report.

1. **The page** (`web/js/`) calls `api('patients/12/report?q=…')` (`core.js`). The router
   (`router.js`) maps the address (`#/patient/12`) to a view in `views/` or `patient/`, whose
   `show()` function loads the data and draws it.
2. **The `guard` middleware** (`server.rs`) sets the security headers on every response, and
   requires the `X-Medorama` header on API requests that change something.
3. **The `privacy` middleware** (`server.rs`) finds the request's user once. It calls
   `session_user`, which asks `signin::identify` who the request comes from (by the session
   cookie, or by the proxy's header), and then decides with `in_privacy` whether the session is in
   Privacy Mode. On the way out, it passes every answer of a Privacy Mode session through the
   privacy gate (`gate.rs`), and refuses any answer with data that the gate did not clear.
4. **The handler** receives the user through the `Auth` extractor, as `Auth(User)`: the role, the
   PHI permission and `privacy`. Every handler receives the user this way, whatever the sign-in
   method. The handler runs its work in `blocking(…)`, on a thread of its own with a pooled
   database connection.
5. **The module** (`patient/`, `cohorts/` and so on) reads and writes the database and decides
   what the user may see. Identifiers are left out without `user.phi`, and replaced by
   placeholders and pseudonyms in Privacy Mode (`user.privacy`). The text of notes is included only
   when `user.reads_notes()`. These rules live only on the server: the page shows what it is sent.
6. **The answer** is JSON. An error is `{"detail": "…"}`, with status 400 when the request is
   wrong, 403 when it is refused, 404 when there is no such thing (or it may not be seen), and 500
   when the server failed. A 500 is also printed on the server's standard error.

**Files.** A patient's files (`patients/{id}/file/{item}/{name}`) go through
`patient::attachment_part`, which allows only the item's own files, and then
`server::attachment_response`, which streams them. In Privacy Mode, the file passes the gate
according to its kind (`gate::file_kind`): WFDB headers and MUSE XML are de-identified
(`deid.rs`), DICOM headers are rewritten (`dicom.rs`), EDF and BDF headers are de-identified and
their annotation texts removed (`deid.rs` `edf_header` and `edf_annotations`, `server.rs`
`edf_deidentified`), pictures are marked, and every other kind is refused. Files are named by
tokens (`gate::file_token`).

**Background work.** Imports run in the request. Indexing, `identify` and plug-in actions are
jobs (`jobs.rs`), run by worker threads that `serve` starts (or by `medorama work`). A plug-in is
a program that the catalog starts with the job's description; it can be cancelled and has a time
limit (`actions.rs`).

**Live pages.** Every import or removal advances a counter (the `data_version` table), which the
page polls (`live.js`). Open views redraw, and viewer tabs whose file is gone close.

## Running it while working on it

These commands build Medorama, make the demo data, create a catalog of its own in a temporary
folder, load the demo, and start the server:

```bash
cargo build -p medorama                                  # from the root of the clone: target/debug/medorama
.venv/bin/python tools/make-demo-data.py /tmp/medorama-dev/demo   # synthetic patients and files
export MEDORAMA_HOME=/tmp/medorama-dev/catalog           # a catalog of its own, for every command below
echo 'a long password' | target/debug/medorama init --password-stdin --home $MEDORAMA_HOME --results /tmp/medorama-dev/results
target/debug/medorama demo add /tmp/medorama-dev/demo    # the demo: the source DEMO, the cohort DEMO
target/debug/medorama serve --port 8199                  # http://127.0.0.1:8199, sign in as admin
```

`init` also makes the new catalog the default for later commands that run without `--home` or
`MEDORAMA_HOME`. Run `medorama --home <your catalog> use` to make your own catalog the default
again.

The viewers can be worked on alone. Open `viewers/ecg/index.html` or `viewers/pacs/index.html`
from disk and open files; `#src=URL` opens a file at an address.

## Finding a fault

| Symptom | Where to look |
|---|---|
| The page shows an error message | The request in the browser's network panel, and its `detail`. A 500 is also on the server's standard error. |
| A screen stays empty | The browser's console. Errors that no screen caught are also shown as a message. |
| A database imports, but rows are missing or wrong | The import report (warnings, "held back"), then the Databases tab (each row's status) and the Review tab. |
| A file does not open in its viewer | The row's `file` column, `data_roots` in `medorama.toml`, then the viewer's own message, which names what it cannot read. |
| A job fails | `medorama jobs --state failed`. A plug-in's own log is `log.txt` in its output folder. |
| Privacy Mode shows something it should not | `gate.rs` (the dictionary, `json`, `column_mask`, files), `deid.rs` (dates, `muse_xml`, `wfdb_header`, `edf_header`, `edf_annotations`), `dicom.rs`. `tests/privacy.rs` walks every answer: add the case to its fixture. |
| Sign-in behaves unexpectedly | `signin.rs` (`identify`, `login`, `logout`), the `sign_in` settings in `config.rs`, and `tests/signin.rs`. |
| A cohort finds the wrong patients | `POST cohorts/parse {"text": …}` shows how an expression is read. `cohorts/evaluate.rs` combines the criteria; `cohorts/in_catalog.rs` and `cohorts/in_sources.rs` evaluate each one. |
| A query fails on PostgreSQL only | `MEDORAMA_DEBUG_SQL=1` prints each statement that PostgreSQL refuses, as translated, never with its values. `sql/dialect.rs` translates, and `sql/pg.rs` converts values. |
| A connected database answers wrongly | `medorama connections check NAME`; its views (`sources/omop.rs`, or the connection's own); `MEDORAMA_DEBUG_SQL=1`. |

`sqlite3 medorama.sqlite` opens a SQLite catalog; an encrypted one needs its key
(`MEDORAMA_KEY`). The database holds PHI, so open copies of real catalogs only where PHI may be.

## Common changes

| Change | Where | Then |
|---|---|---|
| An API endpoint | A handler and its `.route(…)` in `server.rs`; the work in the module it belongs to. | Its operation in `medorama/openapi.yaml` (`tests/openapi.rs` fails without it); a row in [api.md](api.md); a test in `tests/`. |
| A database column | The next numbered pair of files, `src/schema/sqlite/0NN_….sql` and `src/schema/postgres/0NN_….sql` (never edit a released one), both listed in `db.rs` (`SQLITE_MIGRATIONS`, `PG_MIGRATIONS`). | The code that reads it; [The database](administration.md#the-database) in the administrator's guide; the tests on both databases. |
| A query | SQLite's dialect, kept to what PostgreSQL also accepts ([below](#tests)). `sql/dialect.rs` translates parameters, `INSERT OR IGNORE`, `LIKE`, `group_concat`, the order of NULLs and subquery names. | The tests on both databases. |
| A connected database's view, or OMOP's | `sources/omop.rs` (the CDM's tables), `sources/mod.rs` (the canonical columns, the filters). | design/sources.md; `tests/connections.rs` (its OMOP fixture). |
| A clinical category | `categories.rs` (key, label, words; `is_recording` for categories whose files are recordings), its color in `web/style.css` (`--c-key`, light and dark; never red or amber), and `recording_title` in `patient/journey.rs` for a recording category. | The categories in the user's guide and the administrator's guide. |
| A cohort criterion | `cohorts/criteria.rs` (`Criterion`), `cohorts/in_catalog.rs` and `in_sources.rs` (`meets_catalog`, `meets_source`), `cohorts/words.rs` (`describe`); its words in `expr.rs`. | The tables in the user's guide, [Cohorts](user-guide.md#cohorts); tests in `expr.rs` and `tests/databases.rs`. |
| A file kind a viewer opens | `patient/files.rs` (`file_kind`, `viewer_for`, `mime_of`) and the viewer's reader ([below](#add-a-reader-for-a-new-file-format)). | [Files and recordings](administration.md#files-and-recordings) in the administrator's guide, and the viewer's README. |
| Something Privacy Mode withholds | `deid.rs` (`FIELDS`), `gate.rs` (`column_placeholder`, the dictionary). | [protecting-patient-data.md](protecting-patient-data.md), [security.md](security.md), the user's guide's [Privacy Mode](user-guide.md#privacy-mode); a canary in `tests/privacy.rs`. |
| A viewer feature | The viewer's folder only; it must keep working from `file://`, without Medorama. | Its README; `node --test`. |

**Colors** are tokens in each stylesheet's `:root` (light) and `[data-theme="dark"]`. Red is
reserved for alarms. Abnormal values are amber, critical values orange and low values blue, and
the warning triangle on a viewer tab whose file may show identifiers is amber. Categories use
none of these colors.

## Add a sign-in method

Sign-in is one module, `medorama/src/signin.rs`. Every handler receives the signed-in user as
`Auth(User)`, and roles, the PHI permission and Privacy Mode are decided elsewhere, the same for
every method. A new method therefore changes only sign-in:
1. Add a variant to `signin::Method`, and parse its settings in `config.rs` (`sign_in_method`,
   with a case in the `sign_in_settings` test).
2. Add its arm to `signin::identify`, which returns who a request comes from:
   `Who::User(user, choice)` (with the session's Privacy Mode choice, `None` if none was made),
   `Who::Nobody`, or `Who::NoAccount` with the name the method found.
3. If the method signs in through the page, add its arm to `signin::login` (and `logout`), and
   tell the page through `GET api/v1/sign-in` (`signin::method`).
4. Test it in `tests/signin.rs`, and describe it in [sign-in.md](sign-in.md) and the settings
   table of the [administrator's guide](administration.md#settings).

[sign-in.md](sign-in.md) explains the two methods that exist, `password` and `proxy`, and how an
institution connects its single sign-on through a proxy. A built-in OpenID Connect method is
planned ([todo.md](todo.md#f1-built-in-openid-connect-sign-in-and-multi-factor-authentication)).

## Add a reader for a new file format

Medorama's readers are organized by format, and each produces one of four data models:
recordings (the index), ECG records, DICOM objects and catalog tables. Where a new reader goes
depends on what it reads. [readers.md](readers.md) explains the four models and gives the steps
for each place:

| What the format holds | Where its reader goes |
|---|---|
| A waveform recording, read by the engine and the three VitalStream viewers | `libs/engine/src/sources/` (the module table in `mod.rs`), with the same reader in the C++ and Python viewers, checked by `make parity`. |
| A waveform recording, read by a separate program in any language | An adapter ([adapters.md](adapters.md)). |
| A 12-lead ECG | `viewers/ecg/readers.js`, a registry whose header comment documents the record, its JSON form and the reader interface; one file per format (`wfdb.js`, `muse.js`, `dicom-ecg.js`, `edf.js`). |
| A table to import | `medorama/src/formats/`: a Rust converter, or a TOML description that needs no code ([Import formats](administration.md#import-formats)). |
| A database read in place | `medorama/src/sources/`: views onto the canonical columns. |

A new kind of file that users can download must also be handled by the privacy gate: until a
de-identifier exists for it, it is refused in Privacy Mode (`gate.rs` `FileKind`).

## Tests

| Command (from the root of the repository) | What it covers |
|---|---|
| `make test` | Everything below except `lint` and the PostgreSQL run, plus VitalStream's Python, Rust and C++ tests. |
| `make test-medorama` (`cargo test` in `medorama/`) | `tests/catalog.rs` (users, HTTP permissions, jobs); `databases.rs` (imports, the patient page, cohorts, exports); `privacy.rs` (Privacy Mode end to end: no identifier in any answer, file or export); `signin.rs` (passwords, and an authenticating proxy: the header counts only from the proxy's addresses, unknown names are refused with a reason, passwords are refused, each visit is audited once); `connections.rs` (a connected OMOP database: cohorts, the registry, the page, Privacy Mode); `browser.rs` (described below); `openapi.rs` (the API's description names every route); `demo.rs` (the demo is loaded, seen by every user, and removed with its cohort and nothing else); `bulk.rs` (the batched import against the row-by-row one); `clif.rs` (synthetic CLIF folders imported: every table, each recognized by its columns alone, susceptibilities on their cultures); `identity.rs`; `sources.rs`; `gateway.rs` (with a real VitalStream viewer, Privacy Mode included); `dicom_samples.rs` (DICOM rewriting on local samples, when present); `robustness.rs` (parsers and tables on malformed input); the unit tests in the modules; and then `tests/web/` (`node --test`: the page's scripts without a browser, covering escaping, dates, addresses, the report's entries, and red only for alarms). |
| `make test-viewers` | The ECG viewer (WFDB, MUSE, DICOM ECG, EDF, the paper, calipers) and the PACS viewer's DICOM reader, with damaged and random files (`viewers/*/tests/`); the VitalStream page's scripts (`viewers/vitalstream/tests/web/`). |
| `make test-python` | VitalStream's Python implementation, `muse-py` and `dicom-py`. |
| `make parity` | The three VitalStream viewers produce the same index and the same answers. |
| `make test-nowfdb` | The builds without WFDB support ([building.md, Without WFDB](building.md#without-wfdb)): the engine, `vitalstream-rs` and Medorama without the Cargo feature `wfdb` (in `target/nowfdb`), the C++ viewer with `VS_WFDB=OFF` (`build/nowfdb`), and the Python tests with wfdb-x made unimportable. The tests on WFDB records are skipped there; `libs/engine/tests/no_wfdb.rs`, `without_wfdb_a_wfdb_record_is_refused` (Rust viewer), `Server.WithoutWfdbAWfdbRecordIsRefused` (C++), `tests/test_no_wfdb.py` (Python) and `without_wfdb_a_wfdb_recording_is_refused_and_edf_is_added` (Medorama's `catalog.rs`) check the refusal. |
| `make test-medorama-postgres` | The same tests with the catalog in PostgreSQL: set `MEDORAMA_TEST_POSTGRES=postgres://user@host/db` to an empty database. Each test gets a schema of its own, and the trigram extension must be created once (`CREATE EXTENSION pg_trgm`). `connections.rs` then also reads an OMOP database in PostgreSQL. |
| `make test-tools` | The tools' tests (`tools/tests/`): the privacy check `tools/git-hooks/check-files` in a scratch repository, case by case; the screenshot tool's demo-only guard and its time limits; the demo's generator and the release archives (the same bytes on every run); the website's link script. |
| `make lint` | `cargo fmt --check`, `clippy -D warnings`, and `cargo doc` without a broken link. |
| `make lint-python` | The Python code: no unused imports or undefined names (`ruff check --select F`, the version CI's `checks.yml` uses: `.venv/bin/pip install ruff==0.17.4`). |
| `make msrv` | The workspace checked with the oldest Rust it supports, `rust-version` in `Cargo.toml` (1.88; `rustup toolchain install 1.88`), as CI's `msrv` job does. Raise it, and the versions in the README, [building.md](building.md) and the manual, when a dependency needs a newer Rust. |
| `make coverage` | The share of Medorama's lines that the tests run (`cargo llvm-cov`, with an HTML report in `target/llvm-cov/html`). It fails under 76 %, as CI's `coverage` job does. |
| `make fuzz` | The parsers fuzzed (`medorama/fuzz/`: tables, WFDB headers, MUSE XML, DICOM, EDF and BDF files with their identifiers and Privacy Mode's copy, what users type), every target that `cargo fuzz list` names, `FUZZ_S` seconds each (60 by default), with the versions of the workspace's `Cargo.lock` (copied into `medorama/fuzz/`). It needs `rustup toolchain install nightly` and `cargo install cargo-fuzz`. CI builds every target on each change and fuzzes each weekly, 5 minutes each. |
| `make audit` | The dependencies: RustSec advisories, licenses and sources (`cargo deny`, `deny.toml`). |

**The browser test** (`tests/browser.rs`) drives the page in headless Chrome. It visits every
tab, a patient's page, Privacy Mode on screen and signing out, and runs an accessibility check on
each in both themes (`tests/a11y.js`: every control named, alternative texts, unique ids, WCAG AA
contrast). It also does what people do, through the page's controls: it makes a cohort, imports a
table through the file dialog, adds, edits, resets the password of and deletes a user, saves
Privacy Mode's settings, works the Review queue, runs the index action from Actions and Jobs,
shows the VitalStream viewer in its tab (when one is built), and deletes the demo's cohort with
its patients. Finally it uses the page by keyboard only: signing in, a cohort search, a patient's
page and its entries, and the whole Tab order, checking that the focus is never lost and always
shown. The test is skipped without Chrome; set `MEDORAMA_TEST_CHROME` to its path, and
`MEDORAMA_TEST_CHROME_REQUIRED=1` to make a missing Chrome a failure.

**SQL that both databases run.** Queries are written once, in SQLite's dialect, and translated
for PostgreSQL. Where the two plan a query very differently, the code chooses its form by
`c.dialect()`, as for the Patient Catalog's filters and a cohort's data
([performance.md](performance.md)). Measure both, with many users (`tools/load-test.py`), before
changing such a query. The translation cannot handle the following, so the code must avoid them:
- A parameter whose type nothing gives (`?1 IS NULL OR …`): write `CAST(?1 AS REAL)` (or
  `INTEGER`, `TEXT`) where it first appears. A parameter that is never used.
- An integer used as a condition (`WHERE disabled`, `CASE WHEN can_edit`): compare it (`!= 0`).
  For `WHERE 0`, write `1 = 0`.
- Other columns of grouped rows without an aggregate (`SELECT code … GROUP BY lower(code)`): use
  `MIN(code)`. A `SELECT DISTINCT` ordered by something it does not select: group instead.
- SQLite's own functions (`json_extract`, `GLOB`, FTS5's `MATCH`, `rowid`): ask the connection's
  `dialect()`, or use a helper (`Connection::text_contains`).
- `INSERT OR REPLACE`, `UPDATE OR IGNORE`: write the upsert (`ON CONFLICT … DO UPDATE`), or the
  condition.
- An `ORDER BY` with ties: add a key (`, id`), so that both databases give the same order.

**Coverage.** CI's `coverage` job runs every test, with Chrome installed and required and a viewer
built for the gateway's test, and fails when fewer than 76 % of the lines run. About 78 % ran when
the floor was set. Code added without tests lowers the figure. When coverage grows, raise the
floor in `.github/workflows/medorama.yml` and the `Makefile` together.

**Keyboard use.** For each thing a keyboard user can do, a test does it in `browser.rs`. A
control drawn as something else (`role="button"` on a `div`) needs `tabindex="0"` and must answer
Enter and Space (`main.js`). A screen that redraws puts the focus back where it was (`router.js`,
`focusLost` and `keepFocus`).

**Test data** are made in the tests or by `tools/make-demo-data.py`, never taken from real
patients. A test that needs a file writes it to a temporary folder.

## Releases

A release is a version tag, `vX.Y.Z`, on `main`. Before tagging, set the version in every place
that states it, in the same commit: the crates (`version` in `libs/*/Cargo.toml`,
`medorama/Cargo.toml`, `viewers/vitalstream/rust/Cargo.toml`, and the workspace's dependencies
in `Cargo.toml`), the Python packages (`pyproject.toml` of `viewers/vitalstream/python/vitalstream`,
`viewers/ecg/python/muse` and `viewers/pacs/python/dicom`), the C++ project
(`project(... VERSION ...)` in `viewers/vitalstream/cpp/CMakeLists.txt`), the manual
(`\mdversion` and `\mddate` in `docs/user-manual/preamble.tex`), and `CITATION.cff` (`version`,
and `date-released`, the tag's date). Move the changelogs' "Unreleased" entries under the
version.

The tag starts `.github/workflows/release.yml` only when the repository variable
`RELEASES_ENABLED` is `true`. Binary releases are off until then, because the programs contain
wfdb-x, under the GPL v3, and how they may be distributed is not settled yet
([THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md)). The workflow:

1. Runs the workspace's tests and `cargo deny` on the tagged commit; nothing is built if they
   fail.
2. Builds the programs for each platform with a pinned Rust (`RELEASE_RUST` in the workflow:
   raise it with each release; CI tests the current stable and 1.88), with the build machine's
   paths remapped (`--remap-path-prefix`), and `vitalstream-cpp` with its unit tests.
3. Packs each platform's programs with `LICENSE`, `COMMERCIAL.md`, `THIRD_PARTY_LICENSES.md`,
   wfdb-x's `LICENSE` from the pinned checkout and, for the Rust programs, the licences of the
   crates compiled in (`THIRD_PARTY_CRATES.txt`, written by `cargo about` with
   `tools/about.toml`). `tools/release-archive.py` makes the archives reproducible: the same
   files give the same bytes.
4. Attaches the archives to a draft release with `SHA256SUMS` and a signed build provenance
   (`actions/attest-build-provenance`). Check a download with `sha256sum -c SHA256SUMS` and
   `gh attestation verify FILE --repo alphanumericslab/medorama`; then publish the draft.
