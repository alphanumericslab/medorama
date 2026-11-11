# Connecting and building databases for Medorama

This guide explains how a hospital's or a study's data reach Medorama, which way to choose, and
how to follow each way from nothing to data on a patient's page. It is written for data managers
and database engineers who have not used Medorama before. Each way ends with a worked example
and with how to check the result.

The exact rules for imported tables (every recognized column name, time format and identifier
rule) are in [importing.md](importing.md), the reference this guide links to. The settings of
connected databases are summarized in
[administration.md, Connected databases](administration.md#connected-databases), and their design
in [design/sources.md](design/sources.md).

## Contents

- [Terms used on this page](#terms-used-on-this-page)
- [Choose a way in](#choose-a-way-in)
- [What every database must say](#what-every-database-must-say)
- [Import tables](#import-tables)
- [Import a known export](#import-a-known-export)
- [Connect an OMOP database](#connect-an-omop-database)
- [Connect any SQL database through views](#connect-any-sql-database-through-views)
  - [Canonical views reference](#canonical-views-reference)
- [Build a new database for Medorama](#build-a-new-database-for-medorama)
- [Files: recordings, ECGs and images](#files-recordings-ecgs-and-images)
- [Check the result](#check-the-result)
- [Checklist](#checklist)
- [Troubleshooting](#troubleshooting)

## Terms used on this page

| Term | Meaning |
|---|---|
| Catalog | Medorama's own database: its patients and their identifiers, encounters, entries, imported tables, users, cohorts and settings. It is SQLite by default, or PostgreSQL. |
| Database | Any source of patient data that Medorama shows in its **Databases** tab: an imported table, or a connected database. |
| Import | Copying a table (CSV, Parquet, Excel, OpenDocument) into the catalog. Each imported table is one database, kept with its columns as they came, and removable as a whole. |
| Connected database | A database that Medorama reads in place, read-only, at each request: an OMOP CDM instance, a research warehouse, an SQLite file. Nothing is copied but the identifiers needed to link its persons to patients. |
| Canonical views | Three SQL queries (`persons`, `visits`, `entries`) that present a connected database in the columns Medorama reads. OMOP has them built in. |
| Entry | A dated item of a patient: a result, a vital sign, a drug, a diagnosis, a note, a file. |
| Category | The column of the patient page's journey that an entry belongs to (`lab`, `med`, `ecg`, `sleep` …; [importing.md, Categories](importing.md#categories)). |
| Data root | A folder named in the `data_roots` setting. When set, every file an import names, and every SQLite file connected from the page, must lie inside one. |
| PHI | Protected health information. Medorama keeps names, MRNs, account numbers and dates of birth in its identifier tables and shows them only to users with the PHI permission, outside Privacy Mode ([security.md](security.md)). |

## Choose a way in

| Your situation | Way in | What you write | Where the data stay |
|---|---|---|---|
| You have exported tables: CSV, Excel, OpenDocument or Parquet files from an EHR, a laboratory system, a study's spreadsheet | [Import tables](#import-tables) | Nothing when the column names are recognized; otherwise a mapping (catalog field = header) | Copied into the catalog |
| You have an export of a system Medorama knows: GE MUSE ECGs, DICOM studies, a CLIF database, or an export your site described in a TOML file | [Import a known export](#import-a-known-export) | Nothing (or one TOML description for your own export format) | Copied into the catalog |
| You have an OMOP CDM 5.3 or 5.4 database (PostgreSQL, or an SQLite file) | [Connect an OMOP database](#connect-an-omop-database) | The connection's settings | In place, read at each request |
| You have another SQL database (a warehouse, a registry) in PostgreSQL or an SQLite file, which should stay where it is | [Connect any SQL database through views](#connect-any-sql-database-through-views) | Three SQL views in a TOML file | In place, read at each request |
| You are building a database for a study, from scratch | [Build a new database for Medorama](#build-a-new-database-for-medorama) | A folder of CSV files, or an SQLite file with the canonical tables | Copied (CSV) or in place (SQLite) |
| You have recordings, ECGs or images (files) | Always a table: [Files](#files-recordings-ecgs-and-images) | A table of patient and file | Files stay where they are; the table is imported |

The same questions as a flow:

1. **Are they files** (recordings, ECGs, DICOM)? Then list them in a table with their patients
   and import it. A connected database cannot bring files.
2. **Is it an OMOP CDM database?** Connect it as `omop`. Do not export it.
3. **Must the data stay in their database**, because they change daily or are too large to copy?
   Connect it through views.
4. **Is it a one-off extract, or a set of spreadsheets?** Import it.

The ways mix in one catalog. A person of a connected database is linked to the same patient as an
imported row with the same patient ID or MRN, so one patient's page can show imported laboratory
results, an OMOP instance's diagnoses and a recording side by side.

| | Import | Connect |
|---|---|---|
| Speed of searches | Fastest: the data are in the catalog | Each search queries the database |
| Freshness | As of the import; import again for newer data | Always current |
| Files (recordings, ECGs, images) | Yes, through a `file` column | No |
| Engines | None needed | PostgreSQL (on-site, Amazon RDS and Aurora, Azure Database for PostgreSQL), SQLite files |
| Removing | Removing the database removes everything it added | Disconnecting removes its links; the database is untouched |

## What every database must say

Whatever the way in, the data must answer five questions for Medorama to place them.

1. **Who.** Every row names its patient by an identifier the institution uses consistently: a
   `patient_id` (a study's or a system's identifier), or an `mrn` (the medical record number),
   with `mrn_authority` when several systems issue MRNs. Names, date of birth and sex describe a
   patient; they never identify one. See
   [importing.md, Identifiers](importing.md#identifiers-patient-ids-mrns-and-authorities).
2. **When.** Every result, note, drug or file has a time: with an offset
   (`2024-03-02T07:10:00-05:00`, `Z`), as local time in the catalog's (or the connection's) time
   zone (`2024-03-02 07:10`), or in Unix seconds. See
   [importing.md, Times and time zones](importing.md#times-and-time-zones).
3. **Where in the stay** (optional, recommended). The encounter (`encounter_id`, or the account
   number `csn`) the row belongs to, and the bed stays (`care_unit`, `bed`, `bed_start`,
   `bed_end`) that link bedside-monitor recordings to their encounters.
4. **What.** A name (`Lactate`, `QTc`, `Vancomycin`), and when there is one, a value with its
   unit, a normal range and a flag, a code with its vocabulary (`2524-7`, `LOINC`), a text.
5. **Which category.** The column of the patient page it belongs to: given by a `category`
   column, by the import's category, or guessed from the table's name and the entry's name. See
   [importing.md, Categories](importing.md#categories), which includes the EEG and Sleep
   categories for recordings.

> **Identifiers are protected data.** Export or expose only the identifiers needed for linking. A
> study that needs no names or dates of birth should not provide them: Medorama links by patient
> IDs and MRNs alone. Users without the PHI permission, and everyone in Privacy Mode, see
> placeholders and pseudonyms instead of identifiers ([security.md](security.md)).

## Import tables

Importing copies a table into the catalog. It is the simplest way in, and the only one that
brings files. The column reference for each kind of table (patients, encounters, bed stays,
results, medications, conditions, notes, files) is in
[importing.md, Column reference](importing.md#column-reference).

### Worked example: patients, an encounter and laboratory results

This example imports three small tables and checks that they reach the patient's page. Every
column name in them is recognized as it is, so no mapping is needed.

1. **Set the time zone and the data roots.** In the catalog's `medorama.toml`, set `timezone` to
   the time zone of the local times in your tables (the default is `America/New_York`), and, on
   a server with real data, `data_roots` to the folders that hold the files your tables name.
   Restart the server after a change.

2. **Write the tables.** `patients.csv` names and describes the patients:

   ```csv
   Patient ID,MRN,Last Name,First Name,DOB,Sex
   P000001,SYN0000001,Doe,Jane,1950-03-04,F
   P000002,SYN0000002,Roe,Richard,1962-11-20,M
   ```

   `encounters.csv` gives one encounter and its bed stay in an intensive-care unit:

   ```csv
   Encounter ID,Patient ID,Admit Time,Discharge Time,Admit Unit,Care Unit,Bed,Bed Start,Bed End
   E000001,P000001,2024-01-01 08:00,2024-01-09 13:00,Cardiology,ICU7,12,2024-01-01 11:00,2024-01-03 11:00
   ```

   `labs.csv` holds results, the first matched to its encounter, the second only to its patient
   by MRN:

   ```csv
   MRN,Encounter ID,Test Name,Result,Units,Ref Low,Ref High,Abnormal Flag,Collected,Code,Code System
   SYN0000001,E000001,Lactate,4.1,mmol/L,0.5,2,H,2024-01-02 06:10,2524-7,LOINC
   SYN0000002,,Creatinine,1.1,mg/dL,0.6,1.3,,2024-02-02 06:10,2160-0,LOINC
   ```

3. **Preview each table.** A dry run reads and checks every row as the import would, and keeps
   nothing. Run it on the server, as the account that runs the catalog (or with `--home`):

   ```bash
   medorama import-table patients.csv --dry-run
   ```

   It reports what the import would add:

   ```
   (dry run) 2 rows: 2 patient(s) and 0 encounter(s) added, 0 item(s) added (0 already there, 0 waiting), 0 recording(s) added, 0 file(s) attached, 0 row(s) held back
   ```

4. **Import the tables**, patients first:

   ```bash
   medorama import-table patients.csv
   medorama import-table encounters.csv
   medorama import-table labs.csv
   ```

   Each import reports its number (`import 3: 2 rows: … 2 item(s) added …`).

5. **Check the result.** The following command lists the imports, newest first:

   ```bash
   medorama imports list
   ```

   Then, in the page: the **Databases** tab shows the three tables as imported; the **Patient
   Catalog** lists the two patients; Jane Doe's page shows the stay in the ICU column and the
   lactate, flagged high, in the Labs column. **Review** lists any row that waits for its patient
   or was held back.

The same import in the web page:

1. Open **Import** › **Add a database**, and keep **Import tables** selected.
2. Drop the three files. For each, Medorama shows the columns it recognized.
3. Select **Preview all**, read the report, then select **Import all**.

### When the columns have other names

When a table's headers are not among the recognized names, map them once. In the page, choose
the catalog field for each header under **Columns**, and select **Save mapping as…** to reuse it
for later tables of the same shape. On the command line, write the mapping as a TOML file of
`catalog field = "header"`:

```toml
# mapping.toml
mrn = "Hosp No"
name = "Analyte"
value = "Res"
time = "Specimen taken"
```

The following commands check, then import, a table with those headers:

```bash
medorama import-table export.csv --mapping mapping.toml --dry-run
medorama import-table export.csv --mapping mapping.toml --source "LIS extract 2024-03"
```

Columns that are not mapped are kept under their own names, and are shown with the table.
Columns whose names carry a unit (`QT (ms)`, `LVEF (%)`) become searchable numbers
([importing.md, Measurement columns](importing.md#measurement-columns)).

## Import a known export

Some systems' exports are recognized by their columns and converted as they are imported; this
conversion is called an **import format**. The command `medorama formats` lists the formats the
catalog knows, built in and described by your site. The page's **Import formats** card lists them
too, and **Format** in the Import tab names one.

| Source | How to import it | What comes in |
|---|---|---|
| A folder of GE MUSE XML ECGs | `muse-py index FOLDER -o ecg.csv`, then `medorama import-table ecg.csv` | One row per ECG: the patient (MUSE's PatientID, in a column `mrn`), the time, the measurements, the statements and the file, opened by the ECG viewer. |
| A table of GE MUSE metadata (the XML elements as columns) | `medorama import-table muse.csv` (recognized: format `ge-muse`, the patient by MRN), or `--format ge-muse-patient-id` (the patient by a research ID) | The same, without the staff, visit numbers, rooms and beds, which the format leaves out. |
| A folder of DICOM studies | `dicom-py index FOLDER -o imaging.csv`, then import it | One row per series, loop, run, report, PDF or ECG, with the patient (DICOM's PatientID, in a column `mrn`), the category and the file. |
| A CLIF database (the Common Longitudinal ICU data Format), as CSV or Parquet files | `medorama import-clif FOLDER`, or Import › Add a database › **A CLIF folder on the server** (inside the data roots: typed, or picked with **Choose…**) | Each CLIF table as a database: patients, hospitalizations and ADT first, then vitals, labs, medications, respiratory support and the rest ([administration.md, Import formats](administration.md#import-formats)). |
| Your own system's export | A TOML description of its columns in a folder of `formats_dirs` (below) | What the description names, under the catalog's column names. |

`muse-py` and `dicom-py` are the viewers' Python tools ([viewers/README.md](../viewers/README.md)).
Their `index` command writes file paths relative to the CSV file's folder, so import the CSV
from where it was written.

### Describe your own export in TOML

A site describes an export that it imports regularly, column by column, in a TOML file. No code
and no new build of Medorama are needed. The description is read from a folder named in
`formats_dirs` (default: the `formats` folder of the catalog's home). The rules are in
[medorama/src/formats/declared.rs](../medorama/src/formats/declared.rs).

1. **Write the description.** This example converts a laboratory system's nightly export whose
   columns are `HospNo`, `TestName`, `TestCode`, `CodeSystem`, `ResultValue`, `ResultUnit`,
   `CollDate`, `CollTime`, `AbnFlag` and `Clerk`:

   ```toml
   # <catalog home>/formats/acme-labs.toml
   key = "acme-labs"                    # lowercase letters, digits and -, starting with a letter
   label = "ACME laboratory export"
   category = "lab"                     # the category of its entries
   about = "One row per result, from the ACME LIS nightly export."
   detect = ["HospNo", "TestCode", "ResultValue"]   # recognized when a table has all of these
   required = ["mrn", "name"]           # rows with any of these empty are left out (counted)

   [[column]]
   name = "mrn"                         # a catalog field, or a column name of its own
   from = "HospNo"                      # copied, trimmed

   [[column]]
   name = "name"
   from = "TestName"

   [[column]]
   name = "code"
   from = "TestCode"

   [[column]]
   name = "vocabulary"
   from = "CodeSystem"

   [[column]]
   name = "value"
   from = "ResultValue"
   number = true                        # kept only when it is a number

   [[column]]
   name = "unit"
   from = "ResultUnit"

   [[column]]
   name = "time"
   from = "CollDate"
   date = "%d/%m/%Y"                    # the form of the date (chrono's format), written as ISO 8601
   time_from = "CollTime"               # the time of day, from this column
   time = "%H%M"

   [[column]]
   name = "flag"
   from = "AbnFlag"
   map = { HIGH = "H", LOW = "L" }      # values replaced; any other value becomes empty
   ```

   A column that the description does not name is left out: the `Clerk` column above, which
   names staff, never enters the catalog. A column of numbered parts can be joined with
   `join = "Statement/{n}/Text"` (and `separator`) instead of `from`.

2. **Check it.** The following command reads the description and lists its columns:

   ```bash
   medorama formats --check formats/acme-labs.toml
   ```

3. **Import a table.** Descriptions are read again at each import, so neither the server nor the
   page needs a restart. The following commands preview and import a table of that export:

   ```bash
   medorama import-table acme-2024-03-01.csv --dry-run   # recognized by its columns
   medorama import-table acme-2024-03-01.csv
   ```

A converter that a description cannot express (rows that become several entries, values that
depend on other rows) is written in Rust instead
([readers.md, Tables](readers.md#tables-add-an-import-format-or-a-connected-database)).

## Connect an OMOP database

An OMOP CDM database is read in place through views that Medorama has built in; you write no
SQL. Medorama supports the OMOP CDM versions 5.3 and 5.4, in PostgreSQL (on-site, Amazon RDS
and Aurora, Azure Database for PostgreSQL, or any server that speaks its protocol) or in an
SQLite file.

### What Medorama reads

Every one of these tables must exist, even when empty: a view that names a missing table fails,
and the connection is refused when it is added. `cdm_source` is therefore required in practice.
The views are in [medorama/src/sources/omop.rs](../medorama/src/sources/omop.rs).

| OMOP table | Read as | Columns read |
|---|---|---|
| `person` | Persons | `person_id` (the key); `person_source_value` (the MRN or patient ID, by the `identifier` setting); `gender_concept_id` (8532 female, 8507 male); `birth_datetime`, else `year_of_birth`, `month_of_birth`, `day_of_birth`. Names are not read: OMOP holds none. |
| `visit_occurrence` | Visits | `visit_occurrence_id`, `person_id`, `visit_start_datetime` (else `visit_start_date`), `visit_end_datetime` (else `visit_end_date`), `care_site_id`, `visit_concept_id` |
| `visit_detail` | Intensive-care visits | A visit is an intensive-care stay when one of its details has `visit_detail_concept_id` 32037, or when its unit's name is that of an intensive-care unit. |
| `care_site` | The visit's unit | `care_site_name` (else the visit concept's name) |
| `measurement` | Entries, category `lab` (`vit` for vital signs, by LOINC code or name) | The concept's name and code (else `measurement_source_value`), `value_as_number` (else the value concept's name, or `value_source_value`, as text), the unit concept's code (UCUM; else `unit_source_value`), `range_low`, `range_high`, `measurement_datetime` (else `measurement_date`) |
| `observation` | Entries, category `obs` | The concept's name and code, `value_as_number` (else `value_as_string` or the value concept's name), the unit, the time |
| `condition_occurrence` | Entries, category `dx` | The concept's name; the source concept's code and vocabulary (ICD-10-CM), else `condition_source_value`; start and end |
| `drug_exposure` | Entries, category `med` | The concept's name and code, `dose_unit_source_value`, start and end |
| `procedure_occurrence` | Entries, category `proc` | The concept's name and code, the time |
| `note` | Entries, category `note` | `note_title` (else the note type's name), `note_text`, the time; the author from `provider` |
| `provider` | A note's author | `provider_name` |
| `concept` | Names and codes | In the vocabulary schema (`vocabulary_schema`, when apart from the CDM's) |
| `cdm_source` | The data's version | `source_release_date` and `cdm_release_date` |

Times are read as the database's local times, in the connection's time zone. On SQLite, dates
that OMOP's tools stored as Unix seconds are converted first.

### How persons are linked to patients

Each OMOP person becomes, or is linked to, a patient of the catalog, through
`person_source_value`. The `identifier` setting says what that column holds:

| `identifier` | `person_source_value` is | Linked to |
|---|---|---|
| `mrn` (default) | the person's MRN (of `mrn_authority`) | The imported patient with that MRN, else a new patient |
| `patient_id` | a patient ID | The imported patient with that ID, else a new patient |
| `none` | not used | Each person becomes a patient of its own, with the patient ID `<connection>:<person_id>` |

A person is linked the first time a search or a page names them. `medorama connections sync`
links every person at once.

### Worked example

1. **Create a read-only account** in the OMOP database for Medorama, and put its password in a
   file only the catalog's account can read (for example `/etc/medorama/omop-password`, mode
   600). The password is never written into the URL.

2. **Connect the database.** The following command connects an OMOP database on PostgreSQL,
   with its CDM tables in the schema `cdm` and its vocabulary in `vocab`:

   ```bash
   medorama connections add omop --kind omop \
       --url "postgres://reader@omop.example.org/cdm?sslmode=require" \
       --password-file /etc/medorama/omop-password \
       --schema cdm --vocabulary-schema vocab --mrn-authority HOSP --timezone America/New_York
   ```

   Medorama checks every view before it keeps the connection, and refuses it with the failing
   view's error. An OMOP database in an SQLite file is connected the same way:

   ```bash
   medorama connections add county --kind omop --url sqlite:/data/county/omop.sqlite --identifier mrn
   ```

3. **Check it.** The following command runs each view and reads the data's version:

   ```bash
   medorama connections check omop
   ```

4. **Optionally link every person now.** Otherwise persons are linked as searches find them:

   ```bash
   medorama connections sync omop
   ```

5. **See the data.** The **Databases** tab lists the connection; a linked patient's page shows the
   OMOP entries in their categories; the cohort criterion `omop.lactate > 4` searches that
   database alone.

In the page, an administrator selects **Import** › **Add a database**, then **Connect a database
server** (PostgreSQL) or **Connect a database file** (an SQLite file inside the data roots),
chooses the layout **OMOP CDM 5.3 or 5.4**, fills in the name, the server or file, the password,
the schemas, **Its patients are named by**, the MRN authority and the time zone, and selects
**Connect**. The **Connected databases** card then offers **Check**, **Link all** and
**Disconnect…**.

### Connection settings

| Setting (command option) | Default | Meaning |
|---|---|---|
| name | (required) | Letters, digits, `-` and `_`, starting with a letter. Cohort criteria write it before a dot: `omop.lactate`. |
| `--kind` | (required) | `omop`, or `sql` for a database read through your own views. |
| `--url` | (required) | `postgres://user@host/db?sslmode=require` without a password, or `sqlite:/path/to/file`. |
| `--password-file` | none | The file holding the password. `$MEDORAMA_SOURCE_PASSWORD_<NAME>` (the name in capitals, other characters as `_`) takes precedence. |
| `--ca-file` | none | Certificate authorities (PEM) for the server's certificate, besides the system's. |
| `--schema`, `--vocabulary-schema` | the default schema | OMOP's CDM tables, and its vocabulary when apart. |
| `--timezone` | the catalog's | The time zone of the database's times (an IANA name such as `Europe/Paris`; an unknown one is refused). When none is given, the catalog's time zone is written into the connection when it is added, and stays fixed even if the catalog's `timezone` changes later. A PostgreSQL `timestamp` (without a time zone) is read in it; a `timestamptz` is an instant. |
| `--identifier` | `mrn` | What OMOP's `person_source_value` holds: `mrn`, `patient_id` or `none`. |
| `--mrn-authority` | empty | The authority that issued the database's MRNs. |
| `--timeout-s` | 60 | Each PostgreSQL query's time limit, in seconds. |
| `--max-rows` | 1,000,000 | The rows one cohort criterion may read, and the persons a sync may read. |
| `--views` | none | Kind `sql`: the TOML file with the views. |

**Read-only access is enforced.** PostgreSQL connections are opened with
`default_transaction_read_only = on`, and SQLite files are opened read-only, so Medorama cannot
write to a connected database even with an account that could. Give it an account that can only
read anyway.

## Connect any SQL database through views

Any database that PostgreSQL or SQLite can query is connected through three **canonical views**:
SQL queries over the database's own tables that return the columns Medorama reads. The views
are the contract between Medorama and the database:

- Only the canonical columns are read, so no view can pass a field that the privacy gate does
  not know. The database's other columns stay where they are.
- The views are written in the database's own SQL dialect, and run as written: Medorama wraps
  each one as a subquery (`SELECT … FROM (your view) v WHERE person_key IN (…)`) and translates
  nothing in it (PostgreSQL's `?` operators, `E''` strings, dollar quotes and comments are
  fine). A view must be a single `SELECT` statement without a trailing semicolon.
- Columns are found by name; their order does not matter. Every column of a view must be
  present, even when it is always empty (`'' AS last_name`, `NULL AS end_time`).

### Canonical views reference

The views and their columns are defined in
[medorama/src/sources/mod.rs](../medorama/src/sources/mod.rs) (`PERSON_COLS`, `VISIT_COLS`,
`ENTRY_COLS`).

| View | Required | One row per |
|---|---|---|
| `persons` | Yes | person |
| `visits` | No | hospital stay or visit |
| `entries` | Yes | result, vital sign, drug, diagnosis, procedure, note |
| `version` | No | (one row, one value: the data's version) |

**`persons`**

| Column | Type | Content |
|---|---|---|
| `person_key` | any (compared as text) | The database's own key for the person. It never leaves the server: persons are shown by pseudonym. |
| `patient_id` | text | The patient ID that links the person to the catalog's patients, or empty. |
| `mrn` | text | The MRN that links the person, or empty. A person with neither `patient_id` nor `mrn` becomes a patient of its own, with the patient ID `<connection>:<person_key>`. |
| `mrn_authority` | text | The MRN's authority. Empty: the connection's `mrn_authority` setting. |
| `last_name`, `first_name` | text | Empty strings when the database has none or should not give them (**PHI**). |
| `birth_date` | text | `YYYY-MM-DD` (also `MM/DD/YYYY`, `YYYYMMDD`), or empty (**PHI**). |
| `sex` | text | `F`, `M`, or empty. The cohort criteria `female` and `male` match `F` and `M`. |

**`visits`**

| Column | Type | Content |
|---|---|---|
| `visit_key` | any | The database's key for the visit. |
| `person_key` | any | The visit's person (as in `persons`). |
| `start_time`, `end_time` | time ([below](#times-in-views)) | The stay. |
| `unit` | text | The ward or unit. A unit named like an intensive-care unit (ICU, MICU, CCU …) makes the stay an intensive-care stay. |
| `icu` | integer | `1` for an intensive-care stay, else `0`. A PostgreSQL boolean works too: cohort searches compare `CAST(icu AS INT) = 1` in the database. |

**`entries`**

| Column | Type | Content |
|---|---|---|
| `entry_key` | any, unique | The database's key for the entry. It never leaves the server: entries are shown by a token. When the view is a `UNION` of several tables, prefix the keys so that they stay unique (`'l' || r.id`). |
| `person_key` | any | The entry's person. |
| `visit_key` | any, or NULL | The entry's visit. |
| `category` | text | A category key, in lower case: `lab`, `vit`, `med`, `dx`, `proc`, `note`, `obs`, `micro` … ([importing.md, Categories](importing.md#categories)). Cohort searches filter the view by the key itself, and notes are recognized as `note` only, so write the key. A value that is not a category becomes `lab`, or `vit` when the entry's name or LOINC code is a vital sign's. |
| `time` | time | When the entry happened. |
| `end_time` | time, or NULL | When it ended (a drug's administration, a condition's resolution). |
| `name` | text | What the entry is: `Lactate`. Cohort criteria search by it. |
| `code`, `vocabulary` | text | `2524-7`, `LOINC`. Cohort criteria search by code prefix. |
| `value_num` | number, or NULL | A numeric result. |
| `value_text` | text | A text result (`positive`), or empty. |
| `unit` | text | The value's unit. |
| `ref_low`, `ref_high` | number, or NULL | The normal range. |
| `flag` | text | `H`, `L`, `HH`, `LL`, `A` … ([importing.md, Values, units and flags](importing.md#values-units-and-flags)). |
| `author` | text | Who wrote a note. Shown as `[Staff Name]` in Privacy Mode. |
| `text` | text | A note's free text. In Privacy Mode it is shown with the patient's own names and date of birth replaced and marked (the default), or withheld when the administrator chooses. |

**`version`** is a query that returns one value that changes when the data do (a load time, a
row count). Cohorts record it, so that a refresh can say what changed. Without it, the version is
empty.

#### Times in views

| Given as | Read as |
|---|---|
| Text in a format imports read (`2024-03-02 07:10`, `2024-03-02T07:10:00-05:00`; [importing.md](importing.md#times-and-time-zones)) | Local text in the connection's time zone; text with an offset as that moment |
| A number | Unix seconds |
| PostgreSQL `timestamp with time zone` | That moment |
| PostgreSQL `timestamp` (without time zone) | Unix seconds as if the value were UTC. Cast it to text (`CAST(t AS TEXT)`) so that it is read in the connection's time zone. |
| PostgreSQL `date` | Local midnight |

PostgreSQL columns of other types than text, numbers, booleans, dates, timestamps, UUIDs and
JSON are refused with "columns of type … are not read (cast them in the query)".

### Worked example

1. **Write the views.** This example reads a research warehouse whose tables are
   `research.patients`, `research.admissions`, `research.results`, `research.notes` and
   `research.loads`:

   ```toml
   # warehouse-views.toml
   persons = '''
     SELECT p.id AS person_key, p.study_id AS patient_id, p.mrn, 'HOSP' AS mrn_authority,
            '' AS last_name, '' AS first_name, CAST(p.dob AS TEXT) AS birth_date, p.sex
     FROM research.patients p'''
   visits = '''
     SELECT a.id AS visit_key, a.patient_id AS person_key, CAST(a.admitted AS TEXT) AS start_time,
            CAST(a.discharged AS TEXT) AS end_time, a.ward AS unit,
            CASE WHEN a.icu THEN 1 ELSE 0 END AS icu
     FROM research.admissions a'''
   entries = '''
     SELECT 'l' || r.id AS entry_key, r.patient_id AS person_key, r.admission_id AS visit_key,
            'lab' AS category, CAST(r.collected AS TEXT) AS time, NULL AS end_time, r.test AS name,
            r.loinc AS code, 'LOINC' AS vocabulary, r.value AS value_num, '' AS value_text, r.unit,
            r.low AS ref_low, r.high AS ref_high, r.flag, '' AS author, '' AS text
     FROM research.results r
     UNION ALL
     SELECT 'n' || n.id, n.patient_id, n.admission_id, 'note', CAST(n.written AS TEXT), NULL, n.kind,
            '', '', NULL, '', '', NULL, NULL, '', n.author, n.body
     FROM research.notes n'''
   version = "SELECT MAX(loaded_at) FROM research.loads"
   ```

   In a `UNION ALL`, the column names come from the first `SELECT`.

2. **Connect and check.** The following commands connect the warehouse and run each view:

   ```bash
   medorama connections add warehouse --kind sql --url "postgres://reader@dw.example.org/research" \
       --password-file /etc/medorama/dw-password --views warehouse-views.toml
   medorama connections check warehouse
   ```

   In the page, choose the layout **Any layout, through SQL views** and paste the four queries
   under **SQL views**.

3. **Make it fast.** Medorama asks the views for the persons it shows
   (`WHERE person_key IN (…)`) and, for cohort searches, for the entries with a category, a
   name, a code or a text. Index the underlying tables on the person, the time, and the name or
   code that entries are searched by.

Other engines (SQL Server, Oracle, Snowflake, BigQuery) are not read directly yet. A PostgreSQL
foreign-data wrapper can reach them in the meantime, and Medorama connects to that PostgreSQL
database.

## Build a new database for Medorama

When a study builds its database from scratch, two **lightweight databases** work without any
server: a folder of CSV files, which you import, and a single SQLite file, which you connect.
Choose by how the data will be used.

| | A folder of CSV files | An SQLite file with the canonical tables |
|---|---|---|
| Brings files (recordings, ECGs, images) | Yes | No (list the files in a CSV table beside it) |
| Updated by | Importing again (remove the earlier import first) | Writing to the file; Medorama reads it live |
| Write it with | Any spreadsheet or script | Any SQLite tool or library |
| Needs | Nothing | Nothing (PostgreSQL works the same, as a schema) |

### A folder of CSV files

Write one CSV file per kind of table, with the column names of
[importing.md, Column reference](importing.md#column-reference), and import them in this order:

```
study/
├── patients.csv       patient_id, sex, birth_date (only if needed)
├── encounters.csv     encounter_id, patient_id, admit_time, discharge_time
├── labs.csv           patient_id, encounter_id, time, name, value, unit, ref_low, ref_high, code, vocabulary
├── medications.csv    patient_id, encounter_id, time, end_time, name, value, unit
├── conditions.csv     patient_id, time, name, code, vocabulary
├── notes.csv          patient_id, time, name, text
└── recordings.csv     patient_id, time, category, file   (paths relative to this folder)
```

The file names choose the categories (`labs`, `medications`, `conditions`, `notes`). The following
loop previews and imports them:

```bash
cd study
for t in patients encounters labs medications conditions notes recordings; do
  medorama import-table "$t.csv" --dry-run && medorama import-table "$t.csv" --source study
done
```

### An SQLite file with the canonical tables

The simplest database Medorama connects to is a file whose tables are already the canonical
views. The following SQL creates one:

```sql
-- study.sqlite: a database Medorama connects to as it is
CREATE TABLE persons (
  person_key    TEXT PRIMARY KEY,        -- the database's own key
  patient_id    TEXT,                    -- the study's ID (links to imported patients)
  mrn           TEXT,                    -- or the MRN, when the study may hold it
  mrn_authority TEXT DEFAULT '',
  last_name     TEXT DEFAULT '',         -- leave the identifiers empty when not needed
  first_name    TEXT DEFAULT '',
  birth_date    TEXT DEFAULT '',         -- YYYY-MM-DD
  sex           TEXT DEFAULT ''          -- F, M
);
CREATE TABLE visits (
  visit_key  TEXT PRIMARY KEY,
  person_key TEXT NOT NULL REFERENCES persons(person_key),
  start_time TEXT, end_time TEXT,        -- 2024-03-02 07:10, or ISO 8601 with an offset
  unit       TEXT DEFAULT '',
  icu        INTEGER DEFAULT 0           -- 1: an intensive-care stay
);
CREATE TABLE entries (
  entry_key  TEXT PRIMARY KEY,
  person_key TEXT NOT NULL REFERENCES persons(person_key),
  visit_key  TEXT REFERENCES visits(visit_key),
  category   TEXT NOT NULL,              -- lab, vit, med, dx, proc, note, obs, ...
  time       TEXT NOT NULL,
  end_time   TEXT,
  name       TEXT NOT NULL,
  code       TEXT DEFAULT '', vocabulary TEXT DEFAULT '',
  value_num  REAL, value_text TEXT DEFAULT '', unit TEXT DEFAULT '',
  ref_low    REAL, ref_high REAL, flag TEXT DEFAULT '',
  author     TEXT DEFAULT '', text TEXT DEFAULT ''
);
CREATE INDEX entries_person ON entries(person_key, time);
CREATE INDEX entries_name ON entries(name);
CREATE TABLE loads (loaded_at TEXT);     -- the data's version
```

Its views only select the tables:

```toml
# study-views.toml
persons = "SELECT person_key, patient_id, mrn, mrn_authority, last_name, first_name, birth_date, sex FROM persons"
visits = "SELECT visit_key, person_key, start_time, end_time, unit, icu FROM visits"
entries = '''SELECT entry_key, person_key, visit_key, category, time, end_time, name, code, vocabulary,
  value_num, value_text, unit, ref_low, ref_high, flag, author, text FROM entries'''
version = "SELECT MAX(loaded_at) FROM loads"
```

The following commands connect the file, check it, and link its persons:

```bash
medorama connections add study --kind sql --url sqlite:/data/study/study.sqlite --views study-views.toml
medorama connections check study
medorama connections sync study
```

`connections add` answers with the views it found and the version
(`Connected study (sql): views persons, visits, entries; version 2024-03-05`), and `sync` with the
number of persons linked. From the page, the file must lie inside the data roots, and it cannot be
the catalog's own database.

### Choices to make when building one

- **Identifiers.** A study's own `patient_id` is enough to link its persons to the patients of
  other databases that use the same ID. Hold MRNs, names and dates of birth only when they are
  needed.
- **Times.** Choose one time zone for the database and give it as the connection's `timezone`,
  or store ISO 8601 times with offsets.
- **Codes.** Standard vocabularies (LOINC for results, ICD-10 for diagnoses, RxNorm for drugs)
  let cohorts search by code across databases.
- **Categories.** Store the category keys themselves (`lab`, `med`, `dx`), in lower case.
- **Files** (recordings, ECGs, images) cannot be connected. List them in a small table
  (`patient_id`, `time`, `category`, `file`) and import it.
- **Already in OMOP?** Connect it as `omop`: there are no views to write.

## Files: recordings, ECGs and images

Files always come in through an imported table that names their patients, whatever way the rest
of the data take. A file is read where it lies, inside the data roots; it is never copied or
changed.

| Files | Table columns | Category | Opened in |
|---|---|---|---|
| Bedside-monitor recordings (WFDB, `.Stp` with the stp-adapter add-on) | `patient_id` or `mrn`, `file`; or bed stays, which link recordings by bed and time | `icu` (the default for recordings) | VitalStream |
| Holter recordings | `patient_id`, `time`, `file` | `holter` | VitalStream |
| EEG and sleep studies (EDF, EDF+, BDF, BDF+, WFDB) | `patient_id`, `time`, `file` | `eeg`, `sleep` | VitalStream ([edf.md](edf.md)) |
| 12-lead ECGs (MUSE XML, WFDB, DICOM, EDF, PDF, pictures) | `patient_id` or `mrn`, `time`, `file`, measurement columns | `ecg` | The ECG viewer |
| Echo, angiography, CT, MR (DICOM files and series folders, videos) | `patient_id` or `mrn`, `time`, `file` | `echo`, `cath`, `img` | The PACS viewer |

The rules (relative paths, data roots, which files become recordings, what Privacy Mode sends)
are in [importing.md, Files](importing.md#files). The stp-adapter is a private add-on, available
to institutions under agreement ([adapters.md](adapters.md)).

## Check the result

| Way in | Check before | Check after |
|---|---|---|
| An import | `medorama import-table TABLE --dry-run`, or **Preview all** in the page: every row is read and checked, and the report says what would be added, matched, held back and ignored. Nothing is kept. | `medorama imports list`; the **Databases** tab shows the table as imported, each row with its patient and file |
| A connection | `medorama connections add` checks each view before it keeps the connection | `medorama connections check NAME` (**Check** in the page): each view answers, with its version; `medorama connections list` shows the persons linked |

After either:

- **Review** lists the rows and persons that wait for their patient, the rows held back, and the
  possible duplicates.
- A patient's page shows each entry in its column of the journey. A category that no database
  provides has no column.
- `medorama sources list` shows what each source label contributed.

## Checklist

1. Every row names its patient (`patient_id` or `mrn`), with `mrn_authority` when MRNs come from
   several systems.
2. Every dated row has a time in a readable format, and `timezone` is set for local times (in
   `medorama.toml`, or the connection's).
3. Categories are given, or the names make them clear; connected views give category keys.
4. Values are numeric where they are numbers, with their units; codes have their vocabularies.
5. Identifiers are limited to what linking needs.
6. Files are listed in an imported table, inside the data roots.
7. A dry run (imports) or a check (connections) comes before the real thing, and Review after it.

## Troubleshooting

| Message or symptom | Cause | Remedy |
|---|---|---|
| `line N: no patient_id, mrn, encounter_id or csn` | The row names no patient or encounter. | Add the identifier column, or map it. |
| `line N: not a time: "…"` | A time in a format Medorama does not read, or a number under 10^8. | Use a format of [importing.md, Times](importing.md#times-and-time-zones), or describe the format in TOML. |
| `line N: not a number: "…"` | `ref_low` or `ref_high` holds text. | Leave them empty or give numbers. |
| `line N: no encounter … (import its encounters first)` | A bed-stay row names an encounter not yet imported. | Import the encounters first. |
| `line N: …; held back for review` | The row's identifiers conflict with the catalog's (two patients, another MRN, another patient's encounter). | Open Review; apply or dismiss the row. |
| `N item(s) wait for their patient or encounter` | Entries name patients or encounters that no import has named yet. | Import the patients or encounters; the entries are matched then. |
| `line N: no such file: …`, `… is outside the configured data_roots` | The path is wrong, relative to another folder, or outside the data roots. | Give paths relative to the table's folder; extend `data_roots`. |
| `line N: FILE: not a recording Medorama reads …` | The recording's format needs an add-on that is not installed. | Install the add-on, remove the import, import the table again ([adapters.md](adapters.md#installing-an-add-on)). |
| `several files make one database: give it a name (--name)` | Several files were given to `import-table`. | Add `--name`. |
| `…: its columns are not those of …` | Files meant as one database have different columns. | Import them separately, or make their columns equal. |
| A column is not recognized | Its name is not among the recognized names. | Map it ([When the columns have other names](#when-the-columns-have-other-names)). |
| `leave the password out of the URL` | The connection URL holds a password. | Use `--password-file` or `$MEDORAMA_SOURCE_PASSWORD_<NAME>`. |
| `a connection of kind sql needs its views: persons and entries` | The views file lacks `persons` or `entries`. | Add them. |
| `NAME: the view entries: …` | A view fails in the database: a missing table or column, a trailing semicolon, a syntax error. | Run the view's SQL in the database; give every canonical column. |
| `columns of type … are not read (cast them in the query)` | A PostgreSQL column of an unsupported type. | Cast it to text or a number in the view. |
| `NAME: more than N persons: narrow the criteria` | A sync or a criterion reads more than `max_rows` rows. | Raise `--max-rows`, or let searches link the persons they find. |
| Times are off by hours | Local times were read in the wrong time zone, or a PostgreSQL `timestamp` was given uncast. | Set `timezone` (or `--timezone`); cast `timestamp` columns to text. |
| A connected database's entries are in the wrong column, or notes appear among results | The view gives categories that are not keys. | Give category keys in lower case (`lab`, `note`). |
