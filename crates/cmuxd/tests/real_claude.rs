//! The Step 0 spikes S1 to S7 (docs/design/03-implementation-plan.md) as tests
//! against the real `claude` CLI. Each test checks the spike's pass criterion,
//! or the recorded result where that differs (S3).
//!
//! # Running
//!
//! The tests are `#[ignore]`d, so a plain `cargo test` only compiles them:
//!
//! ```text
//! cargo test -p cmuxd --test real_claude -- --ignored
//! cargo test -p cmuxd --test real_claude -- --ignored s3_   # one spike
//! ```
//!
//! They need `claude` and `python3` on `PATH` (no tmux) and take a few minutes:
//! every turn uses `--model haiku`, and S5 and S6 wait out a 60 s idle notice
//! and a 90 s held hook. The tests are independent and run in parallel.
//!
//! # Auth
//!
//! Each test runs `claude` with `--session-id <new uuid>` in a fresh temp
//! working dir, pre-trusted with cmuxd's own trust code. There are two modes:
//!
//! - **Isolated**, when `CLAUDE_CODE_OAUTH_TOKEN` (a long-lived token from
//!   `claude setup-token`) or `ANTHROPIC_API_KEY` is set: `CLAUDE_CONFIG_DIR`
//!   is a temp dir (onboarding done, key prompt pre-approved), so `~/.claude`
//!   and `~/.claude.json` are never read or written. Such a config dir has no
//!   login of its own; on macOS the keychain login does not carry over to it.
//! - **Default** otherwise: the user's own config dir and login, as cmuxd's
//!   agents run. Each session gets `--setting-sources=project,local`, so the
//!   user's hooks stay out, except in S1, which checks
//!   that the user's `permissions.defaultMode` still applies (the user's own
//!   hooks fire there once). The rig writes the temp dir's trust entry into
//!   `~/.claude.json` (under Claude's lock) and deletes
//!   `~/.claude/projects/<temp dir>` afterwards; the trust entry stays.
//!
//! Every session but S1's also gets `--permission-mode manual`, so S5 and S6
//! see permission prompts even when the account defaults to auto mode.
//!
//! A `claude` that is not logged in fails the test with a message that says so.
//!
//! The child gets claude-mux's environment scrub (`cmuxd::term::pty`), so the
//! suite also works when it runs inside a Claude Code session.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "real_claude/harness.rs"]
mod harness;

use harness::{Claude, Event, Rig, hook_entry};

/// A turn of haiku, with tool calls, takes seconds; leave room for slow days.
const TURN: u64 = 180;

fn tagged<'a>(ev: &'a [Event], tag: &str) -> Vec<&'a Event> {
    ev.iter().filter(|e| e.tag == tag).collect()
}

fn bash_commands<'a>(ev: &'a [Event], tag: &str) -> Vec<&'a str> {
    tagged(ev, tag)
        .into_iter()
        .filter(|e| e.str("tool_name") == "Bash")
        .filter_map(|e| e.payload["tool_input"]["command"].as_str())
        .collect()
}

/// Base settings (every event logged) plus `extra` merged on top.
fn settings(rig: &Rig, name: &str, extra: Value) -> String {
    let mut s = json!({ "hooks": rig.base_hooks() });
    if let Value::Object(extra) = extra {
        for (k, v) in extra {
            if k == "hooks" {
                for (event, entry) in v.as_object().unwrap() {
                    s["hooks"][event] = entry.clone();
                }
            } else {
                s[k] = v;
            }
        }
    }
    rig.write_json(name, &s).display().to_string()
}

/// S1: `--settings` hooks are added to the user's and the project's hooks, and
/// their permission rules still apply.
#[test]
#[ignore = "needs a real claude"]
fn s1_settings_flag_merges_with_user_and_project() {
    let rig = Rig::new();
    // Isolated mode writes a user layer. Default mode leaves the user's own
    // settings alone and checks their `permissions.defaultMode` instead.
    let user_mode = match &rig.cfg {
        Some(cfg) => {
            let user = json!({
                "hooks": { "UserPromptSubmit": hook_entry(&rig.hook("USER_UPS"), 60) },
                "permissions": { "deny": ["Bash(touch:*)"] },
            });
            std::fs::write(cfg.join("settings.json"), user.to_string()).unwrap();
            None
        }
        None => {
            let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
            let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(home.join(".claude"));
            std::fs::read_to_string(dir.join("settings.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| v["permissions"]["defaultMode"].as_str().map(String::from))
        }
    };
    let project = json!({
        "hooks": { "UserPromptSubmit": hook_entry(&rig.hook("PROJ_UPS"), 60) },
        "permissions": { "deny": ["Bash(rm:*)"], "allow": ["Bash(echo:*)"] },
    });
    std::fs::create_dir_all(rig.wd.join(".claude")).unwrap();
    std::fs::write(rig.wd.join(".claude/settings.json"), project.to_string()).unwrap();
    std::fs::write(rig.wd.join("keep-me"), "x").unwrap();
    let flag = settings(&rig, "s1-settings.json", json!({}));

    let c = rig.spawn_with(&["--settings", &flag], 40, 120, &[], true);
    c.ready();
    c.prompt(
        "Run these three bash commands, each as its own Bash tool call, one after the \
         other, even if one is denied: `echo s1-ok`, then `touch made-by-touch`, then \
         `rm keep-me`. Then reply DONE.",
    );
    c.wait_stop(1, TURN);
    let ev = c.events();

    for tag in ["PROJ_UPS", "UserPromptSubmit"] {
        assert_eq!(tagged(&ev, tag).len(), 1, "{tag} must fire once: {ev:#?}");
    }
    let post = bash_commands(&ev, "PostToolUse");
    assert!(
        post.iter().any(|c| c.contains("echo s1-ok")),
        "echo did not run: {post:?}"
    );
    assert!(rig.wd.join("keep-me").exists(), "project deny rule ignored");
    let pre = bash_commands(&ev, "PreToolUse");
    assert!(
        pre.iter().any(|c| c.contains("rm ")),
        "rm not tried: {pre:?}"
    );
    if rig.cfg.is_none() {
        let got = tagged(&ev, "UserPromptSubmit")[0].str("permission_mode");
        match user_mode {
            Some(mode) => assert_eq!(got, mode, "the user's defaultMode must still apply"),
            None => eprintln!("S1: no user defaultMode to check (permission_mode {got})"),
        }
        return;
    }

    assert_eq!(tagged(&ev, "USER_UPS").len(), 1, "user hook must fire once");
    assert!(
        pre.iter().any(|c| c.contains("touch")),
        "touch not tried: {pre:?}"
    );
    assert!(
        !rig.wd.join("made-by-touch").exists(),
        "user deny rule ignored"
    );
    assert!(
        tagged(&ev, "PermissionRequest").is_empty(),
        "allow/deny rules should answer without a prompt"
    );
}

/// The S2 `Stop` hook: blocks the first stop of a chain, allows the next.
const S2_STOP: &str = r#"import sys, json, os
d = sys.argv[1]
p = json.load(sys.stdin)
with open(os.path.join(d, "log.jsonl"), "a") as f:
    f.write(json.dumps({"tag": "Stop", "payload": p}) + "\n")
if not p.get("stop_hook_active"):
    print(json.dumps({"decision": "block", "reason": "CMUX DIGEST item-1: also reply with the single word ZEBRA now."}))
"#;

/// S2: `Stop` → `block` makes Claude act on the reason in the same turn, and
/// the next stop has `stop_hook_active`, reset by the next user prompt.
#[test]
#[ignore = "needs a real claude"]
fn s2_stop_block_is_acted_on() {
    let rig = Rig::new();
    let script = rig.hooks.join("s2_stop.py");
    std::fs::write(&script, S2_STOP).unwrap();
    let cmd = format!("python3 '{}' '{}'", script.display(), rig.hooks.display());
    let s = settings(
        &rig,
        "s2-settings.json",
        json!({ "hooks": { "Stop": hook_entry(&cmd, 60) } }),
    );
    let c = rig.spawn(&["--settings", &s], 40, 120);
    c.ready();

    c.prompt("Reply with the single word PONG.");
    let first = c.wait_stop(1, TURN);
    let second = c.wait_stop(2, TURN);
    assert_eq!(first.payload["stop_hook_active"], json!(false), "{first:?}");
    assert!(
        first.str("last_assistant_message").contains("PONG"),
        "{first:?}"
    );
    assert_eq!(
        second.payload["stop_hook_active"],
        json!(true),
        "{second:?}"
    );
    assert!(
        second.str("last_assistant_message").contains("ZEBRA"),
        "{second:?}"
    );
    let ups = c.events_tagged("UserPromptSubmit");
    assert_eq!(ups.len(), 1, "the block must not start a new prompt");
    let prompt_ids: Vec<&Value> = [&ups[0], &first, &second]
        .iter()
        .map(|e| &e.payload["prompt_id"])
        .collect();
    if prompt_ids.iter().all(|p| !p.is_null()) {
        assert!(
            prompt_ids.windows(2).all(|w| w[0] == w[1]),
            "{prompt_ids:?}"
        );
    }

    c.prompt("Reply with the single word KIWI.");
    let third = c.wait_stop(3, TURN);
    assert_eq!(third.payload["stop_hook_active"], json!(false), "{third:?}");
    assert_eq!(c.events_tagged("UserPromptSubmit").len(), 2);
}

/// S3: one bracketed paste plus `\r` in a single write submits one prompt. Up
/// to 3 lines it is byte-exact; 4 or more lines get wrapped in
/// `<pasted_content>` (the recorded "partial" result).
#[test]
#[ignore = "needs a real claude"]
fn s3_bracketed_paste_submits_one_prompt() {
    let rig = Rig::new();
    let s = settings(&rig, "s3-settings.json", json!({}));
    let c = rig.spawn(&["--settings", &s], 40, 120);
    c.ready();

    let short =
        "Reply with the single word OK.\n  `indented` ünïcödé → ✓\n/not-a-command at line start";
    c.write(format!("\x1b[200~{short}\x1b[201~\r").as_bytes());
    c.wait_stop(1, TURN);
    let ups = c.events_tagged("UserPromptSubmit");
    assert_eq!(ups.len(), 1, "{ups:#?}");
    assert_eq!(ups[0].str("prompt"), short, "3-line paste must be exact");

    let long = "Reply with the single word OK.\nline two\n  line three\nline four";
    c.write(format!("\x1b[200~{long}\x1b[201~\r").as_bytes());
    c.wait_stop(2, TURN);
    let ups = c.events_tagged("UserPromptSubmit");
    assert_eq!(ups.len(), 2, "{ups:#?}");
    let got = ups[1].str("prompt");
    assert!(got.contains(long), "4-line paste lost its text: {got:?}");
    assert!(
        got.contains("<pasted_content"),
        "4-line paste is no longer wrapped (S3 result changed): {got:?}"
    );
}

/// The S4 MCP server: a stdio JSON-RPC server that logs its startup
/// environment to argv[1] and has one tool, `get_env(name)`.
const ENVSRV: &str = r#"import sys, json, os
log = sys.argv[1]
with open(log, "a") as f:
    f.write(json.dumps({"argv": sys.argv, "CMUX_AGENT": os.environ.get("CMUX_AGENT"), "CMUX_TOKEN": os.environ.get("CMUX_TOKEN"), "cwd": os.getcwd()}) + "\n")
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    mid, meth = m.get("id"), m.get("method")
    if meth == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": m["params"].get("protocolVersion", "2024-11-05"), "capabilities": {"tools": {}}, "serverInfo": {"name": "envsrv", "version": "0"}}})
    elif meth == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [{"name": "get_env", "description": "Return the value of an environment variable of the server process", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}}]}})
    elif meth == "tools/call":
        n = m["params"]["arguments"].get("name", "")
        send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "VALUE=" + str(os.environ.get(n))}]}})
    elif meth == "ping":
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "no"}})
"#;

/// S4: the positional prompt of an interactive `claude` is turn 1, and `env`
/// in `--mcp-config` reaches the stdio server and beats the parent's env. Also
/// the gotcha: `--mcp-config <file> "<prompt>"` takes the prompt as a file.
#[test]
#[ignore = "needs a real claude"]
fn s4_kickoff_prompt_and_mcp_env() {
    let rig = Rig::new();
    let server = rig.hooks.join("envsrv.py");
    std::fs::write(&server, ENVSRV).unwrap();
    let srv_log = rig.hooks.join("envsrv.jsonl");
    let mcp = rig.write_json(
        "mcp.json",
        &json!({ "mcpServers": { "cmux": {
            "command": "python3",
            "args": [server, srv_log, "--agent-from-args", "A-7"],
            "env": { "CMUX_AGENT": "T-0001.a1", "CMUX_TOKEN": "tok-from-config-env" },
        }}}),
    );
    let s = settings(
        &rig,
        "s4-settings.json",
        json!({ "permissions": { "allow": ["mcp__cmux"] } }),
    );
    let mcp_flag = format!("--mcp-config={}", mcp.display());
    let kickoff = "Call the get_env tool of the cmux MCP server with name CMUX_TOKEN, \
                   then reply with exactly the text it returned.";

    let c = rig.spawn_with(
        &["--settings", &s, &mcp_flag, kickoff],
        40,
        120,
        &[("CMUX_AGENT", "from-parent-env")],
        false,
    );
    let stop = c.wait_stop(1, TURN);
    let ev = c.events();
    let ups = tagged(&ev, "UserPromptSubmit");
    assert_eq!(ups.len(), 1, "{ev:#?}");
    assert_eq!(
        ups[0].str("prompt"),
        kickoff,
        "kickoff must be turn 1, exact"
    );
    assert!(
        stop.str("last_assistant_message")
            .contains("VALUE=tok-from-config-env"),
        "{stop:?}"
    );
    assert!(
        !tagged(&ev, "PermissionRequest")
            .iter()
            .any(|e| e.str("tool_name").starts_with("mcp__cmux")),
        "the allow rule must pre-approve mcp__cmux"
    );
    let starts: Vec<Value> = std::fs::read_to_string(&srv_log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!starts.is_empty());
    let start = &starts[0];
    assert_eq!(
        start["CMUX_AGENT"], "T-0001.a1",
        "config env must beat parent env"
    );
    assert_eq!(start["CMUX_TOKEN"], "tok-from-config-env");
    assert!(start["argv"].to_string().contains("A-7"), "{start}");
    assert_eq!(start["cwd"], rig.wd.display().to_string());
    drop(c);

    // The gotcha: the variadic flag eats the prompt.
    let bad = rig.spawn(
        &["--mcp-config", &mcp.display().to_string(), "say hi"],
        40,
        120,
    );
    bad.wait_screen("the MCP config error", 60, |s| {
        s.contains("MCP config file not found") || s.contains("say hi")
    });
    let screen = bad.screen();
    assert!(
        screen.contains("not found"),
        "`--mcp-config <file> <prompt>` no longer fails (S4 gotcha changed?):\n{screen}"
    );
}

/// S5: `Notification.notification_type` tells a permission prompt from an idle
/// notice, and `UserPromptSubmit` `additionalContext` reaches the model.
#[test]
#[ignore = "needs a real claude"]
fn s5_notification_types_and_additional_context() {
    let rig = Rig::new();
    let s = settings(&rig, "s5-settings.json", json!({}));
    rig.set_resp(
        "UserPromptSubmit",
        Some(json!({ "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "The secret word is MANGO42.",
        }})),
    );
    let c = rig.spawn(&["--settings", &s], 40, 120);
    c.ready();

    c.prompt("Reply with only the secret word from your context, nothing else.");
    let stop = c.wait_stop(1, TURN);
    assert!(
        stop.str("last_assistant_message").contains("MANGO42"),
        "{stop:?}"
    );
    rig.set_resp("UserPromptSubmit", None);

    // Claude sends the idle notice 60 s after the turn ends.
    let ev = c.wait_events("an idle Notification", 150, |ev| {
        tagged(ev, "Notification")
            .iter()
            .any(|e| e.str("notification_type") == "idle_prompt")
    });
    let idle = tagged(&ev, "Notification");
    assert!(
        idle.iter()
            .all(|e| e.str("notification_type") == "idle_prompt"),
        "{idle:#?}"
    );

    c.prompt("Run the bash command `touch s5-file` now.");
    let ev = c.wait_events("a permission Notification", TURN, |ev| {
        tagged(ev, "Notification")
            .iter()
            .any(|e| e.str("notification_type") == "permission_prompt")
    });
    let perm = tagged(&ev, "Notification")
        .into_iter()
        .find(|e| e.str("notification_type") == "permission_prompt")
        .unwrap();
    assert!(perm.str("message").contains("permission"), "{perm:?}");
    c.write(b"\x1b");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!rig.wd.join("s5-file").exists());
}

fn permission_decision(behavior: &str, message: Option<&str>) -> Value {
    let mut decision = json!({ "behavior": behavior });
    if let Some(m) = message {
        decision["message"] = json!(m);
    }
    json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } })
}

/// S6: a `PermissionRequest` hook's allow and deny answer the prompt, and a
/// hook held for 90 s with `"timeout": 600` still decides it.
#[test]
#[ignore = "needs a real claude"]
fn s6_permission_request_hook_decides() {
    let rig = Rig::new();
    let s = settings(&rig, "s6-settings.json", json!({}));
    let c = rig.spawn(&["--settings", &s], 40, 120);
    c.ready();

    rig.set_resp(
        "PermissionRequest",
        Some(permission_decision("allow", None)),
    );
    c.prompt("Run the bash command `touch s6-allowed` now, then reply DONE.");
    c.wait_stop(1, TURN);
    let req = c.events_tagged("PermissionRequest");
    assert_eq!(req.len(), 1, "{req:#?}");
    assert_eq!(req[0].str("tool_name"), "Bash");
    assert!(
        req[0].payload["tool_input"]["command"]
            .as_str()
            .unwrap()
            .contains("s6-allowed")
    );
    assert!(
        rig.wd.join("s6-allowed").exists(),
        "allow did not run the command"
    );

    let denial = "Denied by the S6 test: QUOKKA";
    rig.set_resp(
        "PermissionRequest",
        Some(permission_decision("deny", Some(denial))),
    );
    c.prompt("Run the bash command `touch s6-denied` now, then reply DONE.");
    c.wait_stop(2, TURN);
    assert!(c.events_tagged("PermissionRequest").len() >= 2);
    assert!(
        !rig.wd.join("s6-denied").exists(),
        "deny still ran the command"
    );
    assert!(c.screen().contains("QUOKKA") || transcript_mentions(&rig, "QUOKKA"));

    rig.set_resp(
        "PermissionRequest",
        Some(permission_decision("allow", None)),
    );
    rig.set_sleep("PermissionRequest", Some(90));
    let asked = Instant::now();
    c.prompt("Run the bash command `touch s6-slow` now, then reply DONE.");
    c.wait_stop(3, TURN + 90);
    let took = asked.elapsed();
    assert!(
        rig.wd.join("s6-slow").exists(),
        "the held allow was dropped"
    );
    assert!(
        took >= Duration::from_secs(90),
        "the hook was not held: {took:?}"
    );
}

/// Whether any transcript of the rig's working dir contains `needle`.
fn transcript_mentions(rig: &Rig, needle: &str) -> bool {
    fn walk(dir: &std::path::Path, needle: &str) -> bool {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, needle)
                } else {
                    p.extension().is_some_and(|x| x == "jsonl")
                        && std::fs::read_to_string(&p).is_ok_and(|t| t.contains(needle))
                }
            })
    }
    walk(&rig.projects, needle)
}

/// S7: a `state_formatted()` snapshot of a mid-turn Claude screen, prefixed
/// with `ESC[?1049h` when on the alternate screen, redraws identically in a
/// second vt100 parser, on several captured screens.
///
/// The xterm.js half of the spike (`spikes/s7-vt100/xterm/compare.mjs`, with
/// `@xterm/headless`) needs node and npm and stays out of this suite.
#[test]
#[ignore = "needs a real claude"]
fn s7_vt100_snapshot_redraws_faithfully() {
    const ROWS: u16 = 30;
    const COLS: u16 = 100;
    let rig = Rig::new();
    let s = settings(
        &rig,
        "s7-settings.json",
        json!({ "permissions": { "allow": ["Bash(sleep:*)"] } }),
    );
    let c = rig.spawn(&["--settings", &s], ROWS, COLS);
    let mut marks: Vec<(&str, usize)> = Vec::new();
    let mark = |marks: &mut Vec<(&'static str, usize)>, c: &Claude, label: &'static str| {
        marks.push((label, c.raw().len()));
    };
    let pause = |secs: f64| std::thread::sleep(Duration::from_secs_f64(secs));

    c.ready();
    mark(&mut marks, &c, "idle-startup");
    c.write(b"Count from 1 to 120, one number per line, then run bash 'sleep 12' and reply DONE");
    pause(1.0);
    mark(&mut marks, &c, "typed-prompt");
    c.write(b"\r");
    c.wait_events("UserPromptSubmit", 60, |ev| {
        !tagged(ev, "UserPromptSubmit").is_empty()
    });
    pause(2.5);
    mark(&mut marks, &c, "turn-1");
    pause(3.0);
    mark(&mut marks, &c, "turn-2");
    c.wait_events("the sleep tool call", TURN, |ev| {
        bash_commands(ev, "PreToolUse")
            .iter()
            .any(|c| c.contains("sleep"))
    });
    pause(2.0);
    mark(&mut marks, &c, "tool-running");
    pause(5.0);
    mark(&mut marks, &c, "tool-running-2");
    c.wait_stop(1, TURN);
    pause(1.5);
    mark(&mut marks, &c, "done");

    let bytes = c.raw();
    drop(c);
    let mut failures = Vec::new();
    let mut screens = std::collections::BTreeSet::new();
    let mut on_alt = 0;
    for (label, off) in &marks {
        let mut a = vt100::Parser::new(ROWS, COLS, 0);
        a.process(&bytes[..*off]);
        let sa = a.screen();
        screens.insert(sa.contents());
        let mut snap = sa.state_formatted();
        if sa.alternate_screen() {
            on_alt += 1;
            snap.splice(0..0, b"\x1b[?1049h".iter().copied());
        }
        let mut b = vt100::Parser::new(ROWS, COLS, 0);
        b.process(&snap);
        let diffs = screen_diffs(sa, b.screen());
        eprintln!(
            "S7 {label:16} offset={off:7} snapshot={:6}B alt={} diffs={}",
            snap.len(),
            sa.alternate_screen(),
            diffs.len()
        );
        if !diffs.is_empty() {
            failures.push(format!("{label}: {}", diffs.join("; ")));
        }
    }
    eprintln!(
        "S7 {on_alt} of {} screens on the alternate screen",
        marks.len()
    );
    assert!(
        screens.len() >= 5,
        "only {} distinct screens captured",
        screens.len()
    );
    assert!(
        failures.is_empty(),
        "snapshot differs:\n{}",
        failures.join("\n")
    );
}

/// Every difference between two screens: cells (text, colours, attributes,
/// width) and terminal modes. At most a few cells are listed.
fn screen_diffs(a: &vt100::Screen, b: &vt100::Screen) -> Vec<String> {
    let mut out = Vec::new();
    let (rows, cols) = a.size();
    let mut cells = 0;
    for r in 0..rows {
        for c in 0..cols {
            let (ca, cb) = (a.cell(r, c).unwrap(), b.cell(r, c).unwrap());
            let same = ca.contents() == cb.contents()
                && ca.fgcolor() == cb.fgcolor()
                && ca.bgcolor() == cb.bgcolor()
                && ca.bold() == cb.bold()
                && ca.dim() == cb.dim()
                && ca.italic() == cb.italic()
                && ca.underline() == cb.underline()
                && ca.inverse() == cb.inverse()
                && ca.is_wide() == cb.is_wide();
            if !same {
                cells += 1;
                if cells <= 3 {
                    out.push(format!(
                        "cell {r},{c}: {:?} vs {:?}",
                        ca.contents(),
                        cb.contents()
                    ));
                }
            }
        }
    }
    if cells > 3 {
        out.push(format!("{cells} cells in all"));
    }
    let modes = [
        (
            "cursor",
            format!("{:?}", a.cursor_position()),
            format!("{:?}", b.cursor_position()),
        ),
        (
            "hide_cursor",
            a.hide_cursor().to_string(),
            b.hide_cursor().to_string(),
        ),
        (
            "alternate_screen",
            a.alternate_screen().to_string(),
            b.alternate_screen().to_string(),
        ),
        (
            "application_cursor",
            a.application_cursor().to_string(),
            b.application_cursor().to_string(),
        ),
        (
            "application_keypad",
            a.application_keypad().to_string(),
            b.application_keypad().to_string(),
        ),
        (
            "bracketed_paste",
            a.bracketed_paste().to_string(),
            b.bracketed_paste().to_string(),
        ),
        (
            "mouse_mode",
            format!("{:?}", a.mouse_protocol_mode()),
            format!("{:?}", b.mouse_protocol_mode()),
        ),
        (
            "mouse_encoding",
            format!("{:?}", a.mouse_protocol_encoding()),
            format!("{:?}", b.mouse_protocol_encoding()),
        ),
    ];
    for (name, va, vb) in modes {
        if va != vb {
            out.push(format!("{name}: {va} vs {vb}"));
        }
    }
    if a.contents_formatted() != b.contents_formatted() {
        out.push("contents_formatted differs".into());
    }
    out
}
