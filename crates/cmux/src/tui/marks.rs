//! Small text marks that several widgets share.

use cmux_proto::view::{Activity, AgentView, ClientId, Life, PrRef, StateView, TermId};

/// `⇲ 120×40` when another client owns the terminal's size (UI design 3.4),
/// to go in the pane header. `None` when `me` owns it or nobody does.
pub fn size_mark(sv: &StateView, term: TermId, me: &ClientId) -> Option<String> {
    let t = sv.terminal(term)?;
    let owner = t.size_owner.as_ref()?;
    (owner != me).then(|| format!("⇲ {}×{}", t.cols, t.rows))
}

/// Whether an agent is done for now: its process is over, or it reported and
/// is not at work. The same rule as the `finished && !working` branch of
/// `cmux_core::glyph::glyph`; a `blocked` agent that resumed is not done.
pub fn agent_done(a: &AgentView) -> bool {
    match a.life {
        Life::Exited { .. } | Life::Stopped => true,
        Life::Starting => false,
        Life::Running => a.report.is_some() && a.activity == Activity::Idle,
    }
}

/// `✉N`, or an empty string when nothing is queued.
pub fn queue_mark(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!("✉{n}")
    }
}

/// `#41 ✓`, `#41 ✗` or `#41 …` by the PR's CI; `#41` before the first poll
/// or without checks.
pub fn pr_mark(pr: &PrRef) -> String {
    match pr.ci {
        Some(ci) => format!("#{} {}", pr.number, ci.glyph()),
        None => format!("#{}", pr.number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_proto::view::{CiMark, TermKind, TerminalView};

    #[test]
    fn pr_mark_shows_the_ci() {
        let mut pr = PrRef::new("https://github.com/o/r/pull/41", 41);
        assert_eq!(pr_mark(&pr), "#41");
        for (ci, want) in [
            (CiMark::Passing, "#41 ✓"),
            (CiMark::Failing, "#41 ✗"),
            (CiMark::Pending, "#41 …"),
        ] {
            pr.ci = Some(ci);
            assert_eq!(pr_mark(&pr), want);
        }
    }

    #[test]
    fn size_mark_only_for_foreign_owner() {
        let mut sv = StateView::default();
        sv.terminals.push(TerminalView {
            id: TermId(1),
            kind: TermKind::Shell,
            agent: None,
            cwd: "/".into(),
            rows: 40,
            cols: 120,
            size_owner: Some(ClientId("phone".into())),
            title: None,
        });
        let me = ClientId("me".into());
        assert_eq!(size_mark(&sv, TermId(1), &me).as_deref(), Some("⇲ 120×40"));
        assert_eq!(size_mark(&sv, TermId(1), &ClientId("phone".into())), None);
        sv.terminals[0].size_owner = None;
        assert_eq!(size_mark(&sv, TermId(1), &me), None);
        assert_eq!(size_mark(&sv, TermId(9), &me), None);
        assert_eq!(queue_mark(0), "");
        assert_eq!(queue_mark(3), "✉3");
    }

    #[test]
    fn a_running_agent_is_done_only_when_idle_with_a_report() {
        use cmux_core::ids::AgentPath;
        use cmux_proto::view::{Report, ReportStatus};
        let report = Report {
            status: ReportStatus::Success,
            summary: "x".into(),
            pr_url: None,
            attempt: 1,
            implicit: false,
            supersedes_blocked: false,
        };
        let plan = cmux_proto::view::PlanId::new(2026, 10, 18, "auth");
        let mut a = AgentView {
            id: cmux_proto::view::AgentId::new(plan, AgentPath::Executor),
            path: AgentPath::Executor,
            level: cmux_proto::view::Level::Executor,
            parent: None,
            task: None,
            term: None,
            cwd: "/".into(),
            life: Life::Running,
            activity: Activity::Idle,
            report: Some(report),
            tokens: Default::default(),
            idle_since: None,
            stalled: false,
        };
        assert!(agent_done(&a));
        for act in [
            Activity::NeedsPermission,
            Activity::Working,
            Activity::Unknown,
        ] {
            a.activity = act;
            assert!(!agent_done(&a), "{act:?}");
        }
        a.activity = Activity::Idle;
        a.report = None;
        assert!(!agent_done(&a));
    }
}
