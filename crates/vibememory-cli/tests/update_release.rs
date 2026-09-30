//! `vibememory update` against a release directory on disk, the way the installers can be pointed
//! at a local copy: the archive's sum decides whether anything runs, and the new program installs
//! itself.

// The test writes a release on disk and runs tar, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]
#![cfg(unix)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::TempDir;
use vibememory_cli::install::Layout;
use vibememory_cli::update::{
    CHECK_EVERY_SECONDS, CURRENT, Updated, check_if_due, newer_known, update,
};
use vibememory_core::release::{archive_name, target};

const NEWER: &str = "99.0.0";

/// A release directory with `latest` naming `version` and an archive whose `vibememory` notes how
/// it was run. `sum` replaces the published sum when given.
fn release(temp: &TempDir, version: &str, sum: Option<&str>) -> (String, PathBuf) {
    let root = temp.dir("dl");
    let marker = temp.path().join("ran");
    let staging = temp.dir("staging");
    let program = staging.join("vibememory");
    fs::write(
        &program,
        // `install` places itself as the engine, as the real one does; every run notes its arguments,
        // its engine directory and whether it was told not to update again
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = install ]; then mkdir -p \"$VIBEMEMORY_DIR/bin\" && cp \"$0\" \"$VIBEMEMORY_DIR/bin/vibememory\"; fi\n\
             printf '%s %s %s\\n' \"$*\" \"$VIBEMEMORY_DIR\" \"$VIBEMEMORY_NO_SELF_UPDATE\" >> '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::write(staging.join("vibememory-mcp"), "#!/bin/sh\n").unwrap();
    for name in ["vibememory", "vibememory-mcp"] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staging.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let target = target(std::env::consts::OS, std::env::consts::ARCH).unwrap();
    let dir = root.join(version);
    fs::create_dir_all(&dir).unwrap();
    let archive = dir.join(archive_name(version, target));
    let packed = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .args(["vibememory", "vibememory-mcp"])
        .status()
        .unwrap();
    assert!(packed.success());
    let actual = vibememory_cli::sha256::hex(&fs::read(&archive).unwrap());
    let published = sum.map_or(actual, str::to_owned);
    let mut sum_file = archive.clone().into_os_string();
    sum_file.push(".sha256");
    fs::write(&sum_file, format!("{published}  x\n")).unwrap();
    fs::write(root.join("latest"), format!("{version}\n")).unwrap();
    (format!("file://{}/", root.display()), marker)
}

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    }
}

fn leftovers(engine: &Path) -> Vec<String> {
    fs::read_dir(engine)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("update-"))
        .collect()
}

#[test]
fn a_newer_release_is_checked_unpacked_and_installs_itself_into_this_engine() {
    let temp = TempDir::new("update-newer");
    let layout = layout(&temp);
    let (base, marker) = release(&temp, NEWER, None);

    assert_eq!(
        update(&layout, &base).unwrap(),
        Updated::Installed(NEWER.to_owned())
    );
    let ran = fs::read_to_string(&marker).unwrap();
    assert_eq!(
        ran.trim_end(),
        format!("install {}", layout.engine_dir.display()),
        "the new program's own install, into this engine directory"
    );
    assert!(
        leftovers(&layout.engine_dir).is_empty(),
        "the work directory goes away"
    );
}

#[test]
fn a_sum_that_does_not_match_runs_nothing() {
    let temp = TempDir::new("update-sum");
    let layout = layout(&temp);
    let (base, marker) = release(&temp, NEWER, Some(&"0".repeat(64)));

    let refused = update(&layout, &base).unwrap_err();
    assert!(refused.contains("does not match"), "{refused}");
    assert!(!marker.exists(), "nothing from the archive ran");
    assert!(leftovers(&layout.engine_dir).is_empty());
}

#[test]
fn the_same_version_is_not_installed_again() {
    let temp = TempDir::new("update-same");
    let layout = layout(&temp);
    let (base, marker) = release(&temp, CURRENT, None);

    assert_eq!(
        update(&layout, &base).unwrap(),
        Updated::Current(CURRENT.to_owned())
    );
    assert!(!marker.exists());
}

#[test]
fn the_tick_asks_once_a_day_and_says_what_it_heard() {
    let temp = TempDir::new("update-check");
    let layout = layout(&temp);
    let (base, _) = release(&temp, NEWER, None);

    assert_eq!(newer_known(&layout), None, "nothing heard yet");
    let now = 1_800_000_000;
    assert_eq!(check_if_due(&layout, &base, now).as_deref(), Some(NEWER));

    // the host now says something else, but a day has not passed: it is not asked
    fs::write(temp.path().join("dl/latest"), format!("{CURRENT}\n")).unwrap();
    assert_eq!(
        check_if_due(&layout, &base, now + 60).as_deref(),
        Some(NEWER)
    );
    // a day later it is, and the running version is the newest again
    assert_eq!(
        check_if_due(&layout, &base, now + CHECK_EVERY_SECONDS),
        None
    );
    assert_eq!(newer_known(&layout), None);
}

#[test]
fn connect_updates_first_and_hands_over_to_the_new_program_before_the_code_is_read() {
    let temp = TempDir::new("update-connect");
    let layout = layout(&temp);
    let (base, marker) = release(&temp, NEWER, None);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
        .args(["connect", "--cabinet", "https://cabinet.invalid"])
        .env(vibememory_cli::update::RELEASES_VAR, &base)
        .env(vibememory_cli::install::ENGINE_DIR_VAR, &layout.engine_dir)
        .env(vibememory_cli::install::CONFIG_DIR_VAR, &layout.config_dir)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("vibememory {NEWER} is out")),
        "{stderr}"
    );
    let engine = layout.engine_dir.display().to_string();
    let runs: Vec<String> = fs::read_to_string(&marker)
        .unwrap()
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect();
    assert_eq!(
        runs,
        vec![
            format!("install {engine}"),
            format!("connect --cabinet https://cabinet.invalid {engine} 1"),
        ],
        "installed, then the same connect by the new program, which is told not to update again"
    );
}
