//! `UserPromptSubmit`: the only hook that can stop a person from typing.
//!
//! It exists for one situation: the same session was continued on another machine, this machine
//! has not caught up yet, and a prompt sent now would fork the conversation into a branch the
//! reader will later have to choose between. Blocking it for a few seconds is cheaper than
//! merging two versions of the same thought.
//!
//! Everything else about this hook is about *not* blocking. A gate that stops the user because it
//! could not read a file, or because two machines disagree by a line that is already ours, is a
//! gate that gets switched off — and then it protects nothing. So the decision is made from one
//! comparison, on data this machine already has, and every doubt resolves to "let it through".

use crate::hook::stop::Tail;

/// What the gate decided about one prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Let the prompt through. The normal answer, and the answer to every uncertainty.
    Allow,
    /// Hold the prompt back, with a sentence the person will read.
    Block {
        /// Which machine is ahead.
        machine: String,
        /// How many lines it has that this machine does not.
        ahead_by: usize,
    },
}

impl Gate {
    /// The sentence shown when the prompt is held back.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Allow => None,
            Self::Block { machine, ahead_by } => Some(format!(
                "VibeMemory: this session was continued on {machine}, which is {ahead_by} \
                 record(s) ahead of this machine. Sending now would split the conversation into \
                 two branches. The next tick brings those records here — usually within two \
                 minutes — and then the prompt goes through."
            )),
        }
    }
}

/// Decides whether a prompt may go through.
///
/// `local_lines` is what this machine's copy of the transcript holds; `others` are the tails
/// reported by the **other** machines, already fetched into the working tree. The caller filters
/// its own machine out — from here the two are indistinguishable, and comparing a machine with
/// itself would block every prompt of a busy session.
///
/// Only one thing blocks: another machine says it holds more of *this* session than we do. Fewer
/// lines mean it is behind, which is its problem and not the typist's; the same count means there
/// is nothing to wait for, even when the records differ — that is a fork the merge already knows
/// how to keep, and stopping the person would not undo it.
#[must_use]
pub fn decide(local_lines: usize, others: &[(String, Tail)]) -> Gate {
    let furthest = others
        .iter()
        .filter(|(_, tail)| tail.lines > local_lines)
        .max_by_key(|(_, tail)| tail.lines);
    match furthest {
        None => Gate::Allow,
        Some((machine, tail)) => Gate::Block {
            machine: machine.clone(),
            ahead_by: tail.lines.saturating_sub(local_lines),
        },
    }
}
