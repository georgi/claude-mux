//! Mouse hit-tests: a screen cell and the rectangle a widget was drawn in
//! give the tree row, board card or overlay item under it. They use the same
//! layout functions as drawing, so they cannot drift apart.

use ratatui::layout::{Position, Rect};

use super::draw::board::card_slots;
use super::draw::inbox::inbox_layout;
use super::draw::tree::{TreeMode, full_columns, row_at, tree_layout};
use super::nav::selected_index;
use super::rows::{needs_you, tree_rows, window_start};
use super::state::{Node, ViewState};
use cmux_proto::view::{ItemId, PlanId, StateView, TaskId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeHit {
    /// A click on the row: select it (double-click also focuses its pane).
    Row(Node),
    /// A click on the expand arrow: toggle collapse
    /// ([`super::nav::toggle`]).
    Arrow(Node),
    /// A click on a needs-you row in the full tree: jump to its agent.
    NeedsYou(ItemId),
    /// A click on the strip's `⚑N`: open the overlay.
    NeedsSummary,
}

/// What is at `(x, y)` in the tree drawn in `area` with `mode`.
pub fn tree_hit(
    area: Rect,
    mode: TreeMode,
    sv: &StateView,
    vs: &ViewState,
    x: u16,
    y: u16,
) -> Option<TreeHit> {
    let pos = Position::new(x, y);
    if !area.contains(pos) {
        return None;
    }
    let items = needs_you(sv);
    let lay = tree_layout(area, mode, items.len());
    if let Some(n) = lay.needs.filter(|n| n.contains(pos)) {
        return match mode {
            TreeMode::Strip => Some(TreeHit::NeedsSummary),
            TreeMode::Full => items
                .get((y - n.y) as usize)
                .map(|i| TreeHit::NeedsYou(i.id)),
        };
    }
    if !lay.rows.contains(pos) {
        return None;
    }
    let rows = tree_rows(sv, vs);
    let idx = row_at(lay.rows, rows.len(), selected_index(&rows, vs), y)?;
    let row = &rows[idx];
    let arrow_col = match mode {
        TreeMode::Full => Some(full_columns(row).0),
        TreeMode::Strip => (row.depth == 0).then_some(0),
    };
    if row.expandable && arrow_col == Some(x - lay.rows.x) {
        Some(TreeHit::Arrow(row.node.clone()))
    } else {
        Some(TreeHit::Row(row.node.clone()))
    }
}

/// The board card under `(x, y)` in a board drawn in `area`.
pub fn board_hit(
    area: Rect,
    sv: &StateView,
    vs: &ViewState,
    plan: &PlanId,
    x: u16,
    y: u16,
) -> Option<TaskId> {
    let pos = Position::new(x, y);
    card_slots(sv, vs, plan, area)
        .into_iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, t)| t)
}

/// The overlay item under `(x, y)`: its index in [`needs_you`] and id. A click
/// outside the popup is `None` (the client may close the overlay).
pub fn inbox_hit(
    area: Rect,
    sv: &StateView,
    vs: &ViewState,
    x: u16,
    y: u16,
) -> Option<(usize, ItemId)> {
    let pos = Position::new(x, y);
    let lay = inbox_layout(area);
    if !lay.list.contains(pos) {
        return None;
    }
    let items = needs_you(sv);
    let sel = vs.inbox_sel.min(items.len().saturating_sub(1));
    let idx = window_start(sel, lay.list.height as usize) + (y - lay.list.y) as usize;
    items.get(idx).map(|i| (idx, i.id))
}

/// True when `(x, y)` is inside the overlay popup.
pub fn in_inbox_popup(area: Rect, x: u16, y: u16) -> bool {
    inbox_layout(area).popup.contains(Position::new(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::draw::tree::{STRIP_WIDTH, tree_width};
    use crate::tui::fixture;
    use crate::tui::nav;
    use cmux_proto::view::TaskId;

    fn setup() -> (StateView, ViewState) {
        let sv = fixture::state();
        let mut vs = fixture::view_state();
        nav::fix_selection(&sv, &mut vs);
        (sv, vs)
    }

    #[test]
    fn full_tree_rows_and_arrows() {
        let (sv, vs) = setup();
        let area = Rect::new(0, 0, tree_width(100), 30);
        // Row 0 sits inside the border at y = 1.
        let plan = fixture::plan_id();
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 10, 1),
            Some(TreeHit::Row(Node::Plan(plan.clone())))
        );
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 1, 1),
            Some(TreeHit::Arrow(Node::Plan(plan.clone())))
        );
        // Row 1 is the executor, row 2 task T-0001, then T-0002 and its subs.
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 8, 2),
            Some(TreeHit::Row(Node::Executor(plan.clone())))
        );
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 8, 4),
            Some(TreeHit::Row(Node::Task(plan.clone(), TaskId::new(2))))
        );
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 3, 4),
            Some(TreeHit::Arrow(Node::Task(plan, TaskId::new(2))))
        );
    }

    #[test]
    fn outside_and_border_miss() {
        let (sv, vs) = setup();
        let area = Rect::new(0, 0, 28, 30);
        assert_eq!(tree_hit(area, TreeMode::Full, &sv, &vs, 0, 5), None);
        assert_eq!(tree_hit(area, TreeMode::Full, &sv, &vs, 40, 5), None);
        assert_eq!(tree_hit(area, TreeMode::Full, &sv, &vs, 5, 0), None);
        // Below the last row (the fixture has fewer rows than the area).
        assert_eq!(tree_hit(area, TreeMode::Full, &sv, &vs, 5, 22), None);
    }

    #[test]
    fn needs_you_rows_in_full_tree_and_strip() {
        let (sv, vs) = setup();
        let area = Rect::new(0, 0, 28, 30);
        let lay = tree_layout(area, TreeMode::Full, 2);
        let n = lay.needs.unwrap();
        let items = needs_you(&sv);
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, n.x + 3, n.y),
            Some(TreeHit::NeedsYou(items[0].id))
        );
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, n.x + 3, n.y + 1),
            Some(TreeHit::NeedsYou(items[1].id))
        );
        // The separator row is neither list nor tree.
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 5, lay.separator.unwrap()),
            None
        );
        let strip = Rect::new(0, 0, STRIP_WIDTH, 24);
        assert_eq!(
            tree_hit(strip, TreeMode::Strip, &sv, &vs, 1, 23),
            Some(TreeHit::NeedsSummary)
        );
    }

    #[test]
    fn strip_rows() {
        let (sv, vs) = setup();
        let area = Rect::new(0, 0, STRIP_WIDTH, 24);
        let plan = fixture::plan_id();
        assert_eq!(
            tree_hit(area, TreeMode::Strip, &sv, &vs, 0, 0),
            Some(TreeHit::Arrow(Node::Plan(plan.clone())))
        );
        assert_eq!(
            tree_hit(area, TreeMode::Strip, &sv, &vs, 2, 0),
            Some(TreeHit::Row(Node::Plan(plan.clone())))
        );
        assert_eq!(
            tree_hit(area, TreeMode::Strip, &sv, &vs, 1, 1),
            Some(TreeHit::Row(Node::Executor(plan)))
        );
    }

    #[test]
    fn hits_follow_the_scroll_window() {
        let (sv, mut vs) = setup();
        let sub = cmux_proto::view::AgentId::new(
            fixture::plan_id(),
            cmux_core::ids::AgentPath::sub(TaskId::new(2), "docs").unwrap(),
        );
        nav::select_agent(&sv, &mut vs, &sub);
        // Three tree rows fit above the needs-you list; the selected sub-agent
        // is pinned to the bottom visible row.
        let area = Rect::new(0, 0, 30, 7);
        assert_eq!(
            tree_hit(area, TreeMode::Full, &sv, &vs, 10, 3),
            Some(TreeHit::Row(Node::Sub(sub)))
        );
    }

    #[test]
    fn board_cards() {
        let (sv, vs) = setup();
        let plan = fixture::plan_id();
        let area = Rect::new(0, 0, 100, 30);
        let slots = card_slots(&sv, &vs, &plan, area);
        assert!(!slots.is_empty());
        for (r, id) in &slots {
            assert_eq!(
                board_hit(area, &sv, &vs, &plan, r.x + 2, r.y + 1),
                Some(*id)
            );
        }
        // The header row and the column title row hit nothing.
        assert_eq!(board_hit(area, &sv, &vs, &plan, 5, 0), None);
        assert_eq!(board_hit(area, &sv, &vs, &plan, 5, 1), None);
    }

    #[test]
    fn inbox_items() {
        let (sv, vs) = setup();
        let area = Rect::new(0, 0, 100, 30);
        let lay = inbox_layout(area);
        let items = needs_you(&sv);
        assert_eq!(
            inbox_hit(area, &sv, &vs, lay.list.x + 4, lay.list.y + 1),
            Some((1, items[1].id))
        );
        assert_eq!(inbox_hit(area, &sv, &vs, lay.list.x, lay.list.y + 2), None);
        assert_eq!(inbox_hit(area, &sv, &vs, 0, 0), None);
        assert!(in_inbox_popup(area, lay.popup.x, lay.popup.y));
        assert!(!in_inbox_popup(area, 0, 0));
    }
}
