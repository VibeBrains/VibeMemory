//! The memory server on the store's host: who may come in is read from the access snapshot, and
//! every admitted request reads and writes its team's repository in its own name.
//!
//! The snapshot is re-read when its file changes — a revocation takes effect with the next
//! request, without a restart — and trusted only while it passes every rule: a file that vanished
//! or broke may have been a revocation, so the door stays shut until it reads cleanly again. A file
//! that reads but is older than what is in force — a lower serial than the snapshot loaded, or than
//! the copy the host applied last — is not taken: the host never goes back.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use vibememory_core::memory::journal::{Event, Memory};

use crate::access::{self, Snapshot, TokenRole};
use crate::git_memories::GitMemories;
use crate::http::{Admission, Door, Grant, Visit};
use crate::layout;
use crate::memories::{DirectoryProject, Handoffs, Memories, TranscriptRef};
use crate::tools::{Limits, Writes};

/// What versions written over HTTPS are signed with, before the token's id: the host wrote them,
/// on behalf of that token.
const HOST_WRITER_PREFIX: &str = "host-";

/// The identity of the snapshot file as it was read: a cabinet replaces it by renaming a new file
/// over it (a new inode), a person editing it by hand changes its time and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    size: u64,
    inode: u64,
}

impl Stamp {
    fn of(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(metadata);
        #[cfg(not(unix))]
        let inode = 0;
        Self {
            modified: metadata.modified().ok(),
            size: metadata.len(),
            inode,
        }
    }
}

/// The stamps of `access.json` and of the copy of the snapshot applied last; `None` for a file that
/// is not there.
type Stamps = (Option<Stamp>, Option<Stamp>);

/// The snapshot as last read, or why it cannot be used, with the stamps of the files it was chosen
/// from and the highest serial ever taken: a broken file in between does not make the server forget
/// how far it has come.
struct Loaded {
    stamps: Stamps,
    state: Result<Arc<Snapshot>, String>,
    serial: u64,
}

/// The server's host: the snapshot, where the teams' repositories are, and one write lock per
/// repository.
pub struct Host {
    access: PathBuf,
    teams: PathBuf,
    cabinet: Option<String>,
    loaded: Mutex<Loaded>,
    locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
    /// When each team's activity file was last touched by this process
    touched: Mutex<HashMap<String, std::time::Instant>>,
}

/// A team's activity file is touched at most this often: the report reads it hourly, and a file
/// written on every request would be a write on every read.
const TOUCH_EVERY: std::time::Duration = std::time::Duration::from_mins(10);

/// A lock whose holder panicked still guards consistent data here: the snapshot is replaced
/// whole, and a repository is moved only by a compare-and-swap of `main`.
fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads and checks the snapshot, stamped with the file it came from: the stamp is taken from the
/// open file, so it always belongs to the bytes that were read.
fn read(path: &Path) -> (Option<Stamp>, Result<Arc<Snapshot>, String>) {
    use std::io::Read as _;

    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return (None, Err(format!("{}: {error}", path.display()))),
    };
    let stamp = file.metadata().ok().map(|metadata| Stamp::of(&metadata));
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        return (stamp, Err(format!("{}: {error}", path.display())));
    }
    let state = access::check(&bytes)
        .map(Arc::new)
        .map_err(|refusal| format!("{}: {}", refusal.code, refusal.detail));
    (stamp, state)
}

/// The snapshot in force: `access.json` when it reads, unless it is not newer than the copy of
/// what the host applied last (`access::in_force`). A copy that is there and does not read is
/// passed over, and the journal says so: the file is checked either way.
fn read_in_force(access: &Path) -> (Stamps, Result<Arc<Snapshot>, String>) {
    let (file_stamp, file) = read(access);
    let copy_path = layout::applied_snapshot_file(access);
    let (copy_stamp, copy) = read(&copy_path);
    let copy = match copy {
        Ok(copy) => Some(copy),
        Err(why) => {
            if copy_path.symlink_metadata().is_ok() {
                eprintln!("vibememory-mcp: the copy of the applied snapshot is passed over: {why}");
            }
            None
        }
    };
    let state = file.map(|file| access::in_force(file, copy));
    ((file_stamp, copy_stamp), state)
}

/// The stamps of the file and of the applied copy as they are now.
fn stamps_now(access: &Path) -> Stamps {
    let stamp = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| Stamp::of(&metadata))
    };
    (stamp(access), stamp(&layout::applied_snapshot_file(access)))
}

/// One line about a snapshot that is in force.
fn summary(snapshot: &Snapshot) -> String {
    format!(
        "serial {}, {} teams, {} tokens, {} machine keys",
        snapshot.serial,
        snapshot.teams.len(),
        snapshot.tokens.len(),
        snapshot.keys.len()
    )
}

impl Host {
    /// Opens the snapshot at `access`; the teams' repositories live in `teams`.
    ///
    /// # Errors
    ///
    /// A snapshot that cannot be read or breaks a rule. A server that cannot tell who may come
    /// in does not start: starting would mean guessing.
    pub fn open(access: PathBuf, teams: PathBuf, cabinet: Option<String>) -> Result<Self, String> {
        let (stamps, state) = read_in_force(&access);
        let snapshot = state?;
        eprintln!(
            "vibememory-mcp: access snapshot {}: {}",
            access.display(),
            summary(&snapshot)
        );
        Ok(Self {
            access,
            teams,
            cabinet,
            loaded: Mutex::new(Loaded {
                stamps,
                serial: snapshot.serial,
                state: Ok(snapshot),
            }),
            locks: Mutex::new(HashMap::new()),
            touched: Mutex::new(HashMap::new()),
        })
    }

    /// The snapshot in force: two `stat`s per request, and a new read only when a file changed.
    fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
        let now = stamps_now(&self.access);
        let mut loaded = held(&self.loaded);
        if now.0.is_none() || now != loaded.stamps {
            let (stamps, state) = read_in_force(&self.access);
            loaded.stamps = stamps;
            match state {
                // An older decision neither replaces a newer one nor reopens a door a broken file
                // shut.
                Ok(read) if read.serial < loaded.serial => eprintln!(
                    "vibememory-mcp: access snapshot of serial {} is older than serial {} taken; \
                     it is not taken",
                    read.serial, loaded.serial
                ),
                Ok(read) => {
                    eprintln!(
                        "vibememory-mcp: access snapshot re-read: {}",
                        summary(&read)
                    );
                    loaded.serial = read.serial;
                    loaded.state = Ok(read);
                }
                Err(why) => {
                    if loaded.state.is_ok() {
                        eprintln!(
                            "vibememory-mcp: REFUSING EVERYONE — the access snapshot cannot be used: \
                             {why}; nobody is let in until it reads cleanly again"
                        );
                    }
                    loaded.state = Err(why);
                }
            }
        }
        loaded.state.clone()
    }

    /// Notes that a team was reached: its activity file's time is when. Best effort — a file that
    /// cannot be written costs only the report's answer, never the request.
    fn touch(&self, slug: &str) {
        let now = std::time::Instant::now();
        {
            let mut touched = held(&self.touched);
            if touched
                .get(slug)
                .is_some_and(|last| now.duration_since(*last) < TOUCH_EVERY)
            {
                return;
            }
            touched.insert(slug.to_owned(), now);
        }
        let file = layout::activity_dir(&self.teams).join(slug);
        // the write truncates the file, and the truncation moves its time: that time is the answer
        if let Err(error) = std::fs::write(&file, b"") {
            eprintln!("vibememory-mcp: {}: {error}", file.display());
        }
    }

    /// The write lock of one repository: writes of one team go one after another, so none of
    /// them spends its attempts on a `main` another request of this server just moved.
    fn lock_of(&self, repo: &Path) -> Arc<Mutex<()>> {
        Arc::clone(held(&self.locks).entry(repo.to_path_buf()).or_default())
    }
}

impl Door for Host {
    fn admit(&self, presented: &str) -> Admission<'_> {
        let Ok(snapshot) = self.snapshot() else {
            return Admission::Closed;
        };
        let token = match snapshot.admit(presented, &vibememory_cli::clock::now()) {
            access::Admission::Unknown => return Admission::Unknown,
            access::Admission::Expired(token) => return Admission::Expired(token.id.clone()),
            access::Admission::Granted(token) => token,
        };
        // A checked snapshot names only teams it holds; the lookup cannot miss.
        let Some(team) = snapshot.teams.get(&token.team) else {
            return Admission::Unknown;
        };
        self.touch(&token.team);
        let writes = if token.role == TokenRole::Reader {
            Writes::ReaderToken
        } else if team.writable {
            Writes::Allowed
        } else {
            Writes::ReadOnlyTeam
        };
        let repo = team.repository(&token.team, &self.teams);
        let memories = TeamMemories {
            git: GitMemories::new(repo.clone(), format!("{HOST_WRITER_PREFIX}{}", token.id)),
            declared: team.projects.clone(),
            lock: self.lock_of(&repo),
            repo,
        };
        let grant = Grant {
            token: token.id.clone(),
            team: token.team.clone(),
            member: token.member.clone(),
            agent: token.agent.clone(),
            writes,
            history: token.history,
            scope: token.projects.clone(),
            limits: Limits {
                max_records: team.limits.max_records,
                max_record_bytes: team.limits.max_record_bytes,
                quota_bytes: team.limits.quota_bytes,
            },
            cabinet: self.cabinet.clone(),
        };
        Admission::Granted(Visit {
            grant,
            memories: Box::new(memories),
        })
    }

    fn outside_scope(&self, grant: &crate::http::Grant, project: &str) {
        // best effort, like the activity file: a note that cannot be kept costs only the report
        if let Err(error) = note_outside(&layout::outside_dir(&self.teams), &grant.token, project) {
            eprintln!("vibememory-mcp: outside {}: {error}", grant.token);
        }
    }
}

/// How many refused projects one token's note keeps: enough for a person to pick from, bounded so
/// a client asking for random names cannot grow the file.
const OUTSIDE_KEPT: usize = 20;

/// Adds `project` to the token's note of projects asked for outside its list, with when.
fn note_outside(dir: &Path, token: &str, project: &str) -> Result<(), String> {
    // the token's public id names the file: nothing else from the request becomes a path
    let named = token
        .strip_prefix("tk_")
        .is_some_and(vibememory_core::token::is_id);
    if !named || project.len() > vibememory_core::MAX_FILE_NAME_BYTES {
        return Ok(());
    }
    let path = dir.join(format!("{token}.json"));
    let mut asked: BTreeMap<String, String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    asked.insert(project.to_owned(), vibememory_cli::clock::now());
    while asked.len() > OUTSIDE_KEPT {
        let Some(oldest) = asked
            .iter()
            .min_by(|a, b| a.1.cmp(b.1))
            .map(|(name, _)| name.clone())
        else {
            break;
        };
        asked.remove(&oldest);
    }
    let text = serde_json::to_string(&asked).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

/// One team's memory, as a request of one token or the session of one machine key sees it.
pub(crate) struct TeamMemories {
    git: GitMemories,
    /// The projects of a `memory` team, as the snapshot lists them: its repository is empty until
    /// the first write, and a project exists because the cabinet says so. `None` for a team whose
    /// projects are what its repository holds.
    declared: Option<Vec<String>>,
    lock: Arc<Mutex<()>>,
    repo: PathBuf,
}

impl TeamMemories {
    /// A team's memory for a process of its own — an ssh session serves one client, so its lock
    /// only orders the session's own writes; writes of other processes meet at the
    /// compare-and-swap of `main`.
    pub(crate) fn for_session(
        repo: PathBuf,
        writer: String,
        declared: Option<Vec<String>>,
    ) -> Self {
        Self {
            git: GitMemories::new(repo.clone(), writer),
            declared,
            lock: Arc::default(),
            repo,
        }
    }
}

impl Memories for TeamMemories {
    fn projects(&self) -> Result<Vec<String>, String> {
        match &self.declared {
            Some(declared) => {
                let mut projects = declared.clone();
                projects.sort();
                Ok(projects)
            }
            None => self.git.projects(),
        }
    }

    fn load(&self, project: &str) -> Result<Memory, String> {
        self.git.load(project)
    }

    fn append(&self, project: &str, event: &Event) -> Result<(), String> {
        let _turn = self.lock.lock().unwrap_or_else(|poisoned| {
            eprintln!(
                "vibememory-mcp: a write to {} panicked earlier; writes go on — the repository \
                 only ever moved by a compare-and-swap of main",
                self.repo.display()
            );
            poisoned.into_inner()
        });
        self.git.append(project, event)
    }

    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String> {
        self.git.transcripts(project)
    }

    fn read_transcript(
        &self,
        project: &str,
        transcript: &TranscriptRef,
    ) -> Result<Vec<u8>, String> {
        self.git.read_transcript(project, transcript)
    }

    fn new_version(&self, id: &str) -> String {
        self.git.new_version(id)
    }

    fn handoffs(&self, project: &str) -> Result<Handoffs, String> {
        self.git.handoffs(project)
    }

    fn now(&self) -> String {
        self.git.now()
    }

    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String> {
        self.git.project_of_directory(directory)
    }

    fn store_bytes(&self) -> Result<u64, String> {
        self.git.store_bytes()
    }
}
