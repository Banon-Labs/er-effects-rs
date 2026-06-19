#!/usr/bin/env python3
"""Harvest EVERY MSVC RTTI vtable -> class name from the deobfuscated ER mapped image,
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
Usage: rtti-scan-all.py [out_file]
"""
import sys, struct, os

BASE = 0x140000000
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/er-deobf-rtti-classmap.tsv"
    data = open(IMG, "rb").read()
    n = len(data)

    def rd_cstr(off):
        if not (0 <= off < n):
            return None
        end = data.find(b"\x00", off)
        return data[off:end].decode("latin1", "replace")

    # PASS 1: find all COLs (u32[O+0x14] == O, signature==1, valid TD name).
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

    # PASS 2: linear scan qwords; if value is a known COL VA, vtable = pos+8.
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
