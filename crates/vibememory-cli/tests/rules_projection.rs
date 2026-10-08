//! Rules and skills written out for the agents of a machine, and the agents' edits taken back: against real
//! directories laid out as an installed machine has them, under a home of the test's own.

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

use support::{TempDir, git};
use vibememory_cli::rules::{
    Agents, Projected, ProjectsRun, project_personal, project_projects, read_rules,
};
use vibememory_core::desktop::roots::Roots;
use vibememory_core::naming::PathSyntax;
use vibememory_core::rules::Rule;

const STAMP: &str = "2026-10-07T20:00:00Z";

struct Machine {
    _temp: TempDir,
    home: PathBuf,
    config_dir: PathBuf,
    engine: PathBuf,
    store: PathBuf,
}

/// A machine with Claude Code and DSH; Codex only when asked.
fn machine(label: &str, codex: bool) -> Machine {
    let temp = TempDir::new(label);
    let home = temp.dir("home");
    fs::create_dir_all(home.join(".dsh")).unwrap();
    if codex {
        fs::create_dir_all(home.join(".codex")).unwrap();
    }
    let config_dir = temp.dir("home/.claude");
    let engine = temp.dir("home/.vibememory");
    let store = temp.dir("home/.vibememory/store");
    fs::create_dir_all(store.join("config/rules")).unwrap();
    fs::write(
        store.join("config/CLAUDE.md"),
        "# Правила владельца\n\nКоротко.\n",
    )
    .unwrap();
    Machine {
        home,
        config_dir,
        engine,
        store,
        _temp: temp,
    }
}

fn rule(id: &str, title: &str, level: &str, body: &str) -> String {
    format!("---\nid: {id}\ntitle: {title}\nlevel: {level}\n---\n{body}")
}

fn agents(m: &Machine) -> Agents {
    Agents::of(
        &m.config_dir,
        Some(&m.home),
        &vibememory_cli::config::RulesConfig::default(),
    )
}

fn personal(m: &Machine) -> Projected {
    project_personal(&m.store, &m.engine, &agents(m), STAMP)
}

fn quarantined(m: &Machine) -> Vec<String> {
    fs::read_dir(m.engine.join("quarantine"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_persons_rules_reach_every_agent_in_the_form_it_reads() {
    let m = machine("rules-personal", true);
    fs::write(
        m.store.join("config/rules/tone.md"),
        rule("tone", "Тон", "personal", "Прямо.\n"),
    )
    .unwrap();
    let done = personal(&m);
    assert!(done.problems.is_empty(), "{:?}", done.problems);

    let claude = fs::read_to_string(m.config_dir.join("rules/vm-tone.md")).unwrap();
    assert!(
        claude.contains("## Тон") && claude.contains("Прямо."),
        "{claude}"
    );
    for agent in [
        m.home.join(".dsh/AGENTS.md"),
        m.home.join(".codex/AGENTS.md"),
    ] {
        let text = fs::read_to_string(&agent).unwrap();
        assert!(
            text.contains("Коротко."),
            "the owner's CLAUDE.md comes first: {text}"
        );
        assert!(text.contains("## Тон"), "{text}");
    }
    // a second run has nothing to write
    assert!(personal(&m).written.is_empty());
}

#[test]
fn the_plain_copy_080_wrote_becomes_the_base_not_a_second_copy() {
    let m = machine("rules-migration", false);
    let dsh = m.home.join(".dsh/AGENTS.md");
    fs::write(&dsh, "# Правила владельца\n\nКоротко.\n").unwrap();
    personal(&m);
    let text = fs::read_to_string(&dsh).unwrap();
    assert_eq!(text.matches("Коротко.").count(), 1, "{text}");
}

#[test]
fn an_edit_in_an_agents_file_is_an_edit_of_the_rule() {
    let m = machine("rules-edit", false);
    fs::write(
        m.store.join("config/rules/tone.md"),
        rule("tone", "Тон", "personal", "Прямо.\n"),
    )
    .unwrap();
    personal(&m);
    let dsh = m.home.join(".dsh/AGENTS.md");
    let edited = fs::read_to_string(&dsh)
        .unwrap()
        .replace("Прямо.", "Прямо и строго.")
        .replace("Коротко.", "Коротко и ясно.");
    fs::write(&dsh, format!("{edited}\n## Моё\n\nТолько здесь.\n")).unwrap();

    let done = personal(&m);
    assert_eq!(
        done.taken,
        vec!["base".to_owned(), "tone".to_owned()],
        "{done:?}"
    );
    let (rules, _) = read_rules(&m.store.join("config/rules"));
    assert_eq!(rules["tone"].body, "Прямо и строго.\n");
    assert!(
        fs::read_to_string(m.store.join("config/CLAUDE.md"))
            .unwrap()
            .contains("Коротко и ясно.")
    );
    assert!(
        fs::read_to_string(m.config_dir.join("rules/vm-tone.md"))
            .unwrap()
            .contains("Прямо и строго."),
        "the edit reaches the other agents"
    );
    assert!(
        fs::read_to_string(&dsh).unwrap().contains("Только здесь."),
        "the person's own text outside the markers stays"
    );
}

#[test]
fn a_rule_changed_on_both_sides_keeps_the_store_and_sets_the_agents_version_aside() {
    let m = machine("rules-conflict", false);
    let stored = m.store.join("config/rules/tone.md");
    fs::write(&stored, rule("tone", "Тон", "personal", "Прямо.\n")).unwrap();
    personal(&m);
    fs::write(&stored, rule("tone", "Тон", "personal", "Строго.\n")).unwrap();
    let claude = m.config_dir.join("rules/vm-tone.md");
    fs::write(
        &claude,
        fs::read_to_string(&claude)
            .unwrap()
            .replace("Прямо.", "Мягко."),
    )
    .unwrap();

    let done = personal(&m);
    assert_eq!(done.conflicts, vec!["tone".to_owned()], "{done:?}");
    assert!(fs::read_to_string(&claude).unwrap().contains("Строго."));
    assert!(
        quarantined(&m)
            .iter()
            .any(|name| name.starts_with("rule-tone-")),
        "{:?}",
        quarantined(&m)
    );
}

#[test]
fn skills_are_linked_for_the_agents_and_one_an_agent_would_refuse_is_named() {
    let m = machine("rules-skills", false);
    let skills = m.store.join("config/skills");
    for (name, manifest) in [
        (
            "watch",
            "---\nname: watch\ndescription: Смотрит видео.\n---\n",
        ),
        ("Bad_Name", "---\nname: Bad_Name\ndescription: x\n---\n"),
    ] {
        fs::create_dir_all(skills.join(name)).unwrap();
        fs::write(skills.join(name).join("SKILL.md"), manifest).unwrap();
    }
    fs::create_dir_all(skills.join("synced/abc")).unwrap();
    // a skill the person keeps by hand under the same name as a stored one is left alone
    fs::create_dir_all(skills.join("notes")).unwrap();
    fs::write(
        skills.join("notes/SKILL.md"),
        "---\nname: notes\ndescription: n\n---\n",
    )
    .unwrap();
    let shared = m.home.join(".agents/skills");
    fs::create_dir_all(shared.join("notes")).unwrap();

    let done = personal(&m);
    assert!(fs::read_link(shared.join("watch")).is_ok(), "{done:?}");
    assert!(!shared.join("Bad_Name").exists());
    assert!(
        !shared.join("synced").exists(),
        "Claude's own synced skills are not other agents'"
    );
    assert!(
        !fs::symlink_metadata(shared.join("notes"))
            .unwrap()
            .is_symlink()
    );
    assert!(
        done.warnings
            .iter()
            .any(|warning| warning.starts_with("skill Bad_Name")),
        "a skill an agent would refuse is said by doctor, not repeated by every tick: {:?}",
        done.warnings
    );
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    let said = vibememory_cli::rules::check(&m.store, &agents(&m));
    for name in ["skill Bad_Name", shared.join("notes").to_str().unwrap()] {
        assert!(
            said.iter().any(|warning| warning.starts_with(name)),
            "doctor says {name}: {said:?}"
        );
    }

    fs::remove_dir_all(skills.join("watch")).unwrap();
    personal(&m);
    assert!(
        fs::symlink_metadata(shared.join("watch")).is_err(),
        "a link to a removed skill goes"
    );
}

#[test]
fn an_agent_left_out_of_rules_agents_gets_its_files_back_without_the_engines_part() {
    let m = machine("rules-released", true);
    fs::write(
        m.store.join("config/rules/commits.md"),
        rule("commits", "Коммиты", "personal", "По-русски.\n"),
    )
    .unwrap();
    fs::create_dir_all(m.store.join("config/skills/watch")).unwrap();
    fs::write(
        m.store.join("config/skills/watch/SKILL.md"),
        "---\nname: watch\ndescription: Смотрит видео.\n---\n",
    )
    .unwrap();
    personal(&m);
    let codex = m.home.join(".codex/AGENTS.md");
    let written = fs::read_to_string(&codex).unwrap();
    fs::write(&codex, format!("{written}\nСвоя строка для Codex.\n")).unwrap();
    assert!(fs::read_link(m.home.join(".codex/skills/watch")).is_ok());

    let settings: vibememory_cli::config::RulesConfig =
        serde_json::from_str(r#"{"agents": ["dsh"]}"#).unwrap();
    let agents = Agents::of(&m.config_dir, Some(&m.home), &settings);
    let done = project_personal(&m.store, &m.engine, &agents, STAMP);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert_eq!(
        fs::read_to_string(&codex).unwrap().trim(),
        "Своя строка для Codex.",
        "the person's own line stays, the engine's header, base and rules go"
    );
    assert!(fs::symlink_metadata(m.home.join(".codex/skills/watch")).is_err());
    assert!(
        fs::read_to_string(m.home.join(".dsh/AGENTS.md"))
            .unwrap()
            .contains("По-русски."),
        "the agent still in the list keeps its rules"
    );

    fs::write(&codex, written).unwrap();
    let done = project_personal(&m.store, &m.engine, &agents, STAMP);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(
        !codex.exists(),
        "a file that held only the engine's part goes whole"
    );
}

fn project_store(m: &Machine, cwd: &Path) {
    fs::create_dir_all(m.store.join("machines/mac-test")).unwrap();
    let links = serde_json::json!({
        "version": 1,
        "links": [{
            "enc": "-work-app", "name": "app", "cwd": cwd.display().to_string(),
            "syntax": "posix", "source": "observed", "predicted": false
        }]
    });
    fs::write(
        m.store.join("machines/mac-test/links.json"),
        links.to_string(),
    )
    .unwrap();
}

fn projects(m: &Machine, team: Option<&str>) -> Projected {
    let roots = Roots::new(std::collections::BTreeMap::new(), PathSyntax::Posix);
    let (personal, _) = read_rules(&m.store.join("config/rules"));
    let personal: Vec<Rule> = personal.into_values().collect();
    let agents = agents(m);
    project_projects(&ProjectsRun {
        store: &m.store,
        engine_dir: &m.engine,
        machine_id: "mac-test",
        roots: &roots,
        personal: &personal,
        personal_store: &m.store,
        team,
        agents: &agents,
        stamp: STAMP,
    })
}

#[test]
fn a_projects_rules_go_into_its_directory_and_git_is_told_to_ignore_them() {
    let m = machine("rules-project", false);
    let cwd = fs::canonicalize(m.home.parent().unwrap())
        .unwrap()
        .join("work");
    fs::create_dir_all(&cwd).unwrap();
    git(&cwd, &["init", "--quiet"]);
    project_store(&m, &cwd);
    fs::write(
        m.store.join("config/rules/tests.md"),
        rule("tests", "Тесты", "personal", "cargo test.\n"),
    )
    .unwrap();
    let rules = m.store.join("projects/app/rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(
        rules.join("tests.md"),
        rule("tests", "Тесты", "project", "bun test.\n"),
    )
    .unwrap();

    let done = projects(&m, None);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    let claude = fs::read_to_string(cwd.join(".claude/rules/vm-tests.md")).unwrap();
    assert!(
        claude.contains("bun test.") && claude.contains("replaces the personal rule"),
        "{claude}"
    );
    assert!(
        fs::read_to_string(cwd.join("AGENTS.local.md"))
            .unwrap()
            .contains("bun test.")
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&cwd)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&status.stdout),
        "",
        "git sees none of it"
    );

    // the project's own edit is the project rule's
    let file = cwd.join("AGENTS.local.md");
    fs::write(
        &file,
        fs::read_to_string(&file)
            .unwrap()
            .replace("bun test.", "bun run test."),
    )
    .unwrap();
    let done = projects(&m, None);
    assert_eq!(done.taken, vec!["tests".to_owned()]);
    assert!(
        fs::read_to_string(rules.join("tests.md"))
            .unwrap()
            .contains("bun run test.")
    );

    // rules gone, the files go
    fs::remove_file(rules.join("tests.md")).unwrap();
    projects(&m, None);
    assert!(!cwd.join(".claude/rules/vm-tests.md").exists());
    assert!(!cwd.join("AGENTS.local.md").exists());
}

#[test]
fn a_member_cannot_change_a_team_rule_from_a_project_file() {
    let m = machine("rules-team", false);
    let cwd = fs::canonicalize(m.home.parent().unwrap())
        .unwrap()
        .join("work");
    fs::create_dir_all(&cwd).unwrap();
    project_store(&m, &cwd);
    fs::create_dir_all(m.store.join("rules")).unwrap();
    let stored = m.store.join("rules/style.md");
    fs::write(&stored, rule("style", "Стиль", "team", "Как у команды.\n")).unwrap();
    vibememory_cli::rules_shown::mark_shown(&m.store).unwrap();
    projects(&m, Some("acme"));
    let file = cwd.join(".claude/rules/vm-style.md");
    fs::write(
        &file,
        fs::read_to_string(&file)
            .unwrap()
            .replace("Как у команды.", "Как хочу."),
    )
    .unwrap();

    let done = projects(&m, Some("acme"));
    assert!(done.taken.is_empty(), "{done:?}");
    assert!(
        fs::read_to_string(&stored)
            .unwrap()
            .contains("Как у команды.")
    );
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("Как у команды."),
        "written back as the team has it"
    );
    assert!(
        done.problems
            .iter()
            .any(|problem| problem.contains("rule add --level team")),
        "{:?}",
        done.problems
    );
}

#[test]
fn a_teams_rule_goes_out_once_shown_and_a_change_keeps_the_shown_text_until_then() {
    let m = machine("rules-team-shown", false);
    let cwd = fs::canonicalize(m.home.parent().unwrap())
        .unwrap()
        .join("work");
    fs::create_dir_all(&cwd).unwrap();
    project_store(&m, &cwd);
    fs::create_dir_all(m.store.join("rules")).unwrap();
    let stored = m.store.join("rules/review.md");
    fs::write(&stored, rule("review", "Ревью", "team", "Через ревью.\n")).unwrap();
    let file = cwd.join(".claude/rules/vm-review.md");

    projects(&m, Some("acme"));
    assert!(
        !file.exists(),
        "a rule nobody was told about does not go out"
    );
    let note =
        vibememory_cli::rules_shown::notice(&m.store, "acme").expect("the session hears of it");
    assert!(note.contains("new review — Ревью"), "{note}");
    assert!(
        vibememory_cli::rules_shown::notice(&m.store, "acme").is_none(),
        "told once"
    );
    projects(&m, Some("acme"));
    assert!(fs::read_to_string(&file).unwrap().contains("Через ревью."));

    fs::write(
        &stored,
        "---\nid: review\ntitle: Ревью\nlevel: team\nenforced: true\n---\nЧерез два ревью.\n",
    )
    .unwrap();
    projects(&m, Some("acme"));
    let laid = fs::read_to_string(&file).unwrap();
    assert!(
        laid.contains("Через ревью.") && !laid.contains("два"),
        "the shown text stays until the change is told: {laid}"
    );
    let note = vibememory_cli::rules_shown::notice(&m.store, "acme").unwrap();
    assert!(note.contains("changed review — Ревью [enforced]"), "{note}");
    projects(&m, Some("acme"));
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("Через два ревью.")
    );

    fs::remove_file(&stored).unwrap();
    projects(&m, Some("acme"));
    assert!(!file.exists(), "a rule the team took away goes at once");
}

#[test]
fn codex_reads_the_projects_own_instructions_and_its_rules_from_one_file() {
    let m = machine("rules-codex-project", true);
    fs::remove_dir_all(m.home.join(".dsh")).unwrap();
    let cwd = fs::canonicalize(m.home.parent().unwrap())
        .unwrap()
        .join("work");
    fs::create_dir_all(&cwd).unwrap();
    git(&cwd, &["init", "--quiet"]);
    project_store(&m, &cwd);
    fs::write(cwd.join("AGENTS.md"), "Own instructions.\n").unwrap();
    let rules = m.store.join("projects/app/rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(
        rules.join("tests.md"),
        rule("tests", "Тесты", "project", "bun test.\n"),
    )
    .unwrap();
    let skill = m.store.join("projects/app/skills/deploy");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: deploy\ndescription: How to deploy.\n---\nSteps.\n",
    )
    .unwrap();

    let done = projects(&m, None);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    let codex = cwd.join("AGENTS.override.md");
    let written = fs::read_to_string(&codex).unwrap();
    assert!(
        written.contains("Own instructions.") && written.contains("bun test."),
        "the project's own file and its rules in the one Codex reads: {written}"
    );
    assert!(
        !cwd.join("AGENTS.local.md").exists(),
        "no DSH here, no file of its"
    );
    assert!(
        fs::read_link(cwd.join(".agents/skills/deploy")).is_ok(),
        "Codex reads the project's .agents/skills"
    );
    let exclude = fs::read_to_string(cwd.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/AGENTS.override.md"), "{exclude}");

    // the project's own part edited where Codex reads it goes back into AGENTS.md, and the person's line stays
    fs::write(
        &codex,
        format!(
            "{}\nMy own line.\n",
            written.replace("Own instructions.", "Own instructions, edited.")
        ),
    )
    .unwrap();
    let done = projects(&m, None);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert_eq!(
        fs::read_to_string(cwd.join("AGENTS.md")).unwrap(),
        "Own instructions, edited.\n"
    );
    let written = fs::read_to_string(&codex).unwrap();
    assert!(
        written.contains("Own instructions, edited.") && written.contains("My own line."),
        "{written}"
    );

    // no rules any more: the engine's file goes, the person's line stays
    fs::remove_file(rules.join("tests.md")).unwrap();
    fs::remove_dir_all(m.store.join("projects/app/skills")).unwrap();
    projects(&m, None);
    assert_eq!(fs::read_to_string(&codex).unwrap().trim(), "My own line.");
}

#[test]
fn a_persons_own_project_files_are_kept() {
    let m = machine("rules-own-project-files", true);
    let cwd = fs::canonicalize(m.home.parent().unwrap())
        .unwrap()
        .join("work");
    fs::create_dir_all(&cwd).unwrap();
    git(&cwd, &["init", "--quiet"]);
    project_store(&m, &cwd);
    fs::write(cwd.join("AGENTS.local.md"), "My local notes.\n").unwrap();
    fs::write(cwd.join("AGENTS.override.md"), "My override.\n").unwrap();
    let rules = m.store.join("projects/app/rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(
        rules.join("tests.md"),
        rule("tests", "Тесты", "project", "bun test.\n"),
    )
    .unwrap();

    let done = projects(&m, None);
    let local = fs::read_to_string(cwd.join("AGENTS.local.md")).unwrap();
    assert!(
        local.contains("My local notes.") && local.contains("bun test."),
        "the person's text beside the rules: {local}"
    );
    assert_eq!(
        fs::read_to_string(cwd.join("AGENTS.override.md")).unwrap(),
        "My override.\n",
        "a person's own override is not the engine's to write"
    );
    assert!(
        done.warnings
            .iter()
            .any(|warning| warning.contains("AGENTS.override.md")),
        "{:?}",
        done.warnings
    );

    // a file whose markers were cut by hand is named and left as it is
    let cut = "<!-- vibememory:rule tests@0000 -->\n## Тесты\n\nhalf of a rule\n";
    fs::write(cwd.join("AGENTS.local.md"), cut).unwrap();
    let done = projects(&m, None);
    assert_eq!(
        fs::read_to_string(cwd.join("AGENTS.local.md")).unwrap(),
        cut
    );
    assert!(
        done.problems
            .iter()
            .any(|problem| problem.contains("AGENTS.local.md")),
        "{:?}",
        done.problems
    );
}
