//! The seven things an agent can do to the memory, and nothing else.
//!
//! Every one of them is the journal's own vocabulary: read the folded state, or append one event.
//! There is no update-in-place and no delete-the-line, because the journal has neither — that is
//! what lets two machines merge it by union without losing a word. The one tool outside that
//! vocabulary, `project_resolve`, only reads the engine's naming rules and writes nothing.

use serde_json::{Value, json};
use vibememory_core::memory::journal::{self, Action, Event, Memory};
use vibememory_core::memory::record::{Record, RecordId, RecordKind, RecordStatus};
use vibememory_core::naming::StoreName;

use crate::memories::{DirectoryProject, Memories};

/// What the tools answer with: text for the agent, or an error it can act on.
pub type ToolResult = Result<Value, String>;

/// The tool list, as `tools/list` reports it.
#[must_use]
pub fn catalogue() -> Value {
    json!([
        {
            "name": "memory_search",
            "description": "Search remembered facts by words in their title, description or body. \
                            Returns identifiers and one-line hooks, not whole records — read one \
                            with memory_get. A result marked stale was true once and is not to be \
                            relied on without checking.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Words to look for; empty lists everything." },
                    "project": { "type": "string", "description": "Limit to one project. Omit to search all of them." },
                    "kind": { "type": "string", "enum": ["user", "feedback", "project", "reference"] },
                    "status": { "type": "string", "enum": ["active", "stale"], "description": "Omit to see both." }
                },
                "required": []
            }
        },
        {
            "name": "memory_get",
            "description": "Read one remembered fact in full, by its identifier. A fact two \
                            machines wrote at once carries `rivals`: the other versions, in full.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "project": { "type": "string", "description": "Where to look; omit to search every project." }
                },
                "required": ["id"]
            }
        },
        {
            "name": "memory_save",
            "description": "Remember a new fact. Fails if the identifier is taken — change the \
                            existing one with memory_update instead of writing a second version \
                            of the same thing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project of the store. Omit to use the one this server was started in." },
                    "id": { "type": "string", "description": "Short kebab-case slug; also the file name of its projection." },
                    "kind": { "type": "string", "enum": ["user", "feedback", "project", "reference"] },
                    "description": { "type": "string", "description": "One line: what is inside, so a reader can tell whether it is relevant." },
                    "body": { "type": "string", "description": "The memory itself, markdown. Link others with [[their-id]]." }
                },
                "required": ["id", "kind", "description", "body"]
            }
        },
        {
            "name": "memory_update",
            "description": "Change a remembered fact, or mark it stale. Writes a new version whose \
                            parent is the one you were shown, so two agents editing at once keep \
                            both versions instead of overwriting each other. To settle a disputed \
                            fact, write the merged text and pass the rival versions you merged \
                            in `merges`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project of the store. Omit to use the one this server was started in." },
                    "id": { "type": "string" },
                    "description": { "type": "string" },
                    "body": { "type": "string" },
                    "status": { "type": "string", "enum": ["active", "stale"] },
                    "merges": { "type": "array", "items": { "type": "string" }, "description": "Versions from `rivals` of memory_get whose text this update takes in; they stop being shown." }
                },
                "required": ["id"]
            }
        },
        {
            "name": "memory_delete",
            "description": "Forget a fact, with every version of it now shown. The versions stay \
                            in the journal; the record stops being shown. Prefer marking it stale when it explains why something \
                            was done.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project of the store. Omit to use the one this server was started in." },
                    "id": { "type": "string" }
                },
                "required": ["id"]
            }
        }
    ])
    .as_array_mut()
    .map(|tools| {
        tools.push(history_search_entry());
        tools.push(project_resolve_entry());
        Value::Array(std::mem::take(tools))
    })
    .unwrap_or_default()
}

/// Past sessions rather than remembered facts: a separate corpus, read the same way.
fn history_search_entry() -> Value {
    json!({
        "name": "history_search",
        "description": "Search past sessions by words said in them. Returns which session, \
                        when, and a short excerpt — never whole transcripts. Newest first, \
                        because that is the order a person asks about their own history in. \
                        Narrow with `project` when you know it: the corpus is gigabytes.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Words that must all appear in one message." },
                "project": { "type": "string", "description": "Limit to one project. Omit to search all of them." },
                "limit": { "type": "integer", "description": "How many matches to return; 20 by default, 100 at most." }
            },
            "required": ["query"]
        }
    })
}

/// The one tool that is not about memory itself: it tells an agent which project its folder is,
/// so a server shared by several windows can still be written to without a guess.
fn project_resolve_entry() -> Value {
    json!({
        "name": "project_resolve",
        "description": "Name the store project of a folder, by the same rules the engine \
                        uses. Call it when the server does not know your project — one \
                        server shared by several windows — and pass the answer as `project` \
                        to writes. Never guess the name from the folder: a repository is \
                        named by its git root, and the owner may rename or ignore folders.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "directory": { "type": "string", "description": "Absolute path of the folder you work in." }
            },
            "required": ["directory"]
        }
    })
}

/// Runs one tool by name.
///
/// # Errors
///
/// A sentence the agent can act on. An unknown tool is an error, never a guess at what was meant.
pub fn call(
    name: &str,
    arguments: &Value,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> ToolResult {
    match name {
        "memory_search" => search(arguments, caller, memories),
        "memory_get" => get(arguments, caller, memories),
        "memory_save" => save(arguments, caller, memories),
        "memory_update" => update(arguments, caller, memories),
        "memory_delete" => delete(arguments, caller, memories),
        "history_search" => history_search(arguments, caller, memories),
        "project_resolve" => project_resolve(arguments, caller, memories),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn text(arguments: &Value, field: &str) -> Result<String, String> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{field} is required"))
}

fn optional(arguments: &Value, field: &str) -> Option<String> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Whether a caller may write, and if not, why: the two refusals need different advice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writes {
    /// Writes go through.
    Allowed,
    /// The token was issued to read.
    ReaderToken,
    /// The team is read-only: its grant is over.
    ReadOnlyTeam,
}

/// Limits a team sets on its memory; `None` is no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Records of memory in one project.
    pub max_records: Option<u64>,
    /// Bytes of one version of a record, as it lands in the journal.
    pub max_record_bytes: Option<u64>,
    /// Bytes the team's store may take on disk.
    pub quota_bytes: Option<u64>,
}

/// Who calls and from where, fixed for the life of a session or of one request.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    /// Written into every record the call creates: memory is shared between agents, so it has to
    /// say who wrote it.
    pub agent: &'a str,
    /// The member of a team the call writes as; `None` for the store's owner.
    pub member: Option<&'a str>,
    /// The store project of the directory the client started the server in, when it is one. A
    /// write that names no project goes there: an agent in an IDE knows its folder, not the name
    /// the store gave it.
    pub project: Option<&'a str>,
    /// Whether the call may change memory.
    pub writes: Writes,
    /// Whether the call may search past sessions.
    pub history: bool,
    /// The projects a token was issued for; `None` is every project of the store.
    pub scope: Option<&'a [String]>,
    /// The team's limits.
    pub limits: Limits,
    /// Where the cabinet is, for refusals only the cabinet can lift.
    pub cabinet: Option<&'a str>,
}

impl<'a> Caller<'a> {
    /// The store's owner on their own store: every right, every project, no limits, no member.
    #[must_use]
    pub const fn owner(agent: &'a str, project: Option<&'a str>) -> Self {
        Self {
            agent,
            member: None,
            project,
            writes: Writes::Allowed,
            history: true,
            scope: None,
            limits: Limits {
                max_records: None,
                max_record_bytes: None,
                quota_bytes: None,
            },
            cabinet: None,
        }
    }
}

/// Refuses a change the caller has no right to, with the advice that fits the reason.
fn may_write(caller: &Caller<'_>) -> Result<(), String> {
    match caller.writes {
        Writes::Allowed => Ok(()),
        Writes::ReaderToken => Err("this token only reads: a token that writes is issued by \
                                    the team's owner or an admin in the cabinet"
            .to_owned()),
        Writes::ReadOnlyTeam => Err(caller.cabinet.map_or_else(
            || "the team is read-only: its grant is over, and the cabinet renews it".to_owned(),
            |cabinet| {
                format!(
                    "the team is read-only: its grant is over, renew it in the cabinet at {cabinet}"
                )
            },
        )),
    }
}

/// Refuses a project outside the token's list, before anything is read.
fn in_scope(project: &str, caller: &Caller<'_>) -> Result<(), String> {
    if caller
        .scope
        .is_some_and(|scope| !scope.iter().any(|allowed| allowed == project))
    {
        return Err(format!(
            "{project} is not among the projects this token opens"
        ));
    }
    Ok(())
}

/// The projects this caller may see: the store's, narrowed to the token's list when it has one.
fn visible(caller: &Caller<'_>, memories: &dyn Memories) -> Result<Vec<String>, String> {
    let mut projects = memories.projects()?;
    if let Some(scope) = caller.scope {
        projects.retain(|project| scope.contains(project));
    }
    Ok(projects)
}

/// The project a write goes to: the one it named, else the one the server was started in — and
/// only one the store already holds. A write does not create a project: on a team store a typo
/// would otherwise start a second memory nobody reads, and on disks that ignore case two such
/// names are one directory.
fn project_of(
    arguments: &Value,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> Result<String, String> {
    let project = optional(arguments, "project")
        .or_else(|| caller.project.map(str::to_owned))
        .ok_or_else(|| {
            "project is required: this server was started outside any project of the store, so \
             name the one to write to"
                .to_owned()
        })?;
    in_scope(&project, caller)?;
    let held = memories.projects()?;
    if held.contains(&project) {
        return Ok(project);
    }
    let key = StoreName::parse(&project).ok().map(|name| name.key());
    let near = held.iter().find(|other| {
        key.as_ref()
            .is_some_and(|key| StoreName::parse(other).is_ok_and(|name| name.key() == *key))
    });
    Err(match near {
        Some(near) => format!(
            "the store holds no project {project}, and a write does not create one; the store \
             has {near}"
        ),
        None => format!("the store holds no project {project}, and a write does not create one"),
    })
}

/// Which projects a read looks at: the one it named, or every one the caller may see. A named
/// project must be one the store lists, as for a write: a `memory` team's archived project keeps
/// its records in the repository, and naming it would read them past the cabinet's list.
fn scope(
    arguments: &Value,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> Result<Vec<String>, String> {
    match optional(arguments, "project") {
        Some(one) => {
            in_scope(&one, caller)?;
            if !memories.projects()?.contains(&one) {
                return Err(format!("the store holds no project {one}"));
            }
            Ok(vec![one])
        }
        None => visible(caller, memories),
    }
}

/// Refuses a version the team's limits do not allow. The names of the refusals are the ones
/// Anthropic's memory tool uses, so an agent recognises them without being taught.
fn within_limits(
    caller: &Caller<'_>,
    project: &str,
    memory: &Memory,
    event: &Event,
    memories: &dyn Memories,
) -> Result<(), String> {
    let new = !memory.records.contains_key(event.id());
    if let Some(max) = caller.limits.max_records
        && new
        && u64::try_from(memory.records.len()).unwrap_or(u64::MAX) >= max
    {
        return Err(format!(
            "too_many_entries: {project} already holds {} memories, the team's limit is {max}; \
             forget or merge some first",
            memory.records.len()
        ));
    }
    let size = journal::encode(event)
        .map_err(|error| error.to_string())?
        .len();
    let size = u64::try_from(size).unwrap_or(u64::MAX);
    if let Some(max) = caller.limits.max_record_bytes
        && size > max
    {
        return Err(format!(
            "entry_too_large: this version takes {size} bytes, the team's limit is {max}"
        ));
    }
    // The store only grows: a forgotten memory stays in the history, so over the quota the team
    // reads and forgets, and writes again once its plan gives it more room
    if let Some(quota) = caller.limits.quota_bytes {
        let taken = memories.store_bytes()?;
        if taken.saturating_add(size) > quota {
            let cabinet = caller
                .cabinet
                .map_or_else(String::new, |cabinet| format!(" at {cabinet}"));
            return Err(format!(
                "quota_exceeded: the team's store takes {taken} bytes of its {quota}; \
                 a bigger plan in the cabinet{cabinet} makes room"
            ));
        }
    }
    Ok(())
}

fn project_resolve(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let directory = text(arguments, "directory")?;
    Ok(match memories.project_of_directory(&directory)? {
        DirectoryProject::Held { name, rule } => json!({
            "directory": directory, "project": name, "rule": rule
        }),
        DirectoryProject::Unheld { name } => json!({
            "directory": directory, "project": null, "wouldBe": name,
            "why": "the store holds no such project, and a write does not create one"
        }),
        DirectoryProject::Ignored { reason } => json!({
            "directory": directory, "project": null,
            "why": format!("the owner excluded this folder ({reason})")
        }),
        DirectoryProject::NotVisible => json!({
            "directory": directory, "project": null,
            "projects": visible(caller, memories)?,
            "why": "the store is on another machine and cannot see your disk: pass one of \
                    `projects` as `project`"
        }),
    })
}

fn search(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let needle = optional(arguments, "query")
        .unwrap_or_default()
        .to_lowercase();
    let kind = optional(arguments, "kind")
        .map(|word| RecordKind::parse(&word).map_err(|error| error.to_string()))
        .transpose()?;
    let status = optional(arguments, "status")
        .map(|word| RecordStatus::parse(&word).map_err(|error| error.to_string()))
        .transpose()?;

    let mut found = Vec::new();
    for project in scope(arguments, caller, memories)? {
        for entry in memories.load(&project)?.records.values() {
            let record = &entry.record;
            if kind.is_some_and(|wanted| wanted != record.kind)
                || status.is_some_and(|wanted| wanted != record.status)
                || !matches(record, &needle)
            {
                continue;
            }
            found.push(json!({
                "id": record.id.as_str(),
                "project": record.project,
                "kind": record.kind.as_str(),
                // Always reported, never only when stale: an agent that has to ask a second
                // question to learn a fact is out of date will skip asking.
                "status": record.status.as_str(),
                // Same reasoning: a disputed record is read differently, and the search is where
                // an agent meets it
                "disputed": entry.is_divergent(),
                "title": record.title(),
                "description": record.description,
                "agent": record.agent,
                "updated": record.updated_at,
            }));
        }
    }
    Ok(json!({ "results": found }))
}

/// Whether a record answers the words asked for. Empty asks for everything.
fn matches(record: &Record, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    needle.split_whitespace().all(|word| {
        record.title().to_lowercase().contains(word)
            || record.description.to_lowercase().contains(word)
            || record.body.to_lowercase().contains(word)
            || record.id.as_str().contains(word)
    })
}

fn get(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    for project in scope(arguments, caller, memories)? {
        let memory = memories.load(&project)?;
        if let Some(entry) = memory.records.get(&id) {
            let record = &entry.record;
            return Ok(json!({
                "id": record.id.as_str(),
                "project": record.project,
                "kind": record.kind.as_str(),
                "status": record.status.as_str(),
                "title": record.title(),
                "description": record.description,
                "body": record.body,
                "links": record.links.iter().map(RecordId::as_str).collect::<Vec<_>>(),
                "agent": record.agent,
                "member": record.member,
                "created": record.created_at,
                "updated": record.updated_at,
                // The version is what memory_update needs as a parent; hiding it would make every
                // edit look like it was made blind.
                "version": entry.version,
                "rivalVersions": entry.rivals.len(),
                // What the other machine wrote: to merge it, an agent has to read it
                "rivals": entry.rivals.iter().map(|(version, rival)| json!({
                    "version": version,
                    "description": rival.description,
                    "body": rival.body,
                    "agent": rival.agent,
                    "member": rival.member,
                    "updated": rival.updated_at,
                })).collect::<Vec<_>>(),
            }));
        }
    }
    Err(format!("no memory with id {}", id.as_str()))
}

fn save(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    may_write(caller)?;
    let agent = caller.agent;
    let project = project_of(arguments, caller, memories)?;
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    let memory = memories.load(&project)?;
    if memory.records.contains_key(&id) {
        return Err(format!(
            "{} already exists in {project}; change it with memory_update",
            id.as_str()
        ));
    }
    let now = memories.now();
    let body = text(arguments, "body")?;
    let record = Record {
        links: links_in(&body),
        id: id.clone(),
        kind: RecordKind::parse(&text(arguments, "kind")?).map_err(|error| error.to_string())?,
        project: project.clone(),
        description: text(arguments, "description")?,
        // Nothing the caller can set: the real memory format has no such field, and the heading
        // shown in the index is derived from the identifier.
        metadata: std::collections::BTreeMap::new(),
        body,
        status: RecordStatus::Active,
        agent: agent.to_owned(),
        member: caller.member.map(str::to_owned),
        created_at: now.clone(),
        updated_at: now,
    };
    record.validate().map_err(|error| error.to_string())?;
    let event = Event {
        uuid: memories.new_version(id.as_str()),
        parent: None,
        merges: Vec::new(),
        action: Action::Upsert { record },
    };
    within_limits(caller, &project, &memory, &event, memories)?;
    memories.append(&project, &event)?;
    Ok(json!({ "saved": id.as_str(), "version": event.uuid }))
}

fn update(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    may_write(caller)?;
    let agent = caller.agent;
    let project = project_of(arguments, caller, memories)?;
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    let memory = memories.load(&project)?;
    let entry = memory
        .records
        .get(&id)
        .ok_or_else(|| format!("no memory with id {} in {project}", id.as_str()))?;

    let merges = merged_rivals(arguments, entry, &id)?;
    let mut record = entry.record.clone();
    if let Some(description) = optional(arguments, "description") {
        record.description = description;
    }
    if let Some(body) = optional(arguments, "body") {
        record.links = links_in(&body);
        record.body = body;
    }
    if let Some(status) = optional(arguments, "status") {
        record.status = RecordStatus::parse(&status).map_err(|error| error.to_string())?;
    }
    agent.clone_into(&mut record.agent);
    record.member = caller.member.map(str::to_owned);
    record.updated_at = memories.now();
    record.validate().map_err(|error| error.to_string())?;

    let event = Event {
        uuid: memories.new_version(id.as_str()),
        // The version this writer saw. Without it a second agent's edit would look like a fresh
        // record rather than a concurrent one, and the loser would be silently overwritten.
        parent: Some(entry.version.clone()),
        merges,
        action: Action::Upsert { record },
    };
    within_limits(caller, &project, &memory, &event, memories)?;
    memories.append(&project, &event)?;
    Ok(json!({
        "updated": id.as_str(),
        "version": event.uuid,
        "parent": entry.version,
        "merged": event.merges,
    }))
}

/// The rivals an update says it merged. Only current rivals of this record: a version that is
/// not one was settled by someone else meanwhile, or never belonged here, and the caller has to
/// read the record again rather than close something it did not see.
fn merged_rivals(
    arguments: &Value,
    entry: &vibememory_core::memory::Entry,
    id: &RecordId,
) -> Result<Vec<String>, String> {
    let Some(list) = arguments.get("merges") else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or_else(|| "merges is a list of rival versions".to_owned())?;
    let mut merges = Vec::new();
    for version in list {
        let version = version
            .as_str()
            .ok_or_else(|| "merges is a list of rival versions".to_owned())?;
        if !entry.rivals.iter().any(|(rival, _)| rival == version) {
            return Err(format!(
                "{version} is not a rival version of {}; read it again with memory_get",
                id.as_str()
            ));
        }
        if !merges.iter().any(|merged| merged == version) {
            merges.push(version.to_owned());
        }
    }
    Ok(merges)
}

fn delete(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    may_write(caller)?;
    let agent = caller.agent;
    let project = project_of(arguments, caller, memories)?;
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    let memory = memories.load(&project)?;
    let entry = memory
        .records
        .get(&id)
        .ok_or_else(|| format!("no memory with id {} in {project}", id.as_str()))?;
    let event = Event {
        uuid: memories.new_version(id.as_str()),
        parent: Some(entry.version.clone()),
        // Forgetting is about the record, not one of its versions: the rivals this server sees go
        // with it. One written concurrently with the delete still wins, as any concurrent edit does
        merges: entry
            .rivals
            .iter()
            .map(|(version, _)| version.clone())
            .collect(),
        action: Action::Delete {
            id: id.clone(),
            agent: agent.to_owned(),
            member: caller.member.map(str::to_owned),
            updated_at: memories.now(),
        },
    };
    memories.append(&project, &event)?;
    Ok(json!({ "forgotten": id.as_str(), "version": event.uuid }))
}

/// How many matches a search returns when the caller does not say.
const DEFAULT_HISTORY_LIMIT: usize = 20;
/// The most it will return however loudly it is asked. The corpus is gigabytes; an answer that
/// does not fit in a reply is not an answer.
const MAX_HISTORY_LIMIT: usize = 100;
/// How much of a matching message comes back. Enough to recognise the moment, not enough to
/// quietly hand a whole conversation to whoever asked.
const EXCERPT: usize = 240;

/// Searches past sessions for words said in them.
///
/// Newest first, and stops as soon as it has enough: the store here holds 1959 transcripts and
/// 1.7 GiB, so reading all of them to answer one question would make the tool useless. A file is
/// read only when the search actually reaches it.
fn history_search(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    if !caller.history {
        return Err(
            "this token cannot search past sessions: only a token of the team's owner or \
                    of an admin can"
                .to_owned(),
        );
    }
    let needle = text(arguments, "query")?.to_lowercase();
    if needle.trim().is_empty() {
        return Err("query is required: an empty search would return the whole history".to_owned());
    }
    let words: Vec<&str> = needle.split_whitespace().collect();
    let limit =
        arguments
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_HISTORY_LIMIT, |asked| {
                usize::try_from(asked)
                    .unwrap_or(MAX_HISTORY_LIMIT)
                    .clamp(1, MAX_HISTORY_LIMIT)
            });

    // Newest first across projects too, not project by project: "what did I say about X" is a
    // question about time, not about directories.
    let mut sessions = Vec::new();
    for project in scope(arguments, caller, memories)? {
        for transcript in memories.transcripts(&project)? {
            sessions.push((transcript.modified, project.clone(), transcript.session));
        }
    }
    sessions.sort_by_key(|(modified, _, _)| std::cmp::Reverse(*modified));

    let mut found = Vec::new();
    for (_, project, session) in sessions {
        if found.len() >= limit {
            break;
        }
        let bytes = memories.read_transcript(&project, &session)?;
        for hit in matches_in(&bytes, &words, limit - found.len()) {
            found.push(json!({
                "project": project,
                "session": session,
                "at": hit.at,
                "role": hit.role,
                "excerpt": hit.excerpt,
            }));
        }
    }
    Ok(json!({ "results": found }))
}

/// One message that matched.
struct Hit {
    at: String,
    role: String,
    excerpt: String,
}

/// Every matching message of one transcript, up to `wanted`.
///
/// A line that cannot be read is skipped, never fatal: the format is Anthropic's and changes
/// between versions, and a search that dies on one unknown line is a search nobody can rely on.
fn matches_in(bytes: &[u8], words: &[&str], wanted: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    for line in String::from_utf8_lossy(bytes).lines() {
        if hits.len() >= wanted {
            break;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = record
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if role != "user" && role != "assistant" {
            continue;
        }
        let said = spoken_text(&record);
        let lowered = said.to_lowercase();
        if !words.iter().all(|word| lowered.contains(word)) {
            continue;
        }
        hits.push(Hit {
            at: record
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            role: role.to_owned(),
            excerpt: excerpt_of(&said),
        });
    }
    hits
}

/// The words of a record, whether the content is a string or a list of blocks.
fn spoken_text(record: &Value) -> String {
    let Some(content) = record
        .get("message")
        .and_then(|message| message.get("content"))
    else {
        return String::new();
    };
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// The first part of a message, cut on a character boundary.
fn excerpt_of(said: &str) -> String {
    let trimmed = said.trim();
    if trimmed.chars().count() <= EXCERPT {
        return trimmed.to_owned();
    }
    let cut: String = trimmed.chars().take(EXCERPT).collect();
    format!("{cut}…")
}

/// The records a body points at, in the order they appear.
///
/// Same rule as the markdown projection: a link to a record that does not exist yet is not an
/// error — the format uses it to mark what is worth writing later.
fn links_in(body: &str) -> Vec<RecordId> {
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = rest.get(start + 2..).unwrap_or_default();
        let Some(end) = after.find("]]") else {
            break;
        };
        if let Ok(id) = RecordId::parse(after.get(..end).unwrap_or_default())
            && !found.contains(&id)
        {
            found.push(id);
        }
        rest = after.get(end + 2..).unwrap_or_default();
    }
    found
}
