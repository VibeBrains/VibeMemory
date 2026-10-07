//! One rule: a file with a frontmatter that names it, and the text an agent follows.
//!
//! The id is what the rule is known by for good: overriding, comparing and the history all go by it, so it is a
//! slug a file name can carry on every system and never changes when the title does.

use std::fmt::Write as _;

use super::RulesError;
use super::frontmatter::{self, Value};

/// The longest id a rule may have: it becomes a file name, and a short one reads in a marker.
pub const MAX_ID: usize = 64;

/// Frontmatter keys this format names itself.
const KNOWN: &[&str] = &["id", "title", "level", "absolute", "enforced", "paths"];

/// A rule's permanent name: lowercase letters and digits in words joined by `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleId(String);

impl RuleId {
    /// Checks an id.
    ///
    /// # Errors
    ///
    /// [`RulesError::BadId`] for anything that is not a kebab-case slug of at most [`MAX_ID`] characters.
    pub fn parse(id: &str) -> Result<Self, RulesError> {
        if is_slug(id) && id.len() <= MAX_ID {
            Ok(Self(id.to_owned()))
        } else {
            Err(RulesError::BadId(id.to_owned()))
        }
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RuleId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Lowercase ASCII letters and digits in non-empty words joined by single hyphens: the name rule DSH and the Agent
/// Skills format apply to a skill, and the one a rule id follows too.
#[must_use]
pub fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whose rule it is, from the bottom of the stack up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// The person's own: every agent of theirs on every machine.
    Personal,
    /// A team's: every member, in the team's projects.
    Team,
    /// A project's: whoever has the project.
    Project,
}

impl Level {
    /// The word the frontmatter and the tools use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Team => "team",
            Self::Project => "project",
        }
    }

    /// The level a word names.
    ///
    /// # Errors
    ///
    /// [`RulesError::BadLevel`] for any other word.
    pub fn parse(word: &str) -> Result<Self, RulesError> {
        match word {
            "personal" => Ok(Self::Personal),
            "team" => Ok(Self::Team),
            "project" => Ok(Self::Project),
            other => Err(RulesError::BadLevel(other.to_owned())),
        }
    }
}

/// A rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// Its permanent name.
    pub id: RuleId,
    /// One line a person reads in a list.
    pub title: String,
    /// Whose it is.
    pub level: Level,
    /// A personal rule nothing above it overrides: attribution, secrets.
    pub absolute: bool,
    /// A team rule a project or a member does not override; set by the team's owner or admins.
    pub enforced: bool,
    /// Globs of the files it is about; empty means always.
    pub paths: Vec<String>,
    /// The text, without the frontmatter, ending with one newline.
    pub body: String,
    /// Keys of a later version of the format, carried untouched.
    pub extra: Vec<(String, Value)>,
}

impl Rule {
    /// Reads a rule file.
    ///
    /// # Errors
    ///
    /// What [`frontmatter::split`] refuses, a missing or bad `id`, `title` or `level`, a flag that is not a boolean,
    /// and whatever [`Rule::validate`] refuses.
    pub fn parse(text: &str) -> Result<Self, RulesError> {
        let split = frontmatter::split(text)?;
        let id = RuleId::parse(split.text("id").ok_or(RulesError::Missing("id"))?)?;
        let title = split
            .text("title")
            .ok_or(RulesError::Missing("title"))?
            .trim()
            .to_owned();
        let level = Level::parse(split.text("level").ok_or(RulesError::Missing("level"))?)?;
        let flag = |key: &'static str| match split.get(key) {
            None => Ok(false),
            Some(Value::Text(text)) if text == "true" => Ok(true),
            Some(Value::Text(text)) if text == "false" || text.is_empty() => Ok(false),
            Some(_) => Err(RulesError::NotBoolean(key)),
        };
        let paths = match split.get("paths") {
            None => Vec::new(),
            Some(Value::List(items)) => items.clone(),
            Some(Value::Text(text)) if text.is_empty() => Vec::new(),
            Some(Value::Text(text)) => vec![text.clone()],
        };
        let rule = Self {
            id,
            title,
            level,
            absolute: flag("absolute")?,
            enforced: flag("enforced")?,
            paths,
            body: normalize_body(&split.body),
            extra: split
                .fields
                .into_iter()
                .filter(|(key, _)| !KNOWN.contains(&key.as_str()))
                .collect(),
        };
        rule.validate()?;
        Ok(rule)
    }

    /// The rule as its file.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::from(frontmatter::FENCE);
        text.push('\n');
        let _ = writeln!(text, "id: {}", self.id);
        let _ = writeln!(text, "title: {}", frontmatter::scalar(&self.title));
        let _ = writeln!(text, "level: {}", self.level.as_str());
        if self.absolute {
            text.push_str("absolute: true\n");
        }
        if self.enforced {
            text.push_str("enforced: true\n");
        }
        if !self.paths.is_empty() {
            text.push_str("paths:\n");
            for path in &self.paths {
                let _ = writeln!(text, "  - {}", frontmatter::scalar(path));
            }
        }
        for (key, value) in &self.extra {
            match value {
                Value::Text(scalar) => {
                    let _ = writeln!(text, "{key}: {}", frontmatter::scalar(scalar));
                }
                Value::List(items) => {
                    let _ = writeln!(text, "{key}:");
                    for item in items {
                        let _ = writeln!(text, "  - {}", frontmatter::scalar(item));
                    }
                }
            }
        }
        text.push_str(frontmatter::FENCE);
        text.push('\n');
        text.push_str(&self.body);
        text
    }

    /// The rule's rules: a title and a text, a flag only at the level it belongs to, and no token in it.
    ///
    /// # Errors
    ///
    /// [`RulesError::Invalid`] naming what is wrong, [`RulesError::HoldsToken`] when the text carries an issued
    /// token — a rule travels to every machine and every member.
    pub fn validate(&self) -> Result<(), RulesError> {
        if self.title.is_empty() || self.title.contains('\n') {
            return Err(RulesError::Invalid(
                "the title is one non-empty line".to_owned(),
            ));
        }
        if self.body.trim().is_empty() {
            return Err(RulesError::Invalid("the rule has no text".to_owned()));
        }
        if self.absolute && self.level != Level::Personal {
            return Err(RulesError::Invalid(
                "absolute is a personal rule's flag: a team marks its own rules enforced"
                    .to_owned(),
            ));
        }
        if self.enforced && self.level != Level::Team {
            return Err(RulesError::Invalid(
                "enforced is a team rule's flag".to_owned(),
            ));
        }
        if crate::token::holds_token(self.title.as_bytes())
            || crate::token::holds_token(self.body.as_bytes())
        {
            return Err(RulesError::HoldsToken);
        }
        Ok(())
    }

    /// The version this rule's content is: what a marker in an assembled file names. Changes with the title or the
    /// text; the flags and paths are the frontmatter's business and are compared as they are.
    #[must_use]
    pub fn version(&self) -> String {
        version_of(&self.title, &self.body)
    }
}

/// The version of a title and a text: 16 hex digits of FNV-1a over both, normalized as [`normalize_body`] does.
///
/// An identity, not a seal: it tells two versions of one rule apart and finds a text among a rule's history, and it
/// is computed the same on every machine.
#[must_use]
pub fn version_of(title: &str, body: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let text = format!("{}\n{}", title.trim(), normalize_body(body));
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// A body as it is kept: line ends as `\n`, trailing spaces gone, no blank lines at either end, one newline at the
/// end. Two texts that differ only in this are the same rule.
#[must_use]
pub fn normalize_body(body: &str) -> String {
    let lines: Vec<&str> = body.lines().map(str::trim_end).collect();
    let first = lines.iter().position(|line| !line.is_empty());
    let last = lines.iter().rposition(|line| !line.is_empty());
    match (first, last) {
        (Some(first), Some(last)) => {
            let mut text = lines.get(first..=last).unwrap_or_default().join("\n");
            text.push('\n');
            text
        }
        _ => String::new(),
    }
}
