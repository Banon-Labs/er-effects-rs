#!/usr/bin/env python3
"""Find the vtable that holds a given `ersc.dll` function, and name its class via MSVC RTTI.

An adjusted `this` (`mov rdi,[rcx-0x98]`) says a function is a virtual override reached
through a secondary base, and the only way to learn what class that is, is to walk back
out of the vtable it sits in. `.rdata` stores vtable slots as absolute 64-bit virtual
addresses, so a qword scan finds the slot; slot -1 is the `RTTICompleteObjectLocator`,
whose type descriptor carries the decorated class name.

    uv run --with capstone python3 scripts/ersc-vtable.py 0x71dc0 0x73630
    uv run --with capstone python3 scripts/ersc-vtable.py --dump-vtable 0x1a2b30
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import struct
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("disas_ersc", os.path.join(_HERE, "disas-ersc.py"))
_disas = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_disas)
Image = _disas.Image
DEFAULT_DLL = _disas.DEFAULT_DLL


def qword_refs(img: Image, rva: int):
    """Every 8-byte-aligned .rdata/.data slot holding the absolute va of `rva`."""
    needle = struct.pack("<Q", img.base + rva)
    hits = []
    for name, va, vs, rp, rs in img.secs:
        if name not in (".rdata", ".data", "_RDATA"):
            continue
        blob = img.data[rp : rp + rs]
        i = blob.find(needle)
        while i != -1:
            if i % 8 == 0:
                hits.append(va + i)
            i = blob.find(needle, i + 1)
    return hits


def type_name(img: Image, slot_rva: int):
    """Read RTTICompleteObjectLocator at vtable[-1] and return the decorated class name."""
    o = img.off(slot_rva - 8)
    if o is None:
        return None
    col_va = struct.unpack_from("<Q", img.data, o)[0]
    col = col_va - img.base
    co = img.off(col)
    if co is None or not (0 <= col < 0x1000000):
        return None
    sig, _off, _cd, td_rva = struct.unpack_from("<IIII", img.data, co)
    if sig != 1:  # only the 64-bit image-relative form is expected here
        return None
    to = img.off(td_rva)
    if to is None:
        return None
    end = img.data.index(b"\0", to + 16)
    return img.data[to + 16 : end].decode(errors="replace")


def vtable_start(img: Image, slot_rva: int, max_back: int = 0x400):
    """Walk back to the first slot of the vtable containing slot_rva."""
    cur = slot_rva
    while slot_rva - cur < max_back:
        prev = cur - 8
        o = img.off(prev)
        if o is None:
            break
        v = struct.unpack_from("<Q", img.data, o)[0]
        r = v - img.base
        if not (0 < r < 0x200000 and img.section_of(r) == ".text"):
            break
        cur = prev
    return cur


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("rvas", nargs="*")
    ap.add_argument("--dll", default=DEFAULT_DLL)
    ap.add_argument("--dump-vtable", action="append", default=[])
    a = ap.parse_args()
    img = Image(a.dll)

    for raw in a.rvas:
        rva = int(raw, 0)
        print(f"\n===== {rva:#x} =====")
        for slot in qword_refs(img, rva):
            start = vtable_start(img, slot)
            idx = (slot - start) // 8
            nm = type_name(img, start)
            print(
                f"  slot {slot:#x} in {img.section_of(slot)}  vtable {start:#x} "
                f"index {idx}  class {nm}"
            )

    for raw in a.dump_vtable:
        start = int(raw, 0)
        print(f"\n===== vtable {start:#x}  class {type_name(img, start)} =====")
        o = img.off(start)
        for i in range(64):
            v = struct.unpack_from("<Q", img.data, o + i * 8)[0]
            r = v - img.base
            if not (0 < r < 0x200000 and img.section_of(r) == ".text"):
                break
            print(f"  [{i:2}] {r:#08x}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
