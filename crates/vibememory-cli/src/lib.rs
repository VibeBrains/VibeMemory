//! The engine: everything that touches this machine.
//!
//! The core decides, this crate acts. Here live the hooks the CLI calls, the tick that talks to
//! git, and `install` / `doctor` / `status`. Every rule worth testing belongs on the other side
//! of the boundary, in `vibememory-core`, where it can be checked without a file system.
//!
//! The binary is a thin shell over this library: what the engine does has to be testable without
//! spawning a process.

// This crate does I/O by design: it is the half of the engine the purity gate exists to keep the
// core away from.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

pub mod agents;
pub mod clock;
pub mod config;
pub mod connect;
pub mod credentials;
pub mod desktop_store;
pub mod dir_link;
pub mod foreign_session;
pub mod forget;
pub mod git;
pub mod guard;
pub mod held;
pub mod hook;
pub mod install;
pub mod links_file;
pub mod local_only;
pub mod managed;
pub mod mcp_config;
pub mod memory;
pub mod merge_driver;
pub mod merge_report;
pub mod migrate;
pub mod mirror;
pub mod outbox;
pub mod personal_connect;
pub mod process;
pub mod project;
pub mod project_move;
pub mod registrations;
pub mod relink;
pub mod route;
pub mod rules;
pub mod sha256;
pub mod store;
pub mod stores;
pub mod switch;
pub mod team_connect;
pub mod team_ops;
pub mod tick;
pub mod update;
pub mod user_path;
