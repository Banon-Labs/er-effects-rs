#!/usr/bin/env python3
"""Tell a real function entry from an MSVC chained-unwind continuation record.

Why `.pdata` alone MISLEADS here
--------------------------------
`scripts/classify-1170-entry-kind.py` answers "does this image's `.pdata` declare a function to
begin at this address", and that is the right question for most addresses. But MSVC splits one
function's unwind data across several `RUNTIME_FUNCTION` records whenever the compiler moves a
chunk of it (cold-path outlining, `/OPT:ICF` runs, region-based unwind). Every chunk after the
first gets its own record, so it classifies as `ENTRY` -- while being, in the machine's terms, an
address in the middle of a live function.

The discriminator is in the UNWIND_INFO the record points at: bit 2 of its flags nibble
(`UNW_FLAG_CHAININFO`, 0x4) means "this record is a continuation; the record I chain to owns the
real prologue". Following that chain to a record without the flag gives the function's actual
entry.

This distinction decides whether an address may carry a detour. `0x8c47c0` chains to nothing --
it is a genuine entry whose first chunk is only six bytes, which is why the whole-image signature
matcher skipped it. `0xc57666` does carry the flag, so a row for it would license MinHook to
overwrite five bytes inside a function that starts 0x86 bytes earlier.

Usage
    python3 scripts/pdata-chain-root-1170.py 1162:0x140c57666 1170:0x140c58d36
"""

import bisect
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}
BASE = 0x140000000
UNW_FLAG_CHAININFO = 0x4


def records(image):
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
    return out


def chain_root(image, table, starts, rva, depth=0):
    """`(entry_rva, hops, chained)` for the record covering `rva`."""
    index = bisect.bisect_right(starts, rva) - 1
    if index < 0:
        return None, depth, False
    begin, end, unwind = table[index]
    if not (begin <= rva < end):
        return None, depth, False
    flags = image[unwind] >> 3
    if not flags & UNW_FLAG_CHAININFO:
        return begin, depth, depth > 0
    # The chained RUNTIME_FUNCTION follows the (padded) unwind codes array.
    count = image[unwind + 2]
    tail = unwind + 4 + 2 * ((count + 1) & ~1)
    parent_begin = struct.unpack_from("<I", image, tail)[0]
    return chain_root(image, table, starts, parent_begin, depth + 1)


def describe(build, va):
    with open(IMAGES[build], "rb") as handle:
        image = handle.read()
    table = records(image)
    starts = [begin for begin, _end, _unwind in table]
    rva = va - BASE if va >= BASE else va
    root, hops, chained = chain_root(image, table, starts, rva)
    if root is None:
        print(f"[{build}] {BASE + rva:#x}: no .pdata record covers it (leaf or data)")
        return
    index = bisect.bisect_right(starts, rva) - 1
    begin, end, _ = table[index]
    kind = (
        f"CHAINED continuation ({hops} hop(s)); real entry {BASE + root:#x}"
        if chained
        else "ROOT record -- a genuine function entry"
    )
    print(
        f"[{build}] {BASE + rva:#x}: record {BASE + begin:#x}..{BASE + end:#x}"
        f" size {end - begin:#x} -- {kind}"
    )


def main(argv):
    if not argv:
        sys.exit(__doc__)
    for target in argv:
        build, va = target.split(":")
        describe(build, int(va, 16))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
