//! Remote mode: the client attached to a `cmuxd` server (UI design 3, plan
//! steps 7 and 8).
//!
//! The server owns plans, agents and terminals. This half of the client keeps
//! a [`Mirror`] of its state, derives windows from it (`tui::windows`), shows
//! each terminal as a [`Pane`] over a [`RemoteTerm`], and turns keys and
//! mouse clicks into REST calls (`tui::remote`). Local mode never reaches
//! this file: every hook in `app/mod.rs` is guarded by `self.remote`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cmux_core::glyph::Glyph;
use cmux_proto::api::{NewTerminalKind, ProjectsResp};
use cmux_proto::view::{AgentId, Level, Life, PlanId, TermId, TermKind, TerminalView};
use ratatui::layout::{Position, Rect};
use serde_json::Value;

use super::{App, Mode, Place, Prompt, PromptKind};
use crate::Event;
use crate::conn::servers::{self, Endpoint};
use crate::conn::stream::{ConnHandle, Connection, Mirror, StreamConfig, StreamEvent};
use crate::history::SessionInfo;
use crate::keys::{Key, Mouse};
use crate::layout::{Axis, Node as LayoutNode, PRESETS};
use crate::pane::{Kind, Pane, SpawnSpec};
use crate::tui::draw::tree::{MIN_WIDTH, TreeMode, tree_mode, tree_width};
use crate::tui::hit::{self, TreeHit};
use crate::tui::intent::{self, TreeIntent};
use crate::tui::nav::{self, Move};
use crate::tui::remote::{
    Action, ActiveForm, FormOutcome, Pending, Request, Rest, Then, action_for, inbox_action,
};
use crate::tui::remote_term::RemoteTerm;
use crate::tui::rows;
use crate::tui::state::{Focus, Node, ViewState};
use crate::tui::windows::{Derived, PaneSpec, ServerView, WindowKey, derive};
use crate::window::Window;

/// Runs a closure off the UI thread. Tests run it inline.
pub type Spawner = fn(Box<dyn FnOnce() + Send>);

fn on_thread(f: Box<dyn FnOnce() + Send>) {
    std::thread::spawn(f);
}

#[cfg(test)]
pub fn inline(f: Box<dyn FnOnce() + Send>) {
    f();
}

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

pub struct RemoteState {
    /// The name in the tab bar and the view file.
    pub name: String,
    conn: Option<Connection>,
    pub handle: ConnHandle,
    rest: Arc<dyn Rest>,
    spawner: Spawner,
    pub mirror: Mirror,
    pub vs: ViewState,
    pub view: ServerView,
    view_path: Option<PathBuf>,
    view_dirty: bool,
    pub derived: Derived,
    /// The workspace on show.
    pub plan: Option<PlanId>,
    /// The key of each of `App::windows`.
    pub keys: Vec<WindowKey>,
    window_ids: HashMap<WindowKey, u64>,
    /// Pane ids by terminal, to find the pane of a terminal's output.
    pane_ids: HashMap<TermId, u64>,
    /// Pane ids by agent: an agent that comes back on a new terminal keeps
    /// its pane in the window and its focus.
    agent_pane_ids: HashMap<AgentId, u64>,
    pub form: Option<ActiveForm>,
    form_from_inbox: bool,
    pub pending: Option<Pending>,
    /// A terminal we asked for: show it as soon as it is in the state.
    want_term: Option<TermId>,
    /// Where the next created terminal goes when it is a split.
    pending_split: Option<(Axis, WindowKey)>,
    split_axis: HashMap<TermId, Axis>,
    /// The terminal whose size we asked for last, to focus only on a change.
    focused: Option<TermId>,
    announced: HashSet<TermId>,
    last_click: Option<(Instant, Node)>,
    /// The mouse drags the right border of the tree.
    tree_drag: bool,
    seen_state: bool,
    /// The server is cmuxd on this machine (Unix socket): our paths are its paths.
    local_server: bool,
    /// `cmux attach <agent>`: the node to show once the first state is in.
    pub open_on_start: Option<Node>,
}

impl RemoteState {
    pub fn new(
        name: String,
        conn: Option<Connection>,
        handle: ConnHandle,
        rest: Arc<dyn Rest>,
        view_path: Option<PathBuf>,
    ) -> RemoteState {
        let view = view_path
            .as_deref()
            .and_then(|p| ServerView::load(p).ok())
            .unwrap_or_default();
        RemoteState {
            name,
            conn,
            handle,
            rest,
            spawner: on_thread,
            mirror: Mirror::default(),
            vs: ViewState::default(),
            view,
            view_path,
            view_dirty: false,
            derived: Derived::default(),
            plan: None,
            keys: Vec::new(),
            window_ids: HashMap::new(),
            pane_ids: HashMap::new(),
            agent_pane_ids: HashMap::new(),
            form: None,
            form_from_inbox: false,
            pending: None,
            want_term: None,
            pending_split: None,
            split_axis: HashMap::new(),
            focused: None,
            announced: HashSet::new(),
            last_click: None,
            tree_drag: false,
            seen_state: false,
            local_server: false,
            open_on_start: None,
        }
    }

    /// Starts the connection thread for `endpoint`; its events go to the
    /// event loop.
    pub fn connect(
        name: String,
        endpoint: Endpoint,
        rest: Arc<dyn Rest>,
        tx: std::sync::mpsc::Sender<Event>,
    ) -> RemoteState {
        let local_server = matches!(endpoint, Endpoint::Unix(_));
        let conn = Connection::spawn(
            StreamConfig {
                endpoint,
                info: format!(
                    "cmux {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS
                ),
            },
            move |ev| {
                let _ = tx.send(Event::Stream(ev));
            },
        );
        let handle = conn.handle();
        let view_path = Some(servers::view_path(&servers::config_dir(), &name));
        let mut r = RemoteState::new(name, Some(conn), handle, rest, view_path);
        r.local_server = local_server;
        r
    }

    /// For tests: run REST calls inline.
    #[cfg(test)]
    pub fn inline(mut self) -> RemoteState {
        self.spawner = inline;
        self
    }

    // ---- geometry -----------------------------------------------------------

    /// Columns the in-flow tree takes (0 when hidden).
    pub fn tree_cols(&self, cols: u16) -> u16 {
        if self.view.tree_hidden {
            0
        } else if tree_mode(cols) == TreeMode::Full {
            self.full_tree_width(cols)
        } else {
            tree_width(cols).min(cols.saturating_sub(10))
        }
    }

    /// Columns of the full tree: the user's width, else the default.
    fn full_tree_width(&self, cols: u16) -> u16 {
        let max = (cols / 2).max(MIN_WIDTH);
        self.view
            .tree_width
            .map_or_else(|| tree_width(cols.max(100)), |w| w.clamp(MIN_WIDTH, max))
            .min(cols.saturating_sub(10))
    }

    /// Set the width of the full tree for a screen `cols` wide.
    pub fn set_tree_width(&mut self, w: u16, cols: u16) {
        let w = w.clamp(MIN_WIDTH, (cols / 2).max(MIN_WIDTH));
        if self.view.tree_width != Some(w) {
            self.view.tree_width = Some(w);
            self.view_dirty = true;
        }
    }

    /// Where the panes go inside the screen body `full`.
    pub fn pane_area(&self, full: Rect) -> Rect {
        let tw = self.tree_cols(full.width);
        Rect {
            x: full.x + tw,
            width: full.width - tw,
            ..full
        }
    }

    pub fn tree_rect(&self, full: Rect) -> Option<Rect> {
        let tw = self.tree_cols(full.width);
        (tw > 0).then_some(Rect { width: tw, ..full })
    }

    /// The full tree drawn over the body when the tree has focus but is a
    /// strip or hidden (`prefix Tab` at 80 columns).
    pub fn overlay_rect(&self, full: Rect) -> Option<Rect> {
        let in_flow_full = !self.view.tree_hidden && tree_mode(full.width) == TreeMode::Full;
        (!in_flow_full).then(|| Rect {
            width: self.full_tree_width(full.width.max(1)).min(full.width),
            ..full
        })
    }

    /// Where the tree is drawn now and in which mode, for drawing and hits.
    pub fn tree_place(&self, full: Rect, tree_focused: bool) -> Option<(Rect, TreeMode)> {
        if tree_focused && let Some(o) = self.overlay_rect(full) {
            return Some((o, TreeMode::Full));
        }
        self.tree_rect(full)
            .map(|r| (r, tree_mode(full.width.max(1))))
    }

    // ---- glyphs and counts --------------------------------------------------

    pub fn pane_glyph(&self, p: &Pane) -> Option<Glyph> {
        let a = self.mirror.sv.agent(p.agent.as_ref()?)?;
        Some(rows::agent_glyph(&self.mirror.sv, &self.vs, a))
    }

    /// The end of a finished agent, for its pane header.
    pub fn pane_outcome(&self, p: &Pane) -> Option<(&'static str, &'static str)> {
        outcome(self.mirror.sv.agent(p.agent.as_ref()?)?)
    }

    /// The glyph a tab shows: the most urgent of its panes.
    pub fn window_glyph(&self, w: &Window) -> Option<Glyph> {
        let glyphs: Vec<Glyph> = w.panes.iter().filter_map(|p| self.pane_glyph(p)).collect();
        [
            Glyph::NeedsYou,
            Glyph::Stalled,
            Glyph::Working,
            Glyph::Unseen,
            Glyph::Failed,
        ]
        .into_iter()
        .find(|g| glyphs.contains(g))
        .or_else(|| glyphs.first().copied())
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let sv = &self.mirror.sv;
        let working = sv
            .agents
            .iter()
            .filter(|a| rows::agent_glyph(sv, &self.vs, a) == Glyph::Working)
            .count();
        (
            working,
            rows::needs_you(sv).len(),
            self.mirror.presence.len(),
        )
    }

    fn any_working(&self) -> bool {
        self.counts().0 > 0
    }

    fn dispatch(&self, tx: &std::sync::mpsc::Sender<Event>, req: Request) {
        let rest = self.rest.clone();
        let tx = tx.clone();
        (self.spawner)(Box::new(move || {
            let result = rest.send(&req.call);
            let _ = tx.send(Event::Rest {
                then: req.then,
                result,
            });
        }));
    }

    fn save_view(&mut self) {
        if !std::mem::take(&mut self.view_dirty) {
            return;
        }
        if let Some(p) = &self.view_path {
            let _ = self.view.save(p);
        }
    }
}

/// Maps a server terminal kind to the pane kind.
fn pane_kind(k: &TermKind) -> Kind {
    match k {
        TermKind::Claude => Kind::Claude,
        TermKind::Shell => Kind::Shell,
        TermKind::Command(l) => Kind::Command(l.clone()),
    }
}

fn agent_label(a: &AgentId) -> String {
    use cmux_core::ids::AgentPath;
    match a.path() {
        AgentPath::Executor => "executor".into(),
        AgentPath::Task(t) => t.to_string(),
        AgentPath::Sub(_, n) => n.to_string(),
    }
}

impl App {
    /// An app attached to a server. `start` then waits for the first state.
    pub fn new_remote(
        opts: crate::Options,
        events: std::sync::mpsc::Sender<Event>,
        rows: u16,
        cols: u16,
        remote: RemoteState,
    ) -> App {
        let mut app = App::new(opts, events, rows, cols);
        app.remote = Some(Box::new(remote));
        app
    }

    pub(super) fn start_remote(&mut self) {
        // No default session: the server's windows come with its state.
        if self.opts.continue_last {
            let cwd = self.home_cwd.clone();
            self.open_claude(cwd, vec!["--continue".to_string()], None, Place::Window);
        }
        let local = self.remote.as_ref().is_some_and(|r| r.local_server);
        if !local && !self.opts.dirs.is_empty() {
            // Laptop paths mean nothing on another machine.
            self.flash("directories are ignored on a remote server");
        } else {
            for dir in self.opts.dirs.clone() {
                self.new_session(dir, Place::Window);
            }
        }
        if self.opts.resume_picker {
            self.open_picker();
        }
    }

    pub(super) fn shutdown_remote(&mut self) {
        // Detach: the terminals and agents keep running on the server.
        for p in self.windows.iter_mut().flat_map(|w| w.panes.iter_mut()) {
            p.kill();
        }
        self.persist_view(true);
        if let Some(r) = self.remote.as_mut()
            && let Some(mut c) = r.conn.take()
        {
            c.shutdown();
        }
    }

    fn dispatch(&mut self, req: Request) {
        if let Some(r) = &self.remote {
            r.dispatch(&self.events, req);
        }
    }

    // ---- events from the connection and from REST ---------------------------

    pub(super) fn on_stream(&mut self, ev: StreamEvent) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if r.mirror.apply(&ev) {
            self.redraw = true;
        }
        match ev {
            StreamEvent::State(_) | StreamEvent::Deltas { .. } => {
                nav::fix_selection(&r.mirror.sv, &mut r.vs);
                let first = !std::mem::replace(&mut r.seen_state, true);
                let open = if first { r.open_on_start.take() } else { None };
                self.rebuild();
                if let Some(node) = open {
                    if self.show_node(&node) {
                        self.set_tree_focus(false);
                    }
                } else if first && self.windows.is_empty() && matches!(self.mode, Mode::Normal) {
                    // Nothing to look at yet: start in the tree.
                    self.set_tree_focus(true);
                }
            }
            StreamEvent::TermOutput(t) => {
                if let Some(&id) = r.pane_ids.get(&t) {
                    self.on_output(id);
                }
            }
            StreamEvent::TermReset(_) | StreamEvent::Size { .. } => self.redraw = true,
            StreamEvent::Exit { term, .. } => {
                // A shell or command that ended closes its window, as locally.
                if r.mirror.sv.loose.contains(&term) {
                    // Every attached client sees the exit; the losers get 404.
                    self.dispatch(Request::close_terminal(term).then(Then::Quiet));
                }
            }
            StreamEvent::Error { message, .. } => self.flash(message),
            StreamEvent::AuthFailed(m) => {
                self.flash(format!("{m} - run `cmux login` again"));
            }
            _ => {}
        }
    }

    pub(super) fn on_rest(&mut self, then: Then, result: Result<Value, String>) {
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                if then == Then::Sessions {
                    self.picker.loading = false;
                }
                if then != Then::Quiet {
                    self.flash(e);
                }
                return;
            }
        };
        match then {
            Then::Nothing | Then::Quiet => {}
            Then::FocusTerm => {
                let Ok(t) = serde_json::from_value::<TerminalView>(value) else {
                    return;
                };
                if let Some(r) = self.remote.as_mut() {
                    r.want_term = Some(t.id);
                    if let Some((axis, key)) = r.pending_split.take() {
                        r.view.attached.insert(t.id.to_string(), key.encode());
                        r.split_axis.insert(t.id, axis);
                        r.view_dirty = true;
                    }
                }
                self.rebuild();
            }
            Then::Sessions => {
                let Ok(list) = serde_json::from_value::<ProjectsResp>(value) else {
                    return;
                };
                self.picker.sessions = list
                    .sessions
                    .into_iter()
                    .map(|s| SessionInfo {
                        id: s.id,
                        cwd: PathBuf::from(s.cwd),
                        title: s.title,
                        first_prompt: s.first_prompt,
                        last_prompt: s.last_prompt,
                        modified: std::time::UNIX_EPOCH + Duration::from_secs(s.modified),
                        branch: s.branch,
                    })
                    .collect();
                self.picker.loading = false;
                self.refilter();
            }
        }
        self.redraw = true;
    }

    pub(super) fn request_sessions(&mut self) {
        self.dispatch(Request::sessions());
    }

    pub(super) fn remote_tick(&mut self) {
        let tick = self.tick;
        if let Some(r) = self.remote.as_mut() {
            r.vs.spinner = tick as usize;
            if r.any_working() {
                self.redraw = true;
            }
        }
        if self.tick.is_multiple_of(4) {
            self.persist_view(false);
        }
        if let Some((_, at)) = &self.message
            && at.elapsed() > Duration::from_secs(4)
        {
            self.message = None;
            self.redraw = true;
        }
        if self.picker.loading {
            self.redraw = true;
        }
    }

    /// Writes what the user chose (current window, zoom, layout preset) to
    /// the per-server view file, when it changed.
    fn persist_view(&mut self, force: bool) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if let Some(k) = r.keys.get(self.active) {
            let enc = k.encode();
            if r.view.window.as_deref() != Some(enc.as_str()) {
                r.view.window = Some(enc);
                r.view_dirty = true;
            }
        }
        let plan = r.plan.as_ref().map(ToString::to_string);
        if r.view.plan != plan {
            r.view.plan = plan;
            r.view_dirty = true;
        }
        for (k, w) in r.keys.iter().zip(&self.windows) {
            let active = w.pane().remote_term().map(|t| t.term.to_string());
            let wv = r.view.window_mut(k);
            let key = w
                .pane()
                .agent
                .as_ref()
                .map(ToString::to_string)
                .or_else(|| active.map(|t| format!("term {t}")));
            if wv.preset != w.preset || wv.zoomed != w.zoomed || wv.active != key {
                wv.preset = w.preset;
                wv.zoomed = w.zoomed;
                wv.active = key;
                r.view_dirty = true;
            }
        }
        if force {
            r.view_dirty = true;
        }
        r.save_view();
    }

    // ---- windows from state -------------------------------------------------

    pub(crate) fn rebuild(&mut self) {
        self.rebuild_to(None);
    }

    /// Re-derives the windows from the mirror. `target` forces the workspace
    /// and window to show; otherwise the current ones stay.
    fn rebuild_to(&mut self, target: Option<(Option<PlanId>, WindowKey)>) {
        let Some(mut r) = self.remote.take() else {
            return;
        };
        // What the user is looking at stays on screen.
        if let Some(k) = r.keys.get(self.active) {
            r.view.window = Some(k.encode());
        }
        r.view.plan = r.plan.as_ref().map(ToString::to_string);
        if let Some((plan, key)) = target {
            r.view.plan = plan.map(|p| p.to_string());
            r.view.window = Some(key.encode());
        }
        let sv = r.mirror.sv.clone();
        let mut derived = derive(&sv, &r.view);
        derived.attach(&r.view);
        let (plan, mut cur) = derived.current(&r.view);
        let mut show_new = false;
        let specs = derived.windows(plan.as_ref()).to_vec();
        if let Some(t) = r.want_term
            && let Some(w) = specs
                .iter()
                .find(|w| w.panes.iter().any(|p| p.term == Some(t)))
        {
            cur = Some(w.key.clone());
            r.want_term = None;
            show_new = true;
        }
        r.vs.seen.retain(|a| sv.agent(a).is_some());

        // Take every pane out of the old windows; they come back by terminal.
        let area = self.body_area();
        let mut old: HashMap<u64, Window> = self.windows.drain(..).map(|w| (w.id, w)).collect();
        let mut old_panes: HashMap<TermId, Pane> = HashMap::new();
        let mut old_shapes: HashMap<u64, WindowShape> = HashMap::new();
        for (wid, w) in old.drain() {
            let ids: Vec<u64> = w.panes.iter().map(|p| p.id).collect();
            old_shapes.insert(
                wid,
                WindowShape {
                    ids,
                    layout: w.layout,
                    active: w.active,
                    last: w.last,
                    zoomed: w.zoomed,
                    preset: w.preset,
                },
            );
            for p in w.panes {
                if let Some(t) = p.remote_term().map(|t| t.term) {
                    old_panes.insert(t, p);
                }
            }
        }

        let mut windows = Vec::new();
        let mut keys = Vec::new();
        for spec in &specs {
            let panes: Vec<&PaneSpec> = spec
                .panes
                .iter()
                .filter(|p| p.term.is_some_and(|t| sv.terminal(t).is_some()))
                .collect();
            if panes.is_empty() {
                continue;
            }
            let wid = *r.window_ids.entry(spec.key.clone()).or_insert_with(|| {
                let id = self.next_id;
                self.next_id += 1;
                id
            });
            let mut built = Vec::new();
            for ps in &panes {
                let term = ps.term.expect("filtered");
                let pid = match &ps.agent {
                    Some(a) => *r.agent_pane_ids.entry(a.clone()).or_insert_with(|| {
                        let id = self.next_id;
                        self.next_id += 1;
                        id
                    }),
                    None => *r.pane_ids.entry(term).or_insert_with(|| {
                        let id = self.next_id;
                        self.next_id += 1;
                        id
                    }),
                };
                r.pane_ids.insert(term, pid);
                let mut pane = old_panes.remove(&term).unwrap_or_else(|| {
                    let rt = RemoteTerm::new(
                        r.handle.clone(),
                        term,
                        area.height.max(1),
                        area.width.max(1),
                    );
                    Pane::remote(pid, rt, Kind::Shell, PathBuf::new())
                });
                pane.id = pid;
                describe_pane(&mut pane, ps, &sv);
                built.push(pane);
            }
            let ids: Vec<u64> = built.iter().map(|p| p.id).collect();
            let wv = r.view.windows.get(&spec.key.encode()).cloned();
            let mut win = match old_shapes.remove(&wid) {
                Some(shape) => {
                    reuse_window(wid, shape, built, &ids, &mut r.split_axis, &r.pane_ids)
                }
                None => {
                    let preset = wv.as_ref().and_then(|v| v.preset);
                    let layout = LayoutNode::preset(PRESETS[preset.unwrap_or(2)], &ids)
                        .unwrap_or(LayoutNode::Leaf(ids[0]));
                    let active = wv
                        .as_ref()
                        .and_then(|v| v.active.as_deref())
                        .and_then(|k| {
                            built
                                .iter()
                                .find(|p| pane_key(p).as_deref() == Some(k))
                                .map(|p| p.id)
                        })
                        .unwrap_or(ids[0]);
                    Window {
                        id: wid,
                        panes: built,
                        layout,
                        active,
                        last: None,
                        zoomed: wv.as_ref().is_some_and(|v| v.zoomed) && ids.len() > 1,
                        custom_name: None,
                        preset,
                    }
                }
            };
            win.custom_name = Some(spec.title.clone());
            windows.push(win);
            keys.push(spec.key.clone());
        }
        // Panes whose terminal is gone are dropped here (the Drop unsubscribes).
        drop(old_panes);
        r.pane_ids.retain(|t, _| sv.terminal(*t).is_some());
        r.agent_pane_ids.retain(|a, _| sv.agent(a).is_some());
        r.window_ids.retain(|k, _| keys.contains(k));
        r.announced.retain(|t| sv.terminal(*t).is_some());
        r.view.prune(&derived);
        r.derived = derived;
        r.plan = plan;

        let idx = cur
            .as_ref()
            .and_then(|k| keys.iter().position(|x| x == k))
            .unwrap_or(0);
        r.keys = keys;
        self.windows = windows;
        self.active = idx.min(self.windows.len().saturating_sub(1));
        if let Some(w) = self.windows.get_mut(self.active) {
            let id = w.active;
            w.focus(id);
        }
        self.remote = Some(r);
        self.relayout();
        if show_new {
            // The terminal the user asked for takes the focus.
            self.set_tree_focus(false);
        }
        self.sync_remote();
        self.redraw = true;
    }

    /// Size, focus and seen marks of the pane that is on screen.
    fn sync_remote(&mut self) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let Some(w) = self.windows.get(self.active) else {
            return;
        };
        // Tell the server our size for terminals nobody sized yet.
        for p in &w.panes {
            if let Some(t) = p.remote_term()
                && r.announced.insert(t.term)
                && r.mirror
                    .sv
                    .terminal(t.term)
                    .is_none_or(|v| v.size_owner.is_none())
            {
                t.announce_size();
            }
        }
        if r.vs.focus != Focus::Pane || !matches!(self.mode, Mode::Normal | Mode::Prefix) {
            return;
        }
        let pane = w.pane();
        if let Some(a) = &pane.agent
            && r.vs.seen.insert(a.clone())
        {
            self.redraw = true;
        }
        if let Some(t) = pane.remote_term()
            && r.focused != Some(t.term)
        {
            r.focused = Some(t.term);
            t.focus();
        }
    }

    pub(super) fn after_input(&mut self) {
        if self.remote.is_some() {
            self.sync_remote();
        }
    }

    // ---- focus --------------------------------------------------------------

    /// Tree focus on or off, and the matching mode.
    pub(crate) fn set_tree_focus(&mut self, on: bool) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        r.vs.focus = if on { Focus::Tree } else { Focus::Pane };
        self.mode = if on { Mode::Tree } else { Mode::Normal };
        if on {
            r.focused = None;
            // Start from where the pane is.
            let agent = self
                .windows
                .get(self.active)
                .and_then(|w| w.pane().agent.clone());
            if let Some(a) = agent {
                nav::select_agent(&r.mirror.sv, &mut r.vs, &a);
            }
        }
        self.redraw = true;
    }

    /// After a prompt or overlay closed: back to tree keys if the tree had focus.
    pub(super) fn restore_focus_mode(&mut self) {
        if let Some(r) = &self.remote
            && r.vs.focus == Focus::Tree
            && matches!(self.mode, Mode::Normal)
        {
            self.mode = Mode::Tree;
        }
    }

    // ---- showing nodes ------------------------------------------------------

    /// Shows the window of a tree node. `false` (with a message) when the node
    /// has none yet, e.g. a task that has not started.
    pub(crate) fn show_node(&mut self, node: &Node) -> bool {
        let Some(r) = self.remote.as_ref() else {
            return false;
        };
        let sv = &r.mirror.sv;
        let (plan, key, agent) = match node {
            Node::Plan(p) => {
                let first = r
                    .derived
                    .workspace(p)
                    .and_then(|w| w.windows.first())
                    .map(|w| w.key.clone());
                let Some(first) = first else {
                    return self.fail_show(format!("{p} has no windows yet"));
                };
                (Some(p.clone()), first, None)
            }
            Node::Executor(p) => (Some(p.clone()), WindowKey::Executor(p.clone()), None),
            // The task row is the task's own agent, not one of its sub-agents.
            Node::Task(p, t) => (
                Some(p.clone()),
                WindowKey::Task(p.clone(), *t),
                node.agent(sv),
            ),
            Node::Sub(a) => {
                let Some(t) = a.path().task_id() else {
                    return false;
                };
                (
                    Some(a.plan().clone()),
                    WindowKey::Task(a.plan().clone(), t),
                    Some(a.clone()),
                )
            }
            Node::Loose => {
                let first = r.derived.loose.first().map(|w| w.key.clone());
                let Some(first) = first else {
                    return self.fail_show("no loose terminals".into());
                };
                (r.plan.clone(), first, None)
            }
        };
        self.rebuild_to(Some((plan, key.clone())));
        let Some(idx) = self
            .remote
            .as_ref()
            .and_then(|r| r.keys.iter().position(|k| *k == key))
        else {
            let what = match node {
                Node::Task(_, t) => t.to_string(),
                _ => "that agent".into(),
            };
            return self.fail_show(format!("{what} has not started yet"));
        };
        self.select_window(idx);
        if let Some(a) = agent
            && let Some(w) = self.windows.get_mut(idx)
            && let Some(id) = w
                .panes
                .iter()
                .find(|p| p.agent.as_ref() == Some(&a))
                .map(|p| p.id)
        {
            // Only a jump to another pane leaves the zoom.
            if w.active != id {
                w.zoomed = false;
            }
            w.focus(id);
            self.relayout();
        }
        true
    }

    fn fail_show(&mut self, msg: String) -> bool {
        self.flash(msg);
        false
    }

    // ---- creating and closing terminals -------------------------------------

    pub(super) fn spawn_remote(&mut self, spec: SpawnSpec, place: Place) -> bool {
        let area = self.body_area();
        let local = self.remote.as_ref().is_some_and(|r| r.local_server);
        // On another machine only a pane's own directory (a server path) is
        // meaningful, never our fallback `home_cwd`.
        let cwd = if local {
            let abs = std::path::absolute(&spec.cwd).unwrap_or_else(|_| spec.cwd.clone());
            Some(abs.display().to_string())
        } else {
            (!self.windows.is_empty() && spec.cwd != self.home_cwd)
                .then(|| spec.cwd.display().to_string())
        };
        let kind = match spec.kind {
            Kind::Claude => NewTerminalKind::Claude { args: spec.args },
            Kind::Shell => NewTerminalKind::Shell,
            Kind::Command(label) => {
                // `open_command` runs `$SHELL -lc CMD`; the server's shell is its own.
                let mut argv = vec!["sh".to_string()];
                if spec.args.first().map(String::as_str) == Some("-lc") {
                    argv.extend(spec.args);
                } else {
                    argv = vec![spec.program];
                    argv.extend(spec.args);
                }
                NewTerminalKind::Command {
                    argv,
                    label: Some(label),
                }
            }
        };
        if let (Place::Split(axis), Some(r)) = (place, self.remote.as_mut())
            && let Some(key) = r.keys.get(self.active)
        {
            r.pending_split = Some((axis, key.clone()));
        }
        self.dispatch(Request::new_terminal(
            kind,
            cwd,
            area.height.max(1),
            area.width.max(1),
        ));
        true
    }

    pub(super) fn resume_remote(&mut self, s: &SessionInfo, fork: bool) {
        let mut args = vec!["--resume".to_string(), s.id.clone()];
        if fork {
            args.push("--fork-session".into());
        }
        args.extend(self.opts.claude_args.iter().cloned());
        let area = self.body_area();
        self.dispatch(Request::new_terminal(
            NewTerminalKind::Claude { args },
            Some(s.cwd.display().to_string()),
            area.height.max(1),
            area.width.max(1),
        ));
    }

    pub(super) fn kill_pane_remote(&mut self) {
        let Some(w) = self.windows.get(self.active) else {
            return;
        };
        let pane = w.pane();
        let (agent, term) = (pane.agent.clone(), pane.remote_term().map(|t| t.term));
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        match (agent, term) {
            (Some(a), _) => {
                // A sub-agent that reported is done though its process may
                // linger; a task agent that reported goes on (PR, CI) and only
                // an ended process is done.
                let finished = r.mirror.sv.agent(&a).is_none_or(|v| {
                    if v.level == Level::Sub {
                        crate::tui::marks::agent_done(v)
                    } else {
                        matches!(v.life, Life::Exited { .. } | Life::Stopped)
                    }
                });
                if finished {
                    r.view.closed.insert(a.to_string());
                    r.view_dirty = true;
                    self.rebuild();
                } else {
                    self.dispatch(Pending::Stop(a).request());
                }
            }
            (None, Some(t)) => {
                r.view.attached.remove(&t.to_string());
                r.view_dirty = true;
                self.dispatch(Request::close_terminal(t));
            }
            _ => {}
        }
    }

    pub(super) fn kill_window_remote(&mut self) {
        let Some(w) = self.windows.get(self.active) else {
            return;
        };
        let main = w.panes.first().and_then(|p| p.agent.clone());
        let terms: Vec<Request> = w
            .panes
            .iter()
            .filter_map(|p| match (&p.agent, p.remote_term()) {
                (None, Some(t)) => Some(Request::close_terminal(t.term)),
                _ => None,
            })
            .collect();
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if let Some(a) = main {
            let finished = r.mirror.sv.agent(&a).is_none_or(|v| {
                matches!(
                    v.life,
                    cmux_proto::view::Life::Exited { .. } | cmux_proto::view::Life::Stopped
                )
            });
            if finished {
                // Nothing to stop: hide the window instead.
                r.view.closed.insert(a.to_string());
                r.view_dirty = true;
                self.rebuild();
            } else {
                self.dispatch(Pending::Stop(a).request());
            }
        }
        for req in terms {
            self.dispatch(req);
        }
    }

    pub(super) fn rename_remote(&mut self, name: &str) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let Some(key) = r.keys.get(self.active).cloned() else {
            return;
        };
        r.view.window_mut(&key).name = (!name.is_empty()).then(|| name.to_string());
        r.view_dirty = true;
        self.rebuild();
    }

    pub(super) fn confirm_remote(&mut self, yes: bool) {
        let Some(p) = self.remote.as_mut().and_then(|r| r.pending.take()) else {
            return;
        };
        if yes {
            self.dispatch(p.request());
        }
    }

    // ---- keys ---------------------------------------------------------------

    /// Prefix keys of UI design 3.5 and the ones that differ on a server.
    /// Returns whether the key was handled.
    pub(super) fn remote_prefix(&mut self, k: Key) -> bool {
        match k {
            Key::Tab => {
                let on = !matches!(self.mode, Mode::Tree) && {
                    self.remote
                        .as_ref()
                        .is_some_and(|r| r.vs.focus != Focus::Tree)
                };
                self.set_tree_focus(on);
            }
            // Detach: the server keeps every agent running.
            Key::Char('d') | Key::Char('q') => self.quit = true,
            Key::Char('e') => {
                if let Some(p) = self.remote.as_ref().and_then(|r| r.plan.clone()) {
                    self.show_node(&Node::Executor(p));
                } else {
                    self.flash("no plan");
                }
            }
            Key::Char('u') => self.go_up(),
            Key::Char('a') => self.open_inbox(),
            Key::Alt('a') | Key::Alt('A') => {
                let fwd = k == Key::Alt('a');
                let node = self
                    .remote
                    .as_mut()
                    .and_then(|r| nav::jump_needs_you(&r.mirror.sv, &mut r.vs, fwd));
                match node {
                    Some(n) => {
                        if self.show_node(&n) {
                            self.set_tree_focus(false);
                        }
                    }
                    None => self.flash("nothing needs you"),
                }
            }
            Key::Char('m') => {
                let agent = self
                    .windows
                    .get(self.active)
                    .and_then(|w| w.pane().agent.clone());
                match agent {
                    Some(a) => self.do_action(Action::Form(ActiveForm::Message(
                        crate::tui::forms::MessageForm::new(a),
                    ))),
                    None => self.flash("this pane has no agent to message"),
                }
            }
            Key::Char('v') => {
                self.mode = if matches!(self.mode, Mode::Board) {
                    Mode::Normal
                } else {
                    Mode::Board
                };
            }
            Key::Char('P') => self.next_plan(),
            Key::Char('B') => {
                if let Some(r) = self.remote.as_mut() {
                    r.view.tree_hidden = !r.view.tree_hidden;
                    r.view_dirty = true;
                }
                self.relayout();
            }
            Key::Char('=') => self.take_size(),
            Key::Char('S') => self.flash("one server per cmux; start another with cmux -s NAME"),
            Key::Char('<') => self.resize_tree(-2),
            Key::Char('>') => self.resize_tree(2),
            Key::Char('!') => self.flash("panes follow the server's agents"),
            _ => return false,
        }
        true
    }

    /// Make the full tree `delta` columns wider (or narrower).
    fn resize_tree(&mut self, delta: i32) {
        let cols = self.screen_body().width;
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if r.view.tree_hidden || tree_mode(cols) != TreeMode::Full {
            self.flash("the tree is too narrow to resize here");
            return;
        }
        let w = (i32::from(r.tree_cols(cols)) + delta).max(0) as u16;
        r.set_tree_width(w, cols);
        self.relayout();
    }

    fn go_up(&mut self) {
        let Some(a) = self
            .windows
            .get(self.active)
            .and_then(|w| w.pane().agent.clone())
        else {
            return;
        };
        let here = Node::for_agent(&a);
        match here.parent() {
            // A task goes up to the executor; the executor is the top.
            Some(Node::Plan(_)) if matches!(here, Node::Executor(_)) => {
                self.flash("already at the top");
            }
            Some(Node::Plan(p)) => {
                self.show_node(&Node::Executor(p));
            }
            Some(n) => {
                self.show_node(&n);
            }
            None => self.flash("already at the top"),
        }
    }

    fn next_plan(&mut self) {
        let Some(r) = self.remote.as_ref() else {
            return;
        };
        let plans = &r.mirror.sv.plans;
        if plans.is_empty() {
            return;
        }
        let i = r
            .plan
            .as_ref()
            .and_then(|p| plans.iter().position(|x| &x.id == p))
            .map_or(0, |i| (i + 1) % plans.len());
        let next = plans[i].id.clone();
        self.show_node(&Node::Plan(next));
    }

    fn take_size(&mut self) {
        let Some(t) = self
            .windows
            .get(self.active)
            .and_then(|w| w.pane().remote_term())
        else {
            return;
        };
        t.announce_size();
        t.focus();
        self.flash("took over the terminal size");
    }

    fn open_inbox(&mut self) {
        if let Some(r) = self.remote.as_mut() {
            nav::inbox_move(&r.mirror.sv, &mut r.vs, 0);
        }
        self.mode = Mode::Inbox;
    }

    /// A tree-focus key, also used by the board for the selected task.
    pub(super) fn tree_key(&mut self, k: Key) {
        match k {
            Key::Char('<') => return self.resize_tree(-2),
            Key::Char('>') => return self.resize_tree(2),
            _ => {}
        }
        let intent = {
            let Some(r) = self.remote.as_mut() else {
                return;
            };
            intent::handle_tree_key(&r.mirror.sv, &mut r.vs, k)
        };
        if let Some(i) = intent {
            self.tree_intent(i);
        }
        self.redraw = true;
    }

    pub(crate) fn tree_intent(&mut self, i: TreeIntent) {
        let action = {
            let Some(r) = self.remote.as_ref() else {
                return;
            };
            action_for(&r.mirror.sv, i)
        };
        if matches!(action, Action::Back) {
            self.set_tree_focus(false);
        } else {
            self.do_action(action);
        }
    }

    pub(crate) fn do_action(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Open(n) => {
                if self.show_node(&n) {
                    self.set_tree_focus(false);
                }
            }
            Action::Preview(n) => {
                self.show_node(&n);
            }
            Action::Jump(a) => {
                if self.show_node(&Node::for_agent(&a)) {
                    self.set_tree_focus(false);
                }
            }
            Action::Form(f) => {
                if let Some(r) = self.remote.as_mut() {
                    r.form_from_inbox = matches!(self.mode, Mode::Inbox);
                    r.form = Some(f);
                }
                self.mode = Mode::Form;
            }
            Action::Confirm(p) => {
                let label = p.prompt();
                if let Some(r) = self.remote.as_mut() {
                    r.pending = Some(p);
                }
                self.mode = Mode::Prompt(Prompt {
                    kind: PromptKind::ConfirmRemote,
                    input: label,
                });
            }
            Action::Send(req) => self.dispatch(req),
            Action::Board => self.mode = Mode::Board,
            Action::Inbox => self.open_inbox(),
            Action::Help => self.mode = Mode::Help,
            Action::Back => {
                self.mode = Mode::Normal;
            }
            Action::OpenUrl(url) => {
                if web_url(&url) {
                    open_in_browser(&url);
                } else {
                    self.flash("refusing to open a non-https URL");
                }
            }
            Action::Terminal { argv, label, cwd } => {
                let area = self.body_area();
                self.dispatch(Request::new_terminal(
                    NewTerminalKind::Command {
                        argv,
                        label: Some(label.to_string()),
                    },
                    cwd,
                    area.height.max(1),
                    area.width.max(1),
                ));
                self.set_tree_focus(false);
            }
            Action::Flash(m) => self.flash(m),
        }
        self.redraw = true;
    }

    pub(super) fn inbox_key(&mut self, k: Key) {
        let intent = {
            let Some(r) = self.remote.as_mut() else {
                return;
            };
            intent::handle_inbox_key(&r.mirror.sv, &mut r.vs, k)
        };
        match intent.map(inbox_action) {
            Some(Action::Back) => self.mode = Mode::Normal,
            Some(a) => self.do_action(a),
            None => {}
        }
        // The answered item leaves the list: keep the cursor on a row.
        if let Some(r) = self.remote.as_mut() {
            nav::inbox_move(&r.mirror.sv, &mut r.vs, 0);
        }
    }

    pub(super) fn form_key(&mut self, k: Key) {
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        let Some(form) = r.form.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        let outcome = form.handle(k);
        if matches!(outcome, FormOutcome::Editing) {
            return;
        }
        r.form = None;
        self.mode = if r.form_from_inbox {
            Mode::Inbox
        } else {
            Mode::Normal
        };
        if let FormOutcome::Send(req) = outcome {
            self.dispatch(req);
        }
    }

    fn board_plan(&self) -> Option<PlanId> {
        let r = self.remote.as_ref()?;
        r.plan
            .clone()
            .or_else(|| r.vs.current_plan(&r.mirror.sv).cloned())
    }

    pub(super) fn board_key(&mut self, k: Key) {
        let Some(plan) = self.board_plan() else {
            self.mode = Mode::Normal;
            return;
        };
        let mv = match k {
            Key::Up | Key::Char('k') => Some(Move::Up),
            Key::Down | Key::Char('j') => Some(Move::Down),
            Key::Left | Key::Char('h') => Some(Move::Left),
            Key::Right | Key::Char('l') => Some(Move::Right),
            Key::Home => Some(Move::First),
            Key::End => Some(Move::Last),
            _ => None,
        };
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if let Some(mv) = mv {
            nav::board_move(&r.mirror.sv, &mut r.vs, &plan, mv);
            return;
        }
        match k {
            Key::Esc | Key::Char('q') | Key::Char('v') => {
                self.mode = Mode::Normal;
                return;
            }
            Key::Enter => {
                if let Some(t) = nav::board_selected(&r.mirror.sv, &r.vs, &plan)
                    && self.show_node(&Node::Task(plan, t))
                {
                    self.set_tree_focus(false);
                }
                return;
            }
            _ => {}
        }
        // Everything else acts on the selected card like a tree key.
        if let Some(t) = nav::board_selected(&r.mirror.sv, &r.vs, &plan) {
            r.vs.selection = Some(Node::Task(plan, t));
        }
        let was_tree = r.vs.focus;
        self.tree_key(k);
        // Intents that open overlays replace the board; the rest stay on it.
        if matches!(self.mode, Mode::Normal | Mode::Tree) {
            self.mode = Mode::Board;
        }
        if let Some(r) = self.remote.as_mut() {
            r.vs.focus = was_tree;
        }
    }

    // ---- mouse --------------------------------------------------------------

    /// Mouse handling for remote mode. Returns whether the event was consumed.
    pub(super) fn remote_mouse(&mut self, m: Mouse) -> bool {
        if m.row == 0 {
            return false;
        }
        let full = self.screen_body();
        let pane_area = self.body_area();
        let (col, row) = (m.col, m.row);
        // A tree border drag holds the mouse until the button is released.
        if let Some(r) = self.remote.as_mut()
            && r.tree_drag
        {
            if m.release {
                r.tree_drag = false;
            } else if m.is_motion() {
                r.set_tree_width(col.saturating_sub(full.x) + 1, full.width);
                self.relayout();
            }
            return true;
        }
        match self.mode {
            Mode::Form => return true,
            Mode::Inbox => {
                let Some(r) = self.remote.as_mut() else {
                    return true;
                };
                if m.wheel_up() {
                    nav::inbox_move(&r.mirror.sv, &mut r.vs, -1);
                } else if m.wheel_down() {
                    nav::inbox_move(&r.mirror.sv, &mut r.vs, 1);
                } else if m.is_left_press() {
                    match hit::inbox_hit(full, &r.mirror.sv, &r.vs, col, row) {
                        Some((i, _)) => r.vs.inbox_sel = i,
                        None if !hit::in_inbox_popup(full, col, row) => self.mode = Mode::Normal,
                        None => {}
                    }
                }
                return true;
            }
            _ => {}
        }
        let tree_focused = matches!(self.mode, Mode::Tree);
        let place = self
            .remote
            .as_ref()
            .and_then(|r| r.tree_place(full, tree_focused));
        if let Some((rect, mode)) = place
            && rect.contains(Position::new(col, row))
        {
            self.tree_mouse(m, rect, mode);
            return true;
        }
        if tree_focused && m.is_left_press() {
            // A click in the body gives the focus back to the pane.
            self.set_tree_focus(false);
            return false;
        }
        if matches!(self.mode, Mode::Board) {
            if m.is_left_press()
                && let (Some(plan), Some(r)) = (self.board_plan(), self.remote.as_mut())
                && let Some(t) = hit::board_hit(pane_area, &r.mirror.sv, &r.vs, &plan, col, row)
            {
                let double = r.board_double_click(t);
                r.vs.board_sel = Some(t);
                if double && self.show_node(&Node::Task(plan, t)) {
                    self.set_tree_focus(false);
                }
            }
            return true;
        }
        false
    }

    fn tree_mouse(&mut self, m: Mouse, rect: Rect, mode: TreeMode) {
        let full = self.screen_body();
        let Some(r) = self.remote.as_mut() else {
            return;
        };
        if m.wheel_up() || m.wheel_down() {
            let mv = if m.wheel_up() { Move::Up } else { Move::Down };
            nav::navigate(&r.mirror.sv, &mut r.vs, mv);
            return;
        }
        if !m.is_left_press() {
            return;
        }
        // The right border of the in-flow full tree resizes it.
        if mode == TreeMode::Full && m.col + 1 == rect.right() && r.tree_rect(full) == Some(rect) {
            r.tree_drag = true;
            return;
        }
        let hit = hit::tree_hit(rect, mode, &r.mirror.sv, &r.vs, m.col, m.row);
        match hit {
            Some(TreeHit::Row(node)) => {
                let double = r.tree_double_click(&node);
                r.vs.selection = Some(node.clone());
                if double {
                    self.do_action(Action::Open(node));
                } else {
                    // A click shows the window and keeps the tree focused.
                    if !matches!(self.mode, Mode::Tree) {
                        self.set_tree_focus(true);
                    }
                    self.do_action(Action::Preview(node));
                }
            }
            Some(TreeHit::Arrow(node)) => nav::toggle(&r.mirror.sv, &mut r.vs, &node),
            Some(TreeHit::NeedsYou(id)) => {
                let node = r.mirror.sv.item(id).and_then(rows::item_node);
                if let Some(n) = node {
                    self.do_action(Action::Open(n));
                }
            }
            Some(TreeHit::NeedsSummary) => self.open_inbox(),
            // A click anywhere else gives the tree the focus.
            None => self.set_tree_focus(true),
        }
    }
}

impl RemoteState {
    fn tree_double_click(&mut self, node: &Node) -> bool {
        let now = Instant::now();
        let double = matches!(&self.last_click, Some((t, n)) if n == node && now.duration_since(*t) < DOUBLE_CLICK);
        self.last_click = (!double).then(|| (now, node.clone()));
        double
    }

    fn board_double_click(&mut self, task: cmux_proto::view::TaskId) -> bool {
        let Some(plan) = self.plan.clone() else {
            return false;
        };
        self.tree_double_click(&Node::Task(plan, task))
    }
}

/// What the window looked like before a rebuild.
struct WindowShape {
    ids: Vec<u64>,
    layout: LayoutNode,
    active: u64,
    last: Option<u64>,
    zoomed: bool,
    preset: Option<usize>,
}

/// The same window with a possibly different set of panes. The user's layout
/// stays when the panes did not change; panes that left are cut out of it,
/// new split panes go beside the focused one, and new agent panes make the
/// window a `main-vertical` again.
fn reuse_window(
    id: u64,
    shape: WindowShape,
    panes: Vec<Pane>,
    ids: &[u64],
    split_axis: &mut HashMap<TermId, Axis>,
    pane_ids: &HashMap<TermId, u64>,
) -> Window {
    let mut layout = shape.layout;
    let mut active = shape.active;
    let mut relayout = false;
    for gone in shape.ids.iter().filter(|i| !ids.contains(i)) {
        layout.remove(*gone);
    }
    if !ids.contains(&active) {
        active = ids[0];
    }
    for new in ids.iter().filter(|i| !shape.ids.contains(i)) {
        let axis = pane_ids
            .iter()
            .find(|(_, p)| *p == new)
            .and_then(|(t, _)| split_axis.remove(t));
        match axis {
            Some(axis) if layout.split(active, *new, axis) => active = *new,
            _ => relayout = true,
        }
    }
    let mut preset = shape.preset;
    if relayout {
        let p = preset.unwrap_or(2);
        layout = LayoutNode::preset(PRESETS[p], ids).unwrap_or(LayoutNode::Leaf(ids[0]));
        preset = preset.map(|_| p);
    }
    // Keep the pane vector in layout order.
    let order = layout.leaves();
    let mut panes = panes;
    panes.sort_by_key(|p| order.iter().position(|i| *i == p.id).unwrap_or(usize::MAX));
    let last = shape.last.filter(|l| ids.contains(l));
    Window {
        id,
        panes,
        layout,
        active,
        last,
        zoomed: shape.zoomed && ids.len() > 1,
        custom_name: None,
        preset,
    }
}

/// The view-state key of a pane: its agent id, else `term N`.
fn pane_key(p: &Pane) -> Option<String> {
    p.agent
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| p.remote_term().map(|t| format!("term {}", t.term)))
}

/// How an agent ended, as the glyph and words of its pane header (UI design
/// 3.3): `✓ docs · reported success`. It reads the report the server keeps,
/// so the client does not guess. `None` while it runs without a report.
pub fn outcome(a: &cmux_proto::view::AgentView) -> Option<(&'static str, &'static str)> {
    use cmux_proto::view::{Life, ReportStatus};
    if !crate::tui::marks::agent_done(a) {
        return None;
    }
    match (a.report.as_ref().filter(|r| !r.implicit), &a.life) {
        (Some(r), _) => Some(match r.status {
            ReportStatus::Success => ("✓", "reported success"),
            ReportStatus::Blocked => ("!", "blocked"),
            ReportStatus::Failed => ("✗", "failed"),
        }),
        (None, Life::Exited { .. }) => Some(("○", "exited")),
        (None, Life::Stopped) => Some(("○", "stopped")),
        (None, Life::Starting | Life::Running) => None,
    }
}

/// Copies what the state knows about a terminal onto its pane.
fn describe_pane(pane: &mut Pane, spec: &PaneSpec, sv: &cmux_proto::view::StateView) {
    let term = spec.term.and_then(|t| sv.terminal(t));
    pane.agent = spec.agent.clone();
    pane.dim = false;
    pane.label = spec.agent.as_ref().map(agent_label);
    pane.finished = spec.finished;
    if let Some(a) = spec.agent.as_ref().and_then(|a| sv.agent(a)) {
        pane.kind = Kind::Claude;
        pane.cwd = a.cwd.clone();
        pane.dim = spec.level == Some(Level::Sub) && spec.finished;
    } else if let Some(t) = term {
        pane.kind = pane_kind(&t.kind);
        pane.cwd = t.cwd.clone();
    }
    if let Some(t) = term {
        pane.title = t.title.clone().filter(|s| !s.trim().is_empty());
    }
}

/// Only https URLs are handed to the opener: the server is not trusted to pick
/// schemes (`file://`, `smb://`, custom handlers) or options for it.
fn web_url(url: &str) -> bool {
    url.starts_with("https://") && !url.contains(char::is_control)
}

/// Opens a URL in the local browser, ignoring failures.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Rows of the key help that only exist on a server.
pub const REMOTE_HELP: &[(&str, &str)] = &[
    ("d", "detach (agents keep running on the server)"),
    ("Tab", "focus the tree / back to the pane"),
    ("e  u", "go to the plan's executor / up one level"),
    ("a  M-a  M-A", "needs-you overlay / next / previous item"),
    ("m", "message the focused agent"),
    ("v", "board view of the plan"),
    ("P  B", "next plan / hide or show the tree"),
    ("=", "take over this terminal's size"),
];

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
