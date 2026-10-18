//! Forms as pure state machines: feed keys, get a [`FormEvent`]. Submitting
//! returns the request body from `cmux_proto::api`; the client sends it.
//!
//! Common keys: `Tab`/`↓` next field, `BackTab`/`↑` previous, `↵` next field
//! (submit on the last), `Alt-↵` submit from anywhere, `Ctrl-O` newline in
//! multi-line fields, `Ctrl-U` clear the field, `Esc` cancel.

use cmux_proto::api::{
    AnswerAction, AnswerReq, CreatePlanReq, CreateTaskReq, MessageReq, RepoSpec,
};
use cmux_proto::view::{AgentId, ItemId, PlanId, TaskId};

use crate::keys::Key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent<T> {
    Editing,
    Cancel,
    Submit(T),
    /// The input is not valid; the form stays open and shows the message.
    Invalid(String),
}

/// A single text input with a cursor counted in chars.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextField {
    text: String,
    cursor: usize,
}

impl TextField {
    pub fn with_text(text: &str) -> Self {
        TextField {
            text: text.to_string(),
            cursor: text.chars().count(),
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Cursor position in chars.
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }
    pub fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let at = self.byte_at(self.cursor);
            self.text.remove(at);
        }
    }
    pub fn delete(&mut self) {
        if self.cursor < self.text.chars().count() {
            let at = self.byte_at(self.cursor);
            self.text.remove(at);
        }
    }
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }
    pub fn home(&mut self) {
        self.cursor = 0;
    }
    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub label: &'static str,
    pub input: TextField,
    pub multiline: bool,
}

impl Field {
    fn new(label: &'static str, text: &str, multiline: bool) -> Self {
        Field {
            label,
            input: TextField::with_text(text),
            multiline,
        }
    }
}

/// What the shared field handling decided for one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Edited,
    Cancel,
    /// `alt` is true for `Alt-↵`.
    Submit {
        alt: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fields {
    fields: Vec<Field>,
    focus: usize,
}

impl Fields {
    fn handle(&mut self, key: Key) -> Step {
        let last = self.fields.len() - 1;
        match key {
            Key::Esc => return Step::Cancel,
            Key::Tab | Key::Down => self.focus = (self.focus + 1) % self.fields.len(),
            Key::BackTab | Key::Up => {
                self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
            }
            Key::Enter if self.focus < last => self.focus += 1,
            Key::Enter => return Step::Submit { alt: false },
            Key::AltEnter => return Step::Submit { alt: true },
            k => {
                let f = &mut self.fields[self.focus];
                match k {
                    Key::Char(c) => f.input.insert(c),
                    Key::Ctrl('o') if f.multiline => f.input.insert('\n'),
                    Key::Ctrl('u') => f.input.clear(),
                    Key::Backspace => f.input.backspace(),
                    Key::Delete => f.input.delete(),
                    Key::Left => f.input.left(),
                    Key::Right => f.input.right(),
                    Key::Home => f.input.home(),
                    Key::End => f.input.end(),
                    _ => {}
                }
            }
        }
        Step::Edited
    }

    fn text(&self, i: usize) -> &str {
        self.fields[i].input.text()
    }
}

/// What the drawing code needs from a form.
pub trait FormView {
    fn title(&self) -> &str;
    fn fields(&self) -> &[Field];
    fn focus(&self) -> usize;
    fn hint(&self) -> &str;
    fn error(&self) -> Option<&str>;
}

macro_rules! form_view {
    ($t:ty, $title:expr, $hint:expr) => {
        impl FormView for $t {
            fn title(&self) -> &str {
                $title(self)
            }
            fn fields(&self) -> &[Field] {
                &self.f.fields
            }
            fn focus(&self) -> usize {
                self.f.focus
            }
            fn hint(&self) -> &str {
                $hint
            }
            fn error(&self) -> Option<&str> {
                self.error.as_deref()
            }
        }
    };
}

fn fail<T>(error: &mut Option<String>, msg: impl Into<String>) -> FormEvent<T> {
    let msg = msg.into();
    *error = Some(msg.clone());
    FormEvent::Invalid(msg)
}

// ---------------------------------------------------------------------------
// New plan
// ---------------------------------------------------------------------------

/// `N`: title, repositories and the request for the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlanForm {
    f: Fields,
    error: Option<String>,
}

impl NewPlanForm {
    /// `default_repo` pre-fills the repositories field (the client's cwd).
    pub fn new(default_repo: &str) -> Self {
        NewPlanForm {
            f: Fields {
                fields: vec![
                    Field::new("title", "", false),
                    Field::new("repos (path[@branch], ...)", default_repo, false),
                    Field::new("request", "", true),
                ],
                focus: 0,
            },
            error: None,
        }
    }

    pub fn handle(&mut self, key: Key) -> FormEvent<CreatePlanReq> {
        self.error = None;
        match self.f.handle(key) {
            Step::Edited => FormEvent::Editing,
            Step::Cancel => FormEvent::Cancel,
            Step::Submit { .. } => self.submit(),
        }
    }

    fn submit(&mut self) -> FormEvent<CreatePlanReq> {
        let title = self.f.text(0).trim().to_string();
        if title.is_empty() {
            return fail(&mut self.error, "title is empty");
        }
        let repos: Vec<RepoSpec> = self
            .f
            .text(1)
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| match s.rsplit_once('@') {
                Some((path, branch)) if !path.is_empty() && !branch.is_empty() => RepoSpec {
                    path: path.trim().to_string(),
                    base_branch: Some(branch.trim().to_string()),
                },
                _ => RepoSpec {
                    path: s.to_string(),
                    base_branch: None,
                },
            })
            .collect();
        if repos.is_empty() {
            return fail(&mut self.error, "give at least one repository path");
        }
        let request = self.f.text(2).trim().to_string();
        if request.is_empty() {
            return fail(&mut self.error, "request is empty");
        }
        FormEvent::Submit(CreatePlanReq {
            title,
            repos,
            request,
        })
    }
}

form_view!(
    NewPlanForm,
    |_| "new plan",
    "tab next  ↵ next/submit  alt-↵ submit  ^o newline  esc cancel"
);

// ---------------------------------------------------------------------------
// New task
// ---------------------------------------------------------------------------

/// `T`: a task in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTaskForm {
    plan: PlanId,
    repos: usize,
    f: Fields,
    error: Option<String>,
}

impl NewTaskForm {
    /// `repos` is the number of repositories in the plan, to check the index.
    pub fn new(plan: PlanId, repos: usize) -> Self {
        NewTaskForm {
            plan,
            repos,
            f: Fields {
                fields: vec![
                    Field::new("title", "", false),
                    Field::new("body", "", true),
                    Field::new("repo (index)", "0", false),
                    Field::new("depends on (T-0001, ...)", "", false),
                    Field::new("criteria (one per line)", "", true),
                ],
                focus: 0,
            },
            error: None,
        }
    }

    #[allow(dead_code)] // accessor
    pub fn plan(&self) -> &PlanId {
        &self.plan
    }

    pub fn handle(&mut self, key: Key) -> FormEvent<(PlanId, CreateTaskReq)> {
        self.error = None;
        match self.f.handle(key) {
            Step::Edited => FormEvent::Editing,
            Step::Cancel => FormEvent::Cancel,
            Step::Submit { .. } => self.submit(),
        }
    }

    fn submit(&mut self) -> FormEvent<(PlanId, CreateTaskReq)> {
        let title = self.f.text(0).trim().to_string();
        if title.is_empty() {
            return fail(&mut self.error, "title is empty");
        }
        let repo = match self.f.text(2).trim() {
            "" => 0,
            s => match s.parse::<usize>() {
                Ok(n) if n < self.repos.max(1) => n,
                _ => {
                    let msg = format!("repo must be a number below {}", self.repos.max(1));
                    return fail(&mut self.error, msg);
                }
            },
        };
        let mut deps = Vec::new();
        for d in self
            .f
            .text(3)
            .split([',', ' '])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match d.parse::<TaskId>() {
                Ok(t) => deps.push(t),
                Err(_) => return fail(&mut self.error, format!("{d:?} is not a task id (T-0001)")),
            }
        }
        let criteria = self
            .f
            .text(4)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        FormEvent::Submit((
            self.plan.clone(),
            CreateTaskReq {
                title,
                body: self.f.text(1).trim().to_string(),
                repo,
                deps,
                criteria,
            },
        ))
    }
}

form_view!(
    NewTaskForm,
    |_| "new task",
    "tab next  ↵ next/submit  alt-↵ submit  ^o newline  esc cancel"
);

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// `m` / `s`: message an agent. `↵` queues it for the end of the agent's
/// turn, `Alt-↵` steers (interrupts, then delivers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageForm {
    agent: AgentId,
    f: Fields,
    error: Option<String>,
}

impl MessageForm {
    pub fn new(agent: AgentId) -> Self {
        MessageForm {
            agent,
            f: Fields {
                fields: vec![Field::new("message", "", true)],
                focus: 0,
            },
            error: None,
        }
    }

    #[allow(dead_code)] // accessor
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    pub fn handle(&mut self, key: Key) -> FormEvent<(AgentId, MessageReq)> {
        self.error = None;
        match self.f.handle(key) {
            Step::Edited => FormEvent::Editing,
            Step::Cancel => FormEvent::Cancel,
            Step::Submit { alt } => {
                let text = self.f.text(0).trim().to_string();
                if text.is_empty() {
                    return fail(&mut self.error, "message is empty");
                }
                FormEvent::Submit((self.agent.clone(), MessageReq { text, steer: alt }))
            }
        }
    }
}

form_view!(
    MessageForm,
    |_| "message",
    "↵ queue for end of turn  alt-↵ steer now  ^o newline  esc cancel"
);

// ---------------------------------------------------------------------------
// Answer note
// ---------------------------------------------------------------------------

/// The text step of `n` (deny or send-back note) and `r` (reply) in the
/// needs-you overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteForm {
    item: ItemId,
    action: AnswerAction,
    f: Fields,
    error: Option<String>,
}

impl NoteForm {
    pub fn new(item: ItemId, action: AnswerAction) -> Self {
        let label = match action {
            AnswerAction::Reply => "reply",
            _ => "note (optional)",
        };
        NoteForm {
            item,
            action,
            f: Fields {
                fields: vec![Field::new(label, "", true)],
                focus: 0,
            },
            error: None,
        }
    }

    pub fn handle(&mut self, key: Key) -> FormEvent<(ItemId, AnswerReq)> {
        self.error = None;
        match self.f.handle(key) {
            Step::Edited => FormEvent::Editing,
            Step::Cancel => FormEvent::Cancel,
            Step::Submit { .. } => {
                let text = self.f.text(0).trim().to_string();
                if text.is_empty() && self.action == AnswerAction::Reply {
                    return fail(&mut self.error, "a reply needs text");
                }
                FormEvent::Submit((
                    self.item,
                    AnswerReq {
                        action: self.action,
                        text: (!text.is_empty()).then_some(text),
                    },
                ))
            }
        }
    }
}

form_view!(
    NoteForm,
    |s: &NoteForm| match s.action {
        AnswerAction::Reply => "reply",
        AnswerAction::Deny => "deny with a note",
        _ => "send back with a note",
    },
    "↵ send  ^o newline  esc cancel"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str<T>(mut feed: impl FnMut(Key) -> FormEvent<T>, s: &str) {
        for c in s.chars() {
            assert!(matches!(feed(Key::Char(c)), FormEvent::Editing));
        }
    }

    fn plan_id() -> PlanId {
        "P-20261018-auth".parse().unwrap()
    }

    #[test]
    fn text_field_edits_by_char() {
        let mut f = TextField::with_text("héy");
        f.left();
        f.insert('X');
        assert_eq!(f.text(), "héXy");
        f.backspace();
        f.backspace();
        assert_eq!(f.text(), "hy");
        f.home();
        f.delete();
        assert_eq!(f.text(), "y");
        f.end();
        assert_eq!(f.cursor(), 1);
    }

    #[test]
    fn new_plan_submits_repos_and_request() {
        let mut f = NewPlanForm::new("/home/me/api@main, /home/me/web");
        type_str(|k| f.handle(k), "auth refactor");
        assert_eq!(f.handle(Key::Enter), FormEvent::Editing);
        assert_eq!(f.handle(Key::Enter), FormEvent::Editing);
        type_str(|k| f.handle(k), "Move to OAuth");
        match f.handle(Key::Enter) {
            FormEvent::Submit(req) => {
                assert_eq!(req.title, "auth refactor");
                assert_eq!(req.request, "Move to OAuth");
                assert_eq!(req.repos.len(), 2);
                assert_eq!(req.repos[0].path, "/home/me/api");
                assert_eq!(req.repos[0].base_branch.as_deref(), Some("main"));
                assert_eq!(req.repos[1].base_branch, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn new_plan_validates_and_cancels() {
        let mut f = NewPlanForm::new("/r");
        assert!(matches!(f.handle(Key::AltEnter), FormEvent::Invalid(m) if m.contains("title")));
        assert_eq!(f.error(), Some("title is empty"));
        // Any key clears the error.
        type_str(|k| f.handle(k), "t");
        assert_eq!(f.error(), None);
        assert!(matches!(f.handle(Key::AltEnter), FormEvent::Invalid(m) if m.contains("request")));
        assert_eq!(f.handle(Key::Esc), FormEvent::Cancel);
    }

    #[test]
    fn multiline_request_uses_ctrl_o() {
        let mut f = NewPlanForm::new("/r");
        type_str(|k| f.handle(k), "t");
        f.handle(Key::Tab);
        f.handle(Key::Tab);
        type_str(|k| f.handle(k), "a");
        f.handle(Key::Ctrl('o'));
        type_str(|k| f.handle(k), "b");
        // Ctrl-O does nothing in single-line fields.
        f.handle(Key::BackTab);
        f.handle(Key::Ctrl('o'));
        match f.handle(Key::AltEnter) {
            FormEvent::Submit(req) => assert_eq!(req.request, "a\nb"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn new_task_parses_deps_criteria_and_repo() {
        let mut f = NewTaskForm::new(plan_id(), 2);
        type_str(|k| f.handle(k), "api");
        f.handle(Key::Tab);
        type_str(|k| f.handle(k), "Add the endpoint");
        f.handle(Key::Tab);
        f.handle(Key::Ctrl('u'));
        type_str(|k| f.handle(k), "1");
        f.handle(Key::Tab);
        type_str(|k| f.handle(k), "T-0001, T-0002");
        f.handle(Key::Tab);
        type_str(|k| f.handle(k), "tests pass");
        f.handle(Key::Ctrl('o'));
        type_str(|k| f.handle(k), "docs updated");
        match f.handle(Key::Enter) {
            FormEvent::Submit((plan, req)) => {
                assert_eq!(plan, plan_id());
                assert_eq!(req.title, "api");
                assert_eq!(req.body, "Add the endpoint");
                assert_eq!(req.repo, 1);
                assert_eq!(req.deps, vec![TaskId::new(1), TaskId::new(2)]);
                assert_eq!(req.criteria, vec!["tests pass", "docs updated"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn new_task_rejects_bad_repo_and_dep() {
        let mut f = NewTaskForm::new(plan_id(), 1);
        type_str(|k| f.handle(k), "x");
        f.handle(Key::Tab);
        f.handle(Key::Tab);
        type_str(|k| f.handle(k), "3");
        assert!(matches!(f.handle(Key::AltEnter), FormEvent::Invalid(m) if m.contains("repo")));
        f.handle(Key::Backspace);
        f.handle(Key::Tab);
        type_str(|k| f.handle(k), "nope");
        assert!(matches!(f.handle(Key::AltEnter), FormEvent::Invalid(m) if m.contains("nope")));
    }

    #[test]
    fn message_queue_versus_steer() {
        let agent: AgentId = "P-20261018-auth/T-0002".parse().unwrap();
        let mut f = MessageForm::new(agent.clone());
        assert!(matches!(f.handle(Key::Enter), FormEvent::Invalid(_)));
        type_str(|k| f.handle(k), "use sqlx");
        assert_eq!(
            f.handle(Key::Enter),
            FormEvent::Submit((
                agent.clone(),
                MessageReq {
                    text: "use sqlx".into(),
                    steer: false
                }
            ))
        );
        assert_eq!(
            f.handle(Key::AltEnter),
            FormEvent::Submit((
                agent,
                MessageReq {
                    text: "use sqlx".into(),
                    steer: true
                }
            ))
        );
    }

    #[test]
    fn note_form_builds_answer_requests() {
        let mut deny = NoteForm::new(ItemId(7), AnswerAction::Deny);
        assert_eq!(
            deny.handle(Key::Enter),
            FormEvent::Submit((
                ItemId(7),
                AnswerReq {
                    action: AnswerAction::Deny,
                    text: None
                }
            ))
        );
        type_str(|k| deny.handle(k), "no force push");
        assert_eq!(
            deny.handle(Key::Enter),
            FormEvent::Submit((
                ItemId(7),
                AnswerReq {
                    action: AnswerAction::Deny,
                    text: Some("no force push".into())
                }
            ))
        );
        let mut reply = NoteForm::new(ItemId(8), AnswerAction::Reply);
        assert!(matches!(reply.handle(Key::Enter), FormEvent::Invalid(_)));
        type_str(|k| reply.handle(k), "postgres");
        let FormEvent::Submit((_, req)) = reply.handle(Key::Enter) else {
            panic!()
        };
        assert_eq!(req.text.as_deref(), Some("postgres"));
        assert!(req.into_action().is_ok());
    }

    #[test]
    fn tab_cycles_fields() {
        let mut f = NewPlanForm::new("");
        f.handle(Key::Tab);
        f.handle(Key::Tab);
        f.handle(Key::Tab);
        assert_eq!(f.focus(), 0);
        f.handle(Key::BackTab);
        assert_eq!(f.focus(), 2);
    }
}
