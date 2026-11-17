# Changelog: medorama

## 0.2.1 (2026-11-17, pre-release)

- **The ECG viewer reads the rates and formats of GE MUSE files strictly**, in `muse.js` and in
  the Python reader (`muse_reader_py`), which give the same result:
  - the sampling rate is the rhythm strip's `SampleBase`, never guessed: a file without a valid
    one was drawn at 500 Hz; it is now drawn **by sample number** (100 samples per 5 mm, the
    sample numbers under the paper, calipers in samples, the filters off), and `muse-py wfdb`
    writes no record of it;
  - each file is checked against a list of known GE formats (cart and 12SL program, with their
    rates and lengths; some carts record at 250 Hz): the report's new **Recording** line names
    the cart, the program, the rate and whether the format is known, and what does not fit is
    listed under the report with a code for each (a new format, a strip not 10 s long, median
    beats not 1.2 s long or at another rate, leads of different lengths, onsets whose rate is
    unknown, a `QTRGGR` that contradicts the rate …);
  - a lead without an amplitude scale, or of samples other than 16-bit, is refused (it was drawn
    with a scale of 1);
  - the blood pressure, and `GlobalRR` as RR in ms (× 4 at 250 Hz), are among the measurements.
- **Median beats.** A MUSE file's median beats are drawn under the rhythm, on paper of their own
  at the same speed, gain and scale: the twelve leads 3 × 4, with a calibration pulse on each
  row, and the P onset and offset, the QRS onset and offset and the T offset of the device's
  measurements dotted across each beat. The onsets are placed at the measurements' own rate
  (500 Hz even in the 250 Hz files). The calipers measure on the median beats too.
- **A MUSE ECG metadata database connects as it is**, through views
  ([databases.md, A MUSE ECG metadata database](../docs/databases.md#a-muse-ecg-metadata-database)):
  a SQLite database of a MUSE export's metadata (one row per XML file by MD5), the MUSE
  patient ID read as the MRN, each ECG an entry with its file, each measurement an entry that
  cohorts search.
- **The demo** has a sixth database, `demo_muse`: an ECG archive of MUSE files (one at 250 Hz,
  one without its sampling rate) and the ECG metadata database of them, connected through its
  views. Every MUSE file of the demo now holds its median beats with the P, QRS and T onsets, and
  names a known cart (MAC55, 12SL 241).

## 0.2.0 (2026-11-15, pre-release)

Medorama now connects databases instead of importing data (OMOP, SQL views, SQLite databases
built with the new db-builder), keeps a workspace per database encrypted with that database's
own key, grants access per database, stores the recordings' indexes in format 3, ships a full
synthetic demo, and has the fixes of a senior review. A 0.1.0 installation is set up again
rather than updated ([upgrading from 0.1.0](../docs/install.md#upgrading-from-010)).

- **No Dependabot pull requests:** `.github/dependabot.yml` is removed. Dependencies and actions are
  updated by hand; the weekly `cargo deny` run (`rust.yml`) reports new advisories.
- **Licenses by part, checked per file.** The software stays under the PolyForm Noncommercial
  License 1.0.0. The documentation (the `docs/` folders), the user's manual, its figures and
  the screenshots are now under CC BY-NC-ND 4.0 (`docs/LICENSE.md`; the commands, settings and
  code they show stay under the software's license), and the demo library and the viewers'
  examples under CC BY-NC 4.0 (the demo's CT series and radiographs also under the CC BY 3.0 and
  4.0 of their source collections). `REUSE.toml` gives every file's license, `LICENSES/` holds
  the texts, and CI checks every file (`reuse lint`, in `checks.yml`). The README, COMMERCIAL.md,
  CLA.md, CONTRIBUTING.md, THIRD_PARTY_LICENSES.md, every component's README and the manual's
  licenses appendix, introduction and title page say so; the appendix now credits the demo's
  image collections, and the inventory names the website's theme (Minimal, CC0 1.0).
- **The installers ask for the license terms to be accepted.** `tools/install-medorama.sh`,
  `tools/install.sh`, `viewers/vitalstream/install.sh` and their `.ps1` counterparts show a
  summary of the terms (`tools/license-notice.txt`) before they build anything, and go on only
  if the user types `y` or `yes`. `--accept-license` (`-AcceptLicense`) or
  `MEDORAMA_ACCEPT_LICENSE=yes` accepts them in scripts and CI; without a terminal and without
  either, the installer stops and says how. The acceptance is recorded in
  `~/.config/medorama/license-accepted` (Windows `%APPDATA%\medorama\license-accepted`) and asked
  again only when the terms change (`tools/license-accept.sh`, `.ps1`;
  `tools/tests/test_license_accept.py`). `medorama init` shows the same terms.
- **Upgrading from 0.1.0:** a 0.1.0 control store (recognised by its `imports` table, since its
  schema versions 1 to 3 are not this one's) is refused with what to do, instead of being called
  "newer than this program"; a `medorama.toml` without `workspace_dir` says the same.

- **Gateway, per database:** a user who views some database without its `phi` grant goes
  through the gate in the viewer too, outside Privacy Mode. Such a user opens a recording by its
  token (a recording of a database they hold `phi` on also by its path). A dataset answer opened
  by token is de-identified as a whole, by the recording's patient's own offset (a reader's own
  time keys included), and marked cleared so that its times move once. Before, the answer's
  `t0`/`t1` moved by the base offset alone. Test: `tests/gateway.rs` (every block within the
  window asked for, in shown times).
- **The privacy gate never drops a connection:** a date in free text next to a multi-byte
  character (the `°` of "37.1 °C") made the new date reader cut a character in two. That panic
  dropped every Privacy Mode answer of such a patient. Fixed, and the last layer now refuses an
  answer it cannot clear (500) instead of dropping the connection. Tests run the date reader
  and the scrubbers over every string of four pieces mixing digits, date words, separators and
  2–4-byte characters (`no_text_breaks_the_date_reader`, `no_text_breaks_the_scrubbers`).
- **Privacy and access (from a senior privacy review):**
  - **The `phi` grant decides per database:** a user with the PHI permission, Privacy Mode off,
    holding only `view` on a database, gets that database's entries, visits, recordings, files
    (de-identified, by tokens), list times, export rows and cohort reasons as Privacy Mode shows
    them for that patient (`patient/restricted.rs`, `server.rs` `file_restricted`).
  - **The gate's secret is wrapped by the master key** (`keys`, owner `gate`); a plain one is
    wrapped at first use. Key material is zeroised (`zeroize`), the master key held as one cipher.
  - **Searches use the text as shown:** a cohort's text criterion, the patient page's fuzzy
    matches and snippets, and the recordings list (token, label, format; sorted by shown times).
  - **Exports gate every text cell**; "withhold the text of notes and reports" covers every free
    text; ages above 89 test as 90 (`age 90+`).
  - **Administrators without the PHI permission** cannot withhold less in Privacy Mode, nor reset
    the password of a user with it; such a reset ends that user's sessions (`phi_password_reset`).
    The always-withheld identifiers are no longer listed as choices.
  - **More dates and identifiers in text:** two-digit years, HL7/DICOM date-times, month and year,
    ordinals, `Mar.`/`Sept`, `2024.03.11`; identifiers found whatever their case and leading
    zeros; telephone numbers, e-mail and web addresses, SSNs, street addresses and labelled MRNs
    replaced; every linked patient's names in labels, errors, summaries and reasons.
  - **A recording's answer moves by its patient's offset**, plug-in summary times under their own
    keys too; SVG texts and PDF descriptive fields removed in Privacy Mode.
  - **A random offset for new installations** (`serve` warns while the documented default is used).
  - **Jobs** shown to non-admins for the databases granted `view`, their own runs' errors only;
    grants and conflicts routes answer 404 to who may not search the database; patients' files
    `no-store`. A deadlock rebuilding the gate's dictionary is fixed.
  - **Cohorts:** an encounter criterion in intensive care also finds visits with an ICU bed stay.

- **The page (from a senior front-end, clinical UX and accessibility review):**
  - **No reload loop after sign-in:** a first screen that fails (500, 503) is reported with the
    page shown; only a failed sign-in shows the form (which reloads a page that showed a user).
  - **Cohort criteria never dropped silently:** an expression that does not parse, or a builder
    row not complete, stops the search; the count says "1 condition cannot be read: …" and
    Save/Add/Save the changes are disabled. Expressions are keyed by their trimmed text (a
    trailing space no longer dropped one). Sex is chosen as female or male.
  - **Cohorts, new data:** a redraw keeps the open panel, the ticks, the page of results and the
    field being typed in; answers for a screen left meanwhile are dropped.
  - **Journey by keyboard and screen reader:** column names are buttons (Tab, Enter/Space,
    `aria-pressed`), **Columns…** lists them as boxes to tick; the drawing is an image whose text
    says its span and columns. Months on its axis are found by date key (right with day-first
    dates), days at the zone's midnights. Critical marks are outlined (a second channel beside
    the amber); the detail names critical values (`critical` badge, bold and underlined).
  - **Privacy Mode unmistakable:** the switch keeps a word at every width ("Privacy"); in Privacy
    Mode an accent line along the top and a "Privacy Mode" mark on each search line; out of it an
    amber "identifiers shown" mark. A picture, PDF or video in a patient's detail has "May show
    identifiers as recorded" above it in Privacy Mode.
  - **Tablet width (≤ 1100 px):** the patient page scrolls; the journey keeps ≥ 320 px, the
    report ≥ 420 px.
  - **Passwords in a dialog** (password fields, `autocomplete=new-password`, at least 10
    characters), never `prompt()`: Change password and an admin's Reset password.
  - **Dialogs:** ✕ and Cancel are plain buttons (Enter no longer cancels "Choose on the server").
  - **Connect a database:** not sent twice (the form is disabled and `aria-busy` while it runs);
    the path or address is required; a database's variables never drawn for another one; an
    unknown `#/databases/99` says so. For SQL views, an optional **stays** query (stay_key,
    person_key, visit_key, start_time, end_time, unit, icu).
  - **Announced:** messages (`role=status`, errors assertive), the sign-in error (`role=alert`),
    the live counts and expressions' meanings (`aria-live`). Report modes are toggle buttons;
    the patient suggestions a list box with `aria-activedescendant`; a disabled tab is
    `aria-disabled`.
  - **Colours:** the tooltip's warning in the warn tone (no salmon red); the light busy colour
    darker (5.5:1), its badge text white.
  - **Smaller:** stay and visit days counted in Medorama's time zone; trends keep every digit
    (12345, not 12350); the allergy mark ▲ (no emoji); Privacy Mode's settings no longer offer
    names, MRNs, dates of birth and account numbers (always withheld; kept when saving), and
    the per-patient days say 30 to 3650; `openapi.yaml`: GET `/deid` is admins', `spread_days`
    minimum 30, 400/403 on PATCH `/users/{id}`, a typed PATCH `/cohorts/{id}` body, unused
    request bodies removed.
- **Clinical reading (from a senior clinical-informatics review):**
  - **OMOP concept 0:** every concept is joined on `NULLIF(id, 0)`, so "No matching concept" no
    longer hides the source value (an entry's name, its unit, a note's type).
  - **OMOP operators:** a measurement whose `operator_concept_id` is not `=` is text (`<5`), no
    longer the number 5.
  - **OMOP dates of birth:** a year of birth alone is read as 1 July, a year and month as the
    15th (ages within six months); before, such persons had no date of birth.
  - **OMOP on SQLite:** a date and time stored as Unix seconds is that moment (UTC), no longer
    shifted by the time zone's offset; a date stored so is still its local date.
  - **PostgreSQL times as text:** an offset of hours alone or without its colon
    (`2024-03-02 07:10:00-05`, `+0530`) is read; before, such times were dropped. Sessions set
    `DateStyle=ISO,YMD` and `IntervalStyle=iso_8601`, so `CAST(date AS TEXT)` no longer depends on
    the account's settings.
  - **Stays:** a new, optional canonical view `stays` (a visit's stays in its units); a db-builder
    database's bed stays and OMOP's `visit_detail` are read as stays. The journey's ICU column
    draws the stays in intensive-care units, the whole visit only when it has none.
  - **`has icu`** counts a visit in a unit named like an intensive-care unit, as the encounter
    criterion and the patient's page already did, and a stay in one.
  - **ICU units:** a step-down, post-ICU or intermediate-care unit (`ICU step-down`, `IMCU`) is no
    longer an intensive-care unit.
  - **Vital signs:** `RR` alone is no longer a vital sign (it is also an ECG's RR interval, in
    ms); the respiratory rate is `Resp rate`, `RR (resp)`. A day's vital signs are one report
    entry for the day in Medorama's time zone, no longer the UTC day.
  - **Flags:** a value flagged normal (`N`, `normal`, `WNL`) is normal whatever its range says.
  - **Diagnoses:** codes are compared without their dots and in any case (`I214` finds `I21.4`);
    names are searched only for words, not for codes.
  - **Units:** criteria convert pressures (mmHg, kPa, cmH2O), percent, temperatures (°C, °F),
    cell counts (10^9/L, G/L, 10^3/µL, K/µL), µg written `ug` or `mcg`, and read mEq/L as mmol/L
    (monovalent ions).
  - **Times:** from 1700 to 2500 (shifted research datasets: MIMIC-IV's dates past 2200,
    MIMIC-III's births in the 1800s); dates of birth from 1700.
  - **Heart rate from the ECG:** a block's threshold has a floor (a tenth of the hour's median),
    and a minute whose intervals mostly lie more than 20 % from their median is left out, so
    noise and mains hum no longer give a rate; EDF's ECG labels (`EKG`, `ECG1-ECG2`, `ECG L`,
    `ECG …`) are read in any unit.
  - **Demo data:** troponin T in ng/L is coded as high-sensitivity troponin T (LOINC 67151-1);
    hemoglobin's range is the patient's sex's (F 12-16, M 13.5-17.5 g/dL); the demo's OMOP tables
    have `operator_concept_id` and `visit_detail`'s times and care site.

- **Robustness (from a senior review):**
  - **Job queries:** the jobs index now leads with the recording (`jobs(recording, action, id)`).
    Lookups by recording were full table scans, measured at 19 ms each on 500k jobs, inside the
    write lock.
  - **Batched reads:** `recordings::get_many` opens each database's file once. Creating a run,
    claiming a job, the job list and `check` no longer reopen two SQLCipher files and re-read
    the index jobs per recording.
  - **PostgreSQL pool:** a returned connection now wakes a waiting request. Before, the 33rd
    concurrent request waited the full 30 s.
  - **Failed jobs:** a job that fails before it starts (recording gone, key unreadable) is
    marked failed. Before, it was retried every 120 s forever.
  - **Cancelled jobs:** a cancelled index job keeps its cancellation and is not indexed again
    automatically.
  - **Removed persons:** removing the persons a database no longer lists is linear, no longer
    one table scan per identifier.
  - **Databases without a version query:** a database whose version is a row count is refreshed
    at most once per `check_interval_s`, no longer at every 60 s check.
  - **Identity decisions:** patients are decided in a transaction that takes SQLite's write lock
    at once, so two refreshes no longer both decide and then fail.
  - **Privacy-gate dictionary:** it is built once for many waiting requests.
  - **Background thread:** its steps (checks, sealing, automatic indexing) survive a panic in
    any one of them.
  - **Viewer pipe:** the viewer's standard input is written by a thread of its own, so a viewer
    that stops reading can no longer block the server.
- **`GET api/v1/health`**, without a session: 200 when the control store answers and the
  background thread went round in the last two minutes, else 503. For a service manager or a
  load balancer.
- **Security (from a senior review):**
  - **Children's environment:** db-builder and the waveform viewer no longer inherit Medorama's
    secrets (the master key, passwords), as plug-ins already did not.
  - **Gateway:** it strips the gate's own headers from the viewer's answers.
  - **Proxy sign-in limits:** a new setting `trusted_proxies` counts failed sign-ins per client
    behind a reverse proxy (its `X-Forwarded-For`). Before, 30 failures from anyone locked out
    every user.
  - **Audit export:** `audit/verify` now also reports a seal the control store holds but the
    export lacks.
  - **db-builder sign-in:** it checks the password whatever the user name, so timing no longer
    reveals the name.
- **The full demo** (tools/make-demo-data.py; README.md, "Demo"; docs/administration.md, "The
  demo"): about 40 invented patients in five databases, for trying every feature: the main
  hospital (`demo`, db-builder: every category of entry, ICU recordings with breaks, a stretch
  without stated time and PVC markers, Holter ECGs, sleep studies in EDF+, routine EEGs in BDF+,
  12-lead ECGs in five formats, echo, CT, radiographs, angiography; folders of recordings added
  by a file-name pattern), a second hospital (`demo_north`: patients linked by MRN, one in lower
  case, an identity conflict, rewrite rules, a moved folder repointed with `db-builder
  rewrite`), an OMOP database (`demo_omop`), a registry read through SQL views
  (`demo_registry`) and an encrypted db-builder database (`demo_secure`, its demo key in a
  file). Its ECGs, CT series and two radiographs are built from the **demo library**
  (`tools/demo/library`): short pieces of real recordings and public CT collections,
  transformed and checked against re-identification by a maintainer-only tool kept outside the
  repository (tools/demo/README.md). The generator is deterministic for a seed (`--seed`),
  keeps the six-patient demo as `--small`, uses models alone with `--no-library`, builds the
  databases with `--db-builder PATH`, and lists every feature it shows in `FEATURES`, which
  tools/README.md's table and the tests follow. The installers' `--demo` builds the full demo.
- **`medorama init --demo` connects what the demo lists**: every database of the demo folder's
  `demo-databases.json` (name, kind, file, settings, key file), the encrypted one with its key,
  and adds the demo's `actions` folder (the example plug-in) to `actions_dirs`; a demo folder
  without that file connects `demo` and `demo_omop` as before. A new `medorama.toml` writes
  `actions_dirs` as set (`config::to_toml`), not always `["actions"]`.
- **tools/screenshots.py** accepts a demo made with the demo library (its `demo.json` says
  `"library": true`), still refuses one made with `--examples` or `--dicom`, and checks the
  demo's persons against those the generator invents (`identities()`), MRNs in any letter case.
- **The privacy hook** (tools/git-hooks/check-files) allows the demo library's raw arrays
  (`.i16`, `.u16`) in `tools/demo/library/` only, and still refuses DICOM and MUSE XML files
  there.

- **Index format 3 in Medorama** (viewers/vitalstream/docs/index-format.md, "Version 3: the
  index store"): each recording database's indexes are kept in one SQLite database in its
  workspace folder, `<workspace>/db-NNNN/index.sqlite`, encrypted with SQLCipher by that
  database's data key (`workspace::index_store`). The `index` action builds an index as version 2
  files in a private temporary folder (`db-NNNN/index.tmp/`) and packs it into the store in one
  transaction (`actions::run_index`, the engine's `index::build_into_store` and `store.rs`);
  derived series and the jobs read it there. Removing a database destroys its key first, then its
  folder with its index store: nothing of its indexes can be read again. `sweep_index` now
  removes, from each store, the indexes no recording of its database uses. Only the plug-ins'
  output folders (`db-NNNN/results/<action>/…`) are still not encrypted.
- **Removed: the setting `index_dir` and `medorama init --index-dir`.** A `medorama.toml` that
  still has `index_dir` is refused, with a message saying to remove the line; the recordings are
  then indexed again into their databases' stores (their old indexes are of version 2).
  `GET api/v1/config` no longer names `index_dir`.
- **The viewer's keys on a pipe**: the viewer Medorama runs is started with `--keys-on-stdin`;
  Medorama writes one JSON line per store on its standard input at each start (each recording
  database's store with its key, and the viewer's own store, `"own": true`), again when a
  database is connected, and `{"forget": …}` before one is removed (`viewer::store_lines`,
  `viewer::forget_line`). A key is never on a command line, in the environment, in a file or in
  a log.
- **The viewer's own store**: what the viewer indexes itself goes into
  `<workspace>/viewer/index.sqlite`, encrypted with a key of its own (owner `viewer`); the
  viewer's settings give it `cache_dir = <workspace>/viewer`.
- **Plug-ins' job description without `recording.index_dir`**: plug-ins read the recording itself
  (`recording.path`); the index stores' keys are not given to them.
- The speed of index format 3 against format 2 is measured in
  [docs/performance.md](../docs/performance.md#index-format-3): a window reads at most 5 ms more
  from an encrypted store; indexing takes 15 to 35 % longer.
- Tests: `indexes_go_into_the_databases_encrypted_store_and_leave_with_it`
  (`tests/databases.rs`); the gateway's tests check that a recording opened through the viewer is
  indexed into the viewer's own encrypted store.
- **Create a database…** (docs/administration.md, "db-builder"): with the setting
  `db_builder_root` (or `medorama serve --db-builder-root DIR`), Medorama runs db-builder's web
  interface (`db-builder serve --root DIR --host 127.0.0.1 --port <db_builder_port>`, default
  8091, its home in `<home>/db-builder`) as a child process from its start to its end, started
  again when it stops; it waits until db-builder's own password is set (`db-builder serve
  --set-password --home <home>/db-builder`) and says why it does not run. `db_builder_url` links to
  a db-builder running elsewhere instead. The Databases tab's Connect card gets **Create a
  database…** (admins), opening db-builder's page in a tab named `db-builder`; `GET api/v1/config`
  names its address (`db_builder`: mode, url, running, reason) to admins only. Medorama does not
  proxy db-builder and shares no sign-in with it. `serve` warns when `db_builder_root` is outside
  `data_roots`. New module `dbbuilder.rs`.
- **SQLite 3.54.1** (rusqlite 0.42, SQLCipher 4.20): fixes RUSTSEC-2026-0459 (CVE-2026-119375), by
  which a database file opened with a crafted `-wal` file of another page size could return
  uninitialised memory to queries; Medorama opens SQLite files others wrote (connected
  databases). Also in db-builder.
- **Medorama connects databases only; it imports nothing**
  ([docs/design/databases-only.md](../docs/design/databases-only.md)). It reads OMOP databases
  (`omop`), any SQL database through views (`views`) and db-builder SQLite databases (`dbb`) in
  place, read-only, and keeps no copy of their patient data in its control store. A recording
  database (a db-builder `recordings` table, or a `recordings` view) lists its recordings: the
  patient page attaches its ECGs, pictures, PDFs and DICOM studies for the viewers and opens its
  waveforms in VitalStream; recordings are numbered `database × 1,000,000,000 + n`. The canonical
  `persons` gains `patient_id_issuer`; a database's settings gain `issuer` and `files_root`.
- **The workspace** (`workspace_dir`, required: `medorama init --workspace DIR`; replaces
  `results_dir`): one folder per database (`db-NNNN/`: `persons.sqlite`, `variables.sqlite`,
  `recordings.sqlite`, `results-<action>.sqlite`, and the plug-ins' `results/<action>/…`) and one
  file per cohort (`cohorts/c-NNNN.sqlite`), every SQLite file encrypted with SQLCipher by its
  owner's data key. Data keys are wrapped with AES-256-GCM by the master key
  (`$MEDORAMA_MASTER_KEY`, `master_key_command` or `master_key_file`); `medorama keys new` and
  `keys rotate`. A database's password given to Medorama is sealed by the master key, never
  written to a file. SQLCipher is always built (the `encryption` Cargo feature is gone; building
  needs Perl and a C compiler). The recordings' indexes are in the workspace too, encrypted
  (index format 3, above); the files plug-ins write are not encrypted: use disk encryption.
- **Grants**: on a database `search` < `view` < `phi` < `manage`, on a cohort `view` < `edit` <
  `manage`. The admin who connects a database gets every grant on it; other admins see it listed
  and may grant it to themselves (audited, `grant_self`). Sharing a cohort is granting it.
  Identifiers need the PHI permission and the database's `phi` grant.
- **The patient index**: persons linked to patients by exact (issuer, identifier) pairs, kept as
  keyed hashes (issuers and identifiers compared ignoring case, as db-builder matches MRNs);
  identifiers only in each database's `persons.sqlite`. A person whose identifiers
  name two patients is an identity conflict, listed and never merged. Scored matching, merges,
  held rows and the Review tab are gone.
- **Status and variables**: each database is checked at start, every 60 s and on demand (green
  dot online, grey offline, never red); its variables are kept per data version and read again
  when the version changes. The Databases tab lists them, and a database's page has Check now,
  Refresh, Remove… (typing its name), Variables, Who may use it, Identity conflicts and Grant it
  to me; Connect a database replaces the Import tab.
- **Searching by database**: the Cohort tab's *Search in* ticks (offline ones greyed); a
  criterion goes only to the databases whose variables may hold it; a result names the databases
  searched and those left out, and why. A criterion names a database by `database` (`in
  hospital`, `omop.lactate`); `column` criteria are gone. The numbers of an action's summaries are
  searchable as `<action>.<field>` (`hrv.rmssd > 50`: a word with a dot whose first part begins
  no database's name is the result's whole name; quotes always are). Cohorts keep their members in the workspace, each with the
  databases that put it there (provenance), and a saved cohort the databases it searches.
- **Removing a database leaves no trace**: its key is destroyed, then its workspace folder, its
  rows in every cohort, its patient links, the patients no database links any more, its jobs,
  grants and row; a removal a crash interrupted finishes at the next start. The database itself
  is never touched.
- **db-builder** (`db-builder/`, its own README, schema and changelog) turns tables, known exports
  (GE MUSE, CLIF, declared formats) and folders of recordings into a database to connect; the
  installers build it, and their `--demo` makes the demo with it. `medorama init --demo` connects
  `demo` and `demo_omop`; the DEMO cohort and `demo add/remove` are gone.
- **Commands**: new `databases connect|list|check|refresh|remove|variables|conflicts|grants|grant|revoke`
  and `keys new|rotate`; gone `import-table`, `import-clif`, `add`, `remove`,
  `remove-encounters`, `imports`, `sources`, `demo`, `connections` and `formats`. **Routes**: new
  `databases` (connect), `databases/{id}` (remove), `/check`, `/refresh`, `/variables`,
  `/conflicts`, `/grants`; cohorts take `databases`; gone `encounters*`, `recordings/remove*`,
  `recordings/{id}/link` and `/decision`, `review*`, `databases/{id}/rows` and `/export`,
  `connections*`, `tables*`, `imports*`, `import-tasks*`, `sources*`, `demo/removal`,
  `mappings*` and `formats`. History lists patients, not encounters; the `identify` action is
  gone.
- **The control store's schema starts again at version 1**, with no migration from earlier
  catalogs (Medorama is not released yet): set it up afresh with `medorama init`.
- **Files and recordings**: an entry's file (the recording keyed `<entry key>:file`, as db-builder
  writes it) is shown with its entry on the patient's page, the journey and the report, not as a
  recording of its own. A file no reader indexes (a 12-lead ECG in DICOM or XML, a picture, a
  PDF) has the status `file` and is never queued for indexing (it was marked failed). The
  database's `format` and times are kept until indexing reads the recording's own, and taken
  again when the file changes. `medorama check` applies the same rules as a refresh (`data_roots`,
  the workspace, readability) to every recording, not only to whether it exists.
- **Encrypted SQLite databases**: a db-builder database made with `--encrypt` (or any SQLCipher
  file) connects with its key as its password (the form's *Key, if encrypted*, or
  `--password-stdin`, `--password-file`, `$MEDORAMA_SOURCE_PASSWORD_<NAME>`): 64 hex digits a raw
  key, else a passphrase. The connection pool now tells connections apart by their password, so
  a connection opened with no key or another key is never reused.
- **`keys rotate` also seals the stored passwords again** with the new master key (they were left
  sealed by the old one, so such a database stopped opening); a rotation interrupted can be run
  again. Tests: `tests/databases.rs` (`a_new_master_key_wraps_every_key_and_password`,
  `an_encrypted_database_opens_with_its_key`, `an_actions_results_are_found_by_name`).
- **Removing or refreshing a database without `manage`** answers 403 (404 to who may not search
  it), not 400. The page no longer offers *Revoke all* on one's own row of a database's grants.

- **Choose on the server**: the page picks files and folders on the server in a dialog of its
  own, which the server lists (`GET server-files?dir&kind`: folders, and tables, SQLite files or
  nothing else; inside `data_roots`, for admins, refused in Privacy Mode as the viewer's folders
  are), the way VitalStream's Open dialog does. Import › Import tables leads with **Choose tables
  on the server…**: the tables are read where they are, row by row, of any size, as
  `medorama import-table` reads them (`POST tables/server/inspect`, `POST tables/server`; a
  relative `file` path is read from the table's folder). **Upload from this computer** and
  dropping files stay, for a table that is not on the server. The CLIF folder and a connected
  database's file get a **Choose…** button. The browser's own file dialog, which may not open a
  share it would have to mount itself ("operation permitted for root only"), is no longer the way
  to a table on the server. Inspecting a table now reads it row by row and keeps only its first
  five rows, uploads included. Tests: `tests/server_tables.rs`, and `tests/browser.rs` (the
  dialog in Chrome, with the accessibility check).

- **WFDB records with MATLAB signal files** (`.hea` with `.mat`: the PhysioNet Challenge files,
  `wfdb2mat`'s) are read, once the `.mat` file is checked against its header; one that does not
  match (compressed, of doubles, transposed, of another offset or length) is refused with the
  reason ([docs/wfdb-collection.md](../docs/wfdb-collection.md#matlab-signal-files)). In Privacy
  Mode, a `.mat` file a header of the same item names is sent as its first matrix only, its
  descriptive text and the matrix's name blanked (`FileKind::MatSignal`); any other `.mat` file
  is refused. `tests/privacy.rs` holds such a file, with identifiers in its text, its name and a
  second variable.
- **A WFDB record is opened by its header**: a signal file (`.dat`, `.mat`) given to `add` or in
  a table's `file` column is skipped with the header to give in its place (`100.dat is a WFDB
  signal file: open its header, 100.hea`).
- **The ECG viewer opens a WFDB record in two steps**: **Open record…** offers the header (not the
  signal files); once it is chosen, the page names the signal file it needs and opens a second
  chooser for it (`This record's samples are in 100.dat: choose it`), refusing a file the header
  does not name; a signal file chosen or dropped alone asks for its header. Opened from an address
  (`#src`, Medorama's ECG pages), the page fetches the signal files itself, as before. It reads
  `.mat` signal files, with the engine's checks and messages, and no more samples than a header
  states. Tests: `viewers/ecg/tests/wfdb-mat.test.js`.
- **wfdb-x 1.2.1** (commit `4a91916`, was `a707ba9`, version 1.0.0): its library is now under
  the GNU LGPL v2.1 or later (its command-line tools, which Medorama does not use, stay under the
  GPL v3 or later), so the binary releases are no longer held back: `release.yml` runs on every
  version tag, without the repository variable `RELEASES_ENABLED`. `deny.toml` and
  `tools/about.toml` accept the crate's `LGPL-2.1-or-later AND GPL-3.0-or-later`;
  `THIRD_PARTY_LICENSES.md` and the manual's licenses appendix say so.
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
- PostgreSQL: an administrator's **Cancel** of jobs works (the query left a parameter out,
  which PostgreSQL refuses), and `medorama copy-database` copies a catalog into a new
  PostgreSQL catalog (it collided with the counter row the new catalog starts with).

## 0.1.0

First release: the catalog (SQLite or PostgreSQL, schema of this release), imported and
connected databases (OMOP CDM 5.3 and 5.4, mapped SQL views), identity across databases,
cohorts, the patient pages and the Patient Journey, Privacy Mode and the privacy gate, the
gateway to the VitalStream viewer, the ECG and PACS viewers' pages, add-ons (adapter protocol
version 1), actions and the job queue, users, roles and the PHI permission, the sealed audit
log, the HTTP API version 1 (`openapi.yaml`). The demo: invented patients in a cohort DEMO,
made by the installers' `--demo` and loaded by `init`, removed with the cohort.
