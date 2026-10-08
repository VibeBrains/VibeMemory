//! Rules and skills for an agent over MCP: the rules in force for a project, and a rule or a skill written at the
//! level the person named.
//!
//! The level is the person's word, so the server refuses a write without one rather than guessing: an agent that
//! forgot to ask gets `levelRequired` and asks. A rule about the same thing as one already in force is refused with
//! the ones it repeats, so a correction extends a rule instead of writing a fifth one about commits.
//!
//! Rules and skills are files of the store on this machine. The host keeps bare repositories only and answers that
//! they are written where the engine runs — the same answer it gives for hand-offs.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use vibememory_cli::rules::{
    PERSONAL_RULES, PERSONAL_SKILLS, PROJECT_RULES, PROJECT_SKILLS, TEAM_RULES, TEAM_SKILLS,
    read_rules, write_rule,
};
use vibememory_core::rules::compare::similar;
use vibememory_core::rules::layers::{Resolved, resolve};
use vibememory_core::rules::skill::{SKILL_FILE, parse_skill};
use vibememory_core::rules::{Level, Rule, RuleId};

use crate::memories::Memories;
use crate::tools::{Caller, ToolResult};

use vibememory_cli::rules::PROPOSALS;

/// What every rule tool says about levels: the protocol an agent follows when a person asks for a rule.
pub const LEVELS: &str = "Levels are the person's word: «в общие правила», «в глобальные» — personal (every project of \
                          theirs); «в правила команды» — team; «в правило проекта», «для этого проекта» — project. \
                          «Сохрани в правило» without a level: ask the person which of the three before writing. \
                          A rule is short and always in force; a procedure of many steps is a skill instead.";

/// The catalogue entries of the rule and skill tools.
#[must_use]
pub fn catalogue() -> Vec<Value> {
    vec![
        json!({
            "name": "rules_get",
            "description": format!("The rules in force for a project: the person's, the project's team's and the \
                            project's own, stacked — a rule above replaces one of the same id below, except a \
                            personal `absolute` one and a team's `enforced` one. Call it at the start of work when \
                            your client does not load rule files itself. {LEVELS}"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project of the store. Omit for the one this server was started in; without one, the person's and the team's rules only." }
                },
                "required": []
            }
        }),
        json!({
            "name": "rule_save",
            "description": format!("Write a rule, or replace one with the same id, at the level the person named. \
                            Refused without `level` (levelRequired: ask the person), and refused when rules about \
                            the same thing exist (similarFound: extend one of them by saving under its id, or save \
                            anyway with force). A team rule is a proposal to the team's owner and admins. {LEVELS}"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "level": { "type": "string", "enum": ["personal", "team", "project"] },
                    "title": { "type": "string", "description": "One line a person reads in a list." },
                    "body": { "type": "string", "description": "The rule: one thought a line, what to do and why." },
                    "id": { "type": "string", "description": "Kebab-case latin, permanent. Omit to make one from the title." },
                    "project": { "type": "string", "description": "For a project rule: project of the store; omit for the one this server was started in." },
                    "absolute": { "type": "boolean", "description": "Personal only: nothing above overrides it." },
                    "enforced": { "type": "boolean", "description": "Team only: projects and members do not override it." },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Globs of the files it is about; omit for always." },
                    "force": { "type": "boolean", "description": "Save although similar rules exist." }
                },
                "required": ["title", "body"]
            }
        }),
        json!({
            "name": "skill_get",
            "description": "A skill's SKILL.md by name, or the list of skills of the person, the team and the project \
                            when no name is given: for an agent that does not read skill directories itself.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "project": { "type": "string" }
                },
                "required": []
            }
        }),
        json!({
            "name": "skill_save",
            "description": format!("Write a skill's SKILL.md at the level the person named: a procedure loaded when \
                            it is needed, where a rule is always in force. Refused without `level`; a SKILL.md an \
                            agent would skip — a name that is not kebab-case, no description — is refused with the \
                            reason. A team skill is a proposal. {LEVELS}"),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "level": { "type": "string", "enum": ["personal", "team", "project"] },
                    "name": { "type": "string", "description": "Kebab-case; the skill's directory." },
                    "content": { "type": "string", "description": "The whole SKILL.md, frontmatter with name and description first." },
                    "project": { "type": "string" }
                },
                "required": ["name", "content"]
            }
        }),
    ]
}

/// The rule and skill tools by name; `None` for a name that is not one of them.
#[must_use]
pub fn call(
    name: &str,
    arguments: &Value,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> Option<ToolResult> {
    Some(match name {
        "rules_get" => rules_get(arguments, caller, memories),
        "rule_save" => rule_save(arguments, caller, memories),
        "skill_get" => skill_get(arguments, caller, memories),
        "skill_save" => skill_save(arguments, caller, memories),
        _ => return None,
    })
}

/// The directories of one store as rules and skills see them.
struct Places {
    /// The store the server answers for.
    store: PathBuf,
    /// The personal store, where the person's rules are.
    personal: PathBuf,
    /// Whether the store is a team's.
    team: bool,
}

fn places(memories: &dyn Memories) -> Result<Places, String> {
    let (Some(store), Some(personal)) = (memories.store_dir(), memories.personal_store_dir())
    else {
        return Err("rules and skills are files of a machine's store, and this server keeps bare repositories \
                    only: write them where the engine runs — with this tool there, or `vibememory rule add`"
            .to_owned());
    };
    let team = store != personal;
    Ok(Places {
        store,
        personal,
        team,
    })
}

fn text(arguments: &Value, field: &str) -> Option<String> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn flag(arguments: &Value, field: &str) -> bool {
    arguments
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The project a call is about: named, or the one the server was started in, and only one the store holds.
fn project(
    arguments: &Value,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> Result<Option<String>, String> {
    let Some(project) = text(arguments, "project").or_else(|| caller.project.map(str::to_owned))
    else {
        return Ok(None);
    };
    if memories.projects()?.contains(&project) {
        Ok(Some(project))
    } else {
        Err(format!("the store holds no project {project}"))
    }
}

fn rules_in(dir: &Path) -> Vec<Rule> {
    read_rules(dir).0.into_values().collect()
}

fn rules_get(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let places = places(memories)?;
    let project = project(arguments, caller, memories)?;
    let resolved = resolved(&places, project.as_deref());
    let rules: Vec<Value> = resolved
        .rules
        .iter()
        .map(|in_force| {
            let rule = &in_force.rule;
            json!({
                "id": rule.id.as_str(),
                "title": rule.title,
                "level": rule.level.as_str(),
                "absolute": rule.absolute,
                "enforced": rule.enforced,
                "paths": rule.paths,
                "replaces": in_force.replaces.iter().map(|level| level.as_str()).collect::<Vec<_>>(),
                "body": rule.body,
            })
        })
        .collect();
    let conflicts: Vec<Value> = resolved
        .conflicts
        .iter()
        .map(|(absolute, enforced)| {
            json!({
                "id": absolute.id.as_str(),
                "note": "the person's absolute rule and the team's enforced rule say different things; both are in \
                         force — tell the person, do not choose",
                "enforced": enforced.body,
            })
        })
        .collect();
    let mut answer = serde_json::Map::new();
    answer.insert("project".to_owned(), json!(project));
    answer.insert("rules".to_owned(), json!(rules));
    answer.insert("conflicts".to_owned(), json!(conflicts));
    if places.team {
        let waiting: Vec<Value> = vibememory_cli::rules_shown::team_rules(&places.store, false)
            .waiting
            .iter()
            .map(|waiting| {
                json!({ "id": waiting.rule.id.as_str(), "title": waiting.rule.title, "change": waiting.change.as_str() })
            })
            .collect();
        if !waiting.is_empty() {
            answer.insert("teamRulesNotShown".to_owned(), json!({
                "rules": waiting,
                "note": "the team's new or changed rules are not in force on this machine until the person is told: \
                         name them to the person; `vibememory rules status` shows and takes them",
            }));
        }
    }
    Ok(Value::Object(answer))
}

/// A kebab-case id from a title: latin letters and digits; a title in another script gives nothing, and the caller
/// is asked for an id.
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-')
        .chars()
        .take(vibememory_core::rules::rule::MAX_ID)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

fn level_of(arguments: &Value) -> Result<Level, String> {
    let Some(word) = text(arguments, "level") else {
        return Err("levelRequired: the person did not say where this rule goes. Ask them: personal (all their \
                    projects), team (every member, in the team's projects) or project (this project only) — then \
                    save again with `level`."
            .to_owned());
    };
    Level::parse(&word).map_err(|error| error.to_string())
}

fn who(caller: &Caller<'_>) -> String {
    let name = caller.member.unwrap_or(caller.agent);
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The rule a call describes: its id given or made from the title.
fn rule_from(arguments: &Value, level: Level, title: String, body: &str) -> Result<Rule, String> {
    let id = match text(arguments, "id") {
        Some(id) => id,
        None => slug(&title),
    };
    if id.is_empty() {
        return Err(
            "id is required: the title has no latin words to make one from — give a kebab-case id"
                .to_owned(),
        );
    }
    let rule = Rule {
        id: RuleId::parse(&id).map_err(|error| error.to_string())?,
        title,
        level,
        absolute: flag(arguments, "absolute"),
        enforced: flag(arguments, "enforced"),
        paths: arguments
            .get("paths")
            .and_then(Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        body: vibememory_core::rules::rule::normalize_body(body),
        extra: Vec::new(),
    };
    Ok(rule)
}

fn rule_save(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    crate::tools::may_write(caller)?;
    let level = level_of(arguments)?;
    let places = places(memories)?;
    let title = text(arguments, "title").ok_or("title is required")?;
    let body = text(arguments, "body").ok_or("body is required")?;
    let rule = rule_from(arguments, level, title, &body)?;
    rule.validate().map_err(|error| error.to_string())?;
    let project = project(arguments, caller, memories)?;
    let dir = match level {
        Level::Personal => places.personal.join(PERSONAL_RULES),
        Level::Project => places.store.join("projects").join(project.as_deref().ok_or(
            "project is required for a project rule: this server was started outside any project of the store",
        )?).join(PROJECT_RULES),
        Level::Team if places.team => places.store.join(PROPOSALS).join(TEAM_RULES),
        Level::Team => return Err("this project is not a team's: a team rule is written in a team's project".to_owned()),
    };

    let alike = repeats(&places, project.as_deref(), &rule);
    if !alike.is_empty() && !flag(arguments, "force") {
        return Err(format!(
            "similarFound: {} already say something like this. Extend one of them — save under its id with the \
             combined text — or, if this is a different rule, save again with force: true",
            alike.join(", ")
        ));
    }

    if level == Level::Team {
        let proposed = Rule {
            id: rule.id.clone(),
            ..rule.clone()
        };
        let file = dir.join(format!("{}.{}.md", rule.id, who(caller)));
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        std::fs::write(&file, proposed.render()).map_err(|error| error.to_string())?;
        return Ok(json!({
            "proposed": rule.id.as_str(),
            "note": "a team's rule is accepted by its owner or admins: `vibememory rule accept` on their machine",
        }));
    }
    write_rule(&dir, &rule)?;
    Ok(json!({
        "saved": rule.id.as_str(),
        "level": level.as_str(),
        "note": "every agent of the person's machines gets it with the engine's next run, within two minutes",
    }))
}

/// The ids of the rules in force that say something like `rule`: one about the same thing is better extended than
/// written twice.
fn repeats(places: &Places, project: Option<&str>, rule: &Rule) -> Vec<String> {
    let personal = rules_in(&places.personal.join(PERSONAL_RULES));
    let team = if places.team {
        rules_in(&places.store.join(TEAM_RULES))
    } else {
        Vec::new()
    };
    let own = project
        .map(|project| {
            rules_in(
                &places
                    .store
                    .join("projects")
                    .join(project)
                    .join(PROJECT_RULES),
            )
        })
        .unwrap_or_default();
    let known: Vec<(String, String, String)> = personal
        .iter()
        .chain(&team)
        .chain(&own)
        .filter(|other| other.id != rule.id)
        .map(|other| {
            (
                other.id.as_str().to_owned(),
                other.title.clone(),
                other.body.clone(),
            )
        })
        .collect();
    similar(&rule.title, &rule.body, &known)
        .into_iter()
        .map(|(id, _)| id.to_owned())
        .collect()
}

fn skill_dirs(places: &Places, project: Option<&str>) -> Vec<(Level, PathBuf)> {
    let mut dirs = vec![(Level::Personal, places.personal.join(PERSONAL_SKILLS))];
    if places.team {
        dirs.push((Level::Team, places.store.join(TEAM_SKILLS)));
    }
    if let Some(project) = project {
        dirs.push((
            Level::Project,
            places
                .store
                .join("projects")
                .join(project)
                .join(PROJECT_SKILLS),
        ));
    }
    dirs
}

fn skill_get(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    let places = places(memories)?;
    let project = project(arguments, caller, memories)?;
    let dirs = skill_dirs(&places, project.as_deref());
    if let Some(name) = text(arguments, "name") {
        // the level above wins, as for rules
        for (level, dir) in dirs.iter().rev() {
            if let Ok(content) = std::fs::read_to_string(dir.join(&name).join(SKILL_FILE)) {
                return Ok(json!({ "name": name, "level": level.as_str(), "content": content }));
            }
        }
        return Err(format!("no skill {name}"));
    }
    let skills: Vec<Value> = skills(&places, project.as_deref())
        .into_iter()
        .map(|skill| json!({ "name": skill.name, "level": skill.level.as_str(), "description": skill.description }))
        .collect();
    Ok(json!({ "skills": skills }))
}

fn skill_save(arguments: &Value, caller: &Caller<'_>, memories: &dyn Memories) -> ToolResult {
    crate::tools::may_write(caller)?;
    let level = level_of(arguments)?;
    let places = places(memories)?;
    let name = text(arguments, "name").ok_or("name is required")?;
    let content = text(arguments, "content").ok_or("content is required")?;
    parse_skill(&name, &content).map_err(|error| error.to_string())?;
    let project = project(arguments, caller, memories)?;
    let dir = match level {
        Level::Personal => places.personal.join(PERSONAL_SKILLS).join(&name),
        Level::Project => places
            .store
            .join("projects")
            .join(
                project
                    .as_deref()
                    .ok_or("project is required for a project skill")?,
            )
            .join(PROJECT_SKILLS)
            .join(&name),
        Level::Team if places.team => places
            .store
            .join(PROPOSALS)
            .join(TEAM_SKILLS)
            .join(format!("{name}.{}", who(caller))),
        Level::Team => {
            return Err(
                "this project is not a team's: a team skill is written in a team's project"
                    .to_owned(),
            );
        }
    };
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    std::fs::write(dir.join(SKILL_FILE), content).map_err(|error| error.to_string())?;
    let verb = if level == Level::Team {
        "proposed"
    } else {
        "saved"
    };
    let mut answer = serde_json::Map::new();
    answer.insert(verb.to_owned(), json!(name));
    answer.insert("level".to_owned(), json!(level.as_str()));
    Ok(Value::Object(answer))
}

/// The scheme of the resources this server gives: a rule in force, a skill.
pub const SCHEME: &str = "vibememory://";

/// The rules in force and the skills, as MCP resources: an agent whose client reads resources — DSH does — gets them
/// without a directory of its own. Nothing on the host, which keeps no files of a machine.
#[must_use]
pub fn resources(caller: &Caller<'_>, memories: &dyn Memories) -> Vec<Value> {
    let Ok(places) = places(memories) else {
        return Vec::new();
    };
    let project = project(&json!({}), caller, memories).ok().flatten();
    let mut found: Vec<Value> = in_force(&places, project.as_deref())
        .into_iter()
        .map(|rule| {
            json!({
                "uri": format!("{SCHEME}rules/{}", rule.id),
                "name": rule.id.as_str(),
                "title": rule.title,
                "description": format!("A {} rule, always in force", rule.level.as_str()),
                "mimeType": "text/markdown",
            })
        })
        .collect();
    found.extend(skills(&places, project.as_deref()).into_iter().map(|skill| {
        json!({
            "uri": format!("{SCHEME}skills/{}", skill.name),
            "name": skill.name,
            "description": format!("A {} skill: {}", skill.level.as_str(), skill.description),
            "mimeType": "text/markdown",
        })
    }));
    found
}

/// One resource's contents.
///
/// # Errors
///
/// A sentence naming what is not there.
pub fn read_resource(
    uri: &str,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> Result<Vec<Value>, String> {
    let places = places(memories)?;
    let project = project(&json!({}), caller, memories).ok().flatten();
    let text = if let Some(id) = uri.strip_prefix(&format!("{SCHEME}rules/")) {
        in_force(&places, project.as_deref())
            .into_iter()
            .find(|rule| rule.id.as_str() == id)
            .map(|rule| format!("## {}\n\n{}", rule.title, rule.body))
            .ok_or_else(|| format!("no rule {id} is in force here"))?
    } else if let Some(name) = uri.strip_prefix(&format!("{SCHEME}skills/")) {
        skill_dirs(&places, project.as_deref())
            .iter()
            .rev()
            .find_map(|(_, dir)| std::fs::read_to_string(dir.join(name).join(SKILL_FILE)).ok())
            .ok_or_else(|| format!("no skill {name}"))?
    } else {
        return Err(format!(
            "no resource {uri}: this server gives {SCHEME}rules/<id> and {SCHEME}skills/<name>"
        ));
    };
    Ok(vec![
        json!({ "uri": uri, "mimeType": "text/markdown", "text": text }),
    ])
}

/// The rules in force for a project: the person's, the team's as this machine's person was shown them, the
/// project's, stacked.
fn resolved(places: &Places, project: Option<&str>) -> Resolved {
    let personal = rules_in(&places.personal.join(PERSONAL_RULES));
    let team = if places.team {
        vibememory_cli::rules_shown::team_rules(&places.store, false).laid
    } else {
        Vec::new()
    };
    let own = project
        .map(|project| {
            rules_in(
                &places
                    .store
                    .join("projects")
                    .join(project)
                    .join(PROJECT_RULES),
            )
        })
        .unwrap_or_default();
    resolve(&personal, &team, &own)
}

fn in_force(places: &Places, project: Option<&str>) -> Vec<Rule> {
    resolved(places, project)
        .rules
        .into_iter()
        .map(|in_force| in_force.rule)
        .collect()
}

/// A skill an agent can be given.
struct Found {
    name: String,
    level: Level,
    description: String,
    dir: PathBuf,
}

/// The skills for a project, one a name: the level above wins, as for rules. A skill an agent would refuse is left
/// out; Claude's own synced skills are not other agents'.
fn skills(places: &Places, project: Option<&str>) -> Vec<Found> {
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    for (level, dir) in skill_dirs(places, project) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == vibememory_cli::rules::CLAUDE_SYNCED {
                continue;
            }
            if let Ok(manifest) = std::fs::read_to_string(entry.path().join(SKILL_FILE))
                .map_err(|error| error.to_string())
                .and_then(|text| parse_skill(&name, &text).map_err(|error| error.to_string()))
            {
                found.insert(
                    name,
                    Found {
                        name: manifest.name,
                        level,
                        description: manifest.description,
                        dir: entry.path(),
                    },
                );
            }
        }
    }
    found.into_values().collect()
}

/// The skills as MCP prompts: a client that shows prompts as commands — Claude Code as `/mcp__vibememory__<name>` —
/// gives the person every skill of the store by name, in an agent that has no skills directory of its own.
#[must_use]
pub fn prompts(caller: &Caller<'_>, memories: &dyn Memories) -> Vec<Value> {
    let Ok(places) = places(memories) else {
        return Vec::new();
    };
    let project = project(&json!({}), caller, memories).ok().flatten();
    skills(&places, project.as_deref())
        .into_iter()
        .map(|skill| {
            json!({
                "name": skill.name,
                "description": skill.description,
                "arguments": [{
                    "name": PROMPT_TASK,
                    "description": "What to do with the skill; empty leaves it to the conversation.",
                    "required": false,
                }],
            })
        })
        .collect()
}

/// The argument of a skill's prompt: the task it is used for.
const PROMPT_TASK: &str = "task";

/// One skill as a prompt: its `SKILL.md` and where its other files lie, then the task when one is given.
///
/// # Errors
///
/// A sentence naming a skill that is not there.
pub fn prompt(
    name: &str,
    arguments: &Value,
    caller: &Caller<'_>,
    memories: &dyn Memories,
) -> Result<Value, String> {
    let places = places(memories)?;
    let project = project(&json!({}), caller, memories).ok().flatten();
    let skill = skills(&places, project.as_deref())
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| format!("no skill {name}"))?;
    let content = std::fs::read_to_string(skill.dir.join(SKILL_FILE))
        .map_err(|error| format!("skill {name}: {error}"))?;
    let mut text = format!(
        "Use the skill {name}. Its files are in {}; the instructions:\n\n{content}",
        skill.dir.display()
    );
    if let Some(task) = arguments
        .get(PROMPT_TASK)
        .and_then(Value::as_str)
        .filter(|task| !task.trim().is_empty())
    {
        let _ = write!(text, "\n\nThe task: {task}");
    }
    Ok(json!({
        "description": skill.description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }],
    }))
}

/// The rules that hold whatever else says — the person's `absolute` ones and the team's `enforced` ones in force —
/// for the server's introduction: an agent that never calls `rules_get` still carries them.
#[must_use]
pub fn held(caller: &Caller<'_>, memories: &dyn Memories) -> Option<String> {
    let places = places(memories).ok()?;
    let project = project(&json!({}), caller, memories).ok().flatten();
    let held: Vec<String> = in_force(&places, project.as_deref())
        .into_iter()
        .filter(|rule| rule.absolute || rule.enforced)
        .map(|rule| format!("## {}\n\n{}", rule.title, rule.body.trim_end()))
        .collect();
    (!held.is_empty()).then(|| {
        format!(
            "Rules that hold whatever else says (the person's absolute ones, the team's enforced ones):\n\n{}",
            held.join("\n\n")
        )
    })
}
