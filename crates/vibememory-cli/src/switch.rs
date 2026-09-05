//! `switch --from <dir>`: turning this machine from the old synced folder to the store.
//!
//! Everything here is link surgery on `~/.claude`, and all of it is recorded before it is done:
//! every link that is re-aimed and every symlink replaced by a file goes into a rollback file
//! first, so that `switch --rollback` can put the machine back exactly as it was. The old folder
//! itself is not touched.
//!
//! Two things are refused rather than forced. A real project directory stays a directory —
//! `import` handles it, with its liveness check. And the Desktop store is only swapped while
//! Desktop is not running: it rewrites cards on focus, and a directory pulled from under it
//! loses whichever card it was writing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The rollback file, under the engine directory.
pub const ROLLBACK_FILE: &str = "switch-rollback.json";
/// The old scheme's shared store directory inside the synced folder.
const ALL_DIR: &str = "-ALL-";
/// Files of the config directory the old scheme linked into the synced folder.
const LINKED_FILES: &[&str] = &["CLAUDE.md", "settings.json"];

/// One thing the switch changed, and how to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Change {
    /// A symlink that pointed somewhere else and now points into the store.
    Relinked {
        /// The link's own path.
        link: PathBuf,
        /// Where it pointed before.
        old_target: PathBuf,
        /// Where it points now.
        new_target: PathBuf,
    },
    /// A symlink replaced by a regular file with the store's copy.
    Materialized {
        /// The file's path.
        path: PathBuf,
        /// Where the symlink pointed before.
        old_target: PathBuf,
    },
    /// The Desktop store: a symlink moved aside and a real directory created in its place.
    DesktopStore {
        /// The store's path.
        path: PathBuf,
        /// Where the old symlink was moved to.
        moved_to: PathBuf,
    },
}

/// What the switch did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Switched {
    /// Changes made, in order; the rollback file holds the same list.
    pub changes: Vec<Change>,
    /// Links left alone because they point somewhere the switch does not understand.
    pub skipped: Vec<(PathBuf, String)>,
    /// Real project directories, left for `import`.
    pub real_directories: Vec<PathBuf>,
}

/// What the switch needs to know about the machine.
#[derive(Debug, Clone)]
pub struct SwitchInput<'a> {
    /// `~/.claude`.
    pub config_dir: &'a Path,
    /// The store clone.
    pub store: &'a Path,
    /// The old synced folder.
    pub from: &'a Path,
    /// The Desktop store path, when the machine has one.
    pub desktop_store: Option<&'a Path>,
    /// Whether Desktop is running right now — the caller checks the process list.
    pub desktop_running: bool,
    /// Where the rollback file goes.
    pub engine_dir: &'a Path,
}

/// The encoded project directories of sessions running on this machine right now, read from the
/// CLI's own registry `sessions/<pid>.json`.
///
/// A local pid is the right witness here and nowhere else: the switch re-aims links on this
/// machine, and the session that would lose its transcript is a session of this machine. The
/// registry keeps crash leftovers, so a pid that is gone is not a session.
#[must_use]
pub fn live_project_dirs(config_dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(config_dir.join("sessions")) else {
        return Vec::new();
    };
    let mut live = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        // The registry writes the pid as a string; a number is accepted in case that changes.
        let pid = value.get("pid").and_then(|pid| {
            pid.as_str()
                .and_then(|text| text.parse::<u32>().ok())
                .or_else(|| pid.as_u64().and_then(|n| u32::try_from(n).ok()))
        });
        let cwd = value.get("cwd").and_then(serde_json::Value::as_str);
        let (Some(pid), Some(cwd)) = (pid, cwd) else {
            continue;
        };
        if !crate::process::is_running(pid) {
            continue;
        }
        let physical =
            std::fs::canonicalize(cwd).map_or_else(|_| cwd.to_owned(), |p| p.display().to_string());
        let canonical = vibememory_core::naming::canonical_cwd(
            &physical,
            vibememory_core::naming::PathSyntax::Posix,
        );
        if let Ok(enc) = vibememory_core::naming::encode_cwd(&canonical) {
            live.push((enc.as_str().to_owned(), cwd.to_owned()));
        }
    }
    live
}

/// Whether Desktop has a session alive on this machine, by the CLI's own registry: a live pid
/// whose `entrypoint` is `claude-desktop` was spawned by Desktop, and Desktop is therefore
/// running whatever the process list says. The process list alone is not enough — `pgrep` did
/// not see the Desktop process on the first real run, while its two child sessions were there.
#[must_use]
pub fn desktop_session_live(config_dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(config_dir.join("sessions")) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        let spawned_by_desktop =
            value.get("entrypoint").and_then(serde_json::Value::as_str) == Some("claude-desktop");
        let pid = value.get("pid").and_then(|pid| {
            pid.as_str()
                .and_then(|text| text.parse::<u32>().ok())
                .or_else(|| pid.as_u64().and_then(|n| u32::try_from(n).ok()))
        });
        spawned_by_desktop && pid.is_some_and(crate::process::is_running)
    })
}

/// Plans and, unless `dry_run`, performs the switch.
///
/// # Errors
///
/// The text of what went wrong. Whatever was already changed is in the rollback file.
pub fn switch(input: &SwitchInput<'_>, dry_run: bool) -> Result<Switched, String> {
    let mut done = Switched::default();
    relink_projects(input, dry_run, &mut done)?;
    materialize_files(input, dry_run, &mut done)?;
    relink_skills(input, dry_run, &mut done)?;
    swap_desktop_store(input, dry_run, &mut done)?;
    if !dry_run {
        save_rollback(input.engine_dir, &done.changes)?;
    }
    Ok(done)
}

/// Step one, project links: every symlink into the old shared store is re-aimed at the store. A real
/// directory is reported and left: `import` handles it, with its liveness check.
fn relink_projects(
    input: &SwitchInput<'_>,
    dry_run: bool,
    done: &mut Switched,
) -> Result<(), String> {
    let old_all = canonical(&input.from.join("projects").join(ALL_DIR));
    let live = live_project_dirs(input.config_dir);
    for entry in list(&input.config_dir.join("projects"))? {
        let link = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&link) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some((_, cwd)) = live.iter().find(|(enc, _)| *enc == name) {
            done.skipped.push((
                link,
                format!(
                    "a session is running in {cwd} right now; re-aiming its link would lose it"
                ),
            ));
            continue;
        }
        if !metadata.is_symlink() {
            if metadata.is_dir() {
                done.real_directories.push(link);
            }
            continue;
        }
        let Ok(target) = std::fs::read_link(&link) else {
            continue;
        };
        let Some(repo) = repo_of(&target, &old_all) else {
            done.skipped.push((
                link,
                "points outside the old shared store; not ours to move".to_owned(),
            ));
            continue;
        };
        let new_target = input.store.join("projects").join(&repo);
        if !new_target.is_dir() {
            done.skipped.push((
                link,
                format!("projects/{repo} is not in the store yet; migrate first"),
            ));
            continue;
        }
        if !dry_run {
            relink(&link, &new_target)?;
        }
        done.changes.push(Change::Relinked {
            link,
            old_target: target,
            new_target,
        });
    }
    Ok(())
}

/// Step two, the files the old scheme linked into the synced folder become real files holding the
/// store's copy, so that hooks can be added to `settings.json` without writing into the folder
/// the other machine still reads.
fn materialize_files(
    input: &SwitchInput<'_>,
    dry_run: bool,
    done: &mut Switched,
) -> Result<(), String> {
    for name in LINKED_FILES {
        let path = input.config_dir.join(name);
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_symlink() {
            continue;
        }
        let Ok(old_target) = std::fs::read_link(&path) else {
            continue;
        };
        let in_store = input.store.join("config").join(name);
        if !in_store.is_file() {
            done.skipped
                .push((path, format!("config/{name} is not in the store yet")));
            continue;
        }
        if !dry_run {
            let bytes = std::fs::read(&in_store).map_err(|error| error.to_string())?;
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        }
        done.changes.push(Change::Materialized { path, old_target });
    }
    Ok(())
}

/// Step three, skills: the link moves from the synced folder to the store's copy.
fn relink_skills(
    input: &SwitchInput<'_>,
    dry_run: bool,
    done: &mut Switched,
) -> Result<(), String> {
    let skills = input.config_dir.join("skills");
    let Ok(metadata) = std::fs::symlink_metadata(&skills) else {
        return Ok(());
    };
    if !metadata.is_symlink() {
        return Ok(());
    }
    let Ok(old_target) = std::fs::read_link(&skills) else {
        return Ok(());
    };
    let new_target = input.store.join("config").join("skills");
    if !new_target.is_dir() || old_target == new_target {
        return Ok(());
    }
    if !dry_run {
        relink(&skills, &new_target)?;
    }
    done.changes.push(Change::Relinked {
        link: skills,
        old_target,
        new_target,
    });
    Ok(())
}

/// Step four, the Desktop store: from a link into the synced folder to a real directory. Only while
/// Desktop is closed — it rewrites a card on every focus, and a directory pulled from under it
/// loses whichever card it was writing.
fn swap_desktop_store(
    input: &SwitchInput<'_>,
    dry_run: bool,
    done: &mut Switched,
) -> Result<(), String> {
    let Some(desktop) = input.desktop_store else {
        return Ok(());
    };
    let Ok(metadata) = std::fs::symlink_metadata(desktop) else {
        return Ok(());
    };
    if !metadata.is_symlink() {
        return Ok(());
    }
    if input.desktop_running {
        done.skipped.push((
            desktop.to_path_buf(),
            "Desktop is running; quit it and run switch again".to_owned(),
        ));
        return Ok(());
    }
    let moved_to = sibling(desktop, ".bak-vibememory");
    if !dry_run {
        std::fs::rename(desktop, &moved_to).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(desktop).map_err(|error| error.to_string())?;
    }
    done.changes.push(Change::DesktopStore {
        path: desktop.to_path_buf(),
        moved_to,
    });
    Ok(())
}

/// `<path><suffix>` beside the path.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!("{name}{suffix}"))
}

/// Undoes everything the rollback file records, newest change first.
///
/// # Errors
///
/// The text of what went wrong; changes already undone stay undone, the rest stay recorded.
pub fn rollback(engine_dir: &Path) -> Result<Vec<Change>, String> {
    let path = engine_dir.join(ROLLBACK_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut changes: Vec<Change> =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut undone = Vec::new();
    while let Some(change) = changes.pop() {
        match &change {
            Change::Relinked {
                link, old_target, ..
            } => relink(link, old_target)?,
            Change::Materialized { path, old_target } => {
                std::fs::remove_file(path).map_err(|error| error.to_string())?;
                symlink(old_target, path)?;
            }
            Change::DesktopStore { path, moved_to } => {
                // The real directory may hold cards written since; they are kept beside the
                // restored link rather than thrown away.
                let kept = path.with_file_name(format!(
                    "{}.switched-vibememory",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
                std::fs::rename(path, &kept).map_err(|error| error.to_string())?;
                std::fs::rename(moved_to, path).map_err(|error| error.to_string())?;
            }
        }
        undone.push(change);
        save_rollback(engine_dir, &changes)?;
    }
    let _ = std::fs::remove_file(&path);
    Ok(undone)
}

/// Which old store a link into `<from>/projects/-ALL-/<repo>` names.
fn repo_of(target: &Path, old_all: &Path) -> Option<String> {
    let resolved = canonical(target);
    let rest = resolved.strip_prefix(old_all).ok()?;
    let mut components = rest.components();
    let repo = components
        .next()?
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    // Only a link straight at the store directory: a deeper one is something else.
    components.next().is_none().then_some(repo)
}

/// The path with symlinks resolved when it exists, as written otherwise.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Replaces a symlink with one pointing at `target`. The window between the two calls is why
/// `switch` refuses to touch a link under a live session.
fn relink(link: &Path, target: &Path) -> Result<(), String> {
    std::fs::remove_file(link).map_err(|error| format!("{}: {error}", link.display()))?;
    symlink(target, link)
}

fn symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|error| error.to_string())
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).map_err(|error| error.to_string())
    }
}

fn save_rollback(engine_dir: &Path, changes: &[Change]) -> Result<(), String> {
    std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(changes).map_err(|error| error.to_string())?;
    std::fs::write(engine_dir.join(ROLLBACK_FILE), text).map_err(|error| error.to_string())
}

fn list(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut found: Vec<std::fs::DirEntry> = entries.filter_map(Result::ok).collect();
            found.sort_by_key(std::fs::DirEntry::file_name);
            Ok(found)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{}: {error}", dir.display())),
    }
}
