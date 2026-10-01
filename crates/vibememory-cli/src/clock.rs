//! The only place the engine looks at a clock.
//!
//! The core takes every timestamp from its caller, so that its rules can be tested without one.
//! This module is that caller's clock, and it produces exactly one shape: ISO-8601 in UTC, the
//! form the transcripts themselves use.
//!
//! Timestamps here are for reading, never for deciding: two machines disagree about the time by
//! seconds or minutes, and any rule that compared them across machines would be wrong on the day
//! it mattered.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, as `2026-09-05T09:00:00Z`.
#[must_use]
pub fn now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        });
    iso8601(seconds)
}

/// Formats seconds since the epoch as ISO-8601 UTC: the arithmetic is the core's, shared with the
/// stamps it translates from other agents' logs.
#[must_use]
pub fn iso8601(seconds: i64) -> String {
    vibememory_core::utc::iso8601(seconds)
}
