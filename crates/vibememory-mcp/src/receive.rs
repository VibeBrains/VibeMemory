//! What `pre-receive` decides for a push to a team store.
//!
//! Pure: the key that pushes, the team, the update lines, the paths the push changes on `main` and
//! the sizes in; the first rule broken out, with its code from `docs/manuals/hostShellSpec.md` and
//! the paths when the rule is about paths. One broken line refuses the whole push: git applies a
//! push only when the hook lets every line through.

use crate::access::{Mode, Snapshot};

/// The only reference a push may move.
pub const MAIN: &str = "refs/heads/main";
/// Where a sync team's members write their projects.
const PROJECTS_PREFIX: &str = "projects/";
/// Where the files of one machine live: `machines/<storeName>/`.
const MACHINES_PREFIX: &str = "machines/";
/// The store's configuration, which no member may push.
const CONFIG_PREFIX: &str = "config/";

/// One line of `pre-receive`'s input: the reference moves from `old` to `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    /// The object the reference points at now; all zeros when it does not exist yet.
    pub old: String,
    /// The object it is to point at; all zeros when the push deletes it.
    pub new: String,
    /// The full reference name, `refs/heads/main`.
    pub name: String,
}

impl Update {
    /// Whether the push deletes the reference.
    #[must_use]
    pub fn deletes(&self) -> bool {
        is_zero(&self.new)
    }
}

/// Whether an object id is git's "no object": all zeros, of either hash length.
#[must_use]
pub fn is_zero(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte == b'0')
}

/// Reads the lines git gives `pre-receive` on stdin: `<old> <new> <ref>`, one per reference.
///
/// # Errors
///
/// A line that is not three fields: git never sends one, so the push is refused rather than
/// guessed at.
pub fn parse_updates(input: &str) -> Result<Vec<Update>, String> {
    input
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split(' ');
            match (fields.next(), fields.next(), fields.next(), fields.next()) {
                (Some(old), Some(new), Some(name), None)
                    if !old.is_empty() && !new.is_empty() && !name.is_empty() =>
                {
                    Ok(Update {
                        old: old.to_owned(),
                        new: new.to_owned(),
                        name: name.to_owned(),
                    })
                }
                _ => Err(format!(
                    "an update line is not `<old> <new> <ref>`: {line:?}"
                )),
            }
        })
        .collect()
}

/// The update of `main` whose changed paths the hook has to list: present, and not a deletion.
#[must_use]
pub fn main_update(updates: &[Update]) -> Option<&Update> {
    updates
        .iter()
        .find(|update| update.name == MAIN && !update.deletes())
}

/// Sizes the push is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    /// The repository on disk before the push, without the incoming objects.
    pub repo_bytes: u64,
    /// The objects the push brings.
    pub incoming_bytes: u64,
    /// Free space on the partition of the team stores.
    pub free_bytes: u64,
    /// Free space the host keeps for everyone: a push may not take it.
    pub reserve_bytes: u64,
}

/// A push, as the hook sees it.
#[derive(Debug, Clone, Copy)]
pub struct Push<'a> {
    /// The machine key that pushes, from `VIBEMEMORY_KEY`; `None` when the push did not come
    /// through the forced command.
    pub key: Option<&'a str>,
    /// The team whose store receives the push.
    pub team: &'a str,
    /// Every reference the push moves.
    pub updates: &'a [Update],
    /// The paths the push changes on `main`, from `git diff-tree` of the old and the new tree.
    pub changed: &'a [String],
    /// The sizes.
    pub sizes: Sizes,
    /// When the push comes, `YYYY-MM-DDTHH:MM:SSZ`: a banned member's key pushes nothing.
    pub now: &'a str,
    /// The changed paths whose new content holds an agent token of a cabinet.
    pub with_tokens: &'a [String],
}

/// Why the push is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRefusal {
    /// Stable camelCase code from the spec.
    pub code: &'static str,
    /// What exactly, for a person.
    pub detail: String,
    /// The paths the code is about, sorted; empty for a code that is not about paths.
    pub paths: Vec<String>,
}

fn refuse(code: &'static str, detail: impl Into<String>) -> PushRefusal {
    PushRefusal {
        code,
        detail: detail.into(),
        paths: Vec::new(),
    }
}

fn refuse_paths(code: &'static str, detail: &str, mut paths: Vec<String>) -> PushRefusal {
    paths.sort();
    paths.dedup();
    PushRefusal {
        code,
        detail: detail.to_owned(),
        paths,
    }
}

/// Decides whether `push` may land. `snapshot` is `None` when the access snapshot cannot be used:
/// then nothing lands. The key of a banned member is refused as one the snapshot does not have.
///
/// # Errors
///
/// The first rule the push breaks, in the order of the spec's table.
pub fn decide(push: &Push<'_>, snapshot: Option<&Snapshot>) -> Result<(), PushRefusal> {
    let Some(snapshot) = snapshot else {
        return Err(refuse(
            "snapshotUnusable",
            "the host cannot read who may push; nothing lands until it can",
        ));
    };
    let Some(key) = push
        .key
        .and_then(|id| snapshot.key(id))
        .filter(|key| !snapshot.barred(&key.member, push.now))
    else {
        return Err(refuse(
            "unknownKey",
            "the push did not come through a machine key of the snapshot",
        ));
    };
    let Some(team) = snapshot
        .teams
        .get(push.team)
        .filter(|team| team.deleted.is_none())
    else {
        return Err(refuse(
            "teamGone",
            format!("there is no team {}", push.team),
        ));
    };
    if !key.teams.iter().any(|slug| slug == push.team) || !team.members.contains_key(&key.member) {
        return Err(refuse(
            "notAMember",
            format!("key {} does not open team {}", key.id, push.team),
        ));
    }
    if team.mode != Some(Mode::Sync) {
        return Err(refuse(
            "pushDenied",
            format!(
                "{} is a memory team: only the memory server writes its store",
                push.team
            ),
        ));
    }
    if !team.writable {
        return Err(refuse(
            "readOnly",
            format!("{} is read-only: its grant is over", push.team),
        ));
    }
    if let Some(update) = push
        .updates
        .iter()
        .find(|update| update.name != MAIN || update.deletes())
    {
        return Err(refuse(
            "refDenied",
            format!(
                "only {MAIN} is pushed, and it is never deleted: {}",
                update.name
            ),
        ));
    }
    paths_allowed(push.changed, &key.store_name)?;
    if !push.with_tokens.is_empty() {
        // Engines hold such files back themselves; this is for one that does not, and for a
        // plain `git push` — a token in a team's store is in front of every member for good.
        return Err(refuse_paths(
            "tokenInPush",
            "these files hold an agent token; revoke it in the cabinet and push without it",
            push.with_tokens.to_vec(),
        ));
    }
    sizes_allowed(push.sizes, team.limits.quota_bytes)
}

/// The paths a member may change: `projects/` and the directory of their own machine — never the
/// store's configuration, which would run on every member's machine.
fn paths_allowed(changed: &[String], store_name: &str) -> Result<(), PushRefusal> {
    let config: Vec<String> = changed
        .iter()
        .filter(|path| path.starts_with(CONFIG_PREFIX))
        .cloned()
        .collect();
    if !config.is_empty() {
        return Err(refuse_paths(
            "configDenied",
            "the store's configuration is not pushed by members",
            config,
        ));
    }
    let own_machine = format!("{MACHINES_PREFIX}{store_name}/");
    let outside: Vec<String> = changed
        .iter()
        .filter(|path| !path.starts_with(PROJECTS_PREFIX) && !path.starts_with(&own_machine))
        .cloned()
        .collect();
    if !outside.is_empty() {
        return Err(refuse_paths(
            "pathDenied",
            "a member pushes projects/ and the machines/ directory of their own key",
            outside,
        ));
    }
    Ok(())
}

/// The room a push may take: the host keeps its reserve free, and the store stays within the
/// team's quota.
fn sizes_allowed(sizes: Sizes, quota: Option<u64>) -> Result<(), PushRefusal> {
    if sizes.free_bytes.saturating_sub(sizes.incoming_bytes) < sizes.reserve_bytes {
        return Err(refuse(
            "diskReserve",
            format!(
                "the host would keep less than {} bytes free",
                sizes.reserve_bytes
            ),
        ));
    }
    if let Some(quota) = quota
        && sizes.repo_bytes.saturating_add(sizes.incoming_bytes) > quota
    {
        return Err(refuse(
            "quota",
            format!(
                "the store would take {} bytes of the team's {quota}",
                sizes.repo_bytes.saturating_add(sizes.incoming_bytes)
            ),
        ));
    }
    Ok(())
}
