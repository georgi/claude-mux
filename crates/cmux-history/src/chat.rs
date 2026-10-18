//! Transcript lines as chat entries (architecture 5.3), and an incremental
//! reader that follows one transcript file as Claude appends to it.
//!
//! The line rules are shared with the session picker: meta lines, thinking
//! blocks and `<`-wrapped injections (system reminders, slash-command echoes,
//! local command output) are skipped, and a `<pasted_content>` wrapper is
//! unwrapped, so a pasted digest pointer reads as the one line it is.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use cmux_proto::view::{ChatEntry, ChatKind, ChatRole, ChatTool, Timestamp, TokenUsage};
use serde_json::Value;

/// Longest tool result text in an entry, in chars including the `…`.
pub const TOOL_RESULT_CHARS: usize = 400;

/// Longest input summary shown to humans, in chars including the `…`.
pub const SUMMARY_CHARS: usize = 60;

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The text a permission rule is matched against, untruncated: the whole
/// command for `Bash`, the path or URL for the file and web tools, and the
/// compact JSON for anything else (MCP tools).
pub fn match_input(tool: &str, input: &Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Edit" | "Write" | "MultiEdit" | "Read" => "file_path",
        "NotebookEdit" => "notebook_path",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Glob" | "Grep" => "pattern",
        _ => "",
    };
    match str_of(input, key) {
        Some(s) if !key.is_empty() => s.to_string(),
        _ => input.to_string(),
    }
}

/// `match_input` on one line, cut to 60 chars with a trailing `…`.
pub fn summarize(full: &str) -> String {
    cut(&crate::one_line(full), SUMMARY_CHARS)
}

/// `s` cut to `max` chars, the last one a `…` when anything was cut.
pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Replaces each `<pasted_content id="..">…</pasted_content id="..">` block
/// with what was pasted (spike S3: Claude wraps a paste of 4+ lines).
pub fn unwrap_pasted(s: &str) -> String {
    const OPEN: &str = "<pasted_content";
    const CLOSE: &str = "</pasted_content";
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(OPEN) {
        let Some(open_end) = rest[start..].find('>').map(|i| start + i + 1) else {
            break;
        };
        let Some(close) = rest[open_end..].find(CLOSE).map(|i| open_end + i) else {
            break;
        };
        let close_end = rest[close..]
            .find('>')
            .map_or(rest.len(), |i| close + i + 1);
        out.push_str(&rest[..start]);
        out.push_str(rest[open_end..close].trim_matches('\n'));
        rest = &rest[close_end..];
    }
    out.push_str(rest);
    out
}

/// What a human wrote in a user message, trimmed, with pastes unwrapped.
/// `None` for injected text: anything that starts with `<` (system reminders,
/// command echoes, local command output) and command caveats.
pub fn human_text(raw: &str) -> Option<String> {
    let text = unwrap_pasted(raw);
    let t = text.trim();
    if t.is_empty() || t.starts_with('<') || t.starts_with("Caveat:") {
        return None;
    }
    Some(t.to_string())
}

/// Milliseconds since the epoch of an RFC 3339 UTC time as Claude writes it
/// (`2026-10-18T09:00:01.101Z`); 0 when it does not parse.
pub fn parse_ts(s: &str) -> Timestamp {
    fn num(s: &str) -> Option<i64> {
        s.parse().ok()
    }
    let parse = || -> Option<Timestamp> {
        let (date, time) = s.split_once('T')?;
        let mut d = date.splitn(3, '-');
        let (y, m, day) = (num(d.next()?)?, num(d.next()?)?, num(d.next()?)?);
        let time = time.trim_end_matches('Z');
        let mut t = time.splitn(3, ':');
        let (h, mi) = (num(t.next()?)?, num(t.next()?)?);
        let secs = t.next()?;
        let (sec, frac) = secs.split_once('.').unwrap_or((secs, ""));
        let sec = num(sec)?;
        let ms = format!("{frac:0<3}").get(..3).and_then(num).unwrap_or(0);
        // Days from the civil date (Hinnant).
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        let total = ((days * 24 + h) * 60 + mi) * 60 + sec;
        u64::try_from(total * 1000 + ms).ok()
    };
    parse().unwrap_or(0)
}

/// Tool names and summaries by `tool_use` id, so a result names its tool.
pub type ToolNames = HashMap<String, ChatTool>;

/// What one transcript line holds.
#[derive(Debug, Default, PartialEq)]
pub struct Line {
    pub entries: Vec<ChatEntry>,
    /// An assistant message's id and its token usage. Claude writes one line
    /// per content block and repeats the usage on each, so count an id once.
    pub usage: Option<(String, TokenUsage)>,
}

fn usage_of(v: &Value) -> TokenUsage {
    let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_write: n("cache_creation_input_tokens"),
    }
}

/// The text of a tool result's `content`: a string, or the text parts of a list.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match str_of(p, "type") {
                Some("text") => str_of(p, "text").unwrap_or("").to_string(),
                Some("image") => "[image]".to_string(),
                _ => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Parses one transcript line. `tools` remembers tool calls for the results
/// that follow them.
pub fn parse_line(line: &str, tools: &mut ToolNames) -> Line {
    let mut out = Line::default();
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return out;
    };
    let role = match str_of(&v, "type") {
        Some("user") => ChatRole::User,
        Some("assistant") => ChatRole::Assistant,
        _ => return out,
    };
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("isMeta") || flag("isSidechain") || flag("isCompactSummary") {
        return out;
    }
    let Some(msg) = v.get("message") else {
        return out;
    };
    if role == ChatRole::Assistant {
        if str_of(msg, "model") == Some("<synthetic>") {
            return out;
        }
        if let (Some(id), Some(u)) = (str_of(msg, "id"), msg.get("usage")) {
            out.usage = Some((id.to_string(), usage_of(u)));
        }
    }
    let uuid = str_of(&v, "uuid").unwrap_or("").to_string();
    let ts = str_of(&v, "timestamp").map_or(0, parse_ts);
    let mut push = |kind: ChatKind, text: String, tool: Option<ChatTool>| {
        let n = out.entries.len();
        out.entries.push(ChatEntry {
            uuid: if n == 0 {
                uuid.clone()
            } else {
                format!("{uuid}#{n}")
            },
            ts,
            role,
            kind,
            text,
            tool,
        });
    };
    let blocks = match msg.get("content") {
        Some(Value::String(s)) => {
            let text = match role {
                ChatRole::User => human_text(s),
                ChatRole::Assistant => Some(s.trim().to_string()).filter(|t| !t.is_empty()),
            };
            if let Some(t) = text {
                push(ChatKind::Text, t, None);
            }
            return out;
        }
        Some(Value::Array(b)) => b,
        _ => return out,
    };
    for b in blocks {
        match (role, str_of(b, "type")) {
            (ChatRole::User, Some("text")) => {
                if let Some(t) = str_of(b, "text").and_then(human_text) {
                    push(ChatKind::Text, t, None);
                }
            }
            (ChatRole::Assistant, Some("text")) => {
                let t = str_of(b, "text").unwrap_or("").trim();
                if !t.is_empty() {
                    push(ChatKind::Text, t.to_string(), None);
                }
            }
            (ChatRole::Assistant, Some("tool_use")) => {
                let name = str_of(b, "name").unwrap_or("").to_string();
                let full = match_input(&name, b.get("input").unwrap_or(&Value::Null));
                let tool = ChatTool {
                    summary: summarize(&full),
                    name,
                };
                if let Some(id) = str_of(b, "id") {
                    tools.insert(id.to_string(), tool.clone());
                }
                push(
                    ChatKind::ToolCall,
                    cut(&full, TOOL_RESULT_CHARS),
                    Some(tool),
                );
            }
            (ChatRole::User, Some("tool_result")) => {
                let text = result_text(b.get("content").unwrap_or(&Value::Null));
                let tool = str_of(b, "tool_use_id").and_then(|id| tools.get(id).cloned());
                push(ChatKind::ToolResult, cut(&text, TOOL_RESULT_CHARS), tool);
            }
            // Thinking, redacted thinking, images, documents.
            _ => {}
        }
    }
    out
}

/// Follows one transcript file: each [`read_new`](Self::read_new) parses the
/// bytes appended since the last one. A line Claude has not finished writing
/// waits in a buffer until its newline arrives.
#[derive(Debug)]
pub struct TranscriptReader {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    tools: ToolNames,
    counted: HashSet<String>,
    /// Token usage of the assistant messages read so far.
    pub tokens: TokenUsage,
    /// The text of the last assistant text entry read so far.
    pub last_text: Option<String>,
}

impl TranscriptReader {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        TranscriptReader {
            path: path.into(),
            offset: 0,
            partial: Vec::new(),
            tools: ToolNames::new(),
            counted: HashSet::new(),
            tokens: TokenUsage::default(),
            last_text: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Bytes consumed so far, the partial line included.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Parses what was appended since the last call. A file that does not
    /// exist yet has no entries. A file shorter than what was read was
    /// rewritten: reading starts over (its entries come again).
    pub fn read_new(&mut self) -> std::io::Result<Vec<ChatEntry>> {
        let mut f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let len = f.metadata()?.len();
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        if len == self.offset {
            return Ok(Vec::new());
        }
        f.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::with_capacity((len - self.offset) as usize);
        f.take(len - self.offset).read_to_end(&mut buf)?;
        self.offset += buf.len() as u64;
        Ok(self.feed(&buf))
    }

    /// Parses `bytes` as the next part of the file.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<ChatEntry> {
        self.partial.extend_from_slice(bytes);
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let complete: Vec<u8> = self.partial.drain(..=end).collect();
        let mut entries = Vec::new();
        for raw in complete.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(raw);
            if line.trim().is_empty() {
                continue;
            }
            let parsed = parse_line(&line, &mut self.tools);
            if let Some((id, u)) = parsed.usage
                && self.counted.insert(id)
            {
                self.tokens.input += u.input;
                self.tokens.output += u.output;
                self.tokens.cache_read += u.cache_read;
                self.tokens.cache_write += u.cache_write;
            }
            for e in &parsed.entries {
                if e.role == ChatRole::Assistant && e.kind == ChatKind::Text {
                    self.last_text = Some(e.text.clone());
                }
            }
            entries.extend(parsed.entries);
        }
        entries
    }
}

/// Where Claude keeps the transcript of `session_id` started in `cwd`:
/// `<claude dir>/projects/<cwd, every char but [A-Za-z0-9] as '-'>/<id>.jsonl`.
pub fn transcript_file(cwd: &Path, session_id: &str) -> PathBuf {
    let dir: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    crate::claude_dir()
        .join("projects")
        .join(dir)
        .join(format!("{session_id}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pastes_unwrap_and_injections_drop() {
        let p = "\n\n<pasted_content id=\"fea3\">\nline 1\nline 2\n</pasted_content id=\"fea3\">\n";
        assert_eq!(human_text(p).as_deref(), Some("line 1\nline 2"));
        let mixed = "look: <pasted_content id=\"a\">\nx\n</pasted_content id=\"a\"> ok";
        assert_eq!(human_text(mixed).as_deref(), Some("look: x ok"));
        assert_eq!(human_text("<system-reminder>x</system-reminder>"), None);
        assert_eq!(human_text("Caveat: local"), None);
        assert_eq!(human_text("  "), None);
        // An unclosed wrapper is left as is (and is then an injection).
        assert_eq!(human_text("<pasted_content id=\"a\">x"), None);
    }

    #[test]
    fn cuts_and_summaries() {
        assert_eq!(cut("abc", 3), "abc");
        assert_eq!(cut("abcd", 3), "ab…");
        assert_eq!(summarize("a\n  b\tc"), "a b c");
        assert_eq!(
            match_input("mcp__x__y", &serde_json::json!({"a": 1})),
            r#"{"a":1}"#
        );
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_ts("1970-01-01T00:00:00.000Z"), 0);
        assert_eq!(parse_ts("2026-10-18T09:00:01.101Z"), 1_792_314_001_101);
        assert_eq!(parse_ts("2026-10-18T09:00:01Z"), 1_792_314_001_000);
        assert_eq!(parse_ts("garbage"), 0);
    }

    #[test]
    fn transcript_file_follows_claudes_naming() {
        let p = transcript_file(
            Path::new("/Users/mg/dev/claude-mux/.worktrees/T-0005"),
            "s1",
        );
        assert!(
            p.ends_with("projects/-Users-mg-dev-claude-mux--worktrees-T-0005/s1.jsonl"),
            "{}",
            p.display()
        );
    }
}
