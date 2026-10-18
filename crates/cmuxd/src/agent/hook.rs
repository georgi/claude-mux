//! `cmuxd hook <Event>`: Claude Code's hook entry point.
//!
//! Claude runs this with the hook payload on stdin and `CMUX_SOCK`,
//! `CMUX_AGENT` and `CMUX_TOKEN` in the environment. It posts the payload to
//! `/v1/agent/hook` on the Unix socket and prints what the server answers.
//!
//! A hook must never wedge or fail Claude's turn, so this always exits 0 and
//! prints nothing when the socket is gone, the server refuses, or anything
//! else goes wrong. It gives up after [`HOOK_TIMEOUT`], except for a
//! `PermissionRequest`, which the server may hold open for a remote answer
//! (up to `CMUX_PERMISSION_WAIT_SECS`, default 10 minutes).

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use cmux_core::ids::AgentId;
use cmux_proto::api::{AgentHookReq, AgentHookResp, route};
use serde_json::Value;

use crate::client::Endpoint;

/// How long a hook may take, `PermissionRequest` aside.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(2);

/// Waiting for Claude to finish writing the payload.
const STDIN_TIMEOUT: Duration = Duration::from_millis(1500);

const DEFAULT_PERMISSION_WAIT: Duration = Duration::from_secs(600);

/// Who is calling and where the server is.
#[derive(Debug, Clone)]
pub struct HookEnv {
    pub sock: PathBuf,
    pub agent: AgentId,
    pub token: String,
}

impl HookEnv {
    /// `None` unless all three variables are set and the agent id parses.
    pub fn from_env() -> Option<HookEnv> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Some(HookEnv {
            sock: var("CMUX_SOCK")?.into(),
            agent: var("CMUX_AGENT")?.parse().ok()?,
            token: var("CMUX_TOKEN")?,
        })
    }
}

/// How long to wait for the server's answer to `event`.
pub fn timeout_for(event: &str) -> Duration {
    if event != "PermissionRequest" {
        return HOOK_TIMEOUT;
    }
    std::env::var("CMUX_PERMISSION_WAIT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(DEFAULT_PERMISSION_WAIT, |s| Duration::from_secs(s.max(1)))
}

/// Posts one hook and returns the JSON to print, if any. Every failure is `None`.
pub async fn post(env: &HookEnv, event: &str, payload: Value, timeout: Duration) -> Option<Value> {
    let req = AgentHookReq {
        agent: env.agent.clone(),
        token: env.token.clone(),
        event: event.to_string(),
        payload,
    };
    let ep = Endpoint::Unix(env.sock.clone());
    let resp = tokio::time::timeout(timeout, ep.json("POST", route::AGENT_HOOK, &[], &req))
        .await
        .ok()?
        .ok()?;
    if resp.status != 200 {
        return None;
    }
    resp.json::<AgentHookResp>().ok()?.output
}

/// Reads all of stdin, giving up after [`STDIN_TIMEOUT`].
fn read_stdin() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::stdin().lock().read_to_string(&mut s);
        let _ = tx.send(s);
    });
    rx.recv_timeout(STDIN_TIMEOUT).unwrap_or_default()
}

/// The whole subcommand. Never fails; prints at most one line.
pub fn run(event: &str) {
    let Some(env) = HookEnv::from_env() else {
        return;
    };
    let stdin = read_stdin();
    let payload = serde_json::from_str(&stdin).unwrap_or(Value::Null);
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let out = rt.block_on(post(&env, event, payload, timeout_for(event)));
    if let Some(out) = out {
        println!("{out}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_permission_requests_may_wait() {
        assert_eq!(timeout_for("Stop"), HOOK_TIMEOUT);
        assert_eq!(timeout_for("PreToolUse"), Duration::from_secs(2));
        assert!(timeout_for("PermissionRequest") >= Duration::from_secs(60));
    }

    #[tokio::test]
    async fn no_socket_means_no_output() {
        let env = HookEnv {
            sock: "/nonexistent/cmux.sock".into(),
            agent: "P-20261018-demo/executor".parse().unwrap(),
            token: "t".into(),
        };
        let out = post(&env, "Stop", serde_json::json!({}), HOOK_TIMEOUT).await;
        assert_eq!(out, None);
    }
}
