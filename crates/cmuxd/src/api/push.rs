//! `GET /v1/push/vapid`, `POST /v1/push/subscribe` and
//! `POST /v1/push/unsubscribe` (architecture 11.3). A subscription belongs
//! to the device that made it and goes when the device is revoked.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cmux_proto::api::{PushSubscribeReq, PushUnsubscribeReq, VapidResp, route};

use super::{ApiError, ApiResult, AppState, Principal};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(route::PUSH_VAPID, get(vapid))
        .route(route::PUSH_SUBSCRIBE, post(subscribe))
        .route(route::PUSH_UNSUBSCRIBE, post(unsubscribe))
}

async fn vapid(State(app): State<AppState>) -> Json<VapidResp> {
    Json(VapidResp {
        public_key: app.push.public_key(),
    })
}

async fn subscribe(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(req): Json<PushSubscribeReq>,
) -> ApiResult<StatusCode> {
    app.push
        .subscribe(&p.id, req, app.clock.now_ms())
        .map_err(ApiError::invalid)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unsubscribe(
    State(app): State<AppState>,
    Json(req): Json<PushUnsubscribeReq>,
) -> ApiResult<StatusCode> {
    app.push.unsubscribe(&req.endpoint)?;
    Ok(StatusCode::NO_CONTENT)
}
