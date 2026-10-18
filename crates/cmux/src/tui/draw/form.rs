//! Popup for any [`FormView`]: one labelled line per field.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Widget};
use unicode_width::UnicodeWidthStr;

use super::{centered, clip, put};
use crate::tui::forms::FormView;
use crate::tui::theme;

/// Where the popup sits; the client can use it to place the cursor.
pub fn form_rect(area: Rect, form: &dyn FormView) -> Rect {
    centered(area, 70, form.fields().len() as u16 * 2 + 4)
}

/// Draw the form. Returns the cursor position inside the focused field, which
/// the client passes to `Frame::set_cursor_position`.
pub fn render_form(buf: &mut Buffer, area: Rect, form: &dyn FormView) -> Option<Position> {
    let rect = form_rect(area, form);
    if rect.width < 12 || rect.height < 5 {
        return None;
    }
    Clear.render(rect, buf);
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::ACCENT))
        .title(format!(" {} ", form.title()))
        .render(rect, buf);
    let inner_w = rect.width - 2;
    let mut cursor = None;
    for (i, f) in form.fields().iter().enumerate() {
        let y = rect.y + 1 + i as u16 * 2;
        if y + 1 >= rect.bottom() {
            break;
        }
        let focused = i == form.focus();
        let label_st = if focused {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::GREY)
        };
        put(buf, rect.x + 1, y, inner_w, f.label, label_st);
        // Newlines show as ⏎ so a multi-line value stays on one row; the
        // cursor column counts the same replacement.
        let shown = f.input.text().replace('\n', "⏎");
        let before: String = shown.chars().take(f.input.cursor()).collect();
        let col = before.width() as u16;
        let scroll = col.saturating_sub(inner_w.saturating_sub(3));
        let tail: String = shown
            .chars()
            .scan(0u16, |w, c| {
                let skip = *w < scroll;
                *w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) as u16;
                Some((skip, c))
            })
            .filter(|(skip, _)| !skip)
            .map(|(_, c)| c)
            .collect();
        put(
            buf,
            rect.x + 1,
            y + 1,
            inner_w,
            &format!("> {tail}"),
            Style::default(),
        );
        if focused {
            cursor = Some(Position::new(
                rect.x + 3 + (col - scroll).min(inner_w.saturating_sub(3)),
                y + 1,
            ));
        }
    }
    let foot_y = rect.bottom() - 2;
    if let Some(e) = form.error() {
        put(
            buf,
            rect.x + 1,
            foot_y,
            inner_w,
            &clip(e, inner_w as usize),
            Style::default().fg(theme::RED),
        );
    } else {
        put(
            buf,
            rect.x + 1,
            foot_y,
            inner_w,
            &clip(form.hint(), inner_w as usize),
            Style::default().fg(theme::GREY),
        );
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Key;
    use crate::tui::draw::testutil::dump;
    use crate::tui::forms::{MessageForm, NewTaskForm};

    #[test]
    fn message_form_shows_text_and_cursor() {
        let agent = "P-20261018-auth/T-0002".parse().unwrap();
        let mut f = MessageForm::new(agent);
        for c in "hello".chars() {
            f.handle(Key::Char(c));
        }
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let cur = render_form(&mut buf, area, &f).unwrap();
        let d = dump(&buf);
        assert!(d.contains("> hello"));
        assert!(d.contains("alt-↵ steer now"));
        assert_eq!(buf[(cur.x - 1, cur.y)].symbol(), "o");
    }

    #[test]
    fn form_shows_validation_error_and_fits_small_screens() {
        let mut f = NewTaskForm::new("P-20261018-auth".parse().unwrap(), 1);
        f.handle(Key::AltEnter);
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        render_form(&mut buf, area, &f);
        assert!(dump(&buf).contains("title is empty"));
        let tiny = Rect::new(0, 0, 20, 6);
        render_form(&mut Buffer::empty(tiny), tiny, &f);
    }
}
