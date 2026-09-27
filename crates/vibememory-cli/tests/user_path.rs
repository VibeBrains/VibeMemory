//! The engine's bin directory on the user's `PATH`: the block in a shell's startup file, written once, and
//! the Windows `Path` value with the directory added once.

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

use std::path::Path;

use vibememory_cli::user_path::{startup_file, windows_path_with};

#[test]
fn each_shell_reads_its_own_startup_file() {
    let home = Path::new("/h");
    assert_eq!(startup_file(home, "/bin/zsh", true), home.join(".zshrc"));
    assert_eq!(
        startup_file(home, "/bin/bash", true),
        home.join(".bash_profile")
    );
    assert_eq!(
        startup_file(home, "/usr/bin/bash", false),
        home.join(".bashrc")
    );
    assert_eq!(
        startup_file(home, "/usr/bin/fish", false),
        home.join(".profile")
    );
    assert_eq!(startup_file(home, "", false), home.join(".profile"));
}

#[test]
fn a_windows_path_gets_the_directory_once_in_any_case() {
    let bin = r"D:\Profiles\o\.vibememory\bin";
    assert_eq!(
        windows_path_with(r"C:\Windows;C:\Tools;", bin).as_deref(),
        Some(r"C:\Windows;C:\Tools;D:\Profiles\o\.vibememory\bin")
    );
    assert_eq!(windows_path_with("", bin).as_deref(), Some(bin));
    assert_eq!(
        windows_path_with(r"C:\Windows;d:\profiles\o\.VIBEMEMORY\bin\", bin),
        None
    );
}

#[cfg(unix)]
#[test]
fn the_block_goes_into_the_startup_file_once_and_keeps_what_was_there() {
    use std::fs;

    use support::TempDir;
    use vibememory_cli::user_path::{BLOCK_START, has_block, is_on_path, put_on_path, shell_block};

    let temp = TempDir::new("user-path");
    let home = temp.dir("home");
    let bin = home.join(".vibememory/bin");
    let file = startup_file(
        &home,
        &std::env::var("SHELL").unwrap_or_default(),
        cfg!(target_os = "macos"),
    );
    fs::write(&file, "alias ll='ls -l'").unwrap();

    assert!(!is_on_path(&home, &bin).unwrap());
    put_on_path(&home, &bin).unwrap();
    put_on_path(&home, &bin).unwrap();
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("alias ll='ls -l'\n"), "{text}");
    assert_eq!(text.matches(BLOCK_START).count(), 1, "{text}");
    assert!(text.contains(&shell_block(&bin)), "{text}");
    assert!(has_block(&text) && is_on_path(&home, &bin).unwrap());
}
