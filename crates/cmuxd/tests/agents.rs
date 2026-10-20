//! Agents on the server: the endpoints, `cmuxd hook`, `cmuxd mcp` and the
//! effect runner, up to end-to-end sessions with a fake `claude`
//! (`examples/fake-claude.rs`).

use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cmux_core::engine::{Command, Effect, RunnerEvent};
use cmux_core::ids::{AgentId, TermId};
use cmux_core::model::State;
use cmux_proto::api::{AnswerAction, AnswerReq, Created, route};
use cmux_proto::view::{
    Activity, Delivery, DeliveryPath, ItemKind, ItemView, Life, Recipient, StateView, TaskState,
};
use cmuxd::actor::{ActorHandle, EffectRunner};
use cmuxd::agent::AgentRuntime;
use cmuxd::api::Extra;
use cmuxd::claude::spawn::{SpawnConfig, agent_dir, token_hash};
use cmuxd::client::Endpoint;
use cmuxd::config::Config;
use cmuxd::server::{Server, ServerBuilder};
use serde_json::{Value, json};

#[path = "support/mock_http.rs"]
mod mock_http;

const CMUXD: &str = env!("CARGO_BIN_EXE_cmuxd");

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = Proc::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A clone of a local bare origin with one commit on `main`.
fn make_repo(root: &Path) -> PathBuf {
    let origin = root.join("origin.git");
    let repo = root.join("repo");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--bare", "--initial-branch=main"]);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "first"]);
    git(&repo, &["push", "-u", "origin", "main"]);
    repo
}

fn fake_claude_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let p = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("examples")
        .join("fake-claude");
    if !p.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = Proc::new(cargo)
            .args(["build", "-p", "cmuxd", "--example", "fake-claude"])
            .status()
            .unwrap()
            .success();
        assert!(
            ok && p.exists(),
            "cannot build the fake claude at {}",
            p.display()
        );
    }
    p
}

async fn eventually<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(Instant::now() < until, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

struct Env {
    server: Server,
    unix: Endpoint,
    rt: Option<Arc<AgentRuntime>>,
    tmp: tempfile::TempDir,
    repo: PathBuf,
}

impl Env {
    fn app(&self) -> &cmuxd::api::AppState {
        &self.server.app
    }

    async fn view(&self) -> StateView {
        self.app().actor.state_view().await.unwrap()
    }

    async fn wait<T>(&self, what: &str, f: impl Fn(&StateView) -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let v = self.view().await;
            if let Some(r) = f(&v) {
                return r;
            }
            if Instant::now() >= until {
                let mut screens = String::new();
                for a in &v.agents {
                    screens += &format!("--- {}\n{}\n", a.id, self.screen(&a.id.to_string()).await);
                }
                panic!(
                    "timed out waiting for {what}\n{:#?}\n{:#?}\n{screens}",
                    v.agents, v.items
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn post(&self, path: &str, body: &impl serde::Serialize) -> cmuxd::client::Response {
        self.unix.json("POST", path, &[], body).await.unwrap()
    }

    async fn create_plan(&self) -> String {
        let r = self
            .post(
                route::PLANS,
                &json!({"title": "demo", "repos": [{"path": self.repo.display().to_string()}],
                        "request": "build it"}),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
        r.json::<Created>().unwrap().id
    }

    /// A bodiless POST: the handler never reads one, and a server that closes
    /// with unread request bytes may reset the connection.
    async fn accept(&self, plan: &str) {
        let path = format!("/v1/plans/{plan}/accept");
        let r = self.unix.request("POST", &path, &[], None).await.unwrap();
        assert_eq!(r.status, 204, "{}", r.text());
    }

    async fn answer(&self, item: u64, action: AnswerAction, text: Option<&str>) {
        let r = self
            .post(
                &route::item_answer(item),
                &AnswerReq {
                    action,
                    text: text.map(str::to_string),
                },
            )
            .await;
        assert_eq!(r.status, 204, "{}", r.text());
    }

    async fn term_of(&self, agent: &str) -> Option<TermId> {
        let agent: AgentId = agent.parse().unwrap();
        self.app()
            .actor
            .query(move |s| {
                s.plans
                    .iter()
                    .flat_map(|p| &p.agents)
                    .find(|a| a.id == agent)
                    .and_then(|a| a.term)
            })
            .await
            .ok()
            .flatten()
    }

    /// The text on an agent's terminal.
    async fn screen(&self, agent: &str) -> String {
        match self.term_of(agent).await {
            Some(t) => self
                .app()
                .hub
                .get_term(t)
                .map(|t| t.screen_text())
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    async fn screen_has(&self, agent: &str, needle: &str) {
        let what = format!("{needle:?} on the screen of {agent}");
        self.wait_async(&what, async || {
            self.screen(agent).await.contains(needle).then_some(())
        })
        .await;
    }

    /// Like [`eventually`], but a timeout shows every agent's screen.
    async fn wait_async<T>(&self, what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(v) = f().await {
                return v;
            }
            if Instant::now() >= until {
                let v = self.view().await;
                let mut screens = String::new();
                for a in &v.agents {
                    screens += &format!("--- {}\n{}\n", a.id, self.screen(&a.id.to_string()).await);
                }
                panic!("timed out waiting for {what}\n{:#?}\n{screens}", v.items);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn pending_permissions(v: &StateView) -> Vec<ItemView> {
        v.items
            .iter()
            .filter(|i| {
                matches!(i.kind, ItemKind::Permission { .. }) && i.status == Delivery::Pending
            })
            .cloned()
            .collect()
    }

    async fn shutdown(self) {
        self.server.shutdown().await;
    }
}

fn base_config(tmp: &Path) -> Config {
    Config {
        data_dir: Some(tmp.join("data")),
        socket: Some(tmp.join("api.sock")),
        listen: Some("off".into()),
        ..Config::default()
    }
}

/// A server whose agents are the fake `claude` driven by `scripts`
/// (`(agent path, script)`).
async fn start_fake(scripts: &[(&str, Value)], permission_wait: Duration) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = make_repo(&root);
    let dir = root.join("scripts");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, script) in scripts {
        std::fs::write(
            dir.join(format!("{}.json", name.replace('/', "_"))),
            script.to_string(),
        )
        .unwrap();
    }
    let bin = root.join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nFAKE_CLAUDE_SCRIPTS='{}' exec '{}' \"$@\"\n",
            dir.display(),
            fake_claude_path().display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = Config {
        data_dir: Some(root.join("data")),
        socket: Some(root.join("api.sock")),
        ..base_config(&root)
    };
    let builder = ServerBuilder::new(config.clone()).tick(None);
    let mut spawn = SpawnConfig::new(CMUXD.into(), config.socket_path(), config.data_dir());
    spawn.claude_bin = bin.display().to_string();
    spawn.permission_wait = permission_wait;
    let rt = AgentRuntime::new(builder.hub(), spawn);
    let routes = rt.clone();
    let server = builder
        .transcripts(rt.transcripts().clone())
        .runner(rt.runner())
        .extra_routes(move || Extra {
            unix_only: routes.routes(),
            ..Extra::default()
        })
        .start()
        .await
        .unwrap();
    Env {
        unix: Endpoint::Unix(server.socket_path.clone()),
        server,
        rt: Some(rt),
        tmp,
        repo,
    }
}

// ---------------------------------------------------------------------------
// A runner that starts no process: agents get a token derived from their id
// ---------------------------------------------------------------------------

fn token_for(agent: &AgentId) -> String {
    format!("tok-{agent}")
}

struct StubRunner(ActorHandle, PathBuf);

impl EffectRunner for StubRunner {
    fn run(&self, effect: &Effect, state: &State) -> bool {
        match effect {
            Effect::CreateWorktree {
                plan, task, branch, ..
            } => self.0.send(Command::Runner(RunnerEvent::WorktreeReady {
                plan: plan.clone(),
                task: *task,
                path: self.1.join(task.to_string()),
                branch: branch.clone(),
            })),
            Effect::SpawnAgent { agent, .. } => {
                self.0.send(Command::Runner(RunnerEvent::Spawned {
                    agent: agent.clone(),
                    term: TermId(state.next_term_id.max(1)),
                    session_id: "s".into(),
                    token_hash: token_hash(&token_for(agent)),
                }))
            }
            _ => return false,
        }
        true
    }
}

async fn start_stub() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let wt = root.join("wt");
    let builder = ServerBuilder::new(base_config(&root)).tick(None);
    let rt = AgentRuntime::new(
        builder.hub(),
        SpawnConfig::new(CMUXD.into(), root.join("api.sock"), root.join("data")),
    );
    let routes = rt.clone();
    let server = builder
        .runner(move |h| Box::new(StubRunner(h, wt)))
        .extra_routes(move || Extra {
            unix_only: routes.routes(),
            ..Extra::default()
        })
        .start()
        .await
        .unwrap();
    Env {
        unix: Endpoint::Unix(server.socket_path.clone()),
        server,
        rt: Some(rt),
        tmp,
        repo,
    }
}

/// Runs `cmuxd mcp` as `agent`: sends the requests, closes stdin and returns
/// the responses.
async fn mcp_session(env: &Env, agent: &str, requests: &[Value]) -> Vec<Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let id: AgentId = agent.parse().unwrap();
    let mut child = tokio::process::Command::new(CMUXD)
        .arg("mcp")
        .env("CMUX_SOCK", env.server.socket_path.as_os_str())
        .env("CMUX_AGENT", agent)
        .env("CMUX_TOKEN", token_for(&id))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        stdin.write_all(format!("{r}\n").as_bytes()).await.unwrap();
    }
    drop(stdin);
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut out = Vec::new();
    while let Some(l) = lines.next_line().await.unwrap() {
        out.push(serde_json::from_str(&l).unwrap());
    }
    assert!(child.wait().await.unwrap().success());
    out
}

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn call(id: u64, name: &str, args: Value) -> Value {
    rpc(id, "tools/call", json!({"name": name, "arguments": args}))
}

fn text_of(r: &Value) -> (String, bool) {
    (
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
        r["result"]["isError"].as_bool().unwrap(),
    )
}

fn names(r: &Value) -> Vec<String> {
    r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// MCP, per role
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn scripted_mcp_sessions_per_role() {
    let env = start_stub().await;
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    let init = rpc(1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    let list = rpc(2, "tools/list", json!({}));

    // Executor: lists its tools, plans, and is refused a task agent's tool.
    let out = mcp_session(
        &env,
        &exec,
        &[
            init.clone(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            list.clone(),
            rpc(3, "ping", json!({})),
            call(
                4,
                "create_task",
                json!({"title": "write code", "criteria": ["it works"]}),
            ),
            call(5, "update_plan", json!({"state": "proposed"})),
            call(
                6,
                "report_result",
                json!({"status": "success", "summary": "x"}),
            ),
            call(7, "spawn_task", json!({"task": "T-0001"})),
            call(
                8,
                "transition_task",
                json!({"task": "T-0001", "to": "sideways"}),
            ),
        ],
    )
    .await;
    assert_eq!(out.len(), 8, "the notification gets no answer");
    assert_eq!(out[0]["result"]["serverInfo"]["name"], "cmux");
    let exec_tools = names(&out[1]);
    assert!(
        exec_tools.contains(&"spawn_task".into()) && !exec_tools.contains(&"report_result".into())
    );
    assert_eq!(out[2]["result"], json!({}));
    let (t, err) = text_of(&out[3]);
    assert!(!err && t.contains("T-0001"), "{t}");
    assert!(!text_of(&out[4]).1);
    let (t, err) = text_of(&out[5]);
    assert!(
        err && t.starts_with("error: report_result is not available to executor"),
        "{t}"
    );
    let (t, err) = text_of(&out[6]);
    assert!(err && t.contains("accepted"), "spawn before accept: {t}");
    let (t, err) = text_of(&out[7]);
    assert!(err && t.contains("expected one of"), "{t}");

    // Accept, spawn the task, and run as the task agent.
    env.accept(&plan).await;
    let out = mcp_session(
        &env,
        &exec,
        &[call(1, "spawn_task", json!({"task": "T-0001"}))],
    )
    .await;
    assert!(!text_of(&out[0]).1, "{}", text_of(&out[0]).0);
    let task = format!("{plan}/T-0001");
    env.wait("the task agent", |v| {
        v.agents
            .iter()
            .any(|a| a.id.to_string() == task && a.term.is_some())
            .then_some(())
    })
    .await;
    let out = mcp_session(
        &env,
        &task,
        &[
            list.clone(),
            call(2, "spawn_task", json!({"task": "T-0001"})),
            call(3, "get_task", json!({})),
            call(
                4,
                "transition_task",
                json!({"task": "T-0001", "to": "merged"}),
            ),
            call(
                5,
                "spawn_subagent",
                json!({"name": "lint", "prompt": "lint it"}),
            ),
        ],
    )
    .await;
    let task_tools = names(&out[0]);
    assert!(
        task_tools.contains(&"spawn_subagent".into())
            && task_tools.contains(&"report_result".into())
    );
    assert!(
        !task_tools.contains(&"spawn_task".into()) && !task_tools.contains(&"create_task".into())
    );
    let (t, err) = text_of(&out[1]);
    assert!(err && t.contains("available to: executor"), "{t}");
    let (t, err) = text_of(&out[2]);
    assert!(!err && t.contains("write code"), "{t}");
    let (t, err) = text_of(&out[3]);
    assert!(err && t.contains("only to: testing, blocked"), "{t}");
    assert!(!text_of(&out[4]).1, "{}", text_of(&out[4]).0);

    // The sub-agent sees four tools and cannot read the plan.
    let sub = format!("{plan}/T-0001/lint");
    env.wait("the sub-agent", |v| {
        v.agents
            .iter()
            .any(|a| a.id.to_string() == sub && a.term.is_some())
            .then_some(())
    })
    .await;
    let out = mcp_session(
        &env,
        &sub,
        &[
            list.clone(),
            call(2, "get_plan", json!({})),
            call(
                3,
                "add_note",
                json!({"task": "T-0001", "text": "found a typo"}),
            ),
            call(
                4,
                "report_result",
                json!({"status": "success", "summary": "clean"}),
            ),
        ],
    )
    .await;
    assert_eq!(
        names(&out[0]),
        ["get_task", "add_note", "report_result", "ask_parent"]
    );
    let (t, err) = text_of(&out[1]);
    assert!(err && t.contains("not available to sub-agent"), "{t}");
    assert!(!text_of(&out[2]).1, "{}", text_of(&out[2]).0);
    assert!(!text_of(&out[3]).1, "{}", text_of(&out[3]).0);

    // A wrong token is refused as such.
    let r = env
        .post(
            route::AGENT_TOOL,
            &json!({"agent": sub, "token": "wrong", "name": "get_task"}),
        )
        .await;
    assert_eq!(r.status, 401);
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_routes_exist_on_the_socket_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let mut cfg = base_config(&root);
    cfg.listen = Some("127.0.0.1:0".into());
    let builder = ServerBuilder::new(cfg.clone()).tick(None);
    let rt = AgentRuntime::new(
        builder.hub(),
        SpawnConfig::new(CMUXD.into(), cfg.socket_path(), cfg.data_dir()),
    );
    let routes = rt.clone();
    let server = builder
        .extra_routes(move || Extra {
            unix_only: routes.routes(),
            ..Extra::default()
        })
        .start()
        .await
        .unwrap();
    let body =
        json!({"agent": "P-20261018-x/executor", "token": "t", "event": "Stop", "payload": {}});
    let unix = Endpoint::Unix(server.socket_path.clone());
    let r = unix
        .json("POST", route::AGENT_HOOK, &[], &body)
        .await
        .unwrap();
    assert_eq!(r.status, 401, "unknown agent: {}", r.text());
    // On TCP there is no such route: not even the agent token is looked at.
    let tcp = Endpoint::Tcp(server.tcp_addr.unwrap());
    let r = tcp
        .json("POST", route::AGENT_HOOK, &[], &body)
        .await
        .unwrap();
    assert_eq!(r.status, 404);
    let start: cmux_proto::api::PairStartResp = unix
        .json("POST", route::PAIR_START, &[], &json!({}))
        .await
        .unwrap()
        .json()
        .unwrap();
    let done: cmux_proto::api::PairCompleteResp = tcp
        .json(
            "POST",
            route::PAIR_COMPLETE,
            &[],
            &json!({"code": start.code, "device_name": "p"}),
        )
        .await
        .unwrap()
        .json()
        .unwrap();
    let auth = format!("Bearer {}", done.token);
    let r = tcp
        .json(
            "POST",
            route::AGENT_HOOK,
            &[("Authorization", &auth)],
            &body,
        )
        .await
        .unwrap();
    assert_eq!(r.status, 404);
    server.shutdown().await;
}

// ---------------------------------------------------------------------------
// `cmuxd hook`
// ---------------------------------------------------------------------------

fn run_hook(env: &[(&str, &str)], event: &str) -> (std::process::Output, Duration) {
    use std::io::Write;
    let mut cmd = Proc::new(CMUXD);
    cmd.args(["hook", event])
        .env_remove("CMUX_SOCK")
        .env_remove("CMUX_AGENT")
        .env_remove("CMUX_TOKEN")
        .envs(env.iter().copied())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let started = Instant::now();
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(br#"{"stop_hook_active":false}"#);
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    (out, started.elapsed())
}

#[test]
fn hook_is_silent_and_exits_zero_without_a_server() {
    let (out, _) = run_hook(&[], "Stop");
    assert!(out.status.success() && out.stdout.is_empty());
    let env = [
        ("CMUX_SOCK", "/nonexistent/api.sock"),
        ("CMUX_AGENT", "P-20261018-x/executor"),
        ("CMUX_TOKEN", "t"),
    ];
    let (out, took) = run_hook(&env, "Stop");
    assert!(out.status.success() && out.stdout.is_empty());
    assert!(took < Duration::from_secs(2), "{took:?}");
    // Even a nonsense event and agent id do not fail Claude's turn.
    let (out, _) = run_hook(
        &[
            ("CMUX_SOCK", "/nonexistent"),
            ("CMUX_AGENT", "???"),
            ("CMUX_TOKEN", "t"),
        ],
        "Bogus",
    );
    assert!(out.status.success() && out.stdout.is_empty());
}

#[test]
fn hook_gives_up_on_a_server_that_never_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("hang.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let held = std::thread::spawn(move || {
        let conn = listener.accept();
        std::thread::sleep(Duration::from_secs(10));
        drop(conn);
    });
    let env = [
        ("CMUX_SOCK", sock.to_str().unwrap()),
        ("CMUX_AGENT", "P-20261018-x/executor"),
        ("CMUX_TOKEN", "t"),
    ];
    let (out, took) = run_hook(&env, "Stop");
    assert!(out.status.success() && out.stdout.is_empty());
    // The server holds the connection for 10 s, so returning well before that
    // proves the 2 s cap; the slack covers a cold exec of the debug binary.
    assert!(
        took >= Duration::from_millis(1900) && took < Duration::from_secs(8),
        "{took:?}"
    );
    drop(held);
}

// ---------------------------------------------------------------------------
// End to end with the fake claude
// ---------------------------------------------------------------------------

fn executor_script(later: Value) -> Value {
    json!({
        "first": [
            {"tool": "create_task", "args": {"title": "write code"}},
            {"tool": "update_plan", "args": {"state": "proposed"}},
        ],
        "later": later,
    })
}

fn task_script() -> Value {
    json!({"first": [
        {"tool": "report_result", "args": {"status": "success", "summary": "implemented the thing"}},
    ]})
}

/// Plan created, executor idle with the plan proposed; then accepted.
async fn accepted_plan(env: &Env) -> (String, String) {
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    env.wait("the executor to propose and idle", |v| {
        let proposed = v
            .plans
            .iter()
            .any(|p| p.state == cmux_proto::view::PlanState::Proposed);
        let idle = v
            .agents
            .iter()
            .any(|a| a.id.to_string() == exec && a.activity == Activity::Idle);
        (proposed && idle).then_some(())
    })
    .await;
    // MCP tools reached the engine from a real process.
    env.screen_has(&exec, "TOOLS: ").await;
    env.accept(&plan).await;
    (plan, exec)
}

fn report_item(v: &StateView) -> Option<ItemView> {
    v.items
        .iter()
        .find(|i| i.kind == ItemKind::ChildReport && matches!(i.to, Recipient::Agent(_)))
        .cloned()
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_report_stop_and_the_parent_gets_it_by_paste() {
    let env = start_fake(
        &[
            (
                "executor",
                executor_script(json!([
                    {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
                ])),
            ),
            ("T-0001", task_script()),
        ],
        Duration::from_secs(600),
    )
    .await;
    let (plan, exec) = accepted_plan(&env).await;
    // The executor started the task through MCP: worktree, spawn, SessionStart,
    // report_result, Stop, and the task moves on.
    let task = env
        .wait("the task to pass", |v| {
            v.tasks
                .iter()
                .find(|t| t.state == TaskState::Passing)
                .cloned()
        })
        .await;
    assert_eq!(task.branch.as_deref(), Some("claude/T-0001"));
    assert!(env.repo.join(".worktrees/T-0001").is_dir());
    let tid = format!("{plan}/T-0001");
    let v = env.view().await;
    let agent = v.agents.iter().find(|a| a.id.to_string() == tid).unwrap();
    assert_eq!(agent.level, cmux_proto::view::Level::Task);
    assert_eq!(
        agent.report.as_ref().map(|r| r.summary.as_str()),
        Some("implemented the thing")
    );

    // The executor was idle, so the report is pasted (path B) into its PTY.
    let item = env
        .wait("the report to be delivered", |v| {
            report_item(v).filter(|i| matches!(i.status, Delivery::Delivered { .. }))
        })
        .await;
    assert!(matches!(
        item.status,
        Delivery::Delivered {
            via: DeliveryPath::PtyPaste,
            ..
        }
    ));
    env.screen_has(&exec, "ECHO [cmux]").await;
    // The digest (more than one line) went through a file the pasted line names.
    let dir = agent_dir(&env.app().data_dir, &exec.parse().unwrap());
    // Only `digests/` is given to `--add-dir`, not the agent dir.
    let digests: Vec<String> = std::fs::read_dir(dir.join("digests"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("digest-"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(
        digests.iter().any(|d| d.contains("implemented the thing")),
        "{digests:?}"
    );
    // The agent files are on disk, and the token only in the private one.
    assert!(dir.join("settings.json").exists() && dir.join("system.md").exists());
    let mcp = std::fs::read_to_string(dir.join("mcp.json")).unwrap();
    assert!(mcp.contains("CMUX_TOKEN"));
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_parent_gets_the_report_through_the_stop_hook() {
    let flag = tempfile::tempdir().unwrap();
    let flag_file = flag.path().join("go");
    let env = start_fake(
        &[
            (
                "executor",
                executor_script(json!([
                    {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
                    {"when": "plan was accepted", "wait_file": flag_file.display().to_string()},
                ])),
            ),
            ("T-0001", task_script()),
        ],
        Duration::from_secs(600),
    )
    .await;
    let (_, exec) = accepted_plan(&env).await;
    env.wait("the task to pass", |v| {
        v.tasks
            .iter()
            .find(|t| t.state == TaskState::Passing)
            .map(|_| ())
    })
    .await;
    // The executor is still in its turn, so the report waits.
    let item = env.wait("the report", report_item).await;
    assert_eq!(item.status, Delivery::Pending);
    std::fs::write(&flag_file, "").unwrap();
    // Its Stop hook is answered with a block that carries the digest.
    env.screen_has(&exec, "STOP-BLOCKED:").await;
    env.screen_has(&exec, "implemented the thing").await;
    let item = env
        .wait("delivery", |v| {
            report_item(v).filter(|i| i.status != Delivery::Pending)
        })
        .await;
    assert!(matches!(
        item.status,
        Delivery::Delivered {
            via: DeliveryPath::StopBlock,
            ..
        }
    ));
    env.shutdown().await;
}

fn permission_script(commands: &[&str]) -> Value {
    let steps: Vec<Value> = commands
        .iter()
        .map(|c| json!({"permission": {"tool": "Bash", "input": {"command": c}}}))
        .collect();
    json!({"first": steps})
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_permission_hook_is_answered_over_rest() {
    let env = start_fake(
        &[(
            "executor",
            permission_script(&["cargo test -p x", "cargo test -p y"]),
        )],
        Duration::from_secs(600),
    )
    .await;
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    let held = env
        .wait("a permission item", |v| Env::pending_permissions(v).pop())
        .await;
    assert!(
        matches!(&held.kind, ItemKind::Permission { tool, input_summary }
            if tool == "Bash" && input_summary == "cargo test -p x")
    );
    assert!(held.needs_human);
    // The hook process is open, and the agent is shown as waiting for a person.
    let v = env.view().await;
    let a = v.agents.iter().find(|a| a.id.to_string() == exec).unwrap();
    assert_eq!(a.activity, Activity::NeedsPermission);
    assert!(!env.screen(&exec).await.contains("PERM"));
    assert_eq!(env.rt.as_ref().unwrap().waiters().waiting(), 1);

    // "Allow always" answers this one and every later `cargo test`.
    env.answer(held.id.0, AnswerAction::AllowAlways, None).await;
    env.screen_has(&exec, "PERM allow").await;
    // The second request matches the task's rule... but the executor has no
    // task, so it is held again.
    let second = env
        .wait("the second permission item", |v| {
            Env::pending_permissions(v).pop()
        })
        .await;
    assert_ne!(second.id, held.id);
    env.answer(second.id.0, AnswerAction::Deny, Some("not now"))
        .await;
    env.screen_has(&exec, "PERM deny: not now").await;
    assert_eq!(env.rt.as_ref().unwrap().waiters().waiting(), 0);
    env.shutdown().await;
}

/// What the phone does: the permission arrives as a push, and the
/// notification's Allow answers it with an `Idempotency-Key`.
#[tokio::test(flavor = "multi_thread")]
async fn a_permission_is_pushed_and_allow_from_the_notification_lets_the_agent_go_on() {
    use cmux_proto::api::{NotifyKind, PushSubscribeReq};
    let env = start_fake(
        &[("executor", permission_script(&["cargo test -p x"]))],
        Duration::from_secs(600),
    )
    .await;
    let mock = mock_http::MockHttp::start().await;
    let browser = mock_http::Browser::new();
    let sub = PushSubscribeReq {
        endpoint: format!("{}/push/phone", mock.base),
        keys: browser.keys(),
        kinds: vec![NotifyKind::Permission],
    };
    let r = env.post(route::PUSH_SUBSCRIBE, &sub).await;
    assert_eq!(r.status, 204, "{}", r.text());
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    let hit = mock.wait_for(1).await;
    let p = browser.open(&hit.body);
    assert_eq!(p.kind, NotifyKind::Permission);
    assert_eq!(p.summary, "executor: Bash cargo test -p x");
    let item = p.item.unwrap();
    let held = env
        .wait("a permission item", |v| Env::pending_permissions(v).pop())
        .await;
    assert_eq!(held.id.0, item);

    let allow = AnswerReq {
        action: AnswerAction::Allow,
        text: None,
    };
    let key = [("Idempotency-Key", "notification-1")];
    // A retried tap acts once.
    for _ in 0..2 {
        let r = env
            .unix
            .json("POST", &route::item_answer(item), &key, &allow)
            .await
            .unwrap();
        assert_eq!(r.status, 204, "{}", r.text());
    }
    env.screen_has(&exec, "PERM allow").await;
    assert_eq!(env.rt.as_ref().unwrap().waiters().waiting(), 0);
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allow_rule_answers_a_later_request_at_once() {
    let env = start_fake(
        &[
            (
                "executor",
                executor_script(json!([
                    {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
                ])),
            ),
            (
                "T-0001",
                json!({"first": [
                    {"permission": {"tool": "Bash", "input": {"command": "cargo test -p x"}}},
                    {"permission": {"tool": "Bash", "input": {"command": "cargo test -p y"}}},
                ]}),
            ),
        ],
        Duration::from_secs(600),
    )
    .await;
    let (plan, _) = accepted_plan(&env).await;
    let task = format!("{plan}/T-0001");
    let held = env
        .wait("a permission item", |v| Env::pending_permissions(v).pop())
        .await;
    env.answer(held.id.0, AnswerAction::AllowAlways, None).await;
    env.screen_has(&task, "PERM allow").await;
    // `cargo test *` is now a rule of the task: the second is allowed with no item.
    eventually("the second allow", async || {
        (env.screen(&task).await.matches("PERM allow").count() == 2).then_some(())
    })
    .await;
    assert!(Env::pending_permissions(&env.view().await).is_empty());
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_answer_wins_over_a_held_hook() {
    let env = start_fake(
        &[("executor", permission_script(&["rm -rf build"]))],
        Duration::from_secs(600),
    )
    .await;
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    let held = env
        .wait("a permission item", |v| Env::pending_permissions(v).pop())
        .await;
    // The person presses 1 in the terminal; Claude moves on and kills the hook.
    let term = env.term_of(&exec).await.unwrap();
    env.app().hub.get_term(term).unwrap().input("tui-1", b"1\r");
    env.screen_has(&exec, "PERM terminal").await;
    // The agent's next hook (its Stop) releases the item without a decision.
    let item = env
        .wait("the item to be closed", |v| {
            v.items
                .iter()
                .find(|i| i.id == held.id && i.status != Delivery::Pending)
                .cloned()
        })
        .await;
    assert_eq!(item.status, Delivery::Dismissed);
    let rt = env.rt.clone().unwrap();
    eventually("the waiter to go", async || {
        (rt.waiters().waiting() == 0).then_some(())
    })
    .await;
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_hook_falls_back_to_the_terminal_prompt_on_timeout() {
    let env = start_fake(
        &[("executor", permission_script(&["make deploy"]))],
        Duration::from_secs(30),
    )
    .await;
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    let held = env
        .wait("a permission item", |v| Env::pending_permissions(v).pop())
        .await;
    env.app().clock.advance(Duration::from_secs(601));
    env.app().actor.send(Command::Tick);
    // No decision: the hook prints nothing and Claude shows its own prompt.
    env.screen_has(&exec, "PERM none").await;
    let v = env.view().await;
    let item = v.items.iter().find(|i| i.id == held.id).unwrap();
    assert_eq!(item.status, Delivery::Expired);
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hook_that_disconnects_leaves_no_waiter() {
    use tokio::io::AsyncWriteExt;
    let env = start_stub().await;
    let plan = env.create_plan().await;
    let exec = format!("{plan}/executor");
    let body = json!({
        "agent": exec, "token": token_for(&exec.parse().unwrap()),
        "event": "PermissionRequest",
        "payload": {"tool_name": "Bash", "tool_input": {"command": "ls"}},
    })
    .to_string();
    let mut s = tokio::net::UnixStream::connect(&env.server.socket_path)
        .await
        .unwrap();
    s.write_all(
        format!(
            "POST {} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            route::AGENT_HOOK,
            body.len()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let rt = env.rt.clone().unwrap();
    eventually("the hook to be held", async || {
        (rt.waiters().waiting() == 1).then_some(())
    })
    .await;
    drop(s);
    eventually("the waiter to be dropped", async || {
        (rt.waiters().waiting() == 0).then_some(())
    })
    .await;
    // The item stays for the person until the agent moves on.
    assert_eq!(Env::pending_permissions(&env.view().await).len(), 1);
    let _ = &env.tmp;
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exited_agents_terminal_keeps_its_final_screen_in_the_hub() {
    let env = start_fake(
        &[("executor", executor_script(json!([])))],
        Duration::from_secs(600),
    )
    .await;
    let (_, exec) = accepted_plan(&env).await;
    let term = env.term_of(&exec).await.unwrap();
    let agent: AgentId = exec.parse().unwrap();
    env.app()
        .actor
        .client(
            cmux_core::ids::ClientId("test".into()),
            cmux_core::engine::ClientAction::StopAgent { agent },
        )
        .await
        .unwrap()
        .unwrap();
    let hub = env.app().hub.clone();
    eventually("the process to end", async || {
        hub.get_term(term)
            .filter(|t| t.exited().borrow().is_some())
            .map(|_| ())
    })
    .await;
    // Give the runner time to (wrongly) drop it, then look again.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let t = hub.get_term(term).expect("terminal removed after exit");
    assert!(t.screen_text().contains("TOOLS: "));
    env.shutdown().await;
}

// ---------------------------------------------------------------------------
// Sub-agents and questions (step 9)
// ---------------------------------------------------------------------------

/// Runs the executor through acceptance and starts `T-0001`; returns the plan.
async fn plan_with_task(task: Value, subs: &[(&str, Value)]) -> (Env, String) {
    let mut scripts = vec![
        (
            "executor",
            executor_script(json!([
                {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
            ])),
        ),
        ("T-0001", task),
    ];
    scripts.extend(subs.iter().map(|(n, s)| (*n, s.clone())));
    let env = start_fake(&scripts, Duration::from_secs(600)).await;
    let (plan, _) = accepted_plan(&env).await;
    (env, plan)
}

fn agent_view(v: &StateView, id: &str) -> Option<cmux_proto::view::AgentView> {
    v.agents.iter().find(|a| a.id.to_string() == id).cloned()
}

fn sub_report(v: &StateView, sub: &str) -> Option<ItemView> {
    v.items
        .iter()
        .find(|i| {
            i.kind == ItemKind::ChildReport && i.from.as_ref().is_some_and(|f| f.to_string() == sub)
        })
        .cloned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_agent_spawns_sub_agents_up_to_the_limit_and_gets_their_reports() {
    let flag = tempfile::tempdir().unwrap();
    let flag_file = flag.path().join("go").display().to_string();
    let sub = |summary: &str| {
        json!({"first": [
            {"wait_file": flag_file},
            {"tool": "report_result", "args": {"status": "success", "summary": summary}},
        ]})
    };
    let task = json!({
        "first": [
            {"tool": "spawn_subagent", "args": {"name": "lint", "prompt": "run the linter"}},
            {"tool": "spawn_subagent", "args": {"name": "docs", "prompt": "fix the docs"}},
            {"tool": "spawn_subagent", "args": {"name": "third", "prompt": "one too many"}},
        ],
        "later": [
            {"when": "lint is clean", "tool": "report_result",
             "args": {"status": "success", "summary": "all done"}},
        ],
    });
    let (env, plan) = plan_with_task(
        task,
        &[
            ("T-0001/lint", sub("lint is clean")),
            ("T-0001/docs", sub("docs fixed")),
        ],
    )
    .await;
    let (tid, lint, docs) = (
        format!("{plan}/T-0001"),
        format!("{plan}/T-0001/lint"),
        format!("{plan}/T-0001/docs"),
    );

    // The third is refused through MCP: two sub-agents per task.
    env.screen_has(&tid, "TOOL spawn_subagent: error: limit reached")
        .await;
    let v = env.view().await;
    assert!(agent_view(&v, &format!("{plan}/T-0001/third")).is_none());

    // The sub-agents are real processes in the task's worktree, with the
    // sub-agent prompt and tool set, linked to their task and parent.
    let worktree = env.repo.join(".worktrees/T-0001");
    let a = agent_view(&v, &lint).expect("lint exists");
    assert_eq!(a.level, cmux_proto::view::Level::Sub);
    assert_eq!(
        a.parent.as_ref().map(ToString::to_string),
        Some(tid.clone())
    );
    assert_eq!(a.task.map(|t| t.to_string()), Some("T-0001".into()));
    assert_eq!(a.cwd, worktree);
    env.screen_has(&lint, "SYSTEM: # Sub-agent: T-0001/lint")
        .await;
    env.screen_has(&lint, "/repo/.worktrees/T-0001").await;
    env.screen_has(&lint, "report_result,ask_parent").await;
    env.screen_has(&lint, "run the linter").await;
    assert!(!env.screen(&lint).await.contains("spawn_subagent"));
    assert!(agent_view(&v, &docs).is_some());

    // Release them. The report reaches the task agent, which finishes.
    std::fs::write(&flag_file, "").unwrap();
    let item = env
        .wait("the lint report to be delivered", |v| {
            sub_report(v, &lint).filter(|i| matches!(i.status, Delivery::Delivered { .. }))
        })
        .await;
    assert_eq!(item.to, Recipient::Agent(tid.parse().unwrap()));
    env.wait("the task to pass", |v| {
        v.tasks
            .iter()
            .find(|t| t.state == TaskState::Passing)
            .map(|_| ())
    })
    .await;
    // A finished sub-agent stays listed with its report (clients dim it).
    let v = env.view().await;
    let a = agent_view(&v, &lint).unwrap();
    assert_eq!(
        a.report.as_ref().map(|r| r.summary.as_str()),
        Some("lint is clean")
    );
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_sub_agent_is_stopped_and_the_task_agent_can_spawn_another() {
    // The limit is two sub-agents per task: `busy` never finishes and holds one
    // slot, so `docs` can only start once the finished `lint` has freed the other.
    let never = tempfile::tempdir().unwrap().path().join("never");
    let task = json!({
        "first": [
            {"tool": "spawn_subagent", "args": {"name": "lint", "prompt": "run the linter"}},
            {"tool": "spawn_subagent", "args": {"name": "busy", "prompt": "wait"}},
        ],
        "later": [
            {"when": "lint is clean", "tool": "spawn_subagent",
             "args": {"name": "docs", "prompt": "fix the docs"}},
        ],
    });
    let lint_script = json!({"first": [
        {"tool": "report_result", "args": {"status": "success", "summary": "lint is clean"}},
    ]});
    let docs_script = json!({"first": [{"print": "docs is up"}]});
    let busy_script = json!({"first": [{"wait_file": never.display().to_string()}]});
    let (env, plan) = plan_with_task(
        task,
        &[
            ("T-0001/lint", lint_script),
            ("T-0001/busy", busy_script),
            ("T-0001/docs", docs_script),
        ],
    )
    .await;
    let (tid, lint, docs) = (
        format!("{plan}/T-0001"),
        format!("{plan}/T-0001/lint"),
        format!("{plan}/T-0001/docs"),
    );
    // The report lands, and the finished sub-agent's process goes: it stays
    // listed, stopped, with its report.
    let lint_term = eventually("lint to get a terminal", async || env.term_of(&lint).await).await;
    let a = env
        .wait("the lint sub-agent to be stopped", |v| {
            agent_view(v, &lint).filter(|a| a.life == Life::Stopped)
        })
        .await;
    assert_eq!(
        a.report.as_ref().map(|r| r.summary.as_str()),
        Some("lint is clean")
    );
    let hub = env.app().hub.clone();
    eventually("the lint process to end", async || {
        hub.get_term(lint_term)
            .filter(|t| t.exited().borrow().is_some())
            .map(|_| ())
    })
    .await;
    // The task agent was woken by the report and starts the next one.
    env.screen_has(&tid, "TOOL spawn_subagent: started sub-agent T-0001/docs")
        .await;
    env.screen_has(&docs, "docs is up").await;
    let v = env.view().await;
    assert_eq!(agent_view(&v, &docs).unwrap().life, Life::Running);
    assert_eq!(
        agent_view(&v, &format!("{plan}/T-0001/busy")).unwrap().life,
        Life::Running
    );
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sub_agent_asks_its_parent_and_the_answer_comes_back() {
    let task = json!({
        "first": [{"tool": "spawn_subagent", "args": {"name": "lint", "prompt": "run the linter"}}],
        "later": [
            {"when": "which config?", "tool": "answer_child",
             "args": {"agent": "T-0001/lint", "answer": "use strict.toml"}},
            {"when": "linted with strict.toml", "tool": "report_result",
             "args": {"status": "success", "summary": "all done"}},
        ],
    });
    let sub = json!({
        "first": [{"tool": "ask_parent", "args": {"question": "which config?"}}],
        "later": [
            {"when": "use strict.toml", "print": "CONFIG: strict.toml"},
            {"when": "use strict.toml", "tool": "report_result",
             "args": {"status": "success", "summary": "linted with strict.toml"}},
        ],
    });
    let (env, plan) = plan_with_task(task, &[("T-0001/lint", sub)]).await;
    let lint = format!("{plan}/T-0001/lint");
    let question = env
        .wait("the question", |v| {
            v.items
                .iter()
                .find(|i| i.kind == ItemKind::ChildQuestion)
                .cloned()
        })
        .await;
    assert_eq!(question.body, "which config?");
    assert_eq!(
        question.from.as_ref().map(ToString::to_string),
        Some(lint.clone())
    );
    assert!(!question.needs_human);
    // The task agent answered; the sub-agent got the answer as a prompt and
    // reported, and that report finished the task.
    env.screen_has(&format!("{plan}/T-0001"), "TOOL answer_child: answer sent")
        .await;
    // The answer can arrive by a paste or a Stop-hook block, depending on
    // whether the sub-agent already idled. Both must run the matching steps.
    env.screen_has(&lint, "CONFIG: strict.toml").await;
    env.wait("the question to be answered", |v| {
        v.items
            .iter()
            .find(|i| i.id == question.id && matches!(i.status, Delivery::Answered { .. }))
            .map(|_| ())
    })
    .await;
    env.wait("the task to pass", |v| {
        v.tasks
            .iter()
            .find(|t| t.state == TaskState::Passing)
            .map(|_| ())
    })
    .await;
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_user_is_answered_over_rest_and_expires_after_sixty_minutes() {
    let task = json!({
        "first": [{"tool": "ask_user", "args": {"question": "ship it?"}}],
        "later": [
            {"when": "yes, ship", "tool": "report_result",
             "args": {"status": "success", "summary": "shipped"}},
            {"when": "got no answer", "print": "EXPIRED-SEEN"},
        ],
    });
    let (env, plan) = plan_with_task(task, &[]).await;
    let tid = format!("{plan}/T-0001");
    let ask = |v: &StateView, body: &str| {
        v.items
            .iter()
            .find(|i| i.kind == ItemKind::Question && i.body == body)
            .cloned()
    };
    let q = env.wait("the question", |v| ask(v, "ship it?")).await;
    assert!(q.needs_human);
    assert_eq!(q.to, Recipient::Human);
    env.screen_has(&tid, "(it expires after 60 minutes)").await;
    env.answer(q.id.0, AnswerAction::Reply, Some("yes, ship"))
        .await;
    env.wait("the task to pass", |v| {
        v.tasks
            .iter()
            .find(|t| t.state == TaskState::Passing)
            .map(|_| ())
    })
    .await;
    env.shutdown().await;

    // An unanswered question expires on a tick and the asker is told.
    let task = json!({
        "first": [{"tool": "ask_user", "args": {"question": "which color?"}}],
        "later": [{"when": "got no answer", "print": "EXPIRED-SEEN"}],
    });
    let (env, plan) = plan_with_task(task, &[]).await;
    let tid = format!("{plan}/T-0001");
    let q = env.wait("the question", |v| ask(v, "which color?")).await;
    env.app().clock.advance(Duration::from_secs(59 * 60));
    env.app().actor.send(Command::Tick);
    assert_eq!(
        ask(&env.view().await, "which color?").unwrap().status,
        Delivery::Pending
    );
    env.app().clock.advance(Duration::from_secs(2 * 60));
    env.app().actor.send(Command::Tick);
    env.screen_has(&tid, "EXPIRED-SEEN").await;
    let v = env.view().await;
    assert_eq!(
        v.items.iter().find(|i| i.id == q.id).unwrap().status,
        Delivery::Expired
    );
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sub_agents_git_push_is_denied_by_the_pre_tool_use_hook() {
    let task = json!({
        "first": [{"tool": "spawn_subagent", "args": {"name": "lint", "prompt": "run the linter"}}],
        "later": [{"when": "pushed nothing", "tool": "report_result",
                   "args": {"status": "success", "summary": "done"}}],
    });
    let sub = json!({"first": [
        {"permission": {"tool": "Bash", "input": {"command": "git add -A && git push origin HEAD"}}},
        {"permission": {"tool": "Bash", "input": {"command": "git checkout main"}}},
        {"permission": {"tool": "Bash", "input": {"command": "git status"}}},
        {"tool": "report_result", "args": {"status": "success", "summary": "pushed nothing"}},
    ]});
    let (env, plan) = plan_with_task(task, &[("T-0001/lint", sub)]).await;
    let lint = format!("{plan}/T-0001/lint");
    // The reason reaches Claude in the hook's real output format.
    env.screen_has(&lint, "PRE deny: error: sub-agents may not run git push")
        .await;
    env.screen_has(
        &lint,
        "PRE deny: error: do not check out the base branch main",
    )
    .await;
    // `git status` is fine: the hook prints nothing and the permission flow
    // goes on to hold a request for a human.
    let held = env
        .wait("a permission item for git status", |v| {
            Env::pending_permissions(v).pop()
        })
        .await;
    assert!(held.body.contains("git status"), "{}", held.body);
    assert_eq!(held.from.as_ref().map(ToString::to_string), Some(lint));
    // The denied command never became a request.
    assert!(
        !env.view()
            .await
            .items
            .iter()
            .any(|i| i.body.contains("git push"))
    );
    env.answer(held.id.0, AnswerAction::Allow, None).await;
    env.shutdown().await;
}

/// The executor starts T-0001, and T-0001 spawns the sub-agent `name`.
fn sub_agent_scripts(name: &str, task_later: Value, sub: Value) -> Vec<(String, Value)> {
    vec![
        (
            "executor".into(),
            executor_script(json!([
                {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
            ])),
        ),
        (
            "T-0001".into(),
            json!({
                "first": [
                    {"tool": "spawn_subagent", "args": {"name": name, "prompt": "help out"}},
                ],
                "later": task_later,
            }),
        ),
        (format!("T-0001/{name}"), sub),
    ]
}

async fn start_with(scripts: &[(String, Value)]) -> Env {
    let scripts: Vec<(&str, Value)> = scripts
        .iter()
        .map(|(n, s)| (n.as_str(), s.clone()))
        .collect();
    start_fake(&scripts, Duration::from_secs(600)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sub_agent_git_push_is_denied_through_the_hook() {
    let env = start_with(&sub_agent_scripts(
        "pusher",
        json!([]),
        permission_script(&["git push origin HEAD"]),
    ))
    .await;
    let (plan, _) = accepted_plan(&env).await;
    let sub = format!("{plan}/T-0001/pusher");
    // `cmuxd hook` answered `PreToolUse` with the guard rail's deny.
    env.screen_has(&sub, "PRE deny: error: sub-agents may not run git push")
        .await;
    // The deny needs no person: no permission item, no held hook.
    assert!(Env::pending_permissions(&env.view().await).is_empty());
    assert_eq!(env.rt.as_ref().unwrap().waiters().waiting(), 0);
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ask_parent_and_answer_child_round_trip() {
    let flag = tempfile::tempdir().unwrap();
    let flag_file = flag.path().join("ask");
    let answer_file = flag.path().join("answer");
    let env = start_with(&sub_agent_scripts(
        "db",
        json!([
            // Answer only once the sub-agent idles, so the answer is pasted.
            {"when": "Which database", "wait_file": answer_file.display().to_string()},
            {"when": "Which database", "tool": "answer_child",
             "args": {"agent": "T-0001/db", "answer": "use postgres"}},
        ]),
        json!({
            // Ask only once the task agent idles, so the question is pasted.
            "first": [
                {"wait_file": flag_file.display().to_string()},
                {"tool": "ask_parent", "args": {"question": "Which database?"}},
            ],
            "later": [
                {"when": "use postgres", "tool": "report_result",
                 "args": {"status": "success", "summary": "went with postgres"}},
            ],
        }),
    ))
    .await;
    let (plan, _) = accepted_plan(&env).await;
    let task = format!("{plan}/T-0001");
    let sub = format!("{plan}/T-0001/db");
    env.wait("the task agent to idle with its sub-agent", |v| {
        let idle = v
            .agents
            .iter()
            .any(|a| a.id.to_string() == task && a.activity == Activity::Idle);
        let sub_up = v.agents.iter().any(|a| a.id.to_string() == sub);
        (idle && sub_up).then_some(())
    })
    .await;
    std::fs::write(&flag_file, "").unwrap();
    env.wait("the sub-agent to ask and idle", |v| {
        let asked = v.items.iter().any(|i| i.kind == ItemKind::ChildQuestion);
        let idle = v
            .agents
            .iter()
            .any(|a| a.id.to_string() == sub && a.activity == Activity::Idle);
        (asked && idle).then_some(())
    })
    .await;
    std::fs::write(&answer_file, "").unwrap();

    // The question reaches the task agent (its digest names a file, which the
    // fake reads; its `when` matches the question), and it calls answer_child.
    env.screen_has(&task, "TOOL answer_child: answer sent to T-0001/db")
        .await;
    let question = env
        .wait("the question to be answered", |v| {
            v.items
                .iter()
                .find(|i| {
                    i.kind == ItemKind::ChildQuestion
                        && matches!(i.status, Delivery::Answered { .. })
                })
                .cloned()
        })
        .await;
    assert_eq!(question.to, Recipient::Agent(task.parse().unwrap()));
    assert!(matches!(&question.status,
        Delivery::Answered { answer, .. } if answer == "use postgres"));

    // The answer is pasted into the idle sub-agent (path B). The sub-agent
    // reads it and reports, which its `when` allows only on the answer.
    let answer = env
        .wait("the answer to be delivered", |v| {
            v.items
                .iter()
                .find(|i| {
                    i.to == Recipient::Agent(sub.parse().unwrap())
                        && i.body.contains("use postgres")
                        && matches!(i.status, Delivery::Delivered { .. })
                })
                .cloned()
        })
        .await;
    assert!(matches!(
        answer.status,
        Delivery::Delivered {
            via: DeliveryPath::PtyPaste,
            ..
        }
    ));
    env.screen_has(&sub, "TOOL report_result: report recorded")
        .await;
    env.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_exits_without_a_report_hands_its_last_answer_to_the_parent() {
    let last = "I could not finish: the build is red on main.";
    let env = start_fake(
        &[
            (
                "executor",
                executor_script(json!([
                    {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
                ])),
            ),
            (
                "T-0001",
                json!({"first": [
                    {"tool": "get_task", "args": {}},
                    {"say": "Looking at the build first."},
                    {"say": last},
                    {"exit": 0},
                ]}),
            ),
        ],
        Duration::from_secs(600),
    )
    .await;
    let (plan, exec) = accepted_plan(&env).await;
    let tid = format!("{plan}/T-0001");
    // The task agent ends its session without `report_result`; the parent's
    // item carries the last assistant text of its transcript.
    let item = env
        .wait("the exit to reach the executor", |v| {
            v.items
                .iter()
                .find(|i| {
                    i.kind == ItemKind::ChildExited
                        && matches!(&i.to, Recipient::Agent(a) if a.to_string() == exec)
                })
                .cloned()
        })
        .await;
    assert!(item.body.contains(last), "{}", item.body);
    let v = env.view().await;
    let agent = v.agents.iter().find(|a| a.id.to_string() == tid).unwrap();
    let report = agent.report.as_ref().unwrap();
    assert!(report.implicit);
    assert_eq!(report.summary, last);
    // The token sums came from the same read: three assistant messages (a
    // tool call and two texts) at 10 in, 5 out each.
    assert_eq!(agent.tokens.input, 30, "{:?}", agent.tokens);
    assert_eq!(agent.tokens.output, 15);
    // The executor sees it in its terminal: the item was pasted.
    env.screen_has(&exec, "ECHO [cmux]").await;
    env.shutdown().await;
}

// ---------------------------------------------------------------------------
// Security (architecture 9; the index is in tests/security.rs)
// ---------------------------------------------------------------------------

/// Logs row: a session with a prompt, a tool call, a permission request with
/// its answer, a message and a pairing, and not one of their texts or tokens
/// in the diagnostics log, which still records each event.
#[tokio::test(flavor = "multi_thread")]
async fn the_diagnostics_log_holds_no_prompt_tool_input_or_token() {
    let env = start_fake(
        &[(
            "executor",
            json!({
                "first": [
                    {"permission": {"tool": "Bash", "input": {"command": "echo SECRET-TOOL-INPUT"}}},
                    {"tool": "create_task", "args": {"title": "SECRET-TOOL-ARG"}},
                    {"say": "SECRET-ANSWER"},
                ],
                "later": [{"say": "SECRET-LATER-ANSWER"}],
            }),
        )],
        Duration::from_secs(600),
    )
    .await;
    let r = env
        .post(
            route::PLANS,
            &json!({"title": "demo", "repos": [{"path": env.repo.display().to_string()}],
                    "request": "build it SECRET-PROMPT"}),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let plan = r.json::<Created>().unwrap().id;
    let exec = format!("{plan}/executor");
    let held = env
        .wait("a permission item", |v| Env::pending_permissions(v).pop())
        .await;
    env.answer(held.id.0, AnswerAction::Deny, Some("SECRET-DENY-REASON"))
        .await;
    env.screen_has(&exec, "PERM deny: SECRET-DENY-REASON").await;
    env.wait("the executor to idle", |v| {
        v.agents
            .iter()
            .any(|a| a.id.to_string() == exec && a.activity == Activity::Idle)
            .then_some(())
    })
    .await;
    let r = env
        .post(
            &route::agent(&exec.replace('/', "%2F"), "messages"),
            &json!({"text": "SECRET-MESSAGE", "steer": false}),
        )
        .await;
    assert!(r.status < 300, "{}", r.text());
    // A user message reaches the agent as a digest file it is told to read.
    env.screen_has(&exec, "ECHO [cmux] 1 new message").await;

    // A device token, too.
    let start: cmux_proto::api::PairStartResp = env
        .post(route::PAIR_START, &json!({}))
        .await
        .json()
        .unwrap();
    let done: cmux_proto::api::PairCompleteResp = env
        .post(
            route::PAIR_COMPLETE,
            &json!({"code": start.code, "device_name": "SECRET-DEVICE-NAME"}),
        )
        .await
        .json()
        .unwrap();
    let mcp = std::fs::read_to_string(
        agent_dir(&env.app().data_dir, &exec.parse().unwrap()).join("mcp.json"),
    )
    .unwrap();
    let mcp: Value = serde_json::from_str(&mcp).unwrap();
    let agent_token = mcp["mcpServers"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()["env"]["CMUX_TOKEN"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!agent_token.is_empty());
    let log_text = env.app().diag.path();
    let mut log = std::fs::read_to_string(&log_text).unwrap();
    if let Ok(old) = std::fs::read_to_string(log_text.with_extension("jsonl.1")) {
        log += &old;
    }
    for event in ["agent_hook", "agent_tool", "paired"] {
        assert!(
            log.contains(&format!("\"event\":\"{event}\"")),
            "{event}:\n{log}"
        );
    }
    for secret in [
        "SECRET",
        "build it",
        "echo ",
        done.token.as_str(),
        agent_token.as_str(),
    ] {
        assert!(!log.contains(secret), "{secret:?} in the log:\n{log}");
    }
    env.shutdown().await;
}

/// A push stays inside the 4096 bytes every push service must take
/// (RFC 8030 7.2), however long the tool input, and carries a summary of at
/// most 60 characters that leaves the rest of the input out.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_stays_inside_the_web_push_payload_limit() {
    use cmux_proto::api::{NotifyKind, PUSH_SUMMARY_MAX, PushSubscribeReq};
    let long = format!("echo {} SECRET-TAIL", "ü€😀".repeat(2000));
    let env = start_fake(
        &[("executor", permission_script(&[long.as_str()]))],
        Duration::from_secs(600),
    )
    .await;
    let mock = mock_http::MockHttp::start().await;
    let browser = mock_http::Browser::new();
    let sub = PushSubscribeReq {
        endpoint: format!("{}/push/phone", mock.base),
        keys: browser.keys(),
        kinds: vec![NotifyKind::Permission],
    };
    let r = env.post(route::PUSH_SUBSCRIBE, &sub).await;
    assert_eq!(r.status, 204, "{}", r.text());
    env.create_plan().await;
    let hit = mock.wait_for(1).await;
    assert!(hit.body.len() <= 4096, "{} bytes", hit.body.len());
    let p = browser.open(&hit.body);
    assert_eq!(p.kind, NotifyKind::Permission);
    assert!(
        p.summary.chars().count() <= PUSH_SUMMARY_MAX,
        "{}",
        p.summary
    );
    assert!(
        p.summary.starts_with("executor: Bash echo "),
        "{}",
        p.summary
    );
    assert!(!p.summary.contains("SECRET"), "{}", p.summary);
    env.shutdown().await;
}
