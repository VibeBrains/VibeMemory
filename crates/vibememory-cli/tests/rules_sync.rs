//! A project's own rule files judged against the rules' history in a real git store, and the modes applied.

// The test writes files and runs git, so the purity gate is lifted here.
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
use std::path::{Path, PathBuf};

use support::{TempDir, git, git_repo_with_commit};
use vibememory_cli::rules_sync::{Mode, apply, histories, judge_project};
use vibememory_core::rules::compare::State;

fn rule(id: &str, title: &str, body: &str) -> String {
    format!("---\nid: {id}\ntitle: {title}\nlevel: personal\n---\n{body}")
}

/// A personal store whose rules have a history: `tests` and `tone` changed once, `language` never.
fn store(temp: &TempDir) -> PathBuf {
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let rules = store.join("config/rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(
        rules.join("tests.md"),
        rule("tests", "Тесты", "Гонять cargo test.\n"),
    )
    .unwrap();
    fs::write(
        rules.join("tone.md"),
        rule("tone", "Тон", "Прямо.\nБез воды.\n"),
    )
    .unwrap();
    fs::write(
        rules.join("language.md"),
        rule("language", "Язык", "Отвечать по-русски.\n"),
    )
    .unwrap();
    git(&store, &["add", "."]);
    git(&store, &["commit", "--quiet", "-m", "first"]);
    fs::write(
        rules.join("tests.md"),
        rule("tests", "Тесты", "Гонять cargo test и clippy.\n"),
    )
    .unwrap();
    fs::write(
        rules.join("tone.md"),
        rule("tone", "Тон", "Прямо и строго.\nБез воды.\n"),
    )
    .unwrap();
    git(&store, &["add", "."]);
    git(&store, &["commit", "--quiet", "-m", "second"]);
    store
}

const CLAUDE_MD: &str = "# Проект\n\nВступление проекта.\n\n## Язык\n\nОтвечать по-русски.\n\n## Тесты\n\nГонять cargo test.\n\n\
                         ## Тон\n\nПрямо.\nБез воды.\nИ с примерами.\n\n## Сборка\n\nЧерез make.\n";

fn project(temp: &TempDir) -> PathBuf {
    let dir = temp.dir("work");
    fs::write(dir.join("CLAUDE.md"), CLAUDE_MD).unwrap();
    fs::create_dir_all(dir.join(".vibe/rules")).unwrap();
    fs::write(
        dir.join(".vibe/rules/tone.mdc"),
        "---\ndescription: tone\n---\n# Тон\n\nМягко.\nБез воды.\n",
    )
    .unwrap();
    dir
}

fn quarantine_into(dir: &Path) -> impl FnMut(&str, &str) -> Result<(), String> + '_ {
    move |name: &str, text: &str| {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), text).map_err(|error| error.to_string())
    }
}

#[test]
fn each_block_is_judged_by_the_rules_history() {
    let temp = TempDir::new("rules-sync-judge");
    let store = store(&temp);
    let dir = project(&temp);
    let histories = histories(&store, &store, false, "app");
    assert_eq!(
        histories
            .iter()
            .find(|history| history.id == "tests")
            .unwrap()
            .versions
            .len(),
        2
    );
    let judged = judge_project(&dir, &histories);
    let states: Vec<(String, String)> = judged
        .iter()
        .flat_map(|file| &file.findings)
        .map(|finding| {
            let state = match &finding.state {
                State::Duplicate => "duplicate",
                State::Stale { .. } => "stale",
                State::Custom { .. } => "custom",
                State::ProjectOnly => "projectOnly",
            };
            (finding.block.heading.clone(), state.to_owned())
        })
        .collect();
    let want: Vec<(String, String)> = [
        ("CLAUDE.md", "projectOnly"),
        ("Язык", "duplicate"),
        ("Тесты", "stale"),
        ("Тон", "custom"),
        ("Сборка", "projectOnly"),
        ("Тон", "custom"),
    ]
    .iter()
    .map(|(heading, state)| ((*heading).to_owned(), (*state).to_owned()))
    .collect();
    assert_eq!(states, want);
}

#[test]
fn merge_drops_duplicates_updates_old_versions_and_merges_clean_changes() {
    let temp = TempDir::new("rules-sync-merge");
    let store = store(&temp);
    let dir = project(&temp);
    let histories = histories(&store, &store, false, "app");
    let quarantine = temp.dir("quarantine");
    let mut set_aside = quarantine_into(&quarantine);
    for file in judge_project(&dir, &histories) {
        apply(&file, Mode::Merge, &histories, &mut set_aside).unwrap();
    }
    let text = fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert!(
        text.starts_with("# Проект\n\nВступление проекта.\n"),
        "the project's own text stays: {text}"
    );
    assert!(!text.contains("## Язык"), "the duplicate is gone: {text}");
    assert!(
        text.contains("## Тесты\n\nГонять cargo test и clippy."),
        "the old version is updated: {text}"
    );
    assert!(
        text.contains("Прямо и строго.\nБез воды.\nИ с примерами."),
        "the rule's change merged into the project's own: {text}"
    );
    assert!(text.contains("## Сборка\n\nЧерез make."), "{text}");
    assert!(
        fs::read_to_string(dir.join(".vibe/rules/tone.mdc"))
            .unwrap()
            .contains("Мягко."),
        "a conflicting change is not merged by itself"
    );
    assert!(
        fs::read_dir(&quarantine).unwrap().next().is_none(),
        "merge sets nothing aside"
    );
}

#[test]
fn override_lays_the_rule_over_the_projects_version_and_keeps_that_aside() {
    let temp = TempDir::new("rules-sync-override");
    let store = store(&temp);
    let dir = project(&temp);
    let histories = histories(&store, &store, false, "app");
    let quarantine = temp.dir("quarantine");
    let mut set_aside = quarantine_into(&quarantine);
    for file in judge_project(&dir, &histories) {
        apply(&file, Mode::Override, &histories, &mut set_aside).unwrap();
    }
    let rule_file = fs::read_to_string(dir.join(".vibe/rules/tone.mdc")).unwrap();
    assert!(
        rule_file.contains("Прямо и строго.") && !rule_file.contains("Мягко."),
        "{rule_file}"
    );
    assert!(
        rule_file.starts_with("---\ndescription: tone\n---\n# Тон"),
        "the file's own frame stays: {rule_file}"
    );
    let text = fs::read_to_string(dir.join("CLAUDE.md")).unwrap();
    assert!(
        text.contains("## Тон\n\nПрямо и строго.\nБез воды.\n\n"),
        "{text}"
    );
    assert!(
        text.contains("## Сборка"),
        "the project's own rule is never touched"
    );
    assert_eq!(
        fs::read_dir(&quarantine).unwrap().count(),
        2,
        "both custom versions are kept aside"
    );
}

#[test]
fn a_session_hears_about_the_projects_rule_files_once_a_day() {
    let temp = TempDir::new("rules-sync-notice");
    let store = store(&temp);
    let dir = project(&temp);
    let engine = temp.dir("engine");
    fs::create_dir_all(store.join("projects/app")).unwrap();
    let say = |day: &str| {
        vibememory_cli::rules_sync::notice(&engine, &store, &store, false, "app", &dir, day)
    };
    let first = say("2026-10-07").expect("the project's files repeat the rules");
    assert!(
        first.contains("1 duplicate(s), 1 old version(s), 2 changed by hand"),
        "{first}"
    );
    assert_eq!(say("2026-10-07"), None, "once a day");
    assert!(say("2026-10-08").is_some(), "and again the next day");
    fs::write(store.join("projects/app/rules.mode"), "merge\n").unwrap();
    assert_eq!(
        say("2026-10-09"),
        None,
        "a project in merge mode is handled by the tick, not told"
    );
}
