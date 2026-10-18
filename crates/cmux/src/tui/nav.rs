//! Tree, board and overlay navigation as pure functions over
//! `(StateView, ViewState)`.

use cmux_proto::view::{AgentId, PlanId, StateView, TaskId, TaskState, TaskView};

use super::rows::{self, Row};
use super::state::{Node, ViewState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    /// `←`/`h`: collapse, else go to the parent.
    Left,
    /// `→`/`l`: expand, else go to the first child.
    Right,
    First,
    Last,
}

/// Index of the selection in `rows`, if it is visible.
pub fn selected_index(rows: &[Row], vs: &ViewState) -> Option<usize> {
    let sel = vs.selection.as_ref()?;
    rows.iter().position(|r| &r.node == sel)
}

/// Keep the selection valid after the state changed: a vanished or hidden
/// node moves the selection to its nearest visible ancestor, or the first row.
pub fn fix_selection(sv: &StateView, vs: &mut ViewState) {
    let rows = rows::tree_rows(sv, vs);
    if rows.is_empty() {
        vs.selection = None;
        return;
    }
    if selected_index(&rows, vs).is_some() {
        return;
    }
    let mut cur = vs.selection.as_ref().and_then(Node::parent);
    while let Some(n) = cur {
        if rows.iter().any(|r| r.node == n) {
            vs.selection = Some(n);
            return;
        }
        cur = n.parent();
    }
    vs.selection = Some(rows[0].node.clone());
}

/// Apply a movement; returns the new selection.
pub fn navigate(sv: &StateView, vs: &mut ViewState, mv: Move) -> Option<Node> {
    let rows = rows::tree_rows(sv, vs);
    if rows.is_empty() {
        vs.selection = None;
        return None;
    }
    let cur = selected_index(&rows, vs);
    let Some(i) = cur else {
        vs.selection = Some(rows[0].node.clone());
        return vs.selection.clone();
    };
    let target = match mv {
        Move::Up => i.saturating_sub(1),
        Move::Down => (i + 1).min(rows.len() - 1),
        Move::First => 0,
        Move::Last => rows.len() - 1,
        Move::Left => {
            let r = &rows[i];
            if r.expandable && r.expanded {
                vs.collapsed.insert(r.node.clone());
                i
            } else {
                r.node
                    .parent()
                    .and_then(|p| rows.iter().position(|x| x.node == p))
                    .unwrap_or(i)
            }
        }
        Move::Right => {
            let r = &rows[i];
            if r.expandable && !r.expanded {
                vs.collapsed.remove(&r.node);
                i
            } else if r.expandable && rows.get(i + 1).is_some_and(|n| n.depth > r.depth) {
                i + 1
            } else {
                i
            }
        }
    };
    vs.selection = Some(rows[target].node.clone());
    vs.selection.clone()
}

/// Toggle collapse of the selected node (the click on the arrow).
pub fn toggle(sv: &StateView, vs: &mut ViewState, node: &Node) {
    let rows = rows::tree_rows(sv, vs);
    if rows.iter().any(|r| &r.node == node && r.expandable) {
        if !vs.collapsed.remove(node) {
            vs.collapsed.insert(node.clone());
        }
        fix_selection(sv, vs);
    }
}

/// Expand every ancestor of `node` and select it. False when `node` is not in
/// the tree.
pub fn reveal(sv: &StateView, vs: &mut ViewState, node: &Node) -> bool {
    if !rows::all_rows(sv, vs).iter().any(|r| &r.node == node) {
        return false;
    }
    let mut cur = node.parent();
    while let Some(n) = cur {
        vs.collapsed.remove(&n);
        cur = n.parent();
    }
    vs.selection = Some(node.clone());
    true
}

/// Select an agent's node, expanding what hides it.
pub fn select_agent(sv: &StateView, vs: &mut ViewState, agent: &AgentId) -> bool {
    reveal(sv, vs, &Node::for_agent(agent))
}

/// `M-a` / `M-A`: select the next (or previous) node with an item that needs
/// a human, in tree order, wrapping around. Returns the node.
pub fn jump_needs_you(sv: &StateView, vs: &mut ViewState, forward: bool) -> Option<Node> {
    let all = rows::all_rows(sv, vs);
    let wanted: Vec<Node> = rows::needs_you(sv)
        .into_iter()
        .filter_map(rows::item_node)
        .collect();
    let positions: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, r)| wanted.contains(&r.node))
        .map(|(i, _)| i)
        .collect();
    if positions.is_empty() {
        return None;
    }
    let cur = vs
        .selection
        .as_ref()
        .and_then(|s| all.iter().position(|r| &r.node == s));
    let pick = match (cur, forward) {
        (None, true) => positions[0],
        (None, false) => positions[positions.len() - 1],
        (Some(c), true) => positions
            .iter()
            .copied()
            .find(|p| *p > c)
            .unwrap_or(positions[0]),
        (Some(c), false) => positions
            .iter()
            .rev()
            .copied()
            .find(|p| *p < c)
            .unwrap_or(positions[positions.len() - 1]),
    };
    let node = all[pick].node.clone();
    reveal(sv, vs, &node);
    Some(node)
}

/// Move the needs-you overlay selection, clamped.
pub fn inbox_move(sv: &StateView, vs: &mut ViewState, delta: isize) {
    let n = rows::needs_you(sv).len();
    if n == 0 {
        vs.inbox_sel = 0;
        return;
    }
    let next = vs.inbox_sel.saturating_add_signed(delta);
    vs.inbox_sel = next.min(n - 1);
}

// ---------------------------------------------------------------------------
// Board
// ---------------------------------------------------------------------------

/// Board columns: a title and the task states each one holds.
pub const BOARD_COLUMNS: [(&str, &[TaskState]); 5] = [
    ("todo", &[TaskState::Todo]),
    (
        "active",
        &[
            TaskState::InProgress,
            TaskState::Testing,
            TaskState::Failing,
        ],
    ),
    ("passing", &[TaskState::Passing]),
    ("merged", &[TaskState::Merged]),
    ("blocked", &[TaskState::Blocked, TaskState::Cancelled]),
];

/// The plan's tasks per board column, in plan order.
pub fn board_columns<'a>(sv: &'a StateView, plan: &PlanId) -> Vec<Vec<&'a TaskView>> {
    let tasks: Vec<&TaskView> = sv
        .plan(plan)
        .map(|p| p.tasks.iter().filter_map(|t| sv.task(plan, *t)).collect())
        .unwrap_or_default();
    BOARD_COLUMNS
        .iter()
        .map(|(_, states)| {
            tasks
                .iter()
                .copied()
                .filter(|t| states.contains(&t.state))
                .collect()
        })
        .collect()
}

/// The board's selected task: the stored one if it is on the board, else the
/// first card of the first non-empty column.
pub fn board_selected(sv: &StateView, vs: &ViewState, plan: &PlanId) -> Option<TaskId> {
    let cols = board_columns(sv, plan);
    let stored = vs
        .board_sel
        .filter(|s| cols.iter().flatten().any(|t| t.id == *s));
    stored.or_else(|| cols.iter().flatten().next().map(|t| t.id))
}

/// Position `(column, row)` of a task on the board.
pub fn board_pos(sv: &StateView, plan: &PlanId, task: TaskId) -> Option<(usize, usize)> {
    board_columns(sv, plan)
        .iter()
        .enumerate()
        .find_map(|(c, col)| col.iter().position(|t| t.id == task).map(|r| (c, r)))
}

/// Move the board selection. Left/right skip empty columns and keep the row
/// index when the neighbour is long enough.
pub fn board_move(sv: &StateView, vs: &mut ViewState, plan: &PlanId, mv: Move) {
    let cols = board_columns(sv, plan);
    let Some(cur) = board_selected(sv, vs, plan) else {
        return;
    };
    let Some((c, r)) = board_pos(sv, plan, cur) else {
        return;
    };
    let target = match mv {
        Move::Up => (r > 0).then(|| cols[c][r - 1].id),
        Move::Down => cols[c].get(r + 1).map(|t| t.id),
        Move::Left | Move::Right => {
            let mut k = c;
            loop {
                k = if mv == Move::Left {
                    match k.checked_sub(1) {
                        Some(k) => k,
                        None => break None,
                    }
                } else {
                    k + 1
                };
                if k >= cols.len() {
                    break None;
                }
                if !cols[k].is_empty() {
                    break Some(cols[k][r.min(cols[k].len() - 1)].id);
                }
            }
        }
        Move::First => cols[c].first().map(|t| t.id),
        Move::Last => cols[c].last().map(|t| t.id),
    };
    if let Some(t) = target {
        vs.board_sel = Some(t);
    }
}

#[cfg(test)]
#[path = "nav_tests.rs"]
mod tests;
