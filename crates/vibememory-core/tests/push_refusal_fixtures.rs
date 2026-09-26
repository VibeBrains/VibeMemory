//! Data-driven tests for reading a team host's refusal out of git's stderr
//! (`fixtures/host/pushRefusals.json`).

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use vibememory_core::push_refusal::{Remedy, parse, remedy};

const CASES: &str = include_str!("../../../fixtures/host/pushRefusals.json");

fn remedy_name(remedy: Remedy) -> &'static str {
    match remedy {
        Remedy::Cabinet => "cabinet",
        Remedy::Reclone => "reclone",
        Remedy::Transient => "transient",
    }
}

#[test]
fn push_refusal_fixtures() {
    let file: serde_json::Value = serde_json::from_str(CASES).unwrap();
    let mut failures = Vec::new();
    for case in file["cases"].as_array().unwrap() {
        let label = format!("pushRefusals.json#{}", case["id"]);
        assert!(
            !case["note"].as_str().unwrap().is_empty(),
            "{label}: empty note"
        );
        let actual = parse(case["stderr"].as_str().unwrap()).map(|refusal| {
            serde_json::json!({
                "code": refusal.code,
                "lines": refusal.lines,
                "remedy": remedy_name(remedy(&refusal.code)),
            })
        });
        let expected = (!case["expect"].is_null()).then(|| case["expect"].clone());
        if actual != expected {
            failures.push(format!("{label}: got {actual:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
