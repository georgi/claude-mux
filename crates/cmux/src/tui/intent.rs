//! Keys to intents. The handlers move selections themselves and return an
//! intent only for things the client must do (open a form, call REST, switch
//! windows), so the event loop stays a thin dispatcher.

use cmux_proto::api::{AnswerAction, AnswerReq};
use cmux_proto::view::{AgentId, ItemId, ItemKind, ItemView, PlanId, StateView, TaskId};

use super::nav::{self, Move};
use super::rows;
use super::state::{Node, ViewState};
use crate::keys::Key;

/// What a tree-focus key asks the client to do (UI design 3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeIntent {
    /// `↵`: open the node's window and focus its pane.
    Open(Node),
    /// `Space`: show the node's window, keep tree focus.
    Preview(Node),
    Filter,
    NewPlan,
    /// `t`: new task in the plan of the selection.
    NewTask(PlanId),
    /// `A`: accept the plan and start it, or accept a task.
    Accept(Node),
    /// `S`: pause (or resume) the plan.
    TogglePause(PlanId),
    /// `s`: open the message prompt for the agent.
    Message(AgentId),
    /// `x`: stop the agent (the client asks first).
    Stop(AgentId),
    /// `R`: resume the agent's Claude session.
    Resume(AgentId),
    /// `c`: cancel the task (the client asks first).
    Cancel(PlanId, TaskId),
    /// `e`: edit `plan.md` or the task body on the server.
    Edit(Node),
    /// `o`: open the task's PR in the local browser.
    OpenPr(PlanId, TaskId),
    /// `g`: lazygit in the task's worktree.
    Lazygit(Node),
    /// `D`: diff pager in the task's worktree.
    DiffPager(Node),
    Board,
    Inbox,
    Help,
    /// `Esc`: back to the pane.
    Back,
}

/// A key in tree focus. Movement keys update `vs` and return `None`.
pub fn handle_tree_key(sv: &StateView, vs: &mut ViewState, key: Key) -> Option<TreeIntent> {
    let mv = match key {
        Key::Up | Key::Char('k') => Some(Move::Up),
        Key::Down | Key::Char('j') => Some(Move::Down),
        Key::Left | Key::Char('h') => Some(Move::Left),
        Key::Right | Key::Char('l') => Some(Move::Right),
        Key::Home => Some(Move::First),
        Key::End => Some(Move::Last),
        _ => None,
    };
    if let Some(mv) = mv {
        nav::navigate(sv, vs, mv);
        return None;
    }
    let sel = vs.selection.clone();
    let agent = sel.as_ref().and_then(|n| n.agent(sv));
    let plan = sel.as_ref().and_then(|n| n.plan().cloned());
    let task = match &sel {
        Some(Node::Task(p, t)) => Some((p.clone(), *t)),
        _ => None,
    };
    match key {
        Key::Enter => sel.map(TreeIntent::Open),
        Key::Char(' ') => sel.map(TreeIntent::Preview),
        Key::Char('/') => Some(TreeIntent::Filter),
        Key::Char('n') => Some(TreeIntent::NewPlan),
        Key::Char('t') => plan.map(TreeIntent::NewTask),
        Key::Char('A') => sel
            .filter(|n| matches!(n, Node::Plan(_) | Node::Task(..)))
            .map(TreeIntent::Accept),
        Key::Char('S') => plan.map(TreeIntent::TogglePause),
        Key::Char('s') => agent.map(TreeIntent::Message),
        Key::Char('x') => agent.map(TreeIntent::Stop),
        Key::Char('R') => agent.map(TreeIntent::Resume),
        Key::Char('c') => task.map(|(p, t)| TreeIntent::Cancel(p, t)),
        Key::Char('e') => sel
            .filter(|n| matches!(n, Node::Plan(_) | Node::Task(..)))
            .map(TreeIntent::Edit),
        Key::Char('o') => task
            .filter(|(p, t)| sv.task(p, *t).is_some_and(|t| t.pr.is_some()))
            .map(|(p, t)| TreeIntent::OpenPr(p, t)),
        Key::Char('g') => sel
            .filter(|n| matches!(n, Node::Task(..)))
            .map(TreeIntent::Lazygit),
        Key::Char('D') => sel
            .filter(|n| matches!(n, Node::Task(..)))
            .map(TreeIntent::DiffPager),
        Key::Char('v') => Some(TreeIntent::Board),
        Key::Char('i') => Some(TreeIntent::Inbox),
        Key::Char('?') => Some(TreeIntent::Help),
        Key::Esc => Some(TreeIntent::Back),
        _ => None,
    }
}

/// What a key in the needs-you overlay asks the client to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboxIntent {
    /// Send this answer to `POST /items/:id/answer` now.
    Answer {
        item: ItemId,
        req: AnswerReq,
    },
    /// Ask for text first (a deny note, a send-back note or a reply), then
    /// submit it through [`super::forms::NoteForm`].
    Prompt {
        item: ItemId,
        action: AnswerAction,
    },
    /// `↵`: jump to the asking agent.
    Jump(AgentId),
    Close,
}

/// The pure key mapping for one item (UI design 3.5, overlay keys):
/// `y` allow or accept, `Y` allow always, `n` deny or send back (asks for a
/// note), `r` reply (asks for text), `d` dismiss, `↵` jump.
pub fn inbox_key(item: &ItemView, key: Key) -> Option<InboxIntent> {
    let answer = |action| InboxIntent::Answer {
        item: item.id,
        req: AnswerReq { action, text: None },
    };
    let prompt = |action| InboxIntent::Prompt {
        item: item.id,
        action,
    };
    match (key, &item.kind) {
        (Key::Char('y'), ItemKind::Permission { .. }) => Some(answer(AnswerAction::Allow)),
        (Key::Char('y'), ItemKind::PlanReview) => Some(answer(AnswerAction::Accept)),
        (Key::Char('Y'), ItemKind::Permission { .. }) => Some(answer(AnswerAction::AllowAlways)),
        (Key::Char('n'), ItemKind::Permission { .. }) => Some(prompt(AnswerAction::Deny)),
        (Key::Char('n'), ItemKind::PlanReview) => Some(prompt(AnswerAction::SendBack)),
        (Key::Char('r'), ItemKind::Question) => Some(prompt(AnswerAction::Reply)),
        (Key::Char('d'), _) => Some(answer(AnswerAction::Dismiss)),
        (Key::Enter, _) => item.from.clone().map(InboxIntent::Jump),
        _ => None,
    }
}

/// A key in the overlay: `↑↓`/`jk` move, `Esc`/`q` close, the rest go to
/// [`inbox_key`] for the selected item.
pub fn handle_inbox_key(sv: &StateView, vs: &mut ViewState, key: Key) -> Option<InboxIntent> {
    match key {
        Key::Up | Key::Char('k') => {
            nav::inbox_move(sv, vs, -1);
            None
        }
        Key::Down | Key::Char('j') => {
            nav::inbox_move(sv, vs, 1);
            None
        }
        Key::Esc | Key::Char('q') => Some(InboxIntent::Close),
        k => {
            let items = rows::needs_you(sv);
            let item = items.get(vs.inbox_sel.min(items.len().saturating_sub(1)))?;
            inbox_key(item, k)
        }
    }
}

/// One line of key hints for the item, shown under the overlay.
pub fn inbox_hints(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::Permission { .. } => "y allow  Y always  n deny  d dismiss  ↵ open  esc close",
        ItemKind::Question => "r reply  d dismiss  ↵ open  esc close",
        ItemKind::PlanReview => "y accept  n send back  d dismiss  ↵ open  esc close",
        _ => "d dismiss  ↵ open  esc close",
    }
}

#[cfg(test)]
#[path = "intent_tests.rs"]
mod tests;
