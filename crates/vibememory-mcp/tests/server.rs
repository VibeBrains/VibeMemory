//! The MCP server against a journal held in memory: the protocol, the five tools, and the rules
//! that keep the server from doing the engine's job.

// The test builds records and asserts on them, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use serde_json::{Value, json};
use vibememory_mcp::memories::{DirectoryProject, FakeMemories, Memories};
use vibememory_mcp::protocol::{self, PROTOCOL_VERSION};
use vibememory_mcp::tools::{self, Caller};

const STAMP: &str = "2026-09-09T12:00:00Z";
const AGENT: &str = "codex";
/// A server started outside any project of the store.
const CALLER: Caller<'static> = Caller::owner(AGENT, None);

/// The projects the store holds. A write does not create a project, so the tests' stores start
/// with the ones they write to.
const PROJECTS: &[&str] = &["Promed", "VibeIDE", "VibeIDEA", "VibeMemory"];

fn memories() -> FakeMemories {
    let fake = FakeMemories::new(STAMP);
    for project in PROJECTS {
        fake.seed(project, Vec::new());
    }
    fake
}

/// Calls a tool and unwraps the structured half of the answer.
fn call(name: &str, arguments: &Value, fake: &FakeMemories) -> Result<Value, String> {
    tools::call(name, arguments, &CALLER, fake)
}

fn save_one(fake: &FakeMemories, id: &str, body: &str) -> Value {
    call(
        "memory_save",
        &json!({
            "project": "VibeMemory", "id": id, "kind": "project",
            "title": "Store naming", "description": "where the store name comes from",
            "body": body
        }),
        fake,
    )
    .expect("save")
}

#[test]
fn the_handshake_and_the_catalogue_are_what_a_client_expects() {
    let fake = memories();
    let request =
        protocol::parse(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).expect("parse");
    let answer = protocol::handle(&request, &CALLER, &fake).expect("an id means an answer");
    let value = serde_json::to_value(answer).expect("encode");
    assert_eq!(value["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(value["result"]["serverInfo"]["name"], "vibememory");

    let request =
        protocol::parse(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).expect("parse");
    let answer = protocol::handle(&request, &CALLER, &fake).expect("answer");
    let value = serde_json::to_value(answer).expect("encode");
    let names: Vec<&str> = value["result"]["tools"]
        .as_array()
        .expect("array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "memory_search",
            "memory_get",
            "memory_save",
            "memory_update",
            "memory_delete",
            "history_search",
            "project_resolve"
        ]
    );
}

#[test]
fn a_notification_is_answered_by_saying_nothing() {
    let fake = memories();
    // Every client sends this right after the handshake. Answering it is a protocol error.
    let request = protocol::parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .expect("parse");
    assert!(protocol::handle(&request, &CALLER, &fake).is_none());
}

#[test]
fn an_unknown_method_is_a_protocol_error_but_a_refused_tool_is_not() {
    let fake = memories();

    let request =
        protocol::parse(r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#).expect("parse");
    let value = serde_json::to_value(protocol::handle(&request, &CALLER, &fake).expect("answer"))
        .expect("encode");
    assert_eq!(value["error"]["code"], -32601, "{value}");

    // A tool that says "no, because…" must reach the model as content, not as a transport error:
    // the model is the one that has to act on the reason.
    let request = protocol::parse(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memory_get","arguments":{"id":"missing-thing"}}}"#,
    )
    .expect("parse");
    let value = serde_json::to_value(protocol::handle(&request, &CALLER, &fake).expect("answer"))
        .expect("encode");
    assert!(value["error"].is_null(), "{value}");
    assert_eq!(value["result"]["isError"], true);
    assert!(
        value["result"]["structuredContent"]["error"]
            .as_str()
            .expect("a reason")
            .contains("missing-thing")
    );
}

#[test]
fn saving_writes_one_event_and_refuses_to_shadow_an_existing_record() {
    let fake = memories();
    let saved = save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );
    assert_eq!(saved["saved"], "store-naming");

    let events = fake.written();
    let written = &events["VibeMemory"];
    assert_eq!(written.len(), 1, "one call, one event");
    assert!(written[0].parent.is_none(), "a first version has no parent");

    let stored = call("memory_get", &json!({"id": "store-naming"}), &fake).expect("get");
    assert_eq!(
        stored["agent"], AGENT,
        "memory is shared between agents, so it has to say who wrote it"
    );
    assert_eq!(stored["status"], "active");

    drop(events);
    let again = call(
        "memory_save",
        &json!({
            "project": "VibeMemory", "id": "store-naming", "kind": "project",
            "title": "Other", "description": "other", "body": "other"
        }),
        &fake,
    );
    assert!(
        again
            .expect_err("a taken id must be refused")
            .contains("memory_update"),
        "and the refusal must say what to do instead"
    );
}

#[test]
fn an_update_names_the_version_it_saw_as_its_parent() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );
    let before = call("memory_get", &json!({"id": "store-naming"}), &fake).expect("get");
    let version = before["version"].as_str().expect("version").to_owned();

    let updated = call(
        "memory_update",
        &json!({"project": "VibeMemory", "id": "store-naming", "status": "stale"}),
        &fake,
    )
    .expect("update");

    assert_eq!(
        updated["parent"], version,
        "without the parent a second agent's edit looks like a fresh record, and the loser is \
         overwritten silently"
    );
    let after = call("memory_get", &json!({"id": "store-naming"}), &fake).expect("get");
    assert_eq!(after["status"], "stale");
    assert_eq!(
        after["body"], "The name follows the git common dir.",
        "an update touches only what it was given"
    );
}

#[test]
fn search_filters_by_words_kind_and_status_and_always_reports_staleness() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );
    save_one(&fake, "merge-rules", "Union by uuid, nothing is lost.");
    call(
        "memory_update",
        &json!({"project": "VibeMemory", "id": "merge-rules", "status": "stale"}),
        &fake,
    )
    .expect("update");

    let all = call("memory_search", &json!({}), &fake).expect("search");
    assert_eq!(all["results"].as_array().expect("array").len(), 2);
    for result in all["results"].as_array().expect("array") {
        assert!(
            !result["status"].is_null(),
            "an agent that must ask a second question to learn a fact is out of date will not ask"
        );
    }

    let by_word = call("memory_search", &json!({"query": "union"}), &fake).expect("search");
    let hits = by_word["results"].as_array().expect("array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"], "merge-rules", "the body is searched too");

    let only_active = call("memory_search", &json!({"status": "active"}), &fake).expect("search");
    let active = only_active["results"].as_array().expect("array");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0]["id"], "store-naming");

    let wrong_kind = call("memory_search", &json!({"kind": "user"}), &fake).expect("search");
    assert!(wrong_kind["results"].as_array().expect("array").is_empty());

    // An unknown word for a closed set is refused, never treated as "no filter".
    assert!(call("memory_search", &json!({"status": "disputed"}), &fake).is_err());
}

#[test]
fn deleting_appends_a_tombstone_and_leaves_the_versions_alone() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );

    call(
        "memory_delete",
        &json!({"project": "VibeMemory", "id": "store-naming"}),
        &fake,
    )
    .expect("delete");

    let events = fake.written();
    let written = &events["VibeMemory"];
    assert_eq!(
        written.len(),
        2,
        "the journal grows by one event; nothing is erased from it"
    );
    drop(events);

    assert!(
        call("memory_get", &json!({"id": "store-naming"}), &fake).is_err(),
        "a forgotten record stops being shown"
    );
}

#[test]
fn links_in_the_body_are_read_so_the_projection_matches_what_the_engine_would_write() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir. See [[merge-rules]] and [[merge-rules]] again.",
    );
    let stored = call("memory_get", &json!({"id": "store-naming"}), &fake).expect("get");
    assert_eq!(
        stored["links"],
        json!(["merge-rules"]),
        "each link once, in order of appearance — the same rule the markdown import follows"
    );
}

#[test]
fn a_search_without_a_project_looks_in_every_one_of_them() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );
    call(
        "memory_save",
        &json!({
            "project": "Promed", "id": "branch-names", "kind": "project",
            "title": "Branch names", "description": "how branches are named",
            "body": "Feature branches carry the ticket."
        }),
        &fake,
    )
    .expect("save");

    // A record arriving over MCP has no path to say where it belongs, which is exactly why the
    // record carries its project and why the default scope is everything.
    let found = call("memory_search", &json!({"query": "branch"}), &fake).expect("search");
    let hits = found["results"].as_array().expect("array");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["project"], "Promed");
    assert_eq!(fake.projects().expect("projects"), PROJECTS);
}

#[test]
fn two_updates_in_the_same_second_are_two_versions_and_not_one() {
    // The uuid of an event is what the union merge keys by. When the engine wrote memory, at most
    // one version per record existed per tick, so `{machine}-{stamp}-{id}` was unique. An agent
    // over MCP can update the same record twice inside one second — and two events sharing a uuid
    // are collapsed silently, which loses a version with nothing reported. Found by this gate on
    // the first run.
    let fake = memories(); // a fixed clock, so a stamp-only uuid would certainly collide
    save_one(&fake, "store-naming", "First.");
    call(
        "memory_update",
        &json!({"project": "VibeMemory", "id": "store-naming", "body": "Second."}),
        &fake,
    )
    .expect("first update");
    call(
        "memory_update",
        &json!({"project": "VibeMemory", "id": "store-naming", "body": "Third."}),
        &fake,
    )
    .expect("second update");

    let events = fake.written();
    let written = &events["VibeMemory"];
    let uuids: std::collections::BTreeSet<&str> =
        written.iter().map(|event| event.uuid.as_str()).collect();
    assert_eq!(
        uuids.len(),
        written.len(),
        "every event needs its own uuid or the merge loses one: {uuids:?}"
    );
    drop(events);

    let after = call("memory_get", &json!({"id": "store-naming"}), &fake).expect("get");
    assert_eq!(after["body"], "Third.", "the last write is what stands");
    assert_eq!(
        after["rivalVersions"], 0,
        "a chain of parents is not a fork: each update saw the one before it"
    );
}

/// A transcript as the CLI writes one: one JSON record per line.
fn transcript(
    fake: &FakeMemories,
    project: &str,
    session: &str,
    modified: u64,
    said: &[(&str, &str)],
) {
    let text = said
        .iter()
        .map(|(role, words)| {
            json!({
                "type": role,
                "timestamp": "2026-09-10T09:00:00Z",
                "message": { "content": words }
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    fake.add_transcript(project, (session.to_owned(), modified, text));
}

#[test]
fn history_finds_what_was_said_and_answers_newest_first() {
    let fake = memories();
    transcript(
        &fake,
        "Promed",
        "old-session",
        100,
        &[
            ("user", "давай разберёмся с ветками репозитория"),
            ("assistant", "ответ про что-то другое"),
        ],
    );
    transcript(
        &fake,
        "VibeMemory",
        "new-session",
        900,
        &[("user", "ещё раз про ветками и слияние")],
    );

    let found = call("history_search", &json!({"query": "ветками"}), &fake).expect("search");
    let hits = found["results"].as_array().expect("array");
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(
        hits[0]["session"], "new-session",
        "newest first: that is the order a person asks about their own history in"
    );
    assert_eq!(hits[0]["project"], "VibeMemory");
    assert_eq!(hits[1]["session"], "old-session");
    assert_eq!(hits[0]["role"], "user");
    assert!(
        hits[0]["excerpt"]
            .as_str()
            .expect("excerpt")
            .contains("слияние"),
        "the excerpt has to show the moment: {}",
        hits[0]["excerpt"]
    );
    // Every word must appear, as in memory_search.
    let both = call(
        "history_search",
        &json!({"query": "ветками слияние"}),
        &fake,
    )
    .expect("search");
    assert_eq!(both["results"].as_array().expect("array").len(), 1);
}

#[test]
fn history_stops_at_the_limit_and_never_returns_a_whole_corpus() {
    let fake = memories();
    for n in 0..30 {
        transcript(
            &fake,
            "Promed",
            &format!("s{n}"),
            n,
            &[("user", "одно и то же слово")],
        );
    }

    let default = call("history_search", &json!({"query": "слово"}), &fake).expect("search");
    assert_eq!(
        default["results"].as_array().expect("array").len(),
        20,
        "twenty by default: an answer that does not fit in a reply is not an answer"
    );

    let asked = call(
        "history_search",
        &json!({"query": "слово", "limit": 3}),
        &fake,
    )
    .expect("search");
    assert_eq!(asked["results"].as_array().expect("array").len(), 3);

    // Stops READING, not just stops collecting. On a 1.7 GiB corpus the difference between the
    // two is the difference between a usable tool and one that reads every file every time.
    fake.reads.store(0, std::sync::atomic::Ordering::Relaxed);
    call(
        "history_search",
        &json!({"query": "слово", "limit": 3}),
        &fake,
    )
    .expect("search");
    let read = fake.reads.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        read <= 4,
        "three matches needed three files; it read {read} of thirty"
    );

    // However loudly it is asked. Enough sessions that only the clamp can hold the answer down.
    for n in 30..150 {
        transcript(
            &fake,
            "Promed",
            &format!("s{n}"),
            n,
            &[("user", "одно и то же слово")],
        );
    }
    let greedy = call(
        "history_search",
        &json!({"query": "слово", "limit": 1000}),
        &fake,
    )
    .expect("s");
    assert_eq!(
        greedy["results"].as_array().expect("array").len(),
        100,
        "the ceiling is the ceiling: an answer that does not fit in a reply is not an answer"
    );

    // An empty query would return the whole history, so it is refused rather than obeyed.
    assert!(call("history_search", &json!({"query": "   "}), &fake).is_err());
}

#[test]
fn history_narrows_to_one_project_and_survives_a_line_it_cannot_read() {
    let fake = memories();
    transcript(&fake, "Promed", "a", 10, &[("user", "общее слово")]);
    transcript(&fake, "VibeMemory", "b", 20, &[("user", "общее слово")]);
    // A line this build cannot parse: the format is Anthropic's and changes between versions.
    // A search that dies on one unknown line is a search nobody can rely on.
    fake.add_transcript(
        "Promed",
        (
            "broken".to_owned(),
            30,
            "не json вовсе\n{\"type\":\"user\",\"message\":{\"content\":\"общее слово\"}}"
                .to_owned(),
        ),
    );

    let narrowed = call(
        "history_search",
        &json!({"query": "слово", "project": "Promed"}),
        &fake,
    )
    .expect("s");
    let hits = narrowed["results"].as_array().expect("array");
    assert_eq!(
        hits.len(),
        2,
        "only Promed, and the broken line did not stop it: {hits:?}"
    );
    assert!(hits.iter().all(|hit| hit["project"] == "Promed"));
}

#[test]
fn an_excerpt_shows_the_moment_without_handing_over_the_conversation() {
    let fake = memories();
    let long = "слово ".repeat(300); // far past any sensible excerpt
    transcript(&fake, "Promed", "long", 10, &[("user", long.as_str())]);

    let found = call("history_search", &json!({"query": "слово"}), &fake).expect("search");
    let excerpt = found["results"][0]["excerpt"].as_str().expect("excerpt");
    assert!(
        excerpt.chars().count() <= 241,
        "an excerpt is a hint, not a copy of the conversation: {} chars",
        excerpt.chars().count()
    );
    assert!(
        excerpt.ends_with('…'),
        "and it says that it was cut: {excerpt}"
    );
}

#[test]
fn a_write_without_a_project_goes_to_the_project_the_server_was_started_in() {
    let fake = memories();
    let here = Caller::owner(AGENT, Some("VibeIDE"));
    let fact = |id: &str| json!({ "id": id, "kind": "project", "description": "d", "body": "b" });
    tools::call("memory_save", &fact("from-the-ide"), &here, &fake)
        .expect("an agent in an IDE knows its folder, not the store's name for it");
    assert!(
        call(
            "memory_get",
            &json!({ "project": "VibeIDE", "id": "from-the-ide" }),
            &fake
        )
        .is_ok(),
        "the write landed in the project of the directory"
    );

    let mut named = fact("named");
    named["project"] = json!("VibeIDEA");
    tools::call("memory_save", &named, &here, &fake).expect("save");
    assert!(
        call(
            "memory_get",
            &json!({ "project": "VibeIDEA", "id": "named" }),
            &fake
        )
        .is_ok(),
        "a project the call names wins over the directory"
    );
    assert!(
        call(
            "memory_get",
            &json!({ "project": "VibeIDE", "id": "named" }),
            &fake
        )
        .is_err(),
        "and the directory's project does not get a copy"
    );

    let refused = tools::call("memory_save", &fact("nowhere"), &CALLER, &fake);
    assert!(
        refused
            .as_ref()
            .is_err_and(|problem| problem.contains("project")),
        "outside any project a write has to say where: {refused:?}"
    );
}

#[test]
fn the_handshake_tells_the_agent_which_project_it_is_in() {
    let request =
        protocol::parse(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).expect("parse");
    let fake = memories();
    let here = Caller::owner(AGENT, Some("VibeIDE"));
    let told = serde_json::to_value(protocol::handle(&request, &here, &fake).expect("answer"))
        .expect("json");
    assert!(
        told["result"]["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("VibeIDE")),
        "{told}"
    );
    let silent = serde_json::to_value(protocol::handle(&request, &CALLER, &fake).expect("answer"))
        .expect("json");
    assert!(silent["result"].get("instructions").is_none(), "{silent}");
}

#[test]
fn a_folder_is_named_by_the_rules_and_a_missing_project_says_why() {
    let fake = memories();
    for (directory, answer) in [
        (
            "/work/Held",
            DirectoryProject::Held {
                name: "Held".to_owned(),
                rule: "repo".to_owned(),
            },
        ),
        (
            "/work/new",
            DirectoryProject::Unheld {
                name: "new".to_owned(),
            },
        ),
        (
            "/home",
            DirectoryProject::Ignored {
                reason: "ignoreCwd: /home".to_owned(),
            },
        ),
    ] {
        fake.answer_directory(directory, answer);
    }

    let held = call(
        "project_resolve",
        &json!({ "directory": "/work/Held" }),
        &fake,
    )
    .expect("held");
    assert_eq!(held["project"], "Held");
    assert_eq!(held["rule"], "repo");

    let unheld = call(
        "project_resolve",
        &json!({ "directory": "/work/new" }),
        &fake,
    )
    .expect("unheld");
    assert!(
        unheld["project"].is_null(),
        "no project is offered to write into"
    );
    assert_eq!(unheld["wouldBe"], "new");

    let ignored =
        call("project_resolve", &json!({ "directory": "/home" }), &fake).expect("ignored");
    assert!(ignored["project"].is_null());
    assert!(
        ignored["why"]
            .as_str()
            .expect("why")
            .contains("ignoreCwd: /home")
    );

    let refused = call("project_resolve", &json!({}), &fake).expect_err("directory is required");
    assert!(refused.contains("directory"));
}

#[test]
fn a_token_for_some_projects_sees_and_writes_only_those() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );
    let scope = ["VibeIDE".to_owned()];
    let narrow = Caller {
        scope: Some(&scope),
        ..Caller::owner(AGENT, None)
    };
    let fact = json!({
        "project": "VibeIDE", "id": "ide-only", "kind": "project", "description": "d", "body": "b"
    });
    tools::call("memory_save", &fact, &narrow, &fake).expect("a listed project");

    let all =
        tools::call("memory_search", &json!({ "query": "" }), &narrow, &fake).expect("search");
    let projects: Vec<&str> = all["results"]
        .as_array()
        .expect("array")
        .iter()
        .map(|hit| hit["project"].as_str().expect("project"))
        .collect();
    assert_eq!(
        projects,
        ["VibeIDE"],
        "a search without a project stays inside the list"
    );

    let elsewhere = tools::call(
        "memory_get",
        &json!({ "project": "VibeMemory", "id": "store-naming" }),
        &narrow,
        &fake,
    );
    assert!(
        elsewhere.is_err_and(|problem| problem.contains("not among the projects")),
        "naming a project outside the list is refused, not answered empty"
    );
    let mut outside = fact;
    outside["project"] = json!("VibeMemory");
    outside["id"] = json!("outside");
    assert!(tools::call("memory_save", &outside, &narrow, &fake).is_err());
}

#[test]
fn a_write_to_a_project_the_store_does_not_hold_is_refused_and_names_the_near_one() {
    let fake = memories();
    let fact = json!({
        "project": "vibeide", "id": "typo", "kind": "project", "description": "d", "body": "b"
    });
    let refused = tools::call("memory_save", &fact, &CALLER, &fake).expect_err("refused");
    assert!(
        refused.contains("does not create one") && refused.contains("VibeIDE"),
        "{refused}"
    );
    assert!(
        fake.written().values().all(Vec::is_empty),
        "nothing was written anywhere"
    );
}

/// A record two machines wrote from the same version: `gpd` is shown, `mac` is its rival.
fn disputed() -> FakeMemories {
    let fake = memories();
    let version = |uuid: &str, parent: Option<&str>, body: &str, updated: &str| {
        serde_json::from_value(json!({
            "uuid": uuid, "parent": parent, "action": "upsert",
            "record": {
                "id": "merge-rules", "kind": "project", "project": "VibeMemory",
                "description": "one line about merge rules", "body": body, "links": [],
                "agent": "claude-code", "createdAt": "2026-09-03T10:02:00Z", "updatedAt": updated
            }
        }))
        .expect("event")
    };
    fake.seed(
        "VibeMemory",
        vec![
            version(
                "e3",
                None,
                "The base decides membership.",
                "2026-09-03T10:02:00Z",
            ),
            version(
                "mac",
                Some("e3"),
                "Mac wrote this one.",
                "2026-09-03T11:00:00Z",
            ),
            version(
                "gpd",
                Some("e3"),
                "Windows wrote this one.",
                "2026-09-03T11:00:00Z",
            ),
        ],
    );
    fake
}

#[test]
fn a_disputed_fact_shows_its_rivals_and_an_update_that_merges_them_settles_it() {
    let fake = disputed();
    let before = call("memory_get", &json!({"id": "merge-rules"}), &fake).expect("get");
    assert_eq!(
        before["rivalVersions"], 1,
        "kept for clients that read the count"
    );
    assert_eq!(before["rivals"][0]["version"], "mac");
    assert_eq!(
        before["rivals"][0]["body"], "Mac wrote this one.",
        "an agent cannot merge a text it is not shown"
    );
    let search = call("memory_search", &json!({"query": "merge"}), &fake).expect("search");
    assert_eq!(search["results"][0]["disputed"], true);

    let updated = call(
        "memory_update",
        &json!({"project": "VibeMemory", "id": "merge-rules", "body": "Both agree.", "merges": ["mac"]}),
        &fake,
    )
    .expect("update");
    assert_eq!(updated["merged"], json!(["mac"]));
    let after = call("memory_get", &json!({"id": "merge-rules"}), &fake).expect("get");
    assert_eq!(
        after["rivals"],
        json!([]),
        "the merged rival stops being shown"
    );
    assert_eq!(after["body"], "Both agree.");
}

#[test]
fn an_update_cannot_merge_a_version_that_is_not_a_rival() {
    let fake = disputed();
    for wrong in ["gpd", "e3", "nope"] {
        let refused = call(
            "memory_update",
            &json!({"project": "VibeMemory", "id": "merge-rules", "body": "x", "merges": [wrong]}),
            &fake,
        )
        .expect_err("only a current rival can be merged");
        assert!(refused.contains("memory_get"), "{refused}");
    }
    let after = call("memory_get", &json!({"id": "merge-rules"}), &fake).expect("get");
    assert_eq!(after["rivalVersions"], 1, "a refused update writes nothing");
}

#[test]
fn forgetting_a_disputed_fact_forgets_its_rivals_too() {
    let fake = disputed();
    call(
        "memory_delete",
        &json!({"project": "VibeMemory", "id": "merge-rules"}),
        &fake,
    )
    .expect("delete");
    let gone = call("memory_get", &json!({"id": "merge-rules"}), &fake);
    assert!(
        gone.is_err(),
        "the rival must not step in as the record: {gone:?}"
    );
}

#[test]
fn a_write_past_the_team_quota_is_refused_and_forgetting_still_works() {
    let fake = memories();
    save_one(
        &fake,
        "store-naming",
        "The name follows the git common dir.",
    );
    let caller = Caller {
        limits: tools::Limits {
            quota_bytes: Some(4096),
            ..tools::Limits::default()
        },
        ..CALLER
    };
    let fresh = json!({
        "project": "VibeMemory", "id": "merge-rules", "kind": "project",
        "description": "how journals merge", "body": "Union by uuid."
    });
    fake.store_bytes
        .store(4000, std::sync::atomic::Ordering::Relaxed);

    let refused = tools::call("memory_save", &fresh, &caller, &fake).expect_err("past the quota");
    assert!(refused.starts_with("quota_exceeded"), "{refused}");
    let refused = tools::call(
        "memory_update",
        &json!({"project": "VibeMemory", "id": "store-naming", "body": "A longer text."}),
        &caller,
        &fake,
    )
    .expect_err("an update grows the store as well");
    assert!(refused.starts_with("quota_exceeded"), "{refused}");
    tools::call(
        "memory_delete",
        &json!({"project": "VibeMemory", "id": "store-naming"}),
        &caller,
        &fake,
    )
    .expect("forgetting is not stopped by the quota");

    fake.store_bytes
        .store(0, std::sync::atomic::Ordering::Relaxed);
    tools::call("memory_save", &fresh, &caller, &fake)
        .expect("room again once the store is smaller than the quota");
}

#[test]
fn reasoning_is_not_searched_as_what_was_said() {
    let fake = memories();
    let line = json!({
        "type": "assistant",
        "timestamp": "2026-09-30T09:00:00Z",
        "message": { "content": [
            { "type": "reasoning", "text": "обдумываю тайный план" },
            { "type": "text", "text": "готово" },
        ] }
    });
    fake.add_transcript("Promed", ("dsh".to_owned(), 100, line.to_string()));

    let thought = call("history_search", &json!({"query": "тайный"}), &fake).expect("search");
    assert_eq!(
        thought["results"],
        json!([]),
        "a reasoning block is not speech"
    );
    let said = call("history_search", &json!({"query": "готово"}), &fake).expect("search");
    assert_eq!(said["results"][0]["excerpt"], "готово");
}
