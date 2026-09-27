//! What `doctor` says about the routes of a machine: a pattern naming a directory that is not
//! here, a pattern spelled in another case than the disk, and a project kept in the personal store
//! although a team's pattern now takes its directory.

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

use support::TempDir;
use vibememory_cli::config::Config;
use vibememory_core::links::{LINKS_VERSION, LinkRecord, LinkSource, LinksFile};
use vibememory_core::naming::PathSyntax;

fn config(patterns: &[String]) -> Config {
    Config::parse(
        &serde_json::json!({
            "machineId": "mac-main",
            "stores": { "acme": { "cwd": patterns } }
        })
        .to_string(),
        PathSyntax::Posix,
    )
    .unwrap()
}

#[test]
fn a_pattern_on_a_directory_that_is_not_here_is_named() {
    let temp = TempDir::new("route-missing");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let lines =
        vibememory_cli::route::warnings(&config(&[format!("{base}/gone/**")]), &temp.dir("store"));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("does not exist"), "{lines:?}");
}

#[test]
fn a_pattern_in_the_disks_own_spelling_says_nothing() {
    let temp = TempDir::new("route-exact");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    temp.dir("Work/acme");
    let lines = vibememory_cli::route::warnings(
        &config(&[format!("{base}/Work/acme/**")]),
        &temp.dir("store"),
    );
    assert!(lines.is_empty(), "{lines:?}");
}

#[cfg(target_os = "macos")]
#[test]
fn a_pattern_in_another_case_is_named_with_the_disks_spelling() {
    let temp = TempDir::new("route-case");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    temp.dir("Work/acme");
    let lines = vibememory_cli::route::warnings(
        &config(&[format!("{base}/work/acme/**")]),
        &temp.dir("store"),
    );
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains(&format!("{base}/Work/acme/**")),
        "the line gives the pattern as the disk spells it: {lines:?}"
    );
}

#[test]
fn a_personal_project_under_a_teams_pattern_is_told_to_move() {
    let temp = TempDir::new("route-personal");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    temp.dir("work/acme");
    let store = temp.dir("store");
    let record_file = LinksFile {
        version: LINKS_VERSION,
        links: vec![LinkRecord {
            enc: "-work-acme".to_owned(),
            name: "acme".to_owned(),
            cwd: format!("{base}/work/acme"),
            syntax: PathSyntax::Posix,
            source: LinkSource::Observed,
            predicted: false,
            confirmed_by: None,
        }],
    };
    fs::create_dir_all(store.join("machines/mac-main")).unwrap();
    fs::write(
        store.join("machines/mac-main/links.json"),
        serde_json::to_string(&record_file).unwrap(),
    )
    .unwrap();
    let lines = vibememory_cli::route::warnings(&config(&[format!("{base}/work/acme/**")]), &store);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("project move"), "{lines:?}");
}

#[test]
fn a_route_is_added_and_removed_without_touching_the_rest_of_the_file() {
    let temp = TempDir::new("route-add");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    temp.dir("work/acme");
    let text = "{\n  \"machineId\": \"mac-main\",\n  \"ignoreCwd\": [\"/\"]\n}\n";

    let vibememory_cli::route::Changed::Text {
        text: added,
        patterns,
    } = vibememory_cli::route::add(text, &format!("{base}/work/acme"), "acme").unwrap()
    else {
        panic!("a new route changes the text")
    };
    assert_eq!(patterns, vec![format!("{base}/work/acme/**")]);
    assert!(
        added.starts_with(
            "{\n  \"machineId\": \"mac-main\",\n  \"ignoreCwd\": [\"/\"],\n  \"stores\""
        ),
        "the owner's members stay byte for byte: {added}"
    );
    let config = Config::parse(&added, PathSyntax::Posix).unwrap();
    assert_eq!(
        config
            .routes
            .route(&format!("{base}/work/acme"), PathSyntax::Posix)
            .unwrap(),
        Some("acme")
    );
    assert_eq!(
        vibememory_cli::route::add(&added, &format!("{base}/work/acme"), "acme").unwrap(),
        vibememory_cli::route::Changed::Already
    );
    assert!(
        vibememory_cli::route::add(&added, &format!("{base}/work/acme"), "beta").is_err(),
        "a directory is never routed to two teams"
    );

    let vibememory_cli::route::Changed::Text { text: removed, .. } =
        vibememory_cli::route::remove(&added, &format!("{base}/work/acme")).unwrap()
    else {
        panic!("removing a route changes the text")
    };
    assert_eq!(removed, text, "the file is as it was before the route");
}

#[test]
fn a_route_to_a_directory_that_is_not_here_is_refused() {
    let temp = TempDir::new("route-add-missing");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let text = "{\"machineId\": \"mac-main\"}";
    assert!(vibememory_cli::route::add(text, &format!("{base}/nowhere"), "acme").is_err());
}

#[test]
fn a_directory_under_a_wider_route_is_not_carved_out() {
    let temp = TempDir::new("route-wider");
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    temp.dir("work/acme");
    let text = serde_json::json!({
        "machineId": "mac-main",
        "stores": { "acme": { "cwd": [format!("{base}/work/**")] } }
    })
    .to_string();
    let refused = vibememory_cli::route::remove(&text, &format!("{base}/work/acme")).unwrap_err();
    assert!(refused.contains("wider"), "{refused}");
}
