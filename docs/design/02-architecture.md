# cmux: architecture and technical design (client-server)

Status: draft 2, 2026-10-18. This draft replaces draft 1, which was a single-process TUI. Part 2 of 3. Previous: [01-ui-design.md](01-ui-design.md). Next: [03-implementation-plan.md](03-implementation-plan.md).

## 1. Goals and constraints

1. **Client-server from the start.** A daemon, `cmuxd`, runs on a VPS or a Linux PC and owns every Claude session, plan, task and terminal. Agents keep working with no client attached. Clients (TUI, CLI, web, mobile PWA) connect from anywhere.
2. **Rust where it counts.** The server, the TUI and the CLI are Rust and grow out of claude-mux. The web and mobile client is a small TypeScript app, because a browser needs JavaScript for the terminal widget (xterm.js). It is embedded in the server binary.
3. **Lean.**
   - Two binaries: `cmuxd` (server) and `cmux` (client).
   - A single-user scale target: up to about 20 agents and 5 clients.
   - Idle CPU near zero.
   - Only terminals a client is looking at are streamed.
4. **Claude only.** Every agent is an interactive `claude` process on a server PTY. There is no Agent SDK and no headless mode.
5. **Three levels**: the plan executor, task executors, and sub-agents (UI design section 1).
6. **Safe by default.** The server binds to localhost, and every remote client needs a paired device token. Remote access is meant to go through a private network or TLS.

### 1.1 Taken from the orchestrator, and dropped

| Kept | Dropped |
|---|---|
| Plan, task, criterion, note and dependency model, with the same state names and transition tables | Postgres; `run_inputs`/`run_turns` scheduling; the worker channel; Sprites and containers |
| One branch (`claude/<task-id>`) and one worktree (`<repo>/.worktrees/<task-id>`) per task | Personas as database rows. They become one prompt file per level |
| `report_result` / `ask_parent` semantics: one terminal report per attempt, and an implicit result if an agent stops without one | CodeAct, QuickJS, the subscription API |
| "Status is decided at turn end" | The 10-status run machine |
| A visible distinction between pending and delivered input (the lesson of run 288) | Dollar budgets. Tokens are shown instead |
| A progress watchdog | The CI autofix platform loop. The task agent owns its PR |
| The PR → task-state mapping | GitHub webhooks. A `gh` poller is enough |
| REST plus a push stream for clients, as in the orchestrator TUI's REST + SSE | SSE: a WebSocket is needed anyway for terminal input |

The orchestrator folded levels 2 and 3 into one run, because each child cost a container. Here a child is a PTY and a worktree, which is cheap. The real limit is server memory and parallel heavy checks, so `cmuxd` enforces concurrency limits instead (section 10).

### 1.2 Why not ssh and tmux

ssh and tmux give remote terminals, but nothing else this design needs:
- the structure of plans and tasks,
- answering a permission from a phone,
- push notifications,
- a chat view that fits a phone screen,
- a server that knows when an agent is waiting.

The TUI keeps tmux's keys, so it costs nothing to switch.

## 2. System overview

```
 ┌────────────── clients (anywhere) ──────────────┐
 │  cmux TUI   cmux CLI   web app   phone (PWA)    │
 └──────┬─────────┬──────────┬──────────┬──────────┘
        │  HTTPS: REST /v1/*   +   WebSocket /v1/stream      (paired device token)
        │  (local clients may use the Unix socket instead: no token, uid-checked)
 ┌──────▼──────────────────────── cmuxd (VPS / Linux PC) ───────────────────────────────┐
 │                                                                                      │
 │  api (axum) ──commands──▶ engine actor ──effects──▶ effect runner                    │
 │     ▲  │                  (owns State; pure core)     │  spawn/kill PTY, write PTY,  │
 │     │  └─ subscriptions ◀── state deltas (rev) ◀──────┘  git worktrees, gh, push     │
 │     │                                                                                │
 │  terminal hub: per PTY ─ vt100 parser · 1 MiB ring (seq) · subscribers · size owner  │
 │  transcript tailer: ~/.claude/projects/**/<session>.jsonl → chat entries             │
 │  store: plans/<id>/{plan.md,state.json,events.jsonl} · devices.json · push subs      │
 │  agent endpoint (Unix socket only): /v1/agent/{hook,tool}                            │
 └──────┬───────────────────────────────────────────▲───────────────────────────────────┘
        │ PTY                                        │ HTTP over Unix socket
 ┌──────▼───────────────────────┐     ┌──────────────┴──────────────┐
 │ claude (interactive)         │────▶│ cmuxd mcp   (stdio MCP)      │  one per agent
 │ env CMUX_AGENT, CMUX_TOKEN   │────▶│ cmuxd hook <event>           │  one per hook call
 └──────────────────────────────┘     └──────────────────────────────┘
```

## 3. Repository and crate layout

`georgi/claude-mux` becomes a Cargo workspace. The claude-mux code is split between the server, which takes PTYs and the transcript reader, and the client, which takes layout, keys, drawing and input.

```
claude-mux/
  Cargo.toml                 [workspace]
  crates/
    cmux-core/               pure, no I/O, no async
      model.rs               Plan, Task, Criterion, Note, Agent, Terminal, InboxItem, states + tables
      engine.rs              (State, Command, now) -> Result<(Reply, Vec<Effect>, Vec<Delta>)>
      delivery.rs            how and when pending items reach an agent
      permissions.rs         per-task allow rules for remote approvals
      prompts.rs + prompts/  executor.md task.md subagent.md kickoff-*.md (include_str!)
      ids.rs                 P-/T- ids, agent paths
    cmux-proto/              wire types shared by server, TUI, CLI; exported to TypeScript
      api.rs                 REST request/response bodies
      stream.rs              WebSocket messages and binary frame layout
      view.rs                client-facing snapshots: PlanView, TaskView, AgentView, ItemView
    cmuxd/                   the server binary (tokio)
      main.rs                serve | mcp | hook | pair | doctor
      api/                   rest.rs, stream.rs, auth.rs, static.rs (embedded web app)
      actor.rs               engine actor: mpsc in, deltas out, effect dispatch
      term/                  pty.rs (from claude-mux pane.rs), hub.rs (ring, subscribers, size owner)
      claude/                spawn.rs (argv, env, settings, mcp config), transcript.rs (from history.rs)
      agent/                 hook.rs, mcp.rs (stdio MCP server), endpoint.rs (/v1/agent/*)
      git/                   worktree.rs, github.rs (gh poller)
      store.rs  devices.rs  push.rs  watchdog.rs  diag.rs  config.rs
    cmux/                    the client binary (no async runtime)
      main.rs                tui (default) | CLI verbs | login | servers
      conn/                  http.rs (ureq), stream.rs (tungstenite thread), servers.rs (client.toml)
      tui/                   app.rs, input.rs, mouse.rs, keys.rs, layout.rs, window.rs,
                             remote_term.rs (local vt100 fed by the stream), draw/{tabs,panes,tree,board,inbox,status,overlays}.rs
      cli/                   one file per verb group
  web/                       Vite + Preact + TypeScript + xterm.js
    src/api/types.ts         generated from cmux-proto with ts-rs
    src/views/               tree, agent (terminal | chat), inbox, board, settings, pair
    src/sw.ts                service worker: offline shell, Web Push
```

### 3.1 Dependencies

| Crate | Adds |
|---|---|
| cmux-core | `serde` |
| cmux-proto | `serde`, `serde_json`, `ts-rs` (build-time export only) |
| cmuxd | `tokio`, `axum` (with `ws`), `tower-http` (gzip, static files), `portable-pty`, `vt100`, `serde_json`, `toml`, `uuid`, `rand`, `sha2`, `base64`, `rust-embed`, `tracing` + `tracing-appender`, `qrcode`; behind features: `web-push`, `rustls` (via `axum-server`) |
| cmux | `ratatui`, `vt100`, `tungstenite` (rustls), `ureq`, `serde_json`, `toml`, `base64`, `unicode-width`, `libc` |

The client stays synchronous, using threads and an `mpsc` loop as claude-mux does today. Only the server, which must serve many sockets, uses tokio.

## 4. Data model (cmux-core)

```rust
pub struct State { pub rev: u64, pub plans: Vec<Plan>, pub loose: Vec<TermId>,
                   pub terminals: HashMap<TermId, Terminal>, pub inbox: Vec<InboxItem> }

pub struct Plan {
    pub id: PlanId,                 // "P-20261018-auth-refactor"
    pub title: String,
    pub state: PlanState,           // Draft | Proposed | Accepted | Done | Cancelled
    pub repos: Vec<Repo>,           // repos[0] is where the executor runs
    pub paused: bool,
    pub limits: Limits,
    pub tasks: Vec<Task>,
    pub agents: Vec<Agent>,
    pub next_task_seq: u32,
}

pub struct Task {
    pub id: TaskId, pub title: String, pub body: String, pub state: TaskState,
    pub repo: usize, pub deps: Vec<TaskId>, pub criteria: Vec<Criterion>, pub notes: Vec<Note>,
    pub branch: Option<String>, pub worktree: Option<PathBuf>, pub pr: Option<PrRef>,
    pub agent: Option<AgentId>, pub attempt: u32,
    pub allow_rules: Vec<AllowRule>,   // "allow always for this task" answers
}

pub struct Agent {
    pub id: AgentId, pub path: AgentPath,        // "executor" | "T-0003" | "T-0003/tests"
    pub level: Level,                            // Executor | Task | Sub
    pub parent: Option<AgentId>, pub task: Option<TaskId>,
    pub session_id: String,                      // fixed at spawn with --session-id
    pub term: Option<TermId>, pub cwd: PathBuf,
    pub token_hash: [u8; 32],                    // authenticates its hook and MCP calls
    pub life: Life,                              // Running | Exited{code} | Stopped
    pub activity: Activity,                      // Working | Idle | NeedsPermission | Unknown
    pub report: Option<Report>,
    pub tokens: TokenUsage,                      // from the transcript
}

pub struct Terminal { pub id: TermId, pub kind: TermKind /* Claude|Shell|Command(label) */,
                      pub agent: Option<AgentId>, pub cwd: PathBuf, pub size: (u16, u16),
                      pub size_owner: Option<ClientId>, pub title: Option<String> }

pub struct InboxItem {
    pub id: ItemId, pub to: Recipient,           // Agent(AgentId) | Human
    pub kind: ItemKind,                          // ChildReport | ChildQuestion | ChildExited | UserMessage | Steer
                                                 // | PrChanged | Permission{tool, input_summary} | Question
                                                 // | PlanReview | Stalled | ExitedWithoutReport
    pub from: Option<AgentId>, pub body: String,
    pub status: Delivery,                        // Pending | Delivered{at, via} | Answered{by: ClientId, answer} | Dismissed | Expired
    pub dedupe: Option<String>, pub created: Timestamp,
}
```

- The task and plan transition tables are the orchestrator's (`lib/types.ts`), including the rule that any PR-backed state may move to any other.
- `merged` requires every criterion to be checked, unless the move comes from the GitHub poller.
- An illegal edge is an error, and the error lists the allowed edges.
- The display glyph is a pure function of the agent, its inbox and its children. It is never stored.

### 4.1 Revisions and deltas

Every engine step that changes `State` increments `rev` and emits `Delta`s. Each delta replaces a whole object:

```rust
pub enum Delta { Plan(PlanView), Task(PlanId, TaskView), Agent(AgentView), Terminal(TermView),
                 Item(ItemView), Removed(ObjectRef) }
```

Whole-object replacement is simpler than patches and small at this scale. A task view is under 2 KB without its body; bodies and notes are fetched on demand.

## 5. Server process (cmuxd)

### 5.1 Tasks and threads

- **Engine actor**: one tokio task that owns `State`. Its input is an `mpsc<Command>` from the API, the agent endpoint, the effect runner, the timers, and the gh poller. For each command it calls `cmux_core::engine::apply`, stores the deltas in a ring of the last 10 000, and broadcasts them to stream subscribers. It hands effects to the runner. It never awaits I/O, so it is never blocked.
- **Effect runner**: executes `Effect`s:
  - spawning and killing PTYs,
  - writing input to a PTY,
  - git worktree operations on `spawn_blocking`, at most 2 at a time,
  - writing hook responses,
  - sending push notifications,
  - persisting.
  
  It reports outcomes back to the engine as commands, for example `WorktreeReady` and `Spawned`.
- **PTY readers**: one blocking thread per PTY, as in claude-mux. Each one feeds the terminal hub.
- **Timers**: a 1 s ticker drives the watchdog, question timeouts, permission timeouts, the store flush and the transcript poll.

### 5.2 Terminal hub (term/hub.rs)

Each terminal has:
- a `vt100::Parser` (server-side screen state, used for snapshots, `peek` and the title);
- a ring of the last 1 MiB of raw output, indexed by a byte sequence number `seq`;
- a subscriber list;
- the size and the size owner.

The PTY reader appends to the ring, feeds the parser, and notifies subscribers. Output to each subscriber is batched every 16 ms, so a burst of spinner redraws costs one frame per tick, which is the same coalescing as claude-mux's `dirty` flag.

When a client subscribes with `since_seq`:
- if `since_seq` is still in the ring, it receives the bytes after it;
- otherwise it receives a **snapshot**: `screen.state_formatted()`, which is the escape sequence that redraws the screen, cursor and modes, followed by live bytes.

Scrollback beyond the screen is fetched on demand, with `GET /v1/terminals/:id/scrollback`.

**Size.** The client that last sent input, or a `focus` or `resize` message, becomes the size owner (tmux's `window-size latest`). A change of owner resizes the PTY and the parser, and broadcasts `size{term, rows, cols, owner}`. Other clients render at that size.

**Memory.** The ring is 1 MiB per terminal. vt100 scrollback is 10 000 lines for executors and tasks, and 2 000 for sub-agents and loose shells. Both are configurable. RSS per agent is reported on the server status page.

### 5.3 Transcript tailer (claude/transcript.rs)

This serves the chat view. For each agent that has at least one chat subscriber, the server tails `~/.claude/projects/<dir>/<session_id>.jsonl`. It polls the file size every 500 ms and parses only the bytes it has not yet read.

Each line becomes a `ChatEntry`:
- `{uuid, ts, role: user|assistant, kind: text|tool_call|tool_result, text, tool?: {name, summary}}`;
- tool results are cut to 400 characters, with the full text on request;
- thinking blocks are skipped;
- token usage is summed into `agent.tokens`.

The parsing reuses claude-mux's `history.rs` rules: skip meta lines, skip `<`-wrapped injections, and read titles. The same code produces the implicit report (the last assistant text) when an agent exits without reporting.

### 5.4 Store (store.rs)

```
~/.local/share/cmuxd/
  plans/<plan-id>/plan.md          free markdown; the executor edits it with native tools
  plans/<plan-id>/state.json       plan, tasks, agents (no secrets); atomic temp+fsync+rename, debounced 200 ms
  plans/<plan-id>/events.jsonl     append-only journal of commands and deliveries (audit, debugging)
  plans/<plan-id>/agents/<id>/{settings.json,mcp.json,system.md}
  inbox.json  loose.json  devices.json (token hashes)  push.json  vapid.key (0600)
  diagnostics/cmuxd.jsonl          rotating, 5 MiB × 2
~/.config/cmuxd/config.toml
$XDG_RUNTIME_DIR/cmuxd/api.sock     0600 in a 0700 directory
```

A `flock` on the data directory ensures that only one `cmuxd` runs per user.

### 5.5 Running it

- Install with `cargo install`, or as a static release binary.
- Run `cmuxd setup` once:
  - it writes a systemd **user** unit;
  - it runs `loginctl enable-linger`, so the server survives logout;
  - it checks that `claude` and `gh` are logged in for this user.
- `cmuxd` runs as that user. All agents inherit its Claude login and its `gh` auth.
- On start, agents that were `Working` when the server stopped are resumed with `claude --resume <session_id>` and a note (`[cmux] resumed after a server restart`). Idle agents stay stopped until a client resumes them. A config switch turns auto-resume off.

## 6. Client protocol (cmux-proto)

The same API is served on the TCP listener (remote clients, token required) and on the Unix socket (local clients, uid-checked).

### 6.1 REST (`/v1`)

| Method and path | Purpose |
|---|---|
| `GET /state` | Full snapshot of plans, tasks (without bodies), agents, terminals and items, plus `rev` |
| `POST /plans` · `POST /plans/:id/{accept,pause,resume,cancel}` | Plan lifecycle |
| `GET/PUT /plans/:id/body` | `plan.md` |
| `POST /plans/:id/tasks` · `GET/PATCH /tasks/:plan/:task` | Tasks, including body, notes and criteria |
| `POST /agents/:id/messages {text, steer}` | Queue or steer |
| `POST /agents/:id/{stop,resume}` · `GET /agents/:id/transcript?after=` | Agent control, chat history |
| `POST /items/:id/answer {action, text?}` | `allow`, `allow_always`, `deny`, `reply`, `accept`, `send_back`, `dismiss` |
| `POST /terminals {kind, cwd, cmd?}` · `DELETE /terminals/:id` · `GET /terminals/:id/scrollback?lines=` | Loose terminals (shell, lazygit, plain Claude) and helper panes (`$EDITOR`, diff) |
| `GET/POST /repos` | Repositories known to the server; `POST` with a URL runs `git clone` |
| `POST /pair/start` (local or full-scope only) · `POST /pair/complete` · `GET/DELETE /devices/:id` | Pairing and devices |
| `POST /push/subscribe` · `GET /push/vapid` | Web Push |
| `GET /status` | Version, uptime, load, RSS per agent |

Mutating requests accept an `Idempotency-Key` header, so a phone retrying on a flaky connection cannot approve twice or send a message twice.

### 6.2 WebSocket (`/v1/stream`)

There is one socket per client. Text frames carry JSON control messages, and binary frames carry terminal bytes.

Client → server:
```
hello      { client: "tui"|"web"|"cli", since_rev?, terms: [{id, since_seq}] }
sub_term   { id, since_seq? }          unsub_term { id }
focus      { term }                    resize     { term, rows, cols }
sub_chat   { agent, after? }           unsub_chat { agent }
binary:    [0x01][term_id u32][bytes]  // keyboard input to a PTY
```

Server → client:
```
welcome    { rev, server: {version, name}, client_id }
state      { rev, snapshot }           // when since_rev is too old or missing
deltas     { rev, items: [Delta] }     // in order, with no gaps
term_reset { id, seq, rows, cols }     // followed by a binary snapshot frame
size       { term, rows, cols, owner }
chat       { agent, entries: [ChatEntry] }
presence   { clients: [{id, kind, device}] }
binary:    [0x02][term_id u32][seq u64][bytes]   // PTY output
```

**Reconnect.** The client sends its last `rev` and each terminal's last `seq`. The server answers with deltas and bytes from its rings, or with snapshots if those have rolled over. Nothing else needs to be resent, so a phone that drops off for a minute resumes in one round trip.

**Bandwidth.** Only terminals a client subscribes to are streamed. The TUI subscribes to the panes on screen; the web app to the open agent. The tree, board and needs-you list live entirely on deltas, which are small. The chat view uses transcript entries, which cost a few KB per turn. That makes the phone experience cheap even on a mobile network. WebSocket compression (`permessage-deflate`) is enabled where both sides support it.

### 6.3 TypeScript types

`cmux-proto` derives `ts_rs::TS` on every wire type. `cargo test -p cmux-proto` writes `web/src/api/types.ts`, and CI fails if the generated file differs from the committed one. The web app therefore cannot drift from the server.

## 7. Agents (unchanged in spirit from draft 1, now inside cmuxd)

### 7.1 Spawning

```
claude --session-id <uuid>
       --settings  <data>/plans/<plan>/agents/<id>/settings.json      (hooks only)
       --mcp-config=<data>/plans/<plan>/agents/<id>/mcp.json          (one server: "cmux"; "=" form, the flag is variadic)
       --append-system-prompt "<prompts/{executor,task,subagent}.md, filled>"
       [--permission-mode <per level>] [--model <per level>]
       "<kickoff prompt>"
env:   CMUX_SOCK=<api.sock>  CMUX_AGENT=<id>  CMUX_TOKEN=<random 256-bit>  TERM=xterm-256color
cwd:   executor → repos[0]; task → its worktree; sub-agent → its task's worktree
```

- claude-mux's `SESSION_ENV` scrub stays.
- The token is held only in the agent's environment and in its `mcp.json` (mode 0600). The server stores only its hash.
- The user's own MCP servers stay available, because the server does not pass `--strict-mcp-config`.

**Worktrees.** The procedure is the orchestrator's `localWorktreeFor`, reduced:
1. Reuse the task's worktree if it exists.
2. Reuse any checkout of `claude/<task-id>`.
3. Otherwise run `git worktree add -b claude/<task-id> <repo>/.worktrees/<task-id> origin/<base>`, or add without `-b` when the branch already exists.
4. Add `.worktrees/` to `.git/info/exclude`.

Removal is explicit only: on cancel or plan completion, with a clean status and a merged branch.

### 7.2 Hooks: exact status, remote permissions

Each agent's `settings.json` registers `cmuxd hook <event>` for these events: `SessionStart`, `UserPromptSubmit`, `Stop`, `Notification`, `PreToolUse`, `PermissionRequest`, and `SessionEnd`. The hook binary posts to `/v1/agent/hook` on the Unix socket. If the socket is gone, it exits 0 silently, so a hook can never wedge Claude.

| Hook | Effect |
|---|---|
| `SessionStart` | Confirms `session_id` (it changes after `/clear`); `life = Running` |
| `UserPromptSubmit` | `activity = Working`; confirms PTY deliveries; returns pending items as `additionalContext` (delivery path C) |
| `Stop` | `activity = Idle`; lands the turn (section 8.3); may answer `{"decision":"block","reason":<digest>}` (path A) |
| `PermissionRequest` | **Remote approval.** If a task allow-rule matches, answer allow at once. Otherwise create a `Permission` item, push it to clients, and hold the hook open until a client answers or `permission_wait` passes (default 10 min, the hook's `timeout` set to match). The answer is returned as the hook's decision. On timeout the hook returns no decision, so Claude shows its normal prompt in the terminal |
| `Notification` | A fallback when a permission prompt shows without `PermissionRequest`: `activity = NeedsPermission` and a needs-you item. Answering it then sends the prompt's keys to the PTY (spike S6) |
| `PreToolUse` | Watchdog progress. Policy guard rails: a sub-agent may not run `git push`, and nobody may check out the base branch in a task worktree |
| `SessionEnd` | `life = Exited`; synthesizes an implicit report if there is none. Ignored for `reason: "clear"` (`/clear` keeps the process; `SessionStart` follows) |

**Allow always for this task** adds an `AllowRule {tool, pattern}` to the task. The rule is matched in the hook, and it never touches the user's Claude settings.

### 7.3 MCP tools, by role

`cmuxd mcp` is a stdio JSON-RPC 2.0 server, about 300 lines, with no SDK. It supports `initialize`, `tools/list`, `tools/call` and `ping`, and forwards calls to `/v1/agent/tool`. The tool list depends on the agent's level.

| Tool | Executor | Task | Sub-agent |
|---|:-:|:-:|:-:|
| `get_plan`, `update_plan` | ✓ | read | |
| `list_tasks`, `get_task` | ✓ | ✓ | own task |
| `create_task`, `update_task`, `transition_task` | ✓ | own: testing, blocked | |
| `add_criterion`, `check_criterion`, `add_note` | ✓ | own | `add_note` only |
| `spawn_task(id, instructions?, force?)` | ✓ | | |
| `spawn_subagent(name, prompt)` | | ✓ | |
| `send(agent, text, interrupt?)`, `stop_agent`, `list_agents`, `peek(agent, lines?)` | children | children | |
| `set_task_pr(url)` | | ✓ | |
| `report_result(status, summary, pr_url?)` | | ✓ | ✓ |
| `ask_parent(question)` | | ✓ | ✓ |
| `answer_child(agent, answer)` | ✓ | ✓ | |
| `ask_user(question)` | ✓ | ✓ | |

There is deliberately no "await" tool. An agent with work in flight ends its turn, and the server wakes it. Results are short plain text, and errors start with `error:` and name what is allowed.

## 8. Delivery between levels

### 8.1 Paths

| Path | When | How |
|---|---|---|
| **A. Stop continuation** | The agent is ending a turn with items pending | The `Stop` hook answers `decision: block` with the digest. At most 3 in a row without a tool call; then path B |
| **B. PTY injection** | The agent is idle, with no permission prompt, and no client has typed into it in the last 3 s | Bracketed paste of the digest, then `\r`. Confirmed by the next `UserPromptSubmit` |
| **C. Prompt piggyback** | A human submits a prompt while items are pending | Returned as `additionalContext` |

A **steer** writes `Esc` to the PTY, waits for `Stop` (up to 10 s), then delivers. A **digest** groups everything pending, with item ids, so an agent can recognise one it has already seen (delivery is at-least-once).

### 8.2 Visibility

Every client shows pending items as `✉N`. `list_agents` and `peek` show them to agents.

### 8.3 Turn landing (on each `Stop`)

1. Deliver pending items through path A, and stop here.
2. A task or sub-agent with a report: post `ChildReport` to its parent, deduplicated as `report:<agent>:<attempt>`, and move the task. Success with a PR → `testing`; success without a PR → `passing`; `blocked` → `blocked`; `failed` → `failing`.
3. An agent with live children is parked (`◌`).
4. Otherwise it is idle. A task or sub-agent that stays idle with no report for 10 minutes becomes an `ExitedWithoutReport` needs-you item.

### 8.4 Prompts

The templates live in `cmux-core/prompts/` and are compiled in. A file of the same name in `~/.config/cmuxd/prompts/` overrides each one. They are adapted from the orchestrator's executor persona:

- **executor.md**: you own task states and sequencing for plan X. Spawn task agents with `spawn_task`, and don't implement tasks yourself unless the user asks. While you wait on agents, end your turn, and cmux wakes you. Keep notes on tasks. A claim needs evidence. Mark the plan done when every task is merged or cancelled.
- **task.md**: you own task T in worktree W on branch B, and you never switch branches. Commit, push, run `gh pr create`, then call `set_task_pr`. Run heavy checks yourself, one at a time. Use `spawn_subagent` for separable work on disjoint files. Finish with exactly one `report_result`. Use `ask_parent` for plan questions, and `ask_user` only for decisions a human must make.
- **subagent.md**: you have a narrow job for task T. Don't push, don't open PRs, don't change task state. Finish with `report_result`.
- **kickoff-plan.md**, **kickoff-execute.md**, **kickoff-task.md**: the first prompts. The execute kickoff lists every task as `- T-0003 [todo] ui deps:[T-0002] criteria:0/3`.

## 9. Security

| Concern | Design |
|---|---|
| Exposure | TCP binds to `127.0.0.1:7878` by default. The recommended remote setups are **Tailscale** (`tailscale serve` gives HTTPS with a valid certificate on the tailnet, which PWAs and Web Push require) or a reverse proxy with Let's Encrypt (Caddy). Built-in TLS (`rustls`, certificate files) is optional. `cmuxd` refuses to listen on a non-loopback address without TLS unless `allow_plain_http = true` |
| Pairing | `cmuxd pair`, run on the server or from a full-scope client, prints an 8-character code and a QR code holding the URL and the code. It is valid for 5 minutes, single use, with 5 attempts per minute. The client sends the code with a device name and receives a 256-bit token. The server stores only its SHA-256 hash, the scope (`full` or `read`), and the last-seen time |
| Web sessions | The web app trades its token for an `HttpOnly; Secure; SameSite=Strict` cookie. Mutating requests also need an `X-Cmux` header, which blocks CSRF. The WebSocket is authenticated by the cookie or by a token in `hello`, never in the URL |
| Agent endpoint | `/v1/agent/*` exists only on the Unix socket and accepts only agent tokens. A token acts only for its own agent; the role decides the tools. An agent can never reach the client API |
| What a full token grants | Shell-equivalent access to the server user: terminals, plus Claude with that user's permissions. The pairing screen says so. Use a dedicated Unix user on a shared machine |
| Permissions | `cmuxd` never loosens Claude's permission mode by itself. Remote approval answers Claude's own prompts; it does not bypass them |
| Logs | The diagnostics log never records prompts, message bodies, tool inputs or tokens |

## 10. Limits, GitHub, liveness

- **Limits** (in `config.toml` and per plan):
  - live task agents per plan: 2;
  - sub-agents per task: 2;
  - live Claude processes on the server: 8;
  - concurrent git operations: 2.
  
  A spawn over a limit is refused with a clear `error:` for the calling agent. A spawn from a client is queued and shown as `·`.
- **GitHub**:
  - The server polls `gh pr view --json state,mergedAt,statusCheckRollup,reviewDecision` every 60 s for open PRs, and every 15 s for tasks in `testing` whose agent is idle.
  - Results are mapped with the orchestrator's `prToTaskState`.
  - Changes go to the task agent and the executor. After 3 red-CI attempts, the item goes to you instead.
  - If `gh` is missing, PR tracking is off and the status page says so.
- **Watchdog**: an agent that is `Working` with no PTY output and no hook event for 15 minutes gets `⚠` and a needs-you item. The server never kills anything by itself.
- **Diagnostics**: one JSONL line per hook, tool call, delivery, spawn, exit, client connect and watchdog event, with timings and no bodies. `cmuxd doctor <agent>` prints an agent's last 50.

## 11. Clients

### 11.1 TUI and CLI (`cmux`)

- `conn/stream.rs` runs the WebSocket on its own thread and turns frames into the existing `Event` enum (`Delta`, `TermOutput(term, bytes)`, `TermReset`, `Size`, `Disconnected`), so claude-mux's event loop stays as it is.
- A `RemoteTerm` replaces `Pane`'s PTY. It holds a local `vt100::Parser`, which is fed by output frames. Keyboard bytes go out as input frames. The draw code is unchanged.
- **Windows and layout are client-side.** The client derives workspaces, windows and panes from the state: plan → workspace, task → window, agents → panes. Splits and zoom are kept per server in `~/.config/cmux/view-<server>.toml`.
- **Reconnect**: retry with backoff, capped at 10 s, and show the `◐` dot. On reconnect the client sends its `rev` and each terminal's `seq`.
- There is no local-echo prediction (mosh style) in v1. Typing latency is one round trip, as with ssh. Prediction can be added later inside `RemoteTerm`.
- `~/.config/cmux/client.toml` lists the servers: name, URL, token (file mode 0600), and which one is the default. The local Unix socket is used automatically when `cmuxd` runs on the same machine.

### 11.2 Web app and PWA

- Vite, Preact and TypeScript; about 2 000 lines expected. xterm.js is the terminal, with its fit and WebGL add-ons.
- The build is embedded into `cmuxd` with `rust-embed`, under the `web` feature, which is on by default. It is served at `/`.
- State is one store fed by `state` and `deltas` messages. Views are pure renders of that store.
- The phone layout is the same app, below 640 px wide (UI design section 4.2).
- The service worker caches the app shell and handles Web Push. Notification actions call `/items/:id/answer` directly.
- **Native mobile app**: not planned for v1. If it is needed later, the protocol is the same, and the orchestrator's existing Expo app in `mobile/` shows the shape of one.

### 11.3 Push notifications

- **Web Push** (VAPID keys are generated on first start) goes to each device subscription for the item kinds it opted into: permission, question, plan review, stalled, plan done.
- Optionally, an `ntfy` topic URL in `config.toml` gets the same events, for people who prefer the ntfy app to a PWA.
- Notifications carry the item id and a short text, never message bodies or tool inputs beyond a 60-character summary.

## 12. Open questions

1. **Server restarts kill PTYs.** Agents are resumed with `--resume`, but a turn in flight is lost. Keeping PTYs alive across a `cmuxd` upgrade would need a separate PTY-holder process. This is left out of v1.
2. **Several users.** The design is single-user. Teams would need accounts and per-plan access, which is out of scope.
3. **Executor model**: should the executor default to a cheaper model? This is a per-level config setting with no default.
4. **Other engines** (Codex and others): `claude/spawn.rs`, the hooks and the transcript reader are the engine seam.
