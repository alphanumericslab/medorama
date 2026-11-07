//! The recording engine: everything needed to show and process a recording quickly, without a
//! server around it. Used by Medorama (`medorama`) and the Rust waveform viewer
//! (`vitalstream-rs`); the C++ and Python viewers implement the same engine
//! (docs/architecture.md, viewers/vitalstream/docs/index-format.md).
//!
//! | Module | Contents |
//! |---|---|
//! | [`sources`] | reading recordings: WFDB folders here, other formats through [`adapters`] |
//! | [`adapters`] | programs that read other formats: finding them, and the protocol they speak |
//! | [`common`] | pieces, envelopes, vital-sign series, alarms, placement in time |
//! | [`index`] | the index: key, building, loading |
//! | [`dataset`] | window, vital-sign and marker queries on an indexed recording |
//! | [`paths`] | paths as users write them (`~`, relative), Windows' canonical paths |

// `!(x > 0.0)` and the like deliberately treat NaN as the Python implementation does.
#![allow(clippy::neg_cmp_op_on_partial_ord)]
// Nested maps mirror the Python implementation's data structures.
#![allow(clippy::type_complexity)]

pub mod adapters;
pub mod common;
pub mod dataset;
pub mod index;
pub mod paths;
pub mod sources;
