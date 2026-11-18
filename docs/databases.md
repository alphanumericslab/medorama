# Connecting databases to Medorama

This guide explains how a hospital's or a study's data reach Medorama, what a database must
provide, and how to connect one, from nothing to data on a patient's page. It is written for data
managers and database engineers who have not used Medorama before. Each way in ends with a worked
example and with how to check the result.

**Medorama imports nothing.** It connects databases and reads them in place, read-only, at each
request. Tables, exports and folders of recordings become a database first, with
[db-builder](../db-builder/README.md), the open tool shipped with Medorama; the column rules
db-builder follows are in [importing.md](importing.md). The design is in
[design/databases-only.md](design/databases-only.md),
[design/connected-databases.md](design/connected-databases.md) (what Medorama keeps of a
database) and [design/sources.md](design/sources.md); the settings are summarized in
[administration.md](administration.md).

## Contents

- [Terms used on this page](#terms-used-on-this-page)
- [Choose a way in](#choose-a-way-in)
- [Requirements on a connected database](#requirements-on-a-connected-database)
- [Connecting](#connecting)
  - [What Medorama keeps of a database](#what-medorama-keeps-of-a-database)
  - [Connection settings](#connection-settings)
- [Build a database with db-builder](#build-a-database-with-db-builder)
  - [When the columns have other names](#when-the-columns-have-other-names)
  - [Describe your own export in TOML](#describe-your-own-export-in-toml)
  - [Folders of recordings](#folders-of-recordings)
  - [Build it in the browser: db-builder's page](#build-it-in-the-browser-db-builders-page)
- [Connect an OMOP database](#connect-an-omop-database)
  - [What Medorama reads](#what-medorama-reads)
- [Connect any SQL database through views](#connect-any-sql-database-through-views)
  - [Canonical views reference](#canonical-views-reference)
  - [Indexes for searches](#indexes-for-searches)
  - [A MUSE ECG metadata database](#a-muse-ecg-metadata-database)
- [Files: recordings, ECGs and images](#files-recordings-ecgs-and-images)
- [Check the result](#check-the-result)
- [Checklist](#checklist)
- [Troubleshooting](#troubleshooting)

## Terms used on this page

| Term | Meaning |
|---|---|
| Connected database | A database that Medorama reads in place, read-only, at each request: an OMOP CDM instance, a research warehouse, a db-builder SQLite file. Medorama never writes to it. |
| Kind | How Medorama reads a database: `omop` (OMOP CDM, views built in), `views` (any SQL database, through views the site writes), `dbb` (a db-builder database, whose tables are the views). |
| Canonical views | SQL queries (`persons`, `visits`, `entries`, `recordings`, `stays`, a `version`, a `variables` catalog and a `has` list) that present a database in the columns Medorama reads. |
| Recording database | A connected database with recordings: each file, its patient, its start and end. The files stay where they are. |
| Issuer | Who assigned an identifier: a hospital's MRN system, a study, a registry. The pair (issuer, identifier) names one patient. |
| Entry | A dated item of a patient: a result, a vital sign, a drug, a diagnosis, a note. |
| Category | The column of the patient page's journey that an entry belongs to (`lab`, `med`, `ecg`, `sleep` …; [importing.md, Categories](importing.md#categories)). |
| Workspace | The folder where Medorama keeps what it holds of each database (its persons as linked and their encounters, the names and units of its variables, the recordings it has opened, their indexes, results), encrypted with that database's own key. |
| Data root | A folder named in the `data_roots` setting. When set, every SQLite file connected and every recording file read must lie inside one. |
| PHI | Protected health information. A user sees a database's identifiers only with the `phi` grant on it and the PHI permission, outside Privacy Mode ([security.md](security.md)). |

## Choose a way in

| Your situation | Way in | What you write |
|---|---|---|
| You have an OMOP CDM 5.3 or 5.4 database (PostgreSQL, or a SQLite file) | [Connect an OMOP database](#connect-an-omop-database) (kind `omop`) | The connection's settings |
| You have another SQL database (a warehouse, a registry) in PostgreSQL or a SQLite file, which should stay where it is | [Connect any SQL database through views](#connect-any-sql-database-through-views) (kind `views`) | SQL views in a TOML file |
| You have exported tables: CSV, Excel, OpenDocument or Parquet files from an EHR, a laboratory system, a study's spreadsheet | [Build a database with db-builder](#build-a-database-with-db-builder), then connect it (kind `dbb`) | Nothing when the column names are recognized; otherwise a mapping |
| You have an export of a known system: GE MUSE ECGs, DICOM studies, a CLIF folder, or an export your site described in a TOML file | db-builder (`add`), then connect it | Nothing, or one TOML description |
| You have folders of recordings, ECGs or images | db-builder (`recordings`, or a table with a `file` column), then connect it | A table of file and patient, or a pattern of the files' names |

The same questions as a flow:

1. **Is it an OMOP CDM database?** Connect it as `omop`. Do not export it.
2. **Is it another SQL database that must stay where it is**, because it changes daily or is too
   large to copy? Connect it through views.
3. **Is it anything else** (tables, exports, folders of files)? Build a database with db-builder
   first, then connect that file.

The ways mix. A person of one database is the same patient as a person of another when both give
the same (issuer, identifier) pair, so one patient's page can show a warehouse's laboratory
results, an OMOP instance's diagnoses and a db-builder database's recordings side by side.

## Requirements on a connected database

Every connected database must meet these six requirements. They are the site's responsibility;
Medorama checks what it can and reports the rest.

1. **Identity.** Every person has an identifier unique within its issuer, and the issuer is
   named:
   - a patient ID with its `patient_id_issuer` (a study, a hospital's system), and/or an MRN with
     its `mrn_authority`;
   - the same patient in two databases has the same (issuer, identifier) in both, or Medorama
     shows two patients. Medorama joins databases on **exact** pairs only (ignoring letter case and
     the spaces around them: `x123` is `X123`, but `00123` is not `123`): no matching by name or
     date of birth, no merge;
   - a person whose identifiers name two different patients (an MRN of one, a patient ID of
     another) is a **conflict**: Medorama lists it and links it to nobody. Merges and corrections
     are made in the database, then the database is refreshed;
   - a patient ID without an issuer (in the row and in the database's `issuer` setting) is the
     database's own: it links only that database's persons.
2. **Read-only access.** Give Medorama an account that can only `SELECT`, on the tables or views
   it reads. Medorama opens PostgreSQL sessions read-only and SQLite files read-only anyway.
3. **The canonical columns**, through OMOP, the views, or db-builder's schema
   ([Canonical views reference](#canonical-views-reference)).
4. **A version** Medorama can read, so that a change is noticed: OMOP's `cdm_source`, a
   db-builder database's `meta.version`, or a `version` query. Without one, Medorama counts the
   persons and recordings at each check (a change of entries alone is then not noticed).
5. **Encryption and backups** of the database are the owner's: PostgreSQL's storage encryption,
   disk encryption, the database's own backups. Medorama encrypts only its own workspace.
6. **Recordings**: each file named with its patient, its start and its format; the files
   readable by Medorama's account, inside `data_roots`, and never changed in place (a changed file
   is a new recording).

> **Identifiers are protected data.** Give only the identifiers needed for linking. A study that
> needs no names or dates of birth should not provide them: Medorama links by (issuer,
> identifier) pairs alone. Users without PHI access, and everyone in Privacy Mode, see
> placeholders and pseudonyms instead ([security.md](security.md)).

## Connecting

An admin connects a database in the page or on the command line. Connecting checks that each
view answers, makes the database's data key and workspace folder, grants the database to the
admin who connected it, reads its catalog (the names and units of its variables), links its
persons and keeps their encounters. It counts no entry and reads no recording: a patient's
recordings are read when the patient is opened
([What Medorama keeps of a database](#what-medorama-keeps-of-a-database)).

**In the page:** **Databases** › **Connect a database**. Fill in the name, the kind (db-builder,
OMOP, or views), where it is (a file on this server, chosen with **Choose…**, or a PostgreSQL
server), the password (kept wrapped by the master key, never shown), the schemas, what OMOP's
`person_source_value` holds, the issuer of its patient IDs, the MRN authority, the time zone and,
for views, the SQL views. Then select **Connect**. A demo installation connects only demo
databases, and a site's installation refuses them ([design/demo.md](design/demo.md)).

**On the command line**, on the server, as the account that runs Medorama:

```bash
medorama databases connect NAME --kind omop|views|dbb --url URL [options]
```

A database's page (**Databases** › its name) shows its status (a green dot online, a grey dot
offline, with when it was last checked and why), its version, **Check now**, **Refresh** (read
its persons, encounters and catalog again), **Count now**, **Index all recordings** (a recording
database), **Remove…**, its variables, who may use it (grants) and its identity conflicts.

| Task | Command |
|---|---|
| List the databases and their status | `medorama databases list [--json]` |
| Check one, or all | `medorama databases check [NAME \| --all]` |
| Read its persons, encounters and catalog again | `medorama databases refresh NAME` |
| Replace its views (kind `views`), check them and read it again; its patients keep their numbers | `medorama databases views NAME --views FILE` |
| What it holds (names and units; counts once counted) | `medorama databases variables NAME [--json]` |
| Count its variables' entries and persons (**Count now**) | `medorama databases count NAME` |
| Read and index every recording it lists (**Index all recordings**) | `medorama databases index NAME` |
| Its identity conflicts | `medorama databases conflicts NAME` |
| Who may use it | `medorama databases grants NAME`; `grant NAME USER PERMISSION`; `revoke NAME USER PERMISSION` |
| Remove it, and every trace of it in Medorama | `medorama databases remove NAME [--yes]` |

Permissions are `search` (criteria find its patients), `view` (its data on patient pages, its
recordings), `phi` (its identifiers) and `manage` (check, refresh, count, index, grant, remove; replacing
the views needs an admin who manages it); each implies the weaker ones. Removing a database destroys its key, its workspace folder and every cohort
member it contributed to; the database itself is never touched
([design/databases-only.md, Removing a database](design/databases-only.md#removing-a-database)).

### What Medorama keeps of a database

Medorama links a database's persons to patients and names what the database holds; everything
else is read from the database when it is asked for
([design/connected-databases.md](design/connected-databases.md)). Of each database it keeps, in
its workspace folder, encrypted with the database's own key:

| File | What it holds |
|---|---|
| `persons.sqlite` | The subject index: each person's identifiers and their issuers, names, date of birth and sex, the dates of their first and latest data (when the persons view gives `earliest` and `latest`), the patient number they are linked to and the keyed hashes of their pairs; their encounters (the visit's key, start, end, intensive care or not); identity conflicts. |
| `variables.sqlite` | The catalog: each variable's category, name, vocabulary, code, unit and whether it has numbers; how many entries and persons hold it, only once counted (**Count now**); the categories of its recordings; the version they were read at. |
| `recordings.sqlite` | The recordings Medorama has opened (or indexed when asked): each file's path, kind, size, fingerprint, format, times, label, category, whether it is present and why not; derived series. |
| `index.sqlite` | Their indexes. |
| `results-<action>.sqlite`, `results/` | What actions made of them. |

No entry, no count of anyone's data and no list of the database's files is kept. What is read,
and when:

| When | Read from the database |
|---|---|
| Connecting, a new version, **Refresh**, new views | Every person (however many: `--max-rows` bounds criteria, not this), every visit, the catalog (its `variables` view, else one grouped query over the entries, with no counting), and the categories of its recordings (`SELECT DISTINCT category`). No recording file is read. |
| Every check (each minute) | Whether each view answers; the version (without a version query, a count of its persons and recordings). |
| The Patient Catalog | For the page shown, what each online database holds of those patients (the **Data** and **Entries** columns); a filter on a kind of data (`has ECG`) asks each online database (its `has` view when it has one). |
| A patient's page, a job, a cohort's selection | Their entries, visits, stays and recordings. Their recordings' files are checked then, kept, and queued for indexing. |
| A cohort, a search | The criteria, sent to each database that may hold them; `has` too. A threshold asks only for the values that may pass ([performance.md, What a criterion reads](performance.md#what-a-criterion-reads)). Each row is tested as it is read and only those that hold are kept, so a criterion reads a database of millions of rows (`QTc > 450 ms` over every ECG; `latest` keeps each person's latest value). Each answer is kept while the database's version stays. |
| **Count now** (`medorama databases count NAME`) | One grouped count over the entries: how many entries and persons hold each variable, kept until the catalog is read again. Counts from 1 to 10 are shown as "< 11" (the page, the command, the API). |
| **Index all recordings** (`medorama databases index NAME`) | Every recording it lists (at most `--max-rows`): their files checked, and the new or changed ones queued for indexing. For a database of ICU recordings that should all be indexed ahead of time; `medorama init --demo` does it for the demo's databases. |

An offline database's patients stay listed, with their encounters; their data show as offline,
and the recordings kept from before are shown.

### Connection settings

| Setting (command option) | Default | Meaning |
|---|---|---|
| name | (required) | Letters, digits, `-` and `_`, starting with a letter, at most 40 characters. Cohort criteria write it before a dot: `omop.lactate`. |
| `--kind` | (required) | `omop`, `views` or `dbb`. |
| `--url` | (required) | `postgres://user@host/db?sslmode=require` without a password, or `sqlite:/path/to/file` (a `dbb` database is always a SQLite file). |
| `--password-file`, `--password-stdin` | none | The password, from a file, or from standard input (then kept wrapped by the master key). `$MEDORAMA_SOURCE_PASSWORD_<NAME>` (the name in capitals, other characters as `_`) takes precedence. |
| `--ca-file` | none | Certificate authorities (PEM) for the server's certificate, besides the system's. |
| `--views` | none | Kind `views`: the TOML file with the views (`persons`, `visits`, `entries`, `recordings`, `stays`, `version`, `variables`, `has`). |
| `--schema`, `--vocabulary-schema` | the default schema | OMOP's CDM tables, and its vocabulary when apart. |
| `--identifier` | `mrn` | OMOP: what `person_source_value` holds: `mrn`, `patient_id` or `none`. |
| `--issuer` | empty: the database itself | Who issued the database's patient IDs, for rows that name none. Empty: its patient IDs link only its own persons. |
| `--mrn-authority` | empty | The authority of the database's MRNs, for rows that name none. |
| `--timezone` | the `timezone` setting | The time zone of the database's times written without an offset (an IANA name such as `Europe/Paris`; an unknown one is refused). |
| `--files-root` | a SQLite file's folder | The folder relative recording files are read from. |
| `--timeout-s` | 60 | Each PostgreSQL query's time limit, in seconds. |
| `--max-rows` | 1,000,000 | The rows a criterion may keep (it reads as many as it needs: each is tested as it is read), and the recordings **Index all recordings** may read. The persons behind `has …` are not bounded, and connecting and refreshing read every person and visit, however many. |

## Build a database with db-builder

db-builder turns tables, known exports and folders of recordings into one SQLite file with an
open schema ([db-builder/docs/schema.md](../db-builder/docs/schema.md)), which Medorama connects
as kind `dbb`. Its commands are in [db-builder/README.md](../db-builder/README.md); the columns it
recognizes are in [importing.md, Column reference](importing.md#column-reference). The installers
build it (`.venv/bin/db-builder`).

### Worked example: patients, an encounter and laboratory results

The names and identifiers below are invented.

1. **Write the tables.** `patients.csv` names the patients by a study's ID and the hospital's
   MRN, with no names:

   ```csv
   Patient ID,MRN,DOB,Sex
   S0001,X0000001,1950-03-04,F
   S0002,X0000002,1962-11-20,M
   ```

   `encounters.csv` gives one encounter and its bed stay in an intensive-care unit:

   ```csv
   Encounter ID,Patient ID,Admit Time,Discharge Time,Admit Unit,Care Unit,Bed,Bed Start,Bed End
   E0001,S0001,2024-01-01 08:00,2024-01-09 13:00,Cardiology,ICU7,12,2024-01-01 11:00,2024-01-03 11:00
   ```

   `labs.csv` holds results, the first in its encounter, the second only with its patient, by
   MRN:

   ```csv
   MRN,Encounter ID,Test Name,Result,Units,Ref Low,Ref High,Abnormal Flag,Collected,Code,Code System
   X0000001,E0001,Lactate,4.1,mmol/L,0.5,2,H,2024-01-02 06:10,2524-7,LOINC
   X0000002,,Creatinine,1.1,mg/dL,0.6,1.3,,2024-02-02 06:10,2160-0,LOINC
   ```

2. **Build the database.** The time zone is that of the local times in the tables; the issuer and
   the authority are given for the rows that name none:

   ```bash
   db-builder new /data/study/study.sqlite --timezone America/New_York
   db-builder add /data/study/study.sqlite patients.csv encounters.csv labs.csv \
       --issuer STUDY --mrn-authority HOSP
   ```

   For each table, `add` says what it added (`patients.csv: source 1 (csv): 2 rows; 2 persons and
   0 visits added; …`) and lists the rows it left out, by file and line.

3. **Check it.** `check` lists persons without an issuer, identifiers given to two persons by the
   same issuer, rows that name no person, and missing files; `info` lists the tables and sources:

   ```bash
   db-builder check /data/study/study.sqlite
   db-builder info /data/study/study.sqlite
   ```

4. **Connect it** (the file inside `data_roots`):

   ```bash
   medorama databases connect study --kind dbb --url sqlite:/data/study/study.sqlite
   ```

   Medorama answers with what it found: `Connected study (dbb): version …; 2 person(s) linked to
   2 patient(s); … variable(s); recordings are read when a patient is opened`.

5. **See the data.** The **Databases** tab lists `study`, online; the **Patient Catalog** lists
   the two patients; the first one's page shows the stay in the ICU column and the lactate, flagged
   high, in the Labs column; the criterion `study.lactate > 4` searches that database alone.

**Updating it.** Add the newer tables to the same file: a file added again unchanged is not added
twice, and a file that changed replaces its earlier version. `db-builder remove-source` takes a
source out. Each write changes the database's version, and Medorama reads it again at its next
check (within a minute) or at **Refresh**.

### When the columns have other names

When a table's headers are not among the recognized names, map them once, in a TOML file of
`field = "header"`:

```toml
# mapping.toml
mrn = "Hosp No"
name = "Analyte"
value = "Res"
time = "Specimen taken"
```

```bash
db-builder add study.sqlite export.csv --mapping mapping.toml --mrn-authority HOSP
```

Columns that are not recognized are kept in the database's `extras` table under their own names.
Columns whose names carry a unit (`QT (ms)`, `LVEF (%)`) become searchable measurements
([importing.md, Measurement columns](importing.md#measurement-columns)).

### Describe your own export in TOML

A site describes an export it receives regularly, column by column, in a TOML file given to
db-builder with `--formats`; no code is needed. The form of the file is in
[importing.md, Known exports and declared formats](importing.md#known-exports-and-declared-formats).

### Folders of recordings

`db-builder recordings` adds the recordings of a folder (WFDB records, EDF and BDF files, files an
installed adapter reads), with their patients from a table or from a pattern of their names:

```bash
db-builder recordings /data/icu/icu.sqlite /data/icu --patients icu_patients.csv --issuer HOSP
db-builder recordings /data/psg/psg.sqlite /data/psg --pattern '{patient}_*.edf' --issuer SLEEP-STUDY --category sleep
medorama databases connect icu --kind dbb --url sqlite:/data/icu/icu.sqlite
```

A recording's start and end come from its header (a WFDB or EDF header's clock time is read as
UTC, as the viewers read it). A recording that neither the table nor the pattern names has no
person, and `db-builder check` lists it ([db-builder/README.md, Folders of
recordings](../db-builder/README.md#folders-of-recordings)).

### Build it in the browser: db-builder's page

db-builder has a web page of its own, for the same work without the command line: `db-builder
serve --root DIR` runs it on its own port (8091), with its own sign-in, building databases in
that one folder and reading nothing above it ([db-builder/README.md, Web
interface](../db-builder/README.md#web-interface)). When an administrator sets Medorama's
`db_builder_root` ([administration.md, db-builder](administration.md#db-builder)), Medorama runs
it, and the **Databases** tab's **Create a database…** (admins) opens its page in a tab of its
own, beside Medorama's. There:

1. **Sign in** with db-builder's own account (not Medorama's).
2. **New…** makes `study.sqlite` in the root, in the time zone of its local times.
3. **Import**: choose the tables (and known exports, CLIF folders) in the root, the issuer and
   MRN authority, then **Preview**: for each input, the format recognised, how each column is read
   (a field, a measurement, `extras`), its first rows, what would be added and the warnings. The
   preview writes nothing. Correct what it shows (a mapping file, the rules), then **Add**.
4. **Recordings**: a folder of recordings with a patients table or a pattern, previewed then added.
5. **Rules** keeps the database's rewrite rules beside it, applied to every add from the page;
   **Check** lists what a reader cannot rely on; **Tables** shows its rows.
6. Back in Medorama, **Connect a database** with the file's path (inside `data_roots`).

> **Encryption.** db-builder can write an encrypted database (`--encrypt`, SQLCipher, a raw key
> of 64 hex digits). Medorama connects it with its key, given as the database's password: the
> form's **Key, if encrypted**, or `--password-stdin` (also `--password-file`, or
> `$MEDORAMA_SOURCE_PASSWORD_<NAME>`). A key given to Medorama is kept sealed by its master key.
> A file without its key, or with a wrong one, is refused when connected, and offline later.

## Connect an OMOP database

An OMOP CDM database is read in place through views that Medorama has built in; you write no
SQL. Medorama supports the OMOP CDM versions 5.3 and 5.4, in PostgreSQL (on-site, Amazon RDS
and Aurora, Azure Database for PostgreSQL, or any server that speaks its protocol) or in a SQLite
file.

### What Medorama reads

Every one of these tables must exist, even when empty: a view that names a missing table fails,
and the database is refused. The views are in
[medorama/src/sources/omop.rs](../medorama/src/sources/omop.rs).

| OMOP table | Read as | Columns read |
|---|---|---|
| `person` | Persons | `person_id` (the key); `person_source_value` (the MRN or patient ID, by the `identifier` setting); `gender_concept_id` (8532 female, 8507 male); `birth_datetime`, else `year_of_birth`, `month_of_birth`, `day_of_birth` (a year alone is read as 1 July, a year and month as the 15th: ages within six months). Names are not read: OMOP holds none. |
| `visit_occurrence` | Visits | `visit_occurrence_id`, `person_id`, `visit_start_datetime` (else `visit_start_date`), `visit_end_datetime` (else `visit_end_date`), `care_site_id`, `visit_concept_id` |
| `visit_detail` | Intensive-care visits, and the visits' stays | A visit is an intensive-care stay when one of its details has `visit_detail_concept_id` 32037, or when its unit's name is that of an intensive-care unit. Each detail is a stay (`visit_detail_id`, `visit_occurrence_id`, `visit_detail_start_datetime` else `_date`, `visit_detail_end_datetime` else `_date`, `care_site_id`'s name else the concept's name), in intensive care by concept 32037 or by its unit's name: the journey's ICU column draws those stays rather than the whole visit. |
| `care_site` | The visit's unit | `care_site_name` (else the visit concept's name) |
| `measurement` | Entries, category `lab` (`vit` for vital signs, by LOINC code or name) | The concept's name and code (else `measurement_source_value`), `value_as_number` (else the value concept's name, or `value_source_value`, as text), `operator_concept_id` (an operator other than `=` makes the value text: `<` and 5 is `<5`, not the number 5), the unit concept's code (UCUM; else `unit_source_value`), `range_low`, `range_high`, `measurement_datetime` (else `measurement_date`) |
| `observation` | Entries, category `obs` | The concept's name and code, `value_as_number` (else `value_as_string` or the value concept's name), the unit, the time |
| `condition_occurrence` | Entries, category `dx` | The concept's name; the source concept's code and vocabulary (ICD-10-CM), else `condition_source_value`; start and end |
| `drug_exposure` | Entries, category `med` | The concept's name and code, `dose_unit_source_value`, start and end |
| `procedure_occurrence` | Entries, category `proc` | The concept's name and code, the time |
| `note` | Entries, category `note` | `note_title` (else the note type's name), `note_text`, the time; the author from `provider` |
| `provider` | A note's author | `provider_name` |
| `concept` | Names and codes | In the vocabulary schema (`vocabulary_schema`, when apart from the CDM's) |
| `cdm_source` | The data's version | `source_release_date` and `cdm_release_date` |

Concept 0 ("No matching concept") names nothing: an entry whose concept is 0 is named by its
source value (`measurement_source_value`, `condition_source_value` …), and a unit of concept 0
by `unit_source_value`.

Times are read as the database's local times, in its time zone. On SQLite, where OMOP's tools
may store times as Unix seconds, a date and time stored so is that moment (Unix seconds are
UTC), and a date stored so is that date (local midnight). OMOP holds no recordings.

### How persons are linked to patients

The `identifier` setting says what `person_source_value` holds, and so which pair each person
gives:

| `identifier` | `person_source_value` is | The pair |
|---|---|---|
| `mrn` (default) | the person's MRN | (`mrn:<mrn_authority>`, the MRN) |
| `patient_id` | a patient ID | (the `issuer` setting, else the database itself, the ID) |
| `none` | not used | (the database, `person_id`): each person is a patient of its own |

Every person is linked when the database is connected and at each refresh.

### Worked example

1. **Create a read-only account** in the OMOP database for Medorama.

2. **Connect the database.** The following command connects an OMOP database on PostgreSQL, with
   its CDM tables in the schema `cdm` and its vocabulary in `vocab`, and reads the password from
   standard input (it is then kept wrapped, never in a file):

   ```bash
   medorama databases connect omop --kind omop \
       --url "postgres://reader@omop.example.org/cdm?sslmode=require" --password-stdin \
       --schema cdm --vocabulary-schema vocab --mrn-authority HOSP --timezone America/New_York
   ```

   Medorama checks every view before it keeps the database, and refuses it with the failing
   view's error. An OMOP database in a SQLite file is connected the same way:

   ```bash
   medorama databases connect county --kind omop --url sqlite:/data/county/omop.sqlite --mrn-authority COUNTY
   ```

3. **Check it** (each view, and the version):

   ```bash
   medorama databases check omop
   ```

4. **See the data.** The **Databases** tab lists `omop`, online; a linked patient's page shows
   the OMOP entries in their categories; the criterion `omop.lactate > 4` searches that database
   alone. With `--mrn-authority HOSP`, an OMOP person with the MRN of a db-builder person whose
   `mrn_authority` is `HOSP` is the same patient.

## Connect any SQL database through views

Any database that PostgreSQL or SQLite can query is connected through **canonical views**: SQL
queries over the database's own tables that return the columns Medorama reads. The views are the
contract between Medorama and the database:

- Only the canonical columns are read, so no view can pass a field that the privacy gate does not
  know. The database's other columns stay where they are.
- The views are written in the database's own SQL dialect, and run as written: Medorama wraps
  each one as a subquery (`SELECT … FROM (your view) v WHERE person_key IN (…)`) and translates
  nothing in it (PostgreSQL's `?` operators, `E''` strings, dollar quotes and comments are
  fine). A view must be a single `SELECT` statement without a trailing semicolon.
- Columns are found by name; their order does not matter. Every column of a view must be
  present, even when it is always empty (`'' AS last_name`, `NULL AS end_time`).

### Canonical views reference

The views and their columns are defined in
[medorama/src/sources/mod.rs](../medorama/src/sources/mod.rs) (`PERSON_COLS`, `VISIT_COLS`,
`ENTRY_COLS`, `RECORDING_COLS`, `STAY_COLS`, `VARIABLE_COLS`). A db-builder database's tables
have the same columns.

| View | Required | One row per |
|---|---|---|
| `persons` | Yes | person |
| `visits` | No | hospital stay or visit |
| `entries` | Yes | result, vital sign, drug, diagnosis, procedure, note |
| `recordings` | No: a recording database | file (a recording, a 12-lead ECG, a picture, a PDF, a folder of DICOM) |
| `stays` | No | stay in a unit during a visit (a bed stay, a transfer) |
| `version` | No | (one row, one value: the data's version) |
| `variables` | No | variable the database holds (its catalog: names and units) |
| `has` | No | person and category of data they have |

A recording database read through views still gives an `entries` view; it may return no rows
(`SELECT … WHERE 1 = 0`).

**`persons`**

| Column | Type | Content |
|---|---|---|
| `person_key` | any (compared as text) | The database's own key for the person. It never leaves the server: persons are shown by pseudonym. |
| `patient_id` | text | The person's patient ID, or empty. |
| `patient_id_issuer` | text | Who issued `patient_id`, or empty: the database's `issuer` setting, else the database itself (its IDs then link only its own persons). |
| `mrn` | text | The person's MRN, or empty. |
| `mrn_authority` | text | The MRN's authority. Empty: the database's `mrn_authority` setting. |
| `last_name`, `first_name` | text | Empty strings when the database has none or should not give them (**PHI**). |
| `birth_date` | text | `YYYY-MM-DD` (also `MM/DD/YYYY`, `YYYYMMDD`), or empty (**PHI**). |
| `sex` | text | `F`, `M`, or empty. The cohort criteria `female` and `male` match `F` and `M`. |
| `earliest`, `latest` | time | Optional: the times of the person's first and latest data (an ECG database's `MIN` and `MAX` of its acquisition times). Kept in the subject index; the Patient Catalog is sorted by the latest (with the encounters'), so a database without visits lists its patients newest first. |

A person with neither `patient_id` nor `mrn` is a patient of its own, linked to no other
database's persons.

**`visits`**

| Column | Type | Content |
|---|---|---|
| `visit_key` | any | The database's key for the visit. |
| `person_key` | any | The visit's person (as in `persons`). |
| `start_time`, `end_time` | time ([below](#times-in-views)) | The stay. |
| `unit` | text | The ward or unit. A unit named like an intensive-care unit (ICU, MICU, CCU …) makes the stay an intensive-care stay. |
| `icu` | integer | `1` for an intensive-care stay, else `0`. A PostgreSQL boolean works too (cohort searches read `CAST(icu AS INT)`). `has icu`, the encounter criterion and the patient's page all count a visit as an intensive-care stay when `icu` is 1 or its `unit` is named like an intensive-care unit (not a step-down, post-ICU or intermediate-care unit: `ICU step-down`, `IMCU`). |

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

**`recordings`**

| Column | Type | Content |
|---|---|---|
| `recording_key` | any, unique | The database's key for the recording. It never leaves the server: recordings are shown by number. A recording keyed with an entry's `entry_key` followed by `:file` is that entry's file (a 12-lead ECG, a picture, a PDF): the patient's page shows it with the entry, not on its own (db-builder writes its files so). |
| `person_key` | any, or NULL | The recording's person. |
| `visit_key` | any, or NULL | Its visit. |
| `file` | text | The file or folder: absolute, or relative to the `files_root` setting (else to a SQLite database's own folder). A WFDB record is its header (`.hea`); a WFDB collection its folder. It must lie inside `data_roots`. A file no reader indexes (a picture, a PDF, an ECG in DICOM or XML) has the status `file`: shown, never indexed. |
| `format` | text | The format as the database names it (`WFDB record`, `EDF+` …), shown until indexing reads the recording's own (and again when the file changes). Medorama reads each file's kind itself. |
| `start_time`, `end_time` | time, or NULL | When it was recorded. |
| `label` | text | Its name on the page (else the file's name). |
| `category` | text | `icu`, `holter`, `eeg`, `sleep` (opened in VitalStream), or another category key (`ecg`, `img`, `echo`, `cath` …: attached for the ECG and PACS viewers). Empty: `icu`. |
| `size` | integer, or NULL | The file's size in bytes. |
| `fingerprint` | text | The database's fingerprint of the file, or empty. Medorama computes its own. |

**`stays`** (optional) lists a visit's stays in its units, so that the journey's ICU column
draws the time spent in intensive care rather than the whole visit, and `has icu` finds a stay
in an intensive-care unit of a visit that is not marked as one. Without it, an intensive-care
visit is drawn whole. A db-builder database gives its bed stays (kept in `extras`), and OMOP its
`visit_detail`, without a view.

| Column | Type | Content |
|---|---|---|
| `stay_key` | any | The database's key for the stay. |
| `person_key` | any | The stay's person (as in `persons`). |
| `visit_key` | any, or NULL | Its visit (as in `visits`). A stay of no visit given is drawn on its own. |
| `start_time`, `end_time` | time | The stay. A stay without an end lasts until its visit's end. |
| `unit` | text | The unit or ward. A unit named like an intensive-care unit makes it an intensive-care stay. |
| `icu` | integer | `1` for a stay in intensive care, else `0`. |

**`version`** is a query that returns one value that changes when the data do (a load time, a
row count). Medorama reads it at each check: a new value reads the persons, their encounters
and the catalog again, and cohorts record it. A search keeps each criterion's answer while the
version stays ([performance.md, Answers kept](performance.md#answers-kept)), so the version must
change whenever the data do: data changed under the same version are not read again. Without it, Medorama counts the persons and the
recordings (not the entries, which on a large database would read them all at every check).

**`variables`** (optional) names what the database holds, one row per variable, so that
connecting and refreshing read no entry for the catalog. The catalog is what the cohort builder
completes and the planner sends criteria by. Without it, Medorama reads the names and units with
one grouped query over the `entries` view (`GROUP BY category, name, vocabulary, code, unit`),
with no counting. A database whose variables are its columns (a table of measurements) writes it
as a list of constants; an OMOP-like warehouse may write it over its own summary tables.

| Column | Type | Content |
|---|---|---|
| `category` | text | The category key, as in `entries`. |
| `name` | text | The variable's name, exactly as the `entries` view gives it (`QTc`). |
| `vocabulary`, `code` | text | As in `entries`, or empty. |
| `unit` | text | The unit its values are in, as in `entries`, or empty. |
| `numeric` | integer | `1` when its entries have numeric values (`value_num`), else `0`. |

A variable named here but absent from `entries` is offered by the cohort builder and finds
nobody; a variable of `entries` not named here is not offered, and the planner does not send a
criterion on it to this database. Keep the two in step.

**`has`** (optional) says who has data of each category, one row per person and category, so
that `has ecg` (a cohort's criterion, and the Patient Catalog's filter by kind of data) reads
one small view instead of every entry and recording. Without it, Medorama reads the persons with
entries of that category and with recordings of it. Visits are still read from `visits` and
`stays` for `enc` and `icu`.

| Column | Type | Content |
|---|---|---|
| `person_key` | any | The person (as in `persons`). |
| `category` | text | A Medorama category key, in lower case: `ecg`, `lab`, `vit`, `dx`, `med`, `icu` … ([importing.md, Categories](importing.md#categories)). |

Like `variables`, it must agree with `entries` and `recordings`: a person it leaves out is not
found by `has`.

#### Times in views

| Given as | Read as |
|---|---|
| Text in a format db-builder reads (`2024-03-02 07:10`, `2024-03-02T07:10:00-05:00`; [importing.md](importing.md#times-and-time-zones)) | Local text in the database's time zone; text with an offset as that moment |
| A number | Unix seconds |
| PostgreSQL `timestamp with time zone` | That moment; cast to text (`2024-03-02 07:10:00-05`, an offset of hours alone, or `+0530`), that moment too |
| PostgreSQL `timestamp` (without time zone) | Its clock time, read in the database's time zone, as local text is (cast or not) |
| PostgreSQL `date` | Local midnight |

Medorama's sessions set `DateStyle` to `ISO, YMD` and `IntervalStyle` to `iso_8601`, so that
`CAST(d AS TEXT)` writes `2024-03-02` whatever the account's own settings are (a `DateStyle` set
in the URL's `options` is kept).

PostgreSQL columns of other types than text, numbers, booleans, dates, timestamps, UUIDs and
JSON are refused with "columns of type … are not read (cast them in the query)".

### Worked example

1. **Write the views.** This example reads a research warehouse whose tables are
   `research.patients`, `research.admissions`, `research.results`, `research.notes` and
   `research.loads`. Its patients are named by the study's ID, issued by `STUDY-A`:

   ```toml
   # warehouse-views.toml
   persons = '''
     SELECT p.id AS person_key, p.study_id AS patient_id, 'STUDY-A' AS patient_id_issuer,
            p.mrn, 'HOSP' AS mrn_authority, '' AS last_name, '' AS first_name,
            CAST(p.dob AS TEXT) AS birth_date, p.sex
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

   In a `UNION ALL`, the column names come from the first `SELECT`. A warehouse that lists
   recordings adds a `recordings` view with the columns above.

2. **Connect and check:**

   ```bash
   medorama databases connect warehouse --kind views --url "postgres://reader@dw.example.org/research" \
       --password-file /etc/medorama/dw-password --views warehouse-views.toml
   medorama databases check warehouse
   ```

   In the page, choose the kind **views** and paste the queries under **SQL views**.

3. **Make it fast.** Medorama asks the views for the persons it shows (`WHERE person_key IN
   (…)`) and, for cohort searches, for the entries with a category, a name, a code, a text or a
   value. Index the underlying tables as [Indexes for searches](#indexes-for-searches) says.

Other engines (SQL Server, Oracle, Snowflake, BigQuery) are not read directly yet. A PostgreSQL
foreign-data wrapper can reach them in the meantime, and Medorama connects to that PostgreSQL
database.

### Indexes for searches

A search is as fast as the database answers what each criterion asks
([performance.md, What a criterion reads](performance.md#what-a-criterion-reads)). The views run
as written, inside Medorama's queries, so the indexes that help are those of the tables under
them:

| Medorama asks | An index that serves it |
|---|---|
| A patient's data: `WHERE person_key IN (…)` | Each table under a view, on its patient key; on its time too. |
| A threshold: `WHERE name IN (…) AND COALESCE(unit, '') = 'ms' AND value_num >= …`, reading `person_key, time, name, value_num, unit` | A table of one row per result: on (name, value), or (name, unit, value). A table of one column per measurement (an ECG's QTc): on (that column, the patient key, the time), for each column searched. |
| `has ecg` | A `has` view over a table indexed on the patient key (`SELECT DISTINCT …` reads the index alone), or an index on (category, patient key). |
| A diagnosis by code, a drug by name | On the code, on the name. |

An index that holds every column a query reads (a *covering* index) answers it alone, without
reading the table: with an index on `(qtc_ms, patient_id, acquisition_datetime)`, `QTc > 450`
is a range search of the index. Create the indexes, then let the database update its statistics
(`ANALYZE`, on SQLite and PostgreSQL). A db-builder database has its indexes already
([db-builder/docs/schema.md, Indexes](../db-builder/docs/schema.md#indexes)).

### A MUSE ECG metadata database

An ECG metadata database made from a GE MUSE export's XML files is a SQLite file with a table
`ecg`, one row per distinct XML file (by its MD5), and a table `additions`, one row per batch
merged into it. Medorama connects it as it is, through views; nothing is converted or copied.
The views read these columns of `ecg`:

| Columns | What they hold |
|---|---|
| `md5` | The file's MD5: the row's key. |
| `patient_id`, `last_name`, `first_name`, `birth_date`, `sex` (`female`, `male`) | The patient as the file gives it. |
| `acquisition_datetime` | The acquisition time (ISO 8601, local time). |
| `ventricular_rate_bpm`, `atrial_rate_bpm`, `pr_interval_ms`, `qrs_duration_ms`, `qt_interval_ms`, `qtc_ms`, `qtc_fridericia_ms`, `global_rr_ms`, `p_axis_deg`, `r_axis_deg`, `t_axis_deg`, `systolic_bp_mmhg`, `diastolic_bp_mmhg` | The final (overread) measurements, in their units. |
| `diagnosis_text`, `original_diagnosis_text` | The confirmed statements, and the computer's. |
| `source_file` | The XML file's path, inside a data root. |

and `additions.added_at`, the time of each batch. The views are the demo's `muse/views.toml`
(written by [tools/make-demo-data.py](../tools/make-demo-data.py)); copy it, change the form of the MRNs it reads ([below](#the-mrns-it-reads)), and the
column names if a database names them otherwise:

```bash
medorama databases connect muse --kind views --url sqlite:/data/ecg/ecg_metadata.sqlite \
    --views muse-views.toml --mrn-authority HOSP --timezone America/New_York
```

| View | What it gives |
|---|---|
| `persons` | One person per MUSE patient ID, read as the MRN (of the `--mrn-authority`); the names, date of birth and sex of their latest ECG (`MAX` the one aggregate, so that SQLite reads them from its row; the first ECG's time in a subquery); the times of their first and latest ECG (`earliest`, `latest`). Only MRNs of the site's form ([below](#the-mrns-it-reads)). |
| `entries` | Per ECG, an `ecg` entry named `12-lead ECG`, its text the confirmed statements (else the computer's); and one `ecg` entry per measurement of the final (overread) set: Vent. rate, Atrial rate, PR, QRS, QT, QTc, QTcF, RR, the P, R and T axes and the blood pressure, in their units, for cohorts (`muse.QTc > 450 ms`). |
| `recordings` | Per ECG, its XML file, keyed as the ECG entry's file: the patient's page shows it with the entry, and it opens in the ECG viewer with its median beats ([viewers/ecg](../viewers/ecg/README.md#muse-xml)). |
| `version` | The number of tables merged into it, and when the last was. |
| `variables` | Its catalog, as constants: `12-lead ECG` and the measurements above, each with its unit, so that connecting reads no ECG for it. |
| `has` | Who has an ECG, so that `has ecg` reads one column of `ecg` (over the index on `patient_id`), not every measurement: `SELECT DISTINCT patient_id AS person_key, 'ecg' AS category FROM ecg WHERE patient_id <> '' AND patient_id NOT GLOB '*[^0-9]*'` (the same `WHERE` as the other views). |

- **Its indexes.** Indexes on `ecg(patient_id)` and `ecg(acquisition_datetime)` keep each
  patient's page to an index search. Create them once on the file, if it lacks them:
  `CREATE INDEX IF NOT EXISTS ecg_patient ON ecg (patient_id); CREATE INDEX IF NOT EXISTS
  ecg_time ON ecg (acquisition_datetime); ANALYZE;`. For cohorts, one covering index per
  measurement searched makes a threshold a search of the index
  ([Indexes for searches](#indexes-for-searches)): `CREATE INDEX IF NOT EXISTS ecg_qtc_ms ON
  ecg (qtc_ms, patient_id, acquisition_datetime);`, and the same for `qt_interval_ms`,
  `ventricular_rate_bpm` and the others, then `ANALYZE;`.
- **Its size.** Connecting a database of millions of ECGs reads its persons (one `GROUP BY
  patient_id`, over the index), its `variables` view (no ECG read) and the categories of its
  recordings (one scan): minutes. No XML file is read until a patient is opened; then only that
  patient's few. A criterion such as `muse.QTc > 450 ms` reads the QTc arm of the `entries` view,
  only the values over 450 when the column has its index; `has ecg` reads the `has` view
  ([performance.md, A MUSE-shaped database](performance.md#a-muse-shaped-database)).
  **Index all recordings** is not needed: its files are ECGs, opened in the ECG viewer, not
  indexed.
- **The demo** holds one: `demo_muse` (`muse/ecg_metadata.sqlite`, with the same views in
  `muse/views.toml`).

#### The MRNs it reads

A patient ID typed at the cart is not always an MRN: a name, or `test`. Such an ID links no one,
so the views leave it out: every view reads only the patient IDs of the site's form. For MRNs of
digits only:

```sql
SELECT patient_id AS person_key, '' AS patient_id, '' AS patient_id_issuer,
       patient_id AS mrn, '' AS mrn_authority,
       COALESCE(last_name, '') AS last_name, COALESCE(first_name, '') AS first_name,
       COALESCE(birth_date, '') AS birth_date,
       CASE sex WHEN 'female' THEN 'F' WHEN 'male' THEN 'M' ELSE '' END AS sex,
       (SELECT MIN(e.acquisition_datetime) FROM ecg e WHERE e.patient_id = ecg.patient_id) AS earliest,
       MAX(acquisition_datetime) AS latest
FROM ecg WHERE patient_id NOT GLOB '*[^0-9]*'
GROUP BY patient_id
```

The `entries` and `recordings` views take the same `WHERE`. The demo's views read its invented
MRNs (`patient_id GLOB 'SYN[0-9]*'`), and leave out its one test ECG, whose patient ID is a word.
To change the views of a database already connected, without connecting it again:
`medorama databases views muse --views muse-views.toml` (its patients keep their numbers).

## Files: recordings, ECGs and images

Files always come in through a recording database: db-builder's `recordings` table (from a folder,
or from any table row with a `file` column), or a `recordings` view. A file is read where it lies,
inside the data roots; it is never copied or changed.

| Files | Category | Opened in |
|---|---|---|
| Bedside-monitor recordings (WFDB, `.Stp` with the stp-adapter add-on) | `icu` (the default for recordings) | VitalStream |
| Holter recordings | `holter` | VitalStream |
| EEG and sleep studies (EDF, EDF+, BDF, BDF+, WFDB) | `eeg`, `sleep` | VitalStream ([edf.md](edf.md)) |
| 12-lead ECGs (MUSE XML, WFDB, DICOM, EDF, PDF, pictures) | `ecg` | The ECG viewer |
| Echo, angiography, CT, MR (DICOM files and series folders, videos) | `echo`, `cath`, `img` | The PACS viewer |

A recording is numbered `database × 1,000,000,000 + n` across Medorama (the patient page, the
jobs, the viewer). Indexes of recordings are written into the database's index store in the
workspace (`db-NNNN/index.sqlite`), encrypted with the database's key. The file rules
(relative paths, data roots, which files Privacy Mode sends) are in
[importing.md, Files](importing.md#files). The stp-adapter is a private add-on, available to
institutions under agreement ([adapters.md](adapters.md)).

## Check the result

| When | What to check |
|---|---|
| Before connecting (db-builder) | `db-builder check FILE` reports what Medorama cannot rely on; `db-builder info FILE` lists the tables, rows and sources. |
| Connecting | `medorama databases connect` checks each view before it keeps the database, and says how many persons it linked to how many patients, the variables, and any identity conflicts. |
| After | `medorama databases check NAME` (**Check now**): online, with its version. `medorama databases variables NAME`: what it holds (`medorama databases count NAME` first, for its counts). `medorama databases conflicts NAME`: persons not linked. `medorama recordings`: each recording opened or indexed, present or with its problem (`medorama databases index NAME` reads them all). |

In the page, the **Databases** tab shows the database online (a green dot), its variables and
its conflicts; the **Recordings** view lists the recordings opened or indexed, with their status; the **Patient
Catalog** lists its patients; a patient's page shows each entry in its column of the journey.

## Checklist

1. Every person names an identifier and its issuer (`patient_id_issuer`, `mrn_authority`, or the
   database's `issuer` and `mrn_authority` settings), the same as in the other databases that hold
   the same patient.
2. Every dated row has a time in a readable format, and the time zone is set for local times
   (db-builder's `--timezone`, the database's `--timezone`).
3. Categories are category keys (`lab`, `note`) in views; given or clear from names in tables.
4. Values are numeric where they are numbers, with their units; codes have their vocabularies.
5. Identifiers are limited to what linking needs.
6. Files are listed in a recording database, inside the data roots.
7. Medorama's account can only read the database.
8. `db-builder check` (or a view check) comes before connecting, and the conflicts list after.

## Troubleshooting

| Message or symptom | Cause | Remedy |
|---|---|---|
| `leave the password out of the URL: give it apart (the page, --password-file)` | The URL holds a password. | Use `--password-stdin`, `--password-file` or `$MEDORAMA_SOURCE_PASSWORD_<NAME>`. |
| `a database of kind views needs its views: persons and entries (visits, recordings if any)` | The views file lacks `persons` or `entries`. | Add them (an `entries` view may return no rows). |
| `a db-builder database is a SQLite file: sqlite:/path/to/file.sqlite` | A `dbb` database given by another URL. | Give `sqlite:` and the file's path. |
| `no database file …`, `the file is outside the folders data may come from (data_roots)` | The SQLite file is not there, or outside the data roots. | Check the path; extend `data_roots`. |
| `that is Medorama's own control store`, `that file is in Medorama's workspace` | The file is Medorama's own. | Connect a database file kept elsewhere. |
| `a database named NAME is connected` | The name is taken (names are compared ignoring case). | Choose another name. |
| `this is a demo installation: it connects demo databases (invented patients) only, and NAME is not one …` | A database of real patients connected to a demo installation. | Connect it to a site's installation (`medorama init`, without `--demo`; [design/demo.md](design/demo.md)). |
| `NAME is a demo database (invented patients): connect it to a demo installation …` | A demo database (its `medorama_demo` table) connected to a site's installation. | Connect it to a demo installation, with its own home (`medorama --home ~/medorama-demo init --workspace ~/medorama-demo-workspace --demo`). |
| `demo databases (invented patients) are connected to this site's installation: …` | A control store from before 0.3.1 connects both kinds; every command but listing and removing databases is refused. | Remove the demo's databases it names (`medorama databases remove NAME`), and run the demo as its own installation. |
| `NAME: the view entries: …` | A view fails in the database: a missing table or column, a trailing semicolon, a syntax error. | Run the view's SQL in the database; give every canonical column (including `patient_id_issuer`). |
| `NAME: the version: …` | The version query fails. | Fix it, or leave it out. |
| `columns of type … are not read (cast them in the query)` | A PostgreSQL column of an unsupported type. | Cast it to text or a number in the view. |
| `NAME is connected, but reading it failed (refresh it)` | The views answer, but reading the persons, visits or catalog failed. | Fix the cause it gives, then **Refresh** (`medorama databases refresh NAME`). |
| `NAME: more than N persons: narrow the criteria` | A search keeps more persons (or visits) than `max_rows`. | Narrow the criteria, or raise `--max-rows`. |
| `NAME: more than N recordings` | **Index all recordings** reads more recordings than `max_rows`. | Raise `--max-rows`, or let the recordings be indexed as patients are opened. |
| `NAME: the view variables: …` | The `variables` view fails, or lacks a column. | Give it the columns `category, name, vocabulary, code, unit, numeric`, or leave it out. |
| `NAME: the view has: …` | The `has` view fails, or lacks a column. | Give it the columns `person_key, category`, or leave it out. |
| `NAME: the criterion finds more than N rows: narrow it …` | More than `max_rows` rows hold a criterion (rows that do not hold are not kept, however many are read). | Narrow it (a date range, a database), or raise `--max-rows`. |
| A grey dot: `no such SQLite file`, or a connection error | The database is offline. | Bring it back; Medorama checks every minute, or select **Check now**. |
| Identity conflicts listed | A person's identifiers name two patients. | Correct the identifiers in the database, then refresh. |
| One patient appears twice | The two databases name the patient by different pairs (another issuer, a missing authority). | Give both databases the same issuer and identifier. |
| A recording listed with `no such file or folder`, `outside the folders data may come from (data_roots)` or `inside Medorama's workspace` | The file is missing, outside the data roots, or in the workspace. | Fix the path (or `--files-root`), or extend `data_roots`. |
| Every recording is `missing` after the data were moved or mounted elsewhere (`/labs/data` now `/mnt/data`) | The database lists the files' old paths. | A db-builder database: `db-builder rewrite DB --rules moved.toml` with a `[[path]]` rule ([db-builder/README.md, Rewrite rules](../db-builder/README.md#rewrite-rules)); paths stored relative to the database's folder move with it. Views: change the view, or set `files_root` for relative paths. The recordings are read again when their patients are opened, or all at once with **Index all recordings**. |
| A recording is not read | Its format needs an add-on that is not installed. | Install the add-on ([adapters.md](adapters.md#installing-an-add-on)), then **Index all recordings** (`medorama databases index NAME`). |
| A variable's entries and persons show "—" (or "-") | The catalog is not counted: connecting names the variables without counting them. | **Count now** (`medorama databases count NAME`). |
| Times are off by hours | Local times were read in the wrong time zone, or a view gives Unix seconds of local clock times (Unix seconds are UTC). | Set the time zone (`--timezone`); give local times as text. |
| Entries are in the wrong column, or notes appear among results | The view gives categories that are not keys. | Give category keys in lower case (`lab`, `note`). |
| `…: file is not a database`, or `cannot be read with this key` | The SQLite file is encrypted (db-builder's `--encrypt`), and no key or a wrong one was given. | Connect it with its key as the password ([Build a database with db-builder](#build-a-database-with-db-builder), "Encryption"). |
