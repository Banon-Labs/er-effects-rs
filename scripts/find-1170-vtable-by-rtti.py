#!/usr/bin/env python3
"""Locate a C++ vtable in either ELDEN RING image by its RTTI class name. Exact, not a vote.

Why the vote was not enough. `locate-1170-vtable.py` infers a vtable's new address from how many
of its slots hold already-mapped functions. That works until sibling classes enter the picture.
`RideManipulator`, `ComManipulator`, `PadManipulator` and `NetAIManipulator` all derive from
`ChrManipulator` and therefore share most of their slots -- so a wrong sibling's vtable scores
almost as well as the right one. Measured on 1.16.2 `RideManipulator` (`0x142a2c108`): the top two
candidate bases tied at 42 agreeing slots each. Picking the winner would have been a coin flip
dressed as evidence, and the loser was `0x142a2f118` -- the address a uniform `+0x3010` shift
predicts, which is exactly the kind of plausible-looking wrong answer this repo has been bitten by.

The exact answer. MSVC records the class name in the binary. Every polymorphic class has a
TypeDescriptor holding its decorated name (`.?AVRideManipulator@CS@@`); a Complete Object Locator
points at that descriptor; and the qword immediately before a vtable points at that locator. So
the chain runs name -> descriptor -> locator -> vtable with no similarity metric anywhere in it.
A hit is the class, by the compiler's own record.

    TypeDescriptor : +0x00 vftable* , +0x08 spare , +0x10 name (NUL-terminated)
    COL (x64)      : +0x00 signature(=1) , +0x04 offset , +0x08 cdOffset ,
                     +0x0c pTypeDescriptor(RVA) , +0x10 pClassDescriptor(RVA) , +0x14 pSelf(RVA)
    vtable         : COL qword sits at vtable-8

Both images are FLAT: file offset == RVA, VA = 0x140000000 + offset, `.rdata` included. That is
why an RVA can be compared against a file offset directly here.
"""
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMAGES = {
    "1.16.2": os.path.join(ROOT, "eldenring-deobf.bin"),
    "1.17": os.path.join(ROOT, "eldenring-deobf-1.17.bin"),
}


def find_all(data, needle, limit=64):
    out, start = [], 0
    while len(out) < limit:
        i = data.find(needle, start)
        if i < 0:
            break
        out.append(i)
        start = i + 1
    return out


def vtables_for(data, classname):
    """Every vtable whose RTTI names `classname`, as (vtable_va, col_va, typedesc_va)."""
    name = classname.encode() + b"\x00"
    results = []
    for name_off in find_all(data, name):
        # A TypeDescriptor's name starts 0x10 into it, and its first qword is the type_info
        # vftable pointer -- a cheap sanity check that this is a descriptor and not prose.
        td_off = name_off - 0x10
        if td_off < 0:
            continue
        head = int.from_bytes(data[td_off : td_off + 8], "little")
        if not (BASE <= head < BASE + 0x10000000):
            continue
        td_rva = td_off
        for col_hit in find_all(data, td_rva.to_bytes(4, "little"), limit=256):
            col_off = col_hit - 0x0C  # pTypeDescriptor sits at COL+0x0c
            if col_off < 0:
                continue
            if int.from_bytes(data[col_off : col_off + 4], "little") != 1:
                continue  # x64 COL signature
            if int.from_bytes(data[col_off + 0x14 : col_off + 0x18], "little") != col_off:
                continue  # pSelf must point back at the COL
            col_va = col_off + BASE
            for ptr in find_all(data, col_va.to_bytes(8, "little")):
                results.append((ptr + 8 + BASE, col_va, td_off + BASE))
    return results


def main():
    if len(sys.argv) < 2:
        sys.exit(
            'usage: find-1170-vtable-by-rtti.py "<decorated name>" [...]\n'
            '  e.g. find-1170-vtable-by-rtti.py ".?AVRideManipulator@CS@@"'
        )
    loaded = {v: open(p, "rb").read() for v, p in IMAGES.items()}
    for classname in sys.argv[1:]:
        print(f"== {classname}")
        for version, data in loaded.items():
            hits = vtables_for(data, classname)
            if not hits:
                print(f"   {version:7s} NOT FOUND")
            for vt, col, td in hits:
                print(f"   {version:7s} vtable {vt:#x}   (COL {col:#x}, TypeDescriptor {td:#x})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
