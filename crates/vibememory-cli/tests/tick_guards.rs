//! The two rails around the tick: a cap on deletions per run, and a back-off after failures.
//!
//! The decisions are pure, so most of this needs no store — and that is the point of them living
//! in `guard` rather than inside the tick.

// The test writes files, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use support::TempDir;
use vibememory_cli::guard::{DEFAULT_MAX_DELETIONS_PER_TICK, Deletions, TickState, deletions};

#[test]
fn the_cap_holds_every_deletion_or_none() {
    // Within the cap: nothing to think about.
    assert_eq!(deletions(0, 10, false), Deletions::Allowed);
    assert_eq!(deletions(10, 10, false), Deletions::Allowed);

    // Over it: refused as a whole, and the refusal says both numbers so the person can judge.
    // Not "the first ten": a partial deletion leaves the store in a state nobody chose, and the
    // next run would take ten more — the cap would slow an accident down instead of stopping it.
    assert_eq!(
        deletions(100, 10, false),
        Deletions::Held {
            wanted: 100,
            cap: 10
        }
    );

    // Released for one run: the person looked and said go ahead.
    assert_eq!(deletions(100, 10, true), Deletions::Allowed);
}

#[test]
fn the_back_off_starts_at_the_third_failure_and_stops_growing() {
    let fresh = TickState::default();
    assert!(fresh.store_cycle_allowed(), "a new machine has not failed");

    // Two failures could be one bad network moment; the run still tries.
    let (one, _) = fresh.after_run(true);
    assert_eq!(one.consecutive_failures, 1);
    assert!(one.store_cycle_allowed());
    let (two, _) = one.after_run(true);
    assert!(
        two.store_cycle_allowed(),
        "two in a row is not yet a pattern"
    );

    // The third is, and from then on the wait doubles.
    let (three, _) = two.after_run(true);
    assert!(!three.store_cycle_allowed());
    assert_eq!(three.runs_to_skip, 1);

    // A skipped run neither succeeds nor fails — it did not try — so it only counts down.
    let (skipped, recovered) = three.after_run(false);
    assert!(!recovered, "a run that did not try cannot have recovered");
    assert_eq!(
        skipped.consecutive_failures, 3,
        "the failures are remembered"
    );
    assert!(
        skipped.store_cycle_allowed(),
        "one run was skipped, now try again"
    );

    // Still broken: the wait grows, and it is capped rather than doubling for ever.
    let mut state = skipped;
    let mut waits = Vec::new();
    for _ in 0..8 {
        let (next, _) = state.after_run(true);
        waits.push(next.runs_to_skip);
        // Serve the whole wait, so the next failure is the next real attempt.
        state = next;
        while !state.store_cycle_allowed() {
            state = state.after_run(false).0;
        }
    }
    assert_eq!(
        &waits[..5],
        &[2, 4, 8, 15, 15],
        "doubling, then capped: {waits:?}"
    );
    assert!(
        waits.iter().all(|wait| *wait <= 15),
        "the wait must never grow past the cap: {waits:?}"
    );
}

#[test]
fn recovery_is_announced_once_and_only_after_a_real_pause() {
    // One failure, then success: nothing was ever paused, so there is nothing to announce.
    let (one, _) = TickState::default().after_run(true);
    let (back, recovered) = one.after_run(false);
    assert!(
        !recovered,
        "a single hiccup is not a recovery worth a sentence"
    );
    assert_eq!(back.consecutive_failures, 0);

    // Enough failures to pause, then a run that works: said once…
    let mut state = TickState::default();
    for _ in 0..3 {
        state = state.after_run(true).0;
    }
    while !state.store_cycle_allowed() {
        state = state.after_run(false).0;
    }
    let (healthy, recovered) = state.after_run(false);
    assert!(
        recovered,
        "a sync that silently comes back looks like one still broken"
    );

    // …and never again.
    let (_, again) = healthy.after_run(false);
    assert!(!again, "the second sentence would be noise");
}

#[test]
fn the_state_survives_a_round_trip_and_a_machine_that_never_ticked_is_clean() {
    let temp = TempDir::new("tick-state");
    let engine = temp.dir("engine");

    assert_eq!(
        TickState::read(&engine),
        TickState::default(),
        "no file means no failures, not an error"
    );

    let state = TickState {
        consecutive_failures: 4,
        runs_to_skip: 2,
        deletions_held: 37,
        ignored: Vec::new(),
        pause: Some(vibememory_cli::guard::StorePause {
            code: "quota".to_owned(),
            lines: vec!["over the plan".to_owned()],
            since: "2026-09-05T10:00:00Z".to_owned(),
            recheck_at: "2026-09-05T11:00:00Z".to_owned(),
        }),
        last_failure: Some(
            "fetch from the store's host failed: Permission denied (publickey).".to_owned(),
        ),
    };
    state.write(&engine).expect("write");
    assert_eq!(TickState::read(&engine), state);

    // The held count is what `doctor` reads, so it has to outlive the run that set it.
    assert_eq!(TickState::read(&engine).deletions_held, 37);
}

#[test]
fn the_default_cap_is_more_than_a_day_of_forgetting_and_less_than_an_accident() {
    // Not a round number for its own sake: the point is that ordinary use never meets the rail,
    // and a store-wide accident always does.
    assert_eq!(DEFAULT_MAX_DELETIONS_PER_TICK, 10);
    assert_eq!(
        deletions(3, DEFAULT_MAX_DELETIONS_PER_TICK, false),
        Deletions::Allowed,
        "forgetting a few sessions in a day must not need a decision"
    );
    assert!(
        matches!(
            deletions(500, DEFAULT_MAX_DELETIONS_PER_TICK, false),
            Deletions::Held { .. }
        ),
        "a store-wide sweep must always stop"
    );
}
