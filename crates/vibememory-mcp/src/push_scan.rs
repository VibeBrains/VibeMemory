//! What a push brings that holds an agent token of a cabinet.
//!
//! The push is read object by object, not as the tree it leaves behind: a commit that adds a token
//! and a later one of the same push that removes it would leave a clean tree and the token in the
//! store's history, in front of every member for good. So every commit the push brings is read —
//! its message included — and every blob those commits add or change.
//!
//! Objects are asked for by id, never by path: a name with a newline or bytes that are not UTF-8
//! would split or miss a question by path. The answers are read as a stream, a chunk at a time,
//! so a large blob costs no more memory than a small one; how much a push may make the hook read
//! at all is capped, since a blob of zeros compresses to nothing on the wire.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use vibememory_core::token::{SHAPE_BYTES, holds_token};

use crate::git_memories;

/// Bytes read from git at once while an object is scanned.
const CHUNK_BYTES: usize = 64 * 1024;
/// How `diff-tree` names a path the commit deletes, and an id that is no object.
const DELETED_STATUS: &str = "D";
/// A submodule entry: a commit of another repository, which this one does not hold.
const GITLINK_MODE: &str = "160000";

/// What the scan found.
#[derive(Debug, PartialEq, Eq)]
pub enum Scanned {
    /// The objects that hold a token, each named by the path it came in at, or as a commit.
    Found(Vec<String>),
    /// The push would have the hook read more than `limit` bytes.
    TooLarge {
        /// What the push's objects hold, uncompressed.
        bytes: u64,
    },
}

/// One object of the push and how to name it to a person.
struct Brought {
    id: String,
    label: String,
}

/// Scans what a push that moves a branch to `new` brings into `repo`: the commits no reference of
/// the repository reaches yet — the hook runs before git moves any — and the blobs they add.
///
/// # Errors
///
/// Git that could not answer.
pub fn scan(repo: &Path, new: &str, limit: u64) -> Result<Scanned, String> {
    let brought = brought(repo, new)?;
    if brought.is_empty() {
        return Ok(Scanned::Found(Vec::new()));
    }
    let bytes = total_size(repo, &brought)?;
    if bytes > limit {
        return Ok(Scanned::TooLarge { bytes });
    }
    read_objects(repo, &brought).map(Scanned::Found)
}

/// The commits the push brings, then the blobs each adds or changes; every object once.
fn brought(repo: &Path, new: &str) -> Result<Vec<Brought>, String> {
    let listed = git_memories::run(repo, &["rev-list", new, "--not", "--all"], None, &[])?;
    let commits: Vec<String> = String::from_utf8_lossy(&listed)
        .lines()
        .map(str::to_owned)
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut brought = Vec::new();
    for commit in commits {
        let raw = git_memories::run(
            repo,
            &[
                "diff-tree",
                // a merge lists what it changes against each parent: a blob a conflict resolution
                // wrote is in no other commit
                "-m",
                "-r",
                "-z",
                "--raw",
                "--no-renames",
                "--root",
                "--no-commit-id",
                &commit,
            ],
            None,
            &[],
        )?;
        if seen.insert(commit.clone()) {
            brought.push(Brought {
                label: format!("commit {commit}"),
                id: commit,
            });
        }
        // `:<old mode> <new mode> <old id> <new id> <status>`, then the path, each ended by NUL
        let mut fields = raw.split(|byte| *byte == 0);
        while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
            let header = String::from_utf8_lossy(header);
            let parts: Vec<&str> = header.trim_start_matches(':').split(' ').collect();
            let (Some(mode), Some(id), Some(status)) = (parts.get(1), parts.get(3), parts.get(4))
            else {
                continue;
            };
            if *status == DELETED_STATUS || *mode == GITLINK_MODE {
                continue;
            }
            if seen.insert((*id).to_owned()) {
                brought.push(Brought {
                    id: (*id).to_owned(),
                    label: String::from_utf8_lossy(path).into_owned(),
                });
            }
        }
    }
    Ok(brought)
}

/// What the objects hold, uncompressed, by `cat-file --batch-check`.
fn total_size(repo: &Path, brought: &[Brought]) -> Result<u64, String> {
    let asked = questions(brought);
    let answers = git_memories::run(
        repo,
        &["cat-file", "--batch-check=%(objectsize)"],
        Some(asked.as_bytes()),
        &[],
    )?;
    String::from_utf8_lossy(&answers)
        .lines()
        .map(|line| {
            line.trim()
                .parse::<u64>()
                .map_err(|_| format!("git cat-file answered {line:?}"))
        })
        .sum()
}

/// One object id a line: what `cat-file` reads.
fn questions(brought: &[Brought]) -> String {
    let mut asked = String::new();
    for object in brought {
        asked.push_str(&object.id);
        asked.push('\n');
    }
    asked
}

/// Reads every object in one `cat-file --batch` and returns the labels of those holding a token.
/// The questions go in from a thread of their own while the answers are read here.
fn read_objects(repo: &Path, brought: &[Brought]) -> Result<Vec<String>, String> {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("git could not be started: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "git took no input".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "git gave no output".to_owned())?;
    let asked = questions(brought);
    let found = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(asked.as_bytes()));
        let mut answers = BufReader::new(stdout);
        let mut found = Vec::new();
        for object in brought {
            if object_holds_token(&mut answers)? {
                found.push(object.label.clone());
            }
        }
        writer
            .join()
            .map_err(|_| "the questions to git could not be written".to_owned())?
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(found)
    });
    let status = child.wait().map_err(|error| error.to_string())?;
    let found = found?;
    if !status.success() {
        return Err(format!("git cat-file --batch failed: {status}"));
    }
    Ok(found)
}

/// Reads one answer of `cat-file --batch` — its header, its content, the newline after it — and
/// says whether the content holds a token. The content is scanned a chunk at a time; the last
/// bytes of a chunk are kept in front of the next, so a token cut across two chunks is still seen.
fn object_holds_token(answers: &mut impl BufRead) -> Result<bool, String> {
    let mut header = Vec::new();
    answers
        .read_until(b'\n', &mut header)
        .map_err(|error| error.to_string())?;
    let header = String::from_utf8_lossy(&header);
    let size: usize = header
        .trim_end()
        .rsplit(' ')
        .next()
        .and_then(|size| size.parse().ok())
        .ok_or_else(|| format!("git cat-file answered {:?}", header.trim_end()))?;
    let mut window: Vec<u8> = Vec::with_capacity(CHUNK_BYTES + SHAPE_BYTES);
    let mut chunk = vec![0; CHUNK_BYTES];
    let mut left = size;
    let mut holds = false;
    while left > 0 {
        let wanted = left.min(CHUNK_BYTES);
        let part = chunk
            .get_mut(..wanted)
            .ok_or_else(|| "a chunk out of range".to_owned())?;
        answers
            .read_exact(part)
            .map_err(|error| format!("git cat-file answered short: {error}"))?;
        window.extend_from_slice(part);
        holds = holds || holds_token(&window);
        let keep = window.len().min(SHAPE_BYTES - 1);
        window.drain(..window.len() - keep);
        left -= wanted;
    }
    let mut newline = [0; 1];
    answers
        .read_exact(&mut newline)
        .map_err(|error| format!("git cat-file answered short: {error}"))?;
    Ok(holds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer of `cat-file --batch` for `content`.
    fn answer(content: &[u8]) -> Vec<u8> {
        let mut bytes = format!("0123 blob {}\n", content.len()).into_bytes();
        bytes.extend_from_slice(content);
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn a_token_cut_across_two_chunks_is_seen() {
        let mut content = vec![b'x'; CHUNK_BYTES - 5];
        content.extend_from_slice(b"vmt_7q2m9x4a_secret");
        let mut stream = answer(&content);
        stream.extend(answer(b"nothing here"));
        let mut reader = BufReader::new(stream.as_slice());
        assert!(object_holds_token(&mut reader).unwrap());
        assert!(!object_holds_token(&mut reader).unwrap());
    }
}
