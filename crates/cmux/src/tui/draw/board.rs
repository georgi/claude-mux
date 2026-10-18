//! Board view (UI design 3.6): the plan's tasks as cards in columns by state.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};

use super::{clip, put, put_right};
use crate::tui::nav::{self, BOARD_COLUMNS};
use crate::tui::rows::agent_glyph;
use crate::tui::state::ViewState;
use crate::tui::theme;
use cmux_core::glyph::Glyph;
use cmux_proto::view::{PlanId, StateView, TaskId, TaskState, TaskView};

pub const CARD_HEIGHT: u16 = 4;

/// Header line of the board; the columns fill the rest.
pub fn header_rect(area: Rect) -> Rect {
    Rect {
        height: area.height.min(1),
        ..area
    }
}

/// The five column rectangles below the header (title row included).
pub fn column_rects(area: Rect) -> Vec<Rect> {
    let n = BOARD_COLUMNS.len() as u16;
    let body = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };
    let w = body.width / n;
    (0..n)
        .map(|i| Rect {
            x: body.x + i * w,
            y: body.y,
            width: if i == n - 1 {
                body.width - w * (n - 1)
            } else {
                w
            },
            height: body.height,
        })
        .collect()
}

/// Every visible card with its rectangle. The selected card's column scrolls
/// so the card is visible.
pub fn card_slots(
    sv: &StateView,
    vs: &ViewState,
    plan: &PlanId,
    area: Rect,
) -> Vec<(Rect, TaskId)> {
    let cols = nav::board_columns(sv, plan);
    let rects = column_rects(area);
    let sel = nav::board_selected(sv, vs, plan).and_then(|t| nav::board_pos(sv, plan, t));
    let mut out = Vec::new();
    for (c, (col, rect)) in cols.iter().zip(&rects).enumerate() {
        let cap = (rect.height.saturating_sub(1) / CARD_HEIGHT) as usize;
        let offset = match sel {
            Some((sc, sr)) if sc == c && cap > 0 => (sr + 1).saturating_sub(cap),
            _ => 0,
        };
        for (i, task) in col.iter().skip(offset).take(cap).enumerate() {
            out.push((
                Rect {
                    x: rect.x,
                    y: rect.y + 1 + i as u16 * CARD_HEIGHT,
                    width: rect.width,
                    height: CARD_HEIGHT,
                },
                task.id,
            ));
        }
    }
    out
}

fn tokens(sv: &StateView, plan: &PlanId) -> u64 {
    sv.agents
        .iter()
        .filter(|a| a.id.plan() == plan)
        .map(|a| a.tokens.input + a.tokens.output)
        .sum()
}

fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// Draw the board for `plan` into `area`.
pub fn render_board(buf: &mut Buffer, area: Rect, sv: &StateView, vs: &ViewState, plan: &PlanId) {
    if area.width == 0 || area.height < 2 {
        return;
    }
    let Some(p) = sv.plan(plan) else {
        put(
            buf,
            area.x,
            area.y,
            area.width,
            " no such plan",
            Style::default(),
        );
        return;
    };
    let merged = p
        .tasks
        .iter()
        .filter(|t| {
            sv.task(plan, **t)
                .is_some_and(|t| t.state == TaskState::Merged)
        })
        .count();
    let header = format!(
        " {} · {} · {}/{} merged · {} agents · {} tokens",
        p.title,
        p.state,
        merged,
        p.tasks.len(),
        p.agents.len(),
        short_count(tokens(sv, plan)),
    );
    let h = header_rect(area);
    put(
        buf,
        h.x,
        h.y,
        h.width,
        &clip(&header, h.width as usize),
        Style::default().add_modifier(Modifier::BOLD),
    );
    put_right(
        buf,
        h.right(),
        h.y,
        8,
        "v back ",
        Style::default().fg(theme::GREY),
    );

    let cols = nav::board_columns(sv, plan);
    for ((title, _), (col, rect)) in BOARD_COLUMNS
        .iter()
        .zip(cols.iter().zip(column_rects(area)))
    {
        let t = format!("{title} {}", col.len());
        put(
            buf,
            rect.x + 1,
            rect.y,
            rect.width.saturating_sub(1),
            &clip(&t, rect.width.saturating_sub(1) as usize),
            Style::default()
                .fg(theme::GREY)
                .add_modifier(Modifier::BOLD),
        );
    }

    let selected = nav::board_selected(sv, vs, plan);
    for (rect, id) in card_slots(sv, vs, plan, area) {
        if let Some(task) = sv.task(plan, id) {
            draw_card(buf, rect, sv, vs, task, selected == Some(id));
        }
    }
}

fn draw_card(
    buf: &mut Buffer,
    rect: Rect,
    sv: &StateView,
    vs: &ViewState,
    task: &TaskView,
    selected: bool,
) {
    let border = if selected { theme::ACCENT } else { theme::GREY };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .render(rect, buf);
    let inner_w = rect.width.saturating_sub(2);
    let agent = task.agent.as_ref().and_then(|a| sv.agent(a));
    let g = match (task.state, agent) {
        (TaskState::Merged, _) => Glyph::Success,
        (TaskState::Cancelled | TaskState::Failing | TaskState::Blocked, _) => Glyph::Failed,
        (_, Some(a)) => agent_glyph(sv, vs, a),
        (_, None) => Glyph::Idle,
    };
    let ch = theme::glyph_char(g, vs.spinner);
    let mut st = Style::default();
    if selected {
        st = st.add_modifier(Modifier::BOLD);
    }
    put(
        buf,
        rect.x + 1,
        rect.y + 1,
        1,
        &ch.to_string(),
        st.fg(theme::glyph_color(g)),
    );
    let title = format!("{} {}", task.id, task.title);
    put(
        buf,
        rect.x + 3,
        rect.y + 1,
        inner_w.saturating_sub(2),
        &clip(&title, inner_w.saturating_sub(2) as usize),
        st,
    );
    let mut detail = task.state.label().to_string();
    if let Some(pr) = &task.pr {
        detail.push_str(&format!(" #{}", pr.number));
    }
    if task.criteria_total > 0 {
        detail.push_str(&format!(" {}/{}", task.criteria_done, task.criteria_total));
    }
    put(
        buf,
        rect.x + 1,
        rect.y + 2,
        inner_w,
        &clip(&detail, inner_w as usize),
        Style::default().fg(theme::task_state_color(task.state)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::draw::testutil::dump;
    use crate::tui::fixture;

    fn snap(cols: u16, rows: u16) -> String {
        let sv = fixture::state();
        let vs = fixture::view_state();
        let area = Rect::new(0, 0, cols, rows);
        let mut buf = Buffer::empty(area);
        render_board(&mut buf, area, &sv, &vs, &fixture::plan_id());
        dump(&buf)
    }

    #[test]
    fn board_80x24() {
        insta::assert_snapshot!(snap(80, 24));
    }

    #[test]
    fn board_100x30() {
        insta::assert_snapshot!(snap(100, 30));
    }

    #[test]
    fn board_160x50() {
        insta::assert_snapshot!(snap(160, 50));
    }

    #[test]
    fn selected_column_scrolls() {
        let sv = fixture::state();
        let mut vs = fixture::view_state();
        vs.board_sel = Some(TaskId::new(5));
        // Room for one card per column: the selected one must still show.
        let area = Rect::new(0, 0, 80, 6);
        let slots = card_slots(&sv, &vs, &fixture::plan_id(), area);
        assert!(slots.iter().any(|(_, t)| *t == TaskId::new(5)));
    }

    #[test]
    fn short_counts() {
        assert_eq!(short_count(999), "999");
        assert_eq!(short_count(412_000), "412k");
        assert_eq!(short_count(2_500_000), "2.5M");
    }
}
