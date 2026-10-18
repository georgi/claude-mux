//! MCP tools by role (architecture 7.3).

use super::client::transition;
use super::*;
use crate::ids::ClientId;
use crate::model::{
    Activity, Criterion, Level, Note, PlanState, PrRef, Report, ReportStatus, TaskState,
    TransitionSource,
};
use crate::prompts::task_list_line;

use Level::{Executor, Sub, Task as TaskLevel};

fn text(s: impl Into<String>) -> Result<Reply, EngineError> {
    Ok(Reply::Text(s.into()))
}

fn level_name(l: Level) -> &'static str {
    match l {
        Executor => "executor",
        TaskLevel => "task",
        Sub => "sub-agent",
    }
}

impl ToolCall {
    pub fn name(&self) -> &'static str {
        use ToolCall::*;
        match self {
            GetPlan => "get_plan",
            UpdatePlan { .. } => "update_plan",
            ListTasks => "list_tasks",
            GetTask { .. } => "get_task",
            CreateTask { .. } => "create_task",
            UpdateTask { .. } => "update_task",
            TransitionTask { .. } => "transition_task",
            AddCriterion { .. } => "add_criterion",
            CheckCriterion { .. } => "check_criterion",
            AddNote { .. } => "add_note",
            SpawnTask { .. } => "spawn_task",
            SpawnSubagent { .. } => "spawn_subagent",
            Send { .. } => "send",
            StopAgent { .. } => "stop_agent",
            ListAgents => "list_agents",
            Peek { .. } => "peek",
            SetTaskPr { .. } => "set_task_pr",
            ReportResult { .. } => "report_result",
            AskParent { .. } => "ask_parent",
            AnswerChild { .. } => "answer_child",
            AskUser { .. } => "ask_user",
        }
    }

    /// The levels that may call this tool (the table in architecture 7.3).
    pub fn roles(&self) -> &'static [Level] {
        use ToolCall::*;
        match self {
            GetTask { .. } | AddNote { .. } => &[Executor, TaskLevel, Sub],
            GetPlan
            | ListTasks
            | TransitionTask { .. }
            | AddCriterion { .. }
            | CheckCriterion { .. } => &[Executor, TaskLevel],
            Send { .. } | StopAgent { .. } | ListAgents | Peek { .. } => &[Executor, TaskLevel],
            AnswerChild { .. } | AskUser { .. } => &[Executor, TaskLevel],
            UpdatePlan { .. } | CreateTask { .. } | UpdateTask { .. } | SpawnTask { .. } => {
                &[Executor]
            }
            SpawnSubagent { .. } | SetTaskPr { .. } => &[TaskLevel],
            ReportResult { .. } | AskParent { .. } => &[TaskLevel, Sub],
        }
    }
}

/// Reads `.../pull/<n>` out of an `https` pull request URL.
fn parse_pr(url: &str) -> Result<PrRef, EngineError> {
    let number = Some(url)
        .filter(|u| u.starts_with("https://"))
        .and_then(|u| u.split_once("/pull/"))
        .map(|(_, rest)| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .and_then(|n| n.parse().ok());
    match number {
        Some(number) => Ok(PrRef::new(url, number)),
        None => err(
            ErrorKind::Invalid,
            format!(
                "{url:?} is not a pull request URL; expected https://github.com/<owner>/<repo>/pull/<number>"
            ),
        ),
    }
}

impl Ctx<'_> {
    pub fn tool(&mut self, pi: usize, ai: usize, call: ToolCall) -> Result<Reply, EngineError> {
        let me = self.agent(pi, ai);
        let (me_id, level, own) = (me.id.clone(), me.level, me.task);
        if !call.roles().contains(&level) {
            let allowed: Vec<&str> = call.roles().iter().map(|l| level_name(*l)).collect();
            return err(
                ErrorKind::Forbidden,
                format!(
                    "{} is not available to {} agents; it is available to: {}",
                    call.name(),
                    level_name(level),
                    allowed.join(", ")
                ),
            );
        }
        // Task agents and sub-agents act on their own task only.
        let own_only = |task: TaskId| -> Result<(), EngineError> {
            if level != Executor && own != Some(task) {
                let mine = own.map_or("none".into(), |t| t.to_string());
                return err(
                    ErrorKind::Forbidden,
                    format!("{task} is not your task; you may only use this on {mine}"),
                );
            }
            Ok(())
        };
        match call {
            ToolCall::GetPlan => self.get_plan(pi),
            ToolCall::UpdatePlan { state, title } => self.update_plan(pi, &me_id, state, title),
            ToolCall::ListTasks => text(self.task_lines(pi)),
            ToolCall::GetTask { task } => {
                let t = task.or(own).ok_or_else(|| {
                    EngineError::new(ErrorKind::Invalid, "name a task: get_task needs a task id")
                })?;
                if level == Sub {
                    own_only(t)?;
                }
                self.get_task(pi, t)
            }
            ToolCall::CreateTask {
                title,
                body,
                repo,
                deps,
                criteria,
            } => self.create_task(pi, title, body, repo, deps, criteria),
            ToolCall::UpdateTask { task, patch } => self.update_task(pi, task, patch),
            ToolCall::TransitionTask { task, to } => {
                if level == TaskLevel {
                    own_only(task)?;
                    if !matches!(to, TaskState::Testing | TaskState::Blocked) {
                        return err(
                            ErrorKind::Forbidden,
                            "task agents may move their own task only to: testing, blocked",
                        );
                    }
                }
                self.transition_task(pi, task, to, TransitionSource::Agent)
            }
            ToolCall::AddCriterion { task, text: t } => {
                own_only(task)?;
                let ti = self.task_idx(pi, task)?;
                self.task_mut(pi, ti).criteria.push(Criterion {
                    text: t,
                    checked: false,
                });
                text(format!(
                    "{task} now has {} criteria",
                    self.plan(pi).tasks[ti].criteria.len()
                ))
            }
            ToolCall::CheckCriterion {
                task,
                criterion,
                checked,
            } => {
                own_only(task)?;
                let ti = self.task_idx(pi, task)?;
                let n = self.plan(pi).tasks[ti].criteria.len();
                if criterion == 0 || criterion > n {
                    return err(
                        ErrorKind::Invalid,
                        format!("{task} has {n} criteria; use 1..={n}"),
                    );
                }
                if self.plan(pi).tasks[ti].criteria[criterion - 1].checked != checked {
                    self.task_mut(pi, ti).criteria[criterion - 1].checked = checked;
                }
                text(format!(
                    "criterion {criterion} of {task} is {}",
                    if checked { "checked" } else { "unchecked" }
                ))
            }
            ToolCall::AddNote { task, text: t } => {
                own_only(task)?;
                let ti = self.task_idx(pi, task)?;
                let at = self.now;
                self.task_mut(pi, ti).notes.push(Note {
                    at,
                    by: Some(me_id),
                    text: t,
                });
                text(format!("note added to {task}"))
            }
            ToolCall::SpawnTask {
                task,
                instructions,
                force,
            } => self.spawn_task(pi, task, instructions, force, true),
            ToolCall::SpawnSubagent { name, prompt } => self.spawn_subagent(pi, ai, &name, prompt),
            ToolCall::Send {
                agent,
                text: body,
                interrupt,
            } => {
                self.child(&me_id, &agent)?;
                self.send(&agent, Some(me_id), body, interrupt);
                text(format!("sent to {}", agent.path()))
            }
            ToolCall::StopAgent { agent } => {
                let (cpi, cai) = self.child(&me_id, &agent)?;
                if !self.agent(cpi, cai).is_live() {
                    return err(
                        ErrorKind::Invalid,
                        format!("{} is not running", agent.path()),
                    );
                }
                self.stop_agent(cpi, cai);
                text(format!("stopped {}", agent.path()))
            }
            ToolCall::ListAgents => {
                let lines: Vec<String> = self
                    .plan(pi)
                    .agents
                    .iter()
                    .filter(|a| a.parent.as_ref() == Some(&me_id))
                    .map(|a| self.agent_line(a))
                    .collect();
                text(if lines.is_empty() {
                    "no child agents".to_string()
                } else {
                    lines.join("\n")
                })
            }
            ToolCall::Peek { agent, lines } => {
                let (cpi, cai) = self.child(&me_id, &agent)?;
                Ok(Reply::Peek {
                    agent,
                    term: self.agent(cpi, cai).term,
                    lines,
                })
            }
            ToolCall::SetTaskPr { url } => {
                let pr = parse_pr(&url)?;
                let ti = self.task_idx(pi, own.expect("task agent"))?;
                // The same URL again keeps the CI state the poller recorded.
                if self.plan(pi).tasks[ti].pr.as_ref().map(|p| &p.url) != Some(&pr.url) {
                    self.task_mut(pi, ti).pr = Some(pr.clone());
                }
                text(format!("PR #{} recorded", pr.number))
            }
            ToolCall::ReportResult {
                status,
                summary,
                pr_url,
            } => self.report_result(pi, ai, status, summary, pr_url),
            ToolCall::AskParent { question } => {
                let parent = self
                    .agent(pi, ai)
                    .parent
                    .clone()
                    .expect("task and sub agents have a parent");
                self.post(NewItem {
                    to: Recipient::Agent(parent.clone()),
                    kind: ItemKind::ChildQuestion,
                    from: Some(me_id),
                    body: question,
                    dedupe: None,
                });
                text(format!(
                    "question sent to {}; end your turn, the answer arrives as a message",
                    parent.path()
                ))
            }
            ToolCall::AnswerChild { agent, answer } => {
                self.child(&me_id, &agent)?;
                let open: Vec<ItemId> = self
                    .st
                    .inbox
                    .iter()
                    .filter(|i| {
                        i.kind == ItemKind::ChildQuestion && i.from.as_ref() == Some(&agent)
                    })
                    .filter(|i| i.to == Recipient::Agent(me_id.clone()))
                    .filter(|i| matches!(i.status, Delivery::Pending | Delivery::Delivered { .. }))
                    .map(|i| i.id)
                    .collect();
                if open.is_empty() {
                    return err(
                        ErrorKind::Invalid,
                        format!("{} has no open question for you", agent.path()),
                    );
                }
                let by = ClientId(format!("agent:{}", me_id.path()));
                for q in open {
                    self.set_status(
                        q,
                        Delivery::Answered {
                            by: by.clone(),
                            answer: answer.clone(),
                        },
                    );
                }
                self.message(
                    &agent,
                    Some(me_id),
                    format!("Answer to your question: {answer}"),
                );
                text(format!("answer sent to {}", agent.path()))
            }
            ToolCall::AskUser { question } => {
                self.post(NewItem {
                    to: Recipient::Human,
                    kind: ItemKind::Question,
                    from: Some(me_id),
                    body: question,
                    dedupe: None,
                });
                text(format!(
                    "question sent to the user; end your turn, the answer arrives as a message (it expires after {} minutes)",
                    self.st.timeouts.question / 60_000
                ))
            }
        }
    }

    /// `target` must be a direct child of `me` in the same plan.
    fn child(&self, me: &AgentId, target: &AgentId) -> Result<(usize, usize), EngineError> {
        if target.plan() != me.plan() {
            return err(
                ErrorKind::Forbidden,
                format!("{target} belongs to another plan"),
            );
        }
        let (pi, ai) = self.agent_idx(target)?;
        if self.agent(pi, ai).parent.as_ref() != Some(me) {
            let kids: Vec<String> = self
                .plan(pi)
                .agents
                .iter()
                .filter(|a| a.parent.as_ref() == Some(me))
                .map(|a| a.path.to_string())
                .collect();
            return err(
                ErrorKind::Forbidden,
                format!(
                    "{} is not your child; your children: {}",
                    target.path(),
                    kids.join(", ")
                ),
            );
        }
        Ok((pi, ai))
    }

    fn update_plan(
        &mut self,
        pi: usize,
        me: &AgentId,
        state: Option<PlanState>,
        title: Option<String>,
    ) -> Result<Reply, EngineError> {
        let plan = self.plan(pi);
        match state {
            None | Some(PlanState::Proposed | PlanState::Done) => {}
            Some(s) => {
                return err(
                    ErrorKind::Forbidden,
                    format!("the executor may set the plan to proposed or done, not {s}"),
                );
            }
        }
        if state == Some(PlanState::Proposed) {
            if !matches!(plan.state, PlanState::Draft | PlanState::Proposed) {
                return err(
                    ErrorKind::Invalid,
                    format!(
                        "plan {} is {}; only a draft can be proposed",
                        plan.id, plan.state
                    ),
                );
            }
            if plan.tasks.is_empty() {
                return err(
                    ErrorKind::Invalid,
                    "the plan has no tasks; create them with create_task before proposing",
                );
            }
        }
        if state == Some(PlanState::Done) {
            crate::model::check_plan_transition(plan, PlanState::Done).map_err(transition)?;
            let open: Vec<String> = plan
                .tasks
                .iter()
                .filter(|t| !matches!(t.state, TaskState::Merged | TaskState::Cancelled))
                .map(|t| format!("{} ({})", t.id, t.state))
                .collect();
            if !open.is_empty() {
                return err(
                    ErrorKind::Invalid,
                    format!(
                        "tasks still open: {}; the plan is done only when every task is merged or cancelled",
                        open.join(", ")
                    ),
                );
            }
        }
        if let Some(t) = title.filter(|t| *t != plan.title) {
            self.plan_mut(pi).title = t;
        }
        match state {
            Some(PlanState::Proposed) => {
                let waiting = self.st.inbox.iter().any(|i| {
                    i.is_pending() && i.kind == ItemKind::PlanReview && i.from.as_ref() == Some(me)
                });
                if !waiting {
                    if self.plan(pi).state != PlanState::Proposed {
                        self.plan_mut(pi).state = PlanState::Proposed;
                    }
                    let n = self
                        .st
                        .inbox
                        .iter()
                        .filter(|i| i.kind == ItemKind::PlanReview && i.from.as_ref() == Some(me))
                        .count();
                    self.post(NewItem {
                        to: Recipient::Human,
                        kind: ItemKind::PlanReview,
                        from: Some(me.clone()),
                        body: format!("Plan {} is ready for review", self.plan(pi).id),
                        dedupe: Some(format!("plan-review:{}:{n}", self.plan(pi).id)),
                    });
                }
                text("the plan is proposed; end your turn, the user reviews it and cmux wakes you")
            }
            Some(PlanState::Done) => {
                self.plan_mut(pi).state = PlanState::Done;
                let plan = self.plan(pi).id.clone();
                self.fx.push(Effect::NotifyPlanDone { plan });
                text("the plan is done")
            }
            _ => text("plan updated"),
        }
    }

    fn report_result(
        &mut self,
        pi: usize,
        ai: usize,
        status: ReportStatus,
        summary: String,
        pr_url: Option<String>,
    ) -> Result<Reply, EngineError> {
        let a = self.agent(pi, ai);
        let (level, idle, landed) = (a.level, a.activity == Activity::Idle, a.landed);
        // An explicit blocked report of this attempt may be replaced; anything
        // else is final.
        let prior = a.report.clone();
        let task = a.task.expect("reporting agents have a task");
        let ti = self.task_idx(pi, task)?;
        let attempt = self.plan(pi).tasks[ti].attempt;
        let supersedes_blocked = match &prior {
            None => false,
            // A blocked report that never landed is simply replaced: the task
            // never saw it, so the new one lands as a first report would.
            Some(r) if !r.implicit && r.status == ReportStatus::Blocked && r.attempt == attempt => {
                landed || r.supersedes_blocked
            }
            Some(_) => {
                return err(
                    ErrorKind::Invalid,
                    format!(
                        "you already reported for attempt {attempt}; only a blocked report can be followed by another"
                    ),
                );
            }
        };
        if summary.trim().is_empty() {
            return err(
                ErrorKind::Invalid,
                "report_result needs a summary: what you changed, what you verified, what is open",
            );
        }
        let pr = match (&pr_url, level) {
            (Some(_), Sub) => {
                return err(
                    ErrorKind::Forbidden,
                    "sub-agents do not open PRs; omit pr_url",
                );
            }
            (Some(u), _) => Some(parse_pr(u)?),
            _ => None,
        };
        if let Some(pr) = pr
            && self.plan(pi).tasks[ti].pr.as_ref().map(|p| &p.url) != Some(&pr.url)
        {
            self.task_mut(pi, ti).pr = Some(pr);
        }
        let a = self.agent_mut(pi, ai);
        a.report = Some(Report {
            status,
            summary,
            pr_url,
            attempt,
            implicit: false,
            supersedes_blocked,
        });
        // The new report lands as its own item; until then the agent holds
        // its slot again.
        a.landed = false;
        // Landing waits for the next Stop, unless the agent is already idle.
        if idle {
            self.land(pi, ai);
        }
        text("report recorded; it reaches your parent when your turn ends. Finish now.")
    }

    // ---- read-only views as text ----

    fn task_lines(&self, pi: usize) -> String {
        let lines: Vec<String> = self
            .plan(pi)
            .tasks
            .iter()
            .map(|t| {
                let deps: Vec<String> = t.deps.iter().map(ToString::to_string).collect();
                let deps: Vec<&str> = deps.iter().map(String::as_str).collect();
                let done = t.criteria.len() - t.unchecked_criteria();
                task_list_line(
                    &t.id.to_string(),
                    t.state.label(),
                    &t.title,
                    &deps,
                    done,
                    t.criteria.len(),
                )
            })
            .collect();
        if lines.is_empty() {
            "no tasks".into()
        } else {
            lines.join("\n")
        }
    }

    fn get_plan(&self, pi: usize) -> Result<Reply, EngineError> {
        let p = self.plan(pi);
        let repos: Vec<String> = p
            .repos
            .iter()
            .map(|r| format!("{} (base {})", r.path.display(), r.base_branch))
            .collect();
        text(format!(
            "{} \"{}\" [{}{}]\nrepos: {}\nlimits: {} task agents, {} sub-agents per task\nplan file: {}\ntasks:\n{}",
            p.id,
            p.title,
            p.state,
            if p.paused { ", paused" } else { "" },
            repos.join(", "),
            p.limits.tasks_per_plan,
            p.limits.subagents_per_task,
            self.plan_file(pi).display(),
            self.task_lines(pi)
        ))
    }

    fn get_task(&self, pi: usize, id: TaskId) -> Result<Reply, EngineError> {
        let t = &self.plan(pi).tasks[self.task_idx(pi, id)?];
        let deps: Vec<String> = t.deps.iter().map(ToString::to_string).collect();
        let criteria: Vec<String> = t
            .criteria
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "  {}. [{}] {}",
                    i + 1,
                    if c.checked { "x" } else { " " },
                    c.text
                )
            })
            .collect();
        let notes: Vec<String> = t.notes.iter().map(|n| format!("  - {}", n.text)).collect();
        text(format!(
            "{} \"{}\" [{}] attempt {}\ndeps: {}\nbranch: {}\nPR: {}\n{}\ncriteria:\n{}\nnotes:\n{}",
            t.id,
            t.title,
            t.state,
            t.attempt,
            if deps.is_empty() {
                "none".into()
            } else {
                deps.join(", ")
            },
            t.branch.as_deref().unwrap_or("none"),
            t.pr.as_ref().map_or("none".into(), |p| p.url.clone()),
            t.body,
            criteria.join("\n"),
            notes.join("\n")
        ))
    }

    fn agent_line(&self, a: &Agent) -> String {
        let pending = crate::delivery::pending_for(&a.id, &self.st.inbox).len();
        let report = a
            .report
            .as_ref()
            .map_or(String::new(), |r| format!(" report:{:?}", r.status));
        format!(
            "{} {:?}/{:?}{report} ✉{pending}",
            a.path, a.life, a.activity
        )
    }
}
