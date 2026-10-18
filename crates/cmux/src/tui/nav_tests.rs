use super::*;
use crate::tui::fixture::{plan_id, state, view_state};
use cmux_core::ids::AgentPath;

fn task(n: u32) -> Node {
    Node::Task(plan_id(), TaskId::new(n))
}

fn sub(name: &str) -> Node {
    Node::Sub(AgentId::new(
        plan_id(),
        AgentPath::sub(TaskId::new(2), name).unwrap(),
    ))
}

#[test]
fn up_down_walk_visible_rows_and_clamp() {
    let sv = state();
    let mut vs = view_state();
    assert_eq!(navigate(&sv, &mut vs, Move::Down), Some(sub("tests")));
    assert_eq!(navigate(&sv, &mut vs, Move::Down), Some(sub("docs")));
    assert_eq!(navigate(&sv, &mut vs, Move::Down), Some(task(3)));
    assert_eq!(navigate(&sv, &mut vs, Move::Up), Some(sub("docs")));
    navigate(&sv, &mut vs, Move::First);
    assert_eq!(vs.selection, Some(Node::Plan(plan_id())));
    navigate(&sv, &mut vs, Move::Up);
    assert_eq!(vs.selection, Some(Node::Plan(plan_id())));
    navigate(&sv, &mut vs, Move::Last);
    assert_eq!(vs.selection, Some(Node::Loose));
    navigate(&sv, &mut vs, Move::Down);
    assert_eq!(vs.selection, Some(Node::Loose));
}

#[test]
fn left_collapses_then_goes_to_parent_and_right_expands_then_descends() {
    let sv = state();
    let mut vs = view_state();
    // T-0002 is expanded: left collapses it, a second left goes to the plan.
    navigate(&sv, &mut vs, Move::Left);
    assert!(vs.is_collapsed(&task(2)));
    assert_eq!(vs.selection, Some(task(2)));
    assert!(
        !rows::tree_rows(&sv, &vs)
            .iter()
            .any(|r| r.node == sub("tests"))
    );
    navigate(&sv, &mut vs, Move::Left);
    assert_eq!(vs.selection, Some(Node::Plan(plan_id())));
    // Left on an expanded plan collapses it; right expands it again.
    navigate(&sv, &mut vs, Move::Left);
    assert!(vs.is_collapsed(&Node::Plan(plan_id())));
    navigate(&sv, &mut vs, Move::Right);
    assert!(!vs.is_collapsed(&Node::Plan(plan_id())));
    // Right on an expanded plan enters its first child (the executor).
    navigate(&sv, &mut vs, Move::Right);
    assert_eq!(vs.selection, Some(Node::Executor(plan_id())));
    // Right on a leaf does nothing; left on a leaf goes to the parent.
    navigate(&sv, &mut vs, Move::Right);
    assert_eq!(vs.selection, Some(Node::Executor(plan_id())));
    navigate(&sv, &mut vs, Move::Left);
    assert_eq!(vs.selection, Some(Node::Plan(plan_id())));
}

#[test]
fn collapsing_the_plan_hides_everything_below_it() {
    let sv = state();
    let mut vs = view_state();
    toggle(&sv, &mut vs, &Node::Plan(plan_id()));
    // The selected task vanished, so the selection moved up to the plan.
    assert_eq!(vs.selection, Some(Node::Plan(plan_id())));
    assert_eq!(rows::tree_rows(&sv, &vs).len(), 4); // three plans and loose
    toggle(&sv, &mut vs, &Node::Loose); // not expandable: no-op
    assert_eq!(rows::tree_rows(&sv, &vs).len(), 4);
}

#[test]
fn fix_selection_recovers_from_removed_nodes() {
    let mut sv = state();
    let mut vs = view_state();
    vs.selection = Some(sub("docs"));
    sv.agents.retain(|a| a.id.path().sub_name() != Some("docs"));
    fix_selection(&sv, &mut vs);
    assert_eq!(vs.selection, Some(task(2)));
    vs.selection = None;
    fix_selection(&sv, &mut vs);
    assert_eq!(vs.selection, Some(Node::Plan(plan_id())));
    let empty = StateView::default();
    fix_selection(&empty, &mut vs);
    assert_eq!(vs.selection, None);
    assert_eq!(navigate(&empty, &mut vs, Move::Down), None);
}

#[test]
fn jump_cycles_through_needs_you_nodes_in_tree_order() {
    let sv = state();
    let mut vs = view_state();
    // The executor (question) comes before T-0004 (permission) in the tree;
    // the selection starts on T-0002, so forward reaches T-0004 first.
    assert_eq!(jump_needs_you(&sv, &mut vs, true), Some(task(4)));
    assert_eq!(
        jump_needs_you(&sv, &mut vs, true),
        Some(Node::Executor(plan_id()))
    );
    assert_eq!(jump_needs_you(&sv, &mut vs, true), Some(task(4)));
    assert_eq!(
        jump_needs_you(&sv, &mut vs, false),
        Some(Node::Executor(plan_id()))
    );
    assert_eq!(jump_needs_you(&sv, &mut vs, false), Some(task(4)));
}

#[test]
fn jump_reveals_collapsed_targets_and_handles_none() {
    let sv = state();
    let mut vs = view_state();
    vs.collapsed.insert(Node::Plan(plan_id()));
    vs.selection = Some(Node::Plan(plan_id()));
    let n = jump_needs_you(&sv, &mut vs, true).unwrap();
    assert_eq!(n, Node::Executor(plan_id()));
    assert!(!vs.is_collapsed(&Node::Plan(plan_id())));
    assert_eq!(vs.selection, Some(n));
    let mut quiet = state();
    quiet.items.clear();
    assert_eq!(jump_needs_you(&quiet, &mut vs, true), None);
}

#[test]
fn select_agent_expands_hidden_sub_agents() {
    let sv = state();
    let mut vs = view_state();
    vs.collapsed.insert(task(2));
    let id = AgentId::new(plan_id(), AgentPath::sub(TaskId::new(2), "tests").unwrap());
    assert!(select_agent(&sv, &mut vs, &id));
    assert!(!vs.is_collapsed(&task(2)));
    assert_eq!(vs.selection, Some(sub("tests")));
    let ghost = AgentId::new(plan_id(), AgentPath::Task(TaskId::new(99)));
    assert!(!select_agent(&sv, &mut vs, &ghost));
}

#[test]
fn inbox_selection_clamps() {
    let sv = state();
    let mut vs = view_state();
    inbox_move(&sv, &mut vs, -1);
    assert_eq!(vs.inbox_sel, 0);
    inbox_move(&sv, &mut vs, 1);
    inbox_move(&sv, &mut vs, 1);
    assert_eq!(vs.inbox_sel, 1);
    inbox_move(&sv, &mut vs, 5);
    assert_eq!(vs.inbox_sel, 1);
    inbox_move(&StateView::default(), &mut vs, 1);
    assert_eq!(vs.inbox_sel, 0);
}

#[test]
fn board_groups_tasks_by_state() {
    let sv = state();
    let cols = board_columns(&sv, &plan_id());
    let ids = |c: usize| cols[c].iter().map(|t| t.id.seq()).collect::<Vec<_>>();
    assert_eq!(ids(0), vec![3, 5]);
    assert_eq!(ids(1), vec![2, 4]);
    assert_eq!(ids(2), vec![6]);
    assert_eq!(ids(3), vec![1]);
    assert_eq!(ids(4), Vec::<u32>::new());
    let none = PlanId::new(2020, 1, 1, "none");
    assert!(board_columns(&sv, &none).iter().all(Vec::is_empty));
}

#[test]
fn board_selection_moves_across_columns_skipping_empty_ones() {
    let sv = state();
    let plan = plan_id();
    let mut vs = view_state();
    assert_eq!(board_selected(&sv, &vs, &plan), Some(TaskId::new(3)));
    board_move(&sv, &mut vs, &plan, Move::Down);
    assert_eq!(vs.board_sel, Some(TaskId::new(5)));
    board_move(&sv, &mut vs, &plan, Move::Down);
    assert_eq!(vs.board_sel, Some(TaskId::new(5)));
    // Right keeps the row where it can: row 1 of "active" is T-0004.
    board_move(&sv, &mut vs, &plan, Move::Right);
    assert_eq!(vs.board_sel, Some(TaskId::new(4)));
    board_move(&sv, &mut vs, &plan, Move::Right);
    board_move(&sv, &mut vs, &plan, Move::Right);
    assert_eq!(vs.board_sel, Some(TaskId::new(1)));
    // The blocked column is empty, so right stays.
    board_move(&sv, &mut vs, &plan, Move::Right);
    assert_eq!(vs.board_sel, Some(TaskId::new(1)));
    board_move(&sv, &mut vs, &plan, Move::Left);
    assert_eq!(vs.board_sel, Some(TaskId::new(6)));
    board_move(&sv, &mut vs, &plan, Move::Up);
    assert_eq!(vs.board_sel, Some(TaskId::new(6)));
    // A stale selection falls back to the first card.
    vs.board_sel = Some(TaskId::new(42));
    assert_eq!(board_selected(&sv, &vs, &plan), Some(TaskId::new(3)));
}
