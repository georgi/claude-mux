//! Remote mode, the pure half: what a key means as a REST call.
//!
//! The event loop (`app::remote`) owns the connection, the windows and the
//! drawing. This module turns tree and overlay intents and form submissions
//! into [`Request`]s without touching the network, so the key-to-REST mapping
//! can be tested against a recording [`Rest`] and the real client uses
//! [`HttpRest`].

use cmux_proto::api::{
    AnswerReq, CreateTerminalReq, MessageReq, NewTerminalKind, SpawnTaskReq, route,
};
use cmux_proto::view::{AgentId, ItemId, PlanId, PlanState, StateView, TaskId, TaskState, TermId};
use serde::Serialize;
use serde_json::Value;

use super::forms::{FormEvent, FormView, MessageForm, NewPlanForm, NewTaskForm, NoteForm};
use super::intent::{InboxIntent, TreeIntent};
use super::state::Node;
use crate::conn::http::Http;
use crate::keys::Key;

/// One HTTP call to the server, described as data.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    Get(String),
    Post(String, Value),
    Patch(String, Value),
    Delete(String),
}

/// What the event loop does with a successful answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    Nothing,
    /// The answer is a `TerminalView`: show that terminal when it appears.
    FocusTerm,
    /// The answer is a `ProjectsResp`: fill the session picker.
    Sessions,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub call: Call,
    pub then: Then,
}

impl Request {
    fn new(call: Call) -> Request {
        Request {
            call,
            then: Then::Nothing,
        }
    }

    pub fn post(path: impl Into<String>, body: &impl Serialize) -> Request {
        Request::new(Call::Post(
            path.into(),
            serde_json::to_value(body).unwrap_or(Value::Null),
        ))
    }

    pub fn patch(path: impl Into<String>, body: &impl Serialize) -> Request {
        Request::new(Call::Patch(
            path.into(),
            serde_json::to_value(body).unwrap_or(Value::Null),
        ))
    }

    pub fn post_empty(path: impl Into<String>) -> Request {
        Request::new(Call::Post(path.into(), Value::Null))
    }

    pub fn delete(path: impl Into<String>) -> Request {
        Request::new(Call::Delete(path.into()))
    }

    pub fn then(mut self, then: Then) -> Request {
        self.then = then;
        self
    }

    /// `GET /projects`: the server's Claude sessions.
    pub fn sessions() -> Request {
        Request::new(Call::Get(route::PROJECTS.into())).then(Then::Sessions)
    }

    /// `POST /terminals`: a Claude, shell or command terminal in `cwd`.
    pub fn new_terminal(
        kind: NewTerminalKind,
        cwd: Option<String>,
        rows: u16,
        cols: u16,
    ) -> Request {
        Request::post(
            route::TERMINALS,
            &CreateTerminalReq {
                kind,
                cwd,
                rows: Some(rows.max(1)),
                cols: Some(cols.max(1)),
            },
        )
        .then(Then::FocusTerm)
    }

    /// `DELETE /terminals/:id`.
    pub fn close_terminal(term: TermId) -> Request {
        Request::delete(route::terminal(term.0))
    }

    /// `POST /items/:id/answer`.
    pub fn answer(item: ItemId, req: &AnswerReq) -> Request {
        Request::post(route::item_answer(item.0), req)
    }
}

/// `/agents/:id/:action`. An agent id contains a `/` (`plan/T-0002`), which
/// must be escaped to stay one path segment.
fn agent_route(agent: &AgentId, action: &str) -> String {
    route::agent(&agent.to_string().replace('/', "%2F"), action)
}

/// The HTTP layer behind the event loop. A fake in tests records the calls.
pub trait Rest: Send + Sync {
    fn send(&self, call: &Call) -> Result<Value, String>;
}

/// The real thing: [`Http`] over the Unix socket or TCP.
pub struct HttpRest(pub Http);

impl Rest for HttpRest {
    fn send(&self, call: &Call) -> Result<Value, String> {
        let r = match call {
            Call::Get(p) => self.0.get(p),
            Call::Post(p, b) => self.0.post(p, b),
            Call::Patch(p, b) => self.0.patch(p, b),
            Call::Delete(p) => self.0.delete(p).map(|()| Value::Null),
        };
        r.map_err(|e| e.to_string())
    }
}

/// A server-side action that asks first (`y` confirms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// `x`: stop the agent.
    Stop(AgentId),
    /// `c`: cancel the task.
    Cancel(PlanId, TaskId),
}

impl Pending {
    pub fn prompt(&self) -> String {
        match self {
            Pending::Stop(a) => format!("stop {}? (y/n)", a.path()),
            Pending::Cancel(_, t) => format!("cancel {t}? (y/n)"),
        }
    }

    pub fn request(&self) -> Request {
        match self {
            Pending::Stop(a) => Request::post_empty(agent_route(a, "stop")),
            Pending::Cancel(p, t) => Request::patch(
                route::task(&p.to_string(), &t.to_string()),
                &serde_json::json!({ "state": TaskState::Cancelled }),
            ),
        }
    }
}

/// The open form of tree and overlay keys.
pub enum ActiveForm {
    Plan(NewPlanForm),
    Task(NewTaskForm),
    Message(MessageForm),
    Note(NoteForm),
}

/// What a key in a form did.
#[derive(Debug, PartialEq)]
pub enum FormOutcome {
    Editing,
    Cancel,
    Send(Request),
}

impl ActiveForm {
    pub fn handle(&mut self, key: Key) -> FormOutcome {
        fn conv<T>(ev: FormEvent<T>, f: impl FnOnce(T) -> Request) -> FormOutcome {
            match ev {
                FormEvent::Editing | FormEvent::Invalid(_) => FormOutcome::Editing,
                FormEvent::Cancel => FormOutcome::Cancel,
                FormEvent::Submit(v) => FormOutcome::Send(f(v)),
            }
        }
        match self {
            ActiveForm::Plan(f) => conv(f.handle(key), |req| Request::post(route::PLANS, &req)),
            ActiveForm::Task(f) => conv(f.handle(key), |(plan, req)| {
                Request::post(route::plan(&plan.to_string(), "tasks"), &req)
            }),
            ActiveForm::Message(f) => conv(f.handle(key), |(agent, req): (AgentId, MessageReq)| {
                Request::post(agent_route(&agent, "messages"), &req)
            }),
            ActiveForm::Note(f) => conv(f.handle(key), |(item, req)| Request::answer(item, &req)),
        }
    }

    fn inner(&self) -> &dyn FormView {
        match self {
            ActiveForm::Plan(f) => f,
            ActiveForm::Task(f) => f,
            ActiveForm::Message(f) => f,
            ActiveForm::Note(f) => f,
        }
    }
}

impl FormView for ActiveForm {
    fn title(&self) -> &str {
        self.inner().title()
    }
    fn fields(&self) -> &[super::forms::Field] {
        self.inner().fields()
    }
    fn focus(&self) -> usize {
        self.inner().focus()
    }
    fn hint(&self) -> &str {
        self.inner().hint()
    }
    fn error(&self) -> Option<&str> {
        self.inner().error()
    }
}

/// What the event loop must do for an intent.
pub enum Action {
    None,
    /// Show the node's window and focus its pane.
    Open(Node),
    /// Show the node's window, keep tree focus.
    Preview(Node),
    /// Show the window of the agent that asks.
    Jump(AgentId),
    Form(ActiveForm),
    Confirm(Pending),
    Send(Request),
    Board,
    Inbox,
    Help,
    /// Back to the pane (also closes the inbox).
    Back,
    /// Open this URL in the local browser.
    OpenUrl(String),
    /// A pager or tool in a new server terminal in this directory.
    Terminal {
        argv: Vec<String>,
        label: &'static str,
        cwd: Option<String>,
    },
    Flash(String),
}

/// `default_repo` fills the new-plan form.
pub fn action_for(sv: &StateView, default_repo: &str, intent: TreeIntent) -> Action {
    match intent {
        TreeIntent::Open(n) => Action::Open(n),
        TreeIntent::Preview(n) => Action::Preview(n),
        TreeIntent::Filter => Action::Flash("filter is not available yet".into()),
        TreeIntent::NewPlan => Action::Form(ActiveForm::Plan(NewPlanForm::new(default_repo))),
        TreeIntent::NewTask(plan) => {
            let repos = sv.plan(&plan).map_or(1, |p| p.repos.len().max(1));
            Action::Form(ActiveForm::Task(NewTaskForm::new(plan, repos)))
        }
        TreeIntent::Accept(Node::Plan(plan)) => match sv.plan(&plan).map(|p| p.state) {
            Some(PlanState::Draft | PlanState::Proposed) => Action::Send(Request::post_empty(
                route::plan(&plan.to_string(), "accept"),
            )),
            _ => Action::Flash("the plan is already accepted".into()),
        },
        // Accepting a task starts it: the server queues it if the plan is
        // paused or at a limit.
        TreeIntent::Accept(Node::Task(plan, task)) => match sv.task(&plan, task) {
            Some(t) if t.agent.is_none() && t.state == TaskState::Todo => {
                Action::Send(Request::post(
                    route::task_spawn(&plan.to_string(), &task.to_string()),
                    &SpawnTaskReq::default(),
                ))
            }
            _ => Action::Flash("the task has already started".into()),
        },
        TreeIntent::Accept(_) => Action::None,
        TreeIntent::TogglePause(plan) => {
            let paused = sv.plan(&plan).is_some_and(|p| p.paused);
            let verb = if paused { "resume" } else { "pause" };
            Action::Send(Request::post_empty(route::plan(&plan.to_string(), verb)))
        }
        TreeIntent::Message(agent) => Action::Form(ActiveForm::Message(MessageForm::new(agent))),
        TreeIntent::Stop(a) => Action::Confirm(Pending::Stop(a)),
        TreeIntent::Resume(a) => Action::Send(Request::post_empty(agent_route(&a, "resume"))),
        TreeIntent::Cancel(p, t) => Action::Confirm(Pending::Cancel(p, t)),
        TreeIntent::Edit(_) => Action::Flash(
            "edit is not available yet: the server has no editor pane for plan.md".into(),
        ),
        TreeIntent::OpenPr(plan, task) => match sv.task(&plan, task).and_then(|t| t.pr.as_ref()) {
            Some(pr) => Action::OpenUrl(pr.url.clone()),
            None => Action::Flash("the task has no PR".into()),
        },
        TreeIntent::Lazygit(Node::Task(plan, task)) => {
            terminal_in_worktree(sv, &plan, task, &["lazygit"], "lazygit")
        }
        TreeIntent::DiffPager(Node::Task(plan, task)) => {
            terminal_in_worktree(sv, &plan, task, &["git", "diff"], "diff")
        }
        TreeIntent::Lazygit(_) | TreeIntent::DiffPager(_) => Action::None,
        TreeIntent::Board => Action::Board,
        TreeIntent::Inbox => Action::Inbox,
        TreeIntent::Help => Action::Help,
        TreeIntent::Back => Action::Back,
    }
}

fn terminal_in_worktree(
    sv: &StateView,
    plan: &PlanId,
    task: TaskId,
    argv: &[&str],
    label: &'static str,
) -> Action {
    match sv.task(plan, task).and_then(|t| t.worktree.as_ref()) {
        Some(dir) => Action::Terminal {
            argv: argv.iter().map(ToString::to_string).collect(),
            label,
            cwd: Some(dir.display().to_string()),
        },
        None => Action::Flash(format!("{task} has no worktree yet")),
    }
}

/// An overlay key's intent as an action.
pub fn inbox_action(intent: InboxIntent) -> Action {
    match intent {
        InboxIntent::Answer { item, req } => Action::Send(Request::answer(item, &req)),
        InboxIntent::Prompt { item, action } => {
            Action::Form(ActiveForm::Note(NoteForm::new(item, action)))
        }
        InboxIntent::Jump(a) => Action::Jump(a),
        InboxIntent::Close => Action::Back,
    }
}

/// A `Rest` that records calls and answers with canned JSON. For tests.
#[cfg(test)]
pub mod fake {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct Recording {
        pub calls: Mutex<Vec<Call>>,
        pub reply: Mutex<Option<Result<Value, String>>>,
    }

    impl Rest for Recording {
        fn send(&self, call: &Call) -> Result<Value, String> {
            self.calls.lock().unwrap().push(call.clone());
            self.reply
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Ok(Value::Null))
        }
    }
}
