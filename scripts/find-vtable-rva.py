#!/usr/bin/env python3
"""Locate a C++ class's vtable in an ELDEN RING image, by RTTI, in both builds at once.

Why this exists
---------------
The 1.16.2 -> 1.17 migration has a gate for code addresses (`er_game_base::game_build`, fed by
`docs/recon/rva-map-1162-to-1170.needed-verified.tsv`). It does not cover VTABLE and other `.rdata`
addresses, and those moved too. Measured 2026-08-29: `TITLE_OWNER_VTABLE_RVA = 0x2b63bb0`, hard
coded in three crates, is `CS::TitleStep` in 1.16.2 and is not a vtable at all in 1.17. The scans
that use it are read-only and fault-safe, so nothing crashes -- they silently find nothing, and the
features behind them are quietly dead. A silent wrong answer is worse than a refusal, which is why
this exists as a tool rather than a note.

How it works (MSVC x64 RTTI)
    vtable[-1]  -> RTTICompleteObjectLocator (absolute VA)
    COL + 0x0c  -> pTypeDescriptor, an image-relative RVA
    TD  + 0x10  -> the mangled name, NUL-terminated, e.g. ".?AVCSFadeImp@CS@@"

Both images are FLAT: file offset == RVA, VA = 0x140000000 + offset. So the scan is a single pass
over the file looking for qwords that point at a COL whose type descriptor carries the wanted name.

Usage
    python3 scripts/find-vtable-rva.py CSFadeImp
    python3 scripts/find-vtable-rva.py TitleStep TitleTopDialog
    python3 scripts/find-vtable-rva.py --rva 0x2b63bb0      # inverse: what lives here?
    python3 scripts/find-vtable-rva.py --selftest
"""

from __future__ import annotations

import argparse
import os
import struct
import sys

BASE = 0x140000000
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGES = {
    "1.16.2": os.environ.get("ER_DEOBF_IMAGE_1162", os.path.join(REPO, "eldenring-deobf.bin")),
    "1.17": os.environ.get("ER_DEOBF_IMAGE_1170", os.path.join(REPO, "eldenring-deobf-1.17.bin")),
}
# A type descriptor's name starts 0x10 bytes in, past the vfptr and the spare pointer.
TYPE_DESCRIPTOR_NAME_OFFSET = 0x10
# RTTICompleteObjectLocator: signature, offset, cdOffset, pTypeDescriptor, pClassDescriptor(, pSelf).
COL_TYPE_DESCRIPTOR_OFFSET = 0x0C
# How far back from a slot to look for the vtable it belongs to. Generous: ER's menu/functor
# vtables are short, but a deep interface can carry dozens of entries.
MAX_VTABLE_SLOTS_BACK = 64


def load(path: str) -> bytes | None:
    try:
        with open(path, "rb") as handle:
            return handle.read()
    except OSError:
        return None


def class_name_at_vtable(image: bytes, vtable_rva: int) -> str | None:
    """The RTTI class name for a vtable at `vtable_rva`, or None if that is not a vtable."""
    if not 8 <= vtable_rva < len(image):
        return None
    col_va = struct.unpack_from("<Q", image, vtable_rva - 8)[0]
    if not BASE <= col_va < BASE + len(image):
        return None
    col = col_va - BASE
    if col + COL_TYPE_DESCRIPTOR_OFFSET + 4 > len(image):
        return None
    type_descriptor = struct.unpack_from("<I", image, col + COL_TYPE_DESCRIPTOR_OFFSET)[0]
    start = type_descriptor + TYPE_DESCRIPTOR_NAME_OFFSET
    if not 0 < start < len(image):
        return None
    end = image.find(b"\0", start, start + 256)
    if end < 0:
        return None
    name = image[start:end]
    return name.decode("ascii", "replace") if name.startswith(b".?A") else None


def find_vtables(image: bytes, wanted: str) -> list[int]:
    """Every vtable rva in the image whose RTTI name contains `wanted`.

    Driven from the type DESCRIPTORS rather than by testing all ~12M qwords: find the descriptors
    whose name matches, then the COLs that reference them, then the vtables that reference those.
    Three narrow passes instead of one enormous one.
    """
    needle = wanted.encode("ascii", "replace")
    descriptors = []
    at = image.find(b".?A")
    while at >= 0:
        end = image.find(b"\0", at, at + 256)
        if end > 0 and needle in image[at:end]:
            descriptors.append(at - TYPE_DESCRIPTOR_NAME_OFFSET)
        at = image.find(b".?A", at + 1)
    if not descriptors:
        return []
    wanted_descriptors = set(descriptors)

    locators = set()
    for offset in range(0, len(image) - 4, 4):
        value = struct.unpack_from("<I", image, offset)[0]
        if value in wanted_descriptors and offset >= COL_TYPE_DESCRIPTOR_OFFSET:
            locators.add(BASE + offset - COL_TYPE_DESCRIPTOR_OFFSET)
    if not locators:
        return []

    found = []
    for offset in range(0, len(image) - 8, 8):
        if struct.unpack_from("<Q", image, offset)[0] in locators:
            found.append(offset + 8)
    return found


def vtables_holding(image: bytes, func_rva: int) -> list[tuple[int, int, str | None]]:
    """`(vtable rva, slot index, class name)` for every vtable whose slot holds `func_rva`.

    The only way to carry a virtual method that changed.

    Two other mappers exist and neither can do this. A body-signature mapper identifies a function
    by what it looks like, so it goes silent exactly when the body was rewritten. Caller voting
    fixes that -- but only for a direct call, and a virtual method has no direct caller to vote:
    the game reaches it through `[vtable + n]`. Measured 2026-08-29: the now-loading helper's
    `Update` (1.16.2 0x2a2c40) was absent from the 128,603-row function map, unresolvable by
    masked signature, and reported "no usable caller" -- while the game refused its address 1,513
    times in a single boot and the loading bar never moved.

    A vtable, though, knows its own name. So: find the slot in 1.16.2, carry the VTABLE by RTTI
    (a mangled class name is unique per image), and read the same slot in 1.17. The method's own
    bytes never enter into it.
    """
    out = []
    needle = struct.pack("<Q", BASE + func_rva)
    at = image.find(needle)
    while at >= 0:
        if at % 8 == 0:
            # Walk back and test each candidate start, rather than walking until the preceding
            # qword stops looking like a code pointer. That heuristic overshoots every time: the
            # qword before a vtable is its CompleteObjectLocator, which is also an in-image
            # pointer, so the walk sails past the start it was looking for and lands somewhere
            # with no RTTI -- reporting "no vtable slot holds this function" for a function that
            # is sitting in slot 2 of a named vtable.
            for slot in range(MAX_VTABLE_SLOTS_BACK):
                start = at - slot * 8
                if start < 8:
                    break
                name = class_name_at_vtable(image, start)
                if name:
                    out.append((start, slot, name))
                    break
        at = image.find(needle, at + 1)
    return out


def pointers_to(image: bytes, func_rva: int) -> list[int]:
    """Every 8-byte-aligned qword in the whole image that holds `BASE + func_rva`.

    Wider than `vtables_holding`, which requires the run to carry an RTTI name. A function can be
    reached from a plain function-pointer table, a task registration array, or a jump table with
    no class attached, and none of those is a vtable. When a function has zero direct callers and
    zero rip-relative references, this says whether it is reachable from data at all -- which is
    the difference between "look in a table" and "this needs a live process".
    """
    out = []
    needle = struct.pack("<Q", BASE + func_rva)
    at = image.find(needle)
    while at >= 0:
        if at % 8 == 0:
            out.append(at)
        at = image.find(needle, at + 1)
    return out


def selftest() -> int:
    """Two facts measured on 2026-08-29, one per image, plus the inverse lookup."""
    failures = []
    images = {name: load(path) for name, path in IMAGES.items()}
    for name, image in images.items():
        if image is None:
            print(f"selftest SKIP: {name} image not present at {IMAGES[name]}")
            return 0
    got = class_name_at_vtable(images["1.17"], 0x2B6D728)
    if got != ".?AVCSFadeImp@CS@@":
        failures.append(f"1.17 0x2b6d728: got {got!r}, want '.?AVCSFadeImp@CS@@'")
    got = class_name_at_vtable(images["1.16.2"], 0x2B63BB0)
    if got != ".?AVTitleStep@CS@@":
        failures.append(f"1.16.2 0x2b63bb0: got {got!r}, want '.?AVTitleStep@CS@@'")
    # The whole point: that rva is not the same vtable in 1.17.
    if class_name_at_vtable(images["1.17"], 0x2B63BB0) == ".?AVTitleStep@CS@@":
        failures.append("1.17 0x2b63bb0 still resolves to TitleStep -- the premise of this tool is wrong")
    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("names", nargs="*", help="class names (substring of the mangled RTTI name)")
    parser.add_argument("--rva", help="inverse lookup: name the class whose vtable is at this rva")
    parser.add_argument(
        "--slot-of",
        help="find every vtable whose slot holds this FUNCTION rva, and read the same slot in 1.17",
    )
    parser.add_argument(
        "--pointer-to",
        help="every aligned qword in the whole image holding this FUNCTION rva (data tables, not "
        "just vtables)",
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    images = {}
    for name, path in IMAGES.items():
        image = load(path)
        if image is None:
            print(f"missing image for {name}: {path}", file=sys.stderr)
            return 1
        images[name] = image

    if args.pointer_to:
        func = int(args.pointer_to, 0) & 0xFFFFFFFF
        for name, image in images.items():
            hits = pointers_to(image, func)
            shown = ", ".join(f"0x{h:x}" for h in hits[:10]) or "(none)"
            print(f"  {name:>6}  {len(hits)} pointer(s) to 0x{func:x}: {shown}")
        return 0

    if args.slot_of:
        func = int(args.slot_of, 0) & 0xFFFFFFFF
        old_image, new_image = images["1.16.2"], images["1.17"]
        holders = vtables_holding(old_image, func)
        if not holders:
            print(f"0x{func:x}: no vtable slot holds this function in 1.16.2")
            return 1
        answers: dict[int, list[str]] = {}
        for vtable, slot, name in holders:
            print(f"  1.16.2 vtable 0x{vtable:x} slot {slot}  {name}")
            twin = next((v for v in find_vtables(new_image, name) if class_name_at_vtable(new_image, v) == name), None)
            if twin is None:
                print("         1.17: no vtable with that class name")
                continue
            moved = struct.unpack_from("<Q", new_image, twin + slot * 8)[0] - BASE
            print(f"         1.17 vtable 0x{twin:x} slot {slot} -> 0x{moved:x}  delta +0x{moved - func:x}")
            answers.setdefault(moved, []).append(name)
        if len(answers) == 1:
            only = next(iter(answers))
            print(f"CARRIED 0x{func:x} -> 0x{only:x} (agreed by {len(holders)} vtable slot(s))")
            return 0
        print(f"AMBIGUOUS: {len(answers)} different answers across the holding vtables")
        return 1

    if args.rva:
        rva = int(args.rva, 0) & 0xFFFFFFFF
        print(f"vtable rva 0x{rva:x}")
        for name, image in images.items():
            print(f"  {name:>6} -> {class_name_at_vtable(image, rva) or '(not a vtable)'}")
        return 0

    if not args.names:
        parser.error("give one or more class names, or --rva")
    for wanted in args.names:
        print(f"{wanted}:")
        for name, image in images.items():
            hits = find_vtables(image, wanted)
            shown = ", ".join(f"0x{h:x}" for h in hits[:8]) or "(none)"
            more = f" (+{len(hits) - 8} more)" if len(hits) > 8 else ""
            print(f"  {name:>6}  {shown}{more}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
