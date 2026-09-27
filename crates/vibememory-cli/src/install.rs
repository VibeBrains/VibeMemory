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

use vibememory_core::naming::PathSyntax;

use crate::config::Config;

/// Git settings the store repository must carry, with the reason each one exists.
pub const GIT_SETTINGS: &[(&str, &str, &str)] = &[
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
///
/// `-text` guards the **working tree**, not the merge drivers: measured on git 2.50.1, the temp
/// files a driver is handed carry index form and never a carriage return, even under
/// `text eol=crlf`. What a conversion would reach is the checked-out file the engine reads
/// directly — see `docs/knowledge/git/lineEndings.md`.
pub const GITATTRIBUTES: &str =
    "* -text\n* merge=vibememory-keepboth\n**/*.jsonl merge=vibememory-jsonl\n";

/// The merge drivers git must know about: the `merge.<name>.driver` key, and which driver of the
/// binary it runs. The command itself belongs to the machine — see [`merge_driver_command`].
const MERGE_DRIVERS: &[(&str, &str)] = &[
    ("merge.vibememory-jsonl.driver", "jsonl"),
    ("merge.vibememory-keepboth.driver", "keepboth"),
];

/// `PATH` of a launchd agent that sets none, read off this machine with `launchctl print`: what the
/// tick and every git it starts actually see. Nothing of the engine is on it.
pub const LAUNCHD_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Files of the config directory the store manages as copies.
/// The files of the config directory the store keeps a copy of.
pub const MANAGED_FILES: &[&str] = &["CLAUDE.md", "settings.json"];

/// The clone's directory name inside its state directory, the same for every store.
const STORE_DIR: &str = "store";

/// The directory under `<engine>` that holds one state directory per team store.
const TEAM_STORES_DIR: &str = "stores";

/// The directory of the personal store's key under the engine directory.
const PERSONAL_DIR: &str = "personal";

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
        self.engine_dir.join(STORE_DIR)
    }

    /// The state directory of a team store: `<engine>/stores/<id>`. It holds the clone and the
    /// team's own tick state, quarantine and notes, as `<engine>` does for the personal store —
    /// so a pause or a failure of one team never counts against another or the personal store.
    /// `id` is a slug: the configuration refuses anything else.
    #[must_use]
    pub fn team_state_dir(&self, id: &str) -> PathBuf {
        self.engine_dir.join(TEAM_STORES_DIR).join(id)
    }

    /// The clone of a team store: `<engine>/stores/<id>/store`. The clone's parent is its state
    /// directory, the same relation the personal clone has to `<engine>`: every tool that finds
    /// its state as the clone's parent works for both.
    #[must_use]
    pub fn team_store(&self, id: &str) -> PathBuf {
        self.team_state_dir(id).join(STORE_DIR)
    }

    /// Where a machine connected to the personal store by a code keeps its key, host keys and
    /// record: `<engine>/personal`, beside the main store and apart from the teams'.
    #[must_use]
    pub fn personal_state_dir(&self) -> PathBuf {
        self.engine_dir.join(PERSONAL_DIR)
    }

    /// This machine's layout, from the environment.
    ///
    /// # Errors
    ///
    /// When there is no home directory and no variable naming the directory instead. Never a
    /// relative path in its place: an absent home used to become an empty string, and the engine
    /// then lived in `.vibememory` under whatever directory it was started in — on Windows, where
    /// the scheduler sets no `HOME`, that is a project folder.
    pub fn from_environment() -> Result<Self, String> {
        let home = home_dir();
        Ok(Self {
            config_dir: resolve_dir(
                std::env::var_os(CONFIG_DIR_VAR),
                home.as_deref(),
                ".claude",
                CONFIG_DIR_VAR,
            )?,
            engine_dir: engine_dir_from_environment()?,
        })
    }

    /// [`Layout::from_environment`] with the environment passed in, so the rule can be checked
    /// without one.
    ///
    /// # Errors
    ///
    /// As [`Layout::from_environment`].
    pub fn resolve(
        home: Option<&Path>,
        config_dir: Option<std::ffi::OsString>,
        engine_dir: Option<std::ffi::OsString>,
    ) -> Result<Self, String> {
        Ok(Self {
            config_dir: resolve_dir(config_dir, home, ".claude", CONFIG_DIR_VAR)?,
            engine_dir: resolve_dir(engine_dir, home, ".vibememory", ENGINE_DIR_VAR)?,
        })
    }
}

/// Names the engine directory. Hooks and merge drivers carry it, see [`hook_command`].
pub const ENGINE_DIR_VAR: &str = "VIBEMEMORY_DIR";
/// Names the configuration directory of Claude Code.
pub const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// The home directory of whoever runs the engine.
///
/// The standard library's answer rather than `HOME`: on Windows `HOME` exists only inside Git
/// Bash, so the hooks would see it and the scheduled tick would not, while the profile directory
/// is what the system itself answers. Read once, here — it used to be read in five places, each
/// with its own idea of what an absent value means.
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|home| !home.as_os_str().is_empty())
}

/// The engine directory alone: what the MCP server needs, without also requiring a config
/// directory it never reads.
///
/// # Errors
///
/// As [`Layout::from_environment`].
pub fn engine_dir_from_environment() -> Result<PathBuf, String> {
    resolve_dir(
        std::env::var_os(ENGINE_DIR_VAR),
        home_dir().as_deref(),
        ".vibememory",
        ENGINE_DIR_VAR,
    )
}

/// A directory named by a variable, else under the home. An empty variable counts as unset, as it
/// does for the shell, and no home is an error — never a relative path.
fn resolve_dir(
    given: Option<std::ffi::OsString>,
    home: Option<&Path>,
    under_home: &str,
    variable: &str,
) -> Result<PathBuf, String> {
    match (given.filter(|value| !value.is_empty()), home) {
        (Some(value), _) => Ok(PathBuf::from(value)),
        (None, Some(home)) => Ok(home.join(under_home)),
        (None, None) => Err(format!("no home directory and no {variable}")),
    }
}

/// One thing that must be true about this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A git setting of a store repository.
    GitSetting {
        /// The team whose clone it is set in; `None` for the personal store. A team's clone needs
        /// the merge drivers as much as the personal one: without them two members writing memory
        /// between ticks would conflict on every run.
        team: Option<String>,
        /// Key, e.g. `core.autocrlf`.
        key: &'static str,
        /// The value it must have. Owned, because a merge driver's value names this machine's
        /// binary.
        value: String,
    },
    /// The store's `.gitattributes`, which decides how merges are driven.
    Gitattributes,
    /// A directory a store must hold.
    StoreDir {
        /// The team whose clone holds it; `None` for the personal store.
        team: Option<String>,
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
    /// The scheduled tick: a `LaunchAgent` on macOS, a Task Scheduler task on Windows.
    Schedule,
    /// The engine's hooks in `settings.json`.
    Hooks,
    /// Git Bash, on Windows: Claude Code runs every shell-form hook with it and, without it, sends
    /// them to PowerShell — where the engine's hook lines, POSIX shell, cannot run.
    GitBash,
    /// curl, which `connect` trades a claim code with: the machine's own, like git and ssh — there
    /// is no TLS in this binary.
    Curl,
    /// No deletions held back by the tick's cap and waiting for a person.
    DeletionsHeld,
    /// `~/.vibememory/bin` on the user's lasting `PATH`, so `vibememory` answers by name in a new
    /// terminal wherever it was installed from.
    OnPath,
    /// No flag in the `env` of `settings.json` that switches the prompt cache off or shortens it.
    /// Such a flag travels to every machine with the managed copy, and costs a share of the
    /// usage limits on each of them.
    PromptCacheEnv,
    /// A copy of this binary under the engine directory, so hooks and the scheduler survive a
    /// rebuild or a `cargo clean` of wherever it was built.
    Binary,
    /// The merge drivers on record start from git's own shell, in the environment the tick gives
    /// git. Configured and installed is not runnable: the driver was once written by bare name,
    /// and git could not find it from the tick's `PATH`.
    MergeDriverRuns,
    /// The MCP server beside it, placed and signed the same way.
    ///
    /// It is installed rather than copied by hand for one measured reason: writing over a binary
    /// that has already been executed makes macOS kill it with SIGKILL and no message, while
    /// `codesign --verify` still calls the file valid. The atomic rename here sidesteps that; a
    /// `cp` in a manual does not.
    McpBinary,
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
            Self::GitSetting { team, key, value } => match team {
                None => format!("git config {key}={value}"),
                Some(team) => format!("git config {key}={value} in team {team}'s store"),
            },
            Self::Gitattributes => ".gitattributes of the store".to_owned(),
            Self::StoreDir { team, relative } => match team {
                None => format!("directory {relative}"),
                Some(team) => format!("directory {relative} in team {team}'s store"),
            },
            Self::ManagedCopy { name } => format!("managed copy of {name}"),
            Self::SkillsLink => "skills link".to_owned(),
            Self::Schedule => "scheduled tick".to_owned(),
            Self::Hooks => "hooks in settings.json".to_owned(),
            Self::GitBash => "Git Bash for the hooks".to_owned(),
            Self::Curl => "curl for connect".to_owned(),
            Self::OnPath => "vibememory on PATH in a new terminal".to_owned(),
            Self::PromptCacheEnv => "prompt cache not switched off in settings.json".to_owned(),
            Self::DeletionsHeld => "no deletions waiting for a decision".to_owned(),
            Self::ScaffoldCommitted { .. } => "store scaffolding committed".to_owned(),
            Self::Binary => "engine binary in place".to_owned(),
            Self::MergeDriverRuns => "merge drivers start from git's shell".to_owned(),
            Self::McpBinary => "MCP server binary in place".to_owned(),
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

    push_git_settings(layout, &store, None, &mut actions);
    actions.push(Action {
        step: Step::Gitattributes,
        state: file_state(&store.join(".gitattributes"), GITATTRIBUTES),
    });
    for relative in store_dirs(&config.machine_id) {
        let state = dir_state(&store.join(&relative));
        actions.push(Action {
            step: Step::StoreDir {
                team: None,
                relative,
            },
            state,
        });
    }
    push_team_stores(layout, &mut actions);
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
    push_mcp_step(layout, &mut actions);
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
    // After the binary and its signature: the proof is about the file that will actually run.
    actions.push(Action {
        step: Step::MergeDriverRuns,
        state: merge_driver_runs_state(layout),
    });
    actions.push(Action {
        step: Step::PromptCacheEnv,
        state: prompt_cache_env_state(layout),
    });
    actions.push(Action {
        step: Step::DeletionsHeld,
        state: deletions_held_state(layout),
    });
    if cfg!(windows) {
        actions.push(Action {
            step: Step::GitBash,
            state: git_bash_state(),
        });
    }
    actions.push(Action {
        step: Step::Curl,
        state: curl_state(),
    });
    actions.push(on_path_action(layout));
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
    push_schedule_step(layout, &mut actions);
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
    use crate::managed::{ManagedState, Verdict, verdict, withheld_reason};

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
    // A copy that holds a token never travels, and the person has to act: a conflict, not work
    // `install` could do.
    if local
        .as_deref()
        .is_some_and(vibememory_core::token::holds_token)
    {
        return State::Conflict {
            found: withheld_reason(name),
        };
    }
    let local_hash = local.as_deref().map(crate::sha256::hex);
    let store_hash = store.as_deref().map(crate::sha256::hex);
    let state = ManagedState::read(&layout.engine_dir);
    let base = state.synced.get(name).map(String::as_str);
    match verdict(local_hash.as_deref(), store_hash.as_deref(), base) {
        // Neither side has it, or nothing worth sharing: a resting state, not work left undone.
        // Calling it missing would make `install` perform the same no-op for ever.
        Verdict::Nothing | Verdict::Same => State::Satisfied,
        Verdict::Pull | Verdict::Push => State::Missing,
        Verdict::Withheld => State::Conflict {
            found: withheld_reason(name),
        },
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
fn commit_scaffolding(layout: &Layout, store: &Path, paths: &[String]) -> Result<(), String> {
    let timeout = std::time::Duration::from_mins(1);
    let existing: Vec<String> = paths
        .iter()
        .filter(|path| store.join(path).exists())
        .cloned()
        .collect();
    // Screened like every commit of the engine: a managed copy may hold a token no shape tells
    let staged = crate::held::stage(
        &layout.engine_dir,
        store,
        &existing,
        &crate::clock::now(),
        timeout,
    )?;
    if staged.is_empty() {
        return Ok(());
    }
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

/// The clone a step is about: a team's, or the personal store.
fn store_of(layout: &Layout, team: Option<&str>) -> PathBuf {
    team.map_or_else(|| layout.store(), |team| layout.team_store(team))
}

/// Makes one step true.
fn perform(layout: &Layout, step: &Step) -> Result<(), String> {
    let store = layout.store();
    match step {
        Step::GitSetting { team, key, value } => {
            set_git_setting(&store_of(layout, team.as_deref()), key, value)
        }
        Step::Gitattributes => write_new(&store.join(".gitattributes"), GITATTRIBUTES.as_bytes()),
        Step::StoreDir { team, relative } => {
            let path = store_of(layout, team.as_deref()).join(relative);
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            // A directory with nothing in it is not carried by git, and the CLI's own sweeper
            // removes empty directories under the config root.
            write_new(&path.join(".keep"), b"")
        }
        Step::ManagedCopy { name } => reconcile_managed(layout, &store, name),
        // Nothing to apply: a flag that switches the cache off is the owner's to remove, and the
        // plan never reports this step as missing — only satisfied or in conflict. The same goes
        // for a held deletion: releasing it is a decision, not a repair. And Git for Windows is
        // the owner's to install, and so is curl.
        Step::PromptCacheEnv | Step::DeletionsHeld | Step::GitBash | Step::Curl => Ok(()),
        // Missing only while something it proves is about to be put in place by an earlier step of
        // this same run. By now it has been, and the proof is the run itself.
        Step::MergeDriverRuns => probe_merge_drivers(layout),
        Step::Schedule => install_schedule(layout),
        Step::Binary => install_binary(layout),
        Step::OnPath => crate::user_path::put_on_path(&home_of(layout), &bin_dir(layout)),
        Step::McpBinary => match mcp_source() {
            Some(source) => install_named(layout, MCP_BINARY, &source),
            None => Ok(()),
        },
        Step::BinarySigned { identity } => {
            sign_binary(&installed_binary(layout), identity)?;
            // The server is launched by the client, not by us, and an unsigned copy beside a
            // signed engine would ask the owner for permission all over again.
            let mcp = installed_named(layout, MCP_BINARY);
            if mcp.exists() {
                sign_binary(&mcp, identity)?;
            }
            Ok(())
        }
        Step::Hooks => write_hooks(layout),
        Step::ScaffoldCommitted { machine_id } => {
            commit_scaffolding(layout, &store, &scaffolding_paths(machine_id))
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

/// Creates a directory link, making sure its target exists first: a link to nothing is a link the CLI
/// will replace with a real directory.
fn make_link(target: &Path, link: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
    write_new(&target.join(".keep"), b"")?;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::dir_link::create(target, link)
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

/// Where the scheduler's description of the tick lives. On macOS the `LaunchAgent` — under the
/// engine directory when that has been redirected, so a test never writes into the real
/// `~/Library/LaunchAgents`. On Windows the task file `install` hands the Task Scheduler, which
/// keeps its own copy once registered; this one is what `doctor` compares.
#[must_use]
pub fn schedule_path(layout: &Layout) -> PathBuf {
    if cfg!(windows) {
        return layout.engine_dir.join(SCHEDULED_TASK_FILE);
    }
    if !cfg!(target_os = "macos") {
        return systemd_dir(layout).join(format!("{SYSTEMD_UNIT}.timer"));
    }
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
    let home = home_dir()?;
    is_real_engine(layout).then(|| home.join("Library").join("LaunchAgents"))
}

/// Whether this is the machine's own engine, the one under the home directory — not a test, not a
/// dry run in a scratch directory. Only that one is handed to the system's scheduler.
fn is_real_engine(layout: &Layout) -> bool {
    home_dir().is_some_and(|home| layout.engine_dir == home.join(".vibememory"))
}

/// Writes the scheduler's description of the tick and, for the real engine, hands it over: the
/// `LaunchAgent` to launchd on macOS, the task to the Task Scheduler on Windows.
fn install_schedule(layout: &Layout) -> Result<(), String> {
    let path = schedule_path(layout);
    if cfg!(windows) {
        write_new(&path, &utf16_with_bom(&scheduled_task(layout)))?;
        if is_real_engine(layout) {
            register_scheduled_task(&path)?;
        }
        return Ok(());
    }
    if !cfg!(target_os = "macos") {
        write_new(
            &systemd_service_path(layout),
            systemd_service(layout).as_bytes(),
        )?;
        write_new(&path, systemd_timer().as_bytes())?;
        if is_real_engine(layout) {
            start_linux_schedule(layout)?;
        }
        return Ok(());
    }
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
    installed_named(layout, ENGINE_BINARY)
}

/// The engine itself.
pub const ENGINE_BINARY: &str = "vibememory";
/// The MCP server, installed beside it and treated exactly the same way.
pub const MCP_BINARY: &str = "vibememory-mcp";

/// Where a binary of this engine lives once installed.
#[must_use]
pub fn installed_named(layout: &Layout, name: &str) -> PathBuf {
    // `.exe` on Windows: without it `CreateProcess` looks for `vibememory.exe`, finds nothing, and
    // every hook fails. Nothing on macOS.
    layout
        .engine_dir
        .join("bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// The note beside the installed binary saying which build it was copied from.
///
/// Needed because the installed file stops being byte-identical to its source the moment it is
/// signed: signing rewrites the binary. Without the note every `install` and every `doctor` on a
/// signing machine finds a difference, replaces the binary under the running hooks and signs it
/// again — for ever, and for nothing.
fn source_note(layout: &Layout) -> PathBuf {
    source_note_named(layout, ENGINE_BINARY)
}

/// The same note, for any binary this engine installs.
fn source_note_named(layout: &Layout, name: &str) -> PathBuf {
    layout.engine_dir.join("bin").join(format!("{name}.source"))
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

/// Adds the git settings of the store: the fixed ones, then the merge drivers, whose lines name
/// this machine's binary.
fn push_git_settings(layout: &Layout, store: &Path, team: Option<&str>, actions: &mut Vec<Action>) {
    for (key, value, _why) in GIT_SETTINGS {
        actions.push(Action {
            step: Step::GitSetting {
                team: team.map(str::to_owned),
                key,
                value: (*value).to_owned(),
            },
            state: git_setting_state(store, key, value),
        });
    }
    for (key, driver) in MERGE_DRIVERS {
        let value = merge_driver_command(layout, driver);
        let state = merge_driver_state(store, key, driver, &value);
        actions.push(Action {
            step: Step::GitSetting {
                team: team.map(str::to_owned),
                key,
                value,
            },
            state,
        });
    }
}

/// What a connected team's clone needs: the git settings and merge drivers of the personal store,
/// and this machine's directory — the one of `machines/` the host lets it push. Nothing of
/// `config/`, and no `.gitattributes`: that file is the host's, and the clone gets it by fetch.
/// `connect` applies it right after the clone, so the first merge already has its drivers.
#[must_use]
pub fn plan_team(layout: &Layout, team: &str) -> Vec<Action> {
    let mut actions = Vec::new();
    let Ok(record) = crate::team_connect::read_record(layout, team) else {
        return actions;
    };
    let clone = layout.team_store(team);
    push_git_settings(layout, &clone, Some(team), &mut actions);
    let relative = format!("machines/{}", record.store_name);
    let state = dir_state(&clone.join(&relative));
    actions.push(Action {
        step: Step::StoreDir {
            team: Some(team.to_owned()),
            relative,
        },
        state,
    });
    actions
}

/// Every connected team's plan, for `install` and `doctor`.
fn push_team_stores(layout: &Layout, actions: &mut Vec<Action>) {
    for team in crate::team_connect::connected_teams(layout) {
        actions.extend(plan_team(layout, &team));
    }
}

/// Adds the scheduled tick where there is a scheduler this build knows about: launchd's agent on
/// macOS, the Task Scheduler's task on Windows — each compared as the bytes `install` writes.
fn push_schedule_step(layout: &Layout, actions: &mut Vec<Action>) {
    let state = if cfg!(target_os = "macos") {
        file_state(&schedule_path(layout), &launch_agent(layout))
    } else if cfg!(windows) {
        bytes_state(
            &schedule_path(layout),
            &utf16_with_bom(&scheduled_task(layout)),
        )
    } else {
        // Linux: the timer and its service, both as written; a machine without a user systemd runs
        // the tick from cron, which the step reports as done once the line is there
        match (
            file_state(&schedule_path(layout), &systemd_timer()),
            file_state(&systemd_service_path(layout), &systemd_service(layout)),
        ) {
            (State::Satisfied, State::Satisfied) => State::Satisfied,
            (State::Satisfied, other) | (other, _) => other,
        }
    };
    actions.push(Action {
        step: Step::Schedule,
        state,
    });
}

/// Adds the MCP server step, and only when this machine has anything to do about it.
///
/// A machine that never built the server does not need it, and a step it can do nothing about
/// would make `doctor` fail for ever on a perfectly healthy install.
fn push_mcp_step(layout: &Layout, actions: &mut Vec<Action>) {
    let source = mcp_source();
    // With a team connected the step is never skipped: a local server in the team's project is
    // how its memory is written here, and without the binary it simply does not start
    if source.is_none()
        && !installed_named(layout, MCP_BINARY).exists()
        && crate::team_connect::connected_teams(layout).is_empty()
    {
        return;
    }
    actions.push(Action {
        step: Step::McpBinary,
        state: named_binary_state(layout, MCP_BINARY, source.as_deref()),
    });
}

/// The MCP server built beside whatever is running, when there is one.
///
/// `install` is normally run from a fresh build, and the server is that build's sibling. Running
/// the installed engine finds the installed server, and copying a file onto itself is refused
/// below rather than attempted.
fn mcp_source() -> Option<PathBuf> {
    let running = std::env::current_exe().ok()?;
    let candidate = running.with_file_name(format!("{MCP_BINARY}{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// Whether the installed copy of `name` came from this `source`.
///
/// Public so it can be gated with an explicit source: deriving it from `current_exe` would make
/// the test write a file next to the shared test harness, which every other test in the binary
/// then sees.
#[must_use]
pub fn named_binary_state(layout: &Layout, name: &str, source: Option<&Path>) -> State {
    let installed = installed_named(layout, name);
    let Some(source) = source else {
        // Nothing to install from. What is already there stays; saying "missing" would make
        // `doctor` fail on a machine that simply has no build beside it.
        return State::Satisfied;
    };
    if source == installed {
        return State::Satisfied; // the installed engine found itself
    }
    let Ok(bytes) = std::fs::read(source) else {
        return State::Unknown {
            reason: format!("{} could not be read", source.display()),
        };
    };
    if !installed.exists() {
        return State::Missing;
    }
    let wanted = crate::sha256::hex(&bytes);
    match std::fs::read_to_string(source_note_named(layout, name)) {
        Ok(noted) if noted.trim() == wanted => State::Satisfied,
        _ => State::Missing,
    }
}

/// Places a binary the way the engine places itself: atomically, then proves it runs.
///
/// # Errors
///
/// The text of what went wrong.
pub fn install_named(layout: &Layout, name: &str, source: &Path) -> Result<(), String> {
    let target = installed_named(layout, name);
    if source == target {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = target.with_extension("new");
    std::fs::copy(source, &temporary).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    // Rename, never write in place. Measured 2026-09-10: overwriting a binary that had already
    // been executed made the kernel kill every later run with SIGKILL and nothing on stderr,
    // while `codesign --verify` kept calling the file valid on disk. A rename makes a new inode
    // and the problem cannot arise.
    std::fs::rename(&temporary, &target).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
    std::fs::write(source_note_named(layout, name), crate::sha256::hex(&bytes))
        .map_err(|e| e.to_string())?;
    binary_runs(&target)
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
         \t<key>StartInterval</key><integer>{TICK_INTERVAL_SECONDS}</integer>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The name systemd knows the tick's service and timer by.
pub const SYSTEMD_UNIT: &str = "vibememory-tick";

/// The marker at the end of the cron line of a machine without a user systemd.
const CRON_MARKER: &str = "# vibememory tick";

/// `~/.config/systemd/user` for the real engine; a redirected one keeps its units beside itself and
/// never touches systemd.
fn systemd_dir(layout: &Layout) -> PathBuf {
    match home_dir() {
        Some(home) if is_real_engine(layout) => home.join(".config").join("systemd").join("user"),
        _ => layout.engine_dir.join("systemd"),
    }
}

fn systemd_service_path(layout: &Layout) -> PathBuf {
    systemd_dir(layout).join(format!("{SYSTEMD_UNIT}.service"))
}

/// The service one tick runs as: the engine's own binary, its directory named as launchd names it.
#[must_use]
pub fn systemd_service(layout: &Layout) -> String {
    format!(
        "[Unit]\nDescription=VibeMemory: keeps this machine's store in step with the others\n\n\
         [Service]\nType=oneshot\nEnvironment=\"VIBEMEMORY_DIR={}\"\nExecStart=\"{}\" tick\n",
        layout.engine_dir.display(),
        installed_binary(layout).display()
    )
}

/// The timer: at login and every two minutes after, as launchd and the Task Scheduler run it.
#[must_use]
pub fn systemd_timer() -> String {
    format!(
        "[Unit]\nDescription=VibeMemory tick every {} minutes\n\n\
         [Timer]\nOnStartupSec=30\nOnUnitActiveSec={}\nUnit={SYSTEMD_UNIT}.service\n\n\
         [Install]\nWantedBy=timers.target\n",
        TICK_INTERVAL_SECONDS / 60,
        TICK_INTERVAL_SECONDS
    )
}

/// The cron line of a machine without a user systemd.
#[must_use]
pub fn cron_line(layout: &Layout) -> String {
    format!(
        "*/{} * * * * VIBEMEMORY_DIR='{}' '{}' tick {CRON_MARKER}",
        TICK_INTERVAL_SECONDS / 60,
        layout.engine_dir.display(),
        installed_binary(layout).display()
    )
}

/// Starts the timer, or — where the user has no systemd of their own — puts the tick into cron once.
fn start_linux_schedule(layout: &Layout) -> Result<(), String> {
    let systemd = |args: &[&str]| {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if systemd(&["daemon-reload"])
        && systemd(&["enable", "--now", &format!("{SYSTEMD_UNIT}.timer")])
    {
        return Ok(());
    }
    let listed = std::process::Command::new("crontab")
        .arg("-l")
        .output()
        .map_err(|error| format!("neither a user systemd nor crontab: {error}"))?;
    let current = String::from_utf8_lossy(&listed.stdout).into_owned();
    if current.lines().any(|line| line.ends_with(CRON_MARKER)) {
        return Ok(());
    }
    let next = format!("{}{}\n", current, cron_line(layout));
    let mut child = std::process::Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    std::io::Write::write_all(
        child.stdin.as_mut().ok_or("crontab took no input")?,
        next.as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("crontab refused the tick's line".to_owned())
    }
}

/// How often the tick runs, for both schedulers: launchd's `StartInterval`, the Task Scheduler's
/// repetition interval.
const TICK_INTERVAL_SECONDS: u32 = 120;

/// The name the Task Scheduler knows the tick by, in a folder of its own.
pub const SCHEDULED_TASK_NAME: &str = "VibeMemory\\tick";

/// The console host of Windows 10 and later, run without a window: the Task Scheduler expands the
/// variable.
const HEADLESS_CONSOLE: &str = r"%windir%\System32\conhost.exe";

/// The task file `install` writes under the engine directory and hands the Task Scheduler.
const SCHEDULED_TASK_FILE: &str = "tick-task.xml";

/// The Task Scheduler task: after logon every two minutes, and at every unlock.
///
/// The triggers are the ones `docs/spec/architecture.md` §5 named for Windows. `InteractiveToken`
/// runs it in the owner's session and with the owner's environment — git needs the ssh key and the
/// `PATH` of that session. No second copy while one runs: the tick has its own lock, and a stack of
/// waiting ticks after a sleep is exactly what launchd is told to avoid on macOS.
///
/// The engine is a console program, and so is every git it starts: run by the Task Scheduler in the
/// owner's session, each would open a console window every two minutes. `conhost --headless` gives
/// the whole run one console nobody sees, and the processes it starts share it instead of opening
/// their own.
#[must_use]
pub fn scheduled_task(layout: &Layout) -> String {
    let interval = format!(
        "        <Interval>PT{}M</Interval>",
        TICK_INTERVAL_SECONDS / 60
    );
    let command = format!("      <Command>{HEADLESS_CONSOLE}</Command>");
    let arguments = format!(
        "      <Arguments>--headless &quot;{}&quot; tick</Arguments>",
        xml_text(&installed_binary(layout).to_string_lossy())
    );
    let lines: &[&str] = &[
        r#"<?xml version="1.0" encoding="UTF-16"?>"#,
        r#"<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">"#,
        "  <RegistrationInfo>",
        "    <Description>VibeMemory: keeps this machine's store in step with the others.</Description>",
        "  </RegistrationInfo>",
        "  <Triggers>",
        "    <LogonTrigger>",
        "      <Enabled>true</Enabled>",
        "      <Repetition>",
        interval.as_str(),
        "        <StopAtDurationEnd>false</StopAtDurationEnd>",
        "      </Repetition>",
        "    </LogonTrigger>",
        "    <SessionStateChangeTrigger>",
        "      <Enabled>true</Enabled>",
        "      <StateChange>SessionUnlock</StateChange>",
        "    </SessionStateChangeTrigger>",
        "  </Triggers>",
        "  <Principals>",
        r#"    <Principal id="Author">"#,
        "      <LogonType>InteractiveToken</LogonType>",
        "      <RunLevel>LeastPrivilege</RunLevel>",
        "    </Principal>",
        "  </Principals>",
        "  <Settings>",
        "    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
        "    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
        "    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
        "    <StartWhenAvailable>true</StartWhenAvailable>",
        "    <ExecutionTimeLimit>PT10M</ExecutionTimeLimit>",
        "  </Settings>",
        r#"  <Actions Context="Author">"#,
        "    <Exec>",
        command.as_str(),
        arguments.as_str(),
        "    </Exec>",
        "  </Actions>",
        "</Task>",
    ];
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// Text as XML element content: a user or folder name may hold `&`, and a bare one makes the task
/// file unreadable to the Task Scheduler.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Text the way the Task Scheduler writes its own files: UTF-16LE behind a byte-order mark. The
/// declaration says UTF-16, and a file that says one encoding and is another gets refused.
#[must_use]
pub fn utf16_with_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Hands the task file to the Task Scheduler. `/F` replaces a task of the same name: the file is
/// what `install` owns, and the registered copy follows it. Unlike launchd's "already loaded", a
/// refusal here is a real one and is reported.
fn register_scheduled_task(xml: &Path) -> Result<(), String> {
    let output = std::process::Command::new("schtasks")
        .args(["/Create", "/TN", SCHEDULED_TASK_NAME, "/XML"])
        .arg(xml)
        .arg("/F")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("schtasks could not be run: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "schtasks refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// A file that must hold exactly these bytes — for the task file, which is not UTF-8.
fn bytes_state(path: &Path, expected: &[u8]) -> State {
    match std::fs::read(path) {
        Ok(found) if found == expected => State::Satisfied,
        Ok(_) => State::Conflict {
            found: format!("{} holds something else", path.display()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::Missing,
        Err(error) => State::Unknown {
            reason: error.to_string(),
        },
    }
}

/// The variable Claude Code reads for the location of Git Bash.
const GIT_BASH_VARIABLE: &str = "CLAUDE_CODE_GIT_BASH_PATH";

/// File names Claude Code accepts in [`GIT_BASH_VARIABLE`].
const GIT_BASH_NAMES: &[&str] = &["bash.exe", "sh.exe", "bash", "sh"];

/// The two standard installs of Git for Windows, in the order Claude Code tries them.
const GIT_BASH_STANDARD: &[&str] = &[
    r"C:\Program Files\Git\bin\bash.exe",
    r"C:\Program Files (x86)\Git\bin\bash.exe",
];

/// Where Claude Code finds Git Bash on Windows, step for step as its own lookup does it — read out
/// of the CLI 2.1.232 rather than guessed: [`GIT_BASH_VARIABLE`] when it names a bash or sh that
/// exists; then the two standard installs of Git for Windows; then `bash.exe` in the `bin` two
/// levels above the `git` on `PATH`. `None` is what Claude Code calls "Git Bash not found": its
/// shell-form hooks then go to `PowerShell`.
#[must_use]
pub fn git_bash(
    override_path: Option<&str>,
    git_on_path: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(given) = override_path {
        let name = given
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(given)
            .to_ascii_lowercase();
        if GIT_BASH_NAMES.contains(&name.as_str()) && exists(Path::new(given)) {
            return Some(PathBuf::from(given));
        }
    }
    if let Some(standard) = GIT_BASH_STANDARD
        .iter()
        .map(PathBuf::from)
        .find(|candidate| exists(candidate))
    {
        return Some(standard);
    }
    let beside = git_on_path?
        .parent()?
        .parent()?
        .join("bin")
        .join("bash.exe");
    exists(&beside).then_some(beside)
}

/// Git Bash on this machine, as Claude Code will look for it.
fn git_bash_state() -> State {
    let given = std::env::var(GIT_BASH_VARIABLE).ok();
    let git = on_path("git");
    match git_bash(given.as_deref(), git.as_deref(), Path::is_file) {
        Some(_) => State::Satisfied,
        None => State::Conflict {
            found: format!(
                "no Git Bash: Claude Code would run the hooks in PowerShell, where they cannot \
                 run — install Git for Windows or set {GIT_BASH_VARIABLE}"
            ),
        },
    }
}

/// curl on this machine: on macOS always, on Windows 10 and later in `System32`, and in Git for
/// Windows.
fn curl_state() -> State {
    if on_path("curl").is_some() {
        State::Satisfied
    } else {
        State::Conflict {
            found: "no curl on PATH: connect cannot reach the cabinet — install curl".to_owned(),
        }
    }
}

/// The plan of a machine the engine does not run on: the two binaries in `~/.vibememory/bin` and
/// curl. A member of a `memory` team needs nothing more — no store, no hooks, no schedule — and
/// the archive's `./vibememory install` must not refuse such a machine for lacking a
/// `config.json` it will never have.
#[must_use]
pub fn plan_binaries(layout: &Layout) -> Vec<Action> {
    let mut actions = vec![Action {
        step: Step::Binary,
        state: binary_state(layout),
    }];
    push_mcp_step(layout, &mut actions);
    actions.push(Action {
        step: Step::Curl,
        state: curl_state(),
    });
    actions.push(on_path_action(layout));
    actions
}

/// Where the installed binaries lie.
fn bin_dir(layout: &Layout) -> PathBuf {
    installed_binary(layout)
        .parent()
        .map_or_else(|| layout.engine_dir.join("bin"), Path::to_path_buf)
}

/// The home the engine lives in: the directory that holds `~/.vibememory`.
fn home_of(layout: &Layout) -> PathBuf {
    layout
        .engine_dir
        .parent()
        .map_or_else(|| layout.engine_dir.clone(), Path::to_path_buf)
}

fn on_path_action(layout: &Layout) -> Action {
    Action {
        step: Step::OnPath,
        state: match crate::user_path::is_on_path(&home_of(layout), &bin_dir(layout)) {
            Ok(true) => State::Satisfied,
            Ok(false) => State::Missing,
            Err(reason) => State::Unknown { reason },
        },
    }
}

/// The first executable called `name` on `PATH`.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|candidate| candidate.is_file())
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
///
/// Both paths are shell words, see [`shell_word`]: the line is run by `sh` on macOS and by Git
/// Bash on Windows, and an unquoted path breaks on a space everywhere and on every backslash
/// under Git Bash.
#[must_use]
pub fn hook_command(layout: &Layout, subcommand: &str) -> String {
    format!(
        "{ENGINE_DIR_VAR}={} {} hook {subcommand}",
        shell_word(&layout.engine_dir),
        shell_word(&installed_binary(layout)),
    )
}

/// The command git runs for one merge driver on this machine.
///
/// By absolute path, never by name. Git hands the line to a shell whose `PATH` is whatever started
/// git, and nothing puts the engine's `bin` there — not the tick under launchd ([`LAUNCHD_PATH`]),
/// not a terminal. Measured 2026-09-11: the bare name gave `vibememory: command not found`, git
/// fell back to a textual conflict, and nothing noticed, because the store had never needed a
/// three-way merge — 1299 commits, not one merge. The engine directory travels with it for the
/// reason it travels with the hooks: the quarantine and the merge log belong to the engine that
/// installed the driver.
#[must_use]
pub fn merge_driver_command(layout: &Layout, driver: &str) -> String {
    format!(
        "{ENGINE_DIR_VAR}={} {}{}",
        shell_word(&layout.engine_dir),
        shell_word(&installed_binary(layout)),
        driver_arguments(driver),
    )
}

/// What follows the program in a merge driver line: `%O %A %B %P` is git's own vocabulary — base,
/// ours, theirs, and the path inside the repository. The same tail marks a line as the engine's,
/// whatever binary and engine directory an earlier install put in front of it.
fn driver_arguments(driver: &str) -> String {
    format!(" merge-driver {driver} %O %A %B %P")
}

/// Every git setting the store carries on this machine, in the order `install` checks them.
///
/// Public so a test can configure a repository exactly as `install` does. The drivers used to be
/// registered in the tests by a string of their own, and that is how the one `install` wrote went
/// on being unrunnable with every test green.
#[must_use]
pub fn git_settings(layout: &Layout) -> Vec<(&'static str, String)> {
    GIT_SETTINGS
        .iter()
        .map(|(key, value, _why)| (*key, (*value).to_owned()))
        .chain(
            MERGE_DRIVERS
                .iter()
                .map(|(key, driver)| (*key, merge_driver_command(layout, driver))),
        )
        .collect()
}

/// A path as one word for the POSIX shell that runs hooks and merge drivers: `sh` on macOS, Git
/// Bash on Windows.
#[must_use]
pub fn shell_word(path: &Path) -> String {
    shell_word_for(
        &path.to_string_lossy(),
        crate::hook::session_start::host_syntax(),
    )
}

/// [`shell_word`] for a given syntax, so the Windows form is checked on any machine.
///
/// Single quotes, because nothing is special inside them: a space in a user name is ordinary on
/// Windows and splits an unquoted word everywhere, and an embedded quote is closed, escaped and
/// reopened. Forward slashes for Windows, because Git Bash reads a backslash as an escape — an
/// unquoted `D:\Work` reaches the program as `D:Work` — while every Windows API takes `/`.
#[must_use]
pub fn shell_word_for(path: &str, syntax: PathSyntax) -> String {
    let path = match syntax {
        PathSyntax::Windows => path.replace('\\', "/"),
        PathSyntax::Posix => path.to_owned(),
    };
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// Whether a merge driver line is one this engine wrote, by an earlier install or by this one.
fn is_engine_driver(line: &str, driver: &str) -> bool {
    line.ends_with(&driver_arguments(driver))
}

/// A merge driver setting: satisfied when it is this machine's current line; missing when absent
/// or left by an earlier install, which is replaced as an earlier install's hooks are; in conflict
/// only when it is somebody else's driver.
fn merge_driver_state(store: &Path, key: &str, driver: &str, wanted: &str) -> State {
    match git_setting_state(store, key, wanted) {
        State::Conflict { found } if is_engine_driver(&found, driver) => State::Missing,
        other => other,
    }
}

/// What proves a merge driver line starts: the same line with the merge arguments replaced by
/// `--version`. `None` for a line the engine did not write — there is nothing of ours to run.
#[must_use]
pub fn driver_probe(line: &str, driver: &str) -> Option<String> {
    line.strip_suffix(&driver_arguments(driver))
        .map(|program| format!("{program} --version"))
}

/// How long git, its shell and the binary together may take to print a version.
const DRIVER_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The git alias the probe runs through.
const PROBE_ALIAS: &str = "vibememory-probe";

/// Whether the merge drivers start from git's own shell.
///
/// Missing while `install` is about to write them or the binary they name: the proof then comes at
/// the end of that same run, see [`probe_merge_drivers`]. Otherwise the drivers on record are run,
/// which is the only answer worth having — the driver used to be written by bare name, was
/// configured, installed and green in every test, and could not be started by git at all.
fn merge_driver_runs_state(layout: &Layout) -> State {
    let store = layout.store();
    if !store.join(".git").exists() || !installed_binary(layout).exists() {
        return State::Missing;
    }
    for (key, driver) in MERGE_DRIVERS {
        match merge_driver_state(&store, key, driver, &merge_driver_command(layout, driver)) {
            State::Satisfied => {}
            // Somebody else's driver: the setting reports that conflict itself, and there is no
            // command of the engine here to prove.
            other => return other,
        }
    }
    match probe_merge_drivers(layout) {
        Ok(()) => State::Satisfied,
        Err(found) => State::Conflict { found },
    }
}

/// Whether the binary installed as the engine is a copy of the program running now, when the
/// program running now is not the engine — the test harness, which `install` copies into the
/// engine's place in the suite. Asking it for `--version` proves nothing about the engine; the
/// check after a copy is skipped there for the same reason (see `install_binary`).
///
/// Decided by the note that records where the installed binary came from, not by the name of the
/// running program alone: the probe's own gate places a stand-in by hand, and that one has to run.
/// On a real machine the names agree and nothing is hashed.
fn installed_is_a_copy_of_a_harness(layout: &Layout) -> bool {
    let Ok(running) = std::env::current_exe() else {
        return false;
    };
    if running.file_name() == installed_binary(layout).file_name() {
        return false;
    }
    let Ok(noted) = std::fs::read_to_string(source_note(layout)) else {
        return false;
    };
    std::fs::read(&running).is_ok_and(|bytes| noted.trim() == crate::sha256::hex(&bytes))
}

/// Runs every merge driver line `install` writes the way git will run it, and says what the shell
/// said when one does not start.
///
/// Through a git alias rather than `sh -c`, so that the shell is git's own on every platform — Git
/// Bash's `sh` on Windows. On macOS in the tick's world: launchd gives it [`LAUNCHD_PATH`] and a
/// home, nothing else. On Windows the scheduled task runs with the user's own environment, and
/// that is what is kept.
fn probe_merge_drivers(layout: &Layout) -> Result<(), String> {
    if installed_is_a_copy_of_a_harness(layout) {
        return Ok(());
    }
    let store = layout.store();
    for (_, driver) in MERGE_DRIVERS {
        let line = merge_driver_command(layout, driver);
        let probe = driver_probe(&line, driver)
            .ok_or_else(|| format!("not a merge driver line of the engine: {line}"))?;
        let mut command = std::process::Command::new("git");
        command
            .arg("-c")
            .arg(format!("alias.{PROBE_ALIAS}=!{probe}"))
            .arg(PROBE_ALIAS)
            .current_dir(&store)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if cfg!(target_os = "macos") {
            command.env_clear().env("PATH", LAUNCHD_PATH);
            if let Some(home) = home_dir() {
                command.env("HOME", home);
            }
        }
        match crate::git::run_capturing(command, DRIVER_PROBE_TIMEOUT)? {
            Ok(version) if version.starts_with(&format!("{ENGINE_BINARY} ")) => {}
            Ok(other) => return Err(format!("{line}: answered {other:?} instead of a version")),
            Err(said) => return Err(format!("{line}: {said}")),
        }
    }
    Ok(())
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

/// Whether a hook command is one of ours. Decided by its shape, not by the binary's file name:
/// the binary is `vibememory` in an installation and something else under `cargo test`, and a
/// rule that only recognised the former would leave the latter's hooks in the store.
fn is_engine_hook(command: &str) -> bool {
    command.starts_with(&format!("{ENGINE_DIR_VAR}="))
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
