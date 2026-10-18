use cmux_core::ids::{AgentPath, PlanId, TaskId};

use super::*;

const TOKEN: &str = "tok123";
const SESSION: &str = "11111111-2222-4333-8444-555555555555";

fn plan() -> PlanId {
    PlanId::new(2026, 10, 18, "Auth refactor")
}

fn cfg() -> SpawnConfig {
    let mut c = SpawnConfig::new(
        "/usr/local/bin/cmuxd".into(),
        "/run/user/1000/cmuxd/api.sock".into(),
        "/data/cmuxd".into(),
    );
    c.permission_wait = Duration::from_secs(600);
    c
}

fn id(path: AgentPath) -> AgentId {
    AgentId::new(plan(), path)
}

fn spec(path: AgentPath, level: Level, cwd: &str, resume: bool, cfg: &SpawnConfig) -> SpawnSpec {
    let agent = id(path);
    let req = SpawnRequest {
        agent: &agent,
        level,
        cwd: Path::new(cwd),
        session_id: resume.then_some(SESSION),
        resume,
        system_prompt: "You are an agent.\nBe brief.",
        kickoff: "[cmux] Start.",
    };
    plan_spawn(cfg, &req, SESSION, TOKEN)
}

fn file<'a>(s: &'a SpawnSpec, name: &str) -> &'a SpawnFile {
    s.files
        .iter()
        .find(|f| f.path.file_name().unwrap() == name)
        .unwrap()
}

const SETTINGS: &str = r#"{
  "hooks": {
    "Notification": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook Notification",
            "timeout": 5,
            "type": "command"
          }
        ]
      }
    ],
    "PermissionRequest": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook PermissionRequest",
            "timeout": 600,
            "type": "command"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook PreToolUse",
            "timeout": 5,
            "type": "command"
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook SessionEnd",
            "timeout": 5,
            "type": "command"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook SessionStart",
            "timeout": 5,
            "type": "command"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook Stop",
            "timeout": 5,
            "type": "command"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "command": "/usr/local/bin/cmuxd hook UserPromptSubmit",
            "timeout": 5,
            "type": "command"
          }
        ]
      }
    ]
  },
  "permissions": {
    "allow": [
      "mcp__cmux"
    ]
  }
}
"#;

fn mcp_for(agent: &str) -> String {
    format!(
        r#"{{
  "mcpServers": {{
    "cmux": {{
      "args": [
        "mcp"
      ],
      "command": "/usr/local/bin/cmuxd",
      "env": {{
        "CMUX_AGENT": "{agent}",
        "CMUX_SOCK": "/run/user/1000/cmuxd/api.sock",
        "CMUX_TOKEN": "tok123"
      }}
    }}
  }}
}}
"#
    )
}

#[test]
fn executor_snapshot() {
    let s = spec(
        AgentPath::Executor,
        Level::Executor,
        "/repos/app",
        false,
        &cfg(),
    );
    let dir = "/data/cmuxd/plans/P-20261018-auth-refactor/agents/executor";
    assert_eq!(
        s.argv,
        [
            "claude",
            "--session-id",
            SESSION,
            "--settings",
            &format!("{dir}/settings.json"),
            &format!("--mcp-config={dir}/mcp.json"),
            "--append-system-prompt",
            "You are an agent.\nBe brief.",
            "[cmux] Start.",
        ]
    );
    assert_eq!(s.cwd, Path::new("/repos/app"));
    assert_eq!(file(&s, "settings.json").contents, SETTINGS);
    assert_eq!(
        file(&s, "mcp.json").contents,
        mcp_for("P-20261018-auth-refactor/executor")
    );
    assert_eq!(
        file(&s, "system.md").contents,
        "You are an agent.\nBe brief."
    );
    assert_eq!(
        s.env,
        [
            ("CMUX_SOCK".into(), "/run/user/1000/cmuxd/api.sock".into()),
            (
                "CMUX_AGENT".into(),
                "P-20261018-auth-refactor/executor".into()
            ),
            ("CMUX_TOKEN".into(), TOKEN.into()),
            ("TERM".into(), "xterm-256color".into()),
            ("CMUX_PERMISSION_WAIT_SECS".into(), "600".into()),
        ]
    );
    assert_eq!(s.session_id, SESSION);
}

#[test]
fn task_snapshot_with_level_options() {
    let mut c = cfg();
    c.task = LevelOpts {
        permission_mode: Some("acceptEdits".into()),
        model: Some("sonnet".into()),
    };
    let t = TaskId::new(3);
    let s = spec(
        AgentPath::Task(t),
        Level::Task,
        "/repos/app/.worktrees/T-0003",
        false,
        &c,
    );
    let dir = "/data/cmuxd/plans/P-20261018-auth-refactor/agents/T-0003";
    assert_eq!(
        s.argv,
        [
            "claude",
            "--session-id",
            SESSION,
            "--settings",
            &format!("{dir}/settings.json"),
            &format!("--mcp-config={dir}/mcp.json"),
            "--append-system-prompt",
            "You are an agent.\nBe brief.",
            "--permission-mode",
            "acceptEdits",
            "--model",
            "sonnet",
            "[cmux] Start.",
        ]
    );
    assert_eq!(s.cwd, Path::new("/repos/app/.worktrees/T-0003"));
    assert_eq!(file(&s, "settings.json").contents, SETTINGS);
    assert_eq!(
        file(&s, "mcp.json").contents,
        mcp_for("P-20261018-auth-refactor/T-0003")
    );
    // Other levels are unaffected by the task options.
    let e = spec(AgentPath::Executor, Level::Executor, "/r", false, &c);
    assert!(!e.argv.contains(&"--model".to_string()));
}

#[test]
fn sub_agent_snapshot_and_directory() {
    let t = TaskId::new(3);
    let s = spec(
        AgentPath::sub(t, "tests").unwrap(),
        Level::Sub,
        "/repos/app/.worktrees/T-0003",
        false,
        &cfg(),
    );
    let dir = "/data/cmuxd/plans/P-20261018-auth-refactor/agents/T-0003_tests";
    assert_eq!(s.argv[4], format!("{dir}/settings.json"));
    assert_eq!(s.argv[5], format!("--mcp-config={dir}/mcp.json"));
    assert_eq!(
        file(&s, "mcp.json").contents,
        mcp_for("P-20261018-auth-refactor/T-0003/tests")
    );
    assert!(s.files.iter().all(|f| f.path.starts_with(dir)));
}

#[test]
fn resume_uses_resume_with_the_given_session() {
    let s = spec(AgentPath::Executor, Level::Executor, "/r", true, &cfg());
    assert_eq!(&s.argv[1..3], ["--resume", SESSION]);
    assert!(!s.argv.contains(&"--session-id".to_string()));
}

#[test]
fn the_prompt_comes_last_and_mcp_config_uses_the_equals_form() {
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &cfg());
    assert_eq!(s.argv.last().unwrap(), "[cmux] Start.");
    assert!(!s.argv.contains(&"--mcp-config".to_string()));
    assert!(s.argv.iter().any(|a| a.starts_with("--mcp-config=")));
}

#[test]
fn only_the_mcp_file_is_secret_and_settings_hold_no_token() {
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &cfg());
    for f in &s.files {
        assert_eq!(f.secret, f.path.ends_with("mcp.json"), "{:?}", f.path);
        assert_eq!(f.contents.contains(TOKEN), f.secret, "{:?}", f.path);
    }
    assert_eq!(s.token_hash, token_hash(TOKEN));
    assert_ne!(s.token_hash, [0; 32]);
}

#[test]
fn token_hash_is_sha256_of_the_text() {
    // echo -n abc | sha256sum
    let h = token_hash("abc");
    let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn hook_command_quotes_a_path_with_spaces_and_quotes() {
    let mut c = cfg();
    c.cmuxd_exe = "/opt/my apps/it's/cmuxd".into();
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &c);
    let v: Value = serde_json::from_str(&file(&s, "settings.json").contents).unwrap();
    assert_eq!(
        v["hooks"]["Stop"][0]["hooks"][0]["command"],
        r"'/opt/my apps/it'\''s/cmuxd' hook Stop"
    );
    let m: Value = serde_json::from_str(&file(&s, "mcp.json").contents).unwrap();
    assert_eq!(
        m["mcpServers"]["cmux"]["command"],
        "/opt/my apps/it's/cmuxd"
    );
}

#[test]
fn permission_request_timeout_follows_permission_wait() {
    let mut c = cfg();
    c.permission_wait = Duration::from_secs(90);
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &c);
    let v: Value = serde_json::from_str(&file(&s, "settings.json").contents).unwrap();
    assert_eq!(
        v["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
        90
    );
    assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
}

#[test]
fn env_scrub_removes_session_vars_and_remote_prefix_and_keeps_the_rest() {
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &cfg());
    let base = [
        ("PATH", "/bin"),
        ("CLAUDECODE", "1"),
        ("CLAUDE_CODE_SESSION_ID", "outer"),
        ("CLAUDE_CODE_REMOTE_SESSION_ID", "outer"),
        ("CLAUDE_CODE_REMOTE", "true"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "keep"),
        ("AI_AGENT", "x"),
        ("TERM", "dumb"),
        ("CMUX_AGENT", "stale"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()));
    let out = s.final_env(base);
    let keys: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "PATH",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CMUX_SOCK",
            "CMUX_AGENT",
            "CMUX_TOKEN",
            "TERM",
            "CMUX_PERMISSION_WAIT_SECS"
        ]
    );
    let get = |k: &str| out.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("TERM"), Some("xterm-256color"));
    assert_eq!(get("CMUX_AGENT"), Some("P-20261018-auth-refactor/executor"));
}

#[test]
fn build_spawn_generates_a_session_and_a_fresh_token_each_time() {
    let agent = id(AgentPath::Executor);
    let req = SpawnRequest {
        agent: &agent,
        level: Level::Executor,
        cwd: Path::new("/r"),
        session_id: None,
        resume: false,
        system_prompt: "p",
        kickoff: "k",
    };
    let a = build_spawn(&cfg(), &req);
    let b = build_spawn(&cfg(), &req);
    assert_ne!(a.session_id, b.session_id);
    assert_eq!(
        uuid::Uuid::parse_str(&a.session_id)
            .unwrap()
            .get_version_num(),
        4
    );
    assert_ne!(a.token_hash, b.token_hash);
    let token = &a.env.iter().find(|(k, _)| k == "CMUX_TOKEN").unwrap().1;
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(a.token_hash, token_hash(token));
    assert_eq!(a.argv[2], a.session_id);

    // A resume keeps the session id but still gets a new token.
    let resume = SpawnRequest {
        session_id: Some(SESSION),
        resume: true,
        ..req
    };
    let r = build_spawn(&cfg(), &resume);
    assert_eq!(r.session_id, SESSION);
    assert_ne!(r.token_hash, a.token_hash);
}

#[test]
fn from_effect_reads_spawn_agent_only() {
    let agent = id(AgentPath::Task(TaskId::new(1)));
    let effect = Effect::SpawnAgent {
        agent: agent.clone(),
        term: cmux_core::ids::TermId(3),
        level: Level::Task,
        cwd: "/wt".into(),
        session_id: Some("sid".into()),
        resume: true,
        system_prompt: "sys".into(),
        kickoff: "go".into(),
    };
    let req = SpawnRequest::from_effect(&effect).unwrap();
    assert_eq!(req.agent, &agent);
    assert_eq!(req.level, Level::Task);
    assert_eq!(req.cwd, Path::new("/wt"));
    assert_eq!(req.session_id, Some("sid"));
    assert!(req.resume);
    assert_eq!((req.system_prompt, req.kickoff), ("sys", "go"));
    assert!(SpawnRequest::from_effect(&Effect::KillAgent { agent, term: None }).is_none());
}

#[test]
fn write_files_creates_directories_and_sets_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = cfg();
    c.data_dir = tmp.path().to_path_buf();
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &c);
    write_files(&s.files).unwrap();
    // Again: overwriting works and leaves no temp file.
    write_files(&s.files).unwrap();
    for f in &s.files {
        assert_eq!(std::fs::read_to_string(&f.path).unwrap(), f.contents);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f.path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, if f.secret { 0o600 } else { 0o644 }, "{:?}", f.path);
        }
    }
    let dir = file(&s, "mcp.json").path.parent().unwrap().to_path_buf();
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 3);
}

#[test]
fn write_files_tightens_a_wide_existing_secret_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mcp.json");
    let tmp_path = tmp.path().join("mcp.json.tmp");
    std::fs::write(&tmp_path, "old").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o666)).unwrap();
    }
    let f = SpawnFile {
        path: path.clone(),
        contents: "secret".into(),
        secret: true,
    };
    write_files(&[f]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret");
}

#[test]
fn the_hook_process_learns_the_permission_wait_from_its_environment() {
    let mut c = cfg();
    c.permission_wait = Duration::from_secs(90);
    let s = spec(AgentPath::Executor, Level::Executor, "/r", false, &c);
    assert!(
        s.env
            .iter()
            .any(|(k, v)| k == "CMUX_PERMISSION_WAIT_SECS" && v == "90")
    );
}
