//! What the host measures on disk. The server needs it for a team's quota on every write, the
//! host's own commands for the push guard and the report — one walk, so the three agree.

use std::path::Path;

/// Bytes of every file under `root`, symbolic links not followed, `skip` left out.
pub fn dir_size(root: &Path, skip: Option<&Path>) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if skip.is_some_and(|skip| path == skip) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}
