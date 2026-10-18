//! The GitHub poller (architecture section 10): `gh pr view` for every open
//! PR in state, mapped to [`PrGithubState`] and sent to the engine as
//! [`RunnerEvent::Pr`] when the result changed.
//!
//! - Cadence: 60 s per open PR, 15 s for a task in `testing` whose agent is
//!   idle (`[github]` in `config.toml`).
//! - At most 2 `gh` calls at a time, on the blocking pool, started at least
//!   [`Cadence::stagger_ms`] apart.
//! - `gh auth status` runs once at start. If it fails the poller is off and
//!   `/status` says why.
//!
//! [`Schedule`] is the pure part (time comes in as milliseconds); [`Gh`] is
//! the `gh` binary behind a trait so tests use a fake.

use std::collections::HashMap;
use std::process::Command as Process;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use cmux_core::engine::{Command, RunnerEvent};
use cmux_core::ids::{PlanId, TaskId};
use cmux_core::model::{
    Activity, PrGithubState, RollupCheck, State, TaskState, rollup_to_ci_conclusion,
};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::actor::ActorHandle;

/// The fields asked of `gh pr view`.
pub const PR_FIELDS: &str = "state,mergedAt,statusCheckRollup,reviewDecision";

/// The `gh` CLI, or a fake. Both calls block; the poller runs them on the
/// blocking pool.
pub trait Gh: Send + Sync + 'static {
    /// `gh auth status`: `Err` holds a one-line reason.
    fn auth_status(&self) -> Result<(), String>;
    /// `gh pr view <url> --json ...`: the raw JSON.
    fn pr_view(&self, url: &str) -> Result<String, String>;
}

/// The real `gh` binary.
pub struct GhCli {
    pub bin: String,
}

impl GhCli {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Process::new(&self.bin)
            .args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("{} not found", self.bin),
                _ => format!("{}: {e}", self.bin),
            })?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().map(str::trim).find(|l| !l.is_empty());
        Err(line.map_or_else(
            || format!("{} failed: {}", self.bin, out.status),
            String::from,
        ))
    }
}

impl Gh for GhCli {
    fn auth_status(&self) -> Result<(), String> {
        self.run(&["auth", "status"]).map(drop)
    }

    fn pr_view(&self, url: &str) -> Result<String, String> {
        self.run(&["pr", "view", url, "--json", PR_FIELDS])
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrViewJson {
    state: String,
    merged_at: Option<String>,
    #[serde(default)]
    status_check_rollup: Option<Vec<RollupCheck>>,
}

/// Maps `gh pr view --json state,mergedAt,statusCheckRollup,reviewDecision`.
/// The review decision does not move a task; only state and CI do.
pub fn parse_pr_view(json: &str) -> Result<PrGithubState, String> {
    let v: PrViewJson = serde_json::from_str(json).map_err(|e| format!("bad gh JSON: {e}"))?;
    let merged = v.state.eq_ignore_ascii_case("MERGED")
        || v.merged_at.as_deref().is_some_and(|s| !s.is_empty());
    Ok(PrGithubState {
        merged,
        closed: !merged && v.state.eq_ignore_ascii_case("CLOSED"),
        ci: rollup_to_ci_conclusion(v.status_check_rollup.as_deref().unwrap_or_default()),
        head_sha: None,
    })
}

/// How often to look, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    /// Every open PR.
    pub open_ms: u64,
    /// A task in `testing` whose agent is idle.
    pub testing_ms: u64,
    /// The least time between two `gh` starts.
    pub stagger_ms: u64,
    /// `gh` calls at once.
    pub max_in_flight: usize,
}

impl Default for Cadence {
    fn default() -> Self {
        Cadence {
            open_ms: 60_000,
            testing_ms: 15_000,
            stagger_ms: 2_000,
            max_in_flight: 2,
        }
    }
}

/// A PR the poller looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    pub plan: PlanId,
    pub task: TaskId,
    pub url: String,
    /// In `testing` with an idle agent: the short cadence.
    pub fast: bool,
}

type Key = (PlanId, TaskId);

impl Watched {
    fn key(&self) -> Key {
        (self.plan.clone(), self.task)
    }
}

/// The open PRs in state: every task with a PR that is not merged or
/// cancelled.
pub fn watched(s: &State) -> Vec<Watched> {
    let mut out = Vec::new();
    for plan in &s.plans {
        for t in &plan.tasks {
            let Some(pr) = &t.pr else { continue };
            if matches!(t.state, TaskState::Merged | TaskState::Cancelled) {
                continue;
            }
            let idle = t
                .agent
                .as_ref()
                .and_then(|a| plan.agent(a))
                .is_some_and(|a| a.activity == Activity::Idle);
            out.push(Watched {
                plan: plan.id.clone(),
                task: t.id,
                url: pr.url.clone(),
                fast: t.state == TaskState::Testing && idle,
            });
        }
    }
    out
}

#[derive(Debug, Default)]
struct Entry {
    url: String,
    /// When the last call finished.
    checked_at: Option<u64>,
    in_flight: bool,
    /// The last result sent to the engine.
    last: Option<PrGithubState>,
}

/// Which PR to look at when, and which results are news. Pure: the caller
/// passes the time.
#[derive(Debug)]
pub struct Schedule {
    cadence: Cadence,
    entries: HashMap<Key, Entry>,
    in_flight: usize,
    last_start: Option<u64>,
}

impl Schedule {
    pub fn new(cadence: Cadence) -> Self {
        Schedule {
            cadence,
            entries: HashMap::new(),
            in_flight: 0,
            last_start: None,
        }
    }

    /// Syncs with the PRs in state and returns the ones to look at now,
    /// marked in flight. Each must come back through [`Schedule::finished`].
    pub fn due(&mut self, now: u64, watched: &[Watched]) -> Vec<Watched> {
        let c = self.cadence;
        self.entries
            .retain(|k, _| watched.iter().any(|w| w.key() == *k));
        let mut ready: Vec<(Option<u64>, &Watched)> = Vec::new();
        for w in watched {
            let e = self.entries.entry(w.key()).or_default();
            if e.url != w.url {
                // A new PR for the task: start over.
                *e = Entry {
                    url: w.url.clone(),
                    ..Entry::default()
                };
            }
            let every = if w.fast { c.testing_ms } else { c.open_ms };
            let due = e.checked_at.is_none_or(|at| now >= at + every);
            if due && !e.in_flight {
                ready.push((e.checked_at, w));
            }
        }
        // Never looked at first, then the longest ago.
        ready.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.key().cmp(&b.1.key())));
        let mut out = Vec::new();
        for (_, w) in ready {
            if self.in_flight >= c.max_in_flight
                || self.last_start.is_some_and(|t| now < t + c.stagger_ms)
            {
                break;
            }
            if let Some(e) = self.entries.get_mut(&w.key()) {
                e.in_flight = true;
            }
            self.in_flight += 1;
            self.last_start = Some(now);
            out.push(w.clone());
        }
        out
    }

    /// A call came back. Returns the state to send when it differs from the
    /// last one sent; an error is retried at the next cadence.
    pub fn finished(
        &mut self,
        w: &Watched,
        now: u64,
        result: Result<PrGithubState, String>,
    ) -> Option<PrGithubState> {
        self.in_flight = self.in_flight.saturating_sub(1);
        let e = self.entries.get_mut(&w.key()).filter(|e| e.url == w.url)?;
        e.in_flight = false;
        e.checked_at = Some(now);
        let state = result.ok()?;
        if e.last.as_ref() == Some(&state) {
            return None;
        }
        e.last = Some(state.clone());
        Some(state)
    }
}

/// What `/status` shows for GitHub: `on`, `starting` or `off (<reason>)`.
#[derive(Debug, Clone)]
pub struct Status(Arc<RwLock<String>>);

impl Default for Status {
    fn default() -> Self {
        Status(Arc::new(RwLock::new("off (not started)".into())))
    }
}

impl Status {
    pub fn get(&self) -> String {
        self.0.read().map_or_else(|_| "off".into(), |s| s.clone())
    }

    pub fn set(&self, s: impl Into<String>) {
        if let Ok(mut g) = self.0.write() {
            *g = s.into();
        }
    }

    pub fn off(&self, why: &str) {
        self.set(format!("off ({why})"));
    }
}

/// Starts the poller: `gh auth status`, then a look at the due PRs every
/// `tick` until `stop` turns true.
pub fn spawn(
    actor: ActorHandle,
    gh: Arc<dyn Gh>,
    cadence: Cadence,
    tick: Duration,
    status: Status,
    mut stop: watch::Receiver<bool>,
) -> JoinHandle<()> {
    status.set("starting");
    tokio::spawn(async move {
        let auth = {
            let gh = gh.clone();
            tokio::task::spawn_blocking(move || gh.auth_status()).await
        };
        match auth {
            Ok(Ok(())) => status.set("on"),
            Ok(Err(why)) => return status.off(&why),
            Err(e) => return status.off(&e.to_string()),
        }
        let start = Instant::now();
        let now = || start.elapsed().as_millis() as u64;
        let (tx, mut rx) = mpsc::unbounded_channel::<(Watched, Result<PrGithubState, String>)>();
        let mut schedule = Schedule::new(cadence);
        let mut every = tokio::time::interval(tick);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                // The `Ref` is not `Send`; drop it inside.
                () = async { drop(stop.wait_for(|s| *s).await) } => break,
                Some((w, result)) = rx.recv() => {
                    if let Err(e) = &result {
                        tracing::warn!(url = %w.url, error = %e, "gh pr view");
                    }
                    if let Some(state) = schedule.finished(&w, now(), result) {
                        actor.send(Command::Runner(RunnerEvent::Pr {
                            plan: w.plan,
                            task: w.task,
                            state,
                        }));
                    }
                }
                _ = every.tick() => {
                    let Ok(open) = actor.query(watched).await else { break };
                    for w in schedule.due(now(), &open) {
                        let (gh, tx) = (gh.clone(), tx.clone());
                        tokio::spawn(async move {
                            let url = w.url.clone();
                            let result = tokio::task::spawn_blocking(move || {
                                gh.pr_view(&url).and_then(|j| parse_pr_view(&j))
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                            let _ = tx.send((w, result));
                        });
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
