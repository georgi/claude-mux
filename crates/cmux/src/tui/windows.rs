//! Windows derived from server state (architecture 11.1).
//!
//! The server knows plans, tasks, agents and terminals; windows and panes are
//! a client idea. [`derive`] is a pure function of a [`StateView`] and the
//! per-server [`ServerView`]:
//!
//! - a plan is a **workspace**; its executor is the pinned first window;
//! - a task that has an agent is a **window**, in plan order; the task's agent
//!   is the main pane and its sub-agents are the other panes, in spawn order;
//! - terminals that belong to no agent (shells, lazygit) are windows of their
//!   own, after the tasks, in every workspace.
//!
//! [`ServerView`] is what the user decided and the server cannot know: the
//! current workspace and window, window names, layout preset, zoom, and which
//! finished sub-agents were closed. It is saved in `view-<server>.toml`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use cmux_proto::view::{
    AgentId, AgentView, Level, PlanId, StateView, TaskId, TaskState, TermId, TermKind,
};
use serde::{Deserialize, Serialize};

use crate::conn::servers::write_private;

/// Identifies a window across state updates and restarts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowKey {
    Executor(PlanId),
    Task(PlanId, TaskId),
    Loose(TermId),
}

impl WindowKey {
    /// The key as stored in `view-<server>.toml`.
    pub fn encode(&self) -> String {
        match self {
            WindowKey::Executor(p) => format!("executor {p}"),
            WindowKey::Task(p, t) => format!("task {p} {t}"),
            WindowKey::Loose(t) => format!("term {t}"),
        }
    }

    pub fn decode(s: &str) -> Option<WindowKey> {
        let mut it = s.split(' ');
        match (it.next()?, it.next()?, it.next(), it.next()) {
            ("executor", p, None, _) => Some(WindowKey::Executor(p.parse().ok()?)),
            ("task", p, Some(t), None) => Some(WindowKey::Task(p.parse().ok()?, t.parse().ok()?)),
            ("term", t, None, _) => Some(WindowKey::Loose(TermId(t.parse().ok()?))),
            _ => None,
        }
    }

    #[allow(dead_code)] // used by tests and the view file code
    pub fn plan(&self) -> Option<&PlanId> {
        match self {
            WindowKey::Executor(p) | WindowKey::Task(p, _) => Some(p),
            WindowKey::Loose(_) => None,
        }
    }
}

/// One pane of a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpec {
    /// `None` for a loose terminal.
    pub agent: Option<AgentId>,
    /// `None` while the agent is starting and has no terminal yet.
    pub term: Option<TermId>,
    pub level: Option<Level>,
    /// The agent is done (see `marks::agent_done`): its process is over, or it
    /// reported and is not working. Drawn dimmed; closing it is local.
    pub finished: bool,
}

impl PaneSpec {
    /// A stable key for per-pane view state: the agent id, else `term N`.
    #[allow(dead_code)] // used by tests and the view file code
    pub fn key(&self) -> String {
        match (&self.agent, self.term) {
            (Some(a), _) => a.to_string(),
            (None, Some(t)) => format!("term {t}"),
            (None, None) => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSpec {
    pub key: WindowKey,
    /// Tab label: `executor`, `T-0002 api`, or the terminal's title.
    pub title: String,
    /// The task's state, for task windows.
    pub task_state: Option<TaskState>,
    /// The main pane first, then sub-agents in spawn order.
    pub panes: Vec<PaneSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub plan: PlanId,
    pub title: String,
    /// The executor first, then the tasks, then the loose terminals.
    pub windows: Vec<WindowSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Derived {
    pub workspaces: Vec<Workspace>,
    /// Windows for terminals that belong to no plan; also shown in every
    /// workspace, and the only windows when there are no plans.
    pub loose: Vec<WindowSpec>,
}

impl Derived {
    pub fn workspace(&self, plan: &PlanId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| &w.plan == plan)
    }

    /// The windows of the tab bar for `plan` (or only the loose ones).
    pub fn windows(&self, plan: Option<&PlanId>) -> &[WindowSpec] {
        match plan.and_then(|p| self.workspace(p)) {
            Some(w) => &w.windows,
            None => &self.loose,
        }
    }

    /// Moves the loose terminals in `view.attached` into the panes of their
    /// window. A terminal whose window is gone stays a window of its own.
    pub fn attach(&mut self, view: &ServerView) {
        if view.attached.is_empty() {
            return;
        }
        let pairs: Vec<(TermId, WindowKey)> = view
            .attached
            .iter()
            .filter_map(|(t, k)| Some((TermId(t.parse().ok()?), WindowKey::decode(k)?)))
            .collect();
        let apply = |wins: &mut Vec<WindowSpec>| {
            for (term, target) in &pairs {
                if !wins.iter().any(|w| &w.key == target) {
                    continue;
                }
                wins.retain(|w| w.key != WindowKey::Loose(*term));
                if let Some(w) = wins.iter_mut().find(|w| &w.key == target) {
                    w.panes.push(PaneSpec {
                        agent: None,
                        term: Some(*term),
                        level: None,
                        finished: false,
                    });
                }
            }
        };
        for ws in &mut self.workspaces {
            apply(&mut ws.windows);
        }
        apply(&mut self.loose);
    }

    #[allow(dead_code)] // used by tests and the view file code
    pub fn window(&self, key: &WindowKey) -> Option<&WindowSpec> {
        self.windows(key.plan()).iter().find(|w| &w.key == key)
    }

    /// The workspace and window to show: what the view state names if it still
    /// exists, else the first plan and its first window.
    pub fn current(&self, view: &ServerView) -> (Option<PlanId>, Option<WindowKey>) {
        let plan = view
            .plan
            .as_ref()
            .and_then(|p| p.parse::<PlanId>().ok())
            .filter(|p| self.workspace(p).is_some())
            .or_else(|| self.workspaces.first().map(|w| w.plan.clone()));
        let wins = self.windows(plan.as_ref());
        let key = view
            .window
            .as_deref()
            .and_then(WindowKey::decode)
            .filter(|k| wins.iter().any(|w| &w.key == k))
            .or_else(|| wins.first().map(|w| w.key.clone()));
        (plan, key)
    }
}

/// Derives the workspaces and windows. Pure: the same inputs give the same
/// output, and nothing depends on the order of `sv.agents` or `sv.tasks`.
pub fn derive(sv: &StateView, view: &ServerView) -> Derived {
    let loose: Vec<WindowSpec> = sv
        .loose
        .iter()
        .filter_map(|id| sv.terminal(*id))
        .map(|t| {
            let title = view
                .name_of(&WindowKey::Loose(t.id))
                .map(str::to_string)
                .or_else(|| t.title.clone().filter(|s| !s.trim().is_empty()))
                .unwrap_or_else(|| match &t.kind {
                    TermKind::Command(label) => label.clone(),
                    TermKind::Shell => "shell".into(),
                    TermKind::Claude => "claude".into(),
                });
            WindowSpec {
                key: WindowKey::Loose(t.id),
                title,
                task_state: None,
                panes: vec![PaneSpec {
                    agent: None,
                    term: Some(t.id),
                    level: None,
                    finished: false,
                }],
            }
        })
        .collect();

    let workspaces = sv
        .plans
        .iter()
        .map(|plan| {
            let mut windows = Vec::new();
            let agents: Vec<&AgentView> =
                plan.agents.iter().filter_map(|id| sv.agent(id)).collect();

            if let Some(exec) = agents.iter().find(|a| a.level == Level::Executor) {
                let key = WindowKey::Executor(plan.id.clone());
                windows.push(WindowSpec {
                    title: view
                        .name_of(&key)
                        .map_or_else(|| "executor".to_string(), str::to_string),
                    key,
                    task_state: None,
                    panes: vec![pane_of(exec)],
                });
            }

            for tid in &plan.tasks {
                let Some(task) = sv.task(&plan.id, *tid) else {
                    continue;
                };
                let Some(main) = task.agent.as_ref().and_then(|a| sv.agent(a)) else {
                    continue; // not started: no window yet
                };
                let mut panes = vec![pane_of(main)];
                let task_over = matches!(
                    task.state,
                    TaskState::Merged | TaskState::Cancelled | TaskState::Passing
                );
                for a in agents
                    .iter()
                    .filter(|a| a.level == Level::Sub && a.task == Some(*tid))
                {
                    let p = pane_of(a);
                    // A finished sub-agent stays, dimmed, until the user closes
                    // it or its task is over.
                    if p.finished && (view.closed.contains(&a.id.to_string()) || task_over) {
                        continue;
                    }
                    panes.push(p);
                }
                let key = WindowKey::Task(plan.id.clone(), *tid);
                windows.push(WindowSpec {
                    title: view
                        .name_of(&key)
                        .map_or_else(|| format!("{tid} {}", task.title), str::to_string),
                    key,
                    task_state: Some(task.state),
                    panes,
                });
            }
            windows.extend(loose.iter().cloned());
            Workspace {
                plan: plan.id.clone(),
                title: plan.title.clone(),
                windows,
            }
        })
        .collect();

    Derived { workspaces, loose }
}

fn pane_of(a: &AgentView) -> PaneSpec {
    PaneSpec {
        agent: Some(a.id.clone()),
        term: a.term,
        level: Some(a.level),
        finished: super::marks::agent_done(a),
    }
}

// ---------------------------------------------------------------------------
// Per-server view state
// ---------------------------------------------------------------------------

/// What the user chose for one window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowView {
    /// Overrides the derived tab label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Index into `layout::PRESETS` of the last preset applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<usize>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zoomed: bool,
    /// [`PaneSpec::key`] of the focused pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
}

/// `view-<server>.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerView {
    /// The current workspace (a plan id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The current window ([`WindowKey::encode`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tree_hidden: bool,
    /// Agent ids of finished sub-agents the user closed.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub closed: BTreeSet<String>,
    /// Keyed by [`WindowKey::encode`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub windows: BTreeMap<String, WindowView>,
    /// Loose terminals the user split into a window (`prefix %`): terminal id
    /// to [`WindowKey::encode`] of the window that shows it as a pane.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attached: BTreeMap<String, String>,
}

impl ServerView {
    pub fn load(path: &Path) -> Result<ServerView> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                toml::from_str(&s).with_context(|| format!("{}: bad view file", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ServerView::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }

    pub fn name_of(&self, key: &WindowKey) -> Option<&str> {
        self.windows
            .get(&key.encode())
            .and_then(|w| w.name.as_deref())
            .filter(|n| !n.is_empty())
    }

    pub fn window_mut(&mut self, key: &WindowKey) -> &mut WindowView {
        self.windows.entry(key.encode()).or_default()
    }

    /// Records the user's current workspace and window.
    #[allow(dead_code)] // used by tests and the view file code
    pub fn set_current(&mut self, plan: Option<&PlanId>, key: Option<&WindowKey>) {
        self.plan = plan.map(ToString::to_string);
        self.window = key.map(WindowKey::encode);
    }

    /// Forgets view state of windows that no longer exist, so the file does
    /// not grow without bound.
    pub fn prune(&mut self, d: &Derived) {
        let live: BTreeSet<String> = d
            .workspaces
            .iter()
            .flat_map(|w| w.windows.iter())
            .chain(d.loose.iter())
            .map(|w| w.key.encode())
            .collect();
        self.windows.retain(|k, _| live.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use cmux_proto::view::{Activity, ChangeView, Life, Report, ReportStatus};

    use super::*;
    use crate::tui::fixture;

    fn keys(w: &[WindowSpec]) -> Vec<String> {
        w.iter().map(|w| w.key.encode()).collect()
    }

    #[test]
    fn plan_is_a_workspace_task_a_window_sub_agent_a_pane() {
        let sv = fixture::state();
        let d = derive(&sv, &ServerView::default());
        assert_eq!(d.workspaces.len(), 3);
        let ws = d.workspace(&fixture::plan_id()).unwrap();
        let titles: Vec<&str> = ws.windows.iter().map(|w| w.title.as_str()).collect();
        // Executor pinned first; tasks with an agent in plan order (T-0003,
        // T-0005, T-0006 have none); loose terminals last.
        assert_eq!(
            titles,
            [
                "executor",
                "T-0001 schema",
                "T-0002 api",
                "T-0004 perms",
                "zsh",
                "lazygit"
            ]
        );
        let t2 = &ws.windows[2];
        assert_eq!(t2.task_state, Some(TaskState::Testing));
        let panes: Vec<String> = t2.panes.iter().map(PaneSpec::key).collect();
        assert_eq!(panes.len(), 3);
        assert!(panes[0].ends_with("/T-0002"), "{panes:?}");
        assert!(panes[1].ends_with("/T-0002/tests"), "{panes:?}");
        assert!(panes[2].ends_with("/T-0002/docs"), "{panes:?}");
        assert!(!t2.panes[1].finished);
        // Plans without agents only have the loose windows.
        let docs = &d.workspaces[1];
        assert_eq!(keys(&docs.windows), ["term 1", "term 2"]);
        assert_eq!(keys(&d.loose), ["term 1", "term 2"]);
    }

    #[test]
    fn derive_is_pure_and_independent_of_vector_order() {
        let sv = fixture::state();
        let view = ServerView::default();
        let a = derive(&sv, &view);
        assert_eq!(a, derive(&sv.clone(), &view));
        let mut shuffled = sv.clone();
        shuffled.agents.reverse();
        shuffled.tasks.reverse();
        shuffled.terminals.reverse();
        assert_eq!(derive(&shuffled, &view), a);
    }

    #[test]
    fn closed_finished_sub_agents_disappear_running_ones_stay() {
        let mut sv = fixture::state();
        let plan = fixture::plan_id();
        let docs = AgentId::new(plan.clone(), "T-0002/docs".parse().unwrap());
        let tests = AgentId::new(plan.clone(), "T-0002/tests".parse().unwrap());
        // The fixture's docs sub-agent has a report; at work again it is no
        // more finished than a sub-agent without one: still a pane.
        let mut working = sv.agent(&docs).unwrap().clone();
        working.activity = Activity::Working;
        sv.apply(&cmux_proto::view::DeltaView {
            rev: 7,
            change: ChangeView::Agent(working),
        });
        let mut view = ServerView::default();
        view.closed.insert(docs.to_string());
        view.closed.insert(tests.to_string());
        let d = derive(&sv, &view);
        let t2 = &d.workspace(&plan).unwrap().windows[2];
        assert_eq!(t2.panes.len(), 3);

        // Once it exits, closing it hides it.
        let mut sv2 = sv.clone();
        let mut a = sv2.agent(&docs).unwrap().clone();
        a.life = Life::Exited { code: Some(0) };
        sv2.apply(&cmux_proto::view::DeltaView {
            rev: 8,
            change: ChangeView::Agent(a),
        });
        let d2 = derive(&sv2, &view);
        let t2 = &d2.workspace(&plan).unwrap().windows[2];
        assert_eq!(t2.panes.len(), 2);
        // Without the close it stays, dimmed.
        let d3 = derive(&sv2, &ServerView::default());
        let t2 = &d3.workspace(&plan).unwrap().windows[2];
        assert_eq!(t2.panes.len(), 3);
        assert!(t2.panes[2].finished);
    }

    #[test]
    fn a_reported_idle_sub_agent_is_finished_and_can_be_closed() {
        let sv = fixture::state();
        let plan = fixture::plan_id();
        let docs = AgentId::new(plan.clone(), "T-0002/docs".parse().unwrap());
        let mut a = sv.agent(&docs).unwrap().clone();
        a.life = Life::Running;
        a.activity = Activity::Idle;
        a.report = Some(Report {
            status: ReportStatus::Success,
            summary: "done".into(),
            pr_url: None,
            attempt: 1,
            implicit: false,
            supersedes_blocked: false,
        });
        let mut sv2 = sv.clone();
        sv2.apply(&cmux_proto::view::DeltaView {
            rev: 8,
            change: ChangeView::Agent(a.clone()),
        });
        let d = derive(&sv2, &ServerView::default());
        let t2 = &d.workspace(&plan).unwrap().windows[2];
        assert_eq!(t2.panes.len(), 3);
        assert!(t2.panes[2].finished);
        let mut view = ServerView::default();
        view.closed.insert(docs.to_string());
        let d = derive(&sv2, &view);
        assert_eq!(d.workspace(&plan).unwrap().windows[2].panes.len(), 2);

        // Back at work (a blocked report, resumed): not finished, not hidden.
        a.activity = Activity::Working;
        sv2.apply(&cmux_proto::view::DeltaView {
            rev: 9,
            change: ChangeView::Agent(a),
        });
        let d = derive(&sv2, &view);
        let t2 = &d.workspace(&plan).unwrap().windows[2];
        assert_eq!(t2.panes.len(), 3);
        assert!(!t2.panes[2].finished);
    }

    #[test]
    fn custom_names_and_current_window_fall_back() {
        let sv = fixture::state();
        let plan = fixture::plan_id();
        let mut view = ServerView::default();
        view.window_mut(&WindowKey::Task(plan.clone(), TaskId::new(2)))
            .name = Some("backend".into());
        let d = derive(&sv, &view);
        assert_eq!(d.workspace(&plan).unwrap().windows[2].title, "backend");

        // Nothing chosen: the first plan's executor.
        let (p, k) = d.current(&ServerView::default());
        assert_eq!(p, Some(plan.clone()));
        assert_eq!(k, Some(WindowKey::Executor(plan.clone())));
        // A remembered window that still exists wins; a stale one falls back.
        let mut v = ServerView::default();
        v.set_current(
            Some(&plan),
            Some(&WindowKey::Task(plan.clone(), TaskId::new(4))),
        );
        assert_eq!(
            d.current(&v).1,
            Some(WindowKey::Task(plan.clone(), TaskId::new(4)))
        );
        v.set_current(
            Some(&plan),
            Some(&WindowKey::Task(plan.clone(), TaskId::new(3))),
        );
        assert_eq!(d.current(&v).1, Some(WindowKey::Executor(plan)));
    }

    #[test]
    fn empty_state_has_only_loose_windows() {
        let d = derive(&StateView::default(), &ServerView::default());
        assert!(d.workspaces.is_empty());
        assert_eq!(d.current(&ServerView::default()), (None, None));
    }

    #[test]
    fn window_keys_round_trip() {
        let plan = fixture::plan_id();
        for k in [
            WindowKey::Executor(plan.clone()),
            WindowKey::Task(plan, TaskId::new(12)),
            WindowKey::Loose(TermId(3)),
        ] {
            assert_eq!(WindowKey::decode(&k.encode()), Some(k));
        }
        assert_eq!(WindowKey::decode("nonsense"), None);
    }

    #[test]
    fn view_file_round_trips_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("view-vps.toml");
        let plan = fixture::plan_id();
        let key = WindowKey::Task(plan.clone(), TaskId::new(2));
        let mut v = ServerView::default();
        v.set_current(Some(&plan), Some(&key));
        v.tree_hidden = true;
        v.closed.insert("x/y".into());
        let w = v.window_mut(&key);
        w.zoomed = true;
        w.preset = Some(1);
        w.active = Some("a".into());
        v.window_mut(&WindowKey::Loose(TermId(99))).name = Some("gone".into());
        v.save(&path).unwrap();
        assert_eq!(ServerView::load(&path).unwrap(), v);
        assert_eq!(
            ServerView::load(&dir.path().join("none.toml")).unwrap(),
            ServerView::default()
        );

        v.prune(&derive(&fixture::state(), &v));
        assert!(v.windows.contains_key(&key.encode()));
        assert!(
            !v.windows
                .contains_key(&WindowKey::Loose(TermId(99)).encode())
        );
    }
}
