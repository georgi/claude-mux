//! Claude's hook payloads and the model's tool arguments, as core types.
//!
//! Everything here is pure. Errors are `error: ...` strings meant for the
//! model (tools) or the log (hooks).

use cmux_core::engine::{Hook, NotificationKind, TaskPatch, ToolCall};
use cmux_core::ids::{AgentId, AgentPath, PlanId, TaskId};
use cmux_core::model::{PlanState, ReportStatus, TaskState};
use serde_json::Value;

/// Longest input summary shown to humans, in chars including the `…`.
const SUMMARY_CHARS: usize = 60;

/// Lines `peek` returns when the model names none.
const PEEK_DEFAULT_LINES: usize = 40;
/// The most lines `peek` returns.
const PEEK_MAX_LINES: usize = 400;

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
    let one: String = full.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= SUMMARY_CHARS {
        return one;
    }
    let mut s: String = one.chars().take(SUMMARY_CHARS - 1).collect();
    s.push('…');
    s
}

/// Claude's hook payload into a core [`Hook`]. `event` is the name the hook
/// binary was started with.
pub fn parse_hook(event: &str, p: &Value) -> Result<Hook, String> {
    let tool = || str_of(p, "tool_name").unwrap_or("").to_string();
    let input = || p.get("tool_input").cloned().unwrap_or(Value::Null);
    Ok(match event {
        "SessionStart" => Hook::SessionStart {
            session_id: str_of(p, "session_id").unwrap_or("").to_string(),
        },
        "UserPromptSubmit" => Hook::UserPromptSubmit {
            prompt: str_of(p, "prompt").unwrap_or("").to_string(),
        },
        "Stop" => Hook::Stop {
            stop_hook_active: p
                .get("stop_hook_active")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        "Notification" => Hook::Notification {
            kind: match str_of(p, "notification_type") {
                Some("permission_prompt") => NotificationKind::PermissionPrompt,
                Some("idle_prompt") => NotificationKind::IdlePrompt,
                _ => NotificationKind::Other,
            },
            message: str_of(p, "message").unwrap_or("").to_string(),
        },
        "PreToolUse" => {
            let tool = tool();
            let full = match_input(&tool, &input());
            Hook::PreToolUse {
                input_summary: summarize(&full),
                match_input: full,
                tool,
            }
        }
        "PermissionRequest" => {
            let tool = tool();
            let full = match_input(&tool, &input());
            Hook::PermissionRequest {
                // The payload has no tool_use_id (spike S6); one request is
                // held per agent at a time, so a fresh id is enough.
                request_id: uuid::Uuid::new_v4().to_string(),
                input_summary: summarize(&full),
                match_input: full,
                tool,
            }
        }
        "SessionEnd" => Hook::SessionEnd {
            clear: str_of(p, "reason") == Some("clear"),
        },
        other => return Err(format!("error: unknown hook event {other:?}")),
    })
}

// ---------------------------------------------------------------------------
// Tool arguments
// ---------------------------------------------------------------------------

type Res<T> = Result<T, String>;

fn arg_err(tool: &str, msg: impl std::fmt::Display) -> String {
    format!("error: {tool}: {msg}")
}

struct Args<'a> {
    tool: &'a str,
    v: &'a Value,
}

impl Args<'_> {
    fn raw(&self, key: &str) -> Option<&Value> {
        self.v.get(key).filter(|v| !v.is_null())
    }

    fn str(&self, key: &str) -> Res<String> {
        match self.raw(key) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(arg_err(self.tool, format!("`{key}` must be a string"))),
            None => Err(arg_err(self.tool, format!("`{key}` is required"))),
        }
    }

    fn opt_str(&self, key: &str) -> Res<Option<String>> {
        match self.raw(key) {
            None => Ok(None),
            Some(_) => self.str(key).map(Some),
        }
    }

    fn opt_bool(&self, key: &str) -> Res<Option<bool>> {
        match self.raw(key) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(arg_err(self.tool, format!("`{key}` must be true or false"))),
        }
    }

    fn opt_uint(&self, key: &str) -> Res<Option<usize>> {
        match self.raw(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .map(|n| Some(n as usize))
                .ok_or_else(|| arg_err(self.tool, format!("`{key}` must be a whole number"))),
        }
    }

    fn strings(&self, key: &str) -> Res<Option<Vec<String>>> {
        let bad = || arg_err(self.tool, format!("`{key}` must be a list of strings"));
        match self.raw(key) {
            None => Ok(None),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| x.as_str().map(str::to_string).ok_or_else(bad))
                .collect::<Res<Vec<_>>>()
                .map(Some),
            Some(_) => Err(bad()),
        }
    }

    fn task(&self, key: &str) -> Res<TaskId> {
        task_id(&self.str(key)?).map_err(|e| arg_err(self.tool, e))
    }

    fn opt_task(&self, key: &str) -> Res<Option<TaskId>> {
        match self.opt_str(key)? {
            None => Ok(None),
            Some(s) => task_id(&s).map(Some).map_err(|e| arg_err(self.tool, e)),
        }
    }

    fn task_list(&self, key: &str) -> Res<Option<Vec<TaskId>>> {
        match self.strings(key)? {
            None => Ok(None),
            Some(l) => l
                .iter()
                .map(|s| task_id(s))
                .collect::<Result<Vec<_>, _>>()
                .map(Some)
                .map_err(|e| arg_err(self.tool, e)),
        }
    }
}

/// `T-0003`, also `t-3`, `T-3` and `3`.
pub fn task_id(s: &str) -> Res<TaskId> {
    let t = s.trim();
    let digits = t
        .strip_prefix("T-")
        .or_else(|| t.strip_prefix("t-"))
        .unwrap_or(t);
    match digits.parse::<u32>() {
        Ok(n) if digits.bytes().all(|b| b.is_ascii_digit()) => Ok(TaskId::new(n)),
        _ => Err(format!(
            "{s:?} is not a task id; expected T-NNNN, e.g. T-0003"
        )),
    }
}

/// A full agent id, or a path inside the caller's plan (`executor`, `T-0003`,
/// `T-0003/<name>`).
pub fn agent_id(s: &str, plan: &PlanId) -> Res<AgentId> {
    let s = s.trim();
    if let Ok(id) = s.parse::<AgentId>() {
        return Ok(id);
    }
    let lenient = s.strip_prefix("t-").map(|r| format!("T-{r}"));
    s.parse::<AgentPath>()
        .ok()
        .or_else(|| lenient.and_then(|l| l.parse().ok()))
        .map(|path| AgentId::new(plan.clone(), path))
        .ok_or_else(|| format!("{s:?} is not an agent; expected executor, T-NNNN or T-NNNN/<name>"))
}

/// The tool name and arguments the model gave, as a core [`ToolCall`]. `plan`
/// is the caller's plan, which agent names are resolved in.
pub fn parse_tool(name: &str, args: &Value, plan: &PlanId) -> Res<ToolCall> {
    let a = Args {
        tool: name,
        v: args,
    };
    let agent =
        |key: &str| -> Res<AgentId> { agent_id(&a.str(key)?, plan).map_err(|e| arg_err(name, e)) };
    Ok(match name {
        "get_plan" => ToolCall::GetPlan,
        "update_plan" => ToolCall::UpdatePlan {
            state: a
                .opt_str("state")?
                .map(|s| s.parse::<PlanState>().map_err(|e| arg_err(name, e)))
                .transpose()?,
            title: a.opt_str("title")?,
        },
        "list_tasks" => ToolCall::ListTasks,
        "get_task" => ToolCall::GetTask {
            task: a.opt_task("task")?,
        },
        "create_task" => ToolCall::CreateTask {
            title: a.str("title")?,
            body: a.opt_str("body")?.unwrap_or_default(),
            repo: a.opt_uint("repo")?.unwrap_or(0),
            deps: a.task_list("deps")?.unwrap_or_default(),
            criteria: a.strings("criteria")?.unwrap_or_default(),
        },
        "update_task" => ToolCall::UpdateTask {
            task: a.task("task")?,
            patch: TaskPatch {
                title: a.opt_str("title")?,
                body: a.opt_str("body")?,
                repo: a.opt_uint("repo")?,
                deps: a.task_list("deps")?,
            },
        },
        "transition_task" => ToolCall::TransitionTask {
            task: a.task("task")?,
            to: a
                .str("to")?
                .parse::<TaskState>()
                .map_err(|e| arg_err(name, e))?,
        },
        "add_criterion" => ToolCall::AddCriterion {
            task: a.task("task")?,
            text: a.str("text")?,
        },
        "check_criterion" => ToolCall::CheckCriterion {
            task: a.task("task")?,
            criterion: a
                .opt_uint("criterion")?
                .ok_or_else(|| arg_err(name, "`criterion` is required (counts from 1)"))?,
            checked: a.opt_bool("checked")?.unwrap_or(true),
        },
        "add_note" => ToolCall::AddNote {
            task: a.task("task")?,
            text: a.str("text")?,
        },
        "spawn_task" => ToolCall::SpawnTask {
            task: a.task("task")?,
            instructions: a.opt_str("instructions")?,
            force: a.opt_bool("force")?.unwrap_or(false),
        },
        "spawn_subagent" => ToolCall::SpawnSubagent {
            name: a.str("name")?,
            prompt: a.str("prompt")?,
        },
        "send" => ToolCall::Send {
            agent: agent("agent")?,
            text: a.str("text")?,
            interrupt: a.opt_bool("interrupt")?.unwrap_or(false),
        },
        "stop_agent" => ToolCall::StopAgent {
            agent: agent("agent")?,
        },
        "list_agents" => ToolCall::ListAgents,
        "peek" => ToolCall::Peek {
            agent: agent("agent")?,
            lines: a
                .opt_uint("lines")?
                .unwrap_or(PEEK_DEFAULT_LINES)
                .clamp(1, PEEK_MAX_LINES),
        },
        "set_task_pr" => ToolCall::SetTaskPr { url: a.str("url")? },
        "report_result" => ToolCall::ReportResult {
            status: match a.str("status")?.as_str() {
                "success" => ReportStatus::Success,
                "blocked" => ReportStatus::Blocked,
                "failed" => ReportStatus::Failed,
                other => {
                    return Err(arg_err(
                        name,
                        format!("status {other:?} is not valid; use success, blocked or failed"),
                    ));
                }
            },
            summary: a.str("summary")?,
            pr_url: a.opt_str("pr_url")?,
        },
        "ask_parent" => ToolCall::AskParent {
            question: a.str("question")?,
        },
        "answer_child" => ToolCall::AnswerChild {
            agent: agent("agent")?,
            answer: a.str("answer")?,
        },
        "ask_user" => ToolCall::AskUser {
            question: a.str("question")?,
        },
        other => return Err(format!("error: unknown tool {other:?}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan() -> PlanId {
        PlanId::new(2026, 10, 18, "demo")
    }

    #[test]
    fn hook_payloads() {
        let h = |e: &str, p: Value| parse_hook(e, &p).unwrap();
        assert_eq!(
            h("SessionStart", json!({"session_id": "s1"})),
            Hook::SessionStart {
                session_id: "s1".into()
            }
        );
        assert_eq!(
            h("Stop", json!({"stop_hook_active": true})),
            Hook::Stop {
                stop_hook_active: true
            }
        );
        assert_eq!(
            h("Stop", json!({})),
            Hook::Stop {
                stop_hook_active: false
            }
        );
        assert_eq!(
            h("SessionEnd", json!({"reason": "clear"})),
            Hook::SessionEnd { clear: true }
        );
        assert_eq!(
            h(
                "Notification",
                json!({"notification_type": "idle_prompt", "message": "m"})
            ),
            Hook::Notification {
                kind: NotificationKind::IdlePrompt,
                message: "m".into()
            }
        );
        let long = "cargo test ".to_string() + &"x".repeat(100);
        let Hook::PermissionRequest {
            tool,
            input_summary,
            match_input,
            request_id,
        } = h(
            "PermissionRequest",
            json!({"tool_name": "Bash", "tool_input": {"command": long.clone()}}),
        )
        else {
            panic!()
        };
        assert_eq!(
            (tool.as_str(), match_input.as_str()),
            ("Bash", long.as_str())
        );
        assert_eq!(input_summary.chars().count(), 60);
        assert!(input_summary.ends_with('…') && !request_id.is_empty());
        let Hook::PreToolUse { match_input, .. } = h(
            "PreToolUse",
            json!({"tool_name": "Edit", "tool_input": {"file_path": "/a/b.rs", "old_string": "x"}}),
        ) else {
            panic!()
        };
        assert_eq!(match_input, "/a/b.rs");
        assert!(parse_hook("Nope", &json!({})).is_err());
    }

    #[test]
    fn summaries_are_one_line() {
        assert_eq!(summarize("a\n  b\tc"), "a b c");
        assert_eq!(match_input("mcp__x__y", &json!({"a": 1})), r#"{"a":1}"#);
        assert_eq!(
            match_input("Bash", &json!({"description": "d"})),
            r#"{"description":"d"}"#
        );
    }

    #[test]
    fn tool_arguments() {
        let p = plan();
        let t = |n: &str, a: Value| parse_tool(n, &a, &p);
        assert_eq!(t("get_plan", Value::Null).unwrap(), ToolCall::GetPlan);
        assert_eq!(
            t(
                "report_result",
                json!({"status": "success", "summary": "done", "pr_url": null})
            )
            .unwrap(),
            ToolCall::ReportResult {
                status: ReportStatus::Success,
                summary: "done".into(),
                pr_url: None
            }
        );
        assert_eq!(
            t(
                "create_task",
                json!({"title": "x", "deps": ["T-1", "T-0002"], "criteria": ["c"]})
            )
            .unwrap(),
            ToolCall::CreateTask {
                title: "x".into(),
                body: String::new(),
                repo: 0,
                deps: vec![TaskId::new(1), TaskId::new(2)],
                criteria: vec!["c".into()],
            }
        );
        assert_eq!(
            t("send", json!({"agent": "T-0003/lint", "text": "hi"})).unwrap(),
            ToolCall::Send {
                agent: AgentId::new(p.clone(), AgentPath::sub(TaskId::new(3), "lint").unwrap()),
                text: "hi".into(),
                interrupt: false
            }
        );
        assert_eq!(
            t("peek", json!({"agent": "executor", "lines": 100000})).unwrap(),
            ToolCall::Peek {
                agent: AgentId::new(p.clone(), AgentPath::Executor),
                lines: 400
            }
        );
        let e = t(
            "transition_task",
            json!({"task": "T-0001", "to": "sideways"}),
        )
        .unwrap_err();
        assert!(
            e.starts_with("error: transition_task:") && e.contains("expected one of"),
            "{e}"
        );
        let e = t("report_result", json!({"status": "great", "summary": "s"})).unwrap_err();
        assert!(e.contains("success, blocked or failed"), "{e}");
        let e = t("add_note", json!({"task": "banana", "text": "x"})).unwrap_err();
        assert!(e.contains("T-NNNN"), "{e}");
        let e = t("ask_user", json!({})).unwrap_err();
        assert_eq!(e, "error: ask_user: `question` is required");
        assert!(t("rm_rf", json!({})).unwrap_err().contains("unknown tool"));
    }
}
