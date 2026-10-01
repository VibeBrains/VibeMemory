//! `vibememory mcp-config <client>`: how to connect the memory server to one client, printed for
//! this machine — the path to the server under this system, the agent name the client signs with,
//! the file it goes into and in that file's own format.
//!
//! Printed, never written: every client keeps its registration in a file of its own, in a format
//! that changes with its versions, and the engine does not write into other programs'
//! configuration (the same rule as for `.claude.json`). A wrong agent name is what made a client
//! look connected and stay empty; the printed text carries the right one.
//!
//! The formats were read from each client's documentation on 2026-09-30, and from the real files
//! of the clients installed on the owner's Mac: `knowledge/design/mcpClients.md`.

use std::fmt::Write as _;

/// A client the engine knows how to connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    /// Claude Code, the CLI and the IDE extensions.
    ClaudeCode,
    /// The Claude Desktop app's own chat.
    ClaudeDesktop,
    /// Codex: the CLI, the IDE extension and the `ChatGPT` app share one file.
    Codex,
    /// Gemini CLI.
    Gemini,
    /// Cursor.
    Cursor,
    /// `DeepSeek` Harness.
    Dsh,
    /// `VibeIDE`, which connects the server itself.
    VibeIde,
    /// `VibeIDEA`, which connects the server itself.
    VibeIdea,
    /// `ChatGPT`: the browser cannot, the app goes through Codex's file.
    ChatGpt,
}

/// Every client, in the order `mcp-config` lists them.
pub const CLIENTS: &[Client] = &[
    Client::ClaudeCode,
    Client::ClaudeDesktop,
    Client::Codex,
    Client::ChatGpt,
    Client::Gemini,
    Client::Cursor,
    Client::Dsh,
    Client::VibeIde,
    Client::VibeIdea,
];

impl Client {
    /// The name `mcp-config` takes.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor",
            Self::Dsh => "dsh",
            Self::VibeIde => "vibeide",
            Self::VibeIdea => "vibeidea",
            Self::ChatGpt => "chatgpt",
        }
    }

    /// The agent name the client passes as `--agent`: what every record it writes is signed with,
    /// and the name a token for it has to be taken under.
    #[must_use]
    pub fn agent(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::ClaudeDesktop => "claude-desktop",
            // the ChatGPT app starts the servers of Codex's file, so it signs as Codex
            Self::Codex | Self::ChatGpt => "codex",
            Self::Gemini => "gemini",
            Self::Cursor => "cursor",
            Self::Dsh => "dsh-desktop",
            Self::VibeIde => "vibeide",
            Self::VibeIdea => "vibeidea",
        }
    }

    /// The client `mcp-config` was asked for.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        CLIENTS.iter().copied().find(|client| client.name() == name)
    }
}

/// What differs between machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// The installed memory server, absolute.
    pub server: String,
    /// The engine directory, when it is not the default one: then the server has to be told it.
    pub engine_dir: Option<String>,
    /// The home directory: the working directory that holds no project, so a write has to name one.
    pub home: String,
    /// Windows: `.exe`, `%APPDATA%`, backslashes.
    pub windows: bool,
}

/// The server's name in every client: one name, so an agent never chooses between two memories.
const SERVER: &str = crate::registrations::LOCAL_SERVER;

impl Machine {
    /// This machine: the server `install` placed, and the engine directory when it is not the
    /// default one.
    #[must_use]
    pub fn of(layout: &crate::install::Layout) -> Self {
        let home = crate::install::home_dir().unwrap_or_default();
        let default_engine = home.join(".vibememory");
        Self {
            server: crate::install::installed_named(layout, crate::install::MCP_BINARY)
                .display()
                .to_string(),
            engine_dir: (layout.engine_dir != default_engine)
                .then(|| layout.engine_dir.display().to_string()),
            home: home.display().to_string(),
            windows: cfg!(windows),
        }
    }
}

/// The one command that registers the server with Claude Code for every project.
#[must_use]
pub fn claude_code_command(machine: &Machine) -> String {
    format!(
        "claude mcp add -s user{} {SERVER} -- {}",
        env_flag(machine, "-e"),
        server_command(machine, Client::ClaudeCode.agent())
    )
}

/// A string as JSON writes it, quotes included — also a valid YAML double-quoted string.
fn quoted(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

/// A JSON `mcpServers` block with the one server. `cwd` only for the clients that read it.
fn json_block(machine: &Machine, agent: &str, typed: bool, cwd: bool) -> String {
    let mut server = serde_json::Map::new();
    if typed {
        server.insert("type".to_owned(), "stdio".into());
    }
    server.insert("command".to_owned(), machine.server.clone().into());
    server.insert("args".to_owned(), serde_json::json!(["--agent", agent]));
    if cwd {
        server.insert("cwd".to_owned(), machine.home.clone().into());
    }
    if let Some(dir) = &machine.engine_dir {
        server.insert(
            "env".to_owned(),
            serde_json::json!({ "VIBEMEMORY_DIR": dir }),
        );
    }
    let block = serde_json::json!({ "mcpServers": { SERVER: server } });
    serde_json::to_string_pretty(&block).unwrap_or_default()
}

/// The Codex table.
fn toml_block(machine: &Machine, agent: &str) -> String {
    let mut lines = vec![
        format!("[mcp_servers.{SERVER}]"),
        format!("command = {}", quoted(&machine.server)),
        format!("args = [\"--agent\", {}]", quoted(agent)),
    ];
    if let Some(dir) = &machine.engine_dir {
        lines.push(format!("env = {{ VIBEMEMORY_DIR = {} }}", quoted(dir)));
    }
    lines.join("\n")
}

/// The command-line tail that starts the server, for the clients that add servers by command.
fn server_command(machine: &Machine, agent: &str) -> String {
    format!("{} --agent {agent}", quoted(&machine.server))
}

/// A path under the home directory, written the way this system writes it.
fn home_path(machine: &Machine, parts: &[&str]) -> String {
    let separator = if machine.windows { "\\" } else { "/" };
    let mut path = machine.home.clone();
    for part in parts {
        path.push_str(separator);
        path.push_str(part);
    }
    path
}

/// How to connect one client on this machine, as a person reads it.
#[must_use]
pub fn instructions(client: Client, machine: &Machine) -> String {
    let lines = match client {
        Client::ClaudeCode => claude_code(machine),
        Client::ClaudeDesktop => claude_desktop(machine),
        Client::Codex => codex(machine),
        Client::ChatGpt => chatgpt(machine),
        Client::Gemini => gemini(machine),
        Client::Cursor => cursor(machine),
        Client::Dsh => dsh(machine),
        Client::VibeIde => vibeide(machine),
        Client::VibeIdea => vibeidea(machine),
    };
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// ` <flag> VIBEMEMORY_DIR=<dir>` when the engine is not in its default place, else nothing.
fn env_flag(machine: &Machine, flag: &str) -> String {
    machine
        .engine_dir
        .as_ref()
        .map(|dir| format!(" {flag} VIBEMEMORY_DIR={}", quoted(dir)))
        .unwrap_or_default()
}

fn claude_code(machine: &Machine) -> Vec<String> {
    vec![
        "Claude Code — one command, for every project (-s user):".to_owned(),
        String::new(),
        format!("  {}", claude_code_command(machine)),
        String::new(),
        "Then: `claude mcp list` shows vibememory; a new session has the memory tools.".to_owned(),
    ]
}

fn claude_desktop(machine: &Machine) -> Vec<String> {
    let file = if machine.windows {
        "%APPDATA%\\Claude\\claude_desktop_config.json".to_owned()
    } else {
        home_path(
            machine,
            &[
                "Library",
                "Application Support",
                "Claude",
                "claude_desktop_config.json",
            ],
        )
    };
    vec![
        format!("Claude Desktop — into {file}"),
        "(Settings → Developer → Edit Config), merged with what mcpServers already holds:"
            .to_owned(),
        String::new(),
        json_block(machine, Client::ClaudeDesktop.agent(), false, false),
        String::new(),
        "Then quit Claude Desktop completely and start it again.".to_owned(),
    ]
}

fn codex(machine: &Machine) -> Vec<String> {
    let agent = Client::Codex.agent();
    vec![
        format!(
            "Codex (CLI, IDE extension, ChatGPT app) — into {}:",
            home_path(machine, &[".codex", "config.toml"])
        ),
        String::new(),
        toml_block(machine, agent),
        String::new(),
        "or by command:".to_owned(),
        String::new(),
        format!(
            "  codex mcp add {SERVER}{} -- {}",
            env_flag(machine, "--env"),
            server_command(machine, agent)
        ),
        String::new(),
        "Then: a new session; the app — Settings → MCP servers → Restart.".to_owned(),
    ]
}

fn chatgpt(machine: &Machine) -> Vec<String> {
    let mut lines = vec![
        "ChatGPT in the browser cannot use this server: it starts no local servers, and a remote one it"
            .to_owned(),
        "reaches only with OAuth, while the memory server asks for a token.".to_owned(),
        "The ChatGPT app on the computer starts the servers Codex's file names — this is that entry:"
            .to_owned(),
        String::new(),
    ];
    lines.extend(codex(machine));
    lines
}

fn gemini(machine: &Machine) -> Vec<String> {
    vec![
        "Gemini CLI — one command, for every project (-s user; without it the server is the project's):"
            .to_owned(),
        String::new(),
        format!(
            "  gemini mcp add -s user{} {SERVER} {} -- --agent {}",
            env_flag(machine, "-e"),
            quoted(&machine.server),
            Client::Gemini.agent()
        ),
        String::new(),
        "Then: `/mcp reload` in a running session, or a new one.".to_owned(),
    ]
}

fn cursor(machine: &Machine) -> Vec<String> {
    vec![
        format!(
            "Cursor — into {}, merged with what mcpServers already holds:",
            home_path(machine, &[".cursor", "mcp.json"])
        ),
        String::new(),
        json_block(machine, Client::Cursor.agent(), true, false),
        String::new(),
        "Then restart Cursor.".to_owned(),
    ]
}

fn dsh(machine: &Machine) -> Vec<String> {
    let mut lines = vec![
        format!(
            "DeepSeek Harness — at the end of {}:",
            home_path(machine, &[".dsh", "profiles", "<profile>", "cordis.patch.yml"])
        ),
        String::new(),
        "- insert:".to_owned(),
        "    - id: mcp-vibememory".to_owned(),
        "      name: \"@deepseek-ai/dsh-mcp-client\"".to_owned(),
        "      config:".to_owned(),
        format!("        serverName: {SERVER}"),
        "        transport: stdio".to_owned(),
        format!("        command: {}", quoted(&machine.server)),
        format!("        args: [\"--agent\", \"{}\"]", Client::Dsh.agent()),
        format!("        cwd: {}", quoted(&machine.home)),
        String::new(),
        "The home directory as cwd holds no project: a write names its project, and one DSH serves every"
            .to_owned(),
        "project it opens. Then restart DSH: a new session does not read the profile again.".to_owned(),
        "Its sessions go to history once the engine reads its logs, from now on or with the past too:"
            .to_owned(),
        String::new(),
        format!(
            "  vibememory session agent add --agent {} --preset dsh [--backfill]",
            Client::Dsh.agent()
        ),
    ];
    if let Some(dir) = &machine.engine_dir {
        lines.push(format!(
            "The engine is not in its default place: DSH has to run with VIBEMEMORY_DIR={dir}."
        ));
    }
    lines
}

fn vibeide(machine: &Machine) -> Vec<String> {
    vec![
        "VibeIDE connects the server itself: when the server is installed it adds `vibememory` with".to_owned(),
        format!(
            "--agent {}, started in the home directory, and names each open folder's project.",
            Client::VibeIde.agent()
        ),
        "After installing the engine, reload the window. Nothing to register.".to_owned(),
        format!(
            "An entry of your own named vibememory in {} replaces it; it can be switched off in the MCP view.",
            home_path(machine, &[".vibeide", "mcp.json"])
        ),
    ]
}

fn vibeidea(machine: &Machine) -> Vec<String> {
    vec![
        "VibeIDEA connects the server itself: for ACP agents at every session and for the direct chat,".to_owned(),
        format!(
            "with --agent {}, as soon as the server is installed — no restart. Nothing to register.",
            Client::VibeIdea.agent()
        ),
        "An entry of your own named vibememory in a project's .vibe/mcp.json replaces it from 0.9.0;".to_owned(),
        "before 0.9.0 do not add one: the agent would get the server twice.".to_owned(),
        format!(
            "Switched off for ACP agents by \"use_custom_mcp\": false under default_mcp_settings in {}.",
            home_path(machine, &[".jetbrains", "acp.json"])
        ),
    ]
}

/// The list `mcp-config` prints without a client.
#[must_use]
pub fn overview() -> String {
    let mut text = String::from(
        "vibememory mcp-config <client> prints how to connect the memory server to it on this machine:\n",
    );
    for client in CLIENTS {
        let _ = writeln!(text, "  {:<15} --agent {}", client.name(), client.agent());
    }
    text
}
