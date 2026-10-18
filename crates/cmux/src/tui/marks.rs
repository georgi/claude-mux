//! Small text marks that several widgets share.

use cmux_proto::view::{ClientId, PrRef, StateView, TermId};

/// `⇲ 120×40` when another client owns the terminal's size (UI design 3.4),
/// to go in the pane header. `None` when `me` owns it or nobody does.
pub fn size_mark(sv: &StateView, term: TermId, me: &ClientId) -> Option<String> {
    let t = sv.terminal(term)?;
    let owner = t.size_owner.as_ref()?;
    (owner != me).then(|| format!("⇲ {}×{}", t.cols, t.rows))
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
}
