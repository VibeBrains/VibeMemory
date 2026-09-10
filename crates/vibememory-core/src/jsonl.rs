//! What counts as one line of the JSONL files this engine owns.
//!
//! Claude Code writes every record as `JSON.stringify(entry) + "\n"`, on every platform: the
//! terminator is one byte and a carriage return before it is never something the app wrote. It
//! appears when git converts line endings on a Windows checkout — the store sets `* -text` to
//! forbid exactly that, but a guard in one config line is not a reason for the format reader to
//! break when it is missing.
//!
//! Breaking is what it did. A line's identity is its `uuid` when it has one and **its bytes**
//! otherwise, so one stray `\r` makes a state line a different line: merging an LF side with a
//! CRLF side kept both copies of every `bridge-session`, `queue-operation` and `last-prompt`,
//! silently and without a conflict. Memory journals are worse — measured on
//! `fixtures/merge/cli255Journal.jsonl`, not one of its 16 lines carries a `uuid`, so all of them
//! are byte-identified and the whole journal would double.
//!
//! Stripping it can never eat content: `serde_json` escapes a carriage return inside a string as
//! the two characters `\r`, so a raw `0x0D` byte cannot occur inside a record it wrote.

/// Terminator of a JSONL record.
pub const LINE_END: u8 = b'\n';

/// One line without the carriage return git may have put in front of its terminator.
///
/// Applied on the way in, so identity, comparison and output all speak the format the app
/// writes; nothing downstream needs to know that CRLF exists.
#[must_use]
pub fn line_body(raw: &[u8]) -> &[u8] {
    match raw.strip_suffix(b"\r") {
        Some(body) => body,
        None => raw,
    }
}
