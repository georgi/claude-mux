//! The engine actor (architecture 5.1).
//!
//! One tokio task owns the [`State`]. Everything else talks to it through an
//! [`ActorHandle`]:
//!
//! - [`ActorHandle::send`] / [`call`](ActorHandle::call) feed it
//!   [`Command`]s (the API, the agent endpoint, the effect runner, the timer);
//! - [`ActorHandle::query`] reads the state without copying it out;
//! - [`ActorHandle::subscribe_since`] returns the deltas a stream client
//!   missed (or a snapshot) together with a live receiver, atomically.
//!
//! After each `engine::apply` the actor builds [`DeltaView`]s, appends them to
//! a ring of the last 10 000, broadcasts them, and hands each [`Effect`] to the
//! [`EffectRunner`]. The actor never awaits I/O: runners must not block, they
//! queue work to threads or tasks and report back with commands.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cmux_core::engine::{self, ClientAction, Command, Effect, EngineError, Reply};
use cmux_core::ids::{ClientId, PlanId};
use cmux_core::model::{State, Timestamp};
use cmux_proto::view::{DeltaView, StateView};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::store::{Globals, InboxFile, StoreWriter};

/// Deltas kept for `since_rev` catch-up.
pub const RING_CAP: usize = 10_000;
const BROADCAST_CAP: usize = 4096;

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// Wall-clock milliseconds, plus a skew that tests advance to expire things
/// without sleeping. Every server timestamp comes from here.
#[derive(Debug, Clone, Default)]
pub struct Clock {
    skew_ms: Arc<AtomicU64>,
}

impl Clock {
    pub fn now_ms(&self) -> Timestamp {
        let real = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        real + self.skew_ms.load(Ordering::Relaxed)
    }

    /// Moves this clock (and its clones) forward.
    pub fn advance(&self, by: Duration) {
        self.skew_ms
            .fetch_add(by.as_millis() as u64, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Effect runner
// ---------------------------------------------------------------------------

/// Executes the engine's [`Effect`]s. Called on the actor task with the state
/// the effect came from, so it must return quickly: queue the real work and
/// report outcomes back with [`ActorHandle::send`] (`Command::Runner`).
pub trait EffectRunner: Send + Sync + 'static {
    /// Returns whether this runner dealt with the effect.
    fn run(&self, effect: &Effect, state: &State) -> bool;

    /// Called once when the actor stops; may block (flushing, killing PTYs).
    fn shutdown(&self) {}
}

/// Offers each effect to its runners in order and logs the ones nobody took.
pub struct CompositeRunner(pub Vec<Box<dyn EffectRunner>>);

impl EffectRunner for CompositeRunner {
    fn run(&self, effect: &Effect, state: &State) -> bool {
        let handled = self.0.iter().any(|r| r.run(effect, state));
        if !handled {
            tracing::debug!(target: "effects", "unhandled effect: {}", variant(&format!("{effect:?}")));
        }
        handled
    }

    fn shutdown(&self) {
        for r in &self.0 {
            r.shutdown();
        }
    }
}

/// Handles `Effect::Persist` by queueing a snapshot for the [`StoreWriter`].
pub struct PersistRunner(pub StoreWriter);

impl EffectRunner for PersistRunner {
    fn run(&self, effect: &Effect, state: &State) -> bool {
        match effect {
            Effect::Persist { plan } => {
                if let Some(p) = state.plans.iter().find(|p| p.id == *plan) {
                    self.0.save_plan(p.clone());
                }
            }
            Effect::DeletePlan { plan } => self.0.delete_plan(plan.clone()),
            Effect::PersistGlobals => {}
            _ => return false,
        }
        self.0.save_globals(
            Globals::of(state),
            InboxFile {
                inbox: state.inbox.clone(),
                next_item_id: state.next_item_id,
            },
        );
        true
    }

    fn shutdown(&self) {
        self.0.close();
    }
}

/// Takes nothing; tests and `CompositeRunner` use it as a floor.
pub struct NullRunner;

impl EffectRunner for NullRunner {
    fn run(&self, _: &Effect, _: &State) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------

/// What a stream client missed.
#[derive(Debug, Clone, PartialEq)]
pub enum Catchup {
    /// Every delta after `since_rev`, oldest first (maybe none).
    Deltas(Vec<DeltaView>),
    /// `since_rev` is unknown or older than the ring: start from this.
    Snapshot(StateView),
}

/// A catch-up plus a receiver for everything after it, with no gap.
pub struct Subscription {
    pub catchup: Catchup,
    pub rx: broadcast::Receiver<DeltaView>,
}

/// The actor has stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActorGone;

impl std::fmt::Display for ActorGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the engine actor has stopped")
    }
}

impl std::error::Error for ActorGone {}

type CommandReply = Result<Reply, EngineError>;

enum Msg {
    Command {
        cmd: Command,
        reply: Option<oneshot::Sender<CommandReply>>,
    },
    Query(Box<dyn FnOnce(&State) + Send>),
    Subscribe {
        since_rev: Option<u64>,
        reply: oneshot::Sender<Subscription>,
    },
    Shutdown(oneshot::Sender<()>),
}

/// Cloneable access to the actor.
#[derive(Clone)]
pub struct ActorHandle {
    tx: mpsc::UnboundedSender<Msg>,
    deltas: broadcast::Sender<DeltaView>,
    clock: Clock,
}

impl ActorHandle {
    /// Queues a command and does not wait for the result. Never blocks, so
    /// runner threads and timers may call it.
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Msg::Command { cmd, reply: None });
    }

    /// Runs a command and returns the engine's reply (or its refusal).
    pub async fn call(&self, cmd: Command) -> Result<CommandReply, ActorGone> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Msg::Command {
                cmd,
                reply: Some(reply),
            })
            .map_err(|_| ActorGone)?;
        rx.await.map_err(|_| ActorGone)
    }

    /// `call` for a client action.
    pub async fn client(
        &self,
        client: ClientId,
        action: ClientAction,
    ) -> Result<CommandReply, ActorGone> {
        self.call(Command::Client { client, action }).await
    }

    /// Reads the state on the actor task. Keep `f` short.
    pub async fn query<T: Send + 'static>(
        &self,
        f: impl FnOnce(&State) -> T + Send + 'static,
    ) -> Result<T, ActorGone> {
        let (tx, rx) = oneshot::channel();
        let job = Box::new(move |s: &State| {
            let _ = tx.send(f(s));
        });
        self.tx.send(Msg::Query(job)).map_err(|_| ActorGone)?;
        rx.await.map_err(|_| ActorGone)
    }

    pub async fn state_view(&self) -> Result<StateView, ActorGone> {
        self.query(|s| StateView::from(s)).await
    }

    /// Live deltas only. Prefer [`subscribe_since`](Self::subscribe_since),
    /// which cannot miss anything between a snapshot and the subscription.
    pub fn subscribe(&self) -> broadcast::Receiver<DeltaView> {
        self.deltas.subscribe()
    }

    /// With `Some(rev)`: the deltas after `rev` if the ring still holds them,
    /// else a snapshot. With `None`: a snapshot. The receiver gets every delta
    /// after that point.
    pub async fn subscribe_since(&self, since_rev: Option<u64>) -> Result<Subscription, ActorGone> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Msg::Subscribe { since_rev, reply })
            .map_err(|_| ActorGone)?;
        rx.await.map_err(|_| ActorGone)
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Stops the actor after the commands already queued, then runs the
    /// runner's `shutdown` (which flushes the store).
    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Shutdown(tx)).is_ok() {
            let _ = rx.await;
        }
    }
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

pub struct ActorOptions {
    pub clock: Clock,
    /// Deltas kept for catch-up.
    pub ring: usize,
    /// `Some(1 s)` in production; `None` in tests that drive `Tick` by hand.
    pub tick: Option<Duration>,
    /// Receives one journal line per state-changing command or refusal.
    pub journal: Option<StoreWriter>,
}

impl Default for ActorOptions {
    fn default() -> Self {
        ActorOptions {
            clock: Clock::default(),
            ring: RING_CAP,
            tick: Some(Duration::from_secs(1)),
            journal: None,
        }
    }
}

/// Starts the actor on the current tokio runtime. `make_runner` gets the
/// handle so a runner can report back to the engine.
pub fn spawn(
    state: State,
    opts: ActorOptions,
    make_runner: impl FnOnce(ActorHandle) -> Box<dyn EffectRunner>,
) -> ActorHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let (deltas, _) = broadcast::channel(BROADCAST_CAP);
    let handle = ActorHandle {
        tx,
        deltas,
        clock: opts.clock.clone(),
    };
    let runner = make_runner(handle.clone());
    if let Some(every) = opts.tick {
        let h = handle.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(every);
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            t.tick().await;
            loop {
                t.tick().await;
                if h.tx
                    .send(Msg::Command {
                        cmd: Command::Tick,
                        reply: None,
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
    }
    let actor = Actor {
        floor_rev: state.rev,
        state,
        ring: VecDeque::new(),
        cap: opts.ring.max(1),
        runner,
        clock: opts.clock,
        journal: opts.journal,
        deltas: handle.deltas.clone(),
    };
    tokio::spawn(actor.run(rx));
    handle
}

struct Actor {
    state: State,
    ring: VecDeque<DeltaView>,
    /// Highest rev of any delta dropped from the ring (or the rev we started
    /// at): a client at or above it has seen everything older.
    floor_rev: u64,
    cap: usize,
    runner: Box<dyn EffectRunner>,
    clock: Clock,
    journal: Option<StoreWriter>,
    deltas: broadcast::Sender<DeltaView>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Command { cmd, reply } => {
                    let r = self.step(cmd);
                    if let Some(reply) = reply {
                        let _ = reply.send(r);
                    }
                }
                Msg::Query(f) => f(&self.state),
                Msg::Subscribe { since_rev, reply } => {
                    let _ = reply.send(self.subscription(since_rev));
                }
                Msg::Shutdown(ack) => {
                    // Anything queued behind the shutdown is dropped with `rx`.
                    let runner = self.runner;
                    let _ = tokio::task::spawn_blocking(move || runner.shutdown()).await;
                    let _ = ack.send(());
                    return;
                }
            }
        }
        let runner = self.runner;
        let _ = tokio::task::spawn_blocking(move || runner.shutdown()).await;
    }

    fn step(&mut self, cmd: Command) -> CommandReply {
        let label = label(&cmd);
        let is_tick = matches!(cmd, Command::Tick);
        let now = self.clock.now_ms();
        let result = engine::apply(&mut self.state, cmd, now);
        let (reply, effects, deltas) = match result {
            Ok(o) => (Ok(o.reply), o.effects, o.deltas),
            Err(e) => (Err(e), Vec::new(), Vec::new()),
        };
        let views: Vec<DeltaView> = deltas
            .iter()
            .filter_map(|d| DeltaView::from_core(&self.state, d))
            .collect();
        self.journal(&label, is_tick, &reply, &effects, deltas.len(), now);
        for v in views {
            if self.ring.len() == self.cap
                && let Some(old) = self.ring.pop_front()
            {
                self.floor_rev = self.floor_rev.max(old.rev);
            }
            self.ring.push_back(v.clone());
            let _ = self.deltas.send(v);
        }
        for e in &effects {
            self.runner.run(e, &self.state);
        }
        reply
    }

    fn journal(
        &self,
        label: &str,
        is_tick: bool,
        reply: &CommandReply,
        effects: &[Effect],
        deltas: usize,
        now: Timestamp,
    ) {
        let Some(j) = &self.journal else { return };
        if is_tick && reply.is_ok() && deltas == 0 {
            return;
        }
        let mut plans: Vec<PlanId> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Persist { plan } => Some(plan.clone()),
                _ => None,
            })
            .collect();
        plans.dedup();
        let mut line = json!({"ts": now, "rev": self.state.rev, "cmd": label, "deltas": deltas});
        if let Err(e) = reply {
            line["error"] = json!(e.to_string());
        }
        let line = line.to_string();
        if plans.is_empty() {
            j.journal(None, line);
        } else {
            for p in plans {
                j.journal(Some(p), line.clone());
            }
        }
    }

    fn subscription(&self, since_rev: Option<u64>) -> Subscription {
        let rx = self.deltas.subscribe();
        let catchup = match since_rev {
            Some(r) if r <= self.state.rev && r >= self.floor_rev => {
                Catchup::Deltas(self.ring.iter().filter(|d| d.rev > r).cloned().collect())
            }
            _ => Catchup::Snapshot(StateView::from(&self.state)),
        };
        Subscription { catchup, rx }
    }
}

/// `client.CreatePlan`, `hook.Stop`, ...: what the journal records of a command
/// (never its arguments, which may hold prompts or tokens).
fn label(cmd: &Command) -> String {
    match cmd {
        Command::Client { action, .. } => format!("client.{}", variant(&format!("{action:?}"))),
        Command::Hook { hook, .. } => format!("hook.{}", variant(&format!("{hook:?}"))),
        Command::Tool { call, .. } => format!("tool.{}", variant(&format!("{call:?}"))),
        Command::Runner(ev) => format!("runner.{}", variant(&format!("{ev:?}"))),
        Command::Tick => "tick".into(),
        Command::Recover { .. } => "recover".into(),
    }
}

/// The leading identifier of a `Debug` string.
fn variant(debug: &str) -> &str {
    let end = debug
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(debug.len());
    &debug[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::model::Repo;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    pub struct Recorder(pub Arc<Mutex<Vec<Effect>>>);

    impl EffectRunner for Recorder {
        fn run(&self, e: &Effect, _: &State) -> bool {
            self.0.lock().unwrap().push(e.clone());
            true
        }
    }

    fn start(ring: usize) -> (ActorHandle, Recorder) {
        let rec = Recorder::default();
        let r2 = rec.clone();
        let h = spawn(
            State::default(),
            ActorOptions {
                ring,
                tick: None,
                ..Default::default()
            },
            move |_| Box::new(r2),
        );
        (h, rec)
    }

    fn create(title: &str) -> ClientAction {
        ClientAction::CreatePlan {
            title: title.into(),
            repos: vec![Repo {
                path: "/r".into(),
                base_branch: "main".into(),
                remote_url: None,
            }],
            request: Some("do".into()),
        }
    }

    async fn make_plan(h: &ActorHandle, title: &str) -> String {
        match h.client(ClientId("t".into()), create(title)).await.unwrap() {
            Ok(Reply::Text(id)) => id,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn commands_broadcast_deltas_and_run_effects() {
        let (h, rec) = start(100);
        let mut rx = h.subscribe();
        let id = make_plan(&h, "first").await;
        let d = rx.recv().await.unwrap();
        assert_eq!(d.rev, 1);
        assert!(id.contains("first"));
        let fx = rec.0.lock().unwrap().clone();
        assert!(fx.iter().any(|e| matches!(e, Effect::Persist { .. })));
        assert!(fx.iter().any(|e| matches!(e, Effect::SpawnAgent { .. })));
        let view = h.state_view().await.unwrap();
        assert_eq!(view.rev, 1);
        assert_eq!(view.plans.len(), 1);
        // A refusal comes back as an Err and changes nothing.
        let err = h
            .client(ClientId("t".into()), create(" "))
            .await
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().starts_with("error:"));
        assert_eq!(h.state_view().await.unwrap().rev, 1);
    }

    #[tokio::test]
    async fn subscribe_since_serves_the_ring_then_a_snapshot() {
        let (h, _) = start(3);
        for t in ["a", "b", "c"] {
            make_plan(&h, t).await;
        }
        let rev = h.state_view().await.unwrap().rev;
        // Up to date: no deltas.
        let s = h.subscribe_since(Some(rev)).await.unwrap();
        assert_eq!(s.catchup, Catchup::Deltas(vec![]));
        // Behind but covered by the ring.
        let s = h.subscribe_since(Some(rev - 1)).await.unwrap();
        let Catchup::Deltas(d) = s.catchup else {
            panic!("expected deltas")
        };
        assert!(d.iter().all(|d| d.rev == rev) && !d.is_empty());
        // Each plan makes at least 2 deltas (plan + agent), so a 3-delta ring
        // no longer reaches back to rev 0, and the future is unknown.
        for since in [Some(0), Some(rev + 5), None] {
            let s = h.subscribe_since(since).await.unwrap();
            assert!(matches!(s.catchup, Catchup::Snapshot(_)), "{since:?}");
        }
        // The receiver sees what happens next.
        let mut s = h.subscribe_since(Some(rev)).await.unwrap();
        make_plan(&h, "d").await;
        assert_eq!(s.rx.recv().await.unwrap().rev, rev + 1);
    }

    #[tokio::test]
    async fn restart_rev_is_the_floor() {
        let st = State {
            rev: 50,
            ..State::default()
        };
        let h = spawn(
            st,
            ActorOptions {
                tick: None,
                ..Default::default()
            },
            |_| Box::new(NullRunner),
        );
        let s = h.subscribe_since(Some(10)).await.unwrap();
        assert!(matches!(s.catchup, Catchup::Snapshot(_)));
        let s = h.subscribe_since(Some(50)).await.unwrap();
        assert_eq!(s.catchup, Catchup::Deltas(vec![]));
    }

    #[tokio::test]
    async fn ticker_sends_ticks_and_shutdown_stops_the_actor() {
        let h = spawn(
            State::default(),
            ActorOptions {
                tick: Some(Duration::from_millis(10)),
                ..Default::default()
            },
            |_| Box::new(NullRunner),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.shutdown().await;
        assert!(h.state_view().await.is_err());
    }

    #[test]
    fn labels_hide_arguments() {
        let c = Command::Client {
            client: ClientId("x".into()),
            action: create("secret title"),
        };
        assert_eq!(label(&c), "client.CreatePlan");
        assert_eq!(label(&Command::Tick), "tick");
    }

    #[test]
    fn clock_advances() {
        let c = Clock::default();
        let a = c.now_ms();
        c.clone().advance(Duration::from_secs(60));
        assert!(c.now_ms() >= a + 60_000);
    }
}
