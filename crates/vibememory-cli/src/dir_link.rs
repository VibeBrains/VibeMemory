//! Directory links: the one way the engine makes and removes them, on every platform.
//!
//! A symlink on unix. A junction on Windows: a directory symlink there needs an administrator or
//! Developer Mode, a junction needs neither, and the design required junctions from the start
//! (`docs/knowledge/design/engineConstraints.md`). The code made `symlink_dir` anyway, in five
//! copies, and the first `switch` or `SessionStart` on an ordinary account would have failed.
//!
//! Reading a link needs nothing of its own. The standard library gives a junction the answer it
//! gives a symlink — `FileType::new` checks the name-surrogate bit of the reparse tag, which
//! `IO_REPARSE_TAG_MOUNT_POINT` carries — and `read_link` follows both; read in the source of
//! 1.97.1 (`library/std/src/sys/fs/windows.rs`). So every `is_symlink()` in the engine already
//! tells a junction from a real directory.

use std::path::Path;

/// Creates a directory link at `link` pointing at `target`.
///
/// # Errors
///
/// What the system said.
pub fn create(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(|error| error.to_string())
    }
    #[cfg(windows)]
    {
        junction::create(target, link).map_err(|error| error.to_string())
    }
}

/// Removes a directory link — the link, never what it points at.
///
/// `remove_file` on unix. On Windows a junction is a directory entry that `remove_file` refuses;
/// `remove_dir` is `RemoveDirectoryW`, which takes the junction away and leaves the target whole.
///
/// # Errors
///
/// What the system said, with the path.
pub fn remove(link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let removed = std::fs::remove_file(link);
    #[cfg(windows)]
    let removed = std::fs::remove_dir(link);
    removed.map_err(|error| format!("{}: {error}", link.display()))
}
