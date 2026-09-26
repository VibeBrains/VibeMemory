//! Data-driven tests for the engine configuration (`config.json` of the engine directory).
//! Expectations live in `fixtures/config/configScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use serde::Deserialize;
use vibememory_cli::config::{Config, ConfigError, DesktopStore};
use vibememory_core::naming::PathSyntax;

const SCENARIOS: &str = include_str!("../../../fixtures/config/configScenarios.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Observed,
    Computed,
    Unverified,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    provenance: Provenance,
    note: String,
    /// The file as it sits on disk.
    text: String,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    #[serde(default)]
    ok: Option<Loaded>,
    /// One of `invalid`, `missing`, `relativeRoot`, `store`, `naming`.
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Loaded {
    machine_id: String,
    roots: BTreeMap<String, String>,
    /// `auto`, or the directory named in the file.
    desktop_store: String,
    /// Team store id to its `storeName`; absent means none.
    #[serde(default)]
    stores: BTreeMap<String, String>,
}

fn code(error: &ConfigError) -> &'static str {
    match error {
        ConfigError::Invalid(_) => "invalid",
        ConfigError::Missing { .. } => "missing",
        ConfigError::RelativeRoot { .. } => "relativeRoot",
        ConfigError::Store { .. } => "store",
        ConfigError::Naming(_) => "naming",
    }
}

fn desktop_store_text(store: &DesktopStore) -> String {
    match store {
        DesktopStore::Auto => "auto".to_owned(),
        DesktopStore::Path(path) => path.clone(),
    }
}

#[test]
fn config_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("configScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();

    for case in &file.cases {
        let label = format!("configScenarios.json#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }

        match (
            Config::parse(&case.text, PathSyntax::Posix),
            &case.expect.ok,
            &case.expect.error,
        ) {
            (Ok(config), Some(want), None) => {
                if config.machine_id != want.machine_id {
                    failures.push(format!(
                        "{label}: machineId {:?}, expected {:?}",
                        config.machine_id, want.machine_id
                    ));
                }
                if config.roots != want.roots {
                    failures.push(format!(
                        "{label}: roots {:?}, expected {:?}",
                        config.roots, want.roots
                    ));
                }
                let stores: BTreeMap<String, String> = config
                    .stores
                    .iter()
                    .map(|(id, store)| (id.clone(), store.store_name.clone()))
                    .collect();
                if stores != want.stores {
                    failures.push(format!(
                        "{label}: stores {stores:?}, expected {:?}",
                        want.stores
                    ));
                }
                let store = desktop_store_text(&config.desktop_store);
                if store != want.desktop_store {
                    failures.push(format!(
                        "{label}: desktopStore {store:?}, expected {:?}",
                        want.desktop_store
                    ));
                }
            }
            (Err(error), None, Some(want)) => {
                if code(&error) != want {
                    failures.push(format!(
                        "{label}: refused as {}, expected {want} ({error})",
                        code(&error)
                    ));
                }
            }
            (Ok(_), None, Some(want)) => {
                failures.push(format!("{label}: accepted, expected {want}"));
            }
            (Err(error), Some(_), None) => {
                failures.push(format!("{label}: refused — {error}"));
            }
            _ => failures.push(format!("{label}: give exactly one of ok and error")),
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
