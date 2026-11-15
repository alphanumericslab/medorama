# To do: what is not built yet, and how to build it

This page is for contributors. It lists every part of Medorama that the documentation calls
"not yet built", "planned" or "later". Each item says what it is for, gives a design that fits
the code as it is, lists the steps file by file, the tests and the documents to change, and says
when the item is done. Items are independent unless they say otherwise, and each is sized to be
one piece of work: one branch and one review.

Read these first: [architecture.md](architecture.md) for the parts, [developing.md](developing.md)
for the code, a request end to end and the SQL rules for both databases,
[design/databases-only.md](design/databases-only.md) for connected databases, the workspace and
its keys, [design/sources.md](design/sources.md) for reading connected databases, and
[security.md](security.md) for what must not break. db-builder's own work (more input formats,
the rest of CLIF, HL7 v2 files) is tracked in [db-builder/](../db-builder/README.md), not here.

## Contents

- [Rules for every item](#rules-for-every-item)
- **Workspace and keys:**
  [H2 Built-in key managers](#h2-built-in-key-managers)
- **Connected databases:**
  [A2 Recordings of an OMOP database](#a2-recordings-of-an-omop-database) ·
  [A3 Value thresholds pushed down](#a3-value-thresholds-pushed-down) ·
  [A7 A Source interface; FHIR](#a7-a-source-interface-fhir) ·
  [A8 SQL Server, Oracle, Snowflake, BigQuery](#a8-sql-server-oracle-snowflake-bigquery)
- **Cohorts:** [B4 Snapshots: reproducible cohorts](#b4-snapshots-reproducible-cohorts) ·
  [B5 Age at the index event](#b5-age-at-the-index-event)
- **Actions and the toolbox:**
  [D1 Encounter-scope actions](#d1-encounter-scope-actions) ·
  [D2 The toolbox and the results format](#d2-the-toolbox-and-the-results-format) ·
  [D3 The engine's streaming reader](#d3-the-engines-streaming-reader) ·
  [D4 Tools: QC, biomarkers, trends](#d4-tools-qc-biomarkers-trends) ·
  [D5 ONNX inference](#d5-onnx-inference)
- **Viewers and imaging:**
  [E1 EHR items and results on VitalStream's time axis](#e1-ehr-items-and-results-on-vitalstreams-time-axis) ·
  [E2 Codecs browsers do not decode](#e2-codecs-browsers-do-not-decode) ·
  [E3 Deflated and big-endian DICOM](#e3-deflated-and-big-endian-dicom) ·
  [E4 A PACS as a source (DICOMweb)](#e4-a-pacs-as-a-source-dicomweb) ·
  [E5 DICOM Holter in VitalStream](#e5-dicom-holter-in-vitalstream) ·
  [E6 Viewing a recording while it is being recorded](#e6-viewing-a-recording-while-it-is-being-recorded) ·
  [E7 A folder of EDF files as one session](#e7-a-folder-of-edf-files-as-one-session) ·
  [E8 An annotation-only EDF+ file over another file's signals](#e8-an-annotation-only-edf-file-over-another-files-signals)
- **Privacy and security:**
  [F1 Built-in OpenID Connect sign-in and multi-factor authentication](#f1-built-in-openid-connect-sign-in-and-multi-factor-authentication) ·
  [F2 Access by cohort](#f2-access-by-cohort) ·
  [F3 Free-text de-identification](#f3-free-text-de-identification) ·
  [F4 Text burned into pixels](#f4-text-burned-into-pixels)
- **Scalability:**
  [S4 The Patient Catalog's text search in SQL](#s4-the-patient-catalogs-text-search-in-sql) ·
  [S5 Files in object storage](#s5-files-in-object-storage)
- **Clients:** [G1 A Python client](#g1-a-python-client)
- [Suggested order](#suggested-order)

## Rules for every item

An item is done when all of the following hold. Reviews check them.

1. **The rules stay true** ([design/databases-only.md](design/databases-only.md)). Medorama imports
   nothing: connected databases and the files they name are only read. Identifiers are kept only
   in the databases and in each database's encrypted workspace folder, never in the control store.
   Databases are joined on exact (issuer, identifier) pairs only. Removing a database leaves no
   trace of it. Columns are never merged across databases.
2. **The server decides.** Access, privacy and search are decided in `medorama/src`. The page
   (`medorama/web`) draws what it is sent and has no build step.
3. **Both databases work.** New SQL follows the rules in
   [developing.md, "SQL that both databases run"](developing.md#tests). A schema change is the
   next numbered pair of files (`src/schema/sqlite/0NN_….sql` and
   `src/schema/postgres/0NN_….sql`), both listed in `db.rs`. The tests pass with `cargo test` and
   with `MEDORAMA_TEST_POSTGRES=postgres://…`.
4. **Privacy Mode holds.** Everything a user can see passes the privacy gate. A new field that can
   identify a patient gets a canary in `tests/privacy.rs` that fails if it leaks. A new kind of
   file is refused in Privacy Mode until a de-identifier exists for it (`gate.rs` `FileKind`).
5. **Access is granted and audited.** A new way to read a database's data checks its grants
   (`grants.rs`); anything that reads identifiers, changes data or changes access writes an audit
   event (`db::audit`), listed in the audit section of [security.md](security.md).
6. **What Medorama generates goes into the workspace**, in the folder of the database it came
   from, encrypted with its key, and goes with it when the database is removed
   (`databases::remove`; `tests/databases.rs`, `removing_a_database_leaves_no_trace`).
7. **Lint and format pass:** `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`;
   `node --test` for the viewers; `pytest` for the Python packages; and `make audit` when a
   dependency is added. A new endpoint gets its operation in `medorama/openapi.yaml`, and a new
   screen gets a step in `tests/browser.rs`.
8. **The documents are updated:** the [user's guide](user-guide.md), the
   [administrator's guide](administration.md) and the [API](api.md);
   [security.md](security.md) and [protecting-patient-data.md](protecting-patient-data.md) when
   the item touches access or privacy; and the [CHANGELOG](../medorama/CHANGELOG.md). Remove the
   item from this page and from the "not yet" lists ([user-guide.md](user-guide.md),
   [design/sources.md](design/sources.md#what-is-not-there-yet), [architecture.md](architecture.md#status)).
9. **The wording rules hold.** Write "male", "female" and "sex" (never man, woman or gender), and
   "added manually". Imported column names are never changed. Red is used for alarms only, and
   colors come from the color-blind-safe palette in `web/style.css`.
10. **No patient data** goes into the repository, the tests or the examples. Use synthetic data
    only (`tools/make-demo-data.py`). Tests build real-looking recording names
    (`UNIT_BED-<Unix time>`) at run time.

---

## Workspace and keys

### H2 Built-in key managers

**Why.** The master key comes from `$MEDORAMA_MASTER_KEY`, from `master_key_command` (a command
that prints it) or from `master_key_file`. A keyring, HashiCorp Vault or a cloud KMS is reached
only through `master_key_command`, so the master key itself always reaches Medorama's memory,
and a site has to write the command.

**Design.** Key managers as sources of the master key, in `workspace.rs`, beside today's three:
- **The operating system's keyring** (macOS Keychain, the Secret Service on Linux, Windows
  Credential Manager), the default on a single machine.
- **Vault's transit engine and cloud KMS** (AWS KMS, Azure Key Vault, Google Cloud KMS): the key
  manager wraps and unwraps the data keys itself, so that the master key never leaves it. The
  `keys` table's `master_id` then names the key manager's key and its version.
- **Per-user wrapped keys**, if wanted: a `keys` row per (owner, user), wrapped with a key only
  that user's sign-in unlocks, so that the server's own administrators cannot read a database
  they are not granted. It changes who can unwrap a key, so it needs a decision on recovery (a
  user who leaves) first.

**Steps.** (1) A `KeySource` trait (`load`, `wrap`, `unwrap`) with today's three as
implementations. (2) The keyring (the `keyring` crate). (3) Vault transit over HTTPS (rustls), then
the cloud KMS clients behind Cargo features. (4) Settings (`master_key_source`, its address and
credentials, in files only); `medorama keys rotate` moves data keys between sources.

**Tests.** A fake Vault in the test process (axum); a wrapped key unwraps only through its source;
rotation from a file to the fake Vault keeps every database readable.

**Docs.** design/databases-only.md (Keys and encryption), security.md (data at rest, key
management), the administrator's guide (settings).

---

## Connected databases

### A2 Recordings of an OMOP database

**Why.** A recording database lists its files in a `recordings` view (kind `views`) or table
(kind `dbb`). An OMOP database has no such table in the CDM, although a site may keep the paths
of ECG files, DICOM studies or monitor recordings in a table of its own, or in `note_nlp`.

**Design.** Let a database of kind `omop` take a `recordings` view in its settings, as `views`
does, with the same canonical columns; its persons are OMOP's. Everything else (the
workspace's `recordings.sqlite`, `data_roots`, the viewers, the gate) is unchanged.

**Steps.**
1. `src/sources/omop.rs` `views`: use `settings.recordings` when it is set.
2. `src/main.rs` and the page: `--views` (or a `--recordings` query) accepted for `omop`.
3. Document how to write the view over a site's table in databases.md.

**Tests.** In `tests/databases.rs`, an OMOP fixture with a `recordings` view naming a WFDB ECG
under a test `data_roots`: the ECG opens from the patient page, identified and in Privacy Mode,
and a path outside `data_roots` is listed with its problem.

### A3 Value thresholds pushed down

**Why.** `omop.lactate > 4` reads every lactate result and tests the values in Medorama, because
units may differ from row to row. On a large OMOP database, that is millions of rows per
criterion.

**Design.** The database's `variables.sqlite` already lists the units it holds for each name.
For each unit, convert the threshold (`expr::convert`) and push down `(unit = ?u AND value_num >
?v)` terms joined by `OR`. Rows of a unit that does not convert are refused, as now. `latest`
cannot be pushed down, because it needs every row of a person: keep it in Medorama, or push it
down as a window query on PostgreSQL only.

**Steps.**
1. `src/sources/mod.rs`: `Find` gains `values: Vec<(String unit, String cmp, f64 value)>` (or a
   range); `find_sql` writes the terms; the units come from `variables::of`.
2. `src/cohorts/in_sources.rs`, `meets_source` (`"measurement"`): fill `values` when `k.latest` is
   false, and still test each row, which is cheap and a safety net.
3. Indexes: say in design/sources.md which OMOP indexes make this fast
   (`measurement (measurement_concept_id, value_as_number)`).

**Tests.** In `tests/databases.rs`: the same answers with and without push-down; a unit that does
not convert is still refused; and a query plan check on PostgreSQL (`EXPLAIN` uses an index on a
fixture that has one).

**Done when** the rows read for `omop.lactate > 4` equal the rows that match, and the answers are
unchanged.

### A7 A Source interface; FHIR

**Why.** Every connected database is read through SQL views today. A FHIR server (Epic's,
Cerner's, a HAPI server) answers HTTP, not SQL.

**Design.** Turn what `sources::Source` does into a trait, implemented by the SQL views and by
FHIR:

```rust
pub trait Reader: Send + Sync {
    fn persons(&self, keys: &[String]) -> anyhow::Result<Vec<Person>>;
    fn entries(&self, keys: &[String], zone: Tz) -> anyhow::Result<Vec<Entry>>;
    fn visits(&self, keys: &[String], zone: Tz) -> anyhow::Result<Vec<Visit>>;
    fn recordings(&self, keys: &[String], zone: Tz) -> anyhow::Result<Vec<Recording>>;
    fn find(&self, f: &Find, zone: Tz) -> anyhow::Result<Vec<Entry>>;
    fn find_persons(&self, f: &Find) -> anyhow::Result<Vec<String>>;
    fn visit_persons(&self, icu: bool) -> anyhow::Result<Vec<String>>;
    fn version(&self) -> anyhow::Result<String>;
    fn check(&self) -> anyhow::Result<Checked>;
}
```

`Source` keeps the database's row and returns a `Box<dyn Reader>` for its kind: `omop`, `views`
and `dbb` (today's code, moved into `sources/views.rs`) and `fhir` (`sources/fhir.rs`). The
canonical model, the variables and the planner do not change.

**FHIR mapping (R4).**

| Canonical | FHIR |
|---|---|
| persons | `Patient`: each `identifier` gives a pair, its `system` the issuer (the MRN's system gives `mrn` and `mrn_authority`); `birthDate`; `gender` gives `F` or `M`; names only if the site allows. |
| visits | `Encounter`: `period`, `location`, `class` (`IMP`, `ICU` and so on). |
| entries | `Observation` (`category` laboratory gives `lab`, vital-signs gives `vit`; `code` in LOINC; `valueQuantity`, `referenceRange`, `interpretation` gives `flag`), `Condition` (`dx`), `MedicationRequest` and `MedicationAdministration` (`med`), `Procedure` (`proc`), `DocumentReference` (`note`, with its text through `content.attachment`). |

- **Searching** maps `Find` to FHIR search parameters, for example
  `Observation?code=http://loinc.org|2524-7&value-quantity=gt4|http://unitsofmeasure.org|mmol/L`.
  Reading every person (at connect and refresh) and the variables uses **Bulk Data** (`$export`,
  NDJSON), because FHIR servers limit search sizes.
- **Authentication** uses SMART Backend Services (a signed JWT, `client_credentials`), with keys in
  files (never in the database) and tokens kept in memory only.
- **The client** is `hyper-util` (already a dependency) with rustls, or `reqwest` with rustls, with
  time limits as for SQL and paging through `Bundle.link[next]` up to `max_rows`.

**Steps.** (1) Add the trait and move the code, with no change in behavior and all tests passing.
(2) Write `fhir.rs` with `check`, `persons`, `entries` and `visits`. (3) Implement `find` for the
criteria kinds (`measurement`, `diagnosis`, `medication`, `has`, `encounter`). (4) Add the `fhir`
kind to `databases connect` (`--client-id`, `--key-file`, `--token-url`).

**Tests.** A small FHIR server in the test process (axum, serving fixed Bundles), with the same
assertions as `tests/databases.rs` makes for db-builder databases, Privacy Mode included.

**Docs.** design/sources.md (FHIR and its limits); security.md (keys, tokens, the network path).

### A8 SQL Server, Oracle, Snowflake, BigQuery

**Why.** Many warehouses are not PostgreSQL. Today they are reached through a PostgreSQL
foreign-data wrapper (`tds_fdw`, `oracle_fdw` and others).

**Design.** Add a backend per engine in `src/sql/`, beside `pg.rs`, used only by connected
databases; the control store stays SQLite or PostgreSQL. The SQL that Medorama writes around a
view must then be written per dialect:

| | PostgreSQL, SQLite | SQL Server | Oracle | Snowflake, BigQuery |
|---|---|---|---|---|
| Parameters | `$1`, `?1` | `@P1` | `:1` | `?` or named |
| Row limit | `LIMIT n` | `TOP (n)` or `OFFSET … FETCH` | `FETCH FIRST n ROWS ONLY` | `LIMIT n` |
| Case-insensitive match | `ILIKE` or `LIKE` | A collation, `LIKE` | `LOWER() LIKE` | `ILIKE` or `LOWER()` |
| Client (Rust) | Built in | `tiberius` (pure Rust, TLS) | `oracle` (needs Oracle's client libraries) | Their REST APIs (HTTPS, key pair or OAuth) |

**Steps.** (1) `sql::Dialect` gains the engines, and `dialect.rs` writes `find_sql`'s wrapper per
dialect (move the wrapper SQL out of `sources/mod.rs` into a function of the dialect). (2) Add SQL
Server first, with `tiberius`: it is the most requested, and pure Rust. (3) Add the others behind
Cargo features, so that a default build stays small.

**Tests.** Per engine, behind an environment variable as for `MEDORAMA_TEST_POSTGRES`
(`MEDORAMA_TEST_MSSQL=…`), with an OMOP fixture created in it; CI with a SQL Server container.

---

## Cohorts

### B4 Snapshots: reproducible cohorts

**Why.** A cohort records its databases' versions, but not the data. Refreshing it after a
database changed changes its members, and an old result cannot be rebuilt.

**Design.** Let a user freeze a cohort. Freezing stores, at that moment, the members and the rows
the criteria used (the reasons, with their values and times), and for exports the data exported,
with a content hash, in the cohort's encrypted workspace file. A frozen cohort is never
refreshed. Removing a database still removes every frozen row it contributed to (the rule that a
removal leaves no trace wins), and the snapshot says so.

**Steps.** In the cohort's file: `snapshots` (time, versions, members, data hash) and
`snapshot_rows` (the values, with their provenance); `POST cohorts/{id}/freeze`;
`GET cohorts/{id}/snapshots/{n}/export`; the page shows frozen cohorts as such; exports name the
snapshot and its hash.

**Tests.** In `tests/databases.rs`: freeze, change a db-builder database (a new version), check
that a refresh is refused and that the export is unchanged (the same hash); remove the database
and check that its rows are gone from the snapshot.

### B5 Age at the index event

**Why.** `age` is the patient's age today. A study asks for the age at an event: "over 65 when
the QT was long", "under 18 at admission". Today `age > 65 AND QT > 470` finds patients over 65
now who had a long QT at any age.

**Design.** A comparison of age bound to the measurement, diagnosis or encounter it is written
with (`QT > 470 at age > 65`, or `age at admission > 65`): the age at that row's time, from the
date of birth (PHI, as `age` is). A date of birth known by its year alone (OMOP's
`year_of_birth`, read as 1 July) gives ages within six months; the reasons say so.

**Steps.** `expr.rs` (the words), `cohorts/` (the age at each row's time, in the test of its
criterion), the builder's page, docs/user-guide.md and the manual's cohorts chapter.

**Tests.** A patient of 70 today with a QT of 480 at 50 and one at 68: `QT > 470 at age > 65`
finds the second row only.

---

## Actions and the toolbox

### D1 Encounter-scope actions

**Why.** Actions run on one recording (`scope = "recording"`). `scope = "encounter"` is accepted in
`action.toml` and listed, but refused when run (`actions.rs`). Trends across all of a stay's
recordings need the whole encounter.

**Design.** A job on a visit of a recording database. The job description lists every recording
of the visit in order (paths, times), the visit's and the patient's pseudonyms, and
an output folder `results/<action>/e<visit>/j<job>/` in the database's workspace folder.

**Steps.**
1. Schema pair: `jobs.recording` becomes nullable; add `jobs.visit` (the visit's keyed hash, as
   `visit_pseudonyms` keeps it, with `jobs.database_id`) and a check that exactly one of the two is
   set. The queue's index on `(state, priority, id)` is
   unchanged.
2. `src/jobs.rs`: `create_run` makes one job per visit for encounter-scope actions; `claim` and
   `execute` build the visit's description from its database's `recordings.sqlite`. A requirement
   (`requires`) is met when the required action is done on every recording of the visit.
3. `src/actions.rs`: remove the refusal, and describe the description's schema in actions.md (The
   contract).
4. Results: `GET jobs` and the patient page show a visit's results under the visit; their numbers
   go into `results-<action>.sqlite` as for recordings.

**Tests.** In `tests/databases.rs`: an example plug-in with `scope = "encounter"` that writes the
number of recordings it saw; requirements across two recordings; cancelling; Privacy Mode (only
pseudonyms in the description).

### D2 The toolbox and the results format

**Why.** Processing on waveforms (filters, beat detection, heart rate, trends, signal quality)
exists only as one-off code: `derived.rs` computes the heart rate from a recording's ECG. The
design calls for one toolbox, usable as an action, a command, a Python function and a viewer track
([architecture.md](architecture.md)).

**Design.**
1. **results-format.md** (in `viewers/vitalstream/docs/`): a result is a folder with
   `result.json` (the tool, its version and parameters, the inputs by recording and channel, the
   times) and typed outputs: *series* (time, value and optional range, as little-endian f64
   arrays, like `recording_series`), *events* (time, duration, label, value) and *summary* (a
   table).
2. **`libs/toolbox`** (`signal-toolbox`): a `Tool` trait (name, version, a parameter schema in JSON
   Schema, and `run(&Inputs, &Params, &mut Outputs)`), a registry, and the engine's sample reader
   as input.
3. **The first tool** is the heart rate, moved out of `medorama/src/derived.rs` into the toolbox
   (QRS detection, `derived::qrs`), with the same numbers.
4. **Four ways to use a tool:** Medorama lists the registered tools as built-in actions;
   `vs-tool run TOOL RECORDING` runs one from the command line; a Python package (PyO3) exposes
   them; and the viewers read `series` and `events` from the job's output folder
   ([E1](#e1-ehr-items-and-results-on-vitalstreams-time-axis)).

**Tests.** The moved heart rate gives the same series as `derived.rs` gives now, on the synthetic
recordings of `viewers/vitalstream/testdata/` and on a synthetic Holter made at test time. Every
result is checked against results-format.md by a validator in the toolbox's tests.

### D3 The engine's streaming reader

**Why.** Tools that work on long recordings, such as a 7-day Holter, need samples in constant
memory, but `Dataset::samples` reads a range at once.

**Design.** `Dataset::stream(channel, from, to, block)` returns an iterator of blocks, with their
start times and gaps. It reads WFDB and EDF data in order with a bounded buffer, and an adapter's
recordings through `serve`'s `read` requests, one block at a time. The C++ engine gets the same
later, for parity.

**Tests.** The streamed samples equal `samples` on the synthetic recordings of
`viewers/vitalstream/testdata/` and through the mock adapter, and memory stays below a bound on a
synthetic week-long WFDB record, made at test time and never committed.

### D4 Tools: QC, biomarkers, trends

**Why.** Several actions are planned: data quality (flat lines, clipping, lead off, noise),
biomarkers (HRV measures, QT and QTc trends, ST deviation) and trends of vital signs.

**Design.** Each is a toolbox tool ([D2](#d2-the-toolbox-and-the-results-format)) with a
documented method and reference: for example HRV (SDNN, RMSSD, pNN50 and LF/HF over 5-minute
windows, after the Task Force of 1996), signal quality (bSQI from two QRS detectors), and QT by the
tangent method. The outputs are series and events, and each tool has a validation note on public
data (PhysioNet records, kept outside git).

**Tests.** Synthetic signals with known answers (a sine at a known rate, an injected flat line),
and the validation script, run manually on public data.

### D5 ONNX inference

**Why.** Machine-learning models exported to ONNX should run as ordinary tools on every recording.

**Design.** A toolbox tool that loads an `.onnx` model named in an action's settings, with a
manifest (input channels, sampling rate, window, stride, and the names and kinds of the outputs).
Inference runs on the CPU with `ort` (ONNX Runtime) or the pure-Rust `tract`, with windows streamed
([D3](#d3-the-engines-streaming-reader)). Models are files that administrators install, like
plug-ins, and they are trusted code: say so in security.md.

**Tests.** A tiny model made in the test (an identity, or a fixed linear layer) whose output is
known.

---

## Viewers and imaging

### E1 EHR items and results on VitalStream's time axis

**Why.** VitalStream shows waveforms and vital signs, while the stay's results, drugs and the
toolbox's series appear on the patient page only.

**Design.** VitalStream stays independent: it reads extra tracks from a file next to what it opens,
or from a URL that Medorama passes in its address (`#tracks=`), in the results format's *series*
and *events*. No Medorama API is used inside the viewer. Medorama's gateway serves that URL for
the opened recording's encounter: the items in its time range, through the gate (with times
shifted in Privacy Mode, as the gateway does today).

**Steps.** (1) Describe the tracks file format in
[viewer-api.md](../viewers/vitalstream/docs/viewer-api.md). (2) The three viewers read and draw it
(the web page is shared: `viewers/vitalstream/web/`). (3) Medorama's gateway answers
`GET /viewer/tracks/{token}`. (4) The patient page opens VitalStream with `#tracks=`.

**Tests.** The viewers' parity tests with a tracks file, and `tests/gateway.rs` (tracks only for
the opened recording, shifted in Privacy Mode).

### E2 Codecs browsers do not decode

**Why.** The PACS viewer decodes native, RLE and baseline or lossless JPEG
(`viewers/pacs/jpeg.js`). JPEG-LS (`1.2.840.10008.1.2.4.80/81`), JPEG 2000 (`.4.90/91`) and
MPEG-2 and MPEG-4 video (`.4.100` to `.4.106`) are refused.

**Design.** Decode on the server. On request, Medorama converts a frame to PNG (lossless) or a
video to an MP4 the browser can play, caches the result in the recording database's workspace folder,
encrypted with its key (never next to the file), and the viewer asks for the converted frame when the transfer syntax is one it cannot
decode. Rust decoders: `charls` bindings or a pure decoder for JPEG-LS; `openjpeg` (C, through a
crate) or `jpeg2k` for JPEG 2000; and video through `ffmpeg` as an external program, if installed,
like a plug-in.

**Steps.** (1) `GET patients/{id}/file/{item}/frame/{n}.png` in `server.rs`, through the same file
checks and, in Privacy Mode, from the de-identified header's pixels (de-identification does not
change pixels). (2) `viewers/pacs/app.js`: when `info` reports an unsupported syntax and the file
came from Medorama, load the frame URL; when the viewer runs alone from disk, say what is needed.
(3) Limit the cache's size.

**Tests.** Synthetic JPEG-LS and JPEG 2000 frames made in the test (a known gradient, encoded) are
decoded to the same pixels, and the PACS viewer's tests cover the fallback.

### E3 Deflated and big-endian DICOM

**Why.** `dicom.rs`, the DICOM de-identifier, refuses deflated explicit VR
(`1.2.840.10008.1.2.1.99`) and big-endian (`1.2.840.10008.1.2.2`, retired) files in Privacy Mode.

**Design.** Read both. For deflated files, inflate the dataset after the meta group (`flate2`,
already in the dependency tree through `calamine`), rewrite it with the same rules, and send it as
explicit VR little endian, changing the transfer syntax in the meta group. For big-endian files,
swap values by VR while parsing. Pixel data are copied, with big-endian pixel words swapped.

**Tests.** `src/dicom.rs` unit tests with synthetic files in both syntaxes give the same rewritten
header as their little-endian twins.

### E4 A PACS as a source (DICOMweb)

**Why.** Imaging comes in as DICOM files listed in a recording database, whereas a PACS answers DICOMweb
(QIDO-RS to search, WADO-RS to fetch).

**Design.** A connected database of studies, in the style of [A7](#a7-a-source-interface-fhir),
whose recordings are studies (`img`, `echo` or `cath` by modality), linked to persons by PatientID
with its issuer (`IssuerOfPatientID`). Opening a study fetches its instances through Medorama
(WADO-RS), de-identified by `dicom.rs` in Privacy Mode, and never stored outside the database's
workspace folder, encrypted.

**Tests.** A small DICOMweb server in the test process (QIDO JSON, WADO multipart), with Privacy
Mode canaries in the fetched headers.

### E5 DICOM Holter in VitalStream

**Why.** Holter recordings exported as DICOM waveforms (General ECG,
`1.2.840.10008.5.1.4.1.1.9.1.2`, the 12-lead ECG IOD's long cousin, or Ambulatory ECG,
`.9.1.3`) open in the ECG viewer only.

**Design.** A DICOM waveform source in the three engines; DICOM is an open format, so it belongs in
the engines rather than in an add-on. The waveform sequence's channels, sampling rate and
acquisition time are read into the index as a WFDB record's are, with no conversion written.
Medorama treats an ambulatory DICOM ECG of category `holter` as a recording.

**Tests.** A synthetic DICOM ambulatory ECG made in `testdata/generate.py`; its samples equal the
generator's; `make parity` gains the case (the same index from the three viewers).

### E6 Viewing a recording while it is being recorded

**Why.** The engines index a recording once it is complete. At the bedside, a recording grows for
days, and a clinician or researcher wants to see the last hours as they come.

**Design.** An index that grows. The engine indexes what is there, remembers where it stopped (per
channel, the last sample and the source's position), and appends when the source grows (new WFDB
records in a collection, new data an adapter reports). The index format gains a version with
append-only arrays and a `complete` flag in `meta.json`, and a reader reads `meta.json` again to
see the new length. The adapter protocol gains an optional `index --resume`; an adapter that does
not offer it is run again in full. The viewer's page polls the dataset's length and follows the
end when asked to.

**Steps.** (1) Write index-format.md version 3 and the protocol's `resume` first. (2) Implement it
in the Rust engine, then in the C++ and Python engines, with parity. (3) Add the viewer API's
`length` query and the page's "follow" button. (4) In Medorama, show a recording that is still
growing as such, and compute its derived series again when it grows.

**Tests.** A collection grown by the test between two indexings: the appended index equals the
index of the whole, byte for byte, in the three viewers.

### E7 A folder of EDF files as one session

**Why.** A folder of WFDB records is read as one session, on one time axis
([wfdb-collection.md](wfdb-collection.md#a-folder-of-records-one-session)), but a folder of EDF
files is not: each EDF or BDF file is a recording of its own ([edf.md](edf.md#limits)). Long EEG
monitoring and home sleep studies are often stored as one EDF file per hour or per night, and
should be seen together.

**Design.** A folder that holds EDF or BDF files and no WFDB records is a recording of kind `edf`,
read as one session with the same rules as a folder of WFDB records:
- **Files** are the `.edf` and `.bdf` files below the folder, in the order of their relative
  paths, skipping hidden entries and symbolic links.
- **Placement:** each file starts at its header's start date and time (UTC), and its EDF+D runs
  stay separate pieces with breaks between them.
- **Channels** are matched across files by label and physical dimension. A signal recorded twice
  at the same time goes into a second lane, `EEG Fpz-Cz (2)`, as for overlapping WFDB records.
- **Annotations** from every file go into the one Annotations lane.
- **Pieces** gain a `file` field (the file's path relative to the folder) before `sig` and
  `data_record`, so that the reader can find each piece's samples again.
- **The index key** uses the folder fingerprint that WFDB folders already use (the
  `[name, size, mtime]` of every file below the folder), so that adding or changing a file makes a
  new index.

A file that cannot be read is left out, and the recording's notes say so, as for WFDB records.

**Steps.**
1. `viewers/vitalstream/docs/index-format.md` and `docs/edf.md`: describe the session and the
   piece field first.
2. `libs/engine/src/sources/edf.rs`: `build_folder(path, …)`, reusing `build` per file and the
   session placement of `wfdb_folder.rs`; `Reader` keeps a list of files.
3. `libs/engine/src/sources/mod.rs`: `kind_of` and `build` recognize such a folder (after WFDB, so
   that a folder of WFDB records is read as before), and `format_label` names it (`EDF files`).
4. The same reader in the C++ engine (`viewers/vitalstream/cpp/engine/src/sources.cpp`) and the
   Python engine (`vitalstream_py/sources/edf.py`).
5. db-builder: the folder search of `db-builder recordings` follows the engine's `kind_of`, so it
   takes such a folder whole, as one `recordings` row. Medorama: `writes_into` must treat an `edf`
   folder as it treats a WFDB folder (nothing written anywhere inside it).
6. Remove the limit from edf.md, the user's guide and the administrator's guide.

**Tests.** A synthetic folder of three EDF+ files made in `testdata/generate.py` (two consecutive
hours and one overlapping file): `make parity` gives the same index from the three viewers; the
channels, lanes, breaks and annotations are as expected; a db-builder database whose recording
is the folder makes one recording in Medorama, and `tests/privacy.rs` shows no identification
field from any of the files.

**Done when** a folder of hourly EDF files opens in VitalStream, alone and in Medorama, as one
recording with one time axis.

### E8 An annotation-only EDF+ file over another file's signals

**Why.** Sleep scoring is often stored in a separate EDF+ file that holds only annotations (a
hypnogram), next to the file with the signals; the public Sleep-EDF data set is organized this
way. Today an annotation-only file has no signal to draw and is refused ("holds no signals, only
annotations", `edf.rs` `build`), so the scoring cannot be shown over the signals.

**Design.** An annotation-only EDF+ file contributes events and no channels:
- **In a session folder** ([E7](#e7-a-folder-of-edf-files-as-one-session)), an annotation-only file
  is part of the session, and its annotations join the Annotations lane. This is the main route,
  and it needs no new setting: put the hypnogram in the same folder as the signals.
- **Opened together:** the viewer API's open request gains an optional list of annotation files
  (`annotations: [path, …]`), and VitalStream's Open dialog an **Add annotations…** choice. The
  annotation files take part in the index key, so that an index with annotations is never mistaken
  for one without.
- **Times:** onsets in an EDF+ file are relative to that file's own start, so each annotation is
  placed at its own file's start time plus its onset, whatever the signal file's start.
- Opened alone, an annotation-only file is still refused, with a message that says how to show it.

**Steps.** (1) Describe the rules in `docs/edf.md` and `index-format.md`. (2)
`libs/engine/src/sources/edf.rs`: `annotations_of(path)` returns the events of any EDF+ file;
`build` and `build_folder` accept files without signals inside a session. (3) The same in the C++
and Python engines. (4) The open request and the Open dialog
(`viewers/vitalstream/docs/viewer-api.md`, `viewers/vitalstream/web/`), and Medorama's gateway,
which passes the list only for files inside `data_roots` and, for other users than
administrators, only for files of the same recording.

**Tests.** A synthetic pair made in `testdata/generate.py` (a PSG file and a hypnogram file that
starts 30 seconds later): the stages are placed at the right times in the three viewers
(`make parity`); a hypnogram alone is refused with the new message; in Privacy Mode the
annotation texts pass the gate as other annotations do.

---

## Privacy and security

### F1 Built-in OpenID Connect sign-in and multi-factor authentication

**Why.** Institutions require their own sign-in, with single sign-on and multi-factor
authentication. Medorama already supports this through an authenticating reverse proxy
(`sign_in = "proxy"`, `signin.rs`; [sign-in.md](sign-in.md)). Some sites cannot run such a proxy
in front of Medorama, or prefer one component fewer. A built-in OpenID Connect method would let
Medorama talk to the institution's identity provider (Microsoft Entra ID, Okta, Google, Keycloak,
Shibboleth's OIDC) directly. Sites that keep `password` sign-in have no second factor today.

**Design.** A third sign-in method, `sign_in = "oidc"`, added to `signin.rs` as the module
describes, so that nothing outside sign-in changes:
- **The flow** is the authorization-code flow with PKCE against the provider found by discovery
  (`/.well-known/openid-configuration`). The provider authenticates the user, multi-factor
  authentication included.
- **The account** is the one whose name equals a claim of the ID token (by default `email`;
  `preferred_username` and others by setting), compared ignoring case, as for the proxy
  (`users::get_by_login`). Accounts, roles and the PHI permission stay in Medorama, and
  provisioning stays deliberate: an unknown name is refused with a reason (403).
- **The session** after the callback is the same session as a password sign-in (a cookie, stored
  hashed), so `identify` reads it as it does for `password`.
- **Second factor for passwords:** for sites that keep `password`, TOTP (RFC 6238) as an optional
  second factor per account.

**Steps.**
1. `src/config.rs`: `sign_in = "oidc"` with `oidc_issuer`, `oidc_client_id`,
   `oidc_client_secret_file`, `oidc_redirect_url` and `oidc_claim`, parsed in `sign_in_method` with
   the same rule as the proxy's settings (an error without `sign_in = "oidc"`), and cases in the
   `sign_in_settings` test; the template in `to_toml`.
2. `src/signin.rs`: `Method::Oidc`; its `identify` arm reads the session cookie; `GET login/oidc`
   (state, nonce and the PKCE verifier in a short-lived cookie) and `GET login/oidc/callback` (the
   code exchanged over rustls; the ID token verified: its signature against the provider's JWKS,
   `iss`, `aud`, `exp` and `nonce`), then `users::new_session`; `login` refuses passwords, as for
   the proxy; `method` reports `oidc` to the page.
3. `src/server.rs`: the two public routes, beside `sign-in`. `web/js/main.js`: a **Sign in with
   your institution** button when the method is `oidc`.
4. TOTP: `users.totp_secret` (a migration pair; encrypted with a key from a file), enrollment and
   verification endpoints, a window of one step either way, and reuse of a code refused.
5. Audit: `login` with the detail `oidc`; `login_failed` with the reason.

**Tests.** In `tests/signin.rs`, a fake OIDC provider in the test process (its own JWKS and signed
tokens): a good sign-in; a wrong `aud`, an expired token, a replayed nonce and a wrong state are
refused; an unknown name is refused with its reason. TOTP with fixed times.

**Docs.** sign-in.md (the method and its settings), the administrator's guide (Settings, Choose the
sign-in method), security.md (authentication, the deployment checklist).

### F2 Access by cohort

**Why.** Grants are per database and per cohort: a user granted `view` on a database sees every
one of its patients. A study often may see only its own patients of a shared database.

**Design.** A grant on a database can be limited to a cohort's members: (user, `db:N`,
permission, `cohort:M`). Such a user sees, of that database, only the patients of that cohort,
everywhere: lists, pages, criteria, exports, files and the gateway. A grant without a cohort sees
every patient, as today. The rule is enforced in one place: a patient filter that every query of a
database's patients goes through.

**Steps.**
1. Schema pair: `grants.cohort` (nullable).
2. `src/grants.rs`: `scope(c, user, database) -> Option<HashSet<i64>>`, cached per data version.
3. One function, `patient::visible(c, user, id)`, and one filter used by `list.rs`, `summary`,
   `journey`, `report`, `detail`, `trends`, the file routes, `cohorts::evaluate` (results
   filtered), `cohorts::members`, the exports, the `gateway` and `jobs` (selections filtered).
4. A test that walks every route as a limited user, as `tests/privacy.rs` walks them for
   identifiers, and finds no patient outside the cohort.

**Docs.** security.md (authorization; known limitations), the administrator's guide (users,
grants), design/databases-only.md (grants).

### F3 Free-text de-identification

**Why.** Notes and reports may name relatives and staff, or give addresses and phone numbers.
Privacy Mode replaces the patient's own names and dates, and lets an administrator withhold notes.

**Design.** A local, rule-based layer first, with no external service: patterns for phone numbers,
email addresses, URLs, IP addresses, street addresses, ID-like numbers and dates in every format
that `deid.rs` knows; dictionaries of staff names (from the authors and providers the databases give)
and of first names (a public list, shipped as data); and each match replaced by its placeholder.
Then an optional model (a clinical de-identification NER model run through
[D5](#d5-onnx-inference)'s ONNX runtime), off by default, measured on a public annotated corpus
(i2b2 2014, under its data use agreement, outside git), with its recall reported in security.md.

**Steps.** `src/deid.rs` `free_text(text, dict) -> String`; the gate's `note()` calls it; a Privacy
Mode setting for notes: withheld, de-identified by rules, or de-identified by rules and model.

**Tests.** Synthetic notes with every pattern, and the canaries in `tests/privacy.rs` extended with
a phone number, an email address, a street address and a relative's name inside a note.

### F4 Text burned into pixels

**Why.** Pictures, scanned PDFs, ultrasound images and DICOM secondary captures may show names in
their pixels. Privacy Mode sends them as they are and marks their tab with a warning triangle.

**Design.** Run OCR on the server (Tesseract as an external program, if installed) over the frame,
compare the words with the patient's identifiers and the dictionary, and black out the regions
that match in a copy, cached in the database's workspace folder, encrypted. For ultrasound, blank the known header band by
rule (`SequenceOfUltrasoundRegions` says where the image is; the rest may hold text). The marking
stays when OCR is not available.

**Tests.** A synthetic image with a known name drawn in it is blacked out, and the canaries pass
on the served pixels.

---

## Scalability

These items address what grows fastest in the measurements on 50,000 patients in
[performance.md](performance.md).

### S4 The Patient Catalog's text search in SQL

**Why.** The Patient Catalog reads each database's `persons.sqlite` (its persons and their
summaries), and a text search matches every field of every patient in memory (`patient/list.rs`).
It grows with the number of patients of every database a user may view.

**Design.** A search table in each database's `persons.sqlite` (`person_key`, `field`, `value`,
with the PHI fields marked), written when the database is connected or refreshed and matched in
SQL there (`LIKE`, with an FTS5 index when it helps); the answers of each database are merged by
patient. Identifier fields are searched only for users with the PHI permission and `phi` on that
database, as now (`search::check`).

**Tests.** The list's search tests give the same answers, and the privacy tests pass (no PHI search
without the permission).

### S5 Files in object storage

**Why.** Recordings, ECGs and images are read from folders (`data_roots`), as the recording
databases name them. Cloud deployments keep them in object storage (S3, Azure Blob).

**Design.** A file layer (`src/files.rs`) with a local backend (today's code) and an object-storage
backend (`s3://bucket/prefix/…` paths in the recording databases), which reads byte ranges for the viewers,
under the same `data_roots` rule (allowed prefixes). It needs a decision first: which stores, and
how Medorama authenticates to them (instance roles, keys in files).

---

## Clients

### G1 A Python client

**Why.** Researchers work in notebooks, and the HTTP API is complete but raw.

**Design.** A small package (`medorama-client`, in `clients/python/`, pure Python with `requests`
or `httpx`) that signs in (with a password or an API token, step 2), works with cohorts (search,
create, members, exports as pandas DataFrames), reads a patient's entries and trends, and
downloads files through the API, and so through the gate. It never reads a database itself. Privacy
Mode is decided by the server, and the client shows what the server sends.

**Steps.** (1) The package, with typed results (dataclasses) and errors taken from the API's
`detail`. (2) An API token kind for scripts (`POST me/tokens`), hashed like sessions, revocable
and audited. With `sign_in = "proxy"`, scripts reach Medorama through the proxy, so the token kind
must also be accepted there, or the proxy must let token requests through; decide this in the
design. (3) A notebook example on the demo data, saved without outputs (the pre-commit hook
checks).

**Tests.** `pytest` against a real `medorama serve` on the demo databases, started by the test.

---

## Suggested order

| Order | Items | Why in this order |
|---|---|---|
| 1 | F2 | Access limited to a study's patients is what institutions ask for before real data. |
| 2 | H2 | Key managers without a command to write; the master key kept inside them. |
| 3 | A3, A7 (the trait, then FHIR) | They improve the scale and reach of connected databases. |
| 4 | E7, E8 | They complete EDF for EEG monitoring and sleep studies. |
| 5 | D2, D3, D1 | The toolbox and its first tools; encounter-scope actions use them. |
| 6 | E2, E3, E1, E6 | Imaging and viewer completeness; recordings as they grow. |
| 7 | B4, B5 | Reproducible cohorts, as the databases change; ages at the events studied. |
| 8 | F1 | For sites that cannot run an authenticating proxy, and a second factor for passwords. |
| 9 | S4 | As the data grow ([performance.md](performance.md)). |
| 10 | A2, A8, E4, E5, F3, F4, S5, D4, D5, G1 | As needs arise. |