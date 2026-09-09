//! The five things an agent can do to the memory, and nothing else.
//!
//! Every one of them is the journal's own vocabulary: read the folded state, or append one event.
//! There is no update-in-place and no delete-the-line, because the journal has neither — that is
//! what lets two machines merge it by union without losing a word.

use serde_json::{Value, json};
use vibememory_core::memory::journal::{Action, Event};
use vibememory_core::memory::record::{Record, RecordId, RecordKind, RecordStatus};

use crate::memories::Memories;

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
                    "project": { "type": "string" },
                    "id": { "type": "string", "description": "Short kebab-case slug; also the file name of its projection." },
                    "kind": { "type": "string", "enum": ["user", "feedback", "project", "reference"] },
                    "description": { "type": "string", "description": "One line: what is inside, so a reader can tell whether it is relevant." },
                    "body": { "type": "string", "description": "The memory itself, markdown. Link others with [[their-id]]." }
                },
                "required": ["project", "id", "kind", "description", "body"]
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
                    "project": { "type": "string" },
                    "id": { "type": "string" },
                    "description": { "type": "string" },
                    "body": { "type": "string" },
                    "status": { "type": "string", "enum": ["active", "stale"] }
                },
                "required": ["project", "id"]
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
                    "project": { "type": "string" },
                    "id": { "type": "string" }
                },
                "required": ["project", "id"]
            }
        }
    ])
}

/// Runs one tool by name.
///
/// # Errors
///
/// A sentence the agent can act on. An unknown tool is an error, never a guess at what was meant.
pub fn call(name: &str, arguments: &Value, agent: &str, memories: &dyn Memories) -> ToolResult {
    match name {
        "memory_search" => search(arguments, memories),
        "memory_get" => get(arguments, memories),
        "memory_save" => save(arguments, agent, memories),
        "memory_update" => update(arguments, agent, memories),
        "memory_delete" => delete(arguments, agent, memories),
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

/// Which projects a call looks at: the one it named, or all of them.
fn scope(arguments: &Value, memories: &dyn Memories) -> Result<Vec<String>, String> {
    optional(arguments, "project").map_or_else(|| memories.projects(), |one| Ok(vec![one]))
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

fn save(arguments: &Value, agent: &str, memories: &dyn Memories) -> ToolResult {
    let project = text(arguments, "project")?;
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

fn update(arguments: &Value, agent: &str, memories: &dyn Memories) -> ToolResult {
    let project = text(arguments, "project")?;
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

fn delete(arguments: &Value, agent: &str, memories: &dyn Memories) -> ToolResult {
    let project = text(arguments, "project")?;
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
