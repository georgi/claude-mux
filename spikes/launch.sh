#!/bin/bash
# usage: launch.sh <session-name> <cwd> <claude args...>
# runs claude in tmux -L spikes with claude-mux's SESSION_ENV scrub and an isolated CLAUDE_CONFIG_DIR (cfg/) so ~/.claude is untouched
S="$(cd "$(dirname "$0")" && pwd)"; name=$1; cwd=$2; shift 2
SCRUB="-u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_SESSION_ID -u CLAUDE_CODE_CHILD_SESSION -u CLAUDE_CODE_SESSION_ATTENDED -u CLAUDE_CODE_MESSAGING_SOCKET -u CLAUDE_CODE_MESSAGING_TOKEN -u CLAUDE_CODE_EXECPATH -u CLAUDE_PID -u CLAUDE_PLUGIN_DATA -u AI_AGENT"
tmux -L spikes kill-session -t $name 2>/dev/null
tmux -L spikes new-session -d -s $name -x ${COLS:-120} -y ${ROWS:-40} -c "$cwd" "env $SCRUB CLAUDE_CONFIG_DIR=$S/cfg $EXTRA_ENV TERM=xterm-256color $(printf '%q ' claude "$@"); sleep 600"
