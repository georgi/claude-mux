//! The agent effect runner (architecture 7.1 and 8.1).
//!
//! [`AgentRuntime`] is shared by three parties: the actor's effect runner
//! (this file), the agent endpoints (`endpoint.rs`, which hold permission
//! hooks open and `peek` the hub) and whoever wires the server.
//!
//! ```ignore
//! let rt = AgentRuntime::from_config(&config, hub.clone());
//! ServerBuilder::new(config)
//!     .runner(rt.runner())
//!     .extra_routes(move || Extra { unix_only: rt.routes(), ..Extra::default() })
//! ```
//!
//! The runner never blocks the actor: worktrees go through the
//! [`WorktreePool`], files and PTYs are made on the blocking pool, and every
//! outcome comes back as a `Command::Runner`. Terminals are registered in the
//! hub under `term_key(TermId)` and removed from it once the process exits.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cmux_core::engine::{Command, Effect, RunnerEvent};
use cmux_core::ids::{AgentId, ItemId, TermId};
use cmux_core::model::{Level, State};

use super::endpoint::PermissionWaiters;
use crate::actor::{ActorHandle, EffectRunner};
use crate::claude::spawn::{
    LevelOpts, SpawnConfig, SpawnRequest, SpawnSpec, agent_dir, build_spawn, write_files,
};
use crate::config::Config;
use crate::git::worktree::{WorktreePool, WorktreeRequest};
use crate::term::hub::{Hub, TermConfig, term_key};
use crate::term::pty::PtySpec;

/// Initial size of an agent's terminal (columns by rows).
const COLS: u16 = 120;
const ROWS: u16 = 40;

/// A pasted digest longer than this goes through a file.
const PASTE_MAX_CHARS: usize = 1000;

/// What the runner and the endpoints share.
pub struct AgentRuntime {
    hub: Arc<Hub>,
    spawn: SpawnConfig,
    pool: WorktreePool,
    pub(crate) waiters: PermissionWaiters,
    next_term: AtomicU32,
    /// Terminals this runtime started, killed at shutdown.
    started: Mutex<Vec<String>>,
}

impl AgentRuntime {
    pub fn new(hub: Arc<Hub>, spawn: SpawnConfig) -> Arc<AgentRuntime> {
        Arc::new(AgentRuntime {
            hub,
            spawn,
            pool: WorktreePool::default(),
            waiters: PermissionWaiters::default(),
            next_term: AtomicU32::new(1),
            started: Mutex::new(Vec::new()),
        })
    }

    /// The spawn settings from `config.toml`: the `claude` binary and levels
    /// from `[agents]`, this executable for `hook` and `mcp`, and the socket
    /// and data directory the server itself uses.
    pub fn from_config(config: &Config, hub: Arc<Hub>) -> Arc<AgentRuntime> {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cmuxd"));
        let mut spawn = SpawnConfig::new(exe, config.socket_path(), config.data_dir());
        let a = &config.agents;
        spawn.claude_bin = a.claude_bin.clone();
        spawn.permission_wait = Duration::from_secs(a.permission_wait_secs.max(1));
        let opts = |l: &crate::config::LevelConfig| LevelOpts {
            permission_mode: l.permission_mode.clone(),
            model: l.model.clone(),
        };
        spawn.executor = opts(&a.executor);
        spawn.task = opts(&a.task);
        spawn.sub = opts(&a.sub);
        AgentRuntime::new(hub, spawn)
    }

    pub fn hub(&self) -> &Arc<Hub> {
        &self.hub
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

    /// A terminal id the engine has not used and this runtime has not handed out.
    fn alloc_term(&self, engine_next: u32) -> TermId {
        let n = self
            .next_term
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |cur| {
                Some(cur.max(engine_next) + 1)
            })
            .map(|prev| prev.max(engine_next))
            .expect("closure always returns Some");
        TermId(n)
    }

    fn note_started(&self, id: &str) {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(id.to_string());
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
            Effect::SpawnAgent { .. } => {
                let Some(req) = SpawnRequest::from_effect(effect) else {
                    return false;
                };
                let spec = build_spawn(&self.rt.spawn, &req);
                let term = self.rt.alloc_term(state.next_term_id);
                let cfg = if req.level == Level::Sub {
                    TermConfig::small()
                } else {
                    TermConfig::default()
                };
                tokio::spawn(start_agent(
                    self.rt.clone(),
                    self.handle.clone(),
                    req.agent.clone(),
                    term,
                    spec,
                    cfg,
                ));
                true
            }
            Effect::KillAgent { agent } => {
                if let Some(t) = term_of(state, agent).and_then(|t| self.rt.hub.get_term(t)) {
                    t.kill();
                }
                true
            }
            Effect::PasteDigest { agent, items, text } => {
                if let Some(term) = term_of(state, agent) {
                    let digest_dir = agent_dir(&self.rt.spawn.data_dir, agent);
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
            // Push notifications belong to a later step.
            Effect::Notify { .. } | Effect::Persist { .. } => false,
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
    let id = term_key(term);
    let hub = rt.hub.clone();
    let made = tokio::task::spawn_blocking({
        let id = id.clone();
        move || {
            write_files(&spec.files).map_err(|e| format!("cannot write agent files: {e}"))?;
            hub.create(&id, &pty_spec(&spec), &cfg)
                .map_err(|e| format!("cannot start {}: {e:#}", spec.argv[0]))
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("spawn task failed: {e}")));
    let terminal = match made {
        Ok(t) => t,
        Err(error) => {
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
    let mut exited = terminal.exited();
    let info = match exited.wait_for(|e| e.is_some()).await {
        Ok(v) => v.clone(),
        Err(_) => None,
    };
    let code = info.and_then(|i| i.signal.is_none().then_some(i.code as i32));
    handle.send(Command::Runner(RunnerEvent::Exited { agent, code }));
    // The stream has seen the exit by now; the engine owns the record.
    rt.hub.remove(&id);
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
    fn term_ids_skip_what_the_engine_knows() {
        let rt = AgentRuntime::new(
            Arc::new(Hub::new()),
            SpawnConfig::new("x".into(), "s".into(), "d".into()),
        );
        assert_eq!(rt.alloc_term(1), TermId(1));
        assert_eq!(rt.alloc_term(1), TermId(2));
        assert_eq!(rt.alloc_term(7), TermId(7));
        assert_eq!(rt.alloc_term(3), TermId(8));
    }
}
