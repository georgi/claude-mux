//! The agent effect runner (architecture 7.1 and 8.1).
//!
//! [`AgentRuntime`] is shared by three parties: the actor's effect runner
//! (this file), the agent endpoints (`endpoint.rs`, which hold permission
//! hooks open and `peek` the hub) and whoever wires the server.
//!
//! ```ignore
//! let rt = AgentRuntime::from_config(&config, hub.clone());
//! ServerBuilder::new(config)
//!     .transcripts(rt.transcripts().clone())
//!     .runner(rt.runner())
//!     .extra_routes(move || Extra { unix_only: rt.routes(), ..Extra::default() })
//! ```
//!
//! The runner never blocks the actor: worktrees go through the
//! [`WorktreePool`], files and PTYs are made on the blocking pool, and every
//! outcome comes back as a `Command::Runner`. Terminals are registered in the
//! hub under `term_key(TermId)` and removed from it once the process exits.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cmux_core::engine::{Command, Effect, RunnerEvent};
use cmux_core::ids::{AgentId, ItemId, TermId};
use cmux_core::model::{Level, State};

use super::endpoint::PermissionWaiters;
use crate::actor::{ActorHandle, EffectRunner};
use crate::claude::spawn::{
    LevelOpts, SpawnConfig, SpawnRequest, SpawnSpec, build_spawn, digest_dir, write_files,
};
use crate::claude::transcript::Transcripts;
use crate::claude::trust;
use crate::config::Config;
use crate::git::worktree::{WorktreePool, WorktreeRequest};
use crate::term::hub::{Hub, PROGRESS_EVERY, TermConfig, term_key};
use crate::term::pty::PtySpec;

/// Initial size of an agent's terminal (columns by rows).
const COLS: u16 = 120;
const ROWS: u16 = 40;

/// Exited agent terminals kept in the hub (with their final screen) beyond
/// the ones the engine still lists; the oldest go first.
const EXITED_KEPT: usize = 32;

/// A pasted digest longer than this goes through a file.
const PASTE_MAX_CHARS: usize = 1000;

/// What the runner and the endpoints share.
pub struct AgentRuntime {
    hub: Arc<Hub>,
    spawn: SpawnConfig,
    pool: WorktreePool,
    pub(crate) waiters: PermissionWaiters,
    /// Terminals this runtime started, killed at shutdown.
    started: Mutex<Vec<String>>,
    /// Terminals made but not yet listed by the engine: their `Spawned` is
    /// still queued in the actor, so they must not be taken for gone.
    unlisted: Mutex<HashSet<String>>,
    /// Read at `Stop` (by the endpoint) and at exit; give the same one to
    /// `ServerBuilder::transcripts`.
    transcripts: Arc<Transcripts>,
}

impl AgentRuntime {
    pub fn new(hub: Arc<Hub>, spawn: SpawnConfig) -> Arc<AgentRuntime> {
        AgentRuntime::with_pool(hub, spawn, WorktreePool::default())
    }

    fn with_pool(hub: Arc<Hub>, spawn: SpawnConfig, pool: WorktreePool) -> Arc<AgentRuntime> {
        Arc::new(AgentRuntime {
            hub,
            spawn,
            pool,
            waiters: PermissionWaiters::default(),
            started: Mutex::new(Vec::new()),
            unlisted: Mutex::new(HashSet::new()),
            transcripts: Transcripts::new(),
        })
    }

    /// The spawn settings from `config.toml`: the `claude` binary and levels
    /// from `[agents]`, this executable for `hook` and `mcp`, and the socket
    /// and data directory the server itself uses.
    pub fn from_config(config: &Config, hub: Arc<Hub>) -> Arc<AgentRuntime> {
        let exe = running_exe();
        let mut spawn = SpawnConfig::new(exe, config.socket_path(), config.data_dir());
        let a = &config.agents;
        spawn.claude_bin = a.claude_bin.clone();
        spawn.permission_wait = Duration::from_secs(config.permission_wait_secs());
        spawn.pre_trust = a.pre_trust;
        let opts = |l: &crate::config::LevelConfig| LevelOpts {
            permission_mode: l.permission_mode.clone(),
            model: l.model.clone(),
        };
        spawn.executor = opts(&a.executor);
        spawn.task = opts(&a.task);
        spawn.sub = opts(&a.sub);
        let pool = WorktreePool::new(config.limits.global().git_ops as usize);
        AgentRuntime::with_pool(hub, spawn, pool)
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
    }

    pub fn transcripts(&self) -> &Arc<Transcripts> {
        &self.transcripts
    }

    pub fn spawn_config(&self) -> &SpawnConfig {
        &self.spawn
    }

    pub fn permission_wait(&self) -> Duration {
        self.spawn.permission_wait
    }

    /// Open `PermissionRequest` hooks (tests).
    pub fn waiters(&self) -> &PermissionWaiters {
        &self.waiters
    }

    /// For `ServerBuilder::runner`.
    pub fn runner(
        self: &Arc<Self>,
    ) -> impl FnOnce(ActorHandle) -> Box<dyn EffectRunner> + Send + 'static {
        let rt = self.clone();
        move |handle| Box::new(AgentRunner { rt, handle })
    }

    /// Removes exited terminals the engine no longer lists (a resumed agent's
    /// old one, a closed one) from the hub, and the oldest exited agent
    /// terminals beyond [`EXITED_KEPT`]. Loose terminals the engine still
    /// lists and terminals whose `Spawned` is still queued stay.
    fn forget_gone_terminals(&self, state: &State) {
        let unlisted = self.unlisted.lock().unwrap_or_else(|e| e.into_inner());
        let mut kept = Vec::new();
        for key in self.hub.ids() {
            let Some(n) = key.strip_prefix('t').and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let exited = self
                .hub
                .get(&key)
                .is_some_and(|t| t.exited().borrow().is_some());
            if !exited || unlisted.contains(&key) {
                continue;
            }
            match state.terminals.get(&TermId(n)) {
                // A loose terminal stays until its `CloseTerminal` drops the
                // record, so it is neither counted nor evicted.
                Some(t) if t.agent.is_none() => {}
                Some(_) => kept.push((n, key)),
                None => {
                    self.hub.remove(&key);
                }
            }
        }
        kept.sort();
        let excess = kept.len().saturating_sub(EXITED_KEPT);
        for (_, key) in &kept[..excess] {
            self.hub.remove(key);
        }
    }

    fn listed(&self, id: &str) {
        self.unlisted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    fn note_started(&self, id: &str) {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(id.to_string());
    }
}

/// This executable's path for the hooks and the MCP server. Linux reports
/// `/path/cmuxd (deleted)` once the binary was replaced under a running
/// server (an upgrade); the new binary sits at the plain path.
pub(crate) fn running_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cmuxd"));
    strip_deleted(exe)
}

fn strip_deleted(exe: PathBuf) -> PathBuf {
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(p) => PathBuf::from(p),
        None => exe,
    }
}

/// The effect runner. Offered effects after persistence.
pub struct AgentRunner {
    rt: Arc<AgentRuntime>,
    handle: ActorHandle,
}

fn term_of(state: &State, agent: &AgentId) -> Option<TermId> {
    state
        .plans
        .iter()
        .flat_map(|p| &p.agents)
        .find(|a| a.id == *agent)
        .and_then(|a| a.term)
}

impl EffectRunner for AgentRunner {
    fn run(&self, effect: &Effect, state: &State) -> bool {
        match effect {
            Effect::CreateWorktree {
                plan,
                task,
                repo_path,
                base,
                branch,
            } => {
                let req = WorktreeRequest {
                    repo_path: repo_path.clone(),
                    name: task.to_string(),
                    base: base.clone(),
                    branch: branch.clone(),
                };
                let (rt, handle) = (self.rt.clone(), self.handle.clone());
                let (plan, task, branch) = (plan.clone(), *task, branch.clone());
                tokio::spawn(async move {
                    let ev = match rt.pool.ensure(req).await {
                        Ok(w) => RunnerEvent::WorktreeReady {
                            plan,
                            task,
                            path: w.path,
                            branch: w.branch.unwrap_or(branch),
                        },
                        Err(e) => RunnerEvent::WorktreeFailed {
                            plan,
                            task,
                            error: e.to_string(),
                        },
                    };
                    handle.send(Command::Runner(ev));
                });
                true
            }
            Effect::SpawnAgent { term, .. } => {
                let Some(req) = SpawnRequest::from_effect(effect) else {
                    return false;
                };
                self.rt.forget_gone_terminals(state);
                let spec = build_spawn(&self.rt.spawn, &req);
                let cfg = if req.level == Level::Sub {
                    TermConfig::small()
                } else {
                    TermConfig::default()
                };
                tokio::spawn(start_agent(
                    self.rt.clone(),
                    self.handle.clone(),
                    req.agent.clone(),
                    *term,
                    spec,
                    cfg,
                ));
                true
            }
            Effect::KillAgent { agent, term } => {
                let term = term.or_else(|| term_of(state, agent));
                if let Some(t) = term.and_then(|t| self.rt.hub.get_term(t)) {
                    t.kill();
                }
                true
            }
            Effect::PasteDigest { agent, items, text } => {
                if let Some(term) = term_of(state, agent) {
                    let digest_dir = digest_dir(&self.rt.spawn.data_dir, agent);
                    tokio::spawn(paste_digest(
                        self.rt.clone(),
                        term,
                        digest_dir,
                        items.clone(),
                        text.clone(),
                    ));
                }
                true
            }
            Effect::WritePty { agent, bytes } => {
                if let Some(t) = term_of(state, agent).and_then(|t| self.rt.hub.get_term(t)) {
                    t.write(bytes);
                }
                true
            }
            Effect::AnswerPermission { item, decision, .. } => {
                self.rt.waiters.resolve(*item, decision.clone());
                true
            }
            // `push::PushRunner` sends notifications.
            Effect::Notify { .. }
            | Effect::NotifyPlanDone { .. }
            | Effect::Persist { .. }
            | Effect::DeletePlan { .. }
            | Effect::PersistGlobals => false,
        }
    }

    fn shutdown(&self) {
        let ids = self.rt.started.lock().unwrap_or_else(|e| e.into_inner());
        for id in ids.iter() {
            if let Some(t) = self.rt.hub.get(id) {
                t.kill();
            }
        }
    }
}

/// The PTY description for a spawn: the scrub lists become explicit removals.
fn pty_spec(spec: &SpawnSpec) -> PtySpec {
    let mut env_remove = spec.env_remove.clone();
    for (k, _) in std::env::vars_os() {
        if let Some(k) = k.to_str()
            && spec.env_remove_prefixes.iter().any(|p| k.starts_with(p))
        {
            env_remove.push(k.to_string());
        }
    }
    PtySpec {
        program: spec.argv[0].clone(),
        args: spec.argv[1..].to_vec(),
        env: spec.env.clone(),
        env_remove,
        cwd: Some(spec.cwd.clone()),
        rows: ROWS,
        cols: COLS,
    }
}

/// Writes the agent's files, starts `claude` in a hub terminal, reports
/// `Spawned` (or `SpawnFailed`) and, when it ends, `Exited`.
async fn start_agent(
    rt: Arc<AgentRuntime>,
    handle: ActorHandle,
    agent: AgentId,
    term: TermId,
    spec: SpawnSpec,
    cfg: TermConfig,
) {
    let (session_id, token_hash) = (spec.session_id.clone(), spec.token_hash);
    let pre_trust = rt.spawn.pre_trust;
    // `--add-dir` names it, so it exists before Claude starts.
    let digests = digest_dir(&rt.spawn.data_dir, &agent);
    let id = term_key(term);
    let hub = rt.hub.clone();
    rt.unlisted
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone());
    let made = tokio::task::spawn_blocking({
        let id = id.clone();
        move || {
            if pre_trust {
                // `std::env::vars` panics on a non-UTF-8 variable.
                let vars = std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
                trust::pre_trust(&spec, vars, &crate::config::home());
            }
            write_files(&spec.files)
                .and_then(|()| std::fs::create_dir_all(&digests))
                .map_err(|e| format!("cannot write agent files: {e}"))?;
            hub.create(&id, &pty_spec(&spec), &cfg)
                .map_err(|e| format!("cannot start {}: {e:#}", spec.argv[0]))
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("spawn task failed: {e}")));
    let terminal = match made {
        Ok(t) => t,
        Err(error) => {
            rt.listed(&id);
            handle.send(Command::Runner(RunnerEvent::SpawnFailed { agent, error }));
            return;
        }
    };
    rt.note_started(&id);
    handle.send(Command::Runner(RunnerEvent::Spawned {
        agent: agent.clone(),
        term,
        session_id,
        token_hash,
    }));
    // The actor handles messages in order: once this query runs, `Spawned` has.
    let _ = handle.query(|_| ()).await;
    rt.listed(&id);
    // Screen changes feed the stall watchdog, at most once per 30 s.
    tokio::spawn({
        let (terminal, handle) = (terminal.clone(), handle.clone());
        async move {
            let progress = || handle.send(Command::Runner(RunnerEvent::Progress { term }));
            terminal.watch_progress(PROGRESS_EVERY, progress).await;
        }
    });
    let mut exited = terminal.exited();
    let info = match exited.wait_for(|e| e.is_some()).await {
        Ok(v) => v.clone(),
        Err(_) => None,
    };
    let code = info.and_then(|i| i.signal.is_none().then_some(i.code as i32));
    // The last answer becomes the implicit report, so it must be read first.
    rt.transcripts.report_turn(&handle, &agent).await;
    // The terminal stays in the hub with its final screen until the engine
    // drops its record or too many pile up (see `forget_gone_terminals`).
    handle.send(Command::Runner(RunnerEvent::Exited { agent, term, code }));
    let _ = handle.query(move |st| rt.forget_gone_terminals(st)).await;
}

/// The one line pasted into the PTY for a digest. A long or multi-line digest
/// is written to `file` and pointed at (spike S3: 4+ pasted lines would reach
/// Claude wrapped in `<pasted_content>` tags).
fn digest_line(items: &[ItemId], text: &str, file: &Path) -> (String, bool) {
    if !text.contains(['\n', '\r']) && text.chars().count() < PASTE_MAX_CHARS {
        return (text.to_string(), false);
    }
    let ids: Vec<String> = items.iter().map(|i| format!("I-{}", i.0)).collect();
    let n = items.len();
    let line = format!(
        "[cmux] {n} new message{} ({}): read {}",
        if n == 1 { "" } else { "s" },
        ids.join(", "),
        file.display()
    );
    (line, true)
}

/// Bracketed paste of one line, then `\r`, in a single write (spike S3).
pub fn paste_bytes(line: &str) -> Vec<u8> {
    let clean: String = line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = b"\x1b[200~".to_vec();
    out.extend_from_slice(clean.as_bytes());
    out.extend_from_slice(b"\x1b[201~\r");
    out
}

async fn paste_digest(
    rt: Arc<AgentRuntime>,
    term: TermId,
    dir: PathBuf,
    items: Vec<ItemId>,
    text: String,
) {
    let first = items.first().map_or(0, |i| i.0);
    let file = dir.join(format!("digest-{first}.md"));
    let (line, needs_file) = digest_line(&items, &text, &file);
    if needs_file {
        let write = async {
            tokio::fs::create_dir_all(&dir).await?;
            tokio::fs::write(&file, &text).await
        };
        if let Err(e) = write.await {
            tracing::warn!(target: "agents", "cannot write digest {}: {e}", file.display());
            return;
        }
    }
    if let Some(t) = rt.hub.get_term(term) {
        t.write(&paste_bytes(&line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_digests_are_pasted_long_ones_go_through_a_file() {
        let f = Path::new("/d/digest-12.md");
        let items = [ItemId(12), ItemId(13)];
        assert_eq!(digest_line(&items, "hello", f), ("hello".into(), false));
        let (line, file) = digest_line(&items, "a\nb", f);
        assert!(file);
        assert_eq!(
            line,
            "[cmux] 2 new messages (I-12, I-13): read /d/digest-12.md"
        );
        let (line, file) = digest_line(&items[..1], &"x".repeat(1000), f);
        assert!(file && line.starts_with("[cmux] 1 new message (I-12)"));
    }

    #[test]
    fn paste_is_one_bracketed_write_with_a_return() {
        assert_eq!(
            paste_bytes("hi\x1b[201~\nthere"),
            b"\x1b[200~hi [201~ there\x1b[201~\r"
        );
    }

    #[test]
    fn a_replaced_binary_keeps_its_plain_path() {
        assert_eq!(
            strip_deleted(PathBuf::from("/opt/cmux/cmuxd (deleted)")),
            PathBuf::from("/opt/cmux/cmuxd")
        );
        assert_eq!(
            strip_deleted(PathBuf::from("/opt/cmux/cmuxd")),
            PathBuf::from("/opt/cmux/cmuxd")
        );
    }
}

#[cfg(test)]
mod forget_tests {
    use super::*;
    use crate::term::pty::PtySpec;
    use cmux_core::ids::{AgentPath, PlanId};
    use cmux_core::model::{TermKind, Terminal};

    fn listed(id: u32) -> Terminal {
        Terminal {
            id: TermId(id),
            kind: TermKind::Claude,
            agent: None,
            cwd: PathBuf::new(),
            size: (80, 24),
            size_owner: None,
            title: None,
        }
    }

    fn agent_terminal(id: u32) -> Terminal {
        let plan = PlanId::new(2026, 10, 18, "Auth refactor");
        Terminal {
            agent: Some(AgentId::new(plan, AgentPath::Executor)),
            ..listed(id)
        }
    }

    fn exited_terminal(hub: &Hub, id: u32) {
        let t = hub
            .create(
                &term_key(TermId(id)),
                &PtySpec::new("true", &[]),
                &TermConfig::small(),
            )
            .unwrap();
        let mut rx = t.exited();
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while rx.borrow_and_update().is_none() {
            assert!(std::time::Instant::now() < end, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn runtime(hub: &Arc<Hub>) -> Arc<AgentRuntime> {
        let cfg = SpawnConfig::new(
            PathBuf::from("cmuxd"),
            PathBuf::from("s"),
            PathBuf::from("d"),
        );
        AgentRuntime::new(hub.clone(), cfg)
    }

    #[test]
    fn a_terminal_whose_spawned_is_still_queued_is_not_forgotten() {
        let hub = Arc::new(Hub::new());
        let rt = runtime(&hub);
        exited_terminal(&hub, 1);
        rt.unlisted.lock().unwrap().insert(term_key(TermId(1)));
        rt.forget_gone_terminals(&State::default());
        assert_eq!(hub.ids(), ["t1"]);
        // Once `Spawned` is applied and the engine does not list it, it goes.
        rt.listed("t1");
        rt.forget_gone_terminals(&State::default());
        assert!(hub.ids().is_empty());
    }

    #[test]
    fn only_the_newest_exited_terminals_stay() {
        let hub = Arc::new(Hub::new());
        let rt = runtime(&hub);
        let mut state = State::default();
        for id in 1..=(EXITED_KEPT as u32 + 3) {
            exited_terminal(&hub, id);
            state.terminals.insert(TermId(id), agent_terminal(id));
        }
        rt.forget_gone_terminals(&state);
        let ids = hub.ids();
        assert_eq!(ids.len(), EXITED_KEPT);
        assert!(!ids.contains(&"t1".to_string()) && ids.contains(&"t4".to_string()));
    }

    #[test]
    fn loose_terminals_are_never_evicted_for_agent_ones() {
        let hub = Arc::new(Hub::new());
        let rt = runtime(&hub);
        let mut state = State::default();
        // Older loose shells first, then one exited agent terminal.
        for id in 1..=(EXITED_KEPT as u32 + 3) {
            exited_terminal(&hub, id);
            state.terminals.insert(TermId(id), listed(id));
        }
        exited_terminal(&hub, 100);
        state.terminals.insert(TermId(100), agent_terminal(100));
        rt.forget_gone_terminals(&state);
        let ids = hub.ids();
        assert_eq!(ids.len(), EXITED_KEPT + 3 + 1);
        assert!(ids.contains(&"t1".to_string()) && ids.contains(&"t100".to_string()));
    }
}
