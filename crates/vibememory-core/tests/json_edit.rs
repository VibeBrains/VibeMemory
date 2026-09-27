//! Editing one top-level member of the owner's `config.json` leaves every other byte as it was.

#![allow(clippy::unwrap_used)]

use vibememory_core::json_edit::{remove_top_level, set_top_level};

const OWNER: &str = "{\n  \"machineId\": \"mac-main\",\n  \"ignoreCwd\": [\"/\",   \"/Users/o\"],\n  \"stores\": {\"old\": {\"cwd\": []}},\n  \"remote\": \"x\"\n}\n";

#[test]
fn a_member_is_replaced_in_place() {
    let out = set_top_level(OWNER, "stores", "{\n  \"acme\": 1\n}").unwrap();
    assert_eq!(
        out,
        "{\n  \"machineId\": \"mac-main\",\n  \"ignoreCwd\": [\"/\",   \"/Users/o\"],\n  \"stores\": {\n    \"acme\": 1\n  },\n  \"remote\": \"x\"\n}\n"
    );
}

#[test]
fn a_missing_member_is_added_last_in_the_files_own_indent() {
    let text = "{\n    \"machineId\": \"m\"\n}\n";
    let out = set_top_level(text, "stores", "{}").unwrap();
    assert_eq!(out, "{\n    \"machineId\": \"m\",\n    \"stores\": {}\n}\n");
    serde_json::from_str::<serde_json::Value>(&out).unwrap();
}

#[test]
fn a_member_is_removed_with_its_separator() {
    let out = remove_top_level(OWNER, "stores").unwrap();
    assert_eq!(
        out,
        "{\n  \"machineId\": \"mac-main\",\n  \"ignoreCwd\": [\"/\",   \"/Users/o\"],\n  \"remote\": \"x\"\n}\n"
    );
    let first = remove_top_level(OWNER, "machineId").unwrap();
    assert!(first.starts_with("{\n  \"ignoreCwd\""), "{first}");
    serde_json::from_str::<serde_json::Value>(&first).unwrap();
}

#[test]
fn strings_with_braces_and_escapes_do_not_confuse_the_scan() {
    let text = r#"{"a": "}{\"", "stores": {"x": ["]"]}, "b": 2}"#;
    let out = set_top_level(text, "stores", "1").unwrap();
    assert_eq!(out, r#"{"a": "}{\"", "stores": 1, "b": 2}"#);
}

#[test]
fn text_that_is_not_an_object_is_refused() {
    assert!(set_top_level("[1]", "stores", "1").is_err());
    assert!(set_top_level("{\"a\": ", "stores", "1").is_err());
}
