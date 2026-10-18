//! Client-facing snapshots (architecture 3 and 4.1).
//!
//! Choice of delta: the stream carries a *view-level* delta ([`DeltaView`]),
//! not `cmux_core::Delta`. The core delta only names the changed object, so a
//! client would have to refetch it; the view delta carries the whole new
//! object, which is what section 4.1 specifies ("each delta replaces a whole
//! object"). Clients keep a [`StateView`] and call [`StateView::apply`] for
//! each delta; they never merge fields by hand. The server builds deltas with
//! [`DeltaView::from_core`] right after the engine step, while it still holds
//! the state that produced them.
//!
//! One engine step bumps `rev` once and may emit several deltas, so deltas
//! with the same `rev` are normal; `rev` never goes backwards.
//!
//! Views never contain secrets: no agent token hash, no held permission
//! input, no data directory. Task bodies and notes are omitted from
//! [`TaskView`] (fetch [`TaskDetail`]).

use std::path::PathBuf;

use cmux_core::engine::{Change, Delta, ObjectRef as CoreObjectRef};
use cmux_core::ids::AgentPath;
use cmux_core::model::{Agent, InboxItem, Plan, QueuedSpawn, State, Task, Terminal};
use serde::{Deserialize, Serialize};

pub use cmux_core::ids::{AgentId, ClientId, ItemId, PlanId, TaskId, TermId};
pub use cmux_core::model::{
    Activity, Criterion, Delivery, DeliveryPath, ItemKind, Level, Life, Limits, Note, PlanState,
    PrRef, Recipient, Repo, Report, ReportStatus, TaskState, TermKind, Timestamp, TokenUsage,
};

/// Everything a client needs to draw the tree, board and inbox.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateView {
    pub rev: u64,
    pub plans: Vec<PlanView>,
    /// All tasks of all plans; each carries its plan id. Plan order is
    /// `PlanView::tasks`.
    pub tasks: Vec<TaskView>,
    pub agents: Vec<AgentView>,
    pub terminals: Vec<TerminalView>,
    /// Terminals that belong to no agent, in creation order.
    pub loose: Vec<TermId>,
    pub items: Vec<ItemView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    pub id: PlanId,
    pub title: String,
    pub state: PlanState,
    pub repos: Vec<Repo>,
    pub paused: bool,
    pub limits: Limits,
    /// Task ids in plan order.
    pub tasks: Vec<TaskId>,
    /// Agent ids in spawn order (the executor first).
    pub agents: Vec<AgentId>,
    /// Client spawns waiting for a slot or for the plan to resume.
    pub queue: Vec<QueuedSpawn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskView {
    pub plan: PlanId,
    pub id: TaskId,
    pub title: String,
    pub state: TaskState,
    /// Index into `PlanView::repos`.
    pub repo: usize,
    pub deps: Vec<TaskId>,
    pub criteria_done: u32,
    pub criteria_total: u32,
    pub notes: u32,
    pub branch: Option<String>,
    pub worktree: Option<PathBuf>,
    pub pr: Option<PrRef>,
    pub agent: Option<AgentId>,
    pub attempt: u32,
}

/// A task with everything `TaskView` leaves out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: TaskView,
    pub body: String,
    pub criteria: Vec<Criterion>,
    pub notes: Vec<Note>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentView {
    pub id: AgentId,
    pub path: AgentPath,
    pub level: Level,
    pub parent: Option<AgentId>,
    pub task: Option<TaskId>,
    pub term: Option<TermId>,
    pub cwd: PathBuf,
    pub life: Life,
    pub activity: Activity,
    pub report: Option<Report>,
    pub tokens: TokenUsage,
    pub idle_since: Option<Timestamp>,
    /// The watchdog flags it: working, with no progress for a while.
    #[serde(default)]
    pub stalled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalView {
    pub id: TermId,
    pub kind: TermKind,
    pub agent: Option<AgentId>,
    pub cwd: PathBuf,
    pub rows: u16,
    pub cols: u16,
    /// The client whose size the PTY follows.
    pub size_owner: Option<ClientId>,
    pub title: Option<String>,
}

/// An inbox item. Items to agents are included so clients can show queues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemView {
    pub id: ItemId,
    pub to: Recipient,
    pub kind: ItemKind,
    pub from: Option<AgentId>,
    pub body: String,
    pub status: Delivery,
    pub created: Timestamp,
    /// Shown to humans as a "needs you" action.
    pub needs_human: bool,
}

/// A whole-object change at `rev`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaView {
    pub rev: u64,
    pub change: ChangeView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ChangeView {
    Plan(PlanView),
    Task(TaskView),
    Agent(AgentView),
    Terminal(TerminalView),
    Item(ItemView),
    Removed(ObjectRef),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObjectRef {
    Plan { plan: PlanId },
    Task { plan: PlanId, task: TaskId },
    Agent { agent: AgentId },
    Terminal { term: TermId },
    Item { item: ItemId },
}

// ---------------------------------------------------------------------------
// Conversions from cmux-core
// ---------------------------------------------------------------------------

impl From<&Plan> for PlanView {
    fn from(p: &Plan) -> Self {
        PlanView {
            id: p.id.clone(),
            title: p.title.clone(),
            state: p.state,
            repos: p.repos.clone(),
            paused: p.paused,
            limits: p.limits,
            tasks: p.tasks.iter().map(|t| t.id).collect(),
            agents: p.agents.iter().map(|a| a.id.clone()).collect(),
            queue: p.spawn_queue.clone(),
        }
    }
}

impl TaskView {
    pub fn new(plan: &Plan, t: &Task) -> Self {
        TaskView {
            plan: plan.id.clone(),
            id: t.id,
            title: t.title.clone(),
            state: t.state,
            repo: t.repo,
            deps: t.deps.clone(),
            criteria_done: t.criteria.iter().filter(|c| c.checked).count() as u32,
            criteria_total: t.criteria.len() as u32,
            notes: t.notes.len() as u32,
            branch: t.branch.clone(),
            worktree: t.worktree.clone(),
            pr: t.pr.clone(),
            agent: t.agent.clone(),
            attempt: t.attempt,
        }
    }
}

impl TaskDetail {
    pub fn new(plan: &Plan, t: &Task) -> Self {
        TaskDetail {
            task: TaskView::new(plan, t),
            body: t.body.clone(),
            criteria: t.criteria.clone(),
            notes: t.notes.clone(),
            instructions: t.instructions.clone(),
        }
    }
}

impl AgentView {
    /// The queued-item count (the N of `✉N`) is not stored here: an agent
    /// delta would go stale when an item is queued. Use
    /// [`StateView::queued_for`].
    pub fn new(a: &Agent) -> Self {
        AgentView {
            id: a.id.clone(),
            path: a.path.clone(),
            level: a.level,
            parent: a.parent.clone(),
            task: a.task,
            term: a.term,
            cwd: a.cwd.clone(),
            life: a.life,
            activity: a.activity,
            report: a.report.clone(),
            tokens: a.tokens,
            idle_since: a.idle_since,
            stalled: a.stalled,
        }
    }
}

impl From<&Terminal> for TerminalView {
    fn from(t: &Terminal) -> Self {
        TerminalView {
            id: t.id,
            kind: t.kind.clone(),
            agent: t.agent.clone(),
            cwd: t.cwd.clone(),
            cols: t.size.0,
            rows: t.size.1,
            size_owner: t.size_owner.clone(),
            title: t.title.clone(),
        }
    }
}

impl From<&InboxItem> for ItemView {
    fn from(i: &InboxItem) -> Self {
        ItemView {
            id: i.id,
            to: i.to.clone(),
            kind: i.kind.clone(),
            from: i.from.clone(),
            body: i.body.clone(),
            status: i.status.clone(),
            created: i.created,
            needs_human: i.kind.needs_human(),
        }
    }
}

impl From<&State> for StateView {
    fn from(s: &State) -> Self {
        StateView {
            rev: s.rev,
            plans: s.plans.iter().map(PlanView::from).collect(),
            tasks: s
                .plans
                .iter()
                .flat_map(|p| p.tasks.iter().map(move |t| TaskView::new(p, t)))
                .collect(),
            agents: s
                .plans
                .iter()
                .flat_map(|p| p.agents.iter())
                .map(AgentView::new)
                .collect(),
            terminals: s.terminals.values().map(TerminalView::from).collect(),
            loose: s.loose.clone(),
            items: s.inbox.iter().map(ItemView::from).collect(),
        }
    }
}

impl From<&CoreObjectRef> for ObjectRef {
    fn from(o: &CoreObjectRef) -> Self {
        match o {
            CoreObjectRef::Plan(p) => ObjectRef::Plan { plan: p.clone() },
            CoreObjectRef::Task(p, t) => ObjectRef::Task {
                plan: p.clone(),
                task: *t,
            },
            CoreObjectRef::Agent(a) => ObjectRef::Agent { agent: a.clone() },
            CoreObjectRef::Terminal(t) => ObjectRef::Terminal { term: *t },
            CoreObjectRef::Item(i) => ObjectRef::Item { item: *i },
        }
    }
}

impl DeltaView {
    /// Build the view delta for a core delta from `state`, the state after the
    /// step. Returns `None` when the object no longer exists (a later
    /// `Removed` delta covers it).
    pub fn from_core(state: &State, d: &Delta) -> Option<DeltaView> {
        let find_plan = |id: &PlanId| state.plans.iter().find(|p| &p.id == id);
        let change = match &d.change {
            Change::Plan(p) => ChangeView::Plan(PlanView::from(find_plan(p)?)),
            Change::Task(p, t) => {
                let plan = find_plan(p)?;
                ChangeView::Task(TaskView::new(plan, plan.task(*t)?))
            }
            Change::Agent(a) => {
                let agent = find_plan(a.plan())?.agent(a)?;
                ChangeView::Agent(AgentView::new(agent))
            }
            Change::Terminal(t) => {
                ChangeView::Terminal(TerminalView::from(state.terminals.get(t)?))
            }
            Change::Item(i) => {
                ChangeView::Item(ItemView::from(state.inbox.iter().find(|x| x.id == *i)?))
            }
            Change::Removed(o) => ChangeView::Removed(ObjectRef::from(o)),
        };
        Some(DeltaView { rev: d.rev, change })
    }
}

// ---------------------------------------------------------------------------
// Client-side apply
// ---------------------------------------------------------------------------

fn upsert<T>(list: &mut Vec<T>, new: T, same: impl Fn(&T) -> bool) {
    match list.iter_mut().find(|x| same(x)) {
        Some(slot) => *slot = new,
        None => list.push(new),
    }
}

impl StateView {
    /// Apply one delta and advance `rev` (never backwards).
    pub fn apply(&mut self, d: &DeltaView) {
        self.rev = self.rev.max(d.rev);
        match &d.change {
            ChangeView::Plan(p) => upsert(&mut self.plans, p.clone(), |x| x.id == p.id),
            ChangeView::Task(t) => upsert(&mut self.tasks, t.clone(), |x| {
                x.plan == t.plan && x.id == t.id
            }),
            ChangeView::Agent(a) => upsert(&mut self.agents, a.clone(), |x| x.id == a.id),
            ChangeView::Terminal(t) => {
                upsert(&mut self.terminals, t.clone(), |x| x.id == t.id);
                // Loose terminals have no agent; the order list follows.
                let loose = t.agent.is_none();
                if loose && !self.loose.contains(&t.id) {
                    self.loose.push(t.id);
                }
            }
            ChangeView::Item(i) => upsert(&mut self.items, i.clone(), |x| x.id == i.id),
            ChangeView::Removed(o) => match o {
                ObjectRef::Plan { plan } => {
                    self.plans.retain(|p| &p.id != plan);
                    self.tasks.retain(|t| &t.plan != plan);
                    self.agents.retain(|a| a.id.plan() != plan);
                }
                ObjectRef::Task { plan, task } => {
                    self.tasks.retain(|t| !(&t.plan == plan && t.id == *task));
                }
                ObjectRef::Agent { agent } => self.agents.retain(|a| &a.id != agent),
                ObjectRef::Terminal { term } => {
                    self.terminals.retain(|t| t.id != *term);
                    self.loose.retain(|t| t != term);
                }
                ObjectRef::Item { item } => self.items.retain(|i| i.id != *item),
            },
        }
    }

    pub fn plan(&self, id: &PlanId) -> Option<&PlanView> {
        self.plans.iter().find(|p| &p.id == id)
    }
    pub fn task(&self, plan: &PlanId, id: TaskId) -> Option<&TaskView> {
        self.tasks.iter().find(|t| &t.plan == plan && t.id == id)
    }
    pub fn agent(&self, id: &AgentId) -> Option<&AgentView> {
        self.agents.iter().find(|a| &a.id == id)
    }
    pub fn terminal(&self, id: TermId) -> Option<&TerminalView> {
        self.terminals.iter().find(|t| t.id == id)
    }
    /// Items queued for `agent` and not yet delivered (the N of `✉N`).
    pub fn queued_for(&self, agent: &AgentId) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == Delivery::Pending && i.to == Recipient::Agent(agent.clone()))
            .count()
    }
    /// A client spawn is queued for this task (see `PlanView::queue`).
    pub fn is_queued(&self, plan: &PlanId, task: TaskId) -> bool {
        self.plan(plan)
            .is_some_and(|p| p.queue.iter().any(|q| q.task == task))
    }
    pub fn item(&self, id: ItemId) -> Option<&ItemView> {
        self.items.iter().find(|i| i.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::engine::{ClientAction, Command, apply};
    use cmux_core::ids::AgentPath;
    use cmux_core::model::{Delivery, ItemKind};

    fn rt<T: Serialize + for<'a> Deserialize<'a> + PartialEq + std::fmt::Debug>(v: T) -> String {
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<T>(&s).unwrap(), v, "{s}");
        s
    }

    fn client() -> ClientId {
        ClientId("c1".into())
    }

    /// A state with one plan, two tasks, an agent with a secret hash, a loose
    /// terminal and a human item.
    fn sample() -> (State, PlanId) {
        let mut st = State::default();
        let out = apply(
            &mut st,
            Command::Client {
                client: client(),
                action: ClientAction::CreatePlan {
                    title: "demo".into(),
                    repos: vec![Repo {
                        path: "/r".into(),
                        base_branch: "main".into(),
                        remote_url: None,
                    }],
                    request: Some("build".into()),
                },
            },
            1000,
        )
        .unwrap();
        assert!(!out.deltas.is_empty());
        let plan = st.plans[0].id.clone();
        for n in 1..=2 {
            apply(
                &mut st,
                Command::Client {
                    client: client(),
                    action: ClientAction::CreateTask {
                        plan: plan.clone(),
                        title: format!("task {n}"),
                        body: "SECRET BODY".into(),
                        repo: 0,
                        deps: vec![],
                        criteria: vec!["a".into(), "b".into()],
                    },
                },
                1000 + n,
            )
            .unwrap();
        }
        let id = AgentId::new(plan.clone(), AgentPath::Task(TaskId::new(1)));
        let mut agent = Agent::new(id.clone(), "sess-1", "/wt".into(), [0xab; 32]);
        agent.term = Some(TermId(1));
        st.plans[0].agents.push(agent);
        st.plans[0].tasks[0].agent = Some(id);
        st.terminals.insert(
            TermId(2),
            Terminal {
                id: TermId(2),
                kind: TermKind::Shell,
                agent: None,
                cwd: "/home".into(),
                size: (100, 30),
                size_owner: Some(client()),
                title: None,
            },
        );
        st.loose.push(TermId(2));
        st.inbox.push(InboxItem {
            id: ItemId(1),
            to: Recipient::Human,
            kind: ItemKind::Question,
            from: None,
            body: "why?".into(),
            status: Delivery::Pending,
            dedupe: None,
            created: 5,
        });
        (st, plan)
    }

    #[test]
    fn state_view_mirrors_state() {
        let (st, plan) = sample();
        let v = StateView::from(&st);
        assert_eq!(v.rev, st.rev);
        assert_eq!(v.plans.len(), 1);
        assert_eq!(v.plans[0].tasks, vec![TaskId::new(1), TaskId::new(2)]);
        assert_eq!(v.plans[0].agents.len(), 2); // executor + ours
        assert_eq!(v.tasks.len(), 2);
        let t = v.task(&plan, TaskId::new(1)).unwrap();
        assert_eq!((t.criteria_total, t.criteria_done), (2, 0));
        assert_eq!(
            t.agent.as_ref().map(|a| a.path().clone()),
            Some(AgentPath::Task(TaskId::new(1)))
        );
        let term = v.terminal(TermId(2)).unwrap();
        assert_eq!((term.cols, term.rows), (100, 30));
        assert_eq!(v.loose, vec![TermId(2)]);
        assert!(v.item(ItemId(1)).unwrap().needs_human);
        assert!(
            v.agents
                .iter()
                .any(|a| a.path == AgentPath::Task(TaskId::new(1)))
        );
        rt(v);
    }

    #[test]
    fn views_leak_no_secrets() {
        let (st, plan) = sample();
        let json = serde_json::to_string(&StateView::from(&st)).unwrap();
        assert!(!json.contains(&"ab".repeat(32)), "token hash leaked");
        assert!(!json.contains("token_hash"));
        assert!(!json.contains("sess-1"), "session id leaked");
        assert!(!json.contains("SECRET BODY"), "task body in the snapshot");
        let d = TaskDetail::new(&st.plans[0], &st.plans[0].tasks[0]);
        assert_eq!(d.body, "SECRET BODY");
        assert_eq!(d.criteria.len(), 2);
        assert_eq!(d.task.plan, plan);
        rt(d);
    }

    #[test]
    fn agent_queued_counts_pending_items() {
        let (mut st, plan) = sample();
        let id = AgentId::new(plan, AgentPath::Task(TaskId::new(1)));
        st.inbox.push(InboxItem {
            id: ItemId(2),
            to: Recipient::Agent(id.clone()),
            kind: ItemKind::UserMessage,
            from: None,
            body: "hi".into(),
            status: Delivery::Pending,
            dedupe: None,
            created: 6,
        });
        let v = StateView::from(&st);
        assert_eq!(v.queued_for(&id), 1);
        assert!(!v.item(ItemId(2)).unwrap().needs_human);
    }

    #[test]
    fn deltas_from_engine_apply_to_a_view() {
        // Mirror a client: start from an empty snapshot and feed it every delta.
        let mut st = State::default();
        let mut mirror = StateView::from(&st);
        let step = |st: &mut State, mirror: &mut StateView, action: ClientAction| {
            let out = apply(
                st,
                Command::Client {
                    client: client(),
                    action,
                },
                10,
            )
            .unwrap();
            for d in &out.deltas {
                if let Some(dv) = DeltaView::from_core(st, d) {
                    let back: DeltaView =
                        serde_json::from_str(&serde_json::to_string(&dv).unwrap()).unwrap();
                    assert_eq!(back, dv);
                    mirror.apply(&back);
                }
            }
        };
        step(
            &mut st,
            &mut mirror,
            ClientAction::CreatePlan {
                title: "demo".into(),
                repos: vec![Repo {
                    path: "/r".into(),
                    base_branch: "main".into(),
                    remote_url: None,
                }],
                request: Some("x".into()),
            },
        );
        let plan = st.plans[0].id.clone();
        step(
            &mut st,
            &mut mirror,
            ClientAction::CreateTask {
                plan: plan.clone(),
                title: "t".into(),
                body: "".into(),
                repo: 0,
                deps: vec![],
                criteria: vec![],
            },
        );
        step(&mut st, &mut mirror, ClientAction::AcceptPlan { plan });
        assert!(mirror.rev > 0);
        assert_eq!(mirror, StateView::from(&st));
    }

    #[test]
    fn apply_upserts_and_removes() {
        let (st, plan) = sample();
        let mut v = StateView::from(&st);
        // Replace, not duplicate.
        let mut t = v.tasks[0].clone();
        t.title = "renamed".into();
        v.apply(&DeltaView {
            rev: 50,
            change: ChangeView::Task(t),
        });
        assert_eq!(v.tasks.len(), 2);
        assert_eq!(v.tasks[0].title, "renamed");
        assert_eq!(v.rev, 50);
        // rev never goes backwards.
        v.apply(&DeltaView {
            rev: 3,
            change: ChangeView::Removed(ObjectRef::Item { item: ItemId(1) }),
        });
        assert_eq!(v.rev, 50);
        assert!(v.items.is_empty());
        // A new loose terminal joins the order list; removing drops both.
        let mut term = v.terminals[0].clone();
        term.id = TermId(9);
        v.apply(&DeltaView {
            rev: 51,
            change: ChangeView::Terminal(term),
        });
        assert_eq!(v.loose, vec![TermId(2), TermId(9)]);
        v.apply(&DeltaView {
            rev: 52,
            change: ChangeView::Removed(ObjectRef::Terminal { term: TermId(9) }),
        });
        assert_eq!(v.loose, vec![TermId(2)]);
        v.apply(&DeltaView {
            rev: 53,
            change: ChangeView::Removed(ObjectRef::Task {
                plan: plan.clone(),
                task: TaskId::new(2),
            }),
        });
        assert_eq!(v.tasks.len(), 1);
        v.apply(&DeltaView {
            rev: 54,
            change: ChangeView::Removed(ObjectRef::Plan { plan }),
        });
        assert!(v.plans.is_empty() && v.tasks.is_empty() && v.agents.is_empty());
    }

    #[test]
    fn from_core_skips_vanished_objects() {
        let (st, plan) = sample();
        let gone = Delta {
            rev: 1,
            change: Change::Task(plan, TaskId::new(99)),
        };
        assert!(DeltaView::from_core(&st, &gone).is_none());
        let rm = Delta {
            rev: 2,
            change: Change::Removed(CoreObjectRef::Terminal(TermId(5))),
        };
        assert_eq!(
            DeltaView::from_core(&st, &rm).unwrap().change,
            ChangeView::Removed(ObjectRef::Terminal { term: TermId(5) })
        );
    }

    #[test]
    fn every_change_variant_round_trips() {
        let (st, plan) = sample();
        let v = StateView::from(&st);
        let agent = v.agents[0].id.clone();
        let removed = [
            ObjectRef::Plan { plan: plan.clone() },
            ObjectRef::Task {
                plan,
                task: TaskId::new(1),
            },
            ObjectRef::Agent { agent },
            ObjectRef::Terminal { term: TermId(2) },
            ObjectRef::Item { item: ItemId(1) },
        ];
        let mut changes = vec![
            ChangeView::Plan(v.plans[0].clone()),
            ChangeView::Task(v.tasks[0].clone()),
            ChangeView::Agent(v.agents[0].clone()),
            ChangeView::Terminal(v.terminals[0].clone()),
            ChangeView::Item(v.items[0].clone()),
        ];
        changes.extend(removed.into_iter().map(ChangeView::Removed));
        for change in changes {
            let s = rt(DeltaView { rev: 3, change });
            assert!(s.contains(r#""kind":""#));
        }
    }
}
