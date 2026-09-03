//! The guard on Desktop cards: what may be exported, what may be imported, and how a card broken
//! locally is repaired.
//!
//! Desktop degrades a card on its own and without asking. A resume that misses clears
//! `cliSessionId` (`clearStaleResumeHandle`) with no mechanism anywhere in the app to put it
//! back; a preflight that cannot find the transcript sets `transcriptUnavailable`. Both verdicts
//! are about *this* machine — the transcript may be sitting on the other one, or in a store
//! directory the local scan is blind to — and both are exactly what the owner loses forever if
//! the engine exports them: the store would carry the damage to every machine within minutes.
//!
//! So the export gate is a ratchet. A card may gain a transcript, never lose one; the stale mark
//! never travels. And the import gate is the mirror image: a card is accepted only once this
//! machine can actually resume it, because a card accepted too early is a card Desktop will
//! break on the first miss.

use crate::desktop::descriptor::Descriptor;

/// Why a card is not exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithholdReason {
    /// It had a transcript in the exported version and has none now: Desktop cleared the handle
    /// after a resume miss, and the other machine must not inherit the empty card.
    LostTranscript,
    /// Desktop marked the transcript unavailable — a verdict about this machine's disk.
    MarkedUnavailable,
    /// Its `cwd` lies outside every declared root, so there is nothing portable to say about it.
    CwdOutsideRoots,
}

/// Whether a card may be written into the outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportVerdict {
    /// Write this version — `cwd` and `originCwd` already in portable form.
    Export {
        /// The card to write.
        descriptor: Box<Descriptor>,
    },
    /// Keep the local version to itself, with the reason for the log and `doctor`.
    Withhold {
        /// Why.
        reason: WithholdReason,
        /// The rule, as text.
        rule: &'static str,
    },
}

/// Why a card from another machine is not adopted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The card names no transcript, so there is nothing to resume.
    NoTranscript,
    /// The transcript is not in the store yet: the other machine committed the card first.
    TranscriptMissing,
    /// The translated `cwd` does not exist here — Desktop would refuse in `prepareSpawnCwd`, and
    /// the refusal costs the card its `cliSessionId`.
    CwdMissing,
    /// The link the CLI resumes through is not confirmed by this machine's `transcript_path`. A
    /// link the reconciler merely intends to create does not count: Desktop's scans are blind to
    /// symlinks, so an unconfirmed guess ends in a resume miss and a cleared handle.
    LinkUnconfirmed,
    /// The portable `cwd` names a root this machine does not declare.
    RootUnknown,
}

/// Whether a card from another machine may be adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportVerdict {
    /// Write this version into the local Desktop store — paths already local.
    Import {
        /// The card to write.
        descriptor: Box<Descriptor>,
    },
    /// Leave it in the outbox for a later tick, with the reason.
    Skip {
        /// Why.
        reason: SkipReason,
        /// The rule, as text.
        rule: &'static str,
    },
}

/// Facts about this machine, measured by the CLI, that the import gate needs.
///
/// Every field is a measurement, not an intention: the caller has to have looked at the disk.
/// This is the whole point of the type — the gate exists because a predicted link and a real one
/// are indistinguishable in a boolean the caller computed from its own plan.
#[derive(Debug, Clone, Copy)]
pub struct MachineFacts<'a> {
    /// The translated `cwd`, as measured to exist (`stat`), or `None` when it does not.
    pub cwd_exists: Option<&'a str>,
    /// Whether the transcript named by the card is present in the store.
    pub transcript_in_store: bool,
    /// The `transcript_path` the CLI reported for this session on this machine, which is what
    /// proves the link resolves. `None` means nothing has confirmed it.
    pub confirmed_transcript_path: Option<&'a str>,
}

/// Decides whether the local card may be exported.
///
/// `exported` is the version last written to the outbox, `local` the version on disk now, and
/// `portable_cwd` the result of translating `local`'s directories — `None` when they lie outside
/// every root. The first export of a card has no `exported` version and nothing to lose, so only
/// the stale mark and the roots can stop it.
#[must_use]
pub fn export_verdict(
    exported: Option<&Descriptor>,
    local: &Descriptor,
    portable_cwd: Option<(String, Option<String>)>,
) -> ExportVerdict {
    if local.is_transcript_unavailable() {
        return ExportVerdict::Withhold {
            reason: WithholdReason::MarkedUnavailable,
            rule: "Desktop marked the transcript unavailable: a verdict about this machine's disk",
        };
    }
    if local.cli_session_id.is_none()
        && exported.is_some_and(|exported| exported.cli_session_id.is_some())
    {
        return ExportVerdict::Withhold {
            reason: WithholdReason::LostTranscript,
            rule: "the card lost its transcript after a resume miss, and Desktop never restores one",
        };
    }
    let Some((cwd, origin_cwd)) = portable_cwd else {
        return ExportVerdict::Withhold {
            reason: WithholdReason::CwdOutsideRoots,
            rule: "the working directory lies outside every declared root",
        };
    };
    ExportVerdict::Export {
        descriptor: Box::new(local.with_cwd(Some(cwd), origin_cwd)),
    }
}

/// Decides whether a card from another machine may be written into the local Desktop store.
///
/// `local_cwd` is the translated directory, `None` when the portable form names a root this
/// machine does not declare. The order of the checks is the order in which they get cheaper to
/// be wrong about: no transcript at all, then the store, then the disk, then the link.
#[must_use]
pub fn import_verdict(
    remote: &Descriptor,
    local_cwd: Option<(String, Option<String>)>,
    facts: MachineFacts<'_>,
) -> ImportVerdict {
    if remote.cli_session_id.is_none() {
        return ImportVerdict::Skip {
            reason: SkipReason::NoTranscript,
            rule: "the card names no transcript, so there is nothing to resume",
        };
    }
    if remote.is_transcript_unavailable() {
        return ImportVerdict::Skip {
            reason: SkipReason::TranscriptMissing,
            rule: "the card arrived already marked unavailable",
        };
    }
    let Some((cwd, origin_cwd)) = local_cwd else {
        return ImportVerdict::Skip {
            reason: SkipReason::RootUnknown,
            rule: "the portable path names a root this machine does not declare",
        };
    };
    if !facts.transcript_in_store {
        return ImportVerdict::Skip {
            reason: SkipReason::TranscriptMissing,
            rule: "the transcript is not in the store yet: adopt the card on a later tick",
        };
    }
    if facts.cwd_exists != Some(cwd.as_str()) {
        return ImportVerdict::Skip {
            reason: SkipReason::CwdMissing,
            rule: "the translated working directory was not measured to exist here",
        };
    }
    if facts.confirmed_transcript_path.is_none() {
        return ImportVerdict::Skip {
            reason: SkipReason::LinkUnconfirmed,
            rule: "no transcript_path of this machine confirms the link the resume goes through",
        };
    }
    ImportVerdict::Import {
        descriptor: Box::new(remote.with_cwd(Some(cwd), origin_cwd)),
    }
}

/// Repairs a card Desktop broke, from the version in the outbox.
///
/// Returns the card to write, or `None` when there is nothing to repair or nothing to repair it
/// with. The transcript id comes from the outbox — the shadow `cliSessionId` — and the stale mark
/// is dropped with it: leaving the mark would send Desktop straight back to the same verdict.
#[must_use]
pub fn shadow_repair(
    local: &Descriptor,
    exported: &Descriptor,
    facts: MachineFacts<'_>,
) -> Option<Descriptor> {
    let broken = local.cli_session_id.is_none() || local.is_transcript_unavailable();
    if !broken {
        return None;
    }
    let shadow = exported.cli_session_id.as_ref()?;
    if exported.is_transcript_unavailable() {
        return None;
    }
    // Handing the card a transcript this machine cannot reach only buys the next resume miss.
    if !facts.transcript_in_store || facts.confirmed_transcript_path.is_none() {
        return None;
    }
    Some(local.with_transcript(shadow.clone()))
}
