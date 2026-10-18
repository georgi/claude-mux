#!/usr/bin/env python3
# Tiny stdio MCP server (newline-delimited JSON-RPC). Logs startup env/args to $ENVSRV_LOG; tool get_env(name) returns os.environ[name].
import sys, json, os, time
log = os.environ.get("ENVSRV_LOG", os.path.join(os.path.dirname(__file__), "envsrv.log"))
with open(log, "a") as f:
    f.write(json.dumps({"ts": time.time(), "argv": sys.argv, "CMUX_AGENT": os.environ.get("CMUX_AGENT"), "CMUX_TOKEN": os.environ.get("CMUX_TOKEN"), "cwd": os.getcwd(), "CLAUDE_vars": sorted(k for k in os.environ if k.startswith("CLAUDE"))}) + "\n")
def send(o): sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    try: m = json.loads(line)
    except Exception: continue
    mid, meth = m.get("id"), m.get("method")
    if meth == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": m["params"].get("protocolVersion", "2024-11-05"), "capabilities": {"tools": {}}, "serverInfo": {"name": "envsrv", "version": "0"}}})
    elif meth == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": [{"name": "get_env", "description": "Return the value of an environment variable of the server process", "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}}]}})
    elif meth == "tools/call":
        n = m["params"]["arguments"].get("name", "")
        send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "VALUE=" + str(os.environ.get(n))}]}})
    elif meth == "ping":
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "no"}})
