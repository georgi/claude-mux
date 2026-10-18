# cmux: plan for M3 to M5 (steps 9 to 16)

Status: draft 1, 2026-10-18. This file refines steps 9 to 16 of [03-implementation-plan.md](03-implementation-plan.md). It starts from the code on `main` at `2aa79da`, plus the uncommitted plain-session MCP work.

The rules of file 03 apply: one PR per step, sizes **S**/**M**/**L**, and every PR keeps `fmt`, `clippy -D warnings` and `cargo test --workspace` green.

## 1. Where we are

M2 is reached. Steps 0 to 8 are done and have an automated acceptance test (`37d66d8`).

| Step | State on `main` | Remaining work |
|---|---|---|
| 9 Sub-agents, questions | Engine, MCP tools, guard rails, timeouts, `prefix u`, `main-vertical` are done and tested | Dimmed finished panes, a guard-rail end-to-end test, layout tests |
| 10 Transcript, chat | Only session metadata in `cmux-history`; `TokenUsage` is never fed; a marked hook point in `deliver.rs:217` | All of it |
| 11 Web app | Cookie sessions, `X-Cmux` CSRF and cookie auth on the stream exist | All of the app; `ts-rs`; `rust-embed`; `/repos` |
| 12 PWA, push | `Effect::Notify` exists but the runner drops it (`runner.rs:287`) | All of it |
| 13 CLI verbs | `login`, `servers`, `logout`, `-s` | 14 verbs, `--json`, exit codes |
| 14 GitHub | `set_task_pr`, `pr_to_task_state`, `RunnerEvent::Pr` routing and the PR mark in the TUI | The `gh` poller, the red-CI cap, the CI mark |
| 15 Recovery, deploy | The idle-without-report watchdog, manual resume, a release build job in CI | Resume on start, delivery reset, the stall watchdog, `doctor`, `setup`, RSS, config sections |
| 16 Docs, perf, security | README covers M2 server mode; many auth tests | Full docs, perf runs, the security matrix |

## 2. Changed order (recommendation)

I recommend this order instead of the strict 9 → 10 → … → 16 order of file 03:

```
8.5 ── 9 (M3) ── 15a ── 13 ──┬── 14 ───────────────────────────┐
                             └── 10 ── 11a ── 11b ── 12 (M4) ──┴── 15b ── 16 (M5)
```

The reasons:

1. **Recovery comes early (15a).** Today a `cmuxd` restart leaves every agent dead and every unconfirmed delivery stuck. This is the largest daily pain once M2 is in use. It does not need the web app.
2. **CLI verbs come early (13).** They are small, they need no new server API, and they give scripts and agents a driver for the later steps. `cmux tail --chat` moves to step 10, where the chat stream exists.
3. **Deployment comes late (15b).** `cmuxd setup` and release packaging need the embedded web app, so they wait for step 12.
4. **Two tracks after 13.** Step 14 (Rust only) and steps 10 to 12 (mostly TypeScript) touch different files. Two agents can take one each.

## Step 8.5: Land the plain-session tools (S)

The working tree holds finished but uncommitted work. Commit it first, so the later steps start from a clean `main`.

- `cmuxd mcp --session` (`crates/cmuxd/src/agent/session.rs`) and its wiring in `terminals.rs` and `claude/spawn.rs`.
- `CreatePlanReq.request: Option<String>`, the executor kickoff on accept, and the refusal to accept a plan with no tasks.
- `DELETE /v1/plans/{plan}`, `StoreWriter::delete_plan`, and the TUI `X` key.
- **Done when** the four green checks pass and the README matches the keys.

## Step 9: Close out sub-agents (S) → **M3**

The engine and tool work is done. Close the visible gaps.

- **Dimmed panes.** Read `PaneSpec.finished` in `ui.rs` `draw_pane_header` and the pane body. Draw a finished pane in the dim style. Show the header text from UI design 3.3: `✓ docs · reported success`, or `✗ name · failed`, or `○ name · exited`.
- **Header text source.** Add the report status to `AgentView` if it is not there, so the client does not guess it.
- Tests:
  - `insta` snapshots at 100×30 of a task window with 1, 2 and 3 sub-agents, one of them finished;
  - an end-to-end test in `tests/agents.rs`: a fake-claude sub-agent sends `PreToolUse` for `git push` through `cmuxd hook` and gets the deny;
  - an end-to-end test: a sub-agent `ask_parent` reaches the task agent, and `answer_child` reaches the sub-agent through path B.
- **Manual acceptance on the VPS.** A task agent spawns two sub-agents. One asks the task agent a question. The task agent asks you with `ask_user`. You answer from the needs-you overlay. Both sub-agents report, and their panes go dim.

## Step 15a: Recovery and the stall watchdog (M)

`cmuxd/src/server.rs`, `cmuxd/src/config.rs`, `cmux-core/src/engine/{deliver,spawn}.rs`.

- **New engine command `Recover { now }`.** The server sends it once after the store loads, before it accepts clients. It does three things:
  1. It marks every agent with `life = Running` as `Exited { reason: server_restart }`. The old PTYs are gone.
  2. It returns every unconfirmed delivery to `pending` (`Agent.unconfirmed`). Digests carry item ids, so a second delivery is safe.
  3. It emits `Effect::Resume` for agents that were `Working`, when `auto_resume` is on. The kickoff is `[cmux] resumed after a server restart; check the worktree and PR state before you continue`. Idle agents stay stopped. Sub-agents are never resumed automatically; the task agent gets a `ChildReport` with status `failed` and reason `server_restart`.
- **Resume uses the existing path.** `resume_agent` already builds `claude --resume <session_id>`. Reuse it; add only the note text.
- **Stall watchdog.** Track `last_progress` per agent: PTY output (sent by the hub as a coarse `Progress` command, at most once per 30 s per agent), every hook, and every tool call. On `Tick`, an agent that is `Working` with no progress for `timeouts.stall` (default 15 min) gets `Glyph::Stalled` and an `ItemKind::Stalled` item. Progress clears both. The server kills nothing.
- **Redraws are not progress.** A spinner redraws the screen without new content. Count a PTY chunk as progress only if the vt100 screen contents change outside the top status row. Compare a cheap hash of `screen.contents()` once per 30 s, not per chunk.
- **Config.** Add `[limits]` (task agents per plan, sub-agents per task, live processes, git ops), `[timeouts]` (question, permission wait, stall, idle without report) and `auto_resume` to `config.toml`. The engine already has the fields; this only wires them.
- **`/status` RSS.** Read `/proc/<pid>/status` `VmRSS` per agent on Linux; leave `None` on macOS.
- Tests:
  - engine: `Recover` resumes only working agents and resets only unconfirmed deliveries;
  - engine: stall with a fake clock; output-free redraws do not reset it; a hook does;
  - server: a save, drop, start cycle with the fake claude restores the plan and resumes the working agent (extend `tests/api.rs:543`);
  - config: every new key round-trips, and an unknown key is an error.

## Step 13: CLI verbs (S)

`crates/cmux/src/main.rs`, a new `crates/cmux/src/conn/verbs.rs`.

- One function per verb from UI design section 5: `plans`, `board`, `new-plan`, `accept`, `pause`, `inbox`, `allow`, `deny`, `send`, `reply`, `stop`, `resume`, `tail`, `attach`.
- Each verb is a thin call through `conn/http.rs`. No verb needs a new server route.
- **Agent paths.** Resolve `plan/T-0002/tests` to an agent id on the client from `GET /state`. Accept a plan title slug or a plan id for the first segment.
- **Output.** Listing verbs print a fixed-width table, or JSON with `--json`. The JSON is the `cmux-proto` view type, so it stays stable.
- **Exit codes.** Map errors to an enum: usage → 1, server or connection → 2. Print one line to stderr.
- **`tail <agent>`** streams the terminal bytes to stdout through the existing stream code. `--chat` waits for step 10.
- **`attach [<agent>]`** opens the TUI with that agent focused.
- Tests: each verb against the in-process server (`conn/it.rs` style); JSON output as `insta` snapshots; exit codes for a bad path, a missing server and a refused action.

## Step 14: GitHub tracking (M)

A new `cmuxd/src/git/github.rs`.

- **Poller task.** It reads the open PRs from state each tick and runs `gh pr view <url> --json state,mergedAt,statusCheckRollup,reviewDecision` on the blocking pool. Cadence: 60 s for open PRs, 15 s for tasks in `testing` whose agent is idle. Stagger the calls; run at most 2 at a time.
- **Command, not effect.** The poller sends `RunnerEvent::Pr` only when the mapped result changed. The engine routing already exists.
- **Red-CI cap.** Add `ci_failures: u8` to `PrRef`. Count each transition into a failing rollup. On the third, send the item to the user (`ItemKind::CiFailing`, UI design 2.3) instead of the task agent. Reset the count when CI goes green.
- **`gh` missing or logged out.** Check once at start with `gh auth status`. If it fails, turn the poller off and show `github: off (<reason>)` in `/status`.
- **CI mark.** Add a CI field to `PrView` (`pending`, `passing`, `failing`). Show `#41 ✓`/`#41 ✗`/`#41 …` in the tree and on board cards.
- Tests:
  - the mapping on recorded `gh` JSON files: merged, closed, failing, pending, no checks, review requested;
  - cadence and staggering with a fake clock and a fake `gh` runner trait;
  - the third red CI goes to the user, a green run resets the count;
  - `gh` missing: no panic, the status says off.

## Step 10: Transcript tailer and chat API (M)

`cmuxd/src/claude/transcript.rs`, `cmux-proto/src/{view,stream}.rs`, `cmux-history`.

- **Find the file.** Read `transcript_path` from the `SessionStart` hook (add it to `wire.rs`). Fall back to `~/.claude/projects/<dir>/<session_id>.jsonl`. A `/clear` gives a new `session_id` and a new file; follow it.
- **`ChatEntry`** in `cmux-proto`: `{uuid, ts, role, kind, text, tool?: {name, summary}}`, as architecture 5.3 says. Cut tool results to 400 chars. Skip thinking blocks, meta lines and `<`-wrapped injections. Unwrap the `<pasted_content>` wrapper from spike S3, so a digest shows as one short line.
- **Parser in `cmux-history`.** Move the line rules there, so the TUI session picker and the server share them. The tailer keeps a byte offset and a partial-line buffer per file.
- **Tailing only with subscribers.** Poll every 500 ms for agents with a chat subscriber. Always read the last part of the file on `Stop` and on exit, for token sums and the implicit report.
- **Token sums.** Feed `Agent.tokens` through a new `TokensUpdated` command, at most once per turn.
- **Implicit report.** Replace the `why` text at `deliver.rs:217` with the last assistant text, cut to 2 000 chars.
- **API.** `GET /v1/agents/:id/transcript?after=<uuid>&limit=`; stream `sub_chat {agent, after?}`, `unsub_chat`, and `chat {agent, entries}`.
- **CLI.** `cmux tail <agent> --chat` prints entries as they arrive.
- **Fake claude.** It writes a JSONL transcript for each scripted turn and sends its path in the hooks.
- Tests:
  - fixtures cut from real transcripts: user, assistant, tool_use, tool_result, meta, compaction, `/clear`, pasted content;
  - incremental reads across partial lines and across a `/clear`;
  - an end-to-end test: the fake claude exits without a report, and the parent gets the last assistant text.

## Step 11a: Web app core and terminal (L)

A new `web/` directory and `cmuxd/src/api/web.rs`.

- **Scaffold.** Vite, Preact, TypeScript, `vitest`. One `npm run build` writes `web/dist/`.
- **Types.** Derive `ts_rs::TS` on the wire types in `cmux-proto`. A test writes `web/src/api/types.ts`. CI fails if it differs from the committed file.
- **Serve.** `rust-embed` under a `web` feature, on by default. Serve `/` and the assets with long cache headers on hashed names. The build must work without `web/dist/` when the feature is off, so `cargo test` does not need Node.
- **App.** The pairing screen (it trades the token for a cookie through `/v1/session`), the store fed by `state` and `deltas`, the tree, and the agent view with xterm.js (fit and WebGL add-ons), input frames, size ownership, and reconnect by `rev` and `seq`.
- **CI.** Add a `web` job: `npm ci`, `npm run build`, `npm test`. The release job builds `web/dist/` first.
- Tests: `vitest` for the store reducer and delta application; one Rust test that the embedded `index.html` is served with the right CSP header.

## Step 11b: Product views and phone layout (L)

- The needs-you list with all actions from UI design 2.3.
- The chat view and the composer (queue and steer) on top of step 10.
- The board, the new-plan form, and `/repos` (`GET` and `POST` with `git clone` on the blocking pool).
- Settings: devices, notifications, repositories, limits, Claude per level, status.
- The phone layout below 640 px, bottom tabs, and the terminal key bar (`Esc`, `Tab`, `Ctrl`, arrows).
- Tests:
  - `vitest` for chat rendering and the needs-you actions;
  - one Playwright test against `cmuxd` with the fake claude: pair, open an agent, type into the terminal, and approve a permission.

## Step 12: PWA and push (M) → **M4**

- **ntfy first.** The `ntfy` sink is a plain HTTP POST from the runner for `Effect::Notify`. It needs no HTTPS and no PWA, so it gives phone alerts on day one of this step.
- **Web Push.** Generate VAPID keys on first start (`vapid.key`, mode 0600). Add `GET /push/vapid`, `POST /push/subscribe` and per-device notification kinds. Drop a subscription on HTTP 404 or 410 from the push service.
- **Service worker.** App shell cache, manifest, install prompt. Notification actions (Allow, Deny, Reply) call `/items/:id/answer` with an `Idempotency-Key`.
- **Payload.** Item id, kind, and a summary of at most 60 chars. No bodies, no tool input beyond that summary.
- **Guide.** HTTPS with `tailscale serve`, in the README.
- Tests: the payload builder; a subscription round trip with a mock push endpoint; the ntfy sink against a mock HTTP server.
- **Manual acceptance from a phone over Tailscale.** A permission request arrives as a push. A tap on Allow lets the agent continue.

## Step 15b: Setup, doctor, release (M)

- **`cmuxd setup`.** It writes `~/.config/systemd/user/cmuxd.service`, runs `systemctl --user enable --now cmuxd` and `loginctl enable-linger`. It checks `claude --version`, the Claude login, and `gh auth status`. It prints what it did and what failed. It is safe to run twice.
- **`cmuxd doctor [<agent>]`.** Without an agent: config, socket, store, `claude`, `gh`, TLS, listen address. With an agent: its last 50 diagnostics lines, its state, and its pending items.
- **Release.** A tag `v*` triggers the release job. It builds `web/dist/`, then the musl binaries for `x86_64` and `aarch64`, and attaches them and a checksum file to a GitHub release.
- Tests: the unit file text as a snapshot; `setup` in a dry-run mode against a temp home; `doctor` output with a fake diagnostics file.

## Step 16: Docs, performance, security (M) → **M5**

- **README.** Rewrite around the client-server model: install, `cmuxd setup`, pairing, Tailscale and Caddy recipes, the web app, the CLI verbs, keys, the full config reference, and the Ghostty bindings. Remove "Early (milestone 2)".
- **Design docs.** Mark the open questions in architecture section 12 that the build answered. Fix the board section in UI design 3.6 or the code, so they agree (the code has columns, the doc has a table).
- **Performance** on a 2-vCPU, 4 GB VPS with 10 fake-claude agents that print at a steady rate and 3 clients:
  - idle `cmuxd` CPU under 1 %;
  - input-to-echo overhead under 5 ms on top of network latency;
  - `cmuxd` RSS (without the claude processes) under 150 MB.
  
  Put the load generator in `crates/cmuxd/examples/load.rs`, so the run can be repeated.
- **Security.** One test per row of architecture section 9 that can be automated. Many exist already (auth on TCP, uid on the socket, pairing limits, read scope, CSRF, plain HTTP refusal, agent routes on the socket only, cross-plan tokens). Add: the diagnostics log never contains a prompt, a tool input or a token; the push payload limit; the WebSocket refuses a token in the URL.
- **Real-claude suite.** Turn the `spikes/` harness into `cargo test --ignored` tests for S1 to S7, so a Claude Code update can be checked in one command.

## 3. Sizes and totals

| Step | Size | Track |
|---|---|---|
| 8.5 | S | main |
| 9 | S | main |
| 15a | M | main |
| 13 | S | main |
| 14 | M | Rust |
| 10 | M | web |
| 11a | L | web |
| 11b | L | web |
| 12 | M | web |
| 15b | M | main |
| 16 | M | main |

One implementer: about 20 to 30 working days. With the Rust and web tracks in parallel after step 13: about 15 to 20 calendar days.

## 4. Risks specific to these steps

| Risk | Effect | Mitigation |
|---|---|---|
| The transcript format changes | The chat view and the implicit report break | Parse leniently, skip unknown lines, keep fixtures from each Claude Code version we test |
| `--resume` after a crash restarts a half-done tool call | Repeated side effects, for example a second `git push` | The note tells the agent to check state first; pushes are idempotent; never auto-resume a sub-agent |
| `gh` rate limits with many PRs | Stale PR state | One call per PR per cadence, staggered; back off on HTTP 403 |
| Web Push differs between iOS and Android | No alerts on one platform | ntfy ships first as the fallback |
| The stall watchdog fires on long builds | Noise in needs-you | Count build output as progress; the default is 15 min; the item has a Dismiss action |
