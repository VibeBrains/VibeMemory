//! Releases as the host publishes them: `<base>latest` holds the newest version on one line,
//! `<base><version>/vibememory-<version>-<target>.tar.gz` the archive of one system, and a file of
//! the same name with `.sha256` its sum. The one-line installers read the same three files, and
//! `vibememory update` reads them the same way.

/// The release target of a system, by `std::env::consts::OS` and `ARCH`: the names the build
/// machine gives its archives. `None` for a system nothing is built for.
#[must_use]
pub fn target(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-gnu"),
        _ => None,
    }
}

/// The version `latest` names, or `None` when the answer is not one: empty, or holding anything
/// but digits, letters, dots and hyphens — the version becomes part of a path and a file name.
#[must_use]
pub fn parse_version(text: &str) -> Option<String> {
    let version = text.trim();
    let valid = !version.is_empty()
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    valid.then(|| version.to_owned())
}

/// Whether `candidate` is newer than `current`, part by dotted part as numbers. A part that is not
/// a number counts as 0 — the rule the host uses to pick `latest`, so both ends agree on "newest".
#[must_use]
pub fn is_newer(candidate: &str, current: &str) -> bool {
    parts(candidate) > parts(current)
}

/// A version as numbers, trailing zeros dropped so that `0.4` and `0.4.0` are the same version.
fn parts(version: &str) -> Vec<u64> {
    let mut numbers: Vec<u64> = version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    while numbers.last() == Some(&0) {
        numbers.pop();
    }
    numbers
}

/// The archive of one version for one system.
#[must_use]
pub fn archive_name(version: &str, target: &str) -> String {
    format!("vibememory-{version}-{target}.tar.gz")
}

/// The sum a `.sha256` file publishes: its first field, when that is 64 hex digits. Lowercase, the
/// form the engine computes.
#[must_use]
pub fn published_sum(text: &str) -> Option<String> {
    let sum = text.split_whitespace().next()?.to_ascii_lowercase();
    (sum.len() == 64 && sum.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(sum)
}
