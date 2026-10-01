//! Moments as ISO-8601 in UTC, the form the transcripts themselves use.
//!
//! The calendar arithmetic is Howard Hinnant's civil-from-days: no dependency, no local time zone,
//! no leap seconds — the same answer on every machine. The clock is the caller's; here is only the
//! formatting, so that the engine's own stamps and the ones it translates from another agent's log
//! come out in one shape.

/// Seconds in a day.
const DAY: i64 = 86_400;
/// Days in the 400-year cycle of the Gregorian calendar.
const CYCLE_DAYS: i64 = 146_097;
/// Days from 0000-03-01 to 1970-01-01, the shift that makes March the first month and leap days
/// land at the end of the year.
const EPOCH_SHIFT: i64 = 719_468;
/// Milliseconds in a second.
const MILLIS: i64 = 1000;

/// Seconds since the epoch as `2026-09-05T09:00:00Z`.
#[must_use]
pub fn iso8601(seconds: i64) -> String {
    let days = seconds.div_euclid(DAY);
    let rest = seconds.rem_euclid(DAY);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Milliseconds since the epoch as `2026-09-05T09:00:00.123Z`: three digits of fraction always,
/// as JavaScript's `toISOString` writes them, so a session converted here and one converted by an
/// agent's own script compare as the same text.
#[must_use]
pub fn iso8601_millis(millis: i64) -> String {
    let seconds = iso8601(millis.div_euclid(MILLIS));
    let fraction = millis.rem_euclid(MILLIS);
    let whole = seconds.strip_suffix('Z').unwrap_or(&seconds);
    format!("{whole}.{fraction:03}Z")
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
