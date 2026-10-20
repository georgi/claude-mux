//! A stand-in for `claude`, for the end-to-end tests. Not installed (it is an
//! example target, so `cargo install` skips it; `cargo test` builds it).
//!
//! It reads its argv the way Claude does (`--session-id`/`--resume`,
//! `--settings`, `--mcp-config=`, the kickoff as the last argument), then
//! behaves like a session:
//!
//! 1. starts the MCP server from `mcp.json` (as `cmuxd mcp`, with the env from
//!    the file), initializes it and prints `CWD: <dir>`, `SYSTEM: <first line
//!    of the appended system prompt>` and `TOOLS: a,b,c`;
//! 2. runs the `SessionStart` and `UserPromptSubmit` (kickoff) hooks from
//!    `settings.json` through `sh -c`, as Claude does;
//! 3. runs the agent's script, then the `Stop` hook; a `block` decision is
//!    printed (`STOP-BLOCKED: <reason>`), its reason becomes the prompt for the
//!    `later` steps that have a `when` (Claude carries on with what the hook
//!    said), and `Stop` runs again with `stop_hook_active`;
//! 4. idles reading its terminal: every line is echoed (`ECHO <line>`, paste
//!    markers removed), submitted as a prompt, answered by the `later` script
//!    and ended with `Stop`. `/exit` runs `SessionEnd` and exits; `/clear`
//!    runs `SessionEnd` (reason `clear`) and `SessionStart` with a new
//!    session id and a new transcript.
//!
//! It keeps a JSONL transcript the way Claude does, one file per session id
//! in `$FAKE_CLAUDE_SCRIPTS/transcripts/` (else the temp dir), and sends its
//! path as `transcript_path` in every hook: the turn's prompt as a user line,
//! each tool call and its result, each `say` as an assistant text line, and,
//! for a turn that said nothing, the text it ends with.
//!
//! The script is `$FAKE_CLAUDE_SCRIPTS/<agent path>.json` (`/` becomes `_`):
//! `{"first": [steps], "later": [steps]}`. Any step may carry `"when": text`
//! and then runs only if the turn's prompt contains that text. Steps:
//!
//! - `{"tool": name, "args": {..}}`: an MCP `tools/call`; first runs `PreToolUse`
//!   with `tool_name` `mcp__cmux__<name>` (the server's name in `mcp.json`), as
//!   Claude does before every tool; prints `TOOL <name>: <text>`
//! - `{"permission": {"tool": "Bash", "input": {"command": ".."}}}`: runs
//!   `PreToolUse`; if that denies (`permissionDecision`) it prints
//!   `PRETOOL deny: <reason>` and stops there, as Claude does. Otherwise runs
//!   `PermissionRequest`. A line `1` on the terminal while the
//!   hook is open wins: the hook is killed (`PERM terminal`). Otherwise prints
//!   `PERM allow`, `PERM deny: <message>` or `PERM none`. A deny from
//!   `PreToolUse` ends the step first: `PRE deny: <reason>`
//! - `{"say": text}`: an assistant text line in the transcript
//! - `{"think": text}`: an assistant thinking line in the transcript
//! - `{"call": name, "input": {..}, "result": text, "is_error": bool}`: a
//!   `tool_use` line and its `tool_result` in the transcript, with no MCP call
//!   (for built-in tools such as `Edit` or `Bash`)
//! - `{"sleep_ms": n}`, `{"wait_file": path}`, `{"print": text}`,
//!   `{"exit": code}` (after `SessionEnd`)

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn say(text: impl AsRef<str>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", text.as_ref());
    let _ = out.flush();
}

struct Args {
    session_id: String,
    settings: PathBuf,
    mcp_config: PathBuf,
    system: String,
    kickoff: String,
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        session_id: String::new(),
        settings: PathBuf::new(),
        mcp_config: PathBuf::new(),
        system: String::new(),
        kickoff: argv.last().cloned().unwrap_or_default(),
    };
    let mut i = 0;
    while i < argv.len() {
        let next = argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--session-id" | "--resume" => {
                a.session_id = next;
                i += 1;
            }
            "--settings" => {
                a.settings = next.into();
                i += 1;
            }
            "--append-system-prompt" => {
                a.system = next;
                i += 1;
            }
            "--permission-mode" | "--model" => i += 1,
            s if s.starts_with("--mcp-config=") => {
                a.mcp_config = s["--mcp-config=".len()..].into();
            }
            _ => {}
        }
        i += 1;
    }
    a
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("fake-claude: cannot read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("valid json")
}

// ---------------------------------------------------------------------------
// Hooks
// ---------------------------------------------------------------------------

struct Hooks {
    settings: Value,
    session_id: String,
    cwd: String,
    transcript: PathBuf,
}

impl Hooks {
    fn command(&self, event: &str) -> Option<String> {
        self.settings["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .map(str::to_string)
    }

    fn payload(&self, event: &str, extra: Value) -> Value {
        let mut p = json!({
            "session_id": self.session_id,
            "transcript_path": self.transcript.display().to_string(),
            "cwd": self.cwd,
            "hook_event_name": event,
        });
        if let (Some(o), Value::Object(e)) = (p.as_object_mut(), extra) {
            o.extend(e);
        }
        p
    }

    /// Starts the hook command the way Claude does: `sh -c`, payload on stdin.
    fn start(&self, event: &str, extra: Value) -> Option<(Child, ChildStdout)> {
        let cmd = self.command(event)?;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start hook");
        let mut stdin: ChildStdin = child.stdin.take().expect("stdin");
        let _ = stdin.write_all(self.payload(event, extra).to_string().as_bytes());
        drop(stdin);
        let out = child.stdout.take().expect("stdout");
        Some((child, out))
    }

    /// Runs a hook to completion; its parsed output, if it printed any.
    fn run(&self, event: &str, extra: Value) -> Option<Value> {
        let (mut child, mut out) = self.start(event, extra)?;
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        let status = child.wait().expect("wait hook");
        assert!(status.success(), "hook {event} must exit 0, got {status}");
        serde_json::from_str(text.trim()).ok()
    }
}

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

/// Where the transcript of `session_id` goes.
fn transcript_path(session_id: &str) -> PathBuf {
    let dir = std::env::var_os("FAKE_CLAUDE_SCRIPTS")
        .map_or_else(|| std::env::temp_dir().join("fake-claude"), PathBuf::from)
        .join("transcripts");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{session_id}.jsonl"))
}

/// Now as Claude writes it: `2026-10-18T09:00:01.101Z`.
fn now_rfc3339() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let (days, rem) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Civil date from days (Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, mi, s, milli) = (
        rem / 3_600_000,
        rem / 60_000 % 60,
        rem / 1000 % 60,
        rem % 1000,
    );
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

/// Appends transcript lines.
struct Transcript {
    path: PathBuf,
    session_id: String,
    cwd: String,
    next: u64,
    /// The turn wrote an assistant text.
    said: bool,
}

impl Transcript {
    fn new(session_id: &str, cwd: &str) -> Transcript {
        Transcript {
            path: transcript_path(session_id),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            next: 0,
            said: false,
        }
    }

    fn write(&mut self, kind: &str, message: Value) {
        self.next += 1;
        let line = json!({
            "type": kind,
            "uuid": format!("{}-{}", &self.session_id[..self.session_id.len().min(8)], self.next),
            "timestamp": now_rfc3339(),
            "sessionId": self.session_id,
            "cwd": self.cwd,
            "isSidechain": false,
            "message": message,
        });
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .expect("open transcript");
        writeln!(f, "{line}").expect("write transcript");
    }

    fn assistant(&mut self, content: Value) {
        let id = format!("msg_{}", self.next + 1);
        self.write(
            "assistant",
            json!({"role": "assistant", "id": id, "model": "fake", "content": content,
                   "usage": {"input_tokens": 10, "output_tokens": 5,
                             "cache_read_input_tokens": 100, "cache_creation_input_tokens": 1}}),
        );
    }

    fn user(&mut self, text: &str) {
        self.said = false;
        self.write("user", json!({"role": "user", "content": text}));
    }

    fn say(&mut self, text: &str) {
        self.said = true;
        self.assistant(json!([{"type": "text", "text": text}]));
    }

    fn think(&mut self, text: &str) {
        self.assistant(json!([{"type": "thinking", "thinking": text, "signature": "ZmFrZQ=="}]));
    }

    fn tool(&mut self, name: &str, args: &Value, result: &str, is_error: bool) {
        let id = format!("toolu_{}", self.next + 1);
        self.assistant(json!([{"type": "tool_use", "id": id, "name": name, "input": args}]));
        self.write(
            "user",
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": result,
                 "is_error": is_error}]}),
        );
    }
}

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(config: &Value) -> Mcp {
        let server = &config["mcpServers"]["cmux"];
        let mut cmd = Command::new(server["command"].as_str().expect("command"));
        for a in server["args"].as_array().into_iter().flatten() {
            cmd.arg(a.as_str().expect("arg"));
        }
        for (k, v) in server["env"].as_object().into_iter().flatten() {
            cmd.env(k, v.as_str().expect("env value"));
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start mcp server");
        let stdin = child.stdin.take().expect("stdin");
        let out = BufReader::new(child.stdout.take().expect("stdout"));
        Mcp {
            child,
            stdin,
            out,
            next_id: 0,
        }
    }

    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").expect("write to mcp");
        self.stdin.flush().expect("flush mcp");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        self.out.read_line(&mut line).expect("read from mcp");
        let v: Value = serde_json::from_str(&line).expect("mcp reply is json");
        assert_eq!(v["id"], id, "reply to the request we sent");
        v
    }

    fn initialize(&mut self) -> Vec<String> {
        self.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "fake-claude", "version": "0"}}),
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let list = self.request("tools/list", json!({}));
        list["result"]["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["name"].as_str().map(str::to_string))
            .collect()
    }

    fn call(&mut self, name: &str, args: &Value) -> String {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

struct Session {
    hooks: Hooks,
    transcript: Transcript,
    mcp: Mcp,
    lines: Receiver<Option<String>>,
    script: Value,
    /// The prompt of the current turn, for `when`.
    prompt: String,
}

fn strip_paste(line: &str) -> String {
    line.replace("\x1b[200~", "").replace("\x1b[201~", "")
}

impl Session {
    fn step(&mut self, step: &Value) {
        if let Some(w) = step["when"].as_str()
            && !self.prompt.contains(w)
        {
            return;
        }
        if let Some(name) = step["tool"].as_str() {
            // Claude runs PreToolUse for MCP tools too, named `mcp__<server>__<tool>`.
            self.hooks.run(
                "PreToolUse",
                json!({
                    "tool_name": format!("mcp__cmux__{name}"),
                    "tool_input": step["args"],
                    "tool_use_id": "toolu_1",
                }),
            );
            let text = self.mcp.call(name, &step["args"]);
            self.transcript
                .tool(&format!("mcp__cmux__{name}"), &step["args"], &text, false);
            say(format!("TOOL {name}: {text}"));
        } else if let Some(name) = step["call"].as_str() {
            let result = step["result"].as_str().unwrap_or("");
            let is_error = step["is_error"].as_bool().unwrap_or(false);
            self.transcript.tool(name, &step["input"], result, is_error);
        } else if let Some(text) = step["say"].as_str() {
            self.transcript.say(text);
        } else if let Some(text) = step["think"].as_str() {
            self.transcript.think(text);
        } else if let Some(p) = step.get("permission") {
            self.permission(p);
        } else if let Some(ms) = step["sleep_ms"].as_u64() {
            std::thread::sleep(Duration::from_millis(ms));
        } else if let Some(path) = step["wait_file"].as_str() {
            let until = Instant::now() + Duration::from_secs(30);
            while !Path::new(path).exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        } else if let Some(text) = step["print"].as_str() {
            say(text);
        } else if let Some(code) = step["exit"].as_i64() {
            self.hooks.run("SessionEnd", json!({"reason": "other"}));
            std::process::exit(code as i32);
        }
    }

    fn steps(&mut self, which: &str) {
        let steps = self.script[which].as_array().cloned().unwrap_or_default();
        for s in &steps {
            self.step(s);
        }
    }

    /// A prompt arriving mid-turn, through a `Stop` block: only the `later`
    /// steps that wait for some text can tell it from the turn they are in.
    fn carry_on(&mut self, reason: &str) {
        self.set_prompt(reason);
        let steps = self.script["later"].as_array().cloned().unwrap_or_default();
        for s in steps.iter().filter(|s| s.get("when").is_some()) {
            self.step(s);
        }
    }

    fn permission(&mut self, p: &Value) {
        let tool = p["tool"].as_str().unwrap_or("Bash");
        let input = p["input"].clone();
        let pre = self.hooks.run(
            "PreToolUse",
            json!({"tool_name": tool, "tool_input": input, "tool_use_id": "toolu_1"}),
        );
        let pre = &pre.unwrap_or(Value::Null)["hookSpecificOutput"];
        if pre["permissionDecision"] == "deny" {
            // Claude does not run the tool and asks for no permission.
            say(format!(
                "PRE deny: {}",
                pre["permissionDecisionReason"].as_str().unwrap_or("")
            ));
            return;
        }
        let Some((mut child, mut out)) = self.hooks.start(
            "PermissionRequest",
            json!({"tool_name": tool, "tool_input": input,
                   "permission_suggestions": [], "permission_mode": "default"}),
        ) else {
            return;
        };
        // Read the hook's output on a thread so we can watch the terminal too.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = out.read_to_string(&mut text);
            let _ = tx.send(text);
        });
        loop {
            if let Ok(text) = rx.recv_timeout(Duration::from_millis(20)) {
                let _ = child.wait();
                let v: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
                let d = &v["hookSpecificOutput"]["decision"];
                match d["behavior"].as_str() {
                    Some("allow") => say("PERM allow"),
                    Some("deny") => say(format!(
                        "PERM deny: {}",
                        d["message"].as_str().unwrap_or("")
                    )),
                    _ => say("PERM none"),
                }
                return;
            }
            if let Ok(Some(line)) = self.lines.try_recv()
                && strip_paste(&line).trim() == "1"
            {
                // A key in the terminal wins; Claude leaves the hook behind.
                let _ = child.kill();
                let _ = child.wait();
                say("PERM terminal");
                return;
            }
        }
    }

    /// Ends the turn: `Stop`, and again after each `block` (spike S2).
    fn stop(&mut self, last: &str) {
        if !self.transcript.said {
            self.transcript.say(last);
        }
        let mut active = false;
        for _ in 0..6 {
            let out = self.hooks.run(
                "Stop",
                json!({"stop_hook_active": active, "last_assistant_message": last}),
            );
            match out.as_ref().and_then(|o| o["decision"].as_str()) {
                Some("block") => {
                    let reason = out.as_ref().expect("some")["reason"].as_str();
                    let reason = reason.unwrap_or("").to_string();
                    say(format!("STOP-BLOCKED: {reason}"));
                    self.carry_on(&reason);
                    active = true;
                }
                _ => return,
            }
        }
    }

    /// Sets the turn's prompt. A digest too long to paste names a file to
    /// read, as Claude would.
    fn set_prompt(&mut self, prompt: &str) {
        self.prompt = prompt.to_string();
        self.transcript.user(prompt);
        // A digest too long to paste names a file to read, as Claude would.
        if let Some((_, path)) = prompt.rsplit_once("read ")
            && let Ok(text) = std::fs::read_to_string(path.trim())
        {
            self.prompt.push('\n');
            self.prompt.push_str(&text);
        }
    }

    fn submit(&mut self, prompt: &str) {
        self.set_prompt(prompt);
        let out = self
            .hooks
            .run("UserPromptSubmit", json!({"prompt": prompt}));
        if let Some(ctx) = out
            .as_ref()
            .and_then(|o| o["hookSpecificOutput"]["additionalContext"].as_str())
        {
            say(format!("CONTEXT: {ctx}"));
        }
    }
}

fn main() {
    let args = parse_args();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    // Claude takes a moment to start; the server needs to have seen `Spawned`.
    std::thread::sleep(Duration::from_millis(150));

    let agent = std::env::var("CMUX_AGENT").unwrap_or_default();
    let path = agent.split_once('/').map_or(agent.as_str(), |(_, p)| p);
    let script = std::env::var("FAKE_CLAUDE_SCRIPTS")
        .ok()
        .map(|d| PathBuf::from(d).join(format!("{}.json", path.replace('/', "_"))))
        .filter(|p| p.exists())
        .map_or(Value::Null, |p| read_json(&p));

    let mut mcp = Mcp::start(&read_json(&args.mcp_config));
    let tools = mcp.initialize();
    say(format!("CWD: {cwd}"));
    say(format!(
        "SYSTEM: {}",
        args.system.lines().next().unwrap_or("")
    ));
    say(format!("TOOLS: {}", tools.join(",")));

    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(std::io::stdin()).lines() {
            if tx.send(l.ok()).is_err() {
                return;
            }
        }
        let _ = tx.send(None);
    });

    let transcript = Transcript::new(&args.session_id, &cwd);
    let mut s = Session {
        hooks: Hooks {
            settings: read_json(&args.settings),
            session_id: args.session_id,
            cwd,
            transcript: transcript.path.clone(),
        },
        transcript,
        mcp,
        lines,
        script,
        prompt: String::new(),
    };
    s.hooks.run(
        "SessionStart",
        json!({"source": "startup", "model": "fake"}),
    );
    say(format!("KICKOFF: {}", args.kickoff));
    s.submit(&args.kickoff.clone());
    s.steps("first");
    s.stop("kickoff done");

    // Idle: every line is a prompt.
    while let Ok(Some(line)) = s.lines.recv() {
        let line = strip_paste(&line);
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        say(format!("ECHO {line}"));
        if line == "/exit" {
            s.hooks
                .run("SessionEnd", json!({"reason": "prompt_input_exit"}));
            break;
        }
        if line == "/clear" {
            s.hooks.run("SessionEnd", json!({"reason": "clear"}));
            let id = uuid::Uuid::new_v4().to_string();
            s.transcript = Transcript::new(&id, &s.hooks.cwd);
            s.hooks.transcript = s.transcript.path.clone();
            s.hooks.session_id = id;
            s.hooks.run("SessionStart", json!({"source": "clear"}));
            continue;
        }
        s.submit(&line);
        s.steps("later");
        s.stop(&line);
    }
    let _ = s.mcp.child.kill();
}
