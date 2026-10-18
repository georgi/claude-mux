//! The WebSocket stream and loose terminals against an in-process server:
//! real PTYs (`cat`, `sh`), real sockets, the real actor.

use std::path::Path;
use std::time::Duration;

use cmux_proto::api::{
    CreateTerminalReq, ErrorCode, NewTerminalKind, Scope, ScrollbackResp, StatusResp, route,
};
use cmux_proto::stream::{ClientKind, ClientMsg, Frame, ServerMsg, TermPos};
use cmux_proto::view::{StateView, TermId, TerminalView};
use cmuxd::client::Endpoint;
use cmuxd::config::Config;
use cmuxd::server::{Server, ServerBuilder};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::{TcpStream, UnixStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const WAIT: Duration = Duration::from_secs(10);

struct Harness {
    server: Server,
    unix: Endpoint,
    tmp: tempfile::TempDir,
}

async fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: Some(tmp.path().join("data")),
        socket: Some(tmp.path().join("api.sock")),
        listen: Some("127.0.0.1:0".into()),
        ..Config::default()
    };
    let server = ServerBuilder::new(cfg).tick(None).start().await.unwrap();
    Harness {
        unix: Endpoint::Unix(server.socket_path.clone()),
        server,
        tmp,
    }
}

impl Harness {
    fn token(&self, scope: Scope) -> String {
        self.server.app.devices.create("test", scope).unwrap().1
    }

    async fn post(&self, path: &str, body: &impl Serialize) -> cmuxd::client::Response {
        self.unix.json("POST", path, &[], body).await.unwrap()
    }

    async fn term(&self, kind: NewTerminalKind) -> TermId {
        let r = self
            .post(
                route::TERMINALS,
                &CreateTerminalReq {
                    kind,
                    cwd: Some(self.tmp.path().display().to_string()),
                    rows: Some(24),
                    cols: Some(80),
                },
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
        r.json::<TerminalView>().unwrap().id
    }

    async fn cat(&self) -> TermId {
        self.term(NewTerminalKind::Command {
            argv: vec!["cat".into()],
            label: None,
        })
        .await
    }

    async fn sh(&self, script: &str) -> TermId {
        self.term(NewTerminalKind::Command {
            argv: vec!["sh".into(), "-c".into(), script.into()],
            label: None,
        })
        .await
    }

    async fn ws(&self) -> Ws<MaybeTlsStream<TcpStream>> {
        let url = format!("ws://{}/v1/stream", self.server.tcp_addr.unwrap());
        let (s, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        Ws(s)
    }

    async fn state(&self) -> StateView {
        self.unix
            .request("GET", route::STATE, &[], None)
            .await
            .unwrap()
            .json()
            .unwrap()
    }
}

#[derive(Debug)]
enum Item {
    Msg(ServerMsg),
    Frame(Frame),
    Closed,
}

struct Ws<S>(WebSocketStream<S>);

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> Ws<S> {
    async fn send(&mut self, m: &ClientMsg) {
        let t = serde_json::to_string(m).unwrap();
        self.0.send(Message::text(t)).await.unwrap();
    }

    async fn input(&mut self, term: TermId, data: &[u8]) {
        let f = Frame::Input {
            term,
            data: data.to_vec(),
        };
        self.0.send(Message::binary(f.encode())).await.unwrap();
    }

    async fn hello(&mut self, token: Option<&str>, since_rev: Option<u64>, terms: Vec<TermPos>) {
        self.send(&ClientMsg::Hello {
            client: ClientKind::Tui,
            since_rev,
            token: token.map(str::to_string),
            info: Some("test".into()),
            terms,
        })
        .await;
    }

    async fn next(&mut self) -> Item {
        loop {
            let m = tokio::time::timeout(WAIT, self.0.next())
                .await
                .expect("timed out waiting for the server");
            match m {
                Some(Ok(Message::Text(t))) => {
                    return Item::Msg(serde_json::from_str(&t).unwrap());
                }
                Some(Ok(Message::Binary(b))) => return Item::Frame(Frame::decode(&b).unwrap()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Item::Closed,
                Some(Ok(other)) => panic!("unexpected {other:?}"),
            }
        }
    }

    async fn msg(&mut self) -> ServerMsg {
        match self.next().await {
            Item::Msg(m) => m,
            other => panic!("wanted a message, got {other:?}"),
        }
    }

    /// Reads until `f` matches a message; frames and other messages are skipped.
    async fn msg_where<T>(&mut self, mut f: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        loop {
            match self.next().await {
                Item::Msg(m) => {
                    if let Some(v) = f(&m) {
                        return v;
                    }
                }
                Item::Frame(_) => {}
                Item::Closed => panic!("closed while waiting"),
            }
        }
    }

    /// The standard connect: hello, then welcome, state/deltas, presence.
    async fn connect(&mut self, token: Option<&str>, since_rev: Option<u64>) -> (u64, ServerMsg) {
        self.hello(token, since_rev, vec![]).await;
        let ServerMsg::Welcome { rev, .. } = self.msg().await else {
            panic!("welcome first")
        };
        let first = self.msg().await;
        assert!(matches!(self.msg().await, ServerMsg::Presence { .. }));
        (rev, first)
    }

    /// Collects terminal output until `done(output)`; returns the whole
    /// output and every frame seen. Asserts the stream is gap-free.
    async fn output_until(
        &mut self,
        term: TermId,
        mut next_seq: Option<u64>,
        done: impl Fn(&[u8]) -> bool,
    ) -> (Vec<u8>, Vec<Frame>) {
        let (mut out, mut frames) = (Vec::new(), Vec::new());
        while !done(&out) {
            match self.next().await {
                Item::Frame(f) => {
                    assert_eq!(f.term(), term);
                    match &f {
                        Frame::Output { seq, data, .. } => {
                            if let Some(n) = next_seq {
                                assert_eq!(*seq, n, "gap or overlap in the stream");
                            }
                            next_seq = Some(seq + data.len() as u64);
                            out.extend_from_slice(data);
                        }
                        Frame::Snapshot { seq, data, .. } => {
                            next_seq = Some(*seq);
                            out = data.clone();
                        }
                        Frame::Input { .. } => panic!("server sent input"),
                    }
                    frames.push(f);
                }
                Item::Msg(_) => {}
                Item::Closed => panic!("closed; output so far {:?}", String::from_utf8_lossy(&out)),
            }
        }
        (out, frames)
    }
}

fn contains(hay: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(hay).contains(needle)
}

fn count(hay: &[u8], needle: &str) -> usize {
    String::from_utf8_lossy(hay).matches(needle).count()
}

async fn wait_for<T>(mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let end = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < end, "timed out");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_order_and_input_echoes_in_order() {
    let h = harness().await;
    let term = h.cat().await;
    let token = h.token(Scope::Full);
    let mut ws = h.ws().await;
    let (rev, first) = ws.connect(Some(&token), None).await;
    let ServerMsg::State { rev: r, snapshot } = first else {
        panic!("a fresh client gets state")
    };
    assert_eq!(r, rev);
    assert_eq!(snapshot.terminal(term).unwrap().cols, 80);
    assert_eq!(snapshot.loose, vec![term]);

    ws.send(&ClientMsg::Subscribe {
        term,
        since_seq: None,
    })
    .await;
    // A snapshot, then the size.
    let Item::Frame(Frame::Snapshot { seq, .. }) = ws.next().await else {
        panic!("snapshot first")
    };
    assert!(matches!(
        ws.msg().await,
        ServerMsg::Size {
            rows: 24,
            cols: 80,
            ..
        }
    ));

    let mut next = Some(seq);
    let mut all = Vec::new();
    for w in ["one", "two", "three"] {
        ws.input(term, format!("{w}\n").as_bytes()).await;
        let (out, _) = ws.output_until(term, next, |o| count(o, w) >= 2).await;
        next = Some(seq + (all.len() + out.len()) as u64);
        all.extend(out);
    }
    let s = String::from_utf8_lossy(&all).into_owned();
    let (a, b, c) = (
        s.find("one").unwrap(),
        s.find("two").unwrap(),
        s.find("three").unwrap(),
    );
    assert!(a < b && b < c, "{s:?}");
    assert!(s.ends_with("three\r\nthree\r\n"), "{s:?}");

    // Counted while connected, and gone after.
    let clients = || async {
        let r = h
            .unix
            .request("GET", route::STATUS, &[], None)
            .await
            .unwrap();
        r.json::<StatusResp>().unwrap().clients
    };
    assert_eq!(clients().await, 1);
    drop(ws);
    wait_for(async || (clients().await == 0).then_some(())).await;
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_within_the_ring_sends_only_the_missing_bytes() {
    let h = harness().await;
    let term = h.cat().await;
    let token = h.token(Scope::Full);
    let mut ws = h.ws().await;
    ws.connect(Some(&token), None).await;
    ws.send(&ClientMsg::Subscribe {
        term,
        since_seq: None,
    })
    .await;
    let Item::Frame(Frame::Snapshot { seq, .. }) = ws.next().await else {
        panic!()
    };
    ws.input(term, b"aaa\n").await;
    let (out, _) = ws
        .output_until(term, Some(seq), |o| count(o, "aaa") >= 2)
        .await;
    let since = seq + out.len() as u64;
    drop(ws);

    // Output the client never saw: typed by someone else.
    let mut other = h.ws().await;
    other.connect(Some(&token), None).await;
    other.input(term, b"bbb\n").await;
    wait_for(async || {
        let t = h.server.app.hub.get_term(term).unwrap();
        (t.stats().seq >= since + 10).then_some(())
    })
    .await;

    let mut back = h.ws().await;
    back.hello(
        Some(&token),
        None,
        vec![TermPos {
            id: term,
            since_seq: since,
        }],
    )
    .await;
    let (out, frames) = back
        .output_until(term, None, |o| count(o, "bbb") >= 2)
        .await;
    assert!(
        frames.iter().all(|f| matches!(f, Frame::Output { .. })),
        "{frames:?}"
    );
    let Frame::Output { seq, .. } = frames[0] else {
        panic!()
    };
    assert_eq!(seq, since, "resumes exactly where the client stopped");
    assert!(
        !contains(&out, "aaa"),
        "{:?}",
        String::from_utf8_lossy(&out)
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_outside_the_ring_gets_a_snapshot() {
    let h = harness().await;
    // 2.5 MB then a marker line; the ring holds 1 MiB.
    let term = h
        .sh("head -c 2500000 /dev/zero | tr '\\0' x; echo; echo MARK; cat")
        .await;
    let hub_term = h.server.app.hub.get_term(term).unwrap();
    wait_for(async || (hub_term.stats().seq >= 2_500_000).then_some(())).await;
    wait_for(async || contains(hub_term.screen_text().as_bytes(), "MARK").then_some(())).await;
    let total = hub_term.stats().seq;

    let token = h.token(Scope::Full);
    let mut ws = h.ws().await;
    ws.hello(
        Some(&token),
        None,
        vec![TermPos {
            id: term,
            since_seq: 10,
        }],
    )
    .await;
    let (_, frames) = ws.output_until(term, None, |o| contains(o, "MARK")).await;
    let Frame::Snapshot { seq, .. } = frames[0] else {
        panic!("expected a snapshot, got {:?}", frames[0])
    };
    assert_eq!(seq, total);
    assert_eq!(frames.len(), 1, "the snapshot alone carries the screen");
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn input_from_another_client_moves_the_size_owner() {
    let h = harness().await;
    let term = h.cat().await;
    let token = h.token(Scope::Full);
    let (mut a, mut b) = (h.ws().await, h.ws().await);
    let ServerMsg::Welcome { client_id: ida, .. } = ({
        a.hello(Some(&token), None, vec![]).await;
        a.msg().await
    }) else {
        panic!()
    };
    let ServerMsg::Welcome { client_id: idb, .. } = ({
        b.hello(Some(&token), None, vec![]).await;
        b.msg().await
    }) else {
        panic!()
    };
    assert_ne!(ida, idb);
    for w in [&mut a, &mut b] {
        w.send(&ClientMsg::Subscribe {
            term,
            since_seq: None,
        })
        .await;
    }

    a.send(&ClientMsg::Resize {
        term,
        rows: 30,
        cols: 100,
    })
    .await;
    let want = Some(ida.clone());
    let size = |m: &ServerMsg| match m {
        ServerMsg::Size {
            rows, cols, owner, ..
        } if owner.is_some() => Some((*rows, *cols, owner.clone())),
        _ => None,
    };
    assert_eq!(a.msg_where(size).await, (30, 100, want.clone()));
    assert_eq!(b.msg_where(size).await, (30, 100, want));

    // B types: it becomes the owner (its own size is not known yet).
    b.input(term, b"x").await;
    let (_, _, owner) = a
        .msg_where(|m| size(m).filter(|s| s.2.as_ref() != Some(&ida)))
        .await;
    assert_eq!(owner, Some(idb.clone()));

    // The engine's terminal view follows.
    wait_for(async || {
        let t = h.state().await.terminal(term).cloned()?;
        (t.size_owner == Some(idb.clone()) && (t.rows, t.cols) == (30, 100)).then_some(())
    })
    .await;
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deltas_resume_from_since_rev() {
    let h = harness().await;
    let token = h.token(Scope::Full);
    let plan = |title: &str| {
        serde_json::json!({
            "title": title,
            "repos": [{"path": h.tmp.path().display().to_string(), "base_branch": "main"}],
            "request": "x",
        })
    };
    let mut ws = h.ws().await;
    let (rev0, first) = ws.connect(Some(&token), None).await;
    assert!(matches!(first, ServerMsg::State { .. }));
    assert_eq!(h.post("/v1/plans", &plan("first")).await.status, 201);
    let live = ws
        .msg_where(|m| match m {
            ServerMsg::Deltas { rev, items } => Some((*rev, items.len())),
            _ => None,
        })
        .await;
    assert!(live.0 > rev0 && live.1 > 0);
    drop(ws);

    assert_eq!(h.post("/v1/plans", &plan("second")).await.status, 201);
    let mut ws = h.ws().await;
    let (rev, first) = ws.connect(Some(&token), Some(live.0)).await;
    let ServerMsg::Deltas { rev: r, items } = first else {
        panic!("a recent client gets deltas, not state")
    };
    assert_eq!(r, rev);
    assert!(items.iter().all(|d| d.rev > live.0));
    let titles: Vec<String> = items
        .iter()
        .filter_map(|d| match &d.change {
            cmux_proto::view::ChangeView::Plan(p) => Some(p.title.clone()),
            _ => None,
        })
        .collect();
    assert!(titles.contains(&"second".to_string()), "{titles:?}");
    assert!(!titles.contains(&"first".to_string()), "{titles:?}");

    // Caught up exactly: an empty deltas message, not a state.
    let mut ws2 = h.ws().await;
    let (_, first) = ws2.connect(Some(&token), Some(rev)).await;
    assert!(matches!(first, ServerMsg::Deltas { items, .. } if items.is_empty()));
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_devices_watch_but_cannot_type_or_resize() {
    let h = harness().await;
    let term = h.cat().await;
    let token = h.token(Scope::Read);
    let mut ws = h.ws().await;
    ws.hello(Some(&token), None, vec![]).await;
    let ServerMsg::Welcome { scope, .. } = ws.msg().await else {
        panic!()
    };
    assert_eq!(scope, Scope::Read);
    ws.send(&ClientMsg::Subscribe {
        term,
        since_seq: None,
    })
    .await;
    ws.input(term, b"nope\n").await;
    ws.send(&ClientMsg::Resize {
        term,
        rows: 10,
        cols: 10,
    })
    .await;
    ws.send(&ClientMsg::Focus { term }).await;
    for _ in 0..3 {
        let code = ws
            .msg_where(|m| match m {
                ServerMsg::Error { code, .. } => Some(*code),
                _ => None,
            })
            .await;
        assert_eq!(code, ErrorCode::Forbidden);
    }
    // Still connected and still watching.
    ws.send(&ClientMsg::Ping { nonce: 9 }).await;
    ws.msg_where(|m| matches!(m, ServerMsg::Pong { nonce: 9 }).then_some(()))
        .await;
    let t = h.server.app.hub.get_term(term).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!contains(t.screen_text().as_bytes(), "nope"));
    assert_eq!(t.size().owner, None);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthenticated_tcp_hello_is_rejected() {
    let h = harness().await;
    for token in [None, Some("not-a-token")] {
        let mut ws = h.ws().await;
        ws.hello(token, None, vec![]).await;
        match ws.next().await {
            Item::Msg(ServerMsg::Error { code, .. }) => assert_eq!(code, ErrorCode::Unauthorized),
            other => panic!("{other:?}"),
        }
        assert!(matches!(ws.next().await, Item::Closed));
    }
    // Not hello first.
    let mut ws = h.ws().await;
    ws.send(&ClientMsg::Ping { nonce: 1 }).await;
    assert!(matches!(
        ws.next().await,
        Item::Msg(ServerMsg::Error {
            code: ErrorCode::Invalid,
            ..
        })
    ));
    assert!(matches!(ws.next().await, Item::Closed));
    assert_eq!(
        h.server
            .app
            .clients
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_unix_socket_needs_no_token() {
    let h = harness().await;
    let s = UnixStream::connect(&h.server.socket_path).await.unwrap();
    let (s, _) = tokio_tungstenite::client_async("ws://localhost/v1/stream", s)
        .await
        .unwrap();
    let mut ws = Ws(s);
    let (_, first) = ws.connect(None, None).await;
    assert!(matches!(first, ServerMsg::State { .. }));
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn presence_lists_connected_clients() {
    let h = harness().await;
    let token = h.token(Scope::Full);
    let mut a = h.ws().await;
    a.connect(Some(&token), None).await;
    let mut b = h.ws().await;
    b.connect(Some(&token), None).await;
    let n = a
        .msg_where(|m| match m {
            ServerMsg::Presence { clients } if clients.len() == 2 => Some(clients.len()),
            _ => None,
        })
        .await;
    assert_eq!(n, 2);
    drop(b);
    a.msg_where(|m| {
        matches!(m, ServerMsg::Presence { clients } if clients.len() == 1).then_some(())
    })
    .await;
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_is_reported_and_a_late_subscriber_sees_it_too() {
    let h = harness().await;
    let term = h.sh("echo bye; exit 3").await;
    let t = h.server.app.hub.get_term(term).unwrap();
    wait_for(async || t.exit_status().map(|_| ())).await;
    let token = h.token(Scope::Full);
    let mut ws = h.ws().await;
    ws.connect(Some(&token), None).await;
    ws.send(&ClientMsg::Subscribe {
        term,
        since_seq: None,
    })
    .await;
    let code = ws
        .msg_where(|m| match m {
            ServerMsg::Exit { code, .. } => Some(*code),
            _ => None,
        })
        .await;
    assert_eq!(code, 3);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribing_to_an_unknown_terminal_is_an_error() {
    let h = harness().await;
    let mut ws = h.ws().await;
    let token = h.token(Scope::Full);
    ws.connect(Some(&token), None).await;
    ws.send(&ClientMsg::Subscribe {
        term: TermId(77),
        since_seq: None,
    })
    .await;
    let code = ws
        .msg_where(|m| match m {
            ServerMsg::Error { code, .. } => Some(*code),
            _ => None,
        })
        .await;
    assert_eq!(code, ErrorCode::NotFound);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn loose_terminal_rest_lifecycle() {
    let h = harness().await;
    // Bad requests.
    let bad_cwd = h
        .post(
            route::TERMINALS,
            &serde_json::json!({"kind": "shell", "cwd": "/no/such/dir"}),
        )
        .await;
    assert_eq!(bad_cwd.status, 400);
    let empty = h
        .post(
            route::TERMINALS,
            &serde_json::json!({"kind": "command", "argv": []}),
        )
        .await;
    assert_eq!(empty.status, 400);
    let missing = h
        .post(
            route::TERMINALS,
            &serde_json::json!({"kind": "command", "argv": ["/no/such/program"]}),
        )
        .await;
    assert_eq!(missing.status, 400, "{}", missing.text());
    assert!(
        h.state().await.terminals.is_empty(),
        "failed spawns roll back"
    );

    let term = h.sh("seq 1 100; sleep 30").await;
    let t = h.server.app.hub.get_term(term).unwrap();
    wait_for(async || contains(t.screen_text().as_bytes(), "100").then_some(())).await;
    let sv = h.state().await;
    let view = sv.terminal(term).unwrap();
    assert!(view.agent.is_none() && sv.loose == vec![term]);

    let path = format!("{}?lines=10", route::scrollback(term.0));
    let r = h.unix.request("GET", &path, &[], None).await.unwrap();
    assert_eq!(r.status, 200);
    let sb: ScrollbackResp = r.json().unwrap();
    assert_eq!(sb.term, term);
    assert_eq!(sb.lines.len(), 10);
    assert!(sb.total >= 70, "{}", sb.total);
    // Oldest first, ending just above the 24-row screen.
    let nums: Vec<u32> = sb.lines.iter().map(|l| l.trim().parse().unwrap()).collect();
    assert!(nums.windows(2).all(|w| w[1] == w[0] + 1), "{nums:?}");

    let gone = route::terminal(term.0);
    let r = h.unix.request("DELETE", &gone, &[], None).await.unwrap();
    assert_eq!(r.status, 204);
    assert!(h.server.app.hub.get_term(term).is_none());
    assert!(h.state().await.terminals.is_empty());
    let r = h.unix.request("DELETE", &gone, &[], None).await.unwrap();
    assert_eq!(r.status, 404);
    let r = h.unix.request("GET", &gone, &[], None).await.unwrap();
    assert_eq!(r.status, 404);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_devices_cannot_create_terminals_but_can_read_scrollback() {
    let h = harness().await;
    let term = h.cat().await;
    let tok = h.token(Scope::Read);
    let tcp = Endpoint::Tcp(h.server.tcp_addr.unwrap());
    let auth = format!("Bearer {tok}");
    let body = serde_json::json!({"kind": "shell"});
    let r = tcp
        .json("POST", route::TERMINALS, &[("Authorization", &auth)], &body)
        .await
        .unwrap();
    assert_eq!(r.status, 403);
    let r = tcp
        .request(
            "GET",
            &route::scrollback(term.0),
            &[("Authorization", &auth)],
            None,
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orphaned_loose_terminals_are_dropped_at_start() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = |p: &Path| Config {
        data_dir: Some(p.join("data")),
        socket: Some(p.join("api.sock")),
        listen: None,
        ..Config::default()
    };
    let s = ServerBuilder::new(cfg(tmp.path()))
        .tick(None)
        .start()
        .await
        .unwrap();
    let ep = Endpoint::Unix(s.socket_path.clone());
    let r = ep
        .json(
            "POST",
            route::TERMINALS,
            &[],
            &serde_json::json!({"kind": "command", "argv": ["cat"], "cwd": tmp.path()}),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201, "{}", r.text());
    s.shutdown().await;

    // The data-dir lock is released by the store thread a moment after shutdown.
    let s = loop {
        match ServerBuilder::new(cfg(tmp.path())).tick(None).start().await {
            Ok(s) => break s,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };
    assert!(s.app.actor.state_view().await.unwrap().terminals.is_empty());
    s.shutdown().await;
}
