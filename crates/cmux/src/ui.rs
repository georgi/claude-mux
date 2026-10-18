//! Rendering: tab bar, the panes of the active window, status line and overlays.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode, PromptKind};
use crate::history::{self, tilde};
use crate::keys::prefix_name;
use crate::pane::{Kind, Pane, Status};
use crate::tui::draw::{board, form, inbox, tree};
use crate::tui::remote::ActiveForm;
use crate::tui::{marks, theme};
use crate::window::Window;

const ACCENT: Color = Color::Rgb(217, 119, 87); // Claude orange
const BAR_BG: Color = Color::Indexed(235);
const BAR_FG: Color = Color::Indexed(250);
const DIM: Color = Color::Indexed(244);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.height < 3 || area.width < 10 {
        return;
    }
    let top = Rect { height: 1, ..area };
    let screen_body = Rect {
        y: 1,
        height: area.height - 2,
        ..area
    };
    let body = match &app.remote {
        Some(r) => r.pane_area(screen_body),
        None => screen_body,
    };
    let bottom = Rect {
        y: area.height - 1,
        height: 1,
        ..area
    };

    draw_tabs(f, app, top);
    draw_status(f, app, bottom);

    if app.remote.is_some() {
        draw_remote_body(f, app, screen_body, body);
    } else if let Some(w) = app.windows.get(app.active) {
        draw_window(f, app, w, body);
    }

    match &app.mode {
        Mode::Picker => draw_picker(f, app, body),
        Mode::Help => draw_help(f, app, body),
        Mode::Windows { query, selected } => draw_windows(f, app, body, query, *selected),
        Mode::Prompt(p) if p.kind == PromptKind::ConfirmRemote => {}
        Mode::Prompt(p) => {
            let x = bottom.x + p.kind.label().width() as u16 + 3 + p.input.width() as u16;
            if x < bottom.width {
                f.set_cursor_position(Position { x, y: bottom.y });
            }
        }
        _ => {}
    }
}

/// Remote mode: the tree, then the window or the board, then the overlays.
fn draw_remote_body(f: &mut Frame, app: &App, full: Rect, body: Rect) {
    let Some(r) = &app.remote else { return };
    let sv = &r.mirror.sv;
    let tree_focused = matches!(app.mode, Mode::Tree);
    if let Some(rect) = r.tree_rect(full) {
        let mode = tree::tree_mode(full.width);
        tree::render_tree(f.buffer_mut(), rect, sv, &r.vs, mode);
    }
    if matches!(app.mode, Mode::Board) {
        f.render_widget(Clear, body);
        if let Some(plan) = r.plan.as_ref().or_else(|| r.vs.current_plan(sv)) {
            board::render_board(f.buffer_mut(), body, sv, &r.vs, plan);
        }
    } else if let Some(w) = app.windows.get(app.active) {
        draw_window(f, app, w, body);
    } else {
        let msg = if sv.plans.is_empty() && sv.terminals.is_empty() {
            "nothing on this server yet - n new plan, prefix c new Claude, prefix t shell"
        } else {
            "no windows yet - open a node of the tree"
        };
        f.render_widget(
            Paragraph::new(Line::styled(msg, Style::default().fg(DIM))),
            Rect {
                x: body.x + 1,
                y: body.y + 1,
                width: body.width.saturating_sub(2),
                height: 1.min(body.height),
            },
        );
    }
    // A strip or hidden tree with focus is drawn full, over the body.
    if tree_focused && let Some(rect) = r.overlay_rect(full) {
        f.render_widget(Clear, rect);
        tree::render_tree(f.buffer_mut(), rect, sv, &r.vs, tree::TreeMode::Full);
    }
    match &app.mode {
        Mode::Inbox => inbox::render_inbox(f.buffer_mut(), full, sv, &r.vs),
        Mode::Form => {
            if let Some(form_state) = &r.form
                && let Some(pos) =
                    form::render_form(f.buffer_mut(), full, form_state as &ActiveForm)
            {
                f.set_cursor_position(pos);
            }
        }
        _ => {}
    }
}

fn draw_window(f: &mut Frame, app: &App, w: &Window, area: Rect) {
    let areas = w.areas(area);
    for a in &areas {
        let Some(pane) = w.get(a.id) else { continue };
        let focused = a.id == w.active;
        let p = pane.parser.lock().unwrap();
        let screen = p.screen();
        f.render_widget(Screen(screen), a.body);
        if pane.dim {
            f.buffer_mut()
                .set_style(a.body, Style::default().add_modifier(Modifier::DIM));
        }
        if let Some(h) = a.header {
            draw_pane_header(f, app, pane, focused, screen.scrollback(), h);
        }
        let show_cursor = focused
            && matches!(app.mode, Mode::Normal | Mode::Prefix)
            && !screen.hide_cursor()
            && screen.scrollback() == 0;
        if show_cursor {
            let (r, c) = screen.cursor_position();
            if r < a.body.height && c < a.body.width {
                f.set_cursor_position(Position {
                    x: a.body.x + c,
                    y: a.body.y + r,
                });
            }
        }
    }
    if !w.is_split() {
        return;
    }
    // Vertical lines between side-by-side panes. The part next to the focused
    // pane is drawn in the accent color, like tmux.
    let focus = areas.iter().find(|a| a.id == w.active).map(|a| Rect {
        y: a.body.y - 1,
        height: a.body.height + 1,
        ..a.body
    });
    let buf = f.buffer_mut();
    for d in w
        .layout
        .dividers(area)
        .iter()
        .filter(|d| d.axis == crate::layout::Axis::Horizontal)
    {
        for y in d.area.y..d.area.bottom() {
            let next_to_focus = focus.is_some_and(|r| {
                (r.y..r.bottom()).contains(&y) && (r.right() == d.at || r.x == d.at + 1)
            });
            let color = if next_to_focus {
                ACCENT
            } else {
                Color::Indexed(238)
            };
            if let Some(cell) = buf.cell_mut((d.at, y)) {
                cell.set_symbol("│");
                cell.set_style(Style::default().fg(color).bg(Color::Reset));
            }
        }
    }
}

/// The title row of a pane in a split window: status, name, directory.
fn draw_pane_header(
    f: &mut Frame,
    app: &App,
    pane: &Pane,
    focused: bool,
    scrolled: usize,
    area: Rect,
) {
    let style = if pane.dim {
        let bg = if focused { Color::Indexed(238) } else { BAR_BG };
        Style::default().bg(bg).fg(DIM)
    } else if focused {
        Style::default()
            .bg(Color::Indexed(238))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else if pane.attention {
        Style::default()
            .bg(BAR_BG)
            .fg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(BAR_BG).fg(BAR_FG)
    };
    let w = area.width as usize;
    let name = truncate(&pane.name(), w.saturating_sub(4).max(1));
    let mut spans = vec![
        Span::styled(" ", style),
        status_glyph(app, pane).patch_style(Style::default().bg(style.bg.unwrap_or(BAR_BG))),
        Span::styled(format!(" {name} "), style),
    ];
    let mut used = 4 + name.width();
    if let Some(r) = &app.remote
        && let (Some(t), Some(me)) = (pane.remote_term(), r.mirror.client_id.as_ref())
        && let Some(mark) = marks::size_mark(&r.mirror.sv, t.term, me)
    {
        used += mark.width() + 1;
        spans.push(Span::styled(
            format!("{mark} "),
            Style::default().bg(BAR_BG).fg(theme::AMBER),
        ));
    }
    if scrolled > 0 {
        spans.push(Span::styled(
            format!(" +{scrolled} "),
            Style::default().bg(Color::Cyan).fg(Color::Black),
        ));
    } else if w > used + 4 {
        // An agent that reported says so instead of showing its directory.
        let report = app
            .remote
            .as_ref()
            .and_then(|r| r.mirror.sv.agent(pane.agent.as_ref()?))
            .and_then(marks::report_mark);
        let (sep, what) = match (&pane.agent, report) {
            (_, Some(r)) => ("· ", r),
            (Some(_), None) => ("· ", tilde(&pane.cwd)),
            (None, None) => (" ", tilde(&pane.cwd)),
        };
        // Columns left after the separator and the bar's closing space.
        let room = (w - used).saturating_sub(sep.width() + 1);
        if room >= 4 {
            let text = truncate(&what, room);
            spans.push(Span::styled(
                format!("{sep}{text}"),
                style.fg(DIM).remove_modifier(Modifier::BOLD),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
}

/// Copies a vt100 screen into the ratatui buffer.
struct Screen<'a>(&'a vt100::Screen);

fn conv(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

impl Widget for Screen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (rows, cols) = self.0.size();
        for r in 0..rows.min(area.height) {
            for c in 0..cols.min(area.width) {
                let Some(cell) = self.0.cell(r, c) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut m = Modifier::empty();
                if cell.bold() {
                    m |= Modifier::BOLD;
                }
                if cell.dim() {
                    m |= Modifier::DIM;
                }
                if cell.italic() {
                    m |= Modifier::ITALIC;
                }
                if cell.underline() {
                    m |= Modifier::UNDERLINED;
                }
                if cell.inverse() {
                    m |= Modifier::REVERSED;
                }
                let style = Style::default()
                    .fg(conv(cell.fgcolor()))
                    .bg(conv(cell.bgcolor()))
                    .add_modifier(m);
                if let Some(out) = buf.cell_mut((area.x + c, area.y + r)) {
                    out.set_symbol(if cell.has_contents() {
                        cell.contents()
                    } else {
                        " "
                    });
                    out.set_style(style);
                }
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn glyph_span(app: &App, g: cmux_core::glyph::Glyph) -> Span<'static> {
    Span::styled(
        theme::glyph_char(g, app.tick as usize).to_string(),
        Style::default().fg(theme::glyph_color(g)),
    )
}

fn status_glyph(app: &App, t: &Pane) -> Span<'static> {
    if let Some(g) = app.remote.as_ref().and_then(|r| r.pane_glyph(t)) {
        return glyph_span(app, g);
    }
    let spin = SPINNER[(app.tick as usize) % SPINNER.len()];
    if t.attention {
        return Span::styled(
            "●",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        );
    }
    match &t.kind {
        Kind::Shell => return Span::styled("$", Style::default().fg(Color::Blue)),
        Kind::Command(_) => return Span::styled("▸", Style::default().fg(Color::Blue)),
        Kind::Claude => {}
    }
    match &t.status {
        Status::Busy => Span::styled(spin, Style::default().fg(Color::Yellow)),
        Status::Idle => Span::styled("○", Style::default().fg(Color::Green)),
        Status::Other(_) => Span::styled(
            "?",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
        Status::Unknown if app.is_busy(t) => Span::styled(spin, Style::default().fg(Color::Yellow)),
        Status::Unknown => Span::styled("·", Style::default().fg(DIM)),
    }
}

/// The glyph of a window: an alert in any pane, else any working Claude
/// pane, else the focused pane's glyph.
fn window_glyph(app: &App, w: &Window) -> Span<'static> {
    if let Some(g) = app.remote.as_ref().and_then(|r| r.window_glyph(w)) {
        return glyph_span(app, g);
    }
    let pick = w
        .panes
        .iter()
        .find(|p| p.attention)
        .or_else(|| w.panes.iter().find(|p| app.is_busy(p)))
        .or_else(|| {
            w.panes
                .iter()
                .find(|p| matches!(p.status, Status::Other(_)))
        })
        .unwrap_or_else(|| w.pane());
    status_glyph(app, pick)
}

/// Tab label: the window name, with the pane count and a zoom mark.
fn tab_label(w: &Window, max: usize) -> String {
    let mut flags = String::new();
    if w.panes.len() > 1 {
        flags.push_str(&format!(" ⊞{}", w.panes.len()));
    }
    if w.zoomed {
        flags.push_str(" Z");
    }
    let name = truncate(&w.name(), max.saturating_sub(flags.width()).max(4));
    format!(" {name}{flags} ")
}

const BRAND: &str = " cmux ";

/// The first cell of the tab bar: `cmux`, or the server's name with the
/// connection dot (`●` connected, `◐` reconnecting, `○` offline) and the
/// workspace.
pub fn brand(app: &App) -> String {
    match &app.remote {
        None => BRAND.to_string(),
        Some(r) => {
            let plan = r
                .plan
                .as_ref()
                .and_then(|p| r.mirror.sv.plan(p))
                .map(|p| format!(" {} ▸", p.title))
                .unwrap_or_default();
            format!(" {} {}{plan} ", r.name, r.mirror.status.dot())
        }
    }
}

/// Max name width per tab so that all tabs fit: each tab costs " 1 ● " + name + " ".
fn tab_name_width(app: &App, width: u16) -> usize {
    let room = (width as usize).saturating_sub(brand(app).width() + 1);
    (room / app.windows.len().max(1))
        .saturating_sub(6)
        .clamp(4, 28)
}

/// The tab under column `col` of the tab bar. Mirrors the layout in `draw_tabs`.
pub fn tab_at(app: &App, col: u16) -> Option<usize> {
    let per_tab = tab_name_width(app, app.cols);
    let mut x = brand(app).width() + 1;
    for (i, win) in app.windows.iter().enumerate() {
        let w = format!(" {} ", i + 1).width() + 1 + tab_label(win, per_tab).width();
        if (x..x + w).contains(&(col as usize)) {
            return Some(i);
        }
        x += w;
    }
    None
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let bar = Style::default().bg(BAR_BG).fg(BAR_FG);
    let brand = brand(app);
    let per_tab = tab_name_width(app, area.width);

    let mut spans = vec![Span::styled(
        brand.clone(),
        Style::default()
            .bg(ACCENT)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    )];
    spans.push(Span::styled(" ", bar));
    for (i, w) in app.windows.iter().enumerate() {
        let active = i == app.active;
        let style = if active {
            Style::default()
                .bg(Color::Indexed(238))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else if w.attention() {
            bar.fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            bar
        };
        let glyph =
            window_glyph(app, w).patch_style(Style::default().bg(style.bg.unwrap_or(BAR_BG)));
        spans.push(Span::styled(
            format!(" {} ", i + 1),
            style.fg(if active { ACCENT } else { DIM }),
        ));
        spans.push(glyph);
        spans.push(Span::styled(tab_label(w, per_tab), style));
    }
    if app.windows.is_empty() {
        spans.push(Span::styled("no sessions", bar.fg(DIM)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(bar), area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let bar = Style::default().bg(BAR_BG).fg(BAR_FG);
    let badge = |s: &str, bg: Color| {
        Span::styled(
            format!(" {s} "),
            Style::default()
                .bg(bg)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
    };
    let pfx = prefix_name(app.opts.prefix);

    let mut left: Vec<Span> = Vec::new();
    match &app.mode {
        Mode::Prompt(p) if p.kind == PromptKind::ConfirmRemote => {
            left.push(badge(&p.input, ACCENT));
        }
        Mode::Prompt(p) => {
            left.push(badge(p.kind.label(), ACCENT));
            left.push(Span::styled(format!(" {}", p.input), bar.fg(Color::White)));
        }
        Mode::Prefix if app.remote.is_some() => {
            left.push(badge(&pfx, ACCENT));
            left.push(Span::styled(
                " d detach · Tab tree · c new · % \" split · a needs you · v board · N plan · = size · ? keys",
                bar.fg(DIM),
            ));
        }
        Mode::Prefix => {
            left.push(badge(&pfx, ACCENT));
            left.push(Span::styled(
                " c new · % \" split · ←→↑↓ pane · n/p tab · z zoom · s sessions · x kill · ? keys",
                bar.fg(DIM),
            ));
        }
        Mode::Tree => {
            left.push(badge("TREE", theme::PURPLE));
            left.push(Span::styled(
                " jk move · ↵ open · n plan · t task · A accept · S pause · s msg · x stop · i inbox · esc back",
                bar.fg(DIM),
            ));
        }
        Mode::Inbox => {
            left.push(badge("NEEDS YOU", theme::PURPLE));
        }
        Mode::Board => {
            left.push(badge("BOARD", theme::BLUE));
            left.push(Span::styled(
                " arrows move · ↵ open · A accept · s msg · x stop · v back",
                bar.fg(DIM),
            ));
        }
        Mode::Form => {
            let title = app
                .remote
                .as_ref()
                .and_then(|r| r.form.as_ref())
                .map(|f| crate::tui::forms::FormView::title(f).to_string())
                .unwrap_or_default();
            left.push(badge(&title, ACCENT));
        }
        Mode::Scroll => {
            let off = app
                .active_pane()
                .map(|t| t.parser.lock().unwrap().screen().scrollback())
                .unwrap_or(0);
            left.push(badge("SCROLL", Color::Cyan));
            left.push(Span::styled(
                format!(" +{off} lines · ↑↓ PgUp PgDn g G · q exit"),
                bar.fg(DIM),
            ));
        }
        _ => {
            let scrolled = app
                .active_pane()
                .map(|t| t.parser.lock().unwrap().screen().scrollback())
                .unwrap_or(0);
            if scrolled > 0 {
                left.push(badge(&format!("+{scrolled}"), Color::Cyan));
                left.push(Span::styled(
                    " scrolled back · wheel down or type to return",
                    bar.fg(DIM),
                ));
            } else if let Some(t) = app.active_pane() {
                left.push(Span::styled(
                    format!(" {}", tilde(&t.cwd)),
                    bar.fg(Color::White),
                ));
                if let Some(b) = &t.branch {
                    left.push(Span::styled(format!("  {b}"), bar.fg(Color::Magenta)));
                }
                if let Some(id) = &t.session_id {
                    left.push(Span::styled(
                        format!("  {}", &id[..id.len().min(8)]),
                        bar.fg(DIM),
                    ));
                }
                if let Status::Other(s) = &t.status {
                    left.push(Span::styled(format!("  {s}"), bar.fg(Color::Magenta)));
                }
            }
        }
    }

    let busy = app.panes().filter(|t| app.is_busy(t)).count();
    let right = match (&app.message, &app.remote) {
        (Some((m, _)), _) => Span::styled(format!("  {m} "), bar.fg(ACCENT)),
        (None, Some(r)) => {
            let (working, needs, clients) = r.counts();
            let mut s = format!("  {} {} ", r.mirror.status.dot(), r.name);
            if working > 0 {
                s.push_str(&format!("· {working} working "));
            }
            if needs > 0 {
                s.push_str(&format!("· {needs} need you "));
            }
            if clients > 1 {
                s.push_str(&format!("· {clients} clients "));
            }
            s.push_str(&format!("· {pfx} ? help "));
            Span::styled(s, bar.fg(DIM))
        }
        (None, None) if busy > 0 => {
            Span::styled(format!("  {busy} working · {pfx} ? help "), bar.fg(DIM))
        }
        (None, None) => Span::styled(format!("  {pfx} ? help "), bar.fg(DIM)),
    };
    let rw = if matches!(app.mode, Mode::Prompt(_)) {
        0
    } else {
        (right.content.width() as u16).min(area.width / 2)
    };
    let l = Rect {
        width: area.width - rw,
        ..area
    };
    let r = Rect {
        x: area.x + area.width - rw,
        width: rw,
        ..area
    };
    f.render_widget(Paragraph::new(Line::from(left)).style(bar), l);
    f.render_widget(Paragraph::new(Line::from(right)).style(bar), r);
}

fn draw_picker(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(Clear, area);
    let p = &app.picker;
    let scope = if p.all_dirs {
        "all projects".to_string()
    } else {
        tilde(&app.current_cwd())
    };
    let title = format!(
        " Sessions · {scope} · {}/{} ",
        p.filtered.len(),
        p.sessions.len()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height < 6 {
        return;
    }

    let dim = Style::default().fg(DIM);
    let query = Line::from(vec![
        Span::styled(
            "› ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(p.query.clone()),
        Span::styled("▏", Style::default().fg(ACCENT)),
    ]);
    let hint = Line::styled(
        "↑↓ select · Enter resume · ^F fork · ^O new here · Tab this dir/all · Esc close",
        dim,
    );
    f.render_widget(
        Paragraph::new(vec![query, hint]),
        Rect { height: 2, ..inner },
    );

    let preview_h = 4u16;
    let list = Rect {
        y: inner.y + 3,
        height: inner.height.saturating_sub(3 + preview_h),
        ..inner
    };
    let h = list.height as usize;

    if p.loading && p.sessions.is_empty() {
        let spin = SPINNER[(app.tick as usize) % SPINNER.len()];
        f.render_widget(
            Paragraph::new(Line::styled(
                format!("{spin} reading session history…"),
                dim,
            )),
            list,
        );
        return;
    }
    if p.filtered.is_empty() {
        f.render_widget(
            Paragraph::new(Line::styled("no matching sessions", dim)),
            list,
        );
    }

    let offset = p.selected.saturating_sub(h.saturating_sub(1));
    let open_ids: Vec<&str> = app
        .panes()
        .filter_map(|t| t.session_id.as_deref())
        .collect();
    let w = inner.width as usize;
    let mut lines = Vec::with_capacity(h);
    for (row, &i) in p.filtered.iter().enumerate().skip(offset).take(h) {
        let s = &p.sessions[i];
        let sel = row == p.selected;
        let base = if sel {
            Style::default().bg(Color::Indexed(237))
        } else {
            Style::default()
        };
        let marker = if open_ids.contains(&s.id.as_str()) {
            Span::styled(" ● ", base.fg(ACCENT))
        } else if app.live.iter().any(|l| l.session_id == s.id) {
            Span::styled(" ◆ ", base.fg(Color::Cyan))
        } else {
            Span::styled("   ", base)
        };
        let project = s
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into());
        let age = format!("{:>4} ", history::age(s.modified));
        let proj = format!("{:<18} ", truncate(&project, 18));
        let used = 3 + age.width() + proj.width();
        let title = truncate(&s.title, w.saturating_sub(used).max(4));
        let rest = w.saturating_sub(used + title.width());
        let mut spans = vec![
            marker,
            Span::styled(age, base.fg(DIM)),
            Span::styled(proj, base.fg(Color::Cyan)),
            Span::styled(
                title,
                if sel {
                    base.fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    base
                },
            ),
        ];
        if let Some(lp) = &s.last_prompt
            && rest > 8
            && lp != &s.title
        {
            spans.push(Span::styled(
                truncate(&format!("  {lp}"), rest),
                base.fg(DIM),
            ));
        }
        let mut line = Line::from(spans);
        if sel {
            line = line.style(base);
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), list);

    if let Some(s) = app.picker_selection() {
        let pr = Rect {
            y: inner.y + inner.height - preview_h,
            height: preview_h,
            ..inner
        };
        let label = Style::default().fg(DIM);
        let mut info = vec![
            Span::styled(tilde(&s.cwd), Style::default().fg(Color::Cyan)),
            Span::styled(format!("  {}", s.id), label),
        ];
        if let Some(b) = &s.branch {
            info.insert(
                1,
                Span::styled(format!("  {b}"), Style::default().fg(Color::Magenta)),
            );
        }
        let lines = vec![
            Line::styled("─".repeat(w), Style::default().fg(Color::Indexed(238))),
            Line::from(info),
            Line::from(vec![
                Span::styled("first ", label),
                Span::raw(truncate(
                    s.first_prompt.as_deref().unwrap_or("-"),
                    w.saturating_sub(6),
                )),
            ]),
            Line::from(vec![
                Span::styled("last  ", label),
                Span::raw(truncate(
                    s.last_prompt.as_deref().unwrap_or("-"),
                    w.saturating_sub(6),
                )),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), pr);
    }
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let pfx = prefix_name(app.opts.prefix);
    let base: &[(&str, &str)] = &[
        ("c", "new-window: new Claude session here"),
        ("C", "new Claude session in another directory"),
        ("t", "new shell window"),
        ("g", "new lazygit window"),
        ("n  p", "next-window / previous-window"),
        ("M-n  M-p", "next / previous window with an alert"),
        ("0 … 9", "select-window by number"),
        ("%  \"", "split-window: new Claude pane right / below"),
        ("← → ↑ ↓", "select-pane in that direction"),
        ("C-arrow M-arrow", "resize-pane by 1 / 5 cells"),
        ("o  ;", "next pane / last pane"),
        ("z", "zoom the pane (toggle)"),
        ("{  }", "swap the pane with the previous / next one"),
        ("!", "break-pane: move the pane to a new window"),
        ("Space  M-1…5", "next-layout / select-layout"),
        ("'", "select-window by typed index"),
        ("l", "last-window"),
        ("w  f", "choose-window: list and filter open tabs"),
        ("s  r", "choose-session: resume or fork past sessions"),
        (",", "rename-window"),
        ("x  &", "kill-pane / kill-window (asks first)"),
        ("[  PgUp", "copy-mode: scroll back through output"),
        ("< >", "move window left / right"),
        (
            ":",
            "command prompt (neww [-c DIR] [CMD], renamew, swapw -t N, …)",
        ),
        ("i", "display-message: pid and session id"),
        ("d", "quit cmux (no server, so no detach)"),
        ("?", "list-keys"),
    ];
    let mut rows: Vec<(&str, &str)> = base.to_vec();
    if app.remote.is_some() {
        rows.retain(|(k, _)| *k != "d" && *k != "< >" && *k != "!");
        let at = rows.len() - 1;
        for (j, row) in crate::app::remote::REMOTE_HELP.iter().enumerate() {
            rows.insert(at + j, *row);
        }
    }
    let w = 76u16.min(area.width);
    let h = (rows.len() as u16 + 6).min(area.height);
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let mut lines = vec![
        Line::from(vec![
            Span::raw("Press "),
            Span::styled(
                pfx.clone(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(", then a key. "),
            Span::styled(format!("{pfx} {pfx}"), Style::default().fg(ACCENT)),
            Span::raw(" sends a literal "),
            Span::raw(pfx.clone()),
            Span::raw("."),
        ]),
        Line::raw(""),
    ];
    for (k, d) in rows {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {k:<16}"),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(d),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "⠋ working  ○ idle  ● finished, not seen  ? needs input  $ shell  ▸ command",
        Style::default().fg(DIM),
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(" cmux keys ");
    f.render_widget(Paragraph::new(lines).block(block), r);
}

fn draw_windows(f: &mut Frame, app: &App, area: Rect, query: &str, selected: usize) {
    let matches = app.window_matches(query);
    let w = (area.width.saturating_sub(4)).clamp(20, 100);
    let h = (matches.len() as u16 + 4).clamp(6, area.height);
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " windows ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(r);
    f.render_widget(block, r);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                "› ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(query.to_string()),
            Span::styled("▏", Style::default().fg(ACCENT)),
            Span::styled(
                if query.is_empty() {
                    "   type to filter · j/k · Enter · x kill · q close"
                } else {
                    "   ↑↓ · Enter · Esc"
                },
                Style::default().fg(DIM),
            ),
        ]),
        Line::raw(""),
    ];
    let width = inner.width as usize;
    for &i in &matches {
        let win = &app.windows[i];
        let sel = i == selected;
        let base = if sel {
            Style::default().bg(Color::Indexed(237))
        } else {
            Style::default()
        };
        let name = tab_label(win, 34);
        let spans = vec![
            Span::styled(
                format!(" {:>2} ", i + 1),
                base.fg(if i == app.active { ACCENT } else { DIM }),
            ),
            window_glyph(app, win).patch_style(base),
            Span::styled(
                name.clone(),
                if sel {
                    base.fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    base
                },
            ),
            Span::styled(
                truncate(
                    &tilde(&win.pane().cwd),
                    width.saturating_sub(6 + name.width()).max(4),
                ),
                base.fg(Color::Cyan),
            ),
        ];
        let mut line = Line::from(spans);
        if sel {
            line = line.style(base);
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}
