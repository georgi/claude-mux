//! The M2 manual acceptance, automated: a plan with two dependent tasks is
//! accepted, the client detaches and re-attaches, and T2 starts after T1
//! reported. A real in-process cmuxd (wired as `cmuxd serve` does it) runs
//! the fake `claude`; the client is the real connection layer and `Mirror`.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command as Proc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cmux_proto::api::{Created, Scope, route};
use cmux_proto::view::{Activity, AgentView, Life, PlanState, StateView, TaskState, TaskView};
use cmuxd::agent::AgentRuntime;
use cmuxd::api::Extra;
use cmuxd::claude::spawn::SpawnConfig;
use cmuxd::config::Config;
use cmuxd::server::ServerBuilder;
use serde_json::{Value, json};

use super::http::{Http, new_key};
use super::servers::Endpoint;
use super::stream::{ConnStatus, Connection, Mirror, StreamConfig, StreamEvent};
use crate::tui::remote_term::RemoteTerm;

const TIMEOUT: Duration = Duration::from_secs(60);

pub(super) fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + TIMEOUT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn git(dir: &Path, args: &[&str]) {
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
pub(super) fn make_repo(root: &Path) -> PathBuf {
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

/// The `cmuxd` binary and the fake `claude`, built once per test process.
pub(super) fn binaries() -> &'static (PathBuf, PathBuf) {
    static BINS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    BINS.get_or_init(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut build = Proc::new(cargo);
        build.args([
            "build",
            "-p",
            "cmuxd",
            "--bin",
            "cmuxd",
            "--example",
            "fake-claude",
        ]);
        let exe = std::env::current_exe().unwrap();
        let profile = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
        if profile.file_name().is_some_and(|n| n == "release") {
            build.arg("--release");
        }
        assert!(build.status().unwrap().success(), "cannot build cmuxd");
        let cmuxd = profile.join("cmuxd");
        let fake = profile.join("examples").join("fake-claude");
        assert!(cmuxd.exists() && fake.exists(), "{cmuxd:?} {fake:?}");
        (cmuxd, fake)
    })
}

/// A TCP relay the test can cut and restore: the client's "network".
struct Relay {
    addr: SocketAddr,
    up: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<TcpStream>>>,
}

impl Relay {
    fn start(to: SocketAddr) -> Relay {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let up = Arc::new(AtomicBool::new(true));
        let conns: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let (u, c) = (up.clone(), conns.clone());
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                if !u.load(Ordering::SeqCst) {
                    let _ = client.shutdown(Shutdown::Both);
                    continue;
                }
                let Ok(server) = TcpStream::connect(to) else {
                    continue;
                };
                for (mut from, mut into) in [
                    (client.try_clone().unwrap(), server.try_clone().unwrap()),
                    (server.try_clone().unwrap(), client.try_clone().unwrap()),
                ] {
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 16 * 1024];
                        while let Ok(n) = from.read(&mut buf) {
                            if n == 0 || into.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                        let _ = from.shutdown(Shutdown::Both);
                        let _ = into.shutdown(Shutdown::Both);
                    });
                }
                c.lock().unwrap().extend([client, server]);
            }
        });
        Relay { addr, up, conns }
    }

    fn cut(&self) {
        self.up.store(false, Ordering::SeqCst);
        for c in self.conns.lock().unwrap().drain(..) {
            let _ = c.shutdown(Shutdown::Both);
        }
    }

    fn restore(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

fn executor_script() -> Value {
    json!({
        "first": [
            {"tool": "create_task", "args": {"title": "first", "criteria": ["it works"]}},
            {"tool": "create_task", "args": {"title": "second", "deps": ["T-0001"]}},
            {"tool": "update_plan", "args": {"state": "proposed"}},
        ],
        "later": [
            // The dependency: T-0002 waits for T-0001 to be merged.
            {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0002"}},
            {"when": "plan was accepted", "tool": "spawn_task", "args": {"task": "T-0001"}},
            // T-0001's report arrives as a prompt; merge it, then start T-0002.
            {"when": "it did the first thing", "tool": "transition_task",
             "args": {"task": "T-0001", "to": "merged"}},
            {"when": "it did the first thing", "tool": "spawn_task", "args": {"task": "T-0002"}},
        ],
    })
}

fn t1_script(gate: &Path) -> Value {
    json!({"first": [
        {"wait_file": gate.display().to_string()},
        {"tool": "check_criterion", "args": {"task": "T-0001", "criterion": 1}},
        {"tool": "report_result", "args": {"status": "success", "summary": "it did the first thing"}},
        {"exit": 0},
    ]})
}

fn agent<'a>(sv: &'a StateView, suffix: &str) -> Option<&'a AgentView> {
    sv.agents
        .iter()
        .find(|a| a.id.to_string().ends_with(suffix))
}

fn task(sv: &StateView, n: u32) -> Option<&TaskView> {
    sv.tasks
        .iter()
        .find(|t| t.id.to_string() == format!("T-{n:04}"))
}

#[test]
fn plan_with_dependent_tasks_survives_detach_and_reattach() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let repo = make_repo(&root);
    let (cmuxd_exe, fake) = binaries().clone();
    let gate = root.join("t1-go");

    // The fake claude and its scripts.
    let scripts = root.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("executor.json"), executor_script().to_string()).unwrap();
    std::fs::write(scripts.join("T-0001.json"), t1_script(&gate).to_string()).unwrap();
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

    // The server, wired the way `cmuxd serve` does it.
    let rt = tokio::runtime::Runtime::new().unwrap();
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
    let server = rt
        .block_on(
            builder
                .transcripts(agents.transcripts().clone())
                .runner(agents.runner())
                .extra_routes(move || Extra {
                    unix_only: routes.routes(),
                    ..Extra::default()
                })
                .start(),
        )
        .unwrap();

    // Create the plan over REST and wait for the executor's proposal.
    let local = Http::new(Endpoint::Unix(server.socket_path.clone()));
    let created: Created = local
        .post(
            route::PLANS,
            &json!({"title": "demo", "repos": [{"path": repo.display().to_string()}],
                    "request": "build it"}),
        )
        .unwrap();
    let plan = created.id;
    let server_view = || rt.block_on(server.app.actor.state_view()).unwrap();
    let wait_view = |what: &str, f: &dyn Fn(&StateView) -> bool| {
        let end = Instant::now() + TIMEOUT;
        loop {
            let v = server_view();
            if f(&v) {
                return;
            }
            if Instant::now() >= end {
                let mut screens = String::new();
                for a in &v.agents {
                    let text = a
                        .term
                        .and_then(|t| server.app.hub.get_term(t))
                        .map(|t| t.screen_text());
                    screens += &format!("--- {}\n{}\n", a.id, text.unwrap_or_default());
                }
                panic!(
                    "timed out waiting for {what}\n{:#?}\n{:#?}\n{:#?}\n{screens}",
                    v.agents, v.tasks, v.items
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait_view("the executor to propose and idle", &|v| {
        v.plans.iter().any(|p| p.state == PlanState::Proposed)
            && agent(v, "executor").is_some_and(|a| a.activity == Activity::Idle)
    });
    assert_eq!(
        task(&server_view(), 2).unwrap().deps,
        vec![task(&server_view(), 1).unwrap().id]
    );

    // The client attaches (over TCP, through the relay) and watches the
    // executor's terminal.
    let relay = Relay::start(server.tcp_addr.unwrap());
    let token = server.app.devices.create("test", Scope::Full).unwrap().1;
    let events: Arc<Mutex<Vec<StreamEvent>>> = Arc::default();
    let mirror: Arc<Mutex<Mirror>> = Arc::default();
    let (sink, m) = (events.clone(), mirror.clone());
    let mut conn = Connection::spawn(
        StreamConfig {
            endpoint: Endpoint::Remote {
                url: format!("http://{}", relay.addr),
                token,
            },
            info: "cmux acceptance".into(),
        },
        move |e| {
            m.lock().unwrap().apply(&e);
            sink.lock().unwrap().push(e);
        },
    );
    let exec_term = wait("the executor terminal in the mirror", || {
        agent(&mirror.lock().unwrap().sv, "executor").and_then(|a| a.term)
    });
    let rterm = RemoteTerm::new(conn.handle(), exec_term, 40, 120);
    // Everything the client has: scrollback, then the screen, one row per
    // line so no row is read twice. Rows always end in a newline: vt100 drops
    // wrap flags whenever the width changes, so joining wrapped rows would
    // make the same bytes read differently before and after a resize.
    let screen = |rt: &RemoteTerm| {
        let mut p = rt.parser.lock().unwrap();
        let (rows, cols) = p.screen().size();
        p.screen_mut().set_scrollback(usize::MAX);
        let history = p.screen().scrollback();
        let mut lines = Vec::new();
        for off in (1..=history).rev() {
            p.screen_mut().set_scrollback(off);
            lines.extend(p.screen().rows(0, cols).next());
        }
        p.screen_mut().set_scrollback(0);
        lines.extend(p.screen().rows(0, cols));
        debug_assert_eq!(lines.len(), history + rows as usize);
        lines.join("\n")
    };
    let count = |hay: &str, needle: &str| hay.matches(needle).count();

    // Accept. T-0002 is refused for its open dependency; T-0001 starts.
    local
        .request(
            "POST",
            &format!("/v1/plans/{plan}/accept"),
            None,
            Some(&new_key()),
        )
        .unwrap();
    wait("the executor to refuse T-0002", || {
        screen(&rterm)
            .contains("depends on unmerged tasks")
            .then_some(())
    });
    wait_view("T-0001 to run", &|v| {
        agent(v, "T-0001").is_some_and(|a| a.life == Life::Running)
    });
    {
        let v = server_view();
        assert!(agent(&v, "T-0002").is_none(), "T-0002 spawned too early");
        assert_eq!(task(&v, 2).unwrap().state, TaskState::Todo);
        assert!(agent(&v, "T-0001").unwrap().report.is_none());
    }

    // Detach.
    let welcomes = || {
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, StreamEvent::Welcome { .. }))
            .count()
    };
    assert_eq!(welcomes(), 1);
    let before = screen(&rterm);
    relay.cut();
    wait("the client to notice", || {
        (conn.status() != ConnStatus::Connected).then_some(())
    });
    let rev_at_detach = mirror.lock().unwrap().sv.rev;

    // While detached T1 reports; the executor merges it and spawns T2.
    std::fs::write(&gate, "").unwrap();
    wait_view("T-0002 to be started by the executor", &|v| {
        agent(v, "T-0002").is_some_and(|a| a.life == Life::Running)
    });
    {
        let v = server_view();
        assert!(
            agent(&v, "T-0001").unwrap().report.is_some(),
            "T-0002 started before T-0001 reported"
        );
        assert_eq!(task(&v, 1).unwrap().state, TaskState::Merged);
        assert_eq!(
            mirror.lock().unwrap().sv.rev,
            rev_at_detach,
            "the mirror moved while detached"
        );
    }

    // Re-attach: the client resumes from its last rev and seq.
    relay.restore();
    wait("the second welcome", || {
        (welcomes() >= 2 && conn.status() == ConnStatus::Connected).then_some(())
    });
    wait("the mirror to catch up", || {
        let m = mirror.lock().unwrap();
        let t1 = agent(&m.sv, "T-0001")?;
        let t2 = agent(&m.sv, "T-0002")?;
        (t1.report.is_some()
            && matches!(t1.life, Life::Exited { .. } | Life::Stopped)
            && t2.life == Life::Running
            && task(&m.sv, 1)?.state == TaskState::Merged
            && m.sv.rev == server_view().rev)
            .then_some(())
    });

    // The terminal caught up from its seq: the report arrived, nothing twice.
    wait("the report on the executor's screen", || {
        screen(&rterm).contains("ECHO [cmux]").then_some(())
    });
    let after = screen(&rterm);
    assert!(after.contains(before.trim_end()), "{before}\n----\n{after}");
    for marker in ["TOOLS: ", "KICKOFF: ", "depends on unmerged tasks"] {
        assert_eq!(count(&after, marker), 1, "{marker:?} duplicated:\n{after}");
    }

    // One state snapshot and one terminal snapshot in total: both resumed.
    let evs = events.lock().unwrap().clone();
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, StreamEvent::State(_)))
            .count(),
        1
    );
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, StreamEvent::TermReset(_)))
            .count(),
        1
    );

    drop(rterm);
    conn.shutdown();
    rt.block_on(server.shutdown());
}
