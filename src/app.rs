//! Application state and input handling.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::history::{self, LiveSession, SessionInfo, SharedIndex};
use crate::keys::{self, Key, Mouse};
use crate::tab::{Kind, SpawnSpec, Status, Tab};
use crate::{Event, Options};

pub enum Mode {
    Normal,
    Prefix,
    Picker,
    Help,
    Scroll,
    Prompt(Prompt),
    /// tmux `choose-window`: a filterable list of the open tabs.
    Windows { query: String, selected: usize },
}

pub struct Prompt {
    pub kind: PromptKind,
    pub input: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Rename,
    NewDir,
    ConfirmQuit,
    ConfirmKill,
    Command,
    SelectIndex,
}

impl PromptKind {
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::Rename => "rename tab",
            PromptKind::NewDir => "new session in",
            PromptKind::ConfirmQuit => "quit cmux and stop all sessions? (y/n)",
            PromptKind::ConfirmKill => "kill-window? (y/n)",
            PromptKind::Command => ":",
            PromptKind::SelectIndex => "index",
        }
    }
}

pub struct Picker {
    pub query: String,
    pub all_dirs: bool,
    pub loading: bool,
    pub sessions: Vec<SessionInfo>,
    pub filtered: Vec<usize>,
    pub selected: usize,
}

pub struct App {
    pub opts: Options,
    events: Sender<Event>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    last_active_id: Option<u64>,
    next_id: u64,
    pub mode: Mode,
    pub rows: u16,
    pub cols: u16,
    pub home_cwd: PathBuf,
    pub message: Option<(String, Instant)>,
    pub picker: Picker,
    index: SharedIndex,
    pub live: Vec<LiveSession>,
    pub tick: u64,
    pub quit: bool,
    redraw: bool,
    passthrough: Vec<u8>,
    // Input modes currently set on the outer terminal.
    outer_app_cursor: bool,
    outer_bracketed_paste: bool,
    outer_app_keypad: bool,
    outer_any_motion: bool,
}

impl App {
    pub fn new(opts: Options, events: Sender<Event>, rows: u16, cols: u16) -> App {
        App {
            opts,
            events,
            tabs: Vec::new(),
            active: 0,
            last_active_id: None,
            next_id: 1,
            mode: Mode::Normal,
            rows,
            cols,
            home_cwd: std::env::current_dir().unwrap_or_else(|_| history::home()),
            message: None,
            picker: Picker {
                query: String::new(),
                all_dirs: true,
                loading: false,
                sessions: Vec::new(),
                filtered: Vec::new(),
                selected: 0,
            },
            index: Arc::new(Mutex::new(history::Index::default())),
            live: Vec::new(),
            tick: 0,
            quit: false,
            redraw: true,
            passthrough: Vec::new(),
            outer_app_cursor: false,
            outer_bracketed_paste: false,
            outer_app_keypad: false,
            outer_any_motion: false,
        }
    }

    pub fn start(&mut self) {
        self.refresh_live();
        if self.opts.continue_last {
            let cwd = self.home_cwd.clone();
            let args = vec!["--continue".to_string()];
            self.open_tab(cwd, args, None);
        }
        for dir in self.opts.dirs.clone() {
            self.new_session(dir);
        }
        if self.opts.resume_picker {
            self.open_picker();
        } else if self.tabs.is_empty() && !self.opts.continue_last {
            self.new_session(self.home_cwd.clone());
        }
        if self.tabs.is_empty() {
            self.open_picker();
        }
    }

    pub fn shutdown(&mut self) {
        for t in &mut self.tabs {
            t.kill();
        }
    }

    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    /// Bytes to send to the outer terminal after a frame: input mode changes,
    /// OSC 52 clipboard writes and bells.
    pub fn take_passthrough(&mut self) -> Vec<u8> {
        if let Some(t) = self.tabs.get(self.active) {
            let p = t.parser.lock().unwrap();
            let s = p.screen();
            let pane_input = matches!(self.mode, Mode::Normal | Mode::Prefix);
            let (ac, bp, kp) = (s.application_cursor(), s.bracketed_paste() || !pane_input, s.application_keypad());
            if ac != self.outer_app_cursor {
                self.passthrough.extend_from_slice(if ac { b"\x1b[?1h" } else { b"\x1b[?1l" });
                self.outer_app_cursor = ac;
            }
            if bp != self.outer_bracketed_paste {
                self.passthrough.extend_from_slice(if bp { b"\x1b[?2004h" } else { b"\x1b[?2004l" });
                self.outer_bracketed_paste = bp;
            }
            if kp != self.outer_app_keypad {
                self.passthrough.extend_from_slice(if kp { b"\x1b=" } else { b"\x1b>" });
                self.outer_app_keypad = kp;
            }
            let am = self.opts.mouse && pane_input && s.mouse_protocol_mode() == vt100::MouseProtocolMode::AnyMotion;
            if am != self.outer_any_motion {
                self.passthrough.extend_from_slice(if am { b"\x1b[?1003h" } else { b"\x1b[?1003l" });
                self.outer_any_motion = am;
            }
        }
        std::mem::take(&mut self.passthrough)
    }

    pub fn pane_size(&self) -> (u16, u16) {
        (self.rows.saturating_sub(2).max(1), self.cols.max(1))
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
        self.redraw = true;
    }

    // ---- events -------------------------------------------------------------

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::Input(bytes) => {
                self.handle_input(&bytes);
                self.redraw = true;
            }
            Event::Output(id) => self.on_output(id),
            Event::Exited(id) => self.on_exit(id),
            Event::Indexed(sessions) => {
                self.picker.sessions = sessions;
                self.picker.loading = false;
                self.refilter();
                self.redraw = true;
            }
            Event::Resize => {
                if let Ok((c, r)) = ratatui::crossterm::terminal::size() {
                    self.rows = r;
                    self.cols = c;
                    let (pr, pc) = self.pane_size();
                    for t in &self.tabs {
                        t.resize(pr, pc);
                    }
                }
                self.redraw = true;
            }
            Event::Tick => self.on_tick(),
            Event::Quit => self.quit = true,
        }
    }

    fn on_output(&mut self, id: u64) {
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        tab.dirty.store(false, Ordering::Release);
        tab.last_output = Instant::now();
        let mut p = tab.parser.lock().unwrap();
        let cb = p.callbacks_mut();
        if let Some(title) = cb.title.take() {
            tab.title = Some(title);
            self.redraw = true;
        }
        self.passthrough.extend(cb.clipboard.drain(..).flatten());
        if std::mem::take(&mut cb.bell) && active_id != Some(id) {
            tab.attention = true;
            self.redraw = true;
        }
        drop(p);
        if active_id == Some(id) {
            self.redraw = true;
        }
    }

    fn on_exit(&mut self, id: u64) {
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let mut tab = self.tabs.remove(i);
        let name = tab.name();
        tab.kill();
        if i < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.flash(format!("session \"{name}\" ended"));
        if self.tabs.is_empty() {
            self.open_picker();
        }
        self.redraw = true;
    }

    fn on_tick(&mut self) {
        self.tick += 1;
        if self.tick % 4 == 0 {
            self.refresh_live();
            for t in &mut self.tabs {
                if t.has_exited() {
                    let _ = self.events.send(Event::Exited(t.id));
                }
            }
        }
        if self.tick % 20 == 0
            && let Some(t) = self.tabs.get_mut(self.active)
        {
            t.branch = crate::tab::git_branch(&t.cwd);
        }
        if let Some((_, at)) = &self.message
            && at.elapsed() > Duration::from_secs(4)
        {
            self.message = None;
            self.redraw = true;
        }
        // Animate spinners.
        if self.tabs.iter().any(|t| self.is_busy(t)) || self.picker.loading {
            self.redraw = true;
        }
    }

    pub fn is_busy(&self, t: &Tab) -> bool {
        if !t.is_claude() {
            return false;
        }
        match t.status {
            Status::Busy => true,
            Status::Unknown => t.last_output.elapsed() < Duration::from_millis(1500),
            _ => false,
        }
    }

    /// Poll ~/.claude/sessions for the status of each tab's Claude process.
    fn refresh_live(&mut self) {
        self.live = history::live_sessions();
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        let mut ring = false;
        for t in self.tabs.iter_mut().filter(|t| t.is_claude()) {
            let found = self
                .live
                .iter()
                .find(|l| Some(l.pid) == t.pid)
                .or_else(|| self.live.iter().find(|l| Some(&l.session_id) == t.session_id.as_ref()));
            let Some(l) = found else { continue };
            if !l.session_id.is_empty() {
                t.session_id = Some(l.session_id.clone());
            }
            let new = match l.status.as_deref() {
                Some("busy") => Status::Busy,
                Some("idle") => Status::Idle,
                Some(s) => Status::Other(s.to_string()),
                None => Status::Unknown,
            };
            if t.status == Status::Busy && new != Status::Busy && Some(t.id) != active_id {
                t.attention = true;
                ring = true;
            }
            if t.status != new {
                t.status = new;
                self.redraw = true;
            }
        }
        if ring && self.opts.bell {
            self.passthrough.push(0x07);
        }
    }

    // ---- tabs ---------------------------------------------------------------

    fn open_tab(&mut self, cwd: PathBuf, args: Vec<String>, session_id: Option<String>) -> bool {
        if !cwd.is_dir() {
            self.flash(format!("directory not found: {}", history::tilde(&cwd)));
            return false;
        }
        let mut full_args = args;
        full_args.extend(self.opts.claude_args.iter().cloned());
        let spec = SpawnSpec { kind: Kind::Claude, program: self.opts.claude.clone(), args: full_args, cwd, session_id };
        self.spawn_tab(spec)
    }

    fn spawn_tab(&mut self, spec: SpawnSpec) -> bool {
        if !spec.cwd.is_dir() {
            self.flash(format!("directory not found: {}", history::tilde(&spec.cwd)));
            return false;
        }
        let (rows, cols) = self.pane_size();
        let id = self.next_id;
        match Tab::spawn(id, spec, rows, cols, self.events.clone()) {
            Ok(tab) => {
                self.next_id += 1;
                self.tabs.push(tab);
                self.select(self.tabs.len() - 1);
                true
            }
            Err(e) => {
                self.flash(format!("{e:#}"));
                false
            }
        }
    }

    /// A login shell, like a tmux window without a command.
    fn open_shell(&mut self, cwd: PathBuf) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        self.spawn_tab(SpawnSpec { kind: Kind::Shell, program: shell, args: vec!["-l".into()], cwd, session_id: None });
    }

    /// Run `command` through the shell, like `tmux new-window CMD`. The tab
    /// closes when the command exits.
    fn open_command(&mut self, cwd: PathBuf, command: &str) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let label = command.split_whitespace().next().unwrap_or(command).rsplit('/').next().unwrap_or(command);
        let spec = SpawnSpec {
            kind: Kind::Command(label.to_string()),
            program: shell,
            args: vec!["-lc".into(), command.to_string()],
            cwd,
            session_id: None,
        };
        self.spawn_tab(spec);
    }

    fn new_session(&mut self, cwd: PathBuf) {
        let id = uuid::Uuid::new_v4().to_string();
        self.open_tab(cwd, vec!["--session-id".into(), id.clone()], Some(id));
    }

    fn resume_session(&mut self, s: &SessionInfo, fork: bool) {
        if !fork && let Some(i) = self.tabs.iter().position(|t| t.session_id.as_deref() == Some(&s.id)) {
            self.select(i);
            self.flash("session is already open in this tab");
            return;
        }
        let mut args = vec!["--resume".to_string(), s.id.clone()];
        if fork {
            args.push("--fork-session".into());
        }
        let elsewhere = self.live.iter().any(|l| l.session_id == s.id);
        if self.open_tab(s.cwd.clone(), args, if fork { None } else { Some(s.id.clone()) }) {
            let i = self.active;
            self.tabs[i].custom_name = None;
            if elsewhere && !fork {
                self.flash("note: this session is also running in another terminal");
            }
        }
    }

    fn select(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if let Some(cur) = self.tabs.get(self.active)
            && self.active != i
        {
            self.last_active_id = Some(cur.id);
        }
        if let Some(cur) = self.tabs.get(self.active) {
            cur.parser.lock().unwrap().screen_mut().set_scrollback(0);
        }
        self.active = i;
        self.tabs[i].attention = false;
        self.redraw = true;
    }

    fn cycle(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        if n > 0 {
            self.select((self.active as isize + delta).rem_euclid(n) as usize);
        }
    }

    fn move_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        let j = self.active as isize + delta;
        if (0..n).contains(&j) {
            self.tabs.swap(self.active, j as usize);
            self.active = j as usize;
        }
    }

    fn current_cwd(&self) -> PathBuf {
        self.tabs.get(self.active).map(|t| t.cwd.clone()).unwrap_or_else(|| self.home_cwd.clone())
    }

    // ---- picker -------------------------------------------------------------

    fn open_picker(&mut self) {
        self.mode = Mode::Picker;
        self.picker.query.clear();
        self.picker.selected = 0;
        self.picker.loading = true;
        self.refilter();
        let index = self.index.clone();
        let tx = self.events.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Indexed(history::scan(&index)));
        });
    }

    fn close_picker(&mut self) {
        if self.tabs.is_empty() {
            self.quit = true;
        }
        self.mode = Mode::Normal;
    }

    pub fn refilter(&mut self) {
        let words: Vec<String> = self.picker.query.to_lowercase().split_whitespace().map(str::to_string).collect();
        let here = self.current_cwd();
        self.picker.filtered = self
            .picker
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| self.picker.all_dirs || s.cwd == here)
            .filter(|(_, s)| {
                if words.is_empty() {
                    return true;
                }
                let hay = format!(
                    "{} {} {} {} {}",
                    s.title,
                    s.cwd.display(),
                    s.last_prompt.as_deref().unwrap_or(""),
                    s.first_prompt.as_deref().unwrap_or(""),
                    s.id
                )
                .to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        self.picker.selected = self.picker.selected.min(self.picker.filtered.len().saturating_sub(1));
    }

    pub fn picker_selection(&self) -> Option<&SessionInfo> {
        self.picker.filtered.get(self.picker.selected).map(|&i| &self.picker.sessions[i])
    }

    fn picker_key(&mut self, k: Key) {
        let page = self.rows.saturating_sub(10).max(1) as usize;
        let n = self.picker.filtered.len();
        let p = &mut self.picker;
        match k {
            Key::Esc => self.close_picker(),
            Key::Ctrl('c') => self.close_picker(),
            Key::Up | Key::Ctrl('p') | Key::Ctrl('k') => p.selected = p.selected.saturating_sub(1),
            Key::Down | Key::Ctrl('n') | Key::Ctrl('j') => p.selected = (p.selected + 1).min(n.saturating_sub(1)),
            Key::PageUp => p.selected = p.selected.saturating_sub(page),
            Key::PageDown => p.selected = (p.selected + page).min(n.saturating_sub(1)),
            Key::Home => p.selected = 0,
            Key::End => p.selected = n.saturating_sub(1),
            Key::Tab => {
                p.all_dirs = !p.all_dirs;
                self.refilter();
            }
            Key::Backspace => {
                p.query.pop();
                self.refilter();
            }
            Key::Ctrl('u') => {
                p.query.clear();
                self.refilter();
            }
            Key::Ctrl('w') => {
                let t = p.query.trim_end().rfind(' ').map(|i| i + 1).unwrap_or(0);
                p.query.truncate(t);
                self.refilter();
            }
            Key::Char(c) => {
                p.query.push(c);
                p.selected = 0;
                self.refilter();
            }
            Key::Enter | Key::Ctrl('f') => {
                if let Some(s) = self.picker_selection().cloned() {
                    self.mode = Mode::Normal;
                    self.resume_session(&s, k == Key::Ctrl('f'));
                    if self.tabs.is_empty() {
                        self.mode = Mode::Picker;
                    }
                }
            }
            Key::Ctrl('o') => {
                // New session in the selected session's directory.
                if let Some(s) = self.picker_selection().cloned() {
                    self.mode = Mode::Normal;
                    self.new_session(s.cwd);
                    if self.tabs.is_empty() {
                        self.mode = Mode::Picker;
                    }
                }
            }
            _ => {}
        }
    }

    // ---- input --------------------------------------------------------------

    fn handle_input(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            match self.mode {
                Mode::Normal => {
                    let prefix_at = bytes.iter().position(|&b| b == self.opts.prefix);
                    let mouse_at = keys::find_mouse(bytes);
                    let cut = match (prefix_at, mouse_at) {
                        (Some(p), Some(m)) => Some(p.min(m)),
                        (p, m) => p.or(m),
                    };
                    let fwd = &bytes[..cut.unwrap_or(bytes.len())];
                    if !fwd.is_empty()
                        && let Some(t) = self.tabs.get(self.active)
                    {
                        // Typing snaps a scrolled-back pane to the bottom, like a terminal.
                        t.parser.lock().unwrap().screen_mut().set_scrollback(0);
                        t.write(fwd);
                    }
                    bytes = &bytes[fwd.len()..];
                    if cut.is_some() && cut == prefix_at {
                        self.mode = Mode::Prefix;
                        bytes = &bytes[1..];
                    } else if let Some((m, n)) = keys::parse_mouse(bytes) {
                        bytes = &bytes[n..];
                        if let Some(m) = m {
                            self.handle_mouse(m);
                        }
                    }
                }
                _ if keys::parse_mouse(bytes).is_some() => {
                    let (m, n) = keys::parse_mouse(bytes).unwrap();
                    bytes = &bytes[n..];
                    if matches!(self.mode, Mode::Prefix) {
                        self.mode = Mode::Normal;
                    }
                    if let Some(m) = m {
                        self.handle_mouse(m);
                    }
                }
                Mode::Prefix => {
                    self.mode = Mode::Normal;
                    if bytes[0] == self.opts.prefix {
                        if let Some(t) = self.tabs.get(self.active) {
                            t.write(&[self.opts.prefix]);
                        }
                        bytes = &bytes[1..];
                        continue;
                    }
                    let (k, n) = keys::parse_one(bytes);
                    self.prefix_command(k);
                    bytes = &bytes[n..];
                }
                _ => {
                    let (k, n) = keys::parse_one(bytes);
                    bytes = &bytes[n..];
                    match &mut self.mode {
                        Mode::Picker => self.picker_key(k),
                        Mode::Help => self.mode = Mode::Normal,
                        Mode::Scroll => self.scroll_key(k),
                        Mode::Prompt(_) => self.prompt_key(k),
                        Mode::Windows { .. } => self.windows_key(k),
                        Mode::Normal | Mode::Prefix => {}
                    }
                    // Keys that arrive after leaving an overlay in the same
                    // read are dropped instead of leaking into the pane.
                    if matches!(self.mode, Mode::Normal) {
                        return;
                    }
                }
            }
        }
    }

    fn prompt(&mut self, kind: PromptKind, input: String) {
        self.mode = Mode::Prompt(Prompt { kind, input });
    }

    /// tmux default key table, mapped onto tabs (tmux windows).
    fn prefix_command(&mut self, k: Key) {
        match k {
            Key::Char('c') => self.new_session(self.current_cwd()),
            Key::Char('C') => {
                let mut input = history::tilde(&self.current_cwd());
                if !input.ends_with('/') {
                    input.push('/');
                }
                self.prompt(PromptKind::NewDir, input);
            }
            Key::Char('n') | Key::Right => self.cycle(1),
            Key::Char('p') | Key::Left => self.cycle(-1),
            Key::Alt('n') => self.cycle_attention(1),
            Key::Alt('p') => self.cycle_attention(-1),
            Key::Char('l') => self.last_tab(),
            Key::Char('0') => self.select(9),
            Key::Char(c @ '1'..='9') => self.select(c as usize - '1' as usize),
            Key::Char('\'') => self.prompt(PromptKind::SelectIndex, String::new()),
            Key::Char('t') => self.open_shell(self.current_cwd()),
            Key::Char('g') => self.open_command(self.current_cwd(), "lazygit"),
            Key::Char('w') => self.open_windows(String::new()),
            Key::Char('f') => self.open_windows(String::new()),
            Key::Char('s') | Key::Char('r') => self.open_picker(),
            Key::Char('<') => self.move_tab(-1),
            Key::Char('>') => self.move_tab(1),
            Key::Char('&') | Key::Char('x') => {
                if !self.tabs.is_empty() {
                    self.prompt(PromptKind::ConfirmKill, String::new());
                }
            }
            Key::Char(',') => {
                if let Some(t) = self.tabs.get(self.active) {
                    self.prompt(PromptKind::Rename, t.name());
                }
            }
            Key::Char('[') | Key::PageUp => {
                if !self.tabs.is_empty() {
                    self.mode = Mode::Scroll;
                    if k == Key::PageUp {
                        self.scroll_key(k);
                    }
                }
            }
            Key::Char(':') => self.prompt(PromptKind::Command, String::new()),
            Key::Char('i') => self.display_info(),
            Key::Char('?') => self.mode = Mode::Help,
            // No server process, so detach cannot keep the sessions alive.
            Key::Char('d') | Key::Char('q') => self.prompt(PromptKind::ConfirmQuit, String::new()),
            _ => {}
        }
    }

    fn last_tab(&mut self) {
        match self.last_active_id.and_then(|id| self.tabs.iter().position(|t| t.id == id)) {
            Some(i) => self.select(i),
            None => self.flash("no last window"),
        }
    }

    /// tmux M-n / M-p: next tab with an alert.
    fn cycle_attention(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        for step in 1..n {
            let i = (self.active as isize + delta * step).rem_euclid(n) as usize;
            if self.tabs[i].attention {
                self.select(i);
                return;
            }
        }
        self.flash("no window with an alert");
    }

    fn kill_active(&mut self) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            t.kill();
            self.flash("session stopped - resume it with prefix s");
        }
    }

    fn display_info(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            let msg = format!(
                "pid {} · session {}",
                t.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                t.session_id.as_deref().unwrap_or("?")
            );
            self.flash(msg);
        }
    }

    // ---- window chooser -----------------------------------------------------

    fn open_windows(&mut self, query: String) {
        if !self.tabs.is_empty() {
            self.mode = Mode::Windows { query, selected: self.active };
        }
    }

    /// Indices of the tabs that match the window chooser query.
    pub fn window_matches(&self, query: &str) -> Vec<usize> {
        let q = query.to_lowercase();
        (0..self.tabs.len())
            .filter(|&i| {
                let t = &self.tabs[i];
                q.is_empty() || format!("{} {}", t.name(), t.cwd.display()).to_lowercase().contains(&q)
            })
            .collect()
    }

    fn windows_key(&mut self, k: Key) {
        let Mode::Windows { query, selected } = &self.mode else { return };
        let (mut query, mut selected) = (query.clone(), *selected);
        let matches = self.window_matches(&query);
        let pos = matches.iter().position(|&i| i == selected).unwrap_or(0);
        // Like tmux choose-tree: single-letter commands while the filter is empty.
        let cmd = query.is_empty();
        let up = matches!(k, Key::Up | Key::Ctrl('p')) || (cmd && k == Key::Char('k'));
        let down = matches!(k, Key::Down | Key::Ctrl('n')) || (cmd && k == Key::Char('j'));
        if up {
            selected = matches.get(pos.saturating_sub(1)).copied().unwrap_or(selected);
        } else if down {
            selected = matches.get((pos + 1).min(matches.len().saturating_sub(1))).copied().unwrap_or(selected);
        }
        match k {
            _ if up || down => {}
            Key::Esc | Key::Ctrl('c') => {
                self.mode = Mode::Normal;
                return;
            }
            Key::Char('q') if cmd => {
                self.mode = Mode::Normal;
                return;
            }
            Key::Enter => {
                self.mode = Mode::Normal;
                if let Some(&i) = matches.get(pos) {
                    self.select(i);
                }
                return;
            }
            Key::Char('x') | Key::Char('&') if cmd => {
                self.select(selected);
                self.prompt(PromptKind::ConfirmKill, String::new());
                return;
            }
            Key::Char(c @ '1'..='9') if cmd => {
                self.mode = Mode::Normal;
                self.select(c as usize - '1' as usize);
                return;
            }
            Key::Backspace => {
                query.pop();
            }
            Key::Char(c) => query.push(c),
            _ => {}
        }
        let matches = self.window_matches(&query);
        if !matches.contains(&selected) {
            selected = matches.first().copied().unwrap_or(0);
        }
        self.mode = Mode::Windows { query, selected };
    }

    // ---- command prompt -----------------------------------------------------

    /// A small subset of tmux commands, with their aliases.
    fn run_command(&mut self, line: &str) {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some((&cmd, args)) = words.split_first() else { return };
        let flag = |name: &str| args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).copied();
        let rest = || args.iter().filter(|a| !a.starts_with('-')).copied().collect::<Vec<_>>().join(" ");
        match cmd {
            "new-window" | "neww" => {
                let dir = flag("-c").map(history::expand_tilde).unwrap_or_else(|| self.current_cwd());
                let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
                let mut command = Vec::new();
                let mut it = args.iter();
                while let Some(a) = it.next() {
                    match *a {
                        "-c" | "-n" => {
                            it.next();
                        }
                        a if a.starts_with('-') && command.is_empty() => {}
                        a => command.push(a),
                    }
                }
                match command.join(" ").as_str() {
                    "" | "claude" => self.new_session(dir),
                    "shell" | "sh" => self.open_shell(dir),
                    cmd => self.open_command(dir, cmd),
                }
                if let Some(name) = flag("-n")
                    && let Some(t) = self.tabs.get_mut(self.active)
                {
                    t.custom_name = Some(name.to_string());
                }
            }
            "kill-window" | "killw" | "kill-pane" | "killp" => self.kill_active(),
            "rename-window" | "renamew" => {
                let name = rest();
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.custom_name = (!name.is_empty()).then_some(name);
                }
            }
            "select-window" | "selectw" => match flag("-t").and_then(|t| t.trim_start_matches(':').parse::<usize>().ok()) {
                Some(n) if n >= 1 => self.select(n - 1),
                _ => self.flash("usage: select-window -t N"),
            },
            "next-window" | "next" => self.cycle(1),
            "previous-window" | "prev" => self.cycle(-1),
            "last-window" | "last" => self.last_tab(),
            "swap-window" | "swapw" | "move-window" | "movew" => {
                match flag("-t").and_then(|t| t.trim_start_matches(':').parse::<usize>().ok()) {
                    Some(n) if (1..=self.tabs.len()).contains(&n) => {
                        let t = self.tabs.remove(self.active);
                        self.tabs.insert(n - 1, t);
                        self.active = n - 1;
                    }
                    _ => self.flash("usage: swap-window -t N"),
                }
            }
            "choose-tree" | "choose-window" | "find-window" | "findw" => self.open_windows(rest()),
            "choose-session" | "resume" => {
                let id = rest();
                if id.is_empty() {
                    self.open_picker();
                } else if let Some(s) = self.picker.sessions.iter().find(|s| s.id.starts_with(&id)).cloned() {
                    self.resume_session(&s, false);
                } else {
                    let cwd = self.current_cwd();
                    self.open_tab(cwd, vec!["--resume".into(), id.clone()], Some(id));
                }
            }
            "display-message" | "display" => self.display_info(),
            "list-keys" | "lsk" => self.mode = Mode::Help,
            "copy-mode" => self.mode = Mode::Scroll,
            "kill-server" | "kill-session" => self.quit = true,
            "detach-client" | "detach" => self.prompt(PromptKind::ConfirmQuit, String::new()),
            _ => self.flash(format!("unknown command: {cmd}")),
        }
    }

    // ---- mouse --------------------------------------------------------------

    fn handle_mouse(&mut self, m: Mouse) {
        const WHEEL_LINES: usize = 3;
        // Tab bar: click selects, wheel cycles.
        if m.row == 0 {
            if m.is_left_press()
                && let Some(i) = crate::ui::tab_at(self, m.col)
            {
                self.mode = Mode::Normal;
                self.select(i);
            } else if m.wheel_up() {
                self.cycle(-1);
            } else if m.wheel_down() {
                self.cycle(1);
            }
            return;
        }
        let (pane_rows, _) = self.pane_size();
        let in_pane = m.row >= 1 && m.row <= pane_rows;

        match &mut self.mode {
            Mode::Picker => {
                let n = self.picker.filtered.len();
                let p = &mut self.picker;
                if m.wheel_up() {
                    p.selected = p.selected.saturating_sub(WHEEL_LINES);
                } else if m.wheel_down() {
                    p.selected = (p.selected + WHEEL_LINES).min(n.saturating_sub(1));
                }
                return;
            }
            Mode::Windows { .. } => {
                if m.wheel_up() {
                    self.windows_key(Key::Up);
                } else if m.wheel_down() {
                    self.windows_key(Key::Down);
                }
                return;
            }
            Mode::Help => {
                if m.is_left_press() {
                    self.mode = Mode::Normal;
                }
                return;
            }
            Mode::Scroll => {
                if m.wheel_up() {
                    for _ in 0..WHEEL_LINES {
                        self.scroll_key(Key::Up);
                    }
                } else if m.wheel_down() {
                    for _ in 0..WHEEL_LINES {
                        self.scroll_key(Key::Down);
                    }
                }
                return;
            }
            Mode::Prompt(_) | Mode::Prefix => return,
            Mode::Normal => {}
        }
        if !in_pane {
            return;
        }
        let Some(t) = self.tabs.get(self.active) else { return };
        let mut p = t.parser.lock().unwrap();
        let screen = p.screen();
        let mode = screen.mouse_protocol_mode();
        let encoding = screen.mouse_protocol_encoding();
        let alternate = screen.alternate_screen();
        let app_cursor = screen.application_cursor();

        if mode != vt100::MouseProtocolMode::None && screen.scrollback() == 0 {
            drop(p);
            if let Some(bytes) = encode_mouse(m, m.row - 1, mode, encoding) {
                t.write(&bytes);
            }
            return;
        }
        if !m.is_wheel() {
            return;
        }
        if alternate {
            // Full-screen app without mouse support: send arrow keys, like a
            // terminal's alternate-scroll mode.
            drop(p);
            let key: &[u8] = match (m.wheel_up(), app_cursor) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            t.write(&key.repeat(WHEEL_LINES));
        } else {
            // Normal screen (Claude Code): scroll our own scrollback.
            let cur = p.screen().scrollback();
            let next = if m.wheel_up() { cur + WHEEL_LINES } else { cur.saturating_sub(WHEEL_LINES) };
            p.screen_mut().set_scrollback(next);
        }
    }

    fn scroll_key(&mut self, k: Key) {
        let Some(t) = self.tabs.get(self.active) else {
            self.mode = Mode::Normal;
            return;
        };
        let page = self.pane_size().0 as usize;
        let mut p = t.parser.lock().unwrap();
        let cur = p.screen().scrollback();
        let next = match k {
            Key::Up | Key::Char('k') | Key::Ctrl('y') | Key::Ctrl('p') => cur + 1,
            Key::Down | Key::Char('j') | Key::Ctrl('e') | Key::Ctrl('n') => cur.saturating_sub(1),
            Key::PageUp | Key::Ctrl('b') | Key::Ctrl('u') | Key::Char('u') => cur + page,
            Key::PageDown | Key::Ctrl('f') | Key::Ctrl('d') | Key::Char('d') | Key::Char(' ') => {
                cur.saturating_sub(page)
            }
            Key::Home | Key::Char('g') => usize::MAX / 2,
            Key::End | Key::Char('G') => 0,
            Key::Esc | Key::Char('q') | Key::Enter | Key::Ctrl('c') => {
                p.screen_mut().set_scrollback(0);
                drop(p);
                self.mode = Mode::Normal;
                return;
            }
            _ => cur,
        };
        p.screen_mut().set_scrollback(next);
    }

    fn prompt_key(&mut self, k: Key) {
        let Mode::Prompt(prompt) = &mut self.mode else { return };
        if matches!(prompt.kind, PromptKind::ConfirmQuit | PromptKind::ConfirmKill) {
            let kind = prompt.kind;
            self.mode = Mode::Normal;
            if matches!(k, Key::Char('y') | Key::Char('Y')) {
                match kind {
                    PromptKind::ConfirmQuit => self.quit = true,
                    _ => self.kill_active(),
                }
            }
            return;
        }
        match k {
            Key::Esc | Key::Ctrl('c') => self.mode = Mode::Normal,
            Key::Backspace => {
                prompt.input.pop();
            }
            Key::Ctrl('u') => prompt.input.clear(),
            Key::Char(c) => prompt.input.push(c),
            Key::Tab if prompt.kind == PromptKind::NewDir => prompt.input = complete_dir(&prompt.input),
            Key::Enter => {
                let Mode::Prompt(p) = std::mem::replace(&mut self.mode, Mode::Normal) else { return };
                match p.kind {
                    PromptKind::Rename => {
                        if let Some(t) = self.tabs.get_mut(self.active) {
                            let name = p.input.trim();
                            t.custom_name = (!name.is_empty()).then(|| name.to_string());
                        }
                    }
                    PromptKind::NewDir => {
                        let dir = history::expand_tilde(p.input.trim());
                        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
                        self.new_session(dir);
                    }
                    PromptKind::Command => self.run_command(&p.input),
                    PromptKind::SelectIndex => match p.input.trim().parse::<usize>() {
                        Ok(n) if n >= 1 => self.select(n - 1),
                        _ => self.flash("not a window index"),
                    },
                    PromptKind::ConfirmQuit | PromptKind::ConfirmKill => {}
                }
            }
            _ => {}
        }
    }
}

/// Encode a mouse event for a child, in the protocol it asked for. `row` is
/// relative to the pane. Returns `None` when the child's mode filters the event.
fn encode_mouse(m: Mouse, row: u16, mode: vt100::MouseProtocolMode, enc: vt100::MouseProtocolEncoding) -> Option<Vec<u8>> {
    use vt100::MouseProtocolMode as M;
    let wanted = match mode {
        M::None => false,
        M::Press => !m.release && !m.is_motion(),
        M::PressRelease => !m.is_motion(),
        // Motion without a held button (button bits == 3) only for AnyMotion.
        M::ButtonMotion => !m.is_motion() || m.button & 3 != 3,
        M::AnyMotion => true,
    };
    if !wanted {
        return None;
    }
    let (x, y) = (m.col as u32 + 1, row as u32 + 1);
    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            Some(format!("\x1b[<{};{};{}{}", m.button, x, y, if m.release { 'm' } else { 'M' }).into_bytes())
        }
        legacy => {
            // Legacy encodings cannot say which button was released.
            let b = if m.release { (m.button & !3) | 3 } else { m.button } as u32;
            let mut out = b"\x1b[M".to_vec();
            for v in [b + 32, x + 32, y + 32] {
                match legacy {
                    vt100::MouseProtocolEncoding::Utf8 => {
                        let c = char::from_u32(v)?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => out.push(u8::try_from(v).ok()?),
                }
            }
            Some(out)
        }
    }
}

/// Complete the last path component to the longest common prefix of matching directories.
fn complete_dir(input: &str) -> String {
    let (parent, partial) = match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("./", input),
    };
    let Ok(entries) = std::fs::read_dir(history::expand_tilde(parent)) else { return input.to_string() };
    let mut matches: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(partial) && (partial.starts_with('.') || !n.starts_with('.')))
        .collect();
    matches.sort();
    let Some(first) = matches.first() else { return input.to_string() };
    let mut common = first.clone();
    for m in &matches[1..] {
        while !m.starts_with(&common) {
            common.pop();
        }
    }
    let base = if input.contains('/') { parent } else { "" };
    if matches.len() == 1 { format!("{base}{common}/") } else { format!("{base}{common}") }
}
