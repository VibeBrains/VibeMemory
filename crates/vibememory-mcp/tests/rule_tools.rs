//! Rules and skills written and read over MCP, on a store laid out as an installed machine has it: the level is the
//! person's word, a rule that repeats one in force is refused, a team's rule is a proposal.

// The test writes a store on disk, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::needless_pass_by_value
)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use vibememory_mcp::memories::StoreMemories;
use vibememory_mcp::tools::{self, Caller};

const AGENT: &str = "dsh-desktop";

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vibememory-rule-tools-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("engine/store/projects/app")).unwrap();
        fs::create_dir_all(path.join("engine/stores/acme/store/projects/api")).unwrap();
        Self(path)
    }

    fn personal(&self) -> StoreMemories {
        StoreMemories::new(self.0.join("engine/store"), "mac-test".to_owned())
    }

    fn team(&self) -> StoreMemories {
        StoreMemories::in_engine(
            self.0.join("engine"),
            self.0.join("engine/stores/acme/store"),
            "mac-test".to_owned(),
        )
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn call(
    name: &str,
    arguments: Value,
    project: Option<&str>,
    store: &StoreMemories,
) -> Result<Value, String> {
    let caller = Caller::member_of_team(AGENT, "bob", project);
    tools::call(name, &arguments, &caller, store)
}

#[test]
fn a_rule_without_a_level_is_refused_so_the_agent_asks() {
    let temp = Temp::new("level");
    let refused = call(
        "rule_save",
        json!({ "title": "Tone", "body": "Direct." }),
        None,
        &temp.personal(),
    )
    .unwrap_err();
    assert!(refused.starts_with("levelRequired"), "{refused}");
    assert!(
        !temp.0.join("engine/store/config/rules").exists(),
        "nothing is written"
    );
}

#[test]
fn a_personal_rule_is_written_and_in_force_everywhere() {
    let temp = Temp::new("personal");
    let store = temp.personal();
    let saved = call(
        "rule_save",
        json!({ "level": "personal", "title": "Tests before commit", "body": "Run cargo test." }),
        None,
        &store,
    )
    .unwrap();
    assert_eq!(
        saved["saved"], "tests-before-commit",
        "the id is made from a latin title"
    );
    assert!(
        temp.0
            .join("engine/store/config/rules/tests-before-commit.md")
            .is_file()
    );

    let rules = call("rules_get", json!({}), Some("app"), &store).unwrap();
    assert_eq!(rules["rules"][0]["id"], "tests-before-commit", "{rules}");
}

#[test]
fn a_rule_about_the_same_thing_is_refused_until_forced() {
    let temp = Temp::new("similar");
    let store = temp.personal();
    call("rule_save", json!({ "level": "personal", "id": "commits", "title": "Commits in Russian", "body": "Subject and body in Russian." }), None, &store).unwrap();
    let again = json!({ "level": "personal", "id": "commit-language", "title": "Commits in Russian", "body": "Write commits in Russian." });
    let refused = call("rule_save", again.clone(), None, &store).unwrap_err();
    assert!(refused.starts_with("similarFound: commits"), "{refused}");
    let mut forced = again;
    forced["force"] = json!(true);
    assert!(call("rule_save", forced, None, &store).is_ok());
}

#[test]
fn a_project_rule_replaces_the_persons_and_says_so() {
    let temp = Temp::new("project");
    let store = temp.personal();
    call(
        "rule_save",
        json!({ "level": "personal", "id": "tests", "title": "Tests", "body": "cargo test." }),
        None,
        &store,
    )
    .unwrap();
    call(
        "rule_save",
        json!({ "level": "project", "id": "tests", "title": "Tests", "body": "bun test." }),
        Some("app"),
        &store,
    )
    .unwrap();
    assert!(
        temp.0
            .join("engine/store/projects/app/rules/tests.md")
            .is_file()
    );
    let rules = call("rules_get", json!({}), Some("app"), &store).unwrap();
    let tests: Vec<&Value> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|rule| rule["id"] == "tests")
        .collect();
    assert_eq!(tests.len(), 1, "{rules}");
    assert_eq!(tests[0]["level"], "project");
    assert_eq!(tests[0]["replaces"], json!(["personal"]));
}

#[test]
fn a_team_rule_is_a_proposal_and_only_in_a_teams_store() {
    let temp = Temp::new("team");
    let rule =
        json!({ "level": "team", "id": "style", "title": "Style", "body": "As the team writes." });
    let proposed = call("rule_save", rule.clone(), Some("api"), &temp.team()).unwrap();
    assert_eq!(proposed["proposed"], "style");
    assert!(
        temp.0
            .join("engine/stores/acme/store/proposals/rules/style.bob.md")
            .is_file()
    );
    assert!(
        !temp
            .0
            .join("engine/stores/acme/store/rules/style.md")
            .exists(),
        "not in force until accepted"
    );
    let refused = call("rule_save", rule, Some("app"), &temp.personal()).unwrap_err();
    assert!(refused.contains("not a team's"), "{refused}");
}

#[test]
fn a_title_without_latin_words_needs_an_id() {
    let temp = Temp::new("cyrillic");
    let refused = call(
        "rule_save",
        json!({ "level": "personal", "title": "Тон", "body": "Прямо." }),
        None,
        &temp.personal(),
    )
    .unwrap_err();
    assert!(refused.starts_with("id is required"), "{refused}");
}

#[test]
fn skills_are_checked_saved_and_listed() {
    let temp = Temp::new("skills");
    let store = temp.personal();
    let refused = call("skill_save", json!({ "level": "personal", "name": "Bad_Name", "content": "---\nname: Bad_Name\ndescription: x\n---\n" }), None, &store).unwrap_err();
    assert!(refused.contains("kebab-case"), "{refused}");
    call("skill_save", json!({ "level": "personal", "name": "deploy", "content": "---\nname: deploy\ndescription: How to deploy.\n---\nSteps.\n" }), None, &store).unwrap();
    assert!(
        temp.0
            .join("engine/store/config/skills/deploy/SKILL.md")
            .is_file()
    );
    let listed = call("skill_get", json!({}), None, &store).unwrap();
    assert_eq!(listed["skills"][0]["name"], "deploy", "{listed}");
    let one = call("skill_get", json!({ "name": "deploy" }), None, &store).unwrap();
    assert!(one["content"].as_str().unwrap().contains("Steps."));
}

#[test]
fn rules_and_skills_are_resources_for_an_agent_without_their_directories() {
    let temp = Temp::new("resources");
    let store = temp.personal();
    call(
        "rule_save",
        json!({ "level": "personal", "id": "tone", "title": "Tone", "body": "Direct." }),
        None,
        &store,
    )
    .unwrap();
    call("skill_save", json!({ "level": "personal", "name": "deploy", "content": "---\nname: deploy\ndescription: How to deploy.\n---\nSteps.\n" }), None, &store).unwrap();
    let caller = Caller::member_of_team(AGENT, "bob", None);
    let listed = vibememory_mcp::rule_tools::resources(&caller, &store);
    let uris: Vec<&str> = listed
        .iter()
        .filter_map(|resource| resource["uri"].as_str())
        .collect();
    assert_eq!(
        uris,
        vec!["vibememory://rules/tone", "vibememory://skills/deploy"]
    );
    let rule =
        vibememory_mcp::rule_tools::read_resource("vibememory://rules/tone", &caller, &store)
            .unwrap();
    assert_eq!(rule[0]["text"], "## Tone\n\nDirect.\n");
    let skill =
        vibememory_mcp::rule_tools::read_resource("vibememory://skills/deploy", &caller, &store)
            .unwrap();
    assert!(skill[0]["text"].as_str().unwrap().contains("Steps."));
    let missing =
        vibememory_mcp::rule_tools::read_resource("vibememory://rules/none", &caller, &store)
            .unwrap_err();
    assert!(missing.contains("no rule none"), "{missing}");
}
