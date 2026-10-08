//! Data-driven tests for rules and skills: the formats, the levels stacked, an agent's file assembled and read back,
//! a project's rule files judged against the rules' histories, a team's rules held until shown.
//! Every expectation lives in `fixtures/rules/rulesScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde_json::Value;
use vibememory_core::rules::assembly::{Piece, assemble, disassemble};
use vibememory_core::rules::compare::{Block, History, Merge, State, blocks, judge, similar};
use vibememory_core::rules::layers::resolve;
use vibememory_core::rules::shown::gate;
use vibememory_core::rules::skill::parse_skill;
use vibememory_core::rules::{Level, Rule};

const SCENARIOS: &str = include_str!("../../../fixtures/rules/rulesScenarios.json");

fn scenarios() -> Value {
    serde_json::from_str(SCENARIOS).expect("the fixture is JSON")
}

fn text(value: &Value) -> &str {
    value.as_str().expect("a string")
}

fn rules_of(files: &Value) -> Vec<Rule> {
    files
        .as_array()
        .expect("a list of rule files")
        .iter()
        .map(|file| Rule::parse(text(file)).expect("a valid rule file"))
        .collect()
}

fn levels(value: &Value) -> Vec<Level> {
    value
        .as_array()
        .expect("levels")
        .iter()
        .map(|level| Level::parse(text(level)).unwrap())
        .collect()
}

#[test]
fn rule_files_read_as_written_and_refuse_what_an_agent_must_not_get() {
    for case in scenarios()["rules"].as_array().unwrap() {
        let id = text(&case["id"]);
        let parsed = Rule::parse(text(&case["file"]));
        if let Some(error) = case.get("error") {
            let refused = parsed.expect_err(id).to_string();
            assert!(refused.contains(text(error)), "{id}: {refused}");
            continue;
        }
        let rule = parsed.unwrap_or_else(|error| panic!("{id}: {error}"));
        let expect = &case["expect"];
        assert_eq!(rule.id.as_str(), text(&expect["id"]), "{id}");
        assert_eq!(rule.title, text(&expect["title"]), "{id}");
        assert_eq!(rule.level.as_str(), text(&expect["level"]), "{id}");
        assert_eq!(rule.absolute, expect["absolute"].as_bool().unwrap(), "{id}");
        assert_eq!(rule.enforced, expect["enforced"].as_bool().unwrap(), "{id}");
        let paths: Vec<&str> = expect["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(text)
            .collect();
        assert_eq!(rule.paths, paths, "{id}");
        assert_eq!(rule.body, text(&expect["body"]), "{id}");
        let extra: Vec<&str> = rule.extra.iter().map(|(key, _)| key.as_str()).collect();
        let want: Vec<&str> = expect.get("extra").map_or_else(Vec::new, |keys| {
            keys.as_array().unwrap().iter().map(text).collect()
        });
        assert_eq!(extra, want, "{id}: unknown keys are carried, not dropped");
        // written back, it reads as the same rule
        assert_eq!(
            Rule::parse(&rule.render()).unwrap(),
            rule,
            "{id}: render round trip"
        );
    }
}

#[test]
fn skill_manifests_are_checked_as_every_agent_would() {
    for case in scenarios()["skills"].as_array().unwrap() {
        let id = text(&case["id"]);
        let parsed = parse_skill(text(&case["directory"]), text(&case["file"]));
        if let Some(error) = case.get("error") {
            let refused = parsed.expect_err(id).to_string();
            assert!(refused.contains(text(error)), "{id}: {refused}");
            continue;
        }
        let manifest = parsed.unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(manifest.name, text(&case["expect"]["name"]), "{id}");
        assert_eq!(
            manifest.description,
            text(&case["expect"]["description"]),
            "{id}"
        );
        let only: Vec<&str> = case["expect"]["claudeOnly"]
            .as_array()
            .unwrap()
            .iter()
            .map(text)
            .collect();
        assert_eq!(manifest.claude_only, only, "{id}");
    }
}

#[test]
fn levels_stack_with_absolute_and_enforced_held_and_conflicts_named() {
    for case in scenarios()["layers"].as_array().unwrap() {
        let id = text(&case["id"]);
        let resolved = resolve(
            &rules_of(&case["personal"]),
            &rules_of(&case["team"]),
            &rules_of(&case["project"]),
        );
        let in_force: Vec<(String, String, Vec<Level>)> = resolved
            .rules
            .iter()
            .map(|rule| {
                (
                    rule.rule.id.as_str().to_owned(),
                    rule.rule.level.as_str().to_owned(),
                    rule.replaces.clone(),
                )
            })
            .collect();
        let want: Vec<(String, String, Vec<Level>)> = case["expect"]["inForce"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    text(&row[0]).to_owned(),
                    text(&row[1]).to_owned(),
                    levels(&row[2]),
                )
            })
            .collect();
        assert_eq!(in_force, want, "{id}");
        let kept: Vec<(String, String)> = resolved
            .kept
            .iter()
            .map(|kept| {
                (
                    kept.kept.id.as_str().to_owned(),
                    kept.refused.level.as_str().to_owned(),
                )
            })
            .collect();
        let want: Vec<(String, String)> = case["expect"]["kept"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (text(&row[0]).to_owned(), text(&row[1]).to_owned()))
            .collect();
        assert_eq!(kept, want, "{id}");
        assert_eq!(
            resolved.conflicts.len(),
            case["expect"]["conflicts"].as_array().unwrap().len(),
            "{id}"
        );
    }
}

#[test]
fn an_agents_file_reads_back_into_the_rules_it_was_written_from() {
    for case in scenarios()["assembly"].as_array().unwrap() {
        let id = text(&case["id"]);
        let rules: Vec<(Rule, Vec<Level>)> = case["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (Rule::parse(text(&row[0])).unwrap(), levels(&row[1])))
            .collect();
        let refs: Vec<(&Rule, &[Level])> = rules
            .iter()
            .map(|(rule, levels)| (rule, levels.as_slice()))
            .collect();
        let written = assemble(case["base"].as_str(), &refs);

        // untouched, it reads back as nothing edited
        for piece in disassemble(&written).unwrap() {
            assert_eq!(
                piece.version_now().as_deref(),
                piece.from(),
                "{id}: an untouched piece is unchanged"
            );
        }

        let mut edited = written.clone();
        for edit in case["edits"].as_array().unwrap() {
            assert!(
                edited.contains(text(&edit[0])),
                "{id}: the edit finds its text"
            );
            edited = edited.replacen(text(&edit[0]), text(&edit[1]), 1);
        }
        let read = disassemble(&edited);
        if let Some(error) = case.get("error") {
            let refused = read.expect_err(id).to_string();
            assert!(refused.contains(text(error)), "{id}: {refused}");
            continue;
        }
        let pieces = read.unwrap();
        let want = case["expect"].as_array().unwrap();
        assert_eq!(pieces.len(), want.len(), "{id}: {pieces:?}");
        for (piece, want) in pieces.iter().zip(want) {
            match (piece, text(&want["piece"])) {
                (Piece::Base { .. }, "base") => {
                    let edited = piece.version_now().as_deref() != piece.from();
                    assert_eq!(edited, want["edited"].as_bool().unwrap(), "{id}: base");
                }
                (Piece::Rule { id: rule, body, .. }, "rule") => {
                    assert_eq!(rule, text(&want["id"]), "{id}");
                    let edited = piece.version_now().as_deref() != piece.from();
                    assert_eq!(edited, want["edited"].as_bool().unwrap(), "{id}: {rule}");
                    assert_eq!(body, text(&want["body"]), "{id}: {rule}");
                }
                (Piece::Outside(outside), "outside") => {
                    assert_eq!(outside, text(&want["text"]), "{id}");
                }
                (other, kind) => panic!("{id}: expected a {kind} piece, read {other:?}"),
            }
        }
    }
}

#[test]
fn a_projects_rule_files_are_judged_by_the_rules_history() {
    for case in scenarios()["judge"].as_array().unwrap() {
        let id = text(&case["id"]);
        let found: Vec<Block> = blocks(
            text(&case["file"]),
            text(&case["text"]),
            case["whole"].as_bool().unwrap(),
        );
        let histories: Vec<History> = case["histories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|history| History {
                id: text(&history["id"]).to_owned(),
                versions: history["versions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pair| (text(&pair[0]).to_owned(), text(&pair[1]).to_owned()))
                    .collect(),
            })
            .collect();
        let findings = judge(&found, &histories);
        let want = case["expect"].as_array().unwrap();
        assert_eq!(findings.len(), want.len(), "{id}: {findings:#?}");
        for (finding, want) in findings.iter().zip(want) {
            let heading = text(&want["heading"]);
            assert_eq!(finding.block.heading, heading, "{id}");
            assert_eq!(
                finding.rule.as_deref(),
                want["rule"].as_str(),
                "{id}: {heading}"
            );
            assert_eq!(
                finding.guessed,
                want["guessed"].as_bool().unwrap(),
                "{id}: {heading}"
            );
            match (&finding.state, text(&want["state"])) {
                (State::Duplicate, "duplicate") | (State::ProjectOnly, "projectOnly") => {}
                (State::Stale { behind }, "stale") => {
                    assert_eq!(
                        *behind as u64,
                        want["behind"].as_u64().unwrap(),
                        "{id}: {heading}"
                    );
                }
                (State::Custom { merged }, "custom") => {
                    let (clean, merged) = match merged {
                        Merge::Clean(text) => (true, text),
                        Merge::Conflicted(text) => (false, text),
                    };
                    assert_eq!(
                        clean,
                        want["clean"].as_bool().unwrap(),
                        "{id}: {heading}: {merged}"
                    );
                    assert_eq!(merged, text(&want["merged"]), "{id}: {heading}");
                }
                (state, kind) => panic!("{id}: {heading}: expected {kind}, judged {state:?}"),
            }
        }
    }
}

#[test]
fn a_rule_about_to_be_written_finds_the_ones_it_repeats() {
    for case in scenarios()["similar"].as_array().unwrap() {
        let id = text(&case["id"]);
        let rules: Vec<(String, String, String)> = case["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    text(&row[0]).to_owned(),
                    text(&row[1]).to_owned(),
                    text(&row[2]).to_owned(),
                )
            })
            .collect();
        let found: Vec<&str> = similar(text(&case["title"]), text(&case["text"]), &rules)
            .into_iter()
            .map(|(rule, _)| rule)
            .collect();
        let want: Vec<&str> = case["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(text)
            .collect();
        assert_eq!(found, want, "{id}");
    }
}

#[test]
fn a_teams_rules_go_out_as_they_were_shown() {
    for case in scenarios()["shown"].as_array().unwrap() {
        let id = text(&case["id"]);
        let current = rules_of(&case["current"]);
        let shown = rules_of(&case["shown"]);
        let gated = gate(&current, &shown);
        let laid: Vec<(String, String)> = gated
            .laid
            .iter()
            .map(|rule| (rule.id.as_str().to_owned(), rule.body.clone()))
            .collect();
        let want: Vec<(String, String)> = case["expect"]["laid"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (text(&row[0]).to_owned(), text(&row[1]).to_owned()))
            .collect();
        assert_eq!(laid, want, "{id}");
        let unseen: Vec<(String, &str)> = gated
            .unseen
            .iter()
            .map(|unseen| (unseen.rule.id.as_str().to_owned(), unseen.change.as_str()))
            .collect();
        let want: Vec<(String, &str)> = case["expect"]["unseen"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (text(&row[0]).to_owned(), text(&row[1])))
            .collect();
        assert_eq!(unseen, want, "{id}");
    }
}
