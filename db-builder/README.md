# db-builder

db-builder turns tables, known exports and folders of recordings into one SQLite database with
an open schema ([docs/schema.md](docs/schema.md)). The database is not specific to Medorama: any
SQLite program, or any language with an SQLite library, reads it. Medorama connects it as it
connects any other database, read-only and in place.

| Input | Becomes |
|---|---|
| Tables: CSV (UTF-8, comma-separated), Excel (`.xlsx`, `.xls`), OpenDocument (`.ods`), Parquet | `persons`, `visits`, `entries`, with the columns recognised as [docs/importing.md](../docs/importing.md) says; every other column in `extras`, as it came |
| GE MUSE ECG metadata (one row per ECG, the XML elements as columns) | The same tables: the patient, the ECG as an entry with its file, its measurements, its statements |
| A CLIF folder (CSV or Parquet tables of CLIF 2.1, and five of 3.0) | The same tables: patients, hospitalizations as visits, ADT as bed stays, vitals, labs, drugs, support ... |
| A site's export described in a TOML file (a *declared* format) | The same tables, through the columns the file describes |
| A folder of recordings (WFDB records, EDF and BDF files, files an installed adapter reads) | `recordings`: file, format, patient, start, end, size, fingerprint |
| Its recordings that are one file, indexed ahead of time (`indexes`) | `recording_indexes`: each recording's index folder, the file's size and time, who made it |

db-builder reads its inputs where they are and writes only its output database, never into the
folders it reads.

## Contents

- [Build and install](#build-and-install)
- [Commands](#commands)
- [Web interface](#web-interface)
- [Tables](#tables)
- [Known exports](#known-exports)
- [Folders of recordings](#folders-of-recordings)
- [Identifiers and issuers](#identifiers-and-issuers)
- [Rewrite rules](#rewrite-rules)
- [Times](#times)
- [Sources](#sources)
- [Checking a database](#checking-a-database)
- [Encryption](#encryption)
- [Privacy](#privacy)

## Build and install

db-builder is a crate of the repository's Rust workspace (Rust 1.88 or later). From the
repository's root:

```bash
cargo build --release -p db-builder          # target/release/db-builder
cargo install --path db-builder              # or: into ~/.cargo/bin
cargo test --release -p db-builder
```

The build compiles SQLCipher and OpenSSL from source (for `--encrypt`): it needs a C compiler and
Perl, and takes a few minutes the first time. WFDB records are read with wfdb-x (the `wfdb`
feature, on by default). Built without it (`--no-default-features`), a WFDB record is still listed
by its header, without its start and end.

## Commands

Every command takes `--encrypt` for an encrypted database ([Encryption](#encryption)). A
command's errors go to the standard error, with exit status 2.

### new

```bash
db-builder new study.sqlite --timezone Europe/Paris
db-builder new demo.sqlite --demo                 # a demo database: invented patients only
```

Makes an empty database. `--timezone` (default `America/New_York`) is the time zone of the local
times in the inputs; a database has one. `--demo` marks it a demo database (a table
`medorama_demo`, [docs/schema.md](docs/schema.md#tables)): Medorama connects it to a demo
installation only, and a site's installation refuses it
([docs/design/demo.md](../docs/design/demo.md)). Use it only for invented patients. `add` and
`recordings` make the database too when it does not exist yet, unmarked.

### add

```bash
db-builder add study.sqlite patients.csv encounters.xlsx labs.parquet --issuer HOSP --mrn-authority HOSP
db-builder add study.sqlite export.csv --mapping mapping.toml
db-builder add study.sqlite notes.xlsx --category note
db-builder add study.sqlite muse.csv                          # GE MUSE metadata, recognised by its columns
db-builder add study.sqlite muse.csv --format ge-muse-patient-id
db-builder add study.sqlite clif_folder/                      # a CLIF folder
db-builder add study.sqlite acme.csv --formats formats/       # a declared format
```

Adds tables (each file a source) or CLIF folders (each table of the folder a source), all in one
transaction: if one fails, nothing is added.

| Option | Meaning |
|---|---|
| `--mapping FILE` | Map columns by hand: a TOML or JSON file of `field = "header"`, under `[columns]` or at the top level. With a mapping, a header is read as a field only when the mapping names it, or when it is the field's own name and the mapping names no other header for it. |
| `--issuer NAME` | The issuer of the patient IDs of rows that name none. |
| `--mrn-authority NAME` | The authority of the MRNs of rows that name none. |
| `--timezone TZ` | The time zone of a new database; for an existing one, it must be the database's. |
| `--category KEY` | The category of the entries of rows that name none ([docs/importing.md, Categories](../docs/importing.md#categories)). |
| `--format KEY` | `auto` (the default: a known export recognised by its columns), `none`, or a format's key (`db-builder formats`). |
| `--formats PATH` | A TOML file describing a format, or a folder of them (repeatable). |
| `--day-first` | Dates with slashes are day first (`03/02/2024` is 3 February); month first otherwise. |

For each source, `add` says what it added (persons, visits, entries, recordings), how many rows
it left out and why (the first 50 warnings, each with the file and line).

A database made by an earlier db-builder gets the indexes it lacks at its next `add` (or
`recordings`), from the command line or the web page: those a reader's searches use ([docs/schema.md, Indexes](docs/schema.md#indexes)).

```bash
db-builder add study.sqlite labs.csv --issuer HOSP --dry-run
```

With `--dry-run`, `add` (and `recordings`) reads everything and says what it would add, writing
nothing: per source, the known export it was read as, how each column is read (a recognised
field, a measurement, or `extras`), the counts and the warnings. It runs the whole add in a
transaction that is then rolled back, so what it says is what the add would do (a file added
already, or changed since, included); the database's bytes are left as they were, and a database
that does not exist yet is not made (the dry run uses one in memory). It prints no cell of the
rows. The web interface's **Preview** is the same dry run.

### recordings

```bash
db-builder recordings study.sqlite /data/icu --patients icu_patients.csv --issuer HOSP
db-builder recordings study.sqlite /data/psg --pattern '{patient}_*.edf' --issuer SLEEP-STUDY --category sleep
```

Adds the recordings of a folder and its subfolders, as one source ([Folders of
recordings](#folders-of-recordings)). It takes `--patients TABLE` or `--pattern PATTERN` (or both:
the table first), and `--mapping`, `--issuer`, `--mrn-authority`, `--timezone`, `--category`,
`--day-first` as `add` does.

### indexes

```bash
db-builder indexes study.sqlite --dir /data/psg/indexes --category sleep --category eeg
```

Makes the index of each recording that is one file (an EDF or BDF file, a file an installed
adapter reads), ahead of time, into `--dir` (one folder per recording, named by a digest of its
key), and names them in the table `recording_indexes` ([docs/schema.md,
recording_indexes](docs/schema.md#recording_indexes)). `--category KEY` (repeatable) indexes
only the recordings of those categories; all by default. Medorama takes these indexes instead of
indexing the recordings itself, as long as the files still have the size and time they were
indexed at and the same program reads them
([docs/databases.md](../docs/databases.md#indexes-and-signals-a-recording-database-holds)).

Each index is built beside and moved in. Run again, a recording whose index is current (same
size, time and maker, its folder present) is kept, and only new or changed recordings are
indexed. WFDB records and folders are not indexed ahead (Medorama indexes them when they are
opened): `indexes` lists them, with the missing files, among those it skipped.

### check

```bash
db-builder check study.sqlite
```

Reports what a reader cannot rely on ([Checking a database](#checking-a-database)). Exit status 0
when nothing is found, 1 when something is.

### remove-source

```bash
db-builder info study.sqlite            # the sources and their ids
db-builder remove-source study.sqlite 3 4
```

Takes sources out: their entries, recordings and extras, and the persons and visits that no other
source names and no row left refers to.

### info

```bash
db-builder info study.sqlite
```

The database's schema, version, time zone and maker, its tables' rows, and its sources (id, kind,
rows, when added, file).

### formats

```bash
db-builder formats --formats formats/
```

The known exports: built in, and those described in the TOML files given.

### rewrite

```bash
db-builder rewrite study.sqlite --rules moved.toml --dry-run    # count what would change
db-builder rewrite study.sqlite --rules moved.toml
```

Rewrites the paths of the files the database lists (recordings, entries' files, the sources'
inputs) by the `[[path]]` rules of a rules file ([Rewrite rules](#rewrite-rules)), for data moved
or mounted elsewhere after the database was built. Only the paths change: the files are not read
again. Then `check` lists any file still missing.

## Web interface

```bash
db-builder serve --set-password                        # once: the account's password (asked twice)
db-builder serve --root /data/build                    # then open http://127.0.0.1:8091/
```

`db-builder serve` is a web interface of its own on its own port (default 8091, next to
Medorama's 8090), with its own sign-in: one account, `admin` unless `--set-password --user NAME`
names another. `--set-password` reads the password from the standard input when it is not a
terminal (`echo … | db-builder serve --set-password`), and asks for it twice otherwise; `serve`
refuses to start until one is set, and says how to set it.

| Option | Meaning |
|---|---|
| `--root DIR` | The one folder the interface may browse, read and write in: its databases are made there, its inputs are read from there, and nothing above it is ever listed, read or written. |
| `--host H`, `--port P` | Where it listens: `127.0.0.1` (this computer only) and `8091` by default. |
| `--home DIR` | Its password file (`password`: the account's name and an Argon2id hash) and its log of actions (`db-builder.log`: sign-ins, databases made, sources added and removed, rules saved, copies, by path; never a row). Default `~/.local/share/db-builder` (`%LOCALAPPDATA%\db-builder` on Windows). |
| `--encrypt` | Every database it opens and makes is encrypted with the key in `$DB_BUILDER_KEY` ([Encryption](#encryption)). Without it, an encrypted database is not opened, and the page says why. |
| `--secure-cookies` | Mark the session cookie Secure: when it is served over HTTPS, behind a reverse proxy (also done when the proxy says `X-Forwarded-Proto: https`). |
| `--idle-minutes N` | A session unused this long ends (default 60; a session lasts 12 hours at most). |

**The page.** On the left, the databases (`.sqlite` files) of the root and its folders, with
**New…** (an empty database, its time zone chosen) and **Open…**; for the database open, tabs:

- **Tables**: `persons`, `visits`, `entries`, `recordings`, `extras` and `sources`, a hundred rows
  at a time, read-only, with a filter box (rows holding the text in any column).
- **Import**: tables, known exports and CLIF folders chosen in the root, with `add`'s options
  (format, issuer, MRN authority, category, a mapping file, declared formats, day-first dates).
  **Preview** says, for each input, what would be added (the format recognised, how each column
  is read, its first rows, the counts and warnings), writing nothing; **Add** adds them, all or
  nothing.
- **Recordings**: a folder of the root, with a patients table or a pattern; **Preview** (how many
  recordings, the first ones and their patients) and **Add**.
- **Rules**: the database's rewrite rules ([Rewrite rules](#rewrite-rules)), kept beside it as
  `<name>.rules.toml` (`study.rules.toml` for `study.sqlite`), checked before they are saved, and
  applied to every add from the page; **Rewrite** the paths the database lists by its `[[path]]`
  rules, after counting what would change.
- **Check**, **Sources** (each source's id, kind, rows and file; **Remove…** after a
  confirmation), **Info** (its meta and tables' rows), and **Save a copy…**: a copy in a new file
  of the root (`VACUUM INTO`; an encrypted database's copy is encrypted with the same key).

Adding and previewing run on a thread of their own, one at a time; the page shows the file being
read and the rows read so far, then the report of each source (rows, persons and visits added,
entries, recordings, rows left out, warnings).

**Under Medorama.** With Medorama's setting `db_builder_root` (or `medorama serve
--db-builder-root DIR`), Medorama runs `db-builder serve --root DIR --host 127.0.0.1 --port
<db_builder_port>` itself, its home in `<Medorama's home>/db-builder` (set its password once with
`db-builder serve --set-password --home <Medorama's home>/db-builder`), starts it again if it
stops, and stops it when it stops; its Databases tab's **Create a database…** opens this page in
a tab of its own. Medorama does not pass its requests on and shares no sign-in with it
([docs/administration.md](../docs/administration.md#db-builder)).

**Security.**

- Every request but the sign-in needs a session: a random token in a cookie (HttpOnly,
  SameSite=Strict, Secure with `--secure-cookies`), kept hashed in memory (a restart signs
  everyone out). Failed sign-ins are counted, per address and in all, and refused for a minute
  beyond five from one address or thirty in all; each failure waits a second.
- Every request that changes something carries the header `X-DB-Builder: 1`: a page of another
  site cannot add it (the server allows no cross-origin request), so it cannot make the browser
  act for the user (CSRF).
- Every path the page sends is relative to the root (or absolute and inside it), resolved with
  its links, and refused when it lies outside: a `..`, an absolute path elsewhere, a link (or a
  folder of links) leading out. The file chooser lists only what lies inside; a database, a copy
  or a rules file is written only inside, never through a link. Files a table's rows name outside
  the root are left out with a warning, and the recording readers open files inside it only.
- It listens on `127.0.0.1` by default. On another address it says, at start, to serve it behind
  a reverse proxy with TLS (and `--secure-cookies`); its page may not be framed, and its answers
  are not cached.
- Its pages show what the databases and inputs hold, identifiers and names included: give its
  account only to those who may see them, as for the files themselves.

## Tables

- **The reference** for the columns db-builder recognises, their names and their rules is
  [docs/importing.md](../docs/importing.md) (its "Column reference", "Categories", "Times and time
  zones" and "Values, units and flags"). db-builder recognises the same names, ignoring case,
  spaces and punctuation, and one more field: `patient_id_issuer` (headers `patient_id_issuer`,
  `issuer`, `id_issuer`), the issuer of the row's patient ID.
- **One row** may name a patient, an encounter (with a bed stay), an entry and a file, in any
  combination. Tables come in any order: a row that names an encounter no table has named yet
  keeps the encounter's key, and gets its person when the encounters' table is added.
- **A row naming its encounter only** (`encounter_id`, no `patient_id` or `mrn`, nothing that
  describes the encounter) refers to that encounter; it does not make one.
- **Columns db-builder does not recognise** go to `extras`, under their own headers, with the row
  they belong with. A measurement column (`QTc (ms)`, `Lactate [mmol/L]`, a known ECG
  measurement) makes one entry per number.
- **Files**: a `file` cell is a path, relative to the table's folder or absolute. A recording (a
  WFDB header, an EDF or BDF file, a file an installed adapter reads, or a folder of them) of a
  recording category (`icu`, `holter`, `eeg`, `sleep`) or of no category becomes a `recordings`
  row (a WFDB collection the row names, one); any other file (a 12-lead ECG, a PDF, a picture) is
  the entry's (in `extras`, `file`), and a `recordings` row too, of the row's category, keyed
  `<entry key>:file`, so that every file is in `recordings` and a reader finds an entry's file
  by its key. A
  missing file, and a WFDB signal file (`.dat`, `.mat`: give its header), are left out with a
  warning.
- **Rows that cannot be read** (a time or a date of birth that is no date, a normal range that is
  no number, a row that names no patient, encounter or entry) are left out, with a warning that
  names the file and line.
- A spreadsheet is read whole and must be 100 MB or less (save a larger table as CSV); of a
  spreadsheet, the first sheet is read. CSV and Parquet files are read row by row, of any size.

## Known exports

| Key | What |
|---|---|
| `ge-muse` | GE MUSE ECG metadata (one row per resting ECG, its XML elements as columns: `filename`, `PatientDemographics/PatientID`, `TestDemographics/AcquisitionDate`, ...), the patient by MRN. Recognised by its columns. |
| `ge-muse-patient-id` | The same, the patient by a patient ID (named with `--format`). |
| `clif-…` | CLIF's tables, each recognised by its columns: `db-builder formats` lists them. A CLIF folder given to `add` adds each of its tables, patients and hospitalizations first, and puts each susceptibility on its culture. |
| declared | A TOML file per format, given with `--formats`: its key, label, category, the columns that recognise it, and each column of the converted table (copied, a date read in a given form, values mapped, numbered columns joined). The file's form is described in [src/formats/declared.rs](src/formats/declared.rs). |

A format keeps what is worth showing and searching, and leaves out what names staff or the
visit's bookkeeping (physicians, technicians, account numbers, rooms); rows it cannot use are
counted, never named.

## Folders of recordings

`db-builder recordings OUT FOLDER` finds, in the folder and its subfolders (hidden entries
skipped, symbolic links not followed):

- **WFDB collections**, by their folder: a folder with an `index.csv`
  ([docs/wfdb-collection.md](../docs/wfdb-collection.md)) is one recording (format `WFDB
  collection`, its size its files'), its records parts of it, as the viewers read it.
- **WFDB records**, by their header (`.hea`). The segments of a multi-segment record are not
  records of their own. The record's signal files stay next to its header.
- **EDF, EDF+, BDF and BDF+ files** (`.edf`, `.bdf`).
- **Files an installed adapter reads**, such as BedMasterEx `.Stp` files with the stp-adapter
  add-on ([docs/adapters.md](../docs/adapters.md)).

Each recording's start and end come from its header (WFDB and EDF; an adapter's files, from a
BedMaster name `UNIT_BED-<Unix time>`, the start only), read without reading its signals.

Its patient comes from:

- **`--patients TABLE`**: a table (CSV, spreadsheet or Parquet) with a `file` column (or
  `recording_file`) and the patient's columns (`patient_id`, `patient_id_issuer`, `mrn`,
  `mrn_authority`, names ...). A row may also give the recording's `encounter_id`, `category`,
  `name` (its label), `time` and `end_time` (used when the header gives none); its other columns go
  to `extras`. A row's file is matched, ignoring letter case, by its path below the folder, its
  path from the table's folder, its absolute path or its file name, with or without the
  extension (`icu/rec1.hea`, `rec1`).

  ```csv
  file,patient_id,issuer,encounter_id
  icu/rec1.hea,P900001,HOSP,E9100002
  sleep/P900004_psg.edf,P900004,HOSP,
  ```

- **`--pattern PATTERN`**: the patient ID in the files' names. `{patient}` is the ID, `*` any
  text, other characters themselves (letter case ignored); a pattern with a `/` is matched against
  the path below the folder (`{patient}/*` names a folder per patient).

A recording that neither names is added without a person, and `check` lists it. The folder is
one source: added again unchanged, nothing is done; changed (a recording added, removed or
changed, another table or pattern), it replaces its earlier version.

## Identifiers and issuers

Every row should name its patient's issuer: the system that issued the patient ID
(`patient_id_issuer`, or `--issuer`), or the MRN's authority (`mrn_authority`, or
`--mrn-authority`). The pair (issuer, identifier) names one patient, and databases are joined on
exact matches of that pair. `add` warns about rows without an issuer, and `check` lists them.

A row's patient is found by its patient ID and issuer, else by its MRN and authority (ignoring
letter case); else the patient is added. A patient known only by an MRN takes the patient ID of a
later row that gives both. Names, date of birth and sex fill what is empty and never overwrite
what an earlier row gave (a warning says when they differ).

## Rewrite rules

`add` and `recordings` take `--rules FILE`: a TOML file of rules applied to each row as it is
read, before anything is written. Path rules change the files a row names (`file`,
`recording_file`, and the patients table's file column); identifier rules change `patient_id`,
`mrn` and `encounter_id`.

```toml
[[path]]                         # a prefix, matched at a folder boundary (/labs/data, not /labs/database)
prefix = "/labs/data"
to = "/mnt/data"

[[path]]                         # or a regular expression; $1, $2 … its groups
regex = '^/old/(\w+)/'
to = '/new/$1/'

[mrn]                            # also [patient_id] and [encounter_id]
case = "upper"                   # "upper", "lower", or "keep" (the default)
replace = [{ regex = '(?i)^pt_', to = 'PT' }]    # PT_123 and pt_123 become PT123

[patient_id]
replace = [{ regex = '^PT_(\d+)$', to = 'PT$1' }]
```

Path rules apply in order, each to the result of the one before. An identifier's replacements
apply in order, then its case. The regular expressions are those of Rust's `regex` crate
(`(?i)` ignores case). Rules change what is written, so the same rules are given to every `add`
of one database. `db-builder rewrite` applies a file's path rules to a database already built.

## Times

Times are written as ISO 8601 with their offset, in the database's time zone
(`2024-03-02T07:10:00-05:00`). Local times in the inputs are read in that zone, as
[docs/importing.md](../docs/importing.md#times-and-time-zones) says. A recording header's clock
time (WFDB, EDF) is read as UTC, as the recording engine and the viewers read it, so that a
recording's start here is the one its index gives. Set the zone when the database is made
(`new --timezone`, or the first `add`).

## Sources

Each file added (each table of a CLIF folder, each folder of recordings) is a source
(`sources`), identified by its path and fingerprint. The same file added again is not added
twice; a file that changed since it was added replaces its earlier version (its rows removed
first). `remove-source` takes a source out.

## Checking a database

`db-builder check` reports, by key (never by name or MRN):

- persons whose patient ID names no issuer, or whose MRN names no authority;
- identifiers given to two persons by the same issuer (patient IDs, and MRNs of one authority);
- visits, entries and recordings whose `person_key` or `visit_key` names no person or visit, and
  those without a person;
- entries whose person is not their visit's;
- recordings whose file is missing.

## Encryption

```bash
export DB_BUILDER_KEY=$(openssl rand -hex 32)
db-builder --encrypt new study.sqlite
db-builder --encrypt add study.sqlite labs.csv
db-builder --encrypt info study.sqlite
```

With `--encrypt`, the database is a SQLCipher database, its key the 64 hex digits in
`$DB_BUILDER_KEY` (a raw 256-bit key: `PRAGMA key = "x'…'"` opens it in any SQLCipher program).
Without the key, nothing of the file can be read. Keep the key in a key manager, not next to the
file. Medorama connects such a database with its key, given as the database's password (the
page's "Key, if encrypted", or `medorama databases connect … --password-stdin`), which it keeps
sealed by its master key.

## Privacy

The database holds what its inputs hold: identifiers, names and dates of birth when the tables
give them. Keep it, and the inputs, on an encrypted disk (or use `--encrypt`), give no more
identifiers than a study needs, and never commit them to a repository.

## License

db-builder is licensed, like the rest of the repository's software, under the PolyForm Noncommercial
License 1.0.0 ([LICENSE](../LICENSE)): free for noncommercial use, with commercial licenses by
agreement ([COMMERCIAL.md](../COMMERCIAL.md)). Its installers ask you to accept these terms first. The documentation (the `docs/` folders) is
under CC BY-NC-ND 4.0 ([docs/LICENSE.md](../docs/LICENSE.md)); every dependency and its license is
listed in [THIRD_PARTY_LICENSES.md](../THIRD_PARTY_LICENSES.md).
