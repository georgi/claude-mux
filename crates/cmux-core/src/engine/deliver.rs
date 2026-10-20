//! Delivery to agents (paths A, B, C), turn landing, and the 1 s tick.

use super::*;
use crate::delivery::{DeliveryEvent, HUMAN_TYPING_GRACE_MS, choose_path, digest, pending_for};
use crate::model::{
    Activity, DeliveryPath, Level, Report, ReportStatus, TaskState, TransitionSource,
    check_task_transition,
};

impl Ctx<'_> {
    /// Digest and ids of everything pending for `id`.
    fn pending_digest(&self, id: &AgentId) -> Option<(String, Vec<ItemId>)> {
        let items = pending_for(id, &self.st.inbox);
        if items.is_empty() {
            return None;
        }
        Some((digest(&items), items.iter().map(|i| i.id).collect()))
    }

    fn mark_delivered(&mut self, ids: Vec<ItemId>, via: DeliveryPath) {
        for id in ids {
            self.set_status(id, Delivery::Delivered { at: self.now, via });
        }
    }

    /// Path B: paste the digest if the agent is idle and nobody is typing.
    pub fn deliver_idle(&mut self, id: &AgentId) {
        let Ok((pi, ai)) = self.agent_idx(id) else {
            return;
        };
        let Some((text, ids)) = self.pending_digest(id) else {
            return;
        };
        if choose_path(self.agent(pi, ai), &DeliveryEvent::Idle, self.now, true)
            == Some(DeliveryPath::PtyPaste)
        {
            self.paste(id, text, ids);
        }
    }

    fn paste(&mut self, id: &AgentId, text: String, ids: Vec<ItemId>) {
        self.mark_delivered(ids.clone(), DeliveryPath::PtyPaste);
        if let Ok((pi, ai)) = self.agent_idx(id) {
            self.agent_mut(pi, ai)
                .unconfirmed
                .extend(ids.iter().copied());
        }
        self.fx.push(Effect::PasteDigest {
            agent: id.clone(),
            items: ids,
            text,
        });
    }

    /// A submitted prompt that names pasted item ids confirms those pastes.
    pub fn confirm_pastes(&mut self, pi: usize, ai: usize, prompt: &str) {
        let seen: Vec<ItemId> = self
            .agent(pi, ai)
            .unconfirmed
            .iter()
            .copied()
            .filter(|i| mentions(prompt, *i))
            .collect();
        if !seen.is_empty() {
            self.agent_mut(pi, ai)
                .unconfirmed
                .retain(|i| !seen.contains(i));
        }
    }

    /// The user answered in the terminal (or Claude moved on), so a held
    /// permission hook will never be read: release it without a decision.
    pub fn release_held(&mut self, id: &AgentId) {
        let held: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| i.is_pending() && i.from.as_ref() == Some(id))
            .filter(|i| matches!(i.kind, ItemKind::Permission { .. }))
            .map(|i| i.id)
            .collect();
        for item in held {
            if let Some((request_id, _)) = self.st.held_permissions.remove(&item) {
                self.fx.push(Effect::AnswerPermission {
                    item,
                    request_id,
                    decision: None,
                });
            }
            self.set_status(item, Delivery::Dismissed);
        }
    }

    pub fn has_held(&self, id: &AgentId) -> bool {
        self.st.inbox.iter().any(|i| {
            i.is_pending()
                && i.from.as_ref() == Some(id)
                && matches!(i.kind, ItemKind::Permission { .. })
        })
    }

    /// Path C: pending items ride along with a submitted prompt.
    pub fn on_prompt_submit(&mut self, pi: usize, ai: usize) -> HookReply {
        let id = self.agent(pi, ai).id.clone();
        let Some((text, ids)) = self.pending_digest(&id) else {
            return HookReply::None;
        };
        if choose_path(
            self.agent(pi, ai),
            &DeliveryEvent::PromptSubmit,
            self.now,
            true,
        )
        .is_none()
        {
            return HookReply::None;
        }
        self.mark_delivered(ids, DeliveryPath::PromptContext);
        HookReply::AdditionalContext(text)
    }

    /// `Stop`: path A, else landing (architecture 8.3), then path B.
    pub fn on_stop(&mut self, pi: usize, ai: usize, stop_hook_active: bool) -> HookReply {
        let id = self.agent(pi, ai).id.clone();
        let now = self.now;
        {
            let a = self.agent_mut(pi, ai);
            a.activity = Activity::Idle;
            a.idle_since = Some(now);
        }
        if let Some((text, ids)) = self.pending_digest(&id) {
            let ev = DeliveryEvent::Stop { stop_hook_active };
            if choose_path(self.agent(pi, ai), &ev, now, true) == Some(DeliveryPath::StopBlock) {
                self.mark_delivered(ids, DeliveryPath::StopBlock);
                let a = self.agent_mut(pi, ai);
                a.activity = Activity::Working;
                a.idle_since = None;
                a.consecutive_stop_blocks += 1;
                return HookReply::Block { reason: text };
            }
        }
        self.land(pi, ai);
        // Past the cap of path A (or a block that did not apply): paste if idle.
        self.deliver_idle(&id);
        HookReply::None
    }

    /// `base`, or `base#n` when earlier incarnations of the same agent (a resumed
    /// session, a sub-agent that reuses a name) already used it. `landed` keeps one
    /// incarnation from landing twice, so the key must not swallow the next one.
    fn unused_key(&self, base: String) -> String {
        let used = |d: &str| d == base || d.strip_prefix(&base).is_some_and(|r| r.starts_with('#'));
        let n = self
            .st
            .inbox
            .iter()
            .filter(|i| i.dedupe.as_deref().is_some_and(used))
            .count();
        if n == 0 { base } else { format!("{base}#{n}") }
    }

    /// Turn landing: a stored report becomes a parent item and a task move.
    pub fn land(&mut self, pi: usize, ai: usize) {
        let a = self.agent(pi, ai);
        let (Some(report), false) = (a.report.clone(), a.landed) else {
            return;
        };
        let (id, level, task_id, parent) = (a.id.clone(), a.level, a.task, a.parent.clone());
        // A sub-agent of an earlier attempt reports to nobody: its parent is
        // gone, and the new attempt must not take the report for its own.
        let orphan = level == Level::Sub && !self.current_attempt(pi, ai);
        self.agent_mut(pi, ai).landed = true;
        let (kind, verb) = if report.implicit {
            (ItemKind::ChildExited, "exited")
        } else {
            (ItemKind::ChildReport, "reported")
        };
        if let (Some(parent), false) = (parent, orphan) {
            let pr = report
                .pr_url
                .as_ref()
                .map_or(String::new(), |u| format!(" (PR: {u})"));
            self.post(NewItem {
                to: Recipient::Agent(parent),
                kind,
                from: Some(id.clone()),
                body: format!(
                    "{} {verb} {:?}: {}{pr}",
                    id.path(),
                    report.status,
                    report.summary
                ),
                dedupe: Some(self.unused_key(format!("report:{id}:{}", report.attempt))),
            });
        }
        // A report that is not blocked ends the agent's work: close what it still
        // asks, and a sub-agent's process goes (blocked ones can be told how to
        // continue, so they stay).
        if !report.implicit && report.status != ReportStatus::Blocked {
            self.close_questions_of(&id);
            if level == Level::Sub {
                self.stop_agent(pi, ai);
            }
        }
        if let (Level::Task, Some(t)) = (level, task_id)
            && let Ok(ti) = self.task_idx(pi, t)
        {
            let task = &self.plan(pi).tasks[ti];
            let target = match report.status {
                ReportStatus::Success if task.pr.is_some() => TaskState::Testing,
                ReportStatus::Success => TaskState::Passing,
                ReportStatus::Blocked => TaskState::Blocked,
                ReportStatus::Failed => TaskState::Failing,
            };
            // A report that supersedes a blocked one must not undo what the
            // executor or a human did to the task since.
            let moved_on = report.supersedes_blocked && task.state != TaskState::Blocked;
            if task.attempt == report.attempt
                && !moved_on
                && task.state != target
                && check_task_transition(task, target, TransitionSource::Agent).is_ok()
            {
                self.task_mut(pi, ti).state = target;
            }
        }
        self.drain_queue();
    }

    /// An agent is gone (exit, session end, failed spawn). Synthesizes the
    /// implicit report, whose summary is the transcript's last assistant text
    /// (or `why` when there is none), and the needs-you item.
    pub fn end_agent(&mut self, pi: usize, ai: usize, life: Life, why: &str) {
        let id = self.agent(pi, ai).id.clone();
        self.agent_mut(pi, ai).life = life;
        self.expire_held_for(&id);
        self.close_questions_of(&id);
        let a = self.agent(pi, ai);
        let (level, task, has_report) = (a.level, a.task, a.report.is_some());
        let attempt = task
            .and_then(|t| self.plan(pi).task(t))
            .map_or(0, |t| t.attempt);
        let summary = a.last_text.clone().unwrap_or_else(|| why.to_string());
        // The task agent is the sub-agents' parent: without it they are orphans.
        if let (Level::Task, Some(t)) = (level, task) {
            self.stop_subs(pi, t);
        }
        if level != Level::Executor && !has_report {
            self.agent_mut(pi, ai).report = Some(Report {
                status: ReportStatus::Blocked,
                summary,
                pr_url: None,
                attempt,
                implicit: true,
                supersedes_blocked: false,
            });
        }
        if level != Level::Executor {
            // An explicit report whose `Stop` never came lands now.
            self.land(pi, ai);
        }
        if level == Level::Executor || (level == Level::Task && !has_report) {
            self.post(NewItem {
                to: Recipient::Human,
                kind: ItemKind::ExitedWithoutReport,
                from: Some(id.clone()),
                body: format!("{} {why}", id.path()),
                dedupe: (level == Level::Task)
                    .then(|| self.unused_key(format!("exit:{id}:{attempt}"))),
            });
        }
        self.drain_queue();
    }

    /// Held permission requests of a dead agent can no longer be answered.
    pub fn expire_held_for(&mut self, id: &AgentId) {
        let held: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| i.is_pending() && i.from.as_ref() == Some(id))
            .filter(|i| matches!(i.kind, ItemKind::Permission { .. }))
            .map(|i| i.id)
            .collect();
        for item in held {
            self.st.held_permissions.remove(&item);
            self.set_status(item, Delivery::Expired);
        }
    }

    /// An agent that is gone, stopped or done can no longer be answered: its open
    /// questions leave the parent's and the person's lists.
    pub fn close_questions_of(&mut self, id: &AgentId) {
        let open: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| i.from.as_ref() == Some(id))
            .filter(|i| matches!(i.kind, ItemKind::Question | ItemKind::ChildQuestion))
            .filter(|i| matches!(i.status, Delivery::Pending | Delivery::Delivered { .. }))
            .map(|i| i.id)
            .collect();
        for item in open {
            self.set_status(item, Delivery::Dismissed);
        }
    }

    /// Closes a timed-out item and tells the asker, if it can still hear.
    fn expire(&mut self, item: ItemId) {
        let it = self.st.inbox[self.item_idx(item).expect("listed")].clone();
        self.set_status(item, Delivery::Expired);
        match (&it.kind, it.from) {
            (ItemKind::Permission { .. }, _) => {
                if let Some((request_id, _)) = self.st.held_permissions.remove(&item) {
                    self.fx.push(Effect::AnswerPermission {
                        item,
                        request_id,
                        decision: None,
                    });
                }
            }
            (_, Some(asker)) if self.can_hear(&asker) => {
                let mins = self.st.timeouts.question / 60_000;
                self.message(
                    &asker,
                    None,
                    format!(
                        "Your question got no answer within {mins} minutes and expired: {}",
                        it.body
                    ),
                );
            }
            _ => {}
        }
    }

    /// Whether a message to `id` would reach an agent: its process runs. An
    /// agent that is done for good (a sub-agent after its report) is stopped; a
    /// task agent stays up after a success report for the PR and CI loop.
    fn can_hear(&self, id: &AgentId) -> bool {
        self.agent_idx(id)
            .is_ok_and(|(pi, ai)| self.agent(pi, ai).is_live())
    }

    /// The 1 s timer.
    pub fn tick(&mut self) {
        let (now, t) = (self.now, self.st.timeouts);
        let due: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| {
                let age = now.saturating_sub(i.created);
                match i.kind {
                    ItemKind::Question | ItemKind::ChildQuestion => {
                        matches!(i.status, Delivery::Pending | Delivery::Delivered { .. })
                            && age >= t.question
                    }
                    ItemKind::Permission { .. } => i.is_pending() && age >= t.permission_wait,
                    _ => false,
                }
            })
            .map(|i| i.id)
            .collect();
        for item in due {
            self.expire(item);
        }
        self.watchdog();
        self.stall_watch();
        // Steers not picked up at a Stop are pasted after the steer timeout.
        let steered: Vec<AgentId> = self
            .st
            .inbox
            .iter()
            .filter(|i| i.is_pending() && i.kind == ItemKind::Steer)
            .filter(|i| now.saturating_sub(i.created) >= t.steer)
            .filter_map(|i| match &i.to {
                Recipient::Agent(a) => Some(a.clone()),
                Recipient::Human => None,
            })
            .collect();
        for id in steered {
            let Ok((pi, ai)) = self.agent_idx(&id) else {
                continue;
            };
            let a = self.agent(pi, ai);
            let typing = a
                .last_human_input_at
                .is_some_and(|at| now.saturating_sub(at) < HUMAN_TYPING_GRACE_MS);
            // Enter at a permission prompt would approve it.
            if a.life == Life::Running
                && a.activity != Activity::NeedsPermission
                && !typing
                && let Some((text, ids)) = self.pending_digest(&id)
            {
                self.paste(&id, text, ids);
            }
        }
        let ids: Vec<AgentId> = self
            .st
            .plans
            .iter()
            .flat_map(|p| p.agents.iter())
            .filter(|a| a.life == Life::Running && a.activity == Activity::Idle)
            .map(|a| a.id.clone())
            .collect();
        for id in ids {
            self.deliver_idle(&id);
        }
    }

    /// A task or sub-agent idle for 10 minutes with no report needs a human.
    fn watchdog(&mut self) {
        let (now, limit) = (self.now, self.st.timeouts.idle_without_report);
        let mut stalled = Vec::new();
        for plan in &self.st.plans {
            for a in &plan.agents {
                let parked = plan
                    .agents
                    .iter()
                    .any(|c| c.parent.as_ref() == Some(&a.id) && c.is_live() && !c.landed);
                let overdue = a.idle_since.is_some_and(|t| now.saturating_sub(t) >= limit);
                if a.level != Level::Executor
                    && a.life == Life::Running
                    && a.activity == Activity::Idle
                    && a.report.is_none()
                    && overdue
                    && !parked
                {
                    stalled.push((a.id.clone(), a.idle_since.unwrap_or_default()));
                }
            }
        }
        for (id, since) in stalled {
            // One item per idle period: a later stall of the same agent is new.
            self.post(NewItem {
                to: Recipient::Human,
                kind: ItemKind::ExitedWithoutReport,
                from: Some(id.clone()),
                body: format!(
                    "{} has been idle for {} minutes without a report",
                    id.path(),
                    limit / 60_000
                ),
                dedupe: Some(format!("idle:{id}:{since}")),
            });
        }
    }
}

impl Ctx<'_> {
    /// A hook, a tool call or a changed screen. `last_progress` moves in
    /// steps of `PROGRESS_GRAIN`, so a busy agent is not a delta per hook.
    pub fn progress(&mut self, pi: usize, ai: usize) {
        let now = self.now;
        let a = self.agent(pi, ai);
        if a.last_progress
            .is_none_or(|t| now.saturating_sub(t) >= PROGRESS_GRAIN)
        {
            self.agent_mut(pi, ai).last_progress = Some(now);
        }
        if self.agent(pi, ai).stalled {
            self.unstall(pi, ai);
        }
    }

    /// Clears the stalled flag and closes the open stalled item.
    pub fn unstall(&mut self, pi: usize, ai: usize) {
        let id = self.agent(pi, ai).id.clone();
        if self.agent(pi, ai).stalled {
            self.agent_mut(pi, ai).stalled = false;
        }
        let open: Vec<ItemId> = self
            .st
            .inbox
            .iter()
            .filter(|i| i.is_pending() && i.kind == ItemKind::Stalled)
            .filter(|i| i.from.as_ref() == Some(&id))
            .map(|i| i.id)
            .collect();
        for item in open {
            self.set_status(item, Delivery::Dismissed);
        }
    }

    /// A working agent with no progress for `timeouts.stall` is flagged and
    /// gets a needs-you item. Nothing is killed.
    fn stall_watch(&mut self) {
        let (now, limit) = (self.now, self.st.timeouts.stall);
        let mut hits = Vec::new();
        for (pi, plan) in self.st.plans.iter().enumerate() {
            for (ai, a) in plan.agents.iter().enumerate() {
                if a.life == Life::Running
                    && a.activity == Activity::Working
                    && !a.stalled
                    && let Some(since) = a.last_progress
                    && now.saturating_sub(since) >= limit
                {
                    hits.push((pi, ai, since));
                }
            }
        }
        for (pi, ai, since) in hits {
            let id = self.agent(pi, ai).id.clone();
            self.agent_mut(pi, ai).stalled = true;
            self.post(NewItem {
                to: Recipient::Human,
                kind: ItemKind::Stalled,
                from: Some(id.clone()),
                body: format!(
                    "{} is working but has made no progress for {} minutes",
                    id.path(),
                    now.saturating_sub(since) / 60_000
                ),
                dedupe: Some(format!("stall:{id}:{since}")),
            });
        }
    }
}

/// How finely `last_progress` is kept (the hub's own cadence for screens).
pub const PROGRESS_GRAIN: Timestamp = 30_000;

/// Whether `text` contains the id `I-<n>` as a whole token (`I-1` is not in `I-12`).
fn mentions(text: &str, id: ItemId) -> bool {
    let needle = id.to_string();
    text.match_indices(&needle).any(|(at, _)| {
        let after = text[at + needle.len()..].chars().next();
        !after.is_some_and(|c| c.is_ascii_digit())
    })
}
