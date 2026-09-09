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
use vibememory_core::merge::jsonl::{MergeReport, SideReport, merge_jsonl};

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
    // The sentence must not promise which branch the client will show. Measured 2026-09-09 on CLI
    // 2.1.260: it resumed from the file's tail, not from the leaf `last-prompt` names — and the
    // engine had been telling the owner the opposite. A person decides which branch to rescue by
    // this sentence, so a promise here is worse than silence.
    for promise in ["is the one it will show", "will show", "you will see"] {
        assert!(
            !said.contains(promise),
            "the message may not promise the client's choice ({promise:?}): {said}"
        );
    }
    // What it must do instead: state the two facts a person can act on — where the file ends and
    // what `last-prompt` names. They may point at the same side (they do here), so requiring both
    // words would be wrong; requiring both *facts* is the point.
    assert!(
        said.contains("The file ends on the branch from"),
        "the message must say where the file ends: {said}"
    );
    assert!(
        said.contains("last-prompt"),
        "and what last-prompt names: {said}"
    );
    assert!(
        said.contains("depends on the client"),
        "and that the choice is the client's, not ours to promise: {said}"
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
