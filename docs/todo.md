# To do: what is not built yet, and how to build it

This page is for contributors. It lists every part of Medorama that the documentation calls
"not yet built", "planned" or "later". Each item says what it is for, gives a design that fits
the code as it is, lists the steps file by file, the tests and the documents to change, and says
when the item is done. Items are independent unless they say otherwise, and each is sized to be
one piece of work: one branch and one review.

Read these first: [architecture.md](architecture.md) for the parts, [developing.md](developing.md)
for the code, a request end to end and the SQL rules for both databases,
[design/sources.md](design/sources.md) for connected databases, and [security.md](security.md)
for what must not break.

## Contents

- [Rules for every item](#rules-for-every-item)
- **Connected databases:**
  [A2 Files held by a connected database](#a2-files-held-by-a-connected-database) ·
  [A3 Value thresholds pushed down](#a3-value-thresholds-pushed-down) ·
  [A4 Visits as encounters; the list's columns](#a4-visits-as-encounters-the-lists-columns) ·
  [A5 Notes near an entry, from every source](#a5-notes-near-an-entry-from-every-source) ·
  [A6 Access rules per connection](#a6-access-rules-per-connection) ·
  [A7 A Source interface; FHIR](#a7-a-source-interface-fhir) ·
  [A8 SQL Server, Oracle, Snowflake, BigQuery](#a8-sql-server-oracle-snowflake-bigquery)
- **Identity:**
  [B1 Scored matching](#b1-scored-matching) ·
  [B2 Undoing a merge](#b2-undoing-a-merge) ·
  [B3 ADT events](#b3-adt-events) ·
  [B4 Snapshots: reproducible cohorts](#b4-snapshots-reproducible-cohorts)
- **Imports:** [C1 HL7 v2](#c1-hl7-v2) · [C2 CLIF, the rest of it](#c2-clif-the-rest-of-it)
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
  [F2 Per-study access lists](#f2-per-study-access-lists) ·
  [F3 Free-text de-identification](#f3-free-text-de-identification) ·
  [F4 Text burned into pixels](#f4-text-burned-into-pixels)
- **Scalability:**
  [S4 The Patient Catalog's text search in SQL](#s4-the-patient-catalogs-text-search-in-sql) ·
  [S5 Files in object storage](#s5-files-in-object-storage)
- **Clients:** [G1 A Python client](#g1-a-python-client)
- [Suggested order](#suggested-order)

## Rules for every item

An item is done when all of the following hold. Reviews check them.

1. **The rules stay true** ([administrator's guide, Rules](administration.md#rules)). Data
   sources and connected databases are only read; removal affects the database only; identifiers
   live only in the database; a file comes in only with its patient; and columns are never merged
   across sources.
2. **The server decides.** Matching, access, privacy and search are decided in `medorama/src`.
   The page (`medorama/web`) draws what it is sent and has no build step.
3. **Both databases work.** New SQL follows the rules in
   [developing.md, "SQL that both databases run"](developing.md#tests). A schema change is the
   next numbered pair of files (`src/schema/sqlite/0NN_….sql` and
   `src/schema/postgres/0NN_….sql`), both listed in `db.rs`. The tests pass with `cargo test` and
   with `MEDORAMA_TEST_POSTGRES=postgres://…` (PostgreSQL 14 and 17).
4. **Privacy Mode holds.** Everything a user can see passes the privacy gate. A new field that can
   identify a patient gets a canary in `tests/privacy.rs` (or in `tests/connections.rs` for
   connected data) that fails if it leaks. A new kind of file is refused in Privacy Mode until a
   de-identifier exists for it (`gate.rs` `FileKind`).
5. **Access is audited.** Anything that reads identifiers, changes data or changes access writes an
   audit event (`db::audit`), listed in the audit section of [security.md](security.md).
6. **Lint and format pass:** `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`;
   `node --test` for the viewers; `pytest` for the Python packages; and `make audit` when a
   dependency is added. A new endpoint gets its operation in `medorama/openapi.yaml`, and a new
   screen gets a step in `tests/browser.rs`.
7. **The documents are updated:** the [user's guide](user-guide.md), the
   [administrator's guide](administration.md) and the [API](api.md);
   [security.md](security.md) and [protecting-patient-data.md](protecting-patient-data.md) when
   the item touches access or privacy; and the [CHANGELOG](../medorama/CHANGELOG.md). Remove the
   item from this page and from the "not yet" lists ([user-guide.md](user-guide.md),
   [design/sources.md](design/sources.md#what-is-not-there-yet), [architecture.md](architecture.md)).
8. **The wording rules hold.** Write "male", "female" and "sex" (never man, woman or gender), and
   "added manually". Imported column names are never changed. Red is used for alarms only, and
   colors come from the color-blind-safe palette in `web/style.css`.
9. **No patient data** goes into the repository, the tests or the examples. Use synthetic data only
   (`tools/make-demo-data.py`). Tests build real-looking recording names (`UNIT_BED-<Unix time>`)
   at run time.

---

## Connected databases

### A2 Files held by a connected database

**Why.** A connected database's entries carry no attachment, although an OMOP `note` may point
to a PDF and a warehouse may point to ECG files or DICOM studies.

**Design.** Add a fourth, optional canonical column to `entries`: `file`, a path inside the
catalog's `data_roots`, as for imports. Entries with a file are attached like catalog items: the
same viewers, the same file rules and the same gate.

**Steps.**
1. `src/sources/mod.rs`: add `file` to `ENTRY_COLS` and `Entry.file`. Views without the column
   must keep working: either read the column by name only when the view has it (probe once in
   `check` and keep the answer in the pool's state), or require `NULL AS file` in mapped views and
   add it to OMOP's.
2. `src/patient/items.rs` and `detail.rs`: set `It.file` for live entries with a file, and build
   the detail's `Attachment` from it. Live items have no catalog ID, so the file route needs
   another key: `GET patients/{id}/live-file/{connection}/{token}` (and `…/{name}` for folders),
   which finds the entry again by its token (`live_items`), checks `cfg.import_allowed(path)`, and
   then calls the same `attachment_response` as `patient_file` (the gate by `FileKind`).
3. `src/server.rs`: add the two routes. In Privacy Mode they behave as for catalog files: names
   become tokens, and refused kinds stay refused.
4. OMOP has no file column in the CDM, but `note_nlp` or a site's own table may hold paths.
   Document how to add one in a `sql` view.

**Tests.** In `tests/connections.rs`, a WFDB ECG and a DICOM file under a test `data_roots`, named
by a `sql` view, are opened identified and in Privacy Mode (DICOM rewritten, names as tokens),
and a path outside `data_roots` is refused.

**Done when** a connected database's ECG opens in the ECG viewer from the patient page, in both
modes, and the privacy canaries pass.

### A3 Value thresholds pushed down

**Why.** `omop.lactate > 4` reads every lactate result and tests the values in Medorama, because
units may differ from row to row. On a large OMOP database, that is millions of rows per
criterion.

**Design.** When the criterion has a value and a unit, ask the source once for the units it holds
for that name (`SELECT DISTINCT unit …`). For each unit, convert the threshold (`expr::convert`)
and push down `(unit = ?u AND value_num > ?v)` terms joined by `OR`. Rows of a unit that does not
convert are refused, as now. `latest` cannot be pushed down, because it needs every row of a
person: keep it in Medorama, or push it down as a window query on PostgreSQL only.

**Steps.**
1. `src/sources/mod.rs`: `Find` gains `values: Vec<(String unit, String cmp, f64 value)>` (or a
   range); `find_sql` writes the terms; add `units(name)`, cached with `names`.
2. `src/cohorts/in_sources.rs`, `meets_source` (`"measurement"`): fill `values` when `k.latest` is
   false, and still test each row, which is cheap and a safety net.
3. Indexes: say in design/sources.md which OMOP indexes make this fast
   (`measurement (measurement_concept_id, value_as_number)`).

**Tests.** In `tests/connections.rs`: the same answers with and without push-down; a unit that
does not convert is still refused; and a query plan check on PostgreSQL (`EXPLAIN` uses an index
on a fixture that has one).

**Done when** the rows read for `omop.lactate > 4` equal the rows that match, and the answers are
unchanged.

### A4 Visits as encounters; the list's columns

**Why.** A connected database's stays appear under Encounters, but they cannot be chosen to filter
the patient page. The Patient Catalog's columns (encounters, latest, data, items) also count the
catalog's data only.

**Design.** Give visits an encounter key of their own, `live:<connection>:<token>` (the visit key
hashed, as entries are). The page's encounter filter accepts it, and live entries carry their
visit's token, so filtering works for both kinds. For the list, compute a per-patient summary of
each connected database (counts by category, latest visit) for the page shown only (`query_in`
over the linked persons), cached per data version and source version.

**Steps.**
1. `src/sources/mod.rs`: keep `Visit.key`; `Entry.visit` is already read.
2. `src/patient/summary.rs` and `items.rs`: `LiveVisit` gains `key` (the token), and `It` gains
   `visit_token`. The journey and report take an encounter filter that is either a catalog ID or a
   token. Today `pp.enc` in `web/js/views/patient.js` is an ID: make it a string.
3. `src/patient/list.rs`, `list_browse`: for the page's rows only, make one query per source,
   `SELECT person_key, category, COUNT(*), MAX(time) FROM (entries) GROUP BY person_key, category`
   over the page's linked keys, and merge the result into `kinds`, `items` and `latest`.
4. `web/js/views/patient.js`: make the live visit rows clickable (`data-enc` set to the token).

**Tests.** In `tests/connections.rs`: filtering the journey by an OMOP visit shows that visit's
entries only, and the list shows OMOP's counts for linked patients.

**Done when** a patient known only to OMOP looks in the list like an imported one.

### A5 Notes near an entry, from every source

**Why.** An entry's detail shows the notes written within a day of it (`notes_near`), but from the
catalog only.

**Steps.** In `src/patient/detail.rs`, `notes_near` also takes the live items already read
(`live_items`, category `note`, with a time in the window), in the same order and with the same
text rule (`read_notes`); their keys are live tokens. Pass the items in rather than reading the
source again.

**Tests.** In `tests/connections.rs`: the OMOP lactate's detail lists OMOP's discharge summary,
and in Privacy Mode its names are replaced (canaries).

### A6 Access rules per connection

**Why.** Every Medorama user sees the linked persons' connected data, as their mode allows, so only
databases that all users may use can be connected (see the known limitations in
[security.md](security.md)).

**Design.** A connection may name the users, or a role, allowed to read it. For other users the
connection does not exist: no entries on pages, no criteria, no names in the resolver and no
`sources` in the list. Linked patients stay in the registry, because they may be in the catalog
too.

**Steps.**
1. Migration pair: `connection_access (connection, user_id)`. An empty list means everyone, as
   today.
2. `src/sources/mod.rs`: add `list_for(c, user)`. Every caller that today uses `sources::list` or
   `link::persons_of` for a user's answer uses the filtered form (`cohorts::meets`, the resolver,
   `patient::live_items`, `live_visits`, `list_core`, `cohorts::source_versions`).
3. `GET` and `PUT connections/{id}/access` (administrators); Import › Connected databases shows the
   list.
4. Audit changes to `connection_access`.

**Tests.** In `tests/connections.rs`: a user who is not allowed sees no OMOP entry, cannot write
`omop.lactate`, and sees no `omop` in the list; a user who is allowed sees them.

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
    fn find(&self, f: &Find, zone: Tz) -> anyhow::Result<Vec<Entry>>;
    fn find_persons(&self, f: &Find) -> anyhow::Result<Vec<String>>;
    fn visit_persons(&self, icu: bool) -> anyhow::Result<Vec<String>>;
    fn names(&self) -> anyhow::Result<Arc<Vec<(String, String, bool)>>>;
    fn version(&self) -> anyhow::Result<String>;
    fn check(&self) -> anyhow::Result<Checked>;
}
```

`Source` keeps the connection's row and returns a `Box<dyn Reader>` for its kind: `omop` and `sql`
(today's code, moved into `sources/views.rs`) and `fhir` (`sources/fhir.rs`). The canonical model
and the planner do not change.

**FHIR mapping (R4).**

| Canonical | FHIR |
|---|---|
| persons | `Patient`: `identifier` with the MRN's `system` gives `mrn` and `mrn_authority`; `birthDate`; `gender` gives `F` or `M`; names only if the site allows. |
| visits | `Encounter`: `period`, `location`, `class` (`IMP`, `ICU` and so on). |
| entries | `Observation` (`category` laboratory gives `lab`, vital-signs gives `vit`; `code` in LOINC; `valueQuantity`, `referenceRange`, `interpretation` gives `flag`), `Condition` (`dx`), `MedicationRequest` and `MedicationAdministration` (`med`), `Procedure` (`proc`), `DocumentReference` (`note`, with its text through `content.attachment`). |

- **Searching** maps `Find` to FHIR search parameters, for example
  `Observation?code=http://loinc.org|2524-7&value-quantity=gt4|http://unitsofmeasure.org|mmol/L`.
  Bulk criteria over a whole server use **Bulk Data** (`$export`, NDJSON) into a cache, or are
  refused with a message, because FHIR servers limit search sizes.
- **Authentication** uses SMART Backend Services (a signed JWT, `client_credentials`), with keys in
  files (never in the database) and tokens kept in memory only.
- **The client** is `hyper-util` (already a dependency) with rustls, or `reqwest` with rustls, with
  time limits as for SQL and paging through `Bundle.link[next]` up to `max_rows`.

**Steps.** (1) Add the trait and move the code, with no change in behavior and all tests passing.
(2) Write `fhir.rs` with `check`, `persons`, `entries` and `visits`. (3) Implement `find` for the
criteria kinds (`measurement`, `diagnosis`, `medication`, `has`, `encounter`). (4) Add the `fhir`
kind to `connections add` (`--client-id`, `--key-file`, `--token-url`).

**Tests.** A small FHIR server in the test process (axum, serving fixed Bundles), with the same
assertions as `tests/connections.rs` makes for OMOP, Privacy Mode included.

**Docs.** design/sources.md (FHIR and its limits); security.md (keys, tokens, the network path).

### A8 SQL Server, Oracle, Snowflake, BigQuery

**Why.** Many warehouses are not PostgreSQL. Today they are reached through a PostgreSQL
foreign-data wrapper (`tds_fdw`, `oracle_fdw` and others).

**Design.** Add a backend per engine in `src/sql/`, beside `pg.rs`, used only by connected
databases; the catalog's own database stays SQLite or PostgreSQL. The SQL that Medorama writes
around a view must then be written per dialect:

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
(`MEDORAMA_TEST_MSSQL=…`), with the OMOP fixture of `tests/connections.rs` created in it; CI with
a SQL Server container.

---

## Identity

### B1 Scored matching

**Why.** Two records are one patient only through a shared ID or MRN, or a reviewer's merge
(`identity.rs`, and "possible duplicates" in `review.rs`). Exports from several systems without a
common MRN need scored suggestions.

**Design.** Compute a score over name, date of birth, sex and other fields that hold no postcode
(Fellegi-Sunter weights: an agreement and a disagreement weight per field, with Jaro-Winkler
similarity for names), **only for reviewers with the PHI permission**, in the Review tab. The
scores are suggestions only: nothing is merged without a reviewer. Pairs that a reviewer marked as
different people (`patient_pairs`) are never suggested again, which is already the rule.

**Steps.** (1) `src/identity.rs`: `candidates(c) -> Vec<(a, b, score, reasons)>`, with blocking
(the same birth year, or the same first letters of the surname) so that it stays near-linear.
(2) `review.rs` lists the candidates above a threshold (a setting), with the reasons. (3) The
weights are settings, with documented defaults.

**Tests.** In `tests/identity.rs`: synthetic near-duplicates (a typing mistake, a swapped day and
month) are suggested, distinct pairs are not, and no suggestion reaches a user without the PHI
permission, because suggestions are PHI.

### B2 Undoing a merge

**Why.** A wrong merge (`identity::merge`) cannot be undone today: encounters, items, MRNs, values
and links move to the other patient.

**Design.** Record what a merge moved, so that it can be moved back: a `merge_log` (merge ID,
table, row ID) written by `identity::merge`. `unmerge(merge_id)` moves those rows back to a
patient recreated with the old patient ID and its old pseudonym (from `pseudonym_aliases`).
Cohorts whose members came through the merged patient are told, with an event in each cohort's
history, rather than changed silently.

**Steps.** A migration pair (`patient_merges` gains an ID; add `merge_log`); `identity::merge`
writes the log; add `identity::unmerge` and `POST review/unmerge` (administrators with the PHI
permission); the Review tab lists past merges; audit `unmerge_patients`.

**Tests.** In `tests/identity.rs`: a merge followed by an unmerge returns the catalog to the same
rows (compare table dumps), pseudonyms included, on both databases.

### B3 ADT events

**Why.** Encounters and bed stays come from linkage tables, while live feeds send ADT events:
admit (A01), transfer (A02), discharge (A03), update (A08), cancels (A11, A12, A13) and merge
(A40).

**Design.** An event table (`adt_events`: time, kind, patient keys, encounter keys, unit, bed and
the message's ID), applied in order to `encounters` and `bed_stays` by the same rules as a linkage
row (`linkage::apply_row`), with each cancel undoing the event it cancels. Events come from HL7 v2
([C1](#c1-hl7-v2)) or from a table import.

**Steps.** (1) The table and `linkage::apply_event`. (2) Cancels. (3) A40 through
`identity::merge`, held for review when the MRNs' authorities differ. (4) A command that rebuilds
encounters from the events (`medorama adt replay`).

**Tests.** A synthetic event stream (an admission, two transfers, a cancel of the second, a
discharge) gives the expected bed stays and times, and a replay gives the same catalog.

### B4 Snapshots: reproducible cohorts

**Why.** A cohort records its sources' versions, but not the data. Refreshing it after new imports
changes its members, and an old result cannot be rebuilt.

**Design.** Let a user freeze a cohort. Freezing stores, at that moment, the members and the rows
the criteria used (the reasons, with their values and times), and for exports the data exported,
with a content hash. A frozen cohort is never refreshed. Rebuilding the whole catalog as it was at
a date is not the aim, since imports can be removed; the frozen cohort is the record.

**Steps.** A migration pair (`cohort_snapshots`: cohort, time, versions, members as JSON, data
hash; `cohort_snapshot_rows` for the values); `POST cohorts/{id}/freeze`;
`GET cohorts/{id}/snapshots/{n}/export`; the page shows frozen cohorts as such; exports name the
snapshot and its hash.

**Tests.** In `tests/databases.rs`: freeze, import more data, check that a refresh is refused and
that the export is unchanged (the same hash).

---

## Imports

### C1 HL7 v2

**Why.** Hospitals send ADT messages (encounters, stays), ORU messages (results) and waveform
messages over HL7 v2 (MLLP). Today these data must be exported as tables.

**Design.** A reader of HL7 v2.x messages (segments, fields, components, escapes and MSH-18
encodings) in `src/hl7.rs`, which turns them into the rows that the importers already take:
- **ADT** messages become events ([B3](#b3-adt-events)) or linkage rows: PID-3 is the MRN with its
  authority, PID-5 the names, PID-7 the birth date, PID-8 the sex, PV1-19 the encounter, and PV1-3
  the location (unit and bed).
- **ORU^R01** messages become items (OBR and OBX: OBX-3 the code, OBX-5 the value, OBX-6 the unit,
  OBX-7 the range, OBX-8 the flag, OBX-14 the time), as `tables::import` writes them, one batch per
  file or per hour of a feed.
- **Waveforms** (OBX segments with NA or MA values) become a recording kind, written as WFDB by a
  converter, never into the data folders.

The input is files first (`medorama import-hl7 FILE…`), and later an MLLP listener
(`medorama listen-hl7 --port`), on the loopback address or behind TLS, which acknowledges each
message (ACK) only after it is stored.

**Tests.** Synthetic messages only (no real ones in the repository): parsing (escapes, repetitions,
empty fields), each message kind to the expected rows, malformed messages refused with their line,
and privacy canaries for the PID fields.

**Docs.** importing.md (an HL7 section), the administrator's guide (Import formats), security.md
(the listener).

### C2 CLIF, the rest of it

**Done so far.** Every table of CLIF 2.1, and CLIF 3.0's `input`, `output`,
`intermittent_dialysis`, `validated_diagnosis` and `clinical_notes_facts`, are import formats
([clif.rs](../medorama/src/formats/clif.rs); CSV or Parquet; `medorama import-clif FOLDER`, or
Import › A CLIF folder on the server). They cover patients, hospitalizations, ADT, vitals, invasive
hemodynamics, intake and output, labs, respiratory support, ECMO and MCS, CRRT, intermittent
dialysis, medication administration and orders, transfusions, assessments, position, code status,
key ICU orders, therapy details, the care team's roles, place-based indices, clinical trials,
cultures, non-culture tests, susceptibilities (on the folder's cultures, by `organism_id`),
procedures, diagnoses (hospital, patient, validated) and the facts of notes.

**What is left.**
1. **The rest of CLIF 3.0.** Its other new tables (`mcs`, `renal_replacement_therapy`, `airway`,
   `line`, `drain`, `scores`, `consult_orders`, `misc_icu_orders` and others) and its renamed
   columns, once 3.0 is released. `clinical_notes_text` has no columns in CLIF, because 3.0 keeps
   note text out of CLIF and fetches it from the site's warehouse when needed. When it has columns,
   its text becomes an item's `text` in the `note` category, under the privacy gate's rules for
   notes. A susceptibility table imported without its cultures is left out, because the link is
   read from the folder's files, not from cultures imported before.
2. **Size.** A site's vitals and respiratory tables reach hundreds of millions of rows, and an
   import of that size takes an hour or more. For such tables, read CLIF where it is: a `clif` kind
   of connection, with its views (persons, visits, entries) written once over CLIF's tables, in
   PostgreSQL or in DuckDB over the Parquet files if a driver is added, as for `omop`.
3. **mCIDE versions.** Check a table's categories against the mCIDE lists of its CLIF version,
   reporting unknown categories as clifpy's validator does, and keep the version per import.

**Tests.** `tests/clif.rs`, one table at a time; the connection kind as `tests/connections.rs` does
for OMOP.

---

## Actions and the toolbox

### D1 Encounter-scope actions

**Why.** Actions run on one recording (`scope = "recording"`). `scope = "encounter"` is accepted in
`action.toml` and listed, but refused when run (`actions.rs`). Trends across all of a stay's
recordings need the whole encounter.

**Design.** A job on an encounter. The job description lists every recording of the encounter in
order (paths, index folders, times), the encounter's and the patient's pseudonyms, and an output
folder `actions/<action>/e<encounter>/j<job>/`.

**Steps.**
1. Migration pair: `jobs.recording` becomes nullable; add `jobs.encounter` (a foreign key) and a
   check that exactly one of the two is set. The queue's index on `(state, priority, id)` is
   unchanged.
2. `src/jobs.rs`: `create_run` makes one job per encounter for encounter-scope actions; `claim` and
   `execute` build the encounter's description. A requirement (`requires`) is met when the
   required action is done on every recording of the encounter.
3. `src/actions.rs`: remove the refusal, and describe the description's schema in actions.md (The
   contract).
4. Results: `GET jobs` and the patient page show encounter results under the encounter.

**Tests.** In `tests/catalog.rs`: an example plug-in with `scope = "encounter"` that writes the
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
   them; and the viewers read `series` and `events` from the results folder
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
video to an MP4 the browser can play, caches the result in the results folder (never next to the
file), and the viewer asks for the converted frame when the transfer syntax is one it cannot
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

**Why.** Imaging comes in as DICOM files listed in a table, whereas a PACS answers DICOMweb
(QIDO-RS to search, WADO-RS to fetch).

**Design.** A connected source of studies, in the style of [A7](#a7-a-source-interface-fhir),
whose entries are studies (`img`, `echo` or `cath` by modality), linked to persons by PatientID
with its issuer. Opening a study fetches its instances through Medorama (WADO-RS), de-identified by
`dicom.rs` in Privacy Mode, and never stored outside the results folder's cache.

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
5. Medorama: the folder search (`recordings.rs` `find`) follows the engine's `kind_of`, so it
   takes such a folder whole; `writes_into` must treat an `edf` folder as it treats a WFDB folder
   (nothing written anywhere inside it). `identify.rs` `read` reads the identification fields of
   every file and flags a folder whose files name different patients.
6. Remove the limit from edf.md, the user's guide and the administrator's guide.

**Tests.** A synthetic folder of three EDF+ files made in `testdata/generate.py` (two consecutive
hours and one overlapping file): `make parity` gives the same index from the three viewers; the
channels, lanes, breaks and annotations are as expected; in Medorama, a row naming the folder
makes one recording, and `tests/privacy.rs` shows no identification field from any of the files.

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

### F2 Per-study access lists

**Why.** Every user sees every patient of a catalog, as their mode allows.

**Design.** A **study** is a named set of patients (a cohort can define it) with a list of users. A
user who is on some study's list sees only those studies' patients, everywhere: lists, pages,
cohorts, exports, files and the gateway. Users on no list see every patient, as today, so a
deployment opts in. The rule is enforced in one place: a patient filter that every query of
patients goes through.

**Steps.**
1. Migration pair: `studies`, `study_patients` and `study_users`.
2. `src/users.rs`: `User.scope: Option<HashSet<i64>>`, loaded at sign-in and cached per data
   version.
3. One function, `patient::visible(c, user, id)`, and one filter, `patient::visible_ids`, used by
   `list_core`, `summary`, `journey`, `report`, `detail`, `trends`, the file routes,
   `cohorts::search` (results filtered), `cohorts::members`, the exports, `databases::rows` (rows
   of other patients hidden), the `gateway` (recordings of visible encounters only) and `jobs`
   (selections filtered).
4. A test that walks every route as a scoped user, as `tests/privacy.rs` walks them for
   identifiers, and finds no patient outside the scope.

**Docs.** security.md (authorization; known limitations), the administrator's guide (Users, roles
and the PHI permission).

### F3 Free-text de-identification

**Why.** Notes and reports may name relatives and staff, or give addresses and phone numbers.
Privacy Mode replaces the patient's own names and dates, and lets an administrator withhold notes.

**Design.** A local, rule-based layer first, with no external service: patterns for phone numbers,
email addresses, URLs, IP addresses, street addresses, ID-like numbers and dates in every format
that `deid.rs` knows; dictionaries of staff names (from the authors and providers in the catalog)
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
that match in a copy, cached in the results folder. For ultrasound, blank the known header band by
rule (`SequenceOfUltrasoundRegions` says where the image is; the rest may hold text). The marking
stays when OCR is not available.

**Tests.** A synthetic image with a known name drawn in it is blacked out, and the canaries pass
on the served pixels.

---

## Scalability

These items address what grows fastest in the measurements on 50,000 patients in
[performance.md](performance.md).

### S4 The Patient Catalog's text search in SQL

**Why.** Browsing and filtering the list run in the database (`patient/list.rs`, `list_browse`),
but a text search still reads every patient's identifiers, encounters and categories into memory
(0.45 s for 50,000 patients).

**Design.** A search table with one row per patient and field (`patient`, `field`, `value`, with
the PHI fields marked), kept up to date by the importers and matched in SQL (`LIKE` on SQLite,
`ILIKE` with `pg_trgm` on PostgreSQL). Identifier fields are searched only for users with the PHI
permission, as now (`search::check`).

**Tests.** The list's search tests give the same answers, and the privacy tests pass (no PHI search
without the permission).

### S5 Files in object storage

**Why.** Recordings, ECGs and images are read from folders (`data_roots`). Several servers need
them on shared storage, and cloud deployments keep them in object storage (S3, Azure Blob).

**Design.** A file layer (`src/files.rs`) with a local backend (today's code) and an object-storage
backend (`s3://bucket/prefix/…` paths in the tables), which reads byte ranges for the viewers,
under the same `data_roots` rule (allowed prefixes). It needs a decision first: which stores, and
how Medorama authenticates to them (instance roles, keys in files).

---

## Clients

### G1 A Python client

**Why.** Researchers work in notebooks, and the HTTP API is complete but raw.

**Design.** A small package (`medorama-client`, in `clients/python/`, pure Python with `requests`
or `httpx`) that signs in (with a password or an API token, step 2), works with cohorts (search,
create, members, exports as pandas DataFrames), reads a patient's entries and trends, and
downloads files through the API, and so through the gate. It never reads the database. Privacy
Mode is decided by the server, and the client shows what the server sends.

**Steps.** (1) The package, with typed results (dataclasses) and errors taken from the API's
`detail`. (2) An API token kind for scripts (`POST me/tokens`), hashed like sessions, revocable
and audited. With `sign_in = "proxy"`, scripts reach Medorama through the proxy, so the token kind
must also be accepted there, or the proxy must let token requests through; decide this in the
design. (3) A notebook example on the demo data, saved without outputs (the pre-commit hook
checks).

**Tests.** `pytest` against a real `medorama serve` on the demo catalog, started by the test.

---

## Suggested order

| Order | Items | Why in this order |
|---|---|---|
| 1 | A5, A4, C2 | They finish connected databases on the page, and the rest of CLIF. |
| 2 | F2 | Per-study access is what institutions most often require before real data, now that their sign-in works through a proxy. |
| 3 | A6 | It hardens access to connected databases. |
| 4 | A3, A7 (the trait, then FHIR) | They improve the scale and reach of connected databases. |
| 5 | E7, E8 | They complete EDF for EEG monitoring and sleep studies. |
| 6 | D2, D3, D1 | The toolbox and its first tools; encounter-scope actions use them. |
| 7 | E2, E3, E1, E6 | Imaging and viewer completeness; recordings as they grow. |
| 8 | B2, B4, B1 | Identity tools, as the data grow. |
| 9 | F1 | For sites that cannot run an authenticating proxy, and a second factor for passwords. |
| 10 | C1, B3 | Live hospital feeds. |
| 11 | S4 | As the data grow ([performance.md](performance.md)). |
| 12 | A2, A8, E4, E5, F3, F4, S5, D4, D5, G1 | As needs arise. |
