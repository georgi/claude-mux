//! Agent status glyph (UI design 2.1). A pure function of the agent, its
//! inbox items and its children; never stored.

use crate::ids::AgentId;
use crate::model::{Activity, Agent, InboxItem, ItemKind, Level, Life, Recipient, ReportStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Glyph {
    Working,
    Idle,
    Parked,
    NeedsYou,
    /// Finished, and the user has not looked yet.
    Unseen,
    Success,
    Failed,
    Stalled,
}

impl Glyph {
    /// Working is the spinner's static frame; clients animate it.
    pub fn as_char(&self) -> char {
        match self {
            Glyph::Working => '⠹',
            Glyph::Idle => '○',
            Glyph::Parked => '◌',
            Glyph::NeedsYou => '⚑',
            Glyph::Unseen => '●',
            Glyph::Success => '✓',
            Glyph::Failed => '✕',
            Glyph::Stalled => '⚠',
        }
    }
}

/// `◆` prefix for the plan executor.
pub fn executor_prefix(level: Level) -> Option<char> {
    (level == Level::Executor).then_some('◆')
}

/// The N of `✉N`: items queued for this agent and not yet delivered.
pub fn pending_count(agent: &AgentId, inbox: &[InboxItem]) -> usize {
    inbox
        .iter()
        .filter(|i| i.is_pending() && i.to == Recipient::Agent(agent.clone()))
        .count()
}

/// Precedence, first match wins:
/// 1. `NeedsYou`: a pending Permission/Question item from this agent to the
///    human, or activity `NeedsPermission` while running.
/// 2. `Stalled`: the caller's watchdog flag, while running.
/// 3. `Failed`: stopped, or exited with a non-zero code or by signal.
/// 4. Finished with a report (not currently `Working`): `Unseen` if `!seen`,
///    else `Success`, or `Failed` for a failed report. A blocked report shows
///    as `Failed` once seen. A clean exit with no report is `Unseen` until
///    seen, then `Idle`.
/// 5. `Working`, also while the process is still `Starting`.
/// 6. `Parked`: not working, with at least one live (starting or running) child.
/// 7. `Idle`.
pub fn glyph(
    agent: &Agent,
    inbox: &[InboxItem],
    children: &[&Agent],
    seen: bool,
    stalled: bool,
) -> Glyph {
    let running = agent.life == Life::Running;
    let asks = inbox.iter().any(|i| {
        i.is_pending()
            && i.to == Recipient::Human
            && i.from.as_ref() == Some(&agent.id)
            && matches!(i.kind, ItemKind::Permission { .. } | ItemKind::Question)
    });
    if asks || (running && agent.activity == Activity::NeedsPermission) {
        return Glyph::NeedsYou;
    }
    if running && stalled {
        return Glyph::Stalled;
    }
    match agent.life {
        Life::Stopped
        | Life::Exited { code: None }
        | Life::Exited {
            code: Some(1..) | Some(..=-1),
        } => {
            return Glyph::Failed;
        }
        _ => {}
    }
    // Starting: the process is on its way up, which is work, not an exit.
    if agent.life == Life::Starting {
        return Glyph::Working;
    }
    let working = running && agent.activity == Activity::Working;
    let finished = !running || agent.report.is_some();
    if finished && !working {
        if !seen {
            return Glyph::Unseen;
        }
        return match &agent.report {
            Some(r) if r.status == ReportStatus::Success => Glyph::Success,
            Some(_) => Glyph::Failed,
            None => Glyph::Idle,
        };
    }
    if working {
        Glyph::Working
    } else if children.iter().any(|c| c.is_live()) {
        Glyph::Parked
    } else {
        Glyph::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentPath, ItemId, PlanId, TaskId};
    use crate::model::{Delivery, Report};

    fn id(p: AgentPath) -> AgentId {
        AgentId::new(PlanId::new(2026, 10, 18, "auth"), p)
    }

    fn agent(p: AgentPath, activity: Activity) -> Agent {
        let mut a = Agent::new(id(p), "s", "/w".into(), [0; 32]);
        a.activity = activity;
        a
    }

    fn task_agent(activity: Activity) -> Agent {
        agent(AgentPath::Task(TaskId::new(3)), activity)
    }

    fn item(from: &Agent, to: Recipient, kind: ItemKind) -> InboxItem {
        InboxItem {
            id: ItemId(1),
            to,
            kind,
            from: Some(from.id.clone()),
            body: String::new(),
            status: Delivery::Pending,
            dedupe: None,
            created: 0,
        }
    }

    fn report(status: ReportStatus) -> Option<Report> {
        Some(Report {
            status,
            summary: String::new(),
            pr_url: None,
            attempt: 1,
            implicit: false,
            supersedes_blocked: false,
        })
    }

    #[test]
    fn table_2_1_rows() {
        // Working: spinner
        let a = task_agent(Activity::Working);
        assert_eq!(glyph(&a, &[], &[], true, false).as_char(), '⠹');
        // Idle: nothing pending
        let a = task_agent(Activity::Idle);
        assert_eq!(glyph(&a, &[], &[], true, false).as_char(), '○');
        // Parked: idle with a running child
        let child = agent(
            AgentPath::sub(TaskId::new(3), "tests").unwrap(),
            Activity::Working,
        );
        assert_eq!(glyph(&a, &[], &[&child], true, false).as_char(), '◌');
        // Needs you: pending permission request
        let perm = item(
            &a,
            Recipient::Human,
            ItemKind::Permission {
                tool: "Bash".into(),
                input_summary: "ls".into(),
            },
        );
        assert_eq!(glyph(&a, &[perm], &[], true, false).as_char(), '⚑');
        // Finished, not yet looked at
        let mut done = task_agent(Activity::Idle);
        done.report = report(ReportStatus::Success);
        assert_eq!(glyph(&done, &[], &[], false, false).as_char(), '●');
        // Reported success, seen
        assert_eq!(glyph(&done, &[], &[], true, false).as_char(), '✓');
        // Failed: crashed
        let mut crashed = task_agent(Activity::Idle);
        crashed.life = Life::Exited { code: Some(1) };
        assert_eq!(glyph(&crashed, &[], &[], true, false).as_char(), '✕');
        // Stalled
        let a = task_agent(Activity::Working);
        assert_eq!(glyph(&a, &[], &[], true, true).as_char(), '⚠');
        // Executor prefix and queued-message count
        assert_eq!(executor_prefix(Level::Executor), Some('◆'));
        assert_eq!(executor_prefix(Level::Task), None);
        assert_eq!(executor_prefix(Level::Sub), None);
    }

    #[test]
    fn failed_variants() {
        let mut a = task_agent(Activity::Idle);
        a.life = Life::Stopped;
        assert_eq!(glyph(&a, &[], &[], false, false), Glyph::Failed);
        a.life = Life::Exited { code: None };
        assert_eq!(glyph(&a, &[], &[], true, false), Glyph::Failed);
        a.life = Life::Exited { code: Some(-9) };
        assert_eq!(glyph(&a, &[], &[], true, false), Glyph::Failed);
        // Failed report, seen; blocked report likewise.
        let mut r = task_agent(Activity::Idle);
        r.report = report(ReportStatus::Failed);
        assert_eq!(glyph(&r, &[], &[], true, false), Glyph::Failed);
        r.report = report(ReportStatus::Blocked);
        assert_eq!(glyph(&r, &[], &[], true, false), Glyph::Failed);
        // Clean exit, no report.
        let mut c = task_agent(Activity::Idle);
        c.life = Life::Exited { code: Some(0) };
        assert_eq!(glyph(&c, &[], &[], false, false), Glyph::Unseen);
        assert_eq!(glyph(&c, &[], &[], true, false), Glyph::Idle);
    }

    #[test]
    fn precedence() {
        let mut a = task_agent(Activity::NeedsPermission);
        // NeedsPermission activity alone => needs you; beats stalled.
        assert_eq!(glyph(&a, &[], &[], true, true), Glyph::NeedsYou);
        // Stalled beats working and failed report.
        a.activity = Activity::Working;
        a.report = report(ReportStatus::Failed);
        assert_eq!(glyph(&a, &[], &[], false, true), Glyph::Stalled);
        // A working agent with a stale report shows Working.
        assert_eq!(glyph(&a, &[], &[], false, false), Glyph::Working);
        // Stalled does not apply to an exited agent.
        a.life = Life::Exited { code: Some(0) };
        a.activity = Activity::Idle;
        assert_eq!(glyph(&a, &[], &[], true, true), Glyph::Failed);
        // Questions count; other kinds and other recipients do not.
        let b = task_agent(Activity::Idle);
        let q = item(&b, Recipient::Human, ItemKind::Question);
        assert_eq!(
            glyph(&b, std::slice::from_ref(&q), &[], true, false),
            Glyph::NeedsYou
        );
        let other = agent(AgentPath::Task(TaskId::new(4)), Activity::Idle);
        assert_eq!(
            glyph(&other, std::slice::from_ref(&q), &[], true, false),
            Glyph::Idle
        );
        let to_parent = item(
            &b,
            Recipient::Agent(id(AgentPath::Executor)),
            ItemKind::ChildQuestion,
        );
        assert_eq!(glyph(&b, &[to_parent], &[], true, false), Glyph::Idle);
        let mut answered = q;
        answered.status = Delivery::Dismissed;
        assert_eq!(glyph(&b, &[answered], &[], true, false), Glyph::Idle);
        // Exited children do not park the parent; working with children stays Working.
        let mut dead = agent(AgentPath::sub(TaskId::new(3), "x").unwrap(), Activity::Idle);
        dead.life = Life::Stopped;
        assert_eq!(glyph(&b, &[], &[&dead], true, false), Glyph::Idle);
        let w = task_agent(Activity::Working);
        let live = agent(
            AgentPath::sub(TaskId::new(3), "y").unwrap(),
            Activity::Working,
        );
        assert_eq!(glyph(&w, &[], &[&live], true, false), Glyph::Working);
    }

    #[test]
    fn a_starting_agent_shows_as_working_and_a_starting_child_parks_its_parent() {
        let mut a = task_agent(Activity::Unknown);
        a.life = Life::Starting;
        assert_eq!(glyph(&a, &[], &[], false, false), Glyph::Working);
        assert_eq!(glyph(&a, &[], &[], true, false), Glyph::Working);
        let mut child = agent(
            AgentPath::sub(TaskId::new(3), "x").unwrap(),
            Activity::Unknown,
        );
        child.life = Life::Starting;
        let parent = task_agent(Activity::Idle);
        assert_eq!(glyph(&parent, &[], &[&child], true, false), Glyph::Parked);
    }

    #[test]
    fn chars_are_distinct_and_pending_counts() {
        let all = [
            Glyph::Working,
            Glyph::Idle,
            Glyph::Parked,
            Glyph::NeedsYou,
            Glyph::Unseen,
            Glyph::Success,
            Glyph::Failed,
            Glyph::Stalled,
        ];
        let mut cs: Vec<char> = all.iter().map(Glyph::as_char).collect();
        cs.sort();
        cs.dedup();
        assert_eq!(cs.len(), 8);

        let a = task_agent(Activity::Idle);
        let me = Recipient::Agent(a.id.clone());
        let mut delivered = item(&a, me.clone(), ItemKind::UserMessage);
        delivered.status = Delivery::Expired;
        let inbox = [
            item(&a, me.clone(), ItemKind::UserMessage),
            item(&a, me, ItemKind::Steer),
            item(&a, Recipient::Human, ItemKind::Question),
            delivered,
        ];
        assert_eq!(pending_count(&a.id, &inbox), 2);
        assert_eq!(pending_count(&id(AgentPath::Executor), &inbox), 0);
    }
}
