//! The project's own documentation rules, as a test.
//!
//! `CLAUDE.md` says a knowledge entry without a line in the index does not exist, that manuals
//! live in `docs/manuals/` and are listed in the documentation tree, that names under `docs/` are
//! camelCase, and that a finished roadmap item carries its date and branch. Checking that by hand
//! after every change is how such rules quietly rot, so they are checked here instead.

// This test reads the repository on purpose — that is its whole subject — so the purity gate that
// keeps the library free of I/O is lifted for this file alone.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Top-level names that are conventionally not camelCase and stay that way.
const CONVENTIONAL_NAMES: &[&str] = &[
    "README.md",
    "roadmap.md",
    "functional.md",
    "idea.md",
    "CONTRIBUTING.md",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Every markdown file below `dir`, sorted, so failures read the same way on both machines.
fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).unwrap_or_else(|e| panic!("read {}: {e}", current.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Markdown with fenced code blocks removed: a link inside an example is text, not a link.
fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Targets of markdown links, without the anchor part.
fn links(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(offset) = text.get(index..).and_then(|rest| rest.find("](")) {
        let start = index + offset + 2;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b')' {
            end += 1;
        }
        if let Some(target) = text.get(start..end) {
            let target = target.split('#').next().unwrap_or_default().trim();
            if !target.is_empty() {
                found.push(target.to_owned());
            }
        }
        index = end.max(start);
    }
    found
}

fn is_camel_case(name: &str) -> bool {
    let stem = name.strip_suffix(".md").unwrap_or(name);
    !stem.is_empty()
        && stem.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && stem.chars().all(|c| c.is_ascii_alphanumeric())
}

#[test]
fn documentation_holds_its_own_rules() {
    let root = repo_root();
    let docs = root.join("docs");
    let mut failures: Vec<String> = Vec::new();

    let mut all_markdown = markdown_files(&docs);
    for name in ["README.md", "CLAUDE.md", "idea.md", "fixtures/README.md"] {
        all_markdown.push(root.join(name));
    }

    // Every relative link resolves. A dead link in a knowledge base is worse than no link: it
    // says a fact was written down when it was not.
    for file in &all_markdown {
        let text =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let directory = file.parent().expect("file has a directory");
        for target in links(&prose(&text)) {
            if target.starts_with("http") || target.starts_with("mailto:") {
                continue;
            }
            if !directory.join(&target).exists() {
                failures.push(format!("{}: dead link {target}", file.display()));
            }
        }
    }

    // A knowledge entry without a line in the index does not exist.
    let index_path = docs.join("knowledge/README.md");
    let index = fs::read_to_string(&index_path).expect("knowledge index");
    let indexed: BTreeSet<String> = links(&prose(&index)).into_iter().collect();
    for file in markdown_files(&docs.join("knowledge")) {
        let relative = file
            .strip_prefix(docs.join("knowledge"))
            .expect("under knowledge");
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative == "README.md" {
            continue;
        }
        if !indexed.contains(&relative) {
            failures.push(format!("knowledge/README.md: no line for {relative}"));
        }
    }

    // Manuals are listed in the documentation tree, so a reader finds them without grepping.
    let tree = fs::read_to_string(docs.join("README.md")).expect("docs/README.md");
    let manuals = docs.join("manuals");
    if manuals.exists() {
        for file in markdown_files(&manuals) {
            let name = file
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            if !tree.contains(&name) {
                failures.push(format!("docs/README.md: manual {name} is not in the tree"));
            }
        }
    }

    // Names under docs/ are camelCase, apart from the conventional top-level ones.
    for file in markdown_files(&docs) {
        let name = file
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();
        if CONVENTIONAL_NAMES.contains(&name.as_str()) || is_camel_case(&name) {
            continue;
        }
        failures.push(format!("{}: name is not camelCase", file.display()));
    }

    // A finished roadmap item says when it was done and on which branch.
    let roadmap = fs::read_to_string(docs.join("roadmap.md")).expect("roadmap");
    for line in roadmap.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("- [x]") {
            continue;
        }
        if !(trimmed.contains("✅") && trimmed.contains("(20")) {
            failures.push(format!(
                "roadmap.md: finished item without ✅ (date, branch): {trimmed}"
            ));
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

/// The donation block is the same in README and in release notes, and the way to give is not the
/// place to be creative: the link and both QR codes are checked here, as `VibeIDE`'s release lint
/// checks them in its notes. A product whose block lost a QR silently offers one way to give.
#[test]
fn the_donation_block_keeps_its_two_qr_codes() {
    let root = repo_root();
    let readme = fs::read_to_string(root.join("README.md")).expect("README.md");
    let mut failures: Vec<String> = Vec::new();

    for needle in [
        "Если VibeMemory оказался полезным",
        "https://boosty.to/borodatych/donate",
    ] {
        if !readme.contains(needle) {
            failures.push(format!(
                "README.md: the donation block does not carry {needle}"
            ));
        }
    }
    // The images themselves, not only their names: a deleted file leaves the block broken on
    // GitHub while the markdown still looks right.
    for asset in ["media/QR-Boosty.png", "media/QR-Code.jpg"] {
        if !readme.contains(asset) {
            failures.push(format!(
                "README.md: the donation block does not show {asset}"
            ));
        }
        let path = root.join(asset);
        match fs::metadata(&path) {
            Ok(data) if data.len() > 0 => {}
            Ok(_) => failures.push(format!("{asset} is empty")),
            Err(error) => failures.push(format!("{asset}: {error}")),
        }
    }

    // The rule names the same three things, so the block and its rule cannot drift apart.
    let rule = fs::read_to_string(root.join("docs/releaseDonationPhrases.md"))
        .expect("releaseDonationPhrases.md");
    for needle in [
        "https://boosty.to/borodatych/donate",
        "media/QR-Boosty.png",
        "media/QR-Code.jpg",
    ] {
        if !rule.contains(needle) {
            failures.push(format!(
                "releaseDonationPhrases.md: the rule does not name {needle}"
            ));
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
