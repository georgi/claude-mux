//! `cmuxd mcp --session`: plan tools for a plain Claude session.
//!
//! The server gives every Claude terminal it starts (`prefix c`) this MCP
//! server, so the user can say "make a plan for X" in an ordinary session.
//! The session writes the plan and its tasks itself; no planner agent starts.
//! The plan stays a draft until the user accepts it, which starts the executor.
//!
//! Unlike `cmuxd mcp` there is no agent identity: each call goes to the public
//! REST API on the Unix socket (`CMUX_SOCK`), where the peer's uid is the
//! credential, exactly as for the local `cmux` client.

use std::path::{Path, PathBuf};

use cmux_proto::api::{CreatePlanReq, Created, ErrorBody, PlanBody, RepoSpec, route};
use cmux_proto::view::{StateView, TaskDetail};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::mcp::{
    CALL_TIMEOUT, ToolDef, initialize, input_schema, rpc_error, rpc_result, tool_result,
};
use crate::client::Endpoint;

const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "list_plans",
        about: "List the plans on the cmux server with their state and task count.",
        args: &[],
        levels: &[],
    },
    ToolDef {
        name: "get_plan",
        about: "Show one plan: its state, repos, tasks and the written plan (plan.md).",
        args: &[("plan", "string", "Plan id such as P-20261018-auth")],
        levels: &[],
    },
    ToolDef {
        name: "create_plan",
        about: "Create a draft plan. Read the repository first. Then add every task with create_task. No agent starts until the user accepts the plan.",
        args: &[
            ("title", "string", "Short title"),
            (
                "body",
                "string",
                "The written plan in Markdown: the goal, the approach, the files each task creates or changes, and what is out of scope",
            ),
            (
                "repos",
                "strings?",
                "Absolute repository paths as path or path@branch; the first is where the executor runs. Default: this session's directory",
            ),
        ],
        levels: &[],
    },
    ToolDef {
        name: "create_task",
        about: "Add a task to a draft plan. Keep it small and testable: give exact file paths and the command that verifies it.",
        args: &[
            ("plan", "string", "Plan id"),
            ("title", "string", "Short title"),
            ("body", "string", "What to do, with file paths"),
            (
                "criteria",
                "strings",
                "Acceptance criteria, at least one; each one checkable by a command or a merged PR",
            ),
            (
                "deps",
                "strings?",
                "Ids of tasks that must merge first, such as T-0001",
            ),
            ("repo", "integer?", "Index into the plan's repos; default 0"),
        ],
        levels: &[],
    },
    ToolDef {
        name: "get_task",
        about: "Show one task in full: body, criteria, notes and state.",
        args: &[
            ("plan", "string", "Plan id"),
            ("task", "string", "Task id such as T-0003"),
        ],
        levels: &[],
    },
    ToolDef {
        name: "update_task",
        about: "Change a task's title, body, dependencies or repo.",
        args: &[
            ("plan", "string", "Plan id"),
            ("task", "string", "Task id"),
            ("title", "string?", "New title"),
            ("body", "string?", "New body"),
            (
                "deps",
                "strings?",
                "New dependency list (replaces the old one)",
            ),
            ("repo", "integer?", "New repo index"),
        ],
        levels: &[],
    },
    ToolDef {
        name: "accept_plan",
        about: "Accept a draft plan and start its executor, which runs the tasks. Call it only when the user asks you to start the plan.",
        args: &[("plan", "string", "Plan id")],
        levels: &[],
    },
];

/// The `tools/list` result.
pub fn tools_list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
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

/// One REST call: `(status, body)`, or why it did not reach the server.
pub trait Rest {
    fn send(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), String>;
}

fn call<T: DeserializeOwned>(
    rest: &mut impl Rest,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<T, String> {
    let (status, bytes) = rest.send(method, path, body.as_ref())?;
    if !(200..300).contains(&status) {
        let msg = serde_json::from_slice::<ErrorBody>(&bytes).map_or_else(
            |_| String::from_utf8_lossy(&bytes).into_owned(),
            |e| e.message,
        );
        return Err(format!("cmuxd answered {status}: {msg}"));
    }
    // A 204 has no body.
    let bytes: &[u8] = if bytes.is_empty() { b"null" } else { &bytes };
    serde_json::from_slice(bytes).map_err(|e| format!("unreadable answer from cmuxd: {e}"))
}

fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

fn strings(args: &Value, name: &str) -> Result<Vec<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.trim().to_string())
                    .ok_or_else(|| format!("{name} must be a list of strings"))
            })
            .filter(|s| s.as_ref().map_or(true, |s| !s.is_empty()))
            .collect(),
        Some(_) => Err(format!("{name} must be a list of strings")),
    }
}

fn repo_spec(s: &str) -> RepoSpec {
    match s.rsplit_once('@') {
        Some((path, branch)) if !path.is_empty() && !branch.is_empty() => RepoSpec {
            path: path.to_string(),
            base_branch: Some(branch.to_string()),
        },
        _ => RepoSpec {
            path: s.to_string(),
            base_branch: None,
        },
    }
}

/// Copies the optional fields that are present in `args` into `out`.
fn copy_present(args: &Value, names: &[&str], out: &mut serde_json::Map<String, Value>) {
    for n in names {
        if let Some(v) = args.get(*n).filter(|v| !v.is_null()) {
            out.insert(n.to_string(), v.clone());
        }
    }
}

fn state(rest: &mut impl Rest) -> Result<StateView, String> {
    call(rest, "GET", route::STATE, None)
}

/// Runs one tool. `cwd` is the session's directory, the default repo.
pub fn run_tool(
    name: &str,
    args: &Value,
    cwd: &Path,
    rest: &mut impl Rest,
) -> Result<String, String> {
    match name {
        "list_plans" => {
            let sv = state(rest)?;
            if sv.plans.is_empty() {
                return Ok("no plans yet".into());
            }
            let lines: Vec<String> = sv
                .plans
                .iter()
                .map(|p| {
                    format!(
                        "{} [{}] {} - {} tasks",
                        p.id,
                        p.state,
                        p.title,
                        p.tasks.len()
                    )
                })
                .collect();
            Ok(lines.join("\n"))
        }
        "get_plan" => {
            let id = arg(args, "plan")?;
            let sv = state(rest)?;
            let plan = sv
                .plans
                .iter()
                .find(|p| p.id.as_str() == id)
                .ok_or_else(|| format!("no plan {id}; list_plans shows the ids"))?;
            let repos: Vec<String> = plan
                .repos
                .iter()
                .map(|r| format!("{}@{}", r.path.display(), r.base_branch))
                .collect();
            let mut out = vec![
                format!("{} [{}] {}", plan.id, plan.state, plan.title),
                format!("repos: {}", repos.join(", ")),
            ];
            for t in sv.tasks.iter().filter(|t| t.plan == plan.id) {
                let deps: Vec<String> = t.deps.iter().map(|d| d.to_string()).collect();
                let deps = if deps.is_empty() {
                    String::new()
                } else {
                    format!(" (after {})", deps.join(", "))
                };
                out.push(format!(
                    "{} [{}] {}{deps}, criteria {}/{}",
                    t.id,
                    t.state.label(),
                    t.title,
                    t.criteria_done,
                    t.criteria_total
                ));
            }
            let body: PlanBody = call(rest, "GET", &route::plan(id, "body"), None)?;
            out.push(String::new());
            out.push(body.text);
            Ok(out.join("\n"))
        }
        "create_plan" => {
            let title = arg(args, "title")?.to_string();
            let body = arg(args, "body")?.to_string();
            let mut repos: Vec<RepoSpec> = strings(args, "repos")?
                .iter()
                .map(|s| repo_spec(s))
                .collect();
            if repos.is_empty() {
                repos.push(RepoSpec {
                    path: cwd.display().to_string(),
                    base_branch: None,
                });
            }
            let req = CreatePlanReq {
                title,
                repos,
                request: None,
            };
            let created: Created = call(rest, "POST", route::PLANS, Some(json!(req)))?;
            let id = created.id;
            call::<Value>(
                rest,
                "PUT",
                &route::plan(&id, "body"),
                Some(json!(PlanBody { text: body })),
            )?;
            Ok(format!(
                "created draft plan {id}. Add each task with create_task. Then tell the user to review the plan in the cmux tree and accept it with A, or call accept_plan when the user asks you to start it."
            ))
        }
        "create_task" => {
            let plan = arg(args, "plan")?;
            let criteria = strings(args, "criteria")?;
            if criteria.is_empty() {
                return Err("give at least one acceptance criterion".into());
            }
            let mut req = serde_json::Map::new();
            req.insert("title".into(), json!(arg(args, "title")?));
            req.insert("body".into(), json!(arg(args, "body")?));
            req.insert("criteria".into(), json!(criteria));
            req.insert("deps".into(), json!(strings(args, "deps")?));
            copy_present(args, &["repo"], &mut req);
            let created: Created = call(
                rest,
                "POST",
                &route::plan(plan, "tasks"),
                Some(Value::Object(req)),
            )?;
            Ok(format!("created {} in {plan}", created.id))
        }
        "get_task" => {
            let (plan, task) = (arg(args, "plan")?, arg(args, "task")?);
            let d: TaskDetail = call(rest, "GET", &route::task(plan, task), None)?;
            let mut out = vec![
                format!("{} [{}] {}", d.task.id, d.task.state.label(), d.task.title),
                d.body,
                "criteria:".into(),
            ];
            for c in &d.criteria {
                out.push(format!(
                    "[{}] {}",
                    if c.checked { "x" } else { " " },
                    c.text
                ));
            }
            for n in &d.notes {
                out.push(format!("note: {}", n.text));
            }
            Ok(out.join("\n"))
        }
        "update_task" => {
            let (plan, task) = (arg(args, "plan")?, arg(args, "task")?);
            let mut patch = serde_json::Map::new();
            copy_present(args, &["title", "body", "deps", "repo"], &mut patch);
            if patch.is_empty() {
                return Err("give at least one of title, body, deps or repo".into());
            }
            call::<Value>(
                rest,
                "PATCH",
                &route::task(plan, task),
                Some(Value::Object(patch)),
            )?;
            Ok(format!("updated {task}"))
        }
        "accept_plan" => {
            let plan = arg(args, "plan")?;
            call::<Value>(rest, "POST", &route::plan(plan, "accept"), None)?;
            Ok(format!(
                "accepted {plan}; its executor starts the tasks. The user follows progress in the cmux tree."
            ))
        }
        other => {
            let known: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
            Err(format!(
                "unknown tool {other:?}; available: {}",
                known.join(", ")
            ))
        }
    }
}

/// Answers one JSON-RPC message; `None` for notifications and responses.
pub fn handle(msg: &Value, cwd: &Path, rest: &mut impl Rest) -> Option<Value> {
    let id = msg.get("id")?;
    let method = msg.get("method").and_then(Value::as_str)?;
    let params = msg.get("params").unwrap_or(&Value::Null);
    Some(match method {
        "initialize" => initialize(id, params),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, tools_list()),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match run_tool(name, &args, cwd, rest) {
                Ok(text) => rpc_result(id, tool_result(&text, false)),
                Err(e) => rpc_result(id, tool_result(&format!("error: {e}"), true)),
            }
        }
        other => rpc_error(id, -32601, &format!("method not found: {other}")),
    })
}

/// Calls the server on its Unix socket.
pub struct SocketRest {
    sock: PathBuf,
    rt: tokio::runtime::Runtime,
}

impl SocketRest {
    pub fn new(sock: PathBuf) -> std::io::Result<SocketRest> {
        Ok(SocketRest {
            sock,
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        })
    }
}

impl Rest for SocketRest {
    fn send(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Vec<u8>), String> {
        let ep = Endpoint::Unix(self.sock.clone());
        let sent = self.rt.block_on(async {
            let req = async {
                match body {
                    Some(b) => ep.json(method, path, &[], b).await,
                    None => ep.request(method, path, &[], None).await,
                }
            };
            tokio::time::timeout(CALL_TIMEOUT, req).await
        });
        match sent {
            Ok(Ok(r)) => Ok((r.status, r.body)),
            Ok(Err(e)) => Err(format!(
                "cannot reach cmuxd on {}: {e}; the server may be restarting, try again",
                self.sock.display()
            )),
            Err(_) => Err("cmuxd did not answer in time".into()),
        }
    }
}

/// The whole subcommand: serves stdin to stdout until EOF.
pub fn run() -> Result<(), String> {
    use std::io::{BufRead, Write};
    let sock = std::env::var_os("CMUX_SOCK")
        .map(PathBuf::from)
        .ok_or("CMUX_SOCK is not set")?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let mut rest = SocketRest::new(sock).map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, &cwd, &mut rest),
            Err(e) => Some(rpc_error(
                &Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };
        if let Some(r) = reply {
            writeln!(out, "{r}").map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records each call and answers from a queue of `(status, body)`.
    #[derive(Default)]
    struct Fake {
        calls: Vec<(String, String, Option<Value>)>,
        answers: Vec<(u16, Value)>,
    }

    impl Rest for Fake {
        fn send(
            &mut self,
            method: &str,
            path: &str,
            body: Option<&Value>,
        ) -> Result<(u16, Vec<u8>), String> {
            self.calls.push((method.into(), path.into(), body.cloned()));
            let (status, v) = self.answers.remove(0);
            let bytes = if v.is_null() {
                vec![]
            } else {
                v.to_string().into_bytes()
            };
            Ok((status, bytes))
        }
    }

    #[test]
    fn create_plan_defaults_to_the_session_directory_and_writes_the_body() {
        let mut rest = Fake {
            answers: vec![(201, json!({"id": "P-20261018-auth"})), (204, Value::Null)],
            ..Fake::default()
        };
        let out = run_tool(
            "create_plan",
            &json!({"title": "auth", "body": "# Goal"}),
            Path::new("/src/api"),
            &mut rest,
        )
        .unwrap();
        assert!(out.contains("P-20261018-auth"), "{out}");
        assert_eq!(rest.calls[0].1, "/v1/plans");
        assert_eq!(
            rest.calls[0].2,
            Some(json!({"title": "auth", "repos": [{"path": "/src/api", "base_branch": null}]}))
        );
        assert_eq!(
            rest.calls[1],
            (
                "PUT".into(),
                "/v1/plans/P-20261018-auth/body".into(),
                Some(json!({"text": "# Goal"}))
            )
        );
    }

    #[test]
    fn create_task_needs_a_criterion_and_passes_deps() {
        let mut rest = Fake::default();
        let args = json!({"plan": "P-1", "title": "t", "body": "b", "criteria": []});
        assert!(run_tool("create_task", &args, Path::new("/"), &mut rest).is_err());
        assert!(rest.calls.is_empty());

        rest.answers.push((201, json!({"id": "T-0002"})));
        let args = json!({"plan": "P-1", "title": "t", "body": "b",
            "criteria": ["cargo test passes"], "deps": ["T-0001"]});
        let out = run_tool("create_task", &args, Path::new("/"), &mut rest).unwrap();
        assert_eq!(out, "created T-0002 in P-1");
        assert_eq!(
            rest.calls[0].2,
            Some(json!({"title": "t", "body": "b",
                "criteria": ["cargo test passes"], "deps": ["T-0001"]}))
        );
    }

    #[test]
    fn server_errors_reach_claude() {
        let mut rest = Fake {
            answers: vec![(
                400,
                json!({"code": "invalid", "message": "error: no plan P-9"}),
            )],
            ..Fake::default()
        };
        let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "accept_plan", "arguments": {"plan": "P-9"}}});
        let reply = handle(&msg, Path::new("/"), &mut rest).unwrap();
        assert_eq!(reply["result"]["isError"], json!(true));
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("no plan P-9"), "{text}");
    }

    #[test]
    fn lists_every_tool() {
        let names: Vec<String> = tools_list()["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "list_plans",
                "get_plan",
                "create_plan",
                "create_task",
                "get_task",
                "update_task",
                "accept_plan"
            ]
        );
    }
}
