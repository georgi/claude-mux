//! `GET /v1/projects`: the server user's past Claude sessions, for the
//! client's session picker (the same list `cmux` shows locally).

use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use axum::routing::get;
use axum::{Json, Router};
use cmux_history::{Index, SessionInfo, scan};
use cmux_proto::api::{ErrorCode, ProjectsResp, SessionSummary, route};

use super::{ApiError, ApiResult, AppState};

pub fn routes() -> Router<AppState> {
    Router::new().route(route::PROJECTS, get(list))
}

/// Parsed transcripts are cached between requests (keyed by mtime and size).
fn index() -> &'static Arc<Mutex<Index>> {
    static INDEX: OnceLock<Arc<Mutex<Index>>> = OnceLock::new();
    INDEX.get_or_init(Arc::default)
}

pub fn summary(s: SessionInfo) -> SessionSummary {
    SessionSummary {
        id: s.id,
        cwd: s.cwd.display().to_string(),
        title: s.title,
        first_prompt: s.first_prompt,
        last_prompt: s.last_prompt,
        modified: s
            .modified
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        branch: s.branch,
    }
}

async fn list() -> ApiResult<Json<ProjectsResp>> {
    let idx = index().clone();
    let sessions = tokio::task::spawn_blocking(move || scan(&idx))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, format!("error: {e}")))?;
    Ok(Json(ProjectsResp {
        sessions: sessions.into_iter().map(summary).collect(),
    }))
}
