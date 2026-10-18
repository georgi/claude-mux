//! The plan tree (UI design 3.2): the full tree with the needs-you list at
//! its bottom, and the 3-column glyph strip used below 100 columns.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};
use unicode_width::UnicodeWidthStr;

use super::{clip, put, put_right};
use crate::tui::rows::{Row, item_label, needs_you, tree_rows, window_start};
use crate::tui::state::{Focus, ViewState};
use crate::tui::theme;
use cmux_proto::view::StateView;

/// Below this many terminal columns the tree collapses to the strip.
pub const FULL_MIN_COLS: u16 = 100;
pub const STRIP_WIDTH: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeMode {
    Full,
    Strip,
}

/// Strip below 100 columns, else the full tree.
pub fn tree_mode(total_cols: u16) -> TreeMode {
    if total_cols < FULL_MIN_COLS {
        TreeMode::Strip
    } else {
        TreeMode::Full
    }
}

/// Width of the tree column for a terminal that is `total_cols` wide.
pub fn tree_width(total_cols: u16) -> u16 {
    match tree_mode(total_cols) {
        TreeMode::Strip => STRIP_WIDTH,
        TreeMode::Full => (total_cols / 5).clamp(30, 40),
    }
}

/// Where the parts of the tree widget sit inside its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLayout {
    /// The tree rows.
    pub rows: Rect,
    /// The needs-you list (full) or its one-line summary (strip), when there
    /// are items.
    pub needs: Option<Rect>,
    /// Row of the `needs you N` separator (full mode only).
    pub separator: Option<u16>,
}

pub fn tree_layout(area: Rect, mode: TreeMode, n_needs: usize) -> TreeLayout {
    match mode {
        TreeMode::Strip => {
            if n_needs > 0 && area.height > 1 {
                TreeLayout {
                    rows: Rect {
                        height: area.height - 1,
                        ..area
                    },
                    needs: Some(Rect {
                        y: area.bottom() - 1,
                        height: 1,
                        ..area
                    }),
                    separator: None,
                }
            } else {
                TreeLayout {
                    rows: area,
                    needs: None,
                    separator: None,
                }
            }
        }
        TreeMode::Full => {
            let inner = Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(2),
            };
            let nh = (n_needs as u16).min((inner.height / 3).max(1));
            if n_needs > 0 && inner.height >= nh + 3 {
                let sep = inner.bottom() - nh - 1;
                TreeLayout {
                    rows: Rect {
                        height: sep - inner.y,
                        ..inner
                    },
                    needs: Some(Rect {
                        y: sep + 1,
                        height: nh,
                        ..inner
                    }),
                    separator: Some(sep),
                }
            } else {
                TreeLayout {
                    rows: inner,
                    needs: None,
                    separator: None,
                }
            }
        }
    }
}

/// Draw the tree or strip into `area`.
pub fn render_tree(buf: &mut Buffer, area: Rect, sv: &StateView, vs: &ViewState, mode: TreeMode) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let needs = needs_you(sv);
    let lay = tree_layout(area, mode, needs.len());
    let focused = vs.focus == Focus::Tree;
    let border = if focused { theme::ACCENT } else { theme::GREY };
    if mode == TreeMode::Full {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .title(" plans ")
            .render(area, buf);
    }

    let all = tree_rows(sv, vs);
    let sel = vs
        .selection
        .as_ref()
        .and_then(|s| all.iter().position(|r| &r.node == s));
    let start = window_start(sel.unwrap_or(0), lay.rows.height as usize);
    for (i, row) in all
        .iter()
        .skip(start)
        .take(lay.rows.height as usize)
        .enumerate()
    {
        let y = lay.rows.y + i as u16;
        let selected = sel == Some(start + i);
        match mode {
            TreeMode::Full => draw_row(buf, lay.rows, y, row, selected, focused),
            TreeMode::Strip => draw_strip_row(buf, lay.rows, y, row, selected, focused),
        }
    }

    if let Some(r) = lay.needs {
        match mode {
            TreeMode::Full => {
                if let Some(sep) = lay.separator {
                    draw_separator(buf, area, sep, needs.len(), border);
                }
                for (i, item) in needs.iter().take(r.height as usize).enumerate() {
                    let y = r.y + i as u16;
                    let label = clip(&item_label(sv, item), r.width.saturating_sub(2) as usize);
                    put(buf, r.x, y, 1, "⚑", Style::default().fg(theme::PURPLE));
                    put(
                        buf,
                        r.x + 2,
                        y,
                        r.width.saturating_sub(2),
                        &label,
                        Style::default(),
                    );
                }
            }
            TreeMode::Strip => {
                let s = format!("⚑{}", needs.len());
                put(
                    buf,
                    r.x,
                    r.y,
                    r.width,
                    &s,
                    Style::default().fg(theme::PURPLE),
                );
            }
        }
    }
}

fn draw_separator(buf: &mut Buffer, area: Rect, y: u16, n: usize, color: Color) {
    let st = Style::default().fg(color);
    let right = area.right() - 1;
    put(buf, area.x, y, 1, "├", st);
    for x in area.x + 1..right {
        put(buf, x, y, 1, "─", st);
    }
    put(buf, right, y, 1, "┤", st);
    put(
        buf,
        area.x + 2,
        y,
        area.width.saturating_sub(4),
        &format!(" needs you  {n} "),
        st,
    );
}

fn row_style(selected: bool, focused: bool, dim: bool) -> Style {
    let mut st = Style::default();
    if dim {
        st = st.add_modifier(Modifier::DIM);
    }
    if selected {
        st = if focused {
            st.add_modifier(Modifier::REVERSED)
        } else {
            st.bg(theme::SELECT_BG)
        };
    }
    st
}

/// Column of the expand arrow and of the status mark for a row in the full
/// tree: arrow at `depth * 2`, mark right after it.
pub fn full_columns(row: &Row) -> (u16, u16) {
    let arrow = u16::from(row.depth) * 2;
    (arrow, arrow + 1)
}

fn draw_row(buf: &mut Buffer, rect: Rect, y: u16, row: &Row, selected: bool, focused: bool) {
    let base = row_style(selected, focused, row.dim);
    if selected {
        // Paint the whole row so the highlight reaches the right edge.
        for x in rect.x..rect.right() {
            buf[(x, y)].set_style(base);
        }
    }
    let (arrow_col, mark_col) = full_columns(row);
    let arrow = match (row.expandable, row.expanded) {
        (true, true) => "▾",
        (true, false) => "▸",
        _ => " ",
    };
    put(buf, rect.x + arrow_col, y, 1, arrow, base);
    put(
        buf,
        rect.x + mark_col,
        y,
        1,
        &row.mark.to_string(),
        base.fg(row.mark_color),
    );
    let label_x = mark_col + 2;
    // Sub-agents hang off their task: `├ ⠹ tests`.
    if let Some(b) = row.branch {
        put(buf, rect.x + arrow_col - 1, y, 1, &b.to_string(), base);
    }
    let label = &row.label;
    let avail = rect.width.saturating_sub(label_x);
    let right_w = (row.right.width() as u16).min(avail / 2);
    let label_w = avail.saturating_sub(right_w + 1) as usize;
    put(
        buf,
        rect.x + label_x,
        y,
        label_w as u16,
        &clip(label, label_w),
        base,
    );
    put_right(
        buf,
        rect.right(),
        y,
        right_w,
        &clip(&row.right, right_w as usize),
        base.fg(row.right_color),
    );
}

fn draw_strip_row(buf: &mut Buffer, rect: Rect, y: u16, row: &Row, selected: bool, focused: bool) {
    let base = row_style(selected, focused, row.dim);
    if selected {
        for x in rect.x..rect.right() {
            buf[(x, y)].set_style(base);
        }
    }
    if row.depth == 0 && row.expandable {
        let arrow = if row.expanded { "▾" } else { "▸" };
        put(buf, rect.x, y, 1, arrow, base);
    }
    // Plans and depth-1 rows put the mark in column 1, sub-agents in column 2.
    let col = u16::from(row.depth.max(1)).min(rect.width.saturating_sub(1));
    put(
        buf,
        rect.x + col,
        y,
        1,
        &row.mark.to_string(),
        base.fg(row.mark_color),
    );
}

/// Row index (in [`rows::tree_rows`]) shown at screen row `y`, for hit-tests.
pub(crate) fn row_at(
    rows_rect: Rect,
    total_rows: usize,
    selected: Option<usize>,
    y: u16,
) -> Option<usize> {
    if y < rows_rect.y || y >= rows_rect.bottom() {
        return None;
    }
    let start = window_start(selected.unwrap_or(0), rows_rect.height as usize);
    let idx = start + (y - rows_rect.y) as usize;
    (idx < total_rows).then_some(idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::draw::testutil::dump;
    use crate::tui::fixture;
    use crate::tui::nav;
    use crate::tui::state::Node;
    use cmux_core::ids::AgentPath;
    use cmux_proto::view::AgentId;

    fn snap(cols: u16, rows: u16, mode: TreeMode, focus: Focus) -> String {
        let sv = fixture::state();
        let mut vs = fixture::view_state();
        vs.focus = focus;
        nav::fix_selection(&sv, &mut vs);
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        let w = match mode {
            TreeMode::Strip => STRIP_WIDTH,
            // The overlay at 80 columns is as wide as the 100-column tree.
            TreeMode::Full => tree_width(cols.max(FULL_MIN_COLS)),
        };
        render_tree(&mut buf, Rect::new(0, 0, w, rows), &sv, &vs, mode);
        dump(&buf)
    }

    #[test]
    fn tree_80x24_strip() {
        insta::assert_snapshot!(snap(80, 24, TreeMode::Strip, Focus::Tree));
    }

    #[test]
    fn tree_80x24_overlay() {
        // `prefix Tab` opens the full tree as an overlay on narrow screens.
        insta::assert_snapshot!(snap(80, 24, TreeMode::Full, Focus::Tree));
    }

    #[test]
    fn tree_100x30() {
        insta::assert_snapshot!(snap(100, 30, tree_mode(100), Focus::Tree));
    }

    #[test]
    fn tree_160x50() {
        insta::assert_snapshot!(snap(160, 50, tree_mode(160), Focus::Pane));
    }

    #[test]
    fn mode_and_width_follow_columns() {
        assert_eq!(tree_mode(99), TreeMode::Strip);
        assert_eq!(tree_mode(100), TreeMode::Full);
        assert_eq!(tree_width(80), 3);
        assert_eq!(tree_width(100), 30);
        assert_eq!(tree_width(160), 32);
        assert_eq!(tree_width(300), 40);
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let sv = fixture::state();
        let vs = fixture::view_state();
        for (w, h) in [(0, 0), (1, 1), (3, 2), (5, 3), (28, 4)] {
            for mode in [TreeMode::Full, TreeMode::Strip] {
                let area = Rect::new(0, 0, w, h);
                let mut buf = Buffer::empty(area);
                render_tree(&mut buf, area, &sv, &vs, mode);
            }
        }
    }

    #[test]
    fn selection_scrolls_into_view() {
        let sv = fixture::state();
        let mut vs = fixture::view_state();
        let sub = AgentId::new(
            fixture::plan_id(),
            AgentPath::sub(cmux_proto::view::TaskId::new(2), "docs").unwrap(),
        );
        nav::select_agent(&sv, &mut vs, &sub);
        assert_eq!(vs.selection, Some(Node::Sub(sub)));
        let area = Rect::new(0, 0, 30, 6);
        let mut buf = Buffer::empty(area);
        render_tree(&mut buf, area, &sv, &vs, TreeMode::Full);
        assert!(dump(&buf).contains("docs"));
    }
}
