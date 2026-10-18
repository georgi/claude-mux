//! Turning the engine's `SpawnAgent` effect into a process description.
//!
//! `plan_spawn` is pure: it gets the session id and token as arguments and
//! returns what to write and what to run. `build_spawn` adds the randomness,
//! and `write_files` puts the files on disk (0600 for those holding the token).

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cmux_core::engine::Effect;
use cmux_core::ids::AgentId;
use cmux_core::model::Level;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Variables of an enclosing Claude Code session that must not reach a nested
/// `claude`: some make it think it is a child (identity, messaging socket), others
/// carry the parent's mode and memory (an agent started under auto mode and loaded
/// the parent's memory stores). User configuration (`ANTHROPIC_*`,
/// `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_MAX_OUTPUT_TOKENS`,
/// proxies) is deliberately not listed. The single-user `cmux` client and the
/// terminal host use this list too.
pub const SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_PLUGIN_DATA",
    "AI_AGENT",
    "CLAUDE_MEMORY_STORES",
    "CLAUDE_ADDITIONAL_DIRECTORIES",
    "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD",
    "CLAUDE_AFTER_LAST_COMPACT",
    "CLAUDE_CODE_PROJECTS_SESSION",
    "CLAUDE_CODE_SKILL_PROPOSALS",
    "CLAUDE_CODE_WORKER_EPOCH",
    "CLAUDE_CODE_CCR_SURFACE",
    "CLAUDE_CODE_USE_CCR_V2",
    "CLAUDE_CODE_POST_FOR_SESSION_INGRESS_V2",
    "CLAUDE_CODE_TEE_SDK_STDOUT",
    "CLAUDE_SESSION_INGRESS_TOKEN_FILE",
    "CLAUDE_CODE_COORDINATOR_EXTRA_TOOLS",
    "CLAUDE_CODE_TERMINAL_MCP_TOOLS",
    "CLAUDE_CODE_SILENT_TURN_REMINDER",
    "CLAUDE_CODE_BG_TASKS_REPORT_RUNNING",
    "CLAUDE_CODE_DIAGNOSTICS_FILE",
    "CLAUDE_CODE_CONTAINER_ID",
    "CLAUDE_CODE_ENVIRONMENT_RUNNER_VERSION",
    "CLAUDE_EFFORT",
];

/// Prefixes to scrub as well (spike S1: `CLAUDE_CODE_REMOTE_SESSION_ID` leaks the
/// outer session id into hook payloads when cmuxd itself runs inside Claude).
pub const SESSION_ENV_PREFIXES: &[&str] = &[
    "CLAUDE_CODE_REMOTE",
    "CCR_",
    "CLAUDE_COWORK_",
    "CLAUDE_CODE_SYNC_",
];

/// Whether `name` is on the scrub lists.
pub fn is_session_env(name: &str) -> bool {
    SESSION_ENV.contains(&name) || SESSION_ENV_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The events each agent's `settings.json` registers `cmuxd hook <event>` for.
pub const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "PreToolUse",
    "PermissionRequest",
    "SessionEnd",
];

/// Name of the one MCP server, so its tools are `mcp__cmux__*`.
pub const MCP_SERVER: &str = "cmux";

/// Hook timeout in seconds for everything but `PermissionRequest`.
const HOOK_TIMEOUT_SECS: u64 = 5;

/// Per-level options for `claude`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LevelOpts {
    /// `--permission-mode`; unset means `default`, whatever the account prefers.
    pub permission_mode: Option<String>,
    /// `--model`
    pub model: Option<String>,
}

/// Server-wide inputs to a spawn.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// The `claude` binary.
    pub claude_bin: String,
    /// This executable, which runs `hook` and `mcp`.
    pub cmuxd_exe: PathBuf,
    /// The agent API socket (`CMUX_SOCK`).
    pub sock: PathBuf,
    /// The store root (`~/.local/share/cmuxd`).
    pub data_dir: PathBuf,
    /// How long a `PermissionRequest` hook may be held open.
    pub permission_wait: Duration,
    /// Mark each agent's cwd trusted in Claude's global config before starting
    /// it. Off in `new` so tests never touch the real `~/.claude.json`;
    /// `config.toml` turns it on by default.
    pub pre_trust: bool,
    pub executor: LevelOpts,
    pub task: LevelOpts,
    pub sub: LevelOpts,
}

impl SpawnConfig {
    pub fn new(cmuxd_exe: PathBuf, sock: PathBuf, data_dir: PathBuf) -> Self {
        SpawnConfig {
            claude_bin: "claude".into(),
            cmuxd_exe,
            sock,
            data_dir,
            permission_wait: Duration::from_secs(600),
            pre_trust: false,
            executor: LevelOpts::default(),
            task: LevelOpts::default(),
            sub: LevelOpts::default(),
        }
    }

    fn opts(&self, level: Level) -> &LevelOpts {
        match level {
            Level::Executor => &self.executor,
            Level::Task => &self.task,
            Level::Sub => &self.sub,
        }
    }
}

/// The fields of `Effect::SpawnAgent`, all of them.
#[derive(Debug, Clone, Copy)]
pub struct SpawnRequest<'a> {
    pub agent: &'a AgentId,
    pub level: Level,
    pub cwd: &'a Path,
    pub session_id: Option<&'a str>,
    pub resume: bool,
    pub system_prompt: &'a str,
    pub kickoff: &'a str,
}

impl<'a> SpawnRequest<'a> {
    /// `None` unless `effect` is `SpawnAgent`.
    pub fn from_effect(effect: &'a Effect) -> Option<Self> {
        match effect {
            Effect::SpawnAgent {
                agent,
                term: _,
                level,
                cwd,
                session_id,
                resume,
                system_prompt,
                kickoff,
            } => Some(SpawnRequest {
                agent,
                level: *level,
                cwd,
                session_id: session_id.as_deref(),
                resume: *resume,
                system_prompt,
                kickoff,
            }),
            _ => None,
        }
    }
}

/// A file to write before the process starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnFile {
    pub path: PathBuf,
    pub contents: String,
    /// Holds the agent token: written with mode 0600.
    pub secret: bool,
}

/// Everything needed to start one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    /// `argv[0]` is the `claude` binary.
    pub argv: Vec<String>,
    /// Variables to set (after scrubbing).
    pub env: Vec<(String, String)>,
    /// Variables to remove from the inherited environment.
    pub env_remove: Vec<String>,
    /// Remove inherited variables starting with any of these.
    pub env_remove_prefixes: Vec<String>,
    pub cwd: PathBuf,
    pub files: Vec<SpawnFile>,
    /// Goes into `RunnerEvent::Spawned`.
    pub session_id: String,
    /// Goes into `RunnerEvent::Spawned`: SHA-256 of the token's UTF-8 bytes.
    pub token_hash: [u8; 32],
}

impl SpawnSpec {
    /// The environment the child gets: `base` without scrubbed variables, then
    /// `self.env` on top.
    pub fn final_env(
        &self,
        base: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<(String, String)> {
        let set: BTreeSet<&str> = self.env.iter().map(|(k, _)| k.as_str()).collect();
        let mut out: Vec<(String, String)> = base
            .into_iter()
            .filter(|(k, _)| {
                !set.contains(k.as_str())
                    && !self.env_remove.contains(k)
                    && !self.env_remove_prefixes.iter().any(|p| k.starts_with(p))
            })
            .collect();
        out.extend(self.env.iter().cloned());
        out
    }
}

/// `<data>/plans/<plan>/agents/<id>`. The sub-agent `/` becomes `_` (names never
/// contain it).
pub fn agent_dir(data_dir: &Path, agent: &AgentId) -> PathBuf {
    data_dir
        .join("plans")
        .join(agent.plan().as_str())
        .join("agents")
        .join(agent.path().to_string().replace('/', "_"))
}

/// `<agent dir>/digests`: where long digests are written for the agent to read.
/// Only this directory is given to `--add-dir`, not the agent dir, whose
/// `settings.json` (hooks, permissions) the agent must not be free to edit.
pub fn digest_dir(data_dir: &Path, agent: &AgentId) -> PathBuf {
    agent_dir(data_dir, agent).join("digests")
}

/// SHA-256 of the token, as stored in the engine.
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// A random 256-bit token, hex encoded.
pub fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Like `plan_spawn` with a fresh token, and a fresh session id unless the
/// request carries one.
pub fn build_spawn(cfg: &SpawnConfig, req: &SpawnRequest<'_>) -> SpawnSpec {
    let session = req
        .session_id
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    plan_spawn(cfg, req, &session, &new_token())
}

/// The pure part. A resume uses `--resume <session>`, otherwise `--session-id`.
pub fn plan_spawn(
    cfg: &SpawnConfig,
    req: &SpawnRequest<'_>,
    session_id: &str,
    token: &str,
) -> SpawnSpec {
    let dir = agent_dir(&cfg.data_dir, req.agent);
    let settings = dir.join("settings.json");
    let mcp = dir.join("mcp.json");
    let system = dir.join("system.md");
    let opts = cfg.opts(req.level);

    let mut argv = vec![cfg.claude_bin.clone()];
    argv.push(
        if req.resume {
            "--resume"
        } else {
            "--session-id"
        }
        .into(),
    );
    argv.push(session_id.into());
    argv.push("--settings".into());
    argv.push(settings.display().to_string());
    // The `=` form: the flag is variadic and would swallow the prompt (spike S4).
    argv.push(format!("--mcp-config={}", mcp.display()));
    // Digests are written outside the agent's cwd: without this every read of
    // one asks for permission. Also the `=` form, for the same reason.
    argv.push(format!("--add-dir={}", dir.join("digests").display()));
    argv.push("--append-system-prompt".into());
    argv.push(req.system_prompt.into());
    // Always pass a mode: without the flag the account's own default applies,
    // which on some accounts is `auto`.
    argv.push("--permission-mode".into());
    argv.push(opts.permission_mode.as_deref().unwrap_or("default").into());
    if let Some(model) = &opts.model {
        argv.push("--model".into());
        argv.push(model.clone());
    }
    argv.push(req.kickoff.into());

    let identity = [
        ("CMUX_SOCK", cfg.sock.display().to_string()),
        ("CMUX_AGENT", req.agent.to_string()),
        ("CMUX_TOKEN", token.to_string()),
    ];
    let mut env: Vec<(String, String)> = identity
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push((
        "CMUX_PERMISSION_WAIT_SECS".into(),
        cfg.permission_wait.as_secs().max(1).to_string(),
    ));

    let files = vec![
        SpawnFile {
            path: settings,
            contents: pretty(&settings_json(cfg)),
            secret: false,
        },
        SpawnFile {
            path: mcp,
            contents: pretty(&mcp_json(cfg, &identity)),
            secret: true,
        },
        SpawnFile {
            path: system,
            contents: req.system_prompt.to_string(),
            secret: false,
        },
    ];

    SpawnSpec {
        argv,
        env,
        env_remove: SESSION_ENV.iter().map(|s| s.to_string()).collect(),
        env_remove_prefixes: SESSION_ENV_PREFIXES.iter().map(|s| s.to_string()).collect(),
        cwd: req.cwd.to_path_buf(),
        files,
        session_id: session_id.to_string(),
        token_hash: token_hash(token),
    }
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("json value serializes");
    s.push('\n');
    s
}

/// Single-quote for `sh -c`, which is how Claude runs hook commands.
fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+=:,@".contains(&b))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Hooks only, plus the allow rule for our own MCP server (otherwise its first
/// call prompts, spike S4). The agent identity reaches `cmuxd hook` through the
/// environment (`CMUX_AGENT`, `CMUX_TOKEN`, `CMUX_SOCK`), so no secret is in here.
fn settings_json(cfg: &SpawnConfig) -> Value {
    let exe = sh_quote(&cfg.cmuxd_exe.display().to_string());
    let mut hooks = serde_json::Map::new();
    for event in HOOK_EVENTS {
        let timeout = if *event == "PermissionRequest" {
            cfg.permission_wait.as_secs().max(1)
        } else {
            HOOK_TIMEOUT_SECS
        };
        hooks.insert(
            event.to_string(),
            json!([{ "hooks": [{
                "type": "command",
                "command": format!("{exe} hook {event}"),
                "timeout": timeout,
            }]}]),
        );
    }
    json!({
        "hooks": hooks,
        "permissions": { "allow": [format!("mcp__{MCP_SERVER}")] },
    })
}

fn mcp_json(cfg: &SpawnConfig, identity: &[(&str, String); 3]) -> Value {
    let env: serde_json::Map<String, Value> = identity
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.clone())))
        .collect();
    json!({ "mcpServers": { MCP_SERVER: {
        "command": cfg.cmuxd_exe.display().to_string(),
        "args": ["mcp"],
        "env": env,
    }}})
}

/// Session tools that run without a permission prompt. `accept_plan` starts
/// agents, so Claude asks first.
const SESSION_ALLOWED: &[&str] = &[
    "list_plans",
    "get_plan",
    "create_plan",
    "create_task",
    "get_task",
    "update_task",
];

/// `<data>/session/mcp.json`: the plan tools of `cmuxd mcp --session`, shared
/// by every Claude terminal the server starts. It holds no secret.
pub fn session_mcp_file(data_dir: &Path) -> PathBuf {
    data_dir.join("session").join("mcp.json")
}

pub fn session_mcp_json(cmuxd_exe: &Path, sock: &Path) -> String {
    pretty(&json!({ "mcpServers": { MCP_SERVER: {
        "command": cmuxd_exe.display().to_string(),
        "args": ["mcp", "--session"],
        "env": { "CMUX_SOCK": sock.display().to_string() },
    }}}))
}

/// The flags that give a Claude terminal the session tools. The `=` forms,
/// because both flags are variadic (spike S4).
pub fn session_args(mcp_file: &Path) -> Vec<String> {
    let allowed: Vec<String> = SESSION_ALLOWED
        .iter()
        .map(|t| format!("mcp__{MCP_SERVER}__{t}"))
        .collect();
    vec![
        format!("--mcp-config={}", mcp_file.display()),
        format!("--allowedTools={}", allowed.join(",")),
    ]
}

/// Writes each file (creating parent directories) through a temp file and a
/// rename. Secret files are private on Unix.
pub fn write_files(files: &[SpawnFile]) -> std::io::Result<()> {
    for f in files {
        write_file(f)?;
    }
    Ok(())
}

fn write_file(f: &SpawnFile) -> std::io::Result<()> {
    let parent = f.path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = f.path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(if f.secret { 0o600 } else { 0o644 });
    }
    let mut file = opts.open(&tmp)?;
    #[cfg(unix)]
    if f.secret {
        // A leftover temp file may have been made with a wider mode.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(f.contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, &f.path)
}

#[cfg(test)]
mod tests;
