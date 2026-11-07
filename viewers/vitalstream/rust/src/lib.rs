//! The VitalStream viewer (Rust): a web server implementing the viewer API, version 1
//! (`viewers/vitalstream/docs/viewer-api.md`), with the web page embedded, over the recording
//! engine (the crate `recording-engine`, which indexes and reads the recordings).  The command
//! is `vitalstream-rs` (`src/main.rs`).
//!
//! | Module | Contents | Python viewer |
//! |---|---|---|
//! | [`server`] | HTTP API and sign-in | `server.py` |
//! | [`settings`], [`auth`] | settings files, password (`auth_none.rs` in a build without the `password` feature) | `settings.py`, `auth.py` |
//! | [`web`] | the embedded page (`viewers/vitalstream/web/`) | |

#[cfg(feature = "password")]
pub mod auth;
#[cfg(not(feature = "password"))]
#[path = "auth_none.rs"]
pub mod auth;
pub mod server;
pub mod settings;
pub mod web;
