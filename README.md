<div align="center">

# cmux

**A fast terminal multiplexer for Claude Code sessions.**

Run many Claude sessions side by side, see which ones need you, and jump back into any past session — with the tmux keys you already know.

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

## Why

Claude Code works best when you run several sessions at once. A plain terminal or tmux cannot tell you which session is busy, which one waits for you, and which one finished while you looked elsewhere. cmux knows.

## Features

- **One tab per session.** Every Claude session runs on its own PTY.
- **Live status in the tab bar.** Each tab shows if its session is working, idle, or finished but not seen.
- **Alerts.** A background session that finishes rings the terminal bell. `Prefix M-n` jumps to the next tab with an alert.
- **Session picker.** Browse every past session from `~/.claude/projects`. Filter, resume, or fork a session in one key.
- **tmux muscle memory.** The default key table, the `:` command prompt, and the short aliases (`neww`, `killw`, …) all work.
- **Shells and tools too.** Open a shell or lazygit in a tab next to your sessions.
- **Fast.** Written in Rust. Only the changed cells are redrawn. Input goes to the active pane byte for byte.

## Install

You need a Rust toolchain and the [Claude Code](https://docs.claude.com/en/docs/claude-code) CLI on your `PATH`.

```sh
git clone https://github.com/georgi/claude-mux.git
cd claude-mux
cargo install --path .
```

## Quick start

```sh
cmux                      # new session in the current directory
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

OPTIONS:
    -r, --resume      Start with the session picker
    -c, --continue    Continue the most recent session in the current dir
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

## Keys

cmux uses the tmux default key table. A tab is a tmux window. Press the prefix (default `Ctrl-b`), then a key. `Ctrl-b Ctrl-b` sends a literal `Ctrl-b` to Claude.

| Key            | tmux command         | Action                                           |
|----------------|----------------------|--------------------------------------------------|
| `c`            | `new-window`         | New Claude session in the current directory      |
| `C`            |                      | New session in another directory (Tab completes) |
| `t`            | `new-window`         | New shell in the current directory               |
| `g`            | `new-window lazygit` | New lazygit in the current directory             |
| `n` / `p`      | `next-window`        | Next / previous tab                              |
| `M-n` / `M-p`  |                      | Next / previous tab with an alert                |
| `0`…`9`        | `select-window`      | Go to a tab (`0` is tab 10)                      |
| `'`            |                      | Go to a tab by typed index                       |
| `l`            | `last-window`        | Last used tab                                    |
| `w` / `f`      | `choose-window`      | List and filter the open tabs                    |
| `s` / `r`      | `choose-session`     | Resume or fork a past Claude session             |
| `,`            | `rename-window`      | Rename the tab                                   |
| `&` / `x`      | `kill-window`        | Stop the session in this tab (asks first)        |
| `[` / `PgUp`   | `copy-mode`          | Scroll back through output                       |
| `<` / `>`      |                      | Move the tab                                     |
| `:`            | `command-prompt`     | Run a command                                    |
| `i`            | `display-message`    | Show pid and session id                          |
| `d`            |                      | Quit (cmux has no server, so no detach)          |
| `?`            | `list-keys`          | Help                                             |

### Session picker

| Key      | Action                                             |
|----------|----------------------------------------------------|
| *type*   | Filter                                             |
| `Enter`  | Resume the session                                 |
| `Ctrl-f` | Fork the session                                   |
| `Ctrl-o` | Start a new session in that directory              |
| `Tab`    | Toggle between all projects and the current directory |

Markers: `●` open in a cmux tab, `◆` running in another terminal.

### Command prompt

<details>
<summary>Commands for the <code>:</code> prompt</summary>

| Command | Action |
|---|---|
| `new-window [-c DIR] [-n NAME] [CMD]` | No `CMD` starts Claude. `shell` starts your shell. Any other `CMD` runs through `$SHELL -lc`, and the tab closes when it exits. |
| `kill-window` | Stop the session in this tab |
| `rename-window NAME` | Rename the tab |
| `select-window -t N` | Go to tab `N` |
| `swap-window -t N` | Swap this tab with tab `N` |
| `next-window` / `previous-window` / `last-window` | Move between tabs |
| `find-window TEXT` | Find a tab by text |
| `choose-session [ID]` | Open the picker, or resume a session by id |
| `display-message` | Show pid and session id |
| `list-keys` | Help |
| `copy-mode` | Scroll back through output |
| `kill-server` | Quit |

The short tmux aliases (`neww`, `killw`, `renamew`, …) also work.

</details>

## How it works

- [`portable-pty`](https://crates.io/crates/portable-pty) runs each `claude` process.
- A [`vt100`](https://crates.io/crates/vt100) parser per tab keeps the screen state.
- [`ratatui`](https://ratatui.rs) draws only the cells that changed.
- Keyboard input goes to the active pane byte for byte. Only the prefix key is intercepted.
- The status comes from `~/.claude/sessions/<pid>.json`. Tab titles come from the terminal title that Claude sets.

## License

[MIT](LICENSE)
