//! Scenario tests: replay commands against a `State` and assert on state,
//! effects and deltas. The harness checks the rev/delta contract on every call.

use std::path::PathBuf;

use super::*;
use crate::ids::{AgentPath, ClientId, ItemId, PlanId, TaskId, TermId};
use crate::model::*;

const MIN: u64 = 60_000;

fn tok(id: &AgentId) -> [u8; 32] {
    let mut h = [1u8; 32];
    for (i, b) in id.to_string().bytes().enumerate() {
        h[i % 32] = h[i % 32].wrapping_mul(31).wrapping_add(b);
    }
    h[0] |= 1;
    h
}

fn t(n: u32) -> TaskId {
    TaskId::new(n)
}

fn me() -> ClientId {
    ClientId("tui".into())
}

/// Every object that differs between `before` and `after` has a delta, and
/// every non-removal delta names an object that really differs.
fn check_deltas_match_diff(before: &State, after: &State, deltas: &[Delta]) {
    let has = |c: &Change| deltas.iter().any(|d| d.change == *c);
    let mut changed: Vec<Change> = Vec::new();
    let mut removed: Vec<Change> = Vec::new();
    for p in &after.plans {
        let bp = before.plans.iter().find(|x| x.id == p.id);
        let strip = |p: &Plan| Plan {
            tasks: vec![],
            agents: vec![],
            ..p.clone()
        };
        if bp.is_none_or(|b| strip(b) != strip(p)) {
            changed.push(Change::Plan(p.id.clone()));
        }
        for tk in &p.tasks {
            if bp.and_then(|b| b.task(tk.id)) != Some(tk) {
                changed.push(Change::Task(p.id.clone(), tk.id));
            }
        }
        for a in &p.agents {
            if bp.and_then(|b| b.agent(&a.id)) != Some(a) {
                changed.push(Change::Agent(a.id.clone()));
            }
        }
        if let Some(b) = bp {
            for a in &b.agents {
                if p.agent(&a.id).is_none() {
                    removed.push(Change::Removed(ObjectRef::Agent(a.id.clone())));
                }
            }
        }
    }
    for i in &after.inbox {
        if before.inbox.iter().find(|x| x.id == i.id) != Some(i) {
            changed.push(Change::Item(i.id));
        }
    }
    for (id, tm) in &after.terminals {
        if before.terminals.get(id) != Some(tm) {
            changed.push(Change::Terminal(*id));
        }
    }
    for id in before.terminals.keys() {
        if !after.terminals.contains_key(id) {
            removed.push(Change::Removed(ObjectRef::Terminal(*id)));
        }
    }
    for c in changed.iter().chain(&removed) {
        assert!(has(c), "changed {c:?} has no delta; deltas: {deltas:?}");
    }
    for d in deltas {
        if !matches!(d.change, Change::Removed(_)) && !changed.contains(&d.change) {
            panic!("delta {:?} without a real change", d.change);
        }
    }
}

struct H {
    st: State,
    now: Timestamp,
    next_term: u32,
    /// Revs of every call that changed state, in order.
    revs: Vec<u64>,
}

/// Digest text pasted by `PasteDigest` effects, and "<esc>" for a steer's ESC.
fn writes(o: &Outcome, to: &AgentId) -> Vec<String> {
    o.effects
        .iter()
        .filter_map(|e| match e {
            Effect::PasteDigest { agent, text, .. } if agent == to => Some(text.clone()),
            Effect::WritePty { agent, bytes } if agent == to => {
                assert_eq!(bytes, &[0x1b], "WritePty is only for ESC");
                Some("<esc>".to_string())
            }
            _ => None,
        })
        .collect()
}

fn has_fx(o: &Outcome, f: impl Fn(&Effect) -> bool) -> bool {
    o.effects.iter().any(f)
}

fn hook_reply(o: &Outcome) -> HookReply {
    match &o.reply {
        Reply::Hook(h) => h.clone(),
        r => panic!("not a hook reply: {r:?}"),
    }
}

fn text_of(o: &Outcome) -> String {
    match &o.reply {
        Reply::Text(s) => s.clone(),
        r => panic!("not text: {r:?}"),
    }
}

impl H {
    fn new() -> H {
        H {
            st: State::default(),
            now: 1_792_000_000_000,
            next_term: 1,
            revs: vec![],
        }
    }

    /// Applies a command and checks the contract: errors change nothing;
    /// a changing call bumps rev once and tags every delta with it; a no-op
    /// emits no delta.
    fn go(&mut self, cmd: Command) -> Result<Outcome, EngineError> {
        let before = self.st.clone();
        let r = apply(&mut self.st, cmd, self.now);
        match &r {
            Err(e) => {
                assert_eq!(self.st, before, "state changed on error {e}");
                assert!(e.to_string().starts_with("error:"), "{e}");
            }
            Ok(o) => {
                let mut same = before.clone();
                same.rev = self.st.rev;
                // A reserved terminal id is a counter no view shows.
                same.next_term_id = self.st.next_term_id;
                if same == self.st {
                    assert_eq!(self.st.rev, before.rev, "rev bumped without a change");
                    assert!(
                        o.deltas.is_empty(),
                        "deltas without a change: {:?}",
                        o.deltas
                    );
                } else {
                    assert_eq!(self.st.rev, before.rev + 1, "rev must grow by exactly one");
                    assert!(!o.deltas.is_empty(), "a change without deltas");
                    assert!(o.deltas.iter().all(|d| d.rev == self.st.rev));
                    for (i, d) in o.deltas.iter().enumerate() {
                        assert!(
                            !o.deltas[..i].iter().any(|x| x.change == d.change),
                            "duplicate {d:?}"
                        );
                    }
                    self.revs.push(self.st.rev);
                    check_deltas_match_diff(&before, &self.st, &o.deltas);
                }
            }
        }
        r
    }

    fn ok(&mut self, cmd: Command) -> Outcome {
        self.go(cmd).unwrap_or_else(|e| panic!("unexpected {e}"))
    }

    fn client(&mut self, action: ClientAction) -> Result<Outcome, EngineError> {
        self.go(Command::Client {
            client: me(),
            action,
        })
    }

    fn tool(&mut self, who: &AgentId, call: ToolCall) -> Result<Outcome, EngineError> {
        self.go(Command::Tool {
            agent: who.clone(),
            token_hash: tok(who),
            call,
        })
    }

    fn hook(&mut self, who: &AgentId, hook: Hook) -> Result<Outcome, EngineError> {
        self.go(Command::Hook {
            agent: who.clone(),
            token_hash: tok(who),
            hook,
        })
    }

    fn runner(&mut self, ev: RunnerEvent) -> Outcome {
        self.ok(Command::Runner(ev))
    }

    fn tick(&mut self, after_ms: u64) -> Outcome {
        self.now += after_ms;
        self.ok(Command::Tick)
    }

    fn prompt(&mut self, who: &AgentId) -> Outcome {
        self.hook(who, Hook::UserPromptSubmit { prompt: "p".into() })
            .unwrap()
    }

    fn stop(&mut self, who: &AgentId, active: bool) -> Outcome {
        self.hook(
            who,
            Hook::Stop {
                stop_hook_active: active,
            },
        )
        .unwrap()
    }

    /// The runner spawned the PTY and `claude` started.
    fn boot(&mut self, who: &AgentId) {
        let term = TermId(self.next_term);
        self.next_term += 1;
        self.runner(RunnerEvent::Spawned {
            agent: who.clone(),
            term,
            session_id: format!("sess-{who}"),
            token_hash: tok(who),
        });
        self.hook(
            who,
            Hook::SessionStart {
                session_id: format!("sess-{who}"),
                transcript_path: None,
            },
        )
        .unwrap();
    }

    /// Kickoff submitted, turn over: the agent waits at its prompt.
    fn settle(&mut self, who: &AgentId) {
        self.prompt(who);
        self.stop(who, false);
    }

    fn plan(&self, p: &PlanId) -> &Plan {
        self.st.plans.iter().find(|x| x.id == *p).unwrap()
    }
    /// The terminal the agent's current process runs in.
    fn term_of(&self, id: &AgentId) -> TermId {
        self.agent(id).term.expect("the agent has a process")
    }

    fn agent(&self, id: &AgentId) -> &Agent {
        self.plan(id.plan()).agent(id).unwrap()
    }
    fn task(&self, p: &PlanId, n: u32) -> &Task {
        self.plan(p).task(t(n)).unwrap()
    }
    fn human(&self, f: impl Fn(&ItemKind) -> bool) -> Vec<&InboxItem> {
        self.st
            .inbox
            .iter()
            .filter(|i| i.to == Recipient::Human && f(&i.kind))
            .collect()
    }
    fn status(&self, id: ItemId) -> Delivery {
        self.st
            .inbox
            .iter()
            .find(|i| i.id == id)
            .unwrap()
            .status
            .clone()
    }
    fn items_to(&self, who: &AgentId) -> Vec<&InboxItem> {
        self.st
            .inbox
            .iter()
            .filter(|i| i.to == Recipient::Agent(who.clone()))
            .collect()
    }

    // ---- scenario building blocks ----

    /// A draft plan with a booted, idle executor.
    fn new_plan(&mut self, title: &str) -> (PlanId, AgentId) {
        let o = self
            .client(ClientAction::CreatePlan {
                title: title.into(),
                repos: vec![Repo {
                    path: PathBuf::from("/repo"),
                    base_branch: "main".into(),
                    remote_url: None,
                }],
                request: Some("build the thing".into()),
            })
            .unwrap();
        let pid: PlanId = text_of(&o).parse().unwrap();
        let exec = AgentId::new(pid.clone(), AgentPath::Executor);
        self.boot(&exec);
        self.settle(&exec);
        (pid, exec)
    }

    /// `n` independent tasks T-0001.. with one criterion each, made by the executor.
    fn add_tasks(&mut self, exec: &AgentId, n: u32) {
        for i in 1..=n {
            self.tool(
                exec,
                ToolCall::CreateTask {
                    title: format!("task {i}"),
                    body: format!("do part {i}"),
                    repo: 0,
                    deps: vec![],
                    criteria: vec![format!("criterion of {i}")],
                },
            )
            .unwrap();
        }
    }

    /// Planning, review and acceptance; the executor ends up Working on the execute kickoff.
    fn accepted_plan(&mut self, title: &str, tasks: u32) -> (PlanId, AgentId) {
        let (pid, exec) = self.new_plan(title);
        self.prompt(&exec);
        self.add_tasks(&exec, tasks);
        self.tool(
            &exec,
            ToolCall::UpdatePlan {
                state: Some(PlanState::Proposed),
                title: None,
            },
        )
        .unwrap();
        self.stop(&exec, false);
        self.client(ClientAction::AcceptPlan { plan: pid.clone() })
            .unwrap();
        self.prompt(&exec);
        (pid, exec)
    }

    fn task_agent(pid: &PlanId, n: u32) -> AgentId {
        AgentId::new(pid.clone(), AgentPath::Task(t(n)))
    }

    /// Executor spawns task `n`; worktree and process come up; the agent is working.
    fn start(&mut self, pid: &PlanId, exec: &AgentId, n: u32) -> AgentId {
        self.tool(
            exec,
            ToolCall::SpawnTask {
                task: t(n),
                instructions: None,
                force: false,
            },
        )
        .unwrap();
        self.up(pid, n)
    }

    fn up(&mut self, pid: &PlanId, n: u32) -> AgentId {
        self.runner(RunnerEvent::WorktreeReady {
            plan: pid.clone(),
            task: t(n),
            path: PathBuf::from(format!("/repo/.worktrees/{}", t(n))),
            branch: format!("claude/{}", t(n)),
        });
        let a = Self::task_agent(pid, n);
        self.boot(&a);
        self.prompt(&a);
        a
    }

    fn report(&mut self, a: &AgentId, status: ReportStatus) -> Result<Outcome, EngineError> {
        self.tool(
            a,
            ToolCall::ReportResult {
                status,
                summary: "done it".into(),
                pr_url: None,
            },
        )
    }
}

fn err_of(r: Result<Outcome, EngineError>) -> EngineError {
    r.expect_err("expected an error")
}

// ---------------------------------------------------------------------------

#[test]
fn planning_review_accept_and_send_back() {
    let mut h = H::new();
    let o = h
        .client(ClientAction::CreatePlan {
            title: "Auth refactor".into(),
            repos: vec![Repo {
                path: "/repo".into(),
                base_branch: "main".into(),
                remote_url: None,
            }],
            request: Some("split the auth module".into()),
        })
        .unwrap();
    let pid: PlanId = text_of(&o).parse().unwrap();
    assert!(pid.as_str().starts_with("P-2026"));
    assert!(pid.as_str().ends_with("-auth-refactor"));
    assert_eq!(h.plan(&pid).state, PlanState::Draft);
    let exec = AgentId::new(pid.clone(), AgentPath::Executor);
    assert_eq!(h.agent(&exec).life, Life::Starting);
    let Some(Effect::SpawnAgent {
        level,
        cwd,
        resume,
        system_prompt,
        kickoff,
        session_id,
        ..
    }) = o
        .effects
        .iter()
        .find(|e| matches!(e, Effect::SpawnAgent { .. }))
    else {
        panic!("no spawn effect")
    };
    assert_eq!(
        (*level, cwd.as_path(), *resume, session_id),
        (Level::Executor, "/repo".as_ref(), false, &None)
    );
    assert!(system_prompt.contains(pid.as_str()) && !system_prompt.contains("{{"));
    assert!(kickoff.contains("split the auth module"));
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Persist { plan } if *plan == pid)
    ));
    h.boot(&exec);
    h.settle(&exec);
    assert_eq!(h.agent(&exec).life, Life::Running);

    // Proposing needs tasks.
    let e = err_of(h.tool(
        &exec,
        ToolCall::UpdatePlan {
            state: Some(PlanState::Proposed),
            title: None,
        },
    ));
    assert!(e.to_string().contains("no tasks"), "{e}");
    h.add_tasks(&exec, 2);
    let o = h
        .tool(
            &exec,
            ToolCall::UpdatePlan {
                state: Some(PlanState::Proposed),
                title: None,
            },
        )
        .unwrap();
    assert_eq!(h.plan(&pid).state, PlanState::Proposed);
    let review = h.human(|k| *k == ItemKind::PlanReview)[0].id;
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Notify { item } if *item == review)
    ));
    // Proposing again while the review waits changes nothing.
    let o = h
        .tool(
            &exec,
            ToolCall::UpdatePlan {
                state: Some(PlanState::Proposed),
                title: None,
            },
        )
        .unwrap();
    assert!(o.deltas.is_empty());

    // Send back: the note reaches the executor, who proposes again.
    h.prompt(&exec);
    h.client(ClientAction::AnswerItem {
        item: review,
        action: ItemAction::SendBack {
            note: "split T-0002".into(),
        },
    })
    .unwrap();
    assert!(matches!(h.status(review), Delivery::Answered { .. }));
    assert_eq!(h.plan(&pid).state, PlanState::Proposed);
    let o = h.stop(&exec, false);
    assert!(
        matches!(hook_reply(&o), HookReply::Block { reason } if reason.contains("split T-0002"))
    );
    h.tool(
        &exec,
        ToolCall::UpdatePlan {
            state: Some(PlanState::Proposed),
            title: None,
        },
    )
    .unwrap();
    let reviews = h.human(|k| *k == ItemKind::PlanReview);
    assert_eq!(reviews.len(), 2);
    let second = reviews[1].id;

    // Accept through the item: the plan moves, the executor gets the task list.
    h.stop(&exec, false);
    let o = h
        .client(ClientAction::AnswerItem {
            item: second,
            action: ItemAction::Accept,
        })
        .unwrap();
    assert_eq!(h.plan(&pid).state, PlanState::Accepted);
    let pasted = writes(&o, &exec).join("");
    assert!(
        pasted.contains("- T-0001 [todo] task 1 deps:[] criteria:0/1"),
        "{pasted}"
    );
    assert!(pasted.contains("T-0002"));
    // Answering again: first answer wins.
    let e = err_of(h.client(ClientAction::AnswerItem {
        item: second,
        action: ItemAction::Accept,
    }));
    assert!(e.to_string().contains("already answered by tui"), "{e}");
}

#[test]
fn two_spawns_then_limit_then_report_frees_a_slot_and_wakes_the_executor() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("limits", 3);
    // Spawn needs an accepted plan; T1 and T2 start.
    let o = h
        .tool(
            &exec,
            ToolCall::SpawnTask {
                task: t(1),
                instructions: Some("be quick".into()),
                force: false,
            },
        )
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::CreateWorktree { branch, base, repo_path, .. }
        if branch == "claude/T-0001" && base == "main" && repo_path == &PathBuf::from("/repo"))
    ));
    let (task, a1) = (h.task(&pid, 1), H::task_agent(&pid, 1));
    assert_eq!((task.state, task.attempt), (TaskState::InProgress, 1));
    assert_eq!(h.agent(&a1).life, Life::Starting);
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(2),
            instructions: None,
            force: false,
        },
    )
    .unwrap();

    // The limit refuses a third, naming the limit; nothing changes.
    let e = err_of(h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(3),
            instructions: None,
            force: false,
        },
    ));
    assert_eq!(e.kind, ErrorKind::Limit);
    assert!(e.to_string().contains("2 of 2 task agents"), "{e}");
    assert_eq!(h.task(&pid, 3).state, TaskState::Todo);

    // Worktrees come up; the task agents are spawned with rendered prompts.
    let o = h.runner(RunnerEvent::WorktreeReady {
        plan: pid.clone(),
        task: t(1),
        path: "/repo/.worktrees/T-0001".into(),
        branch: "claude/T-0001".into(),
    });
    let Some(Effect::SpawnAgent {
        cwd,
        system_prompt,
        kickoff,
        level,
        ..
    }) = o
        .effects
        .iter()
        .find(|e| matches!(e, Effect::SpawnAgent { .. }))
    else {
        panic!()
    };
    assert_eq!(*level, Level::Task);
    assert_eq!(cwd, &PathBuf::from("/repo/.worktrees/T-0001"));
    assert!(system_prompt.contains("claude/T-0001") && !system_prompt.contains("{{"));
    assert!(
        kickoff.contains("do part 1")
            && kickoff.contains("be quick")
            && kickoff.contains("criterion of 1")
    );
    h.boot(&a1);
    h.prompt(&a1);
    h.up(&pid, 2);
    h.stop(&exec, false);

    // T1 reports. Landing waits for the Stop.
    h.report(&a1, ReportStatus::Success).unwrap();
    assert_eq!(h.task(&pid, 1).state, TaskState::InProgress);
    let e = err_of(h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(3),
            instructions: None,
            force: false,
        },
    ));
    assert_eq!(e.kind, ErrorKind::Limit);
    let o = h.stop(&a1, false);
    assert_eq!(hook_reply(&o), HookReply::None);
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    // The executor was idle: the report arrives by paste (path B).
    let pasted = writes(&o, &exec);
    assert_eq!(pasted.len(), 1);
    assert!(
        pasted[0].starts_with("[cmux] 1 message") && pasted[0].contains("T-0001 reported"),
        "{pasted:?}"
    );
    let report_item = h
        .items_to(&exec)
        .into_iter()
        .find(|i| i.kind == ItemKind::ChildReport)
        .unwrap();
    assert_eq!(
        report_item.dedupe.as_deref(),
        Some(format!("report:{a1}:1").as_str())
    );
    assert!(matches!(
        report_item.status,
        Delivery::Delivered {
            via: DeliveryPath::PtyPaste,
            ..
        }
    ));
    // The slot is free again.
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(3),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
}

#[test]
fn reports_move_the_task_by_status_and_pr() {
    let cases = [
        (ReportStatus::Success, false, TaskState::Passing),
        (ReportStatus::Success, true, TaskState::Testing),
        (ReportStatus::Blocked, false, TaskState::Blocked),
        (ReportStatus::Failed, true, TaskState::Failing),
    ];
    for (status, with_pr, want) in cases {
        let mut h = H::new();
        let (pid, exec) = h.accepted_plan("reports", 1);
        let a = h.start(&pid, &exec, 1);
        if with_pr {
            let e = err_of(h.tool(
                &a,
                ToolCall::SetTaskPr {
                    url: "https://example.com/x".into(),
                },
            ));
            assert!(e.to_string().contains("pull request URL"));
            h.tool(
                &a,
                ToolCall::SetTaskPr {
                    url: "https://github.com/o/r/pull/42".into(),
                },
            )
            .unwrap();
            assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().number, 42);
        }
        h.report(&a, status).unwrap();
        h.stop(&a, false);
        assert_eq!(h.task(&pid, 1).state, want, "{status:?} pr={with_pr}");
        assert!(h.agent(&a).landed);
    }
}

#[test]
fn a_report_from_an_idle_agent_lands_at_once() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("idle report", 1);
    let a = h.start(&pid, &exec, 1);
    h.stop(&a, false);
    h.tool(
        &a,
        ToolCall::ReportResult {
            status: ReportStatus::Success,
            summary: "x".into(),
            pr_url: Some("https://github.com/o/r/pull/7".into()),
        },
    )
    .unwrap();
    assert_eq!(h.task(&pid, 1).state, TaskState::Testing);
    assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().number, 7);
}

fn report_with_pr(h: &mut H, a: &AgentId, status: ReportStatus, pr: Option<&str>) {
    h.tool(
        a,
        ToolCall::ReportResult {
            status,
            summary: format!("{status:?} summary"),
            pr_url: pr.map(Into::into),
        },
    )
    .unwrap();
}

fn slots(h: &H, p: &PlanId) -> usize {
    let live = |a: &&Agent| a.is_live() && !a.landed && a.level == Level::Task;
    h.plan(p).agents.iter().filter(live).count()
}

fn child_reports<'a>(h: &'a H, to: &AgentId) -> Vec<&'a InboxItem> {
    h.items_to(to)
        .into_iter()
        .filter(|i| i.kind == ItemKind::ChildReport)
        .collect()
}

#[test]
fn blocked_report_is_superseded_by_success_with_pr() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("supersede", 1);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.stop(&a, false);
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    assert_eq!(child_reports(&h, &exec).len(), 1);
    // The agent works on after the executor's answer and reports again.
    h.hook(
        &a,
        Hook::UserPromptSubmit {
            prompt: "go on".into(),
        },
    )
    .unwrap();
    report_with_pr(
        &mut h,
        &a,
        ReportStatus::Success,
        Some("https://github.com/o/r/pull/9"),
    );
    h.stop(&a, false);
    assert_eq!(h.task(&pid, 1).state, TaskState::Testing);
    let reports = child_reports(&h, &exec);
    assert_eq!(reports.len(), 2);
    assert!(reports[0].body.contains("Blocked"));
    assert!(reports[1].body.contains("Success"));
    assert_ne!(reports[0].dedupe, reports[1].dedupe);
    // Success is final.
    let e = err_of(h.report(&a, ReportStatus::Failed));
    assert!(e.to_string().contains("only a blocked report"), "{e}");
}

#[test]
fn blocked_then_success_without_pr_passes_and_failed_fails() {
    for (status, want) in [
        (ReportStatus::Success, TaskState::Passing),
        (ReportStatus::Failed, TaskState::Failing),
    ] {
        let mut h = H::new();
        let (pid, exec) = h.accepted_plan("supersede", 1);
        let a = h.start(&pid, &exec, 1);
        h.report(&a, ReportStatus::Blocked).unwrap();
        h.stop(&a, false);
        // Idle agent: the new report lands at once.
        h.report(&a, status).unwrap();
        assert_eq!(h.task(&pid, 1).state, want);
        assert_eq!(child_reports(&h, &exec).len(), 2);
    }
}

#[test]
fn blocked_replaced_before_it_lands_moves_the_task() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("replace", 1);
    let a = h.start(&pid, &exec, 1);
    // Both in one turn: the blocked report never lands.
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.report(&a, ReportStatus::Success).unwrap();
    h.stop(&a, false);
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    let reports = child_reports(&h, &exec);
    assert_eq!(reports.len(), 1);
    assert!(reports[0].body.contains("Success"));
}

#[test]
fn blocked_landed_then_blocked_and_success_in_one_turn_moves_the_task() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("chain", 1);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.stop(&a, false);
    h.hook(
        &a,
        Hook::UserPromptSubmit {
            prompt: "go on".into(),
        },
    )
    .unwrap();
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.report(&a, ReportStatus::Success).unwrap();
    h.stop(&a, false);
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    assert_eq!(child_reports(&h, &exec).len(), 2);
}

#[test]
fn blocked_may_be_repeated_and_frees_the_slot_each_time() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("repeat", 1);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.stop(&a, false);
    assert_eq!(slots(&h, &pid), 0);
    // A second blocked report, while the agent is working, re-takes the slot
    // until it lands at the next Stop.
    h.hook(
        &a,
        Hook::UserPromptSubmit {
            prompt: "go on".into(),
        },
    )
    .unwrap();
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.stop(&a, false);
    assert_eq!(slots(&h, &pid), 0);
    assert_eq!(child_reports(&h, &exec).len(), 2);
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    h.report(&a, ReportStatus::Success).unwrap();
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    assert_eq!(child_reports(&h, &exec).len(), 3);
}

#[test]
fn late_report_after_the_task_moved_on_is_posted_but_does_not_move_it() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("late", 1);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Blocked).unwrap();
    h.stop(&a, false);
    h.tool(
        &exec,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::InProgress,
        },
    )
    .unwrap();
    h.report(&a, ReportStatus::Success).unwrap();
    assert_eq!(h.task(&pid, 1).state, TaskState::InProgress);
    assert_eq!(child_reports(&h, &exec).len(), 2);
}

#[test]
fn final_report_per_attempt_and_a_new_attempt_may_report_again() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("attempts", 1);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Success).unwrap();
    let e = err_of(h.report(&a, ReportStatus::Failed));
    assert!(e.to_string().contains("only a blocked report"), "{e}");
    h.stop(&a, false);
    // Rework: the executor blocks the task, stops the old agent, spawns again.
    h.tool(
        &exec,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Blocked,
        },
    )
    .unwrap();
    let e = err_of(h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    ));
    assert!(e.to_string().contains("already has a live agent"), "{e}");
    let o = h
        .tool(&exec, ToolCall::StopAgent { agent: a.clone() })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::KillAgent { agent, .. } if *agent == a)
    ));
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    assert_eq!(h.task(&pid, 1).attempt, 2);
    assert!(
        h.agent(&a).report.is_none(),
        "the report is cleared for the new attempt"
    );
    let a = h.up(&pid, 1);
    h.report(&a, ReportStatus::Success).unwrap();
}

#[test]
fn exit_without_a_report_is_an_implicit_blocked_report() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("exits", 2);
    let a = h.start(&pid, &exec, 1);
    h.stop(&exec, false);
    let o = h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(1),
    });
    let ag = h.agent(&a);
    assert_eq!(ag.life, Life::Exited { code: Some(1) });
    let r = ag.report.as_ref().unwrap();
    assert!(r.implicit && r.status == ReportStatus::Blocked && r.summary.contains("code 1"));
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    let needs = h.human(|k| *k == ItemKind::ExitedWithoutReport);
    assert_eq!(needs.len(), 1);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Notify { item } if *item == needs[0].id)
    ));
    let to_exec = h
        .items_to(&exec)
        .into_iter()
        .find(|i| i.kind == ItemKind::ChildExited)
        .unwrap();
    assert!(to_exec.body.contains("T-0001"));
    assert_eq!(writes(&o, &exec).len(), 1, "the idle executor is woken");
    // The same exit again, and the hooks of the dead agent, change nothing.
    assert!(
        h.runner(RunnerEvent::Exited {
            agent: a.clone(),
            term: h.term_of(&a),
            code: Some(1)
        })
        .deltas
        .is_empty()
    );
    let e = err_of(h.hook(
        &a,
        Hook::Stop {
            stop_hook_active: false,
        },
    ));
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(e.to_string().contains("has exited"));
    // Resuming voids the implicit report and puts the task back to work.
    let o = h
        .client(ClientAction::ResumeAgent { agent: a.clone() })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::SpawnAgent { resume: true, session_id: Some(s), .. } if s == &format!("sess-{a}"))
    ));
    assert!(h.agent(&a).report.is_none());
    assert_eq!(h.task(&pid, 1).state, TaskState::InProgress);
    assert_eq!(h.agent(&a).life, Life::Starting);
    assert!(!h.items_to(&exec).is_empty());
    assert!(
        h.human(|k| *k == ItemKind::ExitedWithoutReport)
            .iter()
            .all(|i| i.status == Delivery::Dismissed)
    );
}

#[test]
fn the_implicit_report_is_the_last_assistant_text() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("last text", 2);
    let a = h.start(&pid, &exec, 1);
    let tokens = TokenUsage {
        input: 10,
        output: 20,
        cache_read: 30,
        cache_write: 40,
    };
    let long = format!("I am stuck: {}", "x".repeat(3000));
    let o = h.runner(RunnerEvent::TokensUpdated {
        agent: a.clone(),
        tokens,
        last_text: Some(long.clone()),
    });
    assert!(!o.deltas.is_empty());
    assert_eq!(h.agent(&a).tokens, tokens);
    let kept = h.agent(&a).last_text.clone().unwrap();
    assert_eq!(kept.chars().count(), LAST_TEXT_CHARS);
    assert!(kept.starts_with("I am stuck: xxx") && kept.ends_with('…'));
    // The same read again changes nothing; a read with no text keeps the old one.
    let again = RunnerEvent::TokensUpdated {
        agent: a.clone(),
        tokens,
        last_text: Some(long),
    };
    assert!(h.runner(again).deltas.is_empty());
    let none = RunnerEvent::TokensUpdated {
        agent: a.clone(),
        tokens,
        last_text: None,
    };
    assert!(h.runner(none).deltas.is_empty());
    assert_eq!(h.agent(&a).last_text.as_deref(), Some(kept.as_str()));

    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.agent(&a).term.unwrap(),
        code: Some(0),
    });
    let r = h.agent(&a).report.clone().unwrap();
    assert!(r.implicit && r.status == ReportStatus::Blocked);
    assert_eq!(r.summary, kept);
    let to_exec = h
        .items_to(&exec)
        .into_iter()
        .find(|i| i.kind == ItemKind::ChildExited)
        .unwrap();
    assert!(to_exec.body.contains("I am stuck: xxx"), "{}", to_exec.body);
    // The human item still says why.
    let needs = h.human(|k| *k == ItemKind::ExitedWithoutReport);
    assert!(
        needs[0].body.contains("exited without a report"),
        "{}",
        needs[0].body
    );
}

#[test]
fn session_end_then_exit_code_and_a_clean_exit_after_a_report() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("session end", 2);
    let a = h.start(&pid, &exec, 1);
    h.hook(&a, Hook::SessionEnd { clear: false }).unwrap();
    assert_eq!(h.agent(&a).life, Life::Exited { code: None });
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(0),
    });
    assert_eq!(h.agent(&a).life, Life::Exited { code: Some(0) });
    assert_eq!(h.human(|k| *k == ItemKind::ExitedWithoutReport).len(), 1);

    // An agent that reported and then exits raises nothing more.
    let b = h.start(&pid, &exec, 2);
    h.report(&b, ReportStatus::Success).unwrap();
    h.stop(&b, false);
    h.runner(RunnerEvent::Exited {
        agent: b.clone(),
        term: h.term_of(&b),
        code: Some(0),
    });
    assert_eq!(h.task(&pid, 2).state, TaskState::Passing);
    assert_eq!(h.human(|k| *k == ItemKind::ExitedWithoutReport).len(), 1);
}

#[test]
fn stop_resume_and_late_events() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("stop", 1);
    let a = h.start(&pid, &exec, 1);
    let o = h
        .client(ClientAction::StopAgent { agent: a.clone() })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::KillAgent { agent, .. } if *agent == a)
    ));
    assert_eq!(h.agent(&a).life, Life::Stopped);
    assert!(
        h.agent(&a).report.is_none(),
        "a deliberate stop is not an implicit report"
    );
    assert!(
        h.runner(RunnerEvent::Exited {
            agent: a.clone(),
            term: h.term_of(&a),
            code: None
        })
        .deltas
        .is_empty()
    );
    assert_eq!(
        err_of(h.hook(
            &a,
            Hook::Stop {
                stop_hook_active: false
            }
        ))
        .kind,
        ErrorKind::Auth
    );
    assert!(
        err_of(h.client(ClientAction::StopAgent { agent: a.clone() }))
            .to_string()
            .contains("not running")
    );
    h.client(ClientAction::ResumeAgent { agent: a.clone() })
        .unwrap();
    assert!(
        err_of(h.client(ClientAction::ResumeAgent { agent: a.clone() }))
            .to_string()
            .contains("already running")
    );
    h.boot(&a);
    assert_eq!(h.agent(&a).life, Life::Running);
    // The old terminal is replaced by the new one.
    assert_eq!(
        h.st.terminals
            .values()
            .filter(|t| t.agent.as_ref() == Some(&a))
            .count(),
        1
    );
}

#[test]
fn question_and_answer_between_levels_and_to_the_user() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("questions", 1);
    let a = h.start(&pid, &exec, 1);
    h.stop(&exec, false);

    // ask_parent -> the executor; answer_child -> back to the task agent.
    h.tool(
        &a,
        ToolCall::AskParent {
            question: "which db?".into(),
        },
    )
    .unwrap();
    let q = h
        .items_to(&exec)
        .into_iter()
        .find(|i| i.kind == ItemKind::ChildQuestion)
        .unwrap()
        .id;
    assert!(matches!(
        h.status(q),
        Delivery::Delivered {
            via: DeliveryPath::PtyPaste,
            ..
        }
    ));
    h.prompt(&exec);
    h.tool(
        &exec,
        ToolCall::AnswerChild {
            agent: a.clone(),
            answer: "postgres".into(),
        },
    )
    .unwrap();
    assert!(matches!(h.status(q), Delivery::Answered { .. }));
    let answer = h.items_to(&a).into_iter().last().unwrap();
    assert!(answer.body.contains("postgres") && answer.is_pending());
    let e = err_of(h.tool(
        &exec,
        ToolCall::AnswerChild {
            agent: a.clone(),
            answer: "again".into(),
        },
    ));
    assert!(e.to_string().contains("no open question"), "{e}");
    // The task agent gets it at its next Stop (path A).
    let o = h.stop(&a, false);
    assert!(matches!(hook_reply(&o), HookReply::Block { reason } if reason.contains("postgres")));

    // ask_user -> a needs-you item; the reply goes to the asker.
    let o = h
        .tool(
            &a,
            ToolCall::AskUser {
                question: "ship it?".into(),
            },
        )
        .unwrap();
    let q = h.human(|k| *k == ItemKind::Question)[0].id;
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Notify { item } if *item == q)
    ));
    let e = err_of(h.client(ClientAction::AnswerItem {
        item: q,
        action: ItemAction::Allow,
    }));
    assert!(e.to_string().contains("allowed: reply, dismiss"), "{e}");
    h.client(ClientAction::AnswerItem {
        item: q,
        action: ItemAction::Reply { text: "yes".into() },
    })
    .unwrap();
    assert!(matches!(h.status(q), Delivery::Answered { answer, .. } if answer == "yes"));
    let msg = h.items_to(&a).into_iter().last().unwrap();
    assert!(msg.body.contains("ship it?") && msg.body.contains("yes"));
    // Items for agents cannot be answered by people.
    let e = err_of(h.client(ClientAction::AnswerItem {
        item: msg.id,
        action: ItemAction::Dismiss,
    }));
    assert!(e.to_string().contains("not to a person"), "{e}");
}

#[test]
fn questions_expire_after_sixty_minutes_and_the_asker_is_told() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("timeouts", 1);
    // The asker keeps working; this test is about questions, not stalls.
    h.st.timeouts.stall = u64::MAX;
    let a = h.start(&pid, &exec, 1);
    h.stop(&exec, false);
    h.tool(
        &a,
        ToolCall::AskUser {
            question: "user q".into(),
        },
    )
    .unwrap();
    h.tool(
        &a,
        ToolCall::AskParent {
            question: "parent q".into(),
        },
    )
    .unwrap();
    let uq = h.human(|k| *k == ItemKind::Question)[0].id;
    let pq = h
        .items_to(&exec)
        .into_iter()
        .find(|i| i.kind == ItemKind::ChildQuestion)
        .unwrap()
        .id;
    assert!(h.tick(59 * MIN).deltas.is_empty());
    let o = h.tick(MIN);
    assert!(!o.deltas.is_empty());
    assert_eq!(h.status(uq), Delivery::Expired);
    assert_eq!(h.status(pq), Delivery::Expired);
    let told: Vec<_> = h
        .items_to(&a)
        .into_iter()
        .filter(|i| i.body.contains("expired"))
        .collect();
    assert_eq!(told.len(), 2);
    assert!(
        told.iter().any(|i| i.body.contains("user q"))
            && told.iter().any(|i| i.body.contains("parent q"))
    );
    let e = err_of(h.client(ClientAction::AnswerItem {
        item: uq,
        action: ItemAction::Reply {
            text: "late".into(),
        },
    }));
    assert!(e.to_string().contains("expired"));
    assert!(h.tick(MIN).deltas.is_empty());
}

fn perm(id: &str, summary: &str, full: &str) -> Hook {
    Hook::PermissionRequest {
        request_id: id.into(),
        tool: "Bash".into(),
        input_summary: summary.into(),
        match_input: full.into(),
    }
}

fn held(o: &Outcome) -> ItemId {
    match hook_reply(o) {
        HookReply::Held { item } => item,
        r => panic!("expected Held, got {r:?}"),
    }
}

#[test]
fn permission_answered_by_a_client() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("perm", 1);
    let a = h.start(&pid, &exec, 1);
    let o = h
        .hook(&a, perm("r1", "cargo test -p api", "cargo test -p api"))
        .unwrap();
    let item = held(&o);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Notify { item: i } if *i == item)
    ));
    assert_eq!(h.agent(&a).activity, Activity::NeedsPermission);
    assert!(
        matches!(&h.st.inbox.iter().find(|i| i.id == item).unwrap().kind,
        ItemKind::Permission { tool, input_summary } if tool == "Bash" && input_summary == "cargo test -p api")
    );
    let o = h
        .client(ClientAction::AnswerItem {
            item,
            action: ItemAction::Allow,
        })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::AnswerPermission { item: i, request_id, decision: Some(PermissionDecision::Allow) }
        if *i == item && request_id == "r1")
    ));
    assert_eq!(h.agent(&a).activity, Activity::Working);
    assert!(matches!(h.status(item), Delivery::Answered { answer, .. } if answer == "allow"));
    assert!(h.task(&pid, 1).allow_rules.is_empty());
    // Deny carries the note.
    let item = held(
        &h.hook(&a, perm("r2", "rm -rf build", "rm -rf build"))
            .unwrap(),
    );
    let o = h
        .client(ClientAction::AnswerItem {
            item,
            action: ItemAction::Deny {
                note: "use cargo clean".into(),
            },
        })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::AnswerPermission { decision: Some(PermissionDecision::Deny { reason }), .. } if reason == "use cargo clean")
    ));
    // Only permission actions apply to a permission.
    let item = held(&h.hook(&a, perm("r3", "ls", "ls")).unwrap());
    let e = err_of(h.client(ClientAction::AnswerItem {
        item,
        action: ItemAction::Accept,
    }));
    assert!(e.to_string().contains("allow, allow always, deny"), "{e}");
}

#[test]
fn permission_answered_by_an_allow_rule() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("rules", 2);
    let a = h.start(&pid, &exec, 1);
    let item = held(
        &h.hook(&a, perm("r1", "cargo test -p api", "cargo test -p api"))
            .unwrap(),
    );
    let o = h
        .client(ClientAction::AnswerItem {
            item,
            action: ItemAction::AllowAlways,
        })
        .unwrap();
    assert!(has_fx(&o, |e| matches!(
        e,
        Effect::AnswerPermission {
            decision: Some(PermissionDecision::Allow),
            ..
        }
    )));
    assert_eq!(
        h.task(&pid, 1).allow_rules,
        vec![AllowRule {
            tool: "Bash".into(),
            pattern: "cargo test *".into()
        }]
    );

    // The next matching request is allowed at once: no item, no state change.
    let o = h
        .hook(&a, perm("r2", "cargo test -p other", "cargo test -p other"))
        .unwrap();
    assert_eq!(hook_reply(&o), HookReply::Allow);
    assert!(o.deltas.is_empty() && o.effects.is_empty());
    // A different tool is not covered.
    assert!(matches!(
        hook_reply(
            &h.hook(&a, perm("r3", "cargo publish", "cargo publish"))
                .unwrap()
        ),
        HookReply::Held { .. }
    ));
    // The summary is cut at 60 chars; the match runs on the full command, so a
    // hidden "; rm" is not auto-allowed even though the visible prefix matches.
    let full = format!("cargo test -p api {} ; rm -rf ~", "x".repeat(60));
    let summary: String = full.chars().take(59).chain(['…']).collect();
    let o = h.hook(&a, perm("r4", &summary, &full)).unwrap();
    assert!(
        matches!(hook_reply(&o), HookReply::Held { .. }),
        "{:?}",
        hook_reply(&o)
    );
    // Rules are per task: T-0002's agent still asks.
    let b = h.start(&pid, &exec, 2);
    assert!(matches!(
        hook_reply(
            &h.hook(&b, perm("r5", "cargo test -p api", "cargo test -p api"))
                .unwrap()
        ),
        HookReply::Held { .. }
    ));
    // The executor has no task and so no rules.
    assert!(matches!(
        hook_reply(
            &h.hook(&exec, perm("r6", "cargo test", "cargo test"))
                .unwrap()
        ),
        HookReply::Held { .. }
    ));
}

#[test]
fn permission_times_out_to_claudes_own_prompt() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("perm timeout", 1);
    let a = h.start(&pid, &exec, 1);
    let item = held(&h.hook(&a, perm("r9", "make", "make")).unwrap());
    assert!(h.tick(10 * MIN - 1).deltas.is_empty());
    let o = h.tick(1);
    assert_eq!(h.status(item), Delivery::Expired);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::AnswerPermission { item: i, request_id, decision: None } if *i == item && request_id == "r9")
    ));
    // A late answer is refused.
    assert!(
        err_of(h.client(ClientAction::AnswerItem {
            item,
            action: ItemAction::Allow
        }))
        .to_string()
        .contains("expired")
    );
    assert!(h.tick(MIN).effects.is_empty());
    // An agent that dies with a held request expires it without an answer.
    let item = held(&h.hook(&a, perm("r10", "make", "make")).unwrap());
    let o = h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(0),
    });
    assert_eq!(h.status(item), Delivery::Expired);
    assert!(!has_fx(&o, |e| matches!(
        e,
        Effect::AnswerPermission { .. }
    )));
}

#[test]
fn notification_marks_a_permission_prompt() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("notify", 1);
    let a = h.start(&pid, &exec, 1);
    h.hook(
        &a,
        Hook::Notification {
            kind: NotificationKind::IdlePrompt,
            message: "idle".into(),
        },
    )
    .unwrap();
    assert_eq!(h.agent(&a).activity, Activity::Working);
    h.hook(
        &a,
        Hook::Notification {
            kind: NotificationKind::PermissionPrompt,
            message: "needs permission".into(),
        },
    )
    .unwrap();
    assert_eq!(h.agent(&a).activity, Activity::NeedsPermission);
    h.hook(
        &a,
        Hook::PreToolUse {
            tool: "Bash".into(),
            input_summary: "ls".into(),
            match_input: "ls".into(),
        },
    )
    .unwrap();
    assert_eq!(h.agent(&a).activity, Activity::Working);
}

#[test]
fn steer_interrupts_and_delivers_at_stop_or_after_ten_seconds() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("steer", 1);
    // Working: ESC now, delivery at the Stop.
    let o = h
        .client(ClientAction::SendMessage {
            agent: exec.clone(),
            text: "change course".into(),
            steer: true,
        })
        .unwrap();
    assert_eq!(writes(&o, &exec), vec!["<esc>".to_string()]);
    let steer = h
        .items_to(&exec)
        .into_iter()
        .find(|i| i.kind == ItemKind::Steer)
        .unwrap()
        .id;
    assert_eq!(h.status(steer), Delivery::Pending);
    assert!(h.tick(9_999).deltas.is_empty());
    let o = h.stop(&exec, false);
    assert!(
        matches!(hook_reply(&o), HookReply::Block { reason } if reason.contains("change course"))
    );
    assert!(matches!(
        h.status(steer),
        Delivery::Delivered {
            via: DeliveryPath::StopBlock,
            ..
        }
    ));

    // No Stop comes: after 10 s the steer is pasted.
    h.client(ClientAction::SendMessage {
        agent: exec.clone(),
        text: "second try".into(),
        steer: true,
    })
    .unwrap();
    assert!(h.tick(9_999).effects.is_empty());
    let o = h.tick(1);
    let pasted = writes(&o, &exec);
    assert_eq!(pasted.len(), 1);
    assert!(pasted[0].contains("second try"));
    // A human typing holds the paste back.
    h.client(ClientAction::SendMessage {
        agent: exec.clone(),
        text: "third".into(),
        steer: true,
    })
    .unwrap();
    h.tick(10_000 - 100);
    h.client(ClientAction::MarkHumanInput {
        agent: exec.clone(),
    })
    .unwrap();
    assert!(writes(&h.tick(100), &exec).is_empty());
    assert_eq!(writes(&h.tick(3_000), &exec).len(), 1);
}

#[test]
fn a_paused_plan_spawns_nothing_but_still_delivers() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("paused", 2);
    h.client(ClientAction::PausePlan { plan: pid.clone() })
        .unwrap();
    assert!(
        h.client(ClientAction::PausePlan { plan: pid.clone() })
            .unwrap()
            .deltas
            .is_empty()
    );
    let e = err_of(h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    ));
    assert!(e.to_string().contains("paused"), "{e}");
    let o = h
        .client(ClientAction::SpawnTask {
            plan: pid.clone(),
            task: t(1),
            instructions: None,
            force: false,
        })
        .unwrap();
    assert!(text_of(&o).starts_with("queued T-0001"), "{}", text_of(&o));
    assert_eq!(h.plan(&pid).spawn_queue.len(), 1);
    assert!(!has_fx(&o, |e| matches!(e, Effect::CreateWorktree { .. })));
    assert_eq!(h.task(&pid, 1).state, TaskState::Todo);
    // Queuing twice is a no-op.
    assert!(
        h.client(ClientAction::SpawnTask {
            plan: pid.clone(),
            task: t(1),
            instructions: None,
            force: false
        })
        .unwrap()
        .deltas
        .is_empty()
    );
    // Deliveries still go.
    h.stop(&exec, false);
    let o = h
        .client(ClientAction::SendMessage {
            agent: exec.clone(),
            text: "ping".into(),
            steer: false,
        })
        .unwrap();
    assert_eq!(writes(&o, &exec).len(), 1);
    // Resume starts the queue.
    let o = h
        .client(ClientAction::ResumePlan { plan: pid.clone() })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::CreateWorktree { task, .. } if *task == t(1))
    ));
    assert!(h.plan(&pid).spawn_queue.is_empty());
    assert_eq!(h.task(&pid, 1).state, TaskState::InProgress);
}

#[test]
fn client_spawns_over_the_limit_queue_and_start_when_a_slot_frees() {
    let mut h = H::new();
    let (pid, _exec) = h.accepted_plan("queue", 3);
    let spawn = |h: &mut H, n| {
        h.client(ClientAction::SpawnTask {
            plan: pid.clone(),
            task: t(n),
            instructions: None,
            force: false,
        })
        .unwrap()
    };
    spawn(&mut h, 1);
    spawn(&mut h, 2);
    let a1 = h.up(&pid, 1);
    h.up(&pid, 2);
    let o = spawn(&mut h, 3);
    assert!(text_of(&o).starts_with("queued T-0003"));
    h.report(&a1, ReportStatus::Success).unwrap();
    let o = h.stop(&a1, false);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::CreateWorktree { task, .. } if *task == t(3))
    ));
    assert!(h.plan(&pid).spawn_queue.is_empty());
}

#[test]
fn cross_plan_tokens_and_targets_are_rejected() {
    let mut h = H::new();
    let (pa, ea) = h.accepted_plan("alpha", 1);
    let (_pb, eb) = h.accepted_plan("beta", 1);
    // Plan A's token on plan B's agent.
    let e = err_of(h.go(Command::Hook {
        agent: eb.clone(),
        token_hash: tok(&ea),
        hook: Hook::Stop {
            stop_hook_active: false,
        },
    }));
    assert_eq!(e.kind, ErrorKind::Auth);
    let e = err_of(h.go(Command::Tool {
        agent: eb.clone(),
        token_hash: tok(&ea),
        call: ToolCall::GetPlan,
    }));
    assert_eq!(e.kind, ErrorKind::Auth);
    // A valid token naming an agent of another plan.
    let a1 = h.start(&pa, &ea, 1);
    let e = err_of(h.tool(
        &ea,
        ToolCall::Send {
            agent: AgentId::new(_pb.clone(), AgentPath::Task(t(1))),
            text: "x".into(),
            interrupt: false,
        },
    ));
    assert_eq!(e.kind, ErrorKind::Forbidden);
    assert!(e.to_string().contains("another plan"), "{e}");
    let e = err_of(h.tool(&ea, ToolCall::StopAgent { agent: eb.clone() }));
    assert!(e.to_string().contains("another plan"), "{e}");
    // Unknown agent, zero and wrong tokens.
    let ghost = AgentId::new(pa.clone(), AgentPath::Task(t(9)));
    assert_eq!(
        err_of(h.go(Command::Tool {
            agent: ghost,
            token_hash: [1; 32],
            call: ToolCall::GetPlan
        }))
        .kind,
        ErrorKind::Auth
    );
    assert_eq!(
        err_of(h.go(Command::Tool {
            agent: a1.clone(),
            token_hash: [7; 32],
            call: ToolCall::GetPlan
        }))
        .kind,
        ErrorKind::Auth
    );
}

#[test]
fn a_starting_agent_cannot_authenticate_with_a_zero_hash() {
    let mut h = H::new();
    let o = h
        .client(ClientAction::CreatePlan {
            title: "zero".into(),
            repos: vec![Repo {
                path: "/repo".into(),
                base_branch: "main".into(),
                remote_url: None,
            }],
            request: Some("r".into()),
        })
        .unwrap();
    let exec = AgentId::new(text_of(&o).parse().unwrap(), AgentPath::Executor);
    let e = err_of(h.go(Command::Hook {
        agent: exec,
        token_hash: [0; 32],
        hook: Hook::SessionEnd { clear: false },
    }));
    assert_eq!(e.kind, ErrorKind::Auth);
}

#[test]
fn role_authorization() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("roles", 2);
    let task = h.start(&pid, &exec, 1);
    h.start(&pid, &exec, 2);
    h.tool(
        &task,
        ToolCall::SpawnSubagent {
            name: "a".into(),
            prompt: "p".into(),
        },
    )
    .unwrap();
    let sub = AgentId::new(pid.clone(), AgentPath::sub(t(1), "a").unwrap());
    h.boot(&sub);
    h.prompt(&sub);
    let patch = TaskPatch::default;
    let new_task = || ToolCall::CreateTask {
        title: "x".into(),
        body: "".into(),
        repo: 0,
        deps: vec![],
        criteria: vec![],
    };
    let deny: Vec<(&AgentId, ToolCall)> = vec![
        // sub-agents: add_note on their own task and report/ask_parent only
        (&sub, ToolCall::GetPlan),
        (&sub, ToolCall::ListTasks),
        (
            &sub,
            ToolCall::UpdatePlan {
                state: None,
                title: None,
            },
        ),
        (&sub, new_task()),
        (
            &sub,
            ToolCall::UpdateTask {
                task: t(1),
                patch: patch(),
            },
        ),
        (
            &sub,
            ToolCall::TransitionTask {
                task: t(1),
                to: TaskState::Testing,
            },
        ),
        (
            &sub,
            ToolCall::AddCriterion {
                task: t(1),
                text: "x".into(),
            },
        ),
        (
            &sub,
            ToolCall::CheckCriterion {
                task: t(1),
                criterion: 1,
                checked: true,
            },
        ),
        (
            &sub,
            ToolCall::SpawnTask {
                task: t(2),
                instructions: None,
                force: false,
            },
        ),
        (
            &sub,
            ToolCall::SpawnSubagent {
                name: "b".into(),
                prompt: "p".into(),
            },
        ),
        (
            &sub,
            ToolCall::Send {
                agent: task.clone(),
                text: "x".into(),
                interrupt: false,
            },
        ),
        (
            &sub,
            ToolCall::StopAgent {
                agent: task.clone(),
            },
        ),
        (&sub, ToolCall::ListAgents),
        (
            &sub,
            ToolCall::Peek {
                agent: task.clone(),
                lines: 5,
            },
        ),
        (
            &sub,
            ToolCall::SetTaskPr {
                url: "https://github.com/o/r/pull/1".into(),
            },
        ),
        (
            &sub,
            ToolCall::AnswerChild {
                agent: task.clone(),
                answer: "x".into(),
            },
        ),
        (
            &sub,
            ToolCall::AskUser {
                question: "x".into(),
            },
        ),
        (&sub, ToolCall::GetTask { task: Some(t(2)) }),
        (
            &sub,
            ToolCall::AddNote {
                task: t(2),
                text: "x".into(),
            },
        ),
        // task agents: own task only, testing/blocked only
        (
            &task,
            ToolCall::UpdatePlan {
                state: Some(PlanState::Done),
                title: None,
            },
        ),
        (&task, new_task()),
        (
            &task,
            ToolCall::UpdateTask {
                task: t(1),
                patch: patch(),
            },
        ),
        (
            &task,
            ToolCall::SpawnTask {
                task: t(2),
                instructions: None,
                force: false,
            },
        ),
        (
            &task,
            ToolCall::TransitionTask {
                task: t(1),
                to: TaskState::Merged,
            },
        ),
        (
            &task,
            ToolCall::TransitionTask {
                task: t(2),
                to: TaskState::Testing,
            },
        ),
        (
            &task,
            ToolCall::AddNote {
                task: t(2),
                text: "x".into(),
            },
        ),
        (
            &task,
            ToolCall::AddCriterion {
                task: t(2),
                text: "x".into(),
            },
        ),
        (
            &task,
            ToolCall::CheckCriterion {
                task: t(2),
                criterion: 1,
                checked: true,
            },
        ),
        (
            &task,
            ToolCall::Send {
                agent: exec.clone(),
                text: "x".into(),
                interrupt: false,
            },
        ),
        (
            &task,
            ToolCall::StopAgent {
                agent: exec.clone(),
            },
        ),
        (
            &task,
            ToolCall::AnswerChild {
                agent: exec.clone(),
                answer: "x".into(),
            },
        ),
        // the executor: no reporting, no sub-agents, no PR
        (
            &exec,
            ToolCall::ReportResult {
                status: ReportStatus::Success,
                summary: "x".into(),
                pr_url: None,
            },
        ),
        (
            &exec,
            ToolCall::SpawnSubagent {
                name: "a".into(),
                prompt: "p".into(),
            },
        ),
        (
            &exec,
            ToolCall::SetTaskPr {
                url: "https://github.com/o/r/pull/1".into(),
            },
        ),
        (
            &exec,
            ToolCall::AskParent {
                question: "x".into(),
            },
        ),
        (
            &exec,
            ToolCall::UpdatePlan {
                state: Some(PlanState::Cancelled),
                title: None,
            },
        ),
        (
            &exec,
            ToolCall::Send {
                agent: sub.clone(),
                text: "x".into(),
                interrupt: false,
            },
        ),
    ];
    for (who, call) in deny {
        let name = call.name();
        let e = err_of(h.tool(who, call));
        assert_eq!(e.kind, ErrorKind::Forbidden, "{who} {name}: {e}");
        assert!(e.to_string().starts_with("error:"), "{e}");
    }
    // The messages say who may.
    let e = err_of(h.tool(&sub, ToolCall::GetPlan));
    assert!(
        e.to_string().contains("available to: executor, task"),
        "{e}"
    );
    let e = err_of(h.tool(
        &task,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Merged,
        },
    ));
    assert!(e.to_string().contains("testing, blocked"), "{e}");

    // What is allowed works.
    h.tool(
        &sub,
        ToolCall::AddNote {
            task: t(1),
            text: "found it".into(),
        },
    )
    .unwrap();
    assert!(text_of(&h.tool(&sub, ToolCall::GetTask { task: None }).unwrap()).contains("T-0001"));
    h.tool(&task, ToolCall::GetPlan).unwrap();
    assert!(text_of(&h.tool(&task, ToolCall::ListTasks).unwrap()).contains("T-0002 [in_progress]"));
    h.tool(&task, ToolCall::GetTask { task: Some(t(2)) })
        .unwrap();
    h.tool(
        &task,
        ToolCall::CheckCriterion {
            task: t(1),
            criterion: 1,
            checked: true,
        },
    )
    .unwrap();
    assert!(
        err_of(h.tool(
            &task,
            ToolCall::CheckCriterion {
                task: t(1),
                criterion: 2,
                checked: true
            }
        ))
        .to_string()
        .contains("use 1..=1")
    );
    assert!(text_of(&h.tool(&task, ToolCall::ListAgents).unwrap()).contains("T-0001/a"));
    assert!(matches!(
        h.tool(
            &task,
            ToolCall::Peek {
                agent: sub.clone(),
                lines: 5
            }
        )
        .unwrap()
        .reply,
        Reply::Peek {
            term: Some(_),
            lines: 5,
            ..
        }
    ));
    h.tool(
        &task,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Testing,
        },
    )
    .unwrap();
    h.tool(
        &task,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Blocked,
        },
    )
    .unwrap();
    h.tool(
        &exec,
        ToolCall::Send {
            agent: task.clone(),
            text: "hi".into(),
            interrupt: false,
        },
    )
    .unwrap();
    assert!(text_of(&h.tool(&exec, ToolCall::ListAgents).unwrap()).contains("T-0002"));
    // Not a child: the executor's grandchild.
    assert!(
        err_of(h.tool(
            &exec,
            ToolCall::Peek {
                agent: sub.clone(),
                lines: 1
            }
        ))
        .to_string()
        .contains("not your child")
    );
}

#[test]
fn merged_requires_every_criterion_except_from_github() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("criteria", 2);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Success).unwrap();
    h.stop(&a, false);
    let e = err_of(h.tool(
        &exec,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Merged,
        },
    ));
    assert!(e.to_string().contains("1 of 1 criteria unchecked"), "{e}");
    let e = err_of(h.client(ClientAction::TransitionTask {
        plan: pid.clone(),
        task: t(1),
        to: TaskState::Merged,
    }));
    assert!(e.to_string().contains("criteria"), "{e}");
    h.tool(
        &exec,
        ToolCall::CheckCriterion {
            task: t(1),
            criterion: 1,
            checked: true,
        },
    )
    .unwrap();
    h.tool(
        &exec,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Merged,
        },
    )
    .unwrap();
    // Illegal edges list what is allowed.
    let e = err_of(h.tool(
        &exec,
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Todo,
        },
    ));
    assert!(e.to_string().contains("none (terminal)"), "{e}");
    // GitHub may report a merge with unchecked criteria.
    h.start(&pid, &exec, 2);
    h.runner(RunnerEvent::Pr {
        plan: pid.clone(),
        task: t(2),
        state: PrGithubState {
            merged: true,
            closed: false,
            ci: CiConclusion::Success,
            head_sha: None,
        },
    });
    assert_eq!(h.task(&pid, 2).state, TaskState::Merged);
}

#[test]
fn pr_changes_map_to_task_state_and_notify_agent_and_executor() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("prs", 1);
    let a = h.start(&pid, &exec, 1);
    h.tool(
        &a,
        ToolCall::SetTaskPr {
            url: "https://github.com/o/r/pull/5".into(),
        },
    )
    .unwrap();
    h.report(&a, ReportStatus::Success).unwrap();
    h.stop(&a, false);
    h.stop(&exec, false); // takes the report by path A
    h.stop(&exec, false);
    assert_eq!(h.task(&pid, 1).state, TaskState::Testing);
    let pr = |merged, closed, ci| RunnerEvent::Pr {
        plan: pid.clone(),
        task: t(1),
        state: PrGithubState {
            merged,
            closed,
            ci,
            head_sha: Some("abc".into()),
        },
    };
    // The first result records the CI mark but moves nothing.
    let inbox = h.st.inbox.len();
    h.runner(pr(false, false, CiConclusion::Pending));
    assert_eq!(h.st.inbox.len(), inbox, "no move: no item");
    assert!(
        h.runner(pr(false, false, CiConclusion::Pending))
            .deltas
            .is_empty(),
        "unchanged state: nothing"
    );
    let o = h.runner(pr(false, false, CiConclusion::Failure));
    assert_eq!(h.task(&pid, 1).state, TaskState::Failing);
    assert_eq!(
        h.items_to(&a)
            .iter()
            .filter(|i| i.kind == ItemKind::PrChanged)
            .count(),
        1
    );
    assert_eq!(
        h.items_to(&exec)
            .iter()
            .filter(|i| i.kind == ItemKind::PrChanged)
            .count(),
        1
    );
    assert!(writes(&o, &exec)[0].contains("testing -> failing"));
    assert!(
        h.runner(pr(false, false, CiConclusion::Failure))
            .deltas
            .is_empty()
    );
    h.runner(pr(false, false, CiConclusion::Success));
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    h.runner(pr(false, true, CiConclusion::Success));
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
}

#[test]
fn third_red_ci_goes_to_the_human_and_green_resets_the_count() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("ci", 1);
    let a = h.start(&pid, &exec, 1);
    h.tool(
        &a,
        ToolCall::SetTaskPr {
            url: "https://github.com/o/r/pull/41".into(),
        },
    )
    .unwrap();
    h.report(&a, ReportStatus::Success).unwrap();
    h.stop(&a, false);
    h.stop(&exec, false);
    h.stop(&exec, false);
    assert_eq!(h.task(&pid, 1).state, TaskState::Testing);
    let pr = |ci| RunnerEvent::Pr {
        plan: pid.clone(),
        task: t(1),
        state: PrGithubState {
            merged: false,
            closed: false,
            ci,
            head_sha: None,
        },
    };
    let to_agent = |h: &H| {
        h.items_to(&a)
            .iter()
            .filter(|i| i.body.contains("-> failing"))
            .count()
    };
    let ci_failing = |h: &H| h.human(|k| *k == ItemKind::CiFailing).len();
    // Two red runs, each followed by a new push: the task agent hears of both.
    for n in 1..=2u8 {
        h.runner(pr(CiConclusion::Failure));
        assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().ci_failures, n);
        assert_eq!(
            h.task(&pid, 1).pr.as_ref().unwrap().ci,
            Some(CiMark::Failing)
        );
        h.runner(pr(CiConclusion::Pending));
        assert_eq!(
            h.task(&pid, 1).pr.as_ref().unwrap().ci,
            Some(CiMark::Pending)
        );
        assert_eq!(
            h.task(&pid, 1).pr.as_ref().unwrap().ci_failures,
            n,
            "pending keeps it"
        );
    }
    assert_eq!(to_agent(&h), 2);
    assert_eq!(ci_failing(&h), 0);
    // The third goes to the human, not to the task agent.
    h.runner(pr(CiConclusion::Failure));
    assert_eq!(h.task(&pid, 1).state, TaskState::Failing);
    assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().ci_failures, 3);
    assert_eq!(to_agent(&h), 2, "the task agent is not told a third time");
    let items = h.human(|k| *k == ItemKind::CiFailing);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].from.as_ref(), Some(&a), "Message the task agent");
    assert!(
        items[0].body.contains("https://github.com/o/r/pull/41"),
        "Open PR"
    );
    assert!(items[0].kind.needs_human());
    assert!(
        h.items_to(&exec)
            .iter()
            .filter(|i| i.body.contains("-> failing"))
            .count()
            == 3,
        "the executor still hears of every move"
    );
    // The same red result again counts nothing.
    h.runner(pr(CiConclusion::Failure));
    assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().ci_failures, 3);
    assert_eq!(ci_failing(&h), 1);
    // Green resets the count; the next red goes to the task agent again.
    h.runner(pr(CiConclusion::Success));
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    let p = h.task(&pid, 1).pr.clone().unwrap();
    assert_eq!((p.ci, p.ci_failures), (Some(CiMark::Passing), 0));
    h.runner(pr(CiConclusion::Failure));
    assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().ci_failures, 1);
    assert_eq!(to_agent(&h), 3);
    assert_eq!(ci_failing(&h), 1);
    // A PR without checks has no mark.
    h.runner(pr(CiConclusion::None));
    assert_eq!(h.task(&pid, 1).pr.as_ref().unwrap().ci, None);
}

#[test]
fn path_a_blocks_three_times_then_path_b_pastes() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("path a", 1);
    for i in 0..3u8 {
        h.client(ClientAction::SendMessage {
            agent: exec.clone(),
            text: format!("m{i}"),
            steer: false,
        })
        .unwrap();
        let o = h.stop(&exec, i > 0);
        assert!(
            matches!(hook_reply(&o), HookReply::Block { reason } if reason.contains(&format!("m{i}"))),
            "stop {i}"
        );
        assert_eq!(h.agent(&exec).consecutive_stop_blocks, i + 1);
        assert!(writes(&o, &exec).is_empty());
    }
    // The fourth falls to path B.
    h.client(ClientAction::SendMessage {
        agent: exec.clone(),
        text: "m3".into(),
        steer: false,
    })
    .unwrap();
    let o = h.stop(&exec, true);
    assert_eq!(hook_reply(&o), HookReply::None);
    let pasted = writes(&o, &exec);
    assert_eq!(pasted.len(), 1);
    assert!(pasted[0].contains("m3"));
    let last = h.items_to(&exec).into_iter().last().unwrap();
    assert!(matches!(
        last.status,
        Delivery::Delivered {
            via: DeliveryPath::PtyPaste,
            ..
        }
    ));
    // The paste is confirmed by the prompt; a tool call resets the counter.
    h.prompt(&exec);
    h.hook(
        &exec,
        Hook::PreToolUse {
            tool: "Read".into(),
            input_summary: "f".into(),
            match_input: "f".into(),
        },
    )
    .unwrap();
    assert_eq!(h.agent(&exec).consecutive_stop_blocks, 0);
    h.client(ClientAction::SendMessage {
        agent: exec.clone(),
        text: "m4".into(),
        steer: false,
    })
    .unwrap();
    assert!(matches!(
        hook_reply(&h.stop(&exec, false)),
        HookReply::Block { .. }
    ));
}

#[test]
fn path_b_waits_for_idle_and_for_the_human_to_stop_typing() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("path b", 1);
    h.stop(&exec, false);
    h.client(ClientAction::MarkHumanInput {
        agent: exec.clone(),
    })
    .unwrap();
    // Throttled repeat: no state change.
    assert!(
        h.client(ClientAction::MarkHumanInput {
            agent: exec.clone()
        })
        .unwrap()
        .deltas
        .is_empty()
    );
    let o = h
        .client(ClientAction::SendMessage {
            agent: exec.clone(),
            text: "hello".into(),
            steer: false,
        })
        .unwrap();
    assert!(writes(&o, &exec).is_empty(), "someone is typing");
    assert!(h.tick(2_999).effects.is_empty());
    let o = h.tick(1);
    let pasted = writes(&o, &exec);
    assert_eq!(pasted.len(), 1);
    assert!(pasted[0].contains("[cmux] 1 message") && pasted[0].contains("hello"));
    // Delivered once: later ticks do nothing.
    assert!(h.tick(1_000).deltas.is_empty());
}

#[test]
fn path_c_piggybacks_pending_items_on_a_human_prompt() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("path c", 1);
    h.client(ClientAction::SendMessage {
        agent: exec.clone(),
        text: "also this".into(),
        steer: false,
    })
    .unwrap();
    let item = h.items_to(&exec).into_iter().last().unwrap().id;
    assert_eq!(h.status(item), Delivery::Pending);
    let o = h.prompt(&exec);
    assert!(
        matches!(hook_reply(&o), HookReply::AdditionalContext(c) if c.contains("also this") && c.contains(&item.to_string()))
    );
    assert!(matches!(
        h.status(item),
        Delivery::Delivered {
            via: DeliveryPath::PromptContext,
            ..
        }
    ));
    assert_eq!(hook_reply(&h.prompt(&exec)), HookReply::None);
}

#[test]
fn guard_rails_on_pre_tool_use() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("guards", 1);
    let task = h.start(&pid, &exec, 1);
    h.tool(
        &task,
        ToolCall::SpawnSubagent {
            name: "a".into(),
            prompt: "p".into(),
        },
    )
    .unwrap();
    let sub = AgentId::new(pid.clone(), AgentPath::sub(t(1), "a").unwrap());
    h.boot(&sub);
    let bash = |h: &mut H, who: &AgentId, cmd: &str| {
        let o = h
            .hook(
                who,
                Hook::PreToolUse {
                    tool: "Bash".into(),
                    input_summary: cmd.chars().take(60).collect(),
                    match_input: cmd.into(),
                },
            )
            .unwrap();
        hook_reply(&o)
    };
    let denied =
        |r: HookReply| matches!(r, HookReply::Deny { reason } if reason.starts_with("error:"));
    assert!(denied(bash(&mut h, &sub, "git push origin HEAD")));
    assert!(denied(bash(&mut h, &sub, "cargo build && git -C /x push")));
    let long = format!("{} && git push", "echo x ".repeat(20));
    assert!(
        denied(bash(&mut h, &sub, &long)),
        "past the 60-char summary"
    );
    assert!(denied(bash(&mut h, &sub, "git commit -m 'push it'")));
    assert_eq!(
        bash(&mut h, &task, "git push -u origin claude/T-0001"),
        HookReply::None
    );
    assert!(denied(bash(&mut h, &task, "git checkout main")));
    assert!(denied(bash(&mut h, &sub, "git switch main")));
    assert_eq!(
        bash(&mut h, &task, "git checkout -b other"),
        HookReply::None
    );
    assert_eq!(
        bash(&mut h, &task, "git checkout claude/T-0001"),
        HookReply::None
    );
    assert_eq!(bash(&mut h, &exec, "git checkout main"), HookReply::None);
    let o = h
        .hook(
            &task,
            Hook::PreToolUse {
                tool: "Read".into(),
                input_summary: "git push".into(),
                match_input: "git push".into(),
            },
        )
        .unwrap();
    assert_eq!(hook_reply(&o), HookReply::None);
}

#[test]
fn sub_agents_have_a_per_task_limit_share_the_worktree_and_report_to_the_task_agent() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("subs", 1);
    let task = h.start(&pid, &exec, 1);
    for n in ["a", "b"] {
        let o = h
            .tool(
                &task,
                ToolCall::SpawnSubagent {
                    name: n.into(),
                    prompt: format!("job {n}"),
                },
            )
            .unwrap();
        let Some(Effect::SpawnAgent {
            level,
            cwd,
            kickoff,
            ..
        }) = o
            .effects
            .iter()
            .find(|e| matches!(e, Effect::SpawnAgent { .. }))
        else {
            panic!()
        };
        assert_eq!(
            (*level, cwd),
            (Level::Sub, &PathBuf::from("/repo/.worktrees/T-0001"))
        );
        assert!(kickoff.contains(&format!("job {n}")));
    }
    let e = err_of(h.tool(
        &task,
        ToolCall::SpawnSubagent {
            name: "c".into(),
            prompt: "p".into(),
        },
    ));
    assert_eq!(e.kind, ErrorKind::Limit);
    assert!(e.to_string().contains("2 of 2 sub-agents"), "{e}");
    assert!(
        err_of(h.tool(
            &task,
            ToolCall::SpawnSubagent {
                name: "Bad Name".into(),
                prompt: "p".into()
            }
        ))
        .to_string()
        .contains("sub-agent name")
    );
    let sub = AgentId::new(pid.clone(), AgentPath::sub(t(1), "a").unwrap());
    h.boot(&sub);
    h.prompt(&sub);
    h.stop(&task, false);
    let e = err_of(h.tool(
        &sub,
        ToolCall::ReportResult {
            status: ReportStatus::Success,
            summary: "x".into(),
            pr_url: Some("https://github.com/o/r/pull/1".into()),
        },
    ));
    assert_eq!(e.kind, ErrorKind::Forbidden);
    h.report(&sub, ReportStatus::Success).unwrap();
    let o = h.stop(&sub, false);
    assert_eq!(
        h.task(&pid, 1).state,
        TaskState::InProgress,
        "a sub-agent report does not move the task"
    );
    let to_task = h
        .items_to(&task)
        .into_iter()
        .find(|i| i.kind == ItemKind::ChildReport)
        .unwrap();
    assert!(to_task.body.contains("T-0001/a reported"));
    assert_eq!(writes(&o, &task).len(), 1);
    // The slot is free again.
    h.tool(
        &task,
        ToolCall::SpawnSubagent {
            name: "c".into(),
            prompt: "p".into(),
        },
    )
    .unwrap();
}

#[test]
fn dependencies_must_be_merged_unless_forced() {
    let mut h = H::new();
    let (pid, exec) = h.new_plan("deps");
    h.add_tasks(&exec, 2);
    h.tool(
        &exec,
        ToolCall::CreateTask {
            title: "late".into(),
            body: "".into(),
            repo: 0,
            deps: vec![t(1), t(2)],
            criteria: vec![],
        },
    )
    .unwrap();
    assert!(
        err_of(h.tool(
            &exec,
            ToolCall::CreateTask {
                title: "x".into(),
                body: "".into(),
                repo: 0,
                deps: vec![t(9)],
                criteria: vec![]
            }
        ))
        .to_string()
        .contains("does not exist")
    );
    assert!(
        err_of(h.tool(
            &exec,
            ToolCall::UpdateTask {
                task: t(1),
                patch: TaskPatch {
                    deps: Some(vec![t(3)]),
                    ..Default::default()
                }
            }
        ))
        .to_string()
        .contains("depend on itself")
    );
    h.tool(
        &exec,
        ToolCall::UpdatePlan {
            state: Some(PlanState::Proposed),
            title: None,
        },
    )
    .unwrap();
    // Not accepted yet.
    assert!(
        err_of(h.tool(
            &exec,
            ToolCall::SpawnTask {
                task: t(1),
                instructions: None,
                force: false
            }
        ))
        .to_string()
        .contains("accepted")
    );
    h.client(ClientAction::AcceptPlan { plan: pid.clone() })
        .unwrap();
    let e = err_of(h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(3),
            instructions: None,
            force: false,
        },
    ));
    assert!(e.to_string().contains("T-0001, T-0002"), "{e}");
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(3),
            instructions: None,
            force: true,
        },
    )
    .unwrap();
    assert_eq!(h.task(&pid, 3).state, TaskState::InProgress);
}

#[test]
fn a_failed_worktree_puts_the_task_back_and_tells_the_executor() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("worktree", 1);
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    h.runner(RunnerEvent::WorktreeFailed {
        plan: pid.clone(),
        task: t(1),
        error: "dirty".into(),
    });
    assert_eq!(
        (h.task(&pid, 1).state, h.task(&pid, 1).attempt),
        (TaskState::Todo, 0)
    );
    assert!(h.plan(&pid).agent(&H::task_agent(&pid, 1)).is_none());
    assert!(
        h.items_to(&exec)
            .iter()
            .any(|i| i.body.contains("worktree failed: dirty"))
    );
    // A spawn failure of a started agent is an exit without a report.
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    h.runner(RunnerEvent::WorktreeReady {
        plan: pid.clone(),
        task: t(1),
        path: "/w".into(),
        branch: "b".into(),
    });
    let a = H::task_agent(&pid, 1);
    h.runner(RunnerEvent::SpawnFailed {
        agent: a.clone(),
        error: "no claude".into(),
    });
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    assert!(
        h.agent(&a)
            .report
            .as_ref()
            .unwrap()
            .summary
            .contains("no claude")
    );
}

#[test]
fn an_idle_agent_without_a_report_needs_a_human_after_ten_minutes_once() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("watchdog", 1);
    let a = h.start(&pid, &exec, 1);
    h.stop(&a, false);
    assert!(h.tick(10 * MIN - 1).deltas.is_empty());
    let o = h.tick(1);
    let items = h.human(|k| *k == ItemKind::ExitedWithoutReport);
    assert_eq!(items.len(), 1);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Notify { item } if *item == items[0].id)
    ));
    assert!(h.tick(MIN).deltas.is_empty(), "deduped");
    // A working agent, or a parked parent, never triggers it (a working
    // one may stall, which is another item).
    h.prompt(&a);
    h.tick(30 * MIN);
    assert_eq!(h.human(|k| *k == ItemKind::ExitedWithoutReport).len(), 1);
    // The human dismissed it and messaged the agent; a second stall is new.
    h.client(ClientAction::AnswerItem {
        item: items_id(&h),
        action: ItemAction::Dismiss,
    })
    .unwrap();
    h.stop(&a, false);
    h.tick(10 * MIN);
    assert_eq!(h.human(|k| *k == ItemKind::ExitedWithoutReport).len(), 2);
}

fn items_id(h: &H) -> ItemId {
    h.human(|k| *k == ItemKind::ExitedWithoutReport)[0].id
}

#[test]
fn plan_done_needs_all_tasks_settled_and_cancel_stops_everything() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("finish", 2);
    let e = err_of(h.tool(
        &exec,
        ToolCall::UpdatePlan {
            state: Some(PlanState::Done),
            title: None,
        },
    ));
    assert!(e.to_string().contains("T-0001 (todo)"), "{e}");
    let a = h.start(&pid, &exec, 1);
    h.tool(
        &exec,
        ToolCall::TransitionTask {
            task: t(2),
            to: TaskState::Cancelled,
        },
    )
    .unwrap();
    // Cancelling a running task stops its agent.
    let o = h
        .tool(
            &exec,
            ToolCall::TransitionTask {
                task: t(1),
                to: TaskState::Cancelled,
            },
        )
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::KillAgent { agent, .. } if *agent == a)
    ));
    let o = h
        .tool(
            &exec,
            ToolCall::UpdatePlan {
                state: Some(PlanState::Done),
                title: None,
            },
        )
        .unwrap();
    assert_eq!(h.plan(&pid).state, PlanState::Done);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::NotifyPlanDone { plan } if *plan == pid)
    ));
    assert!(
        err_of(h.tool(
            &exec,
            ToolCall::CreateTask {
                title: "x".into(),
                body: "".into(),
                repo: 0,
                deps: vec![],
                criteria: vec![]
            }
        ))
        .to_string()
        .contains("done")
    );

    let (p2, e2) = h.accepted_plan("cancel me", 2);
    let b = h.start(&p2, &e2, 1);
    let o = h
        .client(ClientAction::CancelPlan { plan: p2.clone() })
        .unwrap();
    assert_eq!(h.plan(&p2).state, PlanState::Cancelled);
    assert!(
        h.plan(&p2)
            .tasks
            .iter()
            .all(|t| t.state == TaskState::Cancelled)
    );
    assert_eq!(
        o.effects
            .iter()
            .filter(|e| matches!(e, Effect::KillAgent { .. }))
            .count(),
        2
    );
    assert_eq!(h.agent(&b).life, Life::Stopped);
    assert!(
        err_of(h.client(ClientAction::CancelPlan { plan: p2.clone() }))
            .to_string()
            .contains("allowed")
    );
}

#[test]
fn global_process_limit_applies_to_spawns() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("global", 2);
    h.st.global_limits.live_processes = 1;
    let e = err_of(h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    ));
    assert!(e.to_string().contains("server limit 1"), "{e}");
    let e = err_of(h.client(ClientAction::CreatePlan {
        title: "more".into(),
        repos: vec![Repo {
            path: "/r".into(),
            base_branch: "main".into(),
            remote_url: None,
        }],
        request: Some("".into()),
    }));
    assert_eq!(e.kind, ErrorKind::Limit);
}

#[test]
fn activity_follows_the_hooks() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("activity", 1);
    let a = h.start(&pid, &exec, 1);
    assert_eq!(h.agent(&a).activity, Activity::Working);
    h.stop(&a, false);
    assert_eq!(h.agent(&a).activity, Activity::Idle);
    assert_eq!(h.agent(&a).idle_since, Some(h.now));
    h.hook(
        &a,
        Hook::PreToolUse {
            tool: "Read".into(),
            input_summary: "f".into(),
            match_input: "f".into(),
        },
    )
    .unwrap();
    assert_eq!(h.agent(&a).activity, Activity::Working);
    assert_eq!(h.agent(&a).idle_since, None);
    // The same hook again changes nothing: no delta, no rev.
    let o = h
        .hook(
            &a,
            Hook::PreToolUse {
                tool: "Read".into(),
                input_summary: "f".into(),
                match_input: "f".into(),
            },
        )
        .unwrap();
    assert!(o.deltas.is_empty());
    // After /clear the session id changes.
    h.hook(
        &a,
        Hook::SessionStart {
            session_id: "new".into(),
            transcript_path: None,
        },
    )
    .unwrap();
    assert_eq!(h.agent(&a).session_id, "new");
}

#[test]
fn deltas_are_gap_free_over_a_long_scenario() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("long", 3);
    let a = h.start(&pid, &exec, 1);
    let b = h.start(&pid, &exec, 2);
    // Errors and no-ops in between never touch rev.
    let rev = h.st.rev;
    let _ = h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(3),
            instructions: None,
            force: false,
        },
    );
    let _ = h.tool(
        &a,
        ToolCall::UpdatePlan {
            state: None,
            title: None,
        },
    );
    h.tick(1_000);
    assert_eq!(h.st.rev, rev);
    h.hook(&a, perm("p", "ls", "ls")).unwrap();
    h.tool(
        &b,
        ToolCall::AskUser {
            question: "q".into(),
        },
    )
    .unwrap();
    h.report(&a, ReportStatus::Failed).unwrap();
    h.stop(&a, false);
    h.client(ClientAction::SendMessage {
        agent: b.clone(),
        text: "x".into(),
        steer: true,
    })
    .unwrap();
    h.stop(&b, false);
    h.runner(RunnerEvent::Exited {
        agent: b.clone(),
        term: h.term_of(&b),
        code: Some(2),
    });
    h.tick(61 * MIN);
    // Every changing call grew rev by exactly one, from the first one.
    assert_eq!(h.revs, (1..=h.st.rev).collect::<Vec<_>>());
    assert_eq!(h.revs.len() as u64, h.st.rev);
}

#[test]
fn removed_terminal_and_object_deltas_name_what_changed() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("deltas", 1);
    let o = h
        .tool(
            &exec,
            ToolCall::SpawnTask {
                task: t(1),
                instructions: None,
                force: false,
            },
        )
        .unwrap();
    let a = H::task_agent(&pid, 1);
    let changes: Vec<_> = o.deltas.iter().map(|d| d.change.clone()).collect();
    assert!(changes.contains(&Change::Task(pid.clone(), t(1))));
    assert!(changes.contains(&Change::Agent(a.clone())));
    h.up(&pid, 1);
    h.client(ClientAction::StopAgent { agent: a.clone() })
        .unwrap();
    let o = h
        .client(ClientAction::ResumeAgent { agent: a.clone() })
        .unwrap();
    assert!(
        o.deltas
            .iter()
            .any(|d| matches!(d.change, Change::Removed(ObjectRef::Terminal(_))))
    );
    let o = h.runner(RunnerEvent::Spawned {
        agent: a.clone(),
        term: TermId(99),
        session_id: "s2".into(),
        token_hash: tok(&a),
    });
    assert!(
        o.deltas
            .iter()
            .any(|d| d.change == Change::Terminal(TermId(99)))
    );
    assert!(h.st.next_term_id > 99);
}

#[test]
fn a_permission_answered_in_the_terminal_releases_the_held_hook() {
    // Claude moved on (PreToolUse, Stop, SessionEnd or a new request): the
    // held item is dismissed and the hook released with no decision.
    type Next = fn(&mut H, &AgentId);
    let nexts: [(&str, Next); 4] = [
        ("pre_tool_use", |h, a| {
            h.hook(
                a,
                Hook::PreToolUse {
                    tool: "Read".into(),
                    input_summary: "f".into(),
                    match_input: "f".into(),
                },
            )
            .unwrap();
        }),
        ("stop", |h, a| {
            h.stop(a, false);
        }),
        ("session_end", |h, a| {
            h.hook(a, Hook::SessionEnd { clear: false }).unwrap();
        }),
        ("new request", |h, a| {
            h.hook(a, perm("r2", "ls", "ls")).unwrap();
        }),
    ];
    for (name, next) in nexts {
        let mut h = H::new();
        let (pid, exec) = h.accepted_plan("race", 1);
        let a = h.start(&pid, &exec, 1);
        let item = held(&h.hook(&a, perm("r1", "make", "make")).unwrap());
        let before = h.st.rev;
        next(&mut h, &a);
        assert!(h.st.rev > before, "{name}");
        assert_eq!(h.status(item), Delivery::Dismissed, "{name}");
        assert!(!h.st.held_permissions.contains_key(&item), "{name}");
        // The late answer is refused.
        assert!(
            err_of(h.client(ClientAction::AnswerItem {
                item,
                action: ItemAction::Allow
            }))
            .to_string()
            .contains("closed"),
            "{name}"
        );
    }
    // The release effect itself.
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("race fx", 1);
    let a = h.start(&pid, &exec, 1);
    let item = held(&h.hook(&a, perm("r1", "make", "make")).unwrap());
    let o = h
        .hook(
            &a,
            Hook::PreToolUse {
                tool: "Read".into(),
                input_summary: "f".into(),
                match_input: "f".into(),
            },
        )
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::AnswerPermission { item: i, request_id, decision: None } if *i == item && request_id == "r1")
    ));
}

#[test]
fn a_permission_notification_is_ignored_while_a_request_is_held() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("held notify", 1);
    let a = h.start(&pid, &exec, 1);
    let item = held(&h.hook(&a, perm("r1", "make", "make")).unwrap());
    h.client(ClientAction::AnswerItem {
        item,
        action: ItemAction::Allow,
    })
    .unwrap();
    held(&h.hook(&a, perm("r2", "make", "make")).unwrap());
    // Held: the activity is already NeedsPermission and the notification adds nothing.
    let o = h
        .hook(
            &a,
            Hook::Notification {
                kind: NotificationKind::PermissionPrompt,
                message: "m".into(),
            },
        )
        .unwrap();
    assert!(o.deltas.is_empty());
}

#[test]
fn pasted_digests_are_confirmed_by_a_prompt_that_names_their_ids() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("confirm", 1);
    h.stop(&exec, false);
    let o = h
        .client(ClientAction::SendMessage {
            agent: exec.clone(),
            text: "one".into(),
            steer: false,
        })
        .unwrap();
    let Some(Effect::PasteDigest { items, text, .. }) = o
        .effects
        .iter()
        .find(|e| matches!(e, Effect::PasteDigest { .. }))
    else {
        panic!()
    };
    assert_eq!(items.len(), 1);
    assert!(text.contains(&items[0].to_string()));
    assert!(h.agent(&exec).unconfirmed.contains(&items[0]));
    // A human prompt that does not mention it leaves it unconfirmed.
    h.hook(
        &exec,
        Hook::UserPromptSubmit {
            prompt: "carry on".into(),
        },
    )
    .unwrap();
    assert!(h.agent(&exec).unconfirmed.contains(&items[0]));
    // "I-1" is not "I-12": whole ids only.
    let id = items[0];
    let other = format!("I-{}0", id.0);
    h.hook(
        &exec,
        Hook::UserPromptSubmit {
            prompt: format!("see {other}"),
        },
    )
    .unwrap();
    assert!(h.agent(&exec).unconfirmed.contains(&id));
    h.hook(
        &exec,
        Hook::UserPromptSubmit {
            prompt: format!("[cmux] 1 new message ({id}): read /tmp/x"),
        },
    )
    .unwrap();
    assert!(!h.agent(&exec).unconfirmed.contains(&id));
}

#[test]
fn a_user_prompt_resets_the_stop_block_counter() {
    let mut h = H::new();
    let (_pid, exec) = h.accepted_plan("reset", 1);
    h.client(ClientAction::SendMessage {
        agent: exec.clone(),
        text: "m".into(),
        steer: false,
    })
    .unwrap();
    h.stop(&exec, false);
    assert_eq!(h.agent(&exec).consecutive_stop_blocks, 1);
    h.prompt(&exec);
    assert_eq!(h.agent(&exec).consecutive_stop_blocks, 0);
}

// ---------------------------------------------------------------------------
// Adversarial review: regressions for bugs found in the first pass.
// ---------------------------------------------------------------------------

fn exec_children_reports(h: &H, exec: &AgentId) -> usize {
    h.items_to(exec)
        .iter()
        .filter(|i| matches!(i.kind, ItemKind::ChildReport | ItemKind::ChildExited))
        .count()
}

#[test]
fn cancel_plan_does_not_start_queued_spawns() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("cancel-queue", 3);
    h.start(&pid, &exec, 1);
    h.start(&pid, &exec, 2);
    let o = h
        .client(ClientAction::SpawnTask {
            plan: pid.clone(),
            task: t(3),
            instructions: None,
            force: false,
        })
        .unwrap();
    assert!(text_of(&o).starts_with("queued"));
    let o = h
        .client(ClientAction::CancelPlan { plan: pid.clone() })
        .unwrap();
    assert!(
        !has_fx(&o, |e| matches!(
            e,
            Effect::CreateWorktree { .. } | Effect::SpawnAgent { .. }
        )),
        "cancel must not start the queued task: {:?}",
        o.effects
    );
    assert!(
        h.plan(&pid).agents.iter().all(|a| !a.is_live()),
        "no live agent after cancel"
    );
    assert!(h.plan(&pid).spawn_queue.is_empty());
}

#[test]
fn a_report_followed_by_exit_before_stop_still_lands() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("exit-after-report", 1);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Success).unwrap();
    // Claude is killed before its Stop hook runs.
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(0),
    });
    assert_eq!(
        exec_children_reports(&h, &exec),
        1,
        "the executor must hear the report"
    );
    assert_eq!(h.task(&pid, 1).state, TaskState::Passing);
    assert!(h.agent(&a).landed);
    assert!(
        h.human(|k| *k == ItemKind::ExitedWithoutReport).is_empty(),
        "it did report"
    );
}

#[test]
fn a_report_after_a_resumed_exit_is_not_swallowed_by_the_dedupe_key() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("resume-report", 1);
    let a = h.start(&pid, &exec, 1);
    // Exit with no report: implicit blocked report, human item.
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(1),
    });
    assert_eq!(exec_children_reports(&h, &exec), 1);
    h.client(ClientAction::ResumeAgent { agent: a.clone() })
        .unwrap();
    h.boot(&a);
    h.prompt(&a);
    h.report(&a, ReportStatus::Success).unwrap();
    h.stop(&a, false);
    assert_eq!(
        exec_children_reports(&h, &exec),
        2,
        "the real report must reach the executor"
    );
}

#[test]
fn two_exits_without_report_in_one_attempt_both_reach_the_human() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("double-exit", 1);
    let a = h.start(&pid, &exec, 1);
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(1),
    });
    let first = h.human(|k| *k == ItemKind::ExitedWithoutReport)[0].id;
    // Resuming dismisses the first item.
    h.client(ClientAction::ResumeAgent { agent: a.clone() })
        .unwrap();
    assert_eq!(h.status(first), Delivery::Dismissed);
    h.boot(&a);
    h.prompt(&a);
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(1),
    });
    let open: Vec<_> = h
        .human(|k| *k == ItemKind::ExitedWithoutReport)
        .into_iter()
        .filter(|i| i.is_pending())
        .collect();
    assert_eq!(open.len(), 1, "the second death needs a human too");
    assert_eq!(exec_children_reports(&h, &exec), 2);
}

#[test]
fn a_respawned_sub_agent_with_a_reused_name_reports_to_its_parent() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-reuse", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = AgentId::new(pid.clone(), AgentPath::sub(t(1), "tests").unwrap());
    for round in 1..=2 {
        h.tool(
            &task,
            ToolCall::SpawnSubagent {
                name: "tests".into(),
                prompt: "run them".into(),
            },
        )
        .unwrap();
        h.boot(&sub);
        h.prompt(&sub);
        h.report(&sub, ReportStatus::Success).unwrap();
        h.stop(&sub, false);
        let reports = h
            .items_to(&task)
            .iter()
            .filter(|i| i.kind == ItemKind::ChildReport)
            .count();
        assert_eq!(
            reports, round,
            "round {round}: report must reach the task agent"
        );
        // Sub exits so the name is free again.
        h.runner(RunnerEvent::Exited {
            agent: sub.clone(),
            term: h.term_of(&sub),
            code: Some(0),
        });
    }
}

#[test]
fn dismissing_a_held_permission_releases_the_hook() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("dismiss-perm", 1);
    let a = h.start(&pid, &exec, 1);
    let item = held(&h.hook(&a, perm("r1", "ls", "ls")).unwrap());
    let o = h
        .client(ClientAction::AnswerItem {
            item,
            action: ItemAction::Dismiss,
        })
        .unwrap();
    assert!(
        has_fx(&o, |e| matches!(
            e,
            Effect::AnswerPermission { item: i, request_id, decision: None }
            if *i == item && request_id == "r1"
        )),
        "the held hook must be answered with no decision: {:?}",
        o.effects
    );
    assert!(h.st.held_permissions.is_empty());
    assert_eq!(h.status(item), Delivery::Dismissed);
}

#[test]
fn a_resumed_agent_starts_with_a_clean_activity_and_a_dead_token() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("resume-clean", 1);
    let a = h.start(&pid, &exec, 1);
    h.stop(&a, false);
    h.tick(9 * MIN);
    h.client(ClientAction::StopAgent { agent: a.clone() })
        .unwrap();
    h.client(ClientAction::SendMessage {
        agent: a.clone(),
        text: "welcome back".into(),
        steer: false,
    })
    .unwrap();
    h.client(ClientAction::ResumeAgent { agent: a.clone() })
        .unwrap();
    // The killed process's late SessionEnd must not end the resumed agent.
    let e = err_of(h.hook(&a, Hook::SessionEnd { clear: false }));
    assert_eq!(e.kind, ErrorKind::Auth);
    h.tick(2 * MIN);
    h.boot(&a);
    assert_eq!(h.agent(&a).activity, Activity::Unknown);
    let o = h.tick(MIN);
    assert!(
        writes(&o, &a).is_empty(),
        "nothing is pasted before the kickoff prompt"
    );
    assert!(
        h.human(|k| *k == ItemKind::ExitedWithoutReport).is_empty(),
        "an old idle_since must not raise a stall right after resume"
    );
}

#[test]
fn a_steer_timeout_never_pastes_into_a_permission_prompt() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("steer-perm", 1);
    let a = h.start(&pid, &exec, 1);
    h.hook(&a, perm("r1", "ls", "ls")).unwrap();
    assert_eq!(h.agent(&a).activity, Activity::NeedsPermission);
    h.client(ClientAction::SendMessage {
        agent: a.clone(),
        text: "stop that".into(),
        steer: true,
    })
    .unwrap();
    let o = h.tick(11_000);
    assert!(
        writes(&o, &a).is_empty(),
        "an Enter at a permission prompt would approve it"
    );
}

#[test]
fn respawning_a_task_drops_the_old_terminal() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("respawn-term", 1);
    let a = h.start(&pid, &exec, 1);
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(1),
    });
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    assert!(
        h.st.terminals
            .values()
            .all(|tm| tm.agent.as_ref() != Some(&a)),
        "the dead agent's terminal must be removed when its record is replaced"
    );
}

#[test]
fn allow_always_from_a_truncated_summary_adds_no_rule() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("trunc", 1);
    let a = h.start(&pid, &exec, 1);
    let long = format!("cargo test {}", "x".repeat(80));
    let summary =
        crate::permissions::summarize_input("Bash", &format!("{{\"command\":\"{long}\"}}"));
    assert!(summary.ends_with('…'));
    let item = held(&h.hook(&a, perm("r1", &summary, &long)).unwrap());
    // A restart forgets the held hooks (they are not persisted).
    h.st.held_permissions.clear();
    h.client(ClientAction::AnswerItem {
        item,
        action: ItemAction::AllowAlways,
    })
    .unwrap();
    assert!(
        h.task(&pid, 1).allow_rules.is_empty(),
        "a rule must never come from truncated text: {:?}",
        h.task(&pid, 1).allow_rules
    );
}

#[test]
fn identical_pr_and_second_permission_request_change_nothing_extra() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("noop-deltas", 1);
    let a = h.start(&pid, &exec, 1);
    let url = "https://github.com/o/r/pull/7".to_string();
    h.tool(&a, ToolCall::SetTaskPr { url: url.clone() })
        .unwrap();
    let o = h.tool(&a, ToolCall::SetTaskPr { url }).unwrap();
    assert!(o.deltas.is_empty());
    h.hook(&a, perm("r1", "ls", "ls")).unwrap();
    h.hook(&a, perm("r2", "pwd", "pwd")).unwrap();
    let _ = (&pid, &exec);
}

#[test]
fn two_plans_with_the_same_long_title_get_distinct_ids() {
    let mut h = H::new();
    let title = "a very long plan title that is well over forty characters wide";
    let mut ids = Vec::new();
    for _ in 0..3 {
        let o = h
            .client(ClientAction::CreatePlan {
                title: title.into(),
                repos: vec![Repo {
                    path: PathBuf::from("/repo"),
                    base_branch: "main".into(),
                    remote_url: None,
                }],
                request: Some("x".into()),
            })
            .unwrap();
        let id: PlanId = text_of(&o).parse().expect("a valid plan id");
        assert!(!ids.contains(&id), "duplicate {id}");
        ids.push(id);
    }
}

#[test]
fn resuming_an_agent_that_exited_without_a_report_respects_the_slot_limits() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("resume-limit", 3);
    let a1 = h.start(&pid, &exec, 1);
    h.runner(RunnerEvent::Exited {
        agent: a1.clone(),
        term: h.term_of(&a1),
        code: Some(1),
    });
    // Its slot is free, so two others fill the plan.
    h.start(&pid, &exec, 2);
    h.start(&pid, &exec, 3);
    let e = err_of(h.client(ClientAction::ResumeAgent { agent: a1.clone() }));
    assert_eq!(e.kind, ErrorKind::Limit, "{e}");

    // Same for a sub-agent of T-0002.
    let task = H::task_agent(&pid, 2);
    let (s1, s2, s3) = ("one", "two", "three");
    let sub = |n: &str| AgentId::new(pid.clone(), AgentPath::sub(t(2), n).unwrap());
    let spawn = |h: &mut H, n: &str| {
        h.tool(
            &task,
            ToolCall::SpawnSubagent {
                name: n.into(),
                prompt: "p".into(),
            },
        )
    };
    spawn(&mut h, s1).unwrap();
    h.boot(&sub(s1));
    h.runner(RunnerEvent::Exited {
        agent: sub(s1),
        term: h.term_of(&sub(s1)),
        code: Some(1),
    });
    spawn(&mut h, s2).unwrap();
    spawn(&mut h, s3).unwrap();
    let e = err_of(h.client(ClientAction::ResumeAgent { agent: sub(s1) }));
    assert_eq!(e.kind, ErrorKind::Limit, "{e}");
}

#[test]
fn clear_ends_the_session_but_not_the_agent() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("clear", 1);
    let a = h.start(&pid, &exec, 1);
    h.stop(&a, false);
    // `/clear` in the terminal: SessionEnd(reason clear), then SessionStart with a new id.
    h.hook(&a, Hook::SessionEnd { clear: true }).unwrap();
    h.hook(
        &a,
        Hook::SessionStart {
            session_id: "after-clear".into(),
            transcript_path: Some("/p/after-clear.jsonl".into()),
        },
    )
    .unwrap();
    assert_eq!(h.agent(&a).life, Life::Running);
    assert_eq!(h.agent(&a).session_id, "after-clear");
    // The new transcript is followed from here on.
    assert_eq!(
        h.agent(&a).transcript.as_deref(),
        Some(std::path::Path::new("/p/after-clear.jsonl"))
    );
    assert!(h.agent(&a).report.is_none());
    assert_eq!(h.task(&pid, 1).state, TaskState::InProgress);
    assert!(h.human(|k| *k == ItemKind::ExitedWithoutReport).is_empty());
    assert!(
        h.items_to(&exec)
            .iter()
            .all(|i| i.kind != ItemKind::ChildExited)
    );
    // The agent can still report.
    h.prompt(&a);
    h.report(&a, ReportStatus::Success).unwrap();
}

#[test]
fn allow_always_on_git_with_a_flag_does_not_approve_every_git_command() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("git rule", 1);
    let a = h.start(&pid, &exec, 1);
    let cmd = "git -C /repo/.worktrees/T-0001 status";
    let item = held(&h.hook(&a, perm("r1", cmd, cmd)).unwrap());
    h.client(ClientAction::AnswerItem {
        item,
        action: ItemAction::AllowAlways,
    })
    .unwrap();
    let push = "git -C /repo/.worktrees/T-0001 push --force origin main";
    let o = h.hook(&a, perm("r2", push, push)).unwrap();
    assert!(
        matches!(hook_reply(&o), HookReply::Held { .. }),
        "rules {:?} auto-allowed {push:?}",
        h.task(&pid, 1).allow_rules
    );
}

#[test]
fn role_prompts_name_only_tools_the_role_may_call() {
    use crate::prompts::Template;
    let a = AgentId::new(PlanId::new(2026, 10, 18, "x"), AgentPath::Executor);
    let calls = [
        ToolCall::GetPlan,
        ToolCall::UpdatePlan {
            state: None,
            title: None,
        },
        ToolCall::ListTasks,
        ToolCall::GetTask { task: None },
        ToolCall::CreateTask {
            title: String::new(),
            body: String::new(),
            repo: 0,
            deps: vec![],
            criteria: vec![],
        },
        ToolCall::UpdateTask {
            task: t(1),
            patch: TaskPatch::default(),
        },
        ToolCall::TransitionTask {
            task: t(1),
            to: TaskState::Testing,
        },
        ToolCall::AddCriterion {
            task: t(1),
            text: String::new(),
        },
        ToolCall::CheckCriterion {
            task: t(1),
            criterion: 1,
            checked: true,
        },
        ToolCall::AddNote {
            task: t(1),
            text: String::new(),
        },
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
        ToolCall::SpawnSubagent {
            name: String::new(),
            prompt: String::new(),
        },
        ToolCall::Send {
            agent: a.clone(),
            text: String::new(),
            interrupt: false,
        },
        ToolCall::StopAgent { agent: a.clone() },
        ToolCall::ListAgents,
        ToolCall::Peek {
            agent: a.clone(),
            lines: 1,
        },
        ToolCall::SetTaskPr { url: String::new() },
        ToolCall::ReportResult {
            status: ReportStatus::Success,
            summary: String::new(),
            pr_url: None,
        },
        ToolCall::AskParent {
            question: String::new(),
        },
        ToolCall::AnswerChild {
            agent: a.clone(),
            answer: String::new(),
        },
        ToolCall::AskUser {
            question: String::new(),
        },
    ];
    for (tpl, level) in [
        (Template::Executor, Level::Executor),
        (Template::Task, Level::Task),
        (Template::Subagent, Level::Sub),
    ] {
        let text = tpl.builtin();
        let tools = text.split("## Tools").nth(1).expect("a Tools section");
        for c in &calls {
            let named = tools.contains(&format!("`{}`", c.name()))
                || tools.contains(&format!("`{}(", c.name()));
            assert!(
                !named || c.roles().contains(&level),
                "{} lists {} but {level:?} may not call it",
                tpl.file_name(),
                c.name()
            );
        }
    }
}

#[test]
fn ask_user_names_the_configured_question_timeout() {
    let mut h = H::new();
    h.st.timeouts.question = 15 * MIN;
    let (_pid, exec) = h.accepted_plan("ask timeout", 1);
    let o = h
        .tool(
            &exec,
            ToolCall::AskUser {
                question: "q".into(),
            },
        )
        .unwrap();
    assert!(text_of(&o).contains("15 minutes"), "{}", text_of(&o));
}

#[test]
fn a_new_attempt_closes_the_old_attempts_exited_item() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("retry", 1);
    let a = h.start(&pid, &exec, 1);
    h.runner(RunnerEvent::Exited {
        agent: a.clone(),
        term: h.term_of(&a),
        code: Some(1),
    });
    let item = h.human(|k| *k == ItemKind::ExitedWithoutReport)[0].id;
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    h.start(&pid, &exec, 1);
    assert_eq!(h.task(&pid, 1).attempt, 2);
    assert_eq!(h.status(item), Delivery::Dismissed);
}

#[test]
fn loose_terminals_are_created_sized_titled_and_closed() {
    let mut h = H::new();
    let o = h
        .client(ClientAction::CreateTerminal {
            kind: TermKind::Shell,
            cwd: PathBuf::from("/home"),
            size: (100, 30),
        })
        .unwrap();
    let id = TermId(text_of(&o).parse().unwrap());
    assert_eq!(h.st.loose, vec![id]);
    assert_eq!(h.st.terminals[&id].size, (100, 30));
    let second = h
        .client(ClientAction::CreateTerminal {
            kind: TermKind::Claude,
            cwd: PathBuf::from("/home"),
            size: (80, 24),
        })
        .unwrap();
    assert_ne!(text_of(&second), id.0.to_string());

    let size = |rows, owner: Option<&str>| RunnerEvent::TermSize {
        term: id,
        rows,
        cols: 90,
        owner: owner.map(|o| ClientId(o.into())),
    };
    h.runner(size(40, Some("web")));
    assert_eq!(h.st.terminals[&id].size, (90, 40));
    assert_eq!(h.st.terminals[&id].size_owner, Some(ClientId("web".into())));
    // Unchanged: no new rev (H::go checks this).
    h.runner(size(40, Some("web")));
    h.runner(RunnerEvent::TermTitle {
        term: id,
        title: Some("vim".into()),
    });
    h.runner(RunnerEvent::TermTitle {
        term: id,
        title: Some("vim".into()),
    });
    // A gone terminal is ignored.
    h.runner(RunnerEvent::TermTitle {
        term: TermId(99),
        title: None,
    });

    h.client(ClientAction::CloseTerminal { term: id }).unwrap();
    assert!(!h.st.terminals.contains_key(&id));
    assert!(!h.st.loose.contains(&id));
    let e = err_of(h.client(ClientAction::CloseTerminal { term: id }));
    assert!(e.to_string().contains("no terminal"), "{e}");
}

#[test]
fn a_spawned_after_stop_kills_the_process_it_names() {
    // Stopped while Starting: the term is not on the agent yet, so the kill
    // must carry the one the runner reports.
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("late-kill", 1);
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    let a = H::task_agent(&pid, 1);
    h.client(ClientAction::StopAgent { agent: a.clone() })
        .unwrap();
    let o = h.runner(RunnerEvent::Spawned {
        agent: a.clone(),
        term: TermId(42),
        session_id: "s".into(),
        token_hash: tok(&a),
    });
    assert!(has_fx(&o, |e| matches!(
        e,
        Effect::KillAgent { agent, term: Some(TermId(42)) } if *agent == a
    )));
}

#[test]
fn spawn_effects_and_loose_terminals_never_share_a_term_id() {
    let mut h = H::new();
    let o = h
        .client(ClientAction::CreatePlan {
            title: "ids".into(),
            repos: vec![Repo {
                path: "/repo".into(),
                base_branch: "main".into(),
                remote_url: None,
            }],
            request: Some("r".into()),
        })
        .unwrap();
    let Some(Effect::SpawnAgent { term, .. }) = o
        .effects
        .iter()
        .find(|e| matches!(e, Effect::SpawnAgent { .. }))
    else {
        panic!("no spawn effect")
    };
    let loose = h
        .client(ClientAction::CreateTerminal {
            kind: TermKind::Shell,
            cwd: "/".into(),
            size: (80, 24),
        })
        .unwrap();
    assert_ne!(text_of(&loose), term.0.to_string());
}

#[test]
fn a_change_touching_no_plan_still_persists_the_globals() {
    let mut h = H::new();
    let o = h
        .client(ClientAction::CreateTerminal {
            kind: TermKind::Shell,
            cwd: "/".into(),
            size: (80, 24),
        })
        .unwrap();
    assert!(has_fx(&o, |e| matches!(e, Effect::PersistGlobals)));
}

#[test]
fn only_https_pull_request_urls_are_accepted() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("pr-scheme", 1);
    let a = h.start(&pid, &exec, 1);
    for url in [
        "http://github.com/o/r/pull/42",
        "javascript:alert(1)//pull/42",
        "file:///etc/pull/42",
        "github.com/o/r/pull/42",
    ] {
        let e = err_of(h.tool(&a, ToolCall::SetTaskPr { url: url.into() }));
        assert!(e.to_string().contains("pull request URL"), "{url}: {e}");
    }
    assert!(h.task(&pid, 1).pr.is_none());
}

// ---- sub-agent lifecycle ----

fn sub_id(pid: &PlanId, n: u32, name: &str) -> AgentId {
    AgentId::new(pid.clone(), AgentPath::sub(t(n), name).unwrap())
}

/// The task agent spawns sub-agent `name` and its process comes up and works.
fn start_sub(h: &mut H, pid: &PlanId, task: &AgentId, name: &str) -> AgentId {
    h.tool(
        task,
        ToolCall::SpawnSubagent {
            name: name.into(),
            prompt: "p".into(),
        },
    )
    .unwrap();
    let sub = sub_id(pid, 1, name);
    h.boot(&sub);
    h.prompt(&sub);
    sub
}

fn live_processes(h: &H, pid: &PlanId) -> usize {
    h.plan(pid).agents.iter().filter(|a| a.is_live()).count()
}

#[test]
fn a_sub_agent_that_reported_success_or_failure_is_stopped() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-stop", 1);
    let task = h.start(&pid, &exec, 1);
    for (name, status) in [("a", ReportStatus::Success), ("b", ReportStatus::Failed)] {
        let sub = start_sub(&mut h, &pid, &task, name);
        h.report(&sub, status).unwrap();
        let o = h.stop(&sub, false);
        assert_eq!(h.agent(&sub).life, Life::Stopped, "{name}");
        assert!(has_fx(
            &o,
            |e| matches!(e, Effect::KillAgent { agent, .. } if *agent == sub)
        ));
        assert_eq!(h.agent(&sub).report.as_ref().unwrap().status, status);
    }
    assert_eq!(child_reports(&h, &task).len(), 2);
}

#[test]
fn a_blocked_sub_agent_keeps_running() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-blocked", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.report(&sub, ReportStatus::Blocked).unwrap();
    h.stop(&sub, false);
    assert_eq!(h.agent(&sub).life, Life::Running);
    assert_eq!(child_reports(&h, &task).len(), 1);
}

#[test]
fn rounds_of_two_finished_sub_agents_do_not_exhaust_the_limits() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-rounds", 1);
    let task = h.start(&pid, &exec, 1);
    for round in 0..6 {
        for n in ["a", "b"] {
            let sub = start_sub(&mut h, &pid, &task, &format!("{n}{round}"));
            h.report(&sub, ReportStatus::Success).unwrap();
            h.stop(&sub, false);
        }
        // Two slots per round, none held by the finished ones.
        assert_eq!(
            live_processes(&h, &pid),
            2,
            "round {round}: executor + task"
        );
    }
}

#[test]
fn an_idle_task_agent_whose_sub_agents_finished_gets_the_stall_item() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-stall", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    // While a sub-agent works, the task agent is parked: no stall item.
    h.stop(&task, false);
    h.tick(30 * MIN);
    assert!(h.human(|k| *k == ItemKind::ExitedWithoutReport).is_empty());
    h.report(&sub, ReportStatus::Success).unwrap();
    h.stop(&sub, false);
    // The report woke the task agent; it ends its turn again and idles.
    h.prompt(&task);
    h.stop(&task, false);
    h.tick(11 * MIN);
    let stalls = h.human(|k| *k == ItemKind::ExitedWithoutReport);
    assert_eq!(stalls.len(), 1, "{stalls:?}");
    assert_eq!(stalls[0].from.as_ref(), Some(&task));
}

#[test]
fn a_landed_sub_agent_does_not_park_its_parent_even_when_blocked() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-park", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.report(&sub, ReportStatus::Blocked).unwrap();
    h.stop(&sub, false);
    // The first Stop is blocked by the pending report; the second idles.
    h.stop(&task, false);
    h.stop(&task, true);
    h.tick(11 * MIN);
    assert_eq!(h.human(|k| *k == ItemKind::ExitedWithoutReport).len(), 1);
}

#[test]
fn a_crashed_task_agent_takes_its_live_sub_agents_along() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-orphan", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.runner(RunnerEvent::Exited {
        agent: task.clone(),
        term: h.term_of(&task),
        code: Some(1),
    });
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
    assert_eq!(h.agent(&sub).life, Life::Stopped, "the orphan is stopped");
    // The executor retries; the new attempt gets its full allowance.
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    let task2 = h.up(&pid, 1);
    assert_eq!(h.task(&pid, 1).attempt, 2);
    start_sub(&mut h, &pid, &task2, "b");
    start_sub(&mut h, &pid, &task2, "c");
    let e = err_of(h.tool(
        &task2,
        ToolCall::SpawnSubagent {
            name: "d".into(),
            prompt: "p".into(),
        },
    ));
    assert_eq!(e.kind, ErrorKind::Limit);
}

#[test]
fn a_new_attempt_stops_the_previous_attempts_sub_agents() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-attempt", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    // The task agent's process died (exit code 1), then the task blocked and respawned.
    h.runner(RunnerEvent::Exited {
        agent: task.clone(),
        term: h.term_of(&task),
        code: Some(1),
    });
    // Even if the sub-agent had been left running, the new attempt stops it.
    h.st.plans[0].agents.iter_mut().for_each(|a| {
        if a.id == sub {
            a.life = Life::Running;
        }
    });
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    assert_eq!(h.agent(&sub).life, Life::Stopped);
}

#[test]
fn an_orphans_report_does_not_land_as_the_new_attempts() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-orphan-report", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    assert_eq!(h.agent(&sub).attempt, 1);
    h.runner(RunnerEvent::Exited {
        agent: task.clone(),
        term: h.term_of(&task),
        code: Some(1),
    });
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    let task2 = h.up(&pid, 1);
    // Force the orphan back to life, as a race between kill and report would.
    h.st.plans[0]
        .agents
        .iter_mut()
        .find(|a| a.id == sub)
        .unwrap()
        .life = Life::Running;
    // An orphan holds no slot of the new attempt.
    start_sub(&mut h, &pid, &task2, "b");
    start_sub(&mut h, &pid, &task2, "c");
    h.report(&sub, ReportStatus::Success).unwrap();
    h.stop(&sub, false);
    assert!(child_reports(&h, &task2).is_empty());
    let e = err_of(h.client(ClientAction::ResumeAgent { agent: sub.clone() }));
    assert!(e.to_string().contains("earlier attempt"), "{e}");
}

#[test]
fn merging_a_task_stops_its_sub_agents_but_not_its_task_agent() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-merge", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.tool(
        &exec,
        ToolCall::CheckCriterion {
            task: t(1),
            criterion: 1,
            checked: true,
        },
    )
    .unwrap();
    h.client(ClientAction::TransitionTask {
        plan: pid.clone(),
        task: t(1),
        to: TaskState::Merged,
    })
    .unwrap();
    assert_eq!(h.agent(&sub).life, Life::Stopped);
    assert_eq!(h.agent(&task).life, Life::Running);
}

#[test]
fn a_merged_pull_request_stops_the_sub_agents() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-pr-merge", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.tool(
        &task,
        ToolCall::SetTaskPr {
            url: "https://github.com/o/r/pull/7".into(),
        },
    )
    .unwrap();
    h.report(&task, ReportStatus::Success).unwrap();
    h.stop(&task, false);
    h.runner(RunnerEvent::Pr {
        plan: pid.clone(),
        task: t(1),
        state: PrGithubState {
            merged: true,
            closed: false,
            ci: CiConclusion::Success,
            head_sha: None,
        },
    });
    assert_eq!(h.task(&pid, 1).state, TaskState::Merged);
    assert_eq!(h.agent(&sub).life, Life::Stopped);
}

#[test]
fn finishing_the_plan_stops_every_sub_agent() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-done", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.tool(
        &exec,
        ToolCall::CheckCriterion {
            task: t(1),
            criterion: 1,
            checked: true,
        },
    )
    .unwrap();
    h.client(ClientAction::TransitionTask {
        plan: pid.clone(),
        task: t(1),
        to: TaskState::Merged,
    })
    .unwrap();
    // A sub-agent respawned after the merge (or one the merge missed).
    h.st.plans[0]
        .agents
        .iter_mut()
        .find(|a| a.id == sub)
        .unwrap()
        .life = Life::Running;
    h.tool(
        &exec,
        ToolCall::UpdatePlan {
            state: Some(PlanState::Done),
            title: None,
        },
    )
    .unwrap();
    assert_eq!(h.plan(&pid).state, PlanState::Done);
    assert_eq!(h.agent(&sub).life, Life::Stopped);
}

#[test]
fn a_late_exit_of_a_stopped_process_leaves_its_replacement_alone() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-late-exit", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    let old = h.term_of(&sub);
    h.tool(&task, ToolCall::StopAgent { agent: sub.clone() })
        .unwrap();
    start_sub(&mut h, &pid, &task, "a");
    assert_ne!(h.term_of(&sub), old);
    h.runner(RunnerEvent::Exited {
        agent: sub.clone(),
        term: old,
        code: None,
    });
    assert_eq!(h.agent(&sub).life, Life::Running);
    assert!(h.agent(&sub).report.is_none());
    let exited = h
        .items_to(&task)
        .into_iter()
        .filter(|i| i.kind == ItemKind::ChildExited)
        .count();
    assert_eq!(exited, 0);
    // The new process's own exit still counts.
    h.runner(RunnerEvent::Exited {
        agent: sub.clone(),
        term: h.term_of(&sub),
        code: Some(1),
    });
    assert_eq!(h.agent(&sub).life, Life::Exited { code: Some(1) });
}

fn child_questions(h: &H, to: &AgentId) -> Vec<(ItemKind, Delivery)> {
    h.items_to(to)
        .into_iter()
        .filter(|i| i.kind == ItemKind::ChildQuestion)
        .map(|i| (i.kind.clone(), i.status.clone()))
        .collect()
}

#[test]
fn a_finished_sub_agents_question_closes_and_no_expiry_is_sent() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-question", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.tool(
        &sub,
        ToolCall::AskParent {
            question: "which?".into(),
        },
    )
    .unwrap();
    // The parent answers with send instead of answer_child; the sub finishes.
    h.tool(
        &task,
        ToolCall::Send {
            agent: sub.clone(),
            text: "use x".into(),
            interrupt: false,
        },
    )
    .unwrap();
    h.report(&sub, ReportStatus::Success).unwrap();
    // The first Stop is blocked by the pending message; the second lands.
    h.stop(&sub, false);
    h.stop(&sub, true);
    assert_eq!(
        child_questions(&h, &task),
        vec![(ItemKind::ChildQuestion, Delivery::Dismissed)]
    );
    h.tick(61 * MIN);
    assert!(
        h.items_to(&sub).iter().all(|i| !i.body.contains("expired")),
        "no expiry message to a finished sub-agent"
    );
}

#[test]
fn stopping_or_losing_an_agent_closes_its_questions() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-question-stop", 1);
    let task = h.start(&pid, &exec, 1);
    let a = start_sub(&mut h, &pid, &task, "a");
    let b = start_sub(&mut h, &pid, &task, "b");
    for s in [&a, &b] {
        h.tool(
            s,
            ToolCall::AskParent {
                question: "which?".into(),
            },
        )
        .unwrap();
    }
    h.tool(&task, ToolCall::StopAgent { agent: a.clone() })
        .unwrap();
    h.runner(RunnerEvent::Exited {
        agent: b.clone(),
        term: h.term_of(&b),
        code: Some(1),
    });
    let qs = child_questions(&h, &task);
    assert_eq!(qs.len(), 2);
    assert!(qs.iter().all(|(_, s)| *s == Delivery::Dismissed), "{qs:?}");
    h.tick(61 * MIN);
    assert!(h.items_to(&a).iter().all(|i| !i.body.contains("expired")));
}

#[test]
fn an_expired_question_is_not_sent_to_a_dead_asker() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-question-dead", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    h.tool(
        &sub,
        ToolCall::AskParent {
            question: "which?".into(),
        },
    )
    .unwrap();
    // The asker dies without anything closing the question (old saved state).
    h.st.plans[0]
        .agents
        .iter_mut()
        .find(|a| a.id == sub)
        .unwrap()
        .life = Life::Stopped;
    h.tick(61 * MIN);
    assert!(h.items_to(&sub).iter().all(|i| !i.body.contains("expired")));
}

#[test]
fn a_task_agent_that_reported_still_hears_that_its_question_expired() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("task-reported-expiry", 1);
    h.stop(&exec, false);
    let task = h.start(&pid, &exec, 1);
    h.report(&task, ReportStatus::Success).unwrap();
    assert!(h.agent(&task).is_live());
    h.tool(
        &task,
        ToolCall::AskUser {
            question: "merge it?".into(),
        },
    )
    .unwrap();
    h.tick(61 * MIN);
    assert!(
        h.items_to(&task)
            .iter()
            .any(|i| i.body.contains("expired") && i.body.contains("merge it?")),
        "a task agent in its PR loop is told"
    );
}

#[test]
fn a_sub_agent_saved_without_an_attempt_is_current() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-legacy-attempt", 1);
    let task = h.start(&pid, &exec, 1);
    let sub = start_sub(&mut h, &pid, &task, "a");
    let mut v = serde_json::to_value(h.agent(&sub)).unwrap();
    v.as_object_mut().unwrap().remove("attempt");
    let legacy: Agent = serde_json::from_value(v).unwrap();
    assert_eq!(legacy.attempt, 0);
    *h.st.plans[0]
        .agents
        .iter_mut()
        .find(|a| a.id == sub)
        .unwrap() = legacy;
    assert!(h.agent(&sub).in_attempt(1));
    // Its report lands as the current attempt's, and it still counts.
    h.report(&sub, ReportStatus::Success).unwrap();
    h.stop(&sub, false);
    assert!(h.agent(&sub).landed);
}

#[test]
fn no_sub_agents_for_a_merged_or_cancelled_task() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("sub-finished-task", 1);
    let task = h.start(&pid, &exec, 1);
    for state in [TaskState::Merged, TaskState::Cancelled] {
        h.st.plans[0].tasks[0].state = state;
        let e = h
            .tool(
                &task,
                ToolCall::SpawnSubagent {
                    name: "a".into(),
                    prompt: "p".into(),
                },
            )
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Invalid, "{state:?}");
        assert!(e.to_string().contains("already finished"), "{e}");
    }
    assert!(h.plan(&pid).agent(&sub_id(&pid, 1, "a")).is_none());
}

#[test]
fn guard_rails_table() {
    use super::hooks::guard;
    // (level, command, denied)
    let table: &[(Level, &str, bool)] = &[
        // Sub-agents never push, however the word is dressed up.
        (Level::Sub, "git push", true),
        (Level::Sub, "git push origin HEAD", true),
        (Level::Sub, "/usr/bin/git push", true),
        (Level::Sub, "(git push)", true),
        (Level::Sub, "$(git push)", true),
        (Level::Sub, "x=$(git push 2>&1)", true),
        (Level::Sub, "`git push`", true),
        (Level::Sub, "{ git push; }", true),
        (Level::Sub, "\"git\" \"push\"", true),
        (Level::Sub, "git -C /tmp/x push", true),
        (Level::Sub, "git -c user.name=x push", true),
        (Level::Sub, "git --git-dir /a/.git push", true),
        (
            Level::Sub,
            "git --git-dir=/a/.git --work-tree=/a push",
            true,
        ),
        (Level::Sub, "env GIT_TRACE=1 git push", true),
        (Level::Sub, "sh -c 'git push'", true),
        (Level::Sub, "bash -lc \"cd x && git push origin\"", true),
        (Level::Sub, "git send-pack origin HEAD", true),
        (Level::Sub, "git fetch && git push", true),
        (Level::Sub, "git status; echo push", true),
        (Level::Sub, "git log\ngit push", true),
        // The task agent may push.
        (Level::Task, "git push -u origin claude/T-1", false),
        (Level::Task, "sh -c 'git push'", false),
        // Sub-agents may do everything else.
        (Level::Sub, "git status", false),
        (Level::Sub, "git commit -am wip", false),
        (Level::Sub, "git log --oneline", false),
        (Level::Sub, "cargo test && git diff", false),
        (Level::Sub, "push the button", false),
        (Level::Sub, "cd /repo/git && ls", false),
        // Commit messages are not parsed, so one that mentions push is a known,
        // accepted false positive of the conservative rule.
        (Level::Sub, "git commit -m 'fix push bug'", true),
        (Level::Sub, "git commit -m 'fix push bug' && git push", true),
        (Level::Sub, "git commit -m \"$(git push)\"", true),
        (
            Level::Sub,
            "git commit -m \"$(cat <<'EOF'\nAdd push notifications\nEOF\n)\"",
            true,
        ),
        (Level::Sub, "git commit -F - <<'EOF'\nfix push\nEOF", true),
        (Level::Sub, "env A=1 git commit -m 'fix push'", true),
        // Messages that do not mention push are fine.
        (Level::Sub, "git commit -m 'fix the bug'", false),
        (
            Level::Sub,
            "git add -A && git commit -m \"$(cat <<'EOF'\nAdd notifications\n\nMore detail\nEOF\n)\"",
            false,
        ),
        (Level::Sub, "git commit -F - <<'EOF'\nfix it\nEOF", false),
        (Level::Sub, "git commit -m 'line one\nline two'", false),
        (Level::Sub, "git tag -a v1 -m 'a\nb'", false),
        // Commands chained or substituted around a message are still scanned.
        (Level::Sub, "git commit -m 'x' && git push", true),
        (Level::Sub, "git commit -m x; git push", true),
        (Level::Sub, "git tag x && git push --tags", true),
        (Level::Sub, "git commit -m \"a $(git push)\"", true),
        (Level::Sub, "git commit -m \"a `git push`\"", true),
        (
            Level::Sub,
            "git commit -m \"$(git log -1 --format=\"%s\"; git push)\"",
            true,
        ),
        (
            Level::Sub,
            "git commit -m \"`echo \"a\"; git push; echo \"b\"`\"",
            true,
        ),
        (
            Level::Sub,
            "git commit -m \"$(sh - <<'EOF'\ngit push\nEOF\n)\"",
            true,
        ),
        (
            Level::Sub,
            "git commit -m \"cat <<'EOF'\"\ngit push\nEOF",
            true,
        ),
        (
            Level::Sub,
            "git commit -F - <<'EOF' \"\n$(git push)\nEOF\n\"",
            true,
        ),
        (
            Level::Sub,
            "git commit -m \"$(sh <<EOF\ngit push\nEOF\n)\"",
            true,
        ),
        (Level::Sub, "sh <<'EOF'\ngit push\nEOF", true),
        (Level::Sub, "bash -c 'git status\ngit push'", true),
        (Level::Sub, "git status; echo 'now push it'", true),
        (
            Level::Sub,
            "git commit -m \"$(cat <<EOF\n$(git push)\nEOF\n)\"",
            true,
        ),
        (Level::Sub, "git commit -F - <<EOF\n`git push`\nEOF", true),
        (
            Level::Sub,
            "git commit -m $'Fix: don\\'t crash' && git push origin 'feat'",
            true,
        ),
        (
            Level::Sub,
            "git commit -m fix # can't amend\ngit push origin 'feat'",
            true,
        ),
        (
            Level::Sub,
            "find . -name git -name commit -exec sh -c 'git push' +",
            true,
        ),
        (
            Level::Task,
            "git commit -m x #'\ngit checkout main\n#'",
            true,
        ),
        (
            Level::Task,
            "git commit -m \"$(echo \"a\"; git checkout main)\"",
            true,
        ),
        // A known false positive of the conservative rule.
        (Level::Sub, "echo git; echo hi push", true),
        // Base branch: every spelling, both levels.
        (Level::Task, "git checkout main", true),
        (Level::Sub, "git checkout main", true),
        (Level::Task, "git switch main", true),
        (Level::Task, "git checkout origin/main", true),
        (Level::Task, "git checkout refs/heads/main", true),
        (Level::Task, "git checkout heads/main", true),
        (Level::Task, "git switch origin/main", true),
        (Level::Task, "git switch -d main", true),
        (Level::Task, "git switch --detach origin/main", true),
        (Level::Task, "git checkout --detach main", true),
        (Level::Task, "git checkout -f main", true),
        (Level::Task, "git checkout main --", true),
        (Level::Task, "git checkout 'main'", true),
        (Level::Task, "git checkout \"main\"", true),
        (Level::Task, "/usr/bin/git checkout main", true),
        (Level::Task, "git -C /wt checkout main", true),
        (Level::Task, "git --git-dir /wt/.git checkout main", true),
        (Level::Task, "git --work-tree /wt checkout main", true),
        (Level::Task, "git --namespace x checkout main", true),
        (Level::Task, "git --exec-path /x checkout main", true),
        (Level::Task, "git -c advice.x=1 checkout main", true),
        (Level::Task, "(git checkout main)", true),
        (Level::Task, "$(git checkout main)", true),
        (Level::Task, "`git checkout main`", true),
        (Level::Task, "sh -c 'git checkout main'", true),
        (Level::Task, "bash -c \"cd x; git switch main\"", true),
        (Level::Task, "git fetch && git checkout main", true),
        (Level::Task, "git checkout main^", true),
        // Not the base branch.
        (Level::Task, "git checkout claude/T-1", false),
        (Level::Task, "git checkout -b feat main", false),
        (Level::Task, "git checkout -qb feat main", false),
        (Level::Task, "git checkout -qBmain", true),
        (Level::Task, "git switch -qCmain", true),
        (Level::Task, "git switch -- main", true),
        (Level::Task, "git switch -d -- main", true),
        (Level::Task, "git switch --detach -- main", true),
        // Creating (or resetting) a branch named like the base is a switch to it.
        (Level::Task, "git checkout -B main", true),
        (Level::Task, "git checkout -B main origin/main", true),
        (Level::Task, "git checkout -b main", true),
        (Level::Task, "git checkout -bmain", true),
        (Level::Task, "git checkout -b origin/main", true),
        (Level::Task, "git switch -C main origin/main", true),
        (Level::Task, "git switch -c main", true),
        (Level::Task, "git switch --create main", true),
        (
            Level::Task,
            "git switch --force-create=main origin/main",
            true,
        ),
        (Level::Task, "git checkout --orphan main", true),
        (Level::Task, "git checkout --orphan x main", false),
        (Level::Task, "git checkout -B feat main", false),
        (Level::Task, "git switch -C feat origin/main", false),
        // Messages are not parsed: a line naming the base in one is denied.
        (Level::Task, "git commit -m 'a\ngit checkout main'", true),
        (Level::Task, "git switch -c feat origin/main", false),
        (Level::Task, "git checkout main -- src/a.rs", false),
        (
            Level::Task,
            "git checkout origin/main -- src/a.rs src/b.rs",
            false,
        ),
        (Level::Task, "git checkout -- main", false),
        (Level::Task, "git checkout mainline", false),
        (Level::Task, "git checkout feature/main", false),
        (Level::Task, "git checkout -b main-fix", false),
        (Level::Task, "git commit -m 'checkout main'", false),
        (Level::Task, "git log main", false),
        (Level::Task, "git merge main", false),
        (Level::Task, "echo checkout main", false),
        // The executor is not guarded.
        (Level::Executor, "git checkout main", false),
        (Level::Executor, "git push", false),
    ];
    for (level, cmd, denied) in table {
        let got = guard(*level, Some("main"), "Bash", cmd);
        assert_eq!(got.is_some(), *denied, "{level:?} {cmd:?} -> {got:?}");
    }
    // Other tools are never guarded.
    assert!(guard(Level::Sub, Some("main"), "Edit", "git push").is_none());
}
#[test]
fn delete_plan_stops_its_agents_and_forgets_it() {
    let mut h = H::new();
    let (keep, _) = h.new_plan("keep");
    let (pid, exec) = h.accepted_plan("doomed", 2);
    let a = h.start(&pid, &exec, 1);
    h.report(&a, ReportStatus::Success).unwrap();
    let term = h.agent(&a).term.expect("the task agent has a terminal");
    assert!(
        !h.items_to(&exec).is_empty(),
        "the executor holds the report"
    );

    let o = h
        .client(ClientAction::DeletePlan { plan: pid.clone() })
        .unwrap();
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::KillAgent { agent, term: Some(_) } if *agent == a)
    ));
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::DeletePlan { plan } if *plan == pid)
    ));
    assert!(!has_fx(
        &o,
        |e| matches!(e, Effect::Persist { plan } if *plan == pid)
    ));
    assert!(
        o.deltas
            .iter()
            .any(|d| d.change == Change::Removed(ObjectRef::Plan(pid.clone())))
    );
    assert_eq!(h.st.plans.len(), 1);
    assert_eq!(h.st.plans[0].id, keep);
    assert!(!h.st.terminals.contains_key(&term));
    let ours = |x: &AgentId| x.plan() == &pid;
    assert!(
        h.st.inbox.iter().all(|i| !i.from.as_ref().is_some_and(ours)
            && !matches!(&i.to, Recipient::Agent(x) if ours(x)))
    );
    assert_eq!(
        h.client(ClientAction::DeletePlan { plan: pid.clone() })
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
}

#[test]
fn a_spawn_that_lands_after_its_plan_is_deleted_is_killed() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("late-spawn", 1);
    h.client(ClientAction::DeletePlan { plan: pid.clone() })
        .unwrap();
    let o = h.runner(RunnerEvent::Spawned {
        agent: exec.clone(),
        term: TermId(77),
        session_id: "s".into(),
        token_hash: tok(&exec),
    });
    assert!(has_fx(&o, |e| matches!(
        e,
        Effect::KillAgent { agent, term: Some(TermId(77)) } if *agent == exec
    )));
}

#[test]
fn a_plan_written_by_the_client_starts_its_executor_on_accept() {
    let mut h = H::new();
    let o = h
        .client(ClientAction::CreatePlan {
            title: "from a session".into(),
            repos: vec![Repo {
                path: PathBuf::from("/repo"),
                base_branch: "main".into(),
                remote_url: None,
            }],
            request: None,
        })
        .unwrap();
    assert!(!has_fx(&o, |e| matches!(e, Effect::SpawnAgent { .. })));
    let pid: PlanId = text_of(&o).parse().unwrap();
    assert!(h.plan(&pid).agents.is_empty());
    assert_eq!(h.plan(&pid).state, PlanState::Draft);

    let e = h
        .client(ClientAction::AcceptPlan { plan: pid.clone() })
        .unwrap_err();
    assert!(e.to_string().contains("no tasks"), "{e}");

    h.client(ClientAction::CreateTask {
        plan: pid.clone(),
        title: "one".into(),
        body: "do it".into(),
        repo: 0,
        deps: vec![],
        criteria: vec!["tests pass".into()],
    })
    .unwrap();
    let o = h
        .client(ClientAction::AcceptPlan { plan: pid.clone() })
        .unwrap();
    let exec = AgentId::new(pid.clone(), AgentPath::Executor);
    assert!(has_fx(&o, |e| matches!(
        e,
        Effect::SpawnAgent { agent, resume: false, kickoff, .. }
            if *agent == exec && kickoff.contains("T-0001")
    )));
    assert_eq!(h.plan(&pid).state, PlanState::Accepted);
    assert_eq!(h.agent(&exec).life, Life::Starting);
}

// ---------------------------------------------------------------------------
// Recovery after a server restart, and the stall watchdog
// ---------------------------------------------------------------------------

fn spawns(o: &Outcome) -> Vec<(AgentId, bool, String)> {
    o.effects
        .iter()
        .filter_map(|e| match e {
            Effect::SpawnAgent {
                agent,
                resume,
                kickoff,
                ..
            } => Some((agent.clone(), *resume, kickoff.clone())),
            _ => None,
        })
        .collect()
}

/// An idle executor, a working task agent T-0001 with a working sub-agent,
/// and an idle task agent T-0002 with one confirmed and one unconfirmed paste.
fn before_restart() -> (H, PlanId, [AgentId; 4], [ItemId; 2]) {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("restart", 2);
    let a1 = h.start(&pid, &exec, 1);
    let a2 = h.start(&pid, &exec, 2);
    h.stop(&exec, false);
    h.tool(
        &a1,
        ToolCall::SpawnSubagent {
            name: "tests".into(),
            prompt: "write tests".into(),
        },
    )
    .unwrap();
    let sub = AgentId::new(pid.clone(), AgentPath::sub(t(1), "tests").unwrap());
    h.boot(&sub);
    h.prompt(&sub);
    h.stop(&a2, false);
    let paste = |h: &mut H, text: &str| {
        h.client(ClientAction::SendMessage {
            agent: a2.clone(),
            text: text.into(),
            steer: false,
        })
        .unwrap();
        let id = h.items_to(&a2).last().unwrap().id;
        assert!(matches!(h.status(id), Delivery::Delivered { .. }));
        id
    };
    let confirmed = paste(&mut h, "first");
    h.hook(
        &a2,
        Hook::UserPromptSubmit {
            prompt: format!("[{confirmed}] first"),
        },
    )
    .unwrap();
    h.stop(&a2, false);
    let unconfirmed = paste(&mut h, "second");
    assert_eq!(h.agent(&a2).unconfirmed, vec![unconfirmed]);
    assert_eq!(h.agent(&a1).activity, Activity::Working);
    assert_eq!(h.agent(&exec).activity, Activity::Idle);
    (h, pid, [exec, a1, a2, sub], [confirmed, unconfirmed])
}

#[test]
fn recover_resumes_only_working_agents_and_never_sub_agents() {
    let (mut h, pid, [exec, a1, a2, sub], _) = before_restart();
    let o = h.ok(Command::Recover { auto_resume: true });
    assert_eq!(
        spawns(&o),
        vec![(a1.clone(), true, spawn::RESTART_KICKOFF.to_string())],
        "only the working task agent resumes"
    );
    assert_eq!(h.agent(&a1).life, Life::Starting);
    assert_eq!(h.agent(&a1).session_id, format!("sess-{a1}"));
    for idle in [&exec, &a2, &sub] {
        assert_eq!(h.agent(idle).life, Life::Exited { code: None }, "{idle}");
    }
    // The sub-agent's task agent hears it failed; it rides on the resume.
    let reports: Vec<_> = h
        .items_to(&a1)
        .into_iter()
        .filter(|i| i.kind == ItemKind::ChildReport && i.from.as_ref() == Some(&sub))
        .collect();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].is_pending());
    assert!(reports[0].body.contains("Failed"), "{}", reports[0].body);
    assert!(
        reports[0].body.contains("server_restart"),
        "{}",
        reports[0].body
    );
    assert_eq!(h.task(&pid, 1).state, TaskState::InProgress);
    assert_eq!(h.task(&pid, 2).state, TaskState::InProgress);
    // Recover finds nothing to do a second time.
    let o = h.ok(Command::Recover { auto_resume: true });
    assert!(spawns(&o).is_empty());
}

#[test]
fn recover_without_auto_resume_starts_nothing() {
    let (mut h, _, agents, _) = before_restart();
    let o = h.ok(Command::Recover { auto_resume: false });
    assert!(spawns(&o).is_empty());
    for a in &agents {
        assert!(!h.agent(a).is_live(), "{a}");
    }
}

#[test]
fn recover_resets_only_unconfirmed_deliveries() {
    let (mut h, _, [_, _, a2, _], [confirmed, unconfirmed]) = before_restart();
    h.ok(Command::Recover { auto_resume: true });
    assert!(matches!(h.status(confirmed), Delivery::Delivered { .. }));
    assert_eq!(h.status(unconfirmed), Delivery::Pending);
    assert!(h.agent(&a2).unconfirmed.is_empty());
    // A resume delivers it again, by id, with the kickoff (path C).
    h.client(ClientAction::ResumeAgent { agent: a2.clone() })
        .unwrap();
    h.boot(&a2);
    let o = h.prompt(&a2);
    match hook_reply(&o) {
        HookReply::AdditionalContext(text) => {
            assert!(text.contains(&unconfirmed.to_string()), "{text}");
            assert!(!text.contains(&format!("[{confirmed}]")), "{text}");
        }
        r => panic!("expected the digest, got {r:?}"),
    }
}

#[test]
fn recover_ends_an_agent_that_never_started() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("restart", 1);
    h.tool(
        &exec,
        ToolCall::SpawnTask {
            task: t(1),
            instructions: None,
            force: false,
        },
    )
    .unwrap();
    let a = H::task_agent(&pid, 1);
    assert_eq!(h.agent(&a).life, Life::Starting);
    let o = h.ok(Command::Recover { auto_resume: true });
    // The executor was working, so it resumes; the task never started.
    assert_eq!(spawns(&o).len(), 1);
    assert_eq!(spawns(&o)[0].0, exec);
    assert_eq!(h.agent(&a).life, Life::Exited { code: None });
    assert_eq!(h.task(&pid, 1).state, TaskState::Blocked);
}

fn stalled(h: &H) -> Vec<&InboxItem> {
    h.human(|k| *k == ItemKind::Stalled)
}

#[test]
fn a_working_agent_with_no_progress_for_fifteen_minutes_stalls() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("stall", 1);
    let a = h.start(&pid, &exec, 1);
    h.stop(&exec, false);
    let term = h.agent(&a).term.unwrap();
    // Redraws without new screen content never reach the engine as
    // `Progress`; size and title changes are not progress either.
    h.tick(10 * MIN);
    h.runner(RunnerEvent::TermSize {
        term,
        rows: 30,
        cols: 100,
        owner: None,
    });
    h.runner(RunnerEvent::TermTitle {
        term,
        title: Some("✳ Working".into()),
    });
    h.tick(5 * MIN - 1);
    assert!(stalled(&h).is_empty());
    let o = h.tick(1);
    assert_eq!(stalled(&h).len(), 1);
    let item = stalled(&h)[0].id;
    assert!(h.agent(&a).stalled);
    assert!(has_fx(
        &o,
        |e| matches!(e, Effect::Notify { item: i } if *i == item)
    ));
    assert!(h.tick(MIN).deltas.is_empty(), "one item per stall");
    assert_eq!(h.agent(&a).life, Life::Running, "nothing is killed");

    // A hook is progress: it clears the flag and the item.
    h.hook(
        &a,
        Hook::PreToolUse {
            tool: "Bash".into(),
            input_summary: "ls".into(),
            match_input: "ls".into(),
        },
    )
    .unwrap();
    assert!(!h.agent(&a).stalled);
    assert_eq!(h.status(item), Delivery::Dismissed);
    h.tick(15 * MIN - 1);
    assert_eq!(stalled(&h).len(), 1);
    h.tick(1);
    assert_eq!(stalled(&h).len(), 2);
    assert!(h.agent(&a).stalled);

    // A changed screen is progress too.
    h.runner(RunnerEvent::Progress { term });
    assert!(!h.agent(&a).stalled);
    assert!(stalled(&h).iter().all(|i| !i.is_pending()));

    // An idle agent never stalls.
    h.stop(&a, false);
    h.tick(60 * MIN);
    assert_eq!(stalled(&h).len(), 2);
}

#[test]
fn progress_is_kept_to_thirty_seconds() {
    let mut h = H::new();
    let (pid, exec) = h.accepted_plan("grain", 1);
    let a = h.start(&pid, &exec, 1);
    let term = h.agent(&a).term.unwrap();
    let at = h.agent(&a).last_progress.unwrap();
    h.now += 10_000;
    assert!(h.runner(RunnerEvent::Progress { term }).deltas.is_empty());
    assert_eq!(h.agent(&a).last_progress, Some(at));
    h.now += 20_000;
    assert!(!h.runner(RunnerEvent::Progress { term }).deltas.is_empty());
    assert_eq!(h.agent(&a).last_progress, Some(at + 30_000));
    // A terminal of no agent is ignored.
    assert!(
        h.runner(RunnerEvent::Progress { term: TermId(999) })
            .deltas
            .is_empty()
    );
}
