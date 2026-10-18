//! The needs-you overlay (UI design 2.3 and 3.5): a centred popup with the
//! list, the selected item's detail and the keys that apply to it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};

use super::{centered, clip, put, wrap};
use crate::tui::intent::inbox_hints;
use crate::tui::rows::{item_what, item_who, needs_you, window_start};
use crate::tui::state::ViewState;
use crate::tui::theme;
use cmux_proto::view::{ItemKind, StateView};

const DETAIL_LINES: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboxLayout {
    pub popup: Rect,
    /// One row per item.
    pub list: Rect,
    /// Wrapped body of the selected item.
    pub detail: Rect,
    /// Key hints.
    pub hint: Rect,
}

/// Popup geometry for a terminal area.
pub fn inbox_layout(area: Rect) -> InboxLayout {
    let popup = centered(area, 76, area.height.saturating_sub(2).min(18));
    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let hint = Rect {
        y: inner.bottom().saturating_sub(1),
        height: inner.height.min(1),
        ..inner
    };
    let detail_h = DETAIL_LINES.min(inner.height.saturating_sub(3));
    let detail = Rect {
        y: hint.y.saturating_sub(detail_h),
        height: detail_h,
        ..inner
    };
    // One row above the detail for the separator.
    let list_h = detail.y.saturating_sub(inner.y + 1);
    let list = Rect {
        height: list_h,
        ..inner
    };
    InboxLayout {
        popup,
        list,
        detail,
        hint,
    }
}

/// Draw the overlay over `area` (clears what is under the popup).
pub fn render_inbox(buf: &mut Buffer, area: Rect, sv: &StateView, vs: &ViewState) {
    if area.width < 10 || area.height < 6 {
        return;
    }
    let items = needs_you(sv);
    let lay = inbox_layout(area);
    Clear.render(lay.popup, buf);
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::PURPLE))
        .title(format!(" needs you  {} ", items.len()))
        .render(lay.popup, buf);

    if items.is_empty() {
        put(
            buf,
            lay.list.x + 1,
            lay.list.y,
            lay.list.width,
            "Nothing needs you.",
            Style::default().fg(theme::GREY),
        );
        put(
            buf,
            lay.hint.x,
            lay.hint.y,
            lay.hint.width,
            "esc close",
            Style::default().fg(theme::GREY),
        );
        return;
    }

    let sel = vs.inbox_sel.min(items.len() - 1);
    let start = window_start(sel, lay.list.height as usize);
    for (i, item) in items
        .iter()
        .skip(start)
        .take(lay.list.height as usize)
        .enumerate()
    {
        let y = lay.list.y + i as u16;
        let selected = start + i == sel;
        let st = if selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        if selected {
            for x in lay.list.x..lay.list.right() {
                buf[(x, y)].set_style(st);
            }
        }
        put(buf, lay.list.x + 1, y, 1, "⚑", st.fg(theme::PURPLE));
        let who = item_who(sv, item);
        let who_w = 14.min(lay.list.width / 3) as usize;
        put(
            buf,
            lay.list.x + 3,
            y,
            who_w as u16,
            &clip(&who, who_w),
            st.add_modifier(Modifier::BOLD),
        );
        let what_x = 3 + who_w as u16 + 1;
        let what_w = lay.list.width.saturating_sub(what_x + 1);
        put(
            buf,
            lay.list.x + what_x,
            y,
            what_w,
            &clip(&item_what(item), what_w as usize),
            st,
        );
    }

    // Separator and detail of the selected item.
    let sep_y = lay.detail.y.saturating_sub(1);
    let line = Style::default().fg(theme::PURPLE);
    put(buf, lay.popup.x, sep_y, 1, "├", line);
    put(buf, lay.popup.right() - 1, sep_y, 1, "┤", line);
    for x in lay.popup.x + 1..lay.popup.right() - 1 {
        put(buf, x, sep_y, 1, "─", line);
    }
    let item = items[sel];
    let text = match &item.kind {
        ItemKind::Permission {
            tool,
            input_summary,
        } => format!("{tool}: {input_summary}"),
        _ => item.body.clone(),
    };
    for (i, l) in wrap(&text, lay.detail.width.saturating_sub(2) as usize)
        .iter()
        .take(lay.detail.height as usize)
        .enumerate()
    {
        put(
            buf,
            lay.detail.x + 1,
            lay.detail.y + i as u16,
            lay.detail.width.saturating_sub(2),
            l,
            Style::default(),
        );
    }
    put(
        buf,
        lay.hint.x + 1,
        lay.hint.y,
        lay.hint.width.saturating_sub(1),
        &clip(
            inbox_hints(&item.kind),
            lay.hint.width.saturating_sub(1) as usize,
        ),
        Style::default().fg(theme::GREY),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::draw::testutil::dump;
    use crate::tui::fixture;

    fn snap(cols: u16, rows: u16, sel: usize) -> String {
        let sv = fixture::state();
        let mut vs = fixture::view_state();
        vs.inbox_sel = sel;
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        render_inbox(&mut buf, area, &sv, &vs);
        dump(&buf)
    }

    #[test]
    fn inbox_80x24() {
        insta::assert_snapshot!(snap(80, 24, 0));
    }

    #[test]
    fn inbox_100x30_second_item() {
        insta::assert_snapshot!(snap(100, 30, 1));
    }

    #[test]
    fn inbox_160x50() {
        insta::assert_snapshot!(snap(160, 50, 0));
    }

    #[test]
    fn empty_inbox() {
        let sv = StateView::default();
        let vs = ViewState::default();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render_inbox(&mut buf, area, &sv, &vs);
        assert!(dump(&buf).contains("Nothing needs you."));
    }
}
