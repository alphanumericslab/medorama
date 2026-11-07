# Changelog: medorama

## Unreleased

- **A build without WFDB support**: the Cargo feature `wfdb` (on by default; the engine's), so
  that `cargo build -p medorama --no-default-features` (or `tools/install-medorama.sh --no-wfdb`,
  which also builds its viewer so) contains no wfdb-x code. Such a build reads every other format,
  and refuses a WFDB recording: an import lists it among the skipped files with "this build has no
  WFDB support (built without the `wfdb` feature)", and does not add it; `serve` says so when it
  starts. The ECG viewer's own WFDB reader and Privacy Mode's WFDB header rewriting need no wfdb-x
  and stay. The tests on WFDB recordings are ignored without the feature;
  `without_wfdb_a_wfdb_recording_is_refused_and_edf_is_added` (`tests/catalog.rs`) checks the
  refusal. CI: the `nowfdb` job (`medorama.yml`); `make test-nowfdb`.
- License: the PolyForm Noncommercial License 1.0.0 (`LICENSE`), with commercial licenses by
  agreement (`COMMERCIAL.md`) and a Contributor License Agreement (`CLA.md`), in place of MIT or
  Apache-2.0.
- Sign-in in one module (`signin.rs`), with a second method: `sign_in = "proxy"`, the
  institution's authenticating reverse proxy naming the user in a header (`proxy_user_header`,
  believed only from `proxy_addresses`; `proxy_logout_url`). `GET sign-in` tells the page the
  method; `medorama user add --no-password` (and `POST users` without a password) for accounts
  that sign in through the proxy.
- EDF, EDF+, BDF and BDF+ recordings (the engine's new reader); the EEG and Sleep categories;
  `identify` reads EDF headers' patient and recording fields; in Privacy Mode, EDF files are sent
  de-identified and EDF+ annotations in VitalStream are treated as notes.
- The warning by a viewer's tab for a file that may show identifiers is amber (red is for
  alarms).
- Recordings: a WFDB record (`.hea`), a multi-segment record, and a folder of WFDB records with
  its subfolders, as the engine reads them (index format version 2: recordings are indexed
  again).
- A table row whose file is a WFDB header (`.hea`) brings in that record, no longer its whole
  folder. A column mapping (TOML) may give its keys at the top level as well as in `[columns]`
  (a file without `[columns]` used to be ignored).
- CLIF: every table of CLIF 2.1 (33 formats with five tables of 3.0), each recognised by its
  columns; susceptibilities placed on their cultures; a medication dose not given (stopped,
  held) imported as the event, without a dose.
- The ECG and PACS viewers' pages are built in file by file: their examples, tests and Python
  packages are neither built in nor served.
- The gateway passes the viewer's new `scan` (refused in Privacy Mode) and `cancel` requests,
  each with its rule; the viewer Medorama runs must support a password (`require_password`).
- Changed: Rust 1.88 or later is needed to build Medorama (was 1.85).
- Changed: an action whose manifest gives no `timeout_s` is stopped after 6 hours
  (`timeout_s = 0` still means no limit).
- Changed: the Privacy Mode choice belongs to the session and is kept by the server (in the
  session's row; with proxy sign-in, for the visit, in that server's memory), no longer in the
  `medorama_privacy` cookie, which is not read: every sign-in starts in the user's default mode.
  Signing out, or a session that ends while the page is open, loads the page again, empty. The
  catalog's schema moves to version 3 at start.
- Privacy Mode searches: every search, filter and sort uses only what the session is shown
  (pseudonyms, recordings' tokens, each patient's dates as shown): the encounter and recording
  lists and their fields (`encounter:`, `patient:`, `file:`, `path:`, `admit:`, `start:`, `from`
  and `to`), the Databases tab's search, filters and sort, the patient list's order, the report
  search, and cohort criteria (notes' text, columns, dates). A search for a real identifier or
  date finds nothing. Dates typed in the page are dates as shown, compared by the server with
  each patient's own dates; the page no longer holds the date offset. Searches in Privacy Mode
  are audited (`privacy_search`).
- Privacy Mode settings: `GET deid`, and the date offset and `spread_days` in `GET config`, are
  for administrators only; `spread_days` is at least 30 (from 30 to 3,650).
- The privacy gate: dates move in more written forms (`2024/01/02`, `01/02/2024` in the
  catalog's date order, `2.1.2024`, `Jan 2, 2024`, `2 January 2024`, `20240102`; a recording
  name whose moved time falls before 1970 is written with a minus sign); a compound name is
  replaced part by part (`Smith-Jones`, `O'Brien`); recordings' paths and the folders they lie in
  are replaced in any text (`[Folder]`), error texts included; the identifier dictionary is built
  again whenever an identifier changes.
- Files in Privacy Mode: a WFDB comment that identifies the patient is removed however its key
  is written (`Patient_Name:`, `Name=`, `Birthdate -`), and the patient's names and date of
  birth are replaced in the other comments; a `.dat` file is sent only when a header of the same
  item names it; in GE MUSE XML, demographics are found with attributes and spaces in their tags,
  names and date of birth are replaced in the statements, and a file that still holds a
  patient's name, ID or date of birth is refused; DICOM applies the actions of PS3.15 Table
  E.1-1 to the attributes `dicom.rs` lists (now also descriptions, comments, protocol names,
  structured report text, graphic annotations, icon images and signatures; curves, overlays and
  group 4008 removed), scrubs the free text left, knows person names in implicit-VR files by
  their tags, and refuses sequences nested more than 32 deep.
- Around the gate: in Privacy Mode, the VitalStream viewer's error texts are replaced by a fixed
  sentence and the records a folder left out are named by number, and a failed job's error is
  `failed`; jobs' results folders are shown to administrators only; a download's file name can
  no longer break its `Content-Disposition` header.
- Sign-in: attempts are counted before the password is checked; 5 failures within 60 seconds
  lock a name from that address only, and 30 from one address lock that address; at most 4
  passwords are checked at once; a name no account can have is refused at once; `login_failed`
  keeps at most 64 characters of the name. With proxy sign-in, a header given twice names nobody,
  and an IPv4 address matches its IPv6 form (`::ffff:10.0.0.5`). Medorama warns at start when
  `secure_cookies = false` while it listens beyond this computer or signs users in through a
  proxy. The page's own checks for new data no longer keep an idle session alive. Only an
  administrator who has the PHI permission may give it.
- Paths: `data_roots` are compared by real paths (a link inside a root that leads out of it is
  outside), also for recordings found in folders, whose links are not followed; an attached
  folder's symbolic links are neither listed nor sent; a file a WFDB header names must really lie
  beside it; the recordings' readers open no file outside `data_roots`, whatever command reads
  or indexes them.
- The VitalStream gateway: a request that changes something (`/viewer/api/`) must carry
  `X-Medorama: 1` and be JSON (the viewer's page sends both); the viewer's page has a Content
  Security Policy of its own; the viewer is started again only when its process has ended, or a
  request cannot reach it and it then fails a health check, so a viewer busy with a large
  recording is no longer taken for a dead one, and requests no longer wait for each other.
- Audit log: an entry the database cannot take at once is never lost: it is kept in order,
  written before the next entry or by the next seal (which writes waiting entries first), and
  appended at once to `audit_export` (`"pending": true`). `audit --verify` and `GET audit/verify`
  also find entries and seals missing at the end of the log, and compare the seals with
  `audit_export`.
- Imports run one at a time (one that starts while another runs waits: *waiting*). A batch is
  *loading* until its import ends, and what only it holds is not shown on pages, in cohorts or
  in exports until then; on SQLite a table is committed a chunk of rows at a time, so other
  changes and the audit log are written in between. An import that fails, is cancelled or is
  stopped is removed as a whole, at once or at the next start; a CLIF folder is one import.
  Removing an import also removes the links its rows gave recordings and gives back a generated
  patient ID (`GEN-…`) it replaced; removing a connected database's batch disconnects it.
- Tables: a file separated by semicolons or tabs, a UTF-16 file and a file that is not UTF-8 are
  refused with a message that says which; a spreadsheet larger than 100 MB is refused, and a
  damaged one is an error of its import; of two columns read as the same catalog field, the first
  is, with a warning; warnings name a row by its line (and file); a table of more than 500
  columns is kept, its criteria reading its cells; a row that names a recording without an
  `encounter_id` is said in the report.
- Dates in tables: a date with slashes is read in the catalog's date order (Admin › Dates and
  times: day first for day/month/year), a date with dots day first; a year has four digits
  (`03/02/24` is refused); times before 1900 or after 2200, dates of birth before 1880 or in the
  future, and numbers of 10^11 or more (milliseconds) are refused; a Parquet timestamp not
  adjusted to UTC is a local time; CLIF's times without an offset are UTC.
- Cohort criteria: a text over 8 KiB, or nested more than 64 deep (`(`, `NOT`, `latest`), is
  refused, where it could stop the server. Values whose unit does not compare with the
  criterion's (or without a unit, against one with a unit) are left out and counted, and a
  search none of whose values compare is an error; a value without a unit needs the data to hold
  one unit; g/dL and g/L, mmol/L and µmol/L convert; `m` after a number means male. `latest` is
  each patient's latest value of that exact name across the catalog and the connected databases
  (`latest QT` never reads a QTc). An encounter without a discharge continues until now, and a
  date's end is the next midnight in the catalog's zone. Intensive-care units are recognised by
  one rule everywhere.
- Cohorts: a saved expression keeps what its names meant when it was saved, and a search says
  when they now mean something else; a refresh by a user who reads the data otherwise than the
  criteria's author (notes' text, ages, columns, expressions) is refused, and so is one of a
  subcohort or merge whose parent is no longer shared with the user; **Apply these changes**
  applies the changes previewed and ticked, and says how many others appeared since; a search
  evaluates its criteria once; results no longer depend on the order of conditions on connected
  databases; a merge of patients keeps the cohorts of both.
- Exports: every member of a cohort (no longer the first 1,000); times in UTC, marked `Z`; a cell
  that a spreadsheet would run as a formula is written after a `'` (cohort and database exports);
  in Excel, a sheet over 1,048,576 rows goes on in further sheets, and a cell over 32,767
  characters is cut with a mark.
- The page: an answer that arrives after the user has moved on (another patient, a newer search,
  another address) is dropped, never drawn over the newer one, and a newer search stops the one
  before; **Add to cohort** adds the result shown; the Patient Catalog's rows open with the
  keyboard (the name is a link); highlighting a search word no longer damages the text around it;
  the Encounters tab checks for indexing only while it is shown; durations are never written
  `1 h 60 min`. Trends keep a series per name and unit, and the report search reads only the
  patient's own items.
- Jobs and plug-ins: a heartbeat the database refuses is tried again, and a job's end is written
  again until the database takes it; Ctrl+C or SIGTERM stops the plug-ins (TERM to each one's
  process group, KILL 5 seconds later) and puts their jobs back in the queue; a job run again
  writes to a new folder (`j<id>-2/` …); a cancelled indexing stays cancelled (`failed`, *indexing
  was cancelled*) until **Index now**; installing an add-on that fails leaves the one installed
  as it was.
- The catalog's database: several processes opening a new SQLite catalog at once no longer fail
  with "database is locked" (one of them creates it); on PostgreSQL, a server opens at most 32
  connections, checks an idle one before using it, and gives each 10 seconds to connect, TCP
  keepalives and 5 minutes per statement unless the URL sets them; a URL that holds a password
  is refused; changes to patients and encounters are made one transaction at a time, so one MRN
  never names two patients; `copy-database` reads each table a range of rows at a time and builds
  the databases' tables of numbers again. The settings Medorama writes into `medorama.toml` keep
  `data_roots`.
- Connected databases: an unknown `--timezone` is refused, and schemas are plain names; a `sql`
  connection's views run as written, in the database's own SQL; a PostgreSQL `timestamp` is read
  in the connection's time zone, a `timestamptz` is an instant, and `icu` may be a boolean.
  Adding one (command or page) checks the same things: a SQLite file inside `data_roots` and not
  the catalog's own, a name not already used, before anything is written; a password given in
  the page is kept (`<home>/secrets/<name>.password`) only once the connection works, and goes
  with it.
- The ECG and PACS viewers: a DICOM encapsulated document is shown only when it is a PDF, and
  always as one (any other is listed to open elsewhere); a picture or a video is shown only under
  a picture's or a video's type; a structured report's subject context (name, ID, UID, birth
  date, accession) is never shown; a frame count is bounded by the pixel data. The ECG viewer
  hides every field whose key names the patient anywhere in it (`Full name`, `Subject ID`,
  `NHS number`, `Born` …), caps ages given in months, weeks, days or hours, moves the start in a
  de-identified view (`shift=`), applies a WFDB signal's skew, refuses a MUSE `SampleExponent`
  beyond ±6, and reads XML in one pass whatever its text.
- Installers (`tools/install-medorama.sh`, `tools/install.sh`): an option without its value is
  an error; the programs are built in the clone's `target/` (whatever `CARGO_TARGET_DIR` says)
  and checked before the installer ends; the privacy hooks are set only in a clone of this
  repository, with a warning when another `core.hooksPath` is set. `tools/install-gcc.sh` checks
  the Miniforge download against its published SHA-256.
- Windows: `tools/install.ps1` finds Python again (its variable for the Python command was
  taken for the `-Py` switch, since PowerShell names ignore case).
- `muse-py index` and `dicom-py index` write file paths with `/` on every system, so that a
  table made on Windows imports anywhere; `muse-py serve` and `dicom-py serve`, installed
  without a root, serve a `--root` on any drive on Windows; their wheels build from the sdists
  wherever these are unpacked.

## 0.1.0

First release: the catalog (SQLite or PostgreSQL, schema of this release), imported and
connected databases (OMOP CDM 5.3 and 5.4, mapped SQL views), identity across databases,
cohorts, the patient pages and the Patient Journey, Privacy Mode and the privacy gate, the
gateway to the VitalStream viewer, the ECG and PACS viewers' pages, add-ons (adapter protocol
version 1), actions and the job queue, users, roles and the PHI permission, the sealed audit
log, the HTTP API version 1 (`openapi.yaml`). The demo: invented patients in a cohort DEMO,
made by the installers' `--demo` and loaded by `init`, removed with the cohort.
