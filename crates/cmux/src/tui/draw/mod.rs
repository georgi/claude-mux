//! Rendering. Every function draws into a ratatui `Buffer` inside the `Rect`
//! it is given and has a matching layout function that the hit-tests share.

pub mod board;
pub mod form;
pub mod inbox;
pub mod tree;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

/// Write `s` at `(x, y)`, cut at `max_w` columns. Returns the columns used.
pub(super) fn put(buf: &mut Buffer, x: u16, y: u16, max_w: u16, s: &str, style: Style) -> u16 {
    if max_w == 0 {
        return 0;
    }
    let (end, _) = buf.set_stringn(x, y, s, max_w as usize, style);
    end - x
}

/// Write `s` ending at column `right` (exclusive).
pub(super) fn put_right(buf: &mut Buffer, right: u16, y: u16, max_w: u16, s: &str, style: Style) {
    let w = (s.width() as u16).min(max_w);
    if w > 0 {
        put(buf, right - w, y, w, s, style);
    }
}

/// Cut to `max_w` columns, ending in `…` when it did not fit.
pub(super) fn clip(s: &str, max_w: usize) -> String {
    if s.width() <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max_w {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// A `w`×`h` rectangle centred in `area`, shrunk to fit.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Wrap `text` to `width` columns, keeping explicit newlines.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cur = String::new();
        for word in line.split_whitespace() {
            if !cur.is_empty() && cur.width() + 1 + word.width() > width {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        out.push(cur);
    }
    out
}

#[cfg(test)]
pub(crate) mod testutil {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    /// The buffer as text, trailing blanks trimmed. Rows that carry a
    /// highlight (reversed or a background) get ` «sel»` appended so the
    /// snapshot shows the selection.
    pub fn dump(buf: &Buffer) -> String {
        let area: Rect = buf.area;
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            let mut line = String::new();
            let mut hl = false;
            for x in area.left()..area.right() {
                let c = &buf[(x, y)];
                line.push_str(c.symbol());
                hl |=
                    c.modifier.contains(Modifier::REVERSED) || c.bg != ratatui::style::Color::Reset;
            }
            let mut line = line.trim_end().to_string();
            if hl {
                line.push_str(" «sel»");
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}
