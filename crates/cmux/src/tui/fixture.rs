//! A fixture `StateView` for the draw, navigation and hit-test tests: one
//! accepted plan with six tasks in varied states, two more plans, a pending
//! permission and a question.

use cmux_core::ids::AgentPath;
use cmux_proto::view::{
    Activity, AgentId, AgentView, CiMark, Delivery, ItemId, ItemKind, ItemView, Life, Limits,
    PlanId, PlanState, PlanView, PrRef, Recipient, Repo, Report, ReportStatus, StateView, TaskId,
    TaskState, TaskView, TermId, TermKind, TerminalView, TokenUsage,
};

use super::state::{Node, ViewState};

pub fn plan_id() -> PlanId {
    PlanId::new(2026, 10, 18, "auth-refactor")
}

fn aid(plan: &PlanId, path: AgentPath) -> AgentId {
    AgentId::new(plan.clone(), path)
}

fn agent(plan: &PlanId, path: AgentPath, life: Life, activity: Activity) -> AgentView {
    AgentView {
        id: aid(plan, path.clone()),
        level: path.level(),
        parent: path.parent().map(|p| aid(plan, p)),
        task: path.task_id(),
        term: None,
        cwd: "/home/me/api".into(),
        life,
        activity,
        report: None,
        tokens: TokenUsage {
            input: 60_000,
            output: 20_000,
            ..TokenUsage::default()
        },
        idle_since: None,
        stalled: false,
        path,
    }
}

fn success() -> Report {
    Report {
        status: ReportStatus::Success,
        summary: "done".into(),
        pr_url: None,
        attempt: 1,
        implicit: false,
        supersedes_blocked: false,
    }
}

fn task(plan: &PlanId, n: u32, title: &str, state: TaskState, deps: &[u32]) -> TaskView {
    let id = TaskId::new(n);
    TaskView {
        plan: plan.clone(),
        id,
        title: title.into(),
        state,
        repo: 0,
        deps: deps.iter().map(|d| TaskId::new(*d)).collect(),
        criteria_done: 0,
        criteria_total: 0,
        notes: 0,
        branch: None,
        worktree: None,
        pr: None,
        agent: None,
        attempt: 1,
    }
}

fn plan_view(id: PlanId, title: &str, state: PlanState, tasks: Vec<TaskId>) -> PlanView {
    PlanView {
        agents: vec![],
        id,
        title: title.into(),
        state,
        repos: vec![Repo {
            path: "/home/me/api".into(),
            base_branch: "main".into(),
            remote_url: None,
        }],
        paused: false,
        limits: Limits::default(),
        tasks,
        queue: vec![],
    }
}

pub fn state() -> StateView {
    let plan = plan_id();
    let docs_site = PlanId::new(2026, 10, 17, "docs-site");
    let ci = PlanId::new(2026, 10, 12, "ci-migration");

    let exec = agent(&plan, AgentPath::Executor, Life::Running, Activity::Idle);
    let mut a1 = agent(
        &plan,
        AgentPath::Task(TaskId::new(1)),
        Life::Exited { code: Some(0) },
        Activity::Idle,
    );
    a1.report = Some(success());
    let a2 = agent(
        &plan,
        AgentPath::Task(TaskId::new(2)),
        Life::Running,
        Activity::Working,
    );
    let sub_tests = agent(
        &plan,
        AgentPath::sub(TaskId::new(2), "tests").unwrap(),
        Life::Running,
        Activity::Working,
    );
    let mut sub_docs = agent(
        &plan,
        AgentPath::sub(TaskId::new(2), "docs").unwrap(),
        Life::Running,
        Activity::Idle,
    );
    sub_docs.report = Some(success());
    let a4 = agent(
        &plan,
        AgentPath::Task(TaskId::new(4)),
        Life::Running,
        Activity::NeedsPermission,
    );

    let mut t1 = task(&plan, 1, "schema", TaskState::Merged, &[]);
    t1.agent = Some(a1.id.clone());
    t1.pr = Some(PrRef::new("https://example.com/38", 38));
    t1.pr.as_mut().unwrap().ci = Some(CiMark::Passing);
    t1.criteria_done = 4;
    t1.criteria_total = 4;
    let mut t2 = task(&plan, 2, "api", TaskState::Testing, &[1]);
    t2.agent = Some(a2.id.clone());
    t2.pr = Some(PrRef::new("https://example.com/41", 41));
    t2.pr.as_mut().unwrap().ci = Some(CiMark::Pending);
    t2.criteria_done = 2;
    t2.criteria_total = 5;
    let t3 = task(&plan, 3, "ui", TaskState::Todo, &[2]);
    let mut t4 = task(&plan, 4, "perms", TaskState::InProgress, &[]);
    t4.agent = Some(a4.id.clone());
    t4.criteria_done = 1;
    t4.criteria_total = 2;
    let t5 = task(&plan, 5, "rollout", TaskState::Todo, &[3]);
    let mut t6 = task(&plan, 6, "metrics dashboard", TaskState::Passing, &[]);
    t6.pr = Some(PrRef::new("https://example.com/43", 43));
    t6.pr.as_mut().unwrap().ci = Some(CiMark::Passing);
    t6.criteria_done = 3;
    t6.criteria_total = 3;

    let mut main = plan_view(
        plan.clone(),
        "auth-refactor",
        PlanState::Accepted,
        (1..=6).map(TaskId::new).collect(),
    );
    main.agents = [&exec, &a1, &a2, &sub_tests, &sub_docs, &a4]
        .iter()
        .map(|a| a.id.clone())
        .collect();

    let item = |id, kind, from: &AgentId, body: &str, created| ItemView {
        id: ItemId(id),
        to: Recipient::Human,
        needs_human: kind != ItemKind::UserMessage,
        kind,
        from: Some(from.clone()),
        body: body.into(),
        status: Delivery::Pending,
        created,
    };
    let mut items = vec![
        item(
            1,
            ItemKind::Permission {
                tool: "Bash".into(),
                input_summary: "cargo test -p api".into(),
            },
            &a4.id,
            "",
            100,
        ),
        item(
            2,
            ItemKind::Question,
            &exec.id,
            "Which database should T-0005 migrate first, users or sessions?",
            200,
        ),
    ];
    // A user message waiting for T-0002 (the ✉1 mark); not a needs-you item.
    items.push(ItemView {
        id: ItemId(3),
        to: Recipient::Agent(a2.id.clone()),
        kind: ItemKind::UserMessage,
        from: None,
        body: "use sqlx".into(),
        status: Delivery::Pending,
        created: 300,
        needs_human: false,
    });

    StateView {
        rev: 7,
        plans: vec![
            main,
            plan_view(docs_site, "docs-site", PlanState::Draft, vec![]),
            plan_view(ci, "ci-migration", PlanState::Done, vec![]),
        ],
        tasks: vec![t1, t2, t3, t4, t5, t6],
        agents: vec![exec, a1, a2, sub_tests, sub_docs, a4],
        terminals: ["zsh", "lazygit"]
            .iter()
            .enumerate()
            .map(|(i, name)| TerminalView {
                id: TermId(i as u32 + 1),
                kind: TermKind::Command((*name).into()),
                agent: None,
                cwd: "/home/me".into(),
                rows: 24,
                cols: 80,
                size_owner: None,
                title: None,
            })
            .collect(),
        loose: vec![TermId(1), TermId(2)],
        items,
    }
}

/// Finished agents seen, the other plans collapsed, T-0002 selected.
pub fn view_state() -> ViewState {
    let plan = plan_id();
    let mut vs = ViewState::default();
    vs.collapsed
        .insert(Node::Plan(PlanId::new(2026, 10, 17, "docs-site")));
    vs.collapsed
        .insert(Node::Plan(PlanId::new(2026, 10, 12, "ci-migration")));
    vs.seen.insert(aid(&plan, AgentPath::Task(TaskId::new(1))));
    vs.seen
        .insert(aid(&plan, AgentPath::sub(TaskId::new(2), "docs").unwrap()));
    vs.selection = Some(Node::Task(plan, TaskId::new(2)));
    vs
}
