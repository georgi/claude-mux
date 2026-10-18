//! The pure engine: `apply(state, command, now)` is the only entry point.
//!
//! Design choices where the architecture is silent:
//! - An agent exists in `State` from the moment the engine decides to spawn it,
//!   with `Life::Starting`. The runner answers `SpawnAgent` with
//!   `RunnerEvent::Spawned`, which supplies the session id, the token hash and
//!   the terminal. `SessionStart` then makes the agent `Running`. Until then no
//!   delivery happens and a zero token hash never authenticates.
//! - A command that fails leaves the state untouched: handlers validate first.
//! - Every call that changes the state bumps `rev` once and emits one delta per
//!   changed object; a call that changes nothing emits no delta.
//! - Answers to questions, and kickoff texts, travel as `UserMessage` items.
//! - Delivery by PTY paste marks items `Delivered` at once; the next
//!   `UserPromptSubmit` is the (untracked) confirmation, so delivery is
//!   at-least-once across a restart (step 15 resets unconfirmed ones).

mod client;
mod command;
mod deliver;
mod hooks;
mod spawn;
mod tools;

#[cfg(test)]
mod tests;

pub use command::*;

use crate::ids::{AgentId, ItemId, PlanId, TaskId};
use crate::model::{
    Agent, Delivery, InboxItem, ItemKind, Life, Plan, Recipient, State, Task, Timestamp,
};

/// Applies one command. On `Err` the state is unchanged.
pub fn apply(state: &mut State, cmd: Command, now: Timestamp) -> Result<Outcome, EngineError> {
    let mut cx = Ctx {
        st: state,
        now,
        fx: Vec::new(),
        changes: Vec::new(),
    };
    let reply = cx.dispatch(cmd)?;
    Ok(cx.finish(reply))
}

/// A new inbox item.
pub(crate) struct NewItem {
    pub to: Recipient,
    pub kind: ItemKind,
    pub from: Option<AgentId>,
    pub body: String,
    pub dedupe: Option<String>,
}

/// State plus what this call has produced so far.
pub(crate) struct Ctx<'a> {
    pub st: &'a mut State,
    pub now: Timestamp,
    pub fx: Vec<Effect>,
    changes: Vec<Change>,
}

pub(crate) fn err<T>(kind: ErrorKind, msg: impl AsRef<str>) -> Result<T, EngineError> {
    Err(EngineError::new(kind, msg))
}

impl Ctx<'_> {
    fn dispatch(&mut self, cmd: Command) -> Result<Reply, EngineError> {
        match cmd {
            Command::Client { client, action } => self.client(client, action),
            Command::Hook {
                agent,
                token_hash,
                hook,
            } => {
                let (pi, ai) = self.auth(&agent, &token_hash)?;
                self.hook(pi, ai, hook)
            }
            Command::Tool {
                agent,
                token_hash,
                call,
            } => {
                let (pi, ai) = self.auth(&agent, &token_hash)?;
                self.tool(pi, ai, call)
            }
            Command::Runner(ev) => self.runner(ev).map(|()| Reply::Ok),
            Command::Tick => {
                self.tick();
                Ok(Reply::Ok)
            }
        }
    }

    /// Bumps `rev`, tags the deltas and adds `Persist` effects.
    fn finish(mut self, reply: Reply) -> Outcome {
        if self.changes.is_empty() {
            return Outcome {
                reply,
                effects: self.fx,
                deltas: Vec::new(),
            };
        }
        self.st.rev += 1;
        let rev = self.st.rev;
        let mut plans: Vec<PlanId> = Vec::new();
        for c in &self.changes {
            let plan = match c {
                Change::Plan(p) | Change::Task(p, _) => Some(p.clone()),
                Change::Agent(a) => Some(a.plan().clone()),
                Change::Item(i) => self
                    .st
                    .inbox
                    .iter()
                    .find(|it| it.id == *i)
                    .and_then(|it| {
                        it.from.as_ref().or(match &it.to {
                            Recipient::Agent(a) => Some(a),
                            Recipient::Human => None,
                        })
                    })
                    .map(|a| a.plan().clone()),
                _ => None,
            };
            if let Some(p) = plan.filter(|p| !plans.contains(p)) {
                plans.push(p);
            }
        }
        self.fx
            .extend(plans.into_iter().map(|plan| Effect::Persist { plan }));
        let deltas = self
            .changes
            .into_iter()
            .map(|change| Delta { rev, change })
            .collect();
        Outcome {
            reply,
            effects: self.fx,
            deltas,
        }
    }

    // ---- lookups ----

    pub fn touch(&mut self, c: Change) {
        if !self.changes.contains(&c) {
            self.changes.push(c);
        }
    }

    pub fn plan_idx(&self, id: &PlanId) -> Result<usize, EngineError> {
        match self.st.plans.iter().position(|p| p.id == *id) {
            Some(i) => Ok(i),
            None => err(ErrorKind::NotFound, format!("no plan {id}")),
        }
    }

    pub fn task_idx(&self, pi: usize, id: TaskId) -> Result<usize, EngineError> {
        let plan = &self.st.plans[pi];
        match plan.tasks.iter().position(|t| t.id == id) {
            Some(i) => Ok(i),
            None => {
                let ids: Vec<String> = plan.tasks.iter().map(|t| t.id.to_string()).collect();
                err(
                    ErrorKind::NotFound,
                    format!("no task {id} in {}; tasks: {}", plan.id, ids.join(", ")),
                )
            }
        }
    }

    pub fn agent_idx(&self, id: &AgentId) -> Result<(usize, usize), EngineError> {
        let pi = self.plan_idx(id.plan())?;
        match self.st.plans[pi].agents.iter().position(|a| a.id == *id) {
            Some(ai) => Ok((pi, ai)),
            None => err(ErrorKind::NotFound, format!("no agent {id}")),
        }
    }

    pub fn item_idx(&self, id: ItemId) -> Option<usize> {
        self.st.inbox.iter().position(|i| i.id == id)
    }

    pub fn plan(&self, pi: usize) -> &Plan {
        &self.st.plans[pi]
    }

    pub fn agent(&self, pi: usize, ai: usize) -> &Agent {
        &self.st.plans[pi].agents[ai]
    }

    pub fn plan_mut(&mut self, pi: usize) -> &mut Plan {
        let id = self.st.plans[pi].id.clone();
        self.touch(Change::Plan(id));
        &mut self.st.plans[pi]
    }

    pub fn task_mut(&mut self, pi: usize, ti: usize) -> &mut Task {
        let t = &self.st.plans[pi].tasks[ti];
        let c = Change::Task(self.st.plans[pi].id.clone(), t.id);
        self.touch(c);
        &mut self.st.plans[pi].tasks[ti]
    }

    pub fn agent_mut(&mut self, pi: usize, ai: usize) -> &mut Agent {
        let id = self.st.plans[pi].agents[ai].id.clone();
        self.touch(Change::Agent(id));
        &mut self.st.plans[pi].agents[ai]
    }

    pub fn set_status(&mut self, item: ItemId, status: Delivery) {
        if let Some(i) = self.item_idx(item)
            && self.st.inbox[i].status != status
        {
            self.st.inbox[i].status = status;
            self.touch(Change::Item(item));
        }
    }

    // ---- counting ----

    /// Claude processes that exist or are about to, on the whole server.
    pub fn live_processes(&self) -> u32 {
        let n = self
            .st
            .plans
            .iter()
            .flat_map(|p| &p.agents)
            .filter(|a| a.is_live());
        n.count() as u32
    }

    /// Task agents that hold one of the plan's slots: live and not yet landed.
    pub fn task_slots_used(&self, pi: usize) -> u32 {
        let agents = self.st.plans[pi].agents.iter();
        agents
            .filter(|a| a.path.task_id().is_some() && a.path.sub_name().is_none())
            .filter(|a| a.is_live() && !a.landed)
            .count() as u32
    }

    pub fn subs_used(&self, pi: usize, task: TaskId) -> u32 {
        let agents = self.st.plans[pi].agents.iter();
        agents
            .filter(|a| a.path.sub_name().is_some() && a.task == Some(task))
            .filter(|a| a.is_live() && !a.landed)
            .count() as u32
    }

    // ---- auth ----

    /// The agent must exist, be neither exited nor stopped, and present its token.
    fn auth(&self, id: &AgentId, hash: &[u8; 32]) -> Result<(usize, usize), EngineError> {
        let Ok((pi, ai)) = self.agent_idx(id) else {
            return err(ErrorKind::Auth, format!("unknown agent {id}"));
        };
        let a = self.agent(pi, ai);
        match a.life {
            Life::Exited { .. } => return err(ErrorKind::Auth, format!("{id} has exited")),
            Life::Stopped => return err(ErrorKind::Auth, format!("{id} was stopped")),
            _ => {}
        }
        // Constant-time compare; an unset (zero) hash never matches.
        let diff = a
            .token_hash
            .iter()
            .zip(hash)
            .fold(0u8, |d, (x, y)| d | (x ^ y));
        if diff != 0 || a.token_hash == [0; 32] {
            return err(ErrorKind::Auth, format!("bad token for {id}"));
        }
        Ok((pi, ai))
    }

    // ---- inbox ----

    /// Posts an item unless its dedupe key was used before. Notifies for human
    /// items and tries to wake an idle recipient. Returns the new id.
    pub fn post(&mut self, n: NewItem) -> Option<ItemId> {
        if let Some(key) = &n.dedupe
            && self.st.inbox.iter().any(|i| i.dedupe.as_ref() == Some(key))
        {
            return None;
        }
        let id = ItemId(self.st.next_item_id);
        self.st.next_item_id += 1;
        let human = n.to == Recipient::Human && n.kind.needs_human();
        let to = n.to.clone();
        self.st.inbox.push(InboxItem {
            id,
            to: n.to,
            kind: n.kind,
            from: n.from,
            body: n.body,
            status: Delivery::Pending,
            dedupe: n.dedupe,
            created: self.now,
        });
        self.touch(Change::Item(id));
        if human {
            self.fx.push(Effect::Notify { item: id });
        }
        if let Recipient::Agent(a) = to {
            self.deliver_idle(&a);
        }
        Some(id)
    }

    /// A plain message to an agent, from another agent or from the system.
    pub fn message(&mut self, to: &AgentId, from: Option<AgentId>, body: String) {
        self.post(NewItem {
            to: Recipient::Agent(to.clone()),
            kind: ItemKind::UserMessage,
            from,
            body,
            dedupe: None,
        });
    }
}

/// `(year, month, day)` of a timestamp, UTC (Hinnant's civil-from-days).
pub(crate) fn civil_date(ms: Timestamp) -> (u32, u32, u32) {
    let z = (ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as u32, m as u32, d as u32)
}
