//! Spawning, stopping, resuming, the spawn queue, prompts and runner events.

use std::path::PathBuf;

use super::*;
use crate::ids::{AgentPath, TermId};
use crate::model::{
    Activity, Level, Plan, PlanState, PrRef, QueuedSpawn, TaskState, TermKind, Terminal,
    TransitionSource, check_task_transition, pr_to_task_state,
};
use crate::prompts::{Template, render, task_list_line};

/// Terminal size until a client resizes it.
const DEFAULT_SIZE: (u16, u16) = (120, 40);
/// First prompt of a resumed session; pending items ride along (path C).
const RESUME_KICKOFF: &str =
    "[cmux] Your session was resumed. Continue your work where you left off.";
/// First prompt of a session that `Recover` resumes.
pub const RESTART_KICKOFF: &str =
    "[cmux] resumed after a server restart; check the worktree and PR state before you continue";
/// The reason `Recover` gives for the agents it ends.
pub const SERVER_RESTART: &str = "server_restart";

impl Ctx<'_> {
    // ---- prompts ----

    fn render_tpl(&self, t: Template, vars: &[(&str, &str)]) -> String {
        let custom = self.st.prompt_overrides.get(t.file_name());
        custom
            .and_then(|text| render(text, vars).ok())
            .unwrap_or_else(|| render(t.builtin(), vars).expect("builtin placeholders are filled"))
    }

    pub fn plan_file(&self, pi: usize) -> PathBuf {
        let dir = self
            .st
            .data_dir
            .join("plans")
            .join(self.plan(pi).id.as_str());
        dir.join("plan.md")
    }

    /// The execute kickoff with every task listed.
    pub fn kickoff_execute(&self, pi: usize) -> String {
        let lines: Vec<String> = self
            .plan(pi)
            .tasks
            .iter()
            .map(|t| {
                let deps: Vec<String> = t.deps.iter().map(|d| d.to_string()).collect();
                let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
                let checked = t.criteria.len() - t.unchecked_criteria();
                task_list_line(
                    &t.id.to_string(),
                    t.state.label(),
                    &t.title,
                    &deps,
                    checked,
                    t.criteria.len(),
                )
            })
            .collect();
        self.render_tpl(
            Template::KickoffExecute,
            &[("task_list", &lines.join("\n"))],
        )
    }

    fn system_prompt(&self, pi: usize, ai: usize) -> String {
        let plan = self.plan(pi);
        let a = self.agent(pi, ai);
        let task = a.task.and_then(|t| plan.task(t));
        let tid = a.task.map_or(String::new(), |t| t.to_string());
        let worktree = a.cwd.display().to_string();
        match a.level {
            Level::Executor => {
                let repos: Vec<String> = plan
                    .repos
                    .iter()
                    .map(|r| format!("{} (base {})", r.path.display(), r.base_branch))
                    .collect();
                self.render_tpl(
                    Template::Executor,
                    &[
                        ("plan_file", &self.plan_file(pi).display().to_string()),
                        ("plan_id", plan.id.as_str()),
                        ("plan_title", &plan.title),
                        ("repos", &repos.join(", ")),
                        ("task_limit", &plan.limits.tasks_per_plan.to_string()),
                    ],
                )
            }
            Level::Task => self.render_tpl(
                Template::Task,
                &[
                    (
                        "branch",
                        task.and_then(|t| t.branch.as_deref()).unwrap_or(""),
                    ),
                    ("plan_id", plan.id.as_str()),
                    (
                        "subagent_limit",
                        &plan.limits.subagents_per_task.to_string(),
                    ),
                    ("task_id", &tid),
                    ("task_title", task.map_or("", |t| t.title.as_str())),
                    ("worktree", &worktree),
                ],
            ),
            Level::Sub => self.render_tpl(
                Template::Subagent,
                &[
                    ("agent_path", &a.path.to_string()),
                    ("task_id", &tid),
                    ("worktree", &worktree),
                ],
            ),
        }
    }

    fn kickoff_task(&self, pi: usize, task: usize) -> String {
        let t = &self.plan(pi).tasks[task];
        let none = |s: String| {
            if s.is_empty() {
                "(none)".to_string()
            } else {
                s
            }
        };
        let criteria: Vec<String> = t
            .criteria
            .iter()
            .map(|c| format!("- [{}] {}", if c.checked { "x" } else { " " }, c.text))
            .collect();
        let notes: Vec<String> = t.notes.iter().map(|n| format!("- {}", n.text)).collect();
        self.render_tpl(
            Template::KickoffTask,
            &[
                ("criteria", &none(criteria.join("\n"))),
                (
                    "instructions",
                    &none(t.instructions.clone().unwrap_or_default()),
                ),
                ("notes", &none(notes.join("\n"))),
                ("task_body", &t.body),
            ],
        )
    }

    fn spawn_fx(&mut self, pi: usize, ai: usize, resume: bool, kickoff: String) -> Effect {
        // The one place a process's terminal id is allocated, so the runner
        // and `CreateTerminal` can never hand out the same one.
        let term = TermId(self.st.next_term_id.max(1));
        self.st.next_term_id = term.0 + 1;
        let a = self.agent(pi, ai);
        Effect::SpawnAgent {
            agent: a.id.clone(),
            term,
            level: a.level,
            cwd: a.cwd.clone(),
            session_id: resume.then(|| a.session_id.clone()),
            resume,
            system_prompt: self.system_prompt(pi, ai),
            kickoff,
        }
    }

    /// Registers a `Starting` agent, replacing an older record with the same id.
    fn register(&mut self, pi: usize, id: AgentId, cwd: PathBuf) -> usize {
        let mut agent = Agent::new(id.clone(), "", cwd, [0; 32]);
        agent.life = Life::Starting;
        self.touch(Change::Agent(id.clone()));
        let plan = &mut self.st.plans[pi];
        let (i, old_term) = match plan.agents.iter().position(|a| a.id == id) {
            Some(i) => {
                let old = plan.agents[i].term;
                plan.agents[i] = agent;
                (i, old)
            }
            None => {
                plan.agents.push(agent);
                (plan.agents.len() - 1, None)
            }
        };
        if let Some(t) = old_term
            && self.st.terminals.remove(&t).is_some()
        {
            self.touch(Change::Removed(ObjectRef::Terminal(t)));
        }
        i
    }

    // ---- plans ----

    pub fn create_plan(
        &mut self,
        title: String,
        repos: Vec<crate::model::Repo>,
        request: Option<String>,
    ) -> Result<Reply, EngineError> {
        if title.trim().is_empty() {
            return err(ErrorKind::Invalid, "a plan needs a title");
        }
        if repos.is_empty() {
            return err(
                ErrorKind::Invalid,
                "a plan needs at least one repository; repos[0] is where the executor runs",
            );
        }
        if request.is_some() {
            self.check_global_slot()?;
        }
        let (y, m, d) = civil_date(self.now);
        let mut id = PlanId::new(y, m, d, &title);
        let mut n = 1;
        let slug = crate::ids::slugify(&title);
        while self.st.plans.iter().any(|p| p.id == id) {
            n += 1;
            // The suffix goes after the 40-char cut, or a long title never changes.
            let suffix = format!("-{n}");
            let keep: String = slug.chars().take(40 - suffix.len()).collect();
            id = PlanId::new(y, m, d, &format!("{}{suffix}", keep.trim_end_matches('-')));
        }
        let cwd = repos[0].path.clone();
        let mut plan = Plan::new(id.clone(), title, repos);
        plan.limits = self.st.plan_limits;
        self.st.plans.push(plan);
        self.touch(Change::Plan(id.clone()));
        let Some(request) = request else {
            return Ok(Reply::Text(id.to_string()));
        };
        let pi = self.st.plans.len() - 1;
        let ai = self.register(pi, AgentId::new(id.clone(), AgentPath::Executor), cwd);
        let plan_file = self.plan_file(pi).display().to_string();
        let kickoff = self.render_tpl(
            Template::KickoffPlan,
            &[("plan_file", &plan_file), ("request", &request)],
        );
        let fx = self.spawn_fx(pi, ai, false, kickoff);
        self.fx.push(fx);
        Ok(Reply::Text(id.to_string()))
    }

    // ---- limits ----

    pub fn check_global_slot(&self) -> Result<(), EngineError> {
        let (n, max) = (self.live_processes(), self.st.global_limits.live_processes);
        if n >= max {
            return err(
                ErrorKind::Limit,
                format!(
                    "limit reached: {n} Claude processes are live (server limit {max}); wait for one to finish"
                ),
            );
        }
        Ok(())
    }

    /// Why a task agent cannot start now, if it cannot.
    fn spawn_blocker(&self, pi: usize) -> Option<EngineError> {
        let plan = self.plan(pi);
        if plan.paused {
            return Some(EngineError::new(
                ErrorKind::Invalid,
                format!("plan {} is paused; resume it first", plan.id),
            ));
        }
        let (n, max) = (self.task_slots_used(pi), plan.limits.tasks_per_plan);
        if n >= max {
            return Some(EngineError::new(
                ErrorKind::Limit,
                format!(
                    "limit reached: {n} of {max} task agents are live; wait for one to report, then spawn again"
                ),
            ));
        }
        self.check_global_slot().err()
    }

    // ---- task agents ----

    fn check_spawnable(&self, pi: usize, ti: usize, force: bool) -> Result<(), EngineError> {
        let plan = self.plan(pi);
        let task = &plan.tasks[ti];
        if plan.state != PlanState::Accepted {
            return err(
                ErrorKind::Invalid,
                format!(
                    "plan {} is {}; tasks can only be spawned once it is accepted",
                    plan.id, plan.state
                ),
            );
        }
        if !matches!(task.state, TaskState::Todo | TaskState::Blocked) {
            return err(
                ErrorKind::Invalid,
                format!(
                    "{} is {}; spawn needs a task in todo or blocked",
                    task.id, task.state
                ),
            );
        }
        let id = AgentId::new(plan.id.clone(), AgentPath::Task(task.id));
        if plan.agent(&id).is_some_and(Agent::is_live) {
            return err(
                ErrorKind::Invalid,
                format!("{} already has a live agent; stop_agent it first", task.id),
            );
        }
        let open: Vec<String> = task
            .deps
            .iter()
            .filter(|d| plan.task(**d).is_none_or(|t| t.state != TaskState::Merged))
            .map(ToString::to_string)
            .collect();
        if !open.is_empty() && !force {
            return err(
                ErrorKind::Invalid,
                format!(
                    "{} depends on unmerged tasks: {}; wait for them or pass force",
                    task.id,
                    open.join(", ")
                ),
            );
        }
        Ok(())
    }

    /// `spawn_task` from an agent (errors when blocked) or a client (queues).
    pub fn spawn_task(
        &mut self,
        pi: usize,
        task: TaskId,
        instructions: Option<String>,
        force: bool,
        from_agent: bool,
    ) -> Result<Reply, EngineError> {
        let ti = self.task_idx(pi, task)?;
        self.check_spawnable(pi, ti, force)?;
        if let Some(why) = self.spawn_blocker(pi) {
            if from_agent {
                return Err(why);
            }
            if self.plan(pi).spawn_queue.iter().any(|q| q.task == task) {
                return Ok(Reply::Text(format!("{task} is already queued")));
            }
            self.plan_mut(pi).spawn_queue.push(QueuedSpawn {
                task,
                instructions,
                force,
            });
            return Ok(Reply::Text(format!(
                "queued {task}: {}",
                why.to_string().trim_start_matches("error: ")
            )));
        }
        self.start_task(pi, ti, instructions);
        Ok(Reply::Text(format!(
            "started {task}: preparing its worktree; end your turn and cmux wakes you with its report"
        )))
    }

    fn start_task(&mut self, pi: usize, ti: usize, instructions: Option<String>) {
        let plan = self.plan(pi);
        let (pid, task) = (plan.id.clone(), &plan.tasks[ti]);
        let repo = &plan.repos[task.repo];
        let branch = format!("claude/{}", task.id);
        let cwd = repo.path.join(".worktrees").join(task.id.to_string());
        let fx = Effect::CreateWorktree {
            plan: pid.clone(),
            task: task.id,
            repo_path: repo.path.clone(),
            base: repo.base_branch.clone(),
            branch: branch.clone(),
        };
        let tid = task.id;
        let id = AgentId::new(pid, AgentPath::Task(tid));
        self.register(pi, id.clone(), cwd);
        // The old attempt's "exited without report" is answered by this one.
        self.dismiss_stalled(&id);
        let t = self.task_mut(pi, ti);
        t.state = TaskState::InProgress;
        t.attempt += 1;
        t.agent = Some(id);
        t.branch = Some(branch);
        t.instructions = instructions;
        self.fx.push(fx);
    }

    /// Starts queued spawns while slots are free.
    pub fn drain_queue(&mut self) {
        for pi in 0..self.st.plans.len() {
            while let Some(q) = self.plan(pi).spawn_queue.first().cloned() {
                if self.plan(pi).paused || self.plan(pi).state != PlanState::Accepted {
                    break;
                }
                let ti = self.task_idx(pi, q.task).ok();
                if ti.is_none_or(|ti| self.check_spawnable(pi, ti, q.force).is_err()) {
                    self.plan_mut(pi).spawn_queue.remove(0);
                    continue;
                }
                if self.spawn_blocker(pi).is_some() {
                    break;
                }
                self.plan_mut(pi).spawn_queue.remove(0);
                self.start_task(pi, ti.expect("checked"), q.instructions);
            }
        }
    }

    pub fn spawn_subagent(
        &mut self,
        pi: usize,
        ai: usize,
        name: &str,
        prompt: String,
    ) -> Result<Reply, EngineError> {
        let me = self.agent(pi, ai);
        let task = me.task.expect("task agents have a task");
        let path = AgentPath::sub(task, name)
            .map_err(|e| EngineError::new(ErrorKind::Invalid, e.to_string()))?;
        let cwd = me.cwd.clone();
        let id = AgentId::new(self.plan(pi).id.clone(), path);
        if self.plan(pi).paused {
            return err(
                ErrorKind::Invalid,
                "the plan is paused; sub-agents cannot start until it resumes",
            );
        }
        if self.plan(pi).agent(&id).is_some_and(Agent::is_live) {
            return err(
                ErrorKind::Invalid,
                format!("sub-agent {name} is already running; pick another name"),
            );
        }
        let (n, max) = (
            self.subs_used(pi, task),
            self.plan(pi).limits.subagents_per_task,
        );
        if n >= max {
            return err(
                ErrorKind::Limit,
                format!(
                    "limit reached: {n} of {max} sub-agents are live for {task}; wait for one to report"
                ),
            );
        }
        self.check_global_slot()?;
        let ai = self.register(pi, id, cwd);
        let kickoff = self.render_tpl(Template::KickoffSubagent, &[("prompt", &prompt)]);
        let fx = self.spawn_fx(pi, ai, false, kickoff);
        self.fx.push(fx);
        Ok(Reply::Text(format!(
            "started sub-agent {task}/{name}; end your turn and cmux wakes you with its report"
        )))
    }

    // ---- stop and resume ----

    /// Kills an agent and its live children. No implicit report: this was deliberate.
    pub fn stop_agent(&mut self, pi: usize, ai: usize) {
        let id = self.agent(pi, ai).id.clone();
        let kids: Vec<usize> = (0..self.plan(pi).agents.len())
            .filter(|&c| self.agent(pi, c).parent.as_ref() == Some(&id))
            .collect();
        for c in kids {
            self.stop_agent(pi, c);
        }
        if !self.agent(pi, ai).is_live() {
            return;
        }
        self.agent_mut(pi, ai).life = Life::Stopped;
        self.fx.push(Effect::KillAgent {
            agent: id.clone(),
            term: self.agent(pi, ai).term,
        });
        self.expire_held_for(&id);
        self.dismiss_stalled(&id);
        self.drain_queue();
    }

    /// Stop and Resume answer "stalled / exited without report" items.
    pub fn dismiss_stalled(&mut self, id: &AgentId) {
        let open: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| i.is_pending() && i.from.as_ref() == Some(id))
            .filter(|i| matches!(i.kind, ItemKind::Stalled | ItemKind::ExitedWithoutReport))
            .map(|i| i.id)
            .collect();
        for item in open {
            self.set_status(item, Delivery::Dismissed);
        }
    }

    pub fn resume_agent(
        &mut self,
        id: &AgentId,
        kickoff: Option<String>,
    ) -> Result<Reply, EngineError> {
        let (pi, ai) = self.agent_idx(id)?;
        let plan = self.plan(pi);
        let a = self.agent(pi, ai);
        if a.is_live() {
            return err(ErrorKind::Invalid, format!("{id} is already running"));
        }
        if plan.state.is_terminal() {
            return err(
                ErrorKind::Invalid,
                format!("plan {} is {}; nothing can resume", plan.id, plan.state),
            );
        }
        if a.session_id.is_empty() {
            return err(
                ErrorKind::Invalid,
                format!("{id} never started, so there is no session to resume"),
            );
        }
        self.check_global_slot()?;
        // An implicit report is void once the agent runs again, so such an agent
        // holds a slot again even though it had landed.
        let implicit = a.report.as_ref().is_some_and(|r| r.implicit);
        let holds_slot = !a.landed || implicit;
        if a.level == Level::Task
            && holds_slot
            && self.task_slots_used(pi) >= plan.limits.tasks_per_plan
        {
            return err(
                ErrorKind::Limit,
                format!(
                    "limit reached: {} task agents are live; wait for one to report",
                    plan.limits.tasks_per_plan
                ),
            );
        }
        if let (Level::Sub, true, Some(task)) = (a.level, holds_slot, a.task)
            && self.subs_used(pi, task) >= plan.limits.subagents_per_task
        {
            return err(
                ErrorKind::Limit,
                format!(
                    "limit reached: {} sub-agents are live for {task}; wait for one to report",
                    plan.limits.subagents_per_task
                ),
            );
        }
        // An implicit "exited without report" is void once the agent runs again.
        let task = a.task;
        let old_term = a.term;
        {
            let a = self.agent_mut(pi, ai);
            a.life = Life::Starting;
            a.term = None;
            // A fresh process: the old one's token, activity and idle clock are void.
            a.token_hash = [0; 32];
            a.activity = Activity::Unknown;
            a.idle_since = None;
            a.consecutive_stop_blocks = 0;
            a.stalled = false;
            if implicit {
                a.report = None;
                a.landed = false;
            }
        }
        if let Some(t) = old_term
            && self.st.terminals.remove(&t).is_some()
        {
            self.touch(Change::Removed(ObjectRef::Terminal(t)));
        }
        if let (true, Some(t)) = (implicit, task)
            && let Ok(ti) = self.task_idx(pi, t)
        {
            let task = &self.plan(pi).tasks[ti];
            if task.state == TaskState::Blocked
                && check_task_transition(task, TaskState::InProgress, TransitionSource::Agent)
                    .is_ok()
            {
                self.task_mut(pi, ti).state = TaskState::InProgress;
            }
        }
        self.dismiss_stalled(id);
        let fx = self.spawn_fx(
            pi,
            ai,
            true,
            kickoff.unwrap_or_else(|| RESUME_KICKOFF.to_string()),
        );
        self.fx.push(fx);
        Ok(Reply::Ok)
    }

    /// After a server restart no process of the last run exists. Ends every
    /// live agent, returns unconfirmed pastes to pending, fails sub-agents to
    /// their task agent, and (with `auto_resume`) resumes the agents that
    /// were working. Idle agents stay stopped until someone resumes them.
    pub fn recover(&mut self, auto_resume: bool) {
        let mut resume = Vec::new();
        let mut ended = Vec::new();
        for pi in 0..self.st.plans.len() {
            for ai in 0..self.plan(pi).agents.len() {
                let a = self.agent(pi, ai);
                if !a.unconfirmed.is_empty() {
                    let ids = std::mem::take(&mut self.agent_mut(pi, ai).unconfirmed);
                    for item in ids {
                        let pasted = self.item_idx(item).is_some_and(|i| {
                            matches!(self.st.inbox[i].status, Delivery::Delivered { .. })
                        });
                        if pasted {
                            self.set_status(item, Delivery::Pending);
                        }
                    }
                }
                let a = self.agent(pi, ai);
                if !a.is_live() {
                    continue;
                }
                let working = a.life == Life::Running && a.activity == Activity::Working;
                let (id, level, started) = (a.id.clone(), a.level, !a.session_id.is_empty());
                {
                    let a = self.agent_mut(pi, ai);
                    a.life = Life::Exited { code: None };
                    a.activity = Activity::Unknown;
                    a.stalled = false;
                }
                self.expire_held_for(&id);
                if level == Level::Sub || !started {
                    // Landing below may start queued tasks, so it waits for the resumes.
                    ended.push((id, level));
                } else if working && auto_resume {
                    resume.push(id);
                }
            }
        }
        for id in resume {
            // Refused only by a limit or a finished plan: it then stays stopped.
            let _ = self.resume_agent(&id, Some(RESTART_KICKOFF.to_string()));
        }
        for (id, level) in ended {
            let Ok((pi, ai)) = self.agent_idx(&id) else {
                continue;
            };
            if level == Level::Sub {
                self.fail_sub(pi, ai);
            } else {
                // Never started, so nothing can resume it: an implicit report.
                let why = format!("{SERVER_RESTART}: the server restarted before it started");
                self.end_agent(pi, ai, Life::Exited { code: None }, &why);
            }
        }
        self.drain_queue();
    }

    /// A sub-agent is never resumed automatically: its task agent gets a
    /// failed report and decides.
    fn fail_sub(&mut self, pi: usize, ai: usize) {
        let a = self.agent(pi, ai);
        if a.report.is_none() {
            let attempt = a
                .task
                .and_then(|t| self.plan(pi).task(t))
                .map_or(0, |t| t.attempt);
            self.agent_mut(pi, ai).report = Some(crate::model::Report {
                status: crate::model::ReportStatus::Failed,
                summary: format!(
                    "{SERVER_RESTART}: the server restarted while it ran; sub-agents are not resumed automatically"
                ),
                pr_url: None,
                attempt,
                implicit: false,
                supersedes_blocked: false,
            });
        }
        self.land(pi, ai);
    }

    /// Accepting hands the executor the execute kickoff: as a message when it
    /// runs, as the first prompt of a resumed session when it does not.
    pub fn hand_executor_kickoff(&mut self, pi: usize, kickoff: String) {
        let id = AgentId::new(self.plan(pi).id.clone(), AgentPath::Executor);
        let Ok((_, ai)) = self.agent_idx(&id) else {
            // A plan the client wrote itself has no executor yet.
            let cwd = self.plan(pi).repos[0].path.clone();
            let ai = self.register(pi, id, cwd);
            let fx = self.spawn_fx(pi, ai, false, kickoff);
            self.fx.push(fx);
            return;
        };
        if self.agent(pi, ai).is_live() || self.resume_agent(&id, Some(kickoff.clone())).is_err() {
            self.message(&id, None, kickoff);
        }
    }

    // ---- runner events ----

    pub fn runner(&mut self, ev: RunnerEvent) -> Result<(), EngineError> {
        match ev {
            RunnerEvent::WorktreeReady {
                plan,
                task,
                path,
                branch,
            } => {
                let pi = self.plan_idx(&plan)?;
                let ti = self.task_idx(pi, task)?;
                let id = AgentId::new(plan, AgentPath::Task(task));
                let Ok((_, ai)) = self.agent_idx(&id) else {
                    return Ok(());
                };
                if self.agent(pi, ai).life != Life::Starting {
                    return Ok(());
                }
                let t = &self.plan(pi).tasks[ti];
                if t.worktree.as_ref() != Some(&path) || t.branch.as_ref() != Some(&branch) {
                    let t = self.task_mut(pi, ti);
                    t.worktree = Some(path.clone());
                    t.branch = Some(branch);
                }
                if self.agent(pi, ai).cwd != path {
                    self.agent_mut(pi, ai).cwd = path;
                }
                let kickoff = self.kickoff_task(pi, ti);
                if self.plan(pi).tasks[ti].instructions.is_some() {
                    self.task_mut(pi, ti).instructions = None;
                }
                let fx = self.spawn_fx(pi, ai, false, kickoff);
                self.fx.push(fx);
            }
            RunnerEvent::WorktreeFailed { plan, task, error } => {
                let pi = self.plan_idx(&plan)?;
                let ti = self.task_idx(pi, task)?;
                let id = AgentId::new(plan, AgentPath::Task(task));
                let Ok((_, ai)) = self.agent_idx(&id) else {
                    return Ok(());
                };
                if self.agent(pi, ai).life != Life::Starting {
                    return Ok(());
                }
                self.st.plans[pi].agents.remove(ai);
                self.touch(Change::Removed(ObjectRef::Agent(id.clone())));
                let t = self.task_mut(pi, ti);
                t.state = TaskState::Todo;
                t.agent = None;
                t.attempt = t.attempt.saturating_sub(1);
                let executor = AgentId::new(id.plan().clone(), AgentPath::Executor);
                self.post(NewItem {
                    to: Recipient::Agent(executor),
                    kind: ItemKind::ChildExited,
                    from: None,
                    body: format!(
                        "{task} could not start: worktree failed: {error}. It is back in todo."
                    ),
                    dedupe: None,
                });
                self.drain_queue();
            }
            RunnerEvent::Spawned {
                agent,
                term,
                session_id,
                token_hash,
            } => {
                // The plan was deleted while the process started.
                let Ok((pi, ai)) = self.agent_idx(&agent) else {
                    self.fx.push(Effect::KillAgent {
                        agent,
                        term: Some(term),
                    });
                    return Ok(());
                };
                if self.agent(pi, ai).life == Life::Stopped {
                    self.fx.push(Effect::KillAgent {
                        agent,
                        term: Some(term),
                    });
                    return Ok(());
                }
                let cwd = self.agent(pi, ai).cwd.clone();
                let now = self.now;
                let a = self.agent_mut(pi, ai);
                a.term = Some(term);
                a.session_id = session_id;
                a.token_hash = token_hash;
                a.last_progress = Some(now);
                self.st.next_term_id = self.st.next_term_id.max(term.0 + 1);
                self.st.terminals.insert(
                    term,
                    Terminal {
                        id: term,
                        kind: TermKind::Claude,
                        agent: Some(agent),
                        cwd,
                        size: DEFAULT_SIZE,
                        size_owner: None,
                        title: None,
                    },
                );
                self.touch(Change::Terminal(term));
            }
            RunnerEvent::SpawnFailed { agent, error } => {
                let (pi, ai) = self.agent_idx(&agent)?;
                if self.agent(pi, ai).life == Life::Starting {
                    self.end_agent(
                        pi,
                        ai,
                        Life::Exited { code: None },
                        &format!("failed to start: {error}"),
                    );
                }
            }
            RunnerEvent::Exited { agent, code } => {
                let (pi, ai) = self.agent_idx(&agent)?;
                let a = self.agent(pi, ai);
                match a.life {
                    // A late exit of a replaced process, or of one we stopped.
                    Life::Starting if a.term.is_none() => {}
                    Life::Stopped => {}
                    Life::Exited { code: None } if code.is_some() => {
                        self.agent_mut(pi, ai).life = Life::Exited { code };
                    }
                    Life::Exited { .. } => {}
                    _ => {
                        let why = match code {
                            Some(0) | None => "exited without a report".to_string(),
                            Some(c) => format!("exited with code {c} without a report"),
                        };
                        self.end_agent(pi, ai, Life::Exited { code }, &why);
                    }
                }
            }
            RunnerEvent::TermSize {
                term,
                rows,
                cols,
                owner,
            } => {
                if let Some(t) = self.st.terminals.get_mut(&term)
                    && (t.size != (cols, rows) || t.size_owner != owner)
                {
                    t.size = (cols, rows);
                    t.size_owner = owner;
                    self.touch(Change::Terminal(term));
                }
            }
            RunnerEvent::TermTitle { term, title } => {
                if let Some(t) = self.st.terminals.get_mut(&term)
                    && t.title != title
                {
                    t.title = title;
                    self.touch(Change::Terminal(term));
                }
            }
            RunnerEvent::Progress { term } => {
                let agent = self.st.terminals.get(&term).and_then(|t| t.agent.clone());
                if let Some((pi, ai)) = agent.and_then(|a| self.agent_idx(&a).ok())
                    && self.agent(pi, ai).term == Some(term)
                    && self.agent(pi, ai).is_live()
                {
                    self.progress(pi, ai);
                }
            }
            RunnerEvent::Pr { plan, task, state } => {
                let pi = self.plan_idx(&plan)?;
                let ti = self.task_idx(pi, task)?;
                let to = pr_to_task_state(&state);
                let t = &self.plan(pi).tasks[ti];
                if t.state == to || check_task_transition(t, to, TransitionSource::GitHub).is_err()
                {
                    return Ok(());
                }
                let (from, agent) = (t.state, t.agent.clone());
                let number =
                    t.pr.as_ref()
                        .map_or(String::new(), |p: &PrRef| format!(" #{}", p.number));
                self.task_mut(pi, ti).state = to;
                let body = format!(
                    "{task} PR{number}: {from} -> {to} (GitHub: merged={}, closed={}, CI {:?})",
                    state.merged, state.closed, state.ci
                );
                let executor = AgentId::new(plan, AgentPath::Executor);
                for target in agent.into_iter().chain([executor]) {
                    self.post(NewItem {
                        to: Recipient::Agent(target.clone()),
                        kind: ItemKind::PrChanged,
                        from: None,
                        body: body.clone(),
                        dedupe: None,
                    });
                }
                self.drain_queue();
            }
        }
        Ok(())
    }
}
