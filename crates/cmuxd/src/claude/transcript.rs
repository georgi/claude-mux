//! The transcript tailer (architecture 5.3): Claude's JSONL transcript of each
//! agent as [`ChatEntry`]s, for the chat view, the token sums and the
//! implicit report.
//!
//! The file is the agent's `transcript` from `SessionStart`, else Claude's
//! default place for its `cwd` and `session_id`. After `/clear` the agent
//! names a new file; the tailer finishes the old one and follows the new one,
//! so the conversation it keeps spans both.
//!
//! Reading happens only when someone needs it:
//! - every [`POLL_EVERY`] for agents with at least one chat subscriber;
//! - on a `Stop` and when the agent ends ([`Transcripts::report_turn`]), which
//!   also sends the engine one `TokensUpdated` when the sums or the last
//!   assistant text changed;
//! - for `GET /agents/:id/transcript` and a new `sub_chat`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cmux_core::engine::{Command, RunnerEvent};
use cmux_core::ids::AgentId;
use cmux_core::model::{State, TokenUsage};
use cmux_history::{TranscriptReader, transcript_file};
use cmux_proto::view::ChatEntry;
use tokio::sync::{broadcast, watch};

use crate::actor::ActorHandle;

/// How often subscribed agents are read.
pub const POLL_EVERY: Duration = Duration::from_millis(500);
/// Entries kept per agent; older ones are dropped.
pub const KEPT: usize = 5000;
/// `limit` when a transcript request names none, and the most it may ask for.
pub const PAGE_DEFAULT: usize = 200;
pub const PAGE_MAX: usize = 1000;
/// Batches of new entries queued per subscriber before it lags.
const BROADCAST_CAP: usize = 256;

/// Where `agent`'s transcript is, if the agent exists.
pub fn path_of(state: &State, agent: &AgentId) -> Option<PathBuf> {
    let a = state
        .plans
        .iter()
        .find(|p| p.id == *agent.plan())?
        .agents
        .iter()
        .find(|a| a.id == *agent)?;
    Some(
        a.transcript
            .clone()
            .unwrap_or_else(|| transcript_file(&a.cwd, &a.session_id)),
    )
}

fn add(sum: &mut TokenUsage, u: TokenUsage) {
    sum.input += u.input;
    sum.output += u.output;
    sum.cache_read += u.cache_read;
    sum.cache_write += u.cache_write;
}

/// One agent's conversation so far.
struct Tail {
    reader: Option<TranscriptReader>,
    entries: VecDeque<ChatEntry>,
    /// Tokens of the files finished before the current one (`/clear`).
    base: TokenUsage,
    last_text: Option<String>,
    tx: broadcast::Sender<Vec<ChatEntry>>,
    subscribers: usize,
    /// What the engine was told last.
    reported: Option<(TokenUsage, Option<String>)>,
}

impl Tail {
    fn new() -> Tail {
        Tail {
            reader: None,
            entries: VecDeque::new(),
            base: TokenUsage::default(),
            last_text: None,
            tx: broadcast::channel(BROADCAST_CAP).0,
            subscribers: 0,
            reported: None,
        }
    }

    fn tokens(&self) -> TokenUsage {
        let mut t = self.base;
        if let Some(r) = &self.reader {
            add(&mut t, r.tokens);
        }
        t
    }

    /// Reads what is new in `path`, first finishing the previous file when
    /// the path changed. Returns the new entries, already kept.
    fn read(&mut self, path: PathBuf) -> Vec<ChatEntry> {
        let mut new = Vec::new();
        if self.reader.as_ref().is_some_and(|r| r.path() != path) {
            let mut old = self.reader.take().expect("checked");
            new.extend(read_logged(&mut old));
            if let Some(t) = old.last_text.take() {
                self.last_text = Some(t);
            }
            add(&mut self.base, old.tokens);
        }
        let reader = self
            .reader
            .get_or_insert_with(|| TranscriptReader::new(path));
        new.extend(read_logged(reader));
        if let Some(t) = &reader.last_text {
            self.last_text = Some(t.clone());
        }
        if new.is_empty() {
            return new;
        }
        self.entries.extend(new.iter().cloned());
        let excess = self.entries.len().saturating_sub(KEPT);
        self.entries.drain(..excess);
        // Nobody listening is fine.
        let _ = self.tx.send(new.clone());
        new
    }

    /// The kept entries after `after`, oldest first: all of them when `after`
    /// is `None` or no longer kept.
    fn after(&self, after: Option<&str>) -> Vec<ChatEntry> {
        let start = after
            .and_then(|u| self.entries.iter().position(|e| e.uuid == u))
            .map_or(0, |i| i + 1);
        self.entries.iter().skip(start).cloned().collect()
    }
}

fn read_logged(r: &mut TranscriptReader) -> Vec<ChatEntry> {
    r.read_new().unwrap_or_else(|e| {
        tracing::warn!(target: "transcript", "cannot read {}: {e}", r.path().display());
        Vec::new()
    })
}

/// A live chat subscription: the entries after the client's `after`, then
/// every batch of new ones. Dropping it unsubscribes.
pub struct ChatSub {
    pub catchup: Vec<ChatEntry>,
    pub rx: broadcast::Receiver<Vec<ChatEntry>>,
    owner: Arc<Transcripts>,
    agent: AgentId,
}

impl Drop for ChatSub {
    fn drop(&mut self) {
        if let Some(t) = self.owner.lock().get_mut(&self.agent) {
            t.subscribers = t.subscribers.saturating_sub(1);
        }
    }
}

/// Every agent's tail. Shared by the agent runtime (reads at `Stop` and at
/// exit) and the API (requests, subscriptions, the poller).
#[derive(Default)]
pub struct Transcripts {
    tails: Mutex<HashMap<AgentId, Tail>>,
}

impl Transcripts {
    pub fn new() -> Arc<Transcripts> {
        Arc::new(Transcripts::default())
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<AgentId, Tail>> {
        self.tails.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Reads what is new in `path` for `agent` and broadcasts it. Blocking.
    pub fn read_at(&self, agent: &AgentId, path: PathBuf) -> Vec<ChatEntry> {
        let mut tails = self.lock();
        tails
            .entry(agent.clone())
            .or_insert_with(Tail::new)
            .read(path)
    }

    /// Reads what is new for `agent` at the path the engine knows. `false`
    /// when there is no such agent.
    pub async fn refresh(self: &Arc<Self>, actor: &ActorHandle, agent: &AgentId) -> bool {
        let a = agent.clone();
        let Ok(Some(path)) = actor.query(move |s| path_of(s, &a)).await else {
            return false;
        };
        let (me, agent) = (self.clone(), agent.clone());
        let _ = tokio::task::spawn_blocking(move || me.read_at(&agent, path)).await;
        true
    }

    /// The kept entries after `after`, without reading.
    pub fn since(&self, agent: &AgentId, after: Option<&str>) -> Vec<ChatEntry> {
        self.lock()
            .get(agent)
            .map(|t| t.after(after))
            .unwrap_or_default()
    }

    /// `GET /agents/:id/transcript`: the entries after `after` (at most
    /// `limit`), or the last `limit` entries without `after`. `None` when the
    /// agent does not exist.
    pub async fn page(
        self: &Arc<Self>,
        actor: &ActorHandle,
        agent: &AgentId,
        after: Option<&str>,
        limit: usize,
    ) -> Option<Vec<ChatEntry>> {
        if !self.refresh(actor, agent).await {
            return None;
        }
        let mut all = self.since(agent, after);
        if after.is_some() {
            all.truncate(limit);
        } else {
            all.drain(..all.len().saturating_sub(limit));
        }
        Some(all)
    }

    /// Starts a chat subscription: reads what is new, then hands out the
    /// entries after `after` and a receiver for later ones, with no gap.
    pub async fn subscribe(
        self: &Arc<Self>,
        actor: &ActorHandle,
        agent: &AgentId,
        after: Option<&str>,
    ) -> Option<ChatSub> {
        if !self.refresh(actor, agent).await {
            return None;
        }
        let mut tails = self.lock();
        let t = tails.entry(agent.clone()).or_insert_with(Tail::new);
        t.subscribers += 1;
        Some(ChatSub {
            catchup: t.after(after),
            rx: t.tx.subscribe(),
            owner: self.clone(),
            agent: agent.clone(),
        })
    }

    /// Agents somebody is subscribed to.
    pub fn watched(&self) -> Vec<AgentId> {
        self.lock()
            .iter()
            .filter(|(_, t)| t.subscribers > 0)
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// The token sums and the last assistant text, if they changed since the
    /// last call for `agent`.
    pub fn take_update(&self, agent: &AgentId) -> Option<(TokenUsage, Option<String>)> {
        let mut tails = self.lock();
        let t = tails.get_mut(agent)?;
        let now = (t.tokens(), t.last_text.clone());
        if t.reported.as_ref() == Some(&now) {
            return None;
        }
        t.reported = Some(now.clone());
        Some(now)
    }

    /// At a `Stop` and when the agent ends: reads the new bytes and tells the
    /// engine the token sums and the last assistant text, if they changed.
    pub async fn report_turn(self: &Arc<Self>, actor: &ActorHandle, agent: &AgentId) {
        if !self.refresh(actor, agent).await {
            return;
        }
        if let Some((tokens, last_text)) = self.take_update(agent) {
            actor.send(Command::Runner(RunnerEvent::TokensUpdated {
                agent: agent.clone(),
                tokens,
                last_text,
            }));
        }
    }

    /// Reads subscribed agents every [`POLL_EVERY`] until `stop`.
    pub async fn poll(self: Arc<Self>, actor: ActorHandle, mut stop: watch::Receiver<bool>) {
        let mut every = tokio::time::interval(POLL_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = every.tick() => {}
                _ = stop.wait_for(|s| *s) => return,
            }
            let watched = self.watched();
            if watched.is_empty() {
                continue;
            }
            let Ok(paths) = actor
                .query(move |s| {
                    watched
                        .into_iter()
                        .filter_map(|a| path_of(s, &a).map(|p| (a, p)))
                        .collect::<Vec<_>>()
                })
                .await
            else {
                return;
            };
            let me = self.clone();
            let _ = tokio::task::spawn_blocking(move || {
                for (agent, path) in paths {
                    me.read_at(&agent, path);
                }
            })
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SESSION: &str = include_str!("../../../cmux-history/tests/fixtures/session.jsonl");

    fn agent() -> AgentId {
        "P-20261018-demo/T-0001".parse().unwrap()
    }

    fn texts(e: &[ChatEntry]) -> Vec<String> {
        e.iter().map(|e| e.text.clone()).collect()
    }

    fn line(text: &str, uuid: &str, role: &str, msg_id: &str) -> String {
        let content = if role == "user" {
            serde_json::json!(text)
        } else {
            serde_json::json!([{"type": "text", "text": text}])
        };
        serde_json::json!({
            "type": role, "uuid": uuid, "timestamp": "2026-10-18T09:00:00.000Z",
            "message": {"role": role, "id": msg_id, "content": content,
                        "usage": {"input_tokens": 1, "output_tokens": 2}},
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn incremental_reads_across_partial_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let t = Transcripts::default();
        let mut rx = {
            let mut tails = t.lock();
            tails
                .entry(agent())
                .or_insert_with(Tail::new)
                .tx
                .subscribe()
        };
        let mut f = std::fs::File::create(&path).unwrap();
        // The session arrives in uneven pieces, some ending mid-line.
        let bytes = SESSION.as_bytes();
        let mut got = Vec::new();
        for part in bytes.chunks(333) {
            f.write_all(part).unwrap();
            got.extend(t.read_at(&agent(), path.clone()));
        }
        let whole = TranscriptReader::new("/x").feed(bytes);
        assert_eq!(got, whole);
        assert_eq!(t.since(&agent(), None), whole);
        // Each read with news was one broadcast batch, in order.
        let mut batches = Vec::new();
        while let Ok(b) = rx.try_recv() {
            batches.extend(b);
        }
        assert_eq!(batches, whole);
        // `after` picks up behind an entry.
        let after = t.since(&agent(), Some(&whole[3].uuid));
        assert_eq!(after, whole[4..].to_vec());
        assert_eq!(t.since(&agent(), Some("unknown")), whole);
    }

    #[test]
    fn a_clear_switches_to_the_new_file_and_keeps_both() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("s1.jsonl"), dir.path().join("s2.jsonl"));
        let t = Transcripts::default();
        std::fs::write(&first, line("hello", "u1", "user", "")).unwrap();
        assert_eq!(texts(&t.read_at(&agent(), first.clone())), ["hello"]);
        // Claude writes the last answer, then `/clear` starts a new file. The
        // tail reads the old file's rest before it moves on.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&first)
            .unwrap();
        f.write_all(line("hi there", "u2", "assistant", "m1").as_bytes())
            .unwrap();
        std::fs::write(&second, line("after clear", "u3", "user", "")).unwrap();
        let got = t.read_at(&agent(), second.clone());
        assert_eq!(texts(&got), ["hi there", "after clear"]);
        // The new file goes on growing; the old one is done.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&second)
            .unwrap();
        let half = line("fresh answer", "u4", "assistant", "m2");
        let (a, b) = half.split_at(30);
        f.write_all(a.as_bytes()).unwrap();
        assert_eq!(t.read_at(&agent(), second.clone()), vec![]);
        f.write_all(b.as_bytes()).unwrap();
        assert_eq!(
            texts(&t.read_at(&agent(), second.clone())),
            ["fresh answer"]
        );
        assert_eq!(
            texts(&t.since(&agent(), None)),
            ["hello", "hi there", "after clear", "fresh answer"]
        );
        // Tokens of both files add up; the last text is the newest answer.
        let (tokens, last) = t.take_update(&agent()).unwrap();
        assert_eq!((tokens.input, tokens.output), (2, 4));
        assert_eq!(last.as_deref(), Some("fresh answer"));
        // Nothing new: nothing to tell the engine.
        assert_eq!(t.take_update(&agent()), None);
    }

    #[test]
    fn subscribers_are_counted_until_dropped() {
        let t = Arc::new(Transcripts::default());
        t.lock().insert(agent(), Tail::new());
        let sub = {
            let mut tails = t.lock();
            let tail = tails.get_mut(&agent()).unwrap();
            tail.subscribers += 1;
            ChatSub {
                catchup: Vec::new(),
                rx: tail.tx.subscribe(),
                owner: t.clone(),
                agent: agent(),
            }
        };
        assert_eq!(t.watched(), vec![agent()]);
        drop(sub);
        assert!(t.watched().is_empty());
    }
}
