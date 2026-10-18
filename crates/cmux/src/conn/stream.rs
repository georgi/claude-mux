//! The `/v1/stream` WebSocket on its own thread (architecture 6.2 and 11.1).
//!
//! The thread connects, says `hello`, applies what the server sends and tells
//! the event loop through a `notify` callback. Terminal bytes never travel
//! through the event loop: the thread feeds each subscribed terminal's
//! `vt100::Parser` directly (like the local PTY reader thread does) and only
//! sends a coalesced [`StreamEvent::TermOutput`] to say "redraw".
//!
//! Layers, bottom up:
//! - [`Wire`]: one connected socket (tungstenite in production, a fake in tests).
//! - [`Session`]: what must survive a reconnect (last `rev`, wanted terminals
//!   and each one's next `seq`) and the pure rules that update it.
//! - [`run_session`]: one connection's life, generic over [`Wire`].
//! - [`Connection`]: the reconnect loop with backoff, plus the handle the UI
//!   uses to subscribe, send input and resize.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cmux_proto::api::{ErrorCode, Scope, ServerInfo, route};
use cmux_proto::stream::{ClientKind, ClientMsg, Frame, PresenceEntry, ServerMsg, TermPos};
use cmux_proto::view::{ClientId, DeltaView, StateView, TermId};
use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;

use super::servers::Endpoint;
use crate::pane::Callbacks;

/// Scrollback lines kept per remote terminal (same as a local pane).
pub const SCROLLBACK: usize = 10_000;
/// How long the socket blocks in a read before the thread checks its queue.
const POLL: Duration = Duration::from_millis(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(15);
const DEAD_AFTER: Duration = Duration::from_secs(45);
/// Failed attempts in a row before the dot shows `○` (retrying continues).
const OFFLINE_AFTER: u32 = 5;

// ---------------------------------------------------------------------------
// Events and status
// ---------------------------------------------------------------------------

/// The connection dot in the tab bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnStatus {
    /// `●`
    Connected,
    /// `◐`
    Reconnecting,
    /// `○`
    Offline,
}

impl ConnStatus {
    pub fn dot(self) -> char {
        match self {
            ConnStatus::Connected => '●',
            ConnStatus::Reconnecting => '◐',
            ConnStatus::Offline => '○',
        }
    }

    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> ConnStatus {
        match v {
            0 => ConnStatus::Connected,
            1 => ConnStatus::Reconnecting,
            _ => ConnStatus::Offline,
        }
    }
}

/// What the connection thread tells the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Status(ConnStatus),
    Welcome {
        client_id: ClientId,
        server: ServerInfo,
        scope: Scope,
    },
    /// A full replacement of the state (first connect, or the server's history
    /// no longer reaches our `rev`).
    State(Box<StateView>),
    Deltas {
        rev: u64,
        items: Vec<DeltaView>,
    },
    /// A subscribed terminal's parser changed (bytes or a snapshot). Coalesced:
    /// no second event comes until the UI clears the terminal's `dirty` flag.
    TermOutput(TermId),
    /// The terminal's screen was rebuilt from a server snapshot (first
    /// subscribe, or a resume the server's ring could not cover). Sent
    /// before the matching `TermOutput`.
    TermReset(TermId),
    /// The terminal's PTY size changed (the parser was resized already).
    Size {
        term: TermId,
        rows: u16,
        cols: u16,
        owner: Option<ClientId>,
    },
    Presence(Vec<PresenceEntry>),
    /// The program in a subscribed terminal ended (after its last output).
    Exit {
        term: TermId,
        code: u32,
        signal: Option<String>,
    },
    /// A protocol error from the server that did not end the connection.
    Error {
        code: ErrorCode,
        message: String,
    },
    /// The server refused our token. Reconnecting stops; run `cmux login`.
    AuthFailed(String),
}

/// The UI-side copy of the server state, kept up to date from events.
#[derive(Debug, Clone)]
pub struct Mirror {
    pub sv: StateView,
    pub status: ConnStatus,
    pub client_id: Option<ClientId>,
    pub server: Option<ServerInfo>,
    pub scope: Option<Scope>,
    pub presence: Vec<PresenceEntry>,
    /// Latest PTY size and size owner per terminal.
    pub sizes: HashMap<TermId, (u16, u16, Option<ClientId>)>,
}

impl Default for Mirror {
    fn default() -> Self {
        Mirror {
            sv: StateView::default(),
            status: ConnStatus::Reconnecting,
            client_id: None,
            server: None,
            scope: None,
            presence: Vec::new(),
            sizes: HashMap::new(),
        }
    }
}

impl Mirror {
    /// Applies one event. Returns whether the state, status or presence
    /// changed in a way the UI should redraw for (terminal output is the
    /// caller's business, via `dirty`).
    pub fn apply(&mut self, ev: &StreamEvent) -> bool {
        match ev {
            StreamEvent::Status(s) => self.status = *s,
            StreamEvent::AuthFailed(_) => self.status = ConnStatus::Offline,
            StreamEvent::Welcome {
                client_id,
                server,
                scope,
            } => {
                self.client_id = Some(client_id.clone());
                self.server = Some(server.clone());
                self.scope = Some(*scope);
            }
            StreamEvent::State(sv) => self.sv = (**sv).clone(),
            StreamEvent::Deltas { rev, items } => {
                for d in items {
                    self.sv.apply(d);
                }
                self.sv.rev = self.sv.rev.max(*rev);
            }
            StreamEvent::Size {
                term,
                rows,
                cols,
                owner,
            } => {
                self.sizes.insert(*term, (*rows, *cols, owner.clone()));
            }
            StreamEvent::Presence(p) => self.presence = p.clone(),
            StreamEvent::TermOutput(_)
            | StreamEvent::TermReset(_)
            | StreamEvent::Exit { .. }
            | StreamEvent::Error { .. } => return false,
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Terminals shared with the UI
// ---------------------------------------------------------------------------

/// The parts of a remote terminal that the connection thread writes and the
/// UI reads. `RemoteTerm` holds clones of these.
#[derive(Clone)]
pub struct TermShared {
    pub parser: Arc<Mutex<vt100::Parser<Callbacks>>>,
    pub dirty: Arc<AtomicBool>,
}

impl TermShared {
    pub fn new(rows: u16, cols: u16) -> TermShared {
        TermShared {
            parser: Arc::new(Mutex::new(new_parser(rows, cols))),
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn new_parser(rows: u16, cols: u16) -> vt100::Parser<Callbacks> {
    // A remote terminal does not answer queries (DSR, DA): the server side
    // does, once, so several attached clients do not each reply.
    let sink: crate::pane::Writer = Arc::new(Mutex::new(Box::new(std::io::sink())));
    vt100::Parser::new_with_callbacks(rows.max(1), cols.max(1), SCROLLBACK, Callbacks::new(sink))
}

pub type TermTable = Arc<Mutex<HashMap<TermId, TermShared>>>;

// ---------------------------------------------------------------------------
// Session: what survives a reconnect
// ---------------------------------------------------------------------------

/// Commands from the UI to the connection thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// Start streaming a terminal (its `TermShared` is in the table already).
    Want(TermId),
    Release(TermId),
    Focus(TermId),
    Resize {
        term: TermId,
        rows: u16,
        cols: u16,
    },
    /// Keyboard bytes. Dropped while disconnected, never queued.
    Input {
        term: TermId,
        data: Vec<u8>,
    },
    /// Drop the socket and reconnect now (resuming from `rev` and `seq`).
    Reconnect,
    Shutdown,
}

/// A message to put on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    Msg(ClientMsg),
    Frame(Frame),
}

/// What to do with a terminal frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameAction {
    /// Feed `data` to the parser, after resetting it when `reset`.
    Feed {
        term: TermId,
        reset: bool,
        data: Vec<u8>,
    },
    /// We missed bytes: ask for a fresh snapshot.
    Resync(TermId),
    Ignore,
}

/// Reconnect bookkeeping. Pure: no I/O.
#[derive(Debug, Clone, Default)]
pub struct Session {
    /// Last `rev` applied; sent as `since_rev` on the next hello.
    pub rev: Option<u64>,
    /// Wanted terminals and the stream offset after the last byte applied
    /// (`None` until a first snapshot or output arrives).
    terms: BTreeMap<TermId, Option<u64>>,
    focus: Option<TermId>,
    sizes: BTreeMap<TermId, (u16, u16)>,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// The offset after the last byte applied to `term`.
    pub fn next_seq(&self, term: TermId) -> Option<u64> {
        self.terms.get(&term).copied().flatten()
    }

    pub fn wanted(&self) -> Vec<TermId> {
        self.terms.keys().copied().collect()
    }

    pub fn hello(&self, token: Option<String>, info: Option<String>) -> ClientMsg {
        ClientMsg::Hello {
            client: ClientKind::Tui,
            since_rev: self.rev,
            token,
            info,
            terms: self
                .terms
                .iter()
                .filter_map(|(id, next)| next.map(|since_seq| TermPos { id: *id, since_seq }))
                .collect(),
        }
    }

    /// What to send right after `welcome`: terminals without a position yet
    /// (the hello could not name them), then focus and size.
    pub fn after_welcome(&self) -> Vec<ClientMsg> {
        let mut out: Vec<ClientMsg> = self
            .terms
            .iter()
            .filter(|(_, next)| next.is_none())
            .map(|(id, _)| ClientMsg::Subscribe {
                term: *id,
                since_seq: None,
            })
            .collect();
        if let Some(t) = self.focus.filter(|t| self.terms.contains_key(t)) {
            out.push(ClientMsg::Focus { term: t });
            if let Some(&(rows, cols)) = self.sizes.get(&t) {
                out.push(ClientMsg::Resize {
                    term: t,
                    rows,
                    cols,
                });
            }
        }
        out
    }

    /// Records a UI command; returns what to send if the socket is up.
    pub fn on_cmd(&mut self, cmd: &Cmd) -> Option<Outgoing> {
        match cmd {
            Cmd::Want(t) => {
                if self.terms.contains_key(t) {
                    return None;
                }
                self.terms.insert(*t, None);
                Some(Outgoing::Msg(ClientMsg::Subscribe {
                    term: *t,
                    since_seq: None,
                }))
            }
            Cmd::Release(t) => {
                self.sizes.remove(t);
                if self.focus == Some(*t) {
                    self.focus = None;
                }
                self.terms
                    .remove(t)
                    .map(|_| Outgoing::Msg(ClientMsg::Unsubscribe { term: *t }))
            }
            Cmd::Focus(t) => {
                self.focus = Some(*t);
                Some(Outgoing::Msg(ClientMsg::Focus { term: *t }))
            }
            Cmd::Resize { term, rows, cols } => {
                self.sizes.insert(*term, (*rows, *cols));
                Some(Outgoing::Msg(ClientMsg::Resize {
                    term: *term,
                    rows: *rows,
                    cols: *cols,
                }))
            }
            Cmd::Input { term, data } => Some(Outgoing::Frame(Frame::Input {
                term: *term,
                data: data.clone(),
            })),
            Cmd::Reconnect | Cmd::Shutdown => None,
        }
    }

    /// Updates `rev` and turns a server message into UI events.
    pub fn on_msg(&mut self, msg: ServerMsg) -> Vec<StreamEvent> {
        match msg {
            ServerMsg::Welcome {
                server,
                client_id,
                scope,
                ..
            } => vec![StreamEvent::Welcome {
                client_id,
                server,
                scope,
            }],
            ServerMsg::State { rev, snapshot } => {
                self.rev = Some(rev);
                vec![StreamEvent::State(Box::new(snapshot))]
            }
            ServerMsg::Deltas { rev, items } => {
                self.rev = Some(self.rev.map_or(rev, |r| r.max(rev)));
                vec![StreamEvent::Deltas { rev, items }]
            }
            ServerMsg::Size {
                term,
                rows,
                cols,
                owner,
            } => vec![StreamEvent::Size {
                term,
                rows,
                cols,
                owner,
            }],
            ServerMsg::Presence { clients } => vec![StreamEvent::Presence(clients)],
            ServerMsg::Exit { term, code, signal } => {
                vec![StreamEvent::Exit { term, code, signal }]
            }
            ServerMsg::Error { code, message } => {
                if code == ErrorCode::Unauthorized {
                    vec![StreamEvent::AuthFailed(message)]
                } else {
                    vec![StreamEvent::Error { code, message }]
                }
            }
            ServerMsg::Pong { .. } => Vec::new(),
        }
    }

    /// Updates the term's position and says what to do with its bytes.
    pub fn on_frame(&mut self, frame: Frame) -> FrameAction {
        match frame {
            Frame::Input { .. } => FrameAction::Ignore,
            Frame::Snapshot { term, seq, data } => match self.terms.get_mut(&term) {
                Some(next) => {
                    *next = Some(seq);
                    FrameAction::Feed {
                        term,
                        reset: true,
                        data,
                    }
                }
                None => FrameAction::Ignore,
            },
            Frame::Output { term, seq, data } => {
                let Some(next) = self.terms.get_mut(&term) else {
                    return FrameAction::Ignore;
                };
                let expected = next.unwrap_or(0);
                let end = seq + data.len() as u64;
                if seq > expected {
                    // Bytes are missing; only a snapshot can repair the screen.
                    *next = None;
                    return FrameAction::Resync(term);
                }
                if end <= expected {
                    return FrameAction::Ignore;
                }
                let skip = (expected - seq) as usize;
                *next = Some(end);
                FrameAction::Feed {
                    term,
                    reset: false,
                    data: data[skip..].to_vec(),
                }
            }
        }
    }
}

/// Exponential backoff: 250 ms, doubling, capped at 10 s.
#[derive(Debug, Clone, Default)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// The delay before the next attempt; counts a failure.
    pub fn next(&mut self) -> Duration {
        let ms = 250u64.saturating_mul(1 << self.failures.min(6));
        self.failures += 1;
        Duration::from_millis(ms.min(10_000))
    }

    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

// ---------------------------------------------------------------------------
// One connection
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum WireError {
    Closed,
    Io(String),
}

pub enum Incoming {
    Text(String),
    Binary(Vec<u8>),
}

/// One connected WebSocket.
pub trait Wire {
    fn send(&mut self, out: &Outgoing) -> Result<(), WireError>;
    /// The next message, or `None` if nothing arrived within the poll interval.
    fn recv(&mut self) -> Result<Option<Incoming>, WireError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// The socket broke or the server went silent; reconnect.
    Lost(String),
    Shutdown,
    /// The token was refused; reconnecting cannot help.
    AuthFailed(String),
}

/// Who we are to the server.
#[derive(Debug, Clone)]
pub struct Identity {
    /// Sent in the hello; `None` on the Unix socket.
    pub token: Option<String>,
    pub info: String,
}

/// Runs one connection until it ends. `welcomed` tells the caller whether the
/// server accepted the hello (to reset the backoff).
pub fn run_session<W: Wire>(
    wire: &mut W,
    session: &mut Session,
    id: &Identity,
    cmds: &Receiver<Cmd>,
    terms: &TermTable,
    notify: &dyn Fn(StreamEvent),
    welcomed: &mut bool,
) -> End {
    *welcomed = false;
    let hello = Outgoing::Msg(session.hello(id.token.clone(), Some(id.info.clone())));
    if let Err(e) = wire.send(&hello) {
        return End::Lost(wire_err(e));
    }
    let mut last_rx = Instant::now();
    let mut last_ping = Instant::now();
    let mut nonce = 0u64;
    loop {
        // UI commands first: input latency matters most.
        while let Ok(cmd) = cmds.try_recv() {
            if cmd == Cmd::Shutdown {
                return End::Shutdown;
            }
            if cmd == Cmd::Reconnect {
                return End::Lost("reconnect requested".into());
            }
            let out = session.on_cmd(&cmd);
            if *welcomed
                && let Some(out) = out
                && let Err(e) = wire.send(&out)
            {
                return End::Lost(wire_err(e));
            }
        }
        match wire.recv() {
            Ok(None) => {}
            Ok(Some(msg)) => {
                last_rx = Instant::now();
                match msg {
                    Incoming::Text(t) => {
                        let Ok(m) = serde_json::from_str::<ServerMsg>(&t) else {
                            continue; // unknown message from a newer server
                        };
                        let is_welcome = matches!(m, ServerMsg::Welcome { .. });
                        for ev in session.on_msg(m) {
                            if let StreamEvent::AuthFailed(msg) = ev {
                                return End::AuthFailed(msg);
                            }
                            if let StreamEvent::Size {
                                term, rows, cols, ..
                            } = &ev
                            {
                                resize_local(terms, *term, *rows, *cols, notify);
                            }
                            notify(ev);
                        }
                        if is_welcome {
                            *welcomed = true;
                            notify(StreamEvent::Status(ConnStatus::Connected));
                            for m in session.after_welcome() {
                                if let Err(e) = wire.send(&Outgoing::Msg(m)) {
                                    return End::Lost(wire_err(e));
                                }
                            }
                        }
                    }
                    Incoming::Binary(b) => {
                        let Ok(frame) = Frame::decode(&b) else {
                            continue;
                        };
                        match session.on_frame(frame) {
                            FrameAction::Feed { term, reset, data } => {
                                feed(terms, term, reset, &data, notify)
                            }
                            FrameAction::Resync(term) => {
                                let m = Outgoing::Msg(ClientMsg::Subscribe {
                                    term,
                                    since_seq: None,
                                });
                                if let Err(e) = wire.send(&m) {
                                    return End::Lost(wire_err(e));
                                }
                            }
                            FrameAction::Ignore => {}
                        }
                    }
                }
            }
            Err(e) => return End::Lost(wire_err(e)),
        }
        if last_rx.elapsed() > DEAD_AFTER {
            return End::Lost("no data from the server".into());
        }
        if *welcomed && last_ping.elapsed() > PING_EVERY {
            last_ping = Instant::now();
            nonce += 1;
            if let Err(e) = wire.send(&Outgoing::Msg(ClientMsg::Ping { nonce })) {
                return End::Lost(wire_err(e));
            }
        }
    }
}

fn wire_err(e: WireError) -> String {
    match e {
        WireError::Closed => "connection closed".into(),
        WireError::Io(s) => s,
    }
}

/// Feeds bytes (or a snapshot) to a terminal's parser and flags it dirty.
pub fn feed(
    terms: &TermTable,
    term: TermId,
    reset: bool,
    data: &[u8],
    notify: &dyn Fn(StreamEvent),
) {
    let Some(shared) = terms.lock().unwrap().get(&term).cloned() else {
        return;
    };
    {
        let mut p = shared.parser.lock().unwrap();
        if reset {
            let (rows, cols) = p.screen().size();
            *p = new_parser(rows, cols);
        }
        p.process(data);
    }
    if reset {
        notify(StreamEvent::TermReset(term));
    }
    if !shared.dirty.swap(true, Ordering::AcqRel) {
        notify(StreamEvent::TermOutput(term));
    }
}

fn resize_local(
    terms: &TermTable,
    term: TermId,
    rows: u16,
    cols: u16,
    notify: &dyn Fn(StreamEvent),
) {
    let Some(shared) = terms.lock().unwrap().get(&term).cloned() else {
        return;
    };
    shared
        .parser
        .lock()
        .unwrap()
        .screen_mut()
        .set_size(rows.max(1), cols.max(1));
    if !shared.dirty.swap(true, Ordering::AcqRel) {
        notify(StreamEvent::TermOutput(term));
    }
}

// ---------------------------------------------------------------------------
// tungstenite transport
// ---------------------------------------------------------------------------

pub enum WsWire {
    Unix(Box<tungstenite::WebSocket<UnixStream>>),
    Tcp(Box<tungstenite::WebSocket<MaybeTlsStream<TcpStream>>>),
}

#[derive(Debug)]
pub enum ConnectError {
    /// The server refused the credentials (HTTP 401 or 403 on the upgrade).
    Auth(String),
    Other(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Auth(s) | ConnectError::Other(s) => f.write_str(s),
        }
    }
}

/// `https://host:port/base` to `wss://host:port/base/v1/stream`.
pub fn ws_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{base}")
    };
    format!("{ws}{}", route::STREAM)
}

fn handshake_err<S>(e: tungstenite::HandshakeError<tungstenite::ClientHandshake<S>>) -> ConnectError
where
    S: Read + Write,
{
    match e {
        tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp))
            if matches!(resp.status().as_u16(), 401 | 403) =>
        {
            ConnectError::Auth(format!("server answered HTTP {}", resp.status()))
        }
        other => ConnectError::Other(other.to_string()),
    }
}

fn tcp_of(s: &MaybeTlsStream<TcpStream>) -> Option<&TcpStream> {
    match s {
        MaybeTlsStream::Plain(t) => Some(t),
        MaybeTlsStream::Rustls(t) => Some(t.get_ref()),
        _ => None,
    }
}

impl WsWire {
    pub fn connect(endpoint: &Endpoint) -> Result<WsWire, ConnectError> {
        let other = |e: &dyn std::fmt::Display| ConnectError::Other(e.to_string());
        match endpoint {
            Endpoint::Unix(path) => {
                let s = UnixStream::connect(path)
                    .map_err(|e| ConnectError::Other(format!("{}: {e}", path.display())))?;
                s.set_read_timeout(Some(CONNECT_TIMEOUT))
                    .map_err(|e| other(&e))?;
                s.set_write_timeout(Some(CONNECT_TIMEOUT))
                    .map_err(|e| other(&e))?;
                let url = format!("ws://localhost{}", route::STREAM);
                let (ws, _) = tungstenite::client(url.as_str(), s).map_err(handshake_err)?;
                ws.get_ref()
                    .set_read_timeout(Some(POLL))
                    .map_err(|e| other(&e))?;
                Ok(WsWire::Unix(Box::new(ws)))
            }
            Endpoint::Remote { url, .. } => {
                let ws_url = ws_url(url);
                let uri: tungstenite::http::Uri = ws_url.parse().map_err(|e| other(&e))?;
                let host = uri.host().ok_or_else(|| other(&"url has no host"))?;
                let tls = uri.scheme_str() == Some("wss");
                let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
                let tcp = connect_tcp(host, port).map_err(|e| other(&e))?;
                let _ = tcp.set_nodelay(true);
                tcp.set_read_timeout(Some(CONNECT_TIMEOUT))
                    .map_err(|e| other(&e))?;
                tcp.set_write_timeout(Some(CONNECT_TIMEOUT))
                    .map_err(|e| other(&e))?;
                let (ws, _) =
                    tungstenite::client_tls(ws_url.as_str(), tcp).map_err(handshake_err)?;
                if let Some(tcp) = tcp_of(ws.get_ref()) {
                    tcp.set_read_timeout(Some(POLL)).map_err(|e| other(&e))?;
                }
                Ok(WsWire::Tcp(Box::new(ws)))
            }
        }
    }
}

fn connect_tcp(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let mut last = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no address")))
}

fn ws_send<S: Read + Write>(
    ws: &mut tungstenite::WebSocket<S>,
    out: &Outgoing,
) -> Result<(), WireError> {
    let msg = match out {
        Outgoing::Msg(m) => Message::text(serde_json::to_string(m).expect("serializable")),
        Outgoing::Frame(f) => Message::binary(f.encode()),
    };
    ws.send(msg).map_err(map_ws_err)
}

fn ws_recv<S: Read + Write>(
    ws: &mut tungstenite::WebSocket<S>,
) -> Result<Option<Incoming>, WireError> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => return Ok(Some(Incoming::Text(t.as_str().to_string()))),
            Ok(Message::Binary(b)) => return Ok(Some(Incoming::Binary(b.to_vec()))),
            Ok(Message::Close(_)) => return Err(WireError::Closed),
            // Ping/pong are answered by tungstenite on the next write.
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(map_ws_err(e)),
        }
    }
}

impl Wire for WsWire {
    fn send(&mut self, out: &Outgoing) -> Result<(), WireError> {
        match self {
            WsWire::Unix(ws) => ws_send(ws, out),
            WsWire::Tcp(ws) => ws_send(ws, out),
        }
    }

    fn recv(&mut self) -> Result<Option<Incoming>, WireError> {
        match self {
            WsWire::Unix(ws) => ws_recv(ws),
            WsWire::Tcp(ws) => ws_recv(ws),
        }
    }
}

fn map_ws_err(e: tungstenite::Error) -> WireError {
    match e {
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            WireError::Closed
        }
        e => WireError::Io(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Connection: reconnect loop and the UI's handle
// ---------------------------------------------------------------------------

pub struct StreamConfig {
    pub endpoint: Endpoint,
    /// Free-form client description for the hello, e.g. `cmux 0.1.0 (linux)`.
    pub info: String,
}

/// Cheap to clone; held by every `RemoteTerm`.
#[derive(Clone)]
pub struct ConnHandle {
    cmds: Sender<Cmd>,
    terms: TermTable,
    status: Arc<AtomicU8>,
}

impl ConnHandle {
    /// A handle with no thread behind it: commands queue up in the returned
    /// receiver. For tests and for drawing without a server.
    pub fn detached() -> (ConnHandle, Receiver<Cmd>) {
        let (tx, rx) = mpsc::channel();
        let h = ConnHandle {
            cmds: tx,
            terms: Arc::new(Mutex::new(HashMap::new())),
            status: Arc::new(AtomicU8::new(ConnStatus::Reconnecting.to_u8())),
        };
        (h, rx)
    }

    pub fn status(&self) -> ConnStatus {
        ConnStatus::from_u8(self.status.load(Ordering::Acquire))
    }

    pub fn terms(&self) -> &TermTable {
        &self.terms
    }

    /// Subscribes to a terminal and returns its shared parser. A second call
    /// for the same terminal returns the same parser.
    pub fn open_term(&self, term: TermId, rows: u16, cols: u16) -> TermShared {
        let shared = {
            let mut t = self.terms.lock().unwrap();
            if let Some(s) = t.get(&term) {
                return s.clone();
            }
            let s = TermShared::new(rows, cols);
            t.insert(term, s.clone());
            s
        };
        let _ = self.cmds.send(Cmd::Want(term));
        shared
    }

    /// Unsubscribes and forgets the terminal's parser.
    pub fn close_term(&self, term: TermId) {
        self.terms.lock().unwrap().remove(&term);
        let _ = self.cmds.send(Cmd::Release(term));
    }

    pub fn input(&self, term: TermId, data: &[u8]) {
        let _ = self.cmds.send(Cmd::Input {
            term,
            data: data.to_vec(),
        });
    }

    /// Drops the socket and reconnects at once (no backoff wait is skipped:
    /// the first retry delay is 250 ms).
    pub fn reconnect(&self) {
        let _ = self.cmds.send(Cmd::Reconnect);
    }

    pub fn focus(&self, term: TermId) {
        let _ = self.cmds.send(Cmd::Focus(term));
    }

    pub fn resize(&self, term: TermId, rows: u16, cols: u16) {
        let _ = self.cmds.send(Cmd::Resize { term, rows, cols });
    }
}

pub struct Connection {
    handle: ConnHandle,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Connection {
    /// Starts the thread. `notify` runs on that thread for every event; keep
    /// it cheap (typically `move |e| { let _ = tx.send(Event::Stream(e)); }`).
    pub fn spawn(cfg: StreamConfig, notify: impl Fn(StreamEvent) + Send + 'static) -> Connection {
        let (handle, rx) = ConnHandle::detached();
        let h = handle.clone();
        let join = std::thread::Builder::new()
            .name("cmux-stream".into())
            .spawn(move || connection_loop(cfg, rx, h, notify))
            .expect("spawn stream thread");
        Connection {
            handle,
            join: Some(join),
        }
    }

    pub fn handle(&self) -> ConnHandle {
        self.handle.clone()
    }

    pub fn status(&self) -> ConnStatus {
        self.handle.status()
    }

    /// Stops the thread and waits for it.
    pub fn shutdown(&mut self) {
        let _ = self.handle.cmds.send(Cmd::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn connection_loop(
    cfg: StreamConfig,
    cmds: Receiver<Cmd>,
    handle: ConnHandle,
    notify: impl Fn(StreamEvent),
) {
    let set_status = |s: ConnStatus| {
        handle.status.store(s.to_u8(), Ordering::Release);
        notify(StreamEvent::Status(s));
    };
    let id = Identity {
        token: match &cfg.endpoint {
            Endpoint::Remote { token, .. } if !token.is_empty() => Some(token.clone()),
            _ => None,
        },
        info: cfg.info.clone(),
    };
    let mut session = Session::new();
    let mut backoff = Backoff::default();
    set_status(ConnStatus::Reconnecting);
    loop {
        let mut welcomed = false;
        let end = match WsWire::connect(&cfg.endpoint) {
            Ok(mut wire) => {
                // The status flips to Connected inside run_session on welcome.
                let tracked = |ev: StreamEvent| {
                    if let StreamEvent::Status(s) = &ev {
                        handle.status.store(s.to_u8(), Ordering::Release);
                    }
                    notify(ev);
                };
                run_session(
                    &mut wire,
                    &mut session,
                    &id,
                    &cmds,
                    &handle.terms,
                    &tracked,
                    &mut welcomed,
                )
            }
            Err(ConnectError::Auth(m)) => End::AuthFailed(m),
            Err(ConnectError::Other(m)) => End::Lost(m),
        };
        if welcomed {
            backoff.reset();
        }
        match end {
            End::Shutdown => return,
            End::AuthFailed(m) => {
                handle
                    .status
                    .store(ConnStatus::Offline.to_u8(), Ordering::Release);
                notify(StreamEvent::AuthFailed(m));
                notify(StreamEvent::Status(ConnStatus::Offline));
                // Nothing more to do; wait for the UI to shut us down.
                while let Ok(c) = cmds.recv() {
                    if c == Cmd::Shutdown {
                        return;
                    }
                    session.on_cmd(&c);
                }
                return;
            }
            End::Lost(_) => {}
        }
        let delay = backoff.next();
        set_status(if backoff.failures() >= OFFLINE_AFTER {
            ConnStatus::Offline
        } else {
            ConnStatus::Reconnecting
        });
        if sleep_handling(&cmds, &mut session, delay) {
            return;
        }
    }
}

/// Waits `delay` while keeping the session's subscriptions current. Returns
/// true on shutdown.
fn sleep_handling(cmds: &Receiver<Cmd>, session: &mut Session, delay: Duration) -> bool {
    let until = Instant::now() + delay;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        match cmds.recv_timeout(left) {
            Ok(Cmd::Shutdown) => return true,
            Ok(c) => {
                session.on_cmd(&c);
            }
            Err(RecvTimeoutError::Timeout) => return false,
            Err(RecvTimeoutError::Disconnected) => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_proto::api::ServerInfo;
    use std::collections::VecDeque;

    fn t(n: u32) -> TermId {
        TermId(n)
    }

    #[derive(Default)]
    struct FakeWire {
        incoming: VecDeque<Result<Option<Incoming>, ()>>,
        sent: Vec<Outgoing>,
        calls: usize,
        /// Commands the UI "sends" at the n-th `recv`.
        inject: Vec<(usize, Cmd)>,
        tx: Option<Sender<Cmd>>,
    }

    impl FakeWire {
        fn text(&mut self, m: &ServerMsg) {
            self.incoming
                .push_back(Ok(Some(Incoming::Text(serde_json::to_string(m).unwrap()))));
        }
        fn frame(&mut self, f: Frame) {
            self.incoming
                .push_back(Ok(Some(Incoming::Binary(f.encode()))));
        }
    }

    impl Wire for FakeWire {
        fn send(&mut self, out: &Outgoing) -> Result<(), WireError> {
            self.sent.push(out.clone());
            Ok(())
        }
        fn recv(&mut self) -> Result<Option<Incoming>, WireError> {
            self.calls += 1;
            for (at, cmd) in &self.inject {
                if *at == self.calls {
                    self.tx.as_ref().unwrap().send(cmd.clone()).unwrap();
                }
            }
            match self.incoming.pop_front() {
                Some(Ok(x)) => Ok(x),
                _ => Err(WireError::Closed),
            }
        }
    }

    fn welcome() -> ServerMsg {
        ServerMsg::Welcome {
            rev: 5,
            server: ServerInfo {
                version: "1".into(),
                name: "fake".into(),
            },
            client_id: ClientId("c1".into()),
            scope: Scope::Full,
        }
    }

    fn id() -> Identity {
        Identity {
            token: Some("tok".into()),
            info: "test".into(),
        }
    }

    fn out(term: u32, seq: u64, data: &[u8]) -> Frame {
        Frame::Output {
            term: t(term),
            seq,
            data: data.to_vec(),
        }
    }

    #[test]
    fn backoff_doubles_to_ten_seconds_and_resets() {
        let mut b = Backoff::default();
        let ms: Vec<u128> = (0..8).map(|_| b.next().as_millis()).collect();
        assert_eq!(ms, [250, 500, 1000, 2000, 4000, 8000, 10_000, 10_000]);
        b.reset();
        assert_eq!(b.next().as_millis(), 250);
    }

    #[test]
    fn frame_rules_track_seq() {
        let mut s = Session::new();
        s.on_cmd(&Cmd::Want(t(1)));
        // Unknown terminal: ignored.
        assert_eq!(s.on_frame(out(9, 0, b"x")), FrameAction::Ignore);
        // First output at 0 is fine; next_seq advances.
        assert!(matches!(
            s.on_frame(out(1, 0, b"abc")),
            FrameAction::Feed { reset: false, .. }
        ));
        assert_eq!(s.next_seq(t(1)), Some(3));
        // Overlapping retransmit: only the new tail is fed.
        match s.on_frame(out(1, 1, b"bcde")) {
            FrameAction::Feed { data, .. } => assert_eq!(data, b"de"),
            a => panic!("{a:?}"),
        }
        assert_eq!(s.next_seq(t(1)), Some(5));
        // Fully old: ignored. Gap: resync and forget the position.
        assert_eq!(s.on_frame(out(1, 0, b"ab")), FrameAction::Ignore);
        assert_eq!(s.on_frame(out(1, 99, b"zz")), FrameAction::Resync(t(1)));
        assert_eq!(s.next_seq(t(1)), None);
        // A snapshot resets and sets the position.
        let snap = Frame::Snapshot {
            term: t(1),
            seq: 500,
            data: b"s".to_vec(),
        };
        assert!(matches!(
            s.on_frame(snap),
            FrameAction::Feed { reset: true, .. }
        ));
        assert_eq!(s.next_seq(t(1)), Some(500));
    }

    #[test]
    fn reconnect_resumes_from_rev_and_per_term_seq() {
        let (handle, cmds) = ConnHandle::detached();
        let shared = handle.open_term(t(1), 24, 80);
        handle.open_term(t(2), 24, 80);
        handle.focus(t(1));
        handle.resize(t(1), 30, 100);
        let mut session = Session::new();
        let events = Mutex::new(Vec::new());
        let notify = |e: StreamEvent| events.lock().unwrap().push(e);

        // First connection: full state, bytes for term 1 only, then the link drops.
        let mut w = FakeWire::default();
        w.text(&welcome());
        w.text(&ServerMsg::State {
            rev: 7,
            snapshot: StateView {
                rev: 7,
                ..Default::default()
            },
        });
        w.frame(Frame::Snapshot {
            term: t(1),
            seq: 100,
            data: b"hello".to_vec(),
        });
        w.frame(out(1, 100, b" world"));
        w.text(&ServerMsg::Deltas {
            rev: 9,
            items: vec![],
        });
        let mut welcomed = false;
        let end = run_session(
            &mut w,
            &mut session,
            &id(),
            &cmds,
            handle.terms(),
            &notify,
            &mut welcomed,
        );
        assert_eq!(end, End::Lost("connection closed".into()));
        assert!(welcomed);
        // The hello was a fresh one; term 1 and 2 were wanted, no positions yet.
        match &w.sent[0] {
            Outgoing::Msg(ClientMsg::Hello {
                since_rev,
                terms,
                token,
                ..
            }) => {
                assert_eq!(*since_rev, None);
                assert!(terms.is_empty());
                assert_eq!(token.as_deref(), Some("tok"));
            }
            m => panic!("{m:?}"),
        }
        // After welcome: subscribe both (no position), focus, resize.
        assert_eq!(
            w.sent[1..],
            [
                Outgoing::Msg(ClientMsg::Subscribe {
                    term: t(1),
                    since_seq: None
                }),
                Outgoing::Msg(ClientMsg::Subscribe {
                    term: t(2),
                    since_seq: None
                }),
                Outgoing::Msg(ClientMsg::Focus { term: t(1) }),
                Outgoing::Msg(ClientMsg::Resize {
                    term: t(1),
                    rows: 30,
                    cols: 100
                }),
            ]
        );
        assert_eq!(session.rev, Some(9));
        assert_eq!(session.next_seq(t(1)), Some(106));
        assert_eq!(session.next_seq(t(2)), None);
        assert_eq!(
            shared.parser.lock().unwrap().screen().contents(),
            "hello world"
        );

        // Second connection resumes: rev 9, term 1 at 106, term 2 not named
        // (no bytes yet) but subscribed after welcome.
        let mut w2 = FakeWire::default();
        w2.text(&welcome());
        w2.frame(out(1, 106, b"!"));
        let end = run_session(
            &mut w2,
            &mut session,
            &id(),
            &cmds,
            handle.terms(),
            &notify,
            &mut welcomed,
        );
        assert!(matches!(end, End::Lost(_)));
        match &w2.sent[0] {
            Outgoing::Msg(ClientMsg::Hello {
                since_rev, terms, ..
            }) => {
                assert_eq!(*since_rev, Some(9));
                assert_eq!(
                    *terms,
                    vec![TermPos {
                        id: t(1),
                        since_seq: 106
                    }]
                );
            }
            m => panic!("{m:?}"),
        }
        assert_eq!(
            w2.sent[1],
            Outgoing::Msg(ClientMsg::Subscribe {
                term: t(2),
                since_seq: None
            })
        );
        assert_eq!(session.next_seq(t(1)), Some(107));
        assert_eq!(
            shared.parser.lock().unwrap().screen().contents(),
            "hello world!"
        );
        // One coalesced redraw event per dirty period, not per frame.
        let n = events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, StreamEvent::TermOutput(_)))
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn input_is_sent_only_when_welcomed_and_never_queued() {
        let (handle, cmds) = ConnHandle::detached();
        // Typed before the connection is up: dropped.
        handle.input(t(1), b"early");
        let mut session = Session::new();
        let mut w = FakeWire::default();
        w.text(&welcome());
        // Typed after welcome (during the 3rd recv): sent.
        w.incoming.push_back(Ok(None));
        w.incoming.push_back(Ok(None));
        w.inject.push((
            3,
            Cmd::Input {
                term: t(1),
                data: b"ls\r".to_vec(),
            },
        ));
        w.tx = Some(handle.cmds.clone());
        let mut welcomed = false;
        run_session(
            &mut w,
            &mut session,
            &id(),
            &cmds,
            handle.terms(),
            &|_| {},
            &mut welcomed,
        );
        let inputs: Vec<_> = w
            .sent
            .iter()
            .filter_map(|o| match o {
                Outgoing::Frame(Frame::Input { data, .. }) => Some(data.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(inputs, vec![b"ls\r".to_vec()]);
    }

    #[test]
    fn unauthorized_error_stops_reconnecting() {
        let (handle, cmds) = ConnHandle::detached();
        let mut session = Session::new();
        let mut w = FakeWire::default();
        w.text(&ServerMsg::Error {
            code: ErrorCode::Unauthorized,
            message: "bad token".into(),
        });
        let mut welcomed = false;
        let end = run_session(
            &mut w,
            &mut session,
            &id(),
            &cmds,
            handle.terms(),
            &|_| {},
            &mut welcomed,
        );
        assert_eq!(end, End::AuthFailed("bad token".into()));
    }

    #[test]
    fn mirror_applies_state_then_deltas() {
        let mut m = Mirror::default();
        assert_eq!(m.status, ConnStatus::Reconnecting);
        let sv = StateView {
            rev: 3,
            ..Default::default()
        };
        assert!(m.apply(&StreamEvent::State(Box::new(sv))));
        assert!(m.apply(&StreamEvent::Deltas {
            rev: 4,
            items: vec![]
        }));
        assert_eq!(m.sv.rev, 4);
        assert!(!m.apply(&StreamEvent::TermOutput(t(1))));
        m.apply(&StreamEvent::Status(ConnStatus::Connected));
        assert_eq!(m.status.dot(), '●');
    }

    #[test]
    fn ws_urls() {
        assert_eq!(ws_url("https://a.b:8443/"), "wss://a.b:8443/v1/stream");
        assert_eq!(ws_url("http://127.0.0.1:9"), "ws://127.0.0.1:9/v1/stream");
    }
}
