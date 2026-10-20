//! Forms as pure state machines: feed keys, get a [`FormEvent`]. Submitting
//! returns the request body from `cmux_proto::api`; the client sends it.
//!
//! Common keys: `Tab`/`↓` next field, `BackTab`/`↑` previous, `↵` next field
//! (submit on the last), `Alt-↵` submit from anywhere, `Ctrl-O` newline in
//! multi-line fields, `Ctrl-U` clear the field, `Esc` cancel.

use cmux_proto::api::{AnswerAction, AnswerReq, MessageReq};
use cmux_proto::view::{AgentId, ItemId};

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
}
