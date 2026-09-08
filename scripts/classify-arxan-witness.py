#!/usr/bin/env python3
"""Classify a 1.16.2 witness address by how the image reaches the instruction at it.

Two ways a STRUCT-field "MOVE" can be fabricated in an ARXAN-rewritten image
---------------------------------------------------------------------------
1. ARXAN filler.  Arxan steals a function's opening bytes, parks a 5-byte `jmp` there, and
   REUSES the freed tail bytes to hold an unrelated hoisted instruction.  Those bytes are dead
   in the host's own control flow, so "which function contains this address" is answered by
   physical placement and is wrong.  Pairing two builds' trampolines then pairs two unrelated
   fillers, and any displacement difference between them is an artifact.

2. Unwind FUNCLET.  An MSVC x64 unwind funclet is called as `f(void*, void* rdx = framePtr)`.
   Inside one, `[rdx+N]` is a stack slot, not a structure field -- but `rdx` is not in the
   drift scanner's STACK_BASES ({rsp,esp,rbp,ebp}), so a frame offset is reported as a field.

Both are detectable statically:
    FUNCLET      the address (or the trampoline that jumps to it) appears as an `action` RVA in
                 some FuncInfo's unwind map -> report the owning function and the state index.
    ARXAN-jump   the .pdata extent's first instruction is a 5-byte jmp that lands outside the
                 extent, and the queried address is past it.

Usage
    python3 scripts/classify-arxan-witness.py 1162 0x142968e30 0x140533e20 ...
"""
import bisect
import struct
import sys

BASE = 0x140000000
IMAGES = {
    "1162": "eldenring-deobf.bin",
    "1170": "eldenring-deobf-1.17.bin",
}
_CACHE = {}


def load(build):
    if build in _CACHE:
        return _CACHE[build]
    import os

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    d = open(os.path.join(root, IMAGES[build]), "rb").read()
    e = struct.unpack_from("<I", d, 0x3C)[0]
    magic = struct.unpack_from("<H", d, e + 24)[0]
    dirs = e + 24 + (112 if magic == 0x20B else 96)
    prva, psz = struct.unpack_from("<II", d, dirs + 3 * 8)
    recs = []
    for o in range(prva, prva + psz, 12):
        b, en, u = struct.unpack_from("<III", d, o)
        if b or en:
            recs.append((b, en, u))
    recs.sort()
    # every FuncInfo, and the action RVAs of its unwind map
    action = {}
    fi_of = {}
    for mg in (0x19930520, 0x19930521, 0x19930522):
        needle = struct.pack("<I", mg)
        i = d.find(needle)
        while i != -1:
            if i + 40 <= len(d):
                maxState, pUnwind = struct.unpack_from("<II", d, i + 4)
                if pUnwind and 0 < maxState <= 0x4000 and pUnwind + 8 * maxState <= len(d):
                    for k in range(maxState):
                        a = struct.unpack_from("<I", d, pUnwind + 8 * k + 4)[0]
                        if a:
                            action.setdefault(a, []).append((i, k))
            i = d.find(needle, i + 1)
    for b, en, u in recs:
        flags = d[u] >> 3
        if flags & 0x4 or not (flags & 0x3):
            continue
        cnt = d[u + 2]
        tail = u + 4 + 2 * ((cnt + 1) & ~1)
        _h, fi = struct.unpack_from("<II", d, tail)
        fi_of.setdefault(fi, []).append(b)
    _CACHE[build] = (d, recs, [r[0] for r in recs], action, fi_of)
    return _CACHE[build]


def classify(build, va):
    d, recs, starts, action, fi_of = load(build)
    rva = va - BASE
    i = bisect.bisect_right(starts, rva) - 1
    out = []
    if i >= 0 and recs[i][0] <= rva < recs[i][1]:
        b, en, _u = recs[i]
        out.append(f".pdata extent {BASE+b:#x}..{BASE+en:#x} (size {en-b:#x}, +{rva-b:#x} in)")
        if d[b] == 0xE9:
            tgt = b + 5 + struct.unpack_from("<i", d, b + 1)[0]
            inside = b <= tgt < en
            out.append(
                f"ARXAN-JUMP at extent start -> {BASE+tgt:#x} "
                f"({'inside' if inside else 'OUTSIDE'} the extent)"
            )
        for a, hits in ((b, action.get(b, [])),):
            for fi, state in hits:
                owners = fi_of.get(fi, [])
                own = ", ".join(f"{BASE+o:#x}" for o in owners) or "(no .pdata owner found)"
                maxState = struct.unpack_from("<I", d, fi + 4)[0]
                out.append(
                    f"FUNCLET: unwind action state #{state} of {maxState} "
                    f"in FuncInfo {BASE+fi:#x}; owner function {own}"
                )
    else:
        out.append("no .pdata record covers it (Arxan gap)")
    return out


if __name__ == "__main__":
    build = sys.argv[1]
    for t in sys.argv[2:]:
        va = int(t, 16)
        print(f"[{build}] {va:#x}")
        for line in classify(build, va):
            print("   ", line)
