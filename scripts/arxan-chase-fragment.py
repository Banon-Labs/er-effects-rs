#!/usr/bin/env python3
"""Chase an Arxan-hoisted code fragment back to a function `.pdata` declares.

An Arxan-rewritten ELDEN RING image parks stolen/hoisted instructions in the dead bytes of
other functions' `.pdata` extents.  So "which function contains this address" is answered by
physical placement and is wrong.  The only sound answer is "who branches here", walked until
the walk lands inside a real `.pdata` extent.
"""
import bisect, struct, sys, os
import numpy as np

BASE = 0x140000000
ROOT = "/home/banon/projects/er-mods-rs"
IMAGES = {"1162": ROOT + "/eldenring-deobf.bin", "1170": ROOT + "/eldenring-deobf-1.17.bin"}
_C = {}


def img(build):
    if build not in _C:
        data = open(IMAGES[build], "rb").read()
        e = struct.unpack_from("<I", data, 0x3C)[0]
        magic = struct.unpack_from("<H", data, e + 24)[0]
        dirs = e + 24 + (112 if magic == 0x20B else 96)
        prva, psz = struct.unpack_from("<II", data, dirs + 3 * 8)
        recs = []
        for o in range(prva, prva + psz, 12):
            b, en, u = struct.unpack_from("<III", data, o)
            if b or en:
                recs.append((b, en, u))
        recs.sort()
        _C[build] = (data, recs, [r[0] for r in recs], np.frombuffer(data, dtype=np.uint8))
    return _C[build]


def enclosing(build, va):
    data, recs, starts, _ = img(build)
    rva = va - BASE
    i = bisect.bisect_right(starts, rva) - 1
    if i < 0:
        return None
    b, e, _ = recs[i]
    return (BASE + b, BASE + e) if b <= rva < e else None


def refs(build, target):
    data, recs, starts, a = img(build)
    n = len(data)
    t = target - BASE
    out = []
    i32 = np.lib.stride_tricks.as_strided(a, shape=(n - 3, 4), strides=(1, 1))

    def rel_at(idx):
        return i32[idx].copy().view("<i4").reshape(-1)

    for opc in (0xE8, 0xE9):
        idx = np.nonzero(a[: n - 5] == opc)[0]
        tg = idx + 5 + rel_at(idx + 1).astype(np.int64)
        for j in np.nonzero(tg == t)[0]:
            out.append((BASE + int(idx[j]), "call" if opc == 0xE8 else "jmp"))
    idx = np.nonzero((a[: n - 6] == 0x0F) & (a[1 : n - 5] >= 0x80) & (a[1 : n - 5] <= 0x8F))[0]
    tg = idx + 6 + rel_at(idx + 2).astype(np.int64)
    for j in np.nonzero(tg == t)[0]:
        out.append((BASE + int(idx[j]), "jcc"))
    idx = np.nonzero(
        ((a[: n - 8] == 0x48) | (a[: n - 8] == 0x4C))
        & (a[1 : n - 7] == 0x8D)
        & ((a[2 : n - 6] & 0xC7) == 0x05)
    )[0]
    tg = idx + 7 + rel_at(idx + 3).astype(np.int64)
    for j in np.nonzero(tg == t)[0]:
        out.append((BASE + int(idx[j]), "lea-rip"))
    b8 = struct.pack("<Q", target)
    i = data.find(b8)
    while i != -1:
        out.append((BASE + i, "qword"))
        i = data.find(b8, i + 1)
    b4 = struct.pack("<I", target - BASE)
    i = data.find(b4)
    while i != -1:
        out.append((BASE + i, "rva-dword"))
        i = data.find(b4, i + 1)
    return out


def chase(build, target, depth=0, seen=None, maxdepth=8):
    seen = seen if seen is not None else set()
    if target in seen or depth > maxdepth:
        return
    seen.add(target)
    pad = "  " * depth
    for va, kind in refs(build, target):
        enc = enclosing(build, va)
        if kind in ("qword", "rva-dword"):
            print(f"{pad}{va:#x}  {kind} -> {target:#x}   [{'in ' + hex(enc[0]) if enc else 'no .pdata'}]")
            continue
        where = f"INSIDE {enc[0]:#x}..{enc[1]:#x} (+{va - enc[0]:#x})" if enc else "GAP (arxan)"
        print(f"{pad}{va:#x}  {kind} -> {target:#x}   {where}")
        if enc is None:
            chase(build, va, depth + 1, seen, maxdepth)


def refs_into(build, lo, hi):
    """Every branch/pointer landing anywhere in [lo, hi)."""
    data, recs, starts, a = img(build)
    n = len(data)
    out = []
    i32 = np.lib.stride_tricks.as_strided(a, shape=(n - 3, 4), strides=(1, 1))

    def rel_at(idx):
        return i32[idx].copy().view("<i4").reshape(-1)

    def add(idx, tg, kind):
        m = np.nonzero((tg >= lo - BASE) & (tg < hi - BASE))[0]
        for j in m:
            out.append((BASE + int(idx[j]), BASE + int(tg[j]), kind))

    for opc, kind in ((0xE8, "call"), (0xE9, "jmp")):
        idx = np.nonzero(a[: n - 5] == opc)[0]
        add(idx, idx + 5 + rel_at(idx + 1).astype(np.int64), kind)
    idx = np.nonzero((a[: n - 6] == 0x0F) & (a[1 : n - 5] >= 0x80) & (a[1 : n - 5] <= 0x8F))[0]
    add(idx, idx + 6 + rel_at(idx + 2).astype(np.int64), "jcc")
    idx = np.nonzero(
        ((a[: n - 8] == 0x48) | (a[: n - 8] == 0x4C))
        & (a[1 : n - 7] == 0x8D)
        & ((a[2 : n - 6] & 0xC7) == 0x05)
    )[0]
    add(idx, idx + 7 + rel_at(idx + 3).astype(np.int64), "lea-rip")
    idx = np.nonzero(a[: n - 2] == 0xEB)[0]
    add(idx, idx + 2 + a[idx + 1].astype(np.int8).astype(np.int64), "jmp8")
    q = np.frombuffer(data[: (n // 8) * 8], dtype="<u8")
    m = np.nonzero((q >= lo) & (q < hi))[0]
    for j in m:
        out.append((BASE + int(j) * 8, int(q[j]), "qword"))
    return sorted(out)


if __name__ == "__main__":
    build = sys.argv[1]
    if sys.argv[2] == "--window":
        lo, hi = int(sys.argv[3], 16), int(sys.argv[4], 16)
        for va, tg, kind in refs_into(build, lo, hi):
            enc = enclosing(build, va)
            where = f"INSIDE {enc[0]:#x}..{enc[1]:#x}" if enc else "GAP (arxan)"
            print(f"  {va:#x} {kind:8s} -> {tg:#x}   {where}")
        raise SystemExit
    for t in sys.argv[2:]:
        tv = int(t, 16)
        print(f"=== [{build}] chasing {tv:#x}")
        chase(build, tv)
