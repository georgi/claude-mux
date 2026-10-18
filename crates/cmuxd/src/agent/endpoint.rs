//! `/v1/agent/hook` and `/v1/agent/tool` (architecture 7.2 and 7.3).
//!
//! Registered through `Extra.unix_only`, so they exist on the Unix socket
//! only. They do their own authentication: the request carries the agent's id
//! and token, and the engine compares the token's hash with the one it stores
//! (a bad token is a 401 and changes nothing). A token acts for its own agent.
//!
//! A `PermissionRequest` the engine holds (`HookReply::Held`) keeps the HTTP
//! request open until the engine's `Effect::AnswerPermission` for that item
//! reaches [`PermissionWaiters`] (a client answered, a rule was added, the
//! engine timed it out, or the agent moved on and the engine released it).
//! If the hook process goes away first (Claude cancelled it, or the terminal
//! answer won) the handler future is dropped and its waiter with it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::routing::post;
use axum::{Extension, Json, Router};
use cmux_core::engine::{Command, ErrorKind, Hook, HookReply, PermissionDecision, Reply};
use cmux_core::ids::{AgentId, ItemId, TermId};
use cmux_proto::api::{
    AgentHookReq, AgentHookResp, AgentToolReq, AgentToolResp, ErrorCode, route,
};
use serde_json::json;
use tokio::sync::oneshot;

use super::runner::AgentRuntime;
use super::wire::{parse_hook, parse_tool};
use crate::api::{ApiError, ApiResult, AppState};
use crate::claude::spawn::token_hash;

/// Extra time the endpoint waits past the engine's own permission timeout
/// before it gives up on a held request by itself.
const HOLD_GRACE: Duration = Duration::from_secs(5);

/// How long an answer nobody registered for is kept.
const EARLY_KEEP: Duration = Duration::from_secs(30);

enum Slot {
    Waiting(oneshot::Sender<Option<PermissionDecision>>),
    /// The answer came before the handler registered (or the hook is gone).
    Early(Option<PermissionDecision>, Instant),
}

/// The open `PermissionRequest` hooks, by inbox item.
#[derive(Default)]
pub struct PermissionWaiters {
    slots: Mutex<HashMap<ItemId, Slot>>,
}

impl PermissionWaiters {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ItemId, Slot>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts waiting for `item`'s answer. The receiver yields `None` for "no
    /// decision" (Claude shows its own prompt).
    pub fn register(&self, item: ItemId) -> oneshot::Receiver<Option<PermissionDecision>> {
        let (tx, rx) = oneshot::channel();
        let mut slots = self.lock();
        if let Some(Slot::Early(d, _)) = slots.remove(&item) {
            let _ = tx.send(d);
        } else {
            slots.insert(item, Slot::Waiting(tx));
        }
        rx
    }

    /// Delivers the engine's answer. One that arrives before the handler
    /// registered is kept for it.
    pub fn resolve(&self, item: ItemId, decision: Option<PermissionDecision>) {
        let mut slots = self.lock();
        // Answers for hooks that already went away are never claimed.
        slots.retain(|_, s| match s {
            Slot::Early(_, at) => at.elapsed() < EARLY_KEEP,
            Slot::Waiting(_) => true,
        });
        match slots.remove(&item) {
            Some(Slot::Waiting(tx)) => {
                let _ = tx.send(decision);
            }
            _ => {
                slots.insert(item, Slot::Early(decision, Instant::now()));
            }
        }
    }

    /// Stops waiting (the hook went away).
    pub fn forget(&self, item: ItemId) {
        self.lock().remove(&item);
    }

    /// Open waiters; for tests.
    pub fn waiting(&self) -> usize {
        self.lock()
            .values()
            .filter(|s| matches!(s, Slot::Waiting(_)))
            .count()
    }
}

struct ForgetOnDrop<'a> {
    waiters: &'a PermissionWaiters,
    item: ItemId,
}

impl Drop for ForgetOnDrop<'_> {
    fn drop(&mut self) {
        self.waiters.forget(self.item);
    }
}

impl AgentRuntime {
    /// The two agent routes. Put them in `Extra.unix_only`.
    pub fn routes(self: &Arc<Self>) -> Router<AppState> {
        Router::new()
            .route(route::AGENT_HOOK, post(hook))
            .route(route::AGENT_TOOL, post(tool))
            .layer(Extension(self.clone()))
    }
}

fn internal(msg: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::Internal, msg)
}

async fn hook(
    State(app): State<AppState>,
    Extension(rt): Extension<Arc<AgentRuntime>>,
    Json(req): Json<AgentHookReq>,
) -> ApiResult<Json<AgentHookResp>> {
    let started = Instant::now();
    let hook = parse_hook(&req.event, &req.payload).map_err(ApiError::invalid)?;
    let held = matches!(hook, Hook::PermissionRequest { .. });
    let reply = app
        .actor
        .call(Command::Hook {
            agent: req.agent.clone(),
            token_hash: token_hash(&req.token),
            hook,
        })
        .await??;
    let Reply::Hook(reply) = reply else {
        return Err(internal(format!("unexpected engine reply {reply:?}")));
    };
    let resp = match reply {
        HookReply::Held { item } => hold(&app, &rt, item).await,
        other => AgentHookResp::from_reply(&req.event, &other),
    };
    app.diag.log(
        "info",
        "agent_hook",
        json!({
            "agent": req.agent.to_string(),
            "event": req.event,
            "held": held,
            "answered": resp.output.is_some(),
            "ms": started.elapsed().as_millis() as u64,
        }),
    );
    Ok(Json(resp))
}

/// Waits for the engine to settle a held request.
async fn hold(app: &AppState, rt: &AgentRuntime, item: ItemId) -> AgentHookResp {
    let rx = rt.waiters.register(item);
    let _guard = ForgetOnDrop {
        waiters: &rt.waiters,
        item,
    };
    let wait = rt.permission_wait() + HOLD_GRACE;
    let decision = tokio::select! {
        r = rx => r.unwrap_or(None),
        () = tokio::time::sleep(wait) => {
            app.diag.log("warn", "permission_hold_expired", json!({"item": item.0}));
            None
        }
    };
    AgentHookResp::from_decision(decision.as_ref())
}

async fn tool(
    State(app): State<AppState>,
    Extension(rt): Extension<Arc<AgentRuntime>>,
    Json(req): Json<AgentToolReq>,
) -> ApiResult<Json<AgentToolResp>> {
    let started = Instant::now();
    let fail = |text: String| Ok(Json(AgentToolResp { text, is_error: true }));
    let call = match parse_tool(&req.name, &req.args, req.agent.plan()) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let result = app
        .actor
        .call(Command::Tool {
            agent: req.agent.clone(),
            token_hash: token_hash(&req.token),
            call,
        })
        .await?;
    let reply = match result {
        Ok(r) => r,
        // A bad token is a refusal of the caller, not of the call.
        Err(e) if e.kind == ErrorKind::Auth => return Err(e.into()),
        Err(e) => return fail(e.to_string()),
    };
    let text = match reply {
        Reply::Text(t) => t,
        Reply::Peek { agent, term, lines } => peek(&rt, &agent, term, lines),
        Reply::Ok => "ok".to_string(),
        other => return Err(internal(format!("unexpected engine reply {other:?}"))),
    };
    app.diag.log(
        "info",
        "agent_tool",
        json!({
            "agent": req.agent.to_string(),
            "tool": req.name,
            "ms": started.elapsed().as_millis() as u64,
        }),
    );
    Ok(Json(AgentToolResp {
        is_error: text.starts_with("error:"),
        text,
    }))
}

/// The last `lines` lines of an agent's screen.
fn peek(rt: &AgentRuntime, agent: &AgentId, term: Option<TermId>, lines: usize) -> String {
    let Some(term) = term else {
        return format!("error: {agent} has no terminal yet; it is still starting");
    };
    let Some(t) = rt.hub().get(&term.to_string()) else {
        return format!("error: the terminal of {agent} is gone");
    };
    let screen = t.screen_text();
    let all: Vec<&str> = screen.lines().collect();
    let end = all
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let start = end.saturating_sub(lines);
    let text = all[start..end].join("\n");
    if text.is_empty() {
        format!("{agent}: the screen is empty")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiters_deliver_early_and_late_answers() {
        let w = PermissionWaiters::default();
        let mut rx = w.register(ItemId(1));
        assert_eq!(w.waiting(), 1);
        assert!(rx.try_recv().is_err());
        w.resolve(ItemId(1), Some(PermissionDecision::Allow));
        assert_eq!(rx.try_recv().unwrap(), Some(PermissionDecision::Allow));
        assert_eq!(w.waiting(), 0);

        // The answer beats the registration.
        w.resolve(ItemId(2), None);
        let mut rx = w.register(ItemId(2));
        assert_eq!(rx.try_recv().unwrap(), None);

        // A hook that went away leaves nothing behind.
        let _rx = w.register(ItemId(3));
        w.forget(ItemId(3));
        assert_eq!(w.waiting(), 0);
    }
}
