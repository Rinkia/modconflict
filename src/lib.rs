//! ModConflict: scan a game mod folder and report conflicts before the game
//! crashes.
//!
//! The crate is split into a library and a thin `main.rs` binary. The library
//! is the whole engine — everything except command-line parsing — so it can be
//! fuzzed (`cargo-fuzz` needs a `lib` target), driven from integration tests in
//! `tests/`, and embedded by other tools. `main.rs` only turns command-line
//! flags into a call to [`analyze::run`] and prints the [`report`].

// Public engine surface, used by the binary and available to embedders.
pub mod analyze;
pub mod baseline;
pub mod ignore;
pub mod manager;
pub mod model;
pub mod profile;
pub mod report;
pub mod tui;

// Internal machinery. Cross-referenced within the crate; not part of the API.
mod bg3;
mod conflict;
mod container;
mod hash;
mod loadorder;
mod parse;
mod records;
mod scan;
mod versionreq;

// The untrusted-input parsers and their guards. Private in a normal build;
// made public only under the `fuzzing` feature so the harnesses in fuzz/ can
// call them directly. Not part of the stable API either way.
#[cfg(not(feature = "fuzzing"))]
mod limits;
#[cfg(feature = "fuzzing")]
pub mod limits;
#[cfg(not(feature = "fuzzing"))]
mod value;
#[cfg(feature = "fuzzing")]
pub mod value;

// Test-only support and suites.
#[cfg(test)]
mod corpus;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod hostile;
#[cfg(test)]
mod pipeline_tests;
#[cfg(test)]
mod snapshot;
// Only the record-scale benchmark (records feature) uses the counting
// allocator, so it is compiled only there — otherwise its helpers are dead code
// under `-D warnings` in the no-records build.
#[cfg(all(test, feature = "records"))]
mod testmem;
#[cfg(test)]
mod testutil;
