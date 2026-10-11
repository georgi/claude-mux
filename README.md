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

- **Tabs and panes.** Every Claude session runs on its own PTY. Split a tab into panes like in tmux, and watch several sessions at once.
- **Live status everywhere.** Each tab and each pane header shows if its session is working, idle, or finished but not seen.
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

### Ghostty: Cmd keys without the prefix

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
# Shell, lazygit, new session in another directory, tabs with an alert
keybind = cmd+shift+t=text:\x02t
keybind = cmd+g=text:\x02g
keybind = cmd+shift+o=text:\x02C
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
| `Cmd-w` / `Cmd-Shift-w`         | Stop the pane's session / quit cmux       |

The bindings send `Ctrl-b` (`\x02`). If you use another prefix, change `\x02` to the matching control byte, for example `\x01` for `Ctrl-a`. The bindings apply to every Ghostty window, also when cmux does not run. Put them in a separate file and load it with `config-file` if you want to switch them on and off.

## How it works

- [`portable-pty`](https://crates.io/crates/portable-pty) runs each `claude` process.
- Each tab holds a binary split tree of panes, like tmux layout cells.
- A [`vt100`](https://crates.io/crates/vt100) parser per pane keeps the screen state.
- [`ratatui`](https://ratatui.rs) draws only the cells that changed.
- Keyboard input goes to the focused pane byte for byte. Only the prefix key is intercepted.
- The status comes from `~/.claude/sessions/<pid>.json`. Tab titles come from the terminal title that Claude sets.

## License

[MIT](LICENSE)
