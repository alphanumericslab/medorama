# db-builder's database: schema version 1

A db-builder database is one SQLite file (encrypted with SQLCipher when it was made with
`--encrypt`). Any SQLite program reads it. This page is the contract between db-builder and the
programs that read its databases, such as Medorama: they read the tables and columns below by
their names.

## Contents

- [Tables](#tables)
- [meta](#meta)
- [persons](#persons)
- [visits](#visits)
- [entries](#entries)
- [recordings](#recordings)
- [extras](#extras)
- [sources](#sources)
- [row_sources](#row_sources)
- [Keys](#keys)
- [Times](#times)
- [Indexes](#indexes)
- [The version rule](#the-version-rule)
- [Encryption](#encryption)

## Tables

| Table | One row per |
|---|---|
| `meta` | setting of the database: its schema, version, time zone, maker |
| `persons` | person (patient) |
| `visits` | hospital stay or visit (an encounter) |
| `entries` | result, vital sign, drug, diagnosis, procedure, note, measurement |
| `recordings` | file: a recording (a WFDB record by its header, an EDF or BDF file, an adapter's file, a folder of them), or any other file a row names (a 12-lead ECG, a picture, a PDF), of the row's category |
| `extras` | input cell that is none of the columns above, as it came |
| `sources` | file or folder added |
| `row_sources` | (persons and visits) source that names the row |
| `medorama_demo` | (only in a demo database, made with `new --demo`) one row: "A demo database: every patient in it is invented." Medorama connects such a database to a demo installation only ([docs/design/demo.md](../../docs/design/demo.md)) |

`persons`, `visits` and `entries` have the columns of Medorama's canonical views
([docs/databases.md](../../docs/databases.md#canonical-views-reference)), so a reader needs no SQL
of its own. Text columns are never NULL: an unknown value is the empty string. Times, keys of
other rows and numbers are NULL when unknown.

## meta

| Column | Type | Content |
|---|---|---|
| `key` | TEXT PRIMARY KEY | The setting's name. |
| `value` | TEXT | Its value. |

| Key | Value |
|---|---|
| `schema` | `1`: this page's version. |
| `version` | Changes at every write: the UTC time of the write and a counter, `2026-11-14T10:22:31Z 7`. |
| `timezone` | The IANA time zone (`America/New_York`) local times in the inputs were read in. Set when the database is made; one per database. |
| `created_by` | `db-builder <version>` of the program that made the database. |

## persons

| Column | Type | Content |
|---|---|---|
| `person_key` | TEXT PRIMARY KEY | The person's key ([Keys](#keys)). |
| `patient_id` | TEXT | The patient's identifier, or empty. |
| `patient_id_issuer` | TEXT | Who issued `patient_id` (a hospital's system, a study), or empty. The pair (issuer, identifier) names one patient. |
| `mrn` | TEXT | The medical record number, or empty. |
| `mrn_authority` | TEXT | The system that issued the MRN, or empty. |
| `last_name`, `first_name` | TEXT | The person's names, or empty. |
| `birth_date` | TEXT | `YYYY-MM-DD`, or empty. |
| `sex` | TEXT | `F`, `M`, or empty. |

A person has a `patient_id` or an `mrn`, or both. A row of an input that names a person by an
identifier already in the database (the same `patient_id` and issuer, or the same MRN and
authority, MRNs compared ignoring letter case) is that person; the row's names, date of birth and
sex fill what is empty and never overwrite what another row gave.

## visits

| Column | Type | Content |
|---|---|---|
| `visit_key` | TEXT PRIMARY KEY | The encounter's identifier (`encounter_id` in the input). |
| `person_key` | TEXT, or NULL | The visit's person. NULL until a row names the encounter with its patient. |
| `start_time`, `end_time` | TEXT, or NULL | Admission and discharge ([Times](#times)). |
| `unit` | TEXT | The admitting unit, else the first care unit named. |
| `icu` | INTEGER | `1` when the admitting unit or a care unit of the stay is an intensive-care unit (ICU, MICU, CCU, CICU, ITU, "intensive", "critical care"; not a step-down, post-ICU or intermediate-care unit: `ICU step-down`, `SDU`, `Post-ICU`, `IMCU`, "intermediate"), else `0`. |
| `source_id` | INTEGER | The source that added it (`sources.id`); when that source is removed and another still names the visit, that one. |

The visit's account number (`csn`) and disposition are in `extras`, and so is each bed stay (a
`bed_stays` row: [extras](#extras)). Medorama reads the bed stays as the visits' stays: its ICU
column draws those in intensive-care units, from `bed_start` to `bed_end`.

## entries

| Column | Type | Content |
|---|---|---|
| `entry_key` | TEXT PRIMARY KEY | The entry's key ([Keys](#keys)). |
| `person_key` | TEXT, or NULL | The entry's person. NULL only while the entry names a visit no input has given a person yet. |
| `visit_key` | TEXT, or NULL | The entry's visit. |
| `category` | TEXT | A category key in lower case: `icu`, `vit`, `lab`, `micro`, `path`, `ecg`, `holter`, `eeg`, `sleep`, `echo`, `cath`, `img`, `proc`, `med`, `blood`, `support`, `dx`, `obs`, `note`, `allergy` ([docs/importing.md, Categories](../../docs/importing.md#categories)). |
| `time` | TEXT, or NULL | When it happened. |
| `end_time` | TEXT, or NULL | When it ended. |
| `name` | TEXT | What it is (`Lactate`). |
| `code`, `vocabulary` | TEXT | Its code and the code's vocabulary (`2524-7`, `LOINC`). |
| `value_num` | REAL, or NULL | The value, when it is a number. |
| `value_text` | TEXT | The value, when it is not a number (`positive`). |
| `unit` | TEXT | The value's unit. |
| `ref_low`, `ref_high` | REAL, or NULL | The normal range. |
| `flag` | TEXT | The abnormal flag as written (`H`, `LL`, `A` ...). |
| `author` | TEXT | Who wrote it. |
| `text` | TEXT | A note's or a report's text. |
| `source_id` | INTEGER | Its source. |

A measurement column of an input (`QTc (ms)`, `Lactate [mmol/L]`, a known ECG measurement such as
`QTc`: docs/importing.md, "Measurement columns") makes one more entry per number, with the row's
person, visit, time and category, the measurement's name and unit.

## recordings

| Column | Type | Content |
|---|---|---|
| `recording_key` | TEXT PRIMARY KEY | The recording's key ([Keys](#keys)). |
| `person_key` | TEXT, or NULL | Its person; NULL when no table row or pattern named it. |
| `visit_key` | TEXT, or NULL | Its visit, when the input names one. |
| `file` | TEXT | The file: relative to the database file's folder (`/` between folders) when it lies inside that folder, else absolute. A WFDB record is its header (`.hea`); its signal files are next to it. |
| `format` | TEXT | The format as the recording engine names it: `WFDB record`, `WFDB multi-segment record`, `WFDB collection` (a folder with an `index.csv`: its `file` is the folder), `EDF`, `EDF+`, `BDF`, `BDF+`, an adapter's label; empty for a file no reader indexes (a 12-lead ECG in DICOM or XML, a picture, a PDF). |
| `start_time`, `end_time` | TEXT, or NULL | From the recording's header, when it states them; else from a BedMaster name (`UNIT_BED-<Unix time>`, the start only); else from the input row's `time` and `end_time`. |
| `label` | TEXT | The input row's `name`, else the file's name without its extension. |
| `category` | TEXT | `icu`, `holter`, `eeg`, `sleep` (or another category key the input gives); `icu` when none is given. |
| `size` | INTEGER | The file's size in bytes (a collection: its files' together). |
| `fingerprint` | TEXT | SHA-256 (64 hex digits) of the file's size and modification time (little-endian, as 8 and 16 bytes: nanoseconds since 1970) and its first and last MiB; for a folder (a WFDB collection, a DICOM series), SHA-256 of its files' paths below it, sizes and modification times, subfolders included. A changed file has a new fingerprint. |
| `source_id` | INTEGER | Its source. |

## extras

| Column | Type | Content |
|---|---|---|
| `table_name` | TEXT | The table of the row the cell belongs with: `persons`, `visits`, `entries`, `recordings`, or `bed_stays`. |
| `row_key` | TEXT | That row's key (`person_key`, `visit_key`, `entry_key`, `recording_key`; a bed stay's own key). |
| `name` | TEXT | The input's header, as it came, for a column db-builder does not recognise; or the field's name for a recognised field the tables have no column for (below). |
| `value` | TEXT | The cell, as it came (without surrounding spaces). Empty cells are not kept. |
| `source_id` | INTEGER | Its source. |

Recognised fields kept in `extras`, by their field names:

| Table | Names |
|---|---|
| `visits` | `csn`, `disposition`, `admit_unit` (when a care unit is also given) |
| `bed_stays` | `visit_key`, `care_unit`, `bed`, `bed_start`, `bed_end` ([Times](#times)), `recording_file` |
| `entries` | `file` (the entry's file, stored as `recordings.file` is), `service`, `domain`, `source`, `csn` (an entry that names its visit by account number) |
| any | a field the row cannot use: `sex` when it is neither female nor male, the patient's fields of a row without identifier, the encounter's fields of a row without `encounter_id` |

## sources

| Column | Type | Content |
|---|---|---|
| `id` | INTEGER PRIMARY KEY AUTOINCREMENT | The source's id. Never reused, so that keys made from it stay unique. |
| `file` | TEXT | The file (or folder, for `recordings`) added, absolute. |
| `kind` | TEXT | `csv`, `xlsx`, `xls`, `ods`, `parquet`; a known export's key (`ge-muse`, `clif-vitals` ..., a declared format's key); `recordings` for a folder. |
| `rows` | INTEGER | The rows read (after a known export's conversion), or the recordings found. |
| `added_at` | TEXT | When, in UTC (`2026-11-14T10:22:31Z`). |
| `fingerprint` | TEXT | The file's fingerprint (as `recordings.fingerprint`); for a folder, SHA-256 of its recordings' paths and fingerprints, its patients table's fingerprint and its pattern. |

A source is identified by its file and fingerprint: the same file added again unchanged is not
added again; a changed one replaces its earlier version. `db-builder remove-source` takes a source
out.

## row_sources

| Column | Type | Content |
|---|---|---|
| `table_name` | TEXT | `persons` or `visits`. |
| `row_key` | TEXT | The row's key. |
| `source_id` | INTEGER | A source that names the row. |

The primary key is the three columns. A person or a visit may be named by several sources; it is
removed with the last of them, unless a row of another source still refers to it.

## Keys

| Table | Key |
|---|---|
| `persons` | The patient ID, when the person has one and no other person has that key; else `<source id>:<row number>`. |
| `visits` | The encounter ID (`encounter_id`). |
| `entries` | `<source id>:<row number>`; an entry of a measurement column `<source id>:<row number>:<column number>`. |
| `recordings` | `<source id>:<row number>` (a table's row, or the recording's number in its folder); recordings of a folder a table's row names, `<source id>:<row number>:<n>` (a WFDB collection the row names is one recording, keyed as the row); an entry's file (a 12-lead ECG, a picture, a PDF), `<entry key>:file`. |
| `bed_stays` (in `extras`) | `<source id>:<row number>` |

The row number counts a source's rows from 1 (after a known export's conversion, which may make
several rows of one). Column numbers count from 1. Keys are text and opaque to readers: compare
them, never parse them; the one rule a reader may rely on is that an entry's file is the
`recordings` row keyed with the entry's key followed by `:file` (Medorama shows it with its
entry, not on its own).

## Times

- Every time is ISO 8601 with its offset: `2024-03-02T07:10:00-05:00`, with milliseconds when the
  input has a fraction of a second (`2024-03-02T07:10:00.250-05:00`).
- The offset is that of the database's time zone (`meta.timezone`) at that moment.
- Local times in the inputs (without an offset) were read in that time zone, as
  [docs/importing.md, Times and time zones](../../docs/importing.md#times-and-time-zones) says
  (CLIF's times without an offset as UTC).
- A recording header's clock time (WFDB, EDF) is read as UTC, as the recording engine and the
  viewers read it (a recording's start is then the one its index gives), and written in the
  database's time zone.
- `persons.birth_date` is a date, `YYYY-MM-DD`.

## Indexes

| Table | Index |
|---|---|
| `entries` | `person_key`; `time`; `(category, name)`; `(name, value_num)`; `(category, person_key)`; `visit_key`; `source_id` |
| `visits` | `person_key` |
| `recordings` | `person_key`; `file`; `source_id` |
| `persons` | `(patient_id, patient_id_issuer)`; `(mrn, mrn_authority)` |
| `extras` | `(table_name, row_key)`; `source_id` |
| `row_sources` | `source_id` |

`(name, value_num)` serves a reader's threshold (`QTc > 450`: a range of the index), and
`(category, person_key)` the persons with data of a kind (`has ecg`). A database made before
them (db-builder 0.3.1 and earlier) gets them the next time `db-builder add` or
`db-builder recordings` writes to it, from the command line or the web page (not in a dry run); an index is not a change of the
schema ([The version rule](#the-version-rule)).

## The version rule

- `meta.schema` is the version of this page. A reader checks it and refuses a database of a
  version it does not know.
- A change that a reader could read wrongly (a column removed, renamed or given another meaning,
  another form of times or keys) is a new version.
- A table, a column or an index added for db-builder's own use is not: readers read the tables and
  columns they know by name and ignore the others.
- `meta.version` changes at every write, so that a reader notices that the data changed.

## Encryption

A database made with `--encrypt` is a SQLCipher database (SQLCipher 4's defaults) with a raw
256-bit key: `PRAGMA key = "x'<64 hex digits>'"` opens it. db-builder takes the key from
`$DB_BUILDER_KEY`. Without the key, the file cannot be read, not even its schema.
