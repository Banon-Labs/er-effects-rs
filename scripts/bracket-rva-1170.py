#!/usr/bin/env python3
"""Bracket an unmapped 1.16.2 RVA between its nearest mapped neighbours in the signature map.

`map-rvas-1162-to-1170.py` answers "where does this signature re-occur"; when the body changed
enough that no signature is unique, it says nothing at all. But 1.17 moved code in RUNS: a
contiguous stretch of functions shifts by one delta, because the insertion that moved them sits
before the whole run. So a target with mapped neighbours on both sides at the same delta is
bracketed: the only way for the target to sit elsewhere is for the patch to have moved it out of
its own neighbourhood and moved something else in, which the .pdata ordering would show.

A bracket is a candidate like any other -- it must still be byte-verified. What it buys is a
single address to verify instead of nine shape matches to choose between.

Usage
    python3 scripts/bracket-rva-1170.py 0x14067a1c0 [0x...]
    python3 scripts/bracket-rva-1170.py --neighbours 6 0x14067a1c0
"""

import argparse
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAP_TSV = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.functions.tsv")
BASE = 0x140000000


def load_map(path):
    rows = []
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            if line.startswith("#"):
                continue
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 2:
                continue
            rows.append((int(parts[0], 16), int(parts[1], 16)))
    rows.sort()
    return rows


def bracket(rows, target_rva, neighbours):
    lower = [r for r in rows if r[0] < target_rva][-neighbours:]
    upper = [r for r in rows if r[0] > target_rva][:neighbours]
    return lower, upper


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("addresses", nargs="+")
    parser.add_argument("--neighbours", type=int, default=4)
    parser.add_argument("--map", default=MAP_TSV)
    args = parser.parse_args(argv)

    rows = load_map(args.map)
    for text in args.addresses:
        value = int(text, 16)
        rva = value - BASE if value >= BASE else value
        lower, upper = bracket(rows, rva, args.neighbours)
        print(f"== 0x{BASE + rva:x} (rva 0x{rva:x})")
        for src, dst in lower:
            print(f"   below  0x{BASE+src:x} -> 0x{BASE+dst:x}   delta {dst-src:+#x}")
        deltas = {d - s for s, d in lower[-1:] + upper[:1]}
        verdict = (
            f"BRACKETED at delta {list(deltas)[0]:+#x} -> candidate 0x{BASE + rva + list(deltas)[0]:x}"
            if len(deltas) == 1 and lower and upper
            else "NOT BRACKETED (neighbour deltas disagree)"
        )
        print(f"   >>> 0x{BASE+rva:x}   {verdict}")
        for src, dst in upper:
            print(f"   above  0x{BASE+src:x} -> 0x{BASE+dst:x}   delta {dst-src:+#x}")
        print()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
