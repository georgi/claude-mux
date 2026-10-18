//! The CLI verbs (UI design section 5): `cmux plans`, `cmux send ...`.
//!
//! Each verb is a thin call to the REST API. Agent paths such as
//! `auth-refactor/T-0002/tests` are resolved here from `GET /state`: the first
//! segment is a plan id, the slug part of one, or the slug of a plan title.
//! Exit codes: 0 success, 1 usage error, 2 server or connection error.

use std::io::Write;
use std::time::{Duration, Instant};

use cmux_core::ids::{AgentPath, slugify};
use cmux_proto::api::{AnswerAction, AnswerReq, Created, ErrorCode, MessageReq, RepoSpec, route};
use cmux_proto::stream::{ClientKind, ClientMsg, Frame, ServerMsg};
use cmux_proto::view::{
    AgentId, AgentView, ChangeView, ItemId, ItemKind, ItemView, Life, ObjectRef, PlanView,
    StateView, TaskView, TermId,
};
use serde::Serialize;

use super::http::{Http, HttpError};
use super::servers::{ClientConfig, Endpoint, client_toml, local_socket};
use super::stream::{Cmd, FrameAction, Incoming, Outgoing, Session, Wire, WsWire};
use crate::tui::state::Node;

pub const USAGE: &str = "\
USAGE:
    cmux [-s NAME] plans [--json]                List plans
    cmux [-s NAME] board <plan> [--json]         List the tasks of a plan
    cmux [-s NAME] new-plan -r <repo>... [--request TEXT] <title>
                                                 Create a plan (--request starts its executor)
    cmux [-s NAME] accept <plan>                 Accept a proposed plan
    cmux [-s NAME] pause <plan>                  Pause a plan
    cmux [-s NAME] inbox [--json]                List what needs you
    cmux [-s NAME] allow <item> [--always]       Allow a permission request
    cmux [-s NAME] deny <item> [note]            Deny a permission request
    cmux [-s NAME] reply <item> <text>           Answer a question
    cmux [-s NAME] send <agent> <text> [--steer] Queue a message, or steer (interrupt)
    cmux [-s NAME] stop <agent>                  Stop an agent
    cmux [-s NAME] resume <agent>                Resume a stopped agent
    cmux [-s NAME] tail <agent>                  Stream the agent's terminal to stdout
    cmux [-s NAME] attach [<agent>]              Open the TUI on that agent

<plan> is a plan id or the slug of its title. <agent> is <plan>/executor,
<plan>/T-0002 or <plan>/T-0002/<name>; <plan> alone is the executor.
<item> is I-<n> (see `cmux inbox`).

Exit codes: 0 success, 1 usage error, 2 server or connection error.
";

/// The verbs `main` hands to [`main`]. `attach` is here for its resolution
/// only; `main` starts the TUI.
pub const VERBS: &[&str] = &[
    "plans", "board", "new-plan", "accept", "pause", "inbox", "allow", "deny", "reply", "send",
    "stop", "resume", "tail", "attach",
];

const PING_EVERY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerbError {
    /// Bad arguments, or a plan, agent or item that does not exist. Exit 1.
    Usage(String),
    /// The server refused, failed or could not be reached. Exit 2.
    Server(String),
}

impl VerbError {
    pub fn code(&self) -> i32 {
        match self {
            VerbError::Usage(_) => 1,
            VerbError::Server(_) => 2,
        }
    }

    /// One line for stderr, always `error: ...`.
    pub fn line(&self) -> String {
        let (VerbError::Usage(m) | VerbError::Server(m)) = self;
        let m = m.split_whitespace().collect::<Vec<_>>().join(" ");
        if m.starts_with("error:") {
            m
        } else {
            format!("error: {m}")
        }
    }
}

impl From<HttpError> for VerbError {
    fn from(e: HttpError) -> Self {
        VerbError::Server(e.to_string())
    }
}

fn usage(msg: impl Into<String>) -> VerbError {
    VerbError::Usage(msg.into())
}

type VResult<T> = Result<T, VerbError>;

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Plans {
        json: bool,
    },
    Board {
        plan: String,
        json: bool,
    },
    NewPlan {
        repos: Vec<String>,
        title: String,
        request: Option<String>,
    },
    Accept {
        plan: String,
    },
    Pause {
        plan: String,
    },
    Inbox {
        json: bool,
    },
    Answer {
        item: ItemId,
        action: AnswerAction,
        text: Option<String>,
    },
    Send {
        agent: String,
        text: String,
        steer: bool,
    },
    Stop {
        agent: String,
    },
    Resume {
        agent: String,
    },
    Tail {
        agent: String,
    },
    Attach {
        agent: Option<String>,
    },
    Help,
}

/// Parses the arguments after the verb.
pub fn parse(verb: &str, args: &[String]) -> VResult<Verb> {
    let mut flags: Vec<&str> = Vec::new();
    let mut pos: Vec<String> = Vec::new();
    let mut repos = Vec::new();
    let mut request = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(Verb::Help),
            "-r" | "--repo" if verb == "new-plan" => repos.push(
                it.next()
                    .ok_or_else(|| usage("-r needs a repository path"))?
                    .clone(),
            ),
            "--request" if verb == "new-plan" => {
                request = Some(
                    it.next()
                        .ok_or_else(|| usage("--request needs a text"))?
                        .clone(),
                )
            }
            "--" => pos.extend(it.by_ref().cloned()),
            f @ ("--json" | "--always" | "--steer") => flags.push(f),
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(usage(format!("unknown option {s} for `cmux {verb}`")));
            }
            p => pos.push(p.to_string()),
        }
    }
    let allowed: &[&str] = match verb {
        "plans" | "board" | "inbox" => &["--json"],
        "allow" => &["--always"],
        "send" => &["--steer"],
        _ => &[],
    };
    if let Some(f) = flags.iter().find(|f| !allowed.contains(f)) {
        return Err(usage(format!("unknown option {f} for `cmux {verb}`")));
    }
    let flag = |f: &str| flags.contains(&f);
    let n = pos.len();
    let want = |lo: usize, hi: usize, shape: &str| {
        if (lo..=hi).contains(&n) {
            Ok(())
        } else {
            Err(usage(format!("usage: cmux {verb} {shape}")))
        }
    };
    let mut pos = pos.into_iter();
    let mut next = || pos.next().unwrap_or_default();
    Ok(match verb {
        "plans" => {
            want(0, 0, "[--json]")?;
            Verb::Plans {
                json: flag("--json"),
            }
        }
        "board" => {
            want(1, 1, "<plan> [--json]")?;
            Verb::Board {
                plan: next(),
                json: flag("--json"),
            }
        }
        "new-plan" => {
            want(1, 1, "-r <repo>... [--request TEXT] <title>")?;
            if repos.is_empty() {
                return Err(usage("a plan needs at least one repository: -r <repo>"));
            }
            Verb::NewPlan {
                repos,
                title: next(),
                request,
            }
        }
        "accept" => {
            want(1, 1, "<plan>")?;
            Verb::Accept { plan: next() }
        }
        "pause" => {
            want(1, 1, "<plan>")?;
            Verb::Pause { plan: next() }
        }
        "inbox" => {
            want(0, 0, "[--json]")?;
            Verb::Inbox {
                json: flag("--json"),
            }
        }
        "allow" => {
            want(1, 1, "<item> [--always]")?;
            Verb::Answer {
                item: parse_item(&next())?,
                action: if flag("--always") {
                    AnswerAction::AllowAlways
                } else {
                    AnswerAction::Allow
                },
                text: None,
            }
        }
        "deny" => {
            want(1, usize::MAX, "<item> [note]")?;
            let item = parse_item(&next())?;
            let note: Vec<String> = (1..n).map(|_| next()).collect();
            Verb::Answer {
                item,
                action: AnswerAction::Deny,
                text: (!note.is_empty()).then(|| note.join(" ")),
            }
        }
        "reply" => {
            want(2, 2, "<item> <text>")?;
            let item = parse_item(&next())?;
            let text = next();
            if text.trim().is_empty() {
                return Err(usage("reply needs a non-empty text"));
            }
            Verb::Answer {
                item,
                action: AnswerAction::Reply,
                text: Some(text),
            }
        }
        "send" => {
            want(2, 2, "<agent> <text> [--steer]")?;
            let agent = next();
            let text = next();
            if text.trim().is_empty() {
                return Err(usage("send needs a non-empty text"));
            }
            Verb::Send {
                agent,
                text,
                steer: flag("--steer"),
            }
        }
        "stop" => {
            want(1, 1, "<agent>")?;
            Verb::Stop { agent: next() }
        }
        "resume" => {
            want(1, 1, "<agent>")?;
            Verb::Resume { agent: next() }
        }
        "tail" => {
            want(1, 1, "<agent>")?;
            Verb::Tail { agent: next() }
        }
        "attach" => {
            want(0, 1, "[<agent>]")?;
            Verb::Attach {
                agent: (n == 1).then(next),
            }
        }
        v => return Err(usage(format!("unknown command {v:?}"))),
    })
}

fn parse_item(s: &str) -> VResult<ItemId> {
    let n = s.strip_prefix("I-").unwrap_or(s);
    match n.parse::<u64>() {
        Ok(n) => Ok(ItemId(n)),
        Err(_) => Err(usage(format!(
            "bad item {s:?}: expected I-<n> (see `cmux inbox`)"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Resolving plans and agents
// ---------------------------------------------------------------------------

/// The slug part of `P-YYYYMMDD-slug`.
fn id_slug(p: &PlanView) -> &str {
    let s = p.id.as_str();
    s.get(11..).unwrap_or(s)
}

/// A plan by id, by the slug part of its id, or by the slug of its title.
pub fn resolve_plan<'a>(sv: &'a StateView, seg: &str) -> VResult<&'a PlanView> {
    if let Some(p) = sv.plans.iter().find(|p| p.id.as_str() == seg) {
        return Ok(p);
    }
    let hits: Vec<&PlanView> = sv
        .plans
        .iter()
        .filter(|p| id_slug(p) == seg || slugify(&p.title) == seg)
        .collect();
    match hits.as_slice() {
        [p] => Ok(p),
        [] => Err(usage(format!("no plan {seg:?} (`cmux plans` lists them)"))),
        many => Err(usage(format!(
            "{seg:?} matches several plans ({}); use the plan id",
            many.iter()
                .map(|p| p.id.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// An agent by path: `<plan>`, `<plan>/executor`, `<plan>/T-0002`,
/// `<plan>/T-0002/<name>`.
pub fn resolve_agent<'a>(sv: &'a StateView, path: &str) -> VResult<&'a AgentView> {
    let (seg, rest) = path.split_once('/').unwrap_or((path, "executor"));
    let plan = resolve_plan(sv, seg)?;
    let ap: AgentPath = rest.parse().map_err(|_| {
        usage(format!(
            "bad agent path {path:?}: expected <plan>/executor, <plan>/T-NNNN or <plan>/T-NNNN/<name>"
        ))
    })?;
    let id = AgentId::new(plan.id.clone(), ap);
    sv.agent(&id).ok_or_else(|| {
        usage(format!(
            "no agent {id} (`cmux board {seg}` lists the tasks)"
        ))
    })
}

/// `attach [<agent>]`: the tree node to open, `None` for no agent.
pub fn attach_target(http: &Http, agent: Option<&str>) -> VResult<Option<Node>> {
    let Some(path) = agent else {
        return Ok(None);
    };
    let sv: StateView = http.get(route::STATE)?;
    Ok(Some(Node::for_agent(&resolve_agent(&sv, path)?.id)))
}

fn agent_route(id: &AgentId, action: &str) -> String {
    route::agent(&id.to_string().replace('/', "%2F"), action)
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// Entry point from `main`: `cmux [-s NAME] VERB ARGS...`. Returns the exit
/// code; errors go to stderr as one line.
pub fn main(server: Option<&str>, verb: &str, args: &[String]) -> i32 {
    let endpoint = || -> VResult<Endpoint> {
        let cfg = ClientConfig::load(&client_toml()).map_err(|e| usage(format!("{e:#}")))?;
        cfg.resolve(server, &local_socket())
            .map(|s| s.endpoint)
            .map_err(|e| VerbError::Server(format!("{e:#}")))
    };
    let mut out = std::io::stdout().lock();
    match run(verb, args, endpoint, &mut out) {
        Ok(()) => 0,
        Err(e) => {
            let _ = out.flush();
            eprintln!("{}", e.line());
            e.code()
        }
    }
}

/// Parses, then connects through `endpoint` and runs the verb. Arguments are
/// checked before the server is contacted, so a usage error never costs a
/// round trip.
pub fn run(
    verb: &str,
    args: &[String],
    endpoint: impl FnOnce() -> VResult<Endpoint>,
    out: &mut dyn Write,
) -> VResult<()> {
    let v = parse(verb, args)?;
    if v == Verb::Help {
        let _ = out.write_all(USAGE.as_bytes());
        return Ok(());
    }
    let ep = endpoint()?;
    let http = Http::new(ep.clone());
    exec(&http, &ep, v, out)
}

fn exec(http: &Http, ep: &Endpoint, v: Verb, out: &mut dyn Write) -> VResult<()> {
    let state = || -> VResult<StateView> { Ok(http.get(route::STATE)?) };
    let say = |out: &mut dyn Write, s: String| {
        let _ = writeln!(out, "{s}");
    };
    match v {
        Verb::Help => {}
        Verb::Plans { json } => {
            let sv = state()?;
            if json {
                print_json(out, &sv.plans);
            } else {
                say(out, plans_table(&sv));
            }
        }
        Verb::Board { plan, json } => {
            let sv = state()?;
            let p = resolve_plan(&sv, &plan)?;
            let tasks: Vec<&TaskView> = p.tasks.iter().filter_map(|t| sv.task(&p.id, *t)).collect();
            if json {
                print_json(out, &tasks);
            } else {
                say(out, board_table(&sv, &tasks));
            }
        }
        Verb::NewPlan {
            repos,
            title,
            request,
        } => {
            let repos = repos
                .iter()
                .map(|r| RepoSpec {
                    // A local server sees our paths; a remote one gets them as typed.
                    path: match ep {
                        Endpoint::Unix(_) => std::fs::canonicalize(r)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| r.clone()),
                        Endpoint::Remote { .. } => r.clone(),
                    },
                    base_branch: None,
                })
                .collect();
            let c: Created = http.post(
                route::PLANS,
                &cmux_proto::api::CreatePlanReq {
                    title,
                    repos,
                    request,
                },
            )?;
            say(out, c.id);
        }
        Verb::Accept { plan } => {
            let id = resolve_plan(&state()?, &plan)?.id.clone();
            post_empty(http, &route::plan(id.as_str(), "accept"))?;
            say(out, format!("accepted {id}"));
        }
        Verb::Pause { plan } => {
            let id = resolve_plan(&state()?, &plan)?.id.clone();
            post_empty(http, &route::plan(id.as_str(), "pause"))?;
            say(out, format!("paused {id}"));
        }
        Verb::Inbox { json } => {
            let sv = state()?;
            let items = needs_you(&sv);
            if json {
                print_json(out, &items);
            } else {
                say(out, inbox_table(&items));
            }
        }
        Verb::Answer { item, action, text } => {
            let _: serde_json::Value =
                http.post(&route::item_answer(item.0), &AnswerReq { action, text })?;
            let done = match action {
                AnswerAction::Allow => "allowed",
                AnswerAction::AllowAlways => "allowed always",
                AnswerAction::Deny => "denied",
                _ => "answered",
            };
            say(out, format!("{done} {item}"));
        }
        Verb::Send { agent, text, steer } => {
            let id = resolve_agent(&state()?, &agent)?.id.clone();
            let _: serde_json::Value =
                http.post(&agent_route(&id, "messages"), &MessageReq { text, steer })?;
            let how = if steer { "steered" } else { "queued for" };
            say(out, format!("{how} {id}"));
        }
        Verb::Stop { agent } => {
            let id = resolve_agent(&state()?, &agent)?.id.clone();
            post_empty(http, &agent_route(&id, "stop"))?;
            say(out, format!("stopped {id}"));
        }
        Verb::Resume { agent } => {
            let id = resolve_agent(&state()?, &agent)?.id.clone();
            post_empty(http, &agent_route(&id, "resume"))?;
            say(out, format!("resumed {id}"));
        }
        Verb::Tail { agent } => {
            let sv = state()?;
            let a = resolve_agent(&sv, &agent)?;
            let term = a
                .term
                .filter(|t| sv.terminal(*t).is_some())
                .ok_or_else(|| usage(format!("{} has no terminal (it is not running)", a.id)))?;
            tail(ep, term, out)?;
        }
        Verb::Attach { agent } => {
            // `main` opens the TUI; here only the target is checked.
            attach_target(http, agent.as_deref())?;
        }
    }
    Ok(())
}

fn post_empty(http: &Http, path: &str) -> VResult<()> {
    http.request("POST", path, None, Some(&super::http::new_key()))?;
    Ok(())
}

/// Items waiting for a person, oldest first (the TUI's needs-you list).
pub fn needs_you(sv: &StateView) -> Vec<&ItemView> {
    crate::tui::rows::needs_you(sv)
}

fn print_json<T: Serialize + ?Sized>(out: &mut dyn Write, v: &T) {
    let s = serde_json::to_string_pretty(v).expect("views serialize");
    let _ = writeln!(out, "{s}");
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// Left-aligned columns padded to their widest cell; the last one unpadded.
pub fn table(head: &[&str], rows: &[Vec<String>]) -> String {
    let w = |i: usize| {
        rows.iter()
            .map(|r| r[i].chars().count())
            .chain([head[i].len()])
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..head.len()).map(w).collect();
    let line = |cells: Vec<&str>| {
        let last = cells.len() - 1;
        let mut s = String::new();
        for (i, c) in cells.into_iter().enumerate() {
            if i == last {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<0$}  ", widths[i]));
            }
        }
        s.trim_end().to_string()
    };
    let mut out = vec![line(head.to_vec())];
    out.extend(
        rows.iter()
            .map(|r| line(r.iter().map(String::as_str).collect())),
    );
    out.join("\n")
}

fn plans_table(sv: &StateView) -> String {
    let rows: Vec<Vec<String>> = sv
        .plans
        .iter()
        .map(|p| {
            let tasks: Vec<&TaskView> = sv.tasks.iter().filter(|t| t.plan == p.id).collect();
            let merged = tasks
                .iter()
                .filter(|t| t.state == cmux_proto::view::TaskState::Merged)
                .count();
            let live = sv
                .agents
                .iter()
                .filter(|a| {
                    a.id.plan() == &p.id && matches!(a.life, Life::Starting | Life::Running)
                })
                .count();
            let state = if p.paused {
                format!("{} (paused)", p.state)
            } else {
                p.state.to_string()
            };
            vec![
                p.id.to_string(),
                state,
                format!("{merged}/{}", tasks.len()),
                live.to_string(),
                p.title.clone(),
            ]
        })
        .collect();
    table(&["PLAN", "STATE", "MERGED", "AGENTS", "TITLE"], &rows)
}

fn agent_state(a: &AgentView) -> String {
    match a.life {
        Life::Starting => "starting".into(),
        Life::Running => snake(&a.activity),
        Life::Exited { code: Some(c) } => format!("exited {c}"),
        Life::Exited { code: None } => "exited".into(),
        Life::Stopped => "stopped".into(),
    }
}

fn snake<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
        _ => "?".into(),
    }
}

fn board_table(sv: &StateView, tasks: &[&TaskView]) -> String {
    let rows: Vec<Vec<String>> = tasks
        .iter()
        .map(|t| {
            let agent = t
                .agent
                .as_ref()
                .and_then(|a| sv.agent(a))
                .map_or_else(|| "-".into(), agent_state);
            vec![
                t.id.to_string(),
                t.state.to_string(),
                format!("{}/{}", t.criteria_done, t.criteria_total),
                agent,
                t.pr.as_ref()
                    .map_or_else(|| "-".into(), |p| format!("#{}", p.number)),
                t.title.clone(),
            ]
        })
        .collect();
    table(
        &["TASK", "STATE", "CRITERIA", "AGENT", "PR", "TITLE"],
        &rows,
    )
}

fn inbox_table(items: &[&ItemView]) -> String {
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|i| {
            let what = match &i.kind {
                ItemKind::Permission {
                    tool,
                    input_summary,
                } => format!("{tool}: {input_summary}"),
                _ => i.body.clone(),
            };
            let what = what.lines().next().unwrap_or_default();
            let what: String = if what.chars().count() > 72 {
                what.chars().take(71).chain(['…']).collect()
            } else {
                what.to_string()
            };
            vec![
                i.id.to_string(),
                snake(&i.kind),
                i.from
                    .as_ref()
                    .map_or_else(|| "-".into(), ToString::to_string),
                what,
            ]
        })
        .collect();
    table(&["ITEM", "KIND", "FROM", "WHAT"], &rows)
}

// ---------------------------------------------------------------------------
// tail
// ---------------------------------------------------------------------------

/// Streams the terminal's bytes to `out` until its program ends: first the
/// server's snapshot of the screen, then live output. Built on the stream
/// layer's `Session`, which drops bytes already seen and asks for a new
/// snapshot when some are missing.
pub fn tail(ep: &Endpoint, term: TermId, out: &mut dyn Write) -> VResult<()> {
    let lost = |e: &dyn std::fmt::Display| VerbError::Server(format!("stream: {e}"));
    let mut wire = WsWire::connect(ep).map_err(|e| lost(&e))?;
    let mut session = Session::new();
    session.on_cmd(&Cmd::Want(term));
    let token = match ep {
        Endpoint::Remote { token, .. } if !token.is_empty() => Some(token.clone()),
        _ => None,
    };
    let hello = ClientMsg::Hello {
        client: ClientKind::Cli,
        since_rev: None,
        token,
        info: Some(format!("cmux {} tail", env!("CARGO_PKG_VERSION"))),
        terms: Vec::new(),
    };
    let send = |wire: &mut WsWire, m: ClientMsg| {
        wire.send(&Outgoing::Msg(m))
            .map_err(|e| lost(&format!("{e:?}")))
    };
    send(&mut wire, hello)?;
    let mut last_ping = Instant::now();
    let mut nonce = 0;
    loop {
        match wire.recv().map_err(|e| lost(&format!("{e:?}")))? {
            None => {}
            Some(Incoming::Text(t)) => match serde_json::from_str::<ServerMsg>(&t) {
                Ok(ServerMsg::Welcome { .. }) => {
                    for m in session.after_welcome() {
                        send(&mut wire, m)?;
                    }
                }
                Ok(ServerMsg::Exit { term: t, .. }) if t == term => return Ok(()),
                Ok(ServerMsg::Deltas { items, .. }) => {
                    let gone = items.iter().any(|d| {
                        matches!(&d.change, ChangeView::Removed(ObjectRef::Terminal { term: t }) if *t == term)
                    });
                    if gone {
                        return Ok(());
                    }
                }
                Ok(ServerMsg::Error { code, message }) => {
                    if matches!(code, ErrorCode::Unauthorized | ErrorCode::NotFound) {
                        return Err(VerbError::Server(message));
                    }
                }
                _ => {}
            },
            Some(Incoming::Binary(b)) => {
                let Ok(frame) = Frame::decode(&b) else {
                    continue;
                };
                match session.on_frame(frame) {
                    FrameAction::Feed { data, .. } => {
                        // A closed pipe (`| head`) ends the tail quietly.
                        if out.write_all(&data).and_then(|()| out.flush()).is_err() {
                            return Ok(());
                        }
                    }
                    FrameAction::Resync(t) => send(
                        &mut wire,
                        ClientMsg::Subscribe {
                            term: t,
                            since_seq: None,
                        },
                    )?,
                    FrameAction::Ignore => {}
                }
            }
        }
        if last_ping.elapsed() > PING_EVERY {
            last_ping = Instant::now();
            nonce += 1;
            send(&mut wire, ClientMsg::Ping { nonce })?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_every_verb() {
        let p = |v: &str, a: &[&str]| parse(v, &args(a));
        assert_eq!(p("plans", &["--json"]), Ok(Verb::Plans { json: true }));
        assert_eq!(
            p("new-plan", &["-r", "a", "-r", "b", "Fix it"]),
            Ok(Verb::NewPlan {
                repos: args(&["a", "b"]),
                title: "Fix it".into(),
                request: None
            })
        );
        assert_eq!(
            p("deny", &["I-3", "not", "now"]),
            Ok(Verb::Answer {
                item: ItemId(3),
                action: AnswerAction::Deny,
                text: Some("not now".into())
            })
        );
        assert_eq!(
            p("allow", &["4", "--always"]),
            Ok(Verb::Answer {
                item: ItemId(4),
                action: AnswerAction::AllowAlways,
                text: None
            })
        );
        assert_eq!(
            p("send", &["demo/T-0001", "hi", "--steer"]),
            Ok(Verb::Send {
                agent: "demo/T-0001".into(),
                text: "hi".into(),
                steer: true
            })
        );
        assert_eq!(p("attach", &[]), Ok(Verb::Attach { agent: None }));
        assert_eq!(p("tail", &["--help"]), Ok(Verb::Help));
    }

    #[test]
    fn usage_errors_are_exit_1() {
        let p = |v: &str, a: &[&str]| parse(v, &args(a)).unwrap_err();
        for e in [
            p("plans", &["extra"]),
            p("plans", &["--steer"]),
            p("board", &[]),
            p("new-plan", &["title"]),
            p("allow", &["I-x"]),
            p("reply", &["I-1", " "]),
            p("send", &["a"]),
            p("tail", &["--bogus", "a"]),
        ] {
            assert_eq!(e.code(), 1, "{e:?}");
            assert!(e.line().starts_with("error: "), "{e:?}");
            assert!(!e.line().contains('\n'));
        }
    }

    #[test]
    fn table_pads_columns() {
        let t = table(
            &["A", "BB", "C"],
            &[
                vec!["xxx".into(), "y".into(), "last col".into()],
                vec!["x".into(), "".into(), "z".into()],
            ],
        );
        assert_eq!(t, "A    BB  C\nxxx  y   last col\nx        z");
    }
}
