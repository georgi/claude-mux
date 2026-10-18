//! State model (architecture section 4) and the orchestrator's transition
//! tables. Pure data and pure checks; the engine owns all mutation.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, AgentPath, ClientId, ItemId, PlanId, TaskId, TermId};

/// Milliseconds since the Unix epoch. Always passed in; core never reads a clock.
pub type Timestamp = u64;

/// Whole server state. `rev` is bumped by the engine on every change.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct State {
    pub rev: u64,
    pub plans: Vec<Plan>,
    /// Terminals that belong to no agent (shells, commands).
    pub loose: Vec<TermId>,
    pub terminals: BTreeMap<TermId, Terminal>,
    pub inbox: Vec<InboxItem>,
    pub global_limits: GlobalLimits,
    /// Next `ItemId` to hand out.
    pub next_item_id: u64,
    /// Next `TermId` to hand out.
    pub next_term_id: u32,
    /// Where per-plan files live (`<data_dir>/plans/<plan>/plan.md`). Set by the server.
    #[serde(default)]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub timeouts: Timeouts,
    /// Limits a new plan starts with. Set by the server from `config.toml`.
    #[serde(skip)]
    pub plan_limits: Limits,
    /// `(request_id, untruncated match input)` of each held `PermissionRequest`
    /// hook, by its inbox item. Transient: a restart kills the held hooks.
    #[serde(skip)]
    pub held_permissions: BTreeMap<ItemId, (String, String)>,
    /// Prompt template overrides by file name (`task.md`), loaded by the server.
    #[serde(skip)]
    pub prompt_overrides: BTreeMap<String, String>,
}

/// Engine timers, in milliseconds (architecture sections 7.2, 8.1, 8.3 and 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timeouts {
    /// A held permission request falls back to Claude's own prompt after this.
    pub permission_wait: u64,
    /// `ask_user` / `ask_parent` questions expire after this.
    pub question: u64,
    /// A task or sub-agent idle this long with no report needs a human.
    pub idle_without_report: u64,
    /// A steer waits this long for `Stop` before it is pasted.
    pub steer: u64,
    /// A working agent with no progress for this long is stalled.
    pub stall: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            permission_wait: 10 * 60_000,
            question: 60 * 60_000,
            idle_without_report: 10 * 60_000,
            steer: 10_000,
            stall: 15 * 60_000,
        }
    }
}

/// Server-wide limits (architecture section 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalLimits {
    pub live_processes: u32,
    pub git_ops: u32,
}

impl Default for GlobalLimits {
    fn default() -> Self {
        GlobalLimits {
            live_processes: 8,
            git_ops: 2,
        }
    }
}

/// Per-plan limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub tasks_per_plan: u32,
    pub subagents_per_task: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            tasks_per_plan: 2,
            subagents_per_task: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: PlanId,
    pub title: String,
    pub state: PlanState,
    /// `repos[0]` is where the executor runs.
    pub repos: Vec<Repo>,
    pub paused: bool,
    pub limits: Limits,
    pub tasks: Vec<Task>,
    pub agents: Vec<Agent>,
    /// Next task sequence number (`TaskId::new(next_task_seq)`).
    pub next_task_seq: u32,
    /// Client spawns waiting for a free slot or for the plan to resume.
    #[serde(default)]
    pub spawn_queue: Vec<QueuedSpawn>,
}

/// A `spawn_task` from a client that could not start yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedSpawn {
    pub task: TaskId,
    pub instructions: Option<String>,
    pub force: bool,
}

impl Plan {
    pub fn new(id: PlanId, title: impl Into<String>, repos: Vec<Repo>) -> Self {
        Plan {
            id,
            title: title.into(),
            state: PlanState::Draft,
            repos,
            paused: false,
            limits: Limits::default(),
            tasks: Vec::new(),
            agents: Vec::new(),
            next_task_seq: 1,
            spawn_queue: Vec::new(),
        }
    }
    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }
    pub fn agent(&self, id: &AgentId) -> Option<&Agent> {
        self.agents.iter().find(|a| &a.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repo {
    pub path: PathBuf,
    pub base_branch: String,
    pub remote_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub body: String,
    pub state: TaskState,
    /// Index into `Plan::repos`.
    pub repo: usize,
    pub deps: Vec<TaskId>,
    pub criteria: Vec<Criterion>,
    pub notes: Vec<Note>,
    pub branch: Option<String>,
    pub worktree: Option<PathBuf>,
    pub pr: Option<PrRef>,
    pub agent: Option<AgentId>,
    /// Attempt counter; one report is accepted per attempt (a blocked one may be superseded by a later report).
    pub attempt: u32,
    /// "Allow always for this task" answers.
    pub allow_rules: Vec<AllowRule>,
    /// Extra instructions for the next spawn; kept until the agent starts.
    #[serde(default)]
    pub instructions: Option<String>,
}

impl Task {
    pub fn new(id: TaskId, title: impl Into<String>, body: impl Into<String>) -> Self {
        Task {
            id,
            title: title.into(),
            body: body.into(),
            state: TaskState::Todo,
            repo: 0,
            deps: Vec::new(),
            criteria: Vec::new(),
            notes: Vec::new(),
            branch: None,
            worktree: None,
            pr: None,
            agent: None,
            attempt: 0,
            allow_rules: Vec::new(),
            instructions: None,
        }
    }
    pub fn unchecked_criteria(&self) -> usize {
        self.criteria.iter().filter(|c| !c.checked).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    pub text: String,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub at: Timestamp,
    pub by: Option<AgentId>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRef {
    pub url: String,
    pub number: u64,
}

/// A tool pattern the user approved for a task (`"Bash"`, `"cargo test*"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowRule {
    pub tool: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Executor,
    Task,
    Sub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Life {
    /// Registered by the engine; waiting for the worktree or the process.
    Starting,
    Running,
    Exited {
        code: Option<i32>,
    },
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    Working,
    Idle,
    NeedsPermission,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: AgentId,
    pub path: AgentPath,
    pub level: Level,
    pub parent: Option<AgentId>,
    pub task: Option<TaskId>,
    /// Fixed at spawn with `--session-id` (changes after `/clear`).
    pub session_id: String,
    pub term: Option<TermId>,
    pub cwd: PathBuf,
    /// SHA-256 of the agent's token; serialized as 64 hex chars.
    #[serde(with = "hex32")]
    pub token_hash: [u8; 32],
    pub life: Life,
    pub activity: Activity,
    pub report: Option<Report>,
    pub tokens: TokenUsage,
    /// Stop hooks answered with `block` since the agent last ran a tool (cap 3, path A).
    pub consecutive_stop_blocks: u8,
    /// Last time a human typed into the agent's terminal (path B waits 3 s).
    pub last_human_input_at: Option<Timestamp>,
    /// When the agent last went idle; drives the 10-minute no-report rule.
    pub idle_since: Option<Timestamp>,
    /// Items pasted (path B) and not yet seen in a submitted prompt.
    #[serde(default)]
    pub unconfirmed: Vec<ItemId>,
    /// The report has been landed (posted to the parent, task moved).
    #[serde(default)]
    pub landed: bool,
    /// Last hook, tool call or screen change, to within 30 s (the watchdog clock).
    #[serde(default)]
    pub last_progress: Option<Timestamp>,
    /// Working with no progress for `Timeouts::stall`; progress clears it.
    #[serde(default)]
    pub stalled: bool,
}

impl Agent {
    /// Starting or running: the process exists or is about to.
    pub fn is_live(&self) -> bool {
        matches!(self.life, Life::Starting | Life::Running)
    }

    /// A freshly spawned agent: running, activity unknown, no report. Level,
    /// path and task come from the id; `parent` is derived from the path.
    pub fn new(
        id: AgentId,
        session_id: impl Into<String>,
        cwd: PathBuf,
        token_hash: [u8; 32],
    ) -> Self {
        let path = id.path().clone();
        let parent = path.parent().map(|p| AgentId::new(id.plan().clone(), p));
        Agent {
            level: path.level(),
            task: path.task_id(),
            parent,
            path,
            id,
            session_id: session_id.into(),
            term: None,
            cwd,
            token_hash,
            life: Life::Running,
            activity: Activity::Unknown,
            report: None,
            tokens: TokenUsage::default(),
            consecutive_stop_blocks: 0,
            last_human_input_at: None,
            idle_since: None,
            landed: false,
            unconfirmed: Vec::new(),
            last_progress: None,
            stalled: false,
        }
    }
}

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(b: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity(64);
        for byte in b {
            out.push_str(&format!("{byte:02x}"));
        }
        s.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        if s.len() != 64 || !s.is_ascii() {
            return Err(D::Error::custom("expected 64 hex chars"));
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(D::Error::custom)?;
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Success,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub status: ReportStatus,
    pub summary: String,
    pub pr_url: Option<String>,
    pub attempt: u32,
    /// Synthesized by the server when the agent exited without reporting.
    pub implicit: bool,
    /// Replaces an earlier `blocked` report of the same attempt; it moves the
    /// task only while the task is still `blocked`.
    #[serde(default)]
    pub supersedes_blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: TermId,
    pub kind: TermKind,
    pub agent: Option<AgentId>,
    pub cwd: PathBuf,
    /// (cols, rows)
    pub size: (u16, u16),
    pub size_owner: Option<ClientId>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermKind {
    Claude,
    Shell,
    /// A command with a display label.
    Command(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: ItemId,
    pub to: Recipient,
    pub kind: ItemKind,
    pub from: Option<AgentId>,
    pub body: String,
    pub status: Delivery,
    /// Items with the same dedupe key are posted once (`report:<agent>:<attempt>`).
    pub dedupe: Option<String>,
    pub created: Timestamp,
}

impl InboxItem {
    pub fn is_pending(&self) -> bool {
        self.status == Delivery::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipient {
    Agent(AgentId),
    Human,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    ChildReport,
    ChildQuestion,
    ChildExited,
    UserMessage,
    Steer,
    PrChanged,
    Permission { tool: String, input_summary: String },
    Question,
    PlanReview,
    Stalled,
    ExitedWithoutReport,
}

impl ItemKind {
    /// Kinds shown to humans as "needs you" actions (UI design 2.3).
    pub fn needs_human(&self) -> bool {
        matches!(
            self,
            ItemKind::Permission { .. }
                | ItemKind::Question
                | ItemKind::PlanReview
                | ItemKind::Stalled
                | ItemKind::ExitedWithoutReport
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Pending,
    Delivered { at: Timestamp, via: DeliveryPath },
    Answered { by: ClientId, answer: String },
    Dismissed,
    Expired,
}

/// Architecture 8.1: A = Stop continuation, B = PTY paste, C = prompt piggyback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPath {
    StopBlock,
    PtyPaste,
    PromptContext,
}

// ---------------------------------------------------------------------------
// States and transition tables (copied from the orchestrator's lib/types.ts)
// ---------------------------------------------------------------------------

macro_rules! state_enum {
    ($name:ident { $($v:ident = $s:literal),+ $(,)? }) => {
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];
            /// The orchestrator's snake_case string.
            pub fn label(&self) -> &'static str {
                match self { $($name::$v => $s),+ }
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.label())
            }
        }
        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                Self::ALL.iter().copied().find(|v| v.label() == s).ok_or_else(|| {
                    let all: Vec<_> = Self::ALL.iter().map(|v| v.label()).collect();
                    format!("error: unknown {} {s:?}; expected one of: {}", stringify!($name), all.join(", "))
                })
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Todo,
    InProgress,
    Testing,
    Failing,
    Passing,
    Merged,
    Blocked,
    Cancelled,
}
state_enum!(TaskState {
    Todo = "todo", InProgress = "in_progress", Testing = "testing", Failing = "failing",
    Passing = "passing", Merged = "merged", Blocked = "blocked", Cancelled = "cancelled",
});

impl TaskState {
    /// `TASK_TRANSITIONS` from lib/types.ts.
    pub fn allowed_next(&self) -> &'static [TaskState] {
        use TaskState::*;
        match self {
            Todo => &[InProgress, Cancelled],
            InProgress => &[Testing, Failing, Passing, Merged, Blocked, Cancelled],
            Testing => &[Passing, Failing, Merged, Blocked, Cancelled],
            Failing => &[Testing, Passing, Merged, Blocked, Cancelled],
            Passing => &[Merged, Testing, Failing, Blocked, Cancelled],
            Blocked => &[InProgress, Testing, Failing, Passing, Merged, Cancelled],
            Merged => &[],
            Cancelled => &[],
        }
    }
    pub fn is_terminal(&self) -> bool {
        self.allowed_next().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Draft,
    Proposed,
    Accepted,
    Done,
    Cancelled,
}
state_enum!(PlanState {
    Draft = "draft", Proposed = "proposed", Accepted = "accepted", Done = "done",
    Cancelled = "cancelled",
});

impl PlanState {
    /// `PLAN_TRANSITIONS` from lib/types.ts.
    pub fn allowed_next(&self) -> &'static [PlanState] {
        use PlanState::*;
        match self {
            Draft => &[Proposed, Accepted, Cancelled],
            Proposed => &[Accepted, Cancelled],
            Accepted => &[Done, Cancelled],
            Done => &[],
            Cancelled => &[],
        }
    }
    pub fn is_terminal(&self) -> bool {
        self.allowed_next().is_empty()
    }
}

/// Who asks for a task move. Only GitHub may merge with unchecked criteria.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionSource {
    Agent,
    Client,
    GitHub,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionError {
    /// The edge is not in the table; `allowed` lists the legal targets.
    IllegalEdge {
        subject: String,
        from: String,
        to: String,
        allowed: Vec<String>,
    },
    /// `merged` needs every criterion checked (unless GitHub reports the merge).
    UncheckedCriteria {
        subject: String,
        unchecked: usize,
        total: usize,
    },
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransitionError::IllegalEdge {
                subject,
                from,
                to,
                allowed,
            } => {
                let list = if allowed.is_empty() {
                    "none (terminal)".to_string()
                } else {
                    allowed.join(", ")
                };
                write!(
                    f,
                    "error: cannot move {subject} from {from} to {to}; allowed: {list}"
                )
            }
            TransitionError::UncheckedCriteria {
                subject,
                unchecked,
                total,
            } => write!(
                f,
                "error: cannot move {subject} to merged: {unchecked} of {total} criteria unchecked; check them first"
            ),
        }
    }
}

impl std::error::Error for TransitionError {}

fn illegal<S: fmt::Display + Copy>(
    subject: String,
    from: S,
    to: S,
    allowed: &[S],
) -> TransitionError {
    TransitionError::IllegalEdge {
        subject,
        from: from.to_string(),
        to: to.to_string(),
        allowed: allowed.iter().map(|s| s.to_string()).collect(),
    }
}

/// Checks a task move against the table, then the merged-needs-criteria rule.
pub fn check_task_transition(
    task: &Task,
    to: TaskState,
    source: TransitionSource,
) -> Result<(), TransitionError> {
    if !task.state.allowed_next().contains(&to) {
        return Err(illegal(
            task.id.to_string(),
            task.state,
            to,
            task.state.allowed_next(),
        ));
    }
    let unchecked = task.unchecked_criteria();
    if to == TaskState::Merged && unchecked > 0 && source != TransitionSource::GitHub {
        return Err(TransitionError::UncheckedCriteria {
            subject: task.id.to_string(),
            unchecked,
            total: task.criteria.len(),
        });
    }
    Ok(())
}

/// Checks a plan move against the table. Plans have no source-specific rules.
pub fn check_plan_transition(plan: &Plan, to: PlanState) -> Result<(), TransitionError> {
    if plan.state.allowed_next().contains(&to) {
        Ok(())
    } else {
        Err(illegal(
            plan.id.to_string(),
            plan.state,
            to,
            plan.state.allowed_next(),
        ))
    }
}

// ---------------------------------------------------------------------------
// GitHub PR -> task state (lib/pr-task-state.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiConclusion {
    Success,
    Failure,
    Pending,
    None,
}

/// A PR's real GitHub state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrGithubState {
    pub merged: bool,
    /// Closed without merging.
    pub closed: bool,
    pub ci: CiConclusion,
    pub head_sha: Option<String>,
}

/// merged -> merged; closed -> blocked; CI failure -> failing; CI success -> passing; else testing.
pub fn pr_to_task_state(s: &PrGithubState) -> TaskState {
    if s.merged {
        TaskState::Merged
    } else if s.closed {
        TaskState::Blocked
    } else {
        match s.ci {
            CiConclusion::Failure => TaskState::Failing,
            CiConclusion::Success => TaskState::Passing,
            CiConclusion::Pending | CiConclusion::None => TaskState::Testing,
        }
    }
}

/// One entry of `gh pr view --json statusCheckRollup`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollupCheck {
    /// Check-run status (`queued|in_progress|completed`).
    pub status: Option<String>,
    /// Check-run conclusion (`success|failure|neutral|...`).
    pub conclusion: Option<String>,
    /// Legacy commit-status state (`SUCCESS|FAILURE|PENDING|...`).
    pub state: Option<String>,
}

/// Collapses a rollup: failure beats pending beats success; empty is `None`.
pub fn rollup_to_ci_conclusion(checks: &[RollupCheck]) -> CiConclusion {
    if checks.is_empty() {
        return CiConclusion::None;
    }
    let (mut failure, mut pending) = (false, false);
    for c in checks {
        if c.status
            .as_deref()
            .is_some_and(|s| !s.is_empty() && !s.eq_ignore_ascii_case("completed"))
        {
            pending = true;
            continue;
        }
        let verdict = c
            .conclusion
            .as_deref()
            .or(c.state.as_deref())
            .unwrap_or("")
            .to_ascii_uppercase();
        match verdict.as_str() {
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "CANCELED" | "ACTION_REQUIRED"
            | "STARTUP_FAILURE" | "STALE" => failure = true,
            "PENDING" | "IN_PROGRESS" | "QUEUED" | "REQUESTED" | "WAITING" => pending = true,
            _ => {}
        }
    }
    if failure {
        CiConclusion::Failure
    } else if pending {
        CiConclusion::Pending
    } else {
        CiConclusion::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskState::*;

    fn pid() -> PlanId {
        PlanId::new(2026, 10, 18, "auth")
    }

    fn task(state: TaskState, criteria: &[bool]) -> Task {
        let mut t = Task::new(TaskId::new(3), "t", "body");
        t.state = state;
        t.criteria = criteria
            .iter()
            .map(|c| Criterion {
                text: "c".into(),
                checked: *c,
            })
            .collect();
        t
    }

    /// Written out by hand from lib/types.ts, independent of `allowed_next`.
    fn task_table() -> Vec<(TaskState, Vec<TaskState>)> {
        vec![
            (Todo, vec![InProgress, Cancelled]),
            (
                InProgress,
                vec![Testing, Failing, Passing, Merged, Blocked, Cancelled],
            ),
            (Testing, vec![Passing, Failing, Merged, Blocked, Cancelled]),
            (Failing, vec![Testing, Passing, Merged, Blocked, Cancelled]),
            (Passing, vec![Merged, Testing, Failing, Blocked, Cancelled]),
            (Merged, vec![]),
            (
                Blocked,
                vec![InProgress, Testing, Failing, Passing, Merged, Cancelled],
            ),
            (Cancelled, vec![]),
        ]
    }

    #[test]
    fn task_edges_full_cross_product() {
        let table = task_table();
        assert_eq!(table.len(), 8);
        let mut allowed = 0;
        for (from, ok) in &table {
            for (to, _) in &table {
                let t = task(*from, &[]);
                let r = check_task_transition(&t, *to, TransitionSource::Client);
                assert_eq!(r.is_ok(), ok.contains(to), "{from} -> {to}");
                allowed += r.is_ok() as usize;
            }
            assert_eq!(from.allowed_next(), ok.as_slice(), "{from}");
            assert_eq!(from.is_terminal(), ok.is_empty());
        }
        assert_eq!(allowed, 2 + 6 + 5 + 5 + 5 + 6);
        assert_eq!(TaskState::ALL.len(), 8);
    }

    #[test]
    fn plan_edges_full_cross_product() {
        use PlanState::*;
        let table = [
            (Draft, vec![Proposed, Accepted, Cancelled]),
            (Proposed, vec![Accepted, Cancelled]),
            (Accepted, vec![Done, Cancelled]),
            (Done, vec![]),
            (Cancelled, vec![]),
        ];
        for (from, ok) in &table {
            for (to, _) in &table {
                let mut p = Plan::new(pid(), "p", vec![]);
                p.state = *from;
                assert_eq!(
                    check_plan_transition(&p, *to).is_ok(),
                    ok.contains(to),
                    "{from} -> {to}"
                );
            }
            assert_eq!(from.allowed_next(), ok.as_slice());
            assert_eq!(from.is_terminal(), ok.is_empty());
        }
    }

    #[test]
    fn merged_requires_criteria() {
        let open = task(Passing, &[true, false, false]);
        for src in [TransitionSource::Agent, TransitionSource::Client] {
            let e = check_task_transition(&open, Merged, src).unwrap_err();
            assert!(matches!(
                e,
                TransitionError::UncheckedCriteria {
                    unchecked: 2,
                    total: 3,
                    ..
                }
            ));
            assert!(e.to_string().contains("2 of 3 criteria unchecked"), "{e}");
        }
        assert!(check_task_transition(&open, Merged, TransitionSource::GitHub).is_ok());
        assert!(
            check_task_transition(
                &task(Passing, &[true, true]),
                Merged,
                TransitionSource::Agent
            )
            .is_ok()
        );
        assert!(
            check_task_transition(&task(Passing, &[]), Merged, TransitionSource::Agent).is_ok()
        );
        // Unchecked criteria do not block other moves.
        assert!(check_task_transition(&open, Testing, TransitionSource::Agent).is_ok());
        // GitHub cannot override the table.
        assert!(check_task_transition(&task(Todo, &[]), Merged, TransitionSource::GitHub).is_err());
    }

    #[test]
    fn error_lists_allowed_edges() {
        let e =
            check_task_transition(&task(Todo, &[]), Merged, TransitionSource::Agent).unwrap_err();
        assert_eq!(
            e.to_string(),
            "error: cannot move T-0003 from todo to merged; allowed: in_progress, cancelled"
        );
        let e =
            check_task_transition(&task(Merged, &[]), Todo, TransitionSource::Client).unwrap_err();
        assert_eq!(
            e.to_string(),
            "error: cannot move T-0003 from merged to todo; allowed: none (terminal)"
        );
        let mut p = Plan::new(pid(), "p", vec![]);
        p.state = PlanState::Proposed;
        let e = check_plan_transition(&p, PlanState::Done).unwrap_err();
        assert_eq!(
            e.to_string(),
            "error: cannot move P-20261018-auth from proposed to done; allowed: accepted, cancelled"
        );
    }

    #[test]
    fn labels_and_parsing() {
        assert_eq!(
            serde_json::to_string(&InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::from_str::<TaskState>("\"in_progress\"").unwrap(),
            InProgress
        );
        for s in TaskState::ALL {
            assert_eq!(s.label().parse::<TaskState>().unwrap(), *s);
            assert_eq!(
                serde_json::to_string(s).unwrap(),
                format!("\"{}\"", s.label())
            );
        }
        for s in PlanState::ALL {
            assert_eq!(s.label().parse::<PlanState>().unwrap(), *s);
        }
        assert!(
            "InProgress"
                .parse::<TaskState>()
                .unwrap_err()
                .contains("in_progress")
        );
    }

    fn gh(merged: bool, closed: bool, ci: CiConclusion) -> PrGithubState {
        PrGithubState {
            merged,
            closed,
            ci,
            head_sha: None,
        }
    }

    #[test]
    fn pr_state_mapping() {
        use CiConclusion::*;
        assert_eq!(pr_to_task_state(&gh(true, false, Failure)), Merged);
        assert_eq!(pr_to_task_state(&gh(false, true, Success)), Blocked);
        assert_eq!(pr_to_task_state(&gh(false, false, Failure)), Failing);
        assert_eq!(pr_to_task_state(&gh(false, false, Success)), Passing);
        assert_eq!(pr_to_task_state(&gh(false, false, Pending)), Testing);
        assert_eq!(pr_to_task_state(&gh(false, false, None)), Testing);
    }

    fn chk(status: Option<&str>, conclusion: Option<&str>, state: Option<&str>) -> RollupCheck {
        let f = |s: Option<&str>| s.map(String::from);
        RollupCheck {
            status: f(status),
            conclusion: f(conclusion),
            state: f(state),
        }
    }

    #[test]
    fn rollup_cases() {
        use CiConclusion as C;
        let ok = chk(Some("COMPLETED"), Some("success"), None);
        let bad = chk(Some("completed"), Some("timed_out"), None);
        let run = chk(Some("in_progress"), None, None);
        assert_eq!(rollup_to_ci_conclusion(&[]), C::None);
        assert_eq!(
            rollup_to_ci_conclusion(std::slice::from_ref(&ok)),
            C::Success
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[
                ok.clone(),
                chk(None, Some("NEUTRAL"), None),
                chk(None, Some("skipped"), None)
            ]),
            C::Success
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[ok.clone(), run.clone()]),
            C::Pending
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[ok.clone(), run.clone(), bad.clone()]),
            C::Failure
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[chk(None, None, Some("PENDING"))]),
            C::Pending
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[chk(None, None, Some("failure"))]),
            C::Failure
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[chk(None, None, Some("SUCCESS"))]),
            C::Success
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[chk(Some("queued"), Some("failure"), None)]),
            C::Pending
        );
        for v in [
            "ERROR",
            "CANCELLED",
            "CANCELED",
            "ACTION_REQUIRED",
            "STARTUP_FAILURE",
            "STALE",
        ] {
            assert_eq!(
                rollup_to_ci_conclusion(&[chk(None, Some(v), None)]),
                C::Failure,
                "{v}"
            );
        }
        for v in ["REQUESTED", "WAITING", "QUEUED"] {
            assert_eq!(
                rollup_to_ci_conclusion(&[chk(None, Some(v), None)]),
                C::Pending,
                "{v}"
            );
        }
        // conclusion wins over state; unknown verdicts count as passing.
        assert_eq!(
            rollup_to_ci_conclusion(&[chk(None, Some("success"), Some("FAILURE"))]),
            C::Success
        );
        assert_eq!(
            rollup_to_ci_conclusion(&[chk(None, None, None)]),
            C::Success
        );
    }

    #[test]
    fn defaults() {
        assert_eq!(
            Limits::default(),
            Limits {
                tasks_per_plan: 2,
                subagents_per_task: 2
            }
        );
        assert_eq!(GlobalLimits::default().live_processes, 8);
    }

    #[test]
    fn full_state_round_trip() {
        let pid = pid();
        let exec = AgentId::new(pid.clone(), AgentPath::Executor);
        let tid = TaskId::new(1);
        let aid = AgentId::new(pid.clone(), AgentPath::Task(tid));
        let mut plan = Plan::new(
            pid.clone(),
            "Auth",
            vec![Repo {
                path: "/r".into(),
                base_branch: "main".into(),
                remote_url: Some("git@x:y.git".into()),
            }],
        );
        plan.state = PlanState::Accepted;
        let mut t = Task::new(tid, "login", "do it");
        t.state = InProgress;
        t.criteria.push(Criterion {
            text: "works".into(),
            checked: false,
        });
        t.notes.push(Note {
            at: 5,
            by: Some(exec.clone()),
            text: "n".into(),
        });
        t.pr = Some(PrRef {
            url: "https://github.com/a/b/pull/1".into(),
            number: 1,
        });
        t.agent = Some(aid.clone());
        t.allow_rules.push(AllowRule {
            tool: "Bash".into(),
            pattern: "cargo test*".into(),
        });
        plan.tasks.push(t);
        let mut agent = Agent::new(aid.clone(), "sess", "/w".into(), [0xab; 32]);
        agent.term = Some(TermId(1));
        agent.life = Life::Exited { code: Some(1) };
        agent.report = Some(Report {
            status: ReportStatus::Failed,
            summary: "s".into(),
            pr_url: None,
            attempt: 1,
            implicit: true,
            supersedes_blocked: false,
        });
        assert_eq!(agent.parent, Some(exec.clone()));
        plan.agents.push(agent);
        let mut st = State::default();
        st.plans.push(plan);
        st.terminals.insert(
            TermId(1),
            Terminal {
                id: TermId(1),
                kind: TermKind::Command("make".into()),
                agent: Some(aid.clone()),
                cwd: "/w".into(),
                size: (80, 24),
                size_owner: Some(ClientId("web".into())),
                title: None,
            },
        );
        st.loose.push(TermId(2));
        st.inbox.push(InboxItem {
            id: ItemId(1),
            to: Recipient::Human,
            kind: ItemKind::Permission {
                tool: "Bash".into(),
                input_summary: "cargo test".into(),
            },
            from: Some(aid),
            body: "b".into(),
            status: Delivery::Delivered {
                at: 9,
                via: DeliveryPath::PtyPaste,
            },
            dedupe: Some("k".into()),
            created: 8,
        });
        st.next_item_id = 2;
        st.next_term_id = 3;
        st.rev = 4;
        let json = serde_json::to_string(&st).unwrap();
        assert!(json.contains("\"in_progress\""));
        assert!(json.contains(&"ab".repeat(32)));
        assert!(json.contains("P-20261018-auth/T-0001"));
        assert_eq!(serde_json::from_str::<State>(&json).unwrap(), st);
        // Deterministic output.
        assert_eq!(json, serde_json::to_string(&st).unwrap());
        assert!(serde_json::from_str::<State>(&json.replace(&"ab".repeat(32), "zz")).is_err());
    }
}
