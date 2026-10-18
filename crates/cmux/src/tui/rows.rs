//! The flat list of visible tree rows. Drawing, navigation and hit-testing
//! all start from [`tree_rows`], so they agree on what row N is.

use cmux_core::glyph::Glyph;
use cmux_core::ids::AgentPath;
use cmux_proto::view::{
    Activity, AgentView, Delivery, ItemKind, ItemView, Life, PlanState, PlanView, Recipient,
    ReportStatus, StateView, TaskState, TaskView,
};
use ratatui::style::Color;

use super::state::{Node, ViewState};
use super::theme;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub node: Node,
    /// 0 for plans and loose, 1 for executor and tasks, 2 for sub-agents.
    pub depth: u8,
    /// Has children that can be hidden.
    pub expandable: bool,
    pub expanded: bool,
    /// The status mark (glyph, `◆`, `✓`, ...).
    pub mark: char,
    pub mark_color: Color,
    /// `├ ` or `└ ` in front of a sub-agent's label.
    pub branch: Option<char>,
    pub label: String,
    /// Right-aligned detail: state, PR, dependency, `✉N`.
    pub right: String,
    pub right_color: Color,
    /// Finished sub-agents and cancelled work are dimmed.
    pub dim: bool,
}

/// Rows for the current state: collapsed nodes hide their children.
pub fn tree_rows(sv: &StateView, vs: &ViewState) -> Vec<Row> {
    build(sv, vs, false)
}

/// Rows with everything expanded; the order the jump-to-needs-you cycle uses.
pub fn all_rows(sv: &StateView, vs: &ViewState) -> Vec<Row> {
    build(sv, vs, true)
}

fn build(sv: &StateView, vs: &ViewState, expand_all: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    for plan in &sv.plans {
        let plan_node = Node::Plan(plan.id.clone());
        let open = expand_all || !vs.is_collapsed(&plan_node);
        rows.push(plan_row(plan, open));
        if !open {
            continue;
        }
        let exec_id = cmux_proto::view::AgentId::new(plan.id.clone(), AgentPath::Executor);
        if let Some(exec) = sv.agent(&exec_id) {
            rows.push(executor_row(sv, vs, plan, exec));
        }
        for tid in &plan.tasks {
            let Some(task) = sv.task(&plan.id, *tid) else {
                continue;
            };
            let subs = sub_agents(sv, task);
            let node = Node::Task(plan.id.clone(), task.id);
            let task_open = expand_all || !vs.is_collapsed(&node);
            rows.push(task_row(sv, vs, task, !subs.is_empty(), task_open));
            if task_open {
                for (i, sub) in subs.iter().enumerate() {
                    rows.push(sub_row(sv, vs, sub, i + 1 == subs.len()));
                }
            }
        }
    }
    if !sv.loose.is_empty() {
        rows.push(Row {
            node: Node::Loose,
            depth: 0,
            expandable: false,
            expanded: false,
            mark: '$',
            mark_color: theme::GREY,
            branch: None,
            label: "loose".into(),
            right: sv.loose.len().to_string(),
            right_color: theme::GREY,
            dim: false,
        });
    }
    rows
}

fn sub_agents<'a>(sv: &'a StateView, task: &TaskView) -> Vec<&'a AgentView> {
    sv.agents
        .iter()
        .filter(|a| {
            a.id.plan() == &task.plan
                && matches!(a.id.path(), AgentPath::Sub(t, _) if *t == task.id)
        })
        .collect()
}

fn plan_row(plan: &PlanView, open: bool) -> Row {
    let (mark, color) = match plan.state {
        PlanState::Accepted => ('◆', theme::ACCENT),
        PlanState::Done => ('✓', theme::GREEN),
        PlanState::Cancelled => ('✕', theme::GREY),
        PlanState::Draft | PlanState::Proposed => ('○', theme::GREY),
    };
    let right = if plan.paused {
        "paused".to_string()
    } else {
        plan.state.label().to_string()
    };
    Row {
        node: Node::Plan(plan.id.clone()),
        depth: 0,
        expandable: true,
        expanded: open,
        mark,
        mark_color: color,
        branch: None,
        label: plan.title.clone(),
        right,
        right_color: theme::plan_state_color(plan.state),
        dim: plan.state == PlanState::Cancelled,
    }
}

fn executor_row(sv: &StateView, vs: &ViewState, plan: &PlanView, exec: &AgentView) -> Row {
    let g = agent_glyph(sv, vs, exec);
    Row {
        node: Node::Executor(plan.id.clone()),
        depth: 1,
        expandable: false,
        expanded: false,
        mark: theme::glyph_char(g, vs.spinner),
        mark_color: theme::glyph_color(g),
        branch: None,
        label: "◆ executor".into(),
        right: super::marks::queue_mark(sv.queued_for(&exec.id)),
        right_color: theme::BLUE,
        dim: false,
    }
}

fn task_row(sv: &StateView, vs: &ViewState, task: &TaskView, has_subs: bool, open: bool) -> Row {
    let agent = task.agent.as_ref().and_then(|a| sv.agent(a));
    let g = task_glyph(sv, vs, task, agent);
    let queued = agent.map_or(0, |a| sv.queued_for(&a.id));
    let unmet: Vec<String> = task
        .deps
        .iter()
        .filter(|d| {
            sv.task(&task.plan, **d)
                .is_some_and(|t| t.state != TaskState::Merged)
        })
        .map(ToString::to_string)
        .collect();
    let mut right = if let Some(pr) = &task.pr {
        super::marks::pr_mark(pr)
    } else if task.state == TaskState::Todo && sv.is_queued(&task.plan, task.id) {
        "queued".to_string()
    } else if task.state == TaskState::Todo && !unmet.is_empty() {
        format!("← {}", unmet.join(","))
    } else if agent.is_some_and(|a| a.activity == Activity::NeedsPermission) {
        "asks".to_string()
    } else {
        task.state.label().to_string()
    };
    if queued > 0 {
        right = format!("{} {right}", super::marks::queue_mark(queued));
    }
    Row {
        node: Node::Task(task.plan.clone(), task.id),
        depth: 1,
        expandable: has_subs,
        expanded: open,
        mark: theme::glyph_char(g, vs.spinner),
        mark_color: theme::glyph_color(g),
        branch: None,
        label: format!("{} {}", task.id, task.title),
        right,
        right_color: theme::task_state_color(task.state),
        dim: task.state == TaskState::Cancelled,
    }
}

fn sub_row(sv: &StateView, vs: &ViewState, sub: &AgentView, last: bool) -> Row {
    let g = agent_glyph(sv, vs, sub);
    let name = sub.id.path().sub_name().unwrap_or_default().to_string();
    Row {
        node: Node::Sub(sub.id.clone()),
        depth: 2,
        expandable: false,
        expanded: false,
        mark: theme::glyph_char(g, vs.spinner),
        mark_color: theme::glyph_color(g),
        branch: Some(if last { '└' } else { '├' }),
        label: name,
        right: super::marks::queue_mark(sv.queued_for(&sub.id)),
        right_color: theme::BLUE,
        dim: !is_live(sub) && sub.report.is_some(),
    }
}

fn is_live(a: &AgentView) -> bool {
    matches!(a.life, Life::Starting | Life::Running)
}

/// The glyph a task row shows: its state when settled, else its agent's.
fn task_glyph(sv: &StateView, vs: &ViewState, task: &TaskView, agent: Option<&AgentView>) -> Glyph {
    match task.state {
        TaskState::Merged => return Glyph::Success,
        TaskState::Cancelled | TaskState::Failing => return Glyph::Failed,
        _ => {}
    }
    match agent {
        Some(a) => agent_glyph(sv, vs, a),
        None if task.state == TaskState::Blocked => Glyph::Failed,
        None => Glyph::Idle,
    }
}

/// The agent glyph of UI design 2.1, from views. Same precedence as
/// `cmux_core::glyph::glyph`, which works on the core model.
pub fn agent_glyph(sv: &StateView, vs: &ViewState, a: &AgentView) -> Glyph {
    let running = a.life == Life::Running;
    let asks = sv.items.iter().any(|i| {
        i.status == Delivery::Pending
            && i.to == Recipient::Human
            && i.from.as_ref() == Some(&a.id)
            && matches!(i.kind, ItemKind::Permission { .. } | ItemKind::Question)
    });
    if asks || (running && a.activity == Activity::NeedsPermission) {
        return Glyph::NeedsYou;
    }
    if running && (a.stalled || vs.stalled.contains(&a.id)) {
        return Glyph::Stalled;
    }
    match a.life {
        Life::Stopped
        | Life::Exited { code: None }
        | Life::Exited {
            code: Some(1..) | Some(..=-1),
        } => return Glyph::Failed,
        Life::Starting => return Glyph::Working,
        _ => {}
    }
    let working = running && a.activity == Activity::Working;
    let finished = !running || a.report.is_some();
    if finished && !working {
        if !vs.seen.contains(&a.id) {
            return Glyph::Unseen;
        }
        return match &a.report {
            Some(r) if r.status == ReportStatus::Success => Glyph::Success,
            Some(_) => Glyph::Failed,
            None => Glyph::Idle,
        };
    }
    if working {
        Glyph::Working
    } else if sv
        .agents
        .iter()
        .any(|c| c.parent.as_ref() == Some(&a.id) && is_live(c))
    {
        Glyph::Parked
    } else {
        Glyph::Idle
    }
}

// ---------------------------------------------------------------------------
// Needs-you items
// ---------------------------------------------------------------------------

/// Items that wait for a human, oldest first.
pub fn needs_you(sv: &StateView) -> Vec<&ItemView> {
    let mut v: Vec<&ItemView> = sv
        .items
        .iter()
        .filter(|i| i.needs_human && i.to == Recipient::Human && i.status == Delivery::Pending)
        .collect();
    v.sort_by_key(|i| (i.created, i.id));
    v
}

/// The tree node an item points at (the asking agent).
pub fn item_node(item: &ItemView) -> Option<Node> {
    item.from.as_ref().map(Node::for_agent)
}

/// `T-0004` for task agents, the plan title for the executor.
pub fn item_who(sv: &StateView, item: &ItemView) -> String {
    match &item.from {
        None => "?".into(),
        Some(a) => match a.path() {
            AgentPath::Executor => sv
                .plan(a.plan())
                .map_or_else(|| "executor".into(), |p| p.title.clone()),
            AgentPath::Task(t) => t.to_string(),
            AgentPath::Sub(t, n) => format!("{t}/{n}"),
        },
    }
}

/// One line: what is asked.
pub fn item_what(item: &ItemView) -> String {
    match &item.kind {
        ItemKind::Permission { tool, .. } => format!("allow {tool}?"),
        ItemKind::Question => item.body.lines().next().unwrap_or("").to_string(),
        ItemKind::PlanReview => "plan to review".into(),
        ItemKind::Stalled => "stalled".into(),
        ItemKind::ExitedWithoutReport => "exited without report".into(),
        _ => item.body.lines().next().unwrap_or("").to_string(),
    }
}

pub fn item_label(sv: &StateView, item: &ItemView) -> String {
    format!("{} {}", item_who(sv, item), item_what(item))
}

/// First index of the window of `visible` rows that keeps `sel` in view,
/// with the selection pinned to the bottom edge once it scrolls.
pub fn window_start(sel: usize, visible: usize) -> usize {
    (sel + 1).saturating_sub(visible)
}
