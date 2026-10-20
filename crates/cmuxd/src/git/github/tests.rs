use std::collections::HashMap;
use std::sync::Mutex;

use cmux_core::ids::AgentId;
use cmux_core::model::{Agent, CiConclusion, CiMark, Plan, PrRef, Repo, Task, pr_to_task_state};

use super::*;
use crate::actor::{self, ActorOptions, CompositeRunner};

// ---------------------------------------------------------------------------
// Mapping, on `gh pr view` output recorded from this repository's PRs (merged,
// no checks) and edited copies of it (the rest).
// ---------------------------------------------------------------------------

const MERGED: &str = include_str!("fixtures/merged.json");
const CLOSED: &str = include_str!("fixtures/closed.json");
const FAILING: &str = include_str!("fixtures/failing.json");
const PENDING: &str = include_str!("fixtures/pending.json");
const NO_CHECKS: &str = include_str!("fixtures/no_checks.json");
const REVIEW_REQUESTED: &str = include_str!("fixtures/review_requested.json");

fn gh_state(merged: bool, closed: bool, ci: CiConclusion) -> PrGithubState {
    PrGithubState {
        merged,
        closed,
        ci,
        head_sha: None,
    }
}

#[test]
fn recorded_gh_output_maps_to_task_states() {
    use CiConclusion::*;
    let cases = [
        (
            "merged",
            MERGED,
            gh_state(true, false, Success),
            TaskState::Merged,
        ),
        (
            "closed",
            CLOSED,
            gh_state(false, true, Success),
            TaskState::Blocked,
        ),
        (
            "failing",
            FAILING,
            gh_state(false, false, Failure),
            TaskState::Failing,
        ),
        (
            "pending",
            PENDING,
            gh_state(false, false, Pending),
            TaskState::Testing,
        ),
        (
            "no checks",
            NO_CHECKS,
            gh_state(false, false, None),
            TaskState::Testing,
        ),
        // A review decision does not move the task; green CI does.
        (
            "review requested",
            REVIEW_REQUESTED,
            gh_state(false, false, Success),
            TaskState::Passing,
        ),
    ];
    for (name, json, want, task_state) in cases {
        let got = parse_pr_view(json).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(got, want, "{name}");
        assert_eq!(pr_to_task_state(&got), task_state, "{name}");
    }
    // The CI mark each one shows.
    let mark = |j| CiMark::from_conclusion(parse_pr_view(j).unwrap().ci);
    assert_eq!(mark(FAILING), Some(CiMark::Failing));
    assert_eq!(mark(PENDING), Some(CiMark::Pending));
    assert_eq!(mark(REVIEW_REQUESTED), Some(CiMark::Passing));
    assert_eq!(mark(NO_CHECKS), Option::None);
}

#[test]
fn merged_at_alone_means_merged_and_bad_json_is_an_error() {
    let j = r#"{"state":"OPEN","mergedAt":"2026-10-18T16:22:49Z","statusCheckRollup":null}"#;
    assert!(parse_pr_view(j).unwrap().merged);
    assert!(
        parse_pr_view("not json")
            .unwrap_err()
            .contains("bad gh JSON")
    );
}

#[test]
fn a_missing_gh_binary_is_a_reason_not_a_panic() {
    let gh = GhCli {
        bin: "/nonexistent/cmux-test/gh".into(),
    };
    let why = gh.auth_status().unwrap_err();
    assert!(why.contains("not found"), "{why}");
    assert!(gh.pr_view("https://github.com/o/r/pull/1").is_err());
}

// ---------------------------------------------------------------------------
// Cadence and staggering: a fake clock (`now` in ms) and a fake `gh`.
// ---------------------------------------------------------------------------

/// Answers per URL in order; the last answer repeats. Records every call.
#[derive(Default)]
struct FakeGh {
    answers: Mutex<HashMap<String, Vec<Result<String, String>>>>,
    calls: Mutex<Vec<String>>,
    auth: Option<String>,
}

impl FakeGh {
    fn answer(self, url: &str, seq: &[&str]) -> Self {
        let seq = seq.iter().map(|s| Ok(s.to_string())).collect();
        self.answers.lock().unwrap().insert(url.into(), seq);
        self
    }
}

impl Gh for FakeGh {
    fn auth_status(&self) -> Result<(), String> {
        self.auth.clone().map_or(Ok(()), Err)
    }

    fn pr_view(&self, url: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push(url.into());
        let mut all = self.answers.lock().unwrap();
        let seq = all.get_mut(url).ok_or("no such PR")?;
        if seq.len() > 1 {
            seq.remove(0)
        } else {
            seq[0].clone()
        }
    }
}

fn pr(task: u32, fast: bool) -> Watched {
    Watched {
        plan: PlanId::new(2026, 10, 18, "gh"),
        task: TaskId::new(task),
        url: format!("https://github.com/o/r/pull/{task}"),
        fast,
    }
}

/// Drives a [`Schedule`] second by second against a [`FakeGh`]; each call
/// takes `latency` ms.
struct Sim {
    schedule: Schedule,
    gh: FakeGh,
    latency: u64,
    flying: Vec<(u64, Watched, Result<PrGithubState, String>)>,
    /// `(start ms, task)` per `gh` call.
    starts: Vec<(u64, u32)>,
    /// `(ms, task, state)` per result sent to the engine.
    sent: Vec<(u64, u32, PrGithubState)>,
    most_in_flight: usize,
}

impl Sim {
    fn new(cadence: Cadence, gh: FakeGh, latency: u64) -> Sim {
        Sim {
            schedule: Schedule::new(cadence),
            gh,
            latency,
            flying: Vec::new(),
            starts: Vec::new(),
            sent: Vec::new(),
            most_in_flight: 0,
        }
    }

    fn run(&mut self, from: u64, to: u64, watched: &[Watched]) {
        for now in (from..=to).step_by(1000) {
            let (done, flying) = std::mem::take(&mut self.flying)
                .into_iter()
                .partition(|(at, ..)| *at <= now);
            self.flying = flying;
            for (_, w, result) in done {
                if let Some(s) = self.schedule.finished(&w, now, result) {
                    self.sent.push((now, w.task.seq(), s));
                }
            }
            for w in self.schedule.due(now, watched) {
                self.starts.push((now, w.task.seq()));
                let result = self.gh.pr_view(&w.url).and_then(|j| parse_pr_view(&j));
                self.flying.push((now + self.latency, w, result));
            }
            self.most_in_flight = self.most_in_flight.max(self.flying.len());
        }
    }

    fn starts_of(&self, task: u32) -> Vec<u64> {
        let s = self.starts.iter().filter(|(_, t)| *t == task);
        s.map(|(at, _)| at / 1000).collect()
    }
}

fn fake_for(tasks: &[u32], json: &str) -> FakeGh {
    tasks.iter().fold(FakeGh::default(), |gh, t| {
        gh.answer(&format!("https://github.com/o/r/pull/{t}"), &[json])
    })
}

#[test]
fn open_prs_every_60_s_and_idle_testing_every_15_s() {
    let mut sim = Sim::new(Cadence::default(), fake_for(&[1, 2], PENDING), 1000);
    sim.run(0, 130_000, &[pr(1, false), pr(2, true)]);
    // Seconds; each next look is a cadence after the last call returned.
    assert_eq!(sim.starts_of(1), [0, 61, 122]);
    assert_eq!(sim.starts_of(2), [2, 18, 34, 50, 66, 82, 98, 114, 130]);
    // The agent got busy again: back to 60 s.
    let mut sim = Sim::new(Cadence::default(), fake_for(&[2], PENDING), 1000);
    sim.run(0, 20_000, &[pr(2, true)]);
    sim.run(21_000, 100_000, &[pr(2, false)]);
    assert_eq!(sim.starts_of(2), [0, 16, 77]);
}

#[test]
fn calls_are_staggered_and_at_most_two_run_at_once() {
    let tasks = [1, 2, 3, 4, 5];
    let mut sim = Sim::new(Cadence::default(), fake_for(&tasks, PENDING), 5000);
    let watched: Vec<Watched> = tasks.iter().map(|t| pr(*t, false)).collect();
    sim.run(0, 30_000, &watched);
    let firsts: Vec<(u64, u32)> = sim.starts.iter().map(|(at, t)| (at / 1000, *t)).collect();
    // 2 s apart, and a third waits for a free slot.
    assert_eq!(firsts, [(0, 1), (2, 2), (5, 3), (7, 4), (10, 5)]);
    assert_eq!(sim.most_in_flight, 2);
    for pair in sim.starts.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= 2000, "{pair:?}");
    }
}

#[test]
fn only_a_changed_result_is_sent() {
    let url = "https://github.com/o/r/pull/1";
    let gh = FakeGh::default().answer(url, &[PENDING, PENDING, FAILING, FAILING, MERGED]);
    let cadence = Cadence {
        open_ms: 10_000,
        ..Cadence::default()
    };
    let mut sim = Sim::new(cadence, gh, 1000);
    sim.run(0, 60_000, &[pr(1, false)]);
    assert_eq!(sim.starts_of(1).len(), 6, "five answers, the last repeats");
    let sent: Vec<CiConclusion> = sim.sent.iter().map(|(.., s)| s.ci).collect();
    assert_eq!(
        sent,
        [
            CiConclusion::Pending,
            CiConclusion::Failure,
            CiConclusion::Success
        ]
    );
    assert!(sim.sent[2].2.merged);
}

#[test]
fn a_failed_call_sends_nothing_and_is_retried_at_the_next_cadence() {
    let gh = FakeGh::default(); // knows no PR: every call fails
    let mut sim = Sim::new(Cadence::default(), gh, 1000);
    sim.run(0, 70_000, &[pr(1, false)]);
    assert_eq!(sim.starts_of(1), [0, 61]);
    assert!(sim.sent.is_empty());
}

#[test]
fn a_pr_that_leaves_state_is_no_longer_polled() {
    let mut sim = Sim::new(Cadence::default(), fake_for(&[1, 2], PENDING), 1000);
    sim.run(0, 10_000, &[pr(1, true), pr(2, true)]);
    sim.run(11_000, 100_000, &[pr(2, true)]);
    assert_eq!(sim.starts_of(1), [0]);
    assert!(sim.starts_of(2).len() > 5);
    // A new PR URL for a task starts over and is looked at at once.
    let mut moved = pr(2, true);
    moved.url = "https://github.com/o/r/pull/99".into();
    sim.gh = FakeGh::default().answer(&moved.url, &[FAILING]);
    sim.run(101_000, 101_000, &[moved]);
    assert_eq!(sim.starts.last(), Some(&(101_000, 2)));
}

// ---------------------------------------------------------------------------
// What is watched, and the poller end to end on a real actor.
// ---------------------------------------------------------------------------

fn task_with_pr(n: u32, state: TaskState, agent: Option<&AgentId>) -> Task {
    let mut t = Task::new(TaskId::new(n), format!("t{n}"), "");
    t.state = state;
    t.pr = Some(PrRef::new(
        format!("https://github.com/o/r/pull/{n}"),
        n.into(),
    ));
    t.agent = agent.cloned();
    t
}

fn state_with(tasks: Vec<Task>, agents: Vec<Agent>) -> State {
    let repo = Repo {
        path: "/r".into(),
        base_branch: "main".into(),
        remote_url: None,
    };
    let mut plan = Plan::new(PlanId::new(2026, 10, 18, "gh"), "gh", vec![repo]);
    plan.tasks = tasks;
    plan.agents = agents;
    State {
        plans: vec![plan],
        ..State::default()
    }
}

fn agent(n: u32, activity: Activity) -> Agent {
    let id = AgentId::new(
        PlanId::new(2026, 10, 18, "gh"),
        cmux_core::ids::AgentPath::Task(TaskId::new(n)),
    );
    let mut a = Agent::new(id, "s", "/r".into(), [0; 32]);
    a.activity = activity;
    a
}

#[test]
fn watched_is_every_open_pr_and_fast_only_for_idle_testing() {
    let (idle, busy) = (agent(1, Activity::Idle), agent(2, Activity::Working));
    let mut no_pr = Task::new(TaskId::new(5), "no pr", "");
    no_pr.state = TaskState::InProgress;
    let s = state_with(
        vec![
            task_with_pr(1, TaskState::Testing, Some(&idle.id)),
            task_with_pr(2, TaskState::Testing, Some(&busy.id)),
            task_with_pr(3, TaskState::Merged, None),
            task_with_pr(4, TaskState::Failing, None),
            no_pr,
            task_with_pr(6, TaskState::Cancelled, None),
        ],
        vec![idle, busy],
    );
    let got: Vec<(u32, bool)> = watched(&s).iter().map(|w| (w.task.seq(), w.fast)).collect();
    assert_eq!(got, [(1, true), (2, false), (4, false)]);
}

fn start_actor(s: State) -> ActorHandle {
    actor::spawn(
        s,
        ActorOptions {
            tick: None,
            ..ActorOptions::default()
        },
        |_| Box::new(CompositeRunner(Vec::new())),
    )
}

#[tokio::test]
async fn the_poller_moves_the_task_and_records_the_ci_mark() {
    let s = state_with(vec![task_with_pr(1, TaskState::Testing, None)], Vec::new());
    let actor = start_actor(s);
    let gh = FakeGh::default().answer("https://github.com/o/r/pull/1", &[FAILING]);
    let gh = Arc::new(gh);
    let status = Status::default();
    let (stop, rx) = watch::channel(false);
    let cadence = Cadence {
        open_ms: 50,
        testing_ms: 50,
        stagger_ms: 0,
        max_in_flight: 2,
    };
    let task = spawn(
        actor.clone(),
        gh.clone(),
        cadence,
        Duration::from_millis(10),
        status.clone(),
        rx,
    );
    let pr_of = || {
        actor.query(|s| {
            let t = &s.plans[0].tasks[0];
            (t.state, t.pr.clone().unwrap())
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (state, pr) = pr_of().await.unwrap();
        if state == TaskState::Failing {
            assert_eq!(pr.ci, Some(CiMark::Failing));
            assert_eq!(pr.ci_failures, 1);
            break;
        }
        assert!(Instant::now() < deadline, "the task never moved");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(status.get(), "on");
    // Several more looks, but the same answer moves nothing.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(gh.calls.lock().unwrap().len() >= 3);
    assert_eq!(pr_of().await.unwrap().1.ci_failures, 1);
    stop.send(true).unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn a_failed_auth_check_turns_the_poller_off() {
    let actor = start_actor(state_with(
        vec![task_with_pr(1, TaskState::Testing, None)],
        Vec::new(),
    ));
    let gh = Arc::new(FakeGh {
        auth: Some("You are not logged into any GitHub hosts".into()),
        ..FakeGh::default()
    });
    let (_stop, rx) = watch::channel(false);
    let status = Status::default();
    let task = spawn(
        actor,
        gh.clone(),
        Cadence::default(),
        Duration::from_millis(10),
        status.clone(),
        rx,
    );
    task.await.unwrap();
    assert_eq!(
        status.get(),
        "off (You are not logged into any GitHub hosts)"
    );
    assert!(gh.calls.lock().unwrap().is_empty(), "no PR is looked at");
}
