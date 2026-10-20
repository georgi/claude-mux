//! What a notification says (architecture 11.3): the item id, its kind and a
//! summary of at most [`PUSH_SUMMARY_MAX`] characters. Never a message body,
//! and of a tool call only its name and the hook's own short summary.

use cmux_core::ids::PlanId;
use cmux_core::model::{InboxItem, ItemKind};
use cmux_proto::api::{NotifyKind, PUSH_SUMMARY_MAX, PushPayload};

/// The notify kind of an item, `None` for kinds that never notify.
pub fn notify_kind(kind: &ItemKind) -> Option<NotifyKind> {
    Some(match kind {
        ItemKind::Permission { .. } => NotifyKind::Permission,
        ItemKind::Question => NotifyKind::Question,
        ItemKind::PlanReview => NotifyKind::PlanReview,
        ItemKind::Stalled | ItemKind::ExitedWithoutReport => NotifyKind::Stalled,
        ItemKind::CiFailing => NotifyKind::CiFailing,
        _ => return None,
    })
}

/// The payload for a new human item.
pub fn for_item(item: &InboxItem) -> Option<PushPayload> {
    let kind = notify_kind(&item.kind)?;
    let who = item
        .from
        .as_ref()
        .map_or_else(|| "cmux".to_string(), |a| a.path().to_string());
    let text = match &item.kind {
        ItemKind::Permission {
            tool,
            input_summary,
        } => format!("{who}: {tool} {input_summary}"),
        ItemKind::Question => format!("{who} asks a question"),
        ItemKind::PlanReview => match &item.from {
            Some(a) => format!("Plan {} is ready for review", a.plan()),
            None => "A plan is ready for review".into(),
        },
        ItemKind::Stalled => format!("{who} is stalled"),
        ItemKind::ExitedWithoutReport => format!("{who} exited without a report"),
        ItemKind::CiFailing => format!("CI is failing for {who}"),
        _ => return None,
    };
    Some(PushPayload {
        kind,
        item: Some(item.id.0),
        plan: None,
        summary: summary(&text),
    })
}

/// The payload for a finished plan.
pub fn for_plan_done(plan: &PlanId, title: &str) -> PushPayload {
    PushPayload {
        kind: NotifyKind::PlanDone,
        item: None,
        plan: Some(plan.to_string()),
        summary: summary(&format!("Plan done: {title}")),
    }
}

/// One line of at most [`PUSH_SUMMARY_MAX`] characters; a cut ends in `…`.
pub fn summary(text: &str) -> String {
    let line = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    if line.chars().count() <= PUSH_SUMMARY_MAX {
        return line;
    }
    let mut cut: String = line.chars().take(PUSH_SUMMARY_MAX - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::ids::ItemId;
    use cmux_core::model::{Delivery, Recipient};

    fn item(kind: ItemKind, body: &str) -> InboxItem {
        InboxItem {
            id: ItemId(12),
            to: Recipient::Human,
            kind,
            from: Some("P-20261018-x/T-0003".parse().unwrap()),
            body: body.into(),
            status: Delivery::Pending,
            dedupe: None,
            created: 0,
        }
    }

    #[test]
    fn a_permission_names_the_agent_and_the_tool_but_not_the_body() {
        let p = for_item(&item(
            ItemKind::Permission {
                tool: "Bash".into(),
                input_summary: "cargo test".into(),
            },
            "SECRET body text",
        ))
        .unwrap();
        assert_eq!(p.kind, NotifyKind::Permission);
        assert_eq!(p.item, Some(12));
        assert_eq!(p.summary, "T-0003: Bash cargo test");
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("SECRET"), "{json}");
    }

    #[test]
    fn a_question_never_carries_its_text() {
        let p = for_item(&item(ItemKind::Question, "What is the password?")).unwrap();
        assert_eq!(p.kind, NotifyKind::Question);
        assert_eq!(p.summary, "T-0003 asks a question");
    }

    #[test]
    fn kinds_map_to_the_device_choices() {
        assert_eq!(
            notify_kind(&ItemKind::ExitedWithoutReport),
            Some(NotifyKind::Stalled)
        );
        assert_eq!(
            notify_kind(&ItemKind::CiFailing),
            Some(NotifyKind::CiFailing)
        );
        assert_eq!(
            notify_kind(&ItemKind::PlanReview),
            Some(NotifyKind::PlanReview)
        );
        assert_eq!(notify_kind(&ItemKind::UserMessage), None);
        assert!(for_item(&item(ItemKind::ChildReport, "x")).is_none());
    }

    #[test]
    fn the_summary_is_at_most_60_chars() {
        let long = "x".repeat(200);
        let p = for_item(&item(
            ItemKind::Permission {
                tool: "Bash".into(),
                input_summary: long,
            },
            "",
        ))
        .unwrap();
        assert_eq!(p.summary.chars().count(), PUSH_SUMMARY_MAX);
        assert!(p.summary.ends_with('…'));
        // Exactly 60 stays whole; 61 is cut.
        let sixty = "a".repeat(60);
        assert_eq!(summary(&sixty), sixty);
        let cut = summary(&"a".repeat(61));
        assert_eq!(cut.chars().count(), 60);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn the_summary_counts_chars_not_bytes_and_is_one_line() {
        let s = summary(&"ü".repeat(100));
        assert_eq!(s.chars().count(), 60);
        assert_eq!(summary("a\nb\t c\u{7}"), "a b c");
        assert_eq!(
            summary(&format!("{}  tail", "w".repeat(58)))
                .chars()
                .count(),
            59
        );
    }

    #[test]
    fn a_finished_plan_names_the_plan() {
        let plan: PlanId = "P-20261018-x".parse().unwrap();
        let p = for_plan_done(&plan, "Ship the push step");
        assert_eq!(p.kind, NotifyKind::PlanDone);
        assert_eq!(p.item, None);
        assert_eq!(p.plan.as_deref(), Some("P-20261018-x"));
        assert_eq!(p.summary, "Plan done: Ship the push step");
    }
}
