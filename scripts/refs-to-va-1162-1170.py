#!/usr/bin/env python3
"""Enumerate every reference to one or more VAs in a de-Arxan'd ELDEN RING image.

Why this exists
---------------
`map-rvas-1162-to-1170.py` answers "where does this byte shape re-occur". When a function has a
byte-identical TWIN -- and this image has several; MSVC emits duplicate bodies that COMDAT folding
did not merge -- that question has two answers and no way to choose between them. Byte evidence is
structurally incapable of separating twins: "the body is byte-identical" is the premise, not the
discriminator.

References can separate them, because the twins are called from different places. This prints, for
each target VA, every direct `E8`/`E9` rel32 branch that lands on it and every 8-aligned absolute
qword in the image that holds it (vtable slot, dispatch table, relocated pointer). Compare the
counts and the sites for two twins in one build, then the same for their two candidates in the
other build, and the pairing usually falls out of the asymmetry.

The rel32 scan deliberately decodes at every byte offset rather than only at instruction starts.
That over-counts: a `0xE8` byte inside an immediate or a displacement is decoded as if it were a
call. The over-counting is uniform across candidates, so a comparison stays fair, and a genuine
function entry attracts real hits that dwarf the noise. Do not read a raw count as "this function
has N callers" -- read it as "candidate A has N and candidate B has M".

Byte offsets, not instruction indices: every site is reported as an absolute VA, so evidence stays
usable when an inserted instruction shifts everything after it.

Usage
    python3 scripts/refs-to-va-1162-1170.py 1162 0x140d10370 0x140d103d0
    python3 scripts/refs-to-va-1162-1170.py 1170 0x140d11a40 0x140d11aa0
    python3 scripts/refs-to-va-1162-1170.py both 0x140d103d0
"""

import argparse
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}


def sections(data):
    lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    nsec = struct.unpack_from("<H", data, lfanew + 6)[0]
    opt = struct.unpack_from("<H", data, lfanew + 20)[0]
    table = lfanew + 24 + opt
    out = []
    for index in range(nsec):
        entry = table + index * 40
        name = data[entry : entry + 8].rstrip(b"\0").decode("ascii", "replace")
        vsize, va, _rsize, _rptr = struct.unpack_from("<IIII", data, entry + 8)
        out.append((name, va, vsize))
    return out


def section_of(secs, rva):
    for name, va, vsize in secs:
        if va <= rva < va + vsize:
            return name
    return "?"


def scan(path, targets):
    data = open(path, "rb").read()
    secs = sections(data)
    found = {t: {"call": [], "jmp": [], "ptr": []} for t in targets}
    wanted = set(targets)
    limit = len(data) - 8
    for offset in range(limit):
        opcode = data[offset]
        if opcode == 0xE8 or opcode == 0xE9:
            rel = struct.unpack_from("<i", data, offset + 1)[0]
            target = BASE + offset + 5 + rel
            if target in wanted:
                found[target]["call" if opcode == 0xE8 else "jmp"].append(BASE + offset)
    for offset in range(0, limit, 8):
        value = struct.unpack_from("<Q", data, offset)[0]
        if value in wanted:
            found[value]["ptr"].append(BASE + offset)
    return found, secs


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("image", choices=["1162", "1170", "both"])
    parser.add_argument("vas", nargs="+")
    parser.add_argument("--max-sites", type=int, default=40)
    args = parser.parse_args(argv)

    targets = [int(v, 0) for v in args.vas]
    builds = ["1162", "1170"] if args.image == "both" else [args.image]
    for build in builds:
        found, secs = scan(IMAGES[build], targets)
        for target in targets:
            entry = found[target]
            print(
                f"{build} {target:#x}: calls={len(entry['call'])} "
                f"jmps={len(entry['jmp'])} qword-ptrs={len(entry['ptr'])}"
            )
            for kind in ("call", "jmp", "ptr"):
                sites = entry[kind]
                if not sites:
                    continue
                shown = sites[: args.max_sites]
                rendered = ", ".join(
                    f"{s:#x}[{section_of(secs, s - BASE)}]" for s in shown
                )
                more = "" if len(sites) == len(shown) else f", ...+{len(sites) - len(shown)}"
                print(f"    {kind}: {rendered}{more}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
