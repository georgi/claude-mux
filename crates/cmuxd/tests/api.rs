//! API tests against an in-process server on a real Unix socket and a real
//! TCP port: auth, pairing, idempotency, REST and the store.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::Request;
use cmux_core::ids::TermId;
use cmux_core::model::{Activity, Life};
use cmux_proto::api::{
    Created, DevicesResp, ErrorBody, ErrorCode, PairCompleteReq, PairCompleteResp, PairStartReq,
    PairStartResp, PatchTaskReq, Scope, route,
};
use cmux_proto::view::{StateView, TaskDetail, TaskState};
use cmuxd::agent::AgentRuntime;
use cmuxd::api::auth::PeerCred;
use cmuxd::api::{self, Extra, Transport};
use cmuxd::claude::spawn::SpawnConfig;
use cmuxd::client::{Endpoint, Response};
use cmuxd::config::{Config, ConfigError};
use cmuxd::server::{Server, ServerBuilder, ServerError};
use serde::Serialize;
use tower::ServiceExt;

const CMUXD: &str = env!("CARGO_BIN_EXE_cmuxd");

struct Harness {
    server: Server,
    unix: Endpoint,
    tcp: Endpoint,
    _tmp: tempfile::TempDir,
}

fn config(tmp: &Path) -> Config {
    Config {
        data_dir: Some(tmp.join("data")),
        socket: Some(tmp.join("api.sock")),
        listen: Some("127.0.0.1:0".into()),
        ..Config::default()
    }
}

async fn start_with(tmp: &Path, cfg: Config) -> Result<Server, ServerError> {
    let _ = tmp;
    ServerBuilder::new(cfg).tick(None).start().await
}

/// `examples/fake-claude`, built on demand (as `tests/agents.rs` does).
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
        let ok = std::process::Command::new(cargo)
            .args(["build", "-p", "cmuxd", "--example", "fake-claude"])
            .status()
            .unwrap()
            .success();
        assert!(ok && p.exists(), "cannot build {}", p.display());
    }
    p
}

/// A `claude` that runs the fake with the scripts in `<tmp>/scripts`.
fn fake_claude(tmp: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp.join("scripts");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = tmp.join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nFAKE_CLAUDE_SCRIPTS='{}' exec '{}' \"$@\"\n",
            dir.display(),
            fake_claude_path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// A server whose agents run `claude_bin`.
async fn start_agents(cfg: Config, claude_bin: &Path) -> Result<Server, ServerError> {
    let builder = ServerBuilder::new(cfg.clone()).tick(None);
    let mut spawn = SpawnConfig::new(CMUXD.into(), cfg.socket_path(), cfg.data_dir());
    spawn.claude_bin = claude_bin.display().to_string();
    let rt = AgentRuntime::new(builder.hub(), spawn);
    let routes = rt.clone();
    builder
        .runner(rt.runner())
        .extra_routes(move || Extra {
            unix_only: routes.routes(),
            ..Extra::default()
        })
        .start()
        .await
}

/// Polls `f` for up to 20 s.
async fn eventually<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let until = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(
            std::time::Instant::now() < until,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The executor's life, activity, session id and terminal.
async fn executor(s: &Server) -> (Life, Activity, String, Option<TermId>) {
    s.app
        .actor
        .query(|st| {
            let a = &st.plans[0].agents[0];
            (a.life, a.activity, a.session_id.clone(), a.term)
        })
        .await
        .unwrap()
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let server = start_with(tmp.path(), config(tmp.path())).await.unwrap();
    Harness {
        unix: Endpoint::Unix(server.socket_path.clone()),
        tcp: Endpoint::Tcp(server.tcp_addr.unwrap()),
        server,
        _tmp: tmp,
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn req(ep: &Endpoint, method: &str, path: &str, headers: &[(&str, &str)]) -> Response {
    ep.request(method, path, headers, None).await.unwrap()
}

async fn send(
    ep: &Endpoint,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &impl Serialize,
) -> Response {
    ep.json(method, path, headers, body).await.unwrap()
}

fn plan_req(h: &Harness, title: &str) -> serde_json::Value {
    serde_json::json!({
        "title": title,
        "repos": [{"path": h._tmp.path().display().to_string()}],
        "request": "build it",
    })
}

/// Pairs a device over the socket and TCP; returns its token.
async fn pair(h: &Harness, scope: Scope) -> PairCompleteResp {
    let start: PairStartResp = send(
        &h.unix,
        "POST",
        route::PAIR_START,
        &[],
        &PairStartReq { scope: Some(scope) },
    )
    .await
    .json()
    .unwrap();
    let done = send(
        &h.tcp,
        "POST",
        route::PAIR_COMPLETE,
        &[],
        &PairCompleteReq {
            code: start.code,
            device_name: "test phone".into(),
        },
    )
    .await;
    assert_eq!(done.status, 200, "{}", done.text());
    done.json().unwrap()
}

#[tokio::test]
async fn tcp_needs_a_token_and_the_socket_needs_none() {
    let h = harness().await;
    assert_eq!(req(&h.tcp, "GET", route::STATE, &[]).await.status, 401);
    let bad = bearer("not-a-token");
    let r = req(&h.tcp, "GET", route::STATE, &[("Authorization", &bad)]).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json::<ErrorBody>().unwrap().code, ErrorCode::Unauthorized);
    let r = req(&h.unix, "GET", route::STATE, &[]).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json::<StateView>().unwrap().plans.len(), 0);
    // Responses are never cached.
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let tok = pair(&h, Scope::Full).await.token;
    let a = bearer(&tok);
    let r = req(&h.tcp, "GET", route::STATE, &[("Authorization", &a)]).await;
    assert_eq!(r.status, 200);
    let r = req(&h.tcp, "GET", route::STATUS, &[("Authorization", &a)]).await;
    assert_eq!(r.status, 200);
    h.server.shutdown().await;
}

#[tokio::test]
async fn socket_refuses_a_peer_with_another_uid() {
    let h = harness().await;
    let router = api::router(h.server.app.clone(), Transport::Unix);
    let ours = unsafe { libc::geteuid() };
    let call = |uid: Option<u32>| {
        let mut r = Request::get(route::STATE).body(Body::empty()).unwrap();
        r.extensions_mut().insert(ConnectInfo(PeerCred { uid }));
        router.clone().oneshot(r)
    };
    assert_eq!(call(Some(ours)).await.unwrap().status(), 200);
    assert_eq!(call(Some(ours + 1)).await.unwrap().status(), 403);
    assert_eq!(call(None).await.unwrap().status(), 403);
    // Not even the public pairing route is open to another uid.
    let mut r = Request::post(route::PAIR_COMPLETE)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"code":"x","device_name":"y"}"#))
        .unwrap();
    r.extensions_mut().insert(ConnectInfo(PeerCred {
        uid: Some(ours + 1),
    }));
    assert_eq!(router.oneshot(r).await.unwrap().status(), 403);
    h.server.shutdown().await;
}

#[tokio::test]
async fn pairing_codes_are_single_use_and_expire() {
    let h = harness().await;
    let start: PairStartResp = send(
        &h.unix,
        "POST",
        route::PAIR_START,
        &[],
        &PairStartReq::default(),
    )
    .await
    .json()
    .unwrap();
    assert_eq!(start.code.len(), 8);
    assert_eq!(start.expires_in_secs, 300);
    assert_eq!(start.scope, Scope::Full);
    assert!(start.url.ends_with(&format!("/pair?code={}", start.code)));
    let body = PairCompleteReq {
        code: start.code.clone(),
        device_name: "laptop".into(),
    };
    let ok = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &body).await;
    assert_eq!(ok.status, 200);
    let paired: PairCompleteResp = ok.json().unwrap();
    assert_eq!(paired.scope, Scope::Full);
    assert_eq!(paired.token.len(), 43);
    // Reuse fails.
    let again = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &body).await;
    assert_eq!(again.status, 401);

    // Expiry: a code left past its lifetime is dead.
    let start: PairStartResp = send(
        &h.unix,
        "POST",
        route::PAIR_START,
        &[],
        &PairStartReq::default(),
    )
    .await
    .json()
    .unwrap();
    h.server.app.clock.advance(Duration::from_secs(301));
    let late = PairCompleteReq {
        code: start.code,
        device_name: "late".into(),
    };
    assert_eq!(
        send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &late)
            .await
            .status,
        401
    );
    h.server.shutdown().await;
}

#[tokio::test]
async fn pairing_attempts_are_rate_limited() {
    let h = harness().await;
    let wrong = PairCompleteReq {
        code: "AAAAAAAA".into(),
        device_name: "x".into(),
    };
    for _ in 0..5 {
        let r = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &wrong).await;
        assert_eq!(r.status, 401);
    }
    let r = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &wrong).await;
    assert_eq!(r.status, 429);
    assert!(r.header("retry-after").is_some());
    let body: ErrorBody = r.json().unwrap();
    assert_eq!(body.code, ErrorCode::RateLimited);
    assert!(body.retry_after.unwrap() >= 1);
    // The window slides.
    h.server.app.clock.advance(Duration::from_secs(61));
    let r = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &wrong).await;
    assert_eq!(r.status, 401);
    h.server.shutdown().await;
}

#[tokio::test]
async fn read_scope_cannot_mutate_and_only_full_can_pair() {
    let h = harness().await;
    let tok = pair(&h, Scope::Read).await.token;
    let a = bearer(&tok);
    let auth = [("Authorization", a.as_str())];
    assert_eq!(req(&h.tcp, "GET", route::STATE, &auth).await.status, 200);
    let plan = plan_req(&h, "nope");
    let r = send(&h.tcp, "POST", route::PLANS, &auth, &plan).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.json::<ErrorBody>().unwrap().code, ErrorCode::Forbidden);
    let r = send(
        &h.tcp,
        "POST",
        route::PAIR_START,
        &auth,
        &PairStartReq::default(),
    )
    .await;
    assert_eq!(r.status, 403);
    // The same call is fine from the socket.
    assert_eq!(
        send(&h.unix, "POST", route::PLANS, &[], &plan).await.status,
        201
    );
    h.server.shutdown().await;
}

#[tokio::test]
async fn devices_list_and_revoke() {
    let h = harness().await;
    let a = pair(&h, Scope::Full).await;
    let b = pair(&h, Scope::Read).await;
    let (ha, hb) = (bearer(&a.token), bearer(&b.token));
    let list: DevicesResp = req(&h.tcp, "GET", route::DEVICES, &[("Authorization", &ha)])
        .await
        .json()
        .unwrap();
    assert_eq!(list.devices.len(), 2);
    assert!(
        list.devices
            .iter()
            .find(|d| d.id == a.device_id)
            .unwrap()
            .current
    );
    assert!(
        !list
            .devices
            .iter()
            .find(|d| d.id == b.device_id)
            .unwrap()
            .current
    );
    // The read device cannot revoke; the full one can.
    let path = route::device(&a.device_id);
    let r = req(&h.tcp, "DELETE", &path, &[("Authorization", &hb)]).await;
    assert_eq!(r.status, 403);
    let path = route::device(&b.device_id);
    assert_eq!(
        req(&h.tcp, "DELETE", &path, &[("Authorization", &ha)])
            .await
            .status,
        204
    );
    assert_eq!(
        req(&h.tcp, "DELETE", &path, &[("Authorization", &ha)])
            .await
            .status,
        404
    );
    assert_eq!(
        req(&h.tcp, "GET", route::STATE, &[("Authorization", &hb)])
            .await
            .status,
        401
    );
    h.server.shutdown().await;
}

#[tokio::test]
async fn cookie_sessions_need_the_csrf_header_for_mutations() {
    let h = harness().await;
    let tok = pair(&h, Scope::Full).await.token;
    let a = bearer(&tok);
    let r = req(&h.tcp, "POST", route::SESSION, &[("Authorization", &a)]).await;
    assert_eq!(r.status, 204);
    let set = r.header("set-cookie").unwrap().to_string();
    for flag in ["HttpOnly", "Secure", "SameSite=Strict"] {
        assert!(set.contains(flag), "{set}");
    }
    let cookie = set.split(';').next().unwrap().to_string();
    let c = [("Cookie", cookie.as_str())];
    assert_eq!(req(&h.tcp, "GET", route::STATE, &c).await.status, 200);
    let plan = plan_req(&h, "web");
    let r = send(&h.tcp, "POST", route::PLANS, &c, &plan).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert!(r.json::<ErrorBody>().unwrap().message.contains("X-Cmux"));
    let with = [("Cookie", cookie.as_str()), ("X-Cmux", "1")];
    assert_eq!(
        send(&h.tcp, "POST", route::PLANS, &with, &plan)
            .await
            .status,
        201
    );
    // A cookie cannot start another session; logging out kills it.
    let r = req(&h.tcp, "POST", route::SESSION, &with).await;
    assert_eq!(r.status, 400);
    assert_eq!(
        req(&h.tcp, "DELETE", route::SESSION, &with).await.status,
        204
    );
    assert_eq!(req(&h.tcp, "GET", route::STATE, &c).await.status, 401);
    h.server.shutdown().await;
}

#[tokio::test]
async fn idempotent_retries_replay_the_first_response() {
    let h = harness().await;
    let plan = plan_req(&h, "once");
    let key = [("Idempotency-Key", "k-1")];
    let first = send(&h.unix, "POST", route::PLANS, &key, &plan).await;
    assert_eq!(first.status, 201);
    assert!(first.header("idempotent-replayed").is_none());
    let retry = send(&h.unix, "POST", route::PLANS, &key, &plan).await;
    assert_eq!(retry.status, 201);
    assert_eq!(retry.header("idempotent-replayed"), Some("true"));
    assert_eq!(retry.body, first.body);
    let view: StateView = req(&h.unix, "GET", route::STATE, &[]).await.json().unwrap();
    assert_eq!(
        view.plans.len(),
        1,
        "the retry must not create a second plan"
    );

    // Same key, different request: refused.
    let other = plan_req(&h, "twice");
    let r = send(&h.unix, "POST", route::PLANS, &key, &other).await;
    assert_eq!(r.status, 422);
    assert_eq!(
        r.json::<ErrorBody>().unwrap().code,
        ErrorCode::IdempotencyMismatch
    );
    // Without a key every call acts.
    send(&h.unix, "POST", route::PLANS, &[], &other).await;
    let view: StateView = req(&h.unix, "GET", route::STATE, &[]).await.json().unwrap();
    assert_eq!(view.plans.len(), 2);

    // Errors are replayed too (a 4xx is an answer); keys are per caller.
    let bad = serde_json::json!({"title": " ", "repos": [], "request": "x"});
    let k2 = [("Idempotency-Key", "k-2")];
    let a = send(&h.unix, "POST", route::PLANS, &k2, &bad).await;
    let b = send(&h.unix, "POST", route::PLANS, &k2, &bad).await;
    assert_eq!((a.status, b.status), (400, 400));
    assert_eq!(b.header("idempotent-replayed"), Some("true"));
    let tok = pair(&h, Scope::Full).await.token;
    let auth = bearer(&tok);
    let hdrs = [("Authorization", auth.as_str()), ("Idempotency-Key", "k-1")];
    let r = send(&h.tcp, "POST", route::PLANS, &hdrs, &other).await;
    assert_eq!(r.status, 201);
    assert!(r.header("idempotent-replayed").is_none());
    h.server.shutdown().await;
}

#[tokio::test]
async fn concurrent_retries_run_once() {
    let h = harness().await;
    let plan = plan_req(&h, "race");
    let key = [("Idempotency-Key", "same")];
    let call = || send(&h.unix, "POST", route::PLANS, &key, &plan);
    let (a, b, c, d) = tokio::join!(call(), call(), call(), call());
    let all = [a, b, c, d];
    assert!(all.iter().all(|r| r.status == 201));
    assert!(all.iter().all(|r| r.body == all[0].body));
    let view: StateView = req(&h.unix, "GET", route::STATE, &[]).await.json().unwrap();
    assert_eq!(view.plans.len(), 1);
    h.server.shutdown().await;
}

#[tokio::test]
async fn creating_a_plan_shows_up_in_state() {
    let h = harness().await;
    let created: Created = send(
        &h.unix,
        "POST",
        route::PLANS,
        &[],
        &plan_req(&h, "Add login"),
    )
    .await
    .json()
    .unwrap();
    assert!(created.id.starts_with("P-") && created.id.ends_with("add-login"));
    let view: StateView = req(&h.unix, "GET", route::STATE, &[]).await.json().unwrap();
    assert_eq!(view.plans.len(), 1);
    assert_eq!(view.plans[0].id.to_string(), created.id);
    assert_eq!(view.plans[0].title, "Add login");
    assert_eq!(view.plans[0].repos[0].base_branch, "main");
    assert!(view.rev >= 1);
    // The executor agent exists from the start.
    assert_eq!(view.agents.len(), 1);

    // Tasks.
    let tasks_path = format!("/v1/plans/{}/tasks", created.id);
    let t: Created = send(
        &h.unix,
        "POST",
        &tasks_path,
        &[],
        &serde_json::json!({"title": "Wire it", "criteria": ["it works"]}),
    )
    .await
    .json()
    .unwrap();
    let task_path = route::task(&created.id, &t.id);
    let r = req(&h.unix, "GET", &task_path, &[]).await;
    assert_eq!(r.status, 200, "{task_path} {}", r.text());
    let d: TaskDetail = r.json().unwrap();
    assert_eq!(d.task.title, "Wire it");
    assert_eq!(d.criteria.len(), 1);
    let patch = PatchTaskReq {
        title: Some("Wire it up".into()),
        ..Default::default()
    };
    let d: TaskDetail = send(&h.unix, "PATCH", &task_path, &[], &patch)
        .await
        .json()
        .unwrap();
    assert_eq!(d.task.title, "Wire it up");
    let bad = PatchTaskReq {
        state: Some(TaskState::Merged),
        ..Default::default()
    };
    let r = send(&h.unix, "PATCH", &task_path, &[], &bad).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.json::<ErrorBody>().unwrap().message.starts_with("error:"));
    assert_eq!(
        req(&h.unix, "GET", "/v1/tasks/P-20260101-nope/T-0009", &[])
            .await
            .status,
        404
    );
    assert_eq!(
        req(&h.unix, "GET", "/v1/tasks/garbage/T-9", &[])
            .await
            .status,
        400
    );

    // plan.md round trip.
    let body_path = format!("/v1/plans/{}/body", created.id);
    let r = req(&h.unix, "GET", &body_path, &[]).await;
    assert_eq!(r.json::<serde_json::Value>().unwrap()["text"], "");
    let put = send(
        &h.unix,
        "PUT",
        &body_path,
        &[],
        &serde_json::json!({"text": "# Plan\n"}),
    )
    .await;
    assert_eq!(put.status, 204);
    let r = req(&h.unix, "GET", &body_path, &[]).await;
    assert_eq!(r.json::<serde_json::Value>().unwrap()["text"], "# Plan\n");

    // Lifecycle and answers.
    let pause = format!("/v1/plans/{}/pause", created.id);
    assert_eq!(req(&h.unix, "POST", &pause, &[]).await.status, 204);
    let view: StateView = req(&h.unix, "GET", route::STATE, &[]).await.json().unwrap();
    assert!(view.plans[0].paused);
    let ans = serde_json::json!({"action": "allow"});
    assert_eq!(
        send(&h.unix, "POST", &route::item_answer(99), &[], &ans)
            .await
            .status,
        404
    );
    // The prefixed form the UI shows is the same item.
    assert_eq!(
        send(&h.unix, "POST", "/v1/items/I-99/answer", &[], &ans)
            .await
            .status,
        404
    );
    assert_eq!(
        send(&h.unix, "POST", "/v1/items/I-x/answer", &[], &ans)
            .await
            .status,
        400
    );
    let bad = serde_json::json!({"action": "reply"});
    assert_eq!(
        send(&h.unix, "POST", &route::item_answer(99), &[], &bad)
            .await
            .status,
        400
    );

    // /status.
    let st: cmux_proto::api::StatusResp = req(&h.unix, "GET", route::STATUS, &[])
        .await
        .json()
        .unwrap();
    assert_eq!(st.rev, view.rev);
    assert_eq!(st.server.version, env!("CARGO_PKG_VERSION"));
    h.server.shutdown().await;
}

#[tokio::test]
async fn state_survives_a_restart_and_a_second_server_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let cfg = config(&root);
    let claude = fake_claude(&root);
    // The executor works on its kickoff until the file appears (never).
    std::fs::write(
        root.join("scripts/executor.json"),
        serde_json::json!({"first": [{"wait_file": root.join("never").display().to_string()}]})
            .to_string(),
    )
    .unwrap();
    let server = start_agents(cfg.clone(), &claude).await.unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    let plan = serde_json::json!({
        "title": "persist me",
        "repos": [{"path": root.display().to_string(), "base_branch": "trunk"}],
        "request": "x",
    });
    let created: Created = send(&ep, "POST", route::PLANS, &[], &plan)
        .await
        .json()
        .unwrap();
    let (_, _, session, _) = eventually("the executor to work", async || {
        let e = executor(&server).await;
        (e.0 == Life::Running && e.1 == Activity::Working).then_some(e)
    })
    .await;

    // A second cmuxd on the same data dir refuses to start.
    let mut other = cfg.clone();
    other.socket = Some(tmp.path().join("other.sock"));
    let err = start_with(tmp.path(), other)
        .await
        .err()
        .expect("must refuse");
    assert!(matches!(err, ServerError::Store(_)), "{err}");
    assert!(err.to_string().contains("already running"), "{err}");
    // The first one is unharmed.
    assert_eq!(req(&ep, "GET", route::STATE, &[]).await.status, 200);

    server.shutdown().await;
    let journal = tmp
        .path()
        .join("data/plans")
        .join(&created.id)
        .join("events.jsonl");
    let lines = std::fs::read_to_string(journal).unwrap();
    assert!(lines.contains("client.CreatePlan"), "{lines}");

    // The shutdown killed the process without the engine seeing it exit,
    // as a crash would. The next start finds the executor "running".
    let server = start_agents(cfg, &claude).await.unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    let view: StateView = req(&ep, "GET", route::STATE, &[]).await.json().unwrap();
    assert_eq!(view.plans.len(), 1);
    assert_eq!(view.plans[0].id.to_string(), created.id);
    assert_eq!(view.plans[0].repos[0].base_branch, "trunk");
    assert!(view.rev >= 1);
    // It was working, so it is resumed in its old session with the note.
    let (_, _, resumed, term) = eventually("the executor to be resumed", async || {
        let e = executor(&server).await;
        (e.0 == Life::Running && e.1 == Activity::Working).then_some(e)
    })
    .await;
    assert_eq!(resumed, session);
    let term = server.app.hub.get_term(term.unwrap()).unwrap();
    eventually("the restart note on its screen", async || {
        term.screen_text()
            .contains("KICKOFF: [cmux] resumed after a server restart; check the worktree")
            .then_some(())
    })
    .await;
    server.shutdown().await;
}

#[tokio::test]
async fn paired_devices_survive_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    let s = start_with(tmp.path(), cfg.clone()).await.unwrap();
    let h = Harness {
        unix: Endpoint::Unix(s.socket_path.clone()),
        tcp: Endpoint::Tcp(s.tcp_addr.unwrap()),
        server: s,
        _tmp: tmp,
    };
    let token = pair(&h, Scope::Full).await.token;
    let cfg2 = cfg.clone();
    h.server.shutdown().await;
    let s = ServerBuilder::new(cfg2).tick(None).start().await.unwrap();
    let tcp = Endpoint::Tcp(s.tcp_addr.unwrap());
    let a = bearer(&token);
    assert_eq!(
        req(&tcp, "GET", route::STATE, &[("Authorization", &a)])
            .await
            .status,
        200
    );
    s.shutdown().await;
}

#[tokio::test]
async fn non_loopback_plain_http_is_refused_unless_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(tmp.path());
    cfg.listen = Some("0.0.0.0:0".into());
    let err = start_with(tmp.path(), cfg.clone())
        .await
        .err()
        .expect("must refuse");
    assert!(
        matches!(err, ServerError::Config(ConfigError::PlainHttp(_))),
        "{err}"
    );
    // Nothing was bound or locked by the failed attempt.
    cfg.allow_plain_http = true;
    let s = start_with(tmp.path(), cfg).await.unwrap();
    s.shutdown().await;
}

#[tokio::test]
async fn projects_lists_the_servers_claude_sessions() {
    let h = harness().await;
    let claude = h._tmp.path().join("claude");
    let dir = claude.join("projects/-tmp-demo");
    std::fs::create_dir_all(&dir).unwrap();
    let id = "0b9c2d1e-1111-4222-8333-444455556666";
    std::fs::write(
        dir.join(format!("{id}.jsonl")),
        concat!(
            r#"{"type":"user","cwd":"/tmp/demo","gitBranch":"main","message":{"role":"user","content":"fix the bug"}}"#,
            "\n",
            r#"{"type":"last-prompt","lastPrompt":"and the tests"}"#,
            "\n"
        ),
    )
    .unwrap();
    // A sidechain file (not a 36-char id) is skipped.
    std::fs::write(dir.join("agent-1.jsonl"), "{}\n").unwrap();
    // SAFETY: only this test reads CLAUDE_CONFIG_DIR in this binary.
    unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &claude) };

    let r = req(&h.unix, "GET", route::PROJECTS, &[]).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let list: cmux_proto::api::ProjectsResp = r.json().unwrap();
    assert_eq!(list.sessions.len(), 1, "{list:?}");
    let s = &list.sessions[0];
    assert_eq!(s.id, id);
    assert_eq!(s.cwd, "/tmp/demo");
    assert_eq!(s.title, "fix the bug");
    assert_eq!(s.last_prompt.as_deref(), Some("and the tests"));
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert!(s.modified > 0);

    // Over TCP it needs a token like any other route.
    let r = req(&h.tcp, "GET", route::PROJECTS, &[]).await;
    assert_eq!(r.status, 401);
    h.server.shutdown().await;
}

#[tokio::test]
async fn a_blank_device_name_does_not_spend_the_pairing_code() {
    let h = harness().await;
    let start: PairStartResp = send(
        &h.unix,
        "POST",
        route::PAIR_START,
        &[],
        &PairStartReq::default(),
    )
    .await
    .json()
    .unwrap();
    let blank = PairCompleteReq {
        code: start.code.clone(),
        device_name: " ".into(),
    };
    let r = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &blank).await;
    assert_eq!(r.status, 400);
    let named = PairCompleteReq {
        code: start.code,
        device_name: "phone".into(),
    };
    let r = send(&h.tcp, "POST", route::PAIR_COMPLETE, &[], &named).await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn huge_terminal_sizes_are_refused() {
    let h = harness().await;
    let r = send(
        &h.unix,
        "POST",
        route::TERMINALS,
        &[],
        &serde_json::json!({"kind": "shell", "rows": 60000, "cols": 60000}),
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.text());
}
