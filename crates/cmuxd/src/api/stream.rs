//! The `/v1/stream` WebSocket (architecture 5.2, 6.2; wire types in
//! `cmux_proto::stream`).
//!
//! One connection is one client. Message order on connect:
//!
//! 1. the client sends `hello` (within 10 s; `token` unless the Unix socket,
//!    a `Authorization: Bearer` header or a session cookie already
//!    authenticated the upgrade);
//! 2. the server sends `welcome`, then `state` (full snapshot) or `deltas`
//!    (always sent, possibly empty), then `presence`;
//! 3. for each entry of `hello.terms`: a `size` message, then a binary
//!    `Output` frame with the missed bytes (omitted when there are none) or a
//!    `Snapshot` frame, as for `subscribe` (the size comes first so the client
//!    parser has the terminal's size before it replays a snapshot);
//! 4. live: `deltas`, `presence`, `size`, `exit` and binary `Output` frames,
//!    and `chat` for each `sub_chat` agent (one with the entries after
//!    `after` at once, then one per batch the tailer reads).
//!
//! Terminal ids on the wire are the engine's `TermId`s; the hub names the
//! same terminal `t<N>` ([`term_key`](crate::term::hub::term_key)).
//!
//! A subscriber that falls behind is dropped by the hub; this connection
//! resubscribes at the next byte it owes the client and the client sees either
//! the missing bytes or a `Snapshot` frame (reset the parser on those).
//!
//! WebSocket compression (permessage-deflate) is not offered: the tungstenite
//! version axum uses does not implement it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use cmux_core::engine::{ClientAction, Command, RunnerEvent};
use cmux_core::ids::{AgentId, ClientId, TermId};
use cmux_proto::api::{ErrorCode, Scope, route};
use cmux_proto::stream::{ClientKind, ClientMsg, Frame, PresenceEntry, ServerMsg, TermPos};
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::auth::{self, Principal, Via};
use super::{ApiError, AppState, Transport};
use crate::actor::{ActorHandle, Catchup as StateCatchup};
use crate::term::hub::{Catchup, TermEvent, Terminal};

/// Seconds a new connection has to send `hello`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest accepted message.
const MAX_MESSAGE: usize = 1 << 20;
/// Messages queued for the socket writer before producers wait.
const OUT_QUEUE: usize = 256;
/// A write that takes longer than this closes the connection.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Input chunks queued for the PTY writer thread.
const INPUT_QUEUE: usize = 256;
/// Deltas merged into one message.
const DELTA_BATCH: usize = 256;
/// Largest terminal dimension a client may ask for.
const MAX_DIM: u16 = 1000;
/// How often typing in an agent's terminal is reported to the engine.
const HUMAN_INPUT_EVERY: Duration = Duration::from_millis(250);

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

pub fn routes(transport: Transport) -> Router<AppState> {
    Router::new()
        .route(route::STREAM, get(upgrade))
        .layer(Extension(transport))
}

// ---------------------------------------------------------------------------
// Presence
// ---------------------------------------------------------------------------

/// The connected stream clients; every change is broadcast as `presence`.
pub struct Presence {
    clients: Mutex<BTreeMap<String, PresenceEntry>>,
    tx: watch::Sender<Vec<PresenceEntry>>,
}

impl Default for Presence {
    fn default() -> Self {
        Presence {
            clients: Mutex::new(BTreeMap::new()),
            tx: watch::channel(Vec::new()).0,
        }
    }
}

impl Presence {
    pub fn list(&self) -> Vec<PresenceEntry> {
        self.tx.borrow().clone()
    }

    fn subscribe(&self) -> watch::Receiver<Vec<PresenceEntry>> {
        self.tx.subscribe()
    }

    fn update(&self, f: impl FnOnce(&mut BTreeMap<String, PresenceEntry>)) {
        let mut c = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut c);
        self.tx.send_replace(c.values().cloned().collect());
    }
}

/// Counts the client and lists it as present while it lives.
struct Registered {
    app: AppState,
    id: String,
}

impl Registered {
    fn new(app: &AppState, entry: PresenceEntry) -> Registered {
        app.clients.fetch_add(1, Ordering::Relaxed);
        let id = entry.id.0.clone();
        app.presence.update(|c| {
            c.insert(id.clone(), entry);
        });
        Registered {
            app: app.clone(),
            id,
        }
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.app.clients.fetch_sub(1, Ordering::Relaxed);
        self.app.presence.update(|c| {
            c.remove(&self.id);
        });
    }
}

// ---------------------------------------------------------------------------
// Upgrade and handshake
// ---------------------------------------------------------------------------

async fn upgrade(
    State(app): State<AppState>,
    Extension(transport): Extension<Transport>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    // A bearer header or cookie authenticates the upgrade itself; otherwise
    // the token comes in `hello`.
    let pre = auth::authenticate(&app, transport, &headers).ok();
    if pre.as_ref().is_some_and(|p| p.via == Via::Cookie) && !same_origin(&headers) {
        return ApiError::new(
            ErrorCode::Forbidden,
            "error: cross-origin WebSocket with a session cookie",
        )
        .into_response();
    }
    ws.max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| connection(app, socket, pre))
}

/// Browsers send cookies on cross-site WebSocket upgrades; require the
/// `Origin` (when sent) to name the host we were asked on.
fn same_origin(h: &HeaderMap) -> bool {
    let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin_host = origin.split_once("://").map_or(origin, |(_, r)| r);
    host == Some(origin_host)
}

/// Outgoing messages go through one queue so their order is kept.
type Out = mpsc::Sender<Message>;

fn text(msg: &ServerMsg) -> Message {
    Message::Text(
        serde_json::to_string(msg)
            .expect("server messages serialize")
            .into(),
    )
}

async fn send_msg(out: &Out, msg: &ServerMsg) -> bool {
    out.send(text(msg)).await.is_ok()
}

async fn send_error(out: &Out, code: ErrorCode, message: impl Into<String>) -> bool {
    send_msg(
        out,
        &ServerMsg::Error {
            code,
            message: message.into(),
        },
    )
    .await
}

async fn send_frame(out: &Out, frame: Frame) -> bool {
    out.send(Message::Binary(frame.encode().into()))
        .await
        .is_ok()
}

async fn write_loop(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<Message>,
) {
    while let Some(m) = rx.recv().await {
        match tokio::time::timeout(WRITE_TIMEOUT, sink.send(m)).await {
            Ok(Ok(())) => {}
            _ => return,
        }
    }
    let _ = sink.close().await;
}

async fn connection(app: AppState, socket: WebSocket, pre: Option<Principal>) {
    let (sink, mut input) = socket.split();
    let (out, out_rx) = mpsc::channel(OUT_QUEUE);
    let writer = tokio::spawn(write_loop(sink, out_rx));
    run(&app, &mut input, &out, pre).await;
    let _ = out
        .send(Message::Close(Some(CloseFrame {
            code: 1000,
            reason: "".into(),
        })))
        .await;
    drop(out);
    if tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .is_err()
    {
        tracing::debug!(target: "stream", "writer did not finish");
    }
}

struct Hello {
    client: ClientKind,
    since_rev: Option<u64>,
    token: Option<String>,
    info: Option<String>,
    terms: Vec<TermPos>,
}

/// Waits for the first text message, which must be `hello`.
async fn read_hello(input: &mut SplitStream<WebSocket>, out: &Out) -> Option<Hello> {
    let first = tokio::time::timeout(HELLO_TIMEOUT, async {
        loop {
            match input.next().await? {
                Ok(Message::Text(t)) => return Some(t),
                Ok(Message::Ping(_) | Message::Pong(_)) => {}
                _ => return None,
            }
        }
    })
    .await;
    let Ok(Some(t)) = first else {
        send_error(
            out,
            ErrorCode::Invalid,
            "error: the first message must be hello",
        )
        .await;
        return None;
    };
    match serde_json::from_str::<ClientMsg>(&t) {
        Ok(ClientMsg::Hello {
            client,
            since_rev,
            token,
            info,
            terms,
        }) => Some(Hello {
            client,
            since_rev,
            token,
            info,
            terms,
        }),
        _ => {
            send_error(
                out,
                ErrorCode::Invalid,
                "error: the first message must be hello",
            )
            .await;
            None
        }
    }
}

fn authenticate_hello(app: &AppState, h: &Hello) -> Option<Principal> {
    let d = app.devices.authenticate(h.token.as_deref()?)?;
    Some(Principal {
        id: d.id,
        scope: d.scope,
        via: Via::Bearer,
    })
}

async fn run(
    app: &AppState,
    input: &mut SplitStream<WebSocket>,
    out: &Out,
    pre: Option<Principal>,
) {
    let Some(hello) = read_hello(input, out).await else {
        return;
    };
    let principal = match pre.or_else(|| authenticate_hello(app, &hello)) {
        Some(p) => p,
        None => {
            send_error(
                out,
                ErrorCode::Unauthorized,
                "error: a device token is required in hello",
            )
            .await;
            return;
        }
    };

    let kind = hello.client;
    let n = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    let client = ClientId(format!(
        "{}-{n}",
        match kind {
            ClientKind::Tui => "tui",
            ClientKind::Web => "web",
            ClientKind::Cli => "cli",
        }
    ));

    // Engine state first, so `welcome` can name the revision it starts from.
    let Ok(mut sub) = app.actor.subscribe_since(hello.since_rev).await else {
        return;
    };
    let (rev, first) = catchup_msg(sub.catchup, hello.since_rev);
    let mut last_rev = rev;

    let mut presence_rx = app.presence.subscribe();
    let mut revoked_rx = app.devices.watch_revocations();
    let device = principal.device_id().map(str::to_string);
    // A revocation between authenticating and subscribing was not signalled.
    if device
        .as_deref()
        .is_some_and(|id| app.devices.name_of(id).is_none())
    {
        return;
    }
    let _registered = Registered::new(
        app,
        PresenceEntry {
            id: client.clone(),
            kind,
            device: principal
                .device_id()
                .and_then(|id| app.devices.name_of(id))
                .or(hello.info.clone()),
        },
    );
    let present = presence_rx.borrow_and_update().clone();

    let welcome = ServerMsg::Welcome {
        rev,
        server: app.info.clone(),
        client_id: client.clone(),
        scope: principal.scope,
    };
    if !(send_msg(out, &welcome).await
        && send_msg(out, &first).await
        && send_msg(out, &ServerMsg::Presence { clients: present }).await)
    {
        return;
    }

    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel();
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE);
    let input_worker = tokio::task::spawn_blocking(move || input_loop(input_rx));
    let mut conn = Conn {
        app: app.clone(),
        client,
        scope: principal.scope,
        out: out.clone(),
        subs: HashMap::new(),
        next_gen: 0,
        touched: HashSet::new(),
        agents: HashMap::new(),
        chats: HashMap::new(),
        human_input: HashMap::new(),
        ctl: ctl_tx,
        input: input_tx,
    };

    for TermPos { id, since_seq } in hello.terms {
        if !conn.subscribe(id, Some(since_seq)).await {
            conn.cleanup(input_worker).await;
            return;
        }
    }

    loop {
        tokio::select! {
            msg = input.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    Message::Text(t) => {
                        if !conn.on_text(&t).await { break }
                    }
                    Message::Binary(b) => {
                        if !conn.on_binary(&b).await { break }
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            d = sub.rx.recv() => {
                use tokio::sync::broadcast::error::RecvError;
                let ok = match d {
                    Ok(first) => {
                        let mut items = vec![first];
                        while items.len() < DELTA_BATCH && let Ok(more) = sub.rx.try_recv() {
                            items.push(more);
                        }
                        last_rev = items.last().map_or(last_rev, |d| d.rev);
                        send_msg(out, &ServerMsg::Deltas { rev: last_rev, items }).await
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Too slow for the broadcast: catch up from what we sent.
                        let Ok(again) = app.actor.subscribe_since(Some(last_rev)).await else { break };
                        let (rev, msg) = catchup_msg(again.catchup, Some(last_rev));
                        last_rev = rev;
                        sub.rx = again.rx;
                        send_msg(out, &msg).await
                    }
                    Err(RecvError::Closed) => break,
                };
                if !ok { break }
            }
            Ok(()) = revoked_rx.changed() => {
                // Revoking a device cuts its open connections too.
                let gone = device.as_deref().is_some_and(|id| app.devices.name_of(id).is_none());
                if gone { break }
            }
            Ok(()) = presence_rx.changed() => {
                let clients = presence_rx.borrow_and_update().clone();
                if !send_msg(out, &ServerMsg::Presence { clients }).await { break }
            }
            Some(Ctl::Stalled { term, generation, next_seq }) = ctl_rx.recv() => {
                // Only if this is still the subscription that stalled.
                let current = conn.subs.get(&term).is_some_and(|(g, _)| *g == generation);
                if current && !conn.subscribe(term, Some(next_seq)).await {
                    break;
                }
            }
        }
    }
    conn.cleanup(input_worker).await;
}

fn catchup_msg(c: StateCatchup, since: Option<u64>) -> (u64, ServerMsg) {
    match c {
        StateCatchup::Snapshot(sv) => (
            sv.rev,
            ServerMsg::State {
                rev: sv.rev,
                snapshot: sv,
            },
        ),
        StateCatchup::Deltas(items) => {
            let rev = items.last().map(|d| d.rev).or(since).unwrap_or(0);
            (rev, ServerMsg::Deltas { rev, items })
        }
    }
}

// ---------------------------------------------------------------------------
// One connection
// ---------------------------------------------------------------------------

enum Ctl {
    /// The hub dropped our subscription (the queue overflowed).
    Stalled {
        term: TermId,
        generation: u64,
        next_seq: u64,
    },
}

struct InputJob {
    term: Arc<Terminal>,
    client: String,
    data: Vec<u8>,
    /// Where to report input the terminal could not take.
    out: Out,
}

/// Writes to PTYs in order, off the async threads (a full PTY buffer blocks).
fn input_loop(mut rx: mpsc::Receiver<InputJob>) {
    while let Some(job) = rx.blocking_recv() {
        if job.term.input(&job.client, &job.data) {
            continue;
        }
        // An exited terminal drops input silently, as it always has.
        // The child stopped reading: best effort, as a full outgoing queue
        // means the client is not reading either.
        if !job.term.has_exited() {
            let msg = ServerMsg::Error {
                code: ErrorCode::Limit,
                message: "error: the terminal is not accepting input".into(),
            };
            let _ = job.out.try_send(text(&msg));
        }
    }
}

struct Conn {
    app: AppState,
    client: ClientId,
    scope: Scope,
    out: Out,
    subs: HashMap<TermId, (u64, JoinHandle<()>)>,
    next_gen: u64,
    /// Terminals whose per-client state the hub holds for us.
    touched: HashSet<TermId>,
    /// The agent behind a terminal, if any (cached; a terminal's agent is fixed).
    agents: HashMap<TermId, Option<AgentId>>,
    /// One forwarding task per `sub_chat` agent.
    chats: HashMap<AgentId, JoinHandle<()>>,
    human_input: HashMap<TermId, Instant>,
    ctl: mpsc::UnboundedSender<Ctl>,
    input: mpsc::Sender<InputJob>,
}

impl Conn {
    async fn cleanup(self, input_worker: JoinHandle<()>) {
        for (_, (_, h)) in self.subs {
            h.abort();
        }
        for (_, h) in self.chats {
            h.abort();
        }
        for id in self.touched {
            if let Some(t) = self.app.hub.get_term(id) {
                t.forget_client(&self.client.0);
            }
        }
        drop(self.input);
        let _ = tokio::time::timeout(Duration::from_secs(1), input_worker).await;
    }

    async fn error(&self, code: ErrorCode, message: impl Into<String>) -> bool {
        send_error(&self.out, code, message).await
    }

    async fn on_text(&mut self, t: &str) -> bool {
        let msg = match serde_json::from_str::<ClientMsg>(t) {
            Ok(m) => m,
            Err(e) => return self.error(ErrorCode::Invalid, format!("error: {e}")).await,
        };
        match msg {
            ClientMsg::Hello { .. } => {
                self.error(ErrorCode::Invalid, "error: hello was already sent")
                    .await
            }
            ClientMsg::Subscribe { term, since_seq } => self.subscribe(term, since_seq).await,
            ClientMsg::Unsubscribe { term } => {
                if let Some((_, h)) = self.subs.remove(&term) {
                    h.abort();
                }
                true
            }
            ClientMsg::Focus { term } => {
                if !self.writable().await {
                    return true;
                }
                match self.term(term).await {
                    Some(t) => {
                        self.touched.insert(term);
                        t.focus(&self.client.0);
                        true
                    }
                    None => self.no_term(term).await,
                }
            }
            ClientMsg::Resize { term, rows, cols } => {
                if !self.writable().await {
                    return true;
                }
                if rows == 0 || cols == 0 || rows > MAX_DIM || cols > MAX_DIM {
                    return self
                        .error(ErrorCode::Invalid, "error: rows and cols must be 1..=1000")
                        .await;
                }
                match self.term(term).await {
                    Some(t) => {
                        self.touched.insert(term);
                        t.resize(&self.client.0, rows, cols);
                        true
                    }
                    None => self.no_term(term).await,
                }
            }
            ClientMsg::Ping { nonce } => send_msg(&self.out, &ServerMsg::Pong { nonce }).await,
            ClientMsg::SubChat { agent, after } => self.sub_chat(agent, after).await,
            ClientMsg::UnsubChat { agent } => {
                if let Some(h) = self.chats.remove(&agent) {
                    h.abort();
                }
                true
            }
        }
    }

    /// Sends the entries after `after`, then forwards new ones until
    /// `unsub_chat` or the end of the connection.
    async fn sub_chat(&mut self, agent: AgentId, after: Option<String>) -> bool {
        if let Some(h) = self.chats.remove(&agent) {
            h.abort();
        }
        let tails = self.app.transcripts.clone();
        let Some(mut sub) = tails
            .subscribe(&self.app.actor, &agent, after.as_deref())
            .await
        else {
            return self
                .error(ErrorCode::NotFound, format!("error: no agent {agent}"))
                .await;
        };
        let out = self.out.clone();
        let a = agent.clone();
        let task = tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            let mut entries = std::mem::take(&mut sub.catchup);
            let mut last = after;
            loop {
                if let Some(e) = entries.last() {
                    last = Some(e.uuid.clone());
                }
                let msg = ServerMsg::Chat {
                    agent: a.clone(),
                    entries,
                };
                if !send_msg(&out, &msg).await {
                    return;
                }
                entries = match sub.rx.recv().await {
                    Ok(batch) => batch,
                    // Too slow for the broadcast: catch up from what we sent
                    // (an entry read meanwhile may come twice; same uuid).
                    Err(RecvError::Lagged(_)) => {
                        sub.rx = sub.rx.resubscribe();
                        tails.since(&a, last.as_deref())
                    }
                    Err(RecvError::Closed) => return,
                };
            }
        });
        self.chats.insert(agent, task);
        true
    }

    async fn on_binary(&mut self, b: &[u8]) -> bool {
        let frame = match Frame::decode(b) {
            Ok(f) => f,
            Err(e) => return self.error(ErrorCode::Invalid, format!("error: {e}")).await,
        };
        let Frame::Input { term, data } = frame else {
            return self
                .error(ErrorCode::Invalid, "error: clients send only input frames")
                .await;
        };
        if !self.writable().await {
            return true;
        }
        let Some(t) = self.term(term).await else {
            return self.no_term(term).await;
        };
        self.touched.insert(term);
        let job = InputJob {
            term: t,
            client: self.client.0.clone(),
            data,
            out: self.out.clone(),
        };
        if self.input.send(job).await.is_err() {
            return false;
        }
        // Typing in an agent's terminal delays path-B message delivery.
        if let Some(Some(agent)) = self.agent_of(term).await {
            let due = self
                .human_input
                .get(&term)
                .is_none_or(|t| t.elapsed() >= HUMAN_INPUT_EVERY);
            if due {
                self.human_input.insert(term, Instant::now());
                self.app.actor.send(Command::Client {
                    client: self.client.clone(),
                    action: ClientAction::MarkHumanInput { agent },
                });
            }
        }
        true
    }

    /// Read-only devices may watch but not touch: the error is sent, the
    /// connection stays open, and the caller skips the request.
    async fn writable(&self) -> bool {
        if self.scope.can_write() {
            return true;
        }
        send_error(
            &self.out,
            ErrorCode::Forbidden,
            "error: this device is read-only",
        )
        .await;
        false
    }

    async fn term(&self, id: TermId) -> Option<Arc<Terminal>> {
        if let Some(t) = self.app.hub.get_term(id) {
            return Some(t);
        }
        // Only a loose terminal can be known to the engine before its PTY
        // exists (`POST /v1/terminals` registers it first); an agent's appears
        // in state once its PTY is up, so a miss there is final. Wait for the
        // loose one only while the engine still lists it.
        for _ in 0..200 {
            let starting = self
                .app
                .actor
                .query(move |s| s.terminals.get(&id).is_some_and(|t| t.agent.is_none()))
                .await
                .ok()?;
            if !starting {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if let Some(t) = self.app.hub.get_term(id) {
                return Some(t);
            }
        }
        None
    }

    async fn no_term(&self, id: TermId) -> bool {
        self.error(ErrorCode::NotFound, format!("error: no terminal {id}"))
            .await
    }

    /// `Some(agent)` if the engine knows the terminal.
    async fn agent_of(&mut self, id: TermId) -> Option<Option<AgentId>> {
        if let Some(a) = self.agents.get(&id) {
            return Some(a.clone());
        }
        let found = self
            .app
            .actor
            .query(move |s| s.terminals.get(&id).map(|t| t.agent.clone()))
            .await
            .ok()
            .flatten()?;
        self.agents.insert(id, found.clone());
        Some(found)
    }

    /// Sends the catch-up, then streams live output. Returns false when the
    /// connection is gone.
    async fn subscribe(&mut self, id: TermId, since_seq: Option<u64>) -> bool {
        let (Some(term), Some(_)) = (self.term(id).await, self.agent_of(id).await) else {
            return self.no_term(id).await;
        };
        if let Some((_, old)) = self.subs.remove(&id) {
            old.abort();
        }
        let sub = term.subscribe(since_seq);
        let start = sub.catchup.seq();
        let frame = match sub.catchup {
            Catchup::Bytes { data, seq } => (!data.is_empty()).then(|| Frame::Output {
                term: id,
                seq: seq - data.len() as u64,
                data,
            }),
            Catchup::Snapshot { data, seq } => Some(Frame::Snapshot {
                term: id,
                seq,
                data,
            }),
        };
        let size = ServerMsg::Size {
            term: id,
            rows: sub.size.rows,
            cols: sub.size.cols,
            owner: sub.size.owner.map(ClientId),
        };
        if !send_msg(&self.out, &size).await {
            return false;
        }
        if let Some(f) = frame
            && !send_frame(&self.out, f).await
        {
            return false;
        }
        self.next_gen += 1;
        let generation = self.next_gen;
        let fwd = Forwarder {
            id,
            generation,
            term,
            rx: sub.rx,
            next_seq: start,
            out: self.out.clone(),
            ctl: self.ctl.clone(),
            actor: self.app.actor.clone(),
        };
        self.subs.insert(id, (generation, tokio::spawn(fwd.run())));
        true
    }
}

/// Moves one terminal's events to the socket queue.
struct Forwarder {
    id: TermId,
    generation: u64,
    term: Arc<Terminal>,
    rx: mpsc::Receiver<TermEvent>,
    /// The next byte the client is owed.
    next_seq: u64,
    out: Out,
    ctl: mpsc::UnboundedSender<Ctl>,
    actor: ActorHandle,
}

impl Forwarder {
    async fn run(mut self) {
        let mut title = self.term.title();
        loop {
            match self.rx.recv().await {
                Some(TermEvent::Output { seq, data }) => {
                    self.next_seq = seq + data.len() as u64;
                    let f = Frame::Output {
                        term: self.id,
                        seq,
                        data: data.to_vec(),
                    };
                    if !send_frame(&self.out, f).await {
                        return;
                    }
                    let now = self.term.title();
                    if now != title {
                        title = now.clone();
                        self.actor.send(Command::Runner(RunnerEvent::TermTitle {
                            term: self.id,
                            title: now,
                        }));
                    }
                }
                Some(TermEvent::Size(s)) => {
                    // Every subscriber reports; the engine ignores repeats.
                    self.actor.send(Command::Runner(RunnerEvent::TermSize {
                        term: self.id,
                        rows: s.rows,
                        cols: s.cols,
                        owner: s.owner.clone().map(ClientId),
                    }));
                    let msg = ServerMsg::Size {
                        term: self.id,
                        rows: s.rows,
                        cols: s.cols,
                        owner: s.owner.map(ClientId),
                    };
                    if !send_msg(&self.out, &msg).await {
                        return;
                    }
                }
                Some(TermEvent::Exit(info)) => {
                    send_msg(
                        &self.out,
                        &ServerMsg::Exit {
                            term: self.id,
                            code: info.code,
                            signal: info.signal,
                        },
                    )
                    .await;
                    return;
                }
                None => {
                    let _ = self.ctl.send(Ctl::Stalled {
                        term: self.id,
                        generation: self.generation,
                        next_seq: self.next_seq,
                    });
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use crate::term::hub::{Hub, TermConfig, term_key};
    use crate::term::pty::{INPUT_QUEUE_BYTES, PtySpec};
    use cmux_core::ids::TermId;

    fn job(term: &Arc<Terminal>, data: Vec<u8>, out: &Out) -> InputJob {
        InputJob {
            term: term.clone(),
            client: "c1".into(),
            data,
            out: out.clone(),
        }
    }

    fn exited(hub: &Hub, id: u32) -> Arc<Terminal> {
        let t = hub
            .create(
                &term_key(TermId(id)),
                &PtySpec::new("true", &[]),
                &TermConfig::small(),
            )
            .unwrap();
        let mut rx = t.exited();
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while rx.borrow_and_update().is_none() {
            assert!(std::time::Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        t
    }

    #[test]
    fn input_to_an_exited_terminal_is_dropped_silently() {
        let hub = Hub::new();
        let t = exited(&hub, 1);
        let (out, mut out_rx) = mpsc::channel(4);
        let (tx, rx) = mpsc::channel(4);
        tx.try_send(job(&t, b"x".to_vec(), &out)).unwrap();
        tx.try_send(job(&t, b"y".to_vec(), &out)).unwrap();
        drop(tx);
        input_loop(rx);
        assert!(out_rx.try_recv().is_err());
    }

    #[test]
    fn input_to_a_live_child_that_stopped_reading_reports_limit() {
        let hub = Hub::new();
        let t = hub
            .create(
                &term_key(TermId(2)),
                &PtySpec::new("sleep", &["30"]),
                &TermConfig::small(),
            )
            .unwrap();
        // Fill the queue until a write is refused, then hand input_loop one
        // more chunk at once. Linux blocks the writer on a full tty, but a
        // BSD tty (macOS) discards unread input, so the writer keeps draining
        // slowly there and the queue can free up between the two steps;
        // retrying keeps the test about input_loop rather than that race.
        let full = vec![b'x'; INPUT_QUEUE_BYTES];
        let mut reported = false;
        for _ in 0..50 {
            while t.write(&full) {}
            let (out, mut out_rx) = mpsc::channel(4);
            let (tx, rx) = mpsc::channel(4);
            tx.try_send(job(&t, full.clone(), &out)).unwrap();
            drop(tx);
            input_loop(rx);
            if let Ok(msg) = out_rx.try_recv() {
                let Message::Text(body) = msg else {
                    panic!("expected a text frame");
                };
                assert!(body.contains("not accepting input"));
                reported = true;
                break;
            }
        }
        assert!(reported, "a full queue never reported the limit");
        t.kill();
    }
}
