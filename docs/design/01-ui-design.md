# cmux: UI design (TUI, web and mobile clients)

Status: draft 2, 2026-10-18. This draft replaces draft 1 (`01-tui-design.md`) and is now client-server from the start. Part 1 of 3. Next: [02-architecture.md](02-architecture.md), [03-implementation-plan.md](03-implementation-plan.md).

> Note on wording: the first request said "on the CI I imagine multiple plans I can choose from". This design reads "CI" as the UI. Nothing here concerns continuous integration, apart from showing a task's PR checks.

## 1. Idea

A server, `cmuxd`, runs on a VPS or a Linux PC. It owns every Claude Code session, every plan and every task, so agents keep working when no client is connected. You reach it from four clients:

| Client | Built with | Best for |
|---|---|---|
| **TUI** (`cmux`) | Rust, ratatui; it grows out of claude-mux | Driving agents from a laptop: full terminals, tmux keys, mouse |
| **CLI** (`cmux <verb>`) | the same Rust binary | Scripts, quick checks, sending a message from any shell |
| **Web app** | TypeScript SPA with xterm.js, served by `cmuxd` | Any browser, with no install; full terminals |
| **Mobile** | the web app as an installable PWA, with push notifications | Answering questions, approving permissions, watching progress |

Every agent is a normal interactive Claude Code session on a PTY on the server. There are three levels:

| Level | Agent | Shown in the TUI as |
|---|---|---|
| 1 | Plan executor: one fixed Claude session per plan | the pinned first window of the plan's workspace |
| 2 | Task executor: one Claude session per task | one window (tab) per task |
| 3 | Sub-agent: a Claude session started by a task executor | a pane inside its task's window |

In the TUI the rule is **plan = workspace, task = window, sub-agent = pane**. The server knows only plans, tasks, agents and terminals; layout is each client's own business. The web and mobile clients show the same tree as a list.

Sessions outside any plan (a plain Claude session, a shell, lazygit) are still possible. They live on the server too, in a workspace called "loose", so claude-mux's current use carries over.

## 2. Shared concepts across all clients

### 2.1 Agent status glyphs

| Glyph | Meaning |
|---|---|
| `⠹` (spinner) | working |
| `○` | idle, nothing pending |
| `◌` | parked: idle, waiting on its children |
| `⚑` | needs you: a permission request or a question |
| `●` | finished, and you have not looked yet |
| `✓` | reported success |
| `✕` | failed, crashed or stopped |
| `⚠` | stalled: working with no output for N minutes |
| `◆` | plan executor (prefix) |
| `✉N` | N messages queued for this agent, not yet delivered |

The colours are the orchestrator TUI's six state hues: running amber `#c68108`, asks you purple `#a662ea`, blocked or failed red `#e25050`, done green `#29a46a`, queued grey `#8f8f97`, you blue `#6092dd`. Claude orange `#d97757` is the accent.

### 2.2 Task and plan states

The states are the orchestrator's, unchanged.
- Task: `todo, in_progress, testing, failing, passing, merged, blocked, cancelled`.
- Plan: `draft, proposed, accepted, done, cancelled`.

### 2.3 "Needs you" items

Every client shows the same list. Each item has one-tap actions:

| Item | Actions |
|---|---|
| Permission request (`Bash: cargo test -p api`) | **Allow**, **Allow always for this task**, **Deny with a note**, Open terminal |
| Question from an agent (`ask_user`) | **Reply** (text), Open |
| Plan ready for review | **Accept**, **Send back with a note**, Open |
| Agent stalled or exited without reporting | **Message**, **Resume**, **Stop**, Open |
| PR CI failing after 3 fix attempts | **Open PR**, **Message the task agent** |

Permission requests can be answered remotely because `cmuxd` intercepts them through a Claude Code hook (architecture section 7). If nobody answers within the configured time (default 10 minutes), the request falls back to Claude's normal prompt in the terminal.

### 2.4 Two ways to look at an agent

- **Terminal**: the real Claude Code UI, streamed byte for byte. It is faithful and complete, and it is what the TUI and desktop web show.
- **Chat**: a lightweight conversation view built from the session's transcript file on the server. It shows your prompts, Claude's text, one-line tool calls with results folded, and a composer. Messages go through the same queue as everywhere else. This is the default on phones, where a 200-column terminal does not fit.

## 3. TUI client (`cmux`)

### 3.1 Connecting

```
cmux                       # connect to the default server (local socket if cmuxd runs here)
cmux -s vps                # connect to a named server from ~/.config/cmux/client.toml
cmux login https://box.example.ts.net   # pair this machine with a server (one-time code)
```

`prefix d` **detaches**, as in tmux. Agents keep running on the server. `cmux` re-attaches to the same view.

### 3.2 Layout (≥ 100 columns)

```
 vps ● auth-refactor ▸ ◆ executor ⠹  1 ⠹ T-0002 api  2 ⚑ T-0004 perms  3 ✓ T-0001 schema  4 $ zsh
┌ plans ─────────────────────┐┌─────────────────────────────────────────────────────────────┐
│▾◆ auth-refactor   accepted ││ ✻ Spawning T-0003 now that T-0002 has a PR…                  │
│   ◆ executor           ⠹   ││                                                             │
│   ✓ T-0001 schema   merged ││ ⏺ cmux - spawn_task (MCP)(task: "T-0003")                    │
│ ▾ ⠹ T-0002 api     #41 ◐ci ││   ⎿  started T-0003 in ~/dev/api/.worktrees/t-0003          │
│     ├ ⠹ tests           2m ││                                                             │
│     └ ✓ docs               ││ ⏺ Waiting for T-0002 CI and T-0003. I'll end my turn; cmux   │
│   ○ T-0003 ui   ← T-0002   ││   wakes me when either reports.                             │
│   ⚑ T-0004 perms   asks    ││                                                             │
│   · T-0005 rollout   todo  ││ >                                                           │
│ ▸ ○ docs-site        draft ││                                                             │
│ ▸ ✓ ci-migration      done ││                                                             │
│ ▸   loose               2  ││                                                             │
├ needs you  2 ──────────────┤│                                                             │
│ ⚑ T-0004 allow Bash?       ││                                                             │
│ ⚑ docs-site plan to review ││                                                             │
└────────────────────────────┘└─────────────────────────────────────────────────────────────┘
 ~/dev/api  main  executor · auth-refactor   3 working · 2 need you · 2 clients    ^b Tab tree  ^b ? help
```

The screen has four regions:

1. **Tab bar (top row).**
   - It starts with the server name and a connection dot: `●` connected, `◐` reconnecting, `○` offline.
   - It shows only the current workspace's windows. The executor is always first, marked `◆`. Task windows are numbered from 1. Shells and lazygit windows come after the tasks.
2. **Tree (left).** All plans, each with its executor, tasks and sub-agents. The "needs you" list sits at the bottom.
3. **Body (right).** The selected window and its panes, drawn as cmux does today, from a local vt100 copy that the server stream keeps up to date.
4. **Status line (bottom row).**
   - It shows the focused pane's directory, branch and agent, plus counts of agents working, items that need you, and clients attached.
   - `2 clients` warns that someone (probably you, on your phone) is attached elsewhere.

Below 100 columns, the tree collapses to a 3-column glyph strip. `prefix Tab` opens the full tree as an overlay, and `prefix B` hides it. A click anywhere in the tree gives it the focus. Drag the right border of the full tree to resize it, or use `prefix <` and `prefix >` (`<` and `>` alone when the tree has the focus). cmux saves the width in `view-<server>.toml`.

### 3.3 Task window with sub-agents

The window uses the `main-vertical` layout. The task executor is the main pane on the left, and sub-agents are stacked on the right, each with a header row. A finished sub-agent stays dimmed until you close it or its task is done.

```
┌──────────────────────────────────┬─ ⠹ tests · ~/dev/api/.worktrees/t-0002 ─┐
│ T-0002 api · in_progress · #41   │ ⏺ Running cargo test -p api …            │
│                                  ├─ ✓ docs · reported success ──────────────┤
│ >                                │ ⏺ Done. Updated docs/api.md.             │
└──────────────────────────────────┴──────────────────────────────────────────┘
```

### 3.4 Terminal size with several clients

A PTY has one size. The rule is tmux's `window-size latest`: **the client that last typed into a terminal, or focused it, sets its size**.

When a client is not the size owner:
- The TUI draws the terminal at the server's size, cropped or padded, and marks the pane header `⇲ 120×40`.
- `prefix =` takes over the size.

### 3.5 Focus model and keys

There are two focus zones, as in draft 1.
- **Pane focus** (the default): keys go to Claude byte for byte, and only the prefix is intercepted.
- **Tree focus**: keys drive the tree.

`prefix Tab` or a mouse click switches between them. In pane focus, no key is taken without the prefix, because Claude Code uses `Esc`, `Tab`, `Ctrl-o`, `Ctrl-r`, `Ctrl-t` and `Ctrl-g` itself.

**New prefix keys.** Every claude-mux binding stays, and `d` now detaches.

| Key | Action |
|---|---|
| `Tab` | Focus the tree (toggle) |
| `e` | Go to the current plan's executor |
| `u` | Go up one level: sub-agent → task, task → executor |
| `a` | Needs-you overlay |
| `M-a` / `M-A` | Jump to the next / previous item that needs you |
| `m` | Message an agent (opens the message prompt) |
| `v` | Board view for the current plan (toggle) |
| `N` / `T` | New plan / new task |
| `P` | Plan switcher (fuzzy) |
| `B` | Hide or show the tree |
| `=` | Take over this terminal's size |
| `S` | Server switcher, when more than one server is configured |
| `d` | Detach (agents keep running) |

**Tree focus keys.**

| Key | Action |
|---|---|
| `↑` `↓` / `k` `j` | Move |
| `←` `→` / `h` `l` | Collapse or parent / expand or child |
| `↵` | Open the node's window and focus its pane |
| `Space` | Preview the node's window, keep tree focus |
| `/` | Filter |
| `n` / `t` | New plan / new task |
| `A` | Accept the plan and start execution, or accept a task |
| `S` | Pause the plan |
| `s` | Message the selected agent |
| `x` | Stop the agent (asks first) |
| `R` | Resume the agent's Claude session |
| `c` | Cancel the task (asks first) |
| `e` | Edit `plan.md` or the task body: `$EDITOR` runs on the server in a pane |
| `o` | Open the PR, in the local browser |
| `g` / `D` | lazygit / diff pager in the task's worktree, as a server pane |
| `v` / `i` / `?` | Board / needs you / help |
| `Esc` | Back to the pane |

**Needs-you overlay keys**: `↑↓` move, `↵` jump to the agent, `y` allow or accept, `Y` allow always, `n` deny or send back (asks for a note), `r` reply, `d` dismiss, `Esc` close.

**Message prompt**: `↵` queues the message for delivery at the end of the agent's turn. `M-↵` steers: it interrupts the agent, then delivers.

**Mouse**: as in claude-mux, plus:
- Click a tree row to show its window; double-click to also focus the pane.
- Click a needs-you row to jump to the agent.
- Drag the tree border to resize the tree.

### 3.6 Board view

`prefix v` replaces the body with the plan's tasks as cards in five columns by state. `v`, `q` or `Esc` goes back.

```
 auth-refactor · accepted · 1/6 merged · 6 agents · 480k tokens                              v back
 todo 2              active 2            passing 1           merged 1            blocked 0
┌──────────────────┐┌──────────────────┐┌──────────────────┐┌──────────────────┐
│○ T-0003 ui       ││⠹ T-0002 api      ││○ T-0006 metrics …││✓ T-0001 schema   │
│todo              ││testing #41 … 2/5 ││passing #43 ✓ 3/3 ││merged #38 ✓ 4/4  │
└──────────────────┘└──────────────────┘└──────────────────┘└──────────────────┘
┌──────────────────┐┌──────────────────┐
│○ T-0005 rollout  ││⚑ T-0004 perms    │
│todo              ││in_progress 1/2   │
└──────────────────┘└──────────────────┘
```

- The header shows the plan title, its state, merged / total tasks, the number of agents and the token sum.
- The columns are `todo`; `active` (`in_progress`, `testing`, `failing`); `passing`; `merged`; `blocked` (`blocked`, `cancelled`). Each column title shows its card count.
- A card shows the agent glyph (`✓` for merged, `⚑` for blocked, failing or cancelled), the task id and title, then the state, the PR mark with its CI glyph, and the criteria count.
- Arrow keys or `hjkl` move the selection. Left and right skip empty columns. The selected column scrolls to keep the card visible.
- `Enter` opens the task's window. The other tree keys (`s`, `x`, `R`, `c`, `o`, `g`, `D`, `A`) act on the selected card.
- `cmux board <plan>` prints the same tasks as a table (task, state, criteria, agent, PR with CI, title), for scripts and for a terminal without the TUI. The web app shows the board as a table too (section 4.1).

## 4. Web app

`cmuxd` serves the web app at its own address, so no separate hosting is needed. On a wide screen it mirrors the TUI. On a phone it becomes a three-tab app.

### 4.1 Desktop (≥ 1024 px)

```
┌───────────────┬───────────────────────────────────────────────┬────────────────┐
│ ● vps         │ auth-refactor ▸ T-0002 api        [Terminal|Chat]│ Needs you (2) │
│               │ ┌ executor ┬ T-0002 ┬ T-0004 ┬ + ┐             │                │
│ ◆ auth-refac… │ │                                             │ ⚑ T-0004       │
│   ◆ executor⠹ │ │   xterm.js: the real Claude Code session    │ Bash: cargo    │
│   ✓ T-0001    │ │                                             │ test -p perms  │
│   ⠹ T-0002 #41│ │                                             │ [Allow][Always]│
│     ⠹ tests   │ │                                             │ [Deny…]        │
│     ✓ docs    │ │                                             │                │
│   ⚑ T-0004    │ │                                             │ ⚑ docs-site    │
│ ○ docs-site   │ │                                             │ plan to review │
│ ✓ ci-migrat…  │ │                                             │ [Accept][Back…]│
│ + New plan    │ └─────────────────────────────────────────────┘                │
│               │ [message T-0002…                          ] [Queue] [Steer] │                │
└───────────────┴───────────────────────────────────────────────┴────────────────┘
```

- **Left**: the same tree as the TUI. Click a row to open its agent.
- **Centre**: the agent, with a `Terminal | Chat` toggle.
  - The terminal is xterm.js and fully interactive, keyboard included.
  - Sub-agents of a task appear as tabs or splits across the top.
  - A message bar under the terminal queues or steers messages without typing into the PTY.
- **Right**: the needs-you list with inline action buttons.
- **Board**: a link at the top of each plan opens the board table.

Keyboard shortcuts while focus is outside the terminal: `g p` plans, `g n` needs you, `j/k` move in the tree, `↵` open, `/` filter, `m` message. Inside the terminal, every key goes to Claude.

### 4.2 Phone (PWA)

There are three tabs at the bottom: **Needs you** (with a badge count), **Plans**, and **Live** (agents working now).

```
┌──────────────────────────────┐   ┌──────────────────────────────┐   ┌──────────────────────────────┐
│ Needs you                 ●  │   │ ‹ auth-refactor    accepted  │   │ ‹ T-0002 api      ⠹  #41 ◐   │
│                              │   │ ◆ executor            ⠹     │   │  [Chat] Terminal              │
│ ⚑ T-0004 perms · 2m          │   │ ──────────────────────────── │   │                              │
│ Allow Bash?                  │   │ ✓ T-0001 schema     merged  │   │ › Add pagination to /users   │
│ cargo test -p perms          │   │ ⠹ T-0002 api    #41 testing │   │                              │
│ [ Allow ] [ Always ] [ Deny ]│   │    ⠹ tests  ✓ docs          │   │ Added cursor pagination and  │
│                              │   │ ○ T-0003 ui   waits T-0002  │   │ tests. Opening the PR now.   │
│ ⚑ docs-site · plan review    │   │ ⚑ T-0004 perms      asks    │   │ ▸ Bash git push … ✓          │
│ 6 tasks, 2 repos             │   │ · T-0005 rollout     todo   │   │ ▸ Bash gh pr create … ✓      │
│ [ Accept ] [ Send back… ]    │   │                              │   │ ▸ set_task_pr #41 ✓          │
│                              │   │ [ Pause ] [ Board ]          │   │                              │
│ ⚠ T-0007 stalled 18m         │   │                              │   │ ✉ 1 queued                   │
│ [ Message ] [ Stop ]         │   │                              │   │ ┌──────────────────────────┐ │
│                              │   │                              │   │ │ Message…        [Queue ▾]│ │
├──────────────────────────────┤   ├──────────────────────────────┤   │ └──────────────────────────┘ │
│ Needs you(3)  Plans   Live   │   │ Needs you(3)  Plans   Live   │   ├──────────────────────────────┤
└──────────────────────────────┘   └──────────────────────────────┘   └──────────────────────────────┘
```

- **Chat** is the default agent view on a phone. **Terminal** is one tap away. In terminal view a key bar sits above the keyboard with `Esc`, `Tab`, `↑`, `↓`, `^C`, `⏎`, plus `1` `2` `3` for Claude's numbered prompts. On a phone the terminal shrinks its font to fit the server's size rather than resizing the PTY, unless you tap "Fit to phone".
- **Push notifications** (Web Push) fire for new needs-you items and for finished plans. Each notification has its action buttons (Allow, Deny, Reply) where the platform supports them. You choose which kinds notify you in Settings.
- **New plan** on a phone: a title, a repository picked from those the server knows, and a first message for the executor.

### 4.3 Settings page (web and phone)

The page has these sections:
- **Devices**: paired devices, with last seen times and revoke buttons.
- **Notifications**: which item kinds push to which device.
- **Repositories**: those the server knows (add by path on the server, or by `git clone` URL).
- **Limits**: maximum live agents, and sub-agents per task.
- **Claude**: per-level model and permission mode.
- **Server status**: version, uptime, CPU, memory per agent.

## 5. CLI

The CLI is the same `cmux` binary, with the same server selection (`-s`) and the same pairing.

```
cmux plans [--json]                       cmux board <plan> [--json]
cmux new-plan -r <repo>... "<title>"      cmux accept <plan>      cmux pause <plan>
cmux inbox [--json]                       cmux allow <item> [--always]   cmux deny <item> [note]
cmux send <agent> "<text>" [--steer]      cmux reply <item> "<text>"
cmux tail <agent> [--chat]                cmux stop <agent>       cmux resume <agent>
cmux attach [<agent>]                     # the TUI, opened on that agent
cmux login <url>   cmux logout   cmux servers
```

`<agent>` is a path such as `auth-refactor/T-0002/tests`. Listing verbs take `--json`. Exit codes are 0 for success, 1 for a usage error, 2 for a server or connection error.

## 6. Main flows (the same on every client)

1. **Create a plan.** Give a title, repositories and an optional first line. The plan starts in `draft`, and the executor starts in **planning** mode: it talks with you, writes `plan.md`, and creates tasks with dependencies and acceptance criteria.
2. **Accept.** The plan moves to `accepted`, and the executor gets the **execute** kickoff.
3. **Execution.** The executor calls `spawn_task` for each task whose dependencies are met, up to the limit. Each task gets a worktree at `<repo>/.worktrees/<task-id>` on branch `claude/<task-id>`. The executor then ends its turn and parks (`◌`). Task executors may spawn sub-agents. Reports flow up a level and wake the parent.
4. **Questions and permissions** land in "needs you" on every client, and as a push notification on your phone. Whoever answers first wins, and the other clients update at once.
5. **Steering.** Message any agent from any client. A queued message shows `✉1` everywhere until it is delivered.
6. **Detach and come back.** Close the laptop. The plan keeps running on the server. Approve a permission from your phone on the train, then attach from the TUI at home.

## 7. Left out on purpose

- There is no multi-user model. One person, many devices; every paired device has full access, or read-only if paired that way.
- There are no plan or task editing forms beyond creation. Editing happens in `$EDITOR` (TUI), in a plain textarea (web), or by asking the executor.
- There are no PR review or merge actions. Clients link to the PR.
- Cost is shown in tokens, not dollars, because interactive sessions do not report cost.
