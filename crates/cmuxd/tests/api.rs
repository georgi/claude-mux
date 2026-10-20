//! API tests against an in-process server on a real Unix socket and a real
//! TCP port: auth, pairing, idempotency, REST and the store.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::Request;
use cmux_core::ids::TermId;
use cmux_core::model::{Activity, Level, Life};
use cmux_proto::api::{
    Created, DevicesResp, ErrorBody, ErrorCode, PairCompleteReq, PairCompleteResp, PairStartReq,
    PairStartResp, PatchTaskReq, RepoInfo, ReposResp, Scope, SettingsResp, TranscriptResp, route,
};
use cmux_proto::view::{ChatEntry, ChatKind, StateView, TaskDetail, TaskState};
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
        .transcripts(rt.transcripts().clone())
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
async fn one_transcript_entry_comes_back_uncut() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let claude = fake_claude(&root);
    let long: String = (0..200).map(|i| format!("line {i:03}\n")).collect();
    std::fs::write(
        root.join("scripts/executor.json"),
        serde_json::json!({"first": [
            {"think": "Run the tests first."},
            {"call": "Bash", "input": {"command": "cargo test", "description": "Test"},
             "result": long},
            {"call": "Edit", "input": {"file_path": "/a.rs", "old_string": "x", "new_string": "y"},
             "result": "String to replace not found in file.", "is_error": true},
            {"say": "Tests ran."},
            {"wait_file": root.join("never").display().to_string()},
        ]})
        .to_string(),
    )
    .unwrap();
    let server = start_agents(config(&root), &claude).await.unwrap();
    let h = Harness {
        unix: Endpoint::Unix(server.socket_path.clone()),
        tcp: Endpoint::Tcp(server.tcp_addr.unwrap()),
        server,
        _tmp: tmp,
    };
    let ep = h.unix.clone();
    let plan = serde_json::json!({
        "title": "transcript",
        "repos": [{"path": root.display().to_string()}],
        "request": "x",
    });
    let created: Created = send(&ep, "POST", route::PLANS, &[], &plan)
        .await
        .json()
        .unwrap();
    let agent = format!("{}%2Fexecutor", created.id);
    let entries = eventually("the transcript to hold the turn", async || {
        let r: TranscriptResp = req(&ep, "GET", &route::transcript(&agent), &[])
            .await
            .json()
            .ok()?;
        r.entries
            .iter()
            .any(|e| e.text == "Tests ran.")
            .then_some(r.entries)
    })
    .await;
    let kinds: Vec<ChatKind> = entries.iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&ChatKind::Thinking), "{kinds:?}");
    let bash = entries
        .iter()
        .find(|e| e.kind == ChatKind::ToolResult && !e.is_error)
        .unwrap();
    let call = entries
        .iter()
        .find(|e| e.kind == ChatKind::ToolCall && e.call_id == bash.call_id)
        .unwrap();
    assert_eq!(
        call.tool.as_ref().unwrap().input.as_ref().unwrap()["command"],
        "cargo test"
    );
    assert_eq!(bash.text.chars().count(), 400);
    assert_eq!(bash.full_len, Some(long.chars().count() as u32));
    let error = entries.iter().find(|e| e.is_error).unwrap();
    assert_eq!(error.tool.as_ref().unwrap().name, "Edit");
    let said = entries.iter().find(|e| e.text == "Tests ran.").unwrap();
    assert_eq!(said.model.as_deref(), Some("fake"));

    // The one entry, uncut.
    let r = req(
        &ep,
        "GET",
        &route::transcript_entry(&agent, &bash.uuid),
        &[],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let full: ChatEntry = r.json().unwrap();
    assert_eq!(full.uuid, bash.uuid);
    assert_eq!(full.text, long);
    assert_eq!(full.full_len, None);
    assert_eq!(full.call_id, bash.call_id);
    // Over TCP with a token as well.
    let token = pair(&h, Scope::Read).await.token;
    let r = req(
        &h.tcp,
        "GET",
        &route::transcript_entry(&agent, &bash.uuid),
        &[("Authorization", &bearer(&token))],
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    // Unknown entries and agents are 404.
    let r = req(&ep, "GET", &route::transcript_entry(&agent, "nope"), &[]).await;
    assert_eq!(r.status, 404);
    let r = req(
        &ep,
        "GET",
        &route::transcript_entry(&format!("{}%2FT-0099", created.id), "x"),
        &[],
    )
    .await;
    assert_eq!(r.status, 404);
    h.server.shutdown().await;
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

async fn github_status(ep: &Endpoint) -> String {
    let st: cmux_proto::api::StatusResp = req(ep, "GET", route::STATUS, &[]).await.json().unwrap();
    st.github
}

/// Waits until the poller has left `starting`.
async fn settled_github_status(ep: &Endpoint) -> String {
    for _ in 0..200 {
        let s = github_status(ep).await;
        if s != "starting" {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the GitHub poller never started");
}

#[tokio::test(flavor = "multi_thread")]
async fn with_gh_missing_the_server_starts_and_status_says_github_is_off() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(tmp.path());
    cfg.github.gh_bin = tmp.path().join("no-such-gh").display().to_string();
    // The real ticker, so the poller runs as in production.
    let server = ServerBuilder::new(cfg).start().await.unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    let s = settled_github_status(&ep).await;
    assert!(s.starts_with("off ("), "{s}");
    assert!(s.contains("no-such-gh not found"), "{s}");
    // The server still serves.
    assert_eq!(req(&ep, "GET", route::STATE, &[]).await.status, 200);
    server.shutdown().await;
}

struct LoggedIn;

impl cmuxd::git::github::Gh for LoggedIn {
    fn auth_status(&self) -> Result<(), String> {
        Ok(())
    }
    fn pr_view(&self, _: &str) -> Result<String, String> {
        Err("no PRs here".into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn github_status_is_on_after_a_good_auth_check_and_off_when_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let server = ServerBuilder::new(config(tmp.path()))
        .tick(None)
        .gh(std::sync::Arc::new(LoggedIn))
        .start()
        .await
        .unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    assert_eq!(settled_github_status(&ep).await, "on");
    server.shutdown().await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(tmp.path());
    cfg.github.enabled = false;
    let server = ServerBuilder::new(cfg)
        .gh(std::sync::Arc::new(LoggedIn))
        .start()
        .await
        .unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    assert_eq!(github_status(&ep).await, "off (disabled in config)");
    server.shutdown().await;

    // Tests that drive time by hand get no poller.
    let tmp = tempfile::tempdir().unwrap();
    let server = start_with(tmp.path(), config(tmp.path())).await.unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    assert_eq!(github_status(&ep).await, "off (no ticker)");
    server.shutdown().await;
}

#[tokio::test]
async fn the_web_app_is_served_with_a_strict_csp() {
    let h = harness().await;
    // No token: the page itself is public; the API behind it is not.
    for path in ["/", "/pair?code=ABCD2345"] {
        let r = req(&h.tcp, "GET", path, &[]).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.header("content-security-policy"), Some(api::web::CSP));
        assert!(api::web::CSP.contains("script-src 'self';"));
        assert!(api::web::CSP.contains("default-src 'none'"));
        assert_eq!(r.header("cache-control"), Some(api::web::REVALIDATE));
        assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
        assert!(r.header("content-type").unwrap().starts_with("text/html"));
        let body = String::from_utf8(r.body).unwrap();
        assert!(body.contains("<html"), "{body}");
    }
    // Unknown API routes and missing assets stay 404s, not the page.
    assert_eq!(req(&h.tcp, "GET", "/v1/nope", &[]).await.status, 404);
    assert_eq!(
        req(&h.tcp, "GET", "/assets/nope-1234.js", &[]).await.status,
        404
    );
    assert_eq!(req(&h.tcp, "POST", "/", &[]).await.status, 405);
    // With a build in web/dist/, the hashed entry script is cached for a year.
    let index = String::from_utf8(req(&h.tcp, "GET", "/", &[]).await.body).unwrap();
    if let Some(at) = index.find("/assets/") {
        let src = &index[at..at + index[at..].find('"').unwrap()];
        let r = req(&h.tcp, "GET", src, &[]).await;
        assert_eq!(r.status, 200, "{src}");
        assert_eq!(r.header("cache-control"), Some(api::web::IMMUTABLE));
        assert_eq!(r.header("content-security-policy"), Some(api::web::CSP));
    }
    h.server.shutdown().await;
}

/// Runs `git` in `dir` without the user's branch-name default.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "user.email=t@example.com",
        ])
        .args(["-c", "user.name=Test", "-C"])
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// A repository with one commit at `<dir>/work` and its bare clone at
/// `<dir>/origin.git`. Returns both.
fn origin_repo(dir: &Path) -> (PathBuf, PathBuf) {
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet"]);
    std::fs::write(work.join("README"), "hi\n").unwrap();
    git(&work, &["add", "README"]);
    git(&work, &["commit", "--quiet", "-m", "first"]);
    let bare = dir.join("origin.git");
    git(dir, &["clone", "--quiet", "--bare", "work", "origin.git"]);
    (work, bare)
}

async fn repos(ep: &Endpoint) -> Vec<RepoInfo> {
    let r: ReposResp = req(ep, "GET", route::REPOS, &[]).await.json().unwrap();
    r.repos
}

#[tokio::test]
async fn repos_lists_the_plans_repositories() {
    let h = harness().await;
    assert!(repos(&h.unix).await.is_empty());
    let r = send(&h.unix, "POST", route::PLANS, &[], &plan_req(&h, "x")).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let listed = repos(&h.unix).await;
    assert_eq!(listed.len(), 1);
    let tmp = h._tmp.path().canonicalize().unwrap();
    assert_eq!(listed[0].path, tmp.display().to_string());
    assert_eq!(listed[0].base_branch, "main");

    // A plan's repo and the same repo registered through a subdirectory
    // (and, on macOS, `/var` against `/private/var`) are one entry.
    let (work, _) = origin_repo(h._tmp.path());
    std::fs::create_dir(work.join("sub")).unwrap();
    let plan = serde_json::json!({
        "title": "y",
        "repos": [{"path": work.display().to_string()}],
    });
    let r = send(&h.unix, "POST", route::PLANS, &[], &plan).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let sub = serde_json::json!({ "path": work.join("sub").display().to_string() });
    let r = send(&h.unix, "POST", route::REPOS, &[], &sub).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let canonical = work.canonicalize().unwrap().display().to_string();
    assert_eq!(r.json::<RepoInfo>().unwrap().path, canonical);
    let paths: Vec<String> = repos(&h.unix).await.into_iter().map(|r| r.path).collect();
    assert_eq!(paths, vec![tmp.display().to_string(), canonical]);
    h.server.shutdown().await;
}

#[tokio::test]
async fn repos_can_be_cloned_from_a_url() {
    let h = harness().await;
    let (_, bare) = origin_repo(h._tmp.path());
    let url = bare.display().to_string();
    let body = serde_json::json!({ "url": url });
    let r = send(&h.unix, "POST", route::REPOS, &[], &body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let info: RepoInfo = r.json().unwrap();
    assert!(info.path.ends_with("/repos/origin"), "{}", info.path);
    let dest = PathBuf::from(&info.path);
    assert_eq!(git(&dest, &["log", "--format=%s"]), "first");
    assert_eq!(
        info.base_branch,
        git(&bare, &["symbolic-ref", "--short", "HEAD"])
    );
    assert_eq!(info.remote_url.as_deref(), Some(url.as_str()));
    assert_eq!(repos(&h.unix).await, vec![info.clone()]);

    // The destination is taken now.
    let r = send(&h.unix, "POST", route::REPOS, &[], &body).await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("already exists"), "{}", r.text());
    assert!(dest.join("README").exists());

    // A failed clone leaves nothing behind.
    let missing = h._tmp.path().join("missing.git").display().to_string();
    let r = send(
        &h.unix,
        "POST",
        route::REPOS,
        &[],
        &serde_json::json!({ "url": missing }),
    )
    .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(!dest.with_file_name("missing").exists());
    for bad in [
        serde_json::json!({}),
        serde_json::json!({ "url": url, "path": url }),
        serde_json::json!({ "url": "" }),
        serde_json::json!({ "url": "--upload-pack=touch x" }),
        serde_json::json!({ "url": "https://example.com/.." }),
    ] {
        let r = send(&h.unix, "POST", route::REPOS, &[], &bad).await;
        assert_eq!(r.status, 400, "{bad}: {}", r.text());
    }
    assert_eq!(repos(&h.unix).await, vec![info]);
    h.server.shutdown().await;
}

#[tokio::test]
async fn repos_can_be_registered_by_path() {
    let h = harness().await;
    let (work, _) = origin_repo(h._tmp.path());
    for bad in ["relative/path", "/nonexistent/repo"] {
        let r = send(
            &h.unix,
            "POST",
            route::REPOS,
            &[],
            &serde_json::json!({ "path": bad }),
        )
        .await;
        assert_eq!(r.status, 400, "{bad}: {}", r.text());
    }
    let body = serde_json::json!({ "path": work.display().to_string() });
    let r = send(&h.unix, "POST", route::REPOS, &[], &body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let info: RepoInfo = r.json().unwrap();
    let canonical = work.canonicalize().unwrap().display().to_string();
    assert_eq!(info.path, canonical);
    assert_eq!(info.base_branch, "main");
    assert_eq!(info.remote_url, None);
    // Registering it again keeps one entry.
    let r = send(&h.unix, "POST", route::REPOS, &[], &body).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(repos(&h.unix).await, vec![info]);
    let file = h._tmp.path().join("data").join("repos.json");
    let registered: Vec<String> = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(registered, vec![canonical]);

    // A deleted repository drops out of the list.
    std::fs::remove_dir_all(&work).unwrap();
    assert!(repos(&h.unix).await.is_empty());
    h.server.shutdown().await;
}

#[tokio::test]
async fn read_scope_cannot_add_repos() {
    let h = harness().await;
    let (work, _) = origin_repo(h._tmp.path());
    let a = bearer(&pair(&h, Scope::Read).await.token);
    let auth = [("Authorization", a.as_str())];
    assert_eq!(req(&h.tcp, "GET", route::REPOS, &auth).await.status, 200);
    let body = serde_json::json!({ "path": work.display().to_string() });
    let r = send(&h.tcp, "POST", route::REPOS, &auth, &body).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(r.json::<ErrorBody>().unwrap().code, ErrorCode::Forbidden);
    assert!(repos(&h.unix).await.is_empty());
    h.server.shutdown().await;
}

#[tokio::test]
async fn settings_show_the_limits_and_claude_options() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(tmp.path());
    cfg.limits.tasks_per_plan = 7;
    cfg.limits.subagents_per_task = 3;
    cfg.limits.live_processes = 11;
    cfg.agents.permission_wait_secs = 90;
    cfg.agents.task.model = Some("opus".into());
    cfg.agents.sub.permission_mode = Some("plan".into());
    let server = start_with(tmp.path(), cfg).await.unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    let s: SettingsResp = req(&ep, "GET", route::SETTINGS, &[]).await.json().unwrap();
    assert_eq!(
        (s.tasks_per_plan, s.subagents_per_task, s.live_processes),
        (7, 3, 11)
    );
    assert_eq!(s.permission_wait_secs, 90);
    let levels: Vec<_> = s
        .claude
        .iter()
        .map(|l| (l.level, l.model.as_deref(), l.permission_mode.as_str()))
        .collect();
    assert_eq!(
        levels,
        vec![
            (Level::Executor, None, "auto"),
            (Level::Task, Some("opus"), "auto"),
            (Level::Sub, None, "plan"),
        ]
    );
    server.shutdown().await;

    // `timeouts.permission_wait_secs` wins over the agents' one.
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(tmp.path());
    cfg.agents.permission_wait_secs = 90;
    cfg.timeouts.permission_wait_secs = Some(30);
    let server = start_with(tmp.path(), cfg).await.unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    let s: SettingsResp = req(&ep, "GET", route::SETTINGS, &[]).await.json().unwrap();
    assert_eq!(s.permission_wait_secs, 30);
    server.shutdown().await;
}
