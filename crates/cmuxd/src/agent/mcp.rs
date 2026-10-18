//! `cmuxd mcp`: the stdio MCP server an agent talks to (architecture 7.3).
//!
//! JSON-RPC 2.0, one message per line, no SDK. It answers `initialize`,
//! `tools/list`, `tools/call` and `ping`, and ignores notifications. The tool
//! list depends on the agent's level, which comes from `CMUX_AGENT`; each
//! `tools/call` is forwarded to `/v1/agent/tool` on the Unix socket, where the
//! engine authenticates the token and enforces the role again. Results are
//! short plain text; errors start with `error:` and set `isError`.
//!
//! stdout carries protocol messages only. Claude starts this server once per
//! session with `CMUX_SOCK`, `CMUX_AGENT` and `CMUX_TOKEN` from `mcp.json`.

use std::io::{BufRead, Write};
use std::time::Duration;

use cmux_core::ids::AgentId;
use cmux_core::model::Level;
use cmux_proto::api::{AgentToolReq, AgentToolResp, ErrorBody, route};
use serde_json::{Value, json};

use super::hook::HookEnv;
use crate::client::Endpoint;

/// The newest protocol revision this server speaks.
const LATEST_PROTOCOL: &str = "2025-06-18";
const PROTOCOLS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// How long one tool call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

use Level::{Executor, Sub, Task};

/// One tool: its name, what it does, its arguments and who may call it
/// (the table in architecture 7.3).
struct ToolDef {
    name: &'static str,
    about: &'static str,
    /// `(name, type, description)`; a type ending in `?` is optional. Types:
    /// `string`, `integer`, `boolean`, `strings`.
    args: &'static [(&'static str, &'static str, &'static str)],
    levels: &'static [Level],
}

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "get_plan",
        about: "Show the plan: its state, repos, limits and the task list.",
        args: &[],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "update_plan",
        about: "Change the plan: set its state (for example proposed once the task list is ready, or done when every task is merged or cancelled) or its title.",
        args: &[
            (
                "state",
                "string?",
                "draft, proposed, accepted, done or cancelled",
            ),
            ("title", "string?", "New title"),
        ],
        levels: &[Executor],
    },
    ToolDef {
        name: "list_tasks",
        about: "List every task with its state, dependencies and criteria progress.",
        args: &[],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "get_task",
        about: "Show one task in full: body, criteria, notes, report and agent. Defaults to your own task.",
        args: &[("task", "string?", "Task id such as T-0003")],
        levels: &[Executor, Task, Sub],
    },
    ToolDef {
        name: "create_task",
        about: "Add a task to the plan.",
        args: &[
            ("title", "string", "Short title"),
            (
                "body",
                "string?",
                "What to do, with enough context to start cold",
            ),
            (
                "repo",
                "integer?",
                "Index of the plan's repo, from 0 (default 0)",
            ),
            ("deps", "strings?", "Task ids this task waits for"),
            ("criteria", "strings?", "Acceptance criteria"),
        ],
        levels: &[Executor],
    },
    ToolDef {
        name: "update_task",
        about: "Edit a task's title, body, repo or dependencies.",
        args: &[
            ("task", "string", "Task id such as T-0003"),
            ("title", "string?", "New title"),
            ("body", "string?", "New body"),
            ("repo", "integer?", "New repo index"),
            (
                "deps",
                "strings?",
                "New dependency list (replaces the old one)",
            ),
        ],
        levels: &[Executor],
    },
    ToolDef {
        name: "transition_task",
        about: "Move a task to another state. Task agents may move their own task only to testing or blocked.",
        args: &[
            ("task", "string", "Task id such as T-0003"),
            (
                "to",
                "string",
                "todo, in_progress, testing, failing, passing, merged, blocked or cancelled",
            ),
        ],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "add_criterion",
        about: "Add an acceptance criterion to a task.",
        args: &[
            ("task", "string", "Task id such as T-0003"),
            ("text", "string", "The criterion"),
        ],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "check_criterion",
        about: "Check or uncheck a criterion. Check one only with evidence.",
        args: &[
            ("task", "string", "Task id such as T-0003"),
            (
                "criterion",
                "integer",
                "Number of the criterion, counting from 1 as get_task lists them",
            ),
            ("checked", "boolean?", "false to uncheck (default true)"),
        ],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "add_note",
        about: "Attach a note to a task, for whoever works on it next.",
        args: &[
            ("task", "string", "Task id such as T-0003"),
            ("text", "string", "The note"),
        ],
        levels: &[Executor, Task, Sub],
    },
    ToolDef {
        name: "spawn_task",
        about: "Start the agent for a task whose dependencies are done. Refused over a limit; use force only when the user asked for it.",
        args: &[
            ("task", "string", "Task id such as T-0003"),
            (
                "instructions",
                "string?",
                "Extra instructions for the task agent",
            ),
            ("force", "boolean?", "Spawn even if the task is not ready"),
        ],
        levels: &[Executor],
    },
    ToolDef {
        name: "spawn_subagent",
        about: "Start a sub-agent in your worktree for separable work on files you are not touching.",
        args: &[
            (
                "name",
                "string",
                "Short lowercase name, letters, digits and dashes",
            ),
            (
                "prompt",
                "string",
                "The job, with enough context to start cold",
            ),
        ],
        levels: &[Task],
    },
    ToolDef {
        name: "send",
        about: "Message one of your agents. It arrives when the agent is next idle, or at once with interrupt.",
        args: &[
            (
                "agent",
                "string",
                "The agent, such as T-0003 or T-0003/lint",
            ),
            ("text", "string", "The message"),
            ("interrupt", "boolean?", "Stop what it is doing first"),
        ],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "stop_agent",
        about: "Stop one of your agents.",
        args: &[(
            "agent",
            "string",
            "The agent, such as T-0003 or T-0003/lint",
        )],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "list_agents",
        about: "List your agents with their state and pending messages.",
        args: &[],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "peek",
        about: "Read the last lines of one of your agents' terminal.",
        args: &[
            (
                "agent",
                "string",
                "The agent, such as T-0003 or T-0003/lint",
            ),
            ("lines", "integer?", "How many lines (default 40)"),
        ],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "set_task_pr",
        about: "Record the pull request you opened for your task.",
        args: &[(
            "url",
            "string",
            "https://github.com/<owner>/<repo>/pull/<number>",
        )],
        levels: &[Task],
    },
    ToolDef {
        name: "report_result",
        about: "Finish with one report: success, blocked or failed, then end your turn. A blocked report may be followed by one later report once you are unblocked; success and failed are final.",
        args: &[
            ("status", "string", "success, blocked or failed"),
            ("summary", "string", "What happened, with evidence"),
            ("pr_url", "string?", "The pull request, if you opened one"),
        ],
        levels: &[Task, Sub],
    },
    ToolDef {
        name: "ask_parent",
        about: "Ask the agent that started you a question. The answer arrives as a message after you end your turn.",
        args: &[("question", "string", "The question")],
        levels: &[Task, Sub],
    },
    ToolDef {
        name: "answer_child",
        about: "Answer a question one of your agents asked.",
        args: &[
            ("agent", "string", "The agent that asked"),
            ("answer", "string", "The answer"),
        ],
        levels: &[Executor, Task],
    },
    ToolDef {
        name: "ask_user",
        about: "Ask the human a question, only for decisions a human must make. The answer arrives as a message after you end your turn.",
        args: &[("question", "string", "The question")],
        levels: &[Executor, Task],
    },
];

fn level_name(l: Level) -> &'static str {
    match l {
        Executor => "executor",
        Task => "task",
        Sub => "sub-agent",
    }
}

/// The names of the tools `level` may call, in listing order.
pub fn tool_names(level: Level) -> Vec<&'static str> {
    TOOLS
        .iter()
        .filter(|t| t.levels.contains(&level))
        .map(|t| t.name)
        .collect()
}

/// The levels allowed to call `name`, if there is such a tool.
pub fn tool_levels(name: &str) -> Option<&'static [Level]> {
    TOOLS.iter().find(|t| t.name == name).map(|t| t.levels)
}

fn input_schema(t: &ToolDef) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, ty, about) in t.args {
        let (ty, optional) = match ty.strip_suffix('?') {
            Some(base) => (base, true),
            None => (*ty, false),
        };
        let mut p = match ty {
            "strings" => json!({"type": "array", "items": {"type": "string"}}),
            other => json!({"type": other}),
        };
        p["description"] = json!(about);
        props.insert(name.to_string(), p);
        if !optional {
            required.push(*name);
        }
    }
    json!({"type": "object", "properties": props, "required": required})
}

/// The `tools/list` result for `level`.
pub fn tools_list(level: Option<Level>) -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .filter(|t| level.is_some_and(|l| t.levels.contains(&l)))
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.about,
                "inputSchema": input_schema(t),
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Runs a tool somewhere: `(text, is_error)`.
pub trait ToolCaller {
    fn call(&mut self, name: &str, args: &Value) -> (String, bool);
}

impl<F: FnMut(&str, &Value) -> (String, bool)> ToolCaller for F {
    fn call(&mut self, name: &str, args: &Value) -> (String, bool) {
        self(name, args)
    }
}

fn rpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// Answers one JSON-RPC message; `None` for notifications and responses.
pub fn handle(level: Option<Level>, msg: &Value, caller: &mut impl ToolCaller) -> Option<Value> {
    // No id: a notification (`notifications/initialized`, `notifications/cancelled`).
    let id = msg.get("id")?;
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A response to a request we never sent.
        return None;
    };
    let params = msg.get("params").unwrap_or(&Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|v| PROTOCOLS.contains(v))
                .unwrap_or(LATEST_PROTOCOL);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "cmux", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, tools_list(level)),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let (text, is_error) = call_tool(level, name, &args, caller);
            rpc_result(id, tool_result(&text, is_error))
        }
        other => rpc_error(id, -32601, &format!("method not found: {other}")),
    })
}

fn call_tool(
    level: Option<Level>,
    name: &str,
    args: &Value,
    caller: &mut impl ToolCaller,
) -> (String, bool) {
    let Some(levels) = tool_levels(name) else {
        let known = level.map(tool_names).unwrap_or_default().join(", ");
        return (
            format!("error: unknown tool {name:?}; available: {known}"),
            true,
        );
    };
    match level {
        Some(l) if levels.contains(&l) => caller.call(name, args),
        _ => {
            let who = level.map_or("unidentified", level_name);
            let allowed: Vec<&str> = levels.iter().map(|l| level_name(*l)).collect();
            (
                format!(
                    "error: {name} is not available to {who} agents; it is available to: {}",
                    allowed.join(", ")
                ),
                true,
            )
        }
    }
}

/// Reads requests until EOF, writing one response line per request.
pub fn serve(
    input: impl BufRead,
    out: &mut impl Write,
    level: Option<Level>,
    caller: &mut impl ToolCaller,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(level, &msg, caller),
            Err(e) => Some(rpc_error(
                &Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Forwards tool calls to the server over the agent's socket.
pub struct SocketCaller {
    env: Option<HookEnv>,
    rt: tokio::runtime::Runtime,
}

impl SocketCaller {
    pub fn new(env: Option<HookEnv>) -> std::io::Result<SocketCaller> {
        Ok(SocketCaller {
            env,
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        })
    }
}

impl ToolCaller for SocketCaller {
    fn call(&mut self, name: &str, args: &Value) -> (String, bool) {
        let Some(env) = &self.env else {
            return (
                "error: this MCP server has no agent identity (CMUX_SOCK, CMUX_AGENT, CMUX_TOKEN)"
                    .into(),
                true,
            );
        };
        let req = AgentToolReq {
            agent: env.agent.clone(),
            token: env.token.clone(),
            name: name.to_string(),
            args: args.clone(),
        };
        let ep = Endpoint::Unix(env.sock.clone());
        let sent = self.rt.block_on(async {
            tokio::time::timeout(CALL_TIMEOUT, ep.json("POST", route::AGENT_TOOL, &[], &req)).await
        });
        let resp = match sent {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                return (
                    format!(
                        "error: cannot reach cmuxd on {}: {e}; the server may be restarting, try again",
                        env.sock.display()
                    ),
                    true,
                );
            }
            Err(_) => return ("error: cmuxd did not answer in time".into(), true),
        };
        match resp.status {
            200 => match resp.json::<AgentToolResp>() {
                Ok(r) => (r.text, r.is_error),
                Err(e) => (format!("error: unreadable answer from cmuxd: {e}"), true),
            },
            401 => (
                "error: cmux refused this agent's token; the agent was stopped or has exited"
                    .into(),
                true,
            ),
            status => {
                let msg = resp
                    .json::<ErrorBody>()
                    .map_or_else(|_| resp.text(), |e| e.message);
                (format!("error: cmuxd answered {status}: {msg}"), true)
            }
        }
    }
}

/// The level of the agent named by `CMUX_AGENT`.
pub fn level_from_env() -> Option<Level> {
    let id = std::env::var("CMUX_AGENT").ok()?;
    id.parse::<AgentId>().ok().map(|a| a.path().level())
}

/// The whole subcommand: serves stdin to stdout until EOF.
pub fn run() -> Result<(), String> {
    let level = level_from_env();
    let mut caller = SocketCaller::new(HookEnv::from_env()).map_err(|e| e.to_string())?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    serve(stdin.lock(), &mut stdout, level, &mut caller).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::engine::ToolCall;
    use cmux_core::ids::PlanId;

    fn nothing(_: &str, _: &Value) -> (String, bool) {
        panic!("no tool should be called")
    }

    fn rpc(level: Option<Level>, msg: Value) -> Option<Value> {
        handle(level, &msg, &mut nothing)
    }

    #[test]
    fn initialize_ping_and_notifications() {
        let r = rpc(
            Some(Task),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}),
        )
        .unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "cmux");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        // An unknown revision gets the newest one we speak.
        let r = rpc(
            None,
            json!({"id":"a","method":"initialize","params":{"protocolVersion":"1999"}}),
        )
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], LATEST_PROTOCOL);
        assert_eq!(
            rpc(None, json!({"id":2,"method":"ping"})).unwrap()["result"],
            json!({})
        );
        assert!(rpc(None, json!({"method":"notifications/initialized"})).is_none());
        assert!(rpc(None, json!({"id":3,"result":{}})).is_none());
        let r = rpc(None, json!({"id":4,"method":"resources/list"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn tools_by_role() {
        let names = |l| tool_names(l);
        assert_eq!(
            names(Executor),
            [
                "get_plan",
                "update_plan",
                "list_tasks",
                "get_task",
                "create_task",
                "update_task",
                "transition_task",
                "add_criterion",
                "check_criterion",
                "add_note",
                "spawn_task",
                "send",
                "stop_agent",
                "list_agents",
                "peek",
                "answer_child",
                "ask_user"
            ]
        );
        assert_eq!(
            names(Task),
            [
                "get_plan",
                "list_tasks",
                "get_task",
                "transition_task",
                "add_criterion",
                "check_criterion",
                "add_note",
                "spawn_subagent",
                "send",
                "stop_agent",
                "list_agents",
                "peek",
                "set_task_pr",
                "report_result",
                "ask_parent",
                "answer_child",
                "ask_user"
            ]
        );
        assert_eq!(
            names(Sub),
            ["get_task", "add_note", "report_result", "ask_parent"]
        );
        let r = rpc(Some(Sub), json!({"id":1,"method":"tools/list"})).unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);
        let report = tools.iter().find(|t| t["name"] == "report_result").unwrap();
        assert_eq!(
            report["inputSchema"]["required"],
            json!(["status", "summary"])
        );
        assert_eq!(
            report["inputSchema"]["properties"]["pr_url"]["type"],
            "string"
        );
        // Without an identity there is nothing to offer.
        let r = rpc(None, json!({"id":1,"method":"tools/list"})).unwrap();
        assert_eq!(r["result"]["tools"], json!([]));
    }

    /// The table here is a copy of the engine's: they must agree.
    #[test]
    fn table_matches_the_engines_roles() {
        let plan = PlanId::new(2026, 10, 18, "demo");
        let sample = |name: &str| -> ToolCall {
            let args = json!({
                "task": "T-0001", "agent": "T-0001", "to": "testing", "title": "t", "text": "x",
                "criterion": 1, "name": "n", "prompt": "p", "url": "u", "status": "success",
                "summary": "s", "question": "q", "answer": "a",
            });
            super::super::wire::parse_tool(name, &args, &plan).unwrap()
        };
        for t in TOOLS {
            let call = sample(t.name);
            assert_eq!(call.name(), t.name);
            assert_eq!(call.roles(), t.levels, "{}", t.name);
        }
        assert_eq!(TOOLS.len(), 21);
    }

    #[test]
    fn calls_are_forwarded_or_refused() {
        let mut seen = Vec::new();
        let mut caller = |n: &str, a: &Value| {
            seen.push((n.to_string(), a.clone()));
            ("done".to_string(), false)
        };
        let call = |level, name: &str, caller: &mut dyn FnMut(&str, &Value) -> (String, bool)| {
            let mut c = |n: &str, a: &Value| caller(n, a);
            handle(
                level,
                &json!({"id":1,"method":"tools/call","params":{"name":name,"arguments":{"summary":"s"}}}),
                &mut c,
            )
            .unwrap()
        };
        let r = call(Some(Sub), "report_result", &mut caller);
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["content"][0]["text"], "done");
        let r = call(Some(Sub), "spawn_task", &mut caller);
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(
            r["result"]["content"][0]["text"],
            "error: spawn_task is not available to sub-agent agents; it is available to: executor"
        );
        let r = call(Some(Executor), "rm_rf", &mut caller);
        assert!(
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("error: unknown tool \"rm_rf\"; available: get_plan")
        );
        let r = call(None, "get_plan", &mut caller);
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "report_result");
    }

    #[test]
    fn serve_reads_lines_and_survives_garbage() {
        let input = "not json\n\n{\"id\":1,\"method\":\"ping\"}\n{\"method\":\"notifications/initialized\"}\n";
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, Some(Task), &mut nothing).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32700);
        assert_eq!(lines[1]["result"], json!({}));
    }

    #[test]
    fn level_comes_from_the_agent_id() {
        // Not through the environment (tests share it): the same parse.
        let lvl = |s: &str| s.parse::<cmux_core::ids::AgentId>().unwrap().path().level();
        assert_eq!(lvl("P-20261018-demo/executor"), Executor);
        assert_eq!(lvl("P-20261018-demo/T-0003"), Task);
        assert_eq!(lvl("P-20261018-demo/T-0003/lint"), Sub);
    }
}
