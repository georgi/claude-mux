//! `Idempotency-Key` handling (architecture 6.1).
//!
//! A mutating request that carries the header is run once per
//! (caller, key). A retry with the same key, method, path and body gets the
//! stored response back (marked `Idempotent-Replayed: true`); a retry that
//! differs gets 422. A retry that arrives while the first is still running
//! waits for it. Server errors are not remembered, so they can be retried.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cmux_core::model::Timestamp;
use cmux_proto::api::{ErrorCode, IDEMPOTENCY_KEY};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use super::{ApiError, AppState, Principal};
use crate::actor::Clock;

const TTL_MS: u64 = 10 * 60 * 1000;
const MAX_ENTRIES: usize = 2048;
const MAX_KEY_LEN: usize = 200;
const MAX_BODY: usize = 4 * 1024 * 1024;

struct Stored {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

struct Entry {
    fingerprint: [u8; 32],
    at: Timestamp,
    cell: Arc<OnceCell<Stored>>,
}

pub struct Cache {
    clock: Clock,
    map: Mutex<HashMap<(String, String), Entry>>,
}

impl Cache {
    pub fn new(clock: Clock) -> Cache {
        Cache {
            clock,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// The cell for this key, or `None` if the key was used for another request.
    fn slot(&self, key: (String, String), fingerprint: [u8; 32]) -> Option<Arc<OnceCell<Stored>>> {
        let now = self.clock.now_ms();
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, e| now.saturating_sub(e.at) < TTL_MS);
        if let Some(e) = m.get(&key) {
            return (e.fingerprint == fingerprint).then(|| e.cell.clone());
        }
        if m.len() >= MAX_ENTRIES
            && let Some(oldest) = m.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
        let cell = Arc::new(OnceCell::new());
        m.insert(
            key,
            Entry {
                fingerprint,
                at: now,
                cell: cell.clone(),
            },
        );
        Some(cell)
    }

    fn forget(&self, key: &(String, String)) {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

pub async fn middleware(State(app): State<AppState>, req: Request, next: Next) -> Response {
    let key = match req.headers().get(IDEMPOTENCY_KEY) {
        Some(v) if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) => {
            match v.to_str() {
                Ok(k) if !k.is_empty() && k.len() <= MAX_KEY_LEN => k.to_string(),
                _ => {
                    return ApiError::invalid("error: bad Idempotency-Key header").into_response();
                }
            }
        }
        _ => return next.run(req).await,
    };
    let who = req
        .extensions()
        .get::<Principal>()
        .map_or_else(String::new, |p| p.id.clone());
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_BODY).await else {
        return ApiError::invalid("error: request body too large").into_response();
    };
    let mut h = Sha256::new();
    h.update(parts.method.as_str().as_bytes());
    h.update([0]);
    h.update(parts.uri.to_string().as_bytes());
    h.update([0]);
    h.update(&bytes);
    let fingerprint: [u8; 32] = h.finalize().into();

    let key = (who, key);
    let Some(cell) = app.idem.slot(key.clone(), fingerprint) else {
        return ApiError::new(
            ErrorCode::IdempotencyMismatch,
            "error: this Idempotency-Key was already used for a different request",
        )
        .into_response();
    };
    let mut ran = false;
    let stored = cell
        .get_or_init(|| async {
            ran = true;
            let resp = next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
            let (p, b) = resp.into_parts();
            let body = to_bytes(b, usize::MAX).await.unwrap_or_default();
            Stored {
                status: p.status,
                headers: p.headers,
                body,
            }
        })
        .await;
    if stored.status.is_server_error() {
        app.idem.forget(&key);
    }
    let mut resp = Response::new(Body::from(stored.body.clone()));
    *resp.status_mut() = stored.status;
    for name in [
        header::CONTENT_TYPE,
        header::LOCATION,
        header::SET_COOKIE,
        header::RETRY_AFTER,
    ] {
        if let Some(v) = stored.headers.get(&name) {
            resp.headers_mut().insert(name, v.clone());
        }
    }
    if !ran {
        resp.headers_mut()
            .insert("Idempotent-Replayed", HeaderValue::from_static("true"));
    }
    resp
}
