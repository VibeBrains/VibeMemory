//! A skill's `SKILL.md`: the Agent Skills format Claude Code, `DeepSeek` Harness and Codex read alike.
//!
//! A skill that one agent would skip with a warning is refused at writing, with the reason: the person learns it
//! when they save the skill, not a week later from an agent that never saw it.

use super::RulesError;
use super::frontmatter::{self, Value};
use super::rule::is_slug;

/// The file every skill directory holds.
pub const SKILL_FILE: &str = "SKILL.md";

/// The directory of a skill's scripts: code an agent runs, not text it reads.
pub const SKILL_SCRIPTS: &str = "scripts";

/// The longest name: the Agent Skills format's limit.
pub const MAX_NAME: usize = 64;

/// The longest description: the Agent Skills format's limit; it stays in every agent's context.
pub const MAX_DESCRIPTION: usize = 1024;

/// The largest `SKILL.md`: its body is loaded whole when the skill is used, so a long reference goes beside it.
pub const MAX_FILE: usize = 64 * 1024;

/// Frontmatter keys Claude Code reads and the other agents ignore: the skill works there, without them.
pub const CLAUDE_ONLY: &[&str] = &[
    "allowed-tools",
    "disallowed-tools",
    "hooks",
    "context",
    "agent",
    "model",
    "effort",
    "background",
    "shell",
    "argument-hint",
    "arguments",
    "when_to_use",
];

/// What a `SKILL.md` declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifest {
    /// The skill's name: kebab-case, the same as its directory.
    pub name: String,
    /// When to use it: what an agent decides by.
    pub description: String,
    /// Keys only Claude Code reads, as written.
    pub claude_only: Vec<String>,
}

/// Reads and checks a `SKILL.md`.
///
/// `directory` is the name of the directory it lies in: a skill whose name differs from it is listed under one name
/// and loaded under the other, and DSH warns about it.
///
/// # Errors
///
/// [`RulesError::Invalid`] naming what an agent would refuse, [`RulesError::HoldsToken`] for a token in the file.
pub fn parse_skill(directory: &str, text: &str) -> Result<SkillManifest, RulesError> {
    if text.len() > MAX_FILE {
        return Err(RulesError::Invalid(format!(
            "{SKILL_FILE} is {} bytes, more than {MAX_FILE}: move references beside it",
            text.len()
        )));
    }
    if crate::token::holds_token(text.as_bytes()) {
        return Err(RulesError::HoldsToken);
    }
    let split = frontmatter::split(text)?;
    let name = split
        .text("name")
        .ok_or(RulesError::Missing("name"))?
        .trim();
    if !is_slug(name) || name.len() > MAX_NAME {
        return Err(RulesError::Invalid(format!(
            "the name {name:?} is not kebab-case of at most {MAX_NAME} characters"
        )));
    }
    if name != directory {
        return Err(RulesError::Invalid(format!(
            "the name {name:?} differs from its directory {directory:?}"
        )));
    }
    let description = split
        .text("description")
        .ok_or(RulesError::Missing("description"))?
        .trim();
    if description.is_empty() {
        return Err(RulesError::Missing("description"));
    }
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(RulesError::Invalid(format!(
            "the description is longer than {MAX_DESCRIPTION} characters"
        )));
    }
    for key in ["disable-model-invocation", "user-invocable"] {
        if let Some(value) = split.get(key)
            && !matches!(value, Value::Text(text) if text == "true" || text == "false")
        {
            return Err(RulesError::NotBoolean(if key == "user-invocable" {
                "user-invocable"
            } else {
                "disable-model-invocation"
            }));
        }
    }
    Ok(SkillManifest {
        name: name.to_owned(),
        description: description.to_owned(),
        claude_only: split
            .fields
            .iter()
            .map(|(key, _)| key.clone())
            .filter(|key| CLAUDE_ONLY.contains(&key.as_str()))
            .collect(),
    })
}
