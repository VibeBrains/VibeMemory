//! A Desktop session card as data.
//!
//! Anthropic documents neither the format nor the fields: the shape below was read off 165 real
//! `local_*.json` files and the app bundle, and the next Desktop release may add a key or drop
//! one. So the descriptor keeps every key it did not recognise and writes it back untouched —
//! the engine is a courier here, not an author. Only the fields the guard reasons about are
//! named, and all of them are optional, because a card written by a newer or older Desktop is
//! still a card that has to reach the other machine.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The fields the guard reads, plus everything else the file held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Descriptor {
    /// The card's own id, `local_<uuid>`; also the file name.
    pub session_id: String,
    /// The transcript this card resumes, `projects/<enc>/<cliSessionId>.jsonl`. Desktop clears it
    /// when a resume misses (`clearStaleResumeHandle`), and nothing in the app ever puts it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_id: Option<String>,
    /// The process working directory Desktop resumes into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The directory the person picked; differs from `cwd` only for isolated worktrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_cwd: Option<String>,
    /// Set by Desktop when it decided the transcript is gone. Machine-local by nature: the file
    /// is missing *here*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_unavailable: Option<bool>,
    /// Everything this build of Desktop wrote that the engine does not interpret.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Descriptor {
    /// Whether Desktop has marked the transcript as gone.
    #[must_use]
    pub fn is_transcript_unavailable(&self) -> bool {
        self.transcript_unavailable == Some(true)
    }

    /// The same card with `cwd` and `originCwd` replaced. Used on both sides of the wire: to the
    /// portable `{ROOT}/rel` on the way out, to this machine's paths on the way in.
    #[must_use]
    pub fn with_cwd(&self, cwd: Option<String>, origin_cwd: Option<String>) -> Self {
        Self {
            cwd,
            origin_cwd,
            ..self.clone()
        }
    }

    /// The same card carrying a transcript again, with the stale mark dropped: the local repair
    /// of a card Desktop broke while another machine still holds the transcript.
    #[must_use]
    pub fn with_transcript(&self, cli_session_id: String) -> Self {
        Self {
            cli_session_id: Some(cli_session_id),
            transcript_unavailable: None,
            ..self.clone()
        }
    }
}
