//! The client API: routers, shared state and the error type.
//!
//! [`router`] builds the same `/v1` API for both listeners; what differs is
//! the [`Transport`]. On the Unix socket the peer's uid must equal the
//! server's and no token is needed. On TCP every request but
//! `/v1/pair/complete` needs a device token (or a session cookie).
//! Later steps add routes (the stream, the agent endpoints) through
//! [`router_with`] and [`Extra`].

pub mod auth;
mod idem;
pub mod projects;
pub mod rest;
pub mod stream;
pub mod terminals;

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Instant;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use cmux_core::engine::EngineError;
use cmux_proto::api::{ErrorBody, ErrorCode, ServerInfo};

use crate::actor::{ActorGone, ActorHandle, Clock};
use crate::config::Config;
use crate::devices::{Devices, Pairing};
use crate::diag::Diag;
use crate::term::hub::Hub;

pub use auth::Principal;

const MAX_BODY: usize = 4 * 1024 * 1024;

/// Which listener a request came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// The Unix socket: uid-checked, no token.
    Unix,
    /// TCP: device token or session cookie.
    Tcp,
}

/// Everything handlers share. Cheap to clone.
#[derive(Clone)]
pub struct AppState(Arc<Shared>);

pub struct Shared {
    pub actor: ActorHandle,
    pub devices: Devices,
    pub pairing: Pairing,
    pub config: Config,
    pub clock: Clock,
    pub diag: Diag,
    pub started: Instant,
    pub info: ServerInfo,
    pub data_dir: std::path::PathBuf,
    /// Connected stream clients. Reported by `/status`.
    pub clients: AtomicU32,
    /// Who is connected to the stream, for `presence` messages.
    pub presence: stream::Presence,
    /// The PTYs. Shared with whoever spawns terminals (loose terminals here,
    /// agents in the effect runner); ids follow the engine's `TermId`s.
    pub hub: Arc<Hub>,
    /// The URL put into pairing QR codes; set once the TCP listener is bound.
    pub base_url: std::sync::RwLock<String>,
    idem: idem::Cache,
}

impl std::ops::Deref for AppState {
    type Target = Shared;
    fn deref(&self) -> &Shared {
        &self.0
    }
}

impl AppState {
    pub fn new(
        actor: ActorHandle,
        devices: Devices,
        config: Config,
        diag: Diag,
        data_dir: std::path::PathBuf,
        hub: Arc<Hub>,
    ) -> AppState {
        let clock = actor.clock().clone();
        let p = &config.pairing;
        let pairing = Pairing::new(clock.clone(), p.ttl_secs, p.attempts_per_min);
        let info = ServerInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            name: config.server_name(),
        };
        let base_url = std::sync::RwLock::new(config.base_url(None));
        AppState(Arc::new(Shared {
            actor,
            devices,
            pairing,
            config,
            idem: idem::Cache::new(clock.clone()),
            clock,
            diag,
            started: Instant::now(),
            info,
            data_dir,
            clients: AtomicU32::new(0),
            presence: stream::Presence::default(),
            hub,
            base_url,
        }))
    }
}

/// Every non-2xx response.
#[derive(Debug, Clone)]
pub struct ApiError(pub ErrorBody);

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> ApiError {
        ApiError(ErrorBody::new(code, message))
    }

    pub fn invalid(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::Invalid, message)
    }

    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(ErrorCode::NotFound, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.code.status()).unwrap_or(StatusCode::BAD_REQUEST);
        let mut resp = (status, axum::Json(&self.0)).into_response();
        if let Some(secs) = self.0.retry_after
            && let Ok(v) = secs.to_string().parse()
        {
            resp.headers_mut().insert("Retry-After", v);
        }
        resp
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        ApiError::new(e.kind.into(), e.to_string())
    }
}

impl From<ActorGone> for ApiError {
    fn from(e: ActorGone) -> Self {
        ApiError::new(ErrorCode::Internal, format!("error: {e}"))
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::new(ErrorCode::Internal, format!("error: {e}"))
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Routes a later step adds to the base API.
#[derive(Default)]
pub struct Extra {
    /// Behind device-token / cookie auth, scope checks and idempotency, on
    /// both listeners. Read the caller with `Extension<Principal>`.
    pub authed: Router<AppState>,
    /// Unix socket only, no client auth (the agent endpoints authenticate
    /// their own tokens).
    pub unix_only: Router<AppState>,
    /// Both listeners, no auth layer: the handler authenticates itself (the
    /// WebSocket takes its token in `hello`). The uid check still guards the
    /// Unix socket.
    pub open: Router<AppState>,
}

/// The `/v1` client API for one listener.
pub fn router(app: AppState, transport: Transport) -> Router {
    router_with(app, transport, Extra::default())
}

/// [`router`] plus the routes in `extra`.
pub fn router_with(app: AppState, transport: Transport, extra: Extra) -> Router {
    let public = Router::new().route(
        cmux_proto::api::route::PAIR_COMPLETE,
        post(auth::pair_complete),
    );
    // `layer` wraps what is already there, so the last one added runs first:
    // authenticate, then replay idempotent retries, then the handler.
    let protected = rest::routes()
        .merge(auth::routes())
        .merge(terminals::routes())
        .merge(projects::routes())
        .merge(extra.authed)
        .layer(middleware::from_fn_with_state(
            app.clone(),
            idem::middleware,
        ))
        .layer(middleware::from_fn_with_state(
            (app.clone(), transport),
            auth::middleware,
        ));
    let mut r = public
        .merge(protected)
        .merge(stream::routes(transport))
        .merge(extra.open);
    if transport == Transport::Unix {
        r = r.merge(extra.unix_only);
    }
    let r = r
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn(no_store))
        .with_state(app);
    if transport == Transport::Unix {
        r.layer(middleware::from_fn(auth::uid_guard))
    } else {
        r
    }
}

/// API responses must not be cached by proxies.
async fn no_store(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert("Cache-Control", "no-store".parse().expect("static header"));
    resp
}
