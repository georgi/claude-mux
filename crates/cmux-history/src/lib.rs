//! Reads Claude Code's conversation transcripts, `~/.claude/projects/<project>/<session-id>.jsonl`.
//! Shared by the `cmux` picker (local mode) and `cmuxd` (`GET /v1/projects`,
//! and the chat view through [`chat`]).

pub mod chat;

pub use chat::{TranscriptReader, human_text, parse_line, transcript_file};

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde_json::Value;

const HEAD_BYTES: u64 = 96 * 1024;
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub first_prompt: Option<String>,
    pub last_prompt: Option<String>,
    pub modified: SystemTime,
    pub branch: Option<String>,
}

pub fn claude_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    home().join(".claude")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Cache of parsed transcripts keyed by path, invalidated by (mtime, len).
#[derive(Default)]
pub struct Index {
    cache: HashMap<PathBuf, (SystemTime, u64, Option<SessionInfo>)>,
}

pub type SharedIndex = Arc<Mutex<Index>>;

/// Scan all transcripts. Unchanged files come from the cache. Runs on a worker thread.
pub fn scan(index: &SharedIndex) -> Vec<SessionInfo> {
    let mut files = Vec::new();
    if let Ok(projects) = std::fs::read_dir(claude_dir().join("projects")) {
        for p in projects.flatten() {
            let Ok(entries) = std::fs::read_dir(p.path()) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().is_some_and(|x| x == "jsonl")
                    && let Ok(md) = e.metadata()
                {
                    files.push((
                        path,
                        md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                        md.len(),
                    ));
                }
            }
        }
    }

    let todo: Vec<_> = {
        let idx = index.lock().unwrap();
        files
            .iter()
            .filter(
                |(p, m, l)| !matches!(idx.cache.get(p), Some((cm, cl, _)) if cm == m && cl == l),
            )
            .cloned()
            .collect()
    };

    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let chunk = todo.len().div_ceil(workers).max(1);
    let parsed: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = todo
            .chunks(chunk)
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|(p, m, l)| (p.clone(), *m, *l, parse_transcript(p, *m, *l)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });

    let mut idx = index.lock().unwrap();
    for (p, m, l, info) in parsed {
        idx.cache.insert(p, (m, l, info));
    }
    let mut out: Vec<SessionInfo> = files
        .iter()
        .filter_map(|(p, _, _)| idx.cache.get(p).and_then(|(_, _, i)| i.clone()))
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.modified));
    out
}

fn read_range(f: &mut File, start: u64, len: u64) -> String {
    let mut buf = Vec::with_capacity(len as usize);
    if f.seek(SeekFrom::Start(start)).is_ok() {
        let _ = f.take(len).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn parse_transcript(path: &Path, modified: SystemTime, len: u64) -> Option<SessionInfo> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    if id.len() != 36 {
        return None; // not a top-level session (e.g. agent sidechains)
    }
    let mut f = File::open(path).ok()?;
    let head = read_range(&mut f, 0, HEAD_BYTES);
    let tail = if len > HEAD_BYTES {
        read_range(
            &mut f,
            len.saturating_sub(TAIL_BYTES).max(HEAD_BYTES),
            TAIL_BYTES,
        )
    } else {
        String::new()
    };

    let mut cwd = None;
    let mut branch = None;
    let mut first_prompt = None;
    let mut custom_title = None;
    let mut ai_title = None;
    let mut last_prompt = None;

    for line in head.lines().chain(tail.lines()) {
        let is_user = line.contains("\"type\":\"user\"");
        let wants_cwd = cwd.is_none() && line.contains("\"cwd\":");
        let is_title =
            line.contains("\"type\":\"custom-title\"") || line.contains("\"type\":\"ai-title\"");
        let is_last = line.contains("\"type\":\"last-prompt\"");
        if !(wants_cwd || is_title || is_last || (is_user && first_prompt.is_none())) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(Value::as_str)
        {
            cwd = Some(PathBuf::from(c));
            branch = v
                .get("gitBranch")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        match v.get("type").and_then(Value::as_str) {
            Some("custom-title") => custom_title = str_field(&v, "customTitle").or(custom_title),
            Some("ai-title") => ai_title = str_field(&v, "aiTitle").or(ai_title),
            Some("last-prompt") => last_prompt = str_field(&v, "lastPrompt").or(last_prompt),
            Some("user")
                if first_prompt.is_none() && v.get("isMeta") != Some(&Value::Bool(true)) =>
            {
                first_prompt = user_text(&v);
            }
            _ => {}
        }
    }

    let cwd = cwd?;
    if first_prompt.is_none() && last_prompt.is_none() && custom_title.is_none() {
        return None; // empty session
    }
    let title = custom_title
        .or(ai_title)
        .or_else(|| first_prompt.clone())
        .or_else(|| last_prompt.clone())
        .unwrap_or_else(|| "(untitled)".into());
    Some(SessionInfo {
        id,
        cwd,
        title: one_line(&title),
        first_prompt,
        last_prompt,
        modified,
        branch,
    })
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Extract the human-written text of a user message, by the chat rules
/// ([`human_text`]). Skips tool results and injected wrappers.
fn user_text(v: &Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    human_text(&text).map(|t| one_line(&t))
}

pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
