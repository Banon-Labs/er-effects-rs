#!/usr/bin/env python3
"""Identify a virtual function by the RTTI class name of the vtable that holds it.

Why this is the strongest evidence class for a virtual
-----------------------------------------------------
A masked signature says "these bytes look the same"; a caller vote says "the code that called
the old address calls the new one". Neither applies to a function that is only ever reached
through a vtable: `CS::FeSystemAnnounceView::Update` has zero direct callers in either image
(measured: `report-1170-caller-votes.py 0x8c47c0` -> "0 candidate branch site(s)"), so the whole
caller-voting class is silent on it.

RTTI is not. MSVC emits, immediately before a vtable's first slot, a pointer to that class's
Complete Object Locator; `COL+0x0c` is the RVA of the Type Descriptor, and `+0x10` inside the
descriptor is the mangled class name as a NUL-terminated string. That name is a property of the
C++ source, not of the build layout, so it identifies the same class in two different images
with no address translation anywhere in the chain. If the 1.16.2 address sits in slot N of
`.?AVFeSystemAnnounceView@CS@@`'s vtable and the 1.17 candidate sits in slot N of a vtable whose
RTTI name is the same string, the two addresses are the same virtual method -- derived
independently of every byte-level argument.

The check deliberately reports the slot index as well as the name. Same class, different slot is
a different method, and it is the failure this is here to catch: a vtable that gained a method
shifts every slot after it, so "found in that class's vtable" alone would happily pair `Update`
with the method that used to follow it.

Usage
    python3 scripts/rtti-vtable-identity-1170.py 1162:0x1408c47c0 1170:0x1408c5960
"""

import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}
BASE = 0x140000000
# A vtable is a run of absolute 8-byte code pointers. Scanning every aligned qword of the whole
# image for the target VA finds each slot that holds it; walking back from a slot to the run's
# start finds the vtable head, and the qword before the head is the COL pointer.
PTR = 8


def load(build):
    with open(IMAGES[build], "rb") as handle:
        return handle.read()


def text_range(image):
    """`(begin_rva, end_rva)` of the executable section, so "is this a code pointer" is cheap."""
    e_lfanew = struct.unpack_from("<I", image, 0x3C)[0]
    sections = struct.unpack_from("<H", image, e_lfanew + 6)[0]
    magic = struct.unpack_from("<H", image, e_lfanew + 24)[0]
    table = e_lfanew + 24 + (240 if magic == 0x20B else 224)
    for index in range(sections):
        entry = table + index * 40
        name = image[entry : entry + 8].rstrip(b"\0")
        characteristics = struct.unpack_from("<I", image, entry + 36)[0]
        if name == b".text" or characteristics & 0x20000000:
            rva = struct.unpack_from("<I", image, entry + 12)[0]
            size = struct.unpack_from("<I", image, entry + 8)[0]
            return rva, rva + size
    raise SystemExit("no executable section")


def slots_holding(image, va, lo, hi):
    """Every aligned offset whose qword is `va`, restricted to plausible vtable data."""
    needle = struct.pack("<Q", va)
    out = []
    start = image.find(needle)
    while start != -1:
        if start % PTR == 0 and not (lo <= start < hi):
            out.append(start)
        start = image.find(needle, start + 1)
    return out


def vtable_head(image, slot, lo, hi):
    """Walk back while the preceding qword is also a code pointer; that run start is the head."""
    head = slot
    while head >= PTR:
        previous = struct.unpack_from("<Q", image, head - PTR)[0]
        if not (BASE + lo <= previous < BASE + hi):
            break
        head -= PTR
    return head


def rtti_name(image, head, lo, hi):
    """`vtable[-1]` -> COL -> `COL+0x0c` type descriptor -> `+0x10` mangled name."""
    if head < PTR:
        return None
    col = struct.unpack_from("<Q", image, head - PTR)[0]
    if not (BASE <= col < BASE + len(image)) or BASE + lo <= col < BASE + hi:
        return None
    col_off = col - BASE
    descriptor_rva = struct.unpack_from("<I", image, col_off + 0x0C)[0]
    if not descriptor_rva or descriptor_rva + 0x10 >= len(image):
        return None
    end = image.find(b"\0", descriptor_rva + 0x10)
    if end == -1 or end - (descriptor_rva + 0x10) > 512:
        return None
    return image[descriptor_rva + 0x10 : end].decode("ascii", "replace")


def describe(build, va):
    image = load(build)
    lo, hi = text_range(image)
    found = False
    for slot in slots_holding(image, va, lo, hi):
        head = vtable_head(image, slot, lo, hi)
        name = rtti_name(image, head, lo, hi)
        index = (slot - head) // PTR
        print(
            f"[{build}] {va:#x} in vtable {BASE + head:#x} slot {index}"
            f"  RTTI={name or '<none>'}"
        )
        found = True
    if not found:
        print(f"[{build}] {va:#x} appears in no aligned pointer slot outside .text")


def main(argv):
    if not argv:
        sys.exit(__doc__)
    for target in argv:
        build, va = target.split(":")
        describe(build, int(va, 16))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
