//! The chat parser and the incremental reader on fixtures cut from real
//! Claude Code transcripts (`fixtures/session.jsonl`: identifiers, paths and
//! text replaced, structure kept).

use std::io::Write;

use cmux_history::TranscriptReader;
use cmux_history::chat::{TOOL_RESULT_CHARS, ToolNames, parse_line};
use cmux_proto::view::{ChatEntry, ChatKind, ChatRole, TokenUsage};

const SESSION: &str = include_str!("fixtures/session.jsonl");

/// The fixture's lines by what they hold, in file order.
fn line(kind: &str) -> &'static str {
    let i = [
        "file-history-snapshot",
        "user",
        "attachment",
        "thinking",
        "tool_use",
        "tool_result",
        "assistant",
        "meta",
        "local_stdout",
        "compact_boundary",
        "compact_summary",
        "system_reminder",
        "pasted",
        "assistant_2",
        "ai_title",
        "last_prompt",
        "clear",
    ]
    .iter()
    .position(|k| *k == kind)
    .unwrap();
    SESSION.lines().nth(i).unwrap()
}

fn parse(kind: &str) -> Vec<ChatEntry> {
    parse_line(line(kind), &mut ToolNames::new()).entries
}

#[test]
fn user_and_assistant_text() {
    let u = parse("user");
    assert_eq!(u.len(), 1);
    assert_eq!(
        (u[0].role, u[0].kind, u[0].text.as_str()),
        (
            ChatRole::User,
            ChatKind::Text,
            "Fix the flaky test in parser.rs"
        )
    );
    assert_eq!(u[0].uuid, "00000000-0000-4000-8000-000000000001");
    assert!(u[0].ts > 1_792_000_000_000, "{}", u[0].ts);
    assert!(u[0].tool.is_none());

    let a = parse_line(line("assistant"), &mut ToolNames::new());
    assert_eq!(a.entries.len(), 1);
    assert_eq!(a.entries[0].role, ChatRole::Assistant);
    assert!(a.entries[0].text.starts_with("All 40 parser tests pass."));
    let (id, usage) = a.usage.unwrap();
    assert_eq!(id, "msg_B");
    assert!(usage.output > 0 && usage.cache_read > 0, "{usage:?}");
}

#[test]
fn tool_use_and_tool_result() {
    let mut tools = ToolNames::new();
    let call = parse_line(line("tool_use"), &mut tools).entries;
    assert_eq!(call.len(), 1);
    assert_eq!(call[0].kind, ChatKind::ToolCall);
    assert_eq!(call[0].text, "cargo test -p parser 2>&1 | tail -40");
    let tool = call[0].tool.clone().unwrap();
    assert_eq!(
        (tool.name.as_str(), tool.summary.as_str()),
        ("Bash", "cargo test -p parser 2>&1 | tail -40")
    );

    let result = parse_line(line("tool_result"), &mut tools).entries;
    assert_eq!(result.len(), 1);
    let r = &result[0];
    assert_eq!((r.role, r.kind), (ChatRole::User, ChatKind::ToolResult));
    // Cut to 400 chars, the last one an ellipsis.
    assert_eq!(r.text.chars().count(), TOOL_RESULT_CHARS);
    assert!(r.text.starts_with("test parser::case_000 ... ok") && r.text.ends_with('…'));
    // The result names the call it answers.
    assert_eq!(r.tool.as_ref().unwrap().name, "Bash");
    // Without the call, the result has no tool.
    assert!(parse("tool_result")[0].tool.is_none());
}

#[test]
fn thinking_meta_injections_and_bookkeeping_are_skipped() {
    for kind in [
        "file-history-snapshot",
        "attachment",
        "thinking",
        "meta",
        "local_stdout",
        "system_reminder",
        "ai_title",
        "last_prompt",
    ] {
        assert_eq!(parse(kind), vec![], "{kind}");
    }
    // A thinking line still carries its message's usage.
    let t = parse_line(line("thinking"), &mut ToolNames::new());
    assert_eq!(t.usage.map(|(id, _)| id).as_deref(), Some("msg_A"));
}

#[test]
fn compaction_is_skipped() {
    assert_eq!(parse("compact_boundary"), vec![]);
    assert_eq!(parse("compact_summary"), vec![]);
}

#[test]
fn clear_is_skipped() {
    assert!(line("clear").contains("<command-name>/clear</command-name>"));
    assert_eq!(parse("clear"), vec![]);
}

#[test]
fn pasted_content_is_unwrapped_to_one_line() {
    assert!(line("pasted").contains("pasted_content"));
    let p = parse("pasted");
    assert_eq!(p.len(), 1);
    assert_eq!(
        p[0].text,
        "[cmux] 2 new messages (I-12, I-13): read /data/digests/digest-12.md"
    );
    assert_eq!(p[0].role, ChatRole::User);
}

fn texts(entries: &[ChatEntry]) -> Vec<(ChatKind, String)> {
    entries.iter().map(|e| (e.kind, e.text.clone())).collect()
}

#[test]
fn the_whole_session_in_one_read() {
    let mut r = TranscriptReader::new("/nonexistent");
    let all = r.feed(SESSION.as_bytes());
    let kinds: Vec<ChatKind> = all.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            ChatKind::Text,
            ChatKind::ToolCall,
            ChatKind::ToolResult,
            ChatKind::Text,
            ChatKind::Text,
            ChatKind::Text,
        ]
    );
    assert_eq!(
        r.last_text.as_deref(),
        Some("Read both messages; T-0002 is done.")
    );
    // msg_A spans the thinking and the tool_use line: counted once.
    let usage = |kind: &str| {
        parse_line(line(kind), &mut ToolNames::new())
            .usage
            .unwrap()
            .1
    };
    let (a, b, c) = (usage("thinking"), usage("assistant"), usage("assistant_2"));
    assert_eq!(
        r.tokens,
        TokenUsage {
            input: a.input + b.input + c.input,
            output: a.output + b.output + c.output,
            cache_read: a.cache_read + b.cache_read + c.cache_read,
            cache_write: a.cache_write + b.cache_write + c.cache_write,
        }
    );
}

#[test]
fn partial_lines_wait_for_their_newline() {
    let whole = TranscriptReader::new("/x").feed(SESSION.as_bytes());
    for chunk in [1, 7, 100, 4096] {
        let mut r = TranscriptReader::new("/x");
        let mut got = Vec::new();
        for part in SESSION.as_bytes().chunks(chunk) {
            got.extend(r.feed(part));
        }
        assert_eq!(texts(&got), texts(&whole), "chunk {chunk}");
        assert_eq!(got, whole, "chunk {chunk}");
    }
    // Half a line gives nothing until the rest arrives.
    let user = line("user");
    let mut r = TranscriptReader::new("/x");
    let (a, b) = user.split_at(user.len() / 2);
    assert_eq!(r.feed(a.as_bytes()), vec![]);
    assert_eq!(r.feed(b.as_bytes()), vec![]);
    assert_eq!(r.feed(b"\n").len(), 1);
}

#[test]
fn read_new_follows_a_growing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let mut r = TranscriptReader::new(&path);
    // Not there yet: nothing, no error.
    assert_eq!(r.read_new().unwrap(), vec![]);

    let mut f = std::fs::File::create(&path).unwrap();
    let user = line("user");
    let (a, b) = user.split_at(40);
    f.write_all(a.as_bytes()).unwrap();
    assert_eq!(r.read_new().unwrap(), vec![]);
    assert_eq!(r.offset(), 40);
    write!(f, "{b}\n{}\n", line("thinking")).unwrap();
    let got = r.read_new().unwrap();
    assert_eq!(
        texts(&got),
        [(ChatKind::Text, "Fix the flaky test in parser.rs".into())]
    );
    // Nothing new: nothing.
    assert_eq!(r.read_new().unwrap(), vec![]);
    writeln!(f, "{}\n{}", line("tool_use"), line("assistant")).unwrap();
    let got = r.read_new().unwrap();
    assert_eq!(got.len(), 2);
    assert!(r.last_text.as_deref().unwrap().starts_with("All 40"));

    // A rewritten (shorter) file is read from the start again.
    std::fs::write(&path, format!("{}\n", line("pasted"))).unwrap();
    let got = r.read_new().unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].text.starts_with("[cmux] 2 new messages"));
}
