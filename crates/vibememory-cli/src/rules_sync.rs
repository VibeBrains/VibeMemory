//! A project's own rule files compared with the rules in force, and the person's answer applied.
//!
//! The project's files are read as they are — `CLAUDE.md`, `AGENTS.md`, `.claude/rules/`, `.vibe/rules/`,
//! `.cursor/rules/` — and every block of them is judged against the histories of the rules in force, which git of the
//! store holds whole. What to do about each is the project's mode: `advise` only says, `merge` drops duplicates and
//! brings old versions up to date, `override` lays the rules in force over every block that matches one, setting the
//! project's own version aside in the quarantine. A block that matches no rule is the project's and is never touched.

use std::path::{Path, PathBuf};
use std::time::Duration;

use vibememory_core::rules::Rule;
use vibememory_core::rules::compare::{Block, Finding, History, Merge, State, blocks, judge};

use crate::rules::{FILE_PREFIX, PERSONAL_RULES, PROJECT_RULES, TEAM_RULES};

const GIT: Duration = Duration::from_secs(20);

/// The file of a project's store directory that holds its mode.
pub const MODE_FILE: &str = "rules.mode";

/// What the engine does about a project's own rule files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Says what it found and what it would do; changes nothing.
    Advise,
    /// Drops duplicates, brings old versions and cleanly merged ones up to date.
    Merge,
    /// The rules in force over every block that matches one; the project's version goes to the quarantine.
    Override,
}

impl Mode {
    /// The word of the mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Advise => "advise",
            Self::Merge => "merge",
            Self::Override => "override",
        }
    }

    /// The mode a word names.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "advise" => Some(Self::Advise),
            "merge" => Some(Self::Merge),
            "override" => Some(Self::Override),
            _ => None,
        }
    }
}

/// A project's mode, from its directory in the store; `advise` when none was set.
#[must_use]
pub fn mode_of(project_dir: &Path) -> Mode {
    std::fs::read_to_string(project_dir.join(MODE_FILE))
        .ok()
        .and_then(|text| Mode::parse(&text))
        .unwrap_or(Mode::Advise)
}

/// A rule file of a project, and whether it is one rule or a document of sections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// Its path.
    pub path: PathBuf,
    /// One rule a file, as in a rules directory.
    pub whole: bool,
}

/// The project's own rule files: the engine's files are not among them.
#[must_use]
pub fn local_files(dir: &Path) -> Vec<LocalFile> {
    let mut files = Vec::new();
    for name in ["CLAUDE.md", "AGENTS.md", "CLAUDE.local.md"] {
        let path = dir.join(name);
        if path.is_file() {
            files.push(LocalFile { path, whole: false });
        }
    }
    for sub in [".claude/rules", ".vibe/rules", ".cursor/rules"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "md" || ext == "mdc")
                    && !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(FILE_PREFIX))
            })
            .collect();
        found.sort();
        files.extend(
            found
                .into_iter()
                .map(|path| LocalFile { path, whole: true }),
        );
    }
    files
}

/// Every version of a rule file in a store's history, oldest first, and the working copy last when it differs.
fn versions(store: &Path, relative: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let log = crate::git::run_with_timeout(
        crate::git::command(store, &["log", "--format=%H", "--reverse", "--", relative]),
        GIT,
    )
    .ok()
    .flatten()
    .unwrap_or_default();
    for commit in log.lines().filter(|line| !line.is_empty()) {
        let spec = format!("{commit}:{relative}");
        if let Some(text) =
            crate::git::run_with_timeout(crate::git::command(store, &["show", &spec]), GIT)
                .ok()
                .flatten()
            && let Ok(rule) = Rule::parse(&text)
        {
            let version = (rule.title, rule.body);
            if found.last() != Some(&version) {
                found.push(version);
            }
        }
    }
    if let Ok(rule) = std::fs::read_to_string(store.join(relative))
        .map_err(|_| ())
        .and_then(|text| Rule::parse(&text).map_err(|_| ()))
    {
        let version = (rule.title, rule.body);
        if found.last() != Some(&version) {
            found.push(version);
        }
    }
    found
}

/// The histories of the rules in force for a project, read from the stores' git.
#[must_use]
pub fn histories(personal_store: &Path, store: &Path, team: bool, project: &str) -> Vec<History> {
    let mut sources: Vec<(&Path, String)> = Vec::new();
    for (root, dir) in [
        (personal_store, PERSONAL_RULES.to_owned()),
        (
            store,
            if team {
                TEAM_RULES.to_owned()
            } else {
                String::new()
            },
        ),
        (store, format!("projects/{project}/{PROJECT_RULES}")),
    ] {
        if !dir.is_empty() {
            sources.push((root, dir));
        }
    }
    let mut by_id: std::collections::BTreeMap<String, History> = std::collections::BTreeMap::new();
    for (root, dir) in sources {
        let (rules, _) = crate::rules::read_rules(&root.join(&dir));
        for id in rules.keys() {
            // a rule of the level above is the one in force; its history is the one a project is measured by
            by_id.insert(
                id.clone(),
                History {
                    id: id.clone(),
                    versions: versions(root, &format!("{dir}/{id}.md")),
                },
            );
        }
    }
    by_id.into_values().collect()
}

/// One file judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Judged {
    /// The file.
    pub file: LocalFile,
    /// Its blocks judged, in file order.
    pub findings: Vec<Finding>,
}

/// Judges every rule file of a project.
#[must_use]
pub fn judge_project(dir: &Path, histories: &[History]) -> Vec<Judged> {
    local_files(dir)
        .into_iter()
        .filter_map(|file| {
            let text = std::fs::read_to_string(&file.path).ok()?;
            let name = file.path.file_name()?.to_string_lossy().into_owned();
            let found: Vec<Block> = blocks(&name, &text, file.whole);
            Some(Judged {
                findings: judge(&found, histories),
                file,
            })
        })
        .collect()
}

/// What applying a mode did to one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Blocks removed as duplicates.
    pub dropped: usize,
    /// Blocks brought up to the rule in force.
    pub updated: usize,
    /// Blocks whose project version went to the quarantine.
    pub set_aside: usize,
}

/// What a block becomes under a mode.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    /// Stays as it is.
    Keep,
    /// Goes: the agent gets the rule anyway.
    Drop,
    /// Its text becomes this.
    Replace(String),
}

fn verdict(finding: &Finding, mode: Mode, current: Option<&str>) -> Action {
    let replace =
        |text: Option<&str>| text.map_or(Action::Keep, |text| Action::Replace(text.to_owned()));
    match (&finding.state, mode) {
        (_, Mode::Advise) | (State::ProjectOnly, _) => Action::Keep,
        (State::Duplicate, _) => Action::Drop,
        (
            State::Custom {
                merged: Merge::Clean(text),
            },
            Mode::Merge,
        ) => Action::Replace(text.clone()),
        (State::Custom { .. }, Mode::Merge) => Action::Keep,
        (State::Stale { .. }, _) | (State::Custom { .. }, Mode::Override) => replace(current),
    }
}

/// Whether applying this sets the project's own version aside: its custom text is replaced by the rule in force.
fn sets_aside(finding: &Finding, mode: Mode, action: &Action) -> bool {
    matches!(finding.state, State::Custom { .. })
        && mode == Mode::Override
        && matches!(action, Action::Replace(_))
}

/// Applies a mode to one judged file: whole files are removed or rewritten, a document of sections has its sections
/// dropped or their text replaced, everything else in it byte for byte as it was.
///
/// # Errors
///
/// What the file system or the quarantine said.
pub fn apply(
    judged: &Judged,
    mode: Mode,
    histories: &[History],
    quarantine: &mut dyn FnMut(&str, &str) -> Result<(), String>,
) -> Result<Applied, String> {
    let mut applied = Applied::default();
    // two files may hold a block of one rule: the name of the set-aside version says which file it came from
    let file_name = judged
        .file
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let current = |finding: &Finding| -> Option<String> {
        let id = finding.rule.as_deref()?;
        histories
            .iter()
            .find(|history| history.id == id)
            .and_then(|history| history.versions.last())
            .map(|(_, body)| body.clone())
    };
    let mut note =
        |finding: &Finding, action: &Action, applied: &mut Applied| -> Result<(), String> {
            if sets_aside(finding, mode, action) {
                quarantine(
                    &format!(
                        "project-rule-{}-{file_name}",
                        finding.rule.as_deref().unwrap_or("block")
                    ),
                    &finding.block.text,
                )?;
                applied.set_aside += 1;
            } else {
                match action {
                    Action::Drop => applied.dropped += 1,
                    Action::Replace(_) => applied.updated += 1,
                    Action::Keep => {}
                }
            }
            Ok(())
        };
    let original = std::fs::read_to_string(&judged.file.path).map_err(|error| error.to_string())?;
    if judged.file.whole {
        let Some(finding) = judged.findings.first() else {
            return Ok(applied);
        };
        let action = verdict(finding, mode, current(finding).as_deref());
        note(finding, &action, &mut applied)?;
        match action {
            Action::Drop => {
                std::fs::remove_file(&judged.file.path).map_err(|error| error.to_string())?;
            }
            Action::Replace(text) => {
                let replaced = original.replacen(finding.block.text.trim_end(), text.trim_end(), 1);
                std::fs::write(&judged.file.path, replaced).map_err(|error| error.to_string())?;
            }
            Action::Keep => {}
        }
        return Ok(applied);
    }
    let mut sections = sections(&original);
    for finding in &judged.findings {
        let action = verdict(finding, mode, current(finding).as_deref());
        if action == Action::Keep {
            continue;
        }
        let Some(section) = sections.iter_mut().find(|section| {
            section.heading.as_deref() == Some(finding.block.heading.as_str()) && !section.done
        }) else {
            continue;
        };
        note(finding, &action, &mut applied)?;
        section.done = true;
        match action {
            Action::Drop => section.text.clear(),
            Action::Replace(text) => {
                section.text = format!("## {}\n\n{}\n\n", finding.block.heading, text.trim_end());
            }
            Action::Keep => {}
        }
    }
    let rewritten: String = sections.into_iter().map(|section| section.text).collect();
    if rewritten != original {
        std::fs::write(&judged.file.path, rewritten).map_err(|error| error.to_string())?;
    }
    Ok(applied)
}

struct Section {
    heading: Option<String>,
    text: String,
    done: bool,
}

/// A document cut at its `## ` headings, every byte kept: joined back, the sections are the file.
fn sections(text: &str) -> Vec<Section> {
    let mut found = vec![Section {
        heading: None,
        text: String::new(),
        done: false,
    }];
    for line in text.split_inclusive('\n') {
        if let Some(title) = line.strip_prefix("## ") {
            found.push(Section {
                heading: Some(title.trim().to_owned()),
                text: String::new(),
                done: false,
            });
        }
        if let Some(last) = found.last_mut() {
            last.text.push_str(line);
        }
    }
    found
}
