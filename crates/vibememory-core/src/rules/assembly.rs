//! Rules written out for an agent, and an agent's file read back into rules.
//!
//! An agent that reads one file of instructions gets them assembled: the owner's base file first, then each rule
//! between markers that name it and the version it was written from. An agent that reads a directory of rule files
//! gets one file a rule, with the same markers. The markers are what makes an edit in the agent's file an edit of one
//! rule: its version says what the edit started from, so a change on either side is told from a change on both.

use std::fmt::Write as _;

use super::RulesError;
use super::rule::{Level, Rule, normalize_body, version_of};

const BASE_OPEN: &str = "<!-- vibememory:base@";
const BASE_CLOSE: &str = "<!-- /vibememory:base -->";
const RULE_OPEN: &str = "<!-- vibememory:rule ";
const RULE_CLOSE: &str = "<!-- /vibememory:rule -->";
const MARKER_END: &str = " -->";

/// The first line of every file the engine writes: what the file is and where an edit goes.
pub const HEADER: &str = "<!-- Written by VibeMemory from your rules. Edit a rule between its markers or with \
                          `vibememory rule edit`: the change reaches every agent. Text outside the markers stays \
                          here only. -->";

/// The line a rule carries when it replaces one of the same id below: the agent may hold both.
#[must_use]
pub fn replaces_note(levels: &[Level]) -> String {
    let names: Vec<&str> = levels.iter().map(|level| level.as_str()).collect();
    format!(
        "_This rule replaces the {} rule with the same id: where they differ, follow this one._",
        names.join(" and ")
    )
}

/// One rule for an agent's file: its frontmatter `paths` when it has some, the markers, the title and the text.
#[must_use]
pub fn rule_file(rule: &Rule, replaces: &[Level]) -> String {
    let mut text = String::new();
    if !rule.paths.is_empty() {
        text.push_str("---\npaths:\n");
        for path in &rule.paths {
            let _ = writeln!(text, "  - {}", super::frontmatter::scalar(path));
        }
        text.push_str("---\n");
    }
    text.push_str(HEADER);
    text.push('\n');
    push_rule(&mut text, rule, replaces);
    text
}

/// Everything for an agent that reads one file: the base text, then the rules in the order given.
#[must_use]
pub fn assemble(base: Option<&str>, rules: &[(&Rule, &[Level])]) -> String {
    let mut text = String::from(HEADER);
    text.push('\n');
    if let Some(base) = base {
        let base = normalize_body(base);
        let _ = writeln!(text, "{BASE_OPEN}{}{MARKER_END}", version_of("", &base));
        text.push_str(&base);
        text.push_str(BASE_CLOSE);
        text.push('\n');
    }
    for (rule, replaces) in rules {
        text.push('\n');
        push_rule(&mut text, rule, replaces);
    }
    text
}

fn push_rule(text: &mut String, rule: &Rule, replaces: &[Level]) {
    let _ = writeln!(
        text,
        "{RULE_OPEN}{}@{}{MARKER_END}",
        rule.id,
        rule.version()
    );
    let _ = writeln!(text, "## {}", rule.title);
    text.push('\n');
    if !replaces.is_empty() {
        text.push_str(&replaces_note(replaces));
        text.push_str("\n\n");
    }
    text.push_str(&rule.body);
    text.push_str(RULE_CLOSE);
    text.push('\n');
}

/// A piece of an agent's file read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// The base text, and the version it was written from.
    Base {
        /// Version in the marker.
        from: String,
        /// The text now.
        text: String,
    },
    /// A rule, and the version it was written from.
    Rule {
        /// Its id, as the marker names it.
        id: String,
        /// Version in the marker.
        from: String,
        /// The title now.
        title: String,
        /// The text now.
        body: String,
    },
    /// Text outside every marker: a person's own, kept in that file only.
    Outside(String),
}

impl Piece {
    /// The version this piece's content is now: equal to `from` when nobody edited it.
    #[must_use]
    pub fn version_now(&self) -> Option<String> {
        match self {
            Self::Base { text, .. } => Some(version_of("", text)),
            Self::Rule { title, body, .. } => Some(version_of(title, body)),
            Self::Outside(_) => None,
        }
    }

    /// The version the piece was written from.
    #[must_use]
    pub fn from(&self) -> Option<&str> {
        match self {
            Self::Base { from, .. } | Self::Rule { from, .. } => Some(from),
            Self::Outside(_) => None,
        }
    }
}

/// Reads an agent's file back into its pieces.
///
/// # Errors
///
/// [`RulesError::Markers`] for a marker opened and not closed, or closed without being opened: such a file was cut
/// by hand, and guessing where a rule ends would write half of one rule into another.
pub fn disassemble(text: &str) -> Result<Vec<Piece>, RulesError> {
    let text = skip_frontmatter(text);
    let mut pieces = Vec::new();
    let mut outside = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == HEADER {
            continue;
        }
        if let Some(from) = marker(line, BASE_OPEN) {
            flush(&mut pieces, &mut outside);
            let inner = until(&mut lines, BASE_CLOSE)?;
            pieces.push(Piece::Base {
                from: from.to_owned(),
                text: normalize_body(&inner),
            });
        } else if let Some(named) = marker(line, RULE_OPEN) {
            flush(&mut pieces, &mut outside);
            let (id, from) = named.split_once('@').ok_or_else(|| {
                RulesError::Markers(format!("a rule marker without a version: {line}"))
            })?;
            let inner = until(&mut lines, RULE_CLOSE)?;
            let mut rest = inner.lines();
            let title = rest
                .next()
                .and_then(|first| first.strip_prefix("## "))
                .ok_or_else(|| RulesError::Markers(format!("rule {id} lost its `## title` line")))?
                .trim()
                .to_owned();
            let body: Vec<&str> = rest.collect();
            let mut body = body.join("\n");
            // the note the engine adds is not the rule's text
            if let Some(stripped) = strip_note(&body) {
                body = stripped;
            }
            pieces.push(Piece::Rule {
                id: id.to_owned(),
                from: from.to_owned(),
                title,
                body: normalize_body(&body),
            });
        } else if line == BASE_CLOSE || line == RULE_CLOSE {
            return Err(RulesError::Markers(format!(
                "a closing marker without its opening one: {line}"
            )));
        } else {
            outside.push_str(line);
            outside.push('\n');
        }
    }
    flush(&mut pieces, &mut outside);
    Ok(pieces)
}

fn skip_frontmatter(text: &str) -> &str {
    if let Ok(split) = super::frontmatter::split(text)
        && split.get("paths").is_some()
        && split.fields.len() == 1
    {
        let at = text.len() - split.body.len();
        return text.get(at..).unwrap_or(text);
    }
    text
}

fn marker<'a>(line: &'a str, open: &str) -> Option<&'a str> {
    line.strip_prefix(open)?.strip_suffix(MARKER_END)
}

fn until<'a>(lines: &mut impl Iterator<Item = &'a str>, close: &str) -> Result<String, RulesError> {
    let mut inner = String::new();
    for line in lines.by_ref() {
        if line == close {
            return Ok(inner);
        }
        if line.starts_with(RULE_OPEN) || line.starts_with(BASE_OPEN) {
            return Err(RulesError::Markers(format!(
                "a marker opened inside another: {line}"
            )));
        }
        inner.push_str(line);
        inner.push('\n');
    }
    Err(RulesError::Markers(format!(
        "a block not closed with {close}"
    )))
}

fn strip_note(body: &str) -> Option<String> {
    let trimmed = body.trim_start_matches('\n');
    let first = trimmed.lines().next()?;
    if first.starts_with("_This rule replaces the ") && first.ends_with("follow this one._") {
        Some(trimmed.get(first.len()..).unwrap_or_default().to_owned())
    } else {
        None
    }
}

fn flush(pieces: &mut Vec<Piece>, outside: &mut String) {
    if !outside.trim().is_empty() {
        pieces.push(Piece::Outside(normalize_body(outside)));
    }
    outside.clear();
}
