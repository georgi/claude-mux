//! A mock HTTP server that records every request and answers with a settable
//! status: a push service or an ntfy server. Included with `#[path]`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use ring::agreement;
use ring::rand::{SecureRandom, SystemRandom};

#[derive(Debug, Clone)]
pub struct Hit {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Hit {
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

#[derive(Clone)]
pub struct MockHttp {
    /// `http://127.0.0.1:<port>`.
    pub base: String,
    hits: Arc<Mutex<Vec<Hit>>>,
    status: Arc<AtomicU16>,
}

impl MockHttp {
    pub async fn start() -> MockHttp {
        let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
        let status = Arc::new(AtomicU16::new(201));
        let (h, s) = (hits.clone(), status.clone());
        let app = Router::new().fallback(
            move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
                let (h, s) = (h.clone(), s.clone());
                async move {
                    h.lock().unwrap().push(Hit {
                        method,
                        path: uri.path().to_string(),
                        headers,
                        body: body.to_vec(),
                    });
                    StatusCode::from_u16(s.load(Ordering::SeqCst)).unwrap()
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        MockHttp { base, hits, status }
    }

    /// The status later requests get.
    pub fn answer(&self, code: u16) {
        self.status.store(code, Ordering::SeqCst);
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    /// Waits for the `n`th request (1-based).
    pub async fn wait_for(&self, n: usize) -> Hit {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(h) = self.hits().get(n - 1) {
                return h.clone();
            }
            assert!(Instant::now() < until, "timed out waiting for request {n}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// A browser's push keys: the private half, `p256dh` and `auth` (raw).
pub struct Browser {
    pub key: agreement::EphemeralPrivateKey,
    pub p256dh: Vec<u8>,
    pub auth: [u8; 16],
}

impl Browser {
    pub fn new() -> Browser {
        let rng = SystemRandom::new();
        let key = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let p256dh = key.compute_public_key().unwrap().as_ref().to_vec();
        let mut auth = [0u8; 16];
        rng.fill(&mut auth).unwrap();
        Browser { key, p256dh, auth }
    }

    pub fn keys(&self) -> cmux_proto::api::PushKeys {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        cmux_proto::api::PushKeys {
            p256dh: b64.encode(&self.p256dh),
            auth: b64.encode(self.auth),
        }
    }

    /// Decrypts a push body; the key is single use.
    pub fn open(self, body: &[u8]) -> cmux_proto::api::PushPayload {
        let plain = cmuxd::push::crypto::decrypt(body, self.key, &self.p256dh, &self.auth).unwrap();
        serde_json::from_slice(&plain).unwrap()
    }
}
