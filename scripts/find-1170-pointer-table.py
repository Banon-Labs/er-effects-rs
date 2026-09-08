#!/usr/bin/env python3
"""Carry a 1.16.2 table/slot of absolute pointers onto 1.17 by what it points at.

Why this exists
---------------
`map-data-rvas-1162-to-1170.py` carries a global by the code that references it, and that
fails when the only reference lives in a function 1.17 edited (the instruction is no longer
at the same index) or when the datum is reached solely through a runtime pointer and has no
rip-relative reference at all. Two addresses in the 2026-08-30 batch failed exactly that way:
`MENU_PUMP_KICK_PTR_RVA` (0x3b37c98) and `MenuTraceRva::TaskUpdateTable` (0x2ac72a0).

But a slot full of absolute pointers has content after all -- not its own bytes, which are
relocated and therefore differ between builds, but its targets. Map each target through the
function map and you have a 1.17 fingerprint that is unique to that table.

How
---
Read N qwords at the 1.16.2 RVA. Each one that is `0x140000000 + rva` and pairs in
`docs/recon/rva-map-1162-to-1170.functions.tsv` becomes a required 1.17 value; anything else
(a data pointer, an integer, a null) becomes a wildcard. Then scan the 1.17 image for the
first qword-aligned run that satisfies every required slot. A single hit is the answer; the
scan reports the count so several hits are visibly not an answer.

Usage
    python3 scripts/find-1170-pointer-table.py 0x3b37c98 --slots 2
    python3 scripts/find-1170-pointer-table.py 0x2ac72a0 --slots 8 --before 4
"""

import argparse
import os
import struct
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OLD = os.environ.get("ER_DEOBF_1162", os.path.join(REPO, "eldenring-deobf.bin"))
NEW = os.environ.get("ER_DEOBF_1170", os.path.join(REPO, "eldenring-deobf-1.17.bin"))
FUNCMAP = os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.functions.tsv")
IMAGE_BASE = 0x1_4000_0000


def load_function_map():
    pairs = {}
    with open(FUNCMAP, encoding="utf-8") as fh:
        for line in fh:
            if line.startswith("#"):
                continue
            parts = line.split()
            if len(parts) < 2:
                continue
            pairs[int(parts[0], 16)] = int(parts[1], 16)
    return pairs


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rva", help="1.16.2 RVA of the slot/table (hex 0x... or decimal)")
    ap.add_argument("--slots", type=int, default=4, help="qwords to fingerprint from the RVA on")
    ap.add_argument("--before", type=int, default=0, help="also fingerprint this many qwords BEFORE it")
    args = ap.parse_args()

    rva = int(args.rva, 0)
    start = rva - args.before * 8
    count = args.before + args.slots

    old = open(OLD, "rb").read()
    new = open(NEW, "rb").read()
    fmap = load_function_map()

    want = []
    print(f"1.16.2 {rva:#x}  fingerprint of {count} qword(s) from {start:#x}")
    for i in range(count):
        off = start + i * 8
        (value,) = struct.unpack_from("<Q", old, off)
        target = value - IMAGE_BASE
        mapped = fmap.get(target)
        if mapped is None:
            want.append(None)
            print(f"   +{i * 8:#04x} {value:#x}  WILDCARD (not a mapped function)")
        else:
            want.append(mapped + IMAGE_BASE)
            print(f"   +{i * 8:#04x} {value:#x} -> {mapped + IMAGE_BASE:#x}  (fn map, {mapped - target:+#x})")

    required = [(i, v) for i, v in enumerate(want) if v is not None]
    if not required:
        print("no mapped pointer in the window -- nothing to search on")
        return 1
    print(f"\nsearching 1.17 for {len(required)} required slot(s)...")

    # Anchor on the first required slot, then confirm the rest at their fixed strides.
    anchor_index, anchor_value = required[0]
    anchor_bytes = struct.pack("<Q", anchor_value)
    hits = []
    pos = 0
    while True:
        pos = new.find(anchor_bytes, pos)
        if pos < 0:
            break
        if pos % 8 == 0:
            table = pos - anchor_index * 8
            if table >= 0 and all(
                struct.unpack_from("<Q", new, table + i * 8)[0] == v for i, v in required
            ):
                hits.append(table + args.before * 8)
        pos += 1

    for hit in hits:
        print(f"   1.17 {hit:#x}   delta {hit - rva:+#x}")
    print(f"hits={len(hits)}" + ("  <- unique" if len(hits) == 1 else "  <- NOT unique, do not use" if hits else "  <- none"))
    return 0 if len(hits) == 1 else 1


if __name__ == "__main__":
    sys.exit(main())
