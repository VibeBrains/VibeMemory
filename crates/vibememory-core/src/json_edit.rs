//! Replacing one top-level value of a JSON object in its text, leaving every other byte as it was.
//!
//! `config.json` is the owner's file: the order of its keys, its spacing and whatever he lined up by
//! hand are his. A command that changes one section parses nothing else and rewrites nothing else.

/// Why the text could not be edited.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonEditError {
    /// The text is not a JSON object.
    #[error("not a JSON object: {0}")]
    NotAnObject(String),
}

/// The text with the value of top-level `key` replaced by `value`, or `key` added as the last
/// member when it is not there. `value` is JSON text; `indent` is what each line of it after the
/// first is prefixed with, so a multi-line value lines up with the member it belongs to.
///
/// # Errors
///
/// [`JsonEditError::NotAnObject`] when the text is not a single JSON object.
pub fn set_top_level(text: &str, key: &str, value: &str) -> Result<String, JsonEditError> {
    let members = scan(text)?;
    if let Some(member) = members.members.iter().find(|member| member.key == key) {
        let indent = line_indent(text, member.key_start);
        let mut out = String::with_capacity(text.len() + value.len());
        out.push_str(text.get(..member.value_start).unwrap_or_default());
        out.push_str(&indented(value, &indent));
        out.push_str(text.get(member.value_end..).unwrap_or_default());
        return Ok(out);
    }
    let indent = members.members.first().map_or_else(
        || "  ".to_owned(),
        |member| line_indent(text, member.key_start),
    );
    let quoted = serde_json::to_string(key).unwrap_or_default();
    let before = text.get(..members.close).unwrap_or_default();
    let after = text.get(members.close..).unwrap_or_default();
    let trimmed = before.trim_end();
    let separator = if members.members.is_empty() { "" } else { "," };
    Ok(format!(
        "{trimmed}{separator}\n{indent}{quoted}: {}\n{after}",
        indented(value, &indent)
    ))
}

/// The text without top-level `key`, or unchanged when it is not there.
///
/// # Errors
///
/// [`JsonEditError::NotAnObject`] when the text is not a single JSON object.
pub fn remove_top_level(text: &str, key: &str) -> Result<String, JsonEditError> {
    let members = scan(text)?;
    let Some(index) = members.members.iter().position(|member| member.key == key) else {
        return Ok(text.to_owned());
    };
    let member = members
        .members
        .get(index)
        .ok_or_else(|| not_object("member"))?;
    // the member goes with the separator before it, or after it when it is the first one
    let (cut_start, cut_end) = match (index, members.members.get(index + 1)) {
        (0, Some(next)) => (member.line_start, next.line_start),
        (0, None) => (member.line_start, trim_back(text, members.close)),
        (_, _) => {
            let previous = members
                .members
                .get(index - 1)
                .ok_or_else(|| not_object("member"))?;
            (previous.value_end, member.value_end)
        }
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(text.get(..cut_start).unwrap_or_default());
    out.push_str(text.get(cut_end..).unwrap_or_default());
    Ok(out)
}

/// One member of the top-level object, as byte offsets into the text.
struct Member {
    key: String,
    /// Where the line holding the key starts.
    line_start: usize,
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

struct Members {
    members: Vec<Member>,
    /// The offset of the closing brace.
    close: usize,
}

fn not_object(what: &str) -> JsonEditError {
    JsonEditError::NotAnObject(what.to_owned())
}

/// Splits the top-level object into members. Values are skipped over, not interpreted: strings with
/// their escapes, and nested brackets by depth.
fn scan(text: &str) -> Result<Members, JsonEditError> {
    // the text must be valid JSON before any offset of it is trusted
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|error| not_object(&error.to_string()))?;
    if !parsed.is_object() {
        return Err(not_object("the top level is not an object"));
    }
    let bytes = text.as_bytes();
    let mut at = skip_space(bytes, 0);
    if bytes.get(at) != Some(&b'{') {
        return Err(not_object("no opening brace"));
    }
    at += 1;
    let mut members = Vec::new();
    loop {
        at = skip_space(bytes, at);
        match bytes.get(at) {
            Some(b'}') => {
                return Ok(Members { members, close: at });
            }
            Some(b',') => {
                at += 1;
                continue;
            }
            Some(b'"') => {}
            _ => return Err(not_object("a member does not start with a key")),
        }
        let key_start = at;
        let key_end = skip_string(bytes, at).ok_or_else(|| not_object("unterminated key"))?;
        let key: String = serde_json::from_str(text.get(key_start..key_end).unwrap_or_default())
            .map_err(|error| not_object(&error.to_string()))?;
        at = skip_space(bytes, key_end);
        if bytes.get(at) != Some(&b':') {
            return Err(not_object("a key without a colon"));
        }
        let value_start = skip_space(bytes, at + 1);
        let value_end = skip_value(bytes, value_start).ok_or_else(|| not_object("a value"))?;
        members.push(Member {
            key,
            line_start: line_start(text, key_start),
            key_start,
            value_start,
            value_end,
        });
        at = value_end;
    }
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The offset just past the string that starts at `at`.
fn skip_string(bytes: &[u8], at: usize) -> Option<usize> {
    let mut at = at + 1;
    loop {
        match bytes.get(at)? {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
}

/// The offset just past the value that starts at `at`.
fn skip_value(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at)? {
        b'"' => skip_string(bytes, at),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut at = at;
            loop {
                match bytes.get(at)? {
                    b'"' => {
                        at = skip_string(bytes, at)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at + 1);
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
        }
        _ => {
            let mut at = at;
            while bytes.get(at).is_some_and(|byte| {
                !matches!(byte, b',' | b'}' | b']') && !byte.is_ascii_whitespace()
            }) {
                at += 1;
            }
            Some(at)
        }
    }
}

fn line_start(text: &str, at: usize) -> usize {
    text.get(..at)
        .and_then(|before| before.rfind('\n'))
        .map_or(0, |newline| newline + 1)
}

fn line_indent(text: &str, at: usize) -> String {
    let start = line_start(text, at);
    text.get(start..at)
        .unwrap_or_default()
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Back from `at` over whitespace, to just past the last non-space byte.
fn trim_back(text: &str, at: usize) -> usize {
    text.get(..at).map_or(at, |before| before.trim_end().len())
}

fn indented(value: &str, indent: &str) -> String {
    value.replace('\n', &format!("\n{indent}"))
}
