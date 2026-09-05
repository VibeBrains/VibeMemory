//! The hooks Claude Code calls, and the shape of what it hands them.
//!
//! A hook runs inside somebody's session: it may not block, may not touch the network, and may
//! not fail loudly. Whatever goes wrong, it exits 0 and says what happened through
//! `additionalContext`, because a hook that returns an error stops the session it was supposed
//! to serve.

pub mod prompt_gate;
pub mod session_start;
pub mod stop;

use serde::Deserialize;

/// What the CLI writes to a hook's stdin. Unknown fields are expected: the CLI adds them between
/// versions, and a hook that refused an unfamiliar field would stop working on the next release.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct HookInput {
    /// The session's id.
    pub session_id: String,
    /// Absolute path of the transcript. It does not exist yet when `SessionStart` runs — measured
    /// on `-p`, stream-json and an interactive TTY alike — and neither does its directory.
    pub transcript_path: String,
    /// The working directory, as the CLI reports it.
    pub cwd: String,
    /// `startup`, `resume`, `clear`, `compact`.
    #[serde(default)]
    pub source: Option<String>,
}

/// Why a hook could not act on what it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookError {
    /// stdin was not the JSON this version understands.
    InputInvalid {
        /// What the parser said.
        detail: String,
    },
}

impl HookError {
    /// Stable code for the log and for `doctor`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InputInvalid { .. } => "hookInputInvalid",
        }
    }

    /// A sentence for the session to read.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::InputInvalid { detail } => {
                format!("VibeMemory could not read what the hook was given: {detail}")
            }
        }
    }
}

/// Reads a hook's stdin.
///
/// # Errors
///
/// [`HookError::InputInvalid`] when the text is not the expected JSON.
pub fn parse_input(text: &str) -> Result<HookInput, HookError> {
    serde_json::from_str(text).map_err(|error| HookError::InputInvalid {
        detail: error.to_string(),
    })
}
