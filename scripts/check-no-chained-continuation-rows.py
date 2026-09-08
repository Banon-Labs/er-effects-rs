#!/usr/bin/env python3
"""Refuse a map row whose address is an MSVC chained-unwind continuation record.

What this catches that `classify-1170-entry-kind.py` does not
------------------------------------------------------------
That tool asks: does this image's `.pdata` declare a function to begin here? It is the right
question, and it caught six mid-function rows on 2026-08-30. But `.pdata` answers "yes" for an
address that is not a function start.

MSVC splits one function's unwind data across several `RUNTIME_FUNCTION` records whenever it
outlines a cold path or a region-based unwind. Every chunk after the first gets its own record,
so it reads as `ENTRY` -- while being an address in the middle of a live function. Bit 2 of the
record's `UNWIND_INFO` flags nibble (`UNW_FLAG_CHAININFO`, 0x4) says which kind it is, and the
record it chains to owns the real prologue.

Measured, and this is why the distinction is not academic. `0xc57666` is the record covering the
`CSFreeListMemorySystem` shutdown assert. Both images' `.pdata` declare a function start there,
`classify-1170-entry-kind.py --fail-on-mid` passes it, and it is already paired in
`rva-map-1162-to-1170.functions.tsv` as `0xc57666 -> 0xc58d36`. It is a continuation: the real
function begins 0x86 bytes earlier, at `0xc575e0` (1.16.2) / `0xc58cb0` (1.17), and is vtable
slot 2 of `.?AVCSFreeListMemorySystem@CS@@` in both. A row for `0xc57666` would carry
`BOTH-ENTRIES`, clear every existing gate, and license MinHook to write five bytes 0x86 into a
function other threads are running.

The inverse case is why this cannot simply refuse short records. `0x8c47c0`
(`CS::FeSystemAnnounceView::Update`) has a six-byte `.pdata` record -- and it is a ROOT: the
6 bytes are `push rbx; sub rsp,0x30`, one more than MinHook needs, and the chained record at
`0x8c47c6` points back at it. Record size says nothing; the flag says everything.

Usage
    python3 scripts/check-no-chained-continuation-rows.py            # gate over the tables build.rs reads
    python3 scripts/check-no-chained-continuation-rows.py --rows     # list every flagged row
    python3 scripts/check-no-chained-continuation-rows.py --selftest
"""

import argparse
import bisect
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OLD_IMAGE = os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin"))
NEW_IMAGE = os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin"))
BASE = 0x140000000
UNW_FLAG_CHAININFO = 0x4

# Same three tables `classify-1170-entry-kind.py` gates, and for the same reason: every one of
# them is read by `er-game-base/build.rs` to license a call or a detour.
GATED_MAPS = (
    "docs/recon/rva-map-1162-to-1170.verified.tsv",
    "docs/recon/rva-map-1162-to-1170.needed-verified.tsv",
    "docs/recon/rva-map-1162-to-1170.needed.tsv",
)


class Unwind:
    """One image's `.pdata`, with the chain each record belongs to resolvable."""

    def __init__(self, path):
        with open(path, "rb") as handle:
            self.image = handle.read()
        self.table = self._records()
        self.starts = [begin for begin, _end, _unwind in self.table]

    def _records(self):
        e_lfanew = struct.unpack_from("<I", self.image, 0x3C)[0]
        magic = struct.unpack_from("<H", self.image, e_lfanew + 24)[0]
        directories = e_lfanew + 24 + (112 if magic == 0x20B else 96)
        table_rva, table_size = struct.unpack_from("<II", self.image, directories + 3 * 8)
        out = []
        for offset in range(table_rva, table_rva + table_size, 12):
            begin, end, unwind = struct.unpack_from("<III", self.image, offset)
            if begin or end:
                out.append((begin, end, unwind))
        out.sort()
        return out

    def root(self, rva, depth=0):
        """`(real_entry_rva, chain_hops)`; `(None, 0)` when no record covers `rva`."""
        index = bisect.bisect_right(self.starts, rva) - 1
        if index < 0:
            return None, depth
        begin, end, unwind = self.table[index]
        if not begin <= rva < end:
            return None, depth
        if not (self.image[unwind] >> 3) & UNW_FLAG_CHAININFO:
            return begin, depth
        count = self.image[unwind + 2]
        # The chained RUNTIME_FUNCTION follows the unwind-code array, which is padded to an even
        # number of 2-byte slots.
        tail = unwind + 4 + 2 * ((count + 1) & ~1)
        parent = struct.unpack_from("<I", self.image, tail)[0]
        return self.root(parent, depth + 1)


def rows(path):
    try:
        text = open(path, encoding="utf-8", errors="replace").read()
    except OSError:
        return
    for line in text.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        try:
            old, new = int(fields[0], 16), int(fields[1], 16)
        except ValueError:
            continue
        yield (old - BASE if old >= BASE else old, new - BASE if new >= BASE else new)


def selftest(old, new):
    """The two cases that define the rule, one on each side of it."""
    failures = 0
    entry, hops = old.root(0x8C47C0)
    if (entry, hops) != (0x8C47C0, 0):
        print(f"selftest: 0x8c47c0 should be a ROOT, got {entry and hex(entry)} hops={hops}")
        failures += 1
    entry, hops = old.root(0xC57666)
    if (entry, hops) != (0xC575E0, 1):
        print(f"selftest: 0xc57666 should chain to 0xc575e0, got {entry and hex(entry)}")
        failures += 1
    entry, hops = new.root(0xC58D36)
    if (entry, hops) != (0xC58CB0, 1):
        print(f"selftest: 0xc58d36 should chain to 0xc58cb0, got {entry and hex(entry)}")
        failures += 1
    print(f"selftest: {failures} failure(s)")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rows", action="store_true", help="list every flagged row")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    # The two de-Arxan'd images are gitignored (game-derived bytes are never committed), so a fresh
    # checkout and CI simply do not have them. Skip at exit 0 -- loudly, naming the file -- the way
    # `classify-1170-entry-kind.py`, `verify-data-rvas-by-rtti.py` and
    # `check-singleton-field-offsets.py` already do. Without this the missing image surfaced as a
    # raw FileNotFoundError traceback at a nonzero exit, which is indistinguishable from the gate
    # having run and found a chained-continuation row -- a gate that cannot run must not read like
    # a gate that failed, and must not read like one that passed either.
    #
    # The guard sits above the `--selftest` branch on purpose: both paths construct `Unwind` from
    # these images, and the selftest's three cases are pinned to real addresses in them, so there is
    # nothing it can prove without the bytes.
    missing = [path for path in (OLD_IMAGE, NEW_IMAGE) if not os.path.isfile(path)]
    if missing:
        for path in missing:
            print(f"skipped: missing image {path}")
        print(
            "  NOT A PASS: no row was checked for chained-continuation records. The two de-Arxan'd "
            "images are gitignored; run this on a machine that has them "
            "(scripts/dearxan-deobfuscate.rs regenerates them)."
        )
        return 0
    old, new = Unwind(OLD_IMAGE), Unwind(NEW_IMAGE)
    if args.selftest:
        return 1 if selftest(old, new) else 0
    flagged = 0
    for name in GATED_MAPS:
        path = os.path.join(ROOT, name)
        hits = []
        count = 0
        for source, destination in rows(path):
            count += 1
            source_root, source_hops = old.root(source)
            dest_root, dest_hops = new.root(destination)
            if source_hops or dest_hops:
                hits.append((source, destination, source_root, source_hops, dest_root, dest_hops))
        flagged += len(hits)
        print(f"{name}: {count} rows -- {len(hits)} CHAINED-CONTINUATION")
        if args.rows or hits:
            for source, destination, sr, sh, dr, dh in hits:
                print(
                    f"    {source:#x} -> {destination:#x}"
                    f"   1.16.2 hops={sh} real entry {sr and hex(sr)}"
                    f" | 1.17 hops={dh} real entry {dr and hex(dr)}"
                )
    if flagged:
        print(
            f"\n{flagged} row(s) name a continuation record, not a function entry. "
            "Point the constant at the real entry and add the offset in Rust."
        )
    return 1 if flagged else 0


if __name__ == "__main__":
    raise SystemExit(main())
