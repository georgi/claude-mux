//! Notifications: Web Push subscriptions against a mock push service, and the
//! ntfy sink against a mock ntfy server.

#[path = "support/mock_http.rs"]
mod mock_http;

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cmux_proto::api::{
    NotifyKind, PushPayload, PushSubscribeReq, PushUnsubscribeReq, VapidResp, route,
};
use cmuxd::client::Endpoint;
use cmuxd::config::{Config, PushConfig};
use cmuxd::server::{Server, ServerBuilder};
use mock_http::{Browser, MockHttp};

fn config(tmp: &Path, push: PushConfig) -> Config {
    Config {
        data_dir: Some(tmp.join("data")),
        socket: Some(tmp.join("api.sock")),
        listen: Some("off".into()),
        push,
        ..Config::default()
    }
}

async fn start(tmp: &Path, push: PushConfig) -> (Server, Endpoint) {
    let server = ServerBuilder::new(config(tmp, push))
        .tick(None)
        .start()
        .await
        .unwrap();
    let ep = Endpoint::Unix(server.socket_path.clone());
    (server, ep)
}

fn payload(kind: NotifyKind) -> PushPayload {
    PushPayload {
        kind,
        item: Some(7),
        plan: None,
        summary: "executor: Bash cargo test".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_round_trip_and_a_410_removes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (server, ep) = start(tmp.path(), PushConfig::default()).await;
    let mock = MockHttp::start().await;

    let r = ep
        .request("GET", route::PUSH_VAPID, &[], None)
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.text());
    let vapid: VapidResp = r.json().unwrap();
    assert_eq!(URL_SAFE_NO_PAD.decode(&vapid.public_key).unwrap().len(), 65);
    // The key is made once, private to the server user.
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(tmp.path().join("data/vapid.key")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    let browser = Browser::new();
    let endpoint = format!("{}/push/device-1", mock.base);
    let sub = PushSubscribeReq {
        endpoint: endpoint.clone(),
        keys: browser.keys(),
        kinds: vec![NotifyKind::Permission, NotifyKind::Question],
    };
    let r = ep
        .json("POST", route::PUSH_SUBSCRIBE, &[], &sub)
        .await
        .unwrap();
    assert_eq!(r.status, 204, "{}", r.text());
    assert_eq!(server.app.push.endpoints(), vec![endpoint.clone()]);

    // A kind the device did not choose goes nowhere.
    let sent = server.app.push.notify(&payload(NotifyKind::PlanDone)).await;
    assert_eq!(sent.delivered, 0);
    assert!(mock.hits().is_empty());

    let sent = server
        .app
        .push
        .notify(&payload(NotifyKind::Permission))
        .await;
    assert_eq!(sent.delivered, 1);
    let hit = mock.wait_for(1).await;
    assert_eq!(hit.path, "/push/device-1");
    assert_eq!(hit.header("content-encoding"), "aes128gcm");
    assert_eq!(hit.header("urgency"), "high");
    assert!(hit.header("ttl").parse::<u64>().unwrap() > 0);
    let auth = hit.header("authorization");
    assert!(auth.starts_with("vapid t="), "{auth}");
    assert!(
        auth.ends_with(&format!(", k={}", vapid.public_key)),
        "{auth}"
    );
    // Only the browser can read it, and it says only what the payload says.
    assert_eq!(browser.open(&hit.body), payload(NotifyKind::Permission));

    // The push service forgot the subscription: the server forgets it too.
    mock.answer(410);
    let sent = server.app.push.notify(&payload(NotifyKind::Question)).await;
    assert_eq!((sent.delivered, sent.dropped), (0, 1));
    assert!(server.app.push.endpoints().is_empty());
    let saved = std::fs::read_to_string(tmp.path().join("data/push.json")).unwrap();
    assert!(!saved.contains("device-1"), "{saved}");
    // Nothing is left to send to.
    server.app.push.notify(&payload(NotifyKind::Question)).await;
    assert_eq!(mock.hits().len(), 2);
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_404_also_removes_and_a_500_keeps_the_subscription() {
    let tmp = tempfile::tempdir().unwrap();
    let (server, ep) = start(tmp.path(), PushConfig::default()).await;
    let mock = MockHttp::start().await;
    let sub = |name: &str| PushSubscribeReq {
        endpoint: format!("{}/push/{name}", mock.base),
        keys: Browser::new().keys(),
        kinds: vec![NotifyKind::Permission],
    };
    for name in ["a", "b"] {
        let r = ep
            .json("POST", route::PUSH_SUBSCRIBE, &[], &sub(name))
            .await
            .unwrap();
        assert_eq!(r.status, 204, "{}", r.text());
    }
    mock.answer(500);
    let sent = server
        .app
        .push
        .notify(&payload(NotifyKind::Permission))
        .await;
    assert_eq!((sent.failed, sent.dropped), (2, 0));
    assert_eq!(server.app.push.endpoints().len(), 2);
    mock.answer(404);
    let sent = server
        .app
        .push
        .notify(&payload(NotifyKind::Permission))
        .await;
    assert_eq!(sent.dropped, 2);
    assert!(server.app.push.endpoints().is_empty());
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribe_checks_the_endpoint_and_keys_and_unsubscribe_drops_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (server, ep) = start(tmp.path(), PushConfig::default()).await;
    let keys = Browser::new().keys();
    let bad = [
        // The server POSTs there: no plain HTTP off this machine.
        PushSubscribeReq {
            endpoint: "http://192.0.2.1/push".into(),
            keys: keys.clone(),
            kinds: vec![],
        },
        PushSubscribeReq {
            endpoint: "https://push.example.com/x".into(),
            keys: cmux_proto::api::PushKeys {
                p256dh: "AAAA".into(),
                auth: keys.auth.clone(),
            },
            kinds: vec![],
        },
    ];
    for req in bad {
        let r = ep
            .json("POST", route::PUSH_SUBSCRIBE, &[], &req)
            .await
            .unwrap();
        assert_eq!(r.status, 400, "{}", r.text());
    }
    let endpoint = "https://push.example.com/x".to_string();
    let ok = PushSubscribeReq {
        endpoint: endpoint.clone(),
        keys,
        kinds: vec![NotifyKind::PlanDone],
    };
    // Subscribing twice keeps one subscription.
    for _ in 0..2 {
        let r = ep
            .json("POST", route::PUSH_SUBSCRIBE, &[], &ok)
            .await
            .unwrap();
        assert_eq!(r.status, 204, "{}", r.text());
    }
    assert_eq!(server.app.push.endpoints(), vec![endpoint.clone()]);
    let r = ep
        .json(
            "POST",
            route::PUSH_UNSUBSCRIBE,
            &[],
            &PushUnsubscribeReq { endpoint },
        )
        .await
        .unwrap();
    assert_eq!(r.status, 204, "{}", r.text());
    assert!(server.app.push.endpoints().is_empty());
    server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ntfy_sink_posts_each_notification() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockHttp::start().await;
    let push = PushConfig {
        ntfy: Some(format!("{}/cmux-topic", mock.base)),
        ntfy_token: Some("tk_secret".into()),
        subject: None,
    };
    let (server, _ep) = start(tmp.path(), push).await;
    mock.answer(200);
    let sent = server
        .app
        .push
        .notify(&payload(NotifyKind::Permission))
        .await;
    assert_eq!(sent.ntfy, Some(true));
    let hit = mock.wait_for(1).await;
    assert_eq!(hit.method, "POST");
    assert_eq!(hit.path, "/cmux-topic");
    assert_eq!(hit.header("title"), "cmux: Permission request");
    assert_eq!(hit.header("priority"), "high");
    assert_eq!(hit.header("tags"), "permission");
    assert_eq!(hit.header("authorization"), "Bearer tk_secret");
    assert_eq!(hit.body, b"executor: Bash cargo test");

    let done = PushPayload {
        kind: NotifyKind::PlanDone,
        item: None,
        plan: Some("P-20261018-x".into()),
        summary: "Plan done: x".into(),
    };
    server.app.push.notify(&done).await;
    let hit = mock.wait_for(2).await;
    assert_eq!(hit.header("title"), "cmux: Plan done");
    assert_eq!(hit.header("priority"), "default");

    // A failing topic is reported, not retried.
    mock.answer(503);
    let sent = server.app.push.notify(&done).await;
    assert_eq!(sent.ntfy, Some(false));
    // The topic URL (a secret) stays out of the diagnostics log.
    let log = std::fs::read_to_string(tmp.path().join("data/diagnostics/cmuxd.jsonl")).unwrap();
    assert!(log.contains("ntfy_failed"), "{log}");
    assert!(!log.contains("cmux-topic"), "{log}");
    server.shutdown().await;
}
