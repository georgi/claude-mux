//! The REST endpoints that map onto engine commands (architecture 6.1):
//! state, status, plans, tasks, agent control, inbox answers, the known
//! repositories and the read-only settings.
//!
//! Terminals, the stream and the agent endpoints belong to later steps.

use std::path::{Path as FsPath, PathBuf};
use std::str::FromStr;
use std::sync::atomic::Ordering;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use cmux_core::engine::{ClientAction, Reply, TaskPatch};
use cmux_core::ids::{AgentId, ItemId, PlanId, TaskId, TermId};
use cmux_core::model::{Level, Repo};
use cmux_proto::api::{
    AddRepoReq, AgentStatus, AnswerReq, CreatePlanReq, CreateTaskReq, Created, LevelSettings,
    MessageReq, PatchTaskReq, PlanBody, RepoInfo, ReposResp, SettingsResp, SpawnTaskReq,
    StatusResp, TranscriptResp, route,
};
use cmux_proto::view::{ChatEntry, StateView, TaskDetail};

use super::auth::Principal;
use super::{ApiError, ApiResult, AppState};
use crate::claude::transcript::{PAGE_DEFAULT, PAGE_MAX};
use crate::config::LevelConfig;
use crate::store::atomic_write;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(route::STATE, get(state))
        .route(route::STATUS, get(status))
        .route(route::PLANS, post(create_plan))
        .route("/v1/plans/{plan}/accept", post(accept))
        .route("/v1/plans/{plan}/pause", post(pause))
        .route("/v1/plans/{plan}/resume", post(resume))
        .route("/v1/plans/{plan}/cancel", post(cancel))
        .route("/v1/plans/{plan}", delete(delete_plan))
        .route("/v1/plans/{plan}/body", get(get_body).put(put_body))
        .route("/v1/plans/{plan}/tasks", post(create_task))
        .route("/v1/tasks/{plan}/{task}", get(get_task).patch(patch_task))
        .route("/v1/tasks/{plan}/{task}/spawn", post(spawn_task))
        .route("/v1/agents/{agent}/messages", post(message))
        .route("/v1/agents/{agent}/stop", post(stop_agent))
        .route("/v1/agents/{agent}/resume", post(resume_agent))
        .route("/v1/agents/{agent}/transcript", get(transcript))
        .route(
            "/v1/agents/{agent}/transcript/{uuid}",
            get(transcript_entry),
        )
        .route(route::ITEMS_ANSWER, post(answer))
        .route(route::REPOS, get(list_repos).post(add_repo))
        .route(route::SETTINGS, get(settings))
}

fn parse<T: FromStr>(what: &str, s: &str) -> ApiResult<T> {
    s.parse()
        .map_err(|_| ApiError::invalid(format!("error: {s:?} is not a valid {what} id")))
}

/// An item id as the UI shows it (`I-5`) or as the bare number (`5`).
fn parse_item(s: &str) -> ApiResult<ItemId> {
    match s.parse::<u64>() {
        Ok(n) if s.bytes().all(|b| b.is_ascii_digit()) => Ok(ItemId(n)),
        _ => parse("item", s),
    }
}

/// A task id as shown (`T-0001`) or as the bare number (`1`).
fn parse_task(s: &str) -> ApiResult<TaskId> {
    match s.parse::<u32>() {
        Ok(n) if s.bytes().all(|b| b.is_ascii_digit()) => Ok(TaskId::new(n)),
        _ => parse("task", s),
    }
}

/// Runs a client action as the caller.
async fn act(app: &AppState, p: &Principal, action: ClientAction) -> ApiResult<Reply> {
    Ok(app.actor.client(p.client(), action).await??)
}

fn text(r: Reply) -> ApiResult<String> {
    match r {
        Reply::Text(t) => Ok(t),
        other => Err(ApiError::new(
            cmux_proto::api::ErrorCode::Internal,
            format!("error: unexpected engine reply {other:?}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// State and status
// ---------------------------------------------------------------------------

async fn state(State(app): State<AppState>) -> ApiResult<Json<StateView>> {
    Ok(Json(app.actor.state_view().await?))
}

async fn status(State(app): State<AppState>) -> ApiResult<Json<StatusResp>> {
    let (rev, live, agents) = app
        .actor
        .query(|s| {
            let agents: Vec<(AgentId, Option<TermId>)> = s
                .plans
                .iter()
                .flat_map(|p| &p.agents)
                .filter(|a| a.is_live())
                .map(|a| (a.id.clone(), a.term))
                .collect();
            (s.rev, agents.len() as u32, agents)
        })
        .await?;
    Ok(Json(StatusResp {
        server: app.info.clone(),
        uptime_secs: app.started.elapsed().as_secs(),
        rev,
        load: load_average(),
        live_processes: live,
        clients: app.clients.load(Ordering::Relaxed),
        agents: agents
            .into_iter()
            .map(|(agent, term)| AgentStatus {
                agent,
                rss_kb: term
                    .and_then(|t| app.hub.get_term(t))
                    .and_then(|t| t.pid())
                    .and_then(rss_kb),
            })
            .collect(),
        github: app.github.get(),
    }))
}

/// `VmRSS` from `/proc/<pid>/status`, in kB. Linux only.
#[cfg(target_os = "linux")]
fn rss_kb(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    parse_vm_rss(&status)
}

#[cfg(not(target_os = "linux"))]
fn rss_kb(_pid: u32) -> Option<u64> {
    None
}

/// The `VmRSS:   1234 kB` line of a `/proc/<pid>/status` file.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_vm_rss(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    line["VmRSS:".len()..]
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn load_average() -> Option<[f32; 3]> {
    let mut l = [0f64; 3];
    // SAFETY: `l` has room for the 3 samples we ask for.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    (n == 3).then(|| [l[0] as f32, l[1] as f32, l[2] as f32])
}

// ---------------------------------------------------------------------------
// Plans
// ---------------------------------------------------------------------------

async fn create_plan(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(req): Json<CreatePlanReq>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let mut repos = Vec::new();
    for spec in req.repos {
        let path = PathBuf::from(&spec.path);
        if !path.is_absolute() || !path.is_dir() {
            return Err(ApiError::invalid(format!(
                "error: repo path {} must be an existing absolute directory",
                spec.path
            )));
        }
        let base_branch = match spec.base_branch.filter(|b| !b.is_empty()) {
            Some(b) => b,
            None => default_branch(&path).await,
        };
        repos.push(Repo {
            path,
            base_branch,
            remote_url: None,
        });
    }
    let id = text(
        act(
            &app,
            &p,
            ClientAction::CreatePlan {
                title: req.title,
                repos,
                request: req.request,
            },
        )
        .await?,
    )?;
    Ok((StatusCode::CREATED, Json(Created { id })))
}

/// The first line of `git -C <repo> <args>`, if it succeeds and says
/// something other than `HEAD`.
async fn git_line(repo: &FsPath, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty() && s != "HEAD").then_some(s)
}

/// The branch a repo's `origin/HEAD` points at, else the checked-out branch,
/// else `main`.
async fn default_branch(repo: &FsPath) -> String {
    if let Some(b) = git_line(
        repo,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        return b.strip_prefix("origin/").unwrap_or(&b).to_string();
    }
    git_line(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .unwrap_or_else(|| "main".into())
}

macro_rules! plan_action {
    ($name:ident, $variant:ident) => {
        async fn $name(
            State(app): State<AppState>,
            Extension(p): Extension<Principal>,
            Path(plan): Path<String>,
        ) -> ApiResult<StatusCode> {
            let plan: PlanId = parse("plan", &plan)?;
            act(&app, &p, ClientAction::$variant { plan }).await?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}
plan_action!(accept, AcceptPlan);
plan_action!(pause, PausePlan);
plan_action!(resume, ResumePlan);
plan_action!(cancel, CancelPlan);
plan_action!(delete_plan, DeletePlan);

async fn require_plan(app: &AppState, plan: &PlanId) -> ApiResult<PathBuf> {
    let id = plan.clone();
    let exists = app
        .actor
        .query(move |s| s.plans.iter().any(|p| p.id == id))
        .await?;
    if !exists {
        return Err(ApiError::not_found(format!("error: no plan {plan}")));
    }
    Ok(app
        .data_dir
        .join("plans")
        .join(plan.as_str())
        .join("plan.md"))
}

async fn get_body(
    State(app): State<AppState>,
    Path(plan): Path<String>,
) -> ApiResult<Json<PlanBody>> {
    let file = require_plan(&app, &parse("plan", &plan)?).await?;
    match tokio::fs::read_to_string(&file).await {
        Ok(text) => Ok(Json(PlanBody { text })),
        // The executor has not written it yet.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Json(PlanBody {
            text: String::new(),
        })),
        Err(e) => Err(e.into()),
    }
}

async fn put_body(
    State(app): State<AppState>,
    Path(plan): Path<String>,
    Json(body): Json<PlanBody>,
) -> ApiResult<StatusCode> {
    let file = require_plan(&app, &parse("plan", &plan)?).await?;
    tokio::task::spawn_blocking(move || atomic_write(&file, body.text.as_bytes()))
        .await
        .map_err(|e| {
            ApiError::new(cmux_proto::api::ErrorCode::Internal, format!("error: {e}"))
        })??;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

async fn create_task(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(plan): Path<String>,
    Json(req): Json<CreateTaskReq>,
) -> ApiResult<(StatusCode, Json<Created>)> {
    let plan: PlanId = parse("plan", &plan)?;
    // The engine says `created T-0001`; the id is the last word.
    let reply = text(
        act(
            &app,
            &p,
            ClientAction::CreateTask {
                plan,
                title: req.title,
                body: req.body,
                repo: req.repo,
                deps: req.deps,
                criteria: req.criteria,
            },
        )
        .await?,
    )?;
    let id = reply.rsplit(' ').next().unwrap_or(&reply).to_string();
    Ok((StatusCode::CREATED, Json(Created { id })))
}

async fn task_detail(app: &AppState, plan: PlanId, task: TaskId) -> ApiResult<TaskDetail> {
    let found = app
        .actor
        .query(move |s| {
            let p = s.plans.iter().find(|p| p.id == plan)?;
            Some(TaskDetail::new(p, p.task(task)?))
        })
        .await?;
    found.ok_or_else(|| ApiError::not_found("error: no such task"))
}

async fn get_task(
    State(app): State<AppState>,
    Path((plan, task)): Path<(String, String)>,
) -> ApiResult<Json<TaskDetail>> {
    Ok(Json(
        task_detail(&app, parse("plan", &plan)?, parse_task(&task)?).await?,
    ))
}

async fn patch_task(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((plan, task)): Path<(String, String)>,
    Json(req): Json<PatchTaskReq>,
) -> ApiResult<Json<TaskDetail>> {
    let plan: PlanId = parse("plan", &plan)?;
    let task: TaskId = parse_task(&task)?;
    let patch = TaskPatch {
        title: req.title,
        body: req.body,
        repo: req.repo,
        deps: req.deps,
    };
    let has_fields = patch != TaskPatch::default();
    if has_fields {
        let action = ClientAction::UpdateTask {
            plan: plan.clone(),
            task,
            patch,
        };
        act(&app, &p, action).await?;
    }
    if let Some(to) = req.state {
        let action = ClientAction::TransitionTask {
            plan: plan.clone(),
            task,
            to,
        };
        act(&app, &p, action).await?;
    }
    Ok(Json(task_detail(&app, plan, task).await?))
}

async fn spawn_task(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((plan, task)): Path<(String, String)>,
    body: Option<Json<SpawnTaskReq>>,
) -> ApiResult<StatusCode> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let action = ClientAction::SpawnTask {
        plan: parse("plan", &plan)?,
        task: parse_task(&task)?,
        instructions: req.instructions,
        force: req.force,
    };
    act(&app, &p, action).await?;
    Ok(StatusCode::ACCEPTED)
}

// ---------------------------------------------------------------------------
// Agents and items
// ---------------------------------------------------------------------------

async fn message(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(agent): Path<String>,
    Json(req): Json<MessageReq>,
) -> ApiResult<StatusCode> {
    let action = ClientAction::SendMessage {
        agent: parse("agent", &agent)?,
        text: req.text,
        steer: req.steer,
    };
    act(&app, &p, action).await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(serde::Deserialize)]
struct TranscriptQuery {
    after: Option<String>,
    limit: Option<usize>,
}

/// The agent's conversation (architecture 5.3), read up to now.
async fn transcript(
    State(app): State<AppState>,
    Path(agent): Path<String>,
    Query(q): Query<TranscriptQuery>,
) -> ApiResult<Json<TranscriptResp>> {
    let agent: AgentId = parse("agent", &agent)?;
    let limit = q.limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
    let entries = app
        .transcripts
        .page(&app.actor, &agent, q.after.as_deref(), limit)
        .await
        .ok_or_else(|| ApiError::not_found(format!("error: no agent {agent}")))?;
    Ok(Json(TranscriptResp { agent, entries }))
}

/// One entry of the agent's conversation with its text uncut (1 MB at most).
async fn transcript_entry(
    State(app): State<AppState>,
    Path((agent, uuid)): Path<(String, String)>,
) -> ApiResult<Json<ChatEntry>> {
    let agent: AgentId = parse("agent", &agent)?;
    app.transcripts
        .entry(&app.actor, &agent, &uuid)
        .await
        .ok_or_else(|| ApiError::not_found(format!("error: no agent {agent}")))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("error: no entry {uuid} for {agent}")))
}

async fn stop_agent(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(agent): Path<String>,
) -> ApiResult<StatusCode> {
    let action = ClientAction::StopAgent {
        agent: parse("agent", &agent)?,
    };
    act(&app, &p, action).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resume_agent(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(agent): Path<String>,
) -> ApiResult<StatusCode> {
    let action = ClientAction::ResumeAgent {
        agent: parse("agent", &agent)?,
    };
    act(&app, &p, action).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn answer(
    State(app): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(item): Path<String>,
    Json(req): Json<AnswerReq>,
) -> ApiResult<StatusCode> {
    let action = req.into_action().map_err(ApiError::invalid)?;
    act(
        &app,
        &p,
        ClientAction::AnswerItem {
            item: parse_item(&item)?,
            action,
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Repositories and settings
// ---------------------------------------------------------------------------

/// Guards the read-modify-write of `repos.json`.
static REGISTRY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `<data_dir>/repos.json`: the absolute paths added with `POST /repos`.
fn registry_file(app: &AppState) -> PathBuf {
    app.data_dir.join("repos.json")
}

async fn read_registry(file: &FsPath) -> std::io::Result<Vec<PathBuf>> {
    match tokio::fs::read(file).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Adds `path` to the registry unless it is there already.
async fn register(file: PathBuf, path: PathBuf) -> ApiResult<()> {
    let _guard = REGISTRY.lock().await;
    let mut paths = read_registry(&file).await?;
    if paths.contains(&path) {
        return Ok(());
    }
    paths.push(path);
    let bytes = serde_json::to_vec_pretty(&paths).map_err(std::io::Error::from)?;
    tokio::task::spawn_blocking(move || atomic_write(&file, &bytes))
        .await
        .map_err(joined)??;
    Ok(())
}

fn joined(e: tokio::task::JoinError) -> ApiError {
    ApiError::new(cmux_proto::api::ErrorCode::Internal, format!("error: {e}"))
}

async fn repo_info(path: &FsPath) -> RepoInfo {
    RepoInfo {
        path: path.display().to_string(),
        base_branch: default_branch(path).await,
        remote_url: git_line(path, &["remote", "get-url", "origin"]).await,
    }
}

/// The plans' repositories and the registered ones, by canonical path.
async fn list_repos(State(app): State<AppState>) -> ApiResult<Json<ReposResp>> {
    let planned: Vec<RepoInfo> = app
        .actor
        .query(|s| {
            s.plans
                .iter()
                .flat_map(|p| &p.repos)
                .map(|r| RepoInfo {
                    path: r.path.display().to_string(),
                    base_branch: r.base_branch.clone(),
                    remote_url: r.remote_url.clone(),
                })
                .collect()
        })
        .await?;
    let mut repos = std::collections::BTreeMap::new();
    for mut info in planned {
        // A plan keeps its path as given; a deleted repo keeps it as stored.
        if let Ok(p) = tokio::fs::canonicalize(&info.path).await {
            info.path = p.display().to_string();
        }
        repos.entry(info.path.clone()).or_insert(info);
    }
    for path in read_registry(&registry_file(&app)).await? {
        let key = path.display().to_string();
        // A registered repository may have been deleted since.
        if !repos.contains_key(&key) && path.is_dir() {
            repos.insert(key, repo_info(&path).await);
        }
    }
    Ok(Json(ReposResp {
        repos: repos.into_values().collect(),
    }))
}

async fn add_repo(
    State(app): State<AppState>,
    Json(req): Json<AddRepoReq>,
) -> ApiResult<(StatusCode, Json<RepoInfo>)> {
    let path = match (req.url, req.path) {
        (Some(url), None) => clone_repo(&app.data_dir, url).await?,
        (None, Some(path)) => existing_repo(&path).await?,
        _ => return Err(ApiError::invalid("error: give exactly one of url and path")),
    };
    register(registry_file(&app), path.clone()).await?;
    Ok((StatusCode::CREATED, Json(repo_info(&path).await)))
}

/// The directory a clone of `url` gets: its last path segment without `.git`.
fn clone_name(url: &str) -> Option<&str> {
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    let bad = name.is_empty() || name == "." || name == ".." || name.contains('\\');
    (!bad).then_some(name)
}

/// `git clone <url>` into `<data_dir>/repos/<name>`, on the blocking pool.
async fn clone_repo(data_dir: &FsPath, url: String) -> ApiResult<PathBuf> {
    if url.is_empty() || url.starts_with('-') {
        return Err(ApiError::invalid(format!(
            "error: {url:?} is not a git URL"
        )));
    }
    let name = clone_name(&url)
        .ok_or_else(|| ApiError::invalid(format!("error: no repository name in {url:?}")))?;
    let dest = data_dir.join("repos").join(name);
    tokio::task::spawn_blocking(move || -> ApiResult<PathBuf> {
        crate::config::ensure_private_dir(dest.parent().unwrap_or(&dest))?;
        // Creating it first claims the name against a concurrent clone.
        match std::fs::create_dir(&dest) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(ApiError::invalid(format!(
                    "error: {} already exists",
                    dest.display()
                )));
            }
            r => r?,
        }
        let out = std::process::Command::new("git")
            .args(["clone", "--quiet", "--"])
            .arg(&url)
            .arg(&dest)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output();
        match out {
            Ok(out) if out.status.success() => Ok(dest.canonicalize()?),
            out => {
                let _ = std::fs::remove_dir_all(&dest);
                let msg = match out {
                    Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
                    Err(e) => format!("error: cannot run git: {e}"),
                };
                Err(ApiError::invalid(if msg.is_empty() {
                    format!("error: git clone {url} failed")
                } else {
                    msg
                }))
            }
        }
    })
    .await
    .map_err(joined)?
}

/// The root of the repository at `path` on the server, canonicalized.
async fn existing_repo(path: &str) -> ApiResult<PathBuf> {
    let p = PathBuf::from(path);
    if !p.is_absolute() {
        return Err(ApiError::invalid(format!(
            "error: repo path {path} must be absolute"
        )));
    }
    let top = git_line(&p, &["rev-parse", "--show-toplevel"])
        .await
        .ok_or_else(|| ApiError::invalid(format!("error: {path} is not a git repository")))?;
    Ok(tokio::fs::canonicalize(&top).await?)
}

/// What `config.toml` sets; an unset permission mode is `auto`.
async fn settings(State(app): State<AppState>) -> Json<SettingsResp> {
    let c = &app.config;
    let level = |level, l: &LevelConfig| LevelSettings {
        level,
        model: l.model.clone(),
        permission_mode: l.permission_mode.clone().unwrap_or_else(|| "auto".into()),
    };
    Json(SettingsResp {
        tasks_per_plan: c.limits.tasks_per_plan,
        subagents_per_task: c.limits.subagents_per_task,
        live_processes: c.limits.live_processes,
        permission_wait_secs: c
            .timeouts
            .permission_wait_secs
            .unwrap_or(c.agents.permission_wait_secs),
        claude: vec![
            level(Level::Executor, &c.agents.executor),
            level(Level::Task, &c.agents.task),
            level(Level::Sub, &c.agents.sub),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_take_the_bare_number_and_the_prefixed_form() {
        assert_eq!(parse_item("5").unwrap(), ItemId(5));
        assert_eq!(parse_item("I-5").unwrap(), ItemId(5));
        for bad in ["", "I-", "I5", "-5", "+5", "I-+5", "five", "5x"] {
            assert!(parse_item(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn task_ids_take_the_bare_number_and_the_prefixed_form() {
        assert_eq!(parse_task("1").unwrap(), TaskId::new(1));
        assert_eq!(parse_task("T-0001").unwrap(), TaskId::new(1));
        for bad in ["", "T-", "T-1", "t-0001", "-1", "+1", "x"] {
            assert!(parse_task(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn clone_names_are_the_last_segment_without_git() {
        assert_eq!(clone_name("https://github.com/a/b.git"), Some("b"));
        assert_eq!(clone_name("https://github.com/a/b/"), Some("b"));
        assert_eq!(clone_name("git@github.com:a/b.git"), Some("b"));
        assert_eq!(clone_name("host:b.git/"), Some("b"));
        assert_eq!(clone_name("/tmp/origin.git"), Some("origin"));
        for bad in [
            "",
            "/",
            "https://h/a/.git",
            "https://h/..",
            "https://h/.",
            "a\\b",
        ] {
            assert_eq!(clone_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn vm_rss_is_read_from_a_proc_status_file() {
        let status = "Name:\tclaude\nVmPeak:\t  900 kB\nVmRSS:\t  123456 kB\nThreads:\t9\n";
        assert_eq!(parse_vm_rss(status), Some(123_456));
        assert_eq!(parse_vm_rss("Name:\tzombie\n"), None);
    }

    #[test]
    fn rss_is_only_known_on_linux() {
        let me = std::process::id();
        assert_eq!(rss_kb(me).is_some(), cfg!(target_os = "linux"));
    }
}
