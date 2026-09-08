//! `SessionStart`: put the link in place before the CLI needs it.
//!
//! Measured on 2.1.258 and 2.1.232, on `-p`, stream-json and an interactive TTY alike: when this
//! hook exits, neither the transcript nor even its directory `projects/<enc>` exists. That is the
//! whole opportunity — the engine can put a symlink there first, and every later write of the
//! session lands in the store by itself. Miss it, and the CLI creates a real directory whose
//! transcripts nobody outside this machine will ever see.
//!
//! Three rules bound what the hook may do, and all three are about not destroying a session:
//! it never touches the network, it never re-aims an existing link, and it never turns a real
//! directory into a link while a session may be writing into it. When it cannot act, it says so
//! through `additionalContext` and exits 0.

use std::path::{Path, PathBuf};

use vibememory_core::naming::{EncSlug, PathSyntax, StoreName};

use crate::install::Layout;

/// What the hook decided to do, before anything is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Create `projects/<enc>` pointing at the store directory.
    Link {
        /// The encoded directory name, taken from `transcript_path` — the encoding is the CLI's
        /// own answer, never our guess.
        enc: String,
        /// The store directory it points at.
        name: String,
    },
    /// The link is already there and already right.
    AlreadyLinked {
        /// The encoded directory name.
        enc: String,
        /// The store directory it points at. Carried even though nothing has to be created: this
        /// session's `transcript_path` proves the link, and a proof is worth recording whether or
        /// not the link had to be made. Without it a machine whose links were all put in place by
        /// the migration would never confirm a single one, and everything that waits for a
        /// confirmed path — importing another machine's cards, repairing a broken one — would
        /// wait for ever.
        name: String,
    },
    /// A real directory holds transcripts of earlier sessions; they have to be copied into the
    /// store before anything is linked, and that is the tick's job, not the hook's.
    ImportNeeded {
        /// The encoded directory name.
        enc: String,
        /// The store directory the transcripts belong in.
        name: String,
    },
    /// A link points somewhere else. Never re-aimed automatically: under a live session that
    /// costs the session, and the engine cannot know from here whether one is running on another
    /// machine.
    Disagreement {
        /// The encoded directory name.
        enc: String,
        /// Where the existing link points.
        found: String,
        /// Where this machine thinks it should point.
        wanted: String,
    },
    /// The working directory is one the engine leaves alone.
    Ignored {
        /// Why.
        reason: String,
    },
}

impl Decision {
    /// The store directory this session's link points at, when the link is in place.
    ///
    /// Both outcomes count: the link this session created, and the link it merely found. The
    /// session's `transcript_path` proves either one, and only a decision that leaves no working
    /// link — an import still owed, a disagreement, an ignored directory — has no name to give.
    #[must_use]
    pub fn store_name(&self) -> Option<&str> {
        match self {
            Self::Link { name, .. } | Self::AlreadyLinked { name, .. } => Some(name),
            Self::ImportNeeded { .. } | Self::Disagreement { .. } | Self::Ignored { .. } => None,
        }
    }

    /// The sentence the session should see, if any. Silence is the normal outcome: a hook that
    /// speaks on every start trains the reader to ignore it.
    #[must_use]
    pub fn additional_context(&self) -> Option<String> {
        match self {
            Self::Link { .. } | Self::AlreadyLinked { .. } | Self::Ignored { .. } => None,
            Self::ImportNeeded { enc, name } => Some(format!(
                "VibeMemory: {enc} is a real directory with transcripts in it, so this session is \
                 not synchronised yet. They will be imported into projects/{name} by the next \
                 tick; nothing is lost."
            )),
            Self::Disagreement { enc, found, wanted } => Some(format!(
                "VibeMemory: {enc} already points at {found}, but this machine resolves it to \
                 {wanted}. The link is left exactly as it is and the engine will not change \
                 it: re-aiming a link under a live session loses that session, and from here \
                 there is no way to tell whether one runs on another machine."
            )),
        }
    }
}

/// Decides what to do for one session, reading only what is already on this disk.
///
/// `resolved` is the store name the core produced, or `None` when the core said to ignore this
/// working directory.
#[must_use]
pub fn decide(
    layout: &Layout,
    enc: &EncSlug,
    resolved: Option<&StoreName>,
    ignored_reason: Option<&str>,
) -> Decision {
    let Some(name) = resolved else {
        return Decision::Ignored {
            reason: ignored_reason
                .unwrap_or("the working directory is ignored")
                .to_owned(),
        };
    };
    let link = layout.config_dir.join("projects").join(enc.as_str());
    let target = layout.store().join("projects").join(name.as_str());

    match std::fs::symlink_metadata(&link) {
        Err(_) => Decision::Link {
            enc: enc.as_str().to_owned(),
            name: name.as_str().to_owned(),
        },
        Ok(metadata) if metadata.is_symlink() => match std::fs::read_link(&link) {
            Ok(found) if found == target => Decision::AlreadyLinked {
                enc: enc.as_str().to_owned(),
                name: name.as_str().to_owned(),
            },
            Ok(found) => Decision::Disagreement {
                enc: enc.as_str().to_owned(),
                found: found.display().to_string(),
                wanted: target.display().to_string(),
            },
            Err(error) => Decision::Disagreement {
                enc: enc.as_str().to_owned(),
                found: error.to_string(),
                wanted: target.display().to_string(),
            },
        },
        // A real directory. Whether it holds anything decides who deals with it.
        Ok(_) => {
            if directory_is_empty(&link) {
                Decision::Link {
                    enc: enc.as_str().to_owned(),
                    name: name.as_str().to_owned(),
                }
            } else {
                Decision::ImportNeeded {
                    enc: enc.as_str().to_owned(),
                    name: name.as_str().to_owned(),
                }
            }
        }
    }
}

/// Whether a directory holds nothing that matters. A `memory` subdirectory the CLI creates on
/// every start is not content: it appears before the first transcript does.
fn directory_is_empty(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    !entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        name != "memory" && name != ".keep"
    })
}

/// Carries out a decision. Only [`Decision::Link`] does anything at all.
///
/// # Errors
///
/// The text of what went wrong. The caller still exits 0: a hook that fails the session it was
/// meant to help is worse than a session without synchronisation.
pub fn perform(layout: &Layout, decision: &Decision) -> Result<(), String> {
    let Decision::Link { enc, name } = decision else {
        return Ok(());
    };
    let target = layout.store().join("projects").join(name);
    let link = layout.config_dir.join("projects").join(enc);

    std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    keep_marker(&target)?;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    // An empty real directory may sit there: the CLI creates one for a working directory it has
    // seen, and `decide` already established that it holds nothing.
    if let Ok(metadata) = std::fs::symlink_metadata(&link)
        && metadata.is_dir()
        && !metadata.is_symlink()
    {
        std::fs::remove_dir_all(&link).map_err(|error| error.to_string())?;
    }
    symlink_dir(&target, &link)
}

/// Writes `.keep` so that git carries the directory and the CLI's sweeper leaves it alone.
fn keep_marker(dir: &Path) -> Result<(), String> {
    let path = dir.join(".keep");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn symlink_dir(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|error| error.to_string())
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).map_err(|error| error.to_string())
    }
}

/// The syntax of this machine, for parsing what the CLI reports.
#[must_use]
pub fn host_syntax() -> PathSyntax {
    if cfg!(windows) {
        PathSyntax::Windows
    } else {
        PathSyntax::Posix
    }
}

/// Where the engine keeps what one hook run learned, for the tick to pick up.
#[must_use]
pub fn last_hook_path(layout: &Layout) -> PathBuf {
    layout.engine_dir.join("last-hook.json")
}
