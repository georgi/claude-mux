# Step 0 spikes

Throwaway harness used to answer spikes S1 to S7 of
[docs/design/03-implementation-plan.md](../docs/design/03-implementation-plan.md)
against a real `claude`. The results live in that document. The spikes now
run as a `cargo test --ignored` suite: `crates/cmuxd/tests/real_claude.rs`.
These scripts stay as the record of how the spikes were first run, and for the
xterm.js half of S7.

Setup: the settings and MCP files use the placeholder `__SPIKES_DIR__`.
Replace it with this directory's absolute path before use:

    sed -i "s#__SPIKES_DIR__#$PWD#g" hooks/*.json mcp/*.json

- `launch.sh <name> <cwd> <claude args...>`: runs `claude` in `tmux -L spikes`
  with claude-mux's environment scrub and `CLAUDE_CONFIG_DIR=cfg/`, so
  `~/.claude` is never touched. `cfg/.claude.json` skips onboarding; with an
  isolated config dir, Claude's credentials must come from the environment.
- `ask.sh`, `cap.sh`, `sendraw.py`: send a prompt, capture the screen, send raw
  bytes (`tmux send-keys -H`).
- `hooks/log-hook.sh <tag>`: appends the hook's stdin JSON to `hooks/log.jsonl`;
  replies with `hooks/resp-<tag>.json` and sleeps `hooks/sleep-<tag>` seconds
  when those files exist. `hooks/*-settings.json` wire it up per spike.
- `mcp/envsrv.py`: a stdio MCP server that reports its environment (S4).
- `s7-vt100/`: `capture.py` records raw PTY bytes of a Claude session;
  `src/main.rs` re-parses `state_formatted()` snapshots in a second vt100;
  `xterm/compare.mjs` does the same against `@xterm/headless`.
