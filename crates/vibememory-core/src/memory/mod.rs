//! Memory as records.
//!
//! Claude Code writes memory as markdown files and reads them back; every other agent has its own
//! idea of what memory is. So the file is not the source here — the record is, and the files under
//! `memory/` are a projection the engine writes and re-reads. That is what lets one memory be
//! shared by two machines and, later, by several agents at once.

pub mod error;
pub mod journal;
pub mod markdown;
pub mod record;

pub use error::MemoryError;
pub use journal::{Action, Entry, Event, Memory, fold, parse};
pub use record::{Record, RecordId, RecordKind, RecordStatus};
