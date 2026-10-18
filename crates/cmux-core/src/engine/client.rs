//! Client actions and the plan/task edits shared with the MCP tools.

use super::*;
use crate::ids::{AgentPath, ClientId, TermId};
use crate::model::{
    Activity, Criterion, Life, PlanState, TaskState, Terminal, TransitionSource,
    check_plan_transition, check_task_transition,
};
use crate::permissions::rule_for;

const MOUSE_THROTTLE_MS: u64 = 500;

fn text(s: impl Into<String>) -> Result<Reply, EngineError> {
    Ok(Reply::Text(s.into()))
}

impl Ctx<'_> {
    pub fn client(&mut self, by: ClientId, action: ClientAction) -> Result<Reply, EngineError> {
        match action {
            ClientAction::CreatePlan {
                title,
                repos,
                request,
            } => self.create_plan(title, repos, request),
            ClientAction::AcceptPlan { plan } => {
                let pi = self.plan_idx(&plan)?;
                self.accept_plan(pi, &by)?;
                Ok(Reply::Ok)
            }
            ClientAction::PausePlan { plan } => self.set_paused(&plan, true),
            ClientAction::ResumePlan { plan } => self.set_paused(&plan, false),
            ClientAction::CancelPlan { plan } => {
                let pi = self.plan_idx(&plan)?;
                check_plan_transition(self.plan(pi), PlanState::Cancelled).map_err(transition)?;
                self.cancel_plan(pi);
                Ok(Reply::Ok)
            }
            ClientAction::CreateTask {
                plan,
                title,
                body,
                repo,
                deps,
                criteria,
            } => {
                let pi = self.plan_idx(&plan)?;
                self.create_task(pi, title, body, repo, deps, criteria)
            }
            ClientAction::UpdateTask { plan, task, patch } => {
                let pi = self.plan_idx(&plan)?;
                self.update_task(pi, task, patch)
            }
            ClientAction::TransitionTask { plan, task, to } => {
                let pi = self.plan_idx(&plan)?;
                self.transition_task(pi, task, to, TransitionSource::Client)
            }
            ClientAction::SpawnTask {
                plan,
                task,
                instructions,
                force,
            } => {
                let pi = self.plan_idx(&plan)?;
                self.spawn_task(pi, task, instructions, force, false)
            }
            ClientAction::SendMessage { agent, text, steer } => {
                self.agent_idx(&agent)?;
                self.send(&agent, None, text, steer);
                Ok(Reply::Ok)
            }
            ClientAction::StopAgent { agent } => {
                let (pi, ai) = self.agent_idx(&agent)?;
                if !self.agent(pi, ai).is_live() {
                    return err(ErrorKind::Invalid, format!("{agent} is not running"));
                }
                self.stop_agent(pi, ai);
                Ok(Reply::Ok)
            }
            ClientAction::ResumeAgent { agent } => self.resume_agent(&agent, None),
            ClientAction::AnswerItem { item, action } => self.answer_item(&by, item, action),
            ClientAction::CreateTerminal { kind, cwd, size } => {
                let id = TermId(self.st.next_term_id.max(1));
                self.st.next_term_id = id.0 + 1;
                self.st.terminals.insert(
                    id,
                    Terminal {
                        id,
                        kind,
                        agent: None,
                        cwd,
                        size,
                        size_owner: None,
                        title: None,
                    },
                );
                self.st.loose.push(id);
                self.touch(Change::Terminal(id));
                text(id.0.to_string())
            }
            ClientAction::CloseTerminal { term } => {
                match self.st.terminals.get(&term) {
                    None => return err(ErrorKind::NotFound, format!("error: no terminal {term}")),
                    Some(t) if t.agent.is_some() => {
                        return err(
                            ErrorKind::Invalid,
                            format!("error: terminal {term} belongs to an agent; stop the agent"),
                        );
                    }
                    Some(_) => {}
                }
                self.st.terminals.remove(&term);
                self.st.loose.retain(|t| *t != term);
                self.touch(Change::Removed(ObjectRef::Terminal(term)));
                Ok(Reply::Ok)
            }
            ClientAction::MarkHumanInput { agent } => {
                let (pi, ai) = self.agent_idx(&agent)?;
                let last = self.agent(pi, ai).last_human_input_at;
                if last.is_none_or(|t| self.now.saturating_sub(t) >= MOUSE_THROTTLE_MS) {
                    self.agent_mut(pi, ai).last_human_input_at = Some(self.now);
                }
                Ok(Reply::Ok)
            }
        }
    }

    fn set_paused(&mut self, plan: &PlanId, paused: bool) -> Result<Reply, EngineError> {
        let pi = self.plan_idx(plan)?;
        let p = self.plan(pi);
        if p.state.is_terminal() {
            return err(
                ErrorKind::Invalid,
                format!(
                    "plan {} is {}; it cannot be paused or resumed",
                    p.id, p.state
                ),
            );
        }
        if p.paused != paused {
            self.plan_mut(pi).paused = paused;
            self.drain_queue();
        }
        Ok(Reply::Ok)
    }

    /// Moves a draft or proposed plan to accepted and starts the execute phase.
    pub fn accept_plan(&mut self, pi: usize, by: &ClientId) -> Result<(), EngineError> {
        check_plan_transition(self.plan(pi), PlanState::Accepted).map_err(transition)?;
        let exec = AgentId::new(self.plan(pi).id.clone(), AgentPath::Executor);
        let reviews: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| {
                i.is_pending() && i.kind == ItemKind::PlanReview && i.from.as_ref() == Some(&exec)
            })
            .map(|i| i.id)
            .collect();
        for r in reviews {
            self.set_status(
                r,
                Delivery::Answered {
                    by: by.clone(),
                    answer: "accept".into(),
                },
            );
        }
        self.plan_mut(pi).state = PlanState::Accepted;
        let kickoff = self.kickoff_execute(pi);
        self.hand_executor_kickoff(pi, kickoff);
        Ok(())
    }

    fn cancel_plan(&mut self, pi: usize) {
        // First close the plan and its queue: freeing a slot below must not
        // start a queued spawn.
        let p = self.plan_mut(pi);
        p.state = PlanState::Cancelled;
        p.spawn_queue.clear();
        for ai in 0..self.plan(pi).agents.len() {
            self.stop_agent(pi, ai);
        }
        for ti in 0..self.plan(pi).tasks.len() {
            let t = &self.plan(pi).tasks[ti];
            if check_task_transition(t, TaskState::Cancelled, TransitionSource::Client).is_ok() {
                self.task_mut(pi, ti).state = TaskState::Cancelled;
            }
        }
    }

    // ---- task edits (shared with the tools) ----

    fn check_editable(&self, pi: usize) -> Result<(), EngineError> {
        let p = self.plan(pi);
        if p.state.is_terminal() {
            return err(
                ErrorKind::Invalid,
                format!(
                    "plan {} is {}; its tasks can no longer change",
                    p.id, p.state
                ),
            );
        }
        Ok(())
    }

    /// Dependencies must exist and must not lead back to `task`.
    fn check_deps(
        &self,
        pi: usize,
        task: Option<TaskId>,
        deps: &[TaskId],
    ) -> Result<(), EngineError> {
        let plan = self.plan(pi);
        for d in deps {
            if plan.task(*d).is_none() {
                let ids: Vec<String> = plan.tasks.iter().map(|t| t.id.to_string()).collect();
                return err(
                    ErrorKind::Invalid,
                    format!("dependency {d} does not exist; tasks: {}", ids.join(", ")),
                );
            }
            if Some(*d) == task {
                return err(ErrorKind::Invalid, format!("{d} cannot depend on itself"));
            }
        }
        let Some(task) = task else { return Ok(()) };
        // Walk from each new dependency; reaching `task` means a cycle.
        let mut stack: Vec<TaskId> = deps.to_vec();
        let mut seen = Vec::new();
        while let Some(t) = stack.pop() {
            if t == task {
                return err(
                    ErrorKind::Invalid,
                    format!("these dependencies would make {task} depend on itself"),
                );
            }
            if !seen.contains(&t) {
                seen.push(t);
                stack.extend(plan.task(t).map(|x| x.deps.clone()).unwrap_or_default());
            }
        }
        Ok(())
    }

    fn check_repo(&self, pi: usize, repo: usize) -> Result<(), EngineError> {
        let n = self.plan(pi).repos.len();
        if repo >= n {
            return err(
                ErrorKind::Invalid,
                format!("repo {repo} does not exist; use 0..{n}"),
            );
        }
        Ok(())
    }

    pub fn create_task(
        &mut self,
        pi: usize,
        title: String,
        body: String,
        repo: usize,
        deps: Vec<TaskId>,
        criteria: Vec<String>,
    ) -> Result<Reply, EngineError> {
        self.check_editable(pi)?;
        if title.trim().is_empty() {
            return err(ErrorKind::Invalid, "a task needs a title");
        }
        self.check_repo(pi, repo)?;
        self.check_deps(pi, None, &deps)?;
        let id = TaskId::new(self.plan(pi).next_task_seq);
        let mut task = crate::model::Task::new(id, title, body);
        task.repo = repo;
        task.deps = deps;
        task.criteria = criteria
            .into_iter()
            .map(|text| Criterion {
                text,
                checked: false,
            })
            .collect();
        let plan = self.plan_mut(pi);
        plan.next_task_seq += 1;
        plan.tasks.push(task);
        let pid = plan.id.clone();
        self.touch(Change::Task(pid, id));
        text(format!("created {id}"))
    }

    pub fn update_task(
        &mut self,
        pi: usize,
        task: TaskId,
        patch: TaskPatch,
    ) -> Result<Reply, EngineError> {
        self.check_editable(pi)?;
        let ti = self.task_idx(pi, task)?;
        if self.plan(pi).tasks[ti].state.is_terminal() {
            return err(
                ErrorKind::Invalid,
                format!(
                    "{task} is {}; it can no longer change",
                    self.plan(pi).tasks[ti].state
                ),
            );
        }
        if let Some(r) = patch.repo {
            self.check_repo(pi, r)?;
        }
        if let Some(d) = &patch.deps {
            self.check_deps(pi, Some(task), d)?;
        }
        let t = &self.plan(pi).tasks[ti];
        let changed = patch.title.as_ref().is_some_and(|v| *v != t.title)
            || patch.body.as_ref().is_some_and(|v| *v != t.body)
            || patch.repo.is_some_and(|v| v != t.repo)
            || patch.deps.as_ref().is_some_and(|v| *v != t.deps);
        if changed {
            let t = self.task_mut(pi, ti);
            if let Some(v) = patch.title {
                t.title = v
            }
            if let Some(v) = patch.body {
                t.body = v
            }
            if let Some(v) = patch.repo {
                t.repo = v
            }
            if let Some(v) = patch.deps {
                t.deps = v
            }
        }
        text(format!("updated {task}"))
    }

    pub fn transition_task(
        &mut self,
        pi: usize,
        task: TaskId,
        to: TaskState,
        source: TransitionSource,
    ) -> Result<Reply, EngineError> {
        let ti = self.task_idx(pi, task)?;
        check_task_transition(&self.plan(pi).tasks[ti], to, source).map_err(transition)?;
        self.task_mut(pi, ti).state = to;
        if self.plan(pi).spawn_queue.iter().any(|q| q.task == task) && to.is_terminal() {
            self.plan_mut(pi).spawn_queue.retain(|q| q.task != task);
        }
        if to == TaskState::Cancelled {
            // Cancelling stops the task agent and its sub-agents.
            let id = AgentId::new(self.plan(pi).id.clone(), AgentPath::Task(task));
            if let Ok((_, ai)) = self.agent_idx(&id) {
                self.stop_agent(pi, ai);
            }
        }
        self.drain_queue();
        text(format!("{task} is now {to}"))
    }

    // ---- messages ----

    /// A message or steer from a client (`from` None) or an agent.
    pub fn send(&mut self, to: &AgentId, from: Option<AgentId>, body: String, steer: bool) {
        let (pi, ai) = self.agent_idx(to).expect("checked by the caller");
        let a = self.agent(pi, ai);
        if steer && a.life == Life::Running && a.activity == Activity::Working {
            self.fx.push(Effect::WritePty {
                agent: to.clone(),
                bytes: vec![0x1b],
            });
        }
        let kind = if steer {
            ItemKind::Steer
        } else {
            ItemKind::UserMessage
        };
        self.post(NewItem {
            to: Recipient::Agent(to.clone()),
            kind,
            from,
            body,
            dedupe: None,
        });
    }

    // ---- answering items ----

    fn answer_item(
        &mut self,
        by: &ClientId,
        id: ItemId,
        action: ItemAction,
    ) -> Result<Reply, EngineError> {
        let Some(ii) = self.item_idx(id) else {
            return err(ErrorKind::NotFound, format!("no item {id}"));
        };
        let item = self.st.inbox[ii].clone();
        if let Recipient::Agent(a) = &item.to {
            return err(
                ErrorKind::Invalid,
                format!("{id} is addressed to {}, not to a person", a.path()),
            );
        }
        match &item.status {
            Delivery::Pending => {}
            Delivery::Answered { by, .. } => {
                return err(
                    ErrorKind::Invalid,
                    format!("{id} was already answered by {by}"),
                );
            }
            Delivery::Expired => return err(ErrorKind::Invalid, format!("{id} has expired")),
            _ => return err(ErrorKind::Invalid, format!("{id} is closed")),
        }
        let answered = |answer: &str| Delivery::Answered {
            by: by.clone(),
            answer: answer.to_string(),
        };
        match (&item.kind, action) {
            (
                ItemKind::Permission {
                    tool,
                    input_summary,
                },
                a @ (ItemAction::Allow | ItemAction::AllowAlways | ItemAction::Deny { .. }),
            ) => {
                let (label, decision) = match a {
                    ItemAction::Deny { note } => {
                        let reason = if note.is_empty() {
                            "denied by the user".to_string()
                        } else {
                            note
                        };
                        (
                            format!("deny: {reason}"),
                            PermissionDecision::Deny { reason },
                        )
                    }
                    ItemAction::AllowAlways => {
                        ("allow_always".to_string(), PermissionDecision::Allow)
                    }
                    _ => ("allow".to_string(), PermissionDecision::Allow),
                };
                let agent = item.from.as_ref().and_then(|a| self.agent_idx(a).ok());
                if label == "allow_always" {
                    let held = self.st.held_permissions.get(&id).map(|(_, m)| m.clone());
                    let rule = rule_for(tool, held.as_deref().unwrap_or(input_summary));
                    // Without the held full input (after a restart) a Bash rule
                    // would come from the 60-char summary: build none.
                    let truncated =
                        held.is_none() && tool == "Bash" && input_summary.ends_with('…');
                    let task = agent.and_then(|(pi, ai)| Some((pi, self.agent(pi, ai).task?)));
                    if let (false, Some((pi, t))) = (truncated, task) {
                        let ti = self.task_idx(pi, t)?;
                        if !self.plan(pi).tasks[ti].allow_rules.contains(&rule) {
                            self.task_mut(pi, ti).allow_rules.push(rule);
                        }
                    }
                }
                if let Some((request_id, _)) = self.st.held_permissions.remove(&id) {
                    self.fx.push(Effect::AnswerPermission {
                        item: id,
                        request_id,
                        decision: Some(decision),
                    });
                }
                if let Some((pi, ai)) = agent
                    && self.agent(pi, ai).activity == Activity::NeedsPermission
                {
                    self.agent_mut(pi, ai).activity = Activity::Working;
                }
                self.set_status(id, answered(&label));
            }
            (ItemKind::Question, ItemAction::Reply { text }) => {
                self.set_status(id, answered(&text));
                if let Some(asker) = &item.from {
                    self.message(
                        asker,
                        None,
                        format!(
                            "Answer from the user to your question \"{}\": {text}",
                            item.body
                        ),
                    );
                }
            }
            (ItemKind::PlanReview, ItemAction::Accept) => {
                let from = item.from.as_ref().ok_or_else(|| {
                    EngineError::new(ErrorKind::Invalid, "plan review without a plan")
                })?;
                let pi = self.plan_idx(from.plan())?;
                self.accept_plan(pi, by)?;
            }
            (ItemKind::PlanReview, ItemAction::SendBack { note }) => {
                self.set_status(id, answered(&format!("send_back: {note}")));
                if let Some(exec) = &item.from {
                    self.message(
                        exec,
                        None,
                        format!("The user sent the plan back: {note}\nRevise it, then call update_plan with state proposed again."),
                    );
                }
            }
            (_, ItemAction::Dismiss) => {
                if let Some((request_id, _)) = self.st.held_permissions.remove(&id) {
                    // No decision: Claude shows its own prompt.
                    self.fx.push(Effect::AnswerPermission {
                        item: id,
                        request_id,
                        decision: None,
                    });
                }
                self.set_status(id, Delivery::Dismissed);
                if item.kind == ItemKind::Question
                    && let Some(asker) = &item.from
                {
                    self.message(
                        asker,
                        None,
                        format!(
                            "The user dismissed your question without an answer: {}",
                            item.body
                        ),
                    );
                }
            }
            (kind, _) => {
                let allowed = match kind {
                    ItemKind::Permission { .. } => "allow, allow always, deny",
                    ItemKind::Question => "reply, dismiss",
                    ItemKind::PlanReview => "accept, send back, dismiss",
                    _ => "dismiss",
                };
                return err(
                    ErrorKind::Invalid,
                    format!("{id} does not take that action; allowed: {allowed}"),
                );
            }
        }
        Ok(Reply::Ok)
    }
}

pub(super) fn transition(e: crate::model::TransitionError) -> EngineError {
    EngineError::new(ErrorKind::Invalid, e.to_string())
}
