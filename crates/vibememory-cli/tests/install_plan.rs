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
    Action, GITATTRIBUTES, Layout, State, Step, apply, driver_probe, git_bash,
    merge_driver_command, plan, scheduled_task, shell_word_for, utf16_with_bom,
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
            // A missing settings file likewise holds no flag that switches the cache off, and a
            // machine that has never ticked holds back no deletions: both steps are already
            // satisfied, and neither is ever something `install` performs.
            Step::ManagedCopy { .. } | Step::PromptCacheEnv | Step::DeletionsHeld => {
                assert_eq!(action.state, State::Satisfied);
            }
            // curl is the machine's, like git: found or not, never something install puts there.
            Step::Curl => assert_ne!(action.state, State::Missing, "{action:?}"),
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
    support::link_dir(&elsewhere, &link);

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
    use vibememory_cli::install::{launch_agent, schedule_path};

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

// A *file* link into a synced folder is the old macOS scheme; on Windows a file symlink needs
// privileges and that scheme never existed.
#[cfg(unix)]
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
    runnable(&installed, "one, signed: different bytes");

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

#[test]
fn a_flag_that_switches_the_prompt_cache_off_is_named_as_a_conflict() {
    use vibememory_cli::install::cache_killing_flags;

    let temp = TempDir::new("install-cache-flags");
    let layout = layout(&temp);
    let step = Step::PromptCacheEnv;

    // The flag is set: every machine this file reaches pays for it in usage limits.
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"env":{"DISABLE_PROMPT_CACHING":"1","EDITOR":"vim"}}"#,
    )
    .expect("write");
    match state_of(&plan(&layout, &config(), &[]), &step) {
        State::Conflict { found } => assert!(
            found.contains("DISABLE_PROMPT_CACHING=1"),
            "the flag must be named, not just counted: {found}"
        ),
        other => panic!("a set flag is a conflict, got {other:?}"),
    }

    // Left in place but turned off: not a finding. Reporting it would teach people to delete the
    // line instead of reading it.
    fs::write(
        layout.config_dir.join("settings.json"),
        r#"{"env":{"DISABLE_PROMPT_CACHING":"0","FORCE_PROMPT_CACHING_5M":"false"}}"#,
    )
    .expect("write");
    assert_eq!(
        state_of(&plan(&layout, &config(), &[]), &step),
        &State::Satisfied
    );

    // The pure decision, for the shapes the step reads: per-model switches count, the shortener
    // counts, unrelated variables do not, and the list is sorted so the message is stable.
    let flags = cache_killing_flags(&serde_json::json!({
        "env": {
            "FORCE_PROMPT_CACHING_5M": "1",
            "DISABLE_PROMPT_CACHING_OPUS": "true",
            "CLAUDE_CODE_PROMPT_CACHE_TTL": "1h",
            "ENABLE_PROMPT_CACHING_1H": "1"
        }
    }));
    assert_eq!(
        flags,
        vec![
            "DISABLE_PROMPT_CACHING_OPUS=true".to_owned(),
            "FORCE_PROMPT_CACHING_5M=1".to_owned()
        ]
    );
    assert!(cache_killing_flags(&serde_json::json!({})).is_empty());
}

/// A file that really runs and answers `--version`, so the install's own check is exercised.
fn runnable(path: &Path, version: &str) {
    fs::write(path, format!("#!/bin/sh\necho 'probe {version}'\n")).expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

#[test]
fn a_second_binary_is_placed_by_rename_and_a_signature_is_not_a_new_build() {
    use vibememory_cli::install::{install_named, named_binary_state};

    let temp = TempDir::new("install-named");
    let layout = layout(&temp);
    let source = temp.path().join("vibememory-mcp");
    let installed = layout.engine_dir.join("bin").join("vibememory-mcp");

    // Nothing to install from and nothing installed: a machine that never built the server has
    // nothing to manage, and calling that "missing" would make `doctor` fail for ever.
    assert_eq!(
        named_binary_state(&layout, "vibememory-mcp", None),
        State::Satisfied
    );

    // A real executable, not a stand-in: `install_named` insists that what it placed answers
    // `--version`, and a test whose "binary" cannot run would not exercise that promise at all.
    runnable(&source, "one");
    assert_eq!(
        named_binary_state(&layout, "vibememory-mcp", Some(&source)),
        State::Missing,
        "there is a build and it is not installed yet"
    );

    install_named(&layout, "vibememory-mcp", &source).expect("install");
    assert_eq!(
        fs::read(&installed).expect("read"),
        fs::read(&source).expect("read source"),
        "placed byte for byte"
    );
    assert_eq!(
        named_binary_state(&layout, "vibememory-mcp", Some(&source)),
        State::Satisfied
    );

    // Signing rewrites the file. Measured 2026-09-10: comparing bytes would then call it a new
    // build for ever, and every run would replace it — which is exactly what makes macOS kill a
    // binary that was written over in place.
    fs::write(&installed, b"same build, signed: different bytes").expect("sign");
    assert_eq!(
        named_binary_state(&layout, "vibememory-mcp", Some(&source)),
        State::Satisfied,
        "a signature is not a different build"
    );

    // A genuinely newer build is owed again.
    runnable(&source, "two");
    assert_eq!(
        named_binary_state(&layout, "vibememory-mcp", Some(&source)),
        State::Missing
    );
}

#[test]
fn installing_a_second_time_verifies_the_new_copy_as_well() {
    use std::process::Command;
    use vibememory_cli::install::install_named;

    let temp = TempDir::new("install-over-executed");
    let layout = layout(&temp);
    let source = temp.path().join("vibememory-mcp");
    let installed = layout.engine_dir.join("bin").join("vibememory-mcp");

    runnable(&source, "one");
    install_named(&layout, "vibememory-mcp", &source).expect("first install");

    // Execute it, then replace it, then execute again — the sequence that broke the real server
    // binary on 2026-09-10 when a manual told the owner to `cp`. What this gate proves is that
    // the second install also verifies its copy; it does NOT prove the SIGKILL is gone, because
    // that trap belongs to signed Mach-O binaries and a shell script cannot stand in for one.
    // The protection there rests on the rename and on the measurement recorded in
    // knowledge/design/binaryReplacement.md, which anyone can redo in half a minute.
    let ran = Command::new(&installed)
        .arg("--version")
        .status()
        .expect("run");
    assert!(ran.success(), "the freshly installed file has to run");

    // A newer build over the top of it.
    runnable(&source, "two");
    install_named(&layout, "vibememory-mcp", &source).expect("second install must also verify");

    let again = Command::new(&installed)
        .arg("--version")
        .status()
        .expect("run");
    assert!(again.success(), "and what it placed still runs");
}

#[test]
fn a_layout_without_a_home_is_an_error_not_a_relative_path() {
    // An absent home used to become an empty string, and the engine then lived in `.vibememory`
    // under whatever directory it was started in — on Windows, where the scheduler sets no `HOME`,
    // a project folder.
    let home = Path::new("/work/me");
    let found = Layout::resolve(Some(home), None, None).expect("a home is enough");
    assert_eq!(found.engine_dir, home.join(".vibememory"));
    assert_eq!(found.config_dir, home.join(".claude"));

    let refused = Layout::resolve(None, None, None);
    assert!(
        refused.is_err(),
        "no home and no variables must be refused, not guessed: {refused:?}"
    );

    let given = Layout::resolve(
        None,
        Some("/work/claude".into()),
        Some("/work/engine".into()),
    )
    .expect("the variables are enough without a home");
    assert_eq!(given.engine_dir, Path::new("/work/engine"));
    assert_eq!(given.config_dir, Path::new("/work/claude"));

    // An empty variable is unset, as the shell has it — not a directory called "".
    let empty = Layout::resolve(Some(home), Some("".into()), Some("".into()))
        .expect("an empty variable falls back to the home");
    assert_eq!(empty.engine_dir, home.join(".vibememory"));
}

#[test]
fn shell_words_survive_spaces_quotes_and_windows_separators() {
    assert_eq!(
        shell_word_for("/work/a b/it's", PathSyntax::Posix),
        r"'/work/a b/it'\''s'"
    );
    // Git Bash reads a backslash as an escape: unconverted, `D:\Work` reaches the program as
    // `D:Work`.
    assert_eq!(
        shell_word_for(r"D:\Work\A B\engine", PathSyntax::Windows),
        "'D:/Work/A B/engine'"
    );
    // On POSIX a backslash is an ordinary file-name character and stays one.
    assert_eq!(
        shell_word_for(r"/work/a\b", PathSyntax::Posix),
        r"'/work/a\b'"
    );
}

/// A stand-in for the engine that says what it was given, installed where the lines point.
#[cfg(unix)]
fn install_script(layout: &Layout, script: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    use vibememory_cli::install::installed_binary;
    let binary = installed_binary(layout);
    fs::create_dir_all(binary.parent().expect("bin")).expect("bin");
    fs::write(&binary, script).expect("write the stand-in");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("make it runnable");
}

#[cfg(unix)]
#[test]
fn hook_and_driver_lines_reach_the_binary_with_every_argument_intact() {
    use vibememory_cli::install::hook_command;

    let temp = TempDir::new("install-shell-words");
    // A space and a quote: the two things an unquoted word does not survive.
    let engine = temp.dir("an engine's dir");
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: engine.clone(),
    };
    install_script(
        &layout,
        "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$VIBEMEMORY_DIR\" \"$1\" \"$2\" \"$3\"\n",
    );
    let run = |line: &str| {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(line)
            .output()
            .expect("run sh");
        assert!(
            output.status.success(),
            "{line}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert_eq!(
        run(&hook_command(&layout, "stop")),
        format!("{}|hook|stop|", engine.display())
    );
    assert_eq!(
        run(&merge_driver_command(&layout, "jsonl")),
        format!("{}|merge-driver|jsonl|%O", engine.display())
    );
}

#[test]
fn the_probe_is_the_driver_line_asking_for_a_version_instead_of_merging() {
    let temp = TempDir::new("install-probe-line");
    let layout = layout(&temp);
    let line = merge_driver_command(&layout, "jsonl");
    let probe = driver_probe(&line, "jsonl").expect("a line the engine wrote");
    assert!(
        probe.ends_with(" --version") && !probe.contains("%O"),
        "{probe}"
    );
    assert!(
        probe.starts_with("VIBEMEMORY_DIR="),
        "the engine directory stays in front, as git would run it: {probe}"
    );
    assert_eq!(
        driver_probe("meld-wrapper %O %A %B", "jsonl"),
        None,
        "a driver the engine did not write has nothing of ours to run"
    );
}

/// The state of the git setting `key` in a fresh plan.
fn setting_state(actions: &[Action], key: &str) -> State {
    actions
        .iter()
        .find(|action| matches!(&action.step, Step::GitSetting { key: found, .. } if *found == key))
        .map(|action| action.state.clone())
        .expect("the setting is planned")
}

#[test]
fn a_driver_an_earlier_install_wrote_is_replaced_and_a_foreign_one_is_not() {
    let temp = TempDir::new("install-driver-replace");
    let layout = layout(&temp);
    let store = layout.store();
    fs::create_dir_all(&store).expect("store");
    support::git(&store, &["init", "--quiet"]);
    // What every install wrote until 2026-09-11: the bare name, which git could not find.
    support::git(
        &store,
        &[
            "config",
            "--local",
            "merge.vibememory-jsonl.driver",
            "vibememory merge-driver jsonl %O %A %B %P",
        ],
    );
    support::git(
        &store,
        &[
            "config",
            "--local",
            "merge.vibememory-keepboth.driver",
            "meld-wrapper %O %A %B",
        ],
    );

    let actions = plan(&layout, &config(), &[]);
    assert_eq!(
        setting_state(&actions, "merge.vibememory-jsonl.driver"),
        State::Missing,
        "an earlier install's driver is the engine's own to replace"
    );
    assert!(
        matches!(
            setting_state(&actions, "merge.vibememory-keepboth.driver"),
            State::Conflict { .. }
        ),
        "somebody else's driver is theirs"
    );
}

/// What the plan says about the drivers starting, on a store configured as `install` does and
/// with `script` standing in for the engine.
#[cfg(unix)]
fn probe_with(label: &str, script: &str) -> State {
    use vibememory_cli::install::git_settings;

    let temp = TempDir::new(label);
    let layout = layout(&temp);
    let store = layout.store();
    fs::create_dir_all(&store).expect("store");
    support::git(&store, &["init", "--quiet"]);
    for (key, value) in git_settings(&layout) {
        support::git(&store, &["config", "--local", key, &value]);
    }
    install_script(&layout, script);
    plan(&layout, &config(), &[])
        .into_iter()
        .find(|action| action.step == Step::MergeDriverRuns)
        .expect("the probe is planned")
        .state
}

#[cfg(unix)]
#[test]
fn a_driver_that_does_not_start_is_not_reported_as_running() {
    assert_eq!(
        probe_with("install-probe-ok", "#!/bin/sh\necho 'vibememory 0.1.0'\n"),
        State::Satisfied
    );
    let broken = probe_with(
        "install-probe-broken",
        "#!/bin/sh\necho 'no such thing here' >&2\nexit 127\n",
    );
    assert!(
        matches!(&broken, State::Conflict { found } if found.contains("no such thing here")),
        "what the shell said has to reach doctor: {broken:?}"
    );
}

#[test]
fn git_bash_is_found_the_way_claude_code_finds_it() {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    let found = |existing: &[&str], given: Option<&str>, git: Option<&str>| {
        let existing: BTreeSet<PathBuf> = existing.iter().map(PathBuf::from).collect();
        git_bash(given, git.map(Path::new), |path| existing.contains(path))
    };
    let standard = r"C:\Program Files\Git\bin\bash.exe";
    let x86 = r"C:\Program Files (x86)\Git\bin\bash.exe";

    // The variable wins when it names a bash or sh that is there.
    assert_eq!(
        found(
            &[r"D:\Tools\bash.exe", standard],
            Some(r"D:\Tools\bash.exe"),
            None
        ),
        Some(PathBuf::from(r"D:\Tools\bash.exe"))
    );
    // Anything else in it is ignored, as Claude Code ignores it, and the search goes on.
    assert_eq!(
        found(
            &[r"D:\Tools\zsh.exe", standard],
            Some(r"D:\Tools\zsh.exe"),
            None
        ),
        Some(PathBuf::from(standard))
    );
    assert_eq!(
        found(&[standard], Some(r"D:\Tools\bash.exe"), None),
        Some(PathBuf::from(standard)),
        "a variable naming nothing falls through"
    );
    // The two standard installs, in Claude Code's order.
    assert_eq!(
        found(&[standard, x86], None, None),
        Some(PathBuf::from(standard))
    );
    assert_eq!(found(&[x86], None, None), Some(PathBuf::from(x86)));
    // Then `bin\bash.exe` two levels above the `git` on `PATH`.
    assert_eq!(
        found(
            &["/tools/Git/bin/bash.exe"],
            None,
            Some("/tools/Git/cmd/git.exe")
        ),
        Some(PathBuf::from("/tools/Git/bin/bash.exe"))
    );
    assert_eq!(found(&[], None, Some("/tools/Git/cmd/git.exe")), None);
}

#[test]
fn the_scheduled_task_runs_the_tick_after_logon_every_two_minutes_and_at_unlock() {
    let temp = TempDir::new("install-task");
    // `&` in a folder name: bare, it makes the task file unreadable to the Task Scheduler.
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("R&D engine"),
    };
    let task = scheduled_task(&layout);
    for part in [
        "<Interval>PT2M</Interval>",
        "<StateChange>SessionUnlock</StateChange>",
        "<LogonTrigger>",
        "<LogonType>InteractiveToken</LogonType>",
        "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
        // one console nobody sees for the tick and every git it starts, not a window every two minutes
        r"<Command>%windir%\System32\conhost.exe</Command>",
        "<Arguments>--headless &quot;",
        "&quot; tick</Arguments>",
        "R&amp;D engine",
    ] {
        assert!(task.contains(part), "{part} is missing:\n{task}");
    }
    assert!(!task.contains("R&D"), "a bare ampersand:\n{task}");

    let bytes = utf16_with_bom(&task);
    assert_eq!(&bytes[..2], &[0xFF, 0xFE], "UTF-16LE says so up front");
    let units: Vec<u16> = bytes[2..]
        .chunks(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    assert_eq!(String::from_utf16(&units).expect("utf-16"), task);
}

#[test]
fn install_does_not_commit_a_managed_copy_holding_a_kept_token() {
    let temp = TempDir::new("install-kept-token");
    let layout = layout(&temp);
    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);
    // The owner's token from before the cabinet has no shape; it is known by its kept value.
    let legacy = "0123456789abcdef".repeat(4);
    let kept = layout.engine_dir.join("tokens/personal");
    fs::create_dir_all(&kept).expect("dirs");
    fs::write(kept.join("claude-code"), format!("{legacy}\n")).expect("write");
    fs::write(
        layout.store().join("config/settings.json"),
        format!("{{\"env\":{{\"TOKEN\":\"{legacy}\"}}}}\n"),
    )
    .expect("write");

    let _ = apply(&layout, &plan(&layout, &config(), &[]), false);
    let output = std::process::Command::new("git")
        .args(["log", "-p", "--all"])
        .current_dir(layout.store())
        .output()
        .expect("git log");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(&legacy),
        "the kept value is in no commit"
    );
}

#[test]
fn linux_runs_the_tick_from_a_user_timer_every_two_minutes_or_from_cron() {
    let temp = TempDir::new("install-linux-schedule");
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("an engine"),
    };
    let service = vibememory_cli::install::systemd_service(&layout);
    assert!(service.contains("Type=oneshot"), "{service}");
    // a path with a space stays one word in systemd's own quoting
    assert!(
        service.contains("an engine/bin/vibememory\" tick"),
        "{service}"
    );
    assert!(
        service.contains("Environment=\"VIBEMEMORY_DIR="),
        "{service}"
    );
    let timer = vibememory_cli::install::systemd_timer();
    assert!(
        timer.contains("OnUnitActiveSec=120") && timer.contains("WantedBy=timers.target"),
        "{timer}"
    );
    let cron = vibememory_cli::install::cron_line(&layout);
    assert!(
        cron.starts_with("*/2 * * * * ") && cron.ends_with("tick # vibememory tick"),
        "{cron}"
    );
}
