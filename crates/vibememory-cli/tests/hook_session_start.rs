//! `SessionStart` decides and acts on a real directory layout in a temporary directory.

// The test builds links and directories, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::fs;

use support::TempDir;
use vibememory_cli::hook::parse_input;
use vibememory_cli::hook::session_start::{Decision, decide, perform};
use vibememory_cli::install::Layout;
use vibememory_core::naming::{PathSyntax, StoreName, enc_from_transcript_path};

const ENC: &str = "-tmp-project";

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    }
}

fn enc() -> vibememory_core::naming::EncSlug {
    enc_from_transcript_path(
        &format!("/x/.claude/projects/{ENC}/11111111-1111-4111-8111-111111111111.jsonl"),
        PathSyntax::Posix,
    )
    .expect("enc")
}

fn name() -> StoreName {
    StoreName::parse("Project").expect("name")
}

fn link_path(layout: &Layout) -> std::path::PathBuf {
    layout.config_dir.join("projects").join(ENC)
}

#[test]
fn stdin_that_cannot_be_read_is_named_not_guessed() {
    let error = parse_input("not json").expect_err("must refuse");
    assert_eq!(error.code(), "hookInputInvalid");
    assert!(!error.message().trim().is_empty());

    // A field the CLI adds in a later version must not break the hook.
    let input = parse_input(
        r#"{"session_id":"s","transcript_path":"/x/y.jsonl","cwd":"/x","source":"startup",
            "something_new_in_2_1_300":true}"#,
    )
    .expect("unknown fields are expected");
    assert_eq!(input.source.as_deref(), Some("startup"));
}

#[test]
fn a_missing_link_is_created_and_points_into_the_store() {
    let temp = TempDir::new("hook-link");
    let layout = layout(&temp);
    let decision = decide(&layout, &layout.store(), &enc(), Some(&name()), None);
    assert!(matches!(decision, Decision::Link { .. }));
    assert_eq!(decision.additional_context(), None, "silence is normal");

    perform(&layout, &layout.store(), &decision).expect("perform");
    assert_eq!(
        fs::read_link(link_path(&layout)).expect("read link"),
        layout.store().join("projects").join("Project")
    );
    assert!(
        layout
            .store()
            .join("projects")
            .join("Project")
            .join(".keep")
            .exists(),
        "the store directory needs a .keep or git will not carry it"
    );
}

#[test]
fn an_existing_correct_link_is_left_alone_and_says_nothing() {
    let temp = TempDir::new("hook-already");
    let layout = layout(&temp);
    perform(
        &layout,
        &layout.store(),
        &decide(&layout, &layout.store(), &enc(), Some(&name()), None),
    )
    .expect("first");

    let again = decide(&layout, &layout.store(), &enc(), Some(&name()), None);
    assert!(matches!(again, Decision::AlreadyLinked { .. }));
    assert_eq!(again.additional_context(), None);
    perform(&layout, &layout.store(), &again).expect("second");
    // Silence is not the same as learning nothing: this session's transcript proves the link, and
    // the proof has to be recorded even though there was nothing to create. A machine whose links
    // were all made by the migration otherwise never confirms one, and importing or repairing a
    // Desktop card — both of which wait for a confirmed path — waits for ever.
    assert_eq!(
        again.store_name(),
        Some(name().as_str()),
        "a link that is merely found still names the store directory it points at"
    );
}

#[test]
fn a_link_pointing_elsewhere_is_never_re_aimed() {
    let temp = TempDir::new("hook-disagree");
    let layout = layout(&temp);
    let elsewhere = temp.dir("elsewhere");
    fs::create_dir_all(link_path(&layout).parent().expect("parent")).expect("projects");
    support::link_dir(&elsewhere, &link_path(&layout));

    let decision = decide(&layout, &layout.store(), &enc(), Some(&name()), None);
    let Decision::Disagreement { found, .. } = &decision else {
        panic!("expected a disagreement, got {decision:?}");
    };
    assert_eq!(found, &elsewhere.display().to_string());

    let said = decision
        .additional_context()
        .expect("the session must hear");
    assert!(
        said.contains("left exactly as it is"),
        "it must say the link was not touched: {said}"
    );
    assert!(
        !said.contains("relink"),
        "it must not name a command that does not exist yet: {said}"
    );

    perform(&layout, &layout.store(), &decision).expect("perform");
    assert_eq!(
        fs::read_link(link_path(&layout)).expect("read link"),
        elsewhere,
        "the link must be exactly as it was"
    );
}

#[test]
fn a_real_directory_with_transcripts_is_left_for_the_tick() {
    let temp = TempDir::new("hook-import");
    let layout = layout(&temp);
    let real = link_path(&layout);
    fs::create_dir_all(&real).expect("real dir");
    fs::write(real.join("old-session.jsonl"), b"{}\n").expect("transcript");

    let decision = decide(&layout, &layout.store(), &enc(), Some(&name()), None);
    assert!(matches!(decision, Decision::ImportNeeded { .. }));
    assert!(
        decision
            .additional_context()
            .expect("the session must hear")
            .contains("nothing is lost")
    );

    perform(&layout, &layout.store(), &decision).expect("perform");
    assert!(
        real.join("old-session.jsonl").exists(),
        "a transcript nobody imported may not be destroyed"
    );
    assert!(
        !real.is_symlink(),
        "the directory must stay a directory until the tick has copied it"
    );
}

#[test]
fn an_empty_real_directory_becomes_a_link() {
    let temp = TempDir::new("hook-empty");
    let layout = layout(&temp);
    let real = link_path(&layout);
    // This is what the CLI leaves behind for a working directory it has seen but not written to:
    // the directory and a `memory` subdirectory, no transcript.
    fs::create_dir_all(real.join("memory")).expect("real dir");

    let decision = decide(&layout, &layout.store(), &enc(), Some(&name()), None);
    assert!(matches!(decision, Decision::Link { .. }), "{decision:?}");
    perform(&layout, &layout.store(), &decision).expect("perform");
    assert!(real.is_symlink(), "an empty directory may be replaced");
}

#[test]
fn an_ignored_working_directory_produces_nothing() {
    let temp = TempDir::new("hook-ignored");
    let layout = layout(&temp);
    let decision = decide(
        &layout,
        &layout.store(),
        &enc(),
        None,
        Some("ignoreCwd matched /"),
    );
    assert!(matches!(decision, Decision::Ignored { .. }));
    assert_eq!(decision.additional_context(), None);
    perform(&layout, &layout.store(), &decision).expect("perform");
    assert!(
        !link_path(&layout).exists(),
        "an ignored directory gets no link and no store"
    );
}
