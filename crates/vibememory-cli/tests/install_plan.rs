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
    // The store's copy comes back, and this machine's hooks are added on top of it: what the
    // machine holds is the shared settings plus its own hooks, never less.
    let local: serde_json::Value =
        serde_json::from_slice(&fs::read(layout.config_dir.join("settings.json")).expect("read"))
            .expect("json");
    assert_eq!(
        vibememory_cli::install::without_engine_hooks(local.clone()),
        serde_json::json!({}),
        "a file only the store has comes back to the machine: {local}"
    );
    assert!(
        local.get("hooks").is_some(),
        "with this machine's hooks on top: {local}"
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

    let path = schedule_path(&layout);
    assert_eq!(
        path.file_name().expect("name"),
        "dev.vibememory.tick.plist",
        "the label's own dots are part of its name"
    );
    let text = fs::read_to_string(&path).expect("the agent must be written");
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

#[test]
fn the_store_never_carries_this_machines_hook_commands() {
    let temp = TempDir::new("install-hooks-store");
    let layout = layout(&temp);
    // A settings file with somebody's own hook in it, which must survive untouched.
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"theme":"dark","hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"say done"}]}]}}"#,
    )
    .expect("write");

    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);

    let local = fs::read_to_string(layout.config_dir.join("settings.json")).expect("local");
    for subcommand in ["session-start", "stop", "session-end", "user-prompt-submit"] {
        assert!(
            local.contains(&format!(" hook {subcommand}\"")),
            "the {subcommand} hook is installed: {local}"
        );
    }
    assert!(
        local.contains("say done"),
        "the person's own hook survives: {local}"
    );

    let shared: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(layout.store().join("config").join("settings.json"))
            .expect("store copy"),
    )
    .expect("json");
    assert_eq!(
        shared,
        serde_json::json!({
            "theme": "dark",
            "hooks": {"Stop": [{"matcher": "*", "hooks": [{"type": "command", "command": "say done"}]}]}
        }),
        "the store gets everything but this machine's hooks: a hook command names this machine's \
         binary, on another machine it fails, and a failing UserPromptSubmit hook stops sessions"
    );

    // With the hooks in place the machine is settled: nothing left to do.
    let again = plan(&layout, &config(), &[]);
    let pending: Vec<&Action> = again.iter().filter(|a| !a.is_satisfied()).collect();
    assert!(pending.is_empty(), "{pending:?}");
}

#[test]
fn hooks_are_refused_while_settings_is_a_link_into_a_synced_folder() {
    let temp = TempDir::new("install-hooks-link");
    let layout = layout(&temp);
    let elsewhere = temp.dir("synced");
    fs::write(elsewhere.join("settings.json"), "{}\n").expect("write");
    std::os::unix::fs::symlink(
        elsewhere.join("settings.json"),
        layout.config_dir.join("settings.json"),
    )
    .expect("link");

    let actions = plan(&layout, &config(), &[]);
    let state = state_of(&actions, &Step::Hooks);
    assert!(
        matches!(state, State::Conflict { .. }),
        "writing through that link would put this machine's hooks on the other machine: {state:?}"
    );
    let _ = apply(&layout, &actions, false);
    assert_eq!(
        fs::read_to_string(elsewhere.join("settings.json")).expect("read"),
        "{}\n",
        "the synced file must be exactly as it was"
    );
}

#[test]
fn install_commits_what_it_scaffolds_so_a_clone_gets_the_drivers() {
    let temp = TempDir::new("install-commit");
    let layout = layout(&temp);
    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);

    let output = std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .current_dir(layout.store())
        .output()
        .expect("git ls-tree");
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        tree.contains(".gitattributes"),
        "a .gitattributes outside any commit never reaches the other machine, and a clone \
         without it merges transcripts without the drivers: {tree}"
    );
    assert!(tree.contains("projects/.keep"), "{tree}");

    let status = std::process::Command::new("git")
        .args(["status", "--short"])
        .current_dir(layout.store())
        .output()
        .expect("git status");
    assert!(
        status.stdout.is_empty(),
        "nothing install made may be left uncommitted: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn scaffolding_left_uncommitted_by_an_earlier_run_is_committed_by_the_next() {
    let temp = TempDir::new("install-recommit");
    let layout = layout(&temp);
    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);
    // An earlier version of install left these outside any commit. Simulate it: unstage
    // everything so the files exist but are untracked.
    let reset = std::process::Command::new("git")
        .args(["rm", "-r", "--cached", "--quiet", "."])
        .current_dir(layout.store())
        .status()
        .expect("git rm --cached");
    assert!(reset.success());
    let actions = plan(&layout, &config(), &[]);
    assert_eq!(
        state_of(
            &actions,
            &Step::ScaffoldCommitted {
                machine_id: "mac-test".to_owned(),
            },
        ),
        &State::Missing,
        "doctor must see uncommitted scaffolding"
    );

    let _ = apply(&layout, &actions, false);
    let status = std::process::Command::new("git")
        .args(["status", "--short"])
        .current_dir(layout.store())
        .output()
        .expect("git status");
    assert!(
        status.stdout.is_empty(),
        "the next install commits what the earlier one left: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[test]
fn a_hook_from_an_earlier_install_is_replaced_not_accumulated() {
    let temp = TempDir::new("install-hooks-replace");
    let layout = layout(&temp);
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"VIBEMEMORY_DIR=/old/engine /old/build/vibememory hook stop","timeout":20}]}]}}"#,
    )
    .expect("write");

    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);

    let text = fs::read_to_string(layout.config_dir.join("settings.json")).expect("read");
    assert!(
        !text.contains("/old/build/vibememory"),
        "two entries per event would run two engines, one of them dead: {text}"
    );
    assert_eq!(
        text.matches(" hook stop\"").count(),
        1,
        "exactly one Stop hook of the engine: {text}"
    );
}

#[test]
fn a_binary_that_cannot_run_is_not_reported_as_installed() {
    // Installed is not working. A binary written over in place on macOS loses its signature and
    // is killed on sight; the hooks would then fail on every session with nothing to read. The
    // step must fail loudly instead of reporting success.
    let temp = TempDir::new("install-binary-runs");
    let bin = temp.dir("bin");
    let target = bin.join(if cfg!(windows) {
        "vibememory.exe"
    } else {
        "vibememory"
    });
    // Something executable that is not the engine: it runs and refuses, exactly as a broken
    // install would look from the outside.
    fs::write(&target, b"#!/bin/sh\nexit 3\n").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let state = vibememory_cli::install::binary_runs(&target);
    assert!(
        state.is_err(),
        "a binary that exits 3 must not pass: {state:?}"
    );
}

#[test]
fn a_signature_is_read_from_what_codesign_actually_prints() {
    use vibememory_cli::install::signing_authority;

    // The real report of a self-signed identity, as codesign writes it to stderr.
    let signed = "Executable=/tmp/engine/bin/vibememory\n\
                  Identifier=vibememory\n\
                  CodeDirectory v=20400 size=26520 flags=0x0(none) hashes=825+0\n\
                  Signature size=1234\n\
                  Authority=VibeMemory Local\n\
                  Timestamp=8 Sep 2026\n";
    assert_eq!(
        signing_authority(signed).as_deref(),
        Some("VibeMemory Local")
    );

    // A certificate prints its whole chain, signer first. The issuers below are not who signed
    // this binary — taking the last of them would name Apple for every developer's build.
    let chain = "Signature size=4567\n\
                 Authority=Developer ID Application: Owner (TEAMID)\n\
                 Authority=Developer ID Certification Authority\n\
                 Authority=Apple Root CA\n";
    assert_eq!(
        signing_authority(chain).as_deref(),
        Some("Developer ID Application: Owner (TEAMID)")
    );

    // Ad-hoc: the case the whole step exists for. Verified against `codesign -dvv` on this
    // machine — an ad-hoc signature prints no Authority line at all, and its identity changes
    // with every rebuild, which is why it can never satisfy the step.
    let adhoc = "Executable=/tmp/engine/bin/vibememory\n\
                 CodeDirectory v=20400 size=26520 flags=0x20002(adhoc,linker-signed)\n\
                 Signature=adhoc\n\
                 Info.plist=not bound\n";
    assert_eq!(signing_authority(adhoc), None);

    assert_eq!(signing_authority(""), None);
}

#[test]
fn signing_follows_the_copy_whatever_the_old_file_said() {
    use vibememory_cli::install::signing_state;

    // The old build is properly signed — and is about to be replaced by a copy that carries no
    // signature at all. Asking the file on disk answers about a binary that stops existing, and
    // the engine then runs unsigned until somebody happens to install a second time.
    assert_eq!(
        signing_state(true, State::Satisfied),
        State::Missing,
        "a replaced binary is unsigned afterwards, whatever the old one carried"
    );
    // Even somebody else's signature is not a conflict once the file it was on is gone.
    assert_eq!(
        signing_state(
            true,
            State::Conflict {
                found: "Somebody Else".to_owned()
            }
        ),
        State::Missing
    );
    // Nothing is being replaced: the answer about the file on disk is the answer.
    assert_eq!(signing_state(false, State::Satisfied), State::Satisfied);
    assert_eq!(
        signing_state(
            false,
            State::Conflict {
                found: "Somebody Else".to_owned()
            }
        ),
        State::Conflict {
            found: "Somebody Else".to_owned()
        }
    );
}

#[test]
fn a_signed_binary_is_not_mistaken_for_a_different_build() {
    let temp = TempDir::new("install-signed-idempotent");
    let layout = layout(&temp);
    let config = config();

    // Install once: the note beside the binary records which build it came from.
    let actions = plan(&layout, &config, &[]);
    let applied = apply(&layout, &actions, false);
    assert!(applied.failed.is_empty(), "{:?}", applied.failed);
    let installed = layout.engine_dir.join("bin").join("vibememory");
    assert!(installed.is_file(), "the binary is in place");

    // Signing rewrites the binary — here, any change to its bytes stands for that. Byte
    // comparison would now report a different build, replace it under the running hooks and sign
    // it again, on this and on every later run.
    fs::write(&installed, b"same build, signed: different bytes").expect("sign");

    assert_eq!(
        state_of(&plan(&layout, &config, &[]), &Step::Binary),
        &State::Satisfied,
        "a signature is not a different build"
    );

    // A genuinely different build is still replaced: the note is what changed, not the rule.
    // (The engine running from its own installed path is a separate case, covered by `doctor`
    // on a real machine: `current_exe` in this suite is the test harness, never the engine.)
    fs::write(
        layout.engine_dir.join("bin").join("vibememory.source"),
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .expect("note of an older build");
    assert_eq!(
        state_of(&plan(&layout, &config, &[]), &Step::Binary),
        &State::Missing,
        "an older build is still replaced"
    );
}
