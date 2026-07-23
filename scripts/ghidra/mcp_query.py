#!/usr/bin/env python3
"""Direct client for the headless Ghidra MCP daemon (ghidra.mcp.MCPServer) on localhost:8765.
Framing: 4-byte big-endian length + JSON {"id","method","params"}. Lets a session query the daemon
WITHOUT taking the project lock (bd prefer-ghidra-mcp-daemon-over-perquery-headless)."""
import socket, struct, json, sys, argparse

def query(method, params=None, host="localhost", port=8765, timeout=120):
    req = json.dumps({"id": "1", "method": method, "params": params or {}}).encode()
    with socket.create_connection((host, port), timeout=timeout) as s:
        s.sendall(struct.pack(">I", len(req)) + req)
        hdr = b""
        while len(hdr) < 4:
            c = s.recv(4 - len(hdr))
            if not c: raise IOError("closed reading length")
            hdr += c
        n = struct.unpack(">I", hdr)[0]
        buf = b""
        while len(buf) < n:
            c = s.recv(min(65536, n - len(buf)))
            if not c: raise IOError("closed reading body")
            buf += c
    return json.loads(buf.decode("utf-8", "replace"))

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("method"); ap.add_argument("--params", default="{}"); ap.add_argument("--port", type=int, default=8765)
    a = ap.parse_args()
    print(json.dumps(query(a.method, json.loads(a.params), port=a.port), indent=2))
