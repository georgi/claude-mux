//! Reads Claude Code's on-disk state. Transcript parsing lives in
//! `cmux-history` (the server uses it too); this adds the live-session poll
//! and small path and time helpers.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

pub use cmux_history::{Index, SessionInfo, SharedIndex, claude_dir, home, scan};

/// A live Claude process as reported in ~/.claude/sessions/<pid>.json.
#[derive(Debug, Clone)]
pub struct LiveSession {
    pub pid: u32,
    pub session_id: String,
    pub status: Option<String>,
}

pub fn live_sessions() -> Vec<LiveSession> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(claude_dir().join("sessions")) else {
        return out;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(s) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&s) else {
            continue;
        };
        let Some(pid) = v.get("pid").and_then(Value::as_u64) else {
            continue;
        };
        let pid = pid as u32;
        // Stale files can outlive their process.
        if unsafe { libc::kill(pid as i32, 0) } != 0 {
            continue;
        }
        out.push(LiveSession {
            pid,
            session_id: v
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: v.get("status").and_then(Value::as_str).map(str::to_string),
        });
    }
    out
}

pub fn age(t: SystemTime) -> String {
    let s = SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        86400..604800 => format!("{}d", s / 86400),
        604800..2592000 => format!("{}w", s / 604800),
        _ => format!("{}mo", s / 2592000),
    }
}

pub fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        home()
    } else if let Some(rest) = s.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(s)
    }
}
