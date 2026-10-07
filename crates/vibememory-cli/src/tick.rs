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
/// The store's one branch. Public because `doctor` asks the host about the same branch the tick
/// pushes, and two constants spelling "main" would drift apart on the day it is renamed.
pub const BRANCH: &str = "main";
/// Where the store's own commits go.
const REMOTE: &str = "origin";

/// A directory the naming rules told the tick to leave alone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgnoredDirectory {
    /// The encoded directory name under `projects/`.
    pub enc: String,
    /// Why it was left alone, in the words of the rule that decided.
    pub reason: String,
    /// How many transcripts sit in it. "Skipped one" and "skipped one holding four transcripts"
    /// are different sentences: the second names the cost, the first only the fact.
    pub transcripts: usize,
}

/// What became of the half of the tick that needs the network.
///
/// Three states rather than two flags: a run either tried or was backed off, and "recovered" is a
/// run that tried and was the first to work after failures. Two independent booleans would allow
/// "paused and recovered", which is not a thing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StoreCycle {
    /// Tried, as usual.
    #[default]
    Ran,
    /// Skipped: earlier runs kept failing and the back-off is still counting down.
    Paused,
    /// Tried and worked, and the run before the failures was long enough ago to say so.
    Recovered,
    /// The team's sessions are switched off — the host refused this machine as a memory team does,
    /// or rewrote the store to a new generation. Nothing was merged; the caller leaves the store.
    SessionsOff,
}

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
    /// Links that existed on this machine but were missing from its `links.json`, now recorded.
    pub recorded_links: usize,
    /// Real `projects/<enc>` directories copied into the store and replaced by links.
    pub imported_directories: Vec<String>,
    /// Directories the rules told the tick to leave alone, and what they hold.
    ///
    /// There is a field for what was taken; without one for what was not, a directory whose
    /// transcripts will never leave this disk does not exist as far as anyone outside can tell.
    /// Same argument as `held_back` above: a synchronisation that never happens is one somebody
    /// has to be able to see the reason for.
    pub ignored_directories: Vec<IgnoredDirectory>,
    /// Files under `projects/` committed by this tick: side files, `.keep` markers, imported
    /// directories, memory — everything a session is not writing right now.
    pub shared_files_committed: usize,
    /// Sessions dropped from this machine's live list because nothing has confirmed them for
    /// longer than a machine that crashed would take to come back.
    pub stale_sessions: Vec<String>,
    /// What happened to the managed copies of `CLAUDE.md` and `settings.json`.
    pub managed: crate::managed::Reconciled,
    /// Desktop cards published to the outbox.
    pub cards_out: usize,
    /// Desktop cards written into the local Desktop store.
    pub cards_in: usize,
    /// Desktop cards of this machine repaired from its own outbox: Desktop erases the transcript
    /// handle and never restores it, so nothing else would ever take the mark back.
    pub cards_repaired: usize,
    /// A cloud client's conflict copies folded back into the cards they duplicate: set aside in
    /// the quarantine, or renamed into place when the card they copied is not there at all.
    pub cards_folded: usize,
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
    /// Deletions this run refused to make because the tombstones asked for more than the cap.
    pub deletions_held: usize,
    /// What happened to the part of the run that talks to the remote.
    pub store_cycle: StoreCycle,
    /// What went wrong, if anything. A tick reports and returns; it never panics a machine.
    pub problems: Vec<String>,
    /// The pause the host put this store's pushes under, when there is one after this run.
    pub pause: Option<crate::guard::StorePause>,
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
    /// The home directory other agents keep their directories under, for their copies of the
    /// shared instructions; `None` gives no agent a copy.
    pub home: Option<&'a Path>,
    /// This machine's name in the store.
    pub machine_id: &'a str,
    /// Named roots, for translating working directories.
    pub roots: &'a Roots,
    /// The naming rules, for a directory the tick has to name itself.
    pub naming: &'a vibememory_core::naming::NamingConfig,
    /// Where Desktop keeps its cards, when this machine has them.
    pub desktop_store: Option<&'a Path>,
    /// How many transcripts this run may remove before holding them all back.
    pub max_deletions: usize,
    /// Whether the person released this one run from that cap.
    pub deletions_released: bool,
    /// When a paused store asks its host again, if this run pauses it: the caller's clock, one
    /// recheck interval ahead.
    pub recheck_at: &'a str,
    /// The team whose store this run is over, by id; `None` for the personal store. A team store
    /// carries sessions and memory only: the machine's history, tasks, Desktop cards and managed
    /// copies of the configuration stay in the personal store.
    pub team: Option<&'a str>,
    /// Which store a working directory belongs to: a run links and takes in only the projects
    /// routed to its own store, so a session never lands in two.
    pub routes: &'a vibememory_core::naming::StoreRoutes,
}

/// Whether a working directory of this machine belongs to the store of this run. A directory the
/// routes cannot place — two teams claim it — belongs to none: the hook names the conflict, and a
/// session is never split between stores.
fn routed_here(machine: &Machine<'_>, local_cwd: &str) -> bool {
    let syntax = crate::hook::session_start::host_syntax();
    let canonical = vibememory_core::naming::canonical_cwd(local_cwd, syntax);
    machine
        .routes
        .route(&canonical, syntax)
        .is_ok_and(|found| found == machine.team)
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
        home,
        machine_id,
        roots,
        // `naming` is read through `machine` by `take_real_directories`, which needs the whole
        // of it; destructuring a copy here would leave two names for one thing.
        naming: _,
        // read by `exchange_personal`, which the personal store alone runs
        desktop_store: _,
        max_deletions,
        deletions_released,
        team,
        routes: _,
        recheck_at,
    } = machine;
    let personal = team.is_none();
    let engine_dir = engine_dir_of(store);
    let state = crate::guard::TickState::read(&engine_dir);
    let mut result = Ticked::default();
    let allowed = state.store_cycle_allowed();
    if allowed {
        fetch_and_merge(store, machine_id, !personal, &mut result);
    } else {
        result.store_cycle = StoreCycle::Paused;
    }
    // the team closed its sessions: nothing more is done in a store this machine is about to leave
    if result.store_cycle == StoreCycle::SessionsOff {
        return result;
    }

    let live = live_sessions(store, machine_id);
    match apply_tombstones(store, stamp, max_deletions, deletions_released) {
        Ok(Removals::Applied(removed)) => result.forgotten = removed,
        Ok(Removals::Held { wanted }) => result.deletions_held = wanted,
        Err(problem) => result.problems.push(problem),
    }

    result.recorded_links = record_local_links(store, config_dir, machine_id, roots);

    match reconcile_links(machine) {
        Ok(mut outcome) => {
            result.linked.append(&mut outcome.linked);
            result.disagreements.append(&mut outcome.disagreements);
        }
        Err(problem) => result.problems.push(problem),
    }

    if personal {
        exchange_personal(machine, stamp, &live, &mut result);
    }

    match project_memory(store, machine_id, stamp) {
        Ok(projected) => result.projected_memory = projected,
        Err(problem) => result.problems.push(problem),
    }

    match restore_deleted_transcripts(store) {
        Ok(restored) => result.restored = restored,
        Err(problem) => result.problems.push(problem),
    }

    let seen_before = take_real_directories(machine, stamp, &state, &mut result);

    // Before `config/` is committed, so that what this machine changed in its managed copies is
    // in the store when the commit looks — and after the merge above, so that what another
    // machine changed is what gets pulled.
    if personal {
        match crate::managed::reconcile(
            &crate::install::Layout {
                config_dir: config_dir.to_path_buf(),
                engine_dir: engine_dir_of(store),
                home: home.map(Path::to_path_buf),
            },
            store,
            stamp,
        ) {
            Ok(managed) => result.managed = managed,
            Err(problem) => result.problems.push(problem),
        }
    }

    match commit_shared_files(store, &engine_dir_of(store), &live, stamp, personal) {
        Ok(files) => result.shared_files_committed = files,
        Err(problem) => result.problems.push(problem),
    }

    // Every store: the machine's own directory carries its heartbeats, link records and session
    // tails, which a team's machines read as much as the owner's do. History and tasks reach it
    // only through the personal exchange above, so a team's copy never holds them.
    match commit_own_outbox(store, &engine_dir_of(store), machine_id, stamp) {
        Ok(files) => result.outbox_committed = files,
        Err(problem) => result.problems.push(problem),
    }

    match clear_ended_sessions(store, machine_id, heartbeat_cutoff) {
        Ok(cleared) => result.stale_sessions = cleared,
        Err(problem) => result.problems.push(problem),
    }

    if allowed {
        // A refused push is a failure of the store cycle — it used to be a bare `false`,
        // indistinguishable from "nothing to send": on 2026-09-12 the host's disk filled up, every
        // push was refused for a day, and the tick reported no failures while 291 commits stayed
        // on this machine only. A team's host refusing for the team's own reasons is not that: it
        // pauses the store's pushes, says why, and asks again after `PAUSE_RECHECK`.
        push_or_pause(store, &state, stamp, recheck_at, &mut result);
    }

    // A run counts as failed when something went wrong with the store itself; a held deletion is
    // the rail working as intended, not a failure, and must not push the machine into a back-off.
    let (next, recovered) = state.after_run(!result.problems.is_empty());
    if recovered {
        result.store_cycle = StoreCycle::Recovered;
    }
    let next = crate::guard::TickState {
        deletions_held: result.deletions_held,
        ignored: seen_before,
        pause: result.pause.clone(),
        ..next
    };
    if let Err(problem) = next.write(&engine_dir) {
        result.problems.push(problem);
    }

    result
}

/// Pushes unless the host paused this store and its recheck is not due; a push that goes through
/// ends the pause, a refusal for the team's own reasons starts or renews it.
fn push_or_pause(
    store: &Path,
    state: &crate::guard::TickState,
    stamp: &str,
    recheck_at: &str,
    result: &mut Ticked,
) {
    if !state.push_due(stamp) {
        result.pause.clone_from(&state.pause);
        return;
    }
    match push(store) {
        Ok(pushed) => {
            result.pushed = pushed;
            result.pause = None;
        }
        Err(problem) if refused_as_memory_team(&problem) => {
            result.store_cycle = StoreCycle::SessionsOff;
        }
        Err(problem) => match paused_by_host(&problem, stamp, recheck_at) {
            Some(pause) => result.pause = Some(pause),
            None => result.problems.push(problem),
        },
    }
}

/// Whether a refusal says the team is a memory team now: its sessions were switched off.
fn refused_as_memory_team(problem: &str) -> bool {
    vibememory_core::push_refusal::parse(problem).is_some_and(|refusal| {
        vibememory_core::push_refusal::remedy(&refusal.code)
            == vibememory_core::push_refusal::Remedy::SessionsOff
    })
}

/// A refusal of the host that pauses the store, rather than a failure the back-off counts: the
/// team's standing or a path the clone may not hold. A host short of disk for now is a failure
/// like any other, and the next tick tries again.
fn paused_by_host(
    problem: &str,
    stamp: &str,
    recheck_at: &str,
) -> Option<crate::guard::StorePause> {
    let refusal = vibememory_core::push_refusal::parse(problem)?;
    if vibememory_core::push_refusal::remedy(&refusal.code)
        == vibememory_core::push_refusal::Remedy::Transient
    {
        return None;
    }
    Some(crate::guard::StorePause {
        code: refusal.code,
        lines: refusal.lines,
        since: stamp.to_owned(),
        recheck_at: recheck_at.to_owned(),
    })
}

/// The personal store's own exchange: the machine's history and tasks through the outbox, and
/// Desktop's cards. A team store carries none of them.
fn exchange_personal(
    machine: &Machine<'_>,
    stamp: &str,
    live: &BTreeSet<String>,
    result: &mut Ticked,
) {
    let &Machine {
        store,
        config_dir,
        machine_id,
        roots,
        desktop_store,
        ..
    } = machine;
    match publish_outbox(config_dir, store, machine_id, roots, stamp) {
        Ok(moved) => result.published = moved,
        Err(problem) => result.problems.push(problem),
    }
    match crate::outbox::import(config_dir, store, machine_id, roots, live) {
        Ok(moved) => result.imported = moved,
        // The CLI holding its own lock is not a failure: the next tick is two minutes away.
        Err(problem) => result.problems.push(problem),
    }
    if let Some(desktop) = desktop_store {
        exchange_cards(desktop, store, machine_id, roots, stamp, result);
    }
}

/// Fetches from the remote. No remote at all is not a problem: a store can be local for a while.
fn fetch(store: &Path) -> Result<bool, String> {
    if !has_remote(store) {
        return Ok(false);
    }
    // A refusal here is the network, a locked repository, a rejected key: all of them mean
    // "later", and the tick runs again in two minutes — except a team's host saying the team's
    // sessions are off, which the caller reads from the words
    match git::run_capturing(git::command(store, &["fetch", "--quiet", REMOTE]), TIMEOUT) {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(said)) => Err(said),
        Err(problem) => Err(problem),
    }
}

/// The generation a commit's tree names, if any.
fn generation_at(store: &Path, commit: &str) -> Option<String> {
    let spec = format!("{commit}:{}", vibememory_core::team_store::GENERATION_FILE);
    git::run_with_timeout(git::command(store, &["show", &spec]), TIMEOUT)
        .ok()
        .flatten()
}

/// Whether what a team's host said, or what it now holds, means the team's sessions are off: a
/// refusal of a memory team, or a store the host rewrote to a generation this clone is not of.
fn sessions_switched_off(store: &Path, fetched: &Result<bool, String>) -> bool {
    match fetched {
        Err(said) => refused_as_memory_team(said),
        Ok(true) => {
            let theirs = generation_at(store, &format!("{REMOTE}/{BRANCH}"));
            theirs.is_some() && theirs != generation_at(store, "HEAD")
        }
        Ok(false) => false,
    }
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

/// Imports the real project directories, and reports the ones the rules said to leave alone.
///
/// Returns every ignored directory, for the state to remember. What goes into the report is only
/// what the state has not seen: a directory ignored by an explicit rule is ignored for ever, and
/// a line repeated every two minutes is a line nobody reads. `status` answers from the state at
/// any time, so saying it once loses nothing.
fn take_real_directories(
    machine: &Machine<'_>,
    stamp: &str,
    state: &crate::guard::TickState,
    result: &mut Ticked,
) -> Vec<IgnoredDirectory> {
    let (imported, ignored) = import_real_directories(machine, stamp);
    result.imported_directories = imported;
    result.ignored_directories = ignored
        .iter()
        .filter(|directory| !state.ignored.contains(directory))
        .cloned()
        .collect();
    ignored
}

/// The half of the run that needs the remote: take what is there, merge it when that is safe.
///
/// Split out because the back-off switches exactly this off and nothing else — everything the tick
/// does locally (putting back a vanished transcript, the heartbeat, projecting memory) runs on
/// every tick regardless. A rail that stopped the work which *saves* data would be worse than the
/// failure it is backing off from.
fn fetch_and_merge(store: &Path, machine_id: &str, team: bool, result: &mut Ticked) {
    let fetched = fetch(store);
    result.fetched = matches!(fetched, Ok(true));
    // a team store rewritten or closed to this machine is left, never merged into
    if team && sessions_switched_off(store, &fetched) {
        result.store_cycle = StoreCycle::SessionsOff;
        return;
    }
    match merge_if_safe(store, machine_id) {
        Ok(Merge::Merged) => result.merged = true,
        Ok(Merge::HeldBack { sessions }) => result.held_back = sessions,
        Ok(Merge::NothingToDo) => {}
        Err(problem) => result.problems.push(problem),
    }
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

/// What one run did about the deletions its tombstones asked for.
enum Removals {
    /// Carried out, naming the sessions.
    Applied(Vec<String>),
    /// Refused as a whole, naming how many were asked for.
    Held {
        /// How many transcripts the tombstones named.
        wanted: usize,
    },
}

/// Removes the transcripts every machine has asked to forget, and commits the removal.
///
/// Counts before it removes: a tombstone file that arrived corrupted, mis-merged or simply wrong
/// would otherwise take every transcript it names off this machine and then off all the others,
/// and there is no guard on the other side — `restore_deleted_transcripts` puts back what vanished
/// *without* a tombstone, so a deletion that carries one is honoured everywhere.
fn apply_tombstones(
    store: &Path,
    stamp: &str,
    cap: usize,
    released: bool,
) -> Result<Removals, String> {
    let wanted = tombstoned_paths(store).len();
    if let crate::guard::Deletions::Held { wanted, .. } =
        crate::guard::deletions(wanted, cap, released)
    {
        return Ok(Removals::Held { wanted });
    }
    let mut removed = Vec::new();
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return Ok(Removals::Applied(removed));
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
    Ok(Removals::Applied(removed))
}

/// The store paths every machine's tombstones name and this machine still holds.
///
/// The same walk `apply_tombstones` does, without touching anything: counting has to see exactly
/// what the removal would see, or the cap would guard a different number than the one at risk.
fn tombstoned_paths(store: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return paths;
    };
    for machine in machines.filter_map(Result::ok) {
        let file = machine.path().join(crate::forget::FORGOTTEN_FILE);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(forgotten) = serde_json::from_str::<Forgotten>(&text) else {
            continue;
        };
        for tombstone in forgotten.sessions.into_values() {
            if !tombstone.path.is_empty() && store.join(&tombstone.path).exists() {
                paths.push(tombstone.path);
            }
        }
    }
    paths
}

/// Puts back transcripts that vanished from the working copy without a tombstone.
///
/// A file can disappear for reasons nobody chose: a retention sweeper, a synchronisation client,
/// somebody tidying a directory. Pushing that deletion would spread it to every machine, and the
/// records would be gone for good — so the only deletion the engine honours is the one that came
/// through `forget`.
///
/// Memory projections (`projects/<name>/memory/…`) are not put back. They are derived from the
/// journal beside them: a live record's file is rewritten by the projection anyway, and a file
/// the projection removed belongs to a record the journal forgot. Putting that one back undid the
/// delete on every tick — found 2026-09-13, when a delete from the host never left the Mac.
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
        .filter(|line| !line.is_empty() && !is_memory_projection(line))
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

/// Whether a store path lies inside a project's memory projection directory.
fn is_memory_projection(path: &str) -> bool {
    let mut segments = path.split('/');
    segments.next() == Some("projects")
        && segments.next().is_some()
        && segments.next() == Some("memory")
        && segments.next().is_some()
}

/// Pushes, when there is anything to push and somewhere to push it: `Ok(true)` pushed,
/// `Ok(false)` nothing to send.
///
/// # Errors
///
/// What the remote said when it refused, or why git could not be run.
fn push(store: &Path) -> Result<bool, String> {
    if !has_remote(store) {
        return Ok(false);
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
        return Ok(false);
    }
    match git::run_capturing(
        git::command(store, &["push", "--quiet", REMOTE, BRANCH]),
        TIMEOUT,
    )? {
        Ok(_) => Ok(true),
        Err(said) if said.contains(PACK_TOO_LARGE) => push_in_portions(store),
        Err(said) => Err(format!(
            "push to {REMOTE} refused with {ahead} commit(s) waiting: {said}"
        )),
    }
}

/// What git says when a push is bigger than the host takes in one go (`receive.maxInputSize`).
const PACK_TOO_LARGE: &str = "pack exceeds maximum allowed size";

/// The first portion of commits a too-large push is split into.
const FIRST_PORTION: usize = 64;

/// A push too big for the host in one go — a machine back after weeks offline — sent as a run of
/// smaller pushes, oldest first: each portion pushes the commit it ends at, and a portion the
/// host still finds too big is halved. Only a single commit too big for the host is a failure.
fn push_in_portions(store: &Path) -> Result<bool, String> {
    let listed = git::run_with_timeout(
        git::command(
            store,
            &["rev-list", "--reverse", &format!("{REMOTE}/{BRANCH}..HEAD")],
        ),
        TIMEOUT,
    )?
    .ok_or_else(|| "the commits waiting to be pushed could not be listed".to_owned())?;
    let commits: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
    let mut sent = 0;
    let mut portion = FIRST_PORTION;
    while sent < commits.len() {
        let end = (sent + portion).min(commits.len());
        let Some(last) = commits.get(end - 1) else {
            break;
        };
        let refspec = format!("{last}:refs/heads/{BRANCH}");
        match git::run_capturing(
            git::command(store, &["push", "--quiet", REMOTE, &refspec]),
            TIMEOUT,
        )? {
            Ok(_) => sent = end,
            Err(said) if said.contains(PACK_TOO_LARGE) && portion > 1 => portion /= 2,
            Err(said) => {
                return Err(format!(
                    "push to {REMOTE} in portions stopped after {sent} of {} commit(s): {said}",
                    commits.len()
                ));
            }
        }
    }
    Ok(true)
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
    let kept = crate::held::Kept::read(&engine_dir_of(store));
    let mut projected = Vec::new();
    for project in projects.filter_map(Result::ok) {
        let journal = project.path().join(crate::memory::JOURNAL_FILE);
        if !journal.exists() {
            continue;
        }
        let memory_dir = project.path().join("memory");
        let synced = crate::memory::sync(&memory_dir, &journal, stamp, machine_id, &kept)?;
        if !synced.written.is_empty() || !synced.removed.is_empty() || synced.imported > 0 {
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
fn reconcile_links(machine: &Machine<'_>) -> Result<Reconciled, String> {
    let &Machine {
        store,
        config_dir,
        machine_id,
        roots,
        ..
    } = machine;
    let mut outcome = Reconciled::default();
    let own = links_file::read(store, machine_id);

    for (other, record) in links_file::read_all(store) {
        if other == machine_id {
            continue;
        }
        // A path this machine cannot translate names a directory it does not have.
        let Ok(local_cwd) = roots.to_local(&record.cwd) else {
            continue;
        };
        if !Path::new(&local_cwd).is_dir() {
            continue;
        }
        // A project of another store is that store's run to link
        if !routed_here(machine, &local_cwd) {
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
        crate::dir_link::create(&target, &link)?;
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

/// One round of Desktop's cards: publish, bring in, repair, fold. In that order — see each step.
fn exchange_cards(
    desktop: &Path,
    store: &Path,
    machine_id: &str,
    roots: &Roots,
    stamp: &str,
    result: &mut Ticked,
) {
    match crate::desktop_store::publish(desktop, store, machine_id, roots) {
        Ok(cards) => result.cards_out = cards.exported.len(),
        Err(problem) => result.problems.push(problem),
    }
    let confirmed = confirmed_directories(store, machine_id);
    match crate::desktop_store::import(desktop, store, machine_id, roots, &|id| {
        confirmed_transcript(&confirmed, id)
    }) {
        Ok(cards) => result.cards_in = cards.imported.len(),
        Err(problem) => result.problems.push(problem),
    }
    // After the exchange, not before: a card repaired from a shadow the same tick just published
    // is repaired from the freshest version this machine has.
    match crate::desktop_store::repair(desktop, store, machine_id, &|id| {
        confirmed_transcript(&confirmed, id)
    }) {
        Ok(cards) => result.cards_repaired = cards.len(),
        Err(problem) => result.problems.push(problem),
    }
    // Last: a copy folded before the exchange would take its knowledge out of reach of the repair
    // above, and a copy folded after it has already given everything it had.
    match crate::desktop_store::fold_conflict_copies(desktop, &engine_dir_of(store), stamp, &|id| {
        confirmed_transcript(&confirmed, id)
    }) {
        Ok(folded) => {
            result.cards_repaired += folded.repaired.len();
            result.cards_folded = folded.set_aside.len() + folded.adopted.len();
        }
        Err(problem) => result.problems.push(problem),
    }
}

/// The directories this machine has actually proven, from its own `links.json`.
///
/// What a session proves is the *link*, not one file behind it: it hands the engine a
/// `transcript_path`, and the directory that path names is reachable here from that moment on.
/// Answering only for the single transcript that did the proving would make the whole mechanism
/// useless — a card of a session that ended months ago can never be proven again, and cards are
/// exactly what needs an answer about old sessions.
fn confirmed_directories(store: &Path, machine_id: &str) -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = crate::links_file::read(store, machine_id)
        .links
        .into_iter()
        .filter_map(|record| {
            let path = record.confirmed_by?;
            Path::new(&path).parent().map(Path::to_path_buf)
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// The path of a transcript inside a proven directory, when the file is really there.
///
/// The existence check is the point: a directory proven a week ago says nothing about a file that
/// was never in it, and a card handed a path that resolves to nothing buys the next resume miss.
fn confirmed_transcript(dirs: &[std::path::PathBuf], id: &str) -> Option<String> {
    dirs.iter()
        .map(|dir| dir.join(format!("{id}.jsonl")))
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}

/// How long a paused store waits before it asks its host again.
pub const PAUSE_RECHECK: Duration = Duration::from_hours(1);

/// How long a mark without a process may go unrenewed before this machine stops claiming the
/// session is live.
///
/// Only marks written before they carried the agent's process fall back on it: a hook renews
/// the mark on every stop and at the end, so a session unheard-of for this long ended in a way
/// that ran no hook. A mark with a process follows the process instead.
pub const HEARTBEAT_STALE_AFTER: Duration = Duration::from_hours(1);

/// Drops this machine's own marks of sessions that are over.
///
/// A mark with the agent's process is over the moment that process is gone — a session closed,
/// archived in Desktop or killed runs no hook, and waiting an hour for it locked its project for
/// that hour. While the process runs, the session is live however long it keeps quiet: a night
/// with the session open is not its end. A mark without a process falls back on `cutoff`.
///
/// Only its own. Another machine's `live.json` is that machine's to correct, and deciding from
/// here that somebody else is dead is exactly the mistake that ends with two machines writing one
/// file — their clock is not ours, and their silence may be a closed lid.
///
/// `cutoff` is a stamp from this machine's clock; comparison is textual, which is exact for
/// ISO-8601 in UTC and needs no date arithmetic.
fn clear_ended_sessions(
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
        .filter(|(_, session)| match &session.process {
            Some(process) => !process.is_alive(),
            None => session.at.as_str() < cutoff,
        })
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

/// Copies this machine's live files into its outbox, holding back what holds an agent token.
fn publish_outbox(
    config_dir: &Path,
    store: &Path,
    machine_id: &str,
    roots: &Roots,
    stamp: &str,
) -> Result<crate::outbox::Moved, String> {
    let engine_dir = engine_dir_of(store);
    let kept = crate::held::Kept::read(&engine_dir);
    let holding = crate::outbox::Holding {
        engine_dir: &engine_dir,
        kept: &kept,
        stamp,
    };
    crate::outbox::publish(config_dir, store, machine_id, roots, holding)
}

/// Commits this machine's outbox: `links.json`, `live.json`, `tails.json`, the cards, the
/// tombstones. The hooks write these files but do not commit them — a hook has ten seconds and
/// no business running git commits — so until the tick does, nothing this machine says reaches
/// the others. Only its own directory is staged, file by file and screened like everything else
/// (`crate::held::stage`): a Desktop card copies prompts and titles, and a token pasted there stays
/// on this machine too.
fn commit_own_outbox(
    store: &Path,
    engine_dir: &Path,
    machine_id: &str,
    stamp: &str,
) -> Result<usize, String> {
    let own = format!("machines/{machine_id}");
    if !store.join(&own).is_dir() {
        return Ok(0);
    }
    let changed = git::changed_paths(store, &own, TIMEOUT)?;
    let changed = crate::held::stage(engine_dir, store, &changed, stamp, TIMEOUT)?;
    if changed.is_empty() {
        return Ok(0);
    }
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
fn commit_shared_files(
    store: &Path,
    engine_dir: &Path,
    live: &BTreeSet<String>,
    stamp: &str,
    personal: bool,
) -> Result<usize, String> {
    // `config/` too in the personal store: the shared skills and managed copies live there, and
    // nothing else commits them. A skill rewritten on this machine stayed uncommitted for a day
    // because this step looked only at `projects/`. A team's store carries sessions and memory
    // alone — its host refuses `config/` — so there the engine does not even try.
    let mut changed = git::changed_paths(store, "projects", TIMEOUT)?;
    if personal {
        changed.extend(git::changed_paths(store, "config", TIMEOUT)?);
    }
    let candidates: Vec<String> = changed
        .into_iter()
        .filter(|path| !belongs_to_live_session(path, live))
        .collect();
    // A file that holds an agent token stays on this machine (`crate::held`); the rest goes in
    // with the very bytes that were checked.
    let ours = crate::held::stage(engine_dir, store, &candidates, stamp, TIMEOUT)?;
    if ours.is_empty() {
        return Ok(0);
    }
    let message = format!("vibememory: {} shared file(s) at {stamp}", ours.len());
    git::run_with_timeout(
        git::command(store, &["commit", "--quiet", "-m", &message]),
        TIMEOUT,
    )?
    .ok_or_else(|| "git refused to commit shared files".to_owned())?;
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
    machine: &Machine<'_>,
    stamp: &str,
) -> (Vec<String>, Vec<IgnoredDirectory>) {
    let &Machine {
        store,
        config_dir,
        roots,
        naming,
        ..
    } = machine;
    let engine_dir = &engine_dir_of(store);
    let mut imported = Vec::new();
    let mut ignored = Vec::new();
    let Ok(entries) = std::fs::read_dir(config_dir.join("projects")) else {
        return (imported, ignored);
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
        // taken in only by the store its working directory is routed to
        if !routed_here(machine, &cwd) {
            continue;
        }
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
        let name = match resolved {
            Ok(vibememory_core::naming::Resolution::Named { name, .. }) => name,
            // Ignored is not a failure and not the tick's to overturn — `ignoreCwd` is the
            // owner's own rule. It is only reported, so that "these transcripts stay on this
            // disk" is something a person can find out.
            Ok(vibememory_core::naming::Resolution::Ignored { reason }) => {
                ignored.push(IgnoredDirectory {
                    reason: describe_ignore(&reason),
                    transcripts: transcripts_in(&path),
                    enc,
                });
                continue;
            }
            Err(_) => continue, // the rules could not name it; the hook already said so
        };
        // A refusal means a live session somewhere, or a path this build does not handle: the
        // hook already said so, and the next tick tries again.
        if let Ok(outcome) = crate::relink::import_real_directory(
            config_dir,
            store,
            &enc,
            name.as_str(),
            &portable,
            engine_dir,
            stamp,
        ) {
            // What a team's store takes in from a directory that was already here is the
            // sessions from before the project went to the team: they stay on this machine
            if machine.team.is_some() {
                let local: Vec<String> = outcome
                    .copied
                    .iter()
                    .map(|file| format!("projects/{}/{file}", name.as_str()))
                    .collect();
                if let Err(problem) = crate::local_only::keep(store, &local) {
                    ignored.push(IgnoredDirectory {
                        reason: format!("its old sessions could not be kept local: {problem}"),
                        transcripts: outcome.copied.len(),
                        enc: enc.clone(),
                    });
                }
            }
            imported.push(format!(
                "{enc} -> projects/{} ({} file(s))",
                name.as_str(),
                outcome.copied.len()
            ));
        }
    }
    (imported, ignored)
}

/// The rule's own words for why a directory is left alone.
///
/// Public because both reasons have to be named, and the environment variable behind the second
/// one cannot be set from a test — the workspace forbids `unsafe`, and `set_var` is unsafe. A
/// pure function is the honest thing to gate anyway.
#[must_use]
pub fn describe_ignore(reason: &vibememory_core::naming::IgnoreReason) -> String {
    use vibememory_core::naming::IgnoreReason;

    match reason {
        IgnoreReason::Pattern(pattern) => format!("ignoreCwd matches {pattern}"),
        IgnoreReason::ProjectDirName(name) => {
            format!("CLAUDE_CODE_PROJECT_DIR_NAME is {name}, shared by every working directory")
        }
    }
}

/// How many transcripts a directory holds, counted so the report can name the cost.
fn transcripts_in(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .count()
    })
}

/// The working directory of a link, proven rather than guessed.
///
/// A store directory gathers the transcripts of every working directory that resolves to the
/// same name — `VibeDub`, `VibeDub/server`, `VibeDub/web` — and of every machine, including
/// Windows ones. Taking "the newest transcript's cwd" therefore produces a path that belongs to
/// a different directory or a different machine: the first live run recorded `d:\Projects\…`
/// as this Mac's working directory, and one cwd for three different links.
///
/// `enc` cannot be inverted, but it can be **recomputed**: the right cwd is the one that encodes
/// to exactly this `enc`. Nothing else is accepted.
fn working_directory_for(dir: &Path, enc: &str) -> Option<String> {
    working_directories_in(dir)
        .into_iter()
        .find(|cwd| encodes_to(cwd, enc))
}

/// Whether a working directory encodes to this `enc`.
///
/// Both forms are tried: the path as the session wrote it, and its physical form. A machine
/// where `~/Projects` is a symlink to another volume produced `enc` from one of them and writes
/// the other into its transcripts — checking a single form would call a correct old record wrong.
fn encodes_to(cwd: &str, enc: &str) -> bool {
    let syntax = vibememory_core::naming::PathSyntax::Posix;
    let physical =
        std::fs::canonicalize(cwd).map_or_else(|_| cwd.to_owned(), |p| p.display().to_string());
    [cwd.to_owned(), physical].into_iter().any(|candidate| {
        let canonical = vibememory_core::naming::canonical_cwd(&candidate, syntax);
        vibememory_core::naming::encode_cwd(&canonical).is_ok_and(|e| e.as_str() == enc)
    })
}

/// Every distinct `cwd` the transcripts of a directory mention, newest file first.
fn working_directories_in(dir: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    let mut transcripts: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
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
                let cwd = cwd.to_owned();
                if !found.contains(&cwd) {
                    found.push(cwd);
                }
                break;
            }
        }
    }
    found
}

/// The working directory a project directory's transcripts were written in, for a directory
/// whose `enc` is not in question — the real `projects/<enc>` the CLI itself created.
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

/// Writes into `links.json` every link of this machine that is not there yet.
///
/// A record is how another machine learns that `projects/<name>` belongs to a working directory
/// it may also have. The hook records the links it creates, but `switch` and `import` create
/// them too — and a link nobody recorded teaches the other machines nothing. So the tick walks
/// what is actually on disk and fills the gaps; it is the one place that sees the whole picture.
///
/// The working directory comes from the transcripts in the store, the same way the tick names a
/// real directory: `enc` cannot be inverted, but every record carries the `cwd` it was written in.
fn record_local_links(store: &Path, config_dir: &Path, machine_id: &str, roots: &Roots) -> usize {
    // A record whose cwd does not encode back to its own `enc` was written from a guess — by an
    // earlier build of this function, or from a transcript of another machine. It is not trusted
    // and is derived again.
    let recorded = crate::links_file::read(store, machine_id).links;
    let mut known: BTreeSet<String> = BTreeSet::new();
    let mut untrue: Vec<String> = Vec::new();
    for record in recorded {
        let local = roots
            .to_local(&record.cwd)
            .unwrap_or_else(|_| record.cwd.clone());
        if encodes_to(&local, &record.enc) {
            known.insert(record.enc);
        } else {
            untrue.push(record.enc);
        }
    }
    let Ok(entries) = std::fs::read_dir(config_dir.join("projects")) else {
        return 0;
    };
    let mut recorded = 0;
    for entry in entries.filter_map(Result::ok) {
        let enc = entry.file_name().to_string_lossy().into_owned();
        if known.contains(&enc) {
            continue;
        }
        let link = entry.path();
        let Ok(target) = std::fs::read_link(&link) else {
            continue; // a real directory: the import step deals with it
        };
        // Only links into this store: one pointing elsewhere is not ours to describe.
        let Ok(relative) = target.strip_prefix(store.join("projects")) else {
            continue;
        };
        let Some(name) = relative.components().next() else {
            continue;
        };
        let name = name.as_os_str().to_string_lossy().into_owned();
        let Some(cwd) = working_directory_for(&target, &enc) else {
            // Nothing in the store proves which directory this link belongs to: the next
            // session's hook records it with certainty, and a guess is worse than silence.
            continue;
        };
        // Re-derived: whatever the old record said about this link is superseded below.
        untrue.retain(|wrong| *wrong != enc);
        let portable = roots.to_portable(&cwd).unwrap_or_else(|_| cwd.clone());
        // `reconciled`, not `observed`: this machine did not see the session that made the link,
        // it only sees the link. An observation by a real session outranks it later.
        if crate::links_file::record(
            store,
            machine_id,
            &crate::links_file::Observation {
                enc: &enc,
                name: &name,
                cwd: &portable,
                syntax: vibememory_core::naming::PathSyntax::Posix,
                source: vibememory_core::links::LinkSource::Reconciled,
                confirmed_by: None,
            },
        )
        .is_ok()
        {
            recorded += 1;
        }
    }

    // What could be neither confirmed nor re-derived is removed rather than left standing: a
    // record naming the wrong directory is not a gap in the map, it is a wrong turn on it, and
    // the other machine's reconciler follows it.
    for enc in untrue {
        let _ = crate::links_file::forget(store, machine_id, &enc);
    }
    recorded
}

/// The engine directory that owns this store: the store always lives at `<engine>/store`, so its
/// parent is the answer, and the tick does not need it threaded through every call.
fn engine_dir_of(store: &Path) -> std::path::PathBuf {
    store
        .parent()
        .map_or_else(|| store.to_path_buf(), Path::to_path_buf)
}
