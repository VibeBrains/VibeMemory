//! What `vibememory update` reads of a release: which archive is this system's, which version is
//! newer, and which sum is the published one.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use vibememory_core::release::{archive_name, is_newer, parse_version, published_sum, target};

#[test]
fn every_built_system_has_its_target_and_nothing_else_does() {
    // the names `infra/releaseBuild.sh` gives its archives
    for (os, arch, expected) in [
        ("macos", "aarch64", "aarch64-apple-darwin"),
        ("macos", "x86_64", "x86_64-apple-darwin"),
        ("linux", "x86_64", "x86_64-unknown-linux-gnu"),
        ("windows", "x86_64", "x86_64-pc-windows-gnu"),
    ] {
        assert_eq!(target(os, arch), Some(expected), "{os} {arch}");
    }
    assert_eq!(target("linux", "aarch64"), None);
    assert_eq!(target("windows", "aarch64"), None);
    assert_eq!(
        archive_name("0.4.0", "x86_64-pc-windows-gnu"),
        "vibememory-0.4.0-x86_64-pc-windows-gnu.tar.gz"
    );
}

#[test]
fn newer_is_by_numbers_not_by_text() {
    assert!(is_newer("0.10.0", "0.9.9"), "text would say 0.9.9");
    assert!(is_newer("0.4.1", "0.4.0"));
    assert!(is_newer("1.0.0", "0.99.99"));
    assert!(!is_newer("0.4.0", "0.4.0"));
    assert!(
        !is_newer("0.4", "0.4.0"),
        "the same version written shorter"
    );
    assert!(!is_newer("0.3.9", "0.4.0"));
}

#[test]
fn only_a_version_shaped_answer_is_a_version() {
    assert_eq!(parse_version("0.4.0\n"), Some("0.4.0".to_owned()));
    assert_eq!(parse_version("  0.4.0-rc1 "), Some("0.4.0-rc1".to_owned()));
    for answer in ["", "\n", "<html>", "0.4.0/../x", "0.4.0 0.4.1"] {
        assert_eq!(parse_version(answer), None, "{answer:?}");
    }
}

#[test]
fn the_published_sum_is_the_first_field_in_lowercase() {
    let sum = "A".repeat(64);
    assert_eq!(
        published_sum(&format!("{sum}  vibememory-0.4.0-x.tar.gz\n")),
        Some("a".repeat(64))
    );
    assert_eq!(published_sum(""), None);
    assert_eq!(published_sum("abc  file"), None);
    assert_eq!(published_sum(&format!("{}  file", "z".repeat(64))), None);
}
