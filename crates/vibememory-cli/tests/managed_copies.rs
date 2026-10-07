//! Managed copies of `CLAUDE.md` and `settings.json`: a three-way merge on whole files, with the
//! last agreement remembered as a hash and every conflict left for a person.

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

use std::fs;
use std::path::Path;

use support::TempDir;
use vibememory_cli::config::Config;
use vibememory_cli::install::{Layout, State, Step, plan, without_engine_hooks};
use vibememory_cli::managed::{Verdict, reconcile, verdict};
use vibememory_core::naming::PathSyntax;

const STAMP: &str = "2026-09-08T18:00:00Z";
const CONFIG_TEXT: &str = r#"{"machineId":"mac-test","remote":"ssh://git@host/store.git"}"#;

fn layout(temp: &TempDir) -> Layout {
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
        home: None,
    };
    fs::create_dir_all(layout.store().join("config")).expect("store config dir");
    layout
}

fn settings(layout: &Layout) -> serde_json::Value {
    serde_json::from_slice(&fs::read(layout.config_dir.join("settings.json")).expect("read"))
        .expect("json")
}

fn store_settings(layout: &Layout) -> serde_json::Value {
    serde_json::from_slice(&fs::read(layout.store().join("config/settings.json")).expect("read"))
        .expect("json")
}

fn managed_state(layout: &Layout) -> &'static str {
    match plan(
        layout,
        &Config::parse(CONFIG_TEXT, PathSyntax::Posix).expect("config"),
        &[],
    )
    .into_iter()
    .find(|action| {
        action.step
            == Step::ManagedCopy {
                name: "settings.json",
            }
    })
    .expect("the step exists")
    .state
    {
        State::Satisfied => "ok",
        State::Missing => "missing",
        State::Conflict { .. } => "conflict",
        State::Unknown { .. } => "unknown",
    }
}

fn quarantined(engine: &Path) -> Vec<String> {
    fs::read_dir(engine.join("quarantine"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_verdict_follows_the_side_that_did_not_move() {
    // No file anywhere, or one side only.
    assert_eq!(verdict(None, None, None), Verdict::Nothing);
    assert_eq!(verdict(Some("a"), None, None), Verdict::Push);
    assert_eq!(verdict(None, Some("a"), None), Verdict::Pull);
    // Agreement, whatever the base says.
    assert_eq!(verdict(Some("a"), Some("a"), None), Verdict::Same);
    assert_eq!(verdict(Some("a"), Some("a"), Some("z")), Verdict::Same);
    // The side still on the base has not moved; the other one wins.
    assert_eq!(
        verdict(Some("base"), Some("new"), Some("base")),
        Verdict::Pull
    );
    assert_eq!(
        verdict(Some("new"), Some("base"), Some("base")),
        Verdict::Push
    );
    // Both moved — or nobody remembers an agreement while the copies differ: never a guess.
    assert_eq!(
        verdict(Some("x"), Some("y"), Some("base")),
        Verdict::Conflict
    );
    assert_eq!(verdict(Some("x"), Some("y"), None), Verdict::Conflict);
}

#[test]
fn a_change_made_in_the_store_reaches_the_machine_with_its_hooks_kept() {
    let temp = TempDir::new("managed-pull");
    let layout = layout(&temp);
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("local");
    fs::write(
        layout.store().join("config/settings.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("store");

    // The first reconciliation finds agreement and records it.
    let first = reconcile(&layout, &layout.store(), STAMP).expect("first");
    assert!(first.pushed.is_empty() && first.pulled.is_empty() && first.conflicting.is_empty());
    assert_eq!(managed_state(&layout), "ok");

    // Another machine put a flag into the shared settings. This machine's copy is still on the
    // base, so the store's version wins — and the CLI here reads the new flag.
    fs::write(
        layout.store().join("config/settings.json"),
        r#"{"theme":"dark","env":{"ENABLE_PROMPT_CACHING_1H":"1"}}"#,
    )
    .expect("store change");
    assert_eq!(
        managed_state(&layout),
        "missing",
        "doctor says it is owed, not broken"
    );
    let second = reconcile(&layout, &layout.store(), STAMP).expect("second");
    assert_eq!(second.pulled, vec!["settings.json".to_owned()]);
    let local = settings(&layout);
    assert_eq!(
        without_engine_hooks(local.clone()),
        serde_json::json!({"theme":"dark","env":{"ENABLE_PROMPT_CACHING_1H":"1"}}),
        "the shared part is the store's now: {local}"
    );
    assert!(
        local.get("hooks").is_some(),
        "and this machine's hooks are back on top, or the next session runs without the engine"
    );
    assert_eq!(managed_state(&layout), "ok");
}

#[test]
fn a_change_made_on_the_machine_reaches_the_store_without_its_hooks() {
    let temp = TempDir::new("managed-push");
    let layout = layout(&temp);
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("local");
    fs::write(
        layout.store().join("config/settings.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("store");
    reconcile(&layout, &layout.store(), STAMP).expect("first");

    // The owner sets the flag here, on top of this machine's hooks.
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"theme":"dark","env":{"ENABLE_PROMPT_CACHING_1H":"1"},
            "hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command",
            "command":"VIBEMEMORY_DIR=/e /e/bin/vibememory hook stop","timeout":20}]}]}}"#,
    )
    .expect("local change");
    let done = reconcile(&layout, &layout.store(), STAMP).expect("second");
    assert_eq!(done.pushed, vec!["settings.json".to_owned()]);
    let shared = store_settings(&layout);
    assert_eq!(
        shared,
        serde_json::json!({"theme":"dark","env":{"ENABLE_PROMPT_CACHING_1H":"1"}}),
        "the flag travels, this machine's hook command does not: {shared}"
    );
}

#[test]
fn both_sides_changed_is_left_for_a_person_with_nothing_lost() {
    let temp = TempDir::new("managed-conflict");
    let layout = layout(&temp);
    fs::write(layout.config_dir.join("CLAUDE.md"), b"rules v1\n").expect("local");
    fs::write(layout.store().join("config/CLAUDE.md"), b"rules v1\n").expect("store");
    reconcile(&layout, &layout.store(), STAMP).expect("first");

    fs::write(layout.config_dir.join("CLAUDE.md"), b"rules v1\nmine\n").expect("local edit");
    fs::write(
        layout.store().join("config/CLAUDE.md"),
        b"rules v1\ntheirs\n",
    )
    .expect("their edit");
    let done = reconcile(&layout, &layout.store(), STAMP).expect("second");

    assert_eq!(done.conflicting, vec!["CLAUDE.md".to_owned()]);
    assert_eq!(
        fs::read(layout.config_dir.join("CLAUDE.md")).expect("read"),
        b"rules v1\nmine\n",
        "neither file is touched: a guess here overwrites somebody's hour of work"
    );
    assert_eq!(
        fs::read(layout.store().join("config/CLAUDE.md")).expect("read"),
        b"rules v1\ntheirs\n"
    );
    let waiting = quarantined(&layout.engine_dir);
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert!(
        waiting[0].starts_with("CLAUDE.md-"),
        "this machine's version is set aside, not lost: {waiting:?}"
    );
    // A tick every two minutes must not pile up copies of the same version.
    reconcile(&layout, &layout.store(), "2026-09-08T18:02:00Z").expect("third");
    assert_eq!(quarantined(&layout.engine_dir).len(), 1);
    // And `doctor` says what happened, in words a person can act on.
    match plan(
        &layout,
        &Config::parse(CONFIG_TEXT, PathSyntax::Posix).expect("config"),
        &[],
    )
    .into_iter()
    .find(|action| action.step == Step::ManagedCopy { name: "CLAUDE.md" })
    .expect("step")
    .state
    {
        State::Conflict { found } => assert!(
            found.contains("both copies changed") && found.contains("quarantine"),
            "{found}"
        ),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

#[test]
fn differing_copies_with_no_recorded_agreement_are_never_guessed_about() {
    let temp = TempDir::new("managed-no-base");
    let layout = layout(&temp);
    // Two machines set up separately, each with its own settings — the first sync is exactly
    // where a guess would overwrite one of them.
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("local");
    fs::write(
        layout.store().join("config/settings.json"),
        r#"{"theme":"light"}"#,
    )
    .expect("store");

    let done = reconcile(&layout, &layout.store(), STAMP).expect("reconcile");
    assert_eq!(done.conflicting, vec!["settings.json".to_owned()]);
    assert_eq!(settings(&layout), serde_json::json!({"theme":"dark"}));
    assert_eq!(
        store_settings(&layout),
        serde_json::json!({"theme":"light"})
    );
    assert_eq!(managed_state(&layout), "conflict");
}

/// The settings of one case of `fixtures/export/settingsWithToken.json`: a synthetic token where a
/// real one would sit.
fn settings_with_token(case: &str) -> String {
    let file: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/export/settingsWithToken.json"
    ))
    .expect("settingsWithToken.json");
    file["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|entry| entry["id"] == case)
        .and_then(|entry| entry["text"].as_str())
        .expect("the case exists")
        .to_owned()
}

#[test]
fn a_copy_holding_a_token_stays_on_the_machine_until_the_token_is_taken_out() {
    let temp = TempDir::new("managed-token");
    let layout = layout(&temp);
    fs::write(
        layout.config_dir.join("settings.json"),
        settings_with_token("envHoldsToken"),
    )
    .expect("local");

    let done = reconcile(&layout, &layout.store(), STAMP).expect("reconcile");
    assert_eq!(done.withheld, vec!["settings.json".to_owned()]);
    assert!(done.pushed.is_empty(), "{done:?}");
    assert!(
        !layout.store().join("config/settings.json").exists(),
        "nothing reached the store"
    );
    assert_eq!(
        managed_state(&layout),
        "conflict",
        "`doctor` names the file and `install` leaves it alone"
    );

    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("token taken out");
    let done = reconcile(&layout, &layout.store(), STAMP).expect("reconcile");
    assert_eq!(done.pushed, vec!["settings.json".to_owned()]);
    assert_eq!(store_settings(&layout), serde_json::json!({"theme":"dark"}));
}
