//! The seven things an agent can do to the memory, and nothing else.
//!
//! Every one of them is the journal's own vocabulary: read the folded state, or append one event.
//! There is no update-in-place and no delete-the-line, because the journal has neither — that is
//! what lets two machines merge it by union without losing a word. The one tool outside that
//! vocabulary, `project_resolve`, only reads the engine's naming rules and writes nothing.

use serde_json::{Value, json};
use vibememory_core::memory::journal::{Action, Event};
use vibememory_core::memory::record::{Record, RecordId, RecordKind, RecordStatus};

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
            "description": "Read one remembered fact in full, by its identifier.",
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
                            both versions instead of overwriting each other.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project of the store. Omit to use the one this server was started in." },
                    "id": { "type": "string" },
                    "description": { "type": "string" },
                    "body": { "type": "string" },
                    "status": { "type": "string", "enum": ["active", "stale"] }
                },
                "required": ["id"]
            }
        },
        {
            "name": "memory_delete",
            "description": "Forget a fact. The versions stay in the journal; the record stops \
                            being shown. Prefer marking it stale when it explains why something \
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
        "memory_search" => search(arguments, memories),
        "memory_get" => get(arguments, memories),
        "memory_save" => save(arguments, caller, memories),
        "memory_update" => update(arguments, caller, memories),
        "memory_delete" => delete(arguments, caller, memories),
        "history_search" => history_search(arguments, memories),
        "project_resolve" => project_resolve(arguments, memories),
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

/// Who calls and from where, fixed for the life of the server.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'a> {
    /// Written into every record the call creates: memory is shared between agents, so it has to
    /// say who wrote it.
    pub agent: &'a str,
    /// The store project of the directory the client started the server in, when it is one. A
    /// write that names no project goes there: an agent in an IDE knows its folder, not the name
    /// the store gave it.
    pub project: Option<&'a str>,
}

/// The project a write goes to: the one it named, else the one the server was started in.
fn project_of(arguments: &Value, caller: &Caller<'_>) -> Result<String, String> {
    optional(arguments, "project")
        .or_else(|| caller.project.map(str::to_owned))
        .ok_or_else(|| {
            "project is required: this server was started outside any project of the store, so \
             name the one to write to"
                .to_owned()
        })
}

/// Which projects a call looks at: the one it named, or all of them.
fn scope(arguments: &Value, memories: &dyn Memories) -> Result<Vec<String>, String> {
    optional(arguments, "project").map_or_else(|| memories.projects(), |one| Ok(vec![one]))
}

fn project_resolve(arguments: &Value, memories: &dyn Memories) -> ToolResult {
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
    })
}

fn search(arguments: &Value, memories: &dyn Memories) -> ToolResult {
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
    for project in scope(arguments, memories)? {
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

fn get(arguments: &Value, memories: &dyn Memories) -> ToolResult {
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    for project in scope(arguments, memories)? {
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
                "created": record.created_at,
                "updated": record.updated_at,
                // The version is what memory_update needs as a parent; hiding it would make every
                // edit look like it was made blind.
                "version": entry.version,
                "rivalVersions": entry.rivals.len(),
            }));
        }
    }
    Err(format!("no memory with id {}", id.as_str()))
}

fn save(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let agent = caller.agent;
    let project = project_of(arguments, caller)?;
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    if memories.load(&project)?.records.contains_key(&id) {
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
        created_at: now.clone(),
        updated_at: now,
    };
    record.validate().map_err(|error| error.to_string())?;
    let event = Event {
        uuid: memories.new_version(id.as_str()),
        parent: None,
        action: Action::Upsert { record },
    };
    memories.append(&project, &event)?;
    Ok(json!({ "saved": id.as_str(), "version": event.uuid }))
}

fn update(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let agent = caller.agent;
    let project = project_of(arguments, caller)?;
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    let memory = memories.load(&project)?;
    let entry = memory
        .records
        .get(&id)
        .ok_or_else(|| format!("no memory with id {} in {project}", id.as_str()))?;

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
    record.updated_at = memories.now();
    record.validate().map_err(|error| error.to_string())?;

    let event = Event {
        uuid: memories.new_version(id.as_str()),
        // The version this writer saw. Without it a second agent's edit would look like a fresh
        // record rather than a concurrent one, and the loser would be silently overwritten.
        parent: Some(entry.version.clone()),
        action: Action::Upsert { record },
    };
    memories.append(&project, &event)?;
    Ok(json!({ "updated": id.as_str(), "version": event.uuid, "parent": entry.version }))
}

fn delete(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let agent = caller.agent;
    let project = project_of(arguments, caller)?;
    let id = RecordId::parse(&text(arguments, "id")?).map_err(|error| error.to_string())?;
    let memory = memories.load(&project)?;
    let entry = memory
        .records
        .get(&id)
        .ok_or_else(|| format!("no memory with id {} in {project}", id.as_str()))?;
    let event = Event {
        uuid: memories.new_version(id.as_str()),
        parent: Some(entry.version.clone()),
        action: Action::Delete {
            id: id.clone(),
            agent: agent.to_owned(),
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
fn history_search(arguments: &Value, memories: &dyn Memories) -> ToolResult {
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
    for project in scope(arguments, memories)? {
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
