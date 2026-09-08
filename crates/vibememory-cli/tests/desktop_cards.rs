//! Desktop's session cards moving between machines — with the guard that stops one machine's bad
//! luck from spreading.

// The test writes files, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use support::TempDir;
use vibememory_cli::desktop_store::{OUTBOX_DIR, fold_conflict_copies, import, publish, repair};
use vibememory_core::desktop::roots::Roots;
use vibememory_core::naming::PathSyntax;

const CARD: &str = "local_67c711c9.json";
const CLI_SESSION: &str = "11111111-1111-4111-8111-111111111111";
/// The two levels Desktop puts between its store and a card: an account and an organisation.
const CARD_DIR: &str = "28c0ec84/9e0b6fd7";

fn roots(local_root: &Path) -> Roots {
    let mut entries = BTreeMap::new();
    entries.insert("PROJECTS".to_owned(), local_root.display().to_string());
    Roots::new(entries, PathSyntax::Posix)
}

fn card(cwd: &str, cli_session_id: Option<&str>, unavailable: bool) -> String {
    let session = cli_session_id
        .map(|id| format!("\"cliSessionId\":\"{id}\","))
        .unwrap_or_default();
    let mark = if unavailable {
        "\"transcriptUnavailable\":true,"
    } else {
        ""
    };
    format!(
        "{{\"sessionId\":\"local_67c711c9\",{session}{mark}\"cwd\":\"{cwd}\",\
         \"title\":\"a card\",\"isArchived\":false}}"
    )
}

fn outbox(store: &Path, machine: &str) -> std::path::PathBuf {
    store.join("machines").join(machine).join(OUTBOX_DIR)
}

#[test]
fn a_published_card_carries_a_path_the_other_machine_can_translate() {
    let temp = TempDir::new("cards-publish");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    fs::write(
        desktop.join(CARD),
        card(
            &projects.join("VibeIDE").display().to_string(),
            Some(CLI_SESSION),
            false,
        ),
    )
    .expect("write card");

    let cards = publish(&desktop, &store, "mac-test", &roots(&projects)).expect("publish");
    assert_eq!(cards.exported, vec![CARD.to_owned()]);
    let published = fs::read_to_string(outbox(&store, "mac-test").join(CARD)).expect("read");
    assert!(
        published.contains("{PROJECTS}/VibeIDE"),
        "a D:\\ or /Volumes path means nothing elsewhere: {published}"
    );
    assert!(
        published.contains("\"title\": \"a card\""),
        "fields this build does not interpret must survive: {published}"
    );
}

#[test]
fn a_card_that_lost_its_transcript_here_is_not_exported() {
    let temp = TempDir::new("cards-lost");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let cwd = projects.join("VibeIDE").display().to_string();

    fs::write(desktop.join(CARD), card(&cwd, Some(CLI_SESSION), false)).expect("write");
    publish(&desktop, &store, "mac-test", &roots(&projects)).expect("first");

    // Desktop missed a resume and cleared the handle. It never puts one back — exporting this
    // would take the transcript away from every other machine too.
    fs::write(desktop.join(CARD), card(&cwd, None, false)).expect("write");
    let cards = publish(&desktop, &store, "mac-test", &roots(&projects)).expect("second");

    assert!(cards.exported.is_empty(), "{:?}", cards.exported);
    assert_eq!(cards.withheld.len(), 1);
    let published = fs::read_to_string(outbox(&store, "mac-test").join(CARD)).expect("read");
    assert!(
        published.contains(CLI_SESSION),
        "the outbox must still hold the good version: {published}"
    );
}

#[test]
fn a_card_marked_unavailable_here_is_not_exported() {
    let temp = TempDir::new("cards-unavailable");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    fs::write(
        desktop.join(CARD),
        card(
            &projects.join("VibeIDE").display().to_string(),
            Some(CLI_SESSION),
            true,
        ),
    )
    .expect("write");

    let cards = publish(&desktop, &store, "mac-test", &roots(&projects)).expect("publish");
    assert!(
        cards.exported.is_empty(),
        "`transcriptUnavailable` is a verdict about this machine's disk, not about the session"
    );
    assert_eq!(cards.withheld.len(), 1);
}

#[test]
fn a_card_is_imported_only_when_this_machine_can_actually_resume_it() {
    let temp = TempDir::new("cards-import");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let project = projects.join("VibeIDE");
    fs::create_dir_all(&project).expect("the directory exists here");
    fs::create_dir_all(store.join("projects").join("VibeIDE")).expect("dirs");
    fs::write(
        store
            .join("projects")
            .join("VibeIDE")
            .join(format!("{CLI_SESSION}.jsonl")),
        b"{}\n",
    )
    .expect("the transcript is in the store");
    fs::create_dir_all(outbox(&store, "gpd-win")).expect("dirs");
    fs::write(
        outbox(&store, "gpd-win").join(CARD),
        card("{PROJECTS}/VibeIDE", Some(CLI_SESSION), false),
    )
    .expect("their card");

    // Nothing has confirmed the link yet: the reconciler may have predicted it, and Desktop
    // erases `cliSessionId` on a resume miss.
    let unconfirmed =
        import(&desktop, &store, "mac-test", &roots(&projects), &|_| None).expect("import");
    assert!(
        unconfirmed.imported.is_empty(),
        "a predicted link is not proof: {:?}",
        unconfirmed.imported
    );
    assert_eq!(unconfirmed.skipped.len(), 1);

    // Once a session on this machine has proven the link, the card can come in.
    let confirmed = import(&desktop, &store, "mac-test", &roots(&projects), &|id| {
        (id == CLI_SESSION).then(|| format!("/x/.claude/projects/-p/{id}.jsonl"))
    })
    .expect("import");
    assert_eq!(confirmed.imported, vec![CARD.to_owned()]);
    let written = fs::read_to_string(desktop.join(CARD)).expect("read");
    assert!(
        written.contains(&project.display().to_string()),
        "the card must name a directory this machine has: {written}"
    );
}

#[test]
fn a_card_whose_directory_is_missing_here_is_skipped() {
    let temp = TempDir::new("cards-nodir");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    fs::create_dir_all(store.join("projects").join("VibeIDE")).expect("dirs");
    fs::write(
        store
            .join("projects")
            .join("VibeIDE")
            .join(format!("{CLI_SESSION}.jsonl")),
        b"{}\n",
    )
    .expect("write");
    fs::create_dir_all(outbox(&store, "gpd-win")).expect("dirs");
    fs::write(
        outbox(&store, "gpd-win").join(CARD),
        card("{PROJECTS}/Absent", Some(CLI_SESSION), false),
    )
    .expect("write");

    let cards = import(&desktop, &store, "mac-test", &roots(&projects), &|id| {
        Some(format!("/x/{id}.jsonl"))
    })
    .expect("import");
    assert!(cards.imported.is_empty());
    assert_eq!(
        cards.skipped.len(),
        1,
        "Desktop would refuse to resume it anyway"
    );
}

#[test]
fn a_card_this_build_cannot_read_is_left_where_it_is() {
    let temp = TempDir::new("cards-broken");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    fs::write(desktop.join(CARD), b"{ not json").expect("write");

    let cards = publish(&desktop, &store, "mac-test", &roots(&projects)).expect("publish");
    assert!(cards.exported.is_empty() && cards.withheld.is_empty());
    assert_eq!(
        fs::read_to_string(desktop.join(CARD)).expect("read"),
        "{ not json",
        "Desktop's format is undocumented and changes; what we cannot read we do not touch"
    );
}

#[test]
fn cards_are_found_and_written_where_desktop_actually_keeps_them() {
    let temp = TempDir::new("cards-nested");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let project = projects.join("VibeIDE");
    fs::create_dir_all(&project).expect("the directory exists here");
    // Desktop's own layout: the store's root holds no cards at all.
    let nested = desktop.join(CARD_DIR);
    fs::create_dir_all(&nested).expect("nested");
    fs::write(
        nested.join(CARD),
        card(&project.display().to_string(), Some(CLI_SESSION), false),
    )
    .expect("write card");

    let published = publish(&desktop, &store, "mac-test", &roots(&projects)).expect("publish");
    assert_eq!(
        published.exported,
        vec![CARD.to_owned()],
        "a reader that only looks at the root finds an empty store on every real machine"
    );

    // The other machine's card comes back in — and has to land beside the cards Desktop reads,
    // not in the root, where Desktop would never look at it.
    let theirs = "local_ab000000.json";
    fs::create_dir_all(store.join("projects").join("VibeIDE")).expect("dirs");
    fs::write(
        store
            .join("projects")
            .join("VibeIDE")
            .join(format!("{CLI_SESSION}.jsonl")),
        b"{}\n",
    )
    .expect("transcript");
    fs::create_dir_all(outbox(&store, "gpd-win")).expect("dirs");
    fs::write(
        outbox(&store, "gpd-win").join(theirs),
        card("{PROJECTS}/VibeIDE", Some(CLI_SESSION), false),
    )
    .expect("their card");

    let imported = import(&desktop, &store, "mac-test", &roots(&projects), &|id| {
        (id == CLI_SESSION).then(|| format!("/x/.claude/projects/-p/{id}.jsonl"))
    })
    .expect("import");

    assert_eq!(imported.imported, vec![theirs.to_owned()]);
    assert!(
        nested.join(theirs).is_file(),
        "the card must arrive where Desktop reads its cards"
    );
    assert!(
        !desktop.join(theirs).exists(),
        "and not in the store's root, where nothing would ever see it"
    );
}

#[test]
fn a_card_desktop_marked_unavailable_is_repaired_from_this_machines_own_shadow() {
    let temp = TempDir::new("cards-repair");
    let desktop = temp.dir("desktop");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let project = projects.join("VibeIDE");
    fs::create_dir_all(&project).expect("dirs");
    let nested = desktop.join(CARD_DIR);
    fs::create_dir_all(&nested).expect("nested");
    let cwd = project.display().to_string();

    // The card was healthy once, and that version is in this machine's outbox.
    fs::write(nested.join(CARD), card(&cwd, Some(CLI_SESSION), false)).expect("write");
    publish(&desktop, &store, "mac-test", &roots(&projects)).expect("publish");

    // Then Desktop looked at a transcript that was not on this disk yet — behind a link that did
    // not exist — and wrote its verdict. It never takes that back on its own.
    fs::write(nested.join(CARD), card(&cwd, Some(CLI_SESSION), true)).expect("break");

    // Nothing has proven the path: repairing now only buys the next resume miss.
    let unproven = repair(&desktop, &store, "mac-test", &|_| None).expect("repair");
    assert!(unproven.is_empty(), "{unproven:?}");
    assert!(
        fs::read_to_string(nested.join(CARD))
            .expect("read")
            .contains("transcriptUnavailable"),
        "the mark stays until this machine can actually reach the transcript"
    );

    // The transcript is in the store and this machine has proven the path.
    fs::create_dir_all(store.join("projects").join("VibeIDE")).expect("dirs");
    fs::write(
        store
            .join("projects")
            .join("VibeIDE")
            .join(format!("{CLI_SESSION}.jsonl")),
        b"{}\n",
    )
    .expect("transcript");
    let repaired = repair(&desktop, &store, "mac-test", &|id| {
        (id == CLI_SESSION).then(|| format!("/x/.claude/projects/-p/{id}.jsonl"))
    })
    .expect("repair");

    assert_eq!(repaired, vec![CARD.to_owned()]);
    let fixed = fs::read_to_string(nested.join(CARD)).expect("read");
    assert!(
        !fixed.contains("transcriptUnavailable"),
        "the stale mark must go with the repair, or Desktop walks straight back into it: {fixed}"
    );
    assert!(
        fixed.contains(CLI_SESSION),
        "and the card must keep the transcript it names: {fixed}"
    );
    assert!(
        fixed.contains("\"title\": \"a card\""),
        "fields this build does not interpret must survive a repair too: {fixed}"
    );
}

const COPY: &str = "local_67c711c9-GPD-WIN-MAX2.json";
const STAMP: &str = "2026-09-08T12:00:00Z";

#[test]
fn a_conflict_copy_gives_up_its_transcript_and_leaves_the_sidebar() {
    let temp = TempDir::new("cards-fold");
    let desktop = temp.dir("desktop");
    let engine = temp.dir("engine");
    let projects = temp.dir("Projects");
    let project = projects.join("VibeIDE");
    fs::create_dir_all(&project).expect("dirs");
    let nested = desktop.join(CARD_DIR);
    fs::create_dir_all(&nested).expect("nested");
    let cwd = project.display().to_string();

    // Desktop erased the handle on the original; the cloud client's copy still carries it. The
    // copy can never be repaired itself — its name has a machine suffix, so no outbox holds a
    // shadow of it — and it sits in the sidebar as a second, permanently broken card.
    fs::write(nested.join(CARD), card(&cwd, None, true)).expect("broken original");
    fs::write(nested.join(COPY), card(&cwd, Some(CLI_SESSION), false)).expect("copy");

    let folded = fold_conflict_copies(&desktop, &engine, STAMP, &|id| {
        (id == CLI_SESSION).then(|| format!("/x/.claude/projects/-p/{id}.jsonl"))
    })
    .expect("fold");

    assert_eq!(folded.set_aside, vec![COPY.to_owned()]);
    assert_eq!(folded.repaired, vec![CARD.to_owned()]);
    assert!(
        !nested.join(COPY).exists(),
        "the duplicate must leave the sidebar"
    );
    let fixed = fs::read_to_string(nested.join(CARD)).expect("read");
    assert!(
        fixed.contains(CLI_SESSION) && !fixed.contains("transcriptUnavailable"),
        "what the copy knew must reach the card that stays: {fixed}"
    );
    // Set aside, never deleted: it is another machine's version of that card.
    let waiting: Vec<_> = fs::read_dir(engine.join("quarantine"))
        .expect("quarantine")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert!(waiting[0].starts_with(COPY), "{waiting:?}");
}

#[test]
fn a_copy_whose_original_is_gone_is_the_card_itself() {
    let temp = TempDir::new("cards-fold-orphan");
    let desktop = temp.dir("desktop");
    let engine = temp.dir("engine");
    let projects = temp.dir("Projects");
    let nested = desktop.join(CARD_DIR);
    fs::create_dir_all(&nested).expect("nested");
    fs::write(
        nested.join(COPY),
        card(
            &projects.join("VibeIDE").display().to_string(),
            Some(CLI_SESSION),
            false,
        ),
    )
    .expect("copy");

    let folded = fold_conflict_copies(&desktop, &engine, STAMP, &|_| None).expect("fold");

    assert_eq!(folded.adopted, vec![(COPY.to_owned(), CARD.to_owned())]);
    assert!(
        nested.join(CARD).is_file() && !nested.join(COPY).exists(),
        "a copy of nothing is not a duplicate — it is the session's only card"
    );
    assert!(
        folded.set_aside.is_empty(),
        "and nothing is set aside: {:?}",
        folded.set_aside
    );
}
