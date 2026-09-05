//! Asking the operating system whether a process is still there.

/// Whether a process with this id exists. `kill -0` asks the kernel without touching the
/// process, and needs no permission to answer for a process of the same user.
#[must_use]
pub fn is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}
