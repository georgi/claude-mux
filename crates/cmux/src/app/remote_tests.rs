//! Remote mode against a fake HTTP layer and a fixture state, plus one run
//! against a real in-process `cmuxd`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use cmux_core::ids::AgentPath;
use cmux_proto::api::{Scope, ServerInfo};
use cmux_proto::view::{
    AgentId, ClientId, PlanId, StateView, TaskId, TermId, TermKind, TerminalView,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::{Value, json};

use super::*;
use crate::conn::stream::Cmd;
use crate::tui::fixture;
use crate::tui::remote::Call;
use crate::tui::remote::fake::Recording;

const PREFIX: &[u8] = b"\x02";

pub fn options() -> crate::Options {
    crate::Options {
        prefix: 2,
        claude: "claude".into(),
        claude_args: Vec::new(),
        dirs: Vec::new(),
        resume_picker: false,
        continue_last: false,
        bell: false,
        mouse: true,
        server: None,
        local: false,
    }
}

/// The fixture with a terminal behind every agent.
fn state_with_terms() -> StateView {
    let mut sv = fixture::state();
    for i in 0..sv.agents.len() {
        let id = TermId(10 + i as u32);
        sv.agents[i].term = Some(id);
        let agent = sv.agents[i].id.clone();
        sv.terminals.push(TerminalView {
            id,
            kind: TermKind::Claude,
            agent: Some(agent),
            cwd: "/home/me/api".into(),
            rows: 24,
            cols: 80,
            size_owner: None,
            title: None,
        });
    }
    sv
}

struct Rig {
    app: App,
    rest: Arc<Recording>,
    rx: Receiver<Event>,
    cmds: Receiver<Cmd>,
}

impl Rig {
    fn new(cols: u16, rows: u16) -> Rig {
        let (tx, rx) = channel();
        let (handle, cmds) = ConnHandle::detached();
        let rest = Arc::new(Recording::default());
        let remote = RemoteState::new("test".into(), None, handle, rest.clone(), None).inline();
        let mut app = App::new_remote(options(), tx, rows, cols, remote);
        app.handle(Event::Stream(StreamEvent::Welcome {
            client_id: ClientId("me".into()),
            server: ServerInfo {
                version: "0".into(),
                name: "test".into(),
            },
            scope: Scope::Full,
        }));
        app.handle(Event::Stream(StreamEvent::Status(
            crate::conn::stream::ConnStatus::Connected,
        )));
        app.handle(Event::Stream(StreamEvent::State(Box::new(
            state_with_terms(),
        ))));
        Rig {
            app,
            rest,
            rx,
            cmds,
        }
    }

    fn keys(&mut self, bytes: &[u8]) {
        self.app.handle(Event::Input(bytes.to_vec()));
        self.pump();
    }

    fn prefix(&mut self, bytes: &[u8]) {
        self.keys(&[PREFIX, bytes].concat());
    }

    /// Delivers the answers of REST calls to the app.
    fn pump(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            self.app.handle(ev);
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.rest.calls.lock().unwrap().clone()
    }

    fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut *self.rest.calls.lock().unwrap())
    }

    fn names(&self) -> Vec<String> {
        self.app.windows.iter().map(|w| w.name()).collect()
    }

    fn select(&mut self, n: Node) {
        self.app.remote.as_mut().unwrap().vs.selection = Some(n);
    }

    fn draw(&self, cols: u16, rows: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        term.draw(|f| crate::ui::draw(f, &self.app)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn plan() -> PlanId {
    fixture::plan_id()
}

fn docs_site() -> PlanId {
    PlanId::new(2026, 10, 17, "docs-site")
}

fn t2() -> TaskId {
    TaskId::new(2)
}

fn post(path: impl Into<String>, body: Value) -> Call {
    Call::Post(path.into(), body)
}

#[test]
fn windows_are_built_from_the_mirror() {
    let rig = Rig::new(120, 30);
    assert_eq!(
        rig.names(),
        [
            "executor",
            "T-0001 schema",
            "T-0002 api",
            "T-0004 perms",
            "zsh",
            "lazygit"
        ]
    );
    let r = rig.app.remote.as_ref().unwrap();
    assert_eq!(r.plan.as_ref(), Some(&plan()));
    let t2 = &rig.app.windows[2];
    let labels: Vec<String> = t2.panes.iter().map(|p| p.name()).collect();
    assert_eq!(labels, ["T-0002", "tests", "docs"]);
    assert!(t2.panes.iter().all(|p| p.remote_term().is_some()));
    // The sub-agents are stacked beside the task agent (main-vertical).
    assert_eq!(t2.layout.leaves().len(), 3);
    // Terminals of the other windows are subscribed too.
    assert_eq!(
        rig.app.windows[4].panes[0].kind,
        Kind::Command("zsh".into())
    );
    // A state change keeps the window the user is on.
    let mut rig = rig;
    rig.app.select_window(3);
    let mut sv = state_with_terms();
    sv.rev += 1;
    rig.app
        .handle(Event::Stream(StreamEvent::State(Box::new(sv))));
    assert_eq!(rig.app.active, 3);
}

#[test]
fn a_new_agent_appears_and_a_gone_terminal_disappears() {
    let mut rig = Rig::new(120, 30);
    let mut sv = state_with_terms();
    // T-0004's terminal is gone: its window goes.
    sv.terminals.retain(|t| t.id != TermId(15));
    sv.rev += 1;
    rig.app
        .handle(Event::Stream(StreamEvent::State(Box::new(sv))));
    assert!(!rig.names().iter().any(|n| n.contains("T-0004")));
    assert_eq!(rig.names().len(), 5);
}

#[test]
fn prefix_d_detaches_and_never_asks_or_calls_the_server() {
    let mut rig = Rig::new(120, 30);
    rig.prefix(b"d");
    assert!(rig.app.quit);
    assert!(rig.calls().is_empty());
    rig.app.shutdown();
    // Detaching does not kill the program: no stop, no delete.
    assert!(rig.calls().is_empty());

    // Local mode still asks first (there is no server to keep the sessions).
    let (tx, _rx) = channel();
    let mut local = App::new(options(), tx, 30, 120);
    local.handle(Event::Input(b"\x02d".to_vec()));
    assert!(!local.quit);
    assert!(matches!(
        local.mode,
        Mode::Prompt(Prompt {
            kind: PromptKind::ConfirmQuit,
            ..
        })
    ));
}

#[test]
fn tree_keys_become_rest_calls() {
    let mut rig = Rig::new(120, 30);
    rig.app.set_tree_focus(true);

    // A on a draft plan accepts it.
    rig.select(Node::Plan(docs_site()));
    rig.keys(b"A");
    assert_eq!(
        rig.take_calls(),
        [post(
            format!("/v1/plans/{}/accept", docs_site()),
            Value::Null
        )]
    );

    // S pauses a running plan, and resumes a paused one.
    rig.select(Node::Plan(plan()));
    rig.keys(b"S");
    assert_eq!(
        rig.take_calls(),
        [post(format!("/v1/plans/{}/pause", plan()), Value::Null)]
    );
    let mut sv = state_with_terms();
    sv.plans[0].paused = true;
    sv.rev += 1;
    rig.app
        .handle(Event::Stream(StreamEvent::State(Box::new(sv))));
    rig.keys(b"S");
    assert_eq!(
        rig.take_calls(),
        [post(format!("/v1/plans/{}/resume", plan()), Value::Null)]
    );

    // R resumes the agent's session; the agent id's slash is escaped.
    rig.select(Node::Task(plan(), t2()));
    rig.keys(b"R");
    assert_eq!(
        rig.take_calls(),
        [post(
            format!("/v1/agents/{}%2FT-0002/resume", plan()),
            Value::Null
        )]
    );

    // A on a task that has no agent starts it.
    rig.select(Node::Task(plan(), TaskId::new(3)));
    rig.keys(b"A");
    assert_eq!(
        rig.take_calls(),
        [post(
            format!("/v1/tasks/{}/T-0003/spawn", plan()),
            json!({"instructions": null, "force": false})
        )]
    );
    assert!(rig.app.remote.as_ref().unwrap().vs.focus == Focus::Tree);
}

#[test]
fn stop_and_cancel_ask_first() {
    let mut rig = Rig::new(120, 30);
    rig.app.set_tree_focus(true);
    rig.select(Node::Task(plan(), t2()));

    rig.keys(b"x");
    assert!(matches!(rig.app.mode, Mode::Prompt(_)));
    rig.keys(b"n");
    assert!(rig.calls().is_empty(), "declining sends nothing");
    assert!(matches!(rig.app.mode, Mode::Tree), "back to the tree");

    rig.keys(b"x");
    rig.keys(b"y");
    assert_eq!(
        rig.take_calls(),
        [post(
            format!("/v1/agents/{}%2FT-0002/stop", plan()),
            Value::Null
        )]
    );

    rig.keys(b"c");
    rig.keys(b"y");
    assert_eq!(
        rig.take_calls(),
        [Call::Patch(
            format!("/v1/tasks/{}/T-0002", plan()),
            json!({"state": "cancelled"}),
        )]
    );
}

#[test]
fn messages_queue_or_steer() {
    let mut rig = Rig::new(120, 30);
    rig.app.set_tree_focus(true);
    rig.select(Node::Task(plan(), t2()));
    let path = format!("/v1/agents/{}%2FT-0002/messages", plan());

    rig.keys(b"s");
    assert!(matches!(rig.app.mode, Mode::Form));
    rig.keys(b"use sqlx\r");
    assert_eq!(
        rig.take_calls(),
        [post(
            path.clone(),
            json!({"text": "use sqlx", "steer": false})
        )]
    );
    assert!(matches!(rig.app.mode, Mode::Tree));

    // Alt-Enter steers.
    rig.keys(b"s");
    rig.keys(b"stop that\x1b\r");
    assert_eq!(
        rig.take_calls(),
        [post(path, json!({"text": "stop that", "steer": true}))]
    );
}

#[test]
fn new_plan_and_task_forms_post_their_requests() {
    let mut rig = Rig::new(120, 30);
    rig.app.set_tree_focus(true);
    rig.keys(b"n");
    assert!(matches!(rig.app.mode, Mode::Form));
    rig.keys(b"ship it\t");
    rig.keys(b"\x15/srv/api\t");
    rig.keys(b"do the thing\x1b\r");
    assert_eq!(
        rig.take_calls(),
        [post(
            "/v1/plans",
            json!({
                "title": "ship it",
                "repos": [{"path": "/srv/api", "base_branch": null}],
                "request": "do the thing",
            })
        )]
    );

    rig.select(Node::Plan(plan()));
    rig.keys(b"t");
    rig.keys(b"fix login\x1b\r");
    let calls = rig.take_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let Call::Post(path, body) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(path, &format!("/v1/plans/{}/tasks", plan()));
    assert_eq!(body["title"], "fix login");

    // Escape cancels a form without a call.
    rig.keys(b"n");
    rig.keys(b"\x1b");
    assert!(rig.calls().is_empty());
    assert!(matches!(rig.app.mode, Mode::Tree));
}

#[test]
fn needs_you_overlay_answers_items() {
    let mut rig = Rig::new(120, 30);
    rig.prefix(b"a");
    assert!(matches!(rig.app.mode, Mode::Inbox));
    // The permission is the oldest item.
    rig.keys(b"y");
    rig.keys(b"Y");
    assert_eq!(
        rig.take_calls(),
        [
            post(
                "/v1/items/1/answer",
                json!({"action": "allow", "text": null})
            ),
            post(
                "/v1/items/1/answer",
                json!({"action": "allow_always", "text": null})
            ),
        ]
    );
    // n asks for a note, then denies with it.
    rig.keys(b"n");
    assert!(matches!(rig.app.mode, Mode::Form));
    rig.keys(b"not now\r");
    assert_eq!(
        rig.take_calls(),
        [post(
            "/v1/items/1/answer",
            json!({"action": "deny", "text": "not now"})
        )]
    );
    assert!(matches!(rig.app.mode, Mode::Inbox), "back to the overlay");
    // r on the question replies.
    rig.keys(b"j");
    rig.keys(b"r");
    rig.keys(b"users first\r");
    assert_eq!(
        rig.take_calls(),
        [post(
            "/v1/items/2/answer",
            json!({"action": "reply", "text": "users first"})
        )]
    );
    rig.keys(b"\x1b");
    assert!(matches!(rig.app.mode, Mode::Normal));
}

#[test]
fn rest_errors_show_in_the_status_line() {
    let mut rig = Rig::new(120, 30);
    *rig.rest.reply.lock().unwrap() = Some(Err("error: plan is not draft (HTTP 409)".into()));
    rig.app.set_tree_focus(true);
    rig.select(Node::Plan(docs_site()));
    rig.keys(b"A");
    let msg = rig.app.message.as_ref().map(|(m, _)| m.clone());
    assert_eq!(msg.as_deref(), Some("error: plan is not draft (HTTP 409)"));
    assert!(rig.draw(120, 30).contains("plan is not draft"));
}

#[test]
fn new_windows_and_splits_post_terminals() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(4); // zsh
    rig.prefix(b"c");
    let calls = rig.take_calls();
    let Call::Post(path, body) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(path, "/v1/terminals");
    assert_eq!(body["kind"], "claude");
    assert_eq!(body["cwd"], "/home/me");

    rig.prefix(b"t");
    let calls = rig.take_calls();
    let Call::Post(_, body) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(body["kind"], "shell");
    assert_eq!(body["rows"], 28);
    assert_eq!(body["cols"], 90);
}

#[test]
fn a_split_becomes_a_pane_of_the_window() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(4); // zsh
    let reply = json!({"id": 30, "kind": "shell", "agent": null, "cwd": "/home/me",
        "rows": 24, "cols": 80, "size_owner": null, "title": null});
    *rig.rest.reply.lock().unwrap() = Some(Ok(reply));
    rig.prefix(b"%");
    let calls = rig.take_calls();
    assert!(
        matches!(&calls[0], Call::Post(p, _) if p == "/v1/terminals"),
        "{calls:?}"
    );
    // The server now lists the terminal.
    let mut sv = state_with_terms();
    sv.terminals.push(TerminalView {
        id: TermId(30),
        kind: TermKind::Shell,
        agent: None,
        cwd: "/home/me".into(),
        rows: 24,
        cols: 80,
        size_owner: None,
        title: None,
    });
    sv.loose.push(TermId(30));
    sv.rev += 1;
    rig.app
        .handle(Event::Stream(StreamEvent::State(Box::new(sv))));
    // It is a second pane of the zsh window, not a window of its own.
    assert_eq!(rig.app.windows.len(), 6);
    let w = &rig.app.windows[rig.app.active];
    assert_eq!(w.name(), "zsh");
    assert_eq!(w.panes.len(), 2);
    assert_eq!(w.pane().remote_term().unwrap().term, TermId(30));
}

#[test]
fn closing_a_loose_terminal_deletes_it() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(4);
    rig.prefix(b"x");
    rig.keys(b"y");
    assert_eq!(rig.take_calls(), [Call::Delete("/v1/terminals/1".into())]);
}

#[test]
fn stopping_a_pane_with_an_agent_stops_the_agent() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2); // T-0002
    rig.prefix(b"x");
    rig.keys(b"y");
    assert_eq!(
        rig.take_calls(),
        [post(
            format!("/v1/agents/{}%2FT-0002/stop", plan()),
            Value::Null
        )]
    );
}

#[test]
fn prefix_tab_toggles_tree_focus_and_keys_follow() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2);
    rig.prefix(b"\t");
    assert!(matches!(rig.app.mode, Mode::Tree));
    // The selection starts at the pane's agent.
    let sel = rig.app.remote.as_ref().unwrap().vs.selection.clone();
    assert_eq!(sel, Some(Node::Task(plan(), t2())));
    // Keys in the tree do not reach the pane.
    rig.keys(b"j");
    assert!(!rig.cmds.try_iter().any(|c| matches!(c, Cmd::Input { .. })));
    rig.prefix(b"\t");
    assert!(matches!(rig.app.mode, Mode::Normal));
    // In pane focus, keys are forwarded byte for byte.
    rig.keys(b"hi");
    let sent: Vec<u8> = rig
        .cmds
        .try_iter()
        .filter_map(|c| match c {
            Cmd::Input { data, .. } => Some(data),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(sent, b"hi");
}

#[test]
fn prefix_equals_takes_the_terminal_size() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2);
    let term = rig.app.windows[2].pane().remote_term().unwrap().term;
    let _ = rig.cmds.try_iter().count();
    rig.prefix(b"=");
    let cmds: Vec<Cmd> = rig.cmds.try_iter().collect();
    assert!(cmds.contains(&Cmd::Focus(term)), "{cmds:?}");
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Cmd::Resize { term: t, .. } if *t == term)),
        "{cmds:?}"
    );
}

#[test]
fn focusing_a_pane_focuses_its_terminal() {
    let mut rig = Rig::new(120, 30);
    let _ = rig.cmds.try_iter().count();
    rig.app.select_window(4);
    rig.keys(b""); // any input runs the focus sync
    rig.app.after_input();
    let cmds: Vec<Cmd> = rig.cmds.try_iter().collect();
    assert!(cmds.contains(&Cmd::Focus(TermId(1))), "{cmds:?}");
}

#[test]
fn draws_the_tree_tabs_connection_dot_and_size_marks() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2);
    let screen = rig.draw(120, 30);
    assert!(screen.contains("test ● auth-refactor ▸"), "{screen}");
    assert!(screen.contains("T-0002 api"));
    assert!(screen.contains("plans"));
    assert!(screen.contains("needs you"));
    // The right of the status line: connection dot and counts.
    assert!(screen.contains("2 need you"), "{screen}");

    // Another client owns T-0002's terminal: its pane header says so.
    let mut sv = state_with_terms();
    sv.terminals
        .iter_mut()
        .find(|t| t.id == TermId(12))
        .unwrap()
        .size_owner = Some(ClientId("phone".into()));
    sv.rev += 1;
    rig.app
        .handle(Event::Stream(StreamEvent::State(Box::new(sv))));
    let screen = rig.draw(120, 30);
    assert!(screen.contains("⇲ 80×24"), "{screen}");
}

#[test]
fn narrow_screens_show_the_strip_and_prefix_tab_the_full_tree() {
    let mut rig = Rig::new(80, 24);
    let screen = rig.draw(80, 24);
    // A 3-column strip: no tree titles.
    assert!(!screen.contains("auth-refactor   accepted"), "{screen}");
    assert_eq!(rig.app.body_area().x, 3);
    rig.prefix(b"\t");
    let screen = rig.draw(80, 24);
    assert!(screen.contains("plans"), "the full tree overlay:\n{screen}");
    assert!(screen.contains("T-0002"), "{screen}");
    // The overlay does not move the panes.
    assert_eq!(rig.app.body_area().x, 3);
    rig.keys(b"\x1b");
    assert!(matches!(rig.app.mode, Mode::Normal));
    let screen = rig.draw(80, 24);
    assert!(!screen.contains("plans"), "{screen}");
    // B hides the strip too.
    rig.prefix(b"B");
    assert_eq!(rig.app.body_area().x, 0);
}

#[test]
fn board_view_toggles_and_opens_a_task() {
    let mut rig = Rig::new(120, 30);
    rig.prefix(b"v");
    assert!(matches!(rig.app.mode, Mode::Board));
    let screen = rig.draw(120, 30);
    assert!(screen.contains("merged"), "{screen}");
    // The first card is T-0003, which has not started: no window.
    rig.keys(b"\r");
    assert!(matches!(rig.app.mode, Mode::Board));
    assert!(rig.app.message.is_some());
    rig.app.remote.as_mut().unwrap().vs.board_sel = Some(t2());
    rig.keys(b"\r");
    assert_eq!(rig.names()[rig.app.active], "T-0002 api");
    assert!(matches!(rig.app.mode, Mode::Normal));
    rig.prefix(b"v");
    rig.keys(b"v");
    assert!(matches!(rig.app.mode, Mode::Normal));
}

#[test]
fn mouse_clicks_select_rows_and_double_clicks_focus_the_pane() {
    let mut rig = Rig::new(120, 30);
    rig.app.set_tree_focus(true);
    let full = rig.app.screen_body();
    let r = rig.app.remote.as_ref().unwrap();
    let (rect, mode) = r.tree_place(full, true).unwrap();
    let target = Node::Task(plan(), TaskId::new(4));
    let cell = (rect.x..rect.right())
        .flat_map(|x| (rect.y..rect.bottom()).map(move |y| (x, y)))
        .find(|(x, y)| {
            matches!(
                crate::tui::hit::tree_hit(rect, mode, &r.mirror.sv, &r.vs, *x, *y),
                Some(TreeHit::Row(n)) if n == target
            )
        })
        .expect("T-0004 is on screen");
    let click = format!(
        "\x1b[<0;{};{}M\x1b[<0;{};{}m",
        cell.0 + 1,
        cell.1 + 1,
        cell.0 + 1,
        cell.1 + 1
    );
    rig.keys(click.as_bytes());
    let r = rig.app.remote.as_ref().unwrap();
    assert_eq!(r.vs.selection.as_ref(), Some(&target));
    assert_eq!(rig.names()[rig.app.active], "T-0004 perms");
    assert!(
        matches!(rig.app.mode, Mode::Tree),
        "a click keeps tree focus"
    );
    // The second click on the same row focuses the pane.
    rig.keys(click.as_bytes());
    assert!(matches!(rig.app.mode, Mode::Normal));
}

#[test]
fn the_view_state_is_saved_and_restored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("view-test.toml");
    let (tx, _rx) = channel();
    let (handle, _cmds) = ConnHandle::detached();
    let rest = Arc::new(Recording::default());
    let remote = RemoteState::new(
        "test".into(),
        None,
        handle,
        rest.clone(),
        Some(path.clone()),
    );
    let mut app = App::new_remote(options(), tx, 30, 120, remote);
    app.handle(Event::Stream(StreamEvent::State(Box::new(
        state_with_terms(),
    ))));
    app.select_window(3);
    app.windows[3].custom_name = None;
    app.rename_remote("perms!");
    app.shutdown();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("perms!"), "{saved}");

    let (tx, _rx) = channel();
    let (handle, _cmds) = ConnHandle::detached();
    let remote = RemoteState::new("test".into(), None, handle, rest, Some(path));
    let mut app = App::new_remote(options(), tx, 30, 120, remote);
    app.handle(Event::Stream(StreamEvent::State(Box::new(
        state_with_terms(),
    ))));
    assert_eq!(app.windows[app.active].name(), "perms!");
    assert_eq!(app.active, 3);
}

#[test]
fn jump_to_the_next_item_that_needs_you() {
    let mut rig = Rig::new(120, 30);
    rig.keys(b"\x02\x1ba");
    // From the top of the tree the question of the executor comes first.
    assert_eq!(rig.names()[rig.app.active], "executor");
    rig.keys(b"\x02\x1ba");
    assert_eq!(rig.names()[rig.app.active], "T-0004 perms");
    rig.keys(b"\x02\x1bA");
    assert_eq!(rig.names()[rig.app.active], "executor");
}

#[test]
fn the_session_picker_lists_the_servers_sessions() {
    let mut rig = Rig::new(120, 30);
    *rig.rest.reply.lock().unwrap() = Some(Ok(json!({"sessions": [{
        "id": "0b9c2d1e-1111-4222-8333-444455556666", "cwd": "/srv/api", "title": "fix login",
        "first_prompt": "fix login", "last_prompt": null, "modified": 1_700_000_000u64,
        "branch": "main"}]})));
    rig.prefix(b"s");
    assert!(matches!(rig.app.mode, Mode::Picker));
    assert_eq!(rig.take_calls(), [Call::Get("/v1/projects".into())]);
    assert_eq!(rig.app.picker.sessions.len(), 1);
    assert_eq!(rig.app.picker.sessions[0].title, "fix login");
    assert!(!rig.app.picker.loading);
    // Enter resumes it on the server.
    *rig.rest.reply.lock().unwrap() = None;
    rig.keys(b"\r");
    let calls = rig.take_calls();
    let Call::Post(path, body) = &calls[0] else {
        panic!("{calls:?}")
    };
    assert_eq!(path, "/v1/terminals");
    assert_eq!(body["kind"], "claude");
    assert_eq!(
        body["args"],
        json!(["--resume", "0b9c2d1e-1111-4222-8333-444455556666"])
    );
    assert_eq!(body["cwd"], "/srv/api");
}

// ---------------------------------------------------------------------------
// Against a real cmuxd
// ---------------------------------------------------------------------------

mod live {
    use cmuxd::config::Config;
    use cmuxd::server::ServerBuilder;

    use super::*;
    use crate::conn::http::Http;
    use crate::conn::servers::Endpoint;
    use crate::tui::remote::HttpRest;

    fn wait_for(app: &mut App, rx: &Receiver<Event>, what: &str, f: impl Fn(&App) -> bool) {
        let end = Instant::now() + Duration::from_secs(15);
        while !f(app) {
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}; screen: {:?}",
                screen(app)
            );
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
                app.handle(ev);
            }
        }
    }

    fn attach(sock: &std::path::Path, view: &std::path::Path) -> (App, Receiver<Event>) {
        let (tx, rx) = channel();
        let ep = Endpoint::Unix(sock.to_path_buf());
        let rest = Arc::new(HttpRest(Http::new(ep.clone())));
        let mut remote = RemoteState::connect("live".into(), ep, rest, tx.clone());
        remote.view_path = Some(view.to_path_buf());
        let mut app = App::new_remote(options(), tx, 30, 120, remote);
        app.start();
        (app, rx)
    }

    fn screen(app: &App) -> String {
        app.active_pane()
            .map(|p| p.parser.lock().unwrap().screen().contents())
            .unwrap_or_default()
    }

    #[test]
    fn attach_create_type_detach_and_reattach() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: Some(tmp.path().join("data")),
            socket: Some(tmp.path().join("api.sock")),
            listen: Some("off".into()),
            ..Config::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt
            .block_on(ServerBuilder::new(cfg).tick(None).start())
            .unwrap();
        let view = tmp.path().join("view.toml");

        let (mut app, rx) = attach(&server.socket_path, &view);
        wait_for(&mut app, &rx, "connected", |a| {
            a.remote
                .as_ref()
                .is_some_and(|r| r.mirror.status == crate::conn::stream::ConnStatus::Connected)
        });
        // Empty server: the tree has focus.
        wait_for(&mut app, &rx, "tree focus", |a| {
            matches!(a.mode, Mode::Tree)
        });
        assert!(app.windows.is_empty());

        // A command window (`prefix :` new-window cat) is created on the server.
        app.handle(Event::Input(b"\x02:neww cat\r".to_vec()));
        wait_for(&mut app, &rx, "the cat window", |a| {
            a.windows.len() == 1 && matches!(a.mode, Mode::Normal)
        });
        app.handle(Event::Input(b"hello server\r".to_vec()));
        wait_for(&mut app, &rx, "the echo", |a| {
            screen(a).matches("hello server").count() == 2
        });

        // The server lists it.
        let sv: StateView = Http::new(Endpoint::Unix(server.socket_path.clone()))
            .get("/v1/state")
            .unwrap();
        assert_eq!(sv.terminals.len(), 1);

        // Detach: the client exits, the terminal keeps running.
        app.handle(Event::Input(b"\x02d".to_vec()));
        assert!(app.quit);
        app.shutdown();
        drop(app);
        assert!(server.app.hub.get_term(TermId(1)).is_some());

        // Re-attach: same window, same screen (replayed from the server).
        let (mut app, rx) = attach(&server.socket_path, &view);
        wait_for(&mut app, &rx, "the window again", |a| {
            screen(a).matches("hello server").count() == 2
        });
        assert_eq!(app.windows.len(), 1);

        // Closing the pane deletes the terminal on the server.
        app.handle(Event::Input(b"\x02x".to_vec()));
        app.handle(Event::Input(b"y".to_vec()));
        wait_for(&mut app, &rx, "the window to close", |a| {
            a.windows.is_empty()
        });
        app.shutdown();
        rt.block_on(server.shutdown());
    }

    #[test]
    fn agent_routes_with_an_escaped_slash_reach_the_handler() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: Some(tmp.path().join("data")),
            socket: Some(tmp.path().join("api.sock")),
            listen: Some("off".into()),
            ..Config::default()
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let server = rt
            .block_on(ServerBuilder::new(cfg).tick(None).start())
            .unwrap();
        let rest = HttpRest(Http::new(Endpoint::Unix(server.socket_path.clone())));
        // No such agent: the answer is the engine's "no agent", not a 404 from
        // the router, so the escaped slash was decoded into one path segment.
        let agent: AgentId = AgentId::new(plan(), AgentPath::Executor);
        let req = Pending::Stop(agent).request();
        let err = rest.send(&req.call).unwrap_err();
        assert!(err.contains("HTTP 404") || err.contains("HTTP 4"), "{err}");
        assert!(!err.contains("Not Found"), "{err}");
        rt.block_on(server.shutdown());
    }
}

#[test]
fn only_https_urls_reach_the_opener() {
    assert!(web_url("https://github.com/o/r/pull/1"));
    for bad in [
        "file:///etc/passwd",
        "smb://host/share",
        "http://example.com",
        "-oProxyCommand=x",
        "javascript:alert(1)",
        "https://x.com/\nfoo",
        "",
    ] {
        assert!(!web_url(bad), "{bad}");
    }
}

fn post_cwd(call: &Call) -> Option<String> {
    match call {
        Call::Post(_, body) => body["cwd"].as_str().map(String::from),
        c => panic!("{c:?}"),
    }
}

#[test]
fn a_local_server_gets_the_directory_even_before_the_first_state() {
    let mut rig = Rig::new(120, 30);
    rig.app.remote.as_mut().unwrap().local_server = true;
    rig.app.windows.clear();
    let spec = rig
        .app
        .claude_spec(PathBuf::from("/home/me/proj"), Vec::new(), None);
    rig.app.spawn_remote(spec, Place::Window);
    let calls = rig.take_calls();
    assert_eq!(post_cwd(&calls[0]).as_deref(), Some("/home/me/proj"));
}

#[test]
fn a_remote_server_never_gets_the_laptop_directory() {
    let mut rig = Rig::new(120, 30);
    // The fallback when no pane is focused is our own directory.
    let home = rig.app.home_cwd.clone();
    let spec = rig.app.claude_spec(home, Vec::new(), None);
    rig.app.spawn_remote(spec, Place::Window);
    // A pane's directory (a path on the server) is still sent.
    let spec = rig
        .app
        .claude_spec(PathBuf::from("/srv/app"), Vec::new(), None);
    rig.app.spawn_remote(spec, Place::Window);
    let calls = rig.take_calls();
    assert_eq!(post_cwd(&calls[0]), None);
    assert_eq!(post_cwd(&calls[1]).as_deref(), Some("/srv/app"));
}

#[test]
fn directories_on_the_command_line_are_flashed_away_on_a_remote_server() {
    let mut rig = Rig::new(120, 30);
    rig.app.opts.dirs = vec![PathBuf::from("/home/me/proj")];
    rig.app.start();
    assert!(rig.take_calls().is_empty());
    assert!(rig.app.message.as_ref().unwrap().0.contains("ignored"));
}

#[test]
fn a_failed_delete_after_a_loose_terminal_exits_stays_quiet() {
    let mut rig = Rig::new(120, 30);
    *rig.rest.reply.lock().unwrap() = Some(Err("404 not found".into()));
    rig.app.handle(Event::Stream(StreamEvent::Exit {
        term: TermId(1),
        code: 0,
        signal: None,
    }));
    rig.pump();
    assert_eq!(rig.take_calls(), [Call::Delete("/v1/terminals/1".into())]);
    assert!(rig.app.message.is_none());
    // A deliberate close still reports its failure.
    rig.app.select_window(4);
    rig.prefix(b"x");
    rig.keys(b"y");
    assert!(rig.app.message.is_some());
}

// ---- sub-agents and questions (step 9) ---------------------------------------

/// The fixture with `names` as the only sub-agents of T-0002, each with a
/// terminal, and `tweak` applied to the state.
fn state_with_subs(names: &[&str]) -> StateView {
    let mut sv = state_with_terms();
    let task = t2();
    let old: Vec<AgentId> = sv
        .agents
        .iter()
        .filter(|a| a.level == cmux_proto::view::Level::Sub)
        .map(|a| a.id.clone())
        .collect();
    sv.agents.retain(|a| !old.contains(&a.id));
    sv.terminals
        .retain(|t| t.agent.as_ref().is_none_or(|a| !old.contains(a)));
    let template = sv
        .agent(&AgentId::new(plan(), AgentPath::Task(task)))
        .cloned();
    let mut template = template.unwrap();
    let main_id = template.id.clone();
    for (i, name) in names.iter().enumerate() {
        let path = AgentPath::sub(task, name).unwrap();
        let id = AgentId::new(plan(), path.clone());
        let term = TermId(40 + i as u32);
        template.id = id.clone();
        template.level = path.level();
        template.parent = Some(main_id.clone());
        template.path = path;
        template.term = Some(term);
        template.report = None;
        sv.agents.push(template.clone());
        sv.terminals.push(TerminalView {
            id: term,
            kind: TermKind::Claude,
            agent: Some(id),
            cwd: "/home/me/api".into(),
            rows: 24,
            cols: 80,
            size_owner: None,
            title: None,
        });
    }
    let main = sv.plans.iter_mut().find(|p| p.id == plan()).unwrap();
    main.agents.retain(|a| !old.contains(a));
    main.agents.extend(
        names
            .iter()
            .map(|n| AgentId::new(plan(), AgentPath::sub(task, n).unwrap())),
    );
    sv.items
        .retain(|i| i.from.as_ref().is_none_or(|a| !old.contains(a)));
    sv
}

impl Rig {
    fn set_state(&mut self, mut sv: StateView) {
        sv.rev = self.app.remote.as_ref().unwrap().mirror.sv.rev + 1;
        self.app
            .handle(Event::Stream(StreamEvent::State(Box::new(sv))));
    }

    /// The pane bodies of the shown window, by pane name, as (x, y, w, h).
    fn pane_rects(&self) -> Vec<(String, ratatui::layout::Rect)> {
        let w = &self.app.windows[self.app.active];
        let area = self.app.body_area();
        w.layout
            .layout(area)
            .into_iter()
            .map(|(id, r)| (w.get(id).unwrap().name(), r))
            .collect()
    }

    fn active_agent(&self) -> Option<String> {
        let w = &self.app.windows[self.app.active];
        w.pane().agent.as_ref().map(ToString::to_string)
    }
}

#[test]
fn sub_agents_stack_beside_the_task_agent_in_a_main_vertical() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2);
    let names = ["tests", "docs", "lint"];
    for n in 1..=3 {
        rig.set_state(state_with_subs(&names[..n]));
        assert_eq!(rig.app.windows[rig.app.active].name(), "T-0002 api");
        let rects = rig.pane_rects();
        assert_eq!(rects.len(), n + 1, "{rects:?}");
        let (main_name, main) = &rects[0];
        assert_eq!(main_name, "T-0002");
        let area = rig.app.body_area();
        // Main is the whole left column.
        assert_eq!((main.x, main.y, main.height), (area.x, area.y, area.height));
        assert!(main.width > 0 && main.right() < area.right());
        // Sub-agents are stacked in the right column, in spawn order.
        let mut y = area.y;
        for (i, (name, r)) in rects[1..].iter().enumerate() {
            assert_eq!(name, names[i]);
            assert!(r.x > main.x + main.width - 1 && r.right() == area.right());
            assert_eq!(r.y, y, "{name} follows the one above");
            y = r.bottom();
        }
        assert_eq!(y, area.bottom(), "the stack fills the column");
    }
    // A sub-agent leaves (its terminal is gone): the rest stay main-vertical.
    rig.set_state(state_with_subs(&names[..2]));
    let rects = rig.pane_rects();
    assert_eq!(rects.len(), 3);
    assert!(rects[1].1.x > rects[0].1.x && rects[1].1.x == rects[2].1.x);
}

#[test]
fn a_user_chosen_layout_is_kept_until_a_sub_agent_joins() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2);
    rig.set_state(state_with_subs(&["tests"]));
    // The user picks even-vertical (prefix M-2 is the preset key; set it directly).
    {
        let w = &mut rig.app.windows[2];
        let ids = w.layout.leaves();
        w.layout = crate::layout::Node::preset("even-vertical", &ids).unwrap();
        w.preset = Some(1);
    }
    // The same panes: the layout stays.
    let before = rig.pane_rects();
    rig.set_state(state_with_subs(&["tests"]));
    assert_eq!(rig.pane_rects(), before);
    // A new sub-agent re-applies the user's preset, not main-vertical.
    rig.set_state(state_with_subs(&["tests", "docs"]));
    let rects = rig.pane_rects();
    assert_eq!(rects.len(), 3);
    assert!(rects.iter().all(|(_, r)| r.x == rects[0].1.x), "{rects:?}");
}

#[test]
fn a_finished_sub_agent_is_drawn_dimmed_with_its_report() {
    let mut rig = Rig::new(120, 30);
    rig.app.select_window(2);
    let mut sv = state_with_subs(&["tests", "docs"]);
    // docs: exited after a successful report.
    let docs = sv
        .agents
        .iter_mut()
        .find(|a| a.id.to_string().ends_with("/docs"))
        .unwrap();
    docs.life = cmux_proto::view::Life::Exited { code: Some(0) };
    docs.report = Some(cmux_proto::view::Report {
        status: cmux_proto::view::ReportStatus::Success,
        summary: "done".into(),
        pr_url: None,
        attempt: 1,
        implicit: false,
        supersedes_blocked: false,
    });
    rig.set_state(sv);
    let w = &rig.app.windows[2];
    let dim: Vec<(String, bool)> = w.panes.iter().map(|p| (p.name(), p.dim)).collect();
    assert_eq!(
        dim,
        [
            ("T-0002".into(), false),
            ("tests".into(), false),
            ("docs".into(), true)
        ]
    );

    let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
    term.draw(|f| crate::ui::draw(f, &rig.app)).unwrap();
    let buf = term.backend().buffer().clone();
    let row = |y: u16| -> String { (0..120).map(|x| buf[(x, y)].symbol()).collect() };
    let text: Vec<String> = (0..30).map(row).collect();
    let all = text.join("\n");
    assert!(all.contains("tests · /home/me/api"), "{all}");
    assert!(all.contains("docs · reported success"), "{all}");

    let area = rig.app.body_area();
    let w = &rig.app.windows[2];
    for a in w.areas(area) {
        let name = w.get(a.id).unwrap().name();
        let cell = &buf[(a.body.x, a.body.y)];
        let header = &buf[(a.header.unwrap().x + 1, a.header.unwrap().y)];
        assert_eq!(
            cell.modifier.contains(ratatui::style::Modifier::DIM),
            name == "docs",
            "body of {name}"
        );
        // The header of a finished pane is not bold.
        assert_eq!(
            header.modifier.contains(ratatui::style::Modifier::BOLD),
            name == "T-0002",
            "header of {name}"
        );
    }
}

#[test]
fn prefix_u_goes_sub_agent_to_task_to_executor() {
    let mut rig = Rig::new(120, 30);
    let sub = AgentId::new(plan(), AgentPath::sub(t2(), "tests").unwrap());
    rig.app.show_node(&Node::Sub(sub.clone()));
    assert_eq!(rig.active_agent().unwrap(), sub.to_string());
    rig.prefix(b"u");
    assert_eq!(rig.names()[rig.app.active], "T-0002 api");
    assert_eq!(rig.active_agent().unwrap(), format!("{}/T-0002", plan()));
    rig.prefix(b"u");
    assert_eq!(rig.names()[rig.app.active], "executor");
    assert_eq!(rig.active_agent().unwrap(), format!("{}/executor", plan()));
    // The top: stays put.
    rig.prefix(b"u");
    assert_eq!(rig.names()[rig.app.active], "executor");
}

#[test]
fn closing_a_finished_sub_agent_pane_hides_it_for_good() {
    let mut rig = Rig::new(120, 30);
    let mut sv = state_with_subs(&["tests", "docs"]);
    sv.agents
        .iter_mut()
        .find(|a| a.id.to_string().ends_with("/docs"))
        .unwrap()
        .life = cmux_proto::view::Life::Exited { code: Some(0) };
    rig.set_state(sv.clone());
    let docs = AgentId::new(plan(), AgentPath::sub(t2(), "docs").unwrap());
    rig.app.show_node(&Node::Sub(docs.clone()));
    assert_eq!(rig.active_agent().unwrap(), docs.to_string());
    rig.take_calls();
    rig.prefix(b"x");
    rig.keys(b"y");
    // Closing is local: no request, the pane is gone and remembered.
    assert_eq!(rig.take_calls(), []);
    let r = rig.app.remote.as_ref().unwrap();
    assert!(r.view.closed.contains(&docs.to_string()));
    let w = &rig.app.windows[rig.app.active];
    assert_eq!(w.panes.len(), 2);
    assert!(w.panes.iter().all(|p| p.agent.as_ref() != Some(&docs)));
    // A later state does not bring it back.
    rig.set_state(sv);
    assert_eq!(rig.app.windows[rig.app.active].panes.len(), 2);
    // A running sub-agent is stopped, not closed.
    let tests = AgentId::new(plan(), AgentPath::sub(t2(), "tests").unwrap());
    rig.app.show_node(&Node::Sub(tests.clone()));
    rig.prefix(b"x");
    rig.keys(b"y");
    assert_eq!(
        rig.take_calls(),
        [post(
            format!("/v1/agents/{}%2FT-0002%2Ftests/stop", plan()),
            Value::Null
        )]
    );
}

#[test]
fn a_sub_agents_question_is_replied_to_from_the_overlay() {
    let mut rig = Rig::new(120, 30);
    let mut sv = state_with_subs(&["tests"]);
    let sub = AgentId::new(plan(), AgentPath::sub(t2(), "tests").unwrap());
    sv.items.clear();
    sv.items.push(cmux_proto::view::ItemView {
        id: cmux_proto::view::ItemId(9),
        to: cmux_proto::view::Recipient::Human,
        kind: cmux_proto::view::ItemKind::Question,
        from: Some(sub.clone()),
        body: "Mock the clock?".into(),
        status: cmux_proto::view::Delivery::Pending,
        created: 500,
        needs_human: true,
    });
    rig.set_state(sv);
    // The sub-agent's pane header shows it needs you.
    let w = &rig.app.windows[2];
    let sub_pane = w
        .panes
        .iter()
        .find(|p| p.agent.as_ref() == Some(&sub))
        .unwrap();
    assert_eq!(
        rig.app.remote.as_ref().unwrap().pane_glyph(sub_pane),
        Some(cmux_core::glyph::Glyph::NeedsYou)
    );
    rig.prefix(b"a");
    assert!(matches!(rig.app.mode, Mode::Inbox));
    assert!(rig.draw(120, 30).contains("Mock the clock?"));
    rig.take_calls();
    rig.keys(b"r");
    assert!(matches!(rig.app.mode, Mode::Form));
    rig.keys(b"yes, freeze it\r");
    assert_eq!(
        rig.take_calls(),
        [post(
            "/v1/items/9/answer",
            json!({"action": "reply", "text": "yes, freeze it"})
        )]
    );
    // Enter jumps to the asking sub-agent's pane.
    rig.keys(b"\r");
    assert!(matches!(rig.app.mode, Mode::Normal));
    assert_eq!(rig.active_agent().unwrap(), sub.to_string());
}
