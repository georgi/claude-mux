//! The security checklist of architecture section 9, one test per row that a
//! test can check, against an in-process server on a real Unix socket and a
//! real TCP port.
//!
//! | Row | Tests |
//! |---|---|
//! | Exposure | `exposure_*` here; `config::tests::plain_http_needs_loopback_or_opt_in`; `api::non_loopback_plain_http_is_refused_unless_allowed` |
//! | Pairing | `pairing_*` here; `api::pairing_codes_are_single_use_and_expire`, `api::pairing_attempts_are_rate_limited` |
//! | Web sessions | `web_sessions_*` here; `api::cookie_sessions_need_the_csrf_header_for_mutations` |
//! | Agent endpoint | `agents::agent_routes_exist_on_the_socket_only`, `agents::scripted_mcp_sessions_per_role` (role tools, wrong token) |
//! | What a full token grants | `cli::tests` (the pairing text says a full device can run shells and Claude); `api::read_scope_cannot_mutate_and_only_full_can_pair` |
//! | Permissions | `permissions_*` here |
//! | Logs | `agents::the_diagnostics_log_holds_no_prompt_tool_input_or_token` |
//!
//! Also here: `POST /v1/repos {path}` (added beyond the design), and in
//! `agents.rs` the Web Push payload limit
//! (`agents::a_push_stays_inside_the_web_push_payload_limit`).
//!
//! Not automated: the Unix socket trusts its peer's uid, and agents run as the
//! server's user, so an agent *process* could open the client API on the
//! socket. The row holds for agent *tokens*: they open only `/v1/agent/*`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cmux_core::ids::AgentId;
use cmux_core::model::Level;
use cmux_proto::api::{
    CreateTerminalReq, ErrorBody, ErrorCode, NewTerminalKind, PairCompleteReq, PairCompleteResp,
    PairStartReq, PairStartResp, RepoInfo, ReposResp, Scope, route,
};
use cmuxd::claude::spawn::{LevelOpts, SpawnConfig, SpawnRequest, plan_spawn};
use cmuxd::client::{Endpoint, Response};
use cmuxd::config::{Config, DEFAULT_TCP};
use cmuxd::server::{Server, ServerBuilder};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

struct Harness {
    server: Server,
    unix: Endpoint,
    tcp: Endpoint,
    tmp: tempfile::TempDir,
}

fn config(tmp: &Path) -> Config {
    Config {
        data_dir: Some(tmp.join("data")),
        socket: Some(tmp.join("api.sock")),
        listen: Some("127.0.0.1:0".into()),
        ..Config::default()
    }
}

async fn start(cfg: Config, tmp: tempfile::TempDir) -> Harness {
    let server = ServerBuilder::new(cfg).tick(None).start().await.unwrap();
    Harness {
        unix: Endpoint::Unix(server.socket_path.clone()),
        tcp: Endpoint::Tcp(server.tcp_addr.unwrap()),
        server,
        tmp,
    }
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config(tmp.path());
    start(cfg, tmp).await
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

async fn pair_start(h: &Harness) -> PairStartResp {
    send(
        &h.unix,
        "POST",
        route::PAIR_START,
        &[],
        &PairStartReq {
            scope: Some(Scope::Full),
        },
    )
    .await
    .json()
    .unwrap()
}

async fn pair_complete(h: &Harness, code: &str) -> Response {
    send(
        &h.tcp,
        "POST",
        route::PAIR_COMPLETE,
        &[],
        &PairCompleteReq {
            code: code.into(),
            device_name: "laptop".into(),
        },
    )
    .await
}

async fn token(h: &Harness) -> String {
    let start = pair_start(h).await;
    let r = pair_complete(h, &start.code).await;
    assert_eq!(r.status, 200, "{}", r.text());
    r.json::<PairCompleteResp>().unwrap().token
}

fn code_of(r: &Response) -> ErrorCode {
    r.json::<ErrorBody>().unwrap().code
}

// ---------------------------------------------------------------------------
// Exposure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exposure_tcp_is_loopback_by_default_and_plain_http_elsewhere_is_refused() {
    let addr = Config::default().tcp_addr().unwrap().unwrap();
    assert_eq!(addr.to_string(), DEFAULT_TCP);
    assert!(addr.ip().is_loopback());

    for listen in ["0.0.0.0:0", "[::]:0"] {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config {
            listen: Some(listen.into()),
            ..config(tmp.path())
        };
        let err = ServerBuilder::new(cfg).tick(None).start().await.err();
        let err = err.map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("allow_plain_http"), "{listen}: {err:?}");
    }
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pairing_codes_and_tokens_have_the_designed_strength_and_only_hashes_are_kept() {
    let h = harness().await;
    let start = pair_start(&h).await;
    // 8 characters from an alphabet without look-alikes, valid 5 minutes.
    assert_eq!(start.code.len(), 8, "{}", start.code);
    assert!(
        start
            .code
            .bytes()
            .all(|b| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&b)),
        "{}",
        start.code
    );
    assert_eq!(start.expires_in_secs, 300);
    assert!(start.url.ends_with(&format!("/pair?code={}", start.code)));

    let r = pair_complete(&h, &start.code).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let done: PairCompleteResp = r.json().unwrap();
    // A 256-bit token.
    assert_eq!(URL_SAFE_NO_PAD.decode(&done.token).unwrap().len(), 32);
    // The server keeps the SHA-256 hash, the scope and the last-seen time only.
    let raw = std::fs::read_to_string(h.tmp.path().join("data/devices.json")).unwrap();
    assert!(!raw.contains(&done.token), "{raw}");
    let hash: String = Sha256::digest(done.token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(raw.contains(&hash), "{raw}");

    // Single use: the code is spent. With the good attempt, that is two of
    // five attempts this minute; three more wrong ones, then a wait.
    assert_eq!(
        code_of(&pair_complete(&h, &start.code).await),
        ErrorCode::Unauthorized
    );
    for _ in 0..3 {
        assert_eq!(
            code_of(&pair_complete(&h, "WRONGXYZ").await),
            ErrorCode::Unauthorized
        );
    }
    let fresh = pair_start(&h).await;
    let r = pair_complete(&h, &fresh.code).await;
    assert_eq!(code_of(&r), ErrorCode::RateLimited, "{}", r.text());
    h.server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Web sessions
// ---------------------------------------------------------------------------

async fn upgrade(h: &Harness, query: &str, headers: &[(&str, &str)]) -> Result<u16, u16> {
    let url = format!("ws://{}/v1/stream{query}", h.server.tcp_addr.unwrap());
    let mut req = url.into_client_request().unwrap();
    for (k, v) in headers {
        let name = tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
        req.headers_mut().insert(name, v.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((_, resp)) => Ok(resp.status().as_u16()),
        Err(tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("{e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn web_sessions_the_websocket_refuses_a_token_in_the_url() {
    let h = harness().await;
    let tok = token(&h).await;
    for q in [
        format!("?token={tok}"),
        format!("?access_token={tok}"),
        format!("?client=web&TOKEN={tok}"),
        "?token=".to_string(),
    ] {
        assert_eq!(upgrade(&h, &q, &[]).await, Err(400), "{q}");
    }
    // Other query parameters are no reason to refuse; the token goes in hello.
    assert_eq!(upgrade(&h, "?client=web", &[]).await, Ok(101));
    assert_eq!(
        h.server
            .app
            .clients
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn web_sessions_a_cookie_websocket_from_another_origin_is_refused() {
    let h = harness().await;
    let a = format!("Bearer {}", token(&h).await);
    let r = h
        .tcp
        .request("POST", route::SESSION, &[("Authorization", &a)], None)
        .await
        .unwrap();
    assert_eq!(r.status, 204);
    let cookie = r
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let host = h.server.tcp_addr.unwrap().to_string();
    let same = format!("http://{host}");
    assert_eq!(
        upgrade(
            &h,
            "",
            &[("Cookie", &cookie), ("Origin", "https://evil.example")]
        )
        .await,
        Err(403)
    );
    assert_eq!(
        upgrade(&h, "", &[("Cookie", &cookie), ("Origin", &same)]).await,
        Ok(101)
    );
    h.server.shutdown().await;
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Every level starts `claude` in `auto` mode with Remote Control, unless the
/// level sets another mode; Remote Control stays on either way.
#[test]
fn permissions_agents_start_in_auto_mode_with_remote_control() {
    let mut cfg = SpawnConfig::new("/bin/cmuxd".into(), "/tmp/s".into(), "/tmp/d".into());
    for (path, level) in [
        ("P-20261018-x/executor", Level::Executor),
        ("P-20261018-x/T-0001", Level::Task),
        ("P-20261018-x/T-0001/lint", Level::Sub),
    ] {
        let agent: AgentId = path.parse().unwrap();
        let req = SpawnRequest {
            agent: &agent,
            level,
            cwd: Path::new("/tmp"),
            session_id: None,
            resume: false,
            system_prompt: "",
            kickoff: "go",
        };
        let argv = plan_spawn(&cfg, &req, "s", "t").argv;
        assert!(argv.contains(&"--remote-control".into()), "{argv:?}");
        let mode = argv.iter().position(|a| a == "--permission-mode").unwrap();
        assert_eq!(argv[mode + 1], "auto", "{argv:?}");
    }
    cfg.task = LevelOpts {
        permission_mode: Some("plan".into()),
        model: None,
    };
    let agent: AgentId = "P-20261018-x/T-0001".parse().unwrap();
    let req = SpawnRequest {
        agent: &agent,
        level: Level::Task,
        cwd: Path::new("/tmp"),
        session_id: None,
        resume: false,
        system_prompt: "",
        kickoff: "go",
    };
    let argv = plan_spawn(&cfg, &req, "s", "t").argv;
    let mode = argv.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(argv[mode + 1], "plan");
    assert!(argv.contains(&"--remote-control".into()));
}

/// A plain Claude terminal (`prefix c`, the web app's new session) too.
#[tokio::test(flavor = "multi_thread")]
async fn permissions_claude_terminals_start_in_auto_mode_with_remote_control() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let argv_file = tmp.path().join("argv");
    let bin = tmp.path().join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}.tmp'\nmv '{0}.tmp' '{0}'\nexec sleep 30\n",
            argv_file.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cfg = config(tmp.path());
    cfg.agents.claude_bin = bin.display().to_string();
    let h = start(cfg, tmp).await;
    let r = send(
        &h.unix,
        "POST",
        route::TERMINALS,
        &[],
        &CreateTerminalReq {
            kind: NewTerminalKind::Claude { args: vec![] },
            cwd: Some(h.tmp.path().display().to_string()),
            rows: None,
            cols: None,
        },
    )
    .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !argv_file.exists() {
        assert!(std::time::Instant::now() < until, "claude never started");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let argv: Vec<String> = std::fs::read_to_string(&argv_file)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert!(argv.contains(&"--remote-control".into()), "{argv:?}");
    let mode = argv.iter().position(|a| a == "--permission-mode").unwrap();
    assert_eq!(argv[mode + 1], "auto", "{argv:?}");
    h.server.shutdown().await;
}

// ---------------------------------------------------------------------------
// POST /v1/repos {path}
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo(root: &Path) -> PathBuf {
    let repo = root.join("work");
    std::fs::create_dir_all(repo.join("src/deep")).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    std::fs::write(repo.join("src/deep/a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "first"]);
    repo
}

async fn add_repo(ep: &Endpoint, headers: &[(&str, &str)], path: &str) -> Response {
    let body = serde_json::json!({ "path": path });
    send(ep, "POST", route::REPOS, headers, &body).await
}

async fn registered(h: &Harness) -> Vec<String> {
    let r: ReposResp = h
        .unix
        .request("GET", route::REPOS, &[], None)
        .await
        .unwrap()
        .json()
        .unwrap();
    r.repos.into_iter().map(|r| r.path).collect()
}

/// The route takes an absolute path to a directory inside a git work tree and
/// registers the canonical root of that work tree, nothing else. It needs a
/// full-scope device (or the socket), and it runs no command that the
/// repository's own config names.
#[tokio::test(flavor = "multi_thread")]
async fn repos_by_path_register_only_a_canonical_git_root_for_a_full_device() {
    let h = harness().await;
    let root = h.tmp.path().canonicalize().unwrap();
    let work = repo(&root);
    let canonical = work.display().to_string();
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(root.join("file"), "x").unwrap();

    // Refused: relative, missing, not in a repository, a file, `..` games that
    // end outside a repository, a git dir itself.
    for bad in [
        "work".to_string(),
        "./work".to_string(),
        "".to_string(),
        root.join("missing").display().to_string(),
        plain.display().to_string(),
        root.join("file").display().to_string(),
        format!("{}/../plain", work.display()),
        work.join(".git").display().to_string(),
    ] {
        let r = add_repo(&h.unix, &[], &bad).await;
        assert_eq!(r.status, 400, "{bad:?}: {}", r.text());
    }
    assert!(registered(&h).await.is_empty());

    // A subdirectory, a `..` path and a symlink all register the one root.
    let link = root.join("link");
    std::os::unix::fs::symlink(&work, &link).unwrap();
    for good in [
        work.join("src/deep").display().to_string(),
        format!("{}/src/../src/deep/..", work.display()),
        link.display().to_string(),
    ] {
        let r = add_repo(&h.unix, &[], &good).await;
        assert_eq!(r.status, 201, "{good}: {}", r.text());
        assert_eq!(r.json::<RepoInfo>().unwrap().path, canonical, "{good}");
    }
    assert_eq!(registered(&h).await, vec![canonical.clone()]);

    // A repository's config cannot make the server run a command.
    let other = root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "--initial-branch=main"]);
    let pwned = root.join("pwned");
    let hook = format!("touch '{}'; false", pwned.display());
    for (key, value) in [
        ("core.fsmonitor", hook.as_str()),
        ("core.sshCommand", hook.as_str()),
        ("core.pager", hook.as_str()),
    ] {
        git(&other, &["config", key, value]);
    }
    git(
        &other,
        &["remote", "add", "origin", "ssh://example.invalid/x.git"],
    );
    let r = add_repo(&h.unix, &[], &other.display().to_string()).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert!(!pwned.exists(), "the repository's config ran a command");

    // TCP: no token, a read-only device: refused, and nothing is registered.
    let r = add_repo(&h.tcp, &[], &canonical).await;
    assert_eq!(r.status, 401, "{}", r.text());
    let (_, read) = h.server.app.devices.create("viewer", Scope::Read).unwrap();
    let a = format!("Bearer {read}");
    let third = root.join("third");
    std::fs::create_dir_all(&third).unwrap();
    git(&third, &["init", "--initial-branch=main"]);
    let r = add_repo(
        &h.tcp,
        &[("Authorization", &a)],
        &third.display().to_string(),
    )
    .await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert!(!registered(&h).await.contains(&third.display().to_string()));
    h.server.shutdown().await;
}
