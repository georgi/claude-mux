//! Everything that can be fed to `apply`, and everything that comes out.

use std::path::PathBuf;

use crate::ids::{AgentId, ClientId, ItemId, PlanId, TaskId, TermId};
use crate::model::{Level, PlanState, PrGithubState, Repo, ReportStatus, TaskState, TermKind};

/// One input to the engine, grouped by who sends it.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// An action from a connected client (TUI, web, CLI).
    Client {
        client: ClientId,
        action: ClientAction,
    },
    /// A Claude Code hook. `token_hash` is the SHA-256 of the token the hook presented.
    Hook {
        agent: AgentId,
        token_hash: [u8; 32],
        hook: Hook,
    },
    /// An MCP tool call from an agent.
    Tool {
        agent: AgentId,
        token_hash: [u8; 32],
        call: ToolCall,
    },
    /// The outcome of an effect, reported by the effect runner (trusted).
    Runner(RunnerEvent),
    /// The 1 s timer.
    Tick,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClientAction {
    CreatePlan {
        title: String,
        repos: Vec<Repo>,
        request: String,
    },
    AcceptPlan {
        plan: PlanId,
    },
    PausePlan {
        plan: PlanId,
    },
    ResumePlan {
        plan: PlanId,
    },
    CancelPlan {
        plan: PlanId,
    },
    CreateTask {
        plan: PlanId,
        title: String,
        body: String,
        repo: usize,
        deps: Vec<TaskId>,
        criteria: Vec<String>,
    },
    UpdateTask {
        plan: PlanId,
        task: TaskId,
        patch: TaskPatch,
    },
    TransitionTask {
        plan: PlanId,
        task: TaskId,
        to: TaskState,
    },
    /// Queued when the plan is paused or a limit is reached.
    SpawnTask {
        plan: PlanId,
        task: TaskId,
        instructions: Option<String>,
        force: bool,
    },
    SendMessage {
        agent: AgentId,
        text: String,
        steer: bool,
    },
    StopAgent {
        agent: AgentId,
    },
    ResumeAgent {
        agent: AgentId,
    },
    AnswerItem {
        item: ItemId,
        action: ItemAction,
    },
    /// A client typed into the agent's terminal; path B waits 3 s.
    MarkHumanInput {
        agent: AgentId,
    },
    /// A loose terminal (no agent). The engine allocates the id and returns it
    /// as `Reply::Text`; the server then starts the PTY under that id.
    CreateTerminal {
        kind: TermKind,
        cwd: PathBuf,
        /// (cols, rows)
        size: (u16, u16),
    },
    /// Forget a loose terminal; the server kills its PTY.
    CloseTerminal {
        term: TermId,
    },
}

/// Fields of a task a caller may change; `None` leaves the field alone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub repo: Option<usize>,
    pub deps: Option<Vec<TaskId>>,
}

/// How a human answers a needs-you item (UI design 2.3).
#[derive(Debug, Clone, PartialEq)]
pub enum ItemAction {
    Allow,
    AllowAlways,
    Deny { note: String },
    Reply { text: String },
    Accept,
    SendBack { note: String },
    Dismiss,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationKind {
    /// `notification_type` is `permission_prompt`.
    PermissionPrompt,
    /// `notification_type` is `idle_prompt`; ignored.
    IdlePrompt,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hook {
    SessionStart {
        session_id: String,
    },
    UserPromptSubmit {
        prompt: String,
    },
    Stop {
        stop_hook_active: bool,
    },
    Notification {
        kind: NotificationKind,
        message: String,
    },
    PreToolUse {
        tool: String,
        input_summary: String,
        /// The full, untruncated input (for Bash, the whole command). Guard
        /// rails match on this, never on the 60-char summary.
        match_input: String,
    },
    PermissionRequest {
        request_id: String,
        tool: String,
        /// For display only (at most 60 chars).
        input_summary: String,
        /// The full, untruncated input that allow rules are matched against.
        match_input: String,
    },
    /// `clear` is the payload's `reason == "clear"`: `/clear` ends the session
    /// but not the process, and a `SessionStart` with a new session id follows.
    SessionEnd {
        clear: bool,
    },
}

/// One variant per MCP tool (architecture 7.3). Agents are named by id, tasks by
/// `TaskId`; the server fills in the caller's plan when it parses a name.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    GetPlan,
    UpdatePlan {
        state: Option<PlanState>,
        title: Option<String>,
    },
    ListTasks,
    /// `None` means the caller's own task.
    GetTask {
        task: Option<TaskId>,
    },
    CreateTask {
        title: String,
        body: String,
        repo: usize,
        deps: Vec<TaskId>,
        criteria: Vec<String>,
    },
    UpdateTask {
        task: TaskId,
        patch: TaskPatch,
    },
    TransitionTask {
        task: TaskId,
        to: TaskState,
    },
    AddCriterion {
        task: TaskId,
        text: String,
    },
    /// `criterion` counts from 1, as `get_task` lists them.
    CheckCriterion {
        task: TaskId,
        criterion: usize,
        checked: bool,
    },
    AddNote {
        task: TaskId,
        text: String,
    },
    SpawnTask {
        task: TaskId,
        instructions: Option<String>,
        force: bool,
    },
    SpawnSubagent {
        name: String,
        prompt: String,
    },
    Send {
        agent: AgentId,
        text: String,
        interrupt: bool,
    },
    StopAgent {
        agent: AgentId,
    },
    ListAgents,
    Peek {
        agent: AgentId,
        lines: usize,
    },
    SetTaskPr {
        url: String,
    },
    ReportResult {
        status: ReportStatus,
        summary: String,
        pr_url: Option<String>,
    },
    AskParent {
        question: String,
    },
    AnswerChild {
        agent: AgentId,
        answer: String,
    },
    AskUser {
        question: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerEvent {
    WorktreeReady {
        plan: PlanId,
        task: TaskId,
        path: PathBuf,
        branch: String,
    },
    WorktreeFailed {
        plan: PlanId,
        task: TaskId,
        error: String,
    },
    /// The PTY exists. The server generated the session id and the token.
    Spawned {
        agent: AgentId,
        term: TermId,
        session_id: String,
        token_hash: [u8; 32],
    },
    SpawnFailed {
        agent: AgentId,
        error: String,
    },
    Exited {
        agent: AgentId,
        /// The terminal of the process that exited: a replaced process's late
        /// exit must not end its successor.
        term: TermId,
        code: Option<i32>,
    },
    Pr {
        plan: PlanId,
        task: TaskId,
        state: PrGithubState,
    },
    /// The hub changed a terminal's size or size owner. No-op when unchanged
    /// or when the terminal is gone.
    TermSize {
        term: TermId,
        rows: u16,
        cols: u16,
        owner: Option<ClientId>,
    },
    /// The program in a terminal set its window title.
    TermTitle {
        term: TermId,
        title: Option<String>,
    },
}

/// What the caller gets back for one command.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub reply: Reply,
    pub effects: Vec<Effect>,
    pub deltas: Vec<Delta>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// Nothing to say (client actions, runner events, ticks).
    Ok,
    /// A short plain-text result (an MCP tool result, or a created id).
    Text(String),
    /// What the hook binary prints.
    Hook(HookReply),
    /// `peek`: the server reads the last `lines` lines of `term`.
    Peek {
        agent: AgentId,
        term: Option<TermId>,
        lines: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookReply {
    /// Print nothing.
    None,
    /// Stop: `{"decision":"block","reason":..}`.
    Block { reason: String },
    /// UserPromptSubmit: `additionalContext`.
    AdditionalContext(String),
    /// PermissionRequest: allow now.
    Allow,
    /// PreToolUse / PermissionRequest: refuse, with a reason Claude shows the model.
    Deny { reason: String },
    /// PermissionRequest: keep the hook open; an `Effect::AnswerPermission` for
    /// this item (or a timeout) ends it.
    Held { item: ItemId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
}

/// Work for the effect runner.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Reply with `WorktreeReady` or `WorktreeFailed`.
    CreateWorktree {
        plan: PlanId,
        task: TaskId,
        repo_path: PathBuf,
        base: String,
        branch: String,
    },
    /// Start `claude`; reply with `Spawned` or `SpawnFailed`. The agent is
    /// already in state with `Life::Starting`. `session_id` is `Some` only on
    /// resume (the server generates one otherwise, plus the token).
    SpawnAgent {
        agent: AgentId,
        /// The terminal id to start it under, allocated by the engine.
        term: TermId,
        level: Level,
        cwd: PathBuf,
        session_id: Option<String>,
        resume: bool,
        system_prompt: String,
        kickoff: String,
    },
    /// `term` is set when the engine knows the process but has not recorded
    /// it on the agent (a `Spawned` that arrived after the stop).
    KillAgent {
        agent: AgentId,
        term: Option<TermId>,
    },
    /// Path B. The server pastes `text` itself when it is one line under 1000
    /// chars; otherwise it writes it to a file and pastes one line,
    /// `[cmux] N new messages (I-12, I-13): read <file>` (a multi-line paste of
    /// 4+ lines would reach Claude wrapped in `<pasted_content>` tags).
    PasteDigest {
        agent: AgentId,
        items: Vec<ItemId>,
        text: String,
    },
    /// Raw bytes for the agent's PTY; only the steer `ESC`.
    WritePty { agent: AgentId, bytes: Vec<u8> },
    /// Finish a held `PermissionRequest` hook. `None` means no decision, so
    /// Claude shows its own prompt.
    AnswerPermission {
        item: ItemId,
        request_id: String,
        decision: Option<PermissionDecision>,
    },
    /// Push and needs-you for a new human item.
    Notify { item: ItemId },
    /// Save this plan.
    Persist { plan: PlanId },
    /// Save only what belongs to no plan (rev, terminals, inbox): a change
    /// that touched no plan still bumped `rev`.
    PersistGlobals,
}

/// A whole-object change at `rev`; the server builds the view from the state.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub rev: u64,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Plan(PlanId),
    Task(PlanId, TaskId),
    Agent(AgentId),
    Terminal(TermId),
    Item(ItemId),
    Removed(ObjectRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectRef {
    Plan(PlanId),
    Task(PlanId, TaskId),
    Agent(AgentId),
    Terminal(TermId),
    Item(ItemId),
}

/// Why a command was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Bad, unknown or expired agent token.
    Auth,
    /// The caller's role may not do this.
    Forbidden,
    NotFound,
    /// Wrong state or bad argument.
    Invalid,
    /// A limit is reached.
    Limit,
}

/// Always displays as `error: ...`, naming what is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub kind: ErrorKind,
    msg: String,
}

impl EngineError {
    pub fn new(kind: ErrorKind, msg: impl AsRef<str>) -> Self {
        let msg = msg.as_ref();
        EngineError {
            kind,
            msg: format!("error: {}", msg.strip_prefix("error: ").unwrap_or(msg)),
        }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for EngineError {}
