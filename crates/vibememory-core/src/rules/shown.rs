//! A team's rules on a machine go out only as they were shown to the person.
//!
//! A team's rule is an instruction every member's agents carry out, written by the team's owner or an admin. A rule
//! that arrives with a pull is laid out once the person has been told what came and from whom: until then a new rule
//! waits, and a changed one keeps the text that was shown. Any change counts, flags included: `enforced` switched on
//! is a change the person hears about. A rule the team removed goes at once — taking an instruction away needs no
//! consent.

use std::collections::BTreeMap;

use super::rule::Rule;

/// What a rule not shown yet is to the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A rule the machine has not shown before.
    New,
    /// A rule shown before, changed since.
    Changed,
}

impl Change {
    /// The word the fixtures and the engine use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Changed => "changed",
        }
    }
}

/// A team's rule waiting to be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unseen<'a> {
    /// The rule as the team's store holds it now.
    pub rule: &'a Rule,
    /// New or changed.
    pub change: Change,
}

/// A team's rules split by what the person was shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gate<'a> {
    /// What goes out: each rule as it was last shown, by id.
    pub laid: Vec<&'a Rule>,
    /// What waits to be shown, by id.
    pub unseen: Vec<Unseen<'a>>,
}

/// Splits a team's rules as the store holds them by the versions this machine showed.
#[must_use]
pub fn gate<'a>(current: &'a [Rule], shown: &'a [Rule]) -> Gate<'a> {
    let shown: BTreeMap<&str, &Rule> = shown.iter().map(|rule| (rule.id.as_str(), rule)).collect();
    let mut sorted: Vec<&Rule> = current.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut gate = Gate::default();
    for rule in sorted {
        match shown.get(rule.id.as_str()) {
            Some(seen) if *seen == rule => gate.laid.push(rule),
            Some(seen) => {
                gate.laid.push(seen);
                gate.unseen.push(Unseen {
                    rule,
                    change: Change::Changed,
                });
            }
            None => gate.unseen.push(Unseen {
                rule,
                change: Change::New,
            }),
        }
    }
    gate
}
