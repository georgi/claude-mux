//! Loose terminals: `POST /v1/terminals`, `GET`/`DELETE /v1/terminals/{id}`
//! and the scrollback (architecture 6.1).
//!
//! The engine allocates the `TermId`; the hub names the PTY after it
//! ([`term_key`](crate::term::hub::term_key)). Creating registers the terminal
//! with the engine first and starts the PTY second, so a failed spawn is
//! rolled back with `CloseTerminal`.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cmux_core::engine::{ClientAction, Reply};
use cmux_core::ids::TermId;
use cmux_proto::CLAUDE_DEFAULT_ARGS;
use cmux_proto::api::{
    CreateTerminalReq, CreateTerminalResp, ErrorCode, NewTerminalKind, ScrollbackResp, route,
};
use cmux_proto::view::TerminalView;
use serde::Deserialize;

use super::auth::Principal;
use super::{ApiError, ApiResult, AppState};
use crate::agent::runner::running_exe;
use crate::claude::spawn;
use crate::store::atomic_write;
use crate::term::hub::{TermConfig, term_key};
use crate::term::pty::PtySpec;

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
/// Same cap as the stream's resize.
const MAX_DIM: u16 = 1000;
const DEFAULT_SCROLLBACK_LINES: usize = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(route::TERMINALS, post(create))
        .route("/v1/terminals/{id}", get(show).delete(close))
        .route("/v1/terminals/{id}/scrollback", get(scrollback))
}

/// What to run for a request.
/// `CLAUDE_DEFAULT_ARGS`, then `session_args`, go before a Claude terminal's
/// own arguments.
fn spec_for(
    req: &CreateTerminalReq,
    claude_bin: &str,
    session_args: &[String],
    cwd: PathBuf,
    rows: u16,
    cols: u16,
) -> ApiResult<PtySpec> {
    let mut spec = match &req.kind {
        NewTerminalKind::Claude { args } => {
            let mut s = PtySpec::new(claude_bin, &[]);
            s.args = CLAUDE_DEFAULT_ARGS
                .iter()
                .map(|a| a.to_string())
                .chain(session_args.iter().chain(args).cloned())
                .collect();
            s
        }
        NewTerminalKind::Shell => {
            let shell = std::env::var("SHELL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "/bin/sh".into());
            PtySpec::new(shell, &["-l"])
        }
        NewTerminalKind::Command { argv, .. } => {
            let (program, args) = argv
                .split_first()
                .filter(|(p, _)| !p.is_empty())
                .ok_or_else(|| ApiError::invalid("error: a command needs a non-empty argv"))?;
            let mut s = PtySpec::new(program.clone(), &[]);
            s.args = args.to_vec();
            s
        }
    };
    spec.cwd = Some(cwd);
    spec.rows = rows;
    spec.cols = cols;
    Ok(spec)
}

/// Writes the session MCP config (cheap, and it follows a moved binary or
/// socket) and returns the flags that load it.
async fn session_tools(app: &AppState) -> ApiResult<Vec<String>> {
    let file = spawn::session_mcp_file(&app.data_dir);
    let json = spawn::session_mcp_json(&running_exe(), &app.config.socket_path());
    let path = file.clone();
    tokio::task::spawn_blocking(move || atomic_write(&path, json.as_bytes()))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, format!("error: {e}")))??;
    Ok(spawn::session_args(&file))
}

fn parse_id(s: &str) -> ApiResult<TermId> {
    s.parse::<u32>()
        .map(TermId)
        .map_err(|_| ApiError::invalid(format!("error: {s:?} is not a valid terminal id")))
}

async fn view(app: &AppState, id: TermId) -> ApiResult<TerminalView> {
    app.actor
        .query(move |s| s.terminals.get(&id).map(TerminalView::from))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("error: no terminal {id}")))
}

async fn create(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(req): Json<CreateTerminalReq>,
) -> ApiResult<(StatusCode, Json<CreateTerminalResp>)> {
    let cwd = match &req.cwd {
        Some(c) => PathBuf::from(c),
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/".into()),
    };
    if !cwd.is_dir() {
        return Err(ApiError::invalid(format!(
            "error: cwd {} is not a directory",
            cwd.display()
        )));
    }
    let (rows, cols) = (
        req.rows.unwrap_or(DEFAULT_ROWS).max(1),
        req.cols.unwrap_or(DEFAULT_COLS).max(1),
    );
    if rows > MAX_DIM || cols > MAX_DIM {
        return Err(ApiError::invalid("error: rows and cols must be 1..=1000"));
    }
    let session_args = match req.kind {
        NewTerminalKind::Claude { .. } => session_tools(&app).await?,
        _ => Vec::new(),
    };
    let spec = spec_for(
        &req,
        &app.config.agents.claude_bin,
        &session_args,
        cwd.clone(),
        rows,
        cols,
    )?;
    let reply = app
        .actor
        .client(
            p.client(),
            ClientAction::CreateTerminal {
                kind: req.kind.term_kind(),
                cwd,
                size: (cols, rows),
            },
        )
        .await??;
    let id = match reply {
        Reply::Text(t) => parse_id(&t)?,
        other => {
            return Err(ApiError::new(
                ErrorCode::Internal,
                format!("error: unexpected engine reply {other:?}"),
            ));
        }
    };
    let hub = app.hub.clone();
    let key = term_key(id);
    let spawned =
        tokio::task::spawn_blocking(move || hub.create(&key, &spec, &TermConfig::small()))
            .await
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("error: {e}")))?;
    if let Err(e) = spawned {
        let _ = app
            .actor
            .client(p.client(), ClientAction::CloseTerminal { term: id })
            .await;
        return Err(ApiError::invalid(format!("error: {e:#}")));
    }
    Ok((StatusCode::CREATED, Json(view(&app, id).await?)))
}

async fn show(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TerminalView>> {
    Ok(Json(view(&app, parse_id(&id)?).await?))
}

/// Closes a loose terminal and kills its program. Agent terminals belong to
/// their agent: stop the agent instead.
async fn close(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let term = parse_id(&id)?;
    app.actor
        .client(p.client(), ClientAction::CloseTerminal { term })
        .await??;
    app.hub.remove(&term_key(term));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ScrollbackQuery {
    lines: Option<usize>,
    /// Lines to skip above the live screen, to page back through history.
    offset: Option<usize>,
}

async fn scrollback(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ScrollbackQuery>,
) -> ApiResult<Json<ScrollbackResp>> {
    let term = parse_id(&id)?;
    let t = app
        .hub
        .get_term(term)
        .ok_or_else(|| ApiError::not_found(format!("error: no terminal {term}")))?;
    let lines = q.lines.unwrap_or(DEFAULT_SCROLLBACK_LINES).min(100_000);
    let offset = q.offset.unwrap_or(0);
    let sb = tokio::task::spawn_blocking(move || t.scrollback(lines, offset))
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, format!("error: {e}")))?;
    Ok(Json(ScrollbackResp {
        term,
        lines: sb.lines,
        total: sb.total as u32,
    }))
}
