#!/usr/bin/env python3
"""Recover MSVC RTTI class names -> vtable addresses from a flat ELDEN RING image.

WHY THIS EXISTS. The Ghidra dump served on :8767 is the INSTALLED build (1.17) and has zero class
names -- `searchFunctionsByName` answers 0 for GridControl, MenuWindow and CSMenuMan alike -- so
every name-driven lookup has had to detour through the 1.16.2 dump on :8765 and then translate the
address back. That detour is the single reason anything in this repo still references 1.16.2, and it
is avoidable: the RTTI is PRESENT in the 1.17 image (`.?AVGridControl@CS@@` sits at 0x143c93728),
it was simply never parsed, because `scripts/ghidra/import-runtime-gzf.sh` imports with
`-noanalysis` on the reasoning that a .gzf carries its own analysis. That reasoning held for the
curated 1.16.2 export and does not hold for this file.

Parsing it here rather than re-running Ghidra's analyzer is a minutes-vs-hours choice on a 94 MB
image, and it produces exactly the artifact the lookups need: a name -> vtable table for the build
that is actually running, with no cross-version translation in the path.

LAYOUT (x64 MSVC), which is why the walk is three passes:
  TypeDescriptor      { void* pVFTable; void* spare; char name[]; }   name at +0x10
  CompleteObjectLocator { u32 signature; u32 offset; u32 cdOffset;
                          u32 pTypeDescriptor_rva; u32 pClassDescriptor_rva; u32 pSelf_rva; }
  vtable              [ COL* ][ vfunc0 ][ vfunc1 ] ...   -- the COL pointer sits at vtable-8

`pSelf_rva` is what makes this reliable: a genuine COL stores its own RVA, so a candidate that
points at itself is a COL and a coincidence is not. Signature 1 (x64) is checked too.
"""

import argparse
import os
import re
import struct
import sys

DEFAULT_IMAGE = os.environ.get("ER_DEOBF_BIN") or os.environ.get("ER_DEOBF_IMAGE") or "eldenring-deobf-1.17.bin"
IMAGE_BASE = 0x140000000
COL_SIGNATURE_X64 = 1
TYPE_DESCRIPTOR_NAME_OFFSET = 0x10


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--image", default=DEFAULT_IMAGE, help="flat de-Arxan'd image (default: $ER_DEOBF_BIN)")
    ap.add_argument("--base", type=lambda v: int(v, 0), default=IMAGE_BASE)
    ap.add_argument("--filter", default="", help="only report classes whose name contains this substring")
    ap.add_argument("--selftest", action="store_true", help="assert a known class resolves, then exit")
    args = ap.parse_args()

    try:
        data = open(args.image, "rb").read()
    except OSError as exc:
        print(f"cannot read image: {exc}", file=sys.stderr)
        return 2
    base = args.base

    # Pass 1: every TypeDescriptor, keyed by its own RVA (the COL's pTypeDescriptor points here).
    descriptors = {}
    for m in re.finditer(rb"\.\?AV[\x20-\x7e]{1,200}?@@", data):
        name_off = m.start()
        td_off = name_off - TYPE_DESCRIPTOR_NAME_OFFSET
        if td_off < 0:
            continue
        descriptors[td_off] = m.group().decode("ascii")

    # Pass 2: every CompleteObjectLocator, found by its self-RVA rather than guessed at.
    cols = {}
    for td_off, name in descriptors.items():
        pass
    view = memoryview(data)
    limit = len(data) - 24
    for off in range(0, limit, 4):
        sig, _offset, _cd, td_rva, _cd_rva, self_rva = struct.unpack_from("<6I", view, off)
        if sig != COL_SIGNATURE_X64 or self_rva != off:
            continue
        name = descriptors.get(td_rva)
        if name is not None:
            cols[off] = name

    # Pass 3: the vtable is whatever qword points at a COL, plus 8.
    found = {}
    for off in range(0, len(data) - 8, 8):
        q = int.from_bytes(data[off : off + 8], "little")
        if q < base:
            continue
        col_off = q - base
        name = cols.get(col_off)
        if name is None:
            continue
        found.setdefault(name, []).append(base + off + 8)

    if args.selftest:
        probe = ".?AVGridControl@CS@@"
        vtables = found.get(probe)
        if not vtables:
            print(f"SELFTEST FAILED: {probe} has no vtable in {args.image}", file=sys.stderr)
            return 1
        print(f"SELFTEST OK: {probe} -> {' '.join(hex(v) for v in vtables)}")
        return 0

    needle = args.filter
    printed = 0
    for name in sorted(found):
        if needle and needle not in name:
            continue
        print(f"{name}\t{' '.join(hex(v) for v in found[name])}")
        printed += 1
    print(f"# {printed} classes ({len(found)} total) from {args.image}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
