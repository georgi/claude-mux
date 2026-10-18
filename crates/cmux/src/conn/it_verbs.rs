//! The CLI verbs against a real in-process cmuxd running the fake `claude`
//! (wired as in `it_plan`). Each test drives `verbs::run` the way `main`
//! does and checks the exit code, stdout and the stderr line.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cmux_proto::api::{Created, route};
use cmux_proto::view::{
    Activity, AgentView, Delivery, ItemKind, ItemView, Life, PlanState, StateView,
};
use cmuxd::agent::AgentRuntime;
use cmuxd::api::Extra;
use cmuxd::claude::spawn::SpawnConfig;
use cmuxd::config::Config;
use cmuxd::server::{Server, ServerBuilder};
use serde_json::{Value, json};

use super::http::Http;
use super::it_plan::{binaries, make_repo, wait};
use super::servers::Endpoint;
use super::verbs::{self, VerbError};
use crate::tui::state::Node;

const TIMEOUT: Duration = Duration::from_secs(60);

struct Fx {
    server: Server,
    rt: tokio::runtime::Runtime,
    root: PathBuf,
    repo: PathBuf,
    _tmp: tempfile::TempDir,
}

/// What a verb did: exit code, stdout, the stderr line.
#[derive(Debug)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// A `Write` the test can read while a verb still writes (for `tail`).
#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn run_on(ep: Endpoint, verb: &str, args: &[&str], out: &mut dyn Write) -> Out {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = verbs::run(verb, &args, || Ok(ep), out);
    let (code, stderr) = match r {
        Ok(()) => (0, String::new()),
        Err(e) => (e.code(), e.line()),
    };
    Out {
        code,
        stdout: String::new(),
        stderr,
    }
}

impl Fx {
    /// A server whose fake claude runs `executor` for the executor; with
    /// `plan`, a plan "demo" with that request is created over REST.
    fn start(executor: Value, plan: bool) -> (Fx, Option<String>) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = make_repo(&root);
        let (cmuxd_exe, fake) = binaries().clone();
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("executor.json"), executor.to_string()).unwrap();
        let bin = root.join("claude");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\nFAKE_CLAUDE_SCRIPTS='{}' exec '{}' \"$@\"\n",
                scripts.display(),
                fake.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = {
            let _guard = rt.enter();
            let config = Config {
                data_dir: Some(root.join("data")),
                socket: Some(root.join("api.sock")),
                listen: Some("127.0.0.1:0".into()),
                ..Config::default()
            };
            let builder = ServerBuilder::new(config.clone()).tick(None);
            let mut spawn = SpawnConfig::new(cmuxd_exe, config.socket_path(), config.data_dir());
            spawn.claude_bin = bin.display().to_string();
            spawn.permission_wait = Duration::from_secs(600);
            let agents = AgentRuntime::new(builder.hub(), spawn);
            let routes = agents.clone();
            rt.block_on(
                builder
                    .runner(agents.runner())
                    .extra_routes(move || Extra {
                        unix_only: routes.routes(),
                        ..Extra::default()
                    })
                    .start(),
            )
            .unwrap()
        };
        let fx = Fx {
            server,
            rt,
            root,
            repo,
            _tmp: tmp,
        };
        let id = plan.then(|| {
            let c: Created = fx
                .http()
                .post(
                    route::PLANS,
                    &json!({"title": "demo", "repos": [{"path": fx.repo.display().to_string()}],
                            "request": "build it"}),
                )
                .unwrap();
            c.id
        });
        (fx, id)
    }

    fn ep(&self) -> Endpoint {
        Endpoint::Unix(self.server.socket_path.clone())
    }

    fn http(&self) -> Http {
        Http::new(self.ep())
    }

    fn run(&self, verb: &str, args: &[&str]) -> Out {
        let mut buf = Vec::new();
        let mut o = run_on(self.ep(), verb, args, &mut buf);
        o.stdout = String::from_utf8(buf).unwrap();
        o
    }

    /// Runs a verb that must succeed; returns its stdout.
    fn ok(&self, verb: &str, args: &[&str]) -> String {
        let o = self.run(verb, args);
        assert_eq!(o.code, 0, "cmux {verb} {args:?}: {o:?}");
        o.stdout
    }

    fn view(&self) -> StateView {
        self.rt
            .block_on(self.server.app.actor.state_view())
            .unwrap()
    }

    fn wait_view(&self, what: &str, f: impl Fn(&StateView) -> bool) -> StateView {
        let end = Instant::now() + TIMEOUT;
        loop {
            let v = self.view();
            if f(&v) {
                return v;
            }
            if Instant::now() >= end {
                panic!(
                    "timed out waiting for {what}\n{:#?}\n{:#?}\n{}",
                    v.agents,
                    v.items,
                    self.screens(&v)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn screens(&self, v: &StateView) -> String {
        v.agents
            .iter()
            .map(|a| {
                let text = a
                    .term
                    .and_then(|t| self.server.app.hub.get_term(t))
                    .map(|t| t.screen_text());
                format!("--- {}\n{}\n", a.id, text.unwrap_or_default())
            })
            .collect()
    }

    fn executor<'a>(&self, v: &'a StateView) -> Option<&'a AgentView> {
        v.agents
            .iter()
            .find(|a| a.id.path().to_string() == "executor")
    }

    fn executor_screen(&self) -> String {
        let v = self.view();
        self.executor(&v)
            .and_then(|a| a.term)
            .and_then(|t| self.server.app.hub.get_term(t))
            .map(|t| t.screen_text())
            .unwrap_or_default()
    }

    fn wait_screen(&self, what: &str) {
        let end = Instant::now() + TIMEOUT;
        while !self.executor_screen().contains(what) {
            assert!(
                Instant::now() < end,
                "timed out waiting for {what:?} on\n{}",
                self.executor_screen()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// JSON with the run's paths, plan dates and times made stable.
    fn stable(&self, plan: &str, json: &str) -> String {
        let mut v: Value = serde_json::from_str(json).unwrap();
        scrub(&mut v);
        serde_json::to_string_pretty(&v)
            .unwrap()
            .replace(&self.root.display().to_string(), "[ROOT]")
            .replace(plan, "P-[DATE]-demo")
    }

    fn shutdown(self) {
        self.rt.block_on(self.server.shutdown());
    }
}

/// Zeroes the times a run cannot reproduce.
fn scrub(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if matches!(k.as_str(), "created" | "at" | "idle_since") && !x.is_null() {
                    *x = json!(0);
                } else {
                    scrub(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

fn human_items(v: &StateView) -> Vec<&ItemView> {
    verbs::needs_you(v)
}

fn permission<'a>(v: &'a StateView, cmd: &str) -> Option<&'a ItemView> {
    human_items(v).into_iter().find(|i| {
        matches!(&i.kind, ItemKind::Permission { input_summary, .. } if input_summary.contains(cmd))
    })
}

fn planning_script() -> Value {
    json!({"first": [
        {"tool": "create_task", "args": {"title": "first", "criteria": ["it works", "it is fast"]}},
        {"tool": "create_task", "args": {"title": "second", "deps": ["T-0001"]}},
        {"tool": "update_plan", "args": {"state": "proposed"}},
        {"tool": "ask_user", "args": {"question": "Which database?"}},
        {"permission": {"tool": "Bash", "input": {"command": "cargo test"}}},
    ]})
}

#[test]
fn listing_verbs_print_tables_and_json() {
    let (fx, plan) = Fx::start(planning_script(), true);
    let plan = plan.unwrap();
    fx.wait_view("the question and the permission", |v| {
        permission(v, "cargo test").is_some()
            && human_items(v).iter().any(|i| i.kind == ItemKind::Question)
    });

    // plans
    let table = fx.ok("plans", &[]);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 2, "{table}");
    assert!(lines[0].starts_with("PLAN "), "{table}");
    assert!(
        lines[1].starts_with(&plan) && lines[1].contains("proposed") && lines[1].ends_with("demo"),
        "{table}"
    );
    let json = fx.ok("plans", &["--json"]);
    let _: Vec<cmux_proto::view::PlanView> = serde_json::from_str(&json).unwrap();
    insta::assert_snapshot!("plans_json", fx.stable(&plan, &json));

    // board, by title slug, by the slug part of the id and by id
    let table = fx.ok("board", &["demo"]);
    assert!(table.contains("T-0001  todo"), "{table}");
    assert!(table.contains("0/2"), "{table}");
    assert!(table.lines().last().unwrap().ends_with("second"), "{table}");
    let json = fx.ok("board", &[&plan, "--json"]);
    let _: Vec<cmux_proto::view::TaskView> = serde_json::from_str(&json).unwrap();
    assert_eq!(fx.ok("board", &["demo", "--json"]), json);
    insta::assert_snapshot!("board_json", fx.stable(&plan, &json));

    // inbox
    let table = fx.ok("inbox", &[]);
    assert!(table.starts_with("ITEM"), "{table}");
    assert!(table.contains("Which database?"), "{table}");
    assert!(table.contains("Bash: cargo test"), "{table}");
    let json = fx.ok("inbox", &["--json"]);
    let _: Vec<ItemView> = serde_json::from_str(&json).unwrap();
    insta::assert_snapshot!("inbox_json", fx.stable(&plan, &json));

    // A plan that does not exist is a usage error.
    let o = fx.run("board", &["nope"]);
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stderr.contains("no plan \"nope\""), "{o:?}");
    fx.shutdown();
}

#[test]
fn allow_deny_and_reply_answer_items() {
    let (fx, _) = Fx::start(
        json!({"first": [
            {"tool": "ask_user", "args": {"question": "Which database?"}},
            {"permission": {"tool": "Bash", "input": {"command": "echo one"}}},
            {"permission": {"tool": "Bash", "input": {"command": "echo two"}}},
            {"permission": {"tool": "Bash", "input": {"command": "echo three"}}},
        ]}),
        true,
    );
    let item = |cmd: &str| {
        let v = fx.wait_view(cmd, |v| permission(v, cmd).is_some());
        permission(&v, cmd).unwrap().id.to_string()
    };

    let one = item("echo one");
    assert_eq!(fx.ok("allow", &[&one]), format!("allowed {one}\n"));
    fx.wait_screen("PERM allow");

    let two = item("echo two");
    assert_eq!(
        fx.ok("allow", &[&two, "--always"]),
        format!("allowed always {two}\n")
    );

    let three = item("echo three");
    // The number alone works too.
    let n = three.trim_start_matches("I-");
    assert_eq!(
        fx.ok("deny", &[n, "not", "now"]),
        format!("denied {three}\n")
    );
    fx.wait_screen("PERM deny: not now");

    let v = fx.view();
    let q = v
        .items
        .iter()
        .find(|i| i.kind == ItemKind::Question)
        .unwrap()
        .id
        .to_string();
    assert_eq!(fx.ok("reply", &[&q, "Postgres"]), format!("answered {q}\n"));
    let v = fx.view();
    for id in [&one, &two, &three, &q] {
        let i = v.items.iter().find(|i| &i.id.to_string() == id).unwrap();
        assert!(
            matches!(i.status, Delivery::Answered { .. }),
            "{id}: {:?}",
            i.status
        );
    }

    // A refused action: the item is closed. The server's reason, one line, exit 2.
    let o = fx.run("allow", &[&one]);
    assert_eq!(o.code, 2, "{o:?}");
    assert!(
        o.stderr.starts_with("error: ")
            && o.stderr.contains(&format!("{one} was already answered")),
        "{o:?}"
    );
    assert!(!o.stderr.contains('\n'));
    assert!(o.stdout.is_empty());
    // Reply to a permission: refused with what is allowed.
    let o = fx.run("reply", &[&one, "x"]);
    assert_eq!(o.code, 2, "{o:?}");
    fx.shutdown();
}

#[test]
fn new_plan_accept_and_pause() {
    let (fx, _) = Fx::start(json!({"first": []}), false);
    let repo = fx.repo.display().to_string();
    let id = fx.ok("new-plan", &["-r", &repo, "Auth refactor"]);
    let id = id.trim().to_string();
    assert!(
        id.starts_with("P-") && id.ends_with("-auth-refactor"),
        "{id}"
    );
    let v = fx.view();
    let p = v.plans.iter().find(|p| p.id.as_str() == id).unwrap();
    assert_eq!(p.state, PlanState::Draft);
    assert_eq!(p.repos[0].path, fx.repo);
    // Accept needs a task.
    let _: Created = fx
        .http()
        .post(&route::plan(&id, "tasks"), &json!({"title": "one"}))
        .unwrap();

    assert_eq!(
        fx.ok("accept", &["auth-refactor"]),
        format!("accepted {id}\n")
    );
    assert_eq!(fx.ok("pause", &[&id]), format!("paused {id}\n"));
    let v = fx.view();
    let p = v.plans.iter().find(|p| p.id.as_str() == id).unwrap();
    assert_eq!(p.state, PlanState::Accepted);
    assert!(p.paused);
    assert!(fx.ok("plans", &[]).contains("accepted (paused)"));

    // Accepting twice is refused by the server: exit 2 and its reason.
    let o = fx.run("accept", &["auth-refactor"]);
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.starts_with("error: "), "{o:?}");
    assert!(o.stderr.contains("accepted"), "{o:?}");

    // A repository the server cannot use: refused, exit 2.
    let o = fx.run("new-plan", &["-r", "/nonexistent/repo", "x"]);
    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("/nonexistent/repo"), "{o:?}");
    // No repository at all: a usage error before any request.
    assert_eq!(fx.run("new-plan", &["x"]).code, 1);

    // Two plans with the same title: the slug is ambiguous, the id is not.
    let id2 = fx.ok("new-plan", &["-r", &repo, "Auth refactor"]);
    let o = fx.run("pause", &["auth-refactor"]);
    assert_eq!(o.code, 1, "{o:?}");
    assert!(o.stderr.contains("matches several plans"), "{o:?}");
    fx.ok("board", &[id2.trim()]);
    fx.shutdown();
}

#[test]
fn send_tail_stop_resume_and_attach() {
    let (fx, plan) = Fx::start(json!({"first": []}), true);
    let plan = plan.unwrap();
    fx.wait_view("the executor to idle", |v| {
        fx.executor(v)
            .is_some_and(|a| a.life == Life::Running && a.activity == Activity::Idle)
    });

    // tail streams the executor's terminal until it ends.
    let buf = Shared::default();
    let tail = {
        let (ep, mut out) = (fx.ep(), buf.clone());
        std::thread::spawn(move || run_on(ep, "tail", &["demo"], &mut out))
    };
    wait("the snapshot", || {
        buf.text().contains("KICKOFF: ").then_some(())
    });

    let out = fx.ok("send", &["demo/executor", "hello from the cli"]);
    assert_eq!(out, format!("queued for {plan}/executor\n"));
    // Messages arrive as a digest pointer, pasted and echoed by the fake.
    let echoes = |b: &Shared| b.text().matches("ECHO [cmux] 1 new message").count();
    wait("the echo in tail", || (echoes(&buf) == 1).then_some(()));
    fx.wait_view("the executor to idle again", |v| {
        fx.executor(v).is_some_and(|a| a.activity == Activity::Idle)
    });
    let out = fx.ok("send", &["demo", "steer here", "--steer"]);
    assert_eq!(out, format!("steered {plan}/executor\n"));
    wait("the steer in tail", || (echoes(&buf) == 2).then_some(()));
    let v = fx.view();
    let mut kinds: Vec<(ItemKind, &str)> = v
        .items
        .iter()
        .map(|i| (i.kind.clone(), i.body.as_str()))
        .collect();
    kinds.sort_by_key(|(_, b)| b.to_string());
    assert_eq!(
        kinds,
        [
            (ItemKind::UserMessage, "hello from the cli"),
            (ItemKind::Steer, "steer here")
        ]
    );

    // attach resolves its agent from the state.
    let node = verbs::attach_target(&fx.http(), Some("demo/executor")).unwrap();
    assert_eq!(node, Some(Node::Executor(plan.parse().unwrap())));
    assert_eq!(verbs::attach_target(&fx.http(), None), Ok(None));
    assert_eq!(fx.ok("attach", &["demo"]), "");
    assert_eq!(fx.run("attach", &["demo/T-0009"]).code, 1);

    assert_eq!(
        fx.ok("stop", &["demo"]),
        format!("stopped {plan}/executor\n")
    );
    let t = tail.join().unwrap();
    assert_eq!(t.code, 0, "{t:?}");
    fx.wait_view("the executor to stop", |v| {
        fx.executor(v)
            .is_some_and(|a| !matches!(a.life, Life::Running | Life::Starting))
    });
    // The ended terminal stays: tail prints its last screen and returns.
    let o = fx.run("tail", &["demo"]);
    assert_eq!(o.code, 0, "{o:?}");
    assert!(
        o.stdout.contains("ECHO [cmux] 1 new message (I-1)"),
        "{o:?}"
    );

    assert_eq!(
        fx.ok("resume", &["demo/executor"]),
        format!("resumed {plan}/executor\n")
    );
    fx.wait_view("the executor to run again", |v| {
        fx.executor(v).is_some_and(|a| a.life == Life::Running)
    });
    fx.shutdown();
}

#[test]
fn exit_codes_for_bad_paths_and_a_missing_server() {
    let (fx, _) = Fx::start(json!({"first": []}), true);
    fx.wait_view("the executor", |v| fx.executor(v).is_some());

    // Bad agent paths: exit 1 with one line, nothing on stdout.
    for (path, msg) in [
        ("demo/T-1", "bad agent path"),
        ("demo/T-0001/Bad_Name", "bad agent path"),
        ("demo/T-0007", "no agent"),
        ("nope/executor", "no plan"),
    ] {
        let o = fx.run("send", &[path, "hi"]);
        assert_eq!((o.code, o.stdout.as_str()), (1, ""), "{path}: {o:?}");
        assert!(o.stderr.contains(msg), "{path}: {o:?}");
        assert!(o.stderr.starts_with("error: ") && !o.stderr.contains('\n'));
    }
    assert_eq!(fx.run("stop", &["demo/x/y/z"]).code, 1);
    assert_eq!(fx.run("allow", &["I-nope"]).code, 1);
    fx.shutdown();

    // No server listening: exit 2.
    let gone = Endpoint::Unix(Path::new("/nonexistent/cmuxd.sock").to_path_buf());
    for (verb, args) in [
        ("plans", &[][..]),
        ("inbox", &["--json"][..]),
        ("send", &["demo", "hi"][..]),
    ] {
        let o = run_on(gone.clone(), verb, args, &mut Vec::new());
        assert_eq!(o.code, 2, "{verb}: {o:?}");
        assert!(o.stderr.contains("connection failed"), "{o:?}");
    }
    // No server configured at all: also exit 2, from the resolver.
    let r = verbs::run(
        "plans",
        &[],
        || Err(VerbError::Server("no server".into())),
        &mut Vec::new(),
    );
    assert_eq!(r.unwrap_err().code(), 2);
    // A usage error wins over a missing server: no connection is tried.
    let o = run_on(gone, "plans", &["extra"], &mut Vec::new());
    assert_eq!(o.code, 1, "{o:?}");
}
