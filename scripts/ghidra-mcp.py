#!/usr/bin/env python3
"""Call a Ghidra MCP tool through the running mcp_bridge (stdio -> Ghidra daemon on :8765).

Fallback access to the Ghidra dump when the ghidra MCP is NOT registered as a session tool but the
daemon is up (bd Ghidra Access: MCP-First). Drives the bridge with a minimal MCP JSON-RPC handshake and
one tools/call, printing the tool's text result. Reusable so RE queries don't re-plumb the protocol.

Usage:
  python3 scripts/ghidra-mcp.py <tool_name> '<json-args>'
  e.g. python3 scripts/ghidra-mcp.py decompile_function_by_address '{"address":"0x1426634a0"}'
       python3 scripts/ghidra-mcp.py get_xrefs_to '{"address":"0x1426634a0"}'
"""
from __future__ import annotations

import json
import os
import subprocess
import sys

BRIDGE = os.environ.get(
    "GHIDRA_MCP_BRIDGE",
    os.path.expanduser("~/projects/ghidra-mcp-13bm/mcp-bridge/mcp_bridge"),
)


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    tool = sys.argv[1]
    args = json.loads(sys.argv[2]) if len(sys.argv) > 2 and sys.argv[2].strip() else {}
    msgs = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name": "ghidra-mcp.py", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call",
         "params": {"name": tool, "arguments": args}},
    ]
    stdin = "".join(json.dumps(m) + "\n" for m in msgs)
    try:
        proc = subprocess.run(
            [BRIDGE], input=stdin, capture_output=True, text=True, timeout=30
        )
    except FileNotFoundError:
        print(f"bridge not found: {BRIDGE} (set GHIDRA_MCP_BRIDGE)")
        return 3
    except subprocess.TimeoutExpired:
        print("bridge timed out (Ghidra daemon slow or the query is heavy)")
        return 4

    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if obj.get("id") == 2:
            res = obj.get("result", {})
            if "content" in res:
                for c in res["content"]:
                    if c.get("type") == "text":
                        print(c["text"])
            else:
                print(json.dumps(res, indent=2))
            return 0
    sys.stderr.write(proc.stderr[-2000:] if proc.stderr else "(no result line from bridge)\n")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
