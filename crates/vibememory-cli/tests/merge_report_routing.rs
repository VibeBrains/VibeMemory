//! Where a merge report goes: one line to the log, the parts that change what a person does to
//! the session, the rest to `doctor`.

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

use std::collections::BTreeSet;
use std::fs;

use support::TempDir;
use vibememory_cli::merge_report::{
    LOG_FILE, append_to_log, clear_pending, duplicate_message_ids, read_pending, route,
    save_pending,
};
use vibememory_core::merge::jsonl::{Fork, MergeReport, Side, SideReport, merge_jsonl};

const PATH: &str = "projects/Project/session.jsonl";

fn record(uuid: &str, at: &str) -> String {
    format!("{{\"type\":\"user\",\"uuid\":\"{uuid}\",\"timestamp\":\"{at}\"}}\n")
}

fn empty_report() -> MergeReport {
    MergeReport {
        base_truncated: 0,
        ours: SideReport::default(),
        theirs: SideReport::default(),
        conflicts: Vec::new(),
        opaque: 0,
        before_boundary: 0,
        result_equals_ours: true,
        fork: None,
    }
}

#[test]
fn an_ordinary_merge_says_one_line_to_the_log_and_nothing_to_anybody_else() {
    let routed = route(PATH, &empty_report(), &BTreeSet::new());
    assert!(routed.log.contains(PATH), "the log always gets a line");
    assert!(
        routed.session.is_empty(),
        "a merge that changes nothing for a person must not speak: {:?}",
        routed.session
    );
    assert!(routed.doctor.is_empty(), "{:?}", routed.doctor);
}

#[test]
fn a_fork_is_told_to_the_session_because_it_changes_what_a_person_reads() {
    let base = record("shared", "2026-09-05T08:00:00Z");
    let ours = format!("{base}{}", record("ours", "2026-09-05T08:00:01Z"));
    let theirs = format!("{base}{}", record("theirs", "2026-09-05T08:00:02Z"));
    let merged = merge_jsonl(base.as_bytes(), ours.as_bytes(), theirs.as_bytes()).expect("merge");

    let routed = route(PATH, &merged.report, &BTreeSet::new());
    assert_eq!(routed.session.len(), 1, "{:?}", routed.session);
    let said = &routed.session[0];
    assert!(
        said.contains("/rewind"),
        "a person needs to be told how to reach the other branch: {said}"
    );
    // The sentence may not promise a branch on `visible`, which is what it used to do: measured
    // 2026-09-09 and again 09-10 on CLI 2.1.260, a resume goes to the file's tail on all three
    // paths, never to the leaf `last-prompt` names. A person decides which branch to rescue by
    // this sentence.
    for promise in ["is the one it will show", "will show", "you will see"] {
        assert!(
            !said.contains(promise),
            "the message may not promise the client's choice ({promise:?}): {said}"
        );
    }
    // What it must say instead: where the file ends, that a resume goes there, and the scope of
    // that claim. The version is part of the fact — the old message was wrong precisely because
    // it stated behaviour with no measurement behind it.
    assert!(
        said.contains("The file ends on the branch from"),
        "the message must say where the file ends: {said}"
    );
    assert!(
        said.contains("a resume continues from there"),
        "and that a resume goes there, which is the measured fact: {said}"
    );
    assert!(
        said.contains("2.1.260"),
        "and the version the claim was measured on: {said}"
    );
    // The old hedge is now a false statement, not a modest one: three paths gave one answer.
    assert!(
        !said.contains("depends on the client"),
        "the client's choice is measured, so the hedge may not come back: {said}"
    );
    // `last-prompt` still gets named — it is a real fact about the file — but only alongside what
    // it is not, so nobody reads it as the resume point again.
    assert!(
        said.contains("last-prompt"),
        "and what last-prompt names: {said}"
    );
    assert!(
        said.contains("not where a resume goes"),
        "named together with what it is not: {said}"
    );
}

#[test]
fn the_message_names_the_file_tail_even_when_last_prompt_points_the_other_way() {
    // The fork that matters is the one where the two facts disagree — that disagreement is the
    // whole subject of the measurement, and in an ordinary two-record fork the sides coincide, so
    // a gate built on one cannot tell them apart. Built as a report rather than as a transcript:
    // what merge computes is gated by the merge fixtures, what routing says about it is gated
    // here.
    let mut report = empty_report();
    report.fork = Some(Fork {
        tail_side: Side::Ours,
        visible: Some(Side::Theirs),
    });

    let routed = route(PATH, &report, &BTreeSet::new());
    assert_eq!(routed.session.len(), 1, "{:?}", routed.session);
    let said = &routed.session[0];
    assert!(
        said.contains("The file ends on the branch from Ours"),
        "the resume point is the file's tail, and the message must name that side: {said}"
    );
    assert!(
        said.contains("`last-prompt` names the branch from Theirs"),
        "the other side is named as what `last-prompt` says, not as the resume point: {said}"
    );
}

#[test]
fn a_fork_with_no_last_prompt_still_reads_as_a_sentence() {
    // `visible` is None whenever the leaf sits in the common prefix. The clause that follows it
    // used to be glued on regardless, which produced "no `last-prompt` singles out either branch,
    // which is not where a resume goes" — a sentence that says the opposite of the truth.
    let mut report = empty_report();
    report.fork = Some(Fork {
        tail_side: Side::Theirs,
        visible: None,
    });

    let said = &route(PATH, &report, &BTreeSet::new()).session[0];
    assert!(
        said.contains("No `last-prompt` singles out either branch."),
        "{said}"
    );
    assert!(
        !said.contains("branch, which is not where a resume goes"),
        "the disclaimer belongs to a named branch, not to the absence of one: {said}"
    );
}

#[test]
fn dropped_and_opaque_lines_are_doctor_material_not_session_material() {
    let mut report = empty_report();
    report.ours.dropped = 3;
    report.theirs.resurrected = 1;
    report.opaque = 7;

    let routed = route(PATH, &report, &BTreeSet::new());
    assert!(
        routed.session.is_empty(),
        "none of this changes what a person would type next: {:?}",
        routed.session
    );
    assert_eq!(routed.doctor.len(), 3, "{:?}", routed.doctor);
    assert!(routed.doctor.iter().any(|line| line.contains("dropped")));
    assert!(
        routed
            .doctor
            .iter()
            .any(|line| line.contains("not honoured"))
    );
    assert!(
        routed
            .doctor
            .iter()
            .any(|line| line.contains("format has moved"))
    );
}

#[test]
fn records_sharing_a_message_id_are_found_and_reported() {
    // Measured on 2.1.258 and 2.1.232: the reader collapses records with one message id, so one
    // branch disappears for it although the file holds both.
    let bytes = b"{\"uuid\":\"a\",\"message\":{\"id\":\"m1\"}}\n\
                  {\"uuid\":\"b\",\"message\":{\"id\":\"m1\"}}\n\
                  {\"uuid\":\"c\",\"message\":{\"id\":\"m2\"}}\n";
    let duplicates = duplicate_message_ids(bytes);
    assert_eq!(duplicates.len(), 1);
    assert!(duplicates.contains("m1"));

    let routed = route(PATH, &empty_report(), &duplicates);
    assert_eq!(routed.session.len(), 1);
    assert!(
        routed.session[0].contains("message id"),
        "{}",
        routed.session[0]
    );
}

#[test]
fn a_file_where_every_message_id_is_its_own_reports_nothing() {
    let bytes = b"{\"uuid\":\"a\",\"message\":{\"id\":\"m1\"}}\n\
                  {\"uuid\":\"b\",\"message\":{\"id\":\"m2\"}}\n\
                  {\"uuid\":\"c\"}\n";
    assert!(duplicate_message_ids(bytes).is_empty());
}

#[test]
fn a_note_from_the_tick_waits_for_a_session_and_is_said_once() {
    let temp = TempDir::new("report-pending");
    let engine = temp.dir("engine");
    let mut report = empty_report();
    report.before_boundary = 4;
    let routed = route(PATH, &report, &BTreeSet::new());
    assert_eq!(routed.session.len(), 1);

    // The tick merges with nobody watching; the note has to survive until a session exists.
    save_pending(&engine, &routed).expect("save");
    save_pending(&engine, &routed).expect("save again");
    assert_eq!(
        read_pending(&engine).len(),
        1,
        "the same note twice is still one thing to say"
    );

    clear_pending(&engine).expect("clear");
    assert!(
        read_pending(&engine).is_empty(),
        "once said, it must not be said again on every start"
    );
}

#[test]
fn the_log_keeps_every_merge_including_the_dull_ones() {
    let temp = TempDir::new("report-log");
    let engine = temp.dir("engine");
    append_to_log(&engine, "2026-09-05T12:00:00Z", "first").expect("first");
    append_to_log(&engine, "2026-09-05T12:02:00Z", "second").expect("second");

    let text = fs::read_to_string(engine.join("log").join(LOG_FILE)).expect("read log");
    assert_eq!(text.lines().count(), 2, "a log that forgets is not a log");
    assert!(text.contains("2026-09-05T12:02:00Z second"));
}
