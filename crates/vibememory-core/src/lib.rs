//! Core of `VibeMemory`: pure logic without I/O.
//!
//! Everything here takes strings and values and returns values: no file system, no processes,
//! no environment (enforced by `clippy.toml` at the workspace root). The CLI crate performs the
//! I/O and feeds the results in; the fixtures under `fixtures/` are recordings of real formats
//! and real tool output.

pub mod claim;
pub mod desktop;
pub mod export;
pub mod jsonl;
pub mod links;
pub mod memory;
pub mod merge;
pub mod naming;
pub mod push_refusal;
pub mod team_store;
pub mod terminal;
pub mod token;

/// Longest file name APFS and NTFS accept, in bytes of UTF-8. Every name the engine invents — a
/// store directory, a quarantined version — has to fit, or the checkout breaks on the other
/// machine instead of here.
pub const MAX_FILE_NAME_BYTES: usize = 255;
