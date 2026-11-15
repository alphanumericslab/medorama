# Changelog: db-builder

## 0.2.0 (2026-11-15, pre-release)

First release of db-builder.

- The installers that build db-builder ask for the license terms to be accepted first
  (`--accept-license` in scripts). Its documentation (`docs/schema.md`) is now under
  CC BY-NC-ND 4.0; the software's license is unchanged.
- **The page (from a senior front-end and accessibility review):** an amber banner under the
  header, and the same note on the sign-in card: "This page shows the data as it is,
  identifiers included. Not for presentation." In the dialogs (New database, Save a copy, the
  file chooser) ✕ and Cancel are plain buttons, so Enter makes or chooses, never cancels. A
  preview or an add stops being followed when its tab is left (or another database opened): the
  task goes on on the server, and coming back to the tab follows it again.
- **Clinical reading (from a senior clinical-informatics review):** times from 1700 to 2500, and
  dates of birth from 1700 (shifted research datasets: MIMIC-IV's dates past 2200, MIMIC-III's
  births in the 1800s); an offset of hours alone or without its colon (`-05`, `+0530`) is read;
  a step-down, post-ICU or intermediate-care unit (`ICU step-down`, `IMCU`) no longer makes a
  visit an intensive-care stay; `RR` alone is no longer a vital sign (an ECG's RR interval has
  that name; the respiratory rate is `Resp rate`, `RR (resp)`).

- **A web interface, `db-builder serve --root DIR`** (README "Web interface"): one page on its
  own port (8091, on 127.0.0.1 by default), with its own sign-in (one account, its Argon2id hash
  in `<home>/password`, set with `db-builder serve --set-password`; `serve` refuses to start
  without it). The databases of the root, New, Open and Save a copy (`VACUUM INTO`, or SQLCipher's
  export for an encrypted one); for the one open: its tables a page at a time with a filter;
  Import (tables, exports, CLIF folders, with a preview) and Recordings (a folder with a patients
  table or a pattern, with a preview), run as tasks on a thread of their own whose progress the
  page shows; the database's rewrite rules (`<name>.rules.toml` beside it, checked before saved,
  applied to every add from the page) and Rewrite of its paths; Check; Sources with Remove; Info.
  Every path is confined to the root (links resolved; `..`, absolute paths elsewhere and links
  leading out refused; files rows name outside it left out); sessions in HttpOnly, SameSite=Strict
  cookies with an idle time; changes need the header `X-DB-Builder: 1`; failed sign-ins are
  limited; a log of actions in `<home>/db-builder.log`. With `--encrypt`, it opens and makes
  encrypted databases. Medorama can run it (`db_builder_root`). New dependencies, all used by
  Medorama already: `axum`, `tokio`, `argon2`, `getrandom`, `include_dir`, `mime_guess`,
  `rpassword` (tests: `tower`, `http-body-util`).
- **`add --dry-run`** (and `recordings --dry-run`): what would be added, per source (the format
  recognised, how each column is read, the counts and warnings), writing nothing: the add runs in
  a transaction that is rolled back; a database not made yet is not made. The library's
  `Options::dry_run`; `SourceReport` gains `format`, `columns` and `sample` (its first rows, for
  the web interface's preview; never printed on the command line).
- **Rewrite rules** (`--rules FILE`, TOML; README "Rewrite rules"): files' paths rewritten by
  prefix or regular expression, and `patient_id`, `mrn` and `encounter_id` by regular-expression
  replacements and case, as each row is read. **`db-builder rewrite OUT --rules FILE
  [--dry-run]`** rewrites the paths a built database lists (recordings, entries' files, sources),
  for data moved or mounted elsewhere. New dependency: `regex`.
- **SQLite 3.54.1** (rusqlite 0.42, SQLCipher 4.20): fixes RUSTSEC-2026-0459.
- **A WFDB collection is one recording**: a folder with an `index.csv`, found in a folder of
  recordings or named by a table's row, is one `recordings` row (format `WFDB collection`, its
  size its files'), not one per record inside it, as the viewers read it.
- **An entry's file is keyed `<entry key>:file`** in `recordings`, the rule a reader may rely on
  to show the file with its entry (docs/schema.md, Keys).
- **Every file a row names is in `recordings`**: a 12-lead ECG, a picture or a PDF is the row's
  entry's (in `extras`) and a `recordings` row of the row's category too, so that a reader
  (Medorama) finds every file in one table.
- **A recording header's clock time is read as UTC**, as the recording engine and the viewers
  read it: a recording's start in the database is the one its index gives.

- **db-builder**, a new tool of its own (`db-builder/`, binary `db-builder`, library
  `db_builder`): tables, known exports and folders of recordings made into one open SQLite
  database, schema version 1 ([docs/schema.md](docs/schema.md)): `meta`, `persons`, `visits`,
  `entries`, `recordings`, `extras`, `sources`, and `row_sources` for its own bookkeeping. It does
  not depend on Medorama, and its databases do not either.
- **Inputs**: CSV, Excel (`.xlsx`, `.xls`), OpenDocument (`.ods`) and Parquet tables, with the
  columns recognised as Medorama's imports recognised them (docs/importing.md), and one more
  field, `patient_id_issuer`; GE MUSE ECG metadata (`ge-muse`, `ge-muse-patient-id`); CLIF
  folders and tables (`clif-…`); formats described in TOML files (`--formats`); folders of
  recordings (WFDB records by their headers, EDF and BDF files, files an installed adapter
  reads), their patients from a table or a pattern of their names, their start and end from
  their headers. The readers are copied from Medorama's `tables.rs`, `parquet_rows.rs`,
  `formats/`, `categories.rs` and `util.rs`.
- **Commands**: `new`, `add`, `recordings`, `check` (patient IDs without an issuer, MRNs without
  an authority, identifiers given to two persons by one issuer, dangling person and visit keys,
  entries whose person is not their visit's, missing recording files; exit status 1 when it finds
  any), `remove-source`, `info`, `formats`.
- **Sources**: each file added is a source, identified by its path and fingerprint. The same file
  added again is not added twice; a changed file replaces its earlier version.
- **Encryption**: `--encrypt` writes and reads a SQLCipher database, its key the 64 hex digits in
  `$DB_BUILDER_KEY` (a raw key).
- **Builds without WFDB support** (`--no-default-features`): a WFDB record is still listed by its
  header, without its start and end.
