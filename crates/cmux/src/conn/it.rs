//! The client against a real in-process cmuxd: real PTY (`cat`), real
//! sockets, the real actor.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cmux_proto::api::{CreateTerminalReq, NewTerminalKind, Scope, route};
use cmux_proto::view::{ChangeView, StateView, TermId, TerminalView};
use cmuxd::config::Config;
use cmuxd::server::{Server, ServerBuilder};

use super::http::Http;
use super::servers::Endpoint;
use super::stream::{ConnStatus, Connection, Mirror, StreamConfig, StreamEvent};
use crate::tui::remote_term::RemoteTerm;

fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn new_cat(local: &Http, cwd: &std::path::Path) -> TermId {
    let t: TerminalView = local
        .post(
            route::TERMINALS,
            &CreateTerminalReq {
                kind: NewTerminalKind::Command {
                    argv: vec!["cat".into()],
                    label: None,
                },
                cwd: Some(cwd.display().to_string()),
                rows: Some(24),
                cols: Some(80),
            },
        )
        .unwrap();
    t.id
}

fn count(hay: &str, needle: &str) -> usize {
    hay.matches(needle).count()
}

fn resumes(
    server: &Server,
    rt: &tokio::runtime::Runtime,
    endpoint: Endpoint,
    cwd: &std::path::Path,
) {
    let _guard = rt.enter();
    let local = Http::new(Endpoint::Unix(server.socket_path.clone()));
    let term = new_cat(&local, cwd);

    let events: Arc<Mutex<Vec<StreamEvent>>> = Arc::default();
    let sink = events.clone();
    let mut conn = Connection::spawn(
        StreamConfig {
            endpoint,
            info: "cmux test".into(),
        },
        move |e| sink.lock().unwrap().push(e),
    );
    let rterm = RemoteTerm::new(conn.handle(), term, 24, 80);
    let screen = |rt: &RemoteTerm| rt.parser.lock().unwrap().screen().contents();
    let n_events =
        |f: fn(&StreamEvent) -> bool| events.lock().unwrap().iter().filter(|e| f(e)).count();

    wait("connected", || {
        (conn.status() == ConnStatus::Connected).then_some(())
    });
    rterm.write(b"one\n");
    // The tty echoes the line and cat prints it back.
    wait("first line", || {
        (count(&screen(&rterm), "one") == 2).then_some(())
    });

    // Drop the link. Something happens on the server in the meantime: a new
    // terminal (a state delta) and output nobody saw.
    let dropped = || n_events(|e| matches!(e, StreamEvent::Status(ConnStatus::Reconnecting)));
    let before = dropped();
    conn.handle().reconnect();
    // Wait for the thread to actually drop the socket, or the bytes below
    // would arrive over the old link and resume would go untested.
    wait("link dropped", || (dropped() > before).then_some(()));
    let other = new_cat(&local, cwd);
    server
        .app
        .hub
        .get_term(term)
        .unwrap()
        .input("someone-else", b"two\n");

    wait("second line", || {
        (count(&screen(&rterm), "two") == 2).then_some(())
    });
    wait("second welcome", || {
        (n_events(|e| matches!(e, StreamEvent::Welcome { .. })) >= 2
            && conn.status() == ConnStatus::Connected)
            .then_some(())
    });
    let s = screen(&rterm);
    assert_eq!(count(&s, "one"), 2, "no duplicated bytes: {s:?}");
    assert_eq!(count(&s, "two"), 2, "{s:?}");

    // Resumed from rev: one full state in total, then deltas.
    let evs = events.lock().unwrap().clone();
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, StreamEvent::State(_)))
            .count(),
        1,
        "{evs:?}"
    );
    // Resumed from seq: the only snapshot was the first subscribe.
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, StreamEvent::TermReset(_)))
            .count(),
        1
    );
    let mut mirror = Mirror::default();
    for e in &evs {
        mirror.apply(e);
    }
    assert!(
        mirror.sv.terminal(other).is_some(),
        "the delta made it into the mirror"
    );
    assert!(evs.iter().any(|e| matches!(
        e,
        StreamEvent::Deltas { items, .. }
            if items.iter().any(|d| matches!(&d.change, ChangeView::Terminal(t) if t.id == other))
    )));
    assert_eq!(mirror.status, ConnStatus::Connected);
    assert!(mirror.client_id.is_some());

    drop(rterm);
    conn.shutdown();
    let _: StateView = local.get(route::STATE).unwrap();
}

#[test]
fn reconnect_resumes_from_rev_and_seq_over_unix_and_tcp() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        data_dir: Some(tmp.path().join("data")),
        socket: Some(tmp.path().join("api.sock")),
        listen: Some("127.0.0.1:0".into()),
        ..Config::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt
        .block_on(ServerBuilder::new(cfg).tick(None).start())
        .unwrap();

    resumes(
        &server,
        &rt,
        Endpoint::Unix(server.socket_path.clone()),
        tmp.path(),
    );

    let token = server.app.devices.create("test", Scope::Full).unwrap().1;
    let url = format!("http://{}", server.tcp_addr.unwrap());
    resumes(&server, &rt, Endpoint::Remote { url, token }, tmp.path());

    rt.block_on(server.shutdown());
}
