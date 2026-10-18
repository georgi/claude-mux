//! The small amount of state the plan UI keeps on top of the server's
//! `StateView`.

use std::collections::HashSet;

use cmux_core::ids::AgentPath;
use cmux_proto::view::{AgentId, PlanId, StateView, TaskId};

/// One selectable tree node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    Plan(PlanId),
    Executor(PlanId),
    Task(PlanId, TaskId),
    /// A sub-agent of a task.
    Sub(AgentId),
    /// The "loose" workspace of terminals outside any plan.
    Loose,
}

impl Node {
    pub fn for_agent(id: &AgentId) -> Node {
        match id.path() {
            AgentPath::Executor => Node::Executor(id.plan().clone()),
            AgentPath::Task(t) => Node::Task(id.plan().clone(), *t),
            AgentPath::Sub(..) => Node::Sub(id.clone()),
        }
    }

    pub fn plan(&self) -> Option<&PlanId> {
        match self {
            Node::Plan(p) | Node::Executor(p) | Node::Task(p, _) => Some(p),
            Node::Sub(a) => Some(a.plan()),
            Node::Loose => None,
        }
    }

    /// The agent behind the node, when it has one (a task without a spawned
    /// agent has none).
    pub fn agent(&self, sv: &StateView) -> Option<AgentId> {
        match self {
            Node::Executor(p) => Some(AgentId::new(p.clone(), AgentPath::Executor)),
            Node::Task(p, t) => sv.task(p, *t).and_then(|t| t.agent.clone()),
            Node::Sub(a) => Some(a.clone()),
            Node::Plan(_) | Node::Loose => None,
        }
    }

    /// The node that holds this one in the tree.
    pub fn parent(&self) -> Option<Node> {
        match self {
            Node::Plan(_) | Node::Loose => None,
            Node::Executor(p) | Node::Task(p, _) => Some(Node::Plan(p.clone())),
            Node::Sub(a) => {
                let t = a.path().task_id()?;
                Some(Node::Task(a.plan().clone(), t))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// Keys go to Claude; the tree is drawn passive.
    #[default]
    Pane,
    Tree,
}

#[derive(Debug, Clone, Default)]
pub struct ViewState {
    pub focus: Focus,
    pub selection: Option<Node>,
    /// Plans and tasks whose children are hidden.
    pub collapsed: HashSet<Node>,
    /// Agents whose finished state the user has looked at (UI design 2.1).
    pub seen: HashSet<AgentId>,
    /// Agents the watchdog flags as stalled, while running.
    pub stalled: HashSet<AgentId>,
    /// Row in the needs-you overlay.
    pub inbox_sel: usize,
    /// Selected card of the board; `None` means the first card.
    pub board_sel: Option<TaskId>,
    /// Spinner frame for working agents.
    pub spinner: usize,
}

impl ViewState {
    pub fn is_collapsed(&self, n: &Node) -> bool {
        self.collapsed.contains(n)
    }

    /// The plan the selection belongs to, else the first plan.
    pub fn current_plan<'a>(&self, sv: &'a StateView) -> Option<&'a PlanId> {
        let sel = self.selection.as_ref().and_then(Node::plan);
        sv.plans
            .iter()
            .map(|p| &p.id)
            .find(|id| Some(*id) == sel)
            .or_else(|| sv.plans.first().map(|p| &p.id))
    }
}
