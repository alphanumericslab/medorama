//! Medorama: a research browser over connected databases (OMOP, SQL databases through views,
//! db-builder databases, recording databases), read in place and never written, with cohorts
//! across them, a job queue for actions (indexing and plug-ins), a command-line tool and a web
//! interface.  It keeps no patient data of its own: what it generates is in its workspace,
//! encrypted per database (docs/design/databases-only.md).  Use and administration:
//! `docs/user-guide.md`, `docs/administration.md`; the API: `docs/api.md`; how the code fits
//! together: `docs/developing.md`.
//!
//! | Module | Contents |
//! |---|---|
//! | [`config`] | settings (`medorama.toml`) |
//! | [`audit`] | the audit log's seals: tamper evidence, verification, the export file |
//! | [`db`] | the control store (SQLite or PostgreSQL), its schema, the audit log, copying to PostgreSQL |
//! | [`sql`] | one interface over SQLite and PostgreSQL: the dialect's translation, values, TLS |
//! | [`workspace`] | the workspace: data keys wrapped by the master key, encrypted files per database and cohort |
//! | [`databases`] | the connected databases: connecting, status, refreshing, removing every trace |
//! | [`sources`] | reading a connected database: its views (OMOP, views, db-builder), pools, filters pushed down |
//! | [`grants`] | who may search, view, see the identifiers of, or manage each database and cohort |
//! | [`persons`] | the patient index: each database's persons linked to patients by (issuer, identifier) |
//! | [`variables`] | what each database holds, kept per data version |
//! | [`signin`] | how users sign in (passwords, or an institution's authenticating proxy): the one place that knows |
//! | [`users`] | users, roles, PHI permission, sessions |
//! | [`recordings`] | the recordings of recording databases: presence, fingerprints, index metadata, series |
//! | [`search`] | the search syntax (`field:value`) |
//! | [`actions`] | built-in and plug-in actions |
//! | [`jobs`] | the job queue and its workers |
//! | [`addons`] | the installed adapters: listing, installing, removing |
//! | [`patient`] | the patient list and page: summary, journey, report, detail, trends |
//! | [`categories`] | the clinical categories (journey columns) and the words that name them |
//! | [`derived`] | series computed from recordings (a Holter's heart rate) |
//! | [`cohorts`] | cohorts: criteria (structured), their evaluation across databases, members, sharing, exports |
//! | [`expr`] | criteria written as text, parsed into the structured form |
//! | [`gate`] | the privacy gate: what leaves the server for a session in Privacy Mode |
//! | [`deid`] | Privacy Mode's settings, moving dates, de-identified answers and files |
//! | [`dicom`] | DICOM files as Privacy Mode sends them: the header rewritten, the pixels copied |
//! | [`pseudonyms`] | how Privacy Mode names patients and visits |
//! | [`display`] | how the page writes dates and times |
//! | [`xlsx`] | a minimal Excel writer (exports) |
//! | [`util`] | time zones, time parsing and formatting |
//! | [`server`] | HTTP API (`api/v1`), the web page, Privacy Mode sessions |
//! | [`viewer`] | the viewer Medorama runs: which, starting, restarting |
//! | [`gateway`] | the viewer under `/viewer/`, behind Medorama's sign-in and rules |
//! | [`dbbuilder`] | db-builder's web interface, run (or linked) by Medorama for "Create a database…" |

// Rows read as tuples mirror the SQL they come from.
#![allow(clippy::type_complexity)]

pub mod actions;
pub mod addons;
pub mod audit;
pub mod categories;
pub mod cohorts;
pub mod config;
pub mod databases;
pub mod db;
pub mod dbbuilder;
pub mod deid;
pub mod derived;
pub mod dicom;
pub mod display;
pub mod expr;
pub mod gate;
pub mod gateway;
pub mod grants;
pub mod installation;
pub mod jobs;
pub mod patient;
pub mod persons;
pub mod pseudonyms;
pub mod recordings;
pub mod search;
pub mod server;
pub mod signin;
pub mod sources;
pub mod sql;
pub mod users;
pub mod util;
pub mod variables;
pub mod viewer;
pub mod workspace;
pub mod xlsx;
