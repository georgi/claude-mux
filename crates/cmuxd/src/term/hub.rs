//! The terminal hub: per terminal a vt100 screen, a ring of recent raw output
//! indexed by a byte `seq`, subscribers with 16 ms output batching, and the
//! size owner (architecture section 5.2).
//!
//! A [`Terminal`] is created by [`Hub::create`] (or [`Terminal::spawn`]), which
//! starts the PTY reader thread and a flusher thread. Clients call:
//!
//! - [`Terminal::subscribe`] to get a [`Catchup`] (missing bytes or a snapshot)
//!   and a channel of live [`TermEvent`]s;
//! - [`Terminal::input`], [`Terminal::focus`] and [`Terminal::resize`], each
//!   naming the calling client, which makes that client the size owner;
//! - [`Terminal::scrollback`] for history beyond the screen.
//!
//! Sequence numbers count raw PTY output bytes since the terminal started.
//! `Output { seq, data }` carries the bytes `seq..seq + data.len()`, and a
//! catch-up ends at the `seq` the live stream resumes from, so a client keeps
//! `seq + data.len()` as its `since_seq` for the next reconnect.
//!
//! A subscriber whose queue overflows (a stalled client) is dropped: its
//! receiver yields `None`, and the client resubscribes with its last `since_seq`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use anyhow::{Result, bail};
use cmux_core::ids::TermId;
use tokio::sync::{mpsc, watch};

use super::pty::{ExitInfo, Pty, PtySpec};

/// Identifies a connected client (one per WebSocket connection).
pub type ClientId = String;

/// Scrollback lines for executors and tasks.
pub const SCROLLBACK_DEFAULT: usize = 10_000;
/// Scrollback lines for sub-agents and loose shells.
pub const SCROLLBACK_SMALL: usize = 2_000;
/// Raw output kept per terminal.
pub const RING_BYTES: usize = 1 << 20;
/// Output coalescing interval.
pub const BATCH: Duration = Duration::from_millis(16);
/// Events queued per subscriber before it is dropped as stalled.
const SUB_QUEUE: usize = 1024;

#[derive(Debug, Clone)]
pub struct TermConfig {
    /// vt100 scrollback lines.
    pub scrollback: usize,
    /// Capacity of the raw output ring in bytes.
    pub ring_bytes: usize,
    /// Output batching interval.
    pub batch: Duration,
}

impl Default for TermConfig {
    fn default() -> Self {
        TermConfig {
            scrollback: SCROLLBACK_DEFAULT,
            ring_bytes: RING_BYTES,
            batch: BATCH,
        }
    }
}

impl TermConfig {
    /// For sub-agents and loose shells.
    pub fn small() -> Self {
        TermConfig {
            scrollback: SCROLLBACK_SMALL,
            ..TermConfig::default()
        }
    }
}

/// The terminal size and who decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeEvent {
    pub rows: u16,
    pub cols: u16,
    pub owner: Option<ClientId>,
}

/// Something a subscriber receives after its catch-up.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// Coalesced raw output: the bytes `seq..seq + data.len()`.
    Output { seq: u64, data: Arc<[u8]> },
    /// The size or the size owner changed. Queued after any earlier output.
    Size(SizeEvent),
    /// The child exited. No further events follow.
    Exit(ExitInfo),
}

/// What a new subscriber needs before the live stream.
#[derive(Debug, Clone)]
pub enum Catchup {
    /// The bytes after `since_seq` (possibly empty); `seq` is where live output resumes.
    Bytes { data: Vec<u8>, seq: u64 },
    /// `since_seq` fell out of the ring (or was absent): a redraw of the
    /// screen, cursor and modes, prefixed with `ESC[?1049h` on the alternate
    /// screen (spike S7). The client should clear its terminal first. `seq` is
    /// where live output resumes.
    Snapshot { data: Vec<u8>, seq: u64 },
}

impl Catchup {
    pub fn seq(&self) -> u64 {
        match self {
            Catchup::Bytes { seq, .. } | Catchup::Snapshot { seq, .. } => *seq,
        }
    }
}

pub struct Subscription {
    pub catchup: Catchup,
    /// The size at subscription time.
    pub size: SizeEvent,
    pub rx: mpsc::Receiver<TermEvent>,
}

/// History lines from `Terminal::scrollback`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrollback {
    /// Plain text rows, oldest first.
    pub lines: Vec<String>,
    /// Total history lines available (0 on the alternate screen).
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Raw bytes ever produced; the next byte has this `seq`.
    pub seq: u64,
    pub ring_len: usize,
    pub ring_capacity: usize,
    /// History lines currently held by the parser.
    pub scrollback_len: usize,
    pub scrollback_capacity: usize,
    pub subscribers: usize,
}

/// vt100 callbacks: answers terminal queries and records title and bell.
struct Callbacks {
    replies: Vec<u8>,
    title: Option<String>,
    bell: bool,
}

impl vt100::Callbacks for Callbacks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // DSR: cursor position report.
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                self.replies.extend_from_slice(reply.as_bytes());
            }
            // DSR: device status OK.
            (None, 'n', 5) => self.replies.extend_from_slice(b"\x1b[0n"),
            // DA1: VT220 with ANSI color.
            (None, 'c', 0) => self.replies.extend_from_slice(b"\x1b[?62;22c"),
            // DA2.
            (Some(b'>'), 'c', 0) => self.replies.extend_from_slice(b"\x1b[>1;10;0c"),
            _ => {}
        }
    }
}

struct Sub {
    tx: mpsc::Sender<TermEvent>,
}

struct State {
    parser: vt100::Parser<Callbacks>,
    /// The last `ring_cap` bytes of output; the first has seq `seq - ring.len()`.
    ring: VecDeque<u8>,
    ring_cap: usize,
    scrollback_cap: usize,
    /// Total bytes produced.
    seq: u64,
    /// Output not yet sent to subscribers, starting at `seq - pending.len()`.
    pending: Vec<u8>,
    subs: Vec<Sub>,
    rows: u16,
    cols: u16,
    owner: Option<ClientId>,
    /// Last size each client asked for, applied when it becomes the owner.
    client_sizes: HashMap<ClientId, (u16, u16)>,
    exit: Option<ExitInfo>,
    /// The reader has finished; the flusher exits once `pending` is empty.
    closed: bool,
}

impl State {
    fn size_event(&self) -> SizeEvent {
        SizeEvent {
            rows: self.rows,
            cols: self.cols,
            owner: self.owner.clone(),
        }
    }

    fn broadcast(&mut self, ev: &TermEvent) {
        // try_send fails when the queue is full (stalled client) or closed.
        self.subs.retain(|s| s.tx.try_send(ev.clone()).is_ok());
    }

    /// Send the pending output to every subscriber as one chunk.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let seq = self.seq - self.pending.len() as u64;
        let data: Arc<[u8]> = Arc::from(std::mem::take(&mut self.pending));
        self.broadcast(&TermEvent::Output { seq, data });
    }

    /// The bytes after `since`, if they are all still in the ring.
    fn ring_since(&self, since: u64) -> Option<Vec<u8>> {
        let start = self.seq - self.ring.len() as u64;
        if since < start || since > self.seq {
            return None;
        }
        Some(
            self.ring
                .iter()
                .skip((since - start) as usize)
                .copied()
                .collect(),
        )
    }

    fn snapshot(&self) -> Vec<u8> {
        let screen = self.parser.screen();
        let mut data = Vec::new();
        if screen.alternate_screen() {
            data.extend_from_slice(b"\x1b[?1049h");
        }
        data.extend_from_slice(&screen.state_formatted());
        data
    }
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when output is pending or the reader closes.
    wake: Condvar,
    /// Set right after spawn; replies to terminal queries wait for it.
    pty: OnceLock<Pty>,
    exit_tx: watch::Sender<Option<ExitInfo>>,
    batch: Duration,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Output from the PTY reader thread.
    fn on_output(&self, bytes: &[u8]) {
        {
            let mut st = self.lock();
            st.parser.process(bytes);
            st.ring.extend(bytes);
            let excess = st.ring.len().saturating_sub(st.ring_cap);
            st.ring.drain(..excess);
            st.pending.extend_from_slice(bytes);
            st.seq += bytes.len() as u64;
            self.wake.notify_all();
        }
        self.send_replies();
    }

    /// Write terminal-query answers collected by the parser to the child.
    fn send_replies(&self) {
        let Some(pty) = self.pty.get() else { return };
        let replies = std::mem::take(&mut self.lock().parser.callbacks_mut().replies);
        if !replies.is_empty() {
            pty.write(&replies);
        }
    }

    fn on_exit(&self, info: ExitInfo) {
        {
            let mut st = self.lock();
            st.flush();
            st.exit = Some(info.clone());
            st.broadcast(&TermEvent::Exit(info.clone()));
            st.closed = true;
            self.wake.notify_all();
        }
        self.exit_tx.send_replace(Some(info));
    }

    /// Flusher thread: one coalesced chunk per batch interval.
    fn flush_loop(&self) {
        loop {
            let mut st = self.lock();
            while st.pending.is_empty() && !st.closed {
                st = self.wake.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.pending.is_empty() {
                return;
            }
            drop(st);
            std::thread::sleep(self.batch);
            self.lock().flush();
        }
    }

    /// Make `client` the size owner, adopting its last known size. Returns
    /// whether the owner or size changed. Caller holds the state lock.
    fn set_owner(&self, st: &mut State, client: &str) -> bool {
        let known = st.client_sizes.get(client).copied();
        self.apply(st, client, known)
    }

    fn apply(&self, st: &mut State, client: &str, size: Option<(u16, u16)>) -> bool {
        let mut changed = st.owner.as_deref() != Some(client);
        if changed {
            st.owner = Some(client.to_string());
        }
        if let Some((rows, cols)) = size
            && (rows, cols) != (st.rows, st.cols)
        {
            st.rows = rows;
            st.cols = cols;
            st.parser.screen_mut().set_size(rows, cols);
            if let Some(pty) = self.pty.get() {
                pty.resize(rows, cols);
            }
            changed = true;
        }
        if changed {
            // Output produced at the old size goes out before the change.
            st.flush();
            let ev = TermEvent::Size(st.size_event());
            st.broadcast(&ev);
        }
        changed
    }
}

/// One server-side terminal. Cheap to share behind an `Arc`.
pub struct Terminal {
    id: String,
    shared: Arc<Shared>,
}

impl Terminal {
    /// Start `spec` on a new PTY and its reader and flusher threads.
    pub fn spawn(id: impl Into<String>, spec: &PtySpec, cfg: &TermConfig) -> Result<Arc<Terminal>> {
        let id = id.into();
        let (rows, cols) = (spec.rows.max(1), spec.cols.max(1));
        let callbacks = Callbacks {
            replies: Vec::new(),
            title: None,
            bell: false,
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                parser: vt100::Parser::new_with_callbacks(rows, cols, cfg.scrollback, callbacks),
                ring: VecDeque::new(),
                ring_cap: cfg.ring_bytes,
                scrollback_cap: cfg.scrollback,
                seq: 0,
                pending: Vec::new(),
                subs: Vec::new(),
                rows,
                cols,
                owner: None,
                client_sizes: HashMap::new(),
                exit: None,
                closed: false,
            }),
            wake: Condvar::new(),
            pty: OnceLock::new(),
            exit_tx: watch::channel(None).0,
            batch: cfg.batch,
        });

        let mut spec = spec.clone();
        (spec.rows, spec.cols) = (rows, cols);
        let (out, done) = (shared.clone(), shared.clone());
        let pty = Pty::spawn(&spec, move |b| out.on_output(b), move |e| done.on_exit(e))?;
        let _ = shared.pty.set(pty);
        // Queries the child sent before the PTY handle was stored.
        shared.send_replies();

        let flusher = shared.clone();
        std::thread::Builder::new()
            .name(format!("flush-{id}"))
            .spawn(move || flusher.flush_loop())?;
        Ok(Arc::new(Terminal { id, shared }))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn pid(&self) -> Option<u32> {
        self.shared.pty.get().and_then(Pty::pid)
    }

    /// Subscribe to the terminal. With `since_seq` still in the ring the
    /// catch-up is just the missing bytes; otherwise it is a snapshot.
    pub fn subscribe(&self, since_seq: Option<u64>) -> Subscription {
        let (tx, rx) = mpsc::channel(SUB_QUEUE);
        let mut st = self.shared.lock();
        // Existing subscribers get the pending output now, so that it is
        // neither missing from this catch-up nor delivered to it twice.
        st.flush();
        let seq = st.seq;
        let catchup = match since_seq.and_then(|s| st.ring_since(s)) {
            Some(data) => Catchup::Bytes { data, seq },
            None => Catchup::Snapshot {
                data: st.snapshot(),
                seq,
            },
        };
        if let Some(exit) = st.exit.clone() {
            let _ = tx.try_send(TermEvent::Exit(exit));
        } else {
            st.subs.push(Sub { tx });
        }
        Subscription {
            catchup,
            size: st.size_event(),
            rx,
        }
    }

    /// Input from `client`, who becomes the size owner.
    pub fn input(&self, client: &str, bytes: &[u8]) {
        self.focus(client);
        if let Some(pty) = self.shared.pty.get() {
            pty.write(bytes);
        }
    }

    /// `client` focused this terminal and becomes the size owner. Returns
    /// whether the owner or the size changed.
    pub fn focus(&self, client: &str) -> bool {
        let mut st = self.shared.lock();
        self.shared.set_owner(&mut st, client)
    }

    /// `client` reports its viewport size and becomes the size owner. The size
    /// is remembered and re-applied whenever the client regains ownership.
    pub fn resize(&self, client: &str, rows: u16, cols: u16) -> bool {
        let size = (rows.max(1), cols.max(1));
        let mut st = self.shared.lock();
        st.client_sizes.insert(client.to_string(), size);
        self.shared.apply(&mut st, client, Some(size))
    }

    /// Forget a disconnected client's size. The owner stays until another
    /// client sends input, focus or resize, so the size does not jump.
    pub fn forget_client(&self, client: &str) {
        self.shared.lock().client_sizes.remove(client);
    }

    pub fn size(&self) -> SizeEvent {
        self.shared.lock().size_event()
    }

    /// `lines` history lines ending `offset` lines above the live screen
    /// (offset 0 is the newest history line), oldest first. Clamped to the
    /// available history.
    pub fn scrollback(&self, lines: usize, offset: usize) -> Scrollback {
        let mut st = self.shared.lock();
        let screen = st.parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        let total = screen.scrollback();
        let (_, cols) = screen.size();
        let rows = screen.size().0 as usize;
        let lo = offset.saturating_add(1);
        let mut hi = offset.saturating_add(lines).min(total);
        let mut out = Vec::new();
        // History line j (1 = just above the screen) is row 0 when scrolled back j lines.
        while hi >= lo {
            screen.set_scrollback(hi);
            let take = rows.min(hi - lo + 1);
            out.extend(screen.rows(0, cols).take(take));
            hi -= take;
        }
        screen.set_scrollback(0);
        Scrollback { lines: out, total }
    }

    /// The visible screen as plain text.
    pub fn screen_text(&self) -> String {
        self.shared.lock().parser.screen().contents()
    }

    /// The window title the program set, if any.
    pub fn title(&self) -> Option<String> {
        self.shared.lock().parser.callbacks().title.clone()
    }

    /// Whether the program rang the bell since the last call.
    pub fn take_bell(&self) -> bool {
        std::mem::take(&mut self.shared.lock().parser.callbacks_mut().bell)
    }

    pub fn stats(&self) -> Stats {
        let mut st = self.shared.lock();
        let scrollback_capacity = st.scrollback_cap;
        let screen = st.parser.screen_mut();
        screen.set_scrollback(usize::MAX);
        let scrollback_len = screen.scrollback();
        screen.set_scrollback(0);
        Stats {
            seq: st.seq,
            ring_len: st.ring.len(),
            ring_capacity: st.ring_cap,
            scrollback_len,
            scrollback_capacity,
            subscribers: st.subs.len(),
        }
    }

    pub fn exit_status(&self) -> Option<ExitInfo> {
        self.shared.lock().exit.clone()
    }

    /// Observe the exit: the value turns `Some` once the child is reaped.
    pub fn exited(&self) -> watch::Receiver<Option<ExitInfo>> {
        self.shared.exit_tx.subscribe()
    }

    pub fn kill(&self) {
        if let Some(pty) = self.shared.pty.get() {
            pty.kill();
        }
    }
}

/// The hub id of an engine terminal. The engine's `TermId` (the `u32` in
/// stream frames and REST paths) is the source of truth; the hub names the
/// same terminal `t<N>`, so no mapping table exists to drift.
pub fn term_key(id: TermId) -> String {
    format!("t{}", id.0)
}

/// Terminals by id.
#[derive(Default)]
pub struct Hub {
    terms: Mutex<HashMap<String, Arc<Terminal>>>,
    next: AtomicU64,
}

impl Hub {
    pub fn new() -> Hub {
        Hub::default()
    }

    /// A fresh id of the form `t<N>` that is not in use.
    pub fn new_id(&self) -> String {
        loop {
            let id = format!("t{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
            if !self.lock().contains_key(&id) {
                return id;
            }
        }
    }

    /// Spawn a terminal under `id`. Fails if the id exists.
    pub fn create(&self, id: &str, spec: &PtySpec, cfg: &TermConfig) -> Result<Arc<Terminal>> {
        if self.lock().contains_key(id) {
            bail!("terminal {id} already exists");
        }
        let term = Terminal::spawn(id, spec, cfg)?;
        if self.lock().insert(id.to_string(), term.clone()).is_some() {
            term.kill();
            bail!("terminal {id} already exists");
        }
        Ok(term)
    }

    pub fn get(&self, id: &str) -> Option<Arc<Terminal>> {
        self.lock().get(id).cloned()
    }

    /// The terminal for an engine id (see [`term_key`]).
    pub fn get_term(&self, id: TermId) -> Option<Arc<Terminal>> {
        self.get(&term_key(id))
    }

    /// Remove a terminal and kill its child (a no-op for one that exited).
    /// Exited terminals stay registered, with their final screen, until removed.
    pub fn remove(&self, id: &str) -> Option<Arc<Terminal>> {
        let term = self.lock().remove(id)?;
        term.kill();
        Some(term)
    }

    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.lock().keys().cloned().collect();
        ids.sort();
        ids
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<Terminal>>> {
        self.terms.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests;
