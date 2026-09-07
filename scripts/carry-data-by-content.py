#!/usr/bin/env python3
"""Corroborate a predicted 1.16.2 -> 1.17 data address by the content it holds.

The premise of `scripts/map-data-rvas-1162-to-1170.py` is that "a global has no content: at
rest it is eight zero bytes like every other global".  That is true of a runtime global and
false of an initialised one.  A de-Arxan'd image is a file, so `.rdata` strings, `.data`
pointer tables and literal tables all carry their initial bytes, and those bytes are an
identity the address itself does not have.

Three checks, strongest first:

  bytes    the two ranges are byte-identical and that byte string occurs exactly once in each
           image.  A name that occurs once per image beats any number of agreeing displacements
           -- the same argument the vtable/RTTI path already makes.
  pointers the range is a table of image VAs.  Map each entry through the established function
           map (or, for a data pointer, through the data map).  A table of N pointers that all
           land on their own mapped counterparts is not a coincidence at any N above about 3.
  literals the range is neither, but the two ranges are byte-identical -- weaker, because a run
           of small integers repeats, so it is reported with its occurrence count.

This does not propose an address.  It takes one -- from the bracket, from the vote, from
--shape-search -- and tries to falsify it.  A candidate that fails here is not carried.
"""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000
IMAGE_HI = 0x150000000


def load_fmap(path: Path) -> dict[int, int]:
    out: dict[int, int] = {}
    for line in path.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        a, b = line.split("\t")[:2]
        out[int(a, 16)] = int(b, 16)
    return out


def occurrences(buf: bytes, pat: bytes, cap: int = 8) -> list[int]:
    out, i = [], buf.find(pat)
    while i >= 0 and len(out) < cap:
        out.append(i)
        i = buf.find(pat, i + 1)
    return out


def classify(old: bytes, new: bytes, o: int, n: int, span: int, fmap: dict[int, int]) -> None:
    a, b = old[o : o + span], new[n : n + span]
    print(f"  span {span:#x}: bytes {'IDENTICAL' if a == b else 'differ'}")
    if a == b and a.strip(b"\0"):
        pat = a.rstrip(b"\0")
        if len(pat) >= 6:
            oa, ob = occurrences(old, pat), occurrences(new, pat)
            verdict = "UNIQUE in both" if len(oa) == 1 == len(ob) else f"{len(oa)}/{len(ob)} occurrences"
            hit = (oa[:1] == [o]) and (ob[:1] == [n])
            print(f"    content {verdict}; at predicted addresses: {hit}")
    # pointer-table view
    ptrs_a = [struct.unpack_from("<Q", old, o + i * 8)[0] for i in range(span // 8)]
    ptrs_b = [struct.unpack_from("<Q", new, n + i * 8)[0] for i in range(span // 8)]
    inimg = sum(1 for p in ptrs_a if BASE <= p < IMAGE_HI)
    if inimg < 2:
        return
    print(f"    pointer table: {inimg}/{len(ptrs_a)} entries are image VAs")
    ok = bad = unk = 0
    for i, (pa, pb) in enumerate(zip(ptrs_a, ptrs_b)):
        if not (BASE <= pa < IMAGE_HI):
            continue
        want = fmap.get(pa - BASE)
        if want is None:
            unk += 1
            mark = "unmapped"
            note = ""
        elif want + BASE == pb:
            ok += 1
            mark = "OK"
            note = ""
        else:
            bad += 1
            mark = "MISMATCH"
            note = f" (map says {want + BASE:#x})"
        if i < 16:
            print(f"      [{i:2d}] {pa:#x} -> {pb:#x}  {mark}{note}")
    print(f"    entries: {ok} map correctly, {bad} mismatch, {unk} source not in function map")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("old_rva")
    ap.add_argument("new_rva")
    ap.add_argument("--span", default="0x40")
    ap.add_argument("--map", default="docs/recon/rva-map-1162-to-1170.functions.tsv")
    args = ap.parse_args()
    old = (ROOT / "eldenring-deobf.bin").read_bytes()
    new = (ROOT / "eldenring-deobf-1.17.bin").read_bytes()
    fmap = load_fmap(ROOT / args.map)
    o, n = int(args.old_rva, 16), int(args.new_rva, 16)
    print(f"{o:#x} -> {n:#x}   delta {n - o:+#x}")
    classify(old, new, o, n, int(args.span, 0), fmap)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
