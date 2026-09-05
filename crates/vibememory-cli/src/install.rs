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
const MANAGED_FILES: &[&str] = &["CLAUDE.md", "settings.json"];

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
    /// A copy of this binary under the engine directory, so hooks and the scheduler survive a
    /// rebuild or a `cargo clean` of wherever it was built.
    Binary,
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
            Self::ScaffoldCommitted { .. } => "store scaffolding committed".to_owned(),
            Self::Binary => "engine binary in place".to_owned(),
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
            state: managed_copy_state(
                &layout.config_dir.join(name),
                &store.join("config").join(name),
            ),
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
    actions.push(Action {
        step: Step::Binary,
        state: binary_state(layout),
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
fn managed_copy_state(local: &Path, in_store: &Path) -> State {
    match (std::fs::read(local), std::fs::read(in_store)) {
        (Ok(here), Ok(there)) if same_managed_content(local, &here, &there) => State::Satisfied,
        (Ok(_), Ok(_)) => State::Conflict {
            found: "the copies differ; the tick reconciles them".to_owned(),
        },
        // Neither side has the file. Nothing to manage, and inventing an empty one would put a
        // meaningless file on every machine — so this is a resting state, not work left undone.
        // Calling it missing would make `install` perform the same no-op for ever.
        (Err(_), Err(_)) => State::Satisfied,
        // Only this machine has it, and once its own hooks are taken out nothing remains: a
        // settings file the engine itself created has nothing to share.
        (Ok(here), Err(_)) if nothing_to_share(local, &here) => State::Satisfied,
        // Exactly one side has it: the copy has to be made.
        _ => State::Missing,
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
        Step::ManagedCopy { name } => copy_managed(
            &layout.config_dir.join(name),
            &store.join("config").join(name),
        ),
        Step::Schedule => install_schedule(layout),
        Step::Binary => install_binary(layout),
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
fn copy_managed(local: &Path, in_store: &Path) -> Result<(), String> {
    if let Some(parent) = in_store.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match (local.exists(), in_store.exists()) {
        (true, false) => {
            let bytes = std::fs::read(local).map_err(|e| e.to_string())?;
            // What goes into the store is this machine's settings minus this machine's hooks:
            // a hook command names this machine's binary and engine directory, and on another
            // machine that command fails — and a failing UserPromptSubmit hook stops sessions.
            let shared = for_the_store(local, &bytes);
            std::fs::write(in_store, shared).map_err(|e| e.to_string())
        }
        (false, true) => {
            if let Some(parent) = local.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(in_store, local)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        // Neither side has the file: nothing to manage yet, and inventing an empty one would
        // put a meaningless file on every machine. Both sides present and differing is a
        // conflict the plan already refused, so apply never reaches it either.
        (false, false) | (true, true) => Ok(()),
    }
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
        Some(dir) => dir.join(SCHEDULE_LABEL).with_extension("plist"),
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

/// Whether the installed copy is this binary, byte for byte.
fn binary_state(layout: &Layout) -> State {
    let Ok(this) = std::env::current_exe().and_then(std::fs::read) else {
        return State::Unknown {
            reason: "this binary could not be read".to_owned(),
        };
    };
    match std::fs::read(installed_binary(layout)) {
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
    std::fs::rename(&temporary, &target).map_err(|e| e.to_string())
}

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
fn write_hooks(layout: &Layout) -> Result<(), String> {
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

/// Whether two copies of a managed file say the same thing. For `settings.json` that means the
/// same JSON once the engine's own hooks are removed from both — they are this machine's, never
/// the store's — and formatting does not count. For everything else it means the same bytes.
fn same_managed_content(path: &Path, here: &[u8], there: &[u8]) -> bool {
    if !is_settings(path) {
        return here == there;
    }
    match (
        serde_json::from_slice::<serde_json::Value>(here),
        serde_json::from_slice::<serde_json::Value>(there),
    ) {
        (Ok(a), Ok(b)) => without_engine_hooks(a) == without_engine_hooks(b),
        _ => here == there,
    }
}

/// The bytes of a managed file as they may enter the store.
fn for_the_store(path: &Path, bytes: &[u8]) -> Vec<u8> {
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
fn nothing_to_share(path: &Path, bytes: &[u8]) -> bool {
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
