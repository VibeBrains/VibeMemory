//! Rules and skills on this machine: read from the stores, written out where each agent reads them, and an agent's
//! edit taken back into the rule it was made to.
//!
//! The person's rules go everywhere the person works: one file a rule in `~/.claude/rules/`, assembled into
//! `~/.dsh/AGENTS.md` and `~/.codex/AGENTS.md` behind the owner's own `CLAUDE.md`. A project's rules and its team's go
//! into the project's working directory, into files its git is told to ignore. Skills are linked, not copied, into
//! the shared `~/.agents/skills` and the agents' own directories, so an edit in any of them is the edit of the one in
//! the store.
//!
//! Every file written here is an agent's copy, and an edit in it is taken back: the markers say which rule and which
//! version it was written from. A rule changed in the store since then and in the agent's file too is a conflict —
//! the agent's version goes to the quarantine and the rule as stored is written back. The model —
//! `docs/spec/rulesAndSkills.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vibememory_core::desktop::roots::Roots;
use vibememory_core::rules::assembly::{self, Piece};
use vibememory_core::rules::layers::resolve;
use vibememory_core::rules::rule::{normalize_body, version_of};
use vibememory_core::rules::skill::{SKILL_FILE, parse_skill};
use vibememory_core::rules::{Level, Rule};

use crate::agents::Preset;

/// The person's rules, relative to the personal store: beside `CLAUDE.md` and the skills, which travel the same way.
pub const PERSONAL_RULES: &str = "config/rules";
/// The person's skills, relative to the personal store.
pub const PERSONAL_SKILLS: &str = "config/skills";
/// A team's rules and skills, relative to its store.
pub const TEAM_RULES: &str = "rules";
/// A team's skills, relative to its store.
pub const TEAM_SKILLS: &str = "skills";
/// What a project's rules and skills sit in, under `projects/<project>/`.
pub const PROJECT_RULES: &str = "rules";
/// What a project's skills sit in, under `projects/<project>/`.
pub const PROJECT_SKILLS: &str = "skills";
/// The prefix of the files the engine writes into a rules directory: a person's own files there are never touched.
pub const FILE_PREFIX: &str = "vm-";
/// What Claude Code writes into the skills directory on its own, from claude.ai: not the person's, not for other
/// agents.
pub const CLAUDE_SYNCED: &str = "synced";
/// Where proposals to a team wait for its owner or admins, in the team's store: `rules/<id>.<who>.md` and
/// `skills/<name>.<who>/`.
pub const PROPOSALS: &str = "proposals";
/// The single file DSH reads in a project beside its own instructions.
pub const PROJECT_AGENTS_FILE: &str = "AGENTS.local.md";
/// The file Codex reads in a directory instead of its `AGENTS.md`: the engine writes the project's `AGENTS.md` into
/// it whole, then the rules. Codex takes one file a directory — the override, else `AGENTS.md`, else a fallback name
/// — so no other name reaches it beside a project's own (measured with `codex debug prompt-input`, 0.145, 2026-10-08).
pub const CODEX_PROJECT_FILE: &str = "AGENTS.override.md";
/// A project's own instructions, the base of the file Codex reads.
pub const PROJECT_OWN_FILE: &str = "AGENTS.md";

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Projected {
    /// Agent files written or rewritten.
    pub written: Vec<String>,
    /// Rules (and `CLAUDE.md`, as `base`) changed in an agent's file and taken into the store.
    pub taken: Vec<String>,
    /// Rules changed in an agent's file and in the store both: the agent's version is in the quarantine.
    pub conflicts: Vec<String>,
    /// What could not be done in this run, said for a person.
    pub problems: Vec<String>,
    /// What is wrong with the files themselves — a rule that does not read, a skill an agent would refuse, a skill of
    /// the person's own under a stored name: the same every run until a person changes the file, so `doctor` says it
    /// and the tick does not repeat it every two minutes.
    pub warnings: Vec<String>,
}

/// The rules of a directory, by id, and what is wrong with the files that are not rules.
#[must_use]
pub fn read_rules(dir: &Path) -> (BTreeMap<String, Rule>, Vec<String>) {
    let mut rules = BTreeMap::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (rules, problems);
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|text| Rule::parse(&text).map_err(|error| error.to_string()))
        {
            Ok(rule) if rule.id.as_str() == stem => {
                rules.insert(stem, rule);
            }
            Ok(rule) => problems.push(format!(
                "{}: the file is named {stem}, the rule {}: a rule lives in <id>.md",
                path.display(),
                rule.id
            )),
            Err(error) => problems.push(format!("{}: {error}", path.display())),
        }
    }
    (rules, problems)
}

/// What is wrong with the person's rule and skill files, read without writing anything: what `doctor` says. An
/// assembled file over DSH's budget is named too: DSH drops it whole and says nothing.
#[must_use]
pub fn check(store: &Path, agents: &Agents) -> Vec<String> {
    let (_, mut warnings) = read_rules(&store.join(PERSONAL_RULES));
    let mut report = Projected::default();
    let skills = skills_of(&store.join(PERSONAL_SKILLS), &mut report);
    warnings.extend(report.warnings);
    for dir in &agents.skill_dirs {
        warnings.extend(own_skills(dir, &skills, store));
    }
    for (name, path) in &agents.assembled {
        let Some(dsh_home) = path.parent().filter(|_| *name == DSH_NAME) else {
            continue;
        };
        let budget = crate::mcp_config::dsh_budget(dsh_home);
        let size = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
        if usize::try_from(size).is_ok_and(|size| size > budget) {
            warnings.push(format!(
                "{}: {size} bytes, over DSH's budget of {budget} for instruction files — DSH drops the whole file; \
                 `vibememory mcp-config dsh` gives the line that raises it, `vibememory rules lint` the rules to make \
                 skills",
                path.display()
            ));
        }
    }
    warnings
}

/// How an agent's copy of the person's rules stands against the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentState {
    /// The agent, as a person names it.
    pub agent: &'static str,
    /// Its rules directory or its assembled file.
    pub path: PathBuf,
    /// What the engine wrote there, in bytes.
    pub bytes: u64,
    /// Rules the copy does not hold as the store does: missing, of another version, or gone from the store. The
    /// engine's next run writes them.
    pub behind: Vec<String>,
}

/// Each agent's copy of the person's rules against the store, read without writing: what `rules status` says.
#[must_use]
pub fn agent_states(store: &Path, agents: &Agents) -> Vec<AgentState> {
    let (canon, _) = read_rules(&store.join(PERSONAL_RULES));
    let marked = |pieces: Vec<Piece>, found: &mut BTreeMap<String, String>| {
        for piece in pieces {
            if let Piece::Rule { id, from, .. } = piece {
                found.insert(id, from);
            }
        }
    };
    let behind = |found: &BTreeMap<String, String>| -> Vec<String> {
        let mut ids: Vec<String> = canon
            .values()
            .filter(|rule| found.get(rule.id.as_str()) != Some(&rule.version()))
            .map(|rule| rule.id.as_str().to_owned())
            .chain(found.keys().filter(|id| !canon.contains_key(*id)).cloned())
            .collect();
        ids.sort();
        ids
    };
    let mut states = Vec::new();
    let mut found = BTreeMap::new();
    let mut bytes = 0;
    for path in engine_files(&agents.claude_rules) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        bytes += text.len() as u64;
        marked(assembly::disassemble(&text).unwrap_or_default(), &mut found);
    }
    states.push(AgentState {
        agent: CLAUDE_NAME,
        path: agents.claude_rules.clone(),
        bytes,
        behind: behind(&found),
    });
    for (name, path) in &agents.assembled {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut found = BTreeMap::new();
        marked(assembly::disassemble(&text).unwrap_or_default(), &mut found);
        states.push(AgentState {
            agent: name,
            path: path.clone(),
            bytes: text.len() as u64,
            behind: behind(&found),
        });
    }
    states
}

/// Writes a rule into its directory as `<id>.md`, through a temporary name: an agent may read it meanwhile.
///
/// # Errors
///
/// What the file system said.
pub fn write_rule(dir: &Path, rule: &Rule) -> Result<(), String> {
    write_if_changed(&dir.join(format!("{}.md", rule.id)), &rule.render()).map(|_| ())
}

/// Writes a file whole unless it already holds exactly this; says whether it wrote.
fn write_if_changed(path: &Path, text: &str) -> Result<bool, String> {
    if std::fs::read_to_string(path).is_ok_and(|now| now == text) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension("vibememory.tmp");
    std::fs::write(&temporary, text.as_bytes())
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(true)
}

/// Claude Code, as a person names it.
pub const CLAUDE_NAME: &str = "Claude Code";
/// DSH, as a person names it.
pub const DSH_NAME: &str = "DeepSeek Harness";
/// Codex, as a person names it.
pub const CODEX_NAME: &str = "Codex";

/// Where this machine's agents read the person's rules and skills.
#[derive(Debug, Clone)]
pub struct Agents {
    /// Claude Code's directory of rule files, `~/.claude/rules`.
    pub claude_rules: PathBuf,
    /// Files that hold every rule assembled, by agent: `~/.dsh/AGENTS.md`, `~/.codex/AGENTS.md` — only those of
    /// agents that live here.
    pub assembled: Vec<(&'static str, PathBuf)>,
    /// Skill directories the person's skills are linked into: `~/.agents/skills` when an agent that reads it lives
    /// here, `~/.codex/skills`.
    pub skill_dirs: Vec<PathBuf>,
    /// Whether DSH lives here: a project then also gets `AGENTS.local.md` and `.agents/skills`.
    pub dsh: bool,
    /// Whether Codex lives here: a project then also gets `AGENTS.override.md` and `.agents/skills`.
    pub codex: bool,
    /// Agents that live here and that `rules.agents` leaves out: their assembled file and skill links are given
    /// back — the engine's part taken out, the person's own text left.
    pub released: Released,
}

/// What the engine gives back of the agents the person left out of `rules.agents`.
#[derive(Debug, Clone, Default)]
pub struct Released {
    /// Their assembled files.
    pub files: Vec<PathBuf>,
    /// Their skill directories.
    pub skill_dirs: Vec<PathBuf>,
}

impl Agents {
    /// The agents of a machine whose Claude Code configuration is `config_dir` and whose home is `home`, as far as
    /// the person's `rules.agents` lets them have the rules; without a home only Claude Code is known.
    #[must_use]
    pub fn of(
        config_dir: &Path,
        home: Option<&Path>,
        settings: &crate::config::RulesConfig,
    ) -> Self {
        let mut assembled = Vec::new();
        let mut skill_dirs = Vec::new();
        let mut released = Released::default();
        let (mut dsh, mut codex) = (false, false);
        if let Some(home) = home {
            // DSH reads `~/.agents/skills` as the root agents share (read off its bundle, 2026-10-07)
            let found = [
                (Preset::Dsh, DSH_NAME, home.join(".agents").join("skills")),
                (
                    Preset::Codex,
                    CODEX_NAME,
                    Preset::Codex.home(home).join("skills"),
                ),
            ];
            for (preset, name, skills) in found {
                let agent_home = preset.home(home);
                if !agent_home.is_dir() {
                    continue;
                }
                if settings.allows(preset.name()) {
                    dsh |= preset == Preset::Dsh;
                    codex |= preset == Preset::Codex;
                    assembled.push((name, agent_home.join("AGENTS.md")));
                    skill_dirs.push(skills);
                } else {
                    released.files.push(agent_home.join("AGENTS.md"));
                    released.skill_dirs.push(skills);
                }
            }
        }
        Self {
            claude_rules: config_dir.join("rules"),
            assembled,
            skill_dirs,
            dsh,
            codex,
            released,
        }
    }
}

/// Whether an agent reads the rules the engine writes into its own files on this machine: Claude Code always, DSH
/// and Codex when they live here and `rules.agents` lets them have the rules. An agent that does not reads them only
/// as the memory server gives them.
#[must_use]
pub fn reads_rule_files(
    agent: &str,
    home: Option<&Path>,
    settings: &crate::config::RulesConfig,
) -> bool {
    if agent == crate::mcp_config::Client::ClaudeCode.agent() {
        return true;
    }
    home.is_some_and(|home| {
        [Preset::Dsh, Preset::Codex].into_iter().any(|preset| {
            preset.agent() == agent && preset.home(home).is_dir() && settings.allows(preset.name())
        })
    })
}

/// The owner's `CLAUDE.md` as the store holds it: the base of every assembled file.
struct Base {
    path: PathBuf,
    text: Option<String>,
    changed: bool,
}

/// Takes the edits of one agent file into the rules and the base, and gives back the text that is the person's own.
fn take_edits(
    pieces: Vec<Piece>,
    canon: &mut BTreeMap<String, Rule>,
    changed: &mut Vec<String>,
    base: Option<&mut Base>,
    quarantine: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    report: &mut Projected,
) -> Vec<String> {
    let mut outside = Vec::new();
    let mut base = base;
    for piece in pieces {
        match piece {
            Piece::Rule {
                id,
                from,
                title,
                body,
            } => {
                if version_of(&title, &body) == from {
                    continue;
                }
                match canon.get_mut(&id) {
                    Some(rule) if rule.version() == from => {
                        let mut edited = rule.clone();
                        edited.title = title;
                        edited.body = normalize_body(&body);
                        match edited.validate() {
                            Ok(()) => {
                                *rule = edited;
                                changed.push(id.clone());
                                report.taken.push(id);
                            }
                            Err(error) => report
                                .problems
                                .push(format!("rule {id}: the edit is not taken: {error}")),
                        }
                    }
                    _ => {
                        let text = format!("## {title}\n\n{body}");
                        match quarantine(&format!("rule-{id}"), &text) {
                            Ok(()) => report.conflicts.push(id),
                            Err(error) => report.problems.push(format!("rule {id}: {error}")),
                        }
                    }
                }
            }
            Piece::Base { from, text } => {
                let Some(base) = base.as_deref_mut() else {
                    continue;
                };
                if version_of("", &text) == from {
                    continue;
                }
                let stored = base.text.as_deref().map(|now| version_of("", now));
                if stored.as_deref() == Some(from.as_str()) {
                    base.text = Some(text);
                    base.changed = true;
                    report.taken.push("base".to_owned());
                } else {
                    let name = base
                        .path
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                    match quarantine(&name, &text) {
                        Ok(()) => report.conflicts.push("base".to_owned()),
                        Err(error) => report.problems.push(format!("{name}: {error}")),
                    }
                }
            }
            Piece::Outside(text) => {
                // the plain copy of `CLAUDE.md` 0.8.0 wrote is the base now, not the person's own text
                let is_base = base
                    .as_deref()
                    .and_then(|base| base.text.as_deref())
                    .is_some_and(|base| normalize_body(base) == text);
                if !is_base {
                    outside.push(text);
                }
            }
        }
    }
    outside
}

/// Reads an agent's file into pieces; a file that is not there is no pieces. A file whose markers were cut is left
/// alone and named: rewriting it would lose what the person wrote.
fn pieces_of(path: &Path, report: &mut Projected) -> Option<Vec<Piece>> {
    match std::fs::read_to_string(path) {
        Ok(text) => match assembly::disassemble(&text) {
            Ok(pieces) => Some(pieces),
            Err(error) => {
                report.problems.push(format!(
                    "{}: {error} — fix the markers or remove the file, and it is written again",
                    path.display()
                ));
                None
            }
        },
        Err(_) => Some(Vec::new()),
    }
}

/// The person's rules and skills, out to every agent of this machine, with the agents' edits taken in first.
///
/// `store` is the personal store's clone; `engine_dir` holds the quarantine.
#[must_use]
pub fn project_personal(
    store: &Path,
    engine_dir: &Path,
    agents: &Agents,
    stamp: &str,
) -> Projected {
    let mut report = Projected::default();
    let rules_dir = store.join(PERSONAL_RULES);
    let (mut canon, problems) = read_rules(&rules_dir);
    report.warnings.extend(problems);
    let base_path = store.join("config").join("CLAUDE.md");
    let mut base = Base {
        text: std::fs::read_to_string(&base_path).ok(),
        path: base_path,
        changed: false,
    };
    let mut changed = Vec::new();
    let mut quarantine = |name: &str, text: &str| -> Result<(), String> {
        crate::memory::quarantine(engine_dir, &format!("{name}-{stamp}.md"), text.as_bytes())
            .map(|_| ())
    };

    // the edits first, from every file, so that what is written afterwards holds them
    let claude_files = engine_files(&agents.claude_rules);
    for path in &claude_files {
        if let Some(pieces) = pieces_of(path, &mut report) {
            let _ = take_edits(
                pieces,
                &mut canon,
                &mut changed,
                None,
                &mut quarantine,
                &mut report,
            );
        }
    }
    // a file whose markers were cut is `None`: left alone, not rewritten
    let mut outsides: Vec<Option<Vec<String>>> = Vec::new();
    for (_, path) in &agents.assembled {
        let pieces = pieces_of(path, &mut report);
        outsides.push(pieces.map(|pieces| {
            take_edits(
                pieces,
                &mut canon,
                &mut changed,
                Some(&mut base),
                &mut quarantine,
                &mut report,
            )
        }));
    }
    let released = take_released(
        agents,
        &mut canon,
        &mut changed,
        &mut base,
        &mut quarantine,
        &mut report,
    );
    for id in &changed {
        if let Some(rule) = canon.get(id)
            && let Err(error) = write_rule(&rules_dir, rule)
        {
            report.problems.push(format!("rule {id}: {error}"));
        }
    }
    if base.changed
        && let Some(text) = &base.text
        && let Err(error) = write_if_changed(&base.path, text)
    {
        report.problems.push(format!("CLAUDE.md: {error}"));
    }

    // then everything written from the store
    let rules: Vec<&Rule> = canon.values().collect();
    write_rule_files(
        &agents.claude_rules,
        &rules
            .iter()
            .map(|rule| (*rule, &[][..]))
            .collect::<Vec<_>>(),
        &mut report,
    );
    for ((_, path), outside) in agents.assembled.iter().zip(outsides) {
        let Some(outside) = outside else {
            continue;
        };
        let pairs: Vec<(&Rule, &[Level])> = rules.iter().map(|rule| (*rule, &[][..])).collect();
        write_assembled(path, base.text.as_deref(), &pairs, &outside, &mut report);
    }
    for (path, outside) in released {
        give_back(path, &outside, &mut report);
    }
    link_personal_skills(store, agents, &mut report);
    report
}

/// The files of agents left out of `rules.agents` that the engine wrote: their edits are taken in like any agent's,
/// and what comes back is the person's own text of each, to keep when the file is given back.
fn take_released<'a>(
    agents: &'a Agents,
    canon: &mut BTreeMap<String, Rule>,
    changed: &mut Vec<String>,
    base: &mut Base,
    quarantine: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    report: &mut Projected,
) -> Vec<(&'a Path, Vec<String>)> {
    let mut released = Vec::new();
    for path in &agents.released.files {
        if std::fs::read_to_string(path).is_ok_and(|text| assembly::written_by_engine(&text))
            && let Some(pieces) = pieces_of(path, report)
        {
            let outside = take_edits(pieces, canon, changed, Some(base), quarantine, report);
            released.push((path.as_path(), outside));
        }
    }
    released
}

/// An assembled file written: the base when there is one, the rules, then the person's own text.
fn write_assembled(
    path: &Path,
    base: Option<&str>,
    rules: &[(&Rule, &[Level])],
    own: &[String],
    report: &mut Projected,
) {
    let mut text = assembly::assemble(base, rules);
    for piece in own {
        text.push('\n');
        text.push_str(piece);
    }
    match write_if_changed(path, &text) {
        Ok(true) => report.written.push(path.display().to_string()),
        Ok(false) => {}
        Err(error) => report.problems.push(error),
    }
}

/// An agent's file given back: only the person's own text stays, and a file that held nothing else goes.
fn give_back(path: &Path, outside: &[String], report: &mut Projected) {
    let result = if outside.is_empty() {
        std::fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))
    } else {
        write_if_changed(path, &outside.join("\n")).map(|_| ())
    };
    match result {
        Ok(()) => report.written.push(path.display().to_string()),
        Err(error) => report.problems.push(error),
    }
}

/// The person's skills linked for the agents that get them, and the engine's links taken from those left out.
fn link_personal_skills(store: &Path, agents: &Agents, report: &mut Projected) {
    let skills = skills_of(&store.join(PERSONAL_SKILLS), report);
    for dir in &agents.skill_dirs {
        link_skills(dir, &skills, store, report);
    }
    for dir in &agents.released.skill_dirs {
        if !agents.skill_dirs.contains(dir) {
            unlink_skills(dir, &BTreeMap::new(), store, report);
        }
    }
}

/// The files the engine wrote into a rules directory: `vm-*.md` that start with its header line.
fn engine_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(FILE_PREFIX))
                && std::fs::read_to_string(path)
                    .is_ok_and(|text| assembly::written_by_engine(&text))
        })
        .collect();
    files.sort();
    files
}

/// One file a rule in a rules directory, and the engine's files of rules no longer there removed.
fn write_rule_files(dir: &Path, rules: &[(&Rule, &[Level])], report: &mut Projected) {
    let wanted: BTreeMap<PathBuf, String> = rules
        .iter()
        .map(|(rule, replaces)| {
            (
                dir.join(format!("{FILE_PREFIX}{}.md", rule.id)),
                assembly::rule_file(rule, replaces),
            )
        })
        .collect();
    for stale in engine_files(dir) {
        if !wanted.contains_key(&stale) {
            match std::fs::remove_file(&stale) {
                Ok(()) => report.written.push(stale.display().to_string()),
                Err(error) => report
                    .problems
                    .push(format!("{}: {error}", stale.display())),
            }
        }
    }
    for (path, text) in wanted {
        match write_if_changed(&path, &text) {
            Ok(true) => report.written.push(path.display().to_string()),
            Ok(false) => {}
            Err(error) => report.problems.push(error),
        }
    }
}

/// The valid skills of a directory, by name; a skill an agent would refuse is named instead.
fn skills_of(dir: &Path, report: &mut Projected) -> BTreeMap<String, PathBuf> {
    let mut skills = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return skills;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name == CLAUDE_SYNCED || name.starts_with('.') || !path.is_dir() {
            continue;
        }
        match std::fs::read_to_string(path.join(SKILL_FILE))
            .map_err(|error| error.to_string())
            .and_then(|text| parse_skill(&name, &text).map_err(|error| error.to_string()))
        {
            Ok(_) => {
                skills.insert(name, path);
            }
            Err(error) => report.warnings.push(format!("skill {name}: {error}")),
        }
    }
    skills
}

/// Links each skill into an agent's skills directory; the engine's links to skills no longer there are removed, and
/// a skill of the same name the person put there by hand is left as it is.
fn link_skills(
    dir: &Path,
    skills: &BTreeMap<String, PathBuf>,
    store: &Path,
    report: &mut Projected,
) {
    if let Err(error) = std::fs::create_dir_all(dir) {
        report.problems.push(format!("{}: {error}", dir.display()));
        return;
    }
    unlink_skills(dir, skills, store, report);
    report.warnings.extend(own_skills(dir, skills, store));
    for (name, target) in skills {
        let link = dir.join(name);
        if std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        match crate::dir_link::create(target, &link) {
            Ok(()) => report.written.push(link.display().to_string()),
            Err(error) => report.problems.push(format!("{}: {error}", link.display())),
        }
    }
}

/// The stored skills a skill of the person's own stands in place of in an agent's directory: left as it is, and said.
fn own_skills(dir: &Path, skills: &BTreeMap<String, PathBuf>, store: &Path) -> Vec<String> {
    skills
        .keys()
        .map(|name| (name, dir.join(name)))
        .filter(|(_, link)| std::fs::symlink_metadata(link).is_ok() && !is_engine_link(link, store))
        .map(|(name, link)| {
            format!(
                "{}: a skill of the person's own by this name is there; the stored {name} is not linked over it",
                link.display()
            )
        })
        .collect()
}

/// Whether a path is a link the engine made: one into the store.
fn is_engine_link(path: &Path, store: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink())
        && std::fs::read_link(path).is_ok_and(|target| target.starts_with(store))
}

/// Removes the engine's links in an agent's skills directory that do not lead to `skills` as they are now.
fn unlink_skills(
    dir: &Path,
    skills: &BTreeMap<String, PathBuf>,
    store: &Path,
    report: &mut Projected,
) {
    let ours = |path: &Path| is_engine_link(path, store);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if ours(&path)
                && skills
                    .get(&name)
                    .is_none_or(|target| std::fs::read_link(&path).ok().as_ref() != Some(target))
            {
                match crate::dir_link::remove(&path) {
                    Ok(()) => report.written.push(path.display().to_string()),
                    Err(error) => report.problems.push(error),
                }
            }
        }
    }
}

/// The working directories of this machine's projects in a store: its own link records, translated to this machine.
fn working_directories(
    store: &Path,
    machine_id: &str,
    roots: &Roots,
) -> BTreeMap<String, Vec<PathBuf>> {
    let mut found: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for record in crate::links_file::read(store, machine_id).links {
        // a record written without roots holds this machine's own path, as the tick's import writes it
        let local = roots
            .to_local(&record.cwd)
            .unwrap_or_else(|_| record.cwd.clone());
        let path = PathBuf::from(local);
        if path.is_dir() {
            let dirs = found.entry(record.name.clone()).or_default();
            if !dirs.contains(&path) {
                dirs.push(path);
            }
        }
    }
    found
}

/// One store's run over its projects.
#[derive(Debug, Clone, Copy)]
pub struct ProjectsRun<'a> {
    /// The store's clone.
    pub store: &'a Path,
    /// Where the quarantine is.
    pub engine_dir: &'a Path,
    /// This machine's name in the store: its link records say where its projects are.
    pub machine_id: &'a str,
    /// Named roots, for the working directories.
    pub roots: &'a Roots,
    /// The person's rules: not written into a project, but what a project's rule replaces is told by them.
    pub personal: &'a [Rule],
    /// The personal store, whose git holds the person's rules' history: a project in `merge` or `override` mode is
    /// measured by it.
    pub personal_store: &'a Path,
    /// The team, when the store is a team's: its rules and skills join every project of it.
    pub team: Option<&'a str>,
    /// The agents of this machine.
    pub agents: &'a Agents,
    /// The moment, for the quarantine's names.
    pub stamp: &'a str,
}

/// The rules and skills of the projects in one store, out into their working directories, with edits taken in.
#[must_use]
pub fn project_projects(run: &ProjectsRun<'_>) -> Projected {
    let &ProjectsRun {
        store,
        engine_dir,
        machine_id,
        roots,
        personal,
        personal_store: _,
        team,
        agents,
        stamp,
    } = run;
    let mut report = Projected::default();
    // a team's rules go out as the person was shown them: a new one waits for `SessionStart` or `rules status`
    let team_rules: BTreeMap<String, Rule> = if team.is_some() {
        let held = crate::rules_shown::team_rules(store, false);
        report.warnings.extend(held.warnings);
        held.laid
            .into_iter()
            .map(|rule| (rule.id.as_str().to_owned(), rule))
            .collect()
    } else {
        BTreeMap::new()
    };
    let team_skills = if team.is_some() {
        skills_of(&store.join(TEAM_SKILLS), &mut report)
    } else {
        BTreeMap::new()
    };
    let mut quarantine = |name: &str, text: &str| -> Result<(), String> {
        crate::memory::quarantine(engine_dir, &format!("{name}-{stamp}.md"), text.as_bytes())
            .map(|_| ())
    };
    for (project, dirs) in working_directories(store, machine_id, roots) {
        let project_dir = store.join("projects").join(&project);
        let rules_dir = project_dir.join(PROJECT_RULES);
        let (mut project_rules, problems) = read_rules(&rules_dir);
        report.warnings.extend(problems);
        let mut skills = team_skills.clone();
        skills.extend(skills_of(&project_dir.join(PROJECT_SKILLS), &mut report));

        // edits: a project rule's into the store; a team rule changes through the team's owner or admins
        let mut changed = Vec::new();
        let mut outsides = Outsides::new();
        let mut edits = ProjectEdits {
            team_rules: &team_rules,
            project_rules: &mut project_rules,
            changed: &mut changed,
            quarantine: &mut quarantine,
            report: &mut report,
        };
        for cwd in &dirs {
            edits.take_dir(cwd, &mut outsides);
        }
        for id in &changed {
            if let Some(rule) = project_rules.get(id)
                && let Err(error) = write_rule(&rules_dir, rule)
            {
                report.problems.push(format!("rule {id}: {error}"));
            }
        }

        let team_list: Vec<Rule> = team_rules.values().cloned().collect();
        let project_list: Vec<Rule> = project_rules.values().cloned().collect();
        let resolved = resolve(personal, &team_list, &project_list);
        let in_force: Vec<(&Rule, &[Level])> = resolved
            .rules
            .iter()
            .filter(|rule| rule.rule.level != Level::Personal)
            .map(|rule| (&rule.rule, rule.replaces.as_slice()))
            .collect();
        for cwd in &dirs {
            write_project(
                &ProjectDir {
                    cwd,
                    rules: &in_force,
                    skills: &skills,
                    outsides: &outsides,
                },
                store,
                agents,
                &mut report,
            );
        }
        apply_mode(run, &project, &dirs, &mut quarantine, &mut report);
    }
    report
}

/// The person's own text in a project's assembled files, by file: kept when the engine writes them again.
type Outsides = BTreeMap<PathBuf, Vec<String>>;

/// Whether a file is one the engine wrote.
fn is_engine_file(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| assembly::written_by_engine(&text))
}

/// What a project's files take edits into.
struct ProjectEdits<'a, 'q> {
    team_rules: &'a BTreeMap<String, Rule>,
    project_rules: &'a mut BTreeMap<String, Rule>,
    changed: &'a mut Vec<String>,
    quarantine: &'a mut (dyn FnMut(&str, &str) -> Result<(), String> + 'q),
    report: &'a mut Projected,
}

impl ProjectEdits<'_, '_> {
    /// Takes the edits of one working directory's files; the person's own text of the assembled ones goes into
    /// `outsides`. An edit of the project's own part of the file Codex reads goes back into its `AGENTS.md`.
    fn take_dir(&mut self, cwd: &Path, outsides: &mut Outsides) {
        for path in engine_files(&cwd.join(".claude").join("rules")) {
            if let Some(pieces) = pieces_of(&path, self.report) {
                let _ = self.take(&path, pieces, None);
            }
        }
        let local = cwd.join(PROJECT_AGENTS_FILE);
        if let Some(pieces) = pieces_of(&local, self.report) {
            let outside = self.take(&local, pieces, None);
            outsides.insert(local, outside);
        }
        let codex_file = cwd.join(CODEX_PROJECT_FILE);
        if !is_engine_file(&codex_file) {
            return;
        }
        let Some(pieces) = pieces_of(&codex_file, self.report) else {
            return;
        };
        let own = cwd.join(PROJECT_OWN_FILE);
        let mut base = Base {
            text: std::fs::read_to_string(&own).ok(),
            path: own,
            changed: false,
        };
        let outside = self.take(&codex_file, pieces, Some(&mut base));
        if base.changed
            && let Some(text) = &base.text
        {
            match write_if_changed(&base.path, text) {
                Ok(_) => self.report.written.push(base.path.display().to_string()),
                Err(error) => self.report.problems.push(error),
            }
        }
        outsides.insert(codex_file, outside);
    }

    /// Takes one file's edits: a project rule's into the store, a team rule's into the quarantine — a team's rule
    /// changes through its owner or admins. Gives back the person's own text of the file.
    fn take(&mut self, path: &Path, pieces: Vec<Piece>, base: Option<&mut Base>) -> Vec<String> {
        let team_rules = self.team_rules;
        let project_rules = &*self.project_rules;
        let (team_pieces, project_pieces): (Vec<Piece>, Vec<Piece>) =
            pieces.into_iter().partition(|piece| {
                matches!(piece, Piece::Rule { id, .. } if team_rules.contains_key(id) && !project_rules.contains_key(id))
            });
        let outside = take_edits(
            project_pieces,
            self.project_rules,
            self.changed,
            base,
            self.quarantine,
            self.report,
        );
        for piece in team_pieces {
            if let Piece::Rule {
                id,
                from,
                title,
                body,
            } = piece
                && version_of(&title, &body) != from
            {
                let text = format!("## {title}\n\n{body}");
                if (self.quarantine)(&format!("team-rule-{id}"), &text).is_ok() {
                    self.report.problems.push(format!(
                        "team rule {id} was changed in {}: a team's rule changes through its owner or admins — the \
                         edit is in the quarantine, propose it with `vibememory rule add --level team`",
                        path.display()
                    ));
                }
            }
        }
        outside
    }
}

/// A project's own rule files in the mode the person chose for it; `advise` leaves them to `rules sync`.
fn apply_mode(
    run: &ProjectsRun<'_>,
    project: &str,
    dirs: &[PathBuf],
    quarantine: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    report: &mut Projected,
) {
    let project_dir = run.store.join("projects").join(project);
    let mode = crate::rules_sync::mode_of(&project_dir);
    if mode != crate::rules_sync::Mode::Advise {
        let histories = crate::rules_sync::histories(
            run.personal_store,
            run.store,
            run.team.is_some(),
            project,
        );
        for cwd in dirs {
            for judged in crate::rules_sync::judge_project(cwd, &histories) {
                match crate::rules_sync::apply(&judged, mode, &histories, quarantine) {
                    Ok(done) if done.dropped + done.updated + done.set_aside > 0 => {
                        report.written.push(judged.file.path.display().to_string());
                    }
                    Ok(_) => {}
                    Err(error) => report
                        .problems
                        .push(format!("{}: {error}", judged.file.path.display())),
                }
            }
        }
    }
}

/// One working directory and what goes into it.
struct ProjectDir<'a> {
    cwd: &'a Path,
    rules: &'a [(&'a Rule, &'a [Level])],
    skills: &'a BTreeMap<String, PathBuf>,
    outsides: &'a Outsides,
}

/// One working directory: the rule files, `AGENTS.local.md` when DSH lives here, `AGENTS.override.md` when Codex
/// does, the skill links, and git told to ignore all of them.
fn write_project(dir: &ProjectDir<'_>, store: &Path, agents: &Agents, report: &mut Projected) {
    let ProjectDir {
        cwd,
        rules,
        skills,
        outsides,
    } = *dir;
    let claude_rules = cwd.join(".claude").join("rules");
    if !rules.is_empty() || !engine_files(&claude_rules).is_empty() {
        write_rule_files(&claude_rules, rules, report);
    }
    let own = |path: &Path| outsides.get(path).cloned().unwrap_or_default();
    // a file whose markers were cut is named when its edits are taken, and left as it is: rewriting it would lose
    // what the person wrote
    let cut = |path: &Path| path.exists() && !outsides.contains_key(path);
    let local = cwd.join(PROJECT_AGENTS_FILE);
    if !cut(&local) {
        if agents.dsh && !rules.is_empty() {
            write_assembled(&local, None, rules, &own(&local), report);
        } else if is_engine_file(&local) {
            give_back(&local, &own(&local), report);
        }
    }
    let codex_file = cwd.join(CODEX_PROJECT_FILE);
    if codex_file.exists() && !is_engine_file(&codex_file) {
        if agents.codex && !rules.is_empty() {
            report.warnings.push(format!(
                "{}: the person's own file — Codex reads it instead of AGENTS.md, and the project's rules do not \
                 reach Codex through it",
                codex_file.display()
            ));
        }
    } else if !cut(&codex_file) {
        if agents.codex && !rules.is_empty() {
            let base = std::fs::read_to_string(cwd.join(PROJECT_OWN_FILE)).ok();
            write_assembled(
                &codex_file,
                base.as_deref(),
                rules,
                &own(&codex_file),
                report,
            );
        } else if codex_file.exists() {
            give_back(&codex_file, &own(&codex_file), report);
        }
    }
    // Codex reads a project's `.agents/skills` as DSH does (measured with `codex debug prompt-input`)
    let mut skill_dirs = vec![cwd.join(".claude").join("skills")];
    let shared = cwd.join(".agents").join("skills");
    if agents.dsh || agents.codex {
        skill_dirs.push(shared);
    } else {
        // no agent here reads it any more: the engine's links go, the person's own skills stay
        unlink_skills(&shared, &BTreeMap::new(), store, report);
    }
    for dir in &skill_dirs {
        if !skills.is_empty() || dir.is_dir() {
            link_skills(dir, skills, store, report);
        }
    }
    if rules.is_empty() && skills.is_empty() {
        return;
    }
    let mut ignored = vec![
        format!("/.claude/rules/{FILE_PREFIX}*.md"),
        format!("/{PROJECT_AGENTS_FILE}"),
        format!("/{CODEX_PROJECT_FILE}"),
    ];
    for name in skills.keys() {
        ignored.push(format!("/.claude/skills/{name}"));
        ignored.push(format!("/.agents/skills/{name}"));
    }
    if let Err(error) = exclude(cwd, &ignored) {
        report.problems.push(format!("{}: {error}", cwd.display()));
    }
}

const EXCLUDE_BEGIN: &str = "# vibememory: rules and skills written for agents — begin";
const EXCLUDE_END: &str = "# vibememory: rules and skills written for agents — end";

/// The engine's block of the working directory's `.git/info/exclude`: git leaves its files out of every listing, and
/// the project's `.gitignore`, which is the project's, is not touched. A directory that is not a git checkout has
/// nothing to tell.
fn exclude(cwd: &Path, patterns: &[String]) -> Result<(), String> {
    let Some(path) = crate::git::run_with_timeout(
        crate::git::command(cwd, &["rev-parse", "--git-path", "info/exclude"]),
        std::time::Duration::from_secs(10),
    )?
    else {
        return Ok(());
    };
    let path = cwd.join(path.trim());
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut kept = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        match line {
            EXCLUDE_BEGIN => inside = true,
            EXCLUDE_END => inside = false,
            _ if !inside => kept.push(line.to_owned()),
            _ => {}
        }
    }
    while kept.last().is_some_and(String::is_empty) {
        kept.pop();
    }
    let mut new = kept.join("\n");
    if !new.is_empty() {
        new.push_str("\n\n");
    }
    new.push_str(EXCLUDE_BEGIN);
    new.push('\n');
    for pattern in patterns {
        new.push_str(pattern);
        new.push('\n');
    }
    new.push_str(EXCLUDE_END);
    new.push('\n');
    write_if_changed(&path, &new).map(|_| ())
}
