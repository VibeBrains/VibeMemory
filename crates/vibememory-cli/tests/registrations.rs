//! Which memory servers Claude Code has registered, and what `doctor` says about them: read from
//! its configuration, never written.

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
use vibememory_cli::install::Layout;
use vibememory_cli::registrations::{Registered, advice, read};
use vibememory_core::naming::PathSyntax;

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    }
}

fn config() -> Config {
    Config::parse(r#"{"machineId":"mac-main"}"#, PathSyntax::Posix).unwrap()
}

#[test]
fn servers_are_read_from_the_moved_configuration_directory() {
    let temp = TempDir::new("registrations-moved");
    let layout = layout(&temp);
    let text =
        r#"{"mcpServers":{"vibememory":{},"vibememory-personal":{"type":"http"},"other":{}}}"#;
    fs::write(layout.config_dir.join(".claude.json"), text).unwrap();
    let before = fs::read(layout.config_dir.join(".claude.json")).unwrap();

    let registered = read(&layout);
    assert_eq!(
        registered,
        Registered {
            local: true,
            teams: vec!["personal".to_owned()],
        }
    );
    let lines = advice(&layout, &config(), &registered);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].contains("claude mcp remove -s user vibememory-personal"),
        "{lines:?}"
    );
    assert_eq!(
        fs::read(layout.config_dir.join(".claude.json")).unwrap(),
        before,
        "the engine never writes Claude Code's configuration"
    );
}

#[test]
fn a_team_reached_only_by_its_own_server_is_told_to_route_first() {
    let temp = TempDir::new("registrations-team");
    let layout = layout(&temp);
    fs::write(
        temp.path().join(".claude.json"),
        r#"{"mcpServers":{"vibememory-acme":{"type":"http"}}}"#,
    )
    .unwrap();
    let registered = read(&layout);
    assert_eq!(registered.teams, vec!["acme".to_owned()]);
    assert!(!registered.local);
    let lines = advice(&layout, &config(), &registered);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("claude mcp add -s user vibememory")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("route add") && !line.contains("claude mcp remove")),
        "the only way to a team is not removed before its directories are routed: {lines:?}"
    );
}
