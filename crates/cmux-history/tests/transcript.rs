//! The chat parser and the incremental reader on fixtures cut from real
//! Claude Code transcripts (`fixtures/session.jsonl`: identifiers, paths and
//! text replaced, structure kept).

use std::io::Write;

use cmux_history::TranscriptReader;
use cmux_history::chat::{
    FULL_BYTES, INPUT_BYTES, TOOL_RESULT_CHARS, ToolNames, find_entry, parse_line, structured_input,
};
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
        "thinking_text",
        "edit_call",
        "edit_error",
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
            ChatKind::Thinking,
            ChatKind::ToolCall,
            ChatKind::ToolResult,
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
    // So does msg_D across the thinking and the Edit line.
    let (a, b, c, d) = (
        usage("thinking"),
        usage("assistant"),
        usage("assistant_2"),
        usage("thinking_text"),
    );
    assert_eq!(
        r.tokens,
        TokenUsage {
            input: a.input + b.input + c.input + d.input,
            output: a.output + b.output + c.output + d.output,
            cache_read: a.cache_read + b.cache_read + c.cache_read + d.cache_read,
            cache_write: a.cache_write + b.cache_write + c.cache_write + d.cache_write,
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

#[test]
fn results_pair_with_their_call_by_id() {
    let mut tools = ToolNames::new();
    let call = parse_line(line("tool_use"), &mut tools).entries;
    let result = parse_line(line("tool_result"), &mut tools).entries;
    assert_eq!(call[0].call_id.as_deref(), Some("toolu_A"));
    assert_eq!(result[0].call_id, call[0].call_id);
    // The id is there even when the call was never read.
    assert_eq!(parse("tool_result")[0].call_id.as_deref(), Some("toolu_A"));
    // Text and thinking have none.
    assert_eq!(parse("user")[0].call_id, None);
    assert_eq!(parse("thinking_text")[0].call_id, None);
    // Two calls in flight: each result finds its own.
    let edit = parse_line(line("edit_call"), &mut tools).entries;
    let err = parse_line(line("edit_error"), &mut tools).entries;
    assert_eq!(edit[0].call_id.as_deref(), Some("toolu_B"));
    assert_eq!(err[0].call_id.as_deref(), Some("toolu_B"));
    assert_eq!(err[0].tool.as_ref().unwrap().name, "Edit");
    // A result does not repeat the call's structured input.
    assert_eq!(err[0].tool.as_ref().unwrap().input, None);
}

#[test]
fn error_results_are_flagged() {
    let err = parse("edit_error");
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].kind, ChatKind::ToolResult);
    assert!(err[0].is_error);
    assert!(err[0].text.contains("String to replace not found"));
    // `"is_error": false` and no flag at all are both not an error.
    assert!(!parse("tool_result")[0].is_error);
    assert!(!parse("edit_call")[0].is_error);
}

#[test]
fn thinking_with_text_is_an_entry() {
    let t = parse("thinking_text");
    assert_eq!(t.len(), 1);
    assert_eq!(
        (t[0].role, t[0].kind),
        (ChatRole::Assistant, ChatKind::Thinking)
    );
    assert!(
        t[0].text
            .starts_with("The flake needs a per-test temp dir.")
    );
    assert_eq!(t[0].full_len, None);
    // Thinking is not the turn's last text.
    let mut r = TranscriptReader::new("/x");
    r.feed(format!("{}\n{}\n", line("assistant"), line("thinking_text")).as_bytes());
    assert!(r.last_text.as_deref().unwrap().starts_with("All 40"));
}

#[test]
fn assistant_text_names_its_model() {
    assert_eq!(
        parse("assistant")[0].model.as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(parse("user")[0].model, None);
    assert_eq!(parse("tool_use")[0].model, None);
    assert_eq!(parse("thinking_text")[0].model, None);
}

#[test]
fn edit_calls_carry_their_structured_input() {
    let e = parse("edit_call");
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].text, "/work/demo/src/parser.rs");
    let tool = e[0].tool.as_ref().unwrap();
    assert_eq!(tool.name, "Edit");
    assert_eq!(
        tool.input,
        Some(serde_json::json!({
            "file_path": "/work/demo/src/parser.rs",
            "old_string": "let dir = shared_tmp();",
            "new_string": "let dir = tempfile::tempdir()?;",
        }))
    );
    // Bash keeps its command and description.
    let bash = parse("tool_use");
    assert_eq!(
        bash[0].tool.as_ref().unwrap().input,
        Some(serde_json::json!({
            "command": "cargo test -p parser 2>&1 | tail -40",
            "description": "Run the parser tests",
        }))
    );
}

#[test]
fn structured_input_is_bounded() {
    let big = "é".repeat(INPUT_BYTES);
    let v = structured_input(
        "Write",
        &serde_json::json!({"file_path": "/a", "content": big, "mode": "x"}),
    )
    .unwrap();
    let content = v["content"].as_str().unwrap();
    assert!(content.len() <= INPUT_BYTES && content.len() > INPUT_BYTES - 4);
    assert_eq!(v["truncated"], true);
    assert_eq!(v["file_path"], "/a");
    assert!(v.get("mode").is_none());
    // Lists keep only the fields a view draws.
    let v = structured_input(
        "MultiEdit",
        &serde_json::json!({"file_path": "/a", "edits": [
            {"old_string": "a", "new_string": "b", "extra": 1}]}),
    )
    .unwrap();
    assert_eq!(
        v,
        serde_json::json!({"file_path": "/a", "edits": [{"old_string": "a", "new_string": "b"}]})
    );
    let v = structured_input(
        "Read",
        &serde_json::json!({"file_path": "/a", "offset": 10}),
    )
    .unwrap();
    assert_eq!(v, serde_json::json!({"file_path": "/a", "offset": 10}));
    // Unknown and MCP tools have none.
    assert_eq!(
        structured_input("mcp__x__y", &serde_json::json!({"a": 1})),
        None
    );
}

#[test]
fn cut_results_say_their_full_length_and_can_be_read_whole() {
    let result = parse("tool_result");
    let r = &result[0];
    assert_eq!(r.text.chars().count(), TOOL_RESULT_CHARS);
    // 40 lines of `test parser::case_NNN ... ok`, 39 newlines between them.
    let whole = 40 * "test parser::case_000 ... ok".len() + 39;
    assert_eq!(r.full_len, Some(whole as u32));
    // Short texts are not cut and have no length.
    assert_eq!(parse("edit_error")[0].full_len, None);
    assert_eq!(parse("user")[0].full_len, None);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, SESSION).unwrap();
    let full = find_entry(&path, &r.uuid).unwrap().unwrap();
    assert_eq!(full.text.chars().count(), whole);
    assert!(full.text.ends_with("case_039 ... ok"));
    assert_eq!(full.full_len, None);
    // The call read earlier in the file names the tool.
    assert_eq!(full.tool.as_ref().unwrap().name, "Bash");
    assert_eq!(full.call_id.as_deref(), Some("toolu_A"));
    // Unknown uuids find nothing.
    assert_eq!(find_entry(&path, "nope").unwrap(), None);
    assert_eq!(find_entry(&path, &format!("{}#1", r.uuid)).unwrap(), None);
}

#[test]
fn whole_entries_stop_at_a_megabyte() {
    let text = "x".repeat(FULL_BYTES + 10);
    let line = serde_json::json!({
        "type": "user", "uuid": "big", "timestamp": "2026-10-18T09:00:00.000Z",
        "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": text}]},
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    std::fs::write(&path, format!("{line}\n")).unwrap();
    let e = find_entry(&path, "big").unwrap().unwrap();
    assert_eq!(e.text.len(), FULL_BYTES);
    assert_eq!(e.full_len, Some((FULL_BYTES + 10) as u32));
}
