//! `~/.vibememory/bin` on the user's `PATH`, so `vibememory` answers by name in every new terminal.
//!
//! Only the user's own settings change, never the system's. On macOS and Linux that is a marked
//! block in the shell's startup file — added once, recognised on every later install; on Windows the
//! user's `Path` variable, read and written with PowerShell. A terminal already open keeps the `PATH`
//! it started with, so the install says to open a new one.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The first line of the block this module writes into a shell's startup file.
pub const BLOCK_START: &str = "# >>> vibememory >>>";

/// Its last line.
pub const BLOCK_END: &str = "# <<< vibememory <<<";

/// The block that puts `bin` in front of `PATH`, in a POSIX shell's words.
#[must_use]
pub fn shell_block(bin: &Path) -> String {
    let quoted = bin
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("{BLOCK_START}\nexport PATH=\"{quoted}:$PATH\"\n{BLOCK_END}\n")
}

/// The startup file a shell reads, by the shell's name and the system: zsh reads `.zshrc`; bash on
/// macOS starts every terminal as a login shell and reads `.bash_profile`, on Linux `.bashrc`; any
/// other shell `.profile`.
#[must_use]
pub fn startup_file(home: &Path, shell: &str, macos: bool) -> PathBuf {
    let name = Path::new(shell)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    home.join(match (name.as_str(), macos) {
        ("zsh", _) => ".zshrc",
        ("bash", true) => ".bash_profile",
        ("bash", false) => ".bashrc",
        _ => ".profile",
    })
}

/// Whether a startup file's text already carries the block.
#[must_use]
pub fn has_block(text: &str) -> bool {
    text.lines().any(|line| line.trim() == BLOCK_START)
}

/// A Windows `Path` value with `bin` at its end, or `None` when it is there already; entries are
/// compared without regard to case or a trailing backslash, as Windows compares them.
#[must_use]
pub fn windows_path_with(current: &str, bin: &str) -> Option<String> {
    let same = |entry: &str| {
        entry
            .trim()
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(bin.trim_end_matches('\\'))
    };
    if current.split(';').any(same) {
        return None;
    }
    let kept = current.trim_end_matches(';');
    Some(if kept.is_empty() {
        bin.to_owned()
    } else {
        format!("{kept};{bin}")
    })
}

/// Whether `bin` is on the lasting `PATH` of the user whose home is `home` — the one new terminals
/// get, not this process's. `home` is where the engine directory lies, never read from the
/// environment: an install into a directory of a test must not reach the real startup file.
///
/// # Errors
///
/// The setting could not be read, or on Windows the engine lies outside the user's own home, whose
/// `Path` alone this could change.
pub fn is_on_path(home: &Path, bin: &Path) -> Result<bool, String> {
    if cfg!(windows) {
        own_windows_home(home)?;
        let current = windows_user_path()?;
        Ok(windows_path_with(&current, &bin.display().to_string()).is_none())
    } else {
        let file = unix_startup_file(home);
        Ok(std::fs::read_to_string(&file).is_ok_and(|text| has_block(&text)))
    }
}

/// Puts `bin` on the user's lasting `PATH`; nothing when it is there.
///
/// # Errors
///
/// The setting could not be written.
pub fn put_on_path(home: &Path, bin: &Path) -> Result<(), String> {
    if cfg!(windows) {
        own_windows_home(home)?;
        let current = windows_user_path()?;
        let Some(next) = windows_path_with(&current, &bin.display().to_string()) else {
            return Ok(());
        };
        powershell(&format!(
            "[Environment]::SetEnvironmentVariable('Path', '{}', 'User')",
            next.replace('\'', "''")
        ))
        .map(|_| ())
    } else {
        let file = unix_startup_file(home);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        if has_block(&text) {
            return Ok(());
        }
        let separator = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&file, format!("{text}{separator}\n{}", shell_block(bin)))
            .map_err(|error| format!("{}: {error}", file.display()))
    }
}

fn unix_startup_file(home: &Path) -> PathBuf {
    let shell = std::env::var("SHELL").unwrap_or_default();
    startup_file(home, &shell, cfg!(target_os = "macos"))
}

/// Refuses a home that is not the user's own: the user's `Path` belongs to the real one.
fn own_windows_home(home: &Path) -> Result<(), String> {
    let profile = std::env::var_os("USERPROFILE").ok_or("no USERPROFILE")?;
    if Path::new(&profile) == home {
        Ok(())
    } else {
        Err(format!(
            "the engine lies outside {}: add its bin directory to PATH by hand",
            Path::new(&profile).display()
        ))
    }
}

fn windows_user_path() -> Result<String, String> {
    powershell("[Environment]::GetEnvironmentVariable('Path', 'User')")
}

fn powershell(script: &str) -> Result<String, String> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("PowerShell could not be started: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
