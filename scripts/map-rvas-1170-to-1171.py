#!/usr/bin/env python3
"""Carry an ELDEN RING game address from 1.17.0 (2.7.0.0) to 1.17.1 (2.7.1.0).

Why this one is a lookup table and not a signature matcher
----------------------------------------------------------
`scripts/map-rvas-1162-to-1170.py` has to mask displacements and hunt for masked
byte signatures, because 1.16.2 to 1.17.0 moved code by an amount that changed
over spans as short as 0xb00 bytes. The 1.17.0 to 1.17.1 step is nothing like
that. Measured from the `.pdata` of both de-Arxan'd images:

  * every function entry below `0xafefe9` keeps its address, and
  * every one of the 174,389 function entries at or above it moves by exactly
    `+0x70`, with zero entries left unexplained.

That is the whole patch. One function grew, everything downstream slid, and the
section table did not move a byte: `.rdata`, `.data` and `.pdata` occupy the
same virtual addresses and the same sizes in both builds. So an address is
carried forward by adding a constant or by adding nothing, decided by which
section it lands in and which side of one boundary it sits on.

The oracle here is `.pdata`, not a byte comparison of the two images, and a
byte comparison misleads anyone who tries it: 8.87% of `.text` differs even between the two de-Arxan'd
images, because Arxan re-randomises its inline obfuscation on every build and
`dearxan` neutralises stubs rather than un-mutating function bodies. The
structural evidence is `.pdata`, which `dearxan` does not rewrite.

Usage
-----
    python3 scripts/map-rvas-1170-to-1171.py 0x140afefe9 0x1409a4670
    python3 scripts/map-rvas-1170-to-1171.py --bundle in.rs --out out.rs
    python3 scripts/map-rvas-1170-to-1171.py --selftest

Both images are read-only inputs and are never written. They are the
copyrighted game binary and are gitignored; see `AGENTS.md`.
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys
from pathlib import Path

IMAGE_BASE = 0x140000000

# The one function that grew, and by how much. `0xafeea0` is 329 bytes of code
# in 1.17.0 and 441 in 1.17.1; the extra 112 push everything downstream along.
GROWN_FUNCTION_RVA = 0xAFEEA0
SHIFT = 0x70

# The first address that moves. The chained `.pdata` entries covering the grown
# function run to `0xafefe9`, and that entry is the lowest one whose 1.17.1
# twin sits at `+0x70`.
BOUNDARY_RVA = 0xAFEFE9

# One more function changed without moving anything: `0x82dc20` is 8 bytes
# longer in 1.17.1 and absorbed the difference out of its own alignment
# padding. It is listed so that a caller mapping an address inside it knows the
# body is not the same code, even though the entry address is unchanged.
CHANGED_IN_PLACE = {0x82DC20: (0x126, 0x12E)}

# Section table, identical in both builds. (name, rva, virtual size)
SECTIONS = [
    (".text", 0x00001000, 0x029A4800),
    (".interpr", 0x029A6000, 0x0000C000),
    (".rdata", 0x029B2000, 0x01162800),
    (".data", 0x03B15000, 0x00D51BC4),
    (".pdata", 0x04867000, 0x002B3200),
    (".tls", 0x04B1B000, 0x00000200),
    (".gfids", 0x04B1C000, 0x00001200),
    (".rsrc", 0x04B1E000, 0x00035C00),
    (".reloc", 0x04B54000, 0x000B9E00),
    (".idata", 0x04C0E000, 0x00004200),
    # The Arxan protection section. It grew by 0x4400 and its contents are
    # re-randomised per build, so nothing in it carries forward at all.
    (".text$arxan", 0x04C13000, 0x011FAA00),
]

TEXT_LO = 0x00001000
TEXT_HI = TEXT_LO + 0x029A4800
ARXAN_LO = 0x04C13000

PDATA_RVA = 0x04867000
PDATA_SIZE = 0x002B3200

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OLD = Path(os.environ.get("ER_DEOBF_1170", REPO_ROOT / "eldenring-deobf-1.17.bin"))
DEFAULT_NEW = Path(os.environ.get("ER_DEOBF_1171", REPO_ROOT / "eldenring-deobf-1.17.1.bin"))


class Unmappable(Exception):
    """The address cannot be carried forward, and guessing would be worse."""


def section_of(rva: int) -> str | None:
    for name, lo, size in SECTIONS:
        if lo <= rva < lo + size:
            return name
    return None


def map_rva(rva: int) -> tuple[int, str]:
    """Return the 1.17.1 rva and a one-line account of how it was reached."""
    sec = section_of(rva)
    if sec is None:
        raise Unmappable(f"{rva:#x} is not inside any section of the image")
    if sec == ".text$arxan":
        raise Unmappable(
            f"{rva:#x} is in the Arxan protection section, which is "
            "re-randomised every build and carries nothing forward"
        )
    if sec != ".text":
        return rva, f"unchanged, {sec} did not move between the two builds"
    if rva >= BOUNDARY_RVA:
        return rva + SHIFT, f"+{SHIFT:#x}, at or above the {BOUNDARY_RVA:#x} boundary"
    for start, (old_len, new_len) in CHANGED_IN_PLACE.items():
        if start <= rva < start + old_len:
            return rva, (
                f"unchanged address, but this is inside {start:#x}, whose body "
                f"grew from {old_len:#x} to {new_len:#x} bytes: re-read it"
            )
    return rva, f"unchanged, below the {BOUNDARY_RVA:#x} boundary"


def map_va(va: int) -> tuple[int, str]:
    rva, why = map_rva(va - IMAGE_BASE)
    return rva + IMAGE_BASE, why


def pdata_entries(image: bytes) -> set[tuple[int, int]]:
    """The (begin, end) pairs of `.pdata` that lie in the primary `.text`."""
    out = set()
    for off in range(0, PDATA_SIZE, 12):
        begin, end, _unwind = struct.unpack_from("<III", image, PDATA_RVA + off)
        if TEXT_LO <= begin < TEXT_HI and begin < end <= TEXT_HI:
            out.add((begin, end))
    return out


def function_entries(image: bytes) -> set[int]:
    return {begin for begin, _ in pdata_entries(image)}


def load(path: Path) -> bytes:
    if not path.is_file():
        raise Unmappable(
            f"{path} is absent. It is the de-Arxan'd game image, gitignored on "
            "purpose; regenerate it with scripts/dearxan-deobfuscate.rs"
        )
    return path.read_bytes()


BUNDLE_FIELD = re.compile(r"^(\s*)([a-z0-9_]+)(\s*:\s*)0x([0-9a-fA-F]+)(\s*,\s*)$")


def transform_bundle(text: str, new_image: bytes | None) -> tuple[str, list[str]]:
    """Rewrite every rva literal in a generated `RvaBundle` source file."""
    entries = function_entries(new_image) if new_image is not None else None
    out_lines, notes = [], []
    for line in text.splitlines(keepends=True):
        m = BUNDLE_FIELD.match(line.rstrip("\n"))
        if not m:
            out_lines.append(line)
            continue
        indent, field, sep, hexval, tail = m.groups()
        rva = int(hexval, 16)
        try:
            mapped, why = map_rva(rva)
        except Unmappable as exc:
            notes.append(f"{field}: left as {rva:#x} -- {exc}")
            out_lines.append(line)
            continue
        if entries is not None and section_of(rva) == ".text" and mapped not in entries:
            notes.append(
                f"{field}: {rva:#x} -> {mapped:#x} is not a function entry in "
                "the 1.17.1 image; verify by hand before trusting it"
            )
        if mapped != rva:
            notes.append(f"{field}: {rva:#x} -> {mapped:#x} ({why})")
        out_lines.append(f"{indent}{field}{sep}{mapped:#x}{tail}\n")
    return "".join(out_lines), notes


def selftest() -> int:
    """Re-derive the model from the two images rather than asserting it."""
    old, new = load(DEFAULT_OLD), load(DEFAULT_NEW)
    failures = []

    def check(label, cond, detail=""):
        print(f"  {'ok  ' if cond else 'FAIL'}  {label}{(' -- ' + detail) if detail else ''}")
        if not cond:
            failures.append(label)

    print("section table identical in both images:")
    for name, lo, size in SECTIONS:
        if name == ".text$arxan":
            continue
        check(f"{name} bytes are comparable at {lo:#x}", len(old) > lo + 16 and len(new) > lo + 16)

    print("\nevery function above the boundary moves by exactly +0x70:")
    eo, en = pdata_entries(old), pdata_entries(new)
    above = [p for p in eo if p[0] >= 0xB00000]
    shifted = [p for p in above if (p[0] + SHIFT, p[1] + SHIFT) in en]
    check(
        f"{len(shifted)} of {len(above)} entries above 0xb00000 map at +0x70",
        len(above) > 100000 and len(shifted) == len(above),
    )
    below = [p for p in eo if p[1] <= 0xB00000]
    kept = [p for p in below if p in en]
    check(
        f"{len(kept)} of {len(below)} entries below 0xb00000 keep their address",
        len(below) - len(kept) == 8,
        f"{len(below) - len(kept)} exceptions, expected the 8 around "
        f"{GROWN_FUNCTION_RVA:#x} and {sorted(CHANGED_IN_PLACE)[0]:#x}",
    )

    print("\nthe grown function accounts for the shift:")
    grown_old = sorted(p for p in eo if GROWN_FUNCTION_RVA <= p[0] < BOUNDARY_RVA)
    grown_new = sorted(p for p in en if GROWN_FUNCTION_RVA <= p[0] < BOUNDARY_RVA + SHIFT)
    span_old = max(p[1] for p in grown_old) - GROWN_FUNCTION_RVA
    span_new = max(p[1] for p in grown_new) - GROWN_FUNCTION_RVA
    check(f"body grew {span_old:#x} -> {span_new:#x}", span_new - span_old == SHIFT)

    print("\nvtable pointers follow the model (same rva, shifted contents):")
    for name, rva in (
        ("chr_cam_vmt", 0x2A2AA08),
        ("cscam_vmt", 0x2AA0A18),
        ("csbullet_state_vmt", 0x2A28418),
    ):
        good = bad = moved = 0
        for i in range(24):
            po = struct.unpack_from("<Q", old, rva + i * 8)[0]
            pn = struct.unpack_from("<Q", new, rva + i * 8)[0]
            if not (IMAGE_BASE + TEXT_LO <= po < IMAGE_BASE + TEXT_HI):
                continue
            expected, _ = map_va(po)
            if pn == expected:
                good += 1
                moved += 1 if expected != po else 0
            else:
                bad += 1
        check(f"{name}: {good} pointers match, {moved} of them moved", bad == 0 and moved > 0)

    print("\nknown addresses carry forward:")
    for va, expected in ((0x140AFEEA0, 0x140AFEEA0), (0x140B00000, 0x140B00070)):
        got, _ = map_va(va)
        check(f"{va:#x} -> {got:#x}", got == expected, f"expected {expected:#x}")
    for va in (0x145F00000, 0x144C13000 + 0x100):
        try:
            map_va(va)
            check(f"{va:#x} refused", False, "it was mapped instead")
        except Unmappable:
            check(f"{va:#x} refused rather than guessed", True)

    print()
    if failures:
        print(f"selftest FAILED: {len(failures)} check(s): {failures}")
        return 1
    print("selftest passed")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("addresses", nargs="*", help="virtual addresses or rvas, hex")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--bundle", type=Path, help="a generated RvaBundle .rs to rewrite")
    ap.add_argument("--out", type=Path, help="where to write the rewritten bundle")
    ap.add_argument("--no-verify", action="store_true", help="skip the function-entry check")
    a = ap.parse_args()

    if a.selftest:
        return selftest()

    if a.bundle:
        image = None if a.no_verify else load(DEFAULT_NEW)
        text, notes = transform_bundle(a.bundle.read_text(encoding="utf-8"), image)
        dest = a.out or a.bundle
        dest.write_text(text, encoding="utf-8")
        print(f"wrote {dest} ({len(notes)} field(s) reported)")
        for n in notes:
            print(f"  {n}")
        return 0

    if not a.addresses:
        ap.print_help()
        return 2

    rc = 0
    for raw in a.addresses:
        value = int(raw, 16)
        try:
            if value >= IMAGE_BASE:
                mapped, why = map_va(value)
            else:
                mapped, why = map_rva(value)
        except Unmappable as exc:
            print(f"{value:#x}  UNMAPPABLE  {exc}")
            rc = 1
            continue
        print(f"{value:#x} -> {mapped:#x}  {why}")
    return rc


if __name__ == "__main__":
    sys.exit(main())
