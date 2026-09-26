//! What a team's store keeps when its sessions are switched off: its memory and the files the host
//! itself writes. Everything else — transcripts, machines' directories — goes, history and all:
//! git gives no room back otherwise, and giving it back is the point of switching sessions off.

pub use vibememory_core::team_store::GENERATION_FILE;

/// Whether a path of the store's tree stays when sessions are off.
#[must_use]
pub fn kept_without_sessions(path: &str) -> bool {
    if path == ".gitattributes" || path == GENERATION_FILE {
        return true;
    }
    let Some(inside) = path
        .strip_prefix("projects/")
        .and_then(|rest| rest.split_once('/').map(|(_, inside)| inside))
    else {
        return false;
    };
    inside == "memory.jsonl" || inside.starts_with("memory/")
}

/// The generation after `current`, the text of the file or nothing at all.
#[must_use]
pub fn next_generation(current: Option<&str>) -> u64 {
    current
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .saturating_add(1)
}
