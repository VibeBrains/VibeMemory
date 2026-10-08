//! A team's rules on this machine, held until the person is told what came and from whom.
//!
//! The versions shown are kept beside the team's clone, in `rules-shown/`, as rule files: what this machine's person
//! was told, not something the team shares. The tick lays out what was shown; `SessionStart` and `rules status` tell
//! what waits and mark it shown, and it goes out with the engine's next run. The decision itself is
//! [`vibememory_core::rules::shown::gate`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use vibememory_core::rules::Rule;
use vibememory_core::rules::shown::{Change, gate};

use crate::rules::{TEAM_RULES, read_rules, write_rule};

/// The directory of the versions shown, beside the team's clone.
pub const SHOWN_DIR: &str = "rules-shown";

/// How long reading who changed a rule may take.
const AUTHOR_TIMEOUT: Duration = Duration::from_secs(20);

/// A team's rule waiting to be shown, with who last changed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// The rule as the team's store holds it.
    pub rule: Rule,
    /// New or changed.
    pub change: Change,
    /// The author of the store's last commit of it, when git says.
    pub author: Option<String>,
}

/// A team's rules on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamRules {
    /// What goes out, as last shown.
    pub laid: Vec<Rule>,
    /// What waits to be shown.
    pub waiting: Vec<Waiting>,
    /// Rule files of the store that do not read.
    pub warnings: Vec<String>,
}

/// Where the versions shown of a team's rules are kept: beside its clone `<state>/store`.
#[must_use]
pub fn shown_dir(team_store: &Path) -> PathBuf {
    team_store
        .parent()
        .map_or_else(|| team_store.join(SHOWN_DIR), |state| state.join(SHOWN_DIR))
}

/// A team's rules split by what this machine showed. `authors` reads git for each rule that waits: the tick, which
/// only lays out, does without.
#[must_use]
pub fn team_rules(team_store: &Path, authors: bool) -> TeamRules {
    let (current, warnings) = read_rules(&team_store.join(TEAM_RULES));
    let (shown, _) = read_rules(&shown_dir(team_store));
    let current: Vec<Rule> = current.into_values().collect();
    let shown: Vec<Rule> = shown.into_values().collect();
    let gated = gate(&current, &shown);
    TeamRules {
        laid: gated.laid.into_iter().cloned().collect(),
        waiting: gated
            .unseen
            .into_iter()
            .map(|unseen| Waiting {
                author: authors
                    .then(|| author(team_store, unseen.rule.id.as_str()))
                    .flatten(),
                rule: unseen.rule.clone(),
                change: unseen.change,
            })
            .collect(),
        warnings,
    }
}

/// Who made the store's last commit of a rule.
fn author(team_store: &Path, id: &str) -> Option<String> {
    let path = format!("{TEAM_RULES}/{id}.md");
    crate::git::run_with_timeout(
        crate::git::command(team_store, &["log", "-1", "--format=%an", "--", &path]),
        AUTHOR_TIMEOUT,
    )
    .ok()
    .flatten()
    .map(|name| name.trim().to_owned())
    .filter(|name| !name.is_empty())
}

/// Records the team's rules as the store holds them now as shown; versions of rules the team removed go.
///
/// # Errors
///
/// What the file system said.
pub fn mark_shown(team_store: &Path) -> Result<(), String> {
    let dir = shown_dir(team_store);
    let (current, _) = read_rules(&team_store.join(TEAM_RULES));
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    for rule in current.values() {
        write_rule(&dir, rule)?;
    }
    let (shown, _) = read_rules(&dir);
    for id in shown.keys().filter(|id| !current.contains_key(*id)) {
        let path = dir.join(format!("{id}.md"));
        std::fs::remove_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

/// The lines that tell a person what waits: one a rule, with the change and who made it.
#[must_use]
pub fn lines(waiting: &[Waiting]) -> Vec<String> {
    waiting
        .iter()
        .map(|waiting| {
            let flags = match (waiting.rule.enforced, waiting.rule.paths.is_empty()) {
                (true, _) => " [enforced]",
                (false, false) => " [for some files]",
                (false, true) => "",
            };
            format!(
                "{} {} — {}{flags}{}",
                waiting.change.as_str(),
                waiting.rule.id,
                waiting.rule.title,
                waiting
                    .author
                    .as_ref()
                    .map(|author| format!(", by {author}"))
                    .unwrap_or_default()
            )
        })
        .collect()
}

/// What `SessionStart` says in a team's project when its rules changed, and the change marked shown: it goes out with
/// the engine's next run. Nothing when nothing waits.
#[must_use]
pub fn notice(team_store: &Path, team: &str) -> Option<String> {
    let rules = team_rules(team_store, true);
    if rules.waiting.is_empty() {
        return None;
    }
    let told = lines(&rules.waiting).join("; ");
    mark_shown(team_store).ok()?;
    Some(format!(
        "VibeMemory: team {team} has new or changed rules — {told}. They are in force from the engine's next run; \
         `vibememory rules status` shows them, `vibememory rule show <id>` reads one. Tell the person what came."
    ))
}
