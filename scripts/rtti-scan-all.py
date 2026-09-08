#!/usr/bin/env python3
"""Harvest every MSVC RTTI vtable -> class name from the deobfuscated ER mapped image,
for later Ghidra symbol sync. Mapped image: file offset == RVA, base 0x140000000.

MSVC x64 RTTI CompleteObjectLocator (COL) layout (all RVAs):
  +0x00 signature (1 for x64)
  +0x04 offset
  +0x08 cdOffset
  +0x0C pTypeDescriptor (RVA)   TypeDescriptor+0x10 = mangled name ".?AVClass@NS@@"
  +0x10 pClassDescriptor (RVA)
  +0x14 pSelf (RVA of this COL)   <-- the x64 identifier: u32[O+0x14] == O
A vtable's [base-8] qword holds the absolute VA of its COL.

Output: lines "0x<vtable_va>\t<class_name>" sorted by VA, plus a count header.

Usage: rtti-scan-all.py [out_file] [--image path]

Why --image exists. The image was hard-coded to the 1.16.2 `eldenring-deobf.bin`, which
made this tool unable to answer the one question it is uniquely good at during the
1.16.2 -> 1.17 migration: does the vtable a data-map row points at in 1.17 carry the same
mangled class name as the source did in 1.16.2? That is an identity check, not an
inference, and it needs the 1.17 image. Defaults are unchanged.
"""
import argparse, struct, os

BASE = 0x140000000
REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
IMG = os.path.join(REPO, "eldenring-deobf.bin")


def main():
    ap = argparse.ArgumentParser(description="Harvest MSVC RTTI vtable -> class name.")
    ap.add_argument("out_file", nargs="?", default="/tmp/er-deobf-rtti-classmap.tsv")
    ap.add_argument("--image", default=IMG, help="flat de-Arxan'd image to scan")
    args = ap.parse_args()
    out = args.out_file
    data = open(args.image, "rb").read()
    n = len(data)

    def rd_cstr(off):
        if not (0 <= off < n):
            return None
        end = data.find(b"\x00", off)
        return data[off:end].decode("latin1", "replace")

    # Pass 1: find all COLs (u32[O+0x14] == O, signature==1, valid TD name).
    col_class = {}  # col_va -> class_name
    O = 0
    while O + 0x18 <= n:
        # cheap reject: pSelf RVA must equal O
        self_rva = int.from_bytes(data[O + 0x14 : O + 0x18], "little")
        if self_rva == O:
            sig = int.from_bytes(data[O : O + 4], "little")
            if sig == 1:
                td_rva = int.from_bytes(data[O + 0x0C : O + 0x10], "little")
                name = rd_cstr(td_rva + 0x10) if 0 < td_rva < n else None
                if name and name.startswith(".?A"):
                    col_class[BASE + O] = name
        O += 4

    # Pass 2: linear scan qwords; if value is a known COL VA, vtable = pos+8.
    vtables = {}  # vtable_va -> class_name
    col_set = col_class
    pos = 0
    while pos + 8 <= n:
        v = struct.unpack_from("<Q", data, pos)[0]
        if v in col_set:
            vtables[BASE + pos + 8] = col_set[v]
        pos += 8

    with open(out, "w") as f:
        f.write(f"# deobf-rtti-classmap: {len(vtables)} vtables, {len(col_class)} COLs\n")
        for va in sorted(vtables):
            f.write(f"0x{va:x}\t{vtables[va]}\n")
    print(f"wrote {len(vtables)} vtables ({len(col_class)} COLs) -> {out}")


if __name__ == "__main__":
    main()
