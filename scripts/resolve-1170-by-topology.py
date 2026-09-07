#!/usr/bin/env python3
"""Pick between equally-shaped 1.17 candidates using each candidate's call-graph shape.

The gap this fills. Three tools already exist and each runs out of evidence somewhere:
`map-rvas-1162-to-1170.py` reports "9 shape matches" when a body is too generic to identify;
`resolve-1170-by-caller-rel32.py` needs an already-mapped caller and finds none for a function
reached only indirectly; `find-1170-vtable-by-rtti.py` needs the function to sit in a vtable.
A function with a plain body, no mapped caller and no vtable slot falls through all three.

What is still distinctive about such a function. Not its bytes -- but Ghidra has analysed both
images independently, so each candidate carries a `.pdata`-declared size and a measured number of
callers and callees. Those are properties of the function's role in the program, not of its
opening instructions, and the nine shape-alike candidates almost never share them. Requiring an
exact size match plus equal caller and callee counts turns a nine-way shrug into one answer, and
when it does not, this says so rather than picking the first.

Why exact size and not "CLOSE". 1.17 does move code and does grow structures, so a body can
legitimately change length -- but then the honest verdict is that topology cannot decide it, and
the pair belongs in `verify-rva-map-1170.py` for an instruction-level ruling. A tolerance band
here would just be the nearest-anchor guess wearing a different hat, and a guess that lands
mid-instruction is the specific failure that got `dump-deobf-shift.py` deleted from this repo.

The score is reported alongside the answer so a reader can see which properties agreed. A single
candidate matching on size alone is reported as weak; size plus both arities is what this tool is
for.
"""
import importlib.util
import json
import os
import socket
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000


def _mapper():
    path = os.path.join(ROOT, "scripts", "map-rvas-1162-to-1170.py")
    spec = importlib.util.spec_from_file_location("map_rvas", path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def mcp(method, params, port):
    req = json.dumps({"id": "1", "method": method, "params": params}).encode()
    with socket.create_connection(("localhost", port), timeout=120) as s:
        s.sendall(struct.pack(">I", len(req)) + req)
        hdr = b""
        while len(hdr) < 4:
            c = s.recv(4 - len(hdr))
            if not c:
                raise IOError("closed")
            hdr += c
        n = struct.unpack(">I", hdr)[0]
        buf = b""
        while len(buf) < n:
            c = s.recv(min(65536, n - len(buf)))
            if not c:
                raise IOError("closed")
            buf += c
    return json.loads(buf.decode("utf-8", "replace")).get("result")


def profile(va, port):
    r = mcp("getFunctionByAddress", {"address": f"{va:x}"}, port)
    if not r or "entry" not in r:
        return None
    return {
        "entry": int(r["entry"], 16),
        "size": r.get("size", 0),
        "callees": len(r.get("callees", [])),
        "callers": len(r.get("callers", [])),
    }


def candidates(mapper, target_image, pattern, mask):
    n = len(pattern)
    anchor = next(i for i, m in enumerate(mask) if m)
    needle = pattern[anchor : anchor + 1]
    out, start = [], 0
    while len(out) < 64:
        i = target_image.find(needle, start)
        if i < 0:
            break
        off = i - anchor
        start = i + 1
        if off < 0 or off + n > len(target_image):
            continue
        if all(mask[j] == 0 or target_image[off + j] == pattern[j] for j in range(n)):
            out.append(off + BASE)
    return out


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: resolve-1170-by-topology.py 0x<va_1162> [...]")
    mapper = _mapper()
    src = open(os.path.join(ROOT, "eldenring-deobf.bin"), "rb").read()
    dst = open(os.path.join(ROOT, "eldenring-deobf-1.17.bin"), "rb").read()
    for va in [int(v, 0) for v in sys.argv[1:]]:
        want = profile(va, 8765)
        if not want:
            print(f"{va:#x}\t-\tno 1.16.2 function")
            continue
        picked = None
        for length in (40, 32, 24, 16):
            pat, msk = mapper.build_masked_pattern(src, va - BASE, length)
            cands = candidates(mapper, dst, pat, msk)
            scored = []
            for c in cands:
                p = profile(c, 8767)
                if not p or p["entry"] != c:
                    continue
                score = (
                    (p["size"] == want["size"])
                    + (p["callees"] == want["callees"])
                    + (p["callers"] == want["callers"])
                )
                scored.append((score, c, p))
            best = [s for s in scored if s[0] == 3]
            if len(best) == 1:
                picked = (best[0], length, len(cands), "size+callees+callers")
                break
            near = [s for s in scored if s[0] == 2 and s[2]["size"] == want["size"]]
            if not best and len(near) == 1:
                picked = (near[0], length, len(cands), "WEAK: size + one arity")
                break
        if picked:
            (score, c, p), length, ncand, how = picked
            print(
                f"{va:#x}\t{c:#x}\ttopology ({how})\t"
                f"1.16.2 size={want['size']} callees={want['callees']} callers={want['callers']}"
                f" | 1.17 size={p['size']} callees={p['callees']} callers={p['callers']}"
                f" | {ncand} shape candidates at {length}B"
            )
        else:
            print(f"{va:#x}\t-\tUNRESOLVED\ttopology did not isolate one candidate")
    return 0


if __name__ == "__main__":
    sys.exit(main())
