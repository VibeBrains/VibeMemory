//! What the machine must look like, and how far it is from that.
//!
//! One function computes the desired state as a list of steps; the three commands differ only in
//! what they do with that list. `status` prints it, `doctor` refuses when a step that must hold
//! does not, `install` applies it. Written any other way, the three would drift, and the one that
//! drifts is always the one that repairs.
//!
//! Nothing here re-aims or deletes what somebody else made. A link that points elsewhere is a
//! conflict to report, never a thing to fix silently: under a live session, replacing a link
//! costs the session.

use std::path::{Path, PathBuf};

use crate::config::Config;

/// Git settings the store repository must carry, with the reason each one exists.
const GIT_SETTINGS: &[(&str, &str, &str)] = &[
    (
        "core.autocrlf",
        "false",
        "a transcript is bytes: CRLF conversion would duplicate the state block of every line",
    ),
    (
        "core.filemode",
        "false",
        "the executable bit differs between machines and means nothing here",
    ),
    (
        "core.symlinks",
        "false",
        "the store holds no symlinks; a checkout must not invent them",
    ),
    (
        "core.precomposeunicode",
        "true",
        "APFS reports NFD names: without this git sees a second, different name",
    ),
];

/// Contents of the store's `.gitattributes`. The order is fixed so the file compares equal.
pub const GITATTRIBUTES: &str =
    "* -text\n* merge=vibememory-keepboth\n**/*.jsonl merge=vibememory-jsonl\n";

/// The merge drivers git must know about, as `merge.<name>.driver` settings. `%O %A %B %P` is
/// git's own vocabulary: base, ours, theirs, and the path inside the repository.
const MERGE_DRIVERS: &[(&str, &str)] = &[
    (
        "merge.vibememory-jsonl.driver",
        "vibememory merge-driver jsonl %O %A %B %P",
    ),
    (
        "merge.vibememory-keepboth.driver",
        "vibememory merge-driver keepboth %O %A %B %P",
    ),
];

/// Files of the config directory the store manages as copies.
/// The files of the config directory the store keeps a copy of.
pub const MANAGED_FILES: &[&str] = &["CLAUDE.md", "settings.json"];

/// Where everything lives on this machine.
#[derive(Debug, Clone)]
pub struct Layout {
    /// `~/.claude` — the config directory of Claude Code.
    pub config_dir: PathBuf,
    /// `~/.vibememory` — the engine's own directory.
    pub engine_dir: PathBuf,
}

impl Layout {
    /// The store clone: `<engine>/store`.
    #[must_use]
    pub fn store(&self) -> PathBuf {
        self.engine_dir.join("store")
    }
}

/// One thing that must be true about this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A git setting of the store repository.
    GitSetting {
        /// Key, e.g. `core.autocrlf`.
        key: &'static str,
        /// The value it must have.
        value: &'static str,
    },
    /// The store's `.gitattributes`, which decides how merges are driven.
    Gitattributes,
    /// A directory the store must hold.
    StoreDir {
        /// Relative to the store.
        relative: String,
    },
    /// A file of the config directory that the store keeps a copy of.
    ManagedCopy {
        /// File name, e.g. `CLAUDE.md`.
        name: &'static str,
    },
    /// `~/.claude/skills` → `<store>/config/skills`.
    SkillsLink,
    /// The scheduled tick: a `LaunchAgent` on macOS.
    Schedule,
    /// The engine's hooks in `settings.json`.
    Hooks,
    /// No deletions held back by the tick's cap and waiting for a person.
    DeletionsHeld,
    /// No flag in the `env` of `settings.json` that switches the prompt cache off or shortens it.
    /// Such a flag travels to every machine with the managed copy, and costs a share of the
    /// usage limits on each of them.
    PromptCacheEnv,
    /// A copy of this binary under the engine directory, so hooks and the scheduler survive a
    /// rebuild or a `cargo clean` of wherever it was built.
    Binary,
    /// The installed binary signed with the owner's own identity, so macOS stops asking for the
    /// same permission after every rebuild.
    BinarySigned {
        /// The identity `codesign` is asked for, e.g. `VibeMemory Local`.
        identity: String,
    },
    /// The store's own files committed, so a clone starts with them.
    ScaffoldCommitted {
        /// Whose `machines/<id>` directory belongs to the scaffolding.
        machine_id: String,
    },
    /// `~/.claude/projects/<enc>` → `<store>/projects/<name>`.
    ProjectLink {
        /// The encoded directory name the CLI uses.
        enc: String,
        /// The store directory it must point at.
        name: String,
    },
}

impl Step {
    /// A short line for `status` and `doctor`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::GitSetting { key, value } => format!("git config {key}={value}"),
            Self::Gitattributes => ".gitattributes of the store".to_owned(),
            Self::StoreDir { relative } => format!("directory {relative}"),
            Self::ManagedCopy { name } => format!("managed copy of {name}"),
            Self::SkillsLink => "skills link".to_owned(),
            Self::Schedule => "scheduled tick".to_owned(),
            Self::Hooks => "hooks in settings.json".to_owned(),
            Self::PromptCacheEnv => "prompt cache not switched off in settings.json".to_owned(),
            Self::DeletionsHeld => "no deletions waiting for a decision".to_owned(),
            Self::ScaffoldCommitted { .. } => "store scaffolding committed".to_owned(),
            Self::Binary => "engine binary in place".to_owned(),
            Self::BinarySigned { identity } => format!("engine binary signed by {identity}"),
            Self::ProjectLink { enc, name } => format!("link {enc} -> projects/{name}"),
        }
    }
}

/// How far the machine is from one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Already true.
    Satisfied,
    /// Not there yet; `install` will make it so.
    Missing,
    /// Something else is there. `install` does not touch it, and `doctor` reports it: the engine
    /// never overwrites what it did not create.
    Conflict {
        /// What was found instead.
        found: String,
    },
    /// The state could not be read (permissions, a mount that stopped answering).
    Unknown {
        /// What went wrong.
        reason: String,
    },
}

/// A step together with where the machine stands on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// What must be true.
    pub step: Step,
    /// Whether it is.
    pub state: State,
}

impl Action {
    /// Whether the machine already satisfies this step.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.state == State::Satisfied
    }
}

/// The directories the store always holds. `machines/<id>` is added per machine.
fn store_dirs(machine_id: &str) -> Vec<String> {
    vec![
        "projects".to_owned(),
        "config".to_owned(),
        "config/skills".to_owned(),
        format!("machines/{machine_id}"),
    ]
}

/// Computes what must be true and how far the machine is from it.
///
/// `links` are the records this machine already knows about; a link is planned for each one, so
/// that a project fetched from another machine has its directory before any session needs it.
#[must_use]
pub fn plan(layout: &Layout, config: &Config, links: &[(String, String)]) -> Vec<Action> {
    let store = layout.store();
    let mut actions = Vec::new();

    for (key, value, _why) in GIT_SETTINGS {
        actions.push(Action {
            step: Step::GitSetting { key, value },
            state: git_setting_state(&store, key, value),
        });
    }
    for (key, value) in MERGE_DRIVERS {
        actions.push(Action {
            step: Step::GitSetting { key, value },
            state: git_setting_state(&store, key, value),
        });
    }
    actions.push(Action {
        step: Step::Gitattributes,
        state: file_state(&store.join(".gitattributes"), GITATTRIBUTES),
    });
    for relative in store_dirs(&config.machine_id) {
        let state = dir_state(&store.join(&relative));
        actions.push(Action {
            step: Step::StoreDir { relative },
            state,
        });
    }
    for name in MANAGED_FILES {
        actions.push(Action {
            step: Step::ManagedCopy { name },
            state: managed_copy_state(layout, &store, name),
        });
    }
    actions.push(Action {
        step: Step::SkillsLink,
        state: link_state(
            &layout.config_dir.join("skills"),
            &store.join("config/skills"),
        ),
    });
    // The binary first: the hooks and the scheduler name it.
    let binary = binary_state(layout);
    let binary_will_be_replaced = matches!(binary, State::Missing);
    actions.push(Action {
        step: Step::Binary,
        state: binary,
    });
    // Signing is macOS's answer to "why does it ask me again after every rebuild"; elsewhere the
    // question does not exist, so neither does the step.
    if cfg!(target_os = "macos")
        && let Some(identity) = config.signing_identity.as_deref()
    {
        actions.push(Action {
            step: Step::BinarySigned {
                identity: identity.to_owned(),
            },
            state: signing_state(
                binary_will_be_replaced,
                signature_state(&installed_binary(layout), identity),
            ),
        });
    }
    actions.push(Action {
        step: Step::PromptCacheEnv,
        state: prompt_cache_env_state(layout),
    });
    actions.push(Action {
        step: Step::DeletionsHeld,
        state: deletions_held_state(layout),
    });
    actions.push(Action {
        step: Step::Hooks,
        state: hooks_state(layout),
    });
    // Last: it commits what the steps above created.
    actions.push(Action {
        step: Step::ScaffoldCommitted {
            machine_id: config.machine_id.clone(),
        },
        state: scaffolding_state(&store, &config.machine_id),
    });
    // The scheduled tick exists only where there is a scheduler this build knows about.
    if cfg!(target_os = "macos") {
        actions.push(Action {
            step: Step::Schedule,
            state: file_state(&schedule_path(layout), &launch_agent(layout)),
        });
    }
    for (enc, name) in links {
        let state = link_state(
            &layout.config_dir.join("projects").join(enc),
            &store.join("projects").join(name),
        );
        actions.push(Action {
            step: Step::ProjectLink {
                enc: enc.clone(),
                name: name.clone(),
            },
            state,
        });
    }
    actions
}

/// Reads one git setting of the store.
fn git_setting_state(store: &Path, key: &str, value: &str) -> State {
    if !store.join(".git").exists() {
        return State::Missing;
    }
    let mut command = std::process::Command::new("git");
    command
        .args(["config", "--local", "--get", key])
        .current_dir(store)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match crate::git::run_with_timeout(command, std::time::Duration::from_secs(10)) {
        Err(reason) => State::Unknown { reason },
        Ok(None) => State::Missing,
        Ok(Some(found)) if found == value => State::Satisfied,
        Ok(Some(found)) => State::Conflict { found },
    }
}

/// A file that must hold exactly this text.
fn file_state(path: &Path, expected: &str) -> State {
    match std::fs::read_to_string(path) {
        Ok(found) if found == expected => State::Satisfied,
        Ok(found) => State::Conflict {
            found: summarize(&found),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::Missing,
        Err(error) => State::Unknown {
            reason: error.to_string(),
        },
    }
}

/// A directory that must exist.
fn dir_state(path: &Path) -> State {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => State::Satisfied,
        Ok(_) => State::Conflict {
            found: "not a directory".to_owned(),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::Missing,
        Err(error) => State::Unknown {
            reason: error.to_string(),
        },
    }
}

/// A symlink that must point at `target`.
fn link_state(link: &Path, target: &Path) -> State {
    match std::fs::symlink_metadata(link) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::Missing,
        Err(error) => State::Unknown {
            reason: error.to_string(),
        },
        Ok(metadata) if metadata.is_symlink() => match std::fs::read_link(link) {
            Ok(found) if found == target => State::Satisfied,
            Ok(found) => State::Conflict {
                found: found.display().to_string(),
            },
            Err(error) => State::Unknown {
                reason: error.to_string(),
            },
        },
        // A real directory is not a mistake to overwrite: the first session in a new working
        // directory creates one, and it holds transcripts nobody has imported yet.
        Ok(_) => State::Conflict {
            found: "a real directory".to_owned(),
        },
    }
}

/// A file the store keeps a copy of. Only its presence is planned here; reconciling two edited
/// copies is the tick's business, not the installer's.
/// Where a managed file stands: agreed, about to be brought into agreement, or in conflict.
///
/// The same three-way decision the tick makes (`managed::verdict`), asked without acting: a side
/// that still matches the last-synced base has not moved, so the other side's change is simply
/// owed (`Missing` — `install` performs it, the tick would too). Both sides moved, or no sync was
/// ever recorded while the copies differ, is a conflict a person has to settle.
fn managed_copy_state(layout: &Layout, store: &Path, name: &str) -> State {
    use crate::managed::{ManagedState, Verdict, verdict};

    let local_path = layout.config_dir.join(name);
    let store_path = store.join("config").join(name);
    let local = match std::fs::read(&local_path) {
        Ok(bytes) if nothing_to_share(&local_path, &bytes) => None,
        Ok(bytes) => Some(for_the_store(&local_path, &bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return State::Unknown {
                reason: error.to_string(),
            };
        }
    };
    let store = match std::fs::read(&store_path) {
        Ok(bytes) => Some(for_the_store(&store_path, &bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return State::Unknown {
                reason: error.to_string(),
            };
        }
    };
    let local_hash = local.as_deref().map(crate::sha256::hex);
    let store_hash = store.as_deref().map(crate::sha256::hex);
    let state = ManagedState::read(&layout.engine_dir);
    let base = state.synced.get(name).map(String::as_str);
    match verdict(local_hash.as_deref(), store_hash.as_deref(), base) {
        // Neither side has it, or nothing worth sharing: a resting state, not work left undone.
        // Calling it missing would make `install` perform the same no-op for ever.
        Verdict::Nothing | Verdict::Same => State::Satisfied,
        Verdict::Pull | Verdict::Push => State::Missing,
        Verdict::Conflict => State::Conflict {
            found: if base.is_some() {
                "both copies changed since the last sync; this machine's version is in the \
                 quarantine — make the copies identical to settle it"
                    .to_owned()
            } else {
                "the copies differ and no sync was ever recorded; make them identical once"
                    .to_owned()
            },
        },
    }
}

/// First line of a file, for a conflict message.
fn summarize(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    if text.lines().count() > 1 {
        format!("{first} …")
    } else {
        first.to_owned()
    }
}

/// What applying the plan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Steps that were already true and were not touched.
    pub untouched: Vec<String>,
    /// Steps this run made true.
    pub performed: Vec<String>,
    /// Steps left alone because something else is there, with what was found.
    pub refused: Vec<(String, String)>,
    /// Steps that could not be applied, with the error.
    pub failed: Vec<(String, String)>,
}

impl Applied {
    /// Whether the machine is now as the plan wants it.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.refused.is_empty() && self.failed.is_empty()
    }
}

/// Applies the plan, or reports what it would do when `dry_run`.
///
/// Idempotent by construction: a satisfied step is never touched, and a conflicting one is never
/// resolved — the engine repairs what it made and reports what it did not.
#[must_use]
pub fn apply(layout: &Layout, actions: &[Action], dry_run: bool) -> Applied {
    let mut applied = Applied::default();
    for action in actions {
        let what = action.step.describe();
        match &action.state {
            State::Satisfied => applied.untouched.push(what),
            State::Conflict { found } => applied.refused.push((what, found.clone())),
            State::Unknown { reason } => applied.failed.push((what, reason.clone())),
            State::Missing => {
                if dry_run {
                    applied.performed.push(what);
                    continue;
                }
                match perform(layout, &action.step) {
                    Ok(()) => applied.performed.push(what),
                    Err(error) => applied.failed.push((what, error)),
                }
            }
        }
    }
    applied
}

/// Stages exactly these paths and commits when the index then differs from HEAD. Never
/// `git add -A`: the store may already hold a live transcript or two.
fn commit_scaffolding(store: &Path, paths: &[String]) -> Result<(), String> {
    let timeout = std::time::Duration::from_mins(1);
    let existing: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|path| store.join(path).exists())
        .collect();
    if existing.is_empty() {
        return Ok(());
    }
    let mut args = vec!["add", "--"];
    args.extend(existing.iter().copied());
    crate::git::run_with_timeout(crate::git::command(store, &args), timeout)?
        .ok_or_else(|| "git refused to stage the scaffolding".to_owned())?;
    let has_head = crate::git::run_with_timeout(
        crate::git::command(store, &["rev-parse", "--verify", "--quiet", "HEAD"]),
        timeout,
    )?
    .is_some();
    let dirty = !has_head
        || crate::git::run_with_timeout(
            crate::git::command(store, &["diff-index", "--quiet", "--cached", "HEAD", "--"]),
            timeout,
        )?
        .is_none();
    if !dirty {
        return Ok(());
    }
    crate::git::run_with_timeout(
        crate::git::command(store, &["commit", "--quiet", "-m", "vibememory: install"]),
        timeout,
    )?
    .ok_or_else(|| "git refused to commit the scaffolding".to_owned())?;
    Ok(())
}

/// Makes one step true.
fn perform(layout: &Layout, step: &Step) -> Result<(), String> {
    let store = layout.store();
    match step {
        Step::GitSetting { key, value } => set_git_setting(&store, key, value),
        Step::Gitattributes => write_new(&store.join(".gitattributes"), GITATTRIBUTES.as_bytes()),
        Step::StoreDir { relative } => {
            let path = store.join(relative);
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            // A directory with nothing in it is not carried by git, and the CLI's own sweeper
            // removes empty directories under the config root.
            write_new(&path.join(".keep"), b"")
        }
        Step::ManagedCopy { name } => reconcile_managed(layout, &store, name),
        // Nothing to apply: a flag that switches the cache off is the owner's to remove, and the
        // plan never reports this step as missing — only satisfied or in conflict. The same goes
        // for a held deletion: releasing it is a decision, not a repair.
        Step::PromptCacheEnv | Step::DeletionsHeld => Ok(()),
        Step::Schedule => install_schedule(layout),
        Step::Binary => install_binary(layout),
        Step::BinarySigned { identity } => sign_binary(&installed_binary(layout), identity),
        Step::Hooks => write_hooks(layout),
        Step::ScaffoldCommitted { machine_id } => {
            commit_scaffolding(&store, &scaffolding_paths(machine_id))
        }
        Step::SkillsLink => make_link(
            &store.join("config/skills"),
            &layout.config_dir.join("skills"),
        ),
        Step::ProjectLink { enc, name } => make_link(
            &store.join("projects").join(name),
            &layout.config_dir.join("projects").join(enc),
        ),
    }
}

/// Writes a file only when it does not exist: `install` never overwrites content it did not
/// write, even between the plan and the apply.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => file.write_all(bytes).map_err(|e| e.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Copies a managed file into the store, or back out when only the store has it.
/// Brings one managed file into agreement, the same way the tick does.
fn reconcile_managed(layout: &Layout, store: &Path, name: &str) -> Result<(), String> {
    let mut state = crate::managed::ManagedState::read(&layout.engine_dir);
    crate::managed::reconcile_one(layout, store, name, &mut state, &crate::clock::now())?;
    state.write(&layout.engine_dir)
}

/// Creates a symlink, making sure its target exists first: a link to nothing is a link the CLI
/// will replace with a real directory.
fn make_link(target: &Path, link: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    write_new(&target.join(".keep"), b"")?;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link).map_err(|e| e.to_string())
    }
}

/// Sets one git setting of the store, initialising the repository when it is not there yet.
fn set_git_setting(store: &Path, key: &str, value: &str) -> Result<(), String> {
    std::fs::create_dir_all(store).map_err(|e| e.to_string())?;
    if !store.join(".git").exists() {
        run_git(store, &["init", "--quiet"])?;
    }
    run_git(store, &["config", "--local", key, value])
}

/// Runs git in the store and turns a refusal into an error with its own words.
fn run_git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = std::process::Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match crate::git::run_with_timeout(command, std::time::Duration::from_secs(30)) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!("git {} refused", args.join(" "))),
        Err(reason) => Err(reason),
    }
}

/// Where the `LaunchAgent` of this machine lives. Under the engine directory when that has been
/// redirected, so a test never writes into the real `~/Library/LaunchAgents`.
#[must_use]
pub fn schedule_path(layout: &Layout) -> PathBuf {
    match real_launch_agents_dir(layout) {
        // `with_extension` would eat the `.tick`: the label's own dots are part of its name.
        Some(dir) => dir.join(format!("{SCHEDULE_LABEL}.plist")),
        None => layout
            .engine_dir
            .join("LaunchAgents")
            .join(format!("{SCHEDULE_LABEL}.plist")),
    }
}

/// The label launchd knows the tick by.
pub const SCHEDULE_LABEL: &str = "dev.vibememory.tick";

/// `~/Library/LaunchAgents`, but only for the real engine directory: a redirected engine — a
/// test, a dry run in a scratch directory — keeps its agent beside itself and never touches
/// launchd. The real one is the engine under the home directory, and nothing else.
fn real_launch_agents_dir(layout: &Layout) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    (layout.engine_dir == home.join(".vibememory"))
        .then(|| home.join("Library").join("LaunchAgents"))
}

/// Writes the agent and, for the real engine, hands it to launchd.
fn install_schedule(layout: &Layout) -> Result<(), String> {
    let path = schedule_path(layout);
    write_new(&path, launch_agent(layout).as_bytes())?;
    if real_launch_agents_dir(layout).is_none() {
        return Ok(());
    }
    // `bootstrap` loads it now and on every login; an agent already loaded says so and that is
    // not a failure worth stopping install for.
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| e.to_string())?;
    let domain = format!("gui/{}", String::from_utf8_lossy(&uid.stdout).trim());
    let _ = std::process::Command::new("launchctl")
        .args(["bootstrap", &domain, &path.display().to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}

/// Where the engine's own copy of the binary lives.
#[must_use]
pub fn installed_binary(layout: &Layout) -> PathBuf {
    layout.engine_dir.join("bin").join("vibememory")
}

/// The note beside the installed binary saying which build it was copied from.
///
/// Needed because the installed file stops being byte-identical to its source the moment it is
/// signed: signing rewrites the binary. Without the note every `install` and every `doctor` on a
/// signing machine finds a difference, replaces the binary under the running hooks and signs it
/// again — for ever, and for nothing.
fn source_note(layout: &Layout) -> PathBuf {
    layout.engine_dir.join("bin").join("vibememory.source")
}

/// Whether the installed copy is this build.
fn binary_state(layout: &Layout) -> State {
    let Ok(this) = std::env::current_exe().and_then(std::fs::read) else {
        return State::Unknown {
            reason: "this binary could not be read".to_owned(),
        };
    };
    let installed = installed_binary(layout);
    if !installed.exists() {
        return State::Missing;
    }
    // The engine running from its own installed path is, by definition, installed. Without this
    // the check compares the signed copy against the note that records its unsigned source and
    // declares every `doctor` a missing step — the engine reporting itself uninstalled while
    // running.
    if let (Ok(running), Ok(there)) = (
        std::env::current_exe().and_then(std::fs::canonicalize),
        std::fs::canonicalize(&installed),
    ) && running == there
    {
        return State::Satisfied;
    }
    let wanted = crate::sha256::hex(&this);
    // The note is the answer whenever it exists; the byte comparison stays as the fallback for a
    // machine that never signs and for an engine installed before the note existed.
    if let Ok(noted) = std::fs::read_to_string(source_note(layout)) {
        return if noted.trim() == wanted {
            State::Satisfied
        } else {
            State::Missing
        };
    }
    match std::fs::read(&installed) {
        Ok(there) if there == this => State::Satisfied,
        Ok(_) => State::Missing, // an older build: replaced, never kept
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::Missing,
        Err(error) => State::Unknown {
            reason: error.to_string(),
        },
    }
}

/// Copies this binary into the engine directory, through a temporary file and a rename so a
/// hook that fires mid-copy runs either the old binary or the new one, never half of one.
fn install_binary(layout: &Layout) -> Result<(), String> {
    let source = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = installed_binary(layout);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = target.with_extension("new");
    std::fs::copy(&source, &temporary).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temporary, &target).map_err(|e| e.to_string())?;
    // Installed is not the same as working. On macOS a binary written over in place loses its
    // signature and the kernel kills it with SIGKILL — seen on this machine, from a careless
    // `cp` — and the hooks that call it would then fail on every session with nothing to read.
    // The copy is atomic above, so this is a guard against the environment, not against the
    // copy: quarantine attributes, a partially written file, a wrong architecture.
    // Only when we installed ourselves. `current_exe` is whatever is running — in the test suite
    // that is the test harness, and asking a harness for `--version` proves nothing about the
    // engine. In production the two names are the same file name, and the check runs.
    // Which build this copy came from, so the next plan does not mistake a signature for a
    // different build. Written after the rename: a note without its binary would be a lie.
    let source_bytes = std::fs::read(&source).map_err(|e| e.to_string())?;
    std::fs::write(source_note(layout), crate::sha256::hex(&source_bytes))
        .map_err(|e| e.to_string())?;
    if source.file_name() == target.file_name() {
        binary_runs(&target)
    } else {
        Ok(())
    }
}

/// Runs what was just installed and insists it answers.
///
/// # Errors
///
/// What the binary did instead of answering.
pub fn binary_runs(binary: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new(binary);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    match crate::git::run_with_timeout(command, BINARY_CHECK_TIMEOUT) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!(
            "{} was installed but refuses to run",
            binary.display()
        )),
        Err(reason) => Err(format!("{} was installed but {reason}", binary.display())),
    }
}

/// How long the freshly installed binary gets to say its version. It does nothing else on that
/// path, so a second is generous; the point is never to hang the install.
const BINARY_CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The `LaunchAgent` itself: run at load, then every two minutes.
///
/// `StartInterval` is skipped while the machine sleeps rather than fired repeatedly on waking, so
/// the lag after opening the lid is one interval and not a storm of catch-up runs.
#[must_use]
pub fn launch_agent(layout: &Layout) -> String {
    let binary = installed_binary(layout).display().to_string();
    let engine = layout.engine_dir.display();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key><string>dev.vibememory.tick</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array><string>{binary}</string><string>tick</string></array>\n\
         \t<key>EnvironmentVariables</key>\n\
         \t<dict><key>VIBEMEMORY_DIR</key><string>{engine}</string></dict>\n\
         \t<key>RunAtLoad</key><true/>\n\
         \t<key>StartInterval</key><integer>120</integer>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The events the engine listens to, with the subcommand each runs.
const HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart", "session-start"),
    ("Stop", "stop"),
    ("SessionEnd", "session-end"),
    ("UserPromptSubmit", "user-prompt-submit"),
];

/// The command line of one hook: this binary, the subcommand, and the engine directory so a
/// redirected engine keeps working under the CLI, which does not pass the variable through.
fn hook_command(layout: &Layout, subcommand: &str) -> String {
    format!(
        "VIBEMEMORY_DIR={} {} hook {subcommand}",
        layout.engine_dir.display(),
        installed_binary(layout).display()
    )
}

/// Whether `settings.json` carries every hook of the engine.
///
/// A `settings.json` that is a symlink pointing outside the config directory is a conflict, not
/// a place to write: it is the old scheme's link into a synced folder, and hooks written through
/// it would reach another machine with this machine's binary path — and a hook that fails there
/// stops that machine's sessions.
fn hooks_state(layout: &Layout) -> State {
    let path = layout.config_dir.join("settings.json");
    if let Ok(metadata) = std::fs::symlink_metadata(&path)
        && metadata.is_symlink()
        && let Ok(target) = std::fs::read_link(&path)
        && !target.starts_with(&layout.config_dir)
    {
        return State::Conflict {
            found: format!("a link to {}", target.display()),
        };
    }
    let settings: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                return State::Conflict {
                    found: format!("settings.json is not valid JSON: {error}"),
                };
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Err(error) => {
            return State::Unknown {
                reason: error.to_string(),
            };
        }
    };
    let all_present = HOOK_EVENTS.iter().all(|(event, subcommand)| {
        let wanted = hook_command(layout, subcommand);
        settings
            .get("hooks")
            .and_then(|hooks| hooks.get(event))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|matchers| {
                matchers.iter().any(|matcher| {
                    matcher
                        .get("hooks")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|list| {
                            list.iter().any(|hook| {
                                hook.get("command").and_then(serde_json::Value::as_str)
                                    == Some(wanted.as_str())
                            })
                        })
                })
            })
    });
    if all_present {
        State::Satisfied
    } else {
        State::Missing
    }
}

/// Adds the engine's hooks to `settings.json`, keeping everything else in it exactly as it is.
/// Whether the tick is sitting on deletions it refused to make.
///
/// Read from the tick's own state rather than recounted here: the tick already did the counting,
/// and a second implementation of "how many would be deleted" is a second thing to keep true.
fn deletions_held_state(layout: &Layout) -> State {
    let held = crate::guard::TickState::read(&layout.engine_dir).deletions_held;
    if held == 0 {
        State::Satisfied
    } else {
        State::Conflict {
            found: format!(
                "{held} deletion(s) held back by the cap; check machines/*/forgotten.json, then \
                 `vibememory tick --release-deletions` to go ahead once"
            ),
        }
    }
}

/// Prefix of the environment variables that switch the prompt cache off, per model or entirely.
const CACHE_OFF_PREFIX: &str = "DISABLE_PROMPT_CACHING";
/// The variable that shortens every cache to five minutes.
const CACHE_SHORT: &str = "FORCE_PROMPT_CACHING_5M";

/// The variables in a `settings.json` `env` block that switch the prompt cache off or shorten it.
///
/// A value of `""`, `"0"` or `"false"` does not count: that is how such a flag is left in place
/// but turned off, and reporting it would teach people to delete the line instead of reading it.
#[must_use]
pub fn cache_killing_flags(settings: &serde_json::Value) -> Vec<String> {
    let Some(env) = settings.get("env").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let mut found: Vec<String> = env
        .iter()
        .filter(|(key, _)| key.starts_with(CACHE_OFF_PREFIX) || key.as_str() == CACHE_SHORT)
        .filter(|(_, value)| {
            let text = match value {
                serde_json::Value::String(text) => text.trim().to_owned(),
                other => other.to_string(),
            };
            !(text.is_empty() || text == "0" || text.eq_ignore_ascii_case("false"))
        })
        .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or(&value.to_string())))
        .collect();
    found.sort();
    found
}

/// Whether `settings.json` leaves the prompt cache alone.
fn prompt_cache_env_state(layout: &Layout) -> State {
    let path = layout.config_dir.join("settings.json");
    let settings: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(value) => value,
            // Invalid JSON is the hooks step's finding; saying it twice helps nobody.
            Err(_) => return State::Satisfied,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return State::Satisfied,
        Err(error) => {
            return State::Unknown {
                reason: error.to_string(),
            };
        }
    };
    let flags = cache_killing_flags(&settings);
    if flags.is_empty() {
        State::Satisfied
    } else {
        State::Conflict {
            found: format!(
                "env sets {} — the prompt cache costs usage limits on every machine this file \
                 reaches",
                flags.join(", ")
            ),
        }
    }
}

/// Puts this machine's hook commands into `settings.json`, replacing stale ones of ours.
///
/// # Errors
///
/// The text of what went wrong.
pub fn write_hooks(layout: &Layout) -> Result<(), String> {
    let path = layout.config_dir.join("settings.json");
    let mut settings: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Err(error) => return Err(error.to_string()),
    };
    let root = settings
        .as_object_mut()
        .ok_or_else(|| "settings.json is not an object".to_owned())?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| "settings.json: `hooks` is not an object".to_owned())?;
    for (event, subcommand) in HOOK_EVENTS {
        let command = hook_command(layout, subcommand);
        let matchers = hooks
            .entry(*event)
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        let matchers = matchers
            .as_array_mut()
            .ok_or_else(|| format!("settings.json: hooks.{event} is not an array"))?;
        // Ours from an earlier install — another binary path, another engine directory — go
        // away first: two entries per event would run two engines, one of them dead.
        for matcher in matchers.iter_mut() {
            if let Some(list) = matcher
                .get_mut("hooks")
                .and_then(serde_json::Value::as_array_mut)
            {
                list.retain(|hook| {
                    !hook
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|c| is_engine_hook(c) && c != command)
                });
            }
        }
        matchers.retain(|matcher| {
            matcher
                .get("hooks")
                .and_then(serde_json::Value::as_array)
                .is_none_or(|list| !list.is_empty())
        });
        let present = matchers.iter().any(|matcher| {
            matcher
                .get("hooks")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|list| {
                    list.iter().any(|hook| {
                        hook.get("command").and_then(serde_json::Value::as_str)
                            == Some(command.as_str())
                    })
                })
        });
        if !present {
            matchers.push(serde_json::json!({
                "matcher": "*",
                "hooks": [{ "type": "command", "command": command, "timeout": 20 }]
            }));
        }
    }
    let text = serde_json::to_string_pretty(&settings).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.vibememory");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

/// The prefix every hook command of the engine carries: the engine directory, exported for the
/// subprocess because the CLI does not pass the variable through.
const ENGINE_HOOK_PREFIX: &str = "VIBEMEMORY_DIR=";

/// Whether a hook command is one of ours. Decided by its shape, not by the binary's file name:
/// the binary is `vibememory` in an installation and something else under `cargo test`, and a
/// rule that only recognised the former would leave the latter's hooks in the store.
fn is_engine_hook(command: &str) -> bool {
    command.starts_with(ENGINE_HOOK_PREFIX)
        && HOOK_EVENTS
            .iter()
            .any(|(_, subcommand)| command.ends_with(&format!(" hook {subcommand}")))
}

/// Whether a managed file is `settings.json`, the one file whose comparison is structural.
fn is_settings(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "settings.json")
}

/// The bytes of a managed file as they may enter the store.
#[must_use]
pub fn for_the_store(path: &Path, bytes: &[u8]) -> Vec<u8> {
    if !is_settings(path) {
        return bytes.to_vec();
    }
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => {
            let shared = without_engine_hooks(value);
            serde_json::to_vec_pretty(&shared).unwrap_or_else(|_| bytes.to_vec())
        }
        Err(_) => bytes.to_vec(),
    }
}

/// `settings.json` without the engine's hooks. Matchers, events and the `hooks` object itself
/// disappear when the removal empties them, so a file that held nothing but our hooks compares
/// equal to one that never had them.
#[must_use]
pub fn without_engine_hooks(mut settings: serde_json::Value) -> serde_json::Value {
    let Some(root) = settings.as_object_mut() else {
        return settings;
    };
    let Some(hooks) = root
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return settings;
    };
    for matchers in hooks.values_mut() {
        if let Some(list) = matchers.as_array_mut() {
            for matcher in list.iter_mut() {
                if let Some(inner) = matcher
                    .get_mut("hooks")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    inner.retain(|hook| {
                        !hook
                            .get("command")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(is_engine_hook)
                    });
                }
            }
            list.retain(|matcher| {
                matcher
                    .get("hooks")
                    .and_then(serde_json::Value::as_array)
                    .is_none_or(|inner| !inner.is_empty())
            });
        }
    }
    hooks.retain(|_, matchers| matchers.as_array().is_none_or(|list| !list.is_empty()));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    settings
}

/// Whether a local managed file holds nothing the store would want: for `settings.json`, an
/// empty object once the engine's hooks are removed.
/// Whether a managed file holds nothing but this machine's own hooks — nothing worth a copy.
#[must_use]
pub fn nothing_to_share(path: &Path, bytes: &[u8]) -> bool {
    if !is_settings(path) {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(bytes)
        .map(without_engine_hooks)
        .is_ok_and(|shared| shared.as_object().is_some_and(serde_json::Map::is_empty))
}

/// The store's own files: what every clone needs before it holds a single transcript. A
/// `.gitattributes` outside any commit never reaches the other machine, and a clone without it
/// merges transcripts without the drivers — conflict markers inside.
fn scaffolding_paths(machine_id: &str) -> Vec<String> {
    let mut paths = vec![".gitattributes".to_owned()];
    paths.extend(
        store_dirs(machine_id)
            .into_iter()
            .map(|dir| format!("{dir}/.keep")),
    );
    paths.extend(MANAGED_FILES.iter().map(|name| format!("config/{name}")));
    paths
}

/// Whether every scaffolding file that exists is committed and unchanged.
fn scaffolding_state(store: &Path, machine_id: &str) -> State {
    if !store.join(".git").exists() {
        return State::Missing;
    }
    let existing: Vec<String> = scaffolding_paths(machine_id)
        .into_iter()
        .filter(|path| store.join(path).exists())
        .collect();
    if existing.is_empty() {
        return State::Missing;
    }
    let mut args = vec!["status", "--porcelain", "--"];
    args.extend(existing.iter().map(String::as_str));
    match crate::git::run_with_timeout(
        crate::git::command(store, &args),
        std::time::Duration::from_mins(1),
    ) {
        Ok(Some(output)) if output.trim().is_empty() => State::Satisfied,
        Ok(Some(_)) => State::Missing,
        Ok(None) => State::Unknown {
            reason: "git could not report the status of the store".to_owned(),
        },
        Err(reason) => State::Unknown { reason },
    }
}

/// The identity a binary is signed by, as `codesign` reports it, or `None` when it is unsigned
/// or ad-hoc signed.
///
/// `codesign -dvv` writes its report to stderr, and says `Signature=adhoc` for a binary that has
/// no identity of its own — which is exactly the case this whole step exists for: an ad-hoc
/// signature changes with every rebuild, and every rebuild is a new application as far as the
/// permission database is concerned.
#[must_use]
pub fn signing_authority(report: &str) -> Option<String> {
    // The first `Authority=` and not the last: a real certificate prints its whole chain, signer
    // first, and the issuers below it are not who signed this binary. An ad-hoc signature prints
    // no Authority at all (checked against `codesign -dvv` on this machine), which is exactly
    // what makes it useless here — its identity changes with every rebuild.
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix("Authority="))
        .map(|authority| authority.trim().to_owned())
}

/// Whether the installed binary already carries the identity the owner asked for.
fn signature_state(binary: &Path, wanted: &str) -> State {
    if !binary.exists() {
        return State::Missing;
    }
    match read_signature(binary) {
        Err(reason) => State::Unknown { reason },
        Ok(None) => State::Missing,
        Ok(Some(found)) if found == wanted => State::Satisfied,
        Ok(Some(found)) => State::Conflict { found },
    }
}

/// What the signing step has to do, given whether the binary is about to be replaced.
///
/// The whole plan is computed before any of it is applied, so the signature read off the disk
/// answers about a binary that is about to stop existing. A replaced binary is always unsigned
/// afterwards — the copy does not carry the signature over — so the step is owed regardless of
/// what the old file said, including when the old file carried somebody else's signature: that
/// conflict goes away with the file.
#[must_use]
pub fn signing_state(binary_will_be_replaced: bool, found: State) -> State {
    if binary_will_be_replaced {
        State::Missing
    } else {
        found
    }
}

/// Asks `codesign` who signed a binary.
fn read_signature(binary: &Path) -> Result<Option<String>, String> {
    let output = std::process::Command::new("codesign")
        .args(["-dvv", "--"])
        .arg(binary)
        .output()
        .map_err(|error| format!("codesign could not be run: {error}"))?;
    if !output.status.success() {
        // An unsigned binary makes codesign fail; that is an answer, not a failure to answer.
        return Ok(None);
    }
    Ok(signing_authority(&String::from_utf8_lossy(&output.stderr)))
}

/// Signs the installed binary with the owner's identity.
///
/// # Errors
///
/// What `codesign` said. A missing identity is the usual cause, and the message names it.
fn sign_binary(binary: &Path, identity: &str) -> Result<(), String> {
    let output = std::process::Command::new("codesign")
        .args(["--force", "--sign", identity, "--"])
        .arg(binary)
        .output()
        .map_err(|error| format!("codesign could not be run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "codesign refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // Signing rewrites the file, so the same rule as the copy applies: prove it still runs.
    binary_runs(binary)
}
