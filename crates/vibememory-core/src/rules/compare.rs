//! A project's own rule files compared with the rules in force: what is a duplicate, what is an old version, what was
//! changed by hand, and what exists only in the project.
//!
//! "Old" is decided by history, not by dates: a rule's every version is in the store's git, and a project's text
//! either is one of them or is not. The clocks of two machines never meet here.

use super::assembly::{Piece, disassemble};
use super::rule::{normalize_body, version_of};

/// The share of shingles two texts must have in common to be taken for one rule when nothing else says so.
pub const SAME_RULE: f64 = 0.5;

/// A block of a project's rule file: a section under a heading, or a whole file of one rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Its heading without the hashes, or the file's name for a file without one.
    pub heading: String,
    /// Its text, normalized.
    pub text: String,
    /// The rule a marker names, when the block came from a file the engine wrote.
    pub marked: Option<String>,
}

/// Cuts a project's rule file into blocks.
///
/// A file the engine wrote is cut by its markers. Any other is cut at its `## ` headings, the way `CLAUDE.md` and
/// `AGENTS.md` are written; text before the first of them is a block of its own named after the file. `whole` keeps
/// the file as one block: a file of a rules directory (`.claude/rules/`, `.vibe/rules/`) is one rule.
#[must_use]
pub fn blocks(file_name: &str, text: &str, whole: bool) -> Vec<Block> {
    if let Ok(pieces) = disassemble(text)
        && pieces
            .iter()
            .any(|piece| !matches!(piece, Piece::Outside(_)))
    {
        return pieces
            .into_iter()
            .filter_map(|piece| match piece {
                Piece::Rule {
                    id, title, body, ..
                } => Some(Block {
                    heading: title,
                    text: body,
                    marked: Some(id),
                }),
                Piece::Base { .. } => None,
                Piece::Outside(text) => Some(Block {
                    heading: file_name.to_owned(),
                    text,
                    marked: None,
                }),
            })
            .collect();
    }
    let body = super::frontmatter::split(text).map_or_else(|_| text.to_owned(), |split| split.body);
    if whole {
        // a heading on the first line names the rule and is not its text
        let first = body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default();
        let (heading, text) = match first.strip_prefix('#') {
            Some(rest) => (
                rest.trim_start_matches('#').trim().to_owned(),
                body.split_once(first)
                    .map_or("", |(_, after)| after)
                    .to_owned(),
            ),
            None => (file_name.to_owned(), body.clone()),
        };
        return vec![Block {
            heading,
            text: normalize_body(&text),
            marked: None,
        }];
    }
    let mut found = Vec::new();
    let mut heading = file_name.to_owned();
    let mut current = String::new();
    for line in body.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            push_block(&mut found, &heading, &current);
            title.trim().clone_into(&mut heading);
            current.clear();
        } else if line.starts_with("# ") && found.is_empty() && current.trim().is_empty() {
            // the document's own title, not a rule
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    push_block(&mut found, &heading, &current);
    found
}

fn push_block(found: &mut Vec<Block>, heading: &str, text: &str) {
    let text = normalize_body(text);
    if !text.is_empty() {
        found.push(Block {
            heading: heading.to_owned(),
            text,
            marked: None,
        });
    }
}

/// A rule's history as the store keeps it: every version's title and text, oldest first, the last in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// The rule's id.
    pub id: String,
    /// `(title, text)` of each version, oldest first.
    pub versions: Vec<(String, String)>,
}

impl History {
    fn current(&self) -> Option<&(String, String)> {
        self.versions.last()
    }
}

/// What a block of a project is, measured against the rules in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The same text as the rule in force: the project may drop it, the agent gets the rule anyway.
    Duplicate,
    /// The same text as an earlier version: the rule in force replaces it.
    Stale {
        /// How many versions old it is.
        behind: usize,
    },
    /// Changed by hand from an earlier version: the rule's later changes merged into it.
    Custom {
        /// The project's text with the rule's changes since the version it grew from.
        merged: Merge,
    },
    /// No rule matches: kept here, or raised into the rules.
    ProjectOnly,
}

/// One block judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The block.
    pub block: Block,
    /// The rule it was matched with.
    pub rule: Option<String>,
    /// Whether the match is a guess by heading or likeness, for a person to confirm once, rather than a marker's.
    pub guessed: bool,
    /// What it is.
    pub state: State,
}

/// Judges every block of a project against the histories of the rules in force.
#[must_use]
pub fn judge(blocks: &[Block], histories: &[History]) -> Vec<Finding> {
    blocks
        .iter()
        .map(|block| {
            let (history, guessed) = match_rule(block, histories);
            let state = history.map_or(State::ProjectOnly, |history| state_of(block, history));
            Finding {
                block: block.clone(),
                rule: history.map(|history| history.id.clone()),
                guessed,
                state,
            }
        })
        .collect()
}

fn match_rule<'a>(block: &Block, histories: &'a [History]) -> (Option<&'a History>, bool) {
    if let Some(id) = &block.marked {
        return (histories.iter().find(|history| &history.id == id), false);
    }
    let by_title = histories.iter().find(|history| {
        history
            .versions
            .iter()
            .any(|(title, _)| same_words(title, &block.heading))
    });
    if by_title.is_some() {
        return (by_title, true);
    }
    let best = histories
        .iter()
        .filter_map(|history| {
            let likeness = history
                .versions
                .iter()
                .map(|(_, text)| similarity(text, &block.text))
                .fold(0.0_f64, f64::max);
            (likeness >= SAME_RULE).then_some((history, likeness))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    (best.map(|(history, _)| history), best.is_some())
}

fn state_of(block: &Block, history: &History) -> State {
    let Some((_, now)) = history.current() else {
        return State::ProjectOnly;
    };
    if normalize_body(now) == block.text {
        return State::Duplicate;
    }
    let count = history.versions.len();
    if let Some(at) = history
        .versions
        .iter()
        .rposition(|(_, text)| normalize_body(text) == block.text)
    {
        return State::Stale {
            behind: count - 1 - at,
        };
    }
    // grown from the version it shares the most lines with; of equals the oldest, so that every later change of the
    // rule is either merged in or named as a conflict, never taken for the project's own
    let ours: Vec<&str> = block.text.lines().collect();
    let mut base = String::new();
    let mut best = None;
    for (_, text) in &history.versions {
        let text = normalize_body(text);
        let lines: Vec<&str> = text.lines().collect();
        let shared = matching(&lines, &ours).iter().flatten().count();
        if best.is_none_or(|most| shared > most) {
            best = Some(shared);
            base.clone_from(&text);
        }
    }
    State::Custom {
        merged: merge3(&base, &block.text, &normalize_body(now)),
    }
}

/// Whether two headings say the same, ignoring case, punctuation and spacing.
#[must_use]
pub fn same_words(a: &str, b: &str) -> bool {
    let words = |text: &str| -> Vec<String> { words(text) };
    let (a, b) = (words(a), words(b));
    !a.is_empty() && a == b
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// How alike two texts are, from 0 to 1: the share of word triples they have in common (pairs or single words for a
/// text too short for triples).
#[must_use]
pub fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (words(a), words(b));
    let size = 3.min(a.len()).min(b.len()).max(1);
    let shingles = |words: &[String]| -> std::collections::BTreeSet<String> {
        words.windows(size).map(|window| window.join(" ")).collect()
    };
    let (a, b) = (shingles(&a), shingles(&b));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let common = a.intersection(&b).count();
    let all = a.union(&b).count();
    if all == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let share = common as f64 / all as f64;
    share
}

/// A three-way merge of lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    /// Both sides' changes fit together.
    Clean(String),
    /// Some lines were changed on both sides: the text carries both, between conflict markers, for a person.
    Conflicted(String),
}

/// Merges `ours` (the project's text) and `theirs` (the rule in force), both grown from `base`, by lines.
///
/// Where only one side changed a stretch of lines, its change is taken; where both made the same change, once;
/// where they made different ones, both are written between `<<<<<<< project` and `>>>>>>> rules` markers.
#[must_use]
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Merge {
    let base: Vec<&str> = base.lines().collect();
    let ours: Vec<&str> = ours.lines().collect();
    let theirs: Vec<&str> = theirs.lines().collect();
    let to_ours = matching(&base, &ours);
    let to_theirs = matching(&base, &theirs);
    let mut out: Vec<String> = Vec::new();
    let mut conflicted = false;
    let (mut o, mut a, mut b) = (0usize, 0usize, 0usize);
    loop {
        // the next base line both sides kept, at or after o
        let anchor = (o..base.len()).find_map(|at| {
            let in_ours = to_ours.get(at).copied().flatten()?;
            let in_theirs = to_theirs.get(at).copied().flatten()?;
            (in_ours >= a && in_theirs >= b).then_some((at, in_ours, in_theirs))
        });
        let (next_o, next_a, next_b) = anchor.unwrap_or((base.len(), ours.len(), theirs.len()));
        let base_part = base.get(o..next_o).unwrap_or_default();
        let ours_part = ours.get(a..next_a).unwrap_or_default();
        let theirs_part = theirs.get(b..next_b).unwrap_or_default();
        if ours_part == base_part {
            out.extend(theirs_part.iter().map(|line| (*line).to_owned()));
        } else if theirs_part == base_part || ours_part == theirs_part {
            out.extend(ours_part.iter().map(|line| (*line).to_owned()));
        } else {
            conflicted = true;
            out.push("<<<<<<< project".to_owned());
            out.extend(ours_part.iter().map(|line| (*line).to_owned()));
            out.push("=======".to_owned());
            out.extend(theirs_part.iter().map(|line| (*line).to_owned()));
            out.push(">>>>>>> rules".to_owned());
        }
        match anchor {
            Some((at, in_ours, in_theirs)) => {
                if let Some(line) = base.get(at) {
                    out.push((*line).to_owned());
                }
                o = at + 1;
                a = in_ours + 1;
                b = in_theirs + 1;
            }
            None => break,
        }
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    if conflicted {
        Merge::Conflicted(text)
    } else {
        Merge::Clean(text)
    }
}

/// For each line of `base`, the line of `other` it is matched with by a longest common subsequence.
fn matching(base: &[&str], other: &[&str]) -> Vec<Option<usize>> {
    let (n, m) = (base.len(), other.len());
    let width = m + 1;
    // table[i][j]: the longest common subsequence of base[i..] and other[j..]
    let mut table = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let value = if base.get(i) == other.get(j) {
                table.get((i + 1) * width + j + 1).copied().unwrap_or(0) + 1
            } else {
                let down = table.get((i + 1) * width + j).copied().unwrap_or(0);
                let right = table.get(i * width + j + 1).copied().unwrap_or(0);
                down.max(right)
            };
            if let Some(cell) = table.get_mut(i * width + j) {
                *cell = value;
            }
        }
    }
    let mut matched = vec![None; n];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if base.get(i) == other.get(j) {
            if let Some(slot) = matched.get_mut(i) {
                *slot = Some(j);
            }
            i += 1;
            j += 1;
        } else if table.get((i + 1) * width + j) >= table.get(i * width + j + 1) {
            i += 1;
        } else {
            j += 1;
        }
    }
    matched
}

/// The rules most like a text, best first, for a rule about to be written: one about the same thing is better
/// extended than written twice.
#[must_use]
pub fn similar<'a>(
    title: &str,
    text: &str,
    rules: &'a [(String, String, String)],
) -> Vec<(&'a str, f64)> {
    let mut found: Vec<(&str, f64)> = rules
        .iter()
        .filter_map(|(id, rule_title, rule_text)| {
            let likeness = if same_words(title, rule_title) {
                1.0
            } else {
                similarity(text, rule_text).max(similarity(title, rule_title) * 0.5)
            };
            (likeness >= SAME_RULE * 0.6).then_some((id.as_str(), likeness))
        })
        .collect();
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found
}

/// The version a project's block is, as a marker would name it.
#[must_use]
pub fn block_version(block: &Block) -> String {
    version_of(&block.heading, &block.text)
}
