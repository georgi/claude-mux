use super::*;
use crate::tui::fixture::{plan_id, state, view_state};
use cmux_core::ids::AgentPath;

fn item(sv: &StateView, id: u64) -> ItemView {
    sv.item(ItemId(id)).unwrap().clone()
}

fn answer(item: u64, action: AnswerAction) -> Option<InboxIntent> {
    Some(InboxIntent::Answer {
        item: ItemId(item),
        req: AnswerReq { action, text: None },
    })
}

#[test]
fn permission_keys_map_to_answers() {
    let sv = state();
    let perm = item(&sv, 1);
    assert_eq!(
        inbox_key(&perm, Key::Char('y')),
        answer(1, AnswerAction::Allow)
    );
    assert_eq!(
        inbox_key(&perm, Key::Char('Y')),
        answer(1, AnswerAction::AllowAlways)
    );
    assert_eq!(
        inbox_key(&perm, Key::Char('n')),
        Some(InboxIntent::Prompt {
            item: ItemId(1),
            action: AnswerAction::Deny
        })
    );
    assert_eq!(
        inbox_key(&perm, Key::Char('d')),
        answer(1, AnswerAction::Dismiss)
    );
    assert_eq!(inbox_key(&perm, Key::Char('r')), None);
    assert!(matches!(
        inbox_key(&perm, Key::Enter),
        Some(InboxIntent::Jump(a)) if a.path() == &AgentPath::Task(cmux_proto::view::TaskId::new(4))
    ));
}

#[test]
fn question_and_plan_review_keys() {
    let sv = state();
    let q = item(&sv, 2);
    assert_eq!(
        inbox_key(&q, Key::Char('r')),
        Some(InboxIntent::Prompt {
            item: ItemId(2),
            action: AnswerAction::Reply
        })
    );
    // y/Y/n mean nothing for a question.
    for c in ['y', 'Y', 'n'] {
        assert_eq!(inbox_key(&q, Key::Char(c)), None);
    }
    let mut review = q.clone();
    review.kind = ItemKind::PlanReview;
    assert_eq!(
        inbox_key(&review, Key::Char('y')),
        answer(2, AnswerAction::Accept)
    );
    assert_eq!(
        inbox_key(&review, Key::Char('n')),
        Some(InboxIntent::Prompt {
            item: ItemId(2),
            action: AnswerAction::SendBack
        })
    );
    assert_eq!(inbox_key(&review, Key::Char('Y')), None);
}

#[test]
fn overlay_keys_move_select_and_close() {
    let sv = state();
    let mut vs = view_state();
    // First row is the permission (older), second the question.
    assert_eq!(handle_inbox_key(&sv, &mut vs, Key::Down), None);
    assert_eq!(vs.inbox_sel, 1);
    assert_eq!(
        handle_inbox_key(&sv, &mut vs, Key::Char('r')),
        Some(InboxIntent::Prompt {
            item: ItemId(2),
            action: AnswerAction::Reply
        })
    );
    assert_eq!(handle_inbox_key(&sv, &mut vs, Key::Char('k')), None);
    assert_eq!(
        handle_inbox_key(&sv, &mut vs, Key::Char('y')),
        answer(1, AnswerAction::Allow)
    );
    assert_eq!(
        handle_inbox_key(&sv, &mut vs, Key::Esc),
        Some(InboxIntent::Close)
    );
    // Nothing selected on an empty inbox.
    assert_eq!(
        handle_inbox_key(&StateView::default(), &mut vs, Key::Char('y')),
        None
    );
}

#[test]
fn answers_convert_to_engine_actions() {
    let sv = state();
    let Some(InboxIntent::Answer { req, .. }) = inbox_key(&item(&sv, 1), Key::Char('Y')) else {
        panic!()
    };
    assert_eq!(
        req.into_action().unwrap(),
        cmux_core::engine::ItemAction::AllowAlways
    );
}

#[test]
fn tree_keys_move_and_emit_intents() {
    let sv = state();
    let mut vs = view_state();
    let t2 = Node::Task(plan_id(), cmux_proto::view::TaskId::new(2));
    assert_eq!(handle_tree_key(&sv, &mut vs, Key::Char('j')), None);
    assert_eq!(
        vs.selection.as_ref().map(|n| matches!(n, Node::Sub(_))),
        Some(true)
    );
    assert_eq!(handle_tree_key(&sv, &mut vs, Key::Char('k')), None);
    assert_eq!(vs.selection, Some(t2.clone()));

    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Enter),
        Some(TreeIntent::Open(t2.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char(' ')),
        Some(TreeIntent::Preview(t2.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('n')),
        Some(TreeIntent::NewPlan)
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('t')),
        Some(TreeIntent::NewTask(plan_id()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('A')),
        Some(TreeIntent::Accept(t2.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('S')),
        Some(TreeIntent::TogglePause(plan_id()))
    );
    let agent = AgentId::new(plan_id(), AgentPath::Task(cmux_proto::view::TaskId::new(2)));
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('s')),
        Some(TreeIntent::Message(agent.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('x')),
        Some(TreeIntent::Stop(agent.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('R')),
        Some(TreeIntent::Resume(agent))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('c')),
        Some(TreeIntent::Cancel(
            plan_id(),
            cmux_proto::view::TaskId::new(2)
        ))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('o')),
        Some(TreeIntent::OpenPr(
            plan_id(),
            cmux_proto::view::TaskId::new(2)
        ))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('e')),
        Some(TreeIntent::Edit(t2.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('g')),
        Some(TreeIntent::Lazygit(t2.clone()))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('D')),
        Some(TreeIntent::DiffPager(t2))
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('v')),
        Some(TreeIntent::Board)
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('i')),
        Some(TreeIntent::Inbox)
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('?')),
        Some(TreeIntent::Help)
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Esc),
        Some(TreeIntent::Back)
    );
    assert_eq!(
        handle_tree_key(&sv, &mut vs, Key::Char('/')),
        Some(TreeIntent::Filter)
    );
    assert_eq!(handle_tree_key(&sv, &mut vs, Key::Char('z')), None);
}

#[test]
fn tree_keys_that_need_a_target_are_ignored_without_one() {
    let sv = state();
    let mut vs = view_state();
    // T-0003 has no agent and no PR; the plan row has no agent.
    vs.selection = Some(Node::Task(plan_id(), cmux_proto::view::TaskId::new(3)));
    for c in ['s', 'x', 'R', 'o'] {
        assert_eq!(handle_tree_key(&sv, &mut vs, Key::Char(c)), None, "{c}");
    }
    vs.selection = Some(Node::Plan(plan_id()));
    for c in ['s', 'c', 'g', 'D'] {
        assert_eq!(handle_tree_key(&sv, &mut vs, Key::Char(c)), None, "{c}");
    }
    assert!(handle_tree_key(&sv, &mut vs, Key::Char('A')).is_some());
    vs.selection = None;
    assert_eq!(handle_tree_key(&sv, &mut vs, Key::Enter), None);
    assert_eq!(handle_tree_key(&sv, &mut vs, Key::Char('t')), None);
}
