//! The engine's one clock format, checked against dates whose answers are known independently.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use vibememory_cli::clock::iso8601;

#[test]
fn known_moments_format_exactly() {
    // Values cross-checked with `date -u -r <seconds>`.
    for (seconds, expected) in [
        (0_i64, "1970-01-01T00:00:00Z"),
        (1, "1970-01-01T00:00:01Z"),
        (951_782_400, "2000-02-29T00:00:00Z"),
        (1_788_600_000, "2026-09-05T09:20:00Z"),
        (4_102_444_800, "2100-01-01T00:00:00Z"),
    ] {
        assert_eq!(iso8601(seconds), expected, "for {seconds}");
    }
}

#[test]
fn a_leap_day_is_a_day_like_any_other() {
    // 2100 is not a leap year, and a naive rule would put 29 February there.
    assert_eq!(iso8601(4_107_456_000), "2100-02-28T00:00:00Z");
    assert_eq!(iso8601(4_107_542_400), "2100-03-01T00:00:00Z");
}
