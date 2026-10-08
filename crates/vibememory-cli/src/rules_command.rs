//! `vibememory rule …`, `vibememory skill …` and `vibememory rules …`: the rules and skills from a terminal.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vibememory_cli::install::Layout;
use vibememory_cli::rules::{
    PERSONAL_RULES, PERSONAL_SKILLS, PROJECT_RULES, PROJECT_SKILLS, TEAM_RULES, TEAM_SKILLS,
    read_rules, write_rule,
};
use vibememory_cli::rules_sync::{self, MODE_FILE, Mode};
use vibememory_core::rules::compare::{Merge, State};
use vibememory_core::rules::layers::resolve;
use vibememory_core::rules::rule::{MAX_ID, normalize_body};
use vibememory_core::rules::skill::{SKILL_FILE, SKILL_SCRIPTS, parse_skill};
use vibememory_core::rules::{Level, Rule, RuleId};

const USAGE: &str = "usage: vibememory rule list|show <id>|history <id>|add --level <personal|team|project> --title \
                     <t> [--id <id>] [--absolute] [--enforced] [--path <glob>]… (the text on stdin)|remove <id> --level \
                     <l>|move <id> --level <from> --to <to>|accept <id> [dir]\n       vibememory skill list [dir]\n       \
                     vibememory rules sync [dir] [--apply] [--mode advise|merge|override]|mode <dir> \
                     <advise|merge|override>|status [dir]|lint|split [--apply] [--skills <id>,…]";

/// Where a directory's rules are: the personal store, the store the directory is routed to, the project there.
struct Context {
    personal: PathBuf,
    store: PathBuf,
    team: Option<String>,
    project: Option<String>,
}

fn context(layout: &Layout, dir: Option<&str>) -> Result<Context, String> {
    let config = crate::read_config(layout).map_err(|error| format!("config: {error}"))?;
    let dir = dir.map_or_else(
        || std::env::current_dir().map_err(|error| error.to_string()),
        |dir| Ok(PathBuf::from(dir)),
    )?;
    let real = std::fs::canonicalize(&dir).unwrap_or(dir);
    let syntax = vibememory_cli::hook::session_start::host_syntax();
    let cwd = vibememory_core::naming::canonical_cwd(&real.display().to_string(), syntax);
    let store = vibememory_cli::stores::for_cwd(layout, &config, &cwd, syntax)?;
    let project = match vibememory_cli::project::resolve(&store.clone, &config.naming, &cwd) {
        Ok(vibememory_core::naming::Resolution::Named { name, .. })
            if store.clone.join("projects").join(name.as_str()).is_dir() =>
        {
            Some(name.as_str().to_owned())
        }
        _ => None,
    };
    Ok(Context {
        personal: layout.store(),
        store: store.clone,
        team: store.team,
        project,
    })
}

impl Context {
    fn dir_of(&self, level: Level) -> Result<PathBuf, String> {
        match level {
            Level::Personal => Ok(self.personal.join(PERSONAL_RULES)),
            Level::Team if self.team.is_some() => Ok(self.store.join(TEAM_RULES)),
            Level::Team => Err("this directory is not a team's project".to_owned()),
            Level::Project => self
                .project
                .as_ref()
                .map(|project| {
                    self.store
                        .join("projects")
                        .join(project)
                        .join(PROJECT_RULES)
                })
                .ok_or_else(|| {
                    "this directory is not a project of the store yet: start a session in it first"
                        .to_owned()
                }),
        }
    }

    /// The rules in force here: a team's as this machine's person was shown them, like its agents have them.
    fn in_force(&self) -> vibememory_core::rules::layers::Resolved {
        let read = |dir: PathBuf| -> Vec<Rule> { read_rules(&dir).0.into_values().collect() };
        resolve(
            &read(self.personal.join(PERSONAL_RULES)),
            &if self.team.is_some() {
                vibememory_cli::rules_shown::team_rules(&self.store, false).laid
            } else {
                Vec::new()
            },
            &self
                .project
                .as_ref()
                .map(|project| {
                    read(
                        self.store
                            .join("projects")
                            .join(project)
                            .join(PROJECT_RULES),
                    )
                })
                .unwrap_or_default(),
        )
    }
}

/// Prints what a command did, or what stopped it, prefixed with the command.
fn said(result: Result<String, String>, command: &str) -> ExitCode {
    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("{command}: {error}")),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}

/// `vibememory rule …`.
pub fn rule(layout: &Layout, args: &[String]) -> ExitCode {
    let mut level = None;
    let mut id = None;
    let mut title = None;
    let mut to = None;
    let mut paths = Vec::new();
    let (mut absolute, mut enforced) = (false, false);
    let mut words = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--level" => level = rest.next().cloned(),
            "--id" => id = rest.next().cloned(),
            "--title" => title = rest.next().cloned(),
            "--to" => to = rest.next().cloned(),
            "--path" => paths.extend(rest.next().cloned()),
            "--absolute" => absolute = true,
            "--enforced" => enforced = true,
            _ => words.push(arg.clone()),
        }
    }
    let verb = words.first().map(String::as_str);
    let dir_arg = |at: usize| words.get(at).map(String::as_str);
    match verb {
        Some("list") => {
            context(layout, dir_arg(1)).map_or_else(|error| fail(&error), |context| list(&context))
        }
        Some("show") => {
            let Some(id) = words.get(1) else {
                return fail(USAGE);
            };
            said(
                context(layout, dir_arg(2)).and_then(|context| {
                    context
                        .in_force()
                        .rules
                        .into_iter()
                        .find(|rule| rule.rule.id.as_str() == id)
                        .map(|in_force| in_force.rule.render().trim_end().to_owned())
                        .ok_or_else(|| format!("no rule {id} is in force here"))
                }),
                "rule show",
            )
        }
        Some("add") => add(
            layout,
            level.as_deref(),
            id,
            title,
            paths,
            (absolute, enforced),
            dir_arg(1),
        ),
        Some("remove") => {
            let (Some(id), Some(level)) = (words.get(1), level.as_deref()) else {
                return fail(USAGE);
            };
            let removed = Level::parse(level)
                .map_err(|error| error.to_string())
                .and_then(|level| context(layout, dir_arg(2))?.dir_of(level))
                .and_then(|dir| {
                    std::fs::remove_file(dir.join(format!("{id}.md")))
                        .map_err(|error| error.to_string())
                })
                .map(|()| {
                    format!("rule {id} removed; the agents' copies go with the engine's next run")
                });
            said(removed, "rule remove")
        }
        Some("move") => {
            let (Some(id), Some(from), Some(to)) = (words.get(1), level.as_deref(), to.as_deref())
            else {
                return fail(USAGE);
            };
            said(move_rule(layout, id, from, to, dir_arg(2)), "rule move")
        }
        Some("history") => {
            let Some(id) = words.get(1) else {
                return fail(USAGE);
            };
            said(
                context(layout, dir_arg(2))
                    .and_then(|context| history(&context, id))
                    .map(|lines| lines.join("\n")),
                "rule history",
            )
        }
        Some("accept") => {
            let Some(id) = words.get(1) else {
                return fail(USAGE);
            };
            said(
                context(layout, dir_arg(2)).and_then(|context| accept(&context, id)),
                "rule accept",
            )
        }
        _ => fail(USAGE),
    }
}

/// `vibememory rule add`: the text on stdin, the level the person's word.
fn add(
    layout: &Layout,
    level: Option<&str>,
    id: Option<String>,
    title: Option<String>,
    paths: Vec<String>,
    (absolute, enforced): (bool, bool),
    dir: Option<&str>,
) -> ExitCode {
    let Some(level) = level else {
        return fail("rule add: --level is the person's word — personal, team or project");
    };
    let mut body = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut body).is_err()
        || body.trim().is_empty()
    {
        return fail("rule add: the rule's text goes on stdin");
    }
    let result = (|| -> Result<String, String> {
        let level = Level::parse(level).map_err(|error| error.to_string())?;
        let context = context(layout, dir)?;
        let title = title.ok_or("rule add: --title is required")?;
        let id = id.unwrap_or_else(|| slug(&title));
        let rule = Rule {
            id: RuleId::parse(&id).map_err(|error| error.to_string())?,
            title,
            level,
            absolute,
            enforced,
            paths,
            body: normalize_body(&body),
            extra: Vec::new(),
        };
        rule.validate().map_err(|error| error.to_string())?;
        put(layout, &context, &rule)
    })();
    match result {
        Ok(said) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

/// Writes a rule where its level puts it. A team's rule from a terminal is a proposal, as from an agent: the host
/// takes `rules/` of a team from its owner and admins only, and a member's push of it would hold the whole store
/// back until a reclone. The owner or an admin accepts it with `rule accept`.
fn put(layout: &Layout, context: &Context, rule: &Rule) -> Result<String, String> {
    if rule.level == Level::Team {
        let team = context
            .team
            .as_deref()
            .ok_or("this directory is not a team's project")?;
        let who = vibememory_cli::team_connect::read_record(layout, team)?.member;
        let dir = context
            .store
            .join(vibememory_cli::rules::PROPOSALS)
            .join(TEAM_RULES);
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        std::fs::write(dir.join(format!("{}.{who}.md", rule.id)), rule.render())
            .map_err(|error| error.to_string())?;
        return Ok(format!(
            "rule {} proposed to team {team}: its owner or an admin accepts it with `vibememory rule accept {}`",
            rule.id, rule.id
        ));
    }
    write_rule(&context.dir_of(rule.level)?, rule)?;
    Ok(format!(
        "rule {} saved; every agent gets it with the engine's next run",
        rule.id
    ))
}

/// `rule move <id> --level <from> --to <to>`: a rule that turned out to be wider or narrower than its level. A flag
/// the new level does not have is dropped and said: `absolute` is a person's, `enforced` a team's.
fn move_rule(
    layout: &Layout,
    id: &str,
    from: &str,
    to: &str,
    dir: Option<&str>,
) -> Result<String, String> {
    let from = Level::parse(from).map_err(|error| error.to_string())?;
    let to = Level::parse(to).map_err(|error| error.to_string())?;
    if from == to {
        return Err(format!("rule {id} is already at the {} level", to.as_str()));
    }
    let context = context(layout, dir)?;
    let source = context.dir_of(from)?.join(format!("{id}.md"));
    let text = std::fs::read_to_string(&source)
        .map_err(|error| format!("{}: {error}", source.display()))?;
    let mut rule = Rule::parse(&text).map_err(|error| error.to_string())?;
    let mut dropped = Vec::new();
    if rule.absolute && to != Level::Personal {
        rule.absolute = false;
        dropped.push("absolute");
    }
    if rule.enforced && to != Level::Team {
        rule.enforced = false;
        dropped.push("enforced");
    }
    rule.level = to;
    let said = put(layout, &context, &rule)?;
    let said = if to == Level::Team {
        // a proposal is not a rule yet: the old one stays in force until the team takes the new one
        format!(
            "{said}; the {from} rule stays until then — `vibememory rule remove {id} --level {from}` after it is \
             accepted",
            from = from.as_str()
        )
    } else {
        std::fs::remove_file(&source).map_err(|error| format!("{}: {error}", source.display()))?;
        said
    };
    Ok(if dropped.is_empty() {
        said
    } else {
        format!(
            "{said} ({} dropped: the {} level has no such flag)",
            dropped.join(", "),
            to.as_str()
        )
    })
}

/// `rule history <id>`: every version of the rule in force, from the store's git, oldest first.
fn history(context: &Context, id: &str) -> Result<Vec<String>, String> {
    let resolved = context.in_force();
    let rule = resolved
        .rules
        .iter()
        .find(|in_force| in_force.rule.id.as_str() == id)
        .map(|in_force| &in_force.rule)
        .ok_or_else(|| format!("no rule {id} is in force here"))?;
    let (root, relative) = match rule.level {
        Level::Personal => (
            context.personal.clone(),
            format!("{PERSONAL_RULES}/{id}.md"),
        ),
        Level::Team => (context.store.clone(), format!("{TEAM_RULES}/{id}.md")),
        Level::Project => (
            context.store.clone(),
            format!(
                "projects/{}/{PROJECT_RULES}/{id}.md",
                context.project.as_deref().unwrap_or_default()
            ),
        ),
    };
    let log = vibememory_cli::git::run_with_timeout(
        vibememory_cli::git::command(
            &root,
            &[
                "log",
                "--reverse",
                "--format=%h %ad %s",
                "--date=short",
                "--",
                &relative,
            ],
        ),
        std::time::Duration::from_secs(20),
    )?
    .unwrap_or_default();
    let mut lines: Vec<String> = log
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if lines.is_empty() {
        lines.push("not committed yet: the engine's next run commits it".to_owned());
    }
    lines.push(format!(
        "now: {} — {} bytes, {} level",
        rule.title,
        rule.body.len(),
        rule.level.as_str()
    ));
    Ok(lines)
}

fn list(context: &Context) -> ExitCode {
    let resolved = context.in_force();
    if resolved.rules.is_empty() {
        println!("no rules here yet: vibememory rule add, or rule_save from an agent");
    }
    for in_force in &resolved.rules {
        let rule = &in_force.rule;
        let mut marks = Vec::new();
        if rule.absolute {
            marks.push("absolute".to_owned());
        }
        if rule.enforced {
            marks.push("enforced".to_owned());
        }
        if !in_force.replaces.is_empty() {
            let levels: Vec<&str> = in_force
                .replaces
                .iter()
                .map(|level| level.as_str())
                .collect();
            marks.push(format!("replaces {}", levels.join(", ")));
        }
        let marks = if marks.is_empty() {
            String::new()
        } else {
            format!(" [{}]", marks.join("; "))
        };
        println!(
            "{:<9} {:<32} {}{marks}",
            rule.level.as_str(),
            rule.id.as_str(),
            rule.title
        );
    }
    for (absolute, enforced) in &resolved.conflicts {
        println!(
            "conflict  {}: the person's absolute rule and the team's enforced one say different things — both are in \
             force; settle it with the team ({} bytes against {})",
            absolute.id,
            absolute.body.len(),
            enforced.body.len()
        );
    }
    ExitCode::SUCCESS
}

/// Accepts a proposal to a team: `proposals/rules/<id>.<who>.md` becomes `rules/<id>.md`, and a skill's
/// `proposals/skills/<name>.<who>/` becomes `skills/<name>/`.
fn accept(context: &Context, id: &str) -> Result<String, String> {
    if context.team.is_none() {
        return Err("this directory is not a team's project".to_owned());
    }
    let proposals = context.store.join(vibememory_cli::rules::PROPOSALS);
    for (kind, target) in [(TEAM_RULES, TEAM_RULES), (TEAM_SKILLS, TEAM_SKILLS)] {
        let Ok(entries) = std::fs::read_dir(proposals.join(kind)) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.trim_end_matches(".md");
            let Some((proposed, who)) = stem.rsplit_once('.') else {
                continue;
            };
            if proposed != id {
                continue;
            }
            let destination = if kind == TEAM_RULES {
                context.store.join(target).join(format!("{id}.md"))
            } else {
                context.store.join(target).join(id)
            };
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            if destination.exists() && kind == TEAM_SKILLS {
                std::fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
            }
            std::fs::rename(entry.path(), &destination).map_err(|error| error.to_string())?;
            return Ok(format!(
                "{id} from {who} accepted into the team's {kind}; the host takes it from an owner or an admin"
            ));
        }
    }
    Err(format!("no proposal {id} in the team's store"))
}

/// `vibememory skill list [dir]`.
pub fn skill(layout: &Layout, args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) != Some("list") {
        return fail(USAGE);
    }
    let context = match context(layout, args.get(1).map(String::as_str)) {
        Ok(context) => context,
        Err(error) => return fail(&error),
    };
    for line in skill_lines(&context) {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

/// One line a skill, by level: what an agent gets, what it ignores, and a team's skill that runs code.
fn skill_lines(context: &Context) -> Vec<String> {
    let mut dirs = vec![("personal", context.personal.join(PERSONAL_SKILLS))];
    if context.team.is_some() {
        dirs.push(("team", context.store.join(TEAM_SKILLS)));
    }
    if let Some(project) = &context.project {
        dirs.push((
            "project",
            context
                .store
                .join("projects")
                .join(project)
                .join(PROJECT_SKILLS),
        ));
    }
    let mut lines = Vec::new();
    for (level, dir) in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != vibememory_cli::rules::CLAUDE_SYNCED && !name.starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let checked = std::fs::read_to_string(dir.join(&name).join(SKILL_FILE))
                .map_err(|error| error.to_string())
                .and_then(|text| parse_skill(&name, &text).map_err(|error| error.to_string()));
            lines.push(match checked {
                Ok(manifest) => {
                    let mut marks = Vec::new();
                    if !manifest.claude_only.is_empty() {
                        marks.push(format!(
                            "other agents ignore {}",
                            manifest.claude_only.join(", ")
                        ));
                    }
                    // a script is code an agent runs, not text it reads: a team's is somebody else's code
                    if dir.join(&name).join(SKILL_SCRIPTS).is_dir() {
                        marks.push(if level == "team" {
                            "runs the team's scripts".to_owned()
                        } else {
                            "runs scripts".to_owned()
                        });
                    }
                    if marks.is_empty() {
                        format!("{level:<9} {name}")
                    } else {
                        format!("{level:<9} {name} [{}]", marks.join("; "))
                    }
                }
                Err(error) => format!("{level:<9} {name} — not given to the agents: {error}"),
            });
        }
    }
    lines
}

/// `vibememory rules status [dir]`: each agent's copy of the person's rules against the store, the rules in force
/// here, a team's rules that wait to be shown — shown by this and in force from the engine's next run — and the
/// skills with what an agent ignores in them.
fn status(layout: &Layout, dir: Option<&str>) -> Result<Vec<String>, String> {
    let context = context(layout, dir)?;
    let settings = vibememory_cli::config::RulesConfig::of_engine(&layout.engine_dir);
    let agents =
        vibememory_cli::rules::Agents::of(&layout.config_dir, layout.home.as_deref(), &settings);
    let mut lines = vec!["agents".to_owned()];
    for state in vibememory_cli::rules::agent_states(&context.personal, &agents) {
        let stand = if state.behind.is_empty() {
            "in step".to_owned()
        } else {
            format!(
                "{} behind the store ({}): the engine's next run writes them",
                state.behind.len(),
                state.behind.join(", ")
            )
        };
        lines.push(format!(
            "  {:<17} {} — {} bytes, {stand}",
            state.agent,
            state.path.display(),
            state.bytes
        ));
    }
    for path in &agents.released.files {
        lines.push(format!("  off in rules.agents: {}", path.display()));
    }
    for warning in vibememory_cli::rules::check(&context.personal, &agents) {
        lines.push(format!("  {warning}"));
    }
    let resolved = context.in_force();
    let count = |level: Level| {
        resolved
            .rules
            .iter()
            .filter(|rule| rule.rule.level == level)
            .count()
    };
    lines.push(format!(
        "here: {} rule(s) in force — {} personal, {} team, {} project",
        resolved.rules.len(),
        count(Level::Personal),
        count(Level::Team),
        count(Level::Project)
    ));
    for (absolute, _) in &resolved.conflicts {
        lines.push(format!(
            "  conflict {}: the person's absolute rule and the team's enforced one differ — both are in force",
            absolute.id
        ));
    }
    if let Some(team) = &context.team {
        let held = vibememory_cli::rules_shown::team_rules(&context.store, true);
        if !held.waiting.is_empty() {
            lines.push(format!("team {team}, not shown before:"));
            lines.extend(
                vibememory_cli::rules_shown::lines(&held.waiting)
                    .into_iter()
                    .map(|line| format!("  {line}")),
            );
            vibememory_cli::rules_shown::mark_shown(&context.store)?;
            lines.push("  shown now: in force from the engine's next run".to_owned());
        }
    }
    let skills = skill_lines(&context);
    if !skills.is_empty() {
        lines.push("skills".to_owned());
        lines.extend(skills.into_iter().map(|line| format!("  {line}")));
    }
    Ok(lines)
}

/// `vibememory rules …`.
pub fn rules(layout: &Layout, args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("sync") => sync(layout, args.get(1..).unwrap_or_default()),
        Some("mode") => {
            let (Some(dir), Some(word)) = (args.get(1), args.get(2)) else {
                return fail(USAGE);
            };
            let Some(mode) = Mode::parse(word) else {
                return fail(USAGE);
            };
            let result = context(layout, Some(dir)).and_then(|context| {
                let project = context
                    .project
                    .ok_or("this directory is not a project of the store yet")?;
                let file = context.store.join("projects").join(project).join(MODE_FILE);
                std::fs::write(&file, format!("{}\n", mode.as_str()))
                    .map_err(|error| error.to_string())
            });
            result.map_or_else(
                |error| fail(&format!("rules mode: {error}")),
                |()| {
                    println!("the project's rule files: {}", mode.as_str());
                    ExitCode::SUCCESS
                },
            )
        }
        Some("lint") => lint(layout),
        Some("status") => said(
            status(layout, args.get(1).map(String::as_str)).map(|lines| lines.join("\n")),
            "rules status",
        ),
        Some("split") => {
            let apply = args.iter().any(|arg| arg == "--apply");
            // the sections the owner chose to become skills: a skill is loaded on demand, a rule always
            let skills: Vec<String> = args
                .iter()
                .position(|arg| arg == "--skills")
                .and_then(|at| args.get(at + 1))
                .map(|list| list.split(',').map(str::trim).map(str::to_owned).collect())
                .unwrap_or_default();
            split(layout, apply, &skills)
        }
        _ => fail(USAGE),
    }
}

fn sync(layout: &Layout, args: &[String]) -> ExitCode {
    let apply = args.iter().any(|arg| arg == "--apply");
    let asked = args
        .iter()
        .position(|arg| arg == "--mode")
        .and_then(|at| args.get(at + 1))
        .and_then(|word| Mode::parse(word));
    let dir = args
        .iter()
        .find(|arg| !arg.starts_with("--") && Mode::parse(arg).is_none())
        .cloned();
    let context = match context(layout, dir.as_deref()) {
        Ok(context) => context,
        Err(error) => return fail(&error),
    };
    let Some(project) = context.project.clone() else {
        return fail(
            "this directory is not a project of the store yet: start a session in it first",
        );
    };
    let root = dir.map_or_else(
        || std::env::current_dir().unwrap_or_default(),
        PathBuf::from,
    );
    let histories = rules_sync::histories(
        &context.personal,
        &context.store,
        context.team.is_some(),
        &project,
    );
    let mode = asked
        .unwrap_or_else(|| rules_sync::mode_of(&context.store.join("projects").join(&project)));
    let judged = rules_sync::judge_project(&root, &histories);
    let in_force = context.in_force();
    let ids: Vec<&str> = in_force
        .rules
        .iter()
        .map(|rule| rule.rule.id.as_str())
        .collect();
    let learned = rules_sync::learned(&context.store, &project, &ids);
    if !learned.is_empty() {
        println!("learned in sessions — feedback memories that could be the project's rules:");
        for lesson in &learned {
            println!(
                "  {} — {}: `vibememory rule add --level project --id {} --title …` with the rule on stdin",
                lesson.id, lesson.description, lesson.id
            );
        }
    }
    let any = print_findings(&judged);
    if !any {
        println!("the project has no rule files of its own");
        return ExitCode::SUCCESS;
    }
    if !apply {
        println!(
            "mode {}: nothing changed. `vibememory rules sync --apply --mode merge` drops duplicates and updates old \
             versions; --mode override lays the rules in force over every matched block",
            mode.as_str()
        );
        return ExitCode::SUCCESS;
    }
    let mode = if mode == Mode::Advise {
        Mode::Merge
    } else {
        mode
    };
    let stamp = vibememory_cli::clock::now();
    let mut quarantine = |name: &str, text: &str| -> Result<(), String> {
        vibememory_cli::memory::quarantine(
            &layout.engine_dir,
            &format!("{name}-{stamp}.md"),
            text.as_bytes(),
        )
        .map(|_| ())
    };
    for file in &judged {
        match rules_sync::apply(file, mode, &histories, &mut quarantine) {
            Ok(done) => println!(
                "{}: {} dropped, {} updated, {} set aside in the quarantine",
                file.file.path.display(),
                done.dropped,
                done.updated,
                done.set_aside
            ),
            Err(error) => eprintln!("{}: {error}", file.file.path.display()),
        }
    }
    ExitCode::SUCCESS
}

/// Says what each block of the project's rule files is; whether there were any.
fn print_findings(judged: &[rules_sync::Judged]) -> bool {
    let mut any = false;
    for file in judged {
        for finding in &file.findings {
            any = true;
            let what = match &finding.state {
                State::Duplicate => "duplicate of the rule in force: may go".to_owned(),
                State::Stale { behind } => {
                    format!("{behind} version(s) behind the rule in force: replace")
                }
                State::Custom {
                    merged: Merge::Clean(_),
                } => "changed here; the rule's changes merge cleanly".to_owned(),
                State::Custom {
                    merged: Merge::Conflicted(_),
                } => "changed here and in the rule both: merge by hand".to_owned(),
                State::ProjectOnly => {
                    "the project's own: keep, or raise it with `vibememory rule add`".to_owned()
                }
            };
            let rule = finding.rule.as_deref().map_or(String::new(), |id| {
                format!(
                    " ~ {id}{}",
                    if finding.guessed {
                        " (by likeness: check)"
                    } else {
                        ""
                    }
                )
            });
            println!(
                "{} · {}{rule}: {what}",
                file.file.path.display(),
                finding.block.heading
            );
        }
    }
    any
}

fn lint(layout: &Layout) -> ExitCode {
    let limit = vibememory_cli::config::RulesConfig::of_engine(&layout.engine_dir).long_rule_bytes;
    let (rules, problems) = read_rules(&layout.store().join(PERSONAL_RULES));
    for problem in problems {
        println!("broken    {problem}");
    }
    let mut long = 0;
    for rule in rules.values() {
        if rule.body.len() > limit {
            long += 1;
            println!(
                "long      {} — {} bytes: a procedure, better a skill with a line here pointing at it",
                rule.id,
                rule.body.len()
            );
        }
    }
    if long == 0 {
        println!("{} rule(s), none longer than {limit} bytes", rules.len());
    }
    ExitCode::SUCCESS
}

/// Latin letters for a Russian heading: an id is kebab-case latin.
fn translit(text: &str) -> String {
    const TABLE: &[(char, &str)] = &[
        ('а', "a"),
        ('б', "b"),
        ('в', "v"),
        ('г', "g"),
        ('д', "d"),
        ('е', "e"),
        ('ё', "e"),
        ('ж', "zh"),
        ('з', "z"),
        ('и', "i"),
        ('й', "y"),
        ('к', "k"),
        ('л', "l"),
        ('м', "m"),
        ('н', "n"),
        ('о', "o"),
        ('п', "p"),
        ('р', "r"),
        ('с', "s"),
        ('т', "t"),
        ('у', "u"),
        ('ф', "f"),
        ('х', "h"),
        ('ц', "ts"),
        ('ч', "ch"),
        ('ш', "sh"),
        ('щ', "sch"),
        ('ъ', ""),
        ('ы', "y"),
        ('ь', ""),
        ('э', "e"),
        ('ю', "yu"),
        ('я', "ya"),
    ];
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| {
            TABLE
                .iter()
                .find(|(from, _)| *from == c)
                .map_or_else(|| c.to_string(), |(_, to)| (*to).to_owned())
        })
        .collect()
}

/// A kebab-case id from a title, cut at a word under [`MAX_ID`].
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for word in translit(title)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        if slug.len() + word.len() + 1 > MAX_ID - 16 {
            break;
        }
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(word);
    }
    slug
}

/// A section of `CLAUDE.md` as it would become a rule, and a skill when it is a procedure.
struct Cut {
    id: String,
    title: String,
    body: String,
    absolute: bool,
    skill: bool,
}

fn cuts(text: &str) -> (String, Vec<Cut>) {
    let mut preamble = String::new();
    let mut cuts: Vec<Cut> = Vec::new();
    for line in text.split_inclusive('\n') {
        if let Some(title) = line.strip_prefix("## ") {
            let title = title.trim().to_owned();
            let mut id = slug(&title);
            let base = id.clone();
            let mut n = 2;
            while cuts.iter().any(|cut| cut.id == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            cuts.push(Cut {
                id,
                title,
                body: String::new(),
                absolute: false,
                skill: false,
            });
        } else if let Some(cut) = cuts.last_mut() {
            cut.body.push_str(line);
        } else {
            preamble.push_str(line);
        }
    }
    for cut in &mut cuts {
        cut.body = normalize_body(&cut.body);
        // the heading says it, not a mention in the text: a rule about rules may well name the word
        cut.absolute = cut.title.to_lowercase().contains("абсолют");
        cut.skill = false;
    }
    cuts.retain(|cut| !cut.body.is_empty() && !cut.id.is_empty());
    (preamble, cuts)
}

/// `vibememory rules split [--apply]`: the owner's `CLAUDE.md` cut into rules at its `##` sections, a long one into a
/// skill with a pointing rule. Without `--apply` only the plan is printed, for the owner to read first.
fn split(layout: &Layout, apply: bool, skills: &[String]) -> ExitCode {
    let limit = vibememory_cli::config::RulesConfig::of_engine(&layout.engine_dir).long_rule_bytes;
    let path = layout.store().join("config").join("CLAUDE.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return fail(&format!("{}: not there", path.display()));
    };
    let (preamble, mut cuts) = cuts(&text);
    for cut in &mut cuts {
        // an absolute rule stays in force at every moment: never a skill loaded on demand
        cut.skill = skills.contains(&cut.id) && !cut.absolute;
    }
    if let Some(unknown) = skills
        .iter()
        .find(|id| !cuts.iter().any(|cut| &&cut.id == id))
    {
        return fail(&format!("rules split: no section {unknown}"));
    }
    for cut in &cuts {
        let shape = if cut.skill {
            "rule + skill"
        } else if cut.body.len() > limit && !cut.absolute {
            "rule (long: a skill candidate)"
        } else {
            "rule"
        };
        println!(
            "{:<44} {:>6} bytes  {shape}{}  {}",
            cut.id,
            cut.body.len(),
            if cut.absolute { ", absolute" } else { "" },
            cut.title
        );
    }
    if !apply {
        println!(
            "{} section(s); nothing written — read the plan, then `vibememory rules split --apply [--skills <id>,…]`",
            cuts.len()
        );
        return ExitCode::SUCCESS;
    }
    let rules_dir = layout.store().join(PERSONAL_RULES);
    for cut in &cuts {
        let body = if cut.skill {
            let first = cut
                .body
                .split("\n\n")
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned();
            if let Err(error) = write_skill(&layout.store().join(PERSONAL_SKILLS), cut) {
                return fail(&format!("skill {}: {error}", cut.id));
            }
            format!(
                "{first}\n\nПорядок и подробности — навык `{}`: агент загружает его, когда задача о нём.\n",
                cut.id
            )
        } else {
            cut.body.clone()
        };
        let rule = Rule {
            id: match RuleId::parse(&cut.id) {
                Ok(id) => id,
                Err(error) => return fail(&error.to_string()),
            },
            title: cut.title.clone(),
            level: Level::Personal,
            absolute: cut.absolute,
            enforced: false,
            paths: Vec::new(),
            body: normalize_body(&body),
            extra: Vec::new(),
        };
        if let Err(error) = write_rule(&rules_dir, &rule) {
            return fail(&format!("rule {}: {error}", cut.id));
        }
    }
    let rest = format!(
        "{}\nПравила — отдельными файлами: `vibememory rule list`. Их раскладывает VibeMemory: Claude Code читает \
         `~/.claude/rules/`, другие агенты — свои файлы.\n",
        preamble.trim_end()
    );
    if let Err(error) = std::fs::write(&path, rest) {
        return fail(&format!("{}: {error}", path.display()));
    }
    println!(
        "{} rule(s) written; CLAUDE.md keeps its preamble. Every agent gets them with the next run",
        cuts.len()
    );
    ExitCode::SUCCESS
}

fn write_skill(dir: &Path, cut: &Cut) -> Result<(), String> {
    let description: String = cut
        .body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim_start_matches(['-', '*', ' '])
        .chars()
        .take(300)
        .collect();
    let description = if description.is_empty() {
        cut.title.clone()
    } else {
        description
    };
    let text = format!(
        "---\nname: {}\ndescription: {}\n---\n# {}\n\n{}",
        cut.id,
        vibememory_core::rules::frontmatter::scalar(&description),
        cut.title,
        cut.body
    );
    parse_skill(&cut.id, &text).map_err(|error| error.to_string())?;
    let skill = dir.join(&cut.id);
    std::fs::create_dir_all(&skill).map_err(|error| error.to_string())?;
    std::fs::write(skill.join(SKILL_FILE), text).map_err(|error| error.to_string())
}
