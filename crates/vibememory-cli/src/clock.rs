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

/// Seconds in a day.
const DAY: i64 = 86_400;
/// Days in the 400-year cycle of the Gregorian calendar.
const CYCLE_DAYS: i64 = 146_097;
/// Days from 0000-03-01 to 1970-01-01, the shift that makes March the first month and leap days
/// land at the end of the year.
const EPOCH_SHIFT: i64 = 719_468;

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

/// Formats seconds since the epoch as ISO-8601 UTC.
///
/// The calendar arithmetic is Howard Hinnant's civil-from-days: no dependency, no local time
/// zone, no leap seconds — the same answer on every machine.
#[must_use]
pub fn iso8601(seconds: i64) -> String {
    let days = seconds.div_euclid(DAY);
    let rest = seconds.rem_euclid(DAY);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Days since the epoch → year, month, day.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + EPOCH_SHIFT;
    let era = shifted.div_euclid(CYCLE_DAYS);
    let day_of_era = shifted.rem_euclid(CYCLE_DAYS);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    // March is month 0 in this arithmetic; shift it back to January.
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}
