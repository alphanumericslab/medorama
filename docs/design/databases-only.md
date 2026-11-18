# Databases only: Medorama connects databases and builds none

**Status: built (November 2026),** index format 3 included, except what
[Built differently from the proposal](#built-differently-from-the-proposal) lists. This page is the
design note of how data reach Medorama: it connects databases and imports nothing. It replaced
imports, the catalog's copies of patient data and the results folder. It is written for
contributors and reviewers, and for site engineers who prepare databases for Medorama.

**Since 0.3.0**, what Medorama keeps of each database is narrower than this page says: its
subjects and their encounters, the names and units of its variables, and the recordings it has
opened. [connected-databases.md](connected-databases.md) is the design of that, and replaces this
page's workspace, variables and connecting sections where they differ.

The guide for data managers is [databases.md](../databases.md); the design of reading connected
databases (views, OMOP, the patient index, searching) is [sources.md](sources.md); db-builder's
schema is [db-builder/docs/schema.md](../../db-builder/docs/schema.md).

## Contents

- [Summary](#summary)
- [Decisions](#decisions)
- [Built differently from the proposal](#built-differently-from-the-proposal)
- [The picture](#the-picture)
- [Terms](#terms)
- [Who is responsible for what](#who-is-responsible-for-what)
- [Kinds of databases](#kinds-of-databases)
- [The control store](#the-control-store)
- [The workspace](#the-workspace)
- [Keys and encryption](#keys-and-encryption)
- [Access: grants](#access-grants)
- [Connecting a database](#connecting-a-database)
- [Online and offline](#online-and-offline)
- [Variables](#variables)
- [Identity: the patient index](#identity-the-patient-index)
- [Searching](#searching)
- [Cohorts](#cohorts)
- [Derived databases: indexes and results](#derived-databases-indexes-and-results)
- [Removing a database](#removing-a-database)
- [The viewers](#the-viewers)
- [db-builder](#db-builder)
- [Requirements on a connected database](#requirements-on-a-connected-database)
- [What left Medorama](#what-left-medorama)
- [Build order](#build-order)
- [Questions left](#questions-left)

## Summary

1. **Medorama only connects databases.** It reads them in place, read-only, through SQL: OMOP
   instances, research warehouses, SQLite files. It never imports a table or a folder, and it
   keeps no copy of the patient data it reads in its control store.
2. **A folder of recordings is a database too.** A recording database lists each recording's
   file, patient and time; [db-builder](#db-builder) makes one from a folder. Medorama does not scan
   folders.
3. **db-builder** is a separate, open tool shipped in the repository (`db-builder/`). It turns
   tables (CSV, spreadsheets, Parquet), known exports (GE MUSE, CLIF, declared TOML formats) and
   folders of recordings into an open SQLite database. Its databases are not specific to
   Medorama. Sites with their own databases do not need it.
4. **Medorama keeps a small control store**: users, grants, the audit log, the list of connected
   databases and their status, cohort definitions, the wrapped keys, and patient numbers with
   keyed hashes of their identifiers.
5. **What Medorama generates goes into its workspace**, one folder per connected database, each a
   set of SQLite databases (its persons as linked, its variables, its recordings, the results of
   actions), encrypted with that database's own key. Cohorts are workspace databases too.
6. **Removing a database leaves no trace of it**: its key is destroyed first, then its workspace
   folder, then every cohort row it contributed to. It is no longer offered for searching.
7. **Identity is the databases' responsibility.** Each database names each patient by an
   identifier unique within its issuer. Medorama joins databases on exact matches only.

## Decisions

The decisions made with the project's owner (November 2026), which the rest of this page follows.

| Question | Decision |
|---|---|
| Imports | None. Medorama connects databases only; everything else goes through db-builder first. |
| Folders of recordings | Made into a recording database by db-builder, then connected. |
| What Medorama generates | Into the workspace (the former results folder, renamed), one folder per connected database, as SQLite databases. |
| Encryption of connected databases | Their owner's responsibility. |
| Encryption of the workspace | Medorama's: everything in it is patient data that Medorama generated. Each database's folder has its own key; removal destroys the key first. |
| Who may use a database or cohort | Users granted it. |
| Identity | Not Medorama's responsibility: a documented requirement on every database (an identifier and its issuer). Exact joins only. |
| Cohorts after a removal | No cohort keeps a row that a removed database contributed to. |
| Results of actions (HRV, QC, models) | Derived databases in the workspace, linked to their source database, removed with it. |
| The standalone viewers | Keep their own caches, as SQLite databases in their own folder (index format 3: `cache_dir/index.sqlite`, encrypted when `index_key_file` is set). |
| Servers | One server in this version. |
| Compatibility | None needed (pre-release): earlier catalogs are not migrated. The control store's schema is a fresh version 1. |
| The builder's name | `db-builder`. |

## Built differently from the proposal

What was built differs from the proposal in these points. The sections below describe what was
built.

| Proposed | Built |
|---|---|
| The master key from the operating system's keyring by default, or from Vault or a cloud KMS | From `$MEDORAMA_MASTER_KEY`, `master_key_command` (a command that prints it: a keyring, Vault, a KMS command line) or `master_key_file`. No key manager's client is built in ([todo.md](../todo.md#h2-built-in-key-managers)). |
| A database without an issuer refused | Accepted: a patient ID without an issuer is the database's own (`db:<name>`), so that it links only that database's persons. |
| The control store holds patient numbers only | It also holds a keyed hash of each (issuer, identifier) pair (`patient_keys`), so that databases can be joined without reading each other's workspace. No identifier can be read back from it. |
| `patients.sqlite`, `index.sqlite` in each database's folder | `persons.sqlite`, `variables.sqlite`, `recordings.sqlite`, `results-<action>.sqlite`, `results/` and `index.sqlite`. |
| Index format 3: indexes as encrypted SQLite in the database's folder | Built as proposed: `index.sqlite`, SQLCipher with the database's key, the arrays cut into chunks of 4,096 values (the layout the benchmark chose: [performance.md](../performance.md#index-format-3)). Also a store for the viewer's own indexes (`viewer/index.sqlite`, its own key). The setting `index_dir` is gone. |
| Actions write into a results database the job hands them | Actions still write files into an output folder; Medorama copies the numbers of their `summary.json` into `results-<action>.sqlite`. The output folders are not encrypted. |
| A derived database of two databases in `workspace/d-NNNN/` | The folder name and its removal exist; nothing writes one yet. |
| An encrypted db-builder database connected with its key as a setting | Built as the database's password: for a SQLite file it is its SQLCipher key (64 hex digits a raw key, else a passphrase), sealed by the master key like any password, and part of the connection pool's key. |
| Checks every `check_interval_s` and before each search | Every 60 seconds (fixed) and on demand; `check_interval_s` is the recordings' re-check. |
| `database_links` | `derived` and `derived_sources`. |

## The picture

```
  browser: Databases tab · Cohort tab · patient page · VitalStream · ECG · PACS
                              │ HTTPS
┌─────────────────────────────▼──────────────────────────────────────────────┐
│ Medorama server (one)                                                       │
│  sign-in, sessions, roles, grants, audit    ══ privacy gate ══              │
│  search planner: ticked ∩ granted ∩ online databases, by variable           │
│  key service: unwraps a database's key for a granted request                │
└──────┬───────────────────┬──────────────────────────────┬──────────────────┘
       │ read/write        │ read/write (encrypted)        │ read only
  control store        workspace/                      connected databases
  SQLite or            ├── db-0007/  persons,           OMOP · SQL with views ·
  PostgreSQL           │             variables,         db-builder SQLite ·
  (no identifiers)     │             recordings,        recording databases
                       │             results            (and their files, in place)
                       │             index store
                       ├── db-0012/ …
                       ├── cohorts/  one file per cohort
                       └── viewer/   the viewer's own index store
```

## Terms

| Term | Meaning |
|---|---|
| Connected database | A database Medorama reads in place, read-only: OMOP, SQL through views, a db-builder SQLite file. |
| Recording database | A connected database with recordings (a db-builder `recordings` table, or a `recordings` view): a file (or a WFDB header), its patient, its start and end, its format. The files stay where they are. |
| Control store | Medorama's own database: users, grants, audit, the list of databases, cohort definitions, wrapped keys, patient numbers and keyed hashes. No identifiers, names or values. |
| Workspace | The folder where Medorama keeps what it generates: one folder per connected database, and the cohorts. |
| Derived database | A workspace database computed from a connected database: the results of an action. |
| Data key | The key of one workspace folder (one connected database) or file (one cohort). |
| Master key | The key that wraps every data key. It comes from the environment, a command or a file, never from the workspace. |
| Patient index | The links from each database's (issuer, identifier) pairs to Medorama's patient numbers. |
| Issuer | Who assigned an identifier: a hospital's MRN system, a study, a national registry. The pair (issuer, identifier) names one patient. |

## Who is responsible for what

| | The database's owner | Medorama |
|---|---|---|
| The data | Their content, quality, retention and backups | Reads them; never changes or copies them |
| Encryption of the data | Theirs (PostgreSQL storage encryption, disk encryption) | Encrypts its workspace, its indexes included |
| Identity | One identifier per patient within an issuer; merges and corrections in the source | Joins on exact (issuer, identifier) matches; lists the conflicts it finds |
| Access | Database accounts for Medorama, read-only | Grants per user and database; the privacy gate; the audit log |
| Availability | Their uptime | Shows what is offline; never searches it silently |
| Credentials | Issue Medorama an account and rotate it | Keeps the password wrapped like a data key |

## Kinds of databases

| Kind | How Medorama reads it | Written by |
|---|---|---|
| `omop` | OMOP CDM 5.3 or 5.4 tables, through built-in views ([sources.md](sources.md#omop)) | The site |
| `views` | Any SQL database through the site's views: `persons` and `entries` (required), `visits`, `recordings` (for a recording database) and a `version` query ([databases.md](../databases.md#connect-any-sql-database-through-views)) | The site |
| `dbb` | A db-builder database: its tables are the views; its version is `meta.version` | db-builder |

A recording database is not a kind: any database whose views include `recordings` is one (for
`dbb`, a database with a `recordings` table). A db-builder database can hold both clinical tables
and recordings. Derived databases are not connected; they are listed in `derived`. Engines:
PostgreSQL and SQLite files in this version; the others
([todo.md A8](../todo.md#a8-sql-server-oracle-snowflake-bigquery)) stay later.

## The control store

The control store is SQLite by default, or PostgreSQL (`database_url`). It holds no
identifiers, names or values. Its schema is a fresh version 1
(`medorama/src/schema/sqlite/001_schema.sql`, `medorama/src/schema/postgres/001_schema.sql`).

| Table | What it holds |
|---|---|
| `users`, `sessions` | Accounts and sessions (with each session's Privacy Mode). |
| `audit`, `audit_seals` | The audit log: append-only, sealed. |
| `databases` | Each connected database: `id`, `name`, `kind`, `url` (without a password), `password_file`, `password_wrapped`, `ca_file`, `settings` (JSON), `status` (`online`, `offline`, `unknown`, `removing`), `status_error`, `checked_at`, `data_version`, `variables_version`, `created_by`, `created_at`. |
| `derived`, `derived_sources` | Results databases of actions, and the databases they were computed from. |
| `keys` | Each data key, wrapped by the master key: `owner` (`db:N`, `cohort:N`, `derived:N`), the wrapped bytes, the master key's id. |
| `grants` | `user_id`, `object` (`db:N` or `cohort:N`), `permission`. |
| `patients` | Medorama's patient numbers, their pseudonyms (`MO-…`) and date draws; no identifier. |
| `patient_keys` | `hash`, `database_id`, `patient`: a keyed hash of each (issuer, identifier) pair a database gives, and the patient it links. |
| `visit_pseudonyms` | `database_id`, `visit_hash`, `pseudonym` (`MO-E-…`): a visit's pseudonym. |
| `cohorts` | Each cohort's definition: name, description, owner, criteria, the databases it searches (JSON), source, origin, times, the databases' versions. Its members are in the workspace. |
| `runs`, `jobs` | Runs of actions and their jobs (`jobs.database_id`, `jobs.recording`: the recording's number). |
| `history` | The patients a user opened. |
| `settings`, `data_version` | Settings kept in the database; the data version and the identifiers' version (the privacy gate's dictionary is built again when it changes). |

## The workspace

The workspace is a folder chosen when Medorama is set up (`medorama init --workspace DIR`,
setting `workspace_dir`, required). It replaced `results_dir`.

```
workspace/
├── db-0007/                       one connected database (its id)
│   ├── persons.sqlite             its persons as linked: identifiers, names, birth date, sex,
│   │                              patient number, their pairs' hashes; conflicts; a summary per
│   │                              person for the patient list (visits, latest time, entries,
│   │                              categories)
│   ├── variables.sqlite           category, name, vocabulary, code, unit, entries, persons,
│   │                              numeric; the version read
│   ├── recordings.sqlite          each recording: key, person, visit, path, kind, size,
│   │                              fingerprint, format, start, end, label, category, present,
│   │                              problem; derived series (a Holter's heart rate)
│   ├── results-hrv.sqlite         numbers of an action's job summaries (a derived database)
│   ├── results/hrv/r12/j40/       an action's output folder: recording 12, job 40
│   └── index.sqlite               the index store of its recordings (index format 3);
│                                  index.tmp/ while an index is built
├── db-0012/ …
├── cohorts/
│   └── c-0003.sqlite              one cohort's members, their provenance, those removed
└── viewer/index.sqlite            the viewer's own index store (its own key, owner `viewer`)
```

- **One folder, one key.** Every SQLite file in `db-0007/` is encrypted with database 7's data
  key, and every cohort file with its own key. Removing the database removes the folder as a
  whole.
- **SQLite, through SQLCipher** (AES-256), opened with the raw data key (no key derivation).
  Each file is an ordinary SQLite database once opened with its key. SQLCipher is always built
  (the former `encryption` Cargo feature is gone; building needs Perl and a C compiler for
  OpenSSL).
- **Written only by Medorama**, with permissions for its account only (`0700` folders, `0600`
  files).
- **Not encrypted:** only the files action plug-ins write into their output folders. Their
  numbers go into the encrypted `results-<action>.sqlite`. Keep the workspace on an encrypted
  disk.
- **Never inside a data root.** A workspace inside a connected database's folder is refused, and
  so is connecting a database file that lies inside the workspace or is the control store.

## Keys and encryption

Envelope encryption ([medorama/src/workspace.rs](../../medorama/src/workspace.rs)).

1. **A data key per owner** (`db:N`, `cohort:N`, `derived:N`): 256 random bits, made when the
   database is connected (or the cohort created).
2. **Stored wrapped**: sealed with AES-256-GCM under the master key, with the owner as associated
   data (a wrapped key moved to another owner does not unwrap), in the control store's `keys`
   table, with the master key's id (the first 16 hex digits of its SHA-256), so that a wrong
   master key is named rather than mistaken for damaged data. The data key itself is never
   written.
3. **The master key** (64 hex digits) comes from, in this order:
   - `$MEDORAMA_MASTER_KEY`, from a service manager;
   - `master_key_command`, a command that prints it: a keyring
     (`security find-generic-password -s medorama -w` on macOS, `secret-tool lookup …` on Linux),
     Vault, or a KMS command line;
   - `master_key_file` (default `<home>/master.key`, made by `medorama init` with mode `0600`,
     outside the workspace).

   No key manager's client is built in: they are reached through `master_key_command`. With
   Vault's transit engine or a cloud KMS, the master key still reaches Medorama's memory; wrapping
   inside the key manager is [todo.md H2](../todo.md#h2-built-in-key-managers).
4. **Unwrapped in memory only**, for a request whose user holds a grant, and wiped when dropped;
   never logged, never sent to a browser.
5. **Rotation**: `medorama keys new FILE` makes a key file; `medorama keys rotate --new-key-file
   FILE` unwraps every data key and wraps it again with the new key (the data are untouched);
   then point the setting at the new key.
6. **Shredding**: removing a database or a cohort deletes its wrapped data key first. Without it,
   the files are unreadable, even from a copy of the disk.
7. **Passwords**: a database's password given to Medorama (the page, or `databases connect
   --password-stdin`) is sealed by the master key in `databases.password_wrapped` (owner
   `password:<id>`): never in a file, never shown. A `password_file` or
   `$MEDORAMA_SOURCE_PASSWORD_<NAME>` works too.

**Backups.** A backup of the control store holds the wrapped data keys of its day. Restoring an
old backup of both the control store and the workspace brings back a removed database's files.
Shredding is final once the backups older than the removal have expired; the removal's audit
entry gives the date from which it holds.

**The control store itself** (SQLite) can be encrypted with `key_file` or `$MEDORAMA_KEY`, as
before. **Source databases are not covered**: their encryption is their owner's.

## Access: grants

[medorama/src/grants.rs](../../medorama/src/grants.rs).

| On | Permissions (each implies the weaker) |
|---|---|
| A database | `search` (criteria find its patients) < `view` (its data on patient pages, its recordings, the viewer) < `phi` (its identifiers, with the user's PHI permission, outside Privacy Mode) < `manage` (check, refresh, grant, remove; see its URL, settings and conflicts) |
| A cohort | `view` < `edit` < `manage`. Sharing a cohort is granting it. |

- The admin who connects a database (or makes a cohort) is granted everything on it. Other
  admins are not granted databases by being admins: they see them listed, so that they can grant
  them, and they can grant themselves (audited as `grant_self`).
- The server unwraps a database's data key only for a request whose user holds a grant on it.
  Keys protect the files at rest, grants protect them in use. Keys wrapped for each user would
  only add protection against the server's own administrators; this version does not do it
  ([todo.md H2](../todo.md#h2-built-in-key-managers)).
- The person at the terminal (the command line, `local:<user>`) holds every grant.
- Revoking a grant changes nothing in the data. Removing the database shreds its key.

## Connecting a database

**Databases › Connect a database** (admins), or `medorama databases connect NAME --kind KIND --url
URL …` ([databases.md](../databases.md#connecting)):

1. Kind and connection: a PostgreSQL server and an account, or a SQLite file inside `data_roots`.
2. For `views`: the views (a TOML file, or pasted in the page). For `omop` and `dbb`: nothing.
3. Identity settings: `issuer` (who issued its patient IDs; empty: the database itself),
   `mrn_authority` (when a row names none), and for OMOP `identifier` (what
   `person_source_value` holds).
4. Medorama checks it (each view must answer), reads the version, seals the password, makes the
   data key and the workspace folder, grants it to the admin, reads the [variables](#variables),
   links the persons ([patient index](#identity-the-patient-index)) and lists the recordings.
5. The database is listed, online. A database that connects but cannot be read is kept, and says
   to refresh it.

## Online and offline

- Each database is checked when the server starts, every 60 seconds (fixed), and on demand
  (`medorama databases check`, **Check now**): a SQLite file must be there, and each view must
  answer.
- **The Databases tab** lists every connected database with its status: a green dot for online,
  grey for offline (when it was last checked; managers see the error), never red (red is for
  alarms). Offline databases are listed after the online ones.
- An offline database keeps everything: its workspace, its variables, its grants, and its
  patients on the patient list (from `persons.sqlite`). It is not searched, and its data are not
  on patient pages.

## Variables

- What each database holds is read when it is connected and kept in its `variables.sqlite`:
  each variable's category, name, vocabulary, code and unit, how many entries and persons, and
  whether it is numeric ([medorama/src/variables.rs](../../medorama/src/variables.rs)).
- Kept with the database's data version: the version query (OMOP's `cdm_source`, a db-builder
  database's `meta.version`, a views database's `version`), else
  `count:<persons>:<entries>:<recordings>`. When a check finds a new version, the variables,
  persons and recordings are read again; `medorama databases refresh NAME` does it on demand.
- The planner uses them to send a criterion only to the databases whose variables may hold it,
  and the Cohort tab uses them to complete names.

## Identity: the patient index

[medorama/src/persons.rs](../../medorama/src/persons.rs).

- Each person gives (issuer, identifier) pairs: its patient ID under `patient_id_issuer` (else
  the database's `issuer` setting, else `db:<name>`), its MRN under `mrn:<authority>`; a person
  with neither, (`key:db:<id>`, its `person_key`), which links only to itself. Each pair is
  assumed to name one patient: [requirement 1](#requirements-on-a-connected-database).
- Linking keeps, in the control store, an HMAC of each pair (keyed with the privacy gate's
  secret; issuers and identifiers compared ignoring case, both trimmed) and the patient number it links
  (`patient_keys`): an existing number when another database gave the same pair, a new one
  otherwise. The identifiers themselves are kept only in the database's `persons.sqlite`.
- A person whose pairs lead to two patient numbers is a **conflict**: listed (Databases › the
  database › Identity conflicts; `medorama databases conflicts NAME`), not linked, never merged.
  The owner corrects the source; a refresh links again.
- No scored matching, no merge, no split, no Review tab, no held rows.
- Persons are linked when a database is connected or refreshed (all of them), and as searches
  find new ones. A patient number that no database links any more is removed, with its
  pseudonym.

## Searching

- **The Cohort tab lists the databases** under "Search in", one tick each. Offline ones are
  greyed and cannot be ticked. None ticked means every one.
- **The scope** is the ticked databases, among those granted for `search`, among those online.
  Each criterion goes to the databases of the scope whose variables may hold it (a measurement's
  name, a diagnosis, a drug), one query per database and criterion; the persons found are mapped
  to patient numbers, and the groups (`AND`, `OR`, `NOT`) are combined by patient. A threshold
  is sent with the criterion, per unit; the criteria are read 4 at a time, and each answer is
  kept while the database's data version stays (0.3.2;
  [connected-databases.md, Searching a database](connected-databases.md#searching-a-database)).
- **A result says what it searched**: "searched A, B · left out: C (offline)", with the reasons
  `not ticked` and `offline`.
- `NOT` and the registry's criteria (sex, age, a database) apply among the patients that the
  scope's databases link.

## Cohorts

- A cohort's definition (name, criteria, the databases it searches, their versions) is in the
  control store; its members are in `workspace/cohorts/c-NNNN.sqlite`, encrypted with the
  cohort's own key: `members(patient, how, added_by, added_at)` (`how` is `criteria` or
  `manual`), `provenance(patient, database)`, and `removed(patient, …)`.
- **Each member records the databases whose answers put it there.** For `AND`, all parts'
  databases; for `OR`, those of every part it meets; for the registry's criteria (sex, age,
  database) and `NOT`, the scope's databases that link it; with exclusions, every scope database
  that links it. For `A.lactate > 4 AND B.troponin > 100`, a member has both A and B.
- **Saving a cohort** records each database's version; a cohort's page shows "searches A, B"
  and the versions. **Refreshing** searches again and lists what changed.
- **After a removal** no cohort keeps a member whose provenance includes the removed database.
- Cohorts are shared through grants. Deleting a cohort shreds its key, then deletes its file.

## Derived databases: indexes and results

- **Indexes** are in each recording database's index store, `db-NNNN/index.sqlite` (index format
  3: [index-format.md, Version 3](../../viewers/vitalstream/docs/index-format.md#version-3-the-index-store)),
  encrypted with the database's key and keyed by the recording's key
  ([index-format.md](../../viewers/vitalstream/docs/index-format.md#the-key)). They go with the
  database's folder when it is removed. The indexes that no recording of the database uses any
  more are removed from its store (`sweep_index`).
- **Series derived from a recording** (a Holter's heart rate) are kept in `recordings.sqlite`.
- **Results of actions**: a plug-in writes files into its output folder,
  `results/<action>/r<n>/j<job>/` in the recording database's folder. The numbers of its
  `summary.json` go into `results-<action>.sqlite` (`recording`, `person_key`, `name`, `value`,
  `at`), registered in `derived` and `derived_sources`, with the database's key.
- **Searchable**: a result's numbers are measurements named `<action>.<field>`, so that
  `hrv.rmssd > 50` is a criterion. Its patients are those of its source.
- **A result of two databases** (a model reading recordings of A and labs of B) would be a derived
  database in its own folder `workspace/d-NNNN/`, with its own key, linked to both, and removed
  with either. Removal handles such folders; nothing writes one yet.

## Removing a database

**Databases › the database › Remove…** (typing its name), or `medorama databases remove NAME
[--yes]`, for a user with `manage` ([medorama/src/databases.rs](../../medorama/src/databases.rs),
`remove`):

1. Mark it `removing`: it is no longer offered, searched or opened.
2. **Destroy its data key**, and those of the databases derived from it. From now on its workspace
   files cannot be read.
3. Delete its workspace folder (its index store with it), and the derived folders. The viewer was
   told to forget the store before step 2.
4. Delete every cohort member whose provenance includes it, in every cohort.
5. Delete its `patient_keys`; delete the patients no database links any more, with their
   pseudonyms, and from the cohorts' lists.
6. Delete its jobs, visit pseudonyms, grants, derived rows and its row; forget its pooled
   connections; write the audit entry (who, when, its name; no patient data).

Each step can be repeated: a removal interrupted by a crash finishes when the server or any
command starts (`resume_removals`). The indexes that no recording uses any more are then
removed from the other databases' stores. The connected database itself is never touched.

**The audit log keeps its entries** that name the removed database's patients by number. Health
records law requires audit logs to be kept (HIPAA: six years), so they are not deleted; the
numbers lead nowhere once the links are gone.

## The viewers

- **Index stores.** Launched by Medorama (`--keys-on-stdin`), the viewer reads the index stores
  of the recording databases, whose paths and keys Medorama writes on its standard input, one
  JSON line each, at every start and when a database is connected, and `{"forget": …}` before one
  is removed (never on the command line, in the environment or in a file). What it indexes itself
  goes into its own store, `<workspace>/viewer/index.sqlite`, with a key of its own. Standalone, a
  viewer keeps its own store, `cache_dir/index.sqlite`, encrypted when `index_key_file` is set.
  The three viewers (Rust, C++, Python) write the same rows, checked by the conformance test
  (`viewers/vitalstream/tests/parity/index_parity.py`). Format 3 was measured against format 2
  in the Rust engine before it replaced it ([performance.md](../performance.md#index-format-3)).
- **Access.** Through the gateway, a user other than an admin opens only recordings of databases
  granted to them with `view`.
- **Recording numbers.** A recording is numbered `database × 1,000,000,000 + n` (its number in its
  database), one number across Medorama: the patient page, the jobs, the gateway.

## db-builder

A separate tool in `db-builder/`, with its own [README](../../db-builder/README.md),
[changelog](../../db-builder/CHANGELOG.md) and command (`db-builder`, built by the installers as
`.venv/bin/db-builder`). It does not depend on Medorama, and its databases do not either: an
open SQLite schema, readable by any SQLite program.

| Input | Output |
|---|---|
| Tables: CSV, Excel (`.xlsx`, `.xls`), OpenDocument (`.ods`), Parquet | `persons`, `visits`, `entries`, with the columns recognized as [importing.md](../importing.md) says, and every other column in `extras`, as it came |
| GE MUSE metadata, CLIF folders, a site's export described in TOML | The same tables, through the readers moved from Medorama (`formats/`) |
| A folder of recordings (WFDB, EDF and BDF, files an installed adapter reads) | `recordings`: file, format, patient (from a table or a pattern of the files' names), start, end, size, fingerprint |

DICOM studies and folders of MUSE XML files go through a table first (`dicom-py index`, `muse-py
index`), which `db-builder add` then adds.

**The schema** is version 1, and its reference is
[db-builder/docs/schema.md](../../db-builder/docs/schema.md): tables `meta`, `persons`, `visits`,
`entries`, `recordings`, `extras`, `sources`, `row_sources`; keys, times, indexes and the version
rule. Its `persons`, `visits`, `entries` and `recordings` have the canonical views' columns, so
Medorama reads a db-builder database with no SQL to write. Points the proposal did not have:

- `sources.fingerprint`: a source is identified by its file and fingerprint; the same file added
  again is not added twice, and a changed one replaces its earlier version.
- `row_sources`: the sources that name each person and visit, so that `remove-source` removes a
  person or visit with the last source that names it.
- `extras.source_id`: each extra cell's source.
- **Every file a row names becomes a `recordings` row**, of the row's category: a recording (of a
  recording category, or of none) and also a 12-lead ECG, a picture, a PDF, a folder of DICOM. The
  patient page attaches the non-waveform files for the ECG and PACS viewers and opens the waveform
  categories (`icu`, `holter`, `eeg`, `sleep`) in VitalStream.
- **A recording header's clock time** (WFDB, EDF) is read as UTC, as the recording engine and the
  viewers read it, so that a recording's start in the database is the one its index gives.

- **Commands**: `new`, `add` (tables, known exports, CLIF folders; `--mapping`, `--issuer`,
  `--mrn-authority`, `--timezone`, `--category`, `--format`, `--formats`, `--day-first`),
  `recordings OUT FOLDER --patients TABLE | --pattern PATTERN`, `check`, `remove-source`, `info`,
  `formats`; `--encrypt` with `$DB_BUILDER_KEY`.
- **Every row should name its patient's issuer**; `add` warns, and `check` lists persons without
  one and identifiers given to two persons by the same issuer.
- **It writes only its output database**, never into the folders it reads.
- The import code moved there (`tables.rs`, `parquet_rows.rs`, `formats/`), with its tests.

## Requirements on a connected database

Documented for the sites in [databases.md](../databases.md#requirements-on-a-connected-database).

1. **Identity**: every person has an identifier unique within its issuer, and the issuer is
   named. The same patient in two databases has the same (issuer, identifier) in both, or
   Medorama shows two patients. Merges and corrections are made in the database.
2. **Read-only access** for Medorama: an account with `SELECT` only, on the tables or views it
   reads.
3. **The canonical columns**, through OMOP, the views, or db-builder's schema.
4. **A version** Medorama can read (`cdm_source`, `meta.version`, or a `version` query), so that
   a change is noticed.
5. **Encryption and backups** of the database are the owner's.
6. **Recordings**: each file named with its patient, start and format; the files readable by
   Medorama's account, inside `data_roots`, and not changed in place (a changed file is a new
   recording).

## What left Medorama

- Imports of every kind: the import tables, mappings and held rows; the commands `import-table`,
  `import-clif`, `add`, `remove`, `remove-encounters`, `imports`, `sources`, `connections`,
  `formats`, `demo add|remove`; the routes `tables/*`, `imports/*`, `import-tasks/*`, `sources/*`,
  `connections/*`, `mappings/*`, `formats`, `encounters/*`, `review/*`; the page's Import and
  Review tabs and the Encounters view.
- The catalog's copies of patient data: identifiers, MRNs, encounters, clinical entries,
  recordings and their links.
- Identity resolution: merges, link decisions, reviewed pairs.
- The settings `results_dir` and `formats_dirs`, the `column` criterion (no imported tables) and
  the criterion word `import` (now `database`).
- The demo cohort: the demo is now two connected databases (`demo`, made by db-builder, and
  `demo_omop`), and removing the demo is removing them.

## Build order

Each step was one piece of work with its tests and documents.

| Step | Built | Tests |
|---|---|---|
| 1. Workspace and keys | Yes | A file opens with its key only; a wrapped key moved to another owner does not unwrap. Rotation has no test yet. |
| 2. The list of databases | Yes | An offline SQLite file shows offline, then online; a user without a grant sees nothing |
| 3. Variables | Yes | A new version is read again |
| 4. The patient index | Yes | Two databases sharing patients linked exactly; a conflict listed, not linked |
| 5. Searching by database | Yes | An offline or unticked database is left out and named |
| 6. Cohorts in the workspace | Yes | Provenance recorded; a deleted cohort's key shredded |
| 7. Removal | Yes | Nothing of it found afterwards (workspace, cohorts, patients, search, the API); a crash-interrupted removal finishes |
| 8. db-builder | Yes | db-builder's own tests (inputs, recordings, the database) |
| 9. Index format 3 | Yes | Without its key a store is not read (`libs/engine/tests/store.rs`); a database's indexes go into its encrypted store and leave with it (`medorama/tests/databases.rs`); the three viewers write the same rows (`index_parity.py`) |
| 10. Derived databases | Yes (one source each) | No test of their own yet: one that finds an action's numbers by a criterion is still to write. |
| 11. Removing imports | Yes | Every GET route in Privacy Mode (`tests/privacy.rs`); the page (`tests/browser.rs`) |

The tests are in `medorama/tests/databases.rs`, `privacy.rs`, `browser.rs`, the unit tests, and
`db-builder/tests/`.

## Questions left

- Whether `variables.sqlite` counts per patient should be shown for a database whose owner
  considers them sensitive (they are aggregates; Privacy Mode's small-count rules would apply).
- How a saved cohort shows a criterion that names a removed database, so that a refresh does not
  search it.
- Whether action plug-ins should write into a database the job hands them, so that their output
  is encrypted with the rest.
- Results of two databases (`d-NNNN`): which actions need them, and how their scope is chosen.
- Per-user wrapped keys, and wrapping inside a key manager
  ([todo.md H2](../todo.md#h2-built-in-key-managers)).
