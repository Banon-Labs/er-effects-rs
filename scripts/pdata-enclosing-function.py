#!/usr/bin/env python3
"""Ask each ELDEN RING image's own .pdata where the function containing an address begins.

Why this exists: `verify-rva-map-1170.py` reports a pair's entry evidence as a single word
(both-entries / neither-entry) and stops there. When a pair comes back neither-entry the next
question is always the same -- is the real entry a few bytes earlier, is the address in a
.pdata gap (a hand-written thunk the linker emitted no unwind record for), or did the two
builds disagree about where the function begins? That is what this prints.

Usage
    python3 scripts/pdata-enclosing-function.py 1162:0x141158940 1170:0x14115a740
    python3 scripts/pdata-enclosing-function.py --context 6 1170:0x1408d32b0
"""
import argparse
import bisect
import os
import struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}
_CACHE = {}


def records(build):
    """Sorted (begin, end, unwind) RVA triples from the image's exception directory."""
    if build in _CACHE:
        return _CACHE[build]
    image = open(IMAGES[build], "rb").read()
    e_lfanew = struct.unpack_from("<I", image, 0x3C)[0]
    magic = struct.unpack_from("<H", image, e_lfanew + 24)[0]
    directories = e_lfanew + 24 + (112 if magic == 0x20B else 96)
    table_rva, table_size = struct.unpack_from("<II", image, directories + 3 * 8)
    out = []
    for offset in range(table_rva, table_rva + table_size, 12):
        begin, end, unwind = struct.unpack_from("<III", image, offset)
        if begin or end:
            out.append((begin, end, unwind))
    out.sort()
    _CACHE[build] = out
    return out


def describe(build, va, context=3):
    table = records(build)
    begins = [b for b, _, _ in table]
    rva = va - BASE
    index = bisect.bisect_right(begins, rva) - 1
    print(f"[{build}] {va:#x}  (rva {rva:#x})")
    if index < 0:
        print("    before the first .pdata record")
        return
    begin, end, _ = table[index]
    if rva == begin:
        verdict = "EXACT FUNCTION ENTRY"
    elif rva < end:
        verdict = f"INSIDE a function, +{rva - begin:#x} past its entry {BASE + begin:#x}"
    else:
        nxt = table[index + 1][0] if index + 1 < len(table) else None
        gap = f" (gap {BASE + end:#x} .. {BASE + nxt:#x})" if nxt else ""
        verdict = f"IN A .pdata GAP -- no record covers it{gap}"
    print(f"    {verdict}")
    lo, hi = max(0, index - context), min(len(table), index + context + 1)
    for j in range(lo, hi):
        b, e, _ = table[j]
        mark = "   <<< nearest start at or below" if j == index else ""
        print(f"      {BASE + b:#012x} .. {BASE + e:#012x}  size {e - b:#6x}{mark}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--context", type=int, default=3)
    parser.add_argument("targets", nargs="+", help="build:VA, e.g. 1170:0x14115a740")
    args = parser.parse_args()
    for target in args.targets:
        build, va = target.split(":")
        describe(build, int(va, 16), args.context)
        print()


if __name__ == "__main__":
    main()
