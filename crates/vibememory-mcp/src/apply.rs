//! What applying an access snapshot does to the host: the team directories and `vmgit`'s keys.
//!
//! Pure: the snapshot and the names under `teams/` in; the steps and the problems out. Nothing is
//! ever deleted — a deleted team's directory is renamed, a directory the snapshot does not know is
//! left as it is, and the adopted store is not touched at all: its settings are the owner's. The
//! codes of the problems are those of `docs/manuals/hostStatusSpec.md`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::access::Snapshot;
use crate::layout::{TeamDir, retired_dir, store_dir, team_dir};
use crate::status::Problem;

/// What to do with one team's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Create `teams/<slug>.git` and make its first commit.
    Create {
        /// The team's slug.
        slug: String,
    },
    /// The directory is there: settings and hook again, and `.gitattributes` brought to the
    /// engine's text.
    Keep {
        /// The team's slug.
        slug: String,
    },
    /// Rename `teams/<slug>.git` of a deleted team to `teams/<slug>.deleted-<date>.git`.
    Retire {
        /// The team's slug.
        slug: String,
        /// The directory's new name.
        to: String,
    },
}

/// The steps of one application, in the order of the slugs, and what will not be done.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// What to do.
    pub steps: Vec<Step>,
    /// What is not done, and why.
    pub problems: Vec<Problem>,
}

fn problem(code: &str, slug: &str, detail: String) -> Problem {
    Problem {
        code: code.to_owned(),
        team: Some(slug.to_owned()),
        detail,
    }
}

/// Plans the application of `snapshot` to a host whose `teams/` holds `dirs`.
#[must_use]
pub fn plan(snapshot: &Snapshot, dirs: &[String]) -> Plan {
    let present: BTreeSet<&str> = dirs.iter().map(String::as_str).collect();
    let retired = |slug: &str| {
        dirs.iter().find(
            |dir| matches!(team_dir(dir), Some(TeamDir::Deleted { slug: of, .. }) if of == slug),
        )
    };
    let mut plan = Plan::default();
    for (slug, team) in &snapshot.teams {
        if team.adopted {
            continue;
        }
        let live = store_dir(slug);
        let has_live = present.contains(live.as_str());
        match &team.deleted {
            Some(date) => {
                if !has_live {
                    continue;
                }
                let to = retired_dir(slug, date);
                if present.contains(to.as_str()) {
                    plan.problems.push(problem(
                        "retireBlocked",
                        slug,
                        format!("teams/{to} exists already; teams/{live} is left as it is"),
                    ));
                } else {
                    plan.steps.push(Step::Retire {
                        slug: slug.clone(),
                        to,
                    });
                }
            }
            None if has_live => plan.steps.push(Step::Keep { slug: slug.clone() }),
            None => match retired(slug) {
                Some(dir) => {
                    plan.problems
                        .push(problem("slugRetired", slug, format!("teams/{dir} exists")));
                }
                None => plan.steps.push(Step::Create { slug: slug.clone() }),
            },
        }
    }
    plan
}

/// `vmgit`'s `authorized_keys`: one line per key of the snapshot, in the order of their ids, each
/// bound to `<binary> shell <id>` and stripped of every other ssh right by `restrict`.
///
/// The key material is what the snapshot's check let through — `ssh-ed25519` and the base64 of a
/// 32-byte key, one line, no comment — so no line can carry options of its own.
#[must_use]
pub fn authorized_keys(snapshot: &Snapshot, binary: &str) -> String {
    let mut keys: Vec<_> = snapshot.keys.iter().collect();
    keys.sort_by(|left, right| left.id.cmp(&right.id));
    let mut text = String::from(
        "# Written by vibememory-mcp access-apply from the access snapshot; edits are overwritten.\n",
    );
    for key in keys {
        // Writing into a String cannot fail.
        let _ = writeln!(
            text,
            "restrict,command=\"{binary} shell {id}\" {material} {id}",
            id = key.id,
            material = key.public_key
        );
    }
    text
}
