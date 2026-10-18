//! `cmuxd doctor [<agent>]`: checks the installation, or shows one agent.
//!
//! Both read the data directory without its lock, so they work while the
//! server runs (store saves are atomic renames).

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use cmux_core::delivery::{kind_label, pending_for};
use cmux_core::model::{Agent, InboxItem, ItemKind, Life, Recipient, State};
use cmux_proto::api::{StatusResp, route};

use crate::client::Endpoint;
use crate::config::{Config, Overrides};
use crate::setup::{Exec, Mark, Report, check_claude, check_gh};

/// How many diagnostics lines `doctor <agent>` prints.
pub const DIAG_LINES: usize = 50;

/// Config, listen address, TLS, socket, store, `claude` and `gh`.
pub async fn system(o: &Overrides, exec: &dyn Exec) -> Report {
    let mut r = Report::default();
    let path = o.config.clone().unwrap_or_else(Config::default_path);
    let config = match Config::load(o) {
        Ok(c) => {
            if path.exists() {
                r.push(Mark::Ok, format!("config {}", path.display()));
            } else {
                r.push(
                    Mark::Ok,
                    format!("no config at {}; using defaults", path.display()),
                );
            }
            c
        }
        Err(e) => {
            r.push(Mark::Fail, format!("config: {e}"));
            let mut c = Config::default();
            c.apply(o);
            c
        }
    };
    listen(&config, &mut r);
    tls(&config, &mut r);
    socket(&config, &mut r).await;
    store(&config.data_dir(), &mut r);
    check_claude(&config.agents.claude_bin, exec, &mut r);
    if config.github.enabled {
        check_gh(&config.github.gh_bin, exec, &mut r);
    } else {
        r.push(
            Mark::Skip,
            "GitHub tracking is off ([github] enabled = false)",
        );
    }
    r
}

fn listen(c: &Config, r: &mut Report) {
    match c.tcp_addr() {
        Ok(None) => r.push(Mark::Ok, "TCP listener is off; Unix socket only"),
        Ok(Some(a)) => {
            let scheme = if c.tls.is_some() { "https" } else { "http" };
            r.push(Mark::Ok, format!("listen {scheme}://{a}"));
        }
        Err(e) => r.push(Mark::Fail, format!("listen: {e}")),
    }
}

fn tls(c: &Config, r: &mut Report) {
    let Some(t) = &c.tls else {
        r.push(
            Mark::Skip,
            "TLS is off; for remote access use tailscale serve or a reverse proxy",
        );
        return;
    };
    if !cfg!(feature = "tls") {
        r.push(
            Mark::Fail,
            "[tls] is set, but this cmuxd is built without the `tls` feature",
        );
    }
    for (what, p) in [("certificate", &t.cert), ("key", &t.key)] {
        match std::fs::read(p) {
            Ok(b) if String::from_utf8_lossy(&b).contains("-----BEGIN ") => {
                r.push(Mark::Ok, format!("TLS {what} {}", p.display()))
            }
            Ok(_) => r.push(Mark::Fail, format!("TLS {what} {} is not PEM", p.display())),
            Err(e) => r.push(Mark::Fail, format!("TLS {what} {}: {e}", p.display())),
        }
    }
    if let Ok(m) = std::fs::metadata(&t.key)
        && m.permissions().mode() & 0o077 != 0
    {
        r.push(
            Mark::Fail,
            format!(
                "TLS key {} is readable by others; chmod 600 it",
                t.key.display()
            ),
        );
    }
}

async fn socket(c: &Config, r: &mut Report) {
    let path = c.socket_path();
    let ep = Endpoint::Unix(path.clone());
    let req = ep.request("GET", route::STATUS, &[], None);
    let resp = match tokio::time::timeout(Duration::from_secs(3), req).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(e)) => {
            r.push(
                Mark::Fail,
                format!(
                    "no server on {} ({e}); start it with `systemctl --user start cmuxd` or `cmuxd serve`",
                    path.display()
                ),
            );
            return;
        }
        Err(_) => {
            r.push(
                Mark::Fail,
                format!("{} did not answer in 3 s", path.display()),
            );
            return;
        }
    };
    match resp.json::<StatusResp>() {
        Ok(s) if resp.status == 200 => {
            let mut text = format!(
                "server {} {} on {}: up {} s, {} agents",
                s.server.name,
                s.server.version,
                path.display(),
                s.uptime_secs,
                s.agents.len(),
            );
            if !s.github.is_empty() {
                text.push_str(&format!(", github {}", s.github));
            }
            r.push(Mark::Ok, text);
        }
        _ => r.push(
            Mark::Fail,
            format!(
                "{} answered {}: {}",
                path.display(),
                resp.status,
                resp.text()
            ),
        ),
    }
}

fn store(dir: &Path, r: &mut Report) {
    match std::fs::metadata(dir) {
        Ok(m) if m.permissions().mode() & 0o077 != 0 => r.push(
            Mark::Fail,
            format!(
                "data dir {} is open to others (mode {:o}); chmod 700 it",
                dir.display(),
                m.permissions().mode() & 0o777
            ),
        ),
        Ok(_) => {}
        Err(e) => {
            r.push(
                Mark::Skip,
                format!("no data dir at {} yet ({e})", dir.display()),
            );
            return;
        }
    }
    match crate::store::load_dir(dir) {
        Ok(st) => {
            let agents: usize = st.plans.iter().map(|p| p.agents.len()).sum();
            let pending = st.inbox.iter().filter(|i| i.is_pending()).count();
            r.push(
                Mark::Ok,
                format!(
                    "store {}: {} plans, {agents} agents, {pending} pending items",
                    dir.display(),
                    st.plans.len()
                ),
            );
        }
        Err(e) => r.push(Mark::Fail, format!("store: {e}")),
    }
}

/// One agent: its state, pending items and last [`DIAG_LINES`] diagnostics
/// lines. `query` is a full agent id or a path (`executor`, `T-0003`,
/// `T-0003/lint`) that names one agent.
pub fn agent(data_dir: &Path, query: &str) -> Result<String, String> {
    let st = crate::store::load_dir(data_dir).map_err(|e| e.to_string())?;
    let a = find(&st, query)?;
    let mut out = String::new();
    describe(&mut out, a);
    pending(&mut out, a, &st.inbox);
    let id = a.id.to_string();
    let lines = diag_lines(data_dir, &id, DIAG_LINES);
    if lines.is_empty() {
        let _ = writeln!(out, "\nno diagnostics lines for {id}");
    } else {
        let _ = writeln!(out, "\nlast {} diagnostics lines:", lines.len());
        for l in lines {
            let _ = writeln!(out, "  {l}");
        }
    }
    Ok(out)
}

fn find<'a>(st: &'a State, query: &str) -> Result<&'a Agent, String> {
    let all = || st.plans.iter().flat_map(|p| &p.agents);
    let hits: Vec<&Agent> = all()
        .filter(|a| a.id.to_string() == query || a.path.to_string() == query)
        .collect();
    match hits.as_slice() {
        [a] => Ok(a),
        [] => {
            let known: Vec<String> = all().map(|a| a.id.to_string()).collect();
            Err(if known.is_empty() {
                format!("no agent {query:?}: there are no agents")
            } else {
                format!("no agent {query:?}; agents: {}", known.join(", "))
            })
        }
        many => {
            let ids: Vec<String> = many.iter().map(|a| a.id.to_string()).collect();
            Err(format!(
                "{query:?} names more than one agent; use the full id: {}",
                ids.join(", ")
            ))
        }
    }
}

fn describe(out: &mut String, a: &Agent) {
    let life = match a.life {
        Life::Starting => "starting".to_string(),
        Life::Running => "running".to_string(),
        Life::Exited { code: Some(c) } => format!("exited (code {c})"),
        Life::Exited { code: None } => "exited (signal)".to_string(),
        Life::Stopped => "stopped".to_string(),
    };
    let activity = serde_json::to_value(a.activity)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let report = a.report.as_ref().map_or("none".to_string(), |r| {
        let status = serde_json::to_value(r.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        format!("{status} (attempt {}): {}", r.attempt, short(&r.summary))
    });
    let _ = writeln!(out, "agent {}", a.id);
    let _ = writeln!(out, "  life:      {life}");
    let _ = writeln!(out, "  activity:  {activity}");
    if a.stalled {
        let _ = writeln!(out, "  stalled:   yes");
    }
    let _ = writeln!(out, "  report:    {report}");
    let _ = writeln!(out, "  session:   {}", a.session_id);
    let _ = writeln!(out, "  cwd:       {}", a.cwd.display());
    if let Some(t) = &a.term {
        let _ = writeln!(out, "  terminal:  {t}");
    }
    let t = &a.tokens;
    let _ = writeln!(
        out,
        "  tokens:    {} in, {} out, {} cache read, {} cache write",
        t.input, t.output, t.cache_read, t.cache_write
    );
}

/// Items waiting for the agent, and items it raised that wait for someone.
fn pending(out: &mut String, a: &Agent, inbox: &[InboxItem]) {
    let to = pending_for(&a.id, inbox);
    let from: Vec<&InboxItem> = inbox
        .iter()
        .filter(|i| i.is_pending() && i.from.as_ref() == Some(&a.id))
        .collect();
    let _ = writeln!(out, "\npending items for it: {}", to.len());
    for i in to {
        let _ = writeln!(out, "  {}", item_line(i));
    }
    let _ = writeln!(out, "pending items from it: {}", from.len());
    for i in from {
        let _ = writeln!(out, "  {}", item_line(i));
    }
}

fn item_line(i: &InboxItem) -> String {
    let to = match &i.to {
        Recipient::Agent(a) => a.to_string(),
        Recipient::Human => "human".to_string(),
    };
    let what = match &i.kind {
        ItemKind::Permission { tool, .. } => format!("permission for {tool}"),
        k => kind_label(k).to_string(),
    };
    let from = i
        .from
        .as_ref()
        .map_or("human".to_string(), |a| a.to_string());
    format!("{} {what}, {from} -> {to}: {}", i.id, short(&i.body))
}

/// The first line, cut to 60 chars.
fn short(s: &str) -> String {
    let line = s.lines().next().unwrap_or("");
    let mut cut: String = line.chars().take(60).collect();
    if cut.len() < line.len() || s.lines().nth(1).is_some() {
        cut.push('…');
    }
    cut
}

/// The last `n` lines of `cmuxd.jsonl.1` then `cmuxd.jsonl` whose `agent`
/// field is `id`.
pub fn diag_lines(data_dir: &Path, id: &str, n: usize) -> Vec<String> {
    let dir = data_dir.join("diagnostics");
    let mut hits = Vec::new();
    for name in ["cmuxd.jsonl.1", "cmuxd.jsonl"] {
        let Ok(text) = std::fs::read_to_string(dir.join(name)) else {
            continue;
        };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if v["agent"].as_str() == Some(id) {
                hits.push(line.to_string());
            }
        }
    }
    let skip = hits.len().saturating_sub(n);
    hits.split_off(skip)
}

#[cfg(test)]
mod tests {
    use cmux_core::ids::{AgentId, AgentPath, ItemId, PlanId, TaskId};
    use cmux_core::model::{Activity, Delivery, Plan, Report as AgentReport, ReportStatus};

    use super::*;
    use crate::store::Store;

    fn plan_id() -> PlanId {
        PlanId::new(2026, 10, 18, "doctor test")
    }

    fn agent_id(path: AgentPath) -> AgentId {
        AgentId::new(plan_id(), path)
    }

    fn item(
        id: u64,
        to: Recipient,
        kind: ItemKind,
        from: Option<AgentId>,
        body: &str,
    ) -> InboxItem {
        InboxItem {
            id: ItemId(id),
            to,
            kind,
            from,
            body: body.into(),
            status: Delivery::Pending,
            dedupe: None,
            created: id,
        }
    }

    /// A data dir with one plan, two agents, an inbox and a fake
    /// diagnostics log split over the rotated and the current file.
    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let task = agent_id(AgentPath::Task(TaskId::new(3)));
        let exec = agent_id(AgentPath::Executor);
        let mut plan = Plan::new(plan_id(), "Doctor test", vec![]);
        let mut a = cmux_core::model::Agent::new(
            task.clone(),
            "0b5c-session",
            "/work/T-0003".into(),
            [1; 32],
        );
        a.activity = Activity::NeedsPermission;
        a.report = Some(AgentReport {
            status: ReportStatus::Blocked,
            summary: "waiting for the API key\nsecond line".into(),
            pr_url: None,
            attempt: 1,
            implicit: false,
            supersedes_blocked: false,
        });
        a.tokens.input = 1200;
        a.tokens.output = 340;
        plan.agents.push(a);
        plan.agents.push(cmux_core::model::Agent::new(
            exec.clone(),
            "exec-session",
            "/work".into(),
            [2; 32],
        ));
        let mut answered = item(
            4,
            Recipient::Agent(task.clone()),
            ItemKind::UserMessage,
            None,
            "old",
        );
        answered.status = Delivery::Dismissed;
        let inbox = crate::store::InboxFile {
            inbox: vec![
                item(
                    1,
                    Recipient::Agent(task.clone()),
                    ItemKind::UserMessage,
                    None,
                    "please rebase on main",
                ),
                item(
                    2,
                    Recipient::Human,
                    ItemKind::Permission {
                        tool: "Bash".into(),
                        input_summary: "rm -rf target".into(),
                    },
                    Some(task.clone()),
                    "Bash: rm -rf target",
                ),
                item(
                    3,
                    Recipient::Agent(exec.clone()),
                    ItemKind::ChildQuestion,
                    Some(task.clone()),
                    "Which base branch?",
                ),
                answered,
            ],
            next_item_id: 5,
        };
        {
            let s = Store::open(tmp.path()).unwrap();
            s.save_plan(&plan).unwrap();
            let g = crate::store::Globals::of(&State::default());
            s.save_globals(&g, &inbox).unwrap();
        }
        let diag = tmp.path().join("diagnostics");
        std::fs::create_dir_all(&diag).unwrap();
        let line = |i: u32, agent: &AgentId| {
            format!(
                r#"{{"agent":"{agent}","event":"agent_tool","level":"info","ms":{i},"tool":"add_note","ts":{}}}"#,
                1_000 + i
            )
        };
        let mut old = String::new();
        let mut cur = String::new();
        for i in 0..40 {
            old.push_str(&line(i, &task));
            old.push('\n');
            old.push_str(&line(i, &exec));
            old.push('\n');
        }
        old.push_str("not json\n");
        for i in 40..60 {
            cur.push_str(&line(i, &task));
            cur.push('\n');
            cur.push_str(r#"{"event":"log","level":"info","msg":"x","ts":1}"#);
            cur.push('\n');
        }
        std::fs::write(diag.join("cmuxd.jsonl.1"), old).unwrap();
        std::fs::write(diag.join("cmuxd.jsonl"), cur).unwrap();
        tmp
    }

    #[test]
    fn agent_output() {
        let tmp = fixture();
        let out = agent(tmp.path(), "T-0003").unwrap();
        insta::assert_snapshot!(out);
    }

    #[test]
    fn agent_takes_the_last_50_lines_across_rotation() {
        let tmp = fixture();
        let id = agent_id(AgentPath::Task(TaskId::new(3))).to_string();
        let lines = diag_lines(tmp.path(), &id, DIAG_LINES);
        assert_eq!(lines.len(), 50);
        assert!(lines[0].contains(r#""ms":10,"#), "{}", lines[0]);
        assert!(lines[49].contains(r#""ms":59,"#));
        assert!(lines.iter().all(|l| l.contains(&id)));
    }

    #[test]
    fn agent_by_full_id_and_unknown() {
        let tmp = fixture();
        let full = agent_id(AgentPath::Executor).to_string();
        let out = agent(tmp.path(), &full).unwrap();
        assert!(out.starts_with(&format!("agent {full}\n")), "{out}");
        assert!(out.contains("I-3 question"));
        let err = agent(tmp.path(), "T-0099").unwrap_err();
        assert!(
            err.contains("no agent \"T-0099\"") && err.contains(&full),
            "{err}"
        );
    }

    #[test]
    fn agent_with_no_diagnostics_file() {
        let tmp = fixture();
        std::fs::remove_dir_all(tmp.path().join("diagnostics")).unwrap();
        let out = agent(tmp.path(), "executor").unwrap();
        assert!(out.contains("no diagnostics lines for"), "{out}");
    }

    struct Ok200;
    impl Exec for Ok200 {
        fn run(&self, _: &str, args: &[&str]) -> Result<crate::setup::Output, String> {
            let stdout = if args == ["auth", "status"] {
                r#"{"loggedIn":true,"authMethod":"claude.ai"}"#
            } else {
                "2.1.0 (Claude Code)"
            };
            Ok(crate::setup::Output {
                ok: true,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn system_without_a_server() {
        let tmp = fixture();
        let cfg = tmp.path().join("config.toml");
        std::fs::write(&cfg, "listen = \"off\"\n").unwrap();
        let o = Overrides {
            config: Some(cfg),
            data_dir: Some(tmp.path().to_path_buf()),
            socket: Some(tmp.path().join("api.sock")),
            listen: None,
        };
        let r = system(&o, &Ok200).await;
        let text = r
            .render()
            .replace(&tmp.path().display().to_string(), "$DATA");
        let socket = text.lines().find(|l| l.contains("api.sock")).unwrap();
        assert!(
            socket.starts_with("[FAIL] no server on $DATA/api.sock ("),
            "{text}"
        );
        let rest: Vec<&str> = text.lines().filter(|l| !l.contains("api.sock")).collect();
        insta::assert_snapshot!(rest.join("\n"));
        assert_eq!(r.failed(), 1);
    }
}
