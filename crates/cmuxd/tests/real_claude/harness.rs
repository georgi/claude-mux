//! The rig for `real_claude.rs`: a temp dir with a trusted working dir and a
//! hook logger, and a `claude` on a PTY (`cmuxd::term::pty`,
//! which applies claude-mux's environment scrub) with a vt100 screen.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cmuxd::term::pty::{Pty, PtySpec};
use serde_json::{Value, json};

/// Variables that can authenticate a `claude` whose config dir has no login.
/// With one of them set the rig uses an isolated config dir.
const AUTH_ENV: &[&str] = &["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"];

const AUTH_HELP: &str = "the real-claude tests need a logged-in `claude` (default config dir), \
     or CLAUDE_CODE_OAUTH_TOKEN (make one with `claude setup-token`) or ANTHROPIC_API_KEY";

/// The model for every session: cheap and fast.
pub const MODEL: &str = "haiku";

/// Logs every hook call to `<hooks>/log.jsonl` as `{"tag", "payload"}`. When
/// `sleep-<tag>` exists it sleeps that many seconds; when `resp-<tag>.json`
/// exists it prints it as the hook's answer.
const LOG_HOOK: &str = r#"import sys, json, os, time
d, tag = sys.argv[1], sys.argv[2]
body = sys.stdin.read()
try:
    payload = json.loads(body)
except Exception:
    payload = body
with open(os.path.join(d, "log.jsonl"), "a") as f:
    f.write(json.dumps({"tag": tag, "ts": time.time(), "payload": payload}) + "\n")
s = os.path.join(d, "sleep-" + tag)
if os.path.exists(s):
    time.sleep(float(open(s).read()))
r = os.path.join(d, "resp-" + tag + ".json")
if os.path.exists(r):
    sys.stdout.write(open(r).read())
"#;

/// The events the base settings log, each under its own name as the tag.
pub const BASE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "Notification",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
];

/// One logged hook call.
#[derive(Debug, Clone)]
pub struct Event {
    pub tag: String,
    pub payload: Value,
}

impl Event {
    pub fn str(&self, key: &str) -> &str {
        self.payload[key].as_str().unwrap_or("")
    }
}

/// Where `claude` keeps its state for a rig.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `CLAUDE_CONFIG_DIR=<root>/cfg`, credentials from `AUTH_ENV`.
    Isolated,
    /// The user's own config dir and login, as cmuxd's agents run.
    Default,
}

/// A temp dir: `wd/` (trusted working dir), `hooks/` (scripts, log, canned
/// answers) and, in isolated mode, `cfg/` (`CLAUDE_CONFIG_DIR`).
pub struct Rig {
    _tmp: tempfile::TempDir,
    pub mode: Mode,
    pub root: PathBuf,
    pub wd: PathBuf,
    pub hooks: PathBuf,
    /// The config dir in isolated mode.
    pub cfg: Option<PathBuf>,
    /// `<config dir>/projects/<wd with each non-alphanumeric as '-'>`: the
    /// transcripts. Removed on drop in default mode.
    pub projects: PathBuf,
}

impl Rig {
    pub fn new() -> Rig {
        let mode = if AUTH_ENV
            .iter()
            .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
        {
            Mode::Isolated
        } else {
            Mode::Default
        };
        let tmp = tempfile::Builder::new()
            .prefix("cmux-real-claude-")
            .tempdir()
            .unwrap();
        // Claude keys projects and hook `cwd` by the real path (/var -> /private/var).
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (wd, hooks) = (root.join("wd"), root.join("hooks"));
        for d in [&wd, &hooks] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(hooks.join("log_hook.py"), LOG_HOOK).unwrap();
        let mangled: String = wd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();

        let (cfg, config_dir, claude_json) = match mode {
            Mode::Isolated => {
                let cfg = root.join("cfg");
                std::fs::create_dir_all(&cfg).unwrap();
                let mut config = json!({ "hasCompletedOnboarding": true, "theme": "dark" });
                if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                    // Otherwise an interactive claude asks whether to use the key.
                    let tail = &key[key.len().saturating_sub(20)..];
                    config["customApiKeyResponses"] = json!({ "approved": [tail], "rejected": [] });
                }
                let claude_json = cfg.join(".claude.json");
                std::fs::write(&claude_json, config.to_string()).unwrap();
                (Some(cfg.clone()), cfg, claude_json)
            }
            Mode::Default => {
                let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
                let env_dir = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty());
                let claude_json = cmuxd::claude::trust::claude_json_path(env_dir.as_ref(), &home);
                let config_dir = env_dir.map(PathBuf::from).unwrap_or(home.join(".claude"));
                (None, config_dir, claude_json)
            }
        };
        // Takes Claude's config lock, so it is safe next to a running claude.
        cmuxd::claude::trust::trust_dir(&claude_json, &wd).unwrap();
        Rig {
            _tmp: tmp,
            mode,
            root,
            wd,
            hooks,
            cfg,
            projects: config_dir.join("projects").join(mangled),
        }
    }

    /// The shell command for a logging hook with `tag`.
    pub fn hook(&self, tag: &str) -> String {
        format!(
            "python3 '{}' '{}' {tag}",
            self.hooks.join("log_hook.py").display(),
            self.hooks.display()
        )
    }

    /// `{"<event>": [{"hooks": [<command>]}], ...}` with a logging hook (tag =
    /// event name, timeout 600 s) for each of `BASE_EVENTS`.
    pub fn base_hooks(&self) -> serde_json::Map<String, Value> {
        BASE_EVENTS
            .iter()
            .map(|e| (e.to_string(), hook_entry(&self.hook(e), 600)))
            .collect()
    }

    /// Writes `value` to `<root>/<name>` and returns the path.
    pub fn write_json(&self, name: &str, value: &Value) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, serde_json::to_string_pretty(value).unwrap()).unwrap();
        p
    }

    /// Sets (or with `None` removes) the canned answer of hooks tagged `tag`.
    pub fn set_resp(&self, tag: &str, resp: Option<Value>) {
        let p = self.hooks.join(format!("resp-{tag}.json"));
        match resp {
            Some(v) => std::fs::write(p, v.to_string()).unwrap(),
            None => {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// Sets (or with `None` removes) the delay of hooks tagged `tag`.
    pub fn set_sleep(&self, tag: &str, secs: Option<u64>) {
        let p = self.hooks.join(format!("sleep-{tag}"));
        match secs {
            Some(s) => std::fs::write(p, s.to_string()).unwrap(),
            None => {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    /// Starts `claude --model haiku --session-id <new uuid> <args...>` in `wd`.
    /// It adds `--permission-mode manual` (otherwise the account's default
    /// mode, maybe `auto`, applies) and, in default mode,
    /// `--setting-sources=project,local`, so the user's own hooks stay out.
    pub fn spawn(&self, args: &[&str], rows: u16, cols: u16) -> Claude {
        self.spawn_with(args, rows, cols, &[], false)
    }

    /// `spawn` with extra environment; `user_settings` keeps the user's own
    /// settings in default mode too.
    pub fn spawn_with(
        &self,
        args: &[&str],
        rows: u16,
        cols: u16,
        env: &[(&str, &str)],
        user_settings: bool,
    ) -> Claude {
        let session = uuid::Uuid::new_v4().to_string();
        let mut argv = vec!["--model", MODEL, "--session-id", &session];
        if !user_settings {
            argv.extend(["--permission-mode", "manual"]);
            if self.mode == Mode::Default {
                argv.push("--setting-sources=project,local");
            }
        }
        argv.extend_from_slice(args);
        let mut spec = PtySpec::new("claude", &argv);
        spec.cwd = Some(self.wd.clone());
        spec.rows = rows;
        spec.cols = cols;
        if let Some(cfg) = &self.cfg {
            spec.env
                .push(("CLAUDE_CONFIG_DIR".into(), cfg.display().to_string()));
        }
        spec.env
            .extend(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        // Not on cmuxd's scrub list (yet): the outer session's bridge id, and our
        // own agent identity when this suite runs inside a cmuxd agent.
        spec.env_remove = std::env::vars()
            .map(|(k, _)| k)
            .filter(|k| {
                (k.starts_with("CMUX_") || k == "CLAUDE_CODE_BRIDGE_SESSION_ID")
                    && !env.iter().any(|(e, _)| e == k)
            })
            .collect();

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let raw = Arc::new(Mutex::new(Vec::new()));
        let exited = Arc::new(AtomicBool::new(false));
        let (p, r, x) = (parser.clone(), raw.clone(), exited.clone());
        let pty = Pty::spawn(
            &spec,
            move |bytes| {
                r.lock().unwrap().extend_from_slice(bytes);
                p.lock().unwrap().process(bytes);
            },
            move |_| x.store(true, Ordering::SeqCst),
        )
        .expect("claude must be on PATH");
        Claude {
            pty,
            parser,
            raw,
            exited,
            log: self.hooks.join("log.jsonl"),
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The trust entry in the user's `.claude.json` stays: trust.rs has no
        // removal, and an entry for a deleted temp dir is harmless.
        // Claude's own children (hooks, MCP servers) may still write there for
        // a moment after it exits.
        if self.mode == Mode::Default {
            let mut result = Ok(());
            for _ in 0..5 {
                result = std::fs::remove_dir_all(&self.projects);
                std::thread::sleep(Duration::from_millis(300));
                if !self.projects.exists() {
                    return;
                }
            }
            eprintln!("cannot remove {}: {result:?}", self.projects.display());
        }
    }
}

/// One hook command in settings form.
pub fn hook_entry(command: &str, timeout: u64) -> Value {
    json!([{ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] }])
}

/// Reads the hook log of a rig.
pub fn read_events(log: &Path) -> Vec<Event> {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| Event {
            tag: v["tag"].as_str().unwrap_or("").to_string(),
            payload: v["payload"].clone(),
        })
        .collect()
}

/// A running `claude`. Killed on drop.
pub struct Claude {
    pty: Pty,
    parser: Arc<Mutex<vt100::Parser>>,
    raw: Arc<Mutex<Vec<u8>>>,
    exited: Arc<AtomicBool>,
    log: PathBuf,
}

impl Claude {
    pub fn write(&self, bytes: &[u8]) {
        assert!(self.pty.write(bytes), "claude stopped reading input");
    }

    /// Pastes `text` (bracketed) and presses Enter as a separate write.
    pub fn prompt(&self, text: &str) {
        self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        std::thread::sleep(Duration::from_millis(700));
        self.write(b"\r");
    }

    pub fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    /// All bytes the child wrote so far.
    pub fn raw(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }

    pub fn events(&self) -> Vec<Event> {
        read_events(&self.log)
    }

    pub fn events_tagged(&self, tag: &str) -> Vec<Event> {
        self.events().into_iter().filter(|e| e.tag == tag).collect()
    }

    /// Polls until `pred` holds on the hook log; panics with the screen after
    /// `secs`.
    pub fn wait_events(
        &self,
        what: &str,
        secs: u64,
        pred: impl Fn(&[Event]) -> bool,
    ) -> Vec<Event> {
        self.wait(what, secs, || {
            let ev = self.events();
            pred(&ev).then_some(ev)
        })
    }

    /// Polls until `pred` holds on the screen text.
    pub fn wait_screen(&self, what: &str, secs: u64, pred: impl Fn(&str) -> bool) -> String {
        self.wait(what, secs, || {
            let s = self.screen();
            pred(&s).then_some(s)
        })
    }

    /// Waits until the `n`-th `Stop` (1-based) is logged and returns it.
    pub fn wait_stop(&self, n: usize, secs: u64) -> Event {
        let what = format!("Stop #{n}");
        self.wait_events(&what, secs, |ev| {
            ev.iter().filter(|e| e.tag == "Stop").count() >= n
        })
        .into_iter()
        .filter(|e| e.tag == "Stop")
        .nth(n - 1)
        .unwrap()
    }

    /// Waits for `SessionStart`, then lets the input box settle.
    pub fn ready(&self) {
        self.wait_events("SessionStart", 90, |ev| {
            ev.iter().any(|e| e.tag == "SessionStart")
        });
        std::thread::sleep(Duration::from_secs(3));
    }

    fn wait<T>(&self, what: &str, secs: u64, mut probe: impl FnMut() -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(v) = probe() {
                return v;
            }
            let screen = self.screen();
            if screen.contains("Not logged in") || screen.contains("Invalid API key") {
                panic!("claude is not authenticated: {AUTH_HELP}\n--- screen ---\n{screen}");
            }
            if Instant::now() > end || self.exited.load(Ordering::SeqCst) {
                let exited = self.exited.load(Ordering::SeqCst);
                panic!(
                    "timed out after {secs}s waiting for {what} (claude exited: {exited})\n\
                     --- screen ---\n{screen}\n--- hooks ---\n{:#?}",
                    self.events()
                        .iter()
                        .map(|e| format!("{} {}", e.tag, e.payload))
                        .collect::<Vec<_>>()
                );
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

impl Drop for Claude {
    fn drop(&mut self) {
        self.pty.kill();
        // Let it go before the rig removes its transcript dir.
        let end = Instant::now() + Duration::from_secs(10);
        while !self.exited.load(Ordering::SeqCst) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
