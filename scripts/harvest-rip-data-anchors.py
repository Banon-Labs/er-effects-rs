#!/usr/bin/env python3
"""Harvest the 1.16.2 -> 1.17 data anchor map source-first, aligning references by byte offset.

Two things this does differently from `map-data-rvas-1162-to-1170.py`
---------------------------------------------------------------------
1. It is source-driven, not target-driven.  That tool takes an address and hunts the image for
   references to it; this one walks the 128k established function pairs once and emits every
   data reference it finds on the way.  One pass yields the whole anchor field instead of one
   answer per invocation, and the anchor field is what makes a proposed row auditable: you can
   see where a delta region begins and ends rather than trusting that it is constant.

2. It pairs a reference by its byte offset inside the function, not by instruction index.
   Index alignment is exact while the two bodies decode to the same stream, and breaks the
   moment one instruction is inserted ahead of the reference -- the index then names a
   different instruction and the reference is lost.  Measured: `MENU_PUMP_KICK_PTR_RVA` and
   `PROFILE_OFFSCREEN_SIZE_TABLE_RVA` were both in that ledger's unused list, and both have a
   perfectly good reference sitting at the same byte offset in both builds.

What makes a vote
-----------------
The instruction found at the same byte offset must decode from the function's own entry, carry
the same mnemonic and the same operand shape, and be rip-relative.  Then its 1.17 displacement
is read and the pair is one vote.  Votes are tallied per source address and a source with
disagreeing votes is reported contested rather than resolved by majority -- the .data delta is
not constant and not monotonic, so a majority is not evidence about an individual address.

This proposes rows.  It does not write `docs/recon/*.tsv`.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import struct
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000
MAX_FUNCTION_BYTES = 0x4000


def load(mod: str):
    spec = importlib.util.spec_from_file_location("_" + mod, ROOT / "scripts" / f"{mod}.py")
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def sections(data: bytes) -> list[tuple[str, int, int]]:
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    nsec = struct.unpack_from("<H", data, pe + 6)[0]
    optsz = struct.unpack_from("<H", data, pe + 20)[0]
    off = pe + 24 + optsz
    out = []
    for i in range(nsec):
        e = data[off + i * 40 : off + (i + 1) * 40]
        vsz, va, rsz, _ = struct.unpack_from("<IIII", e, 8)
        out.append((e[:8].rstrip(b"\0").decode("latin1"), va, max(vsz, rsz)))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--map", default="docs/recon/rva-map-1162-to-1170.functions.tsv")
    ap.add_argument("--out", required=True)
    ap.add_argument("--limit", type=int, default=0)
    args = ap.parse_args()

    import capstone

    dsfd = load("detect-struct-field-drift")
    split_memory = dsfd.split_memory
    leaf_extent = dsfd._sibling_leaf_extent()

    old = (ROOT / "eldenring-deobf.bin").read_bytes()
    new = (ROOT / "eldenring-deobf-1.17.bin").read_bytes()
    osec, nsec = sections(old), sections(new)

    def secof(secs, rva):
        for nm, va, sz in secs:
            if va <= rva < va + sz:
                return nm
        return "?"

    # data starts after .text; anything below is code and is the function map's business.
    OLD_DATA_LO = min(va for nm, va, _ in osec if nm in (".rdata", ".data"))
    NEW_DATA_LO = min(va for nm, va, _ in nsec if nm in (".rdata", ".data"))

    def ends_of(data):
        out = {}
        for nm, va, sz in sections(data):
            if nm != ".pdata":
                continue
            for off in range(va, va + sz, 12):
                b0, b1, _ = struct.unpack_from("<III", data, off)
                if b0 and b1 > b0 and b1 - b0 <= 0x20000:
                    out.setdefault(b0, b1)
        return out

    oe, ne = ends_of(old), ends_of(new)
    os_, ns_ = set(oe), set(ne)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)

    def refs(img, fn, end):
        out = {}
        for a, s, m, o in md.disasm_lite(img[fn:end], BASE + fn):
            if "[" not in o:
                continue
            sh, mem = split_memory(o)
            for b, d in mem:
                if b == "rip":
                    out[a - BASE - fn] = (m, sh, (a - BASE) + s + d)
        return out

    votes: dict[int, Counter] = defaultdict(Counter)
    wit: dict[tuple[int, int], list[int]] = defaultdict(list)
    stats = Counter()
    for line in (ROOT / args.map).read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        a_rva, b_rva = (int(x, 16) for x in line.split("\t")[:2])
        stats["pairs"] += 1
        if args.limit and stats["pairs"] > args.limit:
            break
        a_end = dsfd.extent_of(a_rva, oe, old, os_, leaf_extent)
        b_end = dsfd.extent_of(b_rva, ne, new, ns_, leaf_extent)
        if a_end is None or b_end is None or a_end - a_rva > MAX_FUNCTION_BYTES or b_end - b_rva > MAX_FUNCTION_BYTES:
            stats["skipped"] += 1
            continue
        ra = refs(old, a_rva, a_end)
        if not ra:
            continue
        rb = refs(new, b_rva, b_end)
        for off, (m, sh, ta) in ra.items():
            if ta < OLD_DATA_LO:
                continue  # a .text target; the function map owns those
            hit = rb.get(off)
            if hit is None:
                stats["no-offset"] += 1
                continue
            mb, shb, tb = hit
            if mb != m or shb != sh:
                stats["shape-mismatch"] += 1
                continue
            if tb < NEW_DATA_LO:
                stats["target-not-data"] += 1
                continue
            votes[ta][tb] += 1
            if len(wit[(ta, tb)]) < 8:
                wit[(ta, tb)].append(a_rva)
            stats["votes"] += 1

    unan = [k for k, c in votes.items() if len(c) == 1]
    cont = [k for k, c in votes.items() if len(c) > 1]
    print(f"pairs                 {stats['pairs']}")
    print(f"votes cast            {stats['votes']}")
    print(f"distinct 1.16.2 data  {len(votes)}")
    print(f"  unanimous           {len(unan)}")
    print(f"  contested           {len(cont)}")
    print(f"dropped: no instruction at that offset {stats['no-offset']}, shape mismatch {stats['shape-mismatch']}")

    payload = {
        "stats": dict(stats),
        "anchors": [
            {
                "old": k,
                "old_section": secof(osec, k),
                "candidates": [
                    {"new": n, "new_section": secof(nsec, n), "votes": v, "delta": n - k, "witnesses": wit[(k, n)]}
                    for n, v in votes[k].most_common()
                ],
            }
            for k in sorted(votes)
        ],
    }
    Path(args.out).write_text(json.dumps(payload))
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
