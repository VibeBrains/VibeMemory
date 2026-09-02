//! Core of `VibeMemory`: pure logic without I/O.
//!
//! Everything here takes strings and values and returns values: no file system, no processes,
//! no environment (enforced by `clippy.toml` at the workspace root). The CLI crate performs the
//! I/O and feeds the results in; the fixtures under `fixtures/` are recordings of real formats
//! and real tool output.

pub mod naming;
