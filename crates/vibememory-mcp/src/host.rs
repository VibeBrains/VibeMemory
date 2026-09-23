//! The memory server on the store's host: who may come in is read from the access snapshot, and
//! every admitted request reads and writes its team's repository in its own name.
//!
//! The snapshot is re-read when its file changes — a revocation takes effect with the next
//! request, without a restart — and trusted only while it passes every rule: a file that vanished
//! or broke may have been a revocation, so the door stays shut until it reads cleanly again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use vibememory_core::memory::journal::{Event, Memory};

use crate::access::{self, Snapshot, TokenRole};
use crate::git_memories::GitMemories;
use crate::http::{Admission, Door, Grant, Visit};
use crate::memories::{DirectoryProject, Memories, TranscriptRef};
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

/// The snapshot as last read, or why it cannot be used.
struct Loaded {
    stamp: Option<Stamp>,
    state: Result<Arc<Snapshot>, String>,
}

/// The server's host: the snapshot, where the teams' repositories are, and one write lock per
/// repository.
pub struct Host {
    access: PathBuf,
    teams: PathBuf,
    cabinet: Option<String>,
    loaded: Mutex<Loaded>,
    locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

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

/// One line about a snapshot that is in force.
fn summary(snapshot: &Snapshot) -> String {
    format!(
        "{} teams, {} tokens, {} machine keys",
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
        let (stamp, state) = read(&access);
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
                stamp,
                state: Ok(snapshot),
            }),
            locks: Mutex::new(HashMap::new()),
        })
    }

    /// The snapshot in force: one `stat` per request, and a new read only when the file changed.
    fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
        let now = std::fs::metadata(&self.access)
            .ok()
            .map(|metadata| Stamp::of(&metadata));
        let mut loaded = held(&self.loaded);
        if now.is_none() || now != loaded.stamp {
            let (stamp, state) = read(&self.access);
            match (&loaded.state, &state) {
                (_, Ok(snapshot)) => eprintln!(
                    "vibememory-mcp: access snapshot re-read: {}",
                    summary(snapshot)
                ),
                (Ok(_), Err(why)) => eprintln!(
                    "vibememory-mcp: REFUSING EVERYONE — the access snapshot cannot be used: \
                     {why}; nobody is let in until it reads cleanly again"
                ),
                (Err(_), Err(_)) => {}
            }
            *loaded = Loaded { stamp, state };
        }
        loaded.state.clone()
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
            },
            cabinet: self.cabinet.clone(),
        };
        Admission::Granted(Visit {
            grant,
            memories: Box::new(memories),
        })
    }
}

/// One team's memory, as a request of one token sees it.
struct TeamMemories {
    git: GitMemories,
    /// The projects of a `memory` team, as the snapshot lists them: its repository is empty until
    /// the first write, and a project exists because the cabinet says so. `None` for a team whose
    /// projects are what its repository holds.
    declared: Option<Vec<String>>,
    lock: Arc<Mutex<()>>,
    repo: PathBuf,
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

    fn read_transcript(&self, project: &str, session: &str) -> Result<Vec<u8>, String> {
        self.git.read_transcript(project, session)
    }

    fn new_version(&self, id: &str) -> String {
        self.git.new_version(id)
    }

    fn now(&self) -> String {
        self.git.now()
    }

    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String> {
        self.git.project_of_directory(directory)
    }
}
