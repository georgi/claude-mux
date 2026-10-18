//! REST request and response bodies for `/v1` (architecture 6.1 and 9).
//!
//! All bodies are JSON. Errors are always an [`ErrorBody`] with the status
//! from [`ErrorCode::status`]. Mutating requests accept [`IDEMPOTENCY_KEY`];
//! cookie-authenticated ones must also carry [`CSRF_HEADER`].

use cmux_core::engine::{ErrorKind, HookReply, ItemAction, PermissionDecision};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::view::{AgentId, Repo, TaskId, TaskState, TermId, TermKind, TerminalView, Timestamp};

/// Header on mutating requests; a retry with the same key replays the first
/// response instead of acting twice.
pub const IDEMPOTENCY_KEY: &str = "Idempotency-Key";
/// CSRF guard for cookie sessions: mutating requests must carry this header
/// (any non-empty value; clients send [`CSRF_VALUE`]).
pub const CSRF_HEADER: &str = "X-Cmux";
pub const CSRF_VALUE: &str = "1";
/// Device tokens travel as `Authorization: Bearer <token>`.
pub const AUTHORIZATION: &str = "Authorization";
/// The web session cookie (`HttpOnly; Secure; SameSite=Strict`).
pub const SESSION_COOKIE: &str = "cmux_session";

/// Route templates, for servers to register and clients to format.
pub mod route {
    pub const STATE: &str = "/v1/state";
    pub const STATUS: &str = "/v1/status";
    pub const STREAM: &str = "/v1/stream";
    pub const PLANS: &str = "/v1/plans";
    /// axum 0.8 capture syntax (`{id}`), so servers can register it as is.
    pub const ITEMS_ANSWER: &str = "/v1/items/{id}/answer";
    pub const TERMINALS: &str = "/v1/terminals";
    /// `GET`: the server user's Claude sessions (`~/.claude/projects`).
    pub const PROJECTS: &str = "/v1/projects";
    pub const PAIR_START: &str = "/v1/pair/start";
    pub const PAIR_COMPLETE: &str = "/v1/pair/complete";
    pub const DEVICES: &str = "/v1/devices";
    /// `POST` (bearer token) sets the web session cookie; `DELETE` clears it.
    pub const SESSION: &str = "/v1/session";
    pub const AGENT_HOOK: &str = "/v1/agent/hook";
    pub const AGENT_TOOL: &str = "/v1/agent/tool";

    // plan: POST accept|pause|resume|cancel; GET/PUT body; POST tasks
    // DELETE PLANS/{plan}: stop its agents and forget it
    pub fn plan(plan: &str, action: &str) -> String {
        format!("/v1/plans/{plan}/{action}")
    }
    // GET (TaskDetail) / PATCH (PatchTaskReq)
    pub fn task(plan: &str, task: &str) -> String {
        format!("/v1/tasks/{plan}/{task}")
    }
    // POST (SpawnTaskReq)
    pub fn task_spawn(plan: &str, task: &str) -> String {
        format!("/v1/tasks/{plan}/{task}/spawn")
    }
    // POST (MessageReq), stop, resume
    pub fn agent(agent: &str, action: &str) -> String {
        format!("/v1/agents/{agent}/{action}")
    }
    pub fn item_answer(item: u64) -> String {
        format!("/v1/items/{item}/answer")
    }
    // DELETE
    pub fn terminal(term: u32) -> String {
        format!("/v1/terminals/{term}")
    }
    // GET ?lines=
    pub fn scrollback(term: u32) -> String {
        format!("/v1/terminals/{term}/scrollback")
    }
    // GET / DELETE
    pub fn device(id: &str) -> String {
        format!("/v1/devices/{id}")
    }
}

// ---------------------------------------------------------------------------
// Errors, scopes, status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Missing, bad or expired credentials. 401.
    Unauthorized,
    /// Authenticated but not allowed (read-only scope, wrong role). 403.
    Forbidden,
    NotFound,
    /// Wrong state or bad argument. 400.
    Invalid,
    /// A limit is reached. 409.
    Limit,
    /// Pairing attempts or similar. 429.
    RateLimited,
    /// Same `Idempotency-Key` with a different request. 422.
    IdempotencyMismatch,
    Internal,
}

impl ErrorCode {
    pub fn status(&self) -> u16 {
        match self {
            ErrorCode::Unauthorized => 401,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::Invalid => 400,
            ErrorCode::Limit => 409,
            ErrorCode::RateLimited => 429,
            ErrorCode::IdempotencyMismatch => 422,
            ErrorCode::Internal => 500,
        }
    }
}

impl From<ErrorKind> for ErrorCode {
    fn from(k: ErrorKind) -> Self {
        match k {
            ErrorKind::Auth => ErrorCode::Unauthorized,
            ErrorKind::Forbidden => ErrorCode::Forbidden,
            ErrorKind::NotFound => ErrorCode::NotFound,
            ErrorKind::Invalid => ErrorCode::Invalid,
            ErrorKind::Limit => ErrorCode::Limit,
        }
    }
}

/// Body of every non-2xx response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    /// Human text; engine errors start with `error:`.
    pub message: String,
    /// Seconds to wait, for `rate_limited`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ErrorBody {
            code,
            message: message.into(),
            retry_after: None,
        }
    }
}

/// What a device token may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Everything, including terminals and pairing (shell-equivalent).
    Full,
    /// `GET`s and terminal output only: no input, no mutations.
    Read,
}

impl Scope {
    pub fn can_write(&self) -> bool {
        matches!(self, Scope::Full)
    }
}

/// Also the `server` field of the stream `welcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub name: String,
}

/// `GET /status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusResp {
    pub server: ServerInfo,
    pub uptime_secs: u64,
    pub rev: u64,
    /// 1, 5 and 15 minute load averages, if the platform has them.
    pub load: Option<[f32; 3]>,
    pub live_processes: u32,
    pub clients: u32,
    pub agents: Vec<AgentStatus>,
    /// The PR poller: `on`, `starting` or `off (<reason>)`.
    #[serde(default)]
    pub github: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub agent: AgentId,
    pub rss_kb: Option<u64>,
}

/// Response to creating a plan (`id` is the plan id) or a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Created {
    pub id: String,
}

// ---------------------------------------------------------------------------
// Plans and tasks
// ---------------------------------------------------------------------------

/// `POST /plans`. `repos[0]` is where the executor runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatePlanReq {
    pub title: String,
    pub repos: Vec<RepoSpec>,
    /// The user's request for a planner agent. Absent: no agent starts, the
    /// caller adds the tasks, and accepting the plan starts the executor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSpec {
    pub path: String,
    /// The server detects the default branch when absent.
    #[serde(default)]
    pub base_branch: Option<String>,
}

impl From<&Repo> for RepoSpec {
    fn from(r: &Repo) -> Self {
        RepoSpec {
            path: r.path.display().to_string(),
            base_branch: Some(r.base_branch.clone()),
        }
    }
}

/// `GET /plans/:id/body` and `PUT /plans/:id/body` (the `plan.md` text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanBody {
    pub text: String,
}

/// `POST /plans/:id/tasks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTaskReq {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub repo: usize,
    #[serde(default)]
    pub deps: Vec<TaskId>,
    #[serde(default)]
    pub criteria: Vec<String>,
}

/// `PATCH /tasks/:plan/:task`. Absent fields are left alone; `state` is a
/// transition checked against the table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchTaskReq {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub repo: Option<usize>,
    #[serde(default)]
    pub deps: Option<Vec<TaskId>>,
    #[serde(default)]
    pub state: Option<TaskState>,
}

/// `POST /tasks/:plan/:task/spawn` (queued when paused or at a limit).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnTaskReq {
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub force: bool,
}

/// `POST /agents/:id/messages`: `steer` interrupts, otherwise it is queued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReq {
    pub text: String,
    #[serde(default)]
    pub steer: bool,
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerAction {
    Allow,
    AllowAlways,
    Deny,
    Reply,
    Accept,
    SendBack,
    Dismiss,
}

/// `POST /items/:id/answer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerReq {
    pub action: AnswerAction,
    /// The note for `deny` and `send_back`, the text for `reply`.
    #[serde(default)]
    pub text: Option<String>,
}

impl AnswerReq {
    /// The engine action. `reply` needs text; notes default to empty.
    pub fn into_action(self) -> Result<ItemAction, String> {
        let text = self.text;
        Ok(match self.action {
            AnswerAction::Allow => ItemAction::Allow,
            AnswerAction::AllowAlways => ItemAction::AllowAlways,
            AnswerAction::Deny => ItemAction::Deny {
                note: text.unwrap_or_default(),
            },
            AnswerAction::Reply => ItemAction::Reply {
                text: text
                    .filter(|t| !t.is_empty())
                    .ok_or("error: reply needs non-empty text")?,
            },
            AnswerAction::Accept => ItemAction::Accept,
            AnswerAction::SendBack => ItemAction::SendBack {
                note: text.unwrap_or_default(),
            },
            AnswerAction::Dismiss => ItemAction::Dismiss,
        })
    }
}

// ---------------------------------------------------------------------------
// Terminals
// ---------------------------------------------------------------------------

/// What `POST /terminals` starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewTerminalKind {
    /// A plain interactive Claude; `args` are extra arguments such as
    /// `--resume ID`.
    Claude {
        #[serde(default)]
        args: Vec<String>,
    },
    /// The user's login shell.
    Shell,
    /// An argv, shown with `label` (or its first word).
    Command {
        argv: Vec<String>,
        #[serde(default)]
        label: Option<String>,
    },
}

/// `POST /terminals`. The response is a [`TerminalView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTerminalReq {
    #[serde(flatten)]
    pub kind: NewTerminalKind,
    /// Defaults to the server user's home directory.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub rows: Option<u16>,
    #[serde(default)]
    pub cols: Option<u16>,
}

impl NewTerminalKind {
    /// The model's kind for the terminal this request creates.
    pub fn term_kind(&self) -> TermKind {
        match self {
            NewTerminalKind::Claude { .. } => TermKind::Claude,
            NewTerminalKind::Shell => TermKind::Shell,
            NewTerminalKind::Command { argv, label } => TermKind::Command(
                label
                    .clone()
                    .or_else(|| argv.first().cloned())
                    .unwrap_or_default(),
            ),
        }
    }
}

pub type CreateTerminalResp = TerminalView;

/// One past Claude session of the server user, from its transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub cwd: String,
    pub title: String,
    #[serde(default)]
    pub first_prompt: Option<String>,
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// Last modification, seconds since the Unix epoch.
    pub modified: u64,
    #[serde(default)]
    pub branch: Option<String>,
}

/// `GET /projects`: newest session first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectsResp {
    pub sessions: Vec<SessionSummary>,
}

/// `GET /terminals/:id/scrollback?lines=`: plain text lines of the history
/// above the visible screen, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollbackResp {
    pub term: TermId,
    pub lines: Vec<String>,
    /// Lines the server holds, so a client can tell if it got everything.
    pub total: u32,
}

// ---------------------------------------------------------------------------
// Pairing and devices
// ---------------------------------------------------------------------------

/// `POST /pair/start` (Unix socket or full scope).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairStartReq {
    /// Scope of the device that will pair; defaults to `full`.
    #[serde(default)]
    pub scope: Option<Scope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairStartResp {
    /// 8 characters, single use.
    pub code: String,
    /// The URL a client should connect to, as the QR code holds it.
    pub url: String,
    pub expires_in_secs: u64,
    pub scope: Scope,
}

/// `POST /pair/complete` (no auth; rate limited).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairCompleteReq {
    pub code: String,
    pub device_name: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairCompleteResp {
    pub device_id: String,
    /// 256-bit token, shown once; the server keeps only its hash.
    pub token: String,
    pub scope: Scope,
    pub server: ServerInfo,
}

impl std::fmt::Debug for PairCompleteResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairCompleteResp")
            .field("device_id", &self.device_id)
            .field("token", &"<redacted>")
            .field("scope", &self.scope)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub created: Timestamp,
    pub last_seen: Option<Timestamp>,
    /// This is the device making the request.
    pub current: bool,
}

/// `GET /devices`. `DELETE /devices/:id` revokes and answers 204.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesResp {
    pub devices: Vec<DeviceView>,
}

// ---------------------------------------------------------------------------
// Agent endpoint (Unix socket only; agent tokens only)
// ---------------------------------------------------------------------------

/// `POST /v1/agent/hook`, sent by `cmuxd hook <event>`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentHookReq {
    /// `CMUX_AGENT`.
    pub agent: AgentId,
    /// `CMUX_TOKEN`; the server compares its hash.
    pub token: String,
    /// The event name (`Stop`, `PermissionRequest`, ...).
    pub event: String,
    /// Claude's hook payload (stdin), passed through untouched.
    pub payload: Value,
}

impl std::fmt::Debug for AgentHookReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentHookReq")
            .field("agent", &self.agent)
            .field("event", &self.event)
            .finish_non_exhaustive()
    }
}

/// What the hook binary prints to stdout. `None` prints nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AgentHookResp {
    pub output: Option<Value>,
}

impl AgentHookResp {
    /// Claude's output JSON for an engine reply to `event`. A `Held` reply
    /// must be resolved by the server before it answers; it maps to no output
    /// (Claude then shows its own prompt).
    pub fn from_reply(event: &str, reply: &HookReply) -> Self {
        let output = match reply {
            HookReply::None | HookReply::Held { .. } => None,
            HookReply::Block { reason } => Some(json!({"decision": "block", "reason": reason})),
            HookReply::AdditionalContext(text) => Some(json!({"hookSpecificOutput": {
                "hookEventName": event, "additionalContext": text}})),
            HookReply::Allow => Some(permission_output(&PermissionDecision::Allow)),
            HookReply::Deny { reason } if event == "PreToolUse" => {
                Some(json!({"hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": reason}}))
            }
            HookReply::Deny { reason } => Some(permission_output(&PermissionDecision::Deny {
                reason: reason.clone(),
            })),
        };
        AgentHookResp { output }
    }

    /// The `PermissionRequest` output for a human's decision (`None`: no output).
    pub fn from_decision(decision: Option<&PermissionDecision>) -> Self {
        AgentHookResp {
            output: decision.map(permission_output),
        }
    }
}

fn permission_output(d: &PermissionDecision) -> Value {
    let decision = match d {
        PermissionDecision::Allow => json!({"behavior": "allow"}),
        PermissionDecision::Deny { reason } => json!({"behavior": "deny", "message": reason}),
    };
    json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}})
}

/// `POST /v1/agent/tool`, sent by `cmuxd mcp` for a `tools/call`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToolReq {
    pub agent: AgentId,
    pub token: String,
    /// The tool name, e.g. `report_result`.
    pub name: String,
    /// The tool arguments as the model gave them.
    #[serde(default)]
    pub args: Value,
}

impl std::fmt::Debug for AgentToolReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentToolReq")
            .field("agent", &self.agent)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The MCP tool result: short plain text; errors start with `error:`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToolResp {
    pub text: String,
    #[serde(default)]
    pub is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::ids::{AgentPath, ItemId, PlanId};

    fn rt<T: Serialize + for<'a> Deserialize<'a> + PartialEq + std::fmt::Debug>(v: T) -> String {
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<T>(&s).unwrap(), v, "{s}");
        s
    }

    fn agent() -> AgentId {
        AgentId::new(
            PlanId::new(2026, 10, 18, "demo"),
            AgentPath::Task(TaskId::new(1)),
        )
    }

    #[test]
    fn constants() {
        assert_eq!(IDEMPOTENCY_KEY, "Idempotency-Key");
        assert_eq!(CSRF_HEADER, "X-Cmux");
        assert_eq!(route::task("P", "T-1"), "/v1/tasks/P/T-1");
        assert_eq!(route::scrollback(3), "/v1/terminals/3/scrollback");
        assert_eq!(route::item_answer(12), "/v1/items/12/answer");
    }

    #[test]
    fn errors_and_scopes() {
        let e = ErrorBody::new(ErrorCode::NotFound, "error: no such plan");
        assert_eq!(
            rt(e),
            r#"{"code":"not_found","message":"error: no such plan"}"#
        );
        let mut e = ErrorBody::new(ErrorCode::RateLimited, "slow down");
        e.retry_after = Some(12);
        rt(e);
        for (k, c, s) in [
            (ErrorKind::Auth, ErrorCode::Unauthorized, 401),
            (ErrorKind::Forbidden, ErrorCode::Forbidden, 403),
            (ErrorKind::NotFound, ErrorCode::NotFound, 404),
            (ErrorKind::Invalid, ErrorCode::Invalid, 400),
            (ErrorKind::Limit, ErrorCode::Limit, 409),
        ] {
            assert_eq!(ErrorCode::from(k), c);
            assert_eq!(c.status(), s);
        }
        assert_eq!(ErrorCode::RateLimited.status(), 429);
        assert_eq!(rt(Scope::Full), r#""full""#);
        assert_eq!(rt(Scope::Read), r#""read""#);
        assert!(Scope::Full.can_write() && !Scope::Read.can_write());
    }

    #[test]
    fn plan_and_task_bodies() {
        rt(CreatePlanReq {
            title: "t".into(),
            repos: vec![RepoSpec {
                path: "/r".into(),
                base_branch: None,
            }],
            request: Some("do it".into()),
        });
        rt(PlanBody {
            text: "# plan".into(),
        });
        rt(CreateTaskReq {
            title: "a".into(),
            body: "b".into(),
            repo: 1,
            deps: vec![TaskId::new(1)],
            criteria: vec!["c".into()],
        });
        let c: CreateTaskReq = serde_json::from_str(r#"{"title":"x"}"#).unwrap();
        assert_eq!((c.repo, c.body.as_str(), c.deps.len()), (0, "", 0));
        rt(PatchTaskReq {
            title: Some("n".into()),
            state: Some(TaskState::Testing),
            deps: Some(vec![]),
            ..Default::default()
        });
        let p: PatchTaskReq = serde_json::from_str("{}").unwrap();
        assert_eq!(p, PatchTaskReq::default());
        rt(SpawnTaskReq {
            instructions: Some("go".into()),
            force: true,
        });
        rt(MessageReq {
            text: "hi".into(),
            steer: true,
        });
        rt(Created { id: "T-3".into() });
        let repo = Repo {
            path: "/r".into(),
            base_branch: "main".into(),
            remote_url: None,
        };
        assert_eq!(RepoSpec::from(&repo).base_branch.as_deref(), Some("main"));
    }

    #[test]
    fn answers_convert() {
        for a in [
            AnswerAction::Allow,
            AnswerAction::AllowAlways,
            AnswerAction::Deny,
            AnswerAction::Reply,
            AnswerAction::Accept,
            AnswerAction::SendBack,
            AnswerAction::Dismiss,
        ] {
            rt(AnswerReq {
                action: a,
                text: Some("x".into()),
            });
        }
        let r: AnswerReq = serde_json::from_str(r#"{"action":"allow_always"}"#).unwrap();
        assert_eq!(r.into_action(), Ok(ItemAction::AllowAlways));
        let deny = AnswerReq {
            action: AnswerAction::Deny,
            text: Some("no".into()),
        };
        assert_eq!(
            deny.into_action(),
            Ok(ItemAction::Deny { note: "no".into() })
        );
        let deny = AnswerReq {
            action: AnswerAction::SendBack,
            text: None,
        };
        assert_eq!(
            deny.into_action(),
            Ok(ItemAction::SendBack { note: "".into() })
        );
        let reply = AnswerReq {
            action: AnswerAction::Reply,
            text: None,
        };
        assert!(reply.into_action().unwrap_err().starts_with("error:"));
        assert!(serde_json::from_str::<AnswerReq>(r#"{"action":"bogus"}"#).is_err());
    }

    #[test]
    fn terminals() {
        let s = rt(CreateTerminalReq {
            kind: NewTerminalKind::Shell,
            cwd: Some("/tmp".into()),
            rows: Some(24),
            cols: None,
        });
        assert!(s.contains(r#""kind":"shell""#));
        let c = CreateTerminalReq {
            kind: NewTerminalKind::Command {
                argv: vec!["lazygit".into()],
                label: None,
            },
            cwd: None,
            rows: None,
            cols: None,
        };
        rt(c.clone());
        assert_eq!(c.kind.term_kind(), TermKind::Command("lazygit".into()));
        let c: CreateTerminalReq = serde_json::from_str(r#"{"kind":"claude"}"#).unwrap();
        assert_eq!(c.kind.term_kind(), TermKind::Claude);
        assert!(serde_json::from_str::<CreateTerminalReq>(r#"{"kind":"command"}"#).is_err());
        rt(ScrollbackResp {
            term: TermId(1),
            lines: vec!["a".into()],
            total: 1,
        });
    }

    #[test]
    fn pairing_and_devices() {
        rt(PairStartReq::default());
        let r: PairStartReq = serde_json::from_str("{}").unwrap();
        assert_eq!(r.scope, None);
        rt(PairStartResp {
            code: "ABCD2345".into(),
            url: "https://h".into(),
            expires_in_secs: 300,
            scope: Scope::Full,
        });
        rt(PairCompleteReq {
            code: "ABCD2345".into(),
            device_name: "phone".into(),
        });
        let resp = PairCompleteResp {
            device_id: "d1".into(),
            token: "secret-token".into(),
            scope: Scope::Read,
            server: ServerInfo {
                version: "1".into(),
                name: "n".into(),
            },
        };
        rt(resp.clone());
        assert!(!format!("{resp:?}").contains("secret-token"));
        rt(DevicesResp {
            devices: vec![DeviceView {
                id: "d1".into(),
                name: "phone".into(),
                scope: Scope::Full,
                created: 1,
                last_seen: None,
                current: true,
            }],
        });
    }

    #[test]
    fn status() {
        rt(StatusResp {
            server: ServerInfo {
                version: "1".into(),
                name: "n".into(),
            },
            uptime_secs: 5,
            rev: 9,
            load: Some([0.5, 0.25, 0.125]),
            live_processes: 2,
            clients: 1,
            agents: vec![AgentStatus {
                agent: agent(),
                rss_kb: Some(100),
            }],
            github: "off (gh not found)".into(),
        });
    }

    #[test]
    fn agent_endpoint() {
        let req = AgentHookReq {
            agent: agent(),
            token: "tok-123".into(),
            event: "Stop".into(),
            payload: json!({"stop_hook_active": false, "x": [1, 2]}),
        };
        rt(req.clone());
        assert!(!format!("{req:?}").contains("tok-123"));
        let t = AgentToolReq {
            agent: agent(),
            token: "tok-123".into(),
            name: "report_result".into(),
            args: json!({"status": "success"}),
        };
        rt(t.clone());
        assert!(!format!("{t:?}").contains("tok-123"));
        let t: AgentToolReq =
            serde_json::from_value(json!({"agent": agent(), "token": "t", "name": "get_plan"}))
                .unwrap();
        assert_eq!(t.args, Value::Null);
        rt(AgentToolResp {
            text: "ok".into(),
            is_error: false,
        });
        rt(AgentHookResp::default());
    }

    #[test]
    fn hook_replies_map_to_claude_output() {
        let o = |e: &str, r: HookReply| AgentHookResp::from_reply(e, &r).output;
        assert_eq!(o("Stop", HookReply::None), None);
        assert_eq!(
            o("PermissionRequest", HookReply::Held { item: ItemId(1) }),
            None
        );
        assert_eq!(
            o("Stop", HookReply::Block { reason: "r".into() }),
            Some(json!({"decision": "block", "reason": "r"}))
        );
        assert_eq!(
            o(
                "UserPromptSubmit",
                HookReply::AdditionalContext("ctx".into())
            ),
            Some(json!({"hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit", "additionalContext": "ctx"}}))
        );
        assert_eq!(
            o("PermissionRequest", HookReply::Allow),
            Some(
                json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest",
                "decision": {"behavior": "allow"}}})
            )
        );
        assert_eq!(
            o(
                "PermissionRequest",
                HookReply::Deny {
                    reason: "no".into()
                }
            ),
            Some(
                json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest",
                "decision": {"behavior": "deny", "message": "no"}}})
            )
        );
        assert_eq!(
            o(
                "PreToolUse",
                HookReply::Deny {
                    reason: "no".into()
                }
            ),
            Some(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse",
                "permissionDecision": "deny", "permissionDecisionReason": "no"}}))
        );
        assert_eq!(AgentHookResp::from_decision(None).output, None);
        assert!(
            AgentHookResp::from_decision(Some(&PermissionDecision::Allow))
                .output
                .is_some()
        );
    }
}
