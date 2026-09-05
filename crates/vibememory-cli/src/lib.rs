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

pub mod config;
pub mod git;
pub mod install;
pub mod store;
