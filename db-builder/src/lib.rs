//! db-builder: tables (CSV, spreadsheets, Parquet), known exports (GE MUSE ECG metadata, CLIF, a
//! site's export described in TOML) and folders of recordings made into one open SQLite database
//! (docs/schema.md).  The database is not specific to Medorama: any SQLite program reads it.
//!
//! | Module | Contents |
//! |---|---|
//! | [`db`] | the schema, opening (SQLCipher with `--encrypt`), `meta`, sources, fingerprints |
//! | [`tables`] | reading tables row by row; the recognised columns (docs/importing.md) |
//! | [`formats`] | known exports, converted into tables first |
//! | [`add`] | a table's rows written as persons, visits, entries, recordings and extras |
//! | [`recordings`] | a folder of recordings, with their patients from a table or a pattern |
//! | [`check`] | `check` and `info` |
//! | [`rules`] | rewrite rules: files' paths and identifiers changed as they are read (`--rules`) |
//! | [`root`] | the root folder of `serve`: every path the web interface takes, confined to it |
//! | [`serve`] | `db-builder serve`: the web interface, its sign-in and its tasks |
//! | [`categories`], [`util`], [`parquet_rows`] | categories; times and dates; Parquet rows |

pub mod add;
pub mod categories;
pub mod check;
pub mod db;
pub mod formats;
pub mod parquet_rows;
pub mod recordings;
pub mod root;
pub mod rules;
pub mod serve;
pub mod tables;
pub mod util;
