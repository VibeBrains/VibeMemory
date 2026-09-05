//! The freshness gate: the only hook that can stop somebody from typing, and therefore the one
//! that must be sure before it does.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use vibememory_cli::hook::prompt_gate::{Gate, decide};
use vibememory_cli::hook::stop::Tail;

fn tail(lines: usize) -> Tail {
    Tail {
        last_uuid: Some("aaaa".to_owned()),
        lines,
        resume_leaf: None,
        at: "2026-09-05T09:00:00Z".to_owned(),
    }
}

#[test]
fn nothing_known_about_other_machines_lets_the_prompt_through() {
    assert_eq!(decide(10, &[]), Gate::Allow);
}

#[test]
fn a_machine_that_is_behind_does_not_stop_anybody() {
    let others = vec![("gpd".to_owned(), tail(4))];
    assert_eq!(
        decide(10, &others),
        Gate::Allow,
        "its copy being old is its problem, not the typist's"
    );
}

#[test]
fn an_equal_count_lets_the_prompt_through() {
    let others = vec![("gpd".to_owned(), tail(10))];
    assert_eq!(
        decide(10, &others),
        Gate::Allow,
        "the same number of records means there is nothing to wait for"
    );
}

#[test]
fn a_machine_that_is_ahead_holds_the_prompt_back() {
    let others = vec![("gpd".to_owned(), tail(14))];
    let gate = decide(10, &others);
    assert_eq!(
        gate,
        Gate::Block {
            machine: "gpd".to_owned(),
            ahead_by: 4,
        }
    );
    let reason = gate.reason().expect("a block must explain itself");
    assert!(reason.contains("gpd"), "it must name the machine: {reason}");
    assert!(
        reason.contains("two minutes"),
        "it must say how long the wait is: {reason}"
    );
}

#[test]
fn the_machine_furthest_ahead_is_the_one_named() {
    let others = vec![
        ("gpd".to_owned(), tail(12)),
        ("laptop".to_owned(), tail(20)),
        ("old".to_owned(), tail(3)),
    ];
    assert_eq!(
        decide(10, &others),
        Gate::Block {
            machine: "laptop".to_owned(),
            ahead_by: 10,
        }
    );
}

#[test]
fn an_empty_local_transcript_is_not_a_reason_to_block_on_its_own() {
    // A session that has just started has no lines yet, and no other machine knows it either.
    assert_eq!(decide(0, &[]), Gate::Allow);
    // But a session somebody else has been working on does block, and that is the whole point.
    let others = vec![("gpd".to_owned(), tail(7))];
    assert!(matches!(decide(0, &others), Gate::Block { .. }));
}
