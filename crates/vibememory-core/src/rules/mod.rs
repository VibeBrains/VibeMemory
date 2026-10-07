//! Rules and skills for every agent: the person's, a team's and a project's, kept as records in the store, written
//! out for each agent in the form it reads, and compared with what a project already holds.
//!
//! Pure logic: reading and checking the formats, stacking the levels, assembling an agent's file and reading it back,
//! judging a project's own rule files against the rules' histories. Files, git and agents are the engine's business.
//! The model behind it — `docs/spec/rulesAndSkills.md`.

pub mod assembly;
pub mod compare;
pub mod frontmatter;
pub mod layers;
pub mod rule;
pub mod skill;

pub use rule::{Level, Rule, RuleId};

/// What is wrong with a rule or a skill.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesError {
    /// The file does not start with a `---` frontmatter, or never closes it.
    #[error("the file has no frontmatter between --- lines")]
    NoFrontmatter,
    /// A frontmatter line this format does not read.
    #[error("frontmatter: {0}")]
    Frontmatter(String),
    /// A required key is missing.
    #[error("{0} is missing")]
    Missing(&'static str),
    /// A rule id that is not a kebab-case slug.
    #[error("the id {0:?} is not kebab-case of at most 64 characters")]
    BadId(String),
    /// A level other than personal, team or project.
    #[error("the level {0:?} is not personal, team or project")]
    BadLevel(String),
    /// A flag that is not `true` or `false`.
    #[error("{0} is true or false")]
    NotBoolean(&'static str),
    /// Anything else the format refuses, said in words.
    #[error("{0}")]
    Invalid(String),
    /// The text carries an issued token: it would travel to every machine and every member.
    #[error(
        "the text holds a vibememory token (vmt_…): take it out — rules and skills travel to every machine"
    )]
    HoldsToken,
    /// An agent's file whose markers were cut by hand.
    #[error("markers: {0}")]
    Markers(String),
}
