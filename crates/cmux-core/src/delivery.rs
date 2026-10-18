//! Delivery of inbox items to agents (architecture section 8). Pure: the
//! engine supplies the agent, the event and the clock, and acts on the result.

use crate::ids::AgentId;
use crate::model::{
    Activity, Agent, DeliveryPath, InboxItem, ItemKind, Life, Recipient, Timestamp,
};

/// Path A: Stop continuations allowed in a row without a tool call.
pub const MAX_STOP_BLOCKS: u8 = 3;

/// Path B: a client typing into an agent's terminal blocks PTY injection for this long.
pub const HUMAN_TYPING_GRACE_MS: u64 = 3_000;

/// Hook events that can trigger delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryEvent {
    /// The agent is ending a turn. `stop_hook_active` is what Claude Code reports.
    Stop { stop_hook_active: bool },
    /// The agent has gone idle outside a Stop hook.
    Idle,
    /// A human submitted a prompt (path C).
    PromptSubmit,
}

/// Items addressed to this agent (`Recipient::Agent`) that are still Pending,
/// oldest first (by creation time, then id).
pub fn pending_for<'a>(agent: &AgentId, inbox: &'a [InboxItem]) -> Vec<&'a InboxItem> {
    let mut items: Vec<&InboxItem> = inbox
        .iter()
        .filter(|item| item.is_pending())
        .filter(|item| matches!(&item.to, Recipient::Agent(a) if a == agent))
        .collect();
    items.sort_by_key(|item| (item.created, item.id.0));
    items
}

/// Which path, if any, delivers pending items to `agent` for this event.
/// `None` when nothing is pending, the agent is not running, or delivery must wait.
pub fn choose_path(
    agent: &Agent,
    event: &DeliveryEvent,
    now: Timestamp,
    has_pending: bool,
) -> Option<DeliveryPath> {
    if !has_pending || agent.life != Life::Running {
        return None;
    }
    match event {
        DeliveryEvent::Stop { .. } => {
            if agent.consecutive_stop_blocks < MAX_STOP_BLOCKS {
                Some(DeliveryPath::StopBlock)
            } else if paste_allowed(agent, now) {
                Some(DeliveryPath::PtyPaste)
            } else {
                None
            }
        }
        DeliveryEvent::Idle => paste_allowed(agent, now).then_some(DeliveryPath::PtyPaste),
        DeliveryEvent::PromptSubmit => Some(DeliveryPath::PromptContext),
    }
}

/// Path B conditions: the agent is idle (no permission prompt, not working) and
/// no human has typed into its terminal within the grace period.
fn paste_allowed(agent: &Agent, now: Timestamp) -> bool {
    agent.activity == Activity::Idle
        && agent
            .last_human_input_at
            .is_none_or(|at| now.saturating_sub(at) >= HUMAN_TYPING_GRACE_MS)
}

/// The text an agent receives through paths A, B or C: one block per item,
/// each with its id so the agent can skip ones it already handled. Empty input
/// gives an empty string.
pub fn digest(items: &[&InboxItem]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let count = items.len();
    let noun = if count == 1 { "message" } else { "messages" };
    let mut out =
        format!("[cmux] {count} {noun} for you (ids let you skip ones you already handled):");
    for item in items {
        let sender = match &item.from {
            Some(agent) => agent.path().to_string(),
            None => "user".to_string(),
        };
        out.push_str("\n\n");
        out.push_str(&format!(
            "[{}] {} from {}:",
            item.id,
            kind_label(&item.kind),
            sender
        ));
        for line in item.body.trim().lines() {
            out.push('\n');
            if !line.trim().is_empty() {
                out.push_str("  ");
                out.push_str(line.trim_end());
            }
        }
    }
    out
}

fn kind_label(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::ChildReport => "report",
        ItemKind::ChildQuestion | ItemKind::Question => "question",
        ItemKind::ChildExited => "child exited",
        ItemKind::UserMessage => "message",
        ItemKind::Steer => "steer",
        ItemKind::PrChanged => "PR update",
        ItemKind::Permission { .. } => "permission",
        ItemKind::PlanReview => "plan review",
        ItemKind::Stalled => "stalled",
        ItemKind::ExitedWithoutReport => "exited without report",
    }
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// Bytes for a bracketed paste of `text` followed by Enter. Paste markers inside
/// `text` are removed first, so a body cannot end the paste early.
pub fn bracketed_paste(text: &str) -> Vec<u8> {
    let mut clean = text.to_string();
    // Removing one marker can join its neighbours into a new one, so repeat.
    while clean.contains(PASTE_END) || clean.contains("\x1b[200~") {
        clean = clean.replace(PASTE_END, "").replace("\x1b[200~", "");
    }
    let mut out = Vec::with_capacity(PASTE_START.len() + clean.len() + PASTE_END.len() + 1);
    out.extend_from_slice(PASTE_START);
    out.extend_from_slice(clean.as_bytes());
    out.extend_from_slice(PASTE_END.as_bytes());
    out.push(b'\r');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentPath, ItemId, PlanId, TaskId};
    use crate::model::Delivery;
    use std::path::PathBuf;

    fn plan() -> PlanId {
        PlanId::new(2026, 10, 18, "demo")
    }

    fn agent_id(path: AgentPath) -> AgentId {
        AgentId::new(plan(), path)
    }

    fn agent(path: AgentPath) -> Agent {
        Agent::new(agent_id(path), "sess", PathBuf::from("/w"), [0; 32])
    }

    fn task_agent() -> Agent {
        let mut a = agent(AgentPath::Task(TaskId::new(3)));
        a.life = Life::Running;
        a.activity = Activity::Idle;
        a
    }

    fn item(
        id: u64,
        to: Recipient,
        kind: ItemKind,
        from: Option<AgentId>,
        body: &str,
        created: Timestamp,
    ) -> InboxItem {
        InboxItem {
            id: ItemId(id),
            to,
            kind,
            from,
            body: body.to_string(),
            status: Delivery::Pending,
            dedupe: None,
            created,
        }
    }

    fn to_agent(path: AgentPath, id: u64, created: Timestamp) -> InboxItem {
        item(
            id,
            Recipient::Agent(agent_id(path)),
            ItemKind::UserMessage,
            None,
            "hi",
            created,
        )
    }

    #[test]
    fn pending_for_filters_recipient_and_status_and_sorts_oldest_first() {
        let t3 = AgentPath::Task(TaskId::new(3));
        let mut delivered = to_agent(t3.clone(), 1, 5);
        delivered.status = Delivery::Dismissed;
        let inbox = vec![
            to_agent(t3.clone(), 4, 300),
            to_agent(AgentPath::Task(TaskId::new(4)), 2, 100),
            delivered,
            item(3, Recipient::Human, ItemKind::Question, None, "q", 50),
            to_agent(t3.clone(), 5, 200),
            to_agent(t3.clone(), 6, 200),
        ];
        let got: Vec<u64> = pending_for(&agent_id(t3), &inbox)
            .iter()
            .map(|i| i.id.0)
            .collect();
        assert_eq!(got, vec![5, 6, 4]);
    }

    #[test]
    fn pending_for_empty_inbox() {
        assert!(pending_for(&agent_id(AgentPath::Executor), &[]).is_empty());
    }

    #[test]
    fn nothing_pending_gives_none_for_every_event() {
        let a = task_agent();
        for ev in [
            DeliveryEvent::Stop {
                stop_hook_active: false,
            },
            DeliveryEvent::Idle,
            DeliveryEvent::PromptSubmit,
        ] {
            assert_eq!(choose_path(&a, &ev, 1_000_000, false), None);
        }
    }

    #[test]
    fn not_running_gives_none() {
        let mut a = task_agent();
        a.life = Life::Stopped;
        assert_eq!(choose_path(&a, &DeliveryEvent::PromptSubmit, 0, true), None);
        a.life = Life::Exited { code: Some(0) };
        assert_eq!(
            choose_path(
                &a,
                &DeliveryEvent::Stop {
                    stop_hook_active: false
                },
                0,
                true
            ),
            None
        );
    }

    #[test]
    fn stop_uses_stop_block_below_cap() {
        let mut a = task_agent();
        for n in 0..MAX_STOP_BLOCKS {
            a.consecutive_stop_blocks = n;
            assert_eq!(
                choose_path(
                    &a,
                    &DeliveryEvent::Stop {
                        stop_hook_active: true
                    },
                    0,
                    true
                ),
                Some(DeliveryPath::StopBlock)
            );
        }
    }

    #[test]
    fn stop_at_cap_falls_back_to_paste_when_idle_and_no_typing() {
        let mut a = task_agent();
        a.consecutive_stop_blocks = MAX_STOP_BLOCKS;
        a.last_human_input_at = None;
        assert_eq!(
            choose_path(
                &a,
                &DeliveryEvent::Stop {
                    stop_hook_active: true
                },
                10,
                true
            ),
            Some(DeliveryPath::PtyPaste)
        );
    }

    #[test]
    fn stop_at_cap_with_paste_blocked_gives_none() {
        // Not idle: a permission prompt is showing.
        let mut a = task_agent();
        a.consecutive_stop_blocks = MAX_STOP_BLOCKS;
        a.activity = Activity::NeedsPermission;
        assert_eq!(
            choose_path(
                &a,
                &DeliveryEvent::Stop {
                    stop_hook_active: true
                },
                10,
                true
            ),
            None
        );
        // Human typing just now.
        a.activity = Activity::Idle;
        a.last_human_input_at = Some(10);
        assert_eq!(
            choose_path(
                &a,
                &DeliveryEvent::Stop {
                    stop_hook_active: true
                },
                11,
                true
            ),
            None
        );
    }

    #[test]
    fn idle_pastes_only_when_idle_and_no_recent_typing() {
        let mut a = task_agent();
        assert_eq!(
            choose_path(&a, &DeliveryEvent::Idle, 0, true),
            Some(DeliveryPath::PtyPaste)
        );

        a.activity = Activity::Working;
        assert_eq!(choose_path(&a, &DeliveryEvent::Idle, 0, true), None);
        a.activity = Activity::NeedsPermission;
        assert_eq!(choose_path(&a, &DeliveryEvent::Idle, 0, true), None);
        a.activity = Activity::Unknown;
        assert_eq!(choose_path(&a, &DeliveryEvent::Idle, 0, true), None);
    }

    #[test]
    fn idle_never_uses_stop_block() {
        let mut a = task_agent();
        a.consecutive_stop_blocks = 0;
        assert_ne!(
            choose_path(&a, &DeliveryEvent::Idle, 0, true),
            Some(DeliveryPath::StopBlock)
        );
    }

    #[test]
    fn typing_grace_boundary() {
        let mut a = task_agent();
        let typed_at = 100_000;
        a.last_human_input_at = Some(typed_at);
        let ev = DeliveryEvent::Idle;
        // One ms before the grace period ends: still blocked.
        let just_before = typed_at + HUMAN_TYPING_GRACE_MS - 1;
        assert_eq!(choose_path(&a, &ev, just_before, true), None);
        // Exactly at the end of the grace period: allowed.
        let at_end = typed_at + HUMAN_TYPING_GRACE_MS;
        assert_eq!(
            choose_path(&a, &ev, at_end, true),
            Some(DeliveryPath::PtyPaste)
        );
        // Typing in the future (clock skew) counts as recent.
        assert_eq!(choose_path(&a, &ev, typed_at - 5, true), None);
    }

    #[test]
    fn prompt_submit_is_prompt_context_regardless_of_activity() {
        let mut a = task_agent();
        a.activity = Activity::Working;
        a.last_human_input_at = Some(0);
        assert_eq!(
            choose_path(&a, &DeliveryEvent::PromptSubmit, 0, true),
            Some(DeliveryPath::PromptContext)
        );
    }

    #[test]
    fn digest_exact_text_for_two_items() {
        let child = AgentPath::Task(TaskId::new(3));
        let a = item(
            12,
            Recipient::Agent(agent_id(AgentPath::Executor)),
            ItemKind::ChildReport,
            Some(agent_id(child.clone())),
            "Implemented the parser.\nAll tests pass.",
            1,
        );
        let b = item(
            13,
            Recipient::Agent(agent_id(AgentPath::Executor)),
            ItemKind::UserMessage,
            None,
            "Please also check the docs.",
            2,
        );
        let got = digest(&[&a, &b]);
        let want = "[cmux] 2 messages for you (ids let you skip ones you already handled):\n\n\
[I-12] report from T-0003:\n  Implemented the parser.\n  All tests pass.\n\n\
[I-13] message from user:\n  Please also check the docs.";
        assert_eq!(got, want);
    }

    #[test]
    fn digest_single_item_and_kind_labels() {
        let kinds = [
            (ItemKind::ChildReport, "report"),
            (ItemKind::ChildQuestion, "question"),
            (ItemKind::Question, "question"),
            (ItemKind::ChildExited, "child exited"),
            (ItemKind::UserMessage, "message"),
            (ItemKind::Steer, "steer"),
            (ItemKind::PrChanged, "PR update"),
            (
                ItemKind::Permission {
                    tool: "Bash".into(),
                    input_summary: "ls".into(),
                },
                "permission",
            ),
            (ItemKind::PlanReview, "plan review"),
            (ItemKind::Stalled, "stalled"),
            (ItemKind::ExitedWithoutReport, "exited without report"),
        ];
        for (kind, label) in kinds {
            let it = item(
                7,
                Recipient::Human,
                kind,
                Some(agent_id(AgentPath::Executor)),
                "x",
                0,
            );
            assert_eq!(
                digest(&[&it]),
                format!(
                    "[cmux] 1 message for you (ids let you skip ones you already handled):\n\n[I-7] {label} from executor:\n  x"
                )
            );
        }
    }

    #[test]
    fn digest_empty_body_and_blank_lines() {
        let it = item(
            1,
            Recipient::Human,
            ItemKind::Stalled,
            Some(agent_id(AgentPath::Task(TaskId::new(9)))),
            "\n  first\n\n  second  \n\n",
            0,
        );
        assert_eq!(
            digest(&[&it]),
            "[cmux] 1 message for you (ids let you skip ones you already handled):\n\n[I-1] stalled from T-0009:\n  first\n\n    second"
        );
        let empty = item(2, Recipient::Human, ItemKind::Stalled, None, "   ", 0);
        assert!(digest(&[&empty]).ends_with("[I-2] stalled from user:"));
    }

    #[test]
    fn digest_empty_is_empty_string() {
        assert_eq!(digest(&[]), "");
    }

    #[test]
    fn digest_is_deterministic_and_uses_sub_agent_path() {
        let sub = AgentPath::sub(TaskId::new(3), "reviewer").unwrap();
        let it = item(
            5,
            Recipient::Human,
            ItemKind::Question,
            Some(agent_id(sub)),
            "why?",
            0,
        );
        let first = digest(&[&it]);
        assert_eq!(first, digest(&[&it]));
        assert!(first.contains("[I-5] question from T-0003/reviewer:"));
    }

    #[test]
    fn paste_framing() {
        let got = bracketed_paste("hello\nworld");
        assert_eq!(got, b"\x1b[200~hello\nworld\x1b[201~\r".to_vec());
    }

    #[test]
    fn paste_strips_end_marker_from_body() {
        let got = bracketed_paste("a\x1b[201~b");
        assert_eq!(got, b"\x1b[200~ab\x1b[201~\r".to_vec());
    }

    #[test]
    fn paste_strips_markers_that_recombine() {
        // Removing the inner marker joins the outer pieces into a new one.
        let got = bracketed_paste("x\x1b[20\x1b[201~1~y");
        assert_eq!(got, b"\x1b[200~xy\x1b[201~\r".to_vec());
        let got = bracketed_paste("\x1b[201\x1b[201~~\x1b[201~");
        assert_eq!(got, b"\x1b[200~\x1b[201~\r".to_vec());
    }

    #[test]
    fn paste_strips_start_marker_and_keeps_unicode() {
        let got = bracketed_paste("é\x1b[200~ü");
        assert_eq!(got, "\x1b[200~éü\x1b[201~\r".as_bytes().to_vec());
    }

    #[test]
    fn paste_of_empty_text() {
        assert_eq!(bracketed_paste(""), b"\x1b[200~\x1b[201~\r".to_vec());
    }
}
