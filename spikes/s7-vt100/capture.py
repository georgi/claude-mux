#!/usr/bin/env python3
"""Run interactive claude on a PTY (100x30), drive a turn, record raw output bytes with timestamps.
Writes capture.bin (raw bytes) and marks.json (list of {label, offset} cut points)."""
import os, pty, sys, time, json, select, struct, fcntl, termios, signal
S = os.path.dirname(os.path.abspath(__file__)); ROOT = os.path.dirname(S)
ROWS, COLS = 30, 100
scrub = "CLAUDECODE CLAUDE_CODE_ENTRYPOINT CLAUDE_CODE_SESSION_ID CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ATTENDED CLAUDE_CODE_MESSAGING_SOCKET CLAUDE_CODE_MESSAGING_TOKEN CLAUDE_CODE_EXECPATH CLAUDE_PID CLAUDE_PLUGIN_DATA AI_AGENT".split()
env = {k: v for k, v in os.environ.items() if k not in scrub}
env.update(TERM="xterm-256color", CLAUDE_CONFIG_DIR=ROOT + "/cfg")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(ROOT + "/repo")
    os.execvpe("claude", ["claude", "--model", "haiku", "--settings", ROOT + "/hooks/allowbash-settings.json"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
buf = bytearray(); marks = []
def pump(sec):
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: d = os.read(fd, 65536)
            except OSError: return
            if not d: return
            buf.extend(d)
def mark(label): marks.append({"label": label, "offset": len(buf)})
def send(b): os.write(fd, b)
pump(8); mark("idle-startup")
send(b"Count from 1 to 120, one number per line, then run bash 'sleep 12' and reply DONE"); pump(1); mark("typed-prompt")
send(b"\r"); pump(2.5); mark("thinking-or-streaming-1")
pump(3); mark("streaming-2")
pump(4); mark("streaming-3")
pump(5); mark("tool-running")
pump(6); mark("tool-running-2")
pump(8); mark("done")
os.kill(pid, signal.SIGKILL)
open(S + "/capture.bin", "wb").write(buf); json.dump({"rows": ROWS, "cols": COLS, "marks": marks}, open(S + "/marks.json", "w"), indent=1)
print(len(buf), marks)
