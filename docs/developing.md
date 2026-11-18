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
├── db-builder/           tables, exports and recordings made into a database → db-builder/README.md
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
| `medorama/src/` | The server: the connected databases, the workspace, grants, the patient index, cohorts, jobs and the HTTP API. It is one crate, and `lib.rs` lists its modules. | Rust |
| `medorama/src/server.rs` | The routes, the `guard` and `privacy` middleware, and the file responses. | Rust |
| `medorama/src/signin.rs` | Sign-in: the only code that knows how users sign in ([sign-in.md](sign-in.md)). | Rust |
| `medorama/src/gate.rs`, `deid.rs`, `dicom.rs`, `pseudonyms.rs` | The privacy gate and the de-identifiers it uses ([protecting-patient-data.md](protecting-patient-data.md)). | Rust |
| `medorama/src/databases.rs` | The connected databases: connecting, the status checks, reading a new version again (`refresh`), counting the variables and reading every recording on request (`count_variables`, `index_recordings`), and removing a database with every trace (`remove`, `resume_removals`). | Rust |
| `medorama/src/sources/` | Reading a connected database: the canonical views (`mod.rs`: `views` and `dbb` databases, the pools, the filters and thresholds pushed down), OMOP's (`omop.rs`), and a criterion's answers kept while the data's version stays (`reads.rs`). | Rust |
| `medorama/src/workspace.rs` | The workspace: a data key per database, cohort and derived database, wrapped by the master key (`keys`); the encrypted SQLite files of `db-NNNN/` and `cohorts/`; `keys rotate`. | Rust |
| `medorama/src/grants.rs` | Who may `search`, `view`, `phi` or `manage` each database, and `view`, `edit` or `manage` each cohort. | Rust |
| `medorama/src/persons.rs` | The patient index (the subject index): each database's persons in its `persons.sqlite`, linked to patients by exact (issuer, identifier) pairs, kept as keyed hashes in `patient_keys`; conflicts; their encounters, from which the Patient Catalog answers. | Rust |
| `medorama/src/variables.rs` | What each database holds (`variables.sqlite`: names and units from its `variables` view or one grouped query; counts only when asked, small ones shown as "< 11"), kept per data version, read by the search planner and the criteria. | Rust |
| `medorama/src/recordings.rs`, `derived.rs` | The recordings Medorama has opened of recording databases (`recordings.sqlite`: presence, fingerprints, index metadata; `sync_persons` when a patient is opened, `sync` for all on request), numbered `database × 1,000,000,000 + n`; the series derived from them and the results of actions (`results-<action>.sqlite`). | Rust |
| `medorama/src/schema/` | The control store's schema: `sqlite/001_schema.sql` and `postgres/001_schema.sql` (the same tables), which `db.rs` applies. | SQL |
| `db-builder/` | db-builder: tables, known exports (GE MUSE, CLIF, formats declared in TOML) and folders of recordings made into one open SQLite database ([db-builder/README.md](../db-builder/README.md), [its schema](../db-builder/docs/schema.md)). It does not depend on Medorama. | Rust |
| `medorama/web/` | The page: `index.html`, `style.css` and `js/` (ES modules, no build step). | JavaScript |
| `libs/engine/` | The recording engine: readers of recordings (`src/sources/`: WFDB, EDF and BDF, and adapters) and the index the viewers draw from. | Rust |
| `viewers/vitalstream/` | The waveform viewer, in three implementations that produce byte-identical indexes, and their shared web page. | Python, Rust, C++ |
| `viewers/ecg/`, `viewers/pacs/` | The ECG and PACS viewers: static pages with plain scripts, usable on their own. The ECG viewer's readers are registered in `readers.js`. | JavaScript |
| `viewers/ecg/python/muse/`, `viewers/pacs/python/dicom/` | `muse-py` and `dicom-py`: tools that write tables of ECGs and DICOM studies for db-builder, and serve the viewers on their own. | Python |
| `tools/make-demo-data.py` | The demo: about 40 invented patients in six databases (db-builder's, an encrypted one, OMOP, two read through SQL views), with recordings, ECGs and images of every kind, built from models and the transformed demo library (`tools/demo/library`). | Python |
| `tools/bench-viewers.py` | How fast the three VitalStream viewers index and read recordings ([performance.md](performance.md#the-vitalstream-viewers)). | Python |

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
5. **The module** (`patient/`, `cohorts/` and so on) decides what the user may see. It takes the
   databases the user is granted (`databases::granted(c, user, "view")`, `grants::can`), opens
   what Medorama keeps of each in the workspace (`workspace::open_database_file`, which unwraps
   the database's data key only for a granted request), and reads the connected databases
   themselves through `sources/`, read-only. Identifiers are left out without `user.phi` and the
   database's `phi` grant, and replaced by placeholders and pseudonyms in Privacy Mode
   (`user.privacy`). The text of notes is included only when `user.reads_notes()`. These rules
   live only on the server: the page shows what it is sent.
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

**Background work.** `serve` checks every database at start and every 60 seconds
(`databases::check`): a SQLite file present and each view answering gives `online`, else
`offline`. A changed data version reads the database's persons, their encounters and its catalog
again (`databases::refresh`); no recording is read
([design/connected-databases.md](design/connected-databases.md)). Indexing and plug-in actions are jobs (`jobs.rs`), run by worker threads
that `serve` starts (or by `medorama work`). A plug-in is a program that Medorama starts with the
job's description; it can be cancelled and has a time limit (`actions.rs`). A removal that a
crash interrupted is finished when the server or any command starts (`resume_removals`).

**Live pages.** Connecting, refreshing or removing a database, and linking new patients, advance
a counter (the `data_version` table), which the page polls (`live.js`). Open views redraw, and
viewer tabs whose file is gone close.

## Running it while working on it

These commands build Medorama and db-builder, make the demo and its db-builder databases, set
Medorama up in a temporary folder with a workspace of its own, connect the demo's databases, and
start the server:

```bash
cargo build -p medorama -p db-builder                   # from the root of the clone: target/debug/
D=/tmp/medorama-dev/demo
.venv/bin/python tools/make-demo-data.py $D --db-builder target/debug/db-builder   # the demo, its databases built
echo $D > target/debug/medorama.demo                    # where `init --demo` finds the demo
export MEDORAMA_HOME=/tmp/medorama-dev/home             # a home of its own, for every command below
echo 'a long password' | target/debug/medorama init --password-stdin --home $MEDORAMA_HOME \
  --workspace /tmp/medorama-dev/workspace --demo        # connects every database of $D/demo-databases.json
target/debug/medorama serve --port 8199                 # http://127.0.0.1:8199, sign in as admin
```

This makes a demo installation, which connects demo databases only
([design/demo.md](design/demo.md)): a database of real data goes into a site's installation, set up
the same way without `--demo`, where `medorama databases connect NAME --kind dbb --url sqlite:FILE`
connects it. A demo database for a test is made with `db-builder new --demo`. `init` without
`--demo` also makes the new home the default for later commands that run without `--home` or
`MEDORAMA_HOME` (`init --demo` never does). Run `medorama --home <your home> use` to make your own
the default again. The
workspace's files are encrypted with keys that the master key (`<home>/master.key`) wraps: delete
the home and the workspace together.

The viewers can be worked on alone. Open `viewers/ecg/index.html` or `viewers/pacs/index.html`
from disk and open files; `#src=URL` opens a file at an address.

## Finding a fault

| Symptom | Where to look |
|---|---|
| The page shows an error message | The request in the browser's network panel, and its `detail`. A 500 is also on the server's standard error. |
| A screen stays empty | The browser's console. Errors that no screen caught are also shown as a message. |
| A database connects, but patients or data are missing | Its page in the Databases tab (status, version, variables, identity conflicts); `medorama databases check NAME` and `medorama databases variables NAME`; for a db-builder database, `db-builder check FILE`. |
| A file does not open in its viewer | The recording's `file` in its database (relative to `files_root`, or to a SQLite database's folder), `data_roots` in `medorama.toml`, the Recordings subtab's status, then the viewer's own message, which names what it cannot read. |
| A job fails | `medorama jobs --state failed`. A plug-in's own log is `log.txt` in its output folder. |
| Privacy Mode shows something it should not | `gate.rs` (the dictionary, `json`, files), `deid.rs` (dates, `muse_xml`, `wfdb_header`, `edf_header`, `edf_annotations`), `dicom.rs`. `tests/privacy.rs` walks every answer: add the case to its fixture. |
| Sign-in behaves unexpectedly | `signin.rs` (`identify`, `login`, `logout`), the `sign_in` settings in `config.rs`, and `tests/signin.rs`. |
| A cohort finds the wrong patients | `POST cohorts/parse {"text": …}` shows how an expression is read. `cohorts/evaluate.rs` sets the scope (the databases ticked, granted and online), combines the criteria and keeps each member's provenance; `cohorts/in_sources.rs` evaluates each criterion in one database; `cohorts/resolve.rs` reads the names in criteria text against each database's variables. |
| A query fails on PostgreSQL only | `MEDORAMA_DEBUG_SQL=1` prints each statement that PostgreSQL refuses, as translated, never with its values. `sql/dialect.rs` translates, and `sql/pg.rs` converts values. |
| A connected database answers wrongly | `medorama databases check NAME`; its views (`sources/omop.rs`, `sources/mod.rs`, or the database's own `views` settings); `MEDORAMA_DEBUG_SQL=1`. |

`sqlite3 medorama.sqlite` opens a SQLite control store; an encrypted one needs its key
(`MEDORAMA_KEY`). It holds no patient data, but the workspace does: its files open only with
their data key (SQLCipher), which only Medorama unwraps. A workspace holds identifiers (each
database's `persons.sqlite`), so open copies of real workspaces only where PHI may be.

## Common changes

| Change | Where | Then |
|---|---|---|
| An API endpoint | A handler and its `.route(…)` in `server.rs`; the work in the module it belongs to. | Its operation in `medorama/openapi.yaml` (`tests/openapi.rs` fails without it); a row in [api.md](api.md); a test in `tests/`. |
| A control store column | `src/schema/sqlite/001_schema.sql` and `src/schema/postgres/001_schema.sql`, together. Before the first release the schema is edited in place (there is no migration from earlier stores); after it, each change is a new numbered pair of files, listed in `db.rs` (`SQLITE_MIGRATIONS`, `PG_MIGRATIONS`). | The code that reads it; the administrator's guide; the tests on both databases. |
| A query | SQLite's dialect, kept to what PostgreSQL also accepts ([below](#tests)). `sql/dialect.rs` translates parameters, `INSERT OR IGNORE`, `LIKE`, `group_concat`, the order of NULLs and subquery names. | The tests on both databases. |
| A connected database's view, or OMOP's | `sources/omop.rs` (the CDM's tables), `sources/mod.rs` (the canonical columns, the filters, db-builder's tables). | [databases.md](databases.md), [design/sources.md](design/sources.md); `tests/databases.rs` and the OMOP fixture of `tests/browser.rs`; for db-builder's tables, [db-builder/docs/schema.md](../db-builder/docs/schema.md) and its tests. |
| A clinical category | `categories.rs` (key, label, words; `is_recording` for categories whose files are recordings), its color in `web/style.css` (`--c-key`, light and dark; never red or amber), and `recording_title` in `patient/journey.rs` for a recording category. | The categories in the user's guide and the administrator's guide. |
| A cohort criterion | `cohorts/criteria.rs` (`Criterion`), `cohorts/in_sources.rs` (one database's answer), `cohorts/evaluate.rs` (scope and provenance), `cohorts/words.rs` (`describe`); its words in `expr.rs`. | The tables in the user's guide, [Cohorts](user-guide.md#cohorts); tests in `expr.rs` and `tests/databases.rs`. |
| A file kind a viewer opens | `patient/files.rs` (`file_kind`, `viewer_for`, `mime_of`) and the viewer's reader ([below](#add-a-reader-for-a-new-file-format)). | [Files and recordings](administration.md#files-and-recordings) in the administrator's guide, and the viewer's README. |
| Something Privacy Mode withholds | `deid.rs` (`FIELDS`), `gate.rs` (the dictionary). | [protecting-patient-data.md](protecting-patient-data.md), [security.md](security.md), the user's guide's [Privacy Mode](user-guide.md#privacy-mode); a canary in `tests/privacy.rs`. |
| A viewer feature | The viewer's folder only; it must keep working from `file://`, without Medorama. | Its README; `node --test`. |
| A new file | Its license comes from [REUSE.toml](../REUSE.toml): the software's (PolyForm Noncommercial), CC BY-NC-ND 4.0 in a `docs/` folder, CC BY-NC 4.0 for the demo library and the viewers' examples. Material from others: its own entry in `REUSE.toml`, its text in `LICENSES/`, a line in `THIRD_PARTY_LICENSES.md`. | `reuse lint` (CI's `checks`, "licenses"). |
| The license terms the installers show | `tools/license-notice.txt` (shown by every installer and by `medorama init`); the prompt is `tools/license-accept.sh` and `.ps1`. A change to the notice asks every user again. | [building.md, License terms](building.md#license-terms), the manual's installation chapter; `tools/tests/test_license_accept.py`. |

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
recordings (the index), ECG records, DICOM objects and database tables (db-builder's). Where a new reader goes
depends on what it reads. [readers.md](readers.md) explains the four models and gives the steps
for each place:

| What the format holds | Where its reader goes |
|---|---|
| A waveform recording, read by the engine and the three VitalStream viewers | `libs/engine/src/sources/` (the module table in `mod.rs`), with the same reader in the C++ and Python viewers, checked by `make parity`. |
| A waveform recording, read by a separate program in any language | An adapter ([adapters.md](adapters.md)). |
| A 12-lead ECG | `viewers/ecg/readers.js`, a registry whose header comment documents the record, its JSON form and the reader interface; one file per format (`wfdb.js`, `muse.js`, `dicom-ecg.js`, `edf.js`). |
| A table or an export to make into a database | `db-builder/src/formats/`: a Rust converter, or a TOML description that needs no code ([db-builder/README.md](../db-builder/README.md)). |
| A database read in place | `medorama/src/sources/`: views onto the canonical columns. |

A new kind of file that users can download must also be handled by the privacy gate: until a
de-identifier exists for it, it is refused in Privacy Mode (`gate.rs` `FileKind`).

## Tests

| Command (from the root of the repository) | What it covers |
|---|---|
| `make test` | Everything below except `lint` and the PostgreSQL run, plus VitalStream's Python, Rust and C++ tests. |
| `make test-medorama` (`cargo test` in `medorama/`) | `tests/databases.rs` (connected databases: equal identifiers link one patient exactly; identifiers naming two patients are a conflict, never merged; the status follows the database, offline and online; a new version is read again; grants decide what a user sees; a search names the databases it left out; removing a database leaves no trace in the workspace, the cohorts, the patients, the search or the API; a removal a crash interrupted finishes; a deleted cohort's key is shredded; a database inside the workspace or outside `data_roots`, or connected by a non-admin, is refused); `privacy.rs` (Privacy Mode end to end on a connected database seeded with identifiers: no identifier in any answer, file or export, and every GET route of the API walked in Privacy Mode); `signin.rs` (passwords, and an authenticating proxy: the header counts only from the proxy's addresses, unknown names are refused with a reason, passwords are refused, each visit is audited once); `browser.rs` (described below); `gateway.rs` (with a real VitalStream viewer, Privacy Mode included; a non-admin opens only the recordings of the databases granted to them); `audit_queue.rs` (audit entries the database refuses are kept and written later); `openapi.rs` (the API's description names every route); `dicom_samples.rs` (DICOM rewriting on local samples, when present); the unit tests in the modules; and then `tests/web/` (`node --test`: the page's scripts without a browser, covering escaping, dates, addresses, the report's entries, and red only for alarms). |
| `cargo test -p db-builder` | db-builder's tests (`db-builder/tests/`): the tables, exports and declared formats it reads, the folders of recordings, `check`, `remove-source`, and its schema. |
| `make test-viewers` | The ECG viewer (WFDB, MUSE, DICOM ECG, EDF, the paper, calipers) and the PACS viewer's DICOM reader, with damaged and random files (`viewers/*/tests/`); the VitalStream page's scripts (`viewers/vitalstream/tests/web/`). |
| `make test-python` | VitalStream's Python implementation, `muse-py` and `dicom-py`. |
| `make parity` | The three VitalStream viewers produce the same index and the same answers. |
| `make test-nowfdb` | The builds without WFDB support ([building.md, Without WFDB](building.md#without-wfdb)): the engine, `vitalstream-rs` and Medorama without the Cargo feature `wfdb` (in `target/nowfdb`), the C++ viewer with `VS_WFDB=OFF` (`build/nowfdb`), and the Python tests with wfdb-x made unimportable. The tests on WFDB records are skipped there; `libs/engine/tests/no_wfdb.rs`, `without_wfdb_a_wfdb_record_is_refused` (Rust viewer), `Server.WithoutWfdbAWfdbRecordIsRefused` (C++), and `tests/test_no_wfdb.py` (Python) check the refusal. |
| `make test-medorama-postgres` | The same tests with the control store in PostgreSQL: set `MEDORAMA_TEST_POSTGRES=postgres://user@host/db` to an empty database. Each test gets a schema of its own, and the trigram extension must be created once (`CREATE EXTENSION pg_trgm`). |
| `make test-tools` | The tools' tests (`tools/tests/`): the privacy check `tools/git-hooks/check-files` in a scratch repository, case by case; the screenshot tool's demo-only guard and its time limits; the demo's generator and the release archives (the same bytes on every run); the website's link script. |
| `make lint` | `cargo fmt --check`, `clippy -D warnings`, and `cargo doc` without a broken link. |
| `make lint-python` | The Python code: no unused imports or undefined names (`ruff check --select F`, the version CI's `checks.yml` uses: `.venv/bin/pip install ruff==0.17.4`). |
| `make msrv` | The workspace checked with the oldest Rust it supports, `rust-version` in `Cargo.toml` (1.88; `rustup toolchain install 1.88`), as CI's `msrv` job does. Raise it, and the versions in the README, [building.md](building.md) and the manual, when a dependency needs a newer Rust. |
| `make coverage` | The share of Medorama's lines that the tests run (`cargo llvm-cov`, with an HTML report in `target/llvm-cov/html`). It fails under 76 %, as CI's `coverage` job does. |
| `make fuzz` | The parsers fuzzed (`medorama/fuzz/`: WFDB headers, MUSE XML, DICOM, EDF and BDF files with their identifiers and Privacy Mode's copy, what users type), every target that `cargo fuzz list` names, `FUZZ_S` seconds each (60 by default), with the versions of the workspace's `Cargo.lock` (copied into `medorama/fuzz/`). It needs `rustup toolchain install nightly` and `cargo install cargo-fuzz`. CI builds every target on each push, and fuzzes each for 5 minutes when `medorama.yml` is run by hand. |
| `make audit` | The dependencies: RustSec advisories, licenses and sources (`cargo deny`, `deny.toml`). |

**The browser test** (`tests/browser.rs`) drives the page in headless Chrome, on invented
patients in a db-builder database and a connected OMOP database. It visits every tab, a patient's
page, Privacy Mode on screen and signing out, and runs an accessibility check on each in both
themes (`tests/a11y.js`: every control named, alternative texts, unique ids, WCAG AA contrast).
It checks that the next user at the browser sees nothing of the last one's patient. It also does
what people do, through the page's controls: it makes a cohort; chooses a database file in the
page's own dialog and connects it; refuses one outside `data_roots`, grants a database to another
user, sees it greyed and left out of searches when its file goes, and removes it by typing its
name; adds, edits, resets the password of and deletes a user; saves Privacy Mode's settings and
the format of dates; refreshes a cohort after new data; runs the index action from Actions and
Jobs; and shows the VitalStream viewer in its tab (when one is built). Finally it uses the page
by keyboard only: signing in, a cohort search, a patient's page and its entries, and the whole
Tab order, checking that the focus is never lost and always shown. The test is skipped without Chrome; set `MEDORAMA_TEST_CHROME` to its path, and
`MEDORAMA_TEST_CHROME_REQUIRED=1` to make a missing Chrome a failure.

**SQL that both databases run.** Three kinds of database are queried. The workspace's files are
always SQLite (through SQLCipher, opened with `workspace::open_database_file` or
`open_cohort_file`), so their queries are plain SQLite. The views of a connected database are
written in that database's own dialect (`sources/omop.rs` writes both forms; a site's `views`
are its own) and are not translated. Queries on the control store are written once, in SQLite's
dialect, and translated for PostgreSQL. Where the two plan a query very differently, the code
chooses its form by `c.dialect()` ([performance.md](performance.md)). Measure both, with many
users, before changing such a query. The translation cannot handle the following, so the code must avoid them:
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
patients. A test that needs a file writes it to a temporary folder. `tests/common/mod.rs` holds
the helpers: `Setup` sets Medorama up in a temporary folder (its settings, control store,
workspace and `data_roots`, an admin `boss` with the PHI permission and an analyst `reader`
without it) and connects databases as `boss` (`connect`); `Dbb` writes a db-builder database
([db-builder/docs/schema.md](../db-builder/docs/schema.md)) row by row (`person`, `visit`,
`entry`, `note`, `recording`) and gives it a new version (`bump`).

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

The tag starts `.github/workflows/release.yml`, which:

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
