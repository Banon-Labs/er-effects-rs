#!/usr/bin/env python3
"""Answer, for one or more RVAs, whether each image's `.pdata` calls it a function start.

`er-hook` installs a detour by overwriting the first bytes of a function, and MinHook needs a
real function entry to build a trampoline from. A `.pdata` RUNTIME_FUNCTION entry whose
`begin` equals the address is the image's own statement that the address is a function start;
an address that only falls inside an entry is mid-function, and a detour there corrupts the
game. This prints both facts for both builds so a candidate row can be judged without
re-deriving the section table each time.

Usage
    python3 scripts/pdata-lookup-1162-1170.py 0x758a10 0x4f9940
    python3 scripts/pdata-lookup-1162-1170.py --new 0x7598c0        # look the RVA up in 1.17
"""

import argparse
import bisect
import os
import struct
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OLD = os.environ.get("ER_DEOBF_1162", os.path.join(REPO, "eldenring-deobf.bin"))
NEW = os.environ.get("ER_DEOBF_1170", os.path.join(REPO, "eldenring-deobf-1.17.bin"))


def sections(data):
    lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    nsec = struct.unpack_from("<H", data, lfanew + 6)[0]
    opt = struct.unpack_from("<H", data, lfanew + 20)[0]
    table = lfanew + 24 + opt
    out = []
    for i in range(nsec):
        o = table + i * 40
        name = data[o : o + 8].rstrip(b"\0").decode("ascii", "replace")
        vsz, va, _rsz, _rp = struct.unpack_from("<IIII", data, o + 8)
        out.append((name, va, vsz))
    return out


def pdata_entries(path):
    """[(begin, end, unwind)] from the image's `.pdata`, sorted by begin. Images are flat
    (file offset == RVA), so the section's VA is also where it sits in the file."""
    data = open(path, "rb").read()
    for name, va, vsz in sections(data):
        if name == ".pdata":
            break
    else:
        raise SystemExit(f"{path}: no .pdata section")
    out = []
    for off in range(va, va + vsz, 12):
        b, e, u = struct.unpack_from("<III", data, off)
        if b == 0 and e == 0:
            continue
        out.append((b, e, u))
    out.sort()
    return out, data


def section_of(data, rva):
    for name, va, vsz in sections(data):
        if va <= rva < va + vsz:
            return name
    return "<outside every section>"


def describe(entries, data, rva):
    begins = [e[0] for e in entries]
    i = bisect.bisect_right(begins, rva) - 1
    sec = section_of(data, rva)
    if i < 0:
        return f"{sec:9} no .pdata entry at or below"
    b, e, _u = entries[i]
    if b == rva:
        return f"{sec:9} FUNCTION START  [{b:#x}..{e:#x}) len={e - b:#x}"
    if rva < e:
        return f"{sec:9} MID-FUNCTION    inside [{b:#x}..{e:#x}) at +{rva - b:#x}"
    return f"{sec:9} NO ENTRY        (gap; previous ends {e:#x})"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rvas", nargs="+", help="RVAs (hex 0x... or decimal)")
    ap.add_argument("--new", action="store_true", help="the RVAs are 1.17 RVAs; only look there")
    ap.add_argument("--old", action="store_true", help="only look in 1.16.2")
    args = ap.parse_args()

    builds = []
    if not args.new:
        builds.append(("1.16.2", OLD))
    if not args.old:
        builds.append(("1.17  ", NEW))
    loaded = [(tag, *pdata_entries(p)) for tag, p in builds]

    for spec in args.rvas:
        rva = int(spec, 0)
        print(f"{rva:#x}")
        for tag, entries, data in loaded:
            print(f"   {tag}  {describe(entries, data, rva)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
