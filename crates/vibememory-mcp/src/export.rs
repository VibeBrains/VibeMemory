//! A team's memory as an archive a person can open: what the cabinet mails before it lets a team
//! go, and what the team page offers to download.
//!
//! The archive holds, for every project, the journal as the store keeps it and the memory as the
//! agent reads it — the markdown the engine projects. The journal is the truth and goes back into
//! a store unchanged; the markdown is for a person, who has no engine at hand.

use std::io::{Seek, Write};

use vibememory_core::memory::{fold, markdown, parse};

/// What sits at the archive's root and says what the rest is.
const README: &str = "VibeMemory — memory of team {slug}, {at}\n\n\
Each folder is a project.\n\
memory.jsonl — the journal of the memory, as the store keeps it; a store takes it back unchanged.\n\
memory/ — the same memory as files an agent reads: MEMORY.md is the index, one file a record.\n\n\
VibeMemory — память команды {slug}, {at}\n\n\
Каждая папка — проект.\n\
memory.jsonl — журнал памяти, как его хранит стор; стор примет его обратно без изменений.\n\
memory/ — та же память файлами, как её читает агент: MEMORY.md — оглавление, по файлу на запись.\n";

/// The files of the archive, by path, from each project's journal. A project without one is left
/// out: there is nothing of its memory to keep.
#[must_use]
pub fn archive_files(
    slug: &str,
    at: &str,
    journals: &[(String, Vec<u8>)],
) -> Vec<(String, Vec<u8>)> {
    let mut files = vec![(
        "README.txt".to_owned(),
        README
            .replace("{slug}", slug)
            .replace("{at}", at)
            .into_bytes(),
    )];
    for (project, journal) in journals {
        if journal.is_empty() {
            continue;
        }
        files.push((format!("{project}/memory.jsonl"), journal.clone()));
        let (events, unreadable) = parse(journal);
        for (name, bytes) in markdown::project(&fold(&events, unreadable)).files {
            files.push((format!("{project}/memory/{name}"), bytes));
        }
    }
    files
}

/// Writes the files as a zip.
///
/// # Errors
///
/// What the writer or the zip format refused.
pub fn write_zip<W: Write + Seek>(writer: W, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let mut zip = zip::ZipWriter::new(writer);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in files {
        zip.start_file(name.as_str(), options)
            .map_err(|error| format!("{name}: {error}"))?;
        zip.write_all(bytes)
            .map_err(|error| format!("{name}: {error}"))?;
    }
    zip.finish().map_err(|error| error.to_string())?;
    Ok(())
}
