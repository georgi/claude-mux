//! Small text marks that several widgets share.

use cmux_proto::view::{Activity, AgentView, ClientId, Life, ReportStatus, StateView, TermId};

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
        Life::Running => a.report.is_some() && a.activity != Activity::Working,
    }
}

/// `reported success` (or `blocked`, `failed`) for an agent that handed in its
/// report and is done, to go in the pane header instead of the directory (UI
/// design 3.3). An agent that resumed work shows its directory again.
pub fn report_mark(a: &AgentView) -> Option<String> {
    if !agent_done(a) {
        return None;
    }
    let r = a.report.as_ref()?;
    let what = match r.status {
        ReportStatus::Success => "success",
        ReportStatus::Blocked => "blocked",
        ReportStatus::Failed => "failed",
    };
    Some(format!("reported {what}"))
}

/// `✉N`, or an empty string when nothing is queued.
pub fn queue_mark(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!("✉{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_proto::view::{TermKind, TerminalView};

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
}
