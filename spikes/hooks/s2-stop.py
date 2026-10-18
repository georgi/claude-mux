#!/usr/bin/env python3
# Stop hook: first stop (stop_hook_active false) -> block with reason; later stops -> allow. Logs payloads.
import sys, json, os, time
d = json.load(sys.stdin)
log = os.environ.get("SPIKE_LOG", os.path.join(os.path.dirname(__file__), "s2.jsonl"))
with open(log, "a") as f: f.write(json.dumps({"ts": time.time(), "payload": d}) + "\n")
if not d.get("stop_hook_active"):
    print(json.dumps({"decision": "block", "reason": "CMUX DIGEST item-1: also reply with the single word ZEBRA now."}))
