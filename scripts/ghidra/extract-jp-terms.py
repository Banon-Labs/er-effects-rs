#!/usr/bin/env python3
"""Extract every bounded Japanese run from the warm Ghidra program's defined strings, via the
MCP daemon (no second Ghidra open). Writes a frequency-sorted candidate list for translation.

  scripts/ghidra/extract-jp-terms.py [--port 8765] [--out scripts/ghidra/jp-terms.json] [--limit 5000]

Output JSON: [{"jp": "<run>", "count": N, "example": "<a full string containing it>"}], sorted by
count desc so the highest-impact terms get translated first. "bounded run" = a maximal contiguous
sequence of Japanese characters (the same unit the bridge replaces and flags)."""
import argparse, json, re, subprocess, sys, time
from collections import Counter

BRIDGE = "/home/banon/tools/GhidraMCP-13bm/mcp-bridge/mcp_bridge"

def is_jp(ch):
    o = ord(ch)
    return (0x3040 <= o <= 0x30FF) or (0x4E00 <= o <= 0x9FFF) or \
           (0x3400 <= o <= 0x4DBF) or (0xFF66 <= o <= 0xFF9D)

def jp_runs(s):
    runs, cur = [], []
    for ch in s:
        if is_jp(ch):
            cur.append(ch)
        elif cur:
            runs.append("".join(cur)); cur = []
    if cur:
        runs.append("".join(cur))
    return runs

def walk_strings(obj):
    """Yield every string value anywhere in a parsed JSON structure."""
    if isinstance(obj, str):
        yield obj
    elif isinstance(obj, dict):
        for v in obj.values():
            yield from walk_strings(v)
    elif isinstance(obj, list):
        for v in obj:
            yield from walk_strings(v)

class MCP:
    def __init__(self, port):
        self.p = subprocess.Popen([BRIDGE, "-host", "localhost", "-port", str(port)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
        self.id = 0
        self._send("initialize", {"protocolVersion":"2024-11-05","capabilities":{},
                                  "clientInfo":{"name":"jp-extract","version":"0"}}); self._read()
        self._send("notifications/initialized", notify=True)
    def _send(self, method, params=None, notify=False):
        m = {"jsonrpc":"2.0","method":method}
        if params is not None: m["params"] = params
        if not notify: self.id += 1; m["id"] = self.id
        self.p.stdin.write(json.dumps(m)+"\n"); self.p.stdin.flush()
    def _read(self, t=120):
        end = time.time()+t
        while time.time() < end:
            line = self.p.stdout.readline()
            if not line: return None
            line = line.strip()
            if not line: continue
            try: o = json.loads(line)
            except json.JSONDecodeError: continue
            if "id" in o: return o
        return None
    def call(self, name, args):
        self._send("tools/call", {"name":name,"arguments":args}); r = self._read()
        if not r or "result" not in r: return None
        txt = "".join(c.get("text","") for c in r["result"].get("content",[]) if c.get("type")=="text")
        try: return json.loads(txt)
        except json.JSONDecodeError: return {"_raw": txt}
    def close(self):
        self.p.terminate()
        try: self.p.wait(timeout=5)
        except Exception: self.p.kill()

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--out", default="/home/banon/projects/er-effects-rs/scripts/ghidra/jp-terms.json")
    ap.add_argument("--limit", type=int, default=5000)
    args = ap.parse_args()

    mcp = MCP(args.port)
    counts = Counter()
    example = {}
    offset, total, scanned = 0, None, 0
    try:
        while True:
            page = mcp.call("list_strings", {"offset": offset, "limit": args.limit})
            if not page: break
            if total is None:
                total = page.get("totalCount")
            strings = list(walk_strings(page))
            if not strings: break
            page_items = 0
            for s in strings:
                if not any(is_jp(c) for c in s): continue
                page_items += 1
                for run in jp_runs(s):
                    counts[run] += 1
                    example.setdefault(run, s)
            scanned += 1
            got = page.get("items")
            n = len(got) if isinstance(got, list) else 0
            print(f"  offset={offset} page_strings_with_jp={page_items} unique_so_far={len(counts)}", file=sys.stderr)
            if total is not None:
                offset += args.limit
                if offset >= total: break
            else:
                if n == 0: break
                offset += args.limit
    finally:
        mcp.close()

    out = [{"jp": jp, "count": c, "example": example[jp]} for jp, c in counts.most_common()]
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=2)
    print(f"total strings (program totalCount): {total}")
    print(f"unique Japanese runs: {len(out)}  -> {args.out}")
    for row in out[:25]:
        print(f"  {row['count']:>5}  {row['jp']}")

if __name__ == "__main__":
    main()
