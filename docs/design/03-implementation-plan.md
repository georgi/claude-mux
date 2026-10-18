# cmux: implementation plan (client-server)

Status: draft 2, 2026-10-18. This draft replaces draft 1. Part 3 of 3. Previous: [01-ui-design.md](01-ui-design.md), [02-architecture.md](02-architecture.md).

All work happens in `georgi/claude-mux`, which becomes a Cargo workspace plus a `web/` app. Each step is one PR unless it says otherwise. Every step leaves these green: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` and, from step 11, `npm run build && npm test` in `web/`.

Sizes: **S** is up to half a day, **M** 1 to 2 days, **L** 3 to 4 days, including tests.

## Milestones

| Milestone | Steps | You can… |
|---|---|---|
| **M0 Proven** | 0 | trust the Claude Code and terminal behaviours the design depends on |
| **M1 Core** | 1–3 | run the whole plan / task / agent logic in tests, with no I/O |
| **M2 Server + TUI, two levels** | 4–8 | run `cmuxd` on a Linux box, attach with `cmux` from a laptop, create a plan, and watch the executor spawn task agents. Detach and re-attach; agents keep going |
| **M3 Three levels** | 9 | task agents spawn sub-agents; questions flow between levels and to you |
| **M4 From the phone** | 10–12 | approve permissions, answer questions and follow agents in the chat view from a phone, with push notifications |
| **M5 Shippable** | 13–16 | CLI verbs, PR tracking, restart recovery, one-command deployment, docs |

M2 is the first useful version. The order front-loads risk: the spikes come first, and the pure core comes before any I/O.

## Step 0: Spikes (M, findings appended to this file, no PR)

| # | Question | Pass | Fallback |
|---|---|---|---|
| S1 | Does `--settings <file>` with only hooks merge with the user's own settings? | Both hook sets fire; the user's permission rules still apply | Write the hooks to the worktree's `.claude/settings.local.json` |
| S2 | Does `Stop` → `{"decision":"block","reason":R}` make Claude act on R, and is `stop_hook_active` set on the next stop? | Yes to both | Delivery uses path B only |
| S3 | Does a bracketed paste of multi-line text plus `\r` submit exactly one prompt? | One `UserPromptSubmit` with the exact text | Write the digest to a file and paste one line: "read <file>" |
| S4 | Does interactive `claude "<prompt>"` run the prompt as the first turn? Does `env` in `--mcp-config` reach the stdio server? | Yes to both | Inject the kickoff after `SessionStart`; pass identity in `args` |
| S5 | Can `Notification` payloads tell a permission prompt from an idle notice? Does `additionalContext` reach the model? | A field that tells them apart; yes | Treat every notification as needs-you; drop path C |
| S6 | Does a `PermissionRequest` hook exist in the installed Claude Code, and does its decision answer the prompt? What is its maximum `timeout`? If it does not exist, which keys answer the permission prompt in the PTY? | A decision from the hook answers the prompt; the timeout holds for 10 minutes | Answer by sending the prompt's keys (for example `1`, `2`, `Esc`) to the PTY |
| S7 | Does vt100's `state_formatted()` redraw a mid-turn Claude screen faithfully in xterm.js and in a second vt100? | A visual diff with no differences on 5 captured screens | Send the snapshot as `contents_formatted()` plus a cursor-position escape |

**Done when** every spike has a result and a chosen path. Steps 6 and 10 depend on these results.

### Step 0 results (spikes, run 2026-10-18 against Claude Code 2.1.302)

| # | Result | Chosen path |
|---|---|---|
| S1 | Pass. User, project and `--settings` hooks all fire; user and project permission rules still apply | `--settings <hooks-only file>` as designed |
| S2 | Pass. `block` makes Claude act on `reason`; next stop has `stop_hook_active: true`, reset on the next user prompt | Path A as designed |
| S3 | Partial. One prompt, exact text for 1 to 3 lines; 4+ lines (or 3000 chars) get wrapped in `<pasted_content id="..">` tags | One-line paste (digest file + "read <file>" for long digests); match confirmation by item ids, not whole text |
| S4 | Pass, with a gotcha. Positional prompt is turn 1; config `env` reaches the stdio server and beats the parent env | As designed, but write `--mcp-config=<file>` (variadic flag eats the prompt otherwise) |
| S5 | Pass. `Notification.notification_type` is `permission_prompt` or `idle_prompt`; `additionalContext` reaches the model | Path C as designed; ignore `idle_prompt` |
| S6 | Pass. `PermissionRequest` exists; allow/deny decisions answer the prompt; `timeout` honoured at 90 s with 600 | Remote approval via hook as designed; PTY keys `1`/`2`/`4`/`Esc` as backup |
| S7 | Pass after one fix. Cells, colours, attrs, cursor, mouse, bracketed paste all identical; alt-screen flag is lost | `state_formatted()` snapshot, prefixed with `ESC[?1049h` when `screen.alternate_screen()` |

Test rig: tmux (`-L spikes`) or Python pty, `CLAUDE_CONFIG_DIR=spikes/cfg` (pre-seeded `.claude.json` with `hasCompletedOnboarding`, so `~/.claude` is untouched), claude-mux's `SESSION_ENV` scrub, `--model haiku`.

#### S1 --settings merge
- `cfg/settings.json` (user: UPS hook + deny `Bash(touch:*)`), `repo/.claude/settings.json` (project: UPS hook + deny `Bash(rm:*)`, allow `Bash(echo:*)`), `--settings hooks/s1-settings.json` (hooks only: UPS + Stop).
- One prompt fired all three `UserPromptSubmit` hooks (USER, PROJ, FLAG) and the flag `Stop`. `rm` and `touch` were denied, `echo` ran. Hook sets are concatenated, not replaced.
- Common payload keys: `session_id`, `transcript_path`, `cwd`, `prompt_id`, `permission_mode` (`default`), `hook_event_name`, plus `scratchpad_dir` and `effort` on some events.
- Surprise: without `--session-id` the hook `session_id` was the OUTER session's id (an env var other than the scrubbed ones leaks it, likely `CLAUDE_CODE_REMOTE_SESSION_ID`). With `--session-id <uuid>` it is correct. cmuxd always passes `--session-id`; also scrub `CLAUDE_CODE_REMOTE*` when cmuxd itself runs inside Claude.
- Surprise: the MCP server child sees all `CLAUDE_*` vars of the parent, plus `CLAUDECODE` and `CLAUDE_PROJECT_DIR` set by claude.
- `claude --help` has `--setting-sources`, `--bare` (skips hooks), `--permission-mode` choices `acceptEdits auto bypassPermissions manual dontAsk plan` (default mode shows as "manual mode"; hook payload still says `default`).

#### S2 Stop block
- Hook printed `{"decision":"block","reason":"..."}` when `stop_hook_active` was false. Claude replied PONG, UI showed "Stop hook error: <reason>", then answered the reason (ZEBRA) in the same turn (same `prompt_id`, no new `UserPromptSubmit`).
- Second `Stop` payload: `stop_hook_active: true`, `last_assistant_message: "ZEBRA"`. On the next user prompt it is `false` again.
- `Stop` payload keys: `stop_hook_active`, `last_assistant_message`, `background_tasks`, `session_crons`, `effort`, `permission_mode`. The "at most 3 in a row" limit must be counted by cmuxd; Claude does not stop a chain itself (only tested 1 block).

#### S3 Bracketed paste
- Written as one `write`: `ESC[200~ text ESC[201~ \r` (via `tmux send-keys -H`). Works with `\r` in the same write; exactly one `UserPromptSubmit`.
- 1 to 3 lines (blank lines, indent, leading `/`, backticks, UTF-8 all fine): `prompt` is byte-exact.
- 4 or more lines, or about 3000 chars on one line: `prompt` (hook and transcript) becomes `"\n\n<pasted_content id=\"e8dc\">\n<text>\n</pasted_content id=\"e8dc\">\n"`. The id was the same for different pastes. The model still reads it fine.
- Raw typed LF-separated text plus `\r` (no bracketed paste) was submitted exact, but it risks menus on `/` and `@`; not chosen.
- A prompt sent while a turn runs is queued ("Press up to edit queued messages") and `UserPromptSubmit` fires when it is dequeued, so path B must still wait for idle.
- tmux `send-keys "text" Enter` in one call did NOT submit (paste-burst detection ate the Enter); a separate write after about 1 s did. Real clients should write `\r` as its own write or inside the paste write as above.
- Step 6/10 impact: digest confirmation must look for the item ids in `prompt`, tolerate the wrapper, and the transcript user message will also carry the wrapper.

#### S4 Kickoff and MCP env
- `claude --model haiku --session-id U --settings S --mcp-config=M "<prompt>"`: prompt ran as turn 1 (`UserPromptSubmit` fired, `prompt` exact).
- `--mcp-config <file> "<prompt>"` FAILS: the flag is variadic (`<configs...>`) and takes the prompt as a second config path ("MCP config file not found"). Use `--mcp-config=<file>`, or put the prompt before the flags.
- mcp.json `{"command","args","env":{"CMUX_AGENT","CMUX_TOKEN"}}`: server saw both, `env` overrode the parent's `CMUX_AGENT`; `args` and cwd (= claude's cwd) also as expected. Server started once, tools usable on the first turn.
- First MCP tool call prompts for permission. Pre-approve with `--settings` `{"permissions":{"allow":["mcp__cmux"]}}`; verified no prompt.

#### S5 Notification and additionalContext
- Permission prompt: `{"hook_event_name":"Notification","message":"Claude needs your permission","notification_type":"permission_prompt"}`, fired about 6 s after the prompt appeared (and after `PermissionRequest`).
- Idle: `{"message":"Claude is waiting for your input","notification_type":"idle_prompt"}`, 60 s after `Stop`.
- `UserPromptSubmit` answering `{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"..."}}`: model used it (answered MANGO42).

#### S6 PermissionRequest
- Exists. Order: `PreToolUse`, `PermissionRequest`, (6 s) `Notification`, then `PostToolUse` and `Stop`. Payload: `tool_name`, `tool_input` (Bash: `command`, `description`), `permission_suggestions` (list of `{type, ...}`), `permission_mode`; no `tool_use_id` (it is in `PreToolUse`).
- Allow `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}`: command ran, UI "Allowed by PermissionRequest hook". Deny with `"message"`: not run, UI "Denied by PermissionRequest hook", the model saw and quoted the message. `{}` or no output: normal terminal prompt.
- Timeout: hook sleeping 90 s with `"timeout": 600` was honoured (allow applied at 90 s; the default 60 s does not apply). Control: `"timeout": 4` with a 10 s sleep ignored the late allow and left the prompt. Not waited for the full 10 min; unit is seconds.
- The terminal prompt is drawn while the hook is pending. A key typed in the terminal wins; the hook process is not killed, and its late decision is ignored. cmuxd must resolve a held request on the next `PostToolUse`/`Stop`/`PreToolUse` for that agent.
- PTY keys (no Enter needed): `1` yes, `2` always allow (for Bash, "always allow access to <dir>"), `3` switch to auto mode, `4` no, `Esc` cancel. Option numbering varies per tool, so prefer the hook.

#### S7 Snapshot fidelity
- Raw capture of a 100x30 PTY (`s7-vt100/capture.py`): startup, typed prompt, 3 spinner screens ("Hatching... (9s)", tool "Sleeping for 12 seconds"), tool output, done. 8 cut points.
- Claude runs on the ALT SCREEN (`ESC[?1049h`) with mouse modes 1000/1002/1003/1006, focus 1004, bracketed paste 2004, cursor hide.
- vt100 0.16.2 (`s7-vt100/src/main.rs`): parser B fed `A.state_formatted()` had 0 cell diffs (char, fg, bg, bold, italic, underline, inverse, dim, wide), same cursor, hide, bracketed paste, mouse mode/encoding, `contents_formatted()` byte-equal. Only `alternate_screen()` differed (false in B). Prefixing the snapshot with `ESC[?1049h` when A is on the alt screen fixes it.
- xterm.js (`@xterm/headless` 5.5.0, npm registry reachable, `s7-vt100/xterm/compare.mjs`): xterm fed the raw bytes vs xterm fed the (prefixed) vt100 snapshot: 0 cell diffs (char, fg/bg mode and value, bold, italic, underline, inverse, dim, width), same cursor, both on the alternate buffer.
- Not tested: scrollback (alt screen has none), resize while attached, wide-char heavy output.

#### Harness files (for a later `cargo test --ignored` suite)
- `launch.sh` (tmux launch with env scrub and isolated config), `ask.sh`, `cap.sh`, `sendraw.py` (raw bytes via `tmux send-keys -H`)
- `hooks/log-hook.sh` (logs stdin JSON, optional `resp-<Tag>.json` reply and `sleep-<Tag>`), `hooks/s2-stop.py`, `hooks/*-settings.json`
- `mcp/envsrv.py` (stdio MCP env probe), `mcp/mcp.json`
- `s7-vt100/capture.py`, `s7-vt100/src/main.rs`, `s7-vt100/xterm/compare.mjs`, `cfg/.claude.json` (onboarding bypass)

The harness is in [`spikes/`](../../spikes/README.md).

## Step 1: Workspace and CI (M)

- Move today's crate into `crates/cmux/`, unchanged, so the plain local cmux still works. Add empty `cmux-core`, `cmux-proto` and `cmuxd` crates.
- Add GitHub Actions: fmt, clippy and tests on Linux and macOS; a release build of `cmuxd` for `x86_64` and `aarch64` Linux (musl).
- Tests: the existing `keys.rs` tests; new layout tests for `Node::layout` and the presets.

## Step 2: Core model and transition tables (M)

`cmux-core/{model,ids}.rs`.

- The types from architecture section 4, the plan and task tables copied from the orchestrator's `lib/types.ts`, and the glyph function.
- Tests: every allowed and every forbidden edge; id formats; agent path parsing; the glyph for each row of UI design table 2.1.

## Step 3: Engine, delivery, permissions, prompts (L)

`cmux-core/{engine,delivery,permissions,prompts}.rs` and `prompts/*.md`.

- `apply(State, Command, now) -> Result<(Reply, Vec<Effect>, Vec<Delta>)>`. Its commands cover every MCP tool, every hook, every client action, `WorktreeReady`, `Spawned`, `Exited`, `Pr`, and the timer ticks.
- The rules covered:
  - dependency and limit checks;
  - one report per attempt;
  - turn landing (architecture section 8.3);
  - implicit reports;
  - merged-requires-criteria;
  - role authorization;
  - delivery path choice and digests;
  - allow rules;
  - timeouts for questions and permissions;
  - `rev` and delta emission.
- Tests: table-driven scenarios that replay commands and assert on state, effects and deltas. At least:
  - planning → accept → two spawns → the limit refuses a third → a report frees a slot and wakes the executor;
  - a report without a PR;
  - exit with no report;
  - question and answer;
  - question timeout;
  - a permission answered by a client, by an allow rule, and by a timeout;
  - a steer;
  - a paused plan;
  - a cross-plan token is rejected;
  - deltas are gap-free.

**M1 reached.**

## Step 4: Server skeleton (L)

`cmuxd/{main,config,store,actor,devices,diag}.rs` and `cmuxd/api/{rest,auth}.rs`.

- Config, the data directory lock, store load and atomic save, and the journal.
- The engine actor, with an `mpsc` in and broadcast deltas out.
- Axum on the Unix socket (uid check) and on TCP (`127.0.0.1:7878` by default). The server refuses non-loopback plain HTTP unless allowed. Optional rustls.
- Auth: `cmuxd pair` (code and QR), `/pair/complete`, hashed device tokens, scopes, `/devices`, web cookie sessions, the `X-Cmux` CSRF header, and rate limits on pairing.
- REST: `/state`, plan and task endpoints, `/items/:id/answer`, `/status`, and `Idempotency-Key` handling.
- Diagnostics JSONL with rotation.
- Tests:
  - API tests against an in-process server: auth required on TCP, uid-only on the socket;
  - pairing: expiry, reuse, rate limit;
  - idempotent retries;
  - a store round trip;
  - a second `cmuxd` refuses to start.

## Step 5: PTY host, terminal hub and stream (L)

`cmuxd/term/{pty,hub}.rs` and `cmuxd/api/stream.rs`. `pty.rs` is claude-mux's `pane.rs` without drawing.

- Loose terminals through `POST /terminals`: Claude, shell, or a command.
- The ring buffer with `seq`, vt100 snapshots, 16 ms batching, the size owner, and the scrollback endpoint.
- The WebSocket: `hello` / `welcome`, `state` or `deltas` from `since_rev`, terminal subscribe and unsubscribe, input frames, `focus` and `resize`, presence, and compression.
- Tests:
  - a WebSocket client in tests drives a `cat`/`sh` PTY: input echoes, output arrives in order;
  - reconnect within the ring gets only the missing bytes; outside the ring it gets a snapshot;
  - the size owner changes on input from another client;
  - 50 MB of output does not grow memory beyond the ring and the parser.

## Step 6: Agents on the server (L)

`cmuxd/claude/spawn.rs`, `cmuxd/git/worktree.rs` and `cmuxd/agent/{hook,mcp,endpoint}.rs`.

- Per-agent `settings.json`, `mcp.json` and `system.md`; the argv; the env; the token hash.
- Worktrees on a blocking pool of 2, with reuse rules and `info/exclude`.
- The `/v1/agent/hook` and `/v1/agent/tool` endpoints, on the Unix socket only.
- `cmuxd hook`: always exits 0, returns within 2 s (or within `permission_wait` for `PermissionRequest`), and is silent with no socket.
- `cmuxd mcp`: `initialize`, `tools/list` by role, `tools/call`, `ping`.
- The remote permission flow (path chosen in S6).
- Tests:
  - argv and file snapshots per level;
  - worktree cases against a temp repo with a bare origin;
  - scripted MCP sessions per role;
  - an end-to-end test with a **fake `claude`**. It is a small Rust test binary that runs the hooks through `cmuxd hook`, calls MCP tools from a script, and echoes its input. Spawn → `SessionStart` → `report_result` → `Stop` → the parent gets the report through path B;
  - the permission hook held open, then answered over REST.

## Step 7: TUI client over the protocol (L)

`cmux/conn/*`, `cmux/tui/remote_term.rs`, and `login`/`servers`.

- The WebSocket on its own thread, feeding the existing event loop. Reconnect with backoff, and the connection dot.
- `RemoteTerm` replaces the PTY inside `Pane`.
- Windows derived from server state; per-server view state in `view-<server>.toml`.
- `cmux login <url>` (pairing), `client.toml`, and automatic use of the local socket.
- **Parity check**: every claude-mux key in the README works against the server. `prefix d` detaches. The session picker lists the server's `~/.claude/projects`.
- Tests: `RemoteTerm` gives the same screen as a local vt100 on recorded streams; window derivation is a pure function of state; reconnect resumes from `rev` and `seq` (with an in-process server).

## Step 8: TUI plan features (L) → **M2**

`cmux/tui/draw/{tree,board,inbox}.rs`, plus input and mouse changes.

- The tree and narrow strip, the focus model, every key in UI design section 3.5, and the mouse actions.
- Board view; plan, task and message forms; queue and steer; the needs-you overlay with `y`/`Y`/`n`/`r`.
- Size ownership marks (`⇲`) and `prefix =`.
- Tests: draw snapshots at 80×24, 100×30 and 160×50 (with `insta`); tree navigation; mouse hit-tests.
- Manual acceptance on a real VPS: create a plan with two dependent tasks, accept it, detach, re-attach, and see T2 started after T1 reported.

## Step 9: Sub-agents and questions (M) → **M3**

- `spawn_subagent` creates panes in the task window (client layout `main-vertical`). Finished sub-agents are shown dimmed.
- The `PreToolUse` guard rails.
- `ask_parent`, `answer_child`, `ask_user`; the 60-minute question timeout; `prefix u`.
- Tests: engine scenarios for per-task limits and report routing; the guard rail denies `git push` for a sub-agent; layout with 1 to 3 sub-agents.

## Step 10: Transcript tailer and chat API (M)

`cmuxd/claude/transcript.rs`, which grows out of claude-mux's `history.rs`.

- Incremental tailing, `ChatEntry` parsing, token sums, `GET /agents/:id/transcript`, and the `sub_chat` stream.
- Implicit reports now use the last assistant text.
- Tests: fixtures cut from real transcripts (user, assistant, tool_use, tool_result, meta, compaction, `/clear`); incremental reads across partial lines.

## Step 11: Web app (two PRs, L + L)

**11a, core and terminal**:
- The Vite, Preact and TypeScript scaffold; `ts-rs` type generation, with a CI check.
- The pairing screen; the store fed by `state` and `deltas`; the tree; the agent view with xterm.js (fit and WebGL add-ons), input, size ownership, and reconnect.
- Embedded in `cmuxd` with `rust-embed`.

**11b, product views and phone layout**:
- The needs-you list with actions; the chat view and composer (queue and steer); the board; the new-plan form; settings (devices, notifications, repositories, limits, Claude per level, status).
- The phone layout with bottom tabs, and the terminal key bar.

Tests:
- `vitest` for the store reducers, delta application and chat rendering.
- One Playwright test against a `cmuxd` running the fake `claude`: pair, open an agent, type into the terminal, and approve a permission.

## Step 12: PWA and push (M) → **M4**

- The service worker (app shell cache), the manifest, and install prompts.
- VAPID key generation, `/push/subscribe`, and per-device notification kinds. Notification actions (Allow, Deny, Reply) answer items through the API.
- The optional `ntfy` sink.
- A deployment guide for HTTPS with `tailscale serve`, which PWAs and push require.
- Tests: the push payload builder (no bodies, a 60-character summary); a subscription round trip with a mock push endpoint.
- Manual acceptance from a phone over Tailscale: a permission request arrives as a push, and tapping Allow lets the agent continue.

## Step 13: CLI verbs (S)

- Every verb in UI design section 5, with `--json` on listing verbs and exit codes 0, 1 and 2.
- `cmux tail --chat` streams chat entries.
- Tests: each verb against an in-process server; JSON snapshots.

## Step 14: GitHub tracking (M)

`cmuxd/git/github.rs`.

- The `gh` poller with the two cadences; `set_task_pr`; the PR → task-state mapping; `PrChanged` routing; the cap of 3 red-CI attempts.
- The PR and CI marks in all clients.
- Tests: the mapping function on recorded `gh` JSON (merged, closed, failing, pending, no checks); cadence with a fake clock; `gh` missing.

## Step 15: Recovery, watchdog, deployment (M)

- Resume on start for agents that were working; manual `R` / resume for idle ones; unconfirmed deliveries reset to pending.
- Watchdog `⚠` items; `cmuxd doctor`.
- `cmuxd setup`: a systemd user unit, linger, and checks of `claude` and `gh` login. Release binaries with the web app embedded.
- Tests: a save–kill–start cycle restores state and resumes only working agents; the watchdog with a fake clock ignores redraws without output.

## Step 16: Docs, performance, security review (M) → **M5**

- README: the client-server model, `cmuxd setup`, pairing, Tailscale and Caddy recipes, keys, config reference, and the Ghostty bindings.
- Performance on a 2-vCPU, 4 GB VPS with 10 agents and 3 clients:
  - idle `cmuxd` CPU under 1 %;
  - input-to-echo overhead under 5 ms on top of network latency;
  - `cmuxd` RSS (without the claude processes) under 150 MB.
- A security pass over the checklist in architecture section 9, with one test per row where it can be automated.

## Dependency graph

```
0 ─────────────────────────────┐ (results feed 6 and 10)
1 ── 2 ── 3 ── 4 ── 5 ── 6 ── 7 ── 8 (M2) ── 9 (M3) ──┬── 13
                         │                            ├── 14 ──┐
                         └── 10 ── 11a ── 11b ── 12 (M4) ─────┼── 15 ── 16 (M5)
```

- Steps 0 and 1 can run in parallel.
- After step 6, the TUI track (7 to 9) and the web track (10 to 12) are independent. Two implementers, or two task agents once M2 works, can take one each.
- Steps 13 and 14 are independent of each other.

## Totals

| Steps | Sizes | Estimate |
|---|---|---|
| 0–3 (to M1) | M, M, M, L | 6–10 days |
| 4–8 (to M2) | L, L, L, L, L | 15–20 days |
| 9 (to M3) | M | 1–2 days |
| 10–12 (to M4) | M, L, L, M | 8–12 days |
| 13–16 (to M5) | S, M, M, M | 3.5–6.5 days |
| **All** | | **about 34–51 working days for one implementer**; about 25–35 calendar days with the two tracks in parallel |

## Risks

| Risk | Effect | Mitigation |
|---|---|---|
| Claude Code changes hooks or input handling | Status, delivery or remote approval breaks | The spikes stay as a `cargo test --ignored` suite against the real `claude`; every path has a fallback; polling `~/.claude/sessions` stays as a status fallback |
| `PermissionRequest` is missing or its timeout is short (S6) | Remote approval degrades | Fall back to `Notification` plus keys sent to the PTY. Answers still work from the phone; the terminal prompt is the source of truth |
| Exposing a shell-equivalent server to the internet | Full compromise of the server user | Loopback by default; TLS required off-loopback; Tailscale recommended; hashed device tokens; read-only scope; a dedicated Unix user |
| Mobile networks drop often | Lost or double actions | Reconnect by `rev`/`seq`; `Idempotency-Key` on every mutation |
| Typing latency on a far-away VPS | A sluggish feel in the TUI | Pick a nearby VPS; mosh-style prediction in `RemoteTerm` later; the chat and queue paths do not need low latency |
| Memory with many agents | Out-of-memory on a small VPS | Global process limit; scrollback per level; RSS on the status page |
| A server restart loses in-flight turns | Rework after an upgrade | Resume with `--resume` and a note; a separate PTY-holder process is an open question for later |
