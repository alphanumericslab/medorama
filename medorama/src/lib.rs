//! Medorama: a database (the catalog) of patients, encounters, EHR items, recordings and
//! files, filled from imported databases, with a job queue for actions (indexing and plug-ins),
//! a command-line tool and a web interface.  Use and administration:
//! `docs/user-guide.md`, `docs/administration.md`; the API: `docs/api.md`; how the code fits
//! together: `docs/developing.md`.
//!
//! | Module | Contents |
//! |---|---|
//! | [`config`] | settings (`medorama.toml`) |
//! | [`audit`] | the audit log's seals: tamper evidence, verification, the export file |
//! | [`db`] | the database (SQLite or PostgreSQL), migrations, audit log, copying to PostgreSQL |
//! | [`sql`] | one interface over SQLite and PostgreSQL: the dialect's translation, values, TLS |
//! | [`sources`] | connected databases (OMOP, mapped SQL): views, read-only pools, the person registry |
//! | [`signin`] | how users sign in (passwords, or an institution's authenticating proxy): the one place that knows |
//! | [`users`] | users, roles, PHI permission, sessions |
//! | [`recordings`] | import, re-check, status, removal of recordings |
//! | [`linkage`] | patients, encounters, bed stays; linking recordings to encounters |
//! | [`ehr`] | EHR items from CSV files, matched by encounter, encounter number or patient and time |
//! | [`imports`] | batches and sources: what each import added, removing them again |
//! | [`identify`] | the identifiers recordings hold in their own files (PHI) |
//! | [`identity`] | patient identity: MRNs and their authorities, generated IDs, values per batch, merges |
//! | [`review`] | the Review tab: held-back rows, possible duplicate patients, recordings whose files disagree |
//! | [`search`] | the search syntax (`field:value`) |
//! | [`query`] | encounter search, details, per-user history |
//! | [`actions`] | built-in and plug-in actions |
//! | [`jobs`] | the job queue and its workers |
//! | [`addons`] | the installed adapters: listing, installing, removing |
//! | [`tables`] | importing a database (CSV, Excel): the one way data and files come in |
//! | [`bulk`] | an import's chunks: patients and encounters decided together and written as sets |
//! | [`progress`] | an import's progress, and the imports the server runs in the background |
//! | [`typed`] | each database's columns as numbers, for criteria on them |
//! | [`formats`] | known exports (GE MUSE ECG metadata, CLIF ...), recognised and converted on import |
//! | [`parquet_rows`] | Parquet tables read row by row, as CSV ones |
//! | [`databases`] | the Databases tab: each database as a table, its columns, exports |
//! | [`patient`] | the patient list and page: summary, journey, report, detail, trends, attached files |
//! | [`categories`] | the clinical categories (journey columns) and the words that name them |
//! | [`derived`] | series computed from recordings (a Holter's heart rate) |
//! | [`cohorts`] | cohorts: criteria (structured), their evaluation across sources, members, sharing, exports |
//! | [`expr`] | criteria written as text, parsed into the structured form |
//! | [`gate`] | the privacy gate: what leaves the server for a session in Privacy Mode |
//! | [`deid`] | Privacy Mode's settings, moving dates, de-identified answers and files |
//! | [`dicom`] | DICOM files as Privacy Mode sends them: the header rewritten, the pixels copied |
//! | [`pseudonyms`] | how Privacy Mode names patients and encounters |
//! | [`display`] | how the page writes dates and times |
//! | [`xlsx`] | a minimal Excel writer (exports) |
//! | [`util`] | time zones, time parsing and formatting |
//! | [`server`] | HTTP API (`api/v1`), the web page, Privacy Mode sessions |
//! | [`viewer`] | the viewer the catalog runs: which, starting, restarting |
//! | [`gateway`] | the viewer under `/viewer/`, behind the catalog's sign-in and rules |

// Rows read as tuples mirror the SQL they come from.
#![allow(clippy::type_complexity)]

pub mod actions;
pub mod addons;
pub mod audit;
pub mod bulk;
pub mod categories;
pub mod cohorts;
pub mod config;
pub mod databases;
pub mod db;
pub mod deid;
pub mod demo;
pub mod derived;
pub mod dicom;
pub mod display;
pub mod ehr;
pub mod expr;
pub mod formats;
pub mod gate;
pub mod gateway;
pub mod identify;
pub mod identity;
pub mod imports;
pub mod jobs;
pub mod linkage;
pub mod parquet_rows;
pub mod patient;
pub mod progress;
pub mod pseudonyms;
pub mod query;
pub mod recordings;
pub mod review;
pub mod search;
pub mod server;
pub mod signin;
pub mod sources;
pub mod sql;
pub mod tables;
pub mod typed;
pub mod users;
pub mod util;
pub mod viewer;
pub mod xlsx;
