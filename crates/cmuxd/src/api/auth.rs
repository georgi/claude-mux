//! Authentication and the auth-related endpoints (architecture 9).
//!
//! The middleware turns a request into a [`Principal`] (or refuses it) and
//! applies the scope and CSRF rules; handlers read the principal from the
//! request extensions.

use axum::Extension;
use axum::Json;
use axum::Router;
use axum::extract::connect_info::{ConnectInfo, Connected};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::serve::IncomingStream;
use cmux_core::ids::ClientId;
use cmux_proto::api::{
    AUTHORIZATION, CSRF_HEADER, DevicesResp, ErrorCode, PairCompleteReq, PairCompleteResp,
    PairStartReq, PairStartResp, SESSION_COOKIE, Scope, route,
};
use tokio::net::UnixListener;

use super::{ApiError, ApiResult, AppState, Transport};
use crate::devices::PairError;

/// The uid of the process on the other end of a Unix socket connection.
#[derive(Debug, Clone, Copy)]
pub struct PeerCred {
    pub uid: Option<u32>,
}

impl Connected<IncomingStream<'_, UnixListener>> for PeerCred {
    fn connect_info(stream: IncomingStream<'_, UnixListener>) -> Self {
        PeerCred {
            uid: stream.io().peer_cred().ok().map(|c| c.uid()),
        }
    }
}

/// How the caller proved who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The Unix socket and a matching uid.
    Local,
    Bearer,
    Cookie,
}

/// Who is calling, as established by the middleware.
#[derive(Debug, Clone)]
pub struct Principal {
    /// `local`, or the device id.
    pub id: String,
    pub scope: Scope,
    pub via: Via,
}

impl Principal {
    /// The engine's name for this caller (shown as the actor of a change).
    pub fn client(&self) -> ClientId {
        match self.via {
            Via::Local => ClientId("local".into()),
            _ => ClientId(format!("dev-{}", self.id)),
        }
    }

    pub fn device_id(&self) -> Option<&str> {
        (self.via != Via::Local).then_some(self.id.as_str())
    }
}

fn mutating(m: &Method) -> bool {
    !matches!(*m, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Unix socket only: a peer with another uid gets nothing, not even the
/// public routes.
pub async fn uid_guard(req: Request, next: Next) -> Response {
    // SAFETY: geteuid has no preconditions.
    let ours = unsafe { libc::geteuid() };
    let peer = req
        .extensions()
        .get::<ConnectInfo<PeerCred>>()
        .and_then(|c| c.0.uid);
    if peer != Some(ours) {
        return ApiError::new(
            ErrorCode::Forbidden,
            "error: peer uid does not match the server",
        )
        .into_response();
    }
    next.run(req).await
}

fn bearer(h: &HeaderMap) -> Option<&str> {
    let v = h.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

fn cookie(h: &HeaderMap) -> Option<&str> {
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v)
        .filter(|v| !v.is_empty())
}

/// Authenticates, then enforces scope and CSRF.
pub async fn middleware(
    State((app, transport)): State<(AppState, Transport)>,
    mut req: Request,
    next: Next,
) -> Response {
    let principal = match authenticate(&app, transport, req.headers()) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if mutating(req.method()) {
        // Starting a session only mints a cookie; read-only devices may.
        let is_session = req.uri().path() == route::SESSION;
        if !principal.scope.can_write() && !is_session {
            return ApiError::new(ErrorCode::Forbidden, "error: this device is read-only")
                .into_response();
        }
        let csrf = req
            .headers()
            .get(CSRF_HEADER)
            .is_some_and(|v| !v.as_bytes().is_empty());
        if principal.via == Via::Cookie && !csrf {
            return ApiError::new(
                ErrorCode::Forbidden,
                format!("error: cookie sessions must send the {CSRF_HEADER} header"),
            )
            .into_response();
        }
    }
    req.extensions_mut().insert(principal);
    next.run(req).await
}

fn authenticate(app: &AppState, transport: Transport, h: &HeaderMap) -> ApiResult<Principal> {
    if transport == Transport::Unix {
        // `uid_guard` already ran.
        return Ok(Principal {
            id: "local".into(),
            scope: Scope::Full,
            via: Via::Local,
        });
    }
    let unauth = || ApiError::new(ErrorCode::Unauthorized, "error: a device token is required");
    if let Some(token) = bearer(h) {
        let d = app.devices.authenticate(token).ok_or_else(unauth)?;
        return Ok(Principal {
            id: d.id,
            scope: d.scope,
            via: Via::Bearer,
        });
    }
    if let Some(c) = cookie(h) {
        let d = app.devices.authenticate_session(c).ok_or_else(unauth)?;
        return Ok(Principal {
            id: d.id,
            scope: d.scope,
            via: Via::Cookie,
        });
    }
    Err(unauth())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(route::PAIR_START, post(pair_start))
        .route(route::DEVICES, get(list_devices))
        .route("/v1/devices/{id}", delete(revoke_device))
        .route(route::SESSION, post(start_session).delete(end_session))
}

/// `POST /v1/pair/start`: local, or a full-scope device.
async fn pair_start(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Option<Json<PairStartReq>>,
) -> ApiResult<Json<PairStartResp>> {
    if p.scope != Scope::Full {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "error: pairing needs a full-scope device or the local socket",
        ));
    }
    let scope = body.and_then(|b| b.0.scope).unwrap_or(Scope::Full);
    let code = app.pairing.start(scope);
    let base = app.base_url.read().map(|b| b.clone()).unwrap_or_default();
    app.diag.log(
        "info",
        "pair_start",
        serde_json::json!({"by": p.id, "scope": scope}),
    );
    Ok(Json(PairStartResp {
        url: format!("{base}/pair?code={code}"),
        code,
        expires_in_secs: app.pairing.ttl_secs(),
        scope,
    }))
}

/// `POST /v1/pair/complete`: no auth, rate limited.
pub async fn pair_complete(
    State(app): State<AppState>,
    Json(req): Json<PairCompleteReq>,
) -> ApiResult<Json<PairCompleteResp>> {
    let scope = match app.pairing.complete(&req.code) {
        Ok(s) => s,
        Err(PairError::RateLimited { retry_after_secs }) => {
            app.diag
                .log("warn", "pair_rate_limited", serde_json::json!({}));
            let mut e = ApiError::new(
                ErrorCode::RateLimited,
                "error: too many pairing attempts; wait and try again",
            );
            e.0.retry_after = Some(retry_after_secs);
            return Err(e);
        }
        Err(PairError::Invalid) => {
            app.diag.log("warn", "pair_failed", serde_json::json!({}));
            return Err(ApiError::new(
                ErrorCode::Unauthorized,
                "error: invalid or expired pairing code",
            ));
        }
    };
    if req.device_name.trim().is_empty() {
        return Err(ApiError::invalid("error: device_name must not be empty"));
    }
    let (device_id, token) = app.devices.create(&req.device_name, scope)?;
    app.diag.log(
        "info",
        "paired",
        serde_json::json!({"device": device_id, "scope": scope}),
    );
    Ok(Json(PairCompleteResp {
        device_id,
        token,
        scope,
        server: app.info.clone(),
    }))
}

async fn list_devices(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Json<DevicesResp> {
    Json(DevicesResp {
        devices: app.devices.list(p.device_id()),
    })
}

async fn revoke_device(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if app.devices.revoke(&id)? {
        app.diag.log(
            "info",
            "device_revoked",
            serde_json::json!({"device": id, "by": p.id}),
        );
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("error: no device {id}")))
    }
}

/// `POST /v1/session`: trade the bearer token for a cookie.
async fn start_session(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
) -> ApiResult<Response> {
    if p.via != Via::Bearer {
        return Err(ApiError::invalid(
            "error: send the device token as a bearer token to start a session",
        ));
    }
    let value = app.devices.create_session(&p.id)?;
    let max_age = crate::devices::SESSION_TTL_MS / 1000;
    let cookie = format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}"
    );
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|_| ApiError::invalid("bad cookie"))?,
    );
    Ok(resp)
}

async fn end_session(State(app): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(c) = cookie(&headers) {
        app.devices.drop_session(c);
    }
    let clear = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0");
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        clear.parse().map_err(|_| ApiError::invalid("bad cookie"))?,
    );
    Ok(resp)
}
