//! `install` / `doctor` / `status` share one plan; these tests drive it against a real machine
//! layout built inside a temporary directory.

// The test creates directories, links and a git repository, so the purity gate is lifted here.
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
use vibememory_cli::install::{
    Action, GITATTRIBUTES, Layout, State, Step, apply, launch_agent, plan, schedule_path,
};
use vibememory_core::naming::PathSyntax;

const CONFIG_TEXT: &str = r#"{"machineId":"mac-test","remote":"ssh://git@host/store.git"}"#;

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    }
}

fn config() -> Config {
    Config::parse(CONFIG_TEXT, PathSyntax::Posix).expect("config")
}

fn state_of<'a>(actions: &'a [Action], wanted: &Step) -> &'a State {
    &actions
        .iter()
        .find(|action| action.step == *wanted)
        .unwrap_or_else(|| panic!("no action for {wanted:?}"))
        .state
}

#[test]
fn a_fresh_machine_needs_every_step_it_can_have() {
    let temp = TempDir::new("install-fresh");
    let actions = plan(&layout(&temp), &config(), &[]);
    assert!(!actions.is_empty());
    for action in &actions {
        match action.step {
            // A file neither the machine nor the store has is nothing to manage, and saying
            // otherwise would make install repeat the same no-op for ever.
            Step::ManagedCopy { .. } => assert_eq!(action.state, State::Satisfied),
            _ => assert_eq!(action.state, State::Missing, "{action:?}"),
        }
    }
}

#[test]
fn a_dry_run_changes_nothing() {
    let temp = TempDir::new("install-dry");
    let layout = layout(&temp);
    let actions = plan(&layout, &config(), &[]);
    let applied = apply(&layout, &actions, true);

    assert!(applied.is_complete());
    assert!(
        !applied.performed.is_empty(),
        "it must say what it would do"
    );
    assert!(
        !layout.store().exists(),
        "a dry run may not create the store"
    );
}

#[test]
fn applying_makes_every_step_true_and_running_again_touches_nothing() {
    let temp = TempDir::new("install-apply");
    let layout = layout(&temp);
    let links = vec![("-tmp-project".to_owned(), "Project".to_owned())];

    let first = apply(&layout, &plan(&layout, &config(), &links), false);
    assert!(
        first.is_complete(),
        "refused: {:?}, failed: {:?}",
        first.refused,
        first.failed
    );

    let second_plan = plan(&layout, &config(), &links);
    let unsatisfied: Vec<&Action> = second_plan
        .iter()
        .filter(|action| !action.is_satisfied())
        .collect();
    assert!(
        unsatisfied.is_empty(),
        "a second plan must be empty of work: {unsatisfied:?}"
    );

    let second = apply(&layout, &second_plan, false);
    assert!(
        second.performed.is_empty() && second.is_complete(),
        "a second run must touch nothing: {second:?}"
    );

    // The link exists and points into the store.
    let link = layout.config_dir.join("projects").join("-tmp-project");
    assert_eq!(
        fs::read_link(&link).expect("read link"),
        layout.store().join("projects").join("Project")
    );
    assert_eq!(
        fs::read_to_string(layout.store().join(".gitattributes")).expect("read"),
        GITATTRIBUTES
    );
}

#[test]
fn a_link_that_points_elsewhere_is_reported_and_left_alone() {
    let temp = TempDir::new("install-conflict");
    let layout = layout(&temp);
    let links = vec![("-tmp-project".to_owned(), "Project".to_owned())];
    let elsewhere = temp.dir("elsewhere");
    let link = layout.config_dir.join("projects").join("-tmp-project");
    fs::create_dir_all(link.parent().expect("parent")).expect("create projects");
    std::os::unix::fs::symlink(&elsewhere, &link).expect("symlink");

    let actions = plan(&layout, &config(), &links);
    let state = state_of(
        &actions,
        &Step::ProjectLink {
            enc: "-tmp-project".to_owned(),
            name: "Project".to_owned(),
        },
    );
    assert!(
        matches!(state, State::Conflict { .. }),
        "expected a conflict, got {state:?}"
    );

    let applied = apply(&layout, &actions, false);
    assert_eq!(applied.refused.len(), 1, "the conflict must be reported");
    assert_eq!(
        fs::read_link(&link).expect("read link"),
        elsewhere,
        "the existing link must be left exactly as it was"
    );
}

#[test]
fn a_real_directory_is_never_replaced_by_a_link() {
    let temp = TempDir::new("install-realdir");
    let layout = layout(&temp);
    let links = vec![("-tmp-project".to_owned(), "Project".to_owned())];
    // The first session in a new working directory creates a real directory with transcripts in
    // it. Replacing that with a link would throw away history nobody has imported yet.
    let real = layout.config_dir.join("projects").join("-tmp-project");
    fs::create_dir_all(&real).expect("create real dir");
    fs::write(real.join("session.jsonl"), b"{}\n").expect("write transcript");

    let applied = apply(&layout, &plan(&layout, &config(), &links), false);
    assert_eq!(applied.refused.len(), 1);
    assert!(
        real.join("session.jsonl").exists(),
        "the transcript must survive"
    );
}

#[test]
fn a_gitattributes_written_by_somebody_else_is_a_conflict() {
    let temp = TempDir::new("install-attrs");
    let layout = layout(&temp);
    let path = layout.store().join(".gitattributes");
    fs::create_dir_all(path.parent().expect("parent")).expect("create store");
    fs::write(&path, "* text=auto\n").expect("write");

    let actions = plan(&layout, &config(), &[]);
    let state = state_of(&actions, &Step::Gitattributes);
    assert!(
        matches!(state, State::Conflict { .. }),
        "expected a conflict, got {state:?}"
    );
    let _ = apply(&layout, &actions, false);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "* text=auto\n",
        "install must not overwrite a file it did not write"
    );
}

#[test]
fn a_file_that_appears_between_the_plan_and_the_apply_is_not_overwritten() {
    let temp = TempDir::new("install-race");
    let layout = layout(&temp);
    // The plan is computed while the file is absent...
    let actions = plan(&layout, &config(), &[]);
    assert_eq!(state_of(&actions, &Step::Gitattributes), &State::Missing);

    // ...and somebody writes it before the apply. The tick of another machine and a person with
    // an editor both do this; the window is small and real.
    let path = layout.store().join(".gitattributes");
    fs::create_dir_all(path.parent().expect("parent")).expect("create store");
    fs::write(&path, "written by somebody else\n").expect("write");

    let _ = apply(&layout, &actions, false);
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "written by somebody else\n",
        "apply must not overwrite what appeared after the plan"
    );
}

#[test]
fn the_managed_copy_travels_in_both_directions() {
    let temp = TempDir::new("install-managed");
    let layout = layout(&temp);
    fs::write(layout.config_dir.join("CLAUDE.md"), b"local rules\n").expect("write");

    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);
    assert_eq!(
        fs::read(layout.store().join("config").join("CLAUDE.md")).expect("read"),
        b"local rules\n",
        "a file only this machine has goes into the store"
    );

    // The other direction: the store has settings.json, the machine does not.
    let in_store = layout.store().join("config").join("settings.json");
    fs::write(&in_store, b"{}\n").expect("write");
    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);
    assert_eq!(
        fs::read(layout.config_dir.join("settings.json")).expect("read"),
        b"{}\n",
        "a file only the store has comes back to the machine"
    );
}

#[test]
fn the_git_settings_land_in_the_store_repository() {
    let temp = TempDir::new("install-git");
    let layout = layout(&temp);
    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);

    let value = git_config(&layout.store(), "core.autocrlf");
    assert_eq!(
        value, "false",
        "CRLF conversion would duplicate the state block of every transcript line"
    );
}

fn git_config(store: &Path, key: &str) -> String {
    let output = std::process::Command::new("git")
        .args(["config", "--local", "--get", key])
        .current_dir(store)
        .output()
        .expect("run git config");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[cfg(target_os = "macos")]
#[test]
fn the_scheduled_tick_is_written_and_names_this_engine_directory() {
    let temp = TempDir::new("install-schedule");
    let layout = layout(&temp);
    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);

    let text = fs::read_to_string(schedule_path(&layout)).expect("the agent must be written");
    assert!(text.contains("<key>StartInterval</key><integer>120</integer>"));
    assert!(
        text.contains("<string>tick</string>"),
        "it must run the tick"
    );
    assert!(
        text.contains(&layout.engine_dir.display().to_string()),
        "an agent that does not name its engine directory would tick the wrong store"
    );
    assert_eq!(text, launch_agent(&layout));

    // Second run: nothing to do, and the file is not rewritten.
    let second = plan(&layout, &config(), &[]);
    assert!(second.iter().all(Action::is_satisfied), "{second:?}");
}
