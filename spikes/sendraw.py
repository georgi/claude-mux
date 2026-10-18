#!/usr/bin/env python3
# usage: sendraw.py <tmux-session> <python-bytes-literal>   e.g. sendraw.py s3 '\x1b[200~hi\x1b[201~\r'
import sys, subprocess, ast
sess, lit = sys.argv[1], sys.argv[2]
b = ast.literal_eval("b'''" + lit.replace("'''", "") + "'''") if "\n" in lit else ast.literal_eval("b'" + lit.replace("'", "\\'") + "'")
subprocess.run(["tmux", "-L", "spikes", "send-keys", "-t", sess, "-H"] + [format(x, "02x") for x in b], check=True)
