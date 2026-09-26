//! The MCP server: the same memory the engine keeps, offered to any agent that speaks MCP.
//!
//! It has no model of its own. The journal in the store is the truth; this server folds it to
//! answer questions and appends events to change it, exactly as the hooks do. Anything else —
//! projecting markdown, merging, deciding what a record means — belongs to the engine, and doing
//! it here would give one fact two owners.
//!
//! The layers are split so the protocol and the searching can be tested without a store: the
//! [`Memories`] trait is the only thing of the server that touches disk. The host's own commands
//! are split the same way: `shell`, `receive`, `apply` and `status` decide, each against its
//! fixture, and only `hostops` reads the disk and does what they decided.

// The server reads and writes the store, so the purity gate is lifted here; the pure parts live
// in `protocol` and `tools` and are tested through a fake `Memories`.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

pub mod access;
pub mod apply;
pub mod disk;
pub mod export;
pub mod git_memories;
pub mod host;
pub mod hostops;
pub mod http;
pub mod layout;
pub mod memories;
pub mod protocol;
pub mod push_scan;
pub mod receive;
pub mod shell;
pub mod status;
pub mod tools;

pub use memories::{Memories, StoreMemories};
pub use protocol::{Request, Response, handle};
