//! `mcp-config`: what a person puts into each client's configuration, for this machine.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use vibememory_cli::mcp_config::{CLIENTS, Client, Machine, instructions};
use vibememory_core::naming::slug::is_slug;

fn mac() -> Machine {
    Machine {
        server: "/u/me/.vibememory/bin/vibememory-mcp".to_owned(),
        engine_dir: None,
        home: "/u/me".to_owned(),
        windows: false,
    }
}

fn windows() -> Machine {
    Machine {
        server: r"D:\home\.vibememory\bin\vibememory-mcp.exe".to_owned(),
        engine_dir: None,
        home: r"D:\home".to_owned(),
        windows: true,
    }
}

/// The JSON object a text holds, from its first `{` to its last `}`.
fn json_in(text: &str) -> serde_json::Value {
    let start = text.find('{').unwrap();
    let end = text.rfind('}').unwrap();
    serde_json::from_str(&text[start..=end]).unwrap()
}

#[test]
fn every_client_has_its_own_name_and_an_agent_name_the_host_accepts() {
    let mut names: Vec<&str> = CLIENTS.iter().map(|client| client.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CLIENTS.len(), "two clients under one name");
    for client in CLIENTS {
        assert!(is_slug(client.agent()), "{}", client.agent());
        assert_eq!(Client::named(client.name()), Some(*client));
    }
    // the names the IDEs themselves start the server with
    assert_eq!(Client::VibeIde.agent(), "vibeide");
    assert_eq!(Client::VibeIdea.agent(), "vibeidea");
    // the ChatGPT app starts what Codex's file names
    assert_eq!(Client::ChatGpt.agent(), Client::Codex.agent());
}

#[test]
fn a_windows_path_stays_one_path_in_json_toml_and_on_the_command_line() {
    let machine = windows();
    let cursor = json_in(&instructions(Client::Cursor, &machine));
    let server = &cursor["mcpServers"]["vibememory"];
    assert_eq!(server["command"], machine.server.as_str());
    assert_eq!(server["args"], serde_json::json!(["--agent", "cursor"]));
    assert_eq!(server["type"], "stdio");

    let desktop = instructions(Client::ClaudeDesktop, &machine);
    assert!(
        desktop.contains(r"%APPDATA%\Claude\claude_desktop_config.json"),
        "{desktop}"
    );
    assert_eq!(
        json_in(&desktop)["mcpServers"]["vibememory"]["command"],
        machine.server.as_str()
    );

    let codex = instructions(Client::Codex, &machine);
    assert!(
        codex.contains(r#"command = "D:\\home\\.vibememory\\bin\\vibememory-mcp.exe""#),
        "a TOML basic string escapes its backslashes: {codex}"
    );
    assert!(codex.contains(r"D:\home\.codex\config.toml"), "{codex}");
}

#[test]
fn each_client_gets_its_own_format_and_agent() {
    let machine = mac();
    let claude = instructions(Client::ClaudeCode, &machine);
    assert!(claude.contains(
        r#"claude mcp add -s user vibememory -- "/u/me/.vibememory/bin/vibememory-mcp" --agent claude-code"#
    ));
    let codex = instructions(Client::Codex, &machine);
    assert!(codex.contains("[mcp_servers.vibememory]"));
    assert!(codex.contains(r#"args = ["--agent", "codex"]"#));
    assert!(codex.contains("codex mcp add vibememory -- "));
    let gemini = instructions(Client::Gemini, &machine);
    assert!(
        gemini.contains("gemini mcp add -s user vibememory ") && gemini.ends_with('\n'),
        "{gemini}"
    );
    assert!(
        gemini.contains(" -- --agent gemini"),
        "a flag of the server goes after --"
    );
    let dsh = instructions(Client::Dsh, &machine);
    assert!(dsh.contains(r#"args: ["--agent", "dsh-desktop"]"#));
    assert!(
        dsh.contains(r#"cwd: "/u/me""#),
        "no project in the home directory: a write names its own"
    );
    let chatgpt = instructions(Client::ChatGpt, &machine);
    assert!(chatgpt.contains("in the browser cannot"));
    assert!(chatgpt.contains("[mcp_servers.vibememory]"));
    for ide in [Client::VibeIde, Client::VibeIdea] {
        let text = instructions(ide, &machine);
        assert!(text.contains("Nothing to register"), "{text}");
        assert!(
            !text.contains("mcpServers\": {"),
            "an IDE that connects itself gets no entry"
        );
    }
}

#[test]
fn an_engine_outside_its_default_place_is_named_to_every_server() {
    let machine = Machine {
        engine_dir: Some("/data/engine".to_owned()),
        ..mac()
    };
    assert!(
        instructions(Client::ClaudeCode, &machine).contains(r#"-e VIBEMEMORY_DIR="/data/engine""#)
    );
    let codex = instructions(Client::Codex, &machine);
    assert!(codex.contains(r#"env = { VIBEMEMORY_DIR = "/data/engine" }"#));
    assert!(codex.contains(r#"codex mcp add vibememory --env VIBEMEMORY_DIR="/data/engine" -- "#));
    assert_eq!(
        json_in(&instructions(Client::Cursor, &machine))["mcpServers"]["vibememory"]["env"]["VIBEMEMORY_DIR"],
        "/data/engine"
    );
    assert!(instructions(Client::Dsh, &machine).contains("VIBEMEMORY_DIR=/data/engine"));
}
