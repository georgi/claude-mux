use std::time::Instant;

use super::*;

const WAIT: Duration = Duration::from_secs(60);

fn sh(script: &str) -> PtySpec {
    PtySpec::new("sh", &["-c", script])
}

fn cat() -> PtySpec {
    PtySpec::new("cat", &[])
}

/// Poll `f` until it returns Some or the deadline passes.
fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + WAIT;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Collect events until the accumulated output satisfies `done`.
fn read_until(
    rx: &mut mpsc::Receiver<TermEvent>,
    mut done: impl FnMut(&str) -> bool,
) -> Vec<TermEvent> {
    let mut events = Vec::new();
    let mut bytes = Vec::new();
    wait_for(|| {
        while let Ok(ev) = rx.try_recv() {
            if let TermEvent::Output { data, .. } = &ev {
                bytes.extend_from_slice(data);
            }
            events.push(ev);
        }
        done(&String::from_utf8_lossy(&bytes)).then_some(())
    });
    events
}

fn output_text(events: &[TermEvent]) -> String {
    let mut bytes = Vec::new();
    for ev in events {
        if let TermEvent::Output { data, .. } = ev {
            bytes.extend_from_slice(data);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn input_echoes_in_order_with_contiguous_seq() {
    let term = Terminal::spawn("t", &cat(), &TermConfig::default()).unwrap();
    let mut sub = term.subscribe(None);
    assert!(matches!(&sub.catchup, Catchup::Snapshot { seq: 0, .. }));

    term.input("a", b"one\n");
    term.input("a", b"two\n");
    let events = read_until(&mut sub.rx, |s| s.matches("two").count() >= 2);
    let text = output_text(&events);
    assert!(text.find("one").unwrap() < text.find("two").unwrap());

    let mut next = 0;
    for ev in &events {
        if let TermEvent::Output { seq, data } = ev {
            assert_eq!(*seq, next);
            next += data.len() as u64;
        }
    }
    assert_eq!(term.stats().seq, next);
    term.kill();
}

#[test]
fn reconnect_within_ring_gets_only_missing_bytes() {
    let term = Terminal::spawn("t", &cat(), &TermConfig::default()).unwrap();
    let mut sub = term.subscribe(None);
    term.input("a", b"first\n");
    read_until(&mut sub.rx, |s| s.contains("first\r\nfirst"));
    let seen = term.stats().seq;
    drop(sub);

    term.input("a", b"second\n");
    wait_for(|| (term.stats().seq >= seen + 16).then_some(()));
    let sub = term.subscribe(Some(seen));
    let Catchup::Bytes { data, seq } = sub.catchup else {
        panic!("expected bytes");
    };
    let text = String::from_utf8_lossy(&data);
    assert!(
        text.contains("second") && !text.contains("first"),
        "{text:?}"
    );
    assert_eq!(seq, seen + data.len() as u64);

    // Caught up exactly: nothing missing.
    let sub = term.subscribe(Some(term.stats().seq));
    assert!(matches!(sub.catchup, Catchup::Bytes { ref data, .. } if data.is_empty()));
    term.kill();
}

#[test]
fn reconnect_outside_ring_gets_a_snapshot_that_redraws_the_screen() {
    let cfg = TermConfig {
        ring_bytes: 64,
        ..TermConfig::default()
    };
    let script = "i=0; while [ $i -lt 30 ]; do echo line-$i; i=$((i+1)); done; sleep 30";
    let term = Terminal::spawn("t", &sh(script), &cfg).unwrap();
    wait_for(|| term.screen_text().contains("line-29").then_some(()));
    assert!(term.stats().seq > 64);

    let sub = term.subscribe(Some(0));
    let Catchup::Snapshot { data, seq } = &sub.catchup else {
        panic!("expected snapshot");
    };
    assert_eq!(*seq, term.stats().seq);
    let mut other = vt100::Parser::new(24, 80, 0);
    other.process(data);
    assert_eq!(other.screen().contents(), term.screen_text());

    // A client ahead of the server (after a restart) also gets a snapshot.
    assert!(matches!(
        term.subscribe(Some(1 << 40)).catchup,
        Catchup::Snapshot { .. }
    ));
    term.kill();
}

#[test]
fn snapshot_of_the_alternate_screen_enters_it() {
    let term = Terminal::spawn(
        "t",
        &sh("printf '\\033[?1049hALT'; sleep 30"),
        &TermConfig::default(),
    )
    .unwrap();
    wait_for(|| term.screen_text().contains("ALT").then_some(()));
    let Catchup::Snapshot { data, .. } = term.subscribe(None).catchup else {
        panic!("expected snapshot");
    };
    assert!(data.starts_with(b"\x1b[?1049h"));
    let mut other = vt100::Parser::new(24, 80, 0);
    other.process(&data);
    assert!(other.screen().alternate_screen());
    assert!(other.screen().contents().contains("ALT"));
    term.kill();
}

#[test]
fn live_bytes_continue_at_the_snapshot_seq() {
    let term = Terminal::spawn("t", &cat(), &TermConfig::default()).unwrap();
    term.input("a", b"abc\n");
    wait_for(|| term.screen_text().contains("abc\nabc").then_some(()));
    let mut sub = term.subscribe(None);
    let seq = sub.catchup.seq();
    term.input("a", b"def\n");
    let events = read_until(&mut sub.rx, |s| s.contains("def\r\ndef"));
    let first = events.iter().find_map(|e| match e {
        TermEvent::Output { seq, .. } => Some(*seq),
        _ => None,
    });
    assert_eq!(first, Some(seq));
    term.kill();
}

#[test]
fn size_owner_follows_input_and_resizes_the_pty() {
    let term = Terminal::spawn("t", &PtySpec::new("sh", &[]), &TermConfig::default()).unwrap();
    let mut sub = term.subscribe(None);
    assert_eq!(sub.size.owner, None);

    // A has a 30x100 viewport, B has 40x120; the last to speak owns the size.
    assert!(term.resize("a", 30, 100));
    assert!(term.resize("b", 40, 120));
    let size = term.size();
    assert_eq!((size.rows, size.cols), (40, 120));
    assert_eq!(size.owner.as_deref(), Some("b"));

    // Input from A takes the size back, and the PTY really has that size.
    term.input("a", b"stty size\n");
    let size = term.size();
    assert_eq!((size.rows, size.cols), (30, 100));
    assert_eq!(size.owner.as_deref(), Some("a"));
    let events = read_until(&mut sub.rx, |s| s.contains("30 100\r\n"));
    let sizes: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            TermEvent::Size(s) => Some((s.rows, s.cols, s.owner.clone().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(
        sizes,
        [
            (30, 100, "a".to_string()),
            (40, 120, "b".to_string()),
            (30, 100, "a".to_string())
        ]
    );

    // Focus alone also switches the owner and size.
    assert!(term.focus("b"));
    assert_eq!((term.size().rows, term.size().cols), (40, 120));
    assert!(!term.focus("b"));
    term.kill();
}

#[test]
fn size_events_are_ordered_with_output() {
    let term = Terminal::spawn("t", &cat(), &TermConfig::default()).unwrap();
    term.resize("a", 20, 60);
    let mut sub = term.subscribe(None);
    term.resize("b", 10, 50);
    term.input("a", b"x\n");
    let events = read_until(&mut sub.rx, |s| s.contains("x\r\nx"));
    let sizes: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            TermEvent::Size(s) => Some((s.rows, s.cols, s.owner.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        sizes,
        [
            (10, 50, Some("b".to_string())),
            (20, 60, Some("a".to_string()))
        ]
    );
    term.kill();
}

#[test]
fn bursts_are_coalesced_into_few_chunks() {
    let cfg = TermConfig {
        batch: Duration::from_millis(100),
        ..TermConfig::default()
    };
    let script = "sleep 0.3; i=0; while [ $i -lt 200 ]; do printf a; i=$((i+1)); done; sleep 30";
    let term = Terminal::spawn("t", &sh(script), &cfg).unwrap();
    let mut sub = term.subscribe(None);
    let events = read_until(&mut sub.rx, |s| s.len() >= 200);
    let chunks = events
        .iter()
        .filter(|e| matches!(e, TermEvent::Output { .. }))
        .count();
    assert!(chunks <= 3, "{chunks} chunks for a 200-write burst");
    term.kill();
}

#[test]
fn exit_is_observable() {
    let term = Terminal::spawn("t", &sh("echo bye; exit 3"), &TermConfig::default()).unwrap();
    let mut exited = term.exited();
    wait_for(|| exited.borrow_and_update().clone());
    assert_eq!(term.exit_status().unwrap().code, 3);

    // A late subscriber still gets the final screen and the exit.
    let mut sub = term.subscribe(None);
    let Catchup::Snapshot { data, .. } = &sub.catchup else {
        panic!("expected snapshot");
    };
    assert!(String::from_utf8_lossy(data).contains("bye"));
    assert!(matches!(sub.rx.try_recv(), Ok(TermEvent::Exit(e)) if e.code == 3));
}

#[test]
fn live_subscriber_sees_exit_after_output() {
    let term = Terminal::spawn("t", &sh("sleep 0.2; echo bye"), &TermConfig::default()).unwrap();
    let mut sub = term.subscribe(None);
    let mut events = Vec::new();
    wait_for(|| {
        while let Ok(ev) = sub.rx.try_recv() {
            events.push(ev);
        }
        matches!(events.last(), Some(TermEvent::Exit(_))).then_some(())
    });
    assert!(output_text(&events).contains("bye"));
    assert!(matches!(events.last(), Some(TermEvent::Exit(e)) if e.code == 0));
}

#[test]
fn scrollback_returns_history_lines_oldest_first() {
    let mut spec = sh("seq 1 100; sleep 30");
    spec.rows = 10;
    let term = Terminal::spawn("t", &spec, &TermConfig::default()).unwrap();
    wait_for(|| term.screen_text().contains("100").then_some(()));

    // The screen shows 92..=100 and the cursor row, so history is 1..=91.
    let sb = term.scrollback(3, 0);
    assert_eq!(sb.total, 91);
    assert_eq!(sb.lines, ["89", "90", "91"]);
    assert_eq!(term.scrollback(3, 2).lines, ["87", "88", "89"]);

    // A window taller than the screen, and one running past the start.
    let long = term.scrollback(40, 10).lines;
    assert_eq!(long.len(), 40);
    assert_eq!((long[0].as_str(), long[39].as_str()), ("42", "81"));
    assert_eq!(
        term.scrollback(10, 85).lines,
        ["1", "2", "3", "4", "5", "6"]
    );
    assert!(term.scrollback(5, 500).lines.is_empty());
    assert!(term.scrollback(0, 0).lines.is_empty());

    // Reading history leaves the live screen untouched.
    assert!(term.screen_text().contains("92"));
    term.kill();
}

#[test]
fn answers_cursor_position_queries() {
    let script = "stty raw -echo; printf '\\033[6n'; dd bs=1 count=6 2>/dev/null | od -c; sleep 30";
    let term = Terminal::spawn("t", &sh(script), &TermConfig::default()).unwrap();
    wait_for(|| term.screen_text().contains("1   ;   1   R").then_some(()));
    term.kill();
}

#[test]
fn fifty_megabytes_of_output_stay_within_the_ring_and_parser() {
    let cfg = TermConfig {
        scrollback: 500,
        ..TermConfig::default()
    };
    let total: u64 = 50 * 1024 * 1024;
    let script = format!("yes 'the quick brown fox jumps over the lazy dog' | head -c {total}");
    let term = Terminal::spawn("t", &sh(&script), &cfg).unwrap();
    let mut sub = term.subscribe(None);
    let mut exited = term.exited();
    // Drain like a client would, so the subscriber is not dropped as stalled.
    wait_for(|| {
        while sub.rx.try_recv().is_ok() {}
        exited.borrow_and_update().clone()
    });
    let stats = term.stats();
    assert!(stats.seq >= total, "{} bytes produced", stats.seq);
    assert_eq!(stats.ring_len, RING_BYTES);
    assert_eq!(stats.ring_capacity, RING_BYTES);
    assert_eq!(stats.scrollback_len, 500);
    assert_eq!(stats.scrollback_capacity, 500);
    // A reconnect from the start gets a snapshot, not 50 MB.
    let Catchup::Snapshot { data, .. } = term.subscribe(Some(0)).catchup else {
        panic!("expected snapshot");
    };
    assert!(data.len() < 100_000);
}

#[test]
fn a_stalled_subscriber_is_dropped() {
    let term = Terminal::spawn(
        "t",
        &cat(),
        &TermConfig {
            batch: Duration::from_millis(1),
            ..TermConfig::default()
        },
    )
    .unwrap();
    let mut sub = term.subscribe(None);
    for _ in 0..SUB_QUEUE + 50 {
        term.focus("a");
        term.focus("b");
        if term.stats().subscribers == 0 {
            break;
        }
    }
    assert_eq!(term.stats().subscribers, 0);
    while sub.rx.try_recv().is_ok() {}
    assert!(sub.rx.try_recv().is_err());
    term.kill();
}

#[test]
fn hub_creates_gets_and_removes() {
    let hub = Hub::new();
    let id = hub.new_id();
    let term = hub.create(&id, &cat(), &TermConfig::small()).unwrap();
    assert_eq!(term.id(), id);
    assert!(hub.create(&id, &cat(), &TermConfig::small()).is_err());
    assert!(hub.get(&id).is_some() && hub.get("nope").is_none());
    assert_eq!(hub.ids(), std::slice::from_ref(&id));

    let mut exited = term.exited();
    hub.remove(&id).unwrap();
    wait_for(|| exited.borrow_and_update().clone());
    assert!(hub.get(&id).is_none());
    assert_ne!(hub.new_id(), id);
}

#[test]
fn writing_to_a_child_that_never_reads_does_not_block_the_caller() {
    let hub = Hub::new();
    let id = hub.new_id();
    let term = hub
        .create(&id, &sh("sleep 30"), &TermConfig::small())
        .unwrap();
    let start = Instant::now();
    // Far more than the tty buffer holds.
    term.write(&vec![b'x'; 4 * 1024 * 1024]);
    term.input("a", &vec![b'y'; 1024 * 1024]);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    term.kill();
}
