<div align="center">

# cmux

**A fast terminal multiplexer for Claude Code sessions.**

Run many Claude sessions on a server, from your laptop, a browser or your phone. See which ones need you, answer them from anywhere, and let plans of agents work while you are away — with the tmux keys you already know.

[![Rust](https://img.shields.io/badge/rust-2024_edition-orange?logo=rust)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey)

</div>

```
 1 ⠹ api-refactor   2 ● fix-flaky-test   3 ○ docs   4 $ zsh
┌──────────────────────────────────────────────────────────────────┐
│ ✻ Working… (esc to interrupt)                                    │
│                                                                  │
│ > Refactor the auth middleware to use the new token store        │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

Tab status: `⠹` working · `●` finished, not seen yet · `○` idle · `?` other state · `$` shell · `▸` command

## How it fits together

```
 laptop                      server (Linux box or VPS)                 phone
┌──────────┐  HTTPS + WS   ┌──────────────────────────────────┐   ┌──────────┐
│ cmux TUI │──────────────▶│ cmuxd                            │◀──│ web app  │
│ cmux CLI │               │  plans · agents · terminals      │   │ (PWA) +  │
└──────────┘               │  claude ── claude ── claude ...  │──▶│ Web Push │
                           └──────────────────────────────────┘   └──────────┘
```

- **`cmuxd`** is the server. It owns every Claude process, terminal and plan. They keep running when no client is attached, and they survive a restart of `cmuxd` (working agents are resumed).
- **`cmux`** is the client: a TUI with tmux keys, plus CLI verbs for scripts. `prefix d` detaches, and every agent keeps going.
- **The web app** is served by `cmuxd` at `/`. On a phone it installs as an app and gets push notifications with **Allow** and **Deny** buttons.
- **Plans**: a Claude session writes a plan of tasks; you accept it; an executor agent starts one task agent per task, each in its own git worktree, and tracks the PRs and CI.

Without a server, `cmux` still works as a plain local multiplexer for Claude sessions ([Local mode](#local-mode)).

## Install

You need the [Claude Code](https://docs.claude.com/en/docs/claude-code) CLI, logged in, on the machine that runs the agents. `gh` (logged in) turns on PR and CI tracking.

**Server, from a release** (Linux, `x86_64` or `aarch64`, static binaries with the web app inside):

```sh
arch=$(uname -m)   # x86_64 or aarch64
curl -fLo cmuxd "https://github.com/georgi/claude-mux/releases/latest/download/cmuxd-$arch-unknown-linux-musl"
install -m 755 cmuxd ~/.local/bin/cmuxd
```

Each release has a `SHA256SUMS` file. Check the binary with `sha256sum -c --ignore-missing SHA256SUMS`.

**From source** (Rust 2024 edition, and Node 24 for the web app):

```sh
git clone https://github.com/georgi/claude-mux.git
cd claude-mux
(cd web && npm ci && npm run build)      # the web app, embedded into cmuxd
cargo install --path crates/cmuxd        # the server
cargo install --path crates/cmux         # the client, on every machine you work from
```

Without `web/dist`, `cmuxd` serves a placeholder page at `/`.

## Run the server: `cmuxd setup`

On the server, as the user that the agents should run as:

```sh
cmuxd setup --dry-run   # show what it will do
cmuxd setup             # install and start the service
cmuxd doctor            # check config, listener, socket, store, claude and gh
```

`cmuxd setup` writes the systemd user unit `~/.config/systemd/user/cmuxd.service`, enables and starts it, and turns on linger so the server keeps running after you log out. It also checks that `claude` and `gh` are installed and logged in. It is safe to run twice; a changed unit is reloaded and the service restarted.

Without systemd (or on macOS), run `cmuxd serve` in a terminal or under your own supervisor.

| Command | Action |
|---|---|
| `cmuxd serve` | Run the server: the Unix socket, and TCP on `127.0.0.1:7878` |
| `cmuxd setup [--dry-run]` | Install the systemd user service, linger, and check `claude` and `gh` |
| `cmuxd pair [--scope full\|read]` | Print a one-time pairing code and a QR code (needs a running server) |
| `cmuxd doctor [AGENT]` | Check the installation, or show one agent: state, pending items, last 50 diagnostics lines |

Every command takes `--config` / `CMUXD_CONFIG`, `--data-dir` / `CMUXD_DATA_DIR`, `--socket` / `CMUXD_SOCKET` and `--listen` / `CMUXD_LISTEN` (`off` disables TCP). The defaults are `~/.config/cmuxd/config.toml`, `~/.local/share/cmuxd`, and `$XDG_RUNTIME_DIR/cmuxd/api.sock`.

## Remote access

`cmuxd` listens on loopback only. It refuses to serve plain HTTP on another address unless `allow_plain_http = true`, because the token would cross the network in clear text. Put HTTPS in front of it. PWAs and Web Push need HTTPS with a valid certificate.

### Tailscale (recommended)

1. Install Tailscale on the server and on each device, and log in to the same tailnet. Turn on HTTPS certificates in the admin console (DNS → HTTPS Certificates).
2. On the server, put HTTPS in front of the port:

   ```sh
   tailscale serve --bg 7878          # https://box.your-tailnet.ts.net → 127.0.0.1:7878
   tailscale serve status
   ```

3. Set the URL in `~/.config/cmuxd/config.toml`, so that pairing codes and QR codes use it, and restart `cmuxd` (`systemctl --user restart cmuxd`):

   ```toml
   public_url = "https://box.your-tailnet.ts.net"
   ```

The server is reachable only from your tailnet.

### Caddy (public HTTPS with Let's Encrypt)

For a box with a public DNS name. Caddy gets the certificate and proxies the WebSocket without extra settings. `/etc/caddy/Caddyfile`:

```caddy
cmux.example.com {
	reverse_proxy 127.0.0.1:7878
}
```

```sh
sudo systemctl reload caddy
```

Then set `public_url = "https://cmux.example.com"` and restart `cmuxd`. Keep the Host header as Caddy sends it: a session cookie WebSocket must come from the same origin. This exposes a shell-equivalent service to the internet behind device tokens; prefer Tailscale if you can.

### SSH tunnel

No HTTPS, no PWA, but no setup either:

```sh
ssh -N -L 7878:127.0.0.1:7878 box     # then use http://127.0.0.1:7878
```

### Built-in TLS

Build with `--features tls` and set `[tls] cert` and `key` (PEM files). `cmuxd` then serves HTTPS itself, also on a non-loopback address.

## Pair a device

On the server, while `cmuxd` runs:

```sh
cmuxd pair                 # one-time 8-character code and a QR code, valid for 5 minutes
cmuxd pair --scope read    # a device that can watch but not act
```

On a laptop:

```sh
cmux login https://box.your-tailnet.ts.net   # enter the code when asked (or --code CODE)
cmux servers                                  # paired servers; * is the default
cmux servers default NAME                     # connect to NAME by default
cmux logout [NAME]                            # forget a server
```

On a phone, scan the QR code. The page logs the phone in.

- The first paired server becomes the default. `--name` chooses another name.
- A `full` device (the default) can run shells and Claude as the server's user. Pair only machines you trust. On a shared machine, run `cmuxd` as a dedicated Unix user.
- A code is single use, and the server allows 5 pairing attempts per minute.
- The client keeps its token in `~/.config/cmux/client.toml` (mode 0600). The server keeps only a SHA-256 hash of it.
- To revoke a device, use **Settings → Devices** in the web app, or `DELETE /v1/devices/<id>`. Its open streams close and its push subscriptions are dropped.

## Attach with the TUI

```sh
cmux                  # the default server
cmux -s box           # a named server; `-s local` is the cmuxd on this machine
cmux --local          # plain claude-mux, no server
```

Without `-s`, `cmux` uses the default server in `client.toml`. If none is set, it uses this machine's cmuxd when its socket is listening, and otherwise runs claude locally.

Every key in [Keys](#keys) works on a server, with these differences:

- `prefix d` detaches. The server keeps running everything.
- `c`, `t`, `g`, `%` and `"` start Claude, a shell or lazygit on the server.
- `x` and `&` stop the agent or the sessions in the pane or tab (asks first).
- `prefix s` lists the sessions on the server.
- `prefix =` takes over the terminal's size from another client.
- One server per cmux. For another, run `cmux -s NAME` in a second terminal.

Each Claude that `cmuxd` starts runs in `auto` permission mode with Remote Control on, unless a level in `config.toml` sets another mode.

## Plans

A plan is a workspace, a task is a tab and a sub-agent is a pane. The plan tree sits on the left. `prefix Tab` focuses it: `jk` moves, `Enter` opens, `Esc` goes back.

To make a plan, open a Claude session with `prefix c` and ask for one, for example "plan a retry layer for the API client". The server gives each Claude session the cmux plan tools: `create_plan`, `create_task`, `update_task`, `get_plan`, `get_task`, `list_plans` and `accept_plan`. Claude writes the plan and its tasks as a draft, and no agent starts. Review the plan in the tree, then push `A` to accept it, or tell Claude to start it. Then the executor starts the tasks: one agent per task, in a git worktree on the branch `claude/T-000N`. Task agents may start sub-agents for separable work.

| Key                  | Action                                                                                                  |
|----------------------|---------------------------------------------------------------------------------------------------------|
| `A` (in the tree)    | Accept the plan and start it, or accept a task                                                          |
| `prefix m`           | Message the focused agent. `Enter` queues it for the end of the turn, `Alt-Enter` steers now           |
| `prefix v`           | Board view of the plan: cards in columns by state                                                       |
| `prefix e` / `u`     | Go to the plan's executor / up one level                                                                |
| `prefix a`           | Needs-you overlay: `y` allow (or accept a plan), `Y` allow always, `n` deny (or send a plan back), `r` reply to a question, `d` dismiss, `Enter` jump to the agent |
| `prefix M-a` / `M-A` | Next / previous item that needs you                                                                     |

In the tree the other keys act on the selected node: `S` pause or resume a plan, `s` message, `x` stop, `R` resume a stopped agent, `c` cancel a task, `X` delete the plan (it stops the plan's agents first), `o` open its PR, `g` lazygit and `D` diff in its worktree. On the board, the same keys act on the selected card.

**Needs you** items are permission requests, questions from agents (`ask_user`), plans ready for review, stalled agents (no progress for 15 minutes, marked `⚠`), agents that exited without a report, and PRs with red CI after the third failure. `cmuxd` never kills a stalled agent by itself.

**GitHub**: `cmuxd` polls `gh pr view` for each task's PR (every 60 s, every 15 s for a task in `testing`) and moves the task when the PR merges or CI turns red or green. If `gh` is missing or logged out, PR tracking is off and `GET /v1/status` says why.

## The web app

Open the server URL in a browser and log in with a pairing code. The web app shows the same state as the TUI:

- **Needs you**: answer permissions (allow, allow always, deny), questions and plan reviews.
- **Plans**: the plan tree, a board table per plan, new plans, and repositories (add one by path or clone a URL).
- **Agents**: a chat view of the agent's conversation, and the live terminal (xterm.js, fully interactive).
- **Settings**: devices (revoke), push notifications per kind, and the server's limits and Claude options.

Below 640 px wide it switches to the phone layout. In terminal view a key bar gives `Esc`, `Tab`, `Ctrl`, arrows, `^C`, `⏎`, and `1` `2` `3` for Claude's numbered prompts.

### Phone: install the app and turn on push

1. Set up [Tailscale](#tailscale-recommended) or [Caddy](#caddy-public-https-with-lets-encrypt), and set `public_url`.
2. Run `cmuxd pair` and scan the QR code with the phone.
3. Install the app. On Android Chrome, use **Install app** in the browser menu. On iPhone, open the page in Safari, then **Share → Add to Home Screen**, and open the app from the home screen (iOS gives Web Push only to installed apps).
4. In the app, open **Settings → Notifications** and push **Enable push on this device**. Choose the kinds this device wants: permission, question, plan review, stalled, CI failing, plan done.

Permission notifications have **Allow** and **Deny** buttons, and questions have **Reply** where the phone supports it. A notification carries the item id, its kind and a summary of at most 60 characters. It never carries a message body or a tool input beyond that summary.

`cmuxd` makes its VAPID key on first start (`vapid.key` in the data directory, mode 0600) and keeps subscriptions in `push.json`. A subscription that the push service reports as gone (HTTP 404 or 410) is dropped.

No HTTPS, or a phone without Web Push? Set `[push] ntfy` to a topic URL and subscribe to the topic in the [ntfy](https://ntfy.sh) app. It gets the same notifications as a plain POST. Pick a long random topic name: anyone who knows it can read it.

## CLI verbs

The same client talks to the server from scripts. Every verb takes `-s NAME`, and the list verbs take `--json`.

```
cmux plans [--json]                          List plans
cmux board <plan> [--json]                   List the tasks of a plan
cmux new-plan -r <repo>... [--request TEXT] <title>
                                             Create a plan (--request starts its executor)
cmux accept <plan>                           Accept a proposed plan
cmux pause <plan>                            Pause a plan
cmux inbox [--json]                          List what needs you
cmux allow <item> [--always]                 Allow a permission request
cmux deny <item> [note]                      Deny a permission request
cmux reply <item> <text>                     Answer a question
cmux send <agent> <text> [--steer]           Queue a message, or steer (interrupt)
cmux stop <agent>                            Stop an agent
cmux resume <agent>                          Resume a stopped agent
cmux tail <agent> [--chat]                   Stream the agent's terminal (--chat: its conversation)
cmux attach [<agent>]                        Open the TUI on that agent
```

`<plan>` is a plan id or the slug of its title. `<agent>` is `<plan>/executor`, `<plan>/T-0002` or `<plan>/T-0002/<name>`; `<plan>` alone is the executor. `<item>` is `I-<n>` (see `cmux inbox`). Exit codes: 0 success, 1 usage error, 2 server or connection error.

## Keys

cmux uses the tmux default key table. A tab is a tmux window, and a tab can hold several panes. Press the prefix (default `Ctrl-b`), then a key. `Ctrl-b Ctrl-b` sends a literal `Ctrl-b` to Claude.

| Key            | tmux command         | Action                                           |
|----------------|----------------------|--------------------------------------------------|
| `c`            | `new-window`         | New Claude session in the current directory      |
| `C`            |                      | New session in another directory (Tab completes) |
| `t`            | `new-window`         | New shell in the current directory               |
| `g`            | `new-window lazygit` | New lazygit in the current directory             |
| `%` / `"`      | `split-window`       | New Claude pane to the right / below             |
| `←` `→` `↑` `↓` | `select-pane`       | Go to the pane in that direction (`←` `→` at the edge go to the previous / next tab) |
| `C-arrow` / `M-arrow` | `resize-pane` | Resize the pane by 1 / 5 cells                   |
| `o` / `;`      | `select-pane`        | Next pane / last used pane                       |
| `z`            | `resize-pane -Z`     | Zoom the pane to the full tab (toggle)           |
| `{` / `}`      | `swap-pane`          | Swap the pane with the previous / next one       |
| `!`            | `break-pane`         | Move the pane to a new tab                       |
| `Space`        | `next-layout`        | Next preset layout                               |
| `M-1`…`M-5`    | `select-layout`      | even-horizontal, even-vertical, main-vertical, main-horizontal, tiled |
| `n` / `p`      | `next-window`        | Next / previous tab                              |
| `M-n` / `M-p`  |                      | Next / previous tab with an alert                |
| `0`…`9`        | `select-window`      | Go to a tab (`0` is tab 10)                      |
| `'`            |                      | Go to a tab by typed index                       |
| `l`            | `last-window`        | Last used tab                                    |
| `w` / `f`      | `choose-window`      | List and filter the open tabs                    |
| `s` / `r`      | `choose-session`     | Resume or fork a past Claude session             |
| `,`            | `rename-window`      | Rename the tab                                   |
| `x`            | `kill-pane`          | Stop the session in this pane (asks first)       |
| `&`            | `kill-window`        | Stop all sessions in this tab (asks first)       |
| `[` / `PgUp`   | `copy-mode`          | Scroll back through output                       |
| `<` / `>`      |                      | Move the tab                                     |
| `:`            | `command-prompt`     | Run a command                                    |
| `i`            | `display-message`    | Show pid and session id                          |
| `d`            | `detach-client`      | Quit (with a server: detach, agents keep running) |
| `?`            | `list-keys`          | Help                                             |

### Session picker

| Key      | Action                                             |
|----------|----------------------------------------------------|
| *type*   | Filter                                             |
| `Enter`  | Resume the session                                 |
| `Ctrl-f` | Fork the session                                   |
| `Ctrl-o` | Start a new session in that directory              |
| `Tab`    | Toggle between all projects and the current directory |

Markers: `●` open in cmux, `◆` running in another terminal.

### Panes

A split starts a new Claude session in the directory of the current pane. To split with a shell or another program, use the command prompt: `:splitw -h shell` or `:splitw lazygit`.

When a tab has more than one pane, each pane gets a header row with its status, name and directory. Click a pane to focus it. Drag a divider, or the header row of a pane, to resize. The tab bar shows the pane count (`⊞3`) and a `Z` for a zoomed pane.

### Command prompt

<details>
<summary>Commands for the <code>:</code> prompt</summary>

| Command | Action |
|---|---|
| `new-window [-c DIR] [-n NAME] [CMD]` | No `CMD` starts Claude. `shell` starts your shell. Any other `CMD` runs through `$SHELL -lc`, and the tab closes when it exits. |
| `split-window [-h] [-c DIR] [CMD]` | Split the pane. `-h` puts the new pane to the right, else below. `CMD` works as for `new-window`. |
| `kill-window` | Stop all sessions in this tab |
| `kill-pane` | Stop the session in this pane |
| `select-pane -L\|-R\|-U\|-D\|-l\|-t N` | Go to a pane by direction, the last pane, or index (from 0) |
| `last-pane` | Go to the last used pane |
| `resize-pane -L\|-R\|-U\|-D [N]` | Move the pane's divider by `N` cells (default 1) |
| `resize-pane -Z` | Zoom the pane (toggle) |
| `swap-pane -U\|-D` | Swap the pane with the previous / next one |
| `break-pane` | Move the pane to a new tab |
| `select-layout NAME` / `next-layout` | Apply a preset layout |
| `rename-window NAME` | Rename the tab |
| `select-window -t N` | Go to tab `N` |
| `swap-window -t N` | Swap this tab with tab `N` |
| `next-window` / `previous-window` / `last-window` | Move between tabs |
| `find-window TEXT` | Find a tab by text |
| `choose-session [ID]` | Open the picker, or resume a session by id |
| `display-message` | Show pid and session id of the pane |
| `list-keys` | Help |
| `copy-mode` | Scroll back through output |
| `kill-server` | Quit |

The short tmux aliases (`neww`, `killw`, `renamew`, …) also work.

</details>

## Configuration reference

`cmuxd` reads `~/.config/cmuxd/config.toml` (or `$XDG_CONFIG_HOME/cmuxd/config.toml`, or `--config`). Every key is optional. An unknown key is an error, so a typo stops `cmuxd` at start. Restart `cmuxd` after a change.

This is every key with its default. A line that starts with `#` and no space is a key without a default, shown with an example value.

<!-- config-reference: checked by `cargo test -p cmuxd readme_config_reference` -->
```toml
#data_dir = "/home/me/.local/share/cmuxd"    # plans, devices, diagnostics; default ~/.local/share/cmuxd
#socket = "/run/user/1000/cmuxd/api.sock"    # default $XDG_RUNTIME_DIR/cmuxd/api.sock
listen = "127.0.0.1:7878"                    # TCP address, or "off"
allow_plain_http = false                     # serve plain HTTP on a non-loopback address
#name = "box"                                # shown in /v1/status; default the host name
#public_url = "https://box.example.ts.net"   # the URL in pairing codes and QR codes
auto_resume = true                           # after a restart, resume the agents that were working

#[tls]                                       # built-in TLS (build with --features tls)
#cert = "/etc/cmuxd/cert.pem"
#key = "/etc/cmuxd/key.pem"

[pairing]
ttl_secs = 300                               # how long a pairing code is valid
attempts_per_min = 5                         # pairing attempts per minute, all callers together

[agents]
claude_bin = "claude"                        # found on PATH
permission_wait_secs = 600                   # how long a permission request waits for a remote answer
pre_trust = true                             # mark each agent's directory trusted in Claude's config

[agents.executor]                            # the same keys for [agents.task] and [agents.sub]
#permission_mode = "auto"                    # unset: auto
#model = "opus"                              # unset: Claude's own default

[agents.task]

[agents.sub]

[limits]                                     # plan limits apply to new plans
tasks_per_plan = 2                           # live task agents per plan
subagents_per_task = 2
live_processes = 8                           # Claude processes on the whole server
git_ops = 2                                  # concurrent git operations

[timeouts]                                   # seconds
question_secs = 3600                         # ask_user and ask_parent questions expire
#permission_wait_secs = 600                  # wins over [agents] permission_wait_secs when set
stall_secs = 900                             # a working agent with no progress gets ⚠
idle_without_report_secs = 600               # an idle task agent without a report needs you

[github]                                     # PR and CI tracking through gh
enabled = true
gh_bin = "gh"
open_secs = 60                               # every open PR
testing_secs = 15                            # a task in testing whose agent is idle

[push]                                       # Web Push needs no settings
#ntfy = "https://ntfy.sh/your-long-random-topic"   # also POST every notification here
#ntfy_token = "tk_..."                       # for a protected topic
#subject = "mailto:you@example.com"          # VAPID contact; default public_url when it is HTTPS
```

Files in the data directory: `plans/` (one directory per plan, with its agents' files), `devices.json` (device token hashes), `repos.json`, `push.json`, `vapid.key`, and `diagnostics/cmuxd.jsonl`. The diagnostics log has one JSON line per hook, tool call, delivery, spawn, exit and client connect, with timings. It never holds a prompt, a message body, a tool input or a token.

The prompts that agents get are compiled in. A file with the same name in `~/.config/cmuxd/prompts/` overrides each one (see `crates/cmux-core/prompts/`).

The client keeps its servers in `~/.config/cmux/client.toml` (name, URL, token, default; mode 0600), and its splits and zoom per server in `~/.config/cmux/view-<server>.toml`.

## Ghostty bindings

Ghostty can send a key sequence for a key binding. Use this to map `Cmd` shortcuts to the cmux prefix and a key, so that `Cmd-t` opens a new session and `Cmd-1` goes to tab 1. Add these lines to your Ghostty config (`~/.config/ghostty/config`, or `~/Library/Application Support/com.mitchellh.ghostty/config` on macOS):

```ini
# cmux: Cmd+key sends the prefix Ctrl-b (\x02) and a cmux key
# Tabs: Cmd+1..9 selects a tab, Cmd+0 selects tab 10
keybind = cmd+digit_1=text:\x021
keybind = cmd+digit_2=text:\x022
keybind = cmd+digit_3=text:\x023
keybind = cmd+digit_4=text:\x024
keybind = cmd+digit_5=text:\x025
keybind = cmd+digit_6=text:\x026
keybind = cmd+digit_7=text:\x027
keybind = cmd+digit_8=text:\x028
keybind = cmd+digit_9=text:\x029
keybind = cmd+digit_0=text:\x020
keybind = cmd+t=text:\x02c
keybind = cmd+w=text:\x02x
keybind = cmd+shift+w=text:\x02d
keybind = cmd+shift+[=text:\x02p
keybind = cmd+shift+]=text:\x02n
keybind = cmd+shift+left=text:\x02<
keybind = cmd+shift+right=text:\x02>
keybind = cmd+r=text:\x02,
keybind = cmd+s=text:\x02w
keybind = cmd+shift+c=text:\x02[
# Panes: split right / below, move, resize, zoom
keybind = cmd+d=text:\x02%
keybind = cmd+shift+d=text:\x02"
keybind = cmd+alt+h=text:\x02\x1b[D
keybind = cmd+alt+j=text:\x02\x1b[B
keybind = cmd+alt+k=text:\x02\x1b[A
keybind = cmd+alt+l=text:\x02\x1b[C
keybind = cmd+ctrl+h=text:\x02\x1b[1;3D
keybind = cmd+ctrl+j=text:\x02\x1b[1;3B
keybind = cmd+ctrl+k=text:\x02\x1b[1;3A
keybind = cmd+ctrl+l=text:\x02\x1b[1;3C
keybind = cmd+shift+enter=text:\x02z
# Plans (server mode): focus the plan tree, or go back to the pane
keybind = cmd+b=text:\x02\t
# Shell, lazygit, new session in another directory, session picker, tabs with an alert
keybind = cmd+shift+t=text:\x02t
keybind = cmd+g=text:\x02g
keybind = cmd+shift+o=text:\x02C
keybind = cmd+p=text:\x02s
keybind = cmd+j=text:\x02\x1bn
keybind = cmd+shift+j=text:\x02\x1bp
# Claude: Shift+Enter inserts a newline
keybind = shift+enter=text:\n
```

| Shortcut                        | Action                                    |
|---------------------------------|-------------------------------------------|
| `Cmd-1`…`Cmd-9`, `Cmd-0`        | Go to a tab                               |
| `Cmd-t`                         | New Claude session                        |
| `Cmd-Shift-o`                   | New session in another directory          |
| `Cmd-p`                         | Resume or fork a past session (picker)    |
| `Cmd-Shift-t` / `Cmd-g`         | New shell / lazygit                       |
| `Cmd-Shift-[` / `Cmd-Shift-]`   | Previous / next tab                       |
| `Cmd-j` / `Cmd-Shift-j`         | Next / previous tab with an alert         |
| `Cmd-Shift-←` / `Cmd-Shift-→`   | Move the tab                              |
| `Cmd-r`                         | Rename the tab                            |
| `Cmd-s`                         | List the open tabs                        |
| `Cmd-Shift-c`                   | Scroll back through output                |
| `Cmd-d` / `Cmd-Shift-d`         | Split right / below                       |
| `Cmd-Alt-h` `j` `k` `l`         | Go to the pane left / down / up / right   |
| `Cmd-Ctrl-h` `j` `k` `l`        | Resize the pane by 5 cells                |
| `Cmd-Shift-Enter`               | Zoom the pane                             |
| `Cmd-b`                         | Focus the plan tree, or go back (server)  |
| `Cmd-w` / `Cmd-Shift-w`         | Stop the pane's session / quit cmux       |

The bindings send `Ctrl-b` (`\x02`). If you use another prefix, change `\x02` to the matching control byte, for example `\x01` for `Ctrl-a`. The bindings apply to every Ghostty window, also when cmux does not run. Put them in a separate file and load it with `config-file` if you want to switch them on and off.

## Local mode

Without a server, `cmux` runs `claude` itself, one PTY per session, like a tmux built for Claude. All [keys](#keys) work. Plans, the web app and push need `cmuxd`.

```sh
cmux --local              # new session in the current directory
cmux ~/dev/a ~/dev/b      # one tab per directory
cmux -r                   # start with the session picker
cmux -c                   # continue the most recent session here
cmux -- --model opus      # pass arguments to every claude process
cmux -p a                 # use Ctrl-a as the prefix
```

<details>
<summary>All options</summary>

```
USAGE:
    cmux [OPTIONS] [DIR...] [-- CLAUDE_ARGS...]
    cmux login URL | logout [NAME] | servers    Pair with and manage cmuxd servers
    cmux [-s NAME] VERB ARGS...                 See CLI verbs

OPTIONS:
    -r, --resume      Start with the session picker
    -c, --continue    Continue the most recent session in the current dir
    -s, --server NAME Attach to this cmuxd server (see `cmux servers`; `local` is this machine)
        --local       Run claude locally even if a server is configured
    -p, --prefix KEY  Prefix key, a letter for Ctrl-<letter> (default: b)
        --no-bell     Do not ring the terminal bell when a background session finishes
        --no-mouse    Leave the mouse to the terminal (native selection, no wheel scroll)
    -h, --help        Show this help
    -V, --version     Show the version

ENVIRONMENT:
    CMUX_CLAUDE       Path to the claude binary (default: claude)
    CMUX_PREFIX       Default prefix letter
```

</details>

Tab status: `⠹` working · `●` finished, not seen yet · `○` idle · `?` other state · `$` shell · `▸` command. A background session that finishes rings the terminal bell. `prefix M-n` jumps to the next tab with an alert.

## Development

```sh
cargo build --workspace                # debug build of cmux and cmuxd
cargo test --workspace                 # all tests
cargo test -p cmux-core engine         # the plan engine only
cargo test -p cmuxd --test agents      # end to end: agents, hooks, MCP tools, permissions
cargo test -p cmuxd --test api         # the REST API
cargo test -p cmuxd --test stream      # the push stream
cargo test -p cmuxd --test security    # the security checklist (architecture section 9)
(cd web && npm test)                   # the web app
```

The end-to-end tests do not run `claude`. They use a stand-in, [`crates/cmuxd/examples/fake-claude.rs`](crates/cmuxd/examples/fake-claude.rs), that plays a script for each agent. Its doc comment describes the script format.

**Real-claude suite.** The behaviour of Claude Code that cmuxd depends on (hooks, `--settings` merge, `Stop` blocks, bracketed paste, MCP env, `PermissionRequest`, screen snapshots: spikes S1 to S7) is checked against the real `claude` on `PATH`. These tests are ignored by default, use `--model haiku`, and cost a few cents:

```sh
cargo test --workspace -- --ignored
```

See the doc comment of [`crates/cmuxd/tests/real_claude.rs`](crates/cmuxd/tests/real_claude.rs) for the login it needs.

**Load test.** [`crates/cmuxd/examples/load.rs`](crates/cmuxd/examples/load.rs) starts a real `cmuxd` with 10 fake agents that print at a steady rate and 3 WebSocket clients, then measures idle CPU, input-to-echo latency and RSS. The results are in [the implementation plan](docs/design/03-implementation-plan.md#step-16-docs-performance-security-review-m--m5).

```sh
cargo run --release -p cmuxd --example load
```

To try a change with real Claude without touching your own server state, give the server a scratch data dir and socket. `cmux` finds the socket through the same variable:

```sh
export CMUXD_DATA_DIR=/tmp/cmuxd-dev CMUXD_SOCKET=/tmp/cmuxd-dev/api.sock
cargo run -p cmuxd -- serve --listen off   # terminal 1
cargo run -p cmux -- -s local              # terminal 2
```

Design documents: [UI design](docs/design/01-ui-design.md), [architecture](docs/design/02-architecture.md), [implementation plan](docs/design/03-implementation-plan.md).

## How it works

- `cmuxd` is one Tokio process. An actor owns the plan state; the REST API, the WebSocket stream, the hooks and the MCP servers all send it commands.
- [`portable-pty`](https://crates.io/crates/portable-pty) runs each `claude` process on the server. A [`vt100`](https://crates.io/crates/vt100) parser per terminal keeps the screen, so a client that attaches late gets a snapshot, then live bytes.
- Claude's hooks (`cmuxd hook`) report exact status and hold permission requests open for a remote answer. An MCP server per agent (`cmuxd mcp`) gives each role its plan tools.
- The TUI keeps a local `vt100` parser per pane and draws only the cells that changed with [`ratatui`](https://ratatui.rs). Keyboard input goes to the server byte for byte. Only the prefix key is intercepted.
- The web app (Vite, Preact, xterm.js) is embedded into `cmuxd` and served at `/`.

## License

[MIT](LICENSE)
