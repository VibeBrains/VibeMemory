//! The YAML subset rule and skill files use in their frontmatter: what `SKILL.md` files in the wild actually write.
//!
//! Scalars, quoted or not, a trailing `# comment`, flow lists `[a, b]`, block lists of `- item` lines, and the folded
//! (`>`) and literal (`|`) block scalars descriptions are written with. Nothing else: a file that needs more is not
//! one an agent reads either, and a full YAML parser would be one more thing to trust with text from other machines.

use super::RulesError;

/// The fence around a frontmatter block.
pub const FENCE: &str = "---";

/// One value of a frontmatter key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A scalar, unquoted and with block scalars joined.
    Text(String),
    /// A list of scalars.
    List(Vec<String>),
}

/// A file split into its frontmatter, in the order the keys were written, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// Key and value, in file order.
    pub fields: Vec<(String, Value)>,
    /// Everything after the closing fence, with the one newline after it dropped.
    pub body: String,
}

impl Split {
    /// The value of a key, if written.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// A key's value as text; a list is not text.
    #[must_use]
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Text(text) => Some(text.as_str()),
            Value::List(_) => None,
        }
    }
}

/// Splits a file into frontmatter and body.
///
/// # Errors
///
/// [`RulesError::NoFrontmatter`] when the file does not start with a fence or never closes it, and
/// [`RulesError::Frontmatter`] for a line this subset does not read.
pub fn split(text: &str) -> Result<Split, RulesError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first.trim_end() != FENCE {
        return Err(RulesError::NoFrontmatter);
    }
    let mut header: Vec<&str> = Vec::new();
    let mut closed = false;
    let mut consumed = first.len();
    for line in lines.by_ref() {
        consumed += line.len();
        if line.trim_end() == FENCE {
            closed = true;
            break;
        }
        header.push(line.trim_end_matches(['\n', '\r']));
    }
    if !closed {
        return Err(RulesError::NoFrontmatter);
    }
    let body = text.get(consumed..).unwrap_or_default();
    Ok(Split {
        fields: fields(&header)?,
        body: body.to_owned(),
    })
}

fn fields(lines: &[&str]) -> Result<Vec<(String, Value)>, RulesError> {
    let mut fields = Vec::new();
    let mut index = 0;
    while let Some(line) = lines.get(index) {
        index += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(RulesError::Frontmatter(format!(
                "an indented line outside a list or a block: {line}"
            )));
        }
        let Some((key, rest)) = line.split_once(':') else {
            return Err(RulesError::Frontmatter(format!(
                "a line that is not `key: value`: {line}"
            )));
        };
        let key = key.trim().to_owned();
        let rest = strip_comment(rest.trim());
        let value = if rest.is_empty() {
            // a block list follows, or nothing at all
            let mut items = Vec::new();
            while let Some(next) = lines.get(index) {
                let trimmed = next.trim_start();
                if let Some(item) = trimmed
                    .strip_prefix("- ")
                    .or_else(|| (trimmed == "-").then_some(""))
                {
                    items.push(unquote(strip_comment(item.trim())));
                    index += 1;
                } else if next.trim().is_empty() {
                    index += 1;
                } else {
                    break;
                }
            }
            if items.is_empty() {
                Value::Text(String::new())
            } else {
                Value::List(items)
            }
        } else if let Some(style) = block_style(rest) {
            let mut block = Vec::new();
            while let Some(next) = lines.get(index) {
                if next.starts_with(' ') || next.starts_with('\t') || next.trim().is_empty() {
                    block.push(next.trim());
                    index += 1;
                } else {
                    break;
                }
            }
            Value::Text(join_block(&block, style))
        } else if let Some(inner) = rest
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
        {
            Value::List(
                inner
                    .split(',')
                    .map(|item| unquote(item.trim()))
                    .filter(|item| !item.is_empty())
                    .collect(),
            )
        } else {
            Value::Text(unquote(rest))
        };
        fields.push((key, value));
    }
    Ok(fields)
}

/// `>` folds lines into one, `|` keeps them; a chomping indicator after it changes nothing here.
#[derive(Debug, Clone, Copy)]
enum Block {
    Folded,
    Literal,
}

fn block_style(value: &str) -> Option<Block> {
    match value.trim_end_matches(['-', '+']) {
        ">" => Some(Block::Folded),
        "|" => Some(Block::Literal),
        _ => None,
    }
}

fn join_block(lines: &[&str], style: Block) -> String {
    let lines: Vec<&str> = {
        let mut kept = lines.to_vec();
        while kept.last().is_some_and(|line| line.is_empty()) {
            kept.pop();
        }
        kept
    };
    match style {
        Block::Literal => lines.join("\n"),
        Block::Folded => {
            let mut text = String::new();
            for line in lines {
                if line.is_empty() {
                    text.push('\n');
                } else {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push(' ');
                    }
                    text.push_str(line);
                }
            }
            text
        }
    }
}

/// A `# comment` after a value, outside quotes.
fn strip_comment(value: &str) -> &str {
    let mut quote = None;
    let mut previous = ' ';
    for (at, c) in value.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '#') if previous.is_whitespace() => {
                return value.get(..at).unwrap_or(value).trim_end();
            }
            _ => {}
        }
        previous = c;
    }
    value
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        // the two escapes `scalar` writes
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.replace("''", "'");
    }
    value.to_owned()
}

/// A scalar as it may be written back: quoted when a reader would otherwise take it for something else.
#[must_use]
pub fn scalar(value: &str) -> String {
    let plain = !value.is_empty()
        && !value.starts_with([
            ' ', '-', '[', '{', '>', '|', '"', '\'', '#', '&', '*', '!', '%', '@', '`',
        ])
        && !value.ends_with(' ')
        && !value.contains(": ")
        && !value.contains(" #")
        && !value.contains('\n');
    if plain {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
