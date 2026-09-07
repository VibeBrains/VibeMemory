//! The tick: the only thing that talks to the network, and the only thing allowed to merge.
//!
//! It runs every two minutes, so its first duty is to be harmless when it has nothing to do and
//! when something is wrong. Every step below either does its work or reports and stops; none of
//! them destroys anything, and the one step that could — merging somebody else's records into a
//! file this machine is writing — refuses unless it can prove the session is not live here.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use vibememory_core::desktop::roots::Roots;
use vibememory_core::links::LinkSource;

use crate::forget::Forgotten;
use crate::git;
use crate::hook::stop::Live;
use crate::links_file::{self, Observation};

/// How long any one git command of the tick may take. Longer than a hook's, because nobody is
/// waiting: the tick runs in the background.
const TIMEOUT: Duration = Duration::from_mins(2);
/// The branch the store lives on.
const BRANCH: &str = "main";
/// Where the store's own commits go.
const REMOTE: &str = "origin";

/// What one tick did, in the order it did it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ticked {
    /// Whether new commits were fetched.
    pub fetched: bool,
    /// Whether the fetched commits were merged.
    pub merged: bool,
    /// Sessions whose files the incoming commits touch and which are live here, so the merge was
    /// held back. Named for the log: a merge that never happens is a synchronisation that never
    /// happens, and somebody has to be able to see why.
    pub held_back: Vec<String>,
    /// Transcripts removed because a machine asked to forget them.
    pub forgotten: Vec<String>,
    /// Files under `projects/` that were deleted in the working copy and put back.
    pub restored: Vec<String>,
    /// Projects whose memory projection was rewritten from records that arrived from elsewhere.
    pub projected_memory: Vec<String>,
    /// Files of this machine's outbox (`machines/<id>/**`) committed by this tick.
    pub outbox_committed: usize,
    /// Real `projects/<enc>` directories copied into the store and replaced by links.
    pub imported_directories: Vec<String>,
    /// Files under `projects/` committed by this tick: side files, `.keep` markers, imported
    /// directories, memory — everything a session is not writing right now.
    pub project_files_committed: usize,
    /// Sessions dropped from this machine's live list because nothing has confirmed them for
    /// longer than a machine that crashed would take to come back.
    pub stale_sessions: Vec<String>,
    /// Desktop cards published to the outbox.
    pub cards_out: usize,
    /// Desktop cards written into the local Desktop store.
    pub cards_in: usize,
    /// What this machine put into its outbox for the others.
    pub published: crate::outbox::Moved,
    /// What it took from theirs.
    pub imported: crate::outbox::Moved,
    /// Links created ahead of any session that might need them.
    pub linked: Vec<String>,
    /// Encoded directories where this machine and another disagree about the store name. Never
    /// resolved automatically: one of the two is wrong, and guessing which would split a project
    /// into two stores.
    pub disagreements: Vec<String>,
    /// Whether anything was pushed.
    pub pushed: bool,
    /// What went wrong, if anything. A tick reports and returns; it never panics a machine.
    pub problems: Vec<String>,
}

/// Runs one tick over the store.
///
/// `live` is what this machine believes about its own sessions; it decides whether a merge is
/// safe, and it is read from the store rather than from a process list because a session lives
/// across machines, not inside one pid.
#[must_use]
pub struct Machine<'a> {
    /// The store clone.
    pub store: &'a Path,
    /// `~/.claude`.
    pub config_dir: &'a Path,
    /// This machine's name in the store.
    pub machine_id: &'a str,
    /// Named roots, for translating working directories.
    pub roots: &'a Roots,
    /// The naming rules, for a directory the tick has to name itself.
    pub naming: &'a vibememory_core::naming::NamingConfig,
    /// Where Desktop keeps its cards, when this machine has them.
    pub desktop_store: Option<&'a Path>,
}

/// Runs one tick over the store.
///
/// `stamp` is the moment, `heartbeat_cutoff` the stamp before which a heartbeat of this machine
/// is stale. Both come from the caller: the core has no clock and the engine keeps it that way.
#[must_use]
pub fn run(machine: &Machine<'_>, stamp: &str, heartbeat_cutoff: &str) -> Ticked {
    let &Machine {
        store,
        config_dir,
        machine_id,
        roots,
        naming,
        desktop_store,
    } = machine;
    let mut result = Ticked {
        fetched: fetch(store),
        ..Ticked::default()
    };

    let live = live_sessions(store, machine_id);
    match merge_if_safe(store, machine_id) {
        Ok(Merge::Merged) => result.merged = true,
        Ok(Merge::HeldBack { sessions }) => result.held_back = sessions,
        Ok(Merge::NothingToDo) => {}
        Err(problem) => result.problems.push(problem),
    }

    match apply_tombstones(store, stamp) {
        Ok(removed) => result.forgotten = removed,
        Err(problem) => result.problems.push(problem),
    }

    match reconcile_links(store, config_dir, machine_id, roots) {
        Ok(mut outcome) => {
            result.linked.append(&mut outcome.linked);
            result.disagreements.append(&mut outcome.disagreements);
        }
        Err(problem) => result.problems.push(problem),
    }

    match crate::outbox::publish(config_dir, store, machine_id, roots) {
        Ok(moved) => result.published = moved,
        Err(problem) => result.problems.push(problem),
    }
    match crate::outbox::import(config_dir, store, machine_id, roots, &live) {
        Ok(moved) => result.imported = moved,
        // The CLI holding its own lock is not a failure: the next tick is two minutes away.
        Err(problem) => result.problems.push(problem),
    }

    if let Some(desktop) = desktop_store {
        match crate::desktop_store::publish(desktop, store, machine_id, roots) {
            Ok(cards) => result.cards_out = cards.exported.len(),
            Err(problem) => result.problems.push(problem),
        }
        let confirmed = confirmed_transcripts(store, machine_id);
        match crate::desktop_store::import(desktop, store, machine_id, roots, &|id| {
            confirmed.get(id).cloned()
        }) {
            Ok(cards) => result.cards_in = cards.imported.len(),
            Err(problem) => result.problems.push(problem),
        }
    }

    match project_memory(store, machine_id, stamp) {
        Ok(projected) => result.projected_memory = projected,
        Err(problem) => result.problems.push(problem),
    }

    match restore_deleted_transcripts(store) {
        Ok(restored) => result.restored = restored,
        Err(problem) => result.problems.push(problem),
    }

    result
        .imported_directories
        .append(&mut import_real_directories(
            store, config_dir, roots, naming,
        ));

    match commit_project_files(store, &live, stamp) {
        Ok(files) => result.project_files_committed = files,
        Err(problem) => result.problems.push(problem),
    }

    match commit_own_outbox(store, machine_id, stamp) {
        Ok(files) => result.outbox_committed = files,
        Err(problem) => result.problems.push(problem),
    }

    match clear_stale_heartbeats(store, machine_id, heartbeat_cutoff) {
        Ok(cleared) => result.stale_sessions = cleared,
        Err(problem) => result.problems.push(problem),
    }

    result.pushed = push(store);

    result
}

/// Fetches from the remote. No remote at all is not a problem: a store can be local for a while.
fn fetch(store: &Path) -> bool {
    if !has_remote(store) {
        return false;
    }
    // A refusal here is the network, a locked repository, a rejected key: all of them mean
    // "later", and the tick runs again in two minutes.
    matches!(
        git::run_with_timeout(git::command(store, &["fetch", "--quiet", REMOTE]), TIMEOUT),
        Ok(Some(_))
    )
}

/// What the merge step decided.
enum Merge {
    Merged,
    NothingToDo,
    HeldBack { sessions: Vec<String> },
}

/// Merges what was fetched, unless it touches a session that is live on this machine.
///
/// The guard is fail-closed on purpose: if the incoming commits touch the transcript of a session
/// this machine is writing right now, merging would put another machine's records into a file
/// under active append, and the result would be decided by whoever wrote last. Waiting costs two
/// minutes; the other outcome costs records.
fn merge_if_safe(store: &Path, machine_id: &str) -> Result<Merge, String> {
    let target = format!("{REMOTE}/{BRANCH}");
    // Nothing to merge is the common case, and it must be cheap and silent: without this the
    // tick would report a merge on every run and lie in the log.
    let behind = git::run_with_timeout(
        git::command(store, &["rev-list", "--count", &format!("HEAD..{target}")]),
        TIMEOUT,
    )
    .unwrap_or(None)
    .and_then(|text| text.trim().parse::<u64>().ok())
    .unwrap_or(0);
    if behind == 0 {
        return Ok(Merge::NothingToDo);
    }
    let Some(incoming) = changed_paths(store, &target)? else {
        return Ok(Merge::NothingToDo);
    };
    if incoming.is_empty() {
        return Ok(Merge::NothingToDo);
    }

    let live = live_sessions(store, machine_id);
    let touched: Vec<String> = live
        .iter()
        .filter(|session| incoming.iter().any(|path| path.contains(session.as_str())))
        .cloned()
        .collect();
    if !touched.is_empty() {
        return Ok(Merge::HeldBack { sessions: touched });
    }

    if let Ok(Some(_)) = git::run_with_timeout(
        git::command(store, &["merge", "--quiet", "--no-edit", &target]),
        TIMEOUT,
    ) {
        return Ok(Merge::Merged);
    }
    // A merge that could not be completed leaves the repository mid-merge, and nothing else may
    // run until that is undone: a commit with MERGE_HEAD would record somebody's guess.
    let _ = git::run_with_timeout(git::command(store, &["merge", "--abort"]), TIMEOUT);
    Err("the merge was aborted; the working tree is as it was".to_owned())
}

/// Paths the target holds that HEAD does not, or `None` when the target does not exist yet.
fn changed_paths(store: &Path, target: &str) -> Result<Option<Vec<String>>, String> {
    if git::run_with_timeout(
        git::command(store, &["rev-parse", "--verify", "--quiet", target]),
        TIMEOUT,
    )
    .unwrap_or(None)
    .is_none()
    {
        return Ok(None);
    }
    // Three dots on purpose: `HEAD..target` in a diff would also list what *this* machine changed
    // since the merge base, so our own live session would look like an incoming change and hold
    // its own merge back for ever.
    let range = format!("HEAD...{target}");
    let listed = git::run_with_timeout(
        git::command(store, &["diff", "--name-only", &range]),
        TIMEOUT,
    )?;
    Ok(listed.map(|text| {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }))
}

/// Sessions this machine believes are running right now.
fn live_sessions(store: &Path, machine_id: &str) -> BTreeSet<String> {
    let path = store.join("machines").join(machine_id).join("live.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Live>(&text).ok())
        .map(|live| live.sessions.into_keys().collect())
        .unwrap_or_default()
}

/// Removes the transcripts every machine has asked to forget, and commits the removal.
fn apply_tombstones(store: &Path, stamp: &str) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return Ok(removed);
    };
    for machine in machines.filter_map(Result::ok) {
        let file = machine.path().join(crate::forget::FORGOTTEN_FILE);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(forgotten) = serde_json::from_str::<Forgotten>(&text) else {
            continue;
        };
        for (session, tombstone) in forgotten.sessions {
            if tombstone.path.is_empty() {
                continue;
            }
            let path = store.join(&tombstone.path);
            if !path.exists() {
                continue;
            }
            std::fs::remove_file(&path).map_err(|error| error.to_string())?;
            git::run_with_timeout(
                git::command(
                    store,
                    &[
                        "rm",
                        "--quiet",
                        "--cached",
                        "--ignore-unmatch",
                        &tombstone.path,
                    ],
                ),
                TIMEOUT,
            )?;
            removed.push(session);
        }
    }
    if !removed.is_empty() {
        let message = format!("vibememory: forgot {} session(s) at {stamp}", removed.len());
        git::run_with_timeout(
            git::command(store, &["commit", "--quiet", "-m", &message]),
            TIMEOUT,
        )?;
    }
    Ok(removed)
}

/// Puts back transcripts that vanished from the working copy without a tombstone.
///
/// A file can disappear for reasons nobody chose: a retention sweeper, a synchronisation client,
/// somebody tidying a directory. Pushing that deletion would spread it to every machine, and the
/// records would be gone for good — so the only deletion the engine honours is the one that came
/// through `forget`.
fn restore_deleted_transcripts(store: &Path) -> Result<Vec<String>, String> {
    let listed = git::run_with_timeout(
        git::command(store, &["ls-files", "--deleted", "--", "projects"]),
        TIMEOUT,
    )?;
    let Some(text) = listed else {
        return Ok(Vec::new());
    };
    let missing: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    for path in &missing {
        git::run_with_timeout(
            git::command(store, &["checkout", "--", path.as_str()]),
            TIMEOUT,
        )?;
    }
    Ok(missing)
}

/// Pushes, when there is anything to push and somewhere to push it.
fn push(store: &Path) -> bool {
    if !has_remote(store) {
        return false;
    }
    // Nothing of ours to send is the common case; saying "pushed" then would make the log useless
    // for telling a working machine from a stuck one.
    let ahead = git::run_with_timeout(
        git::command(
            store,
            &["rev-list", "--count", &format!("{REMOTE}/{BRANCH}..HEAD")],
        ),
        TIMEOUT,
    )
    .unwrap_or(None)
    .and_then(|text| text.trim().parse::<u64>().ok())
    .unwrap_or(0);
    if ahead == 0 {
        return false;
    }
    matches!(
        git::run_with_timeout(
            git::command(store, &["push", "--quiet", REMOTE, BRANCH]),
            TIMEOUT,
        ),
        Ok(Some(_))
    )
}

/// Whether the store has a remote configured at all.
fn has_remote(store: &Path) -> bool {
    git::run_with_timeout(git::command(store, &["remote"]), TIMEOUT)
        .ok()
        .flatten()
        .is_some_and(|text| !text.trim().is_empty())
}

/// A lock held for the duration of one tick.
///
/// Two ticks in one store would fetch and merge over each other. The lock names the process that
/// holds it, and a lock whose process is gone is taken over: a machine that lost power must not
/// need a person to start synchronising again.
#[derive(Debug)]
pub struct TickLock {
    path: std::path::PathBuf,
}

impl TickLock {
    /// Takes the lock, or reports who holds it.
    ///
    /// # Errors
    ///
    /// The text naming the holder, when another tick of this machine is running.
    pub fn take(engine_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
        let path = engine_dir.join("tick.lock");
        let pid = std::process::id();
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                let _ = write!(file, "{pid}");
                Ok(Self { path })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let holder = std::fs::read_to_string(&path).unwrap_or_default();
                if holder
                    .trim()
                    .parse::<u32>()
                    .is_ok_and(crate::process::is_running)
                {
                    return Err(format!("another tick is running (pid {})", holder.trim()));
                }
                // The holder is gone: its lock is a leftover, not a claim.
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                Self::take(engine_dir)
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

impl Drop for TickLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
/// Writes the memory projection of every project in the store.
///
/// Without this, records that arrived from another machine would sit in the journal unseen until
/// somebody happened to start a session in that project — and memory that nobody can read is
/// memory that was not synchronised. Only the default location is projected here: a session that
/// moved its memory elsewhere projects it through its own hooks, which know where "elsewhere" is.
fn project_memory(store: &Path, machine_id: &str, stamp: &str) -> Result<Vec<String>, String> {
    let Ok(projects) = std::fs::read_dir(store.join("projects")) else {
        return Ok(Vec::new());
    };
    let mut projected = Vec::new();
    for project in projects.filter_map(Result::ok) {
        let journal = project.path().join(crate::memory::JOURNAL_FILE);
        if !journal.exists() {
            continue;
        }
        let memory_dir = project.path().join("memory");
        let synced = crate::memory::sync(&memory_dir, &journal, stamp, machine_id)?;
        if !synced.written.is_empty() || synced.imported > 0 {
            projected.push(project.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(projected)
}

/// What the reconciler did.
#[derive(Debug, Default)]
struct Reconciled {
    linked: Vec<String>,
    disagreements: Vec<String>,
}

/// Creates the links other machines' records call for, before any session needs them.
///
/// The fallback scans of both the CLI and Desktop are blind to symlinks, so a link that does not
/// exist beforehand is a link nobody will find. Three things stop the reconciler: a working
/// directory that does not exist here, a name this machine resolves differently, and a path
/// already occupied. None of them is fixed automatically — the first is somebody else's project,
/// the second is a disagreement worth a person's attention, and the third may be a live session.
fn reconcile_links(
    store: &Path,
    config_dir: &Path,
    machine_id: &str,
    roots: &Roots,
) -> Result<Reconciled, String> {
    let mut outcome = Reconciled::default();
    let own = links_file::read(store, machine_id);

    for (machine, record) in links_file::read_all(store) {
        if machine == machine_id {
            continue;
        }
        // A path this machine cannot translate names a directory it does not have.
        let Ok(local_cwd) = roots.to_local(&record.cwd) else {
            continue;
        };
        if !Path::new(&local_cwd).is_dir() {
            continue;
        }
        // Our own record for the same encoded directory outranks theirs; if the names differ,
        // one of us is wrong and neither may act.
        if let Some(ours) = own.links.iter().find(|ours| ours.enc == record.enc)
            && !ours.same_name_as(&record)
        {
            outcome.disagreements.push(record.enc.clone());
            continue;
        }

        let link = config_dir.join("projects").join(&record.enc);
        if std::fs::symlink_metadata(&link).is_ok() {
            continue; // already there, or something else is — either way, not ours to change
        }
        let target = store.join("projects").join(&record.name);
        std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        symlink_dir(&target, &link)?;
        links_file::record(
            store,
            machine_id,
            &Observation {
                enc: &record.enc,
                name: &record.name,
                cwd: &record.cwd,
                syntax: record.syntax,
                source: LinkSource::Reconciled,
                confirmed_by: None,
            },
        )?;
        outcome.linked.push(record.enc);
    }
    Ok(outcome)
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

/// The transcripts this machine has actually confirmed, from its own `links.json`: a card may only
/// come in when a session here proved the link its transcript needs.
fn confirmed_transcripts(
    store: &Path,
    machine_id: &str,
) -> std::collections::HashMap<String, String> {
    crate::links_file::read(store, machine_id)
        .links
        .into_iter()
        .filter_map(|record| {
            let path = record.confirmed_by?;
            // The confirmed path names the transcript file; its stem is the session id.
            let id = Path::new(&path).file_stem()?.to_string_lossy().into_owned();
            Some((id, path))
        })
        .collect()
}

/// How long a session may go without a heartbeat before this machine stops claiming it is live.
///
/// The hooks refresh it on every stop and at the end, so a session unheard-of for this long ended
/// in a way that ran no hook: a crash, a kill, a machine that lost power. Left in place, it would
/// hold back every merge of that session's file and refuse every relink for ever.
pub const HEARTBEAT_STALE_AFTER: Duration = Duration::from_hours(1);

/// Drops this machine's own stale claims.
///
/// Only its own. Another machine's `live.json` is that machine's to correct, and deciding from
/// here that somebody else is dead is exactly the mistake that ends with two machines writing one
/// file — their clock is not ours, and their silence may be a closed lid.
///
/// `cutoff` is a stamp from this machine's clock; comparison is textual, which is exact for
/// ISO-8601 in UTC and needs no date arithmetic.
fn clear_stale_heartbeats(
    store: &Path,
    machine_id: &str,
    cutoff: &str,
) -> Result<Vec<String>, String> {
    let path = store.join("machines").join(machine_id).join("live.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let Ok(mut live) = serde_json::from_str::<Live>(&text) else {
        return Ok(Vec::new());
    };
    let stale: Vec<String> = live
        .sessions
        .iter()
        .filter(|(_, session)| session.at.as_str() < cutoff)
        .map(|(id, _)| id.clone())
        .collect();
    if stale.is_empty() {
        return Ok(Vec::new());
    }
    for id in &stale {
        live.sessions.remove(id);
    }
    let text = serde_json::to_string_pretty(&live).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    Ok(stale)
}

/// Commits this machine's outbox: `links.json`, `live.json`, `tails.json`, the cards, the
/// tombstones. The hooks write these files but do not commit them — a hook has ten seconds and
/// no business running git commits — so until the tick does, nothing this machine says reaches
/// the others. Only its own directory is staged, by path; never `git add -A`.
fn commit_own_outbox(store: &Path, machine_id: &str, stamp: &str) -> Result<usize, String> {
    let own = format!("machines/{machine_id}");
    if !store.join(&own).is_dir() {
        return Ok(0);
    }
    let changed = git::changed_paths(store, &own, TIMEOUT)?;
    if changed.is_empty() {
        return Ok(0);
    }
    git::run_with_timeout(git::command(store, &["add", "--", &own]), TIMEOUT)?
        .ok_or_else(|| "git refused to stage the outbox".to_owned())?;
    let message = format!("vibememory: outbox of {machine_id} at {stamp}");
    git::run_with_timeout(
        git::command(store, &["commit", "--quiet", "-m", &message]),
        TIMEOUT,
    )?
    .ok_or_else(|| "git refused to commit the outbox".to_owned())?;
    Ok(changed.len())
}

/// Commits what appeared under `projects/` outside any live session: side files of ended
/// sessions, `.keep` markers the hook wrote, directories `import` brought in, memory files.
///
/// A session live on this machine is left entirely alone — its transcript is committed by the
/// `Stop` hook as a snapshot cut at the last newline, and its side files are being written this
/// very moment. Everything else is nobody's but the tick's to commit, and without this it would
/// stay on one machine for ever.
fn commit_project_files(
    store: &Path,
    live: &BTreeSet<String>,
    stamp: &str,
) -> Result<usize, String> {
    let changed = git::changed_paths(store, "projects", TIMEOUT)?;
    let ours: Vec<&str> = changed
        .iter()
        .map(String::as_str)
        .filter(|path| !belongs_to_live_session(path, live))
        .collect();
    if ours.is_empty() {
        return Ok(0);
    }
    for chunk in ours.chunks(200) {
        let mut args = vec!["add", "--"];
        args.extend(chunk.iter().copied());
        git::run_with_timeout(git::command(store, &args), TIMEOUT)?
            .ok_or_else(|| "git refused to stage project files".to_owned())?;
    }
    let message = format!("vibememory: {} project file(s) at {stamp}", ours.len());
    git::run_with_timeout(
        git::command(store, &["commit", "--quiet", "-m", &message]),
        TIMEOUT,
    )?
    .ok_or_else(|| "git refused to commit project files".to_owned())?;
    Ok(ours.len())
}

/// Whether a store path is a live session's transcript (`<sid>.jsonl`) or lies inside its side
/// directory (`<sid>/…`).
fn belongs_to_live_session(path: &str, live: &BTreeSet<String>) -> bool {
    live.iter()
        .any(|sid| path.contains(&format!("/{sid}.jsonl")) || path.contains(&format!("/{sid}/")))
}

/// Copies real `projects/<enc>` directories into the store and puts links in their place.
///
/// This is what `SessionStart` promises when it finds one: the CLI created it before any link
/// existed, and until it is copied its transcripts live on one disk only. The hook cannot do it —
/// it runs inside the session that is writing there — so the tick does, and only for directories
/// no machine reports a session in.
///
/// The working directory is read from the transcripts themselves: `enc` cannot be inverted, but
/// every record carries the `cwd` it was written in.
fn import_real_directories(
    store: &Path,
    config_dir: &Path,
    roots: &Roots,
    naming: &vibememory_core::naming::NamingConfig,
) -> Vec<String> {
    let mut imported = Vec::new();
    let Ok(entries) = std::fs::read_dir(config_dir.join("projects")) else {
        return imported;
    };
    let existing = crate::store::existing(store, TIMEOUT).names;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_symlink() || !metadata.is_dir() {
            continue;
        }
        let enc = entry.file_name().to_string_lossy().into_owned();
        let Some(cwd) = working_directory_of(&path) else {
            continue; // nothing but empty scaffolding: the hook will link it on the next start
        };
        let portable = roots.to_portable(&cwd).unwrap_or_else(|_| cwd.clone());
        let syntax = vibememory_core::naming::PathSyntax::Posix;
        let canonical = vibememory_core::naming::canonical_cwd(&cwd, syntax);
        let input = vibememory_core::naming::NamingInput {
            cwd: &canonical,
            syntax,
            project_dir_name: None,
        };
        let resolved = vibememory_core::naming::resolve_store_name(
            &input,
            || crate::git::probe(Path::new(&canonical), Duration::from_secs(10)),
            naming,
            &existing,
        );
        let Ok(vibememory_core::naming::Resolution::Named { name, .. }) = resolved else {
            continue; // ignored, or the rules could not name it: not the tick's to decide
        };
        // A refusal means a live session somewhere, or a path this build does not handle: the
        // hook already said so, and the next tick tries again.
        if let Ok(outcome) =
            crate::relink::import_real_directory(config_dir, store, &enc, name.as_str(), &portable)
        {
            imported.push(format!(
                "{enc} -> projects/{} ({} file(s))",
                name.as_str(),
                outcome.copied.len()
            ));
        }
    }
    imported
}

/// The working directory a project directory's transcripts were written in.
fn working_directory_of(dir: &Path) -> Option<String> {
    let mut transcripts: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    // Newest first: the most recent session knows the directory as it is called now.
    transcripts.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    for path in transcripts.iter().rev() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines().take(50) {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
                return Some(cwd.to_owned());
            }
        }
    }
    None
}
