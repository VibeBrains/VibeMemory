//! The rules in force for a project: the levels stacked bottom up, a rule above replacing one below with the same id.
//!
//! Two rules are not replaced: a person's `absolute` one, and a team's `enforced` one by anything below the team.
//! When a team's `enforced` rule and a person's `absolute` rule share an id and say different things, nothing is
//! chosen: both stay in force and the pair is named, because which of the two wins is the people's decision.

use std::collections::BTreeMap;

use super::rule::{Level, Rule, RuleId};

/// A rule in force, and what it stands in for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InForce {
    /// The rule.
    pub rule: Rule,
    /// The levels whose rule of the same id it replaces: an agent that holds both is told so.
    pub replaces: Vec<Level>,
}

/// A rule above that could not replace the one below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The rule that stays.
    pub kept: Rule,
    /// The one that wanted to replace it.
    pub refused: Rule,
}

/// The stack resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    /// What is in force, by level and then id.
    pub rules: Vec<InForce>,
    /// Rules above that stayed out because the rule below is absolute or enforced.
    pub kept: Vec<Kept>,
    /// A team's enforced rule against a person's absolute one, both in force: for a person to settle.
    pub conflicts: Vec<(Rule, Rule)>,
}

/// Stacks the levels: `personal`, then the team's, then the project's. A rule in the wrong list is taken at the
/// level it says it is, so a file moved by hand cannot climb the stack.
#[must_use]
pub fn resolve(personal: &[Rule], team: &[Rule], project: &[Rule]) -> Resolved {
    let mut by_id: BTreeMap<RuleId, InForce> = BTreeMap::new();
    let mut resolved = Resolved::default();
    let mut all: Vec<&Rule> = personal.iter().chain(team).chain(project).collect();
    all.sort_by_key(|rule| rule.level);
    for rule in all {
        match by_id.get_mut(&rule.id) {
            None => {
                by_id.insert(
                    rule.id.clone(),
                    InForce {
                        rule: rule.clone(),
                        replaces: Vec::new(),
                    },
                );
            }
            Some(below) if below.rule.level == rule.level => {
                // two files of one level with one id: the first stays, the second is named
                resolved.kept.push(Kept {
                    kept: below.rule.clone(),
                    refused: rule.clone(),
                });
            }
            Some(below) => {
                let held =
                    (below.rule.absolute) || (below.rule.enforced && rule.level > Level::Team);
                if !held {
                    let mut replaces = below.replaces.clone();
                    replaces.push(below.rule.level);
                    *below = InForce {
                        rule: rule.clone(),
                        replaces,
                    };
                } else if below.rule.absolute
                    && rule.enforced
                    && below.rule.version() != rule.version()
                {
                    resolved.conflicts.push((below.rule.clone(), rule.clone()));
                } else if below.rule.version() != rule.version() {
                    resolved.kept.push(Kept {
                        kept: below.rule.clone(),
                        refused: rule.clone(),
                    });
                }
            }
        }
    }
    let mut rules: Vec<InForce> = by_id.into_values().collect();
    for (_, enforced) in &resolved.conflicts {
        rules.push(InForce {
            rule: enforced.clone(),
            replaces: Vec::new(),
        });
    }
    rules.sort_by(|a, b| (a.rule.level, &a.rule.id).cmp(&(b.rule.level, &b.rule.id)));
    resolved.rules = rules;
    resolved
}
