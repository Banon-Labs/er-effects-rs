#!/usr/bin/env python3
"""Enumerate every instruction in the deobf image that touches one struct displacement.

The question this answers is the one every entry in a `layout.rs` rests on: *who writes this
field?* A named constant is only as good as the claim that nothing else moves the value, and that
claim is a whole-image enumeration -- not a sample.

Why not `find-deobf-bytes.py`: that tool takes a fixed byte pattern, so it can only ask about one
instruction encoding at a time. A single `mov` to `[rax+0x1a0]` and the same store to
`[r11+0x1a0]` differ in the REX prefix, and an SIB-form base differs again -- so a pattern scan
either misses forms or drowns in them. This decodes the ModRM/SIB itself, so one run covers every
base register, every prefix and every operand size. It also does not silently truncate: it prints
what it found, all of it.

    python3 scripts/find-deobf-field-access.py 0x1a0
    python3 scripts/find-deobf-field-access.py 0x340 --range 0x1403f0000-0x140480000
    python3 scripts/find-deobf-field-access.py 0x468 --stores-only

The image is flat (file offset == RVA, base 0x140000000); override it with `ER_DEOBF_BIN`, which
is how you point this at 1.17 (`ER_DEOBF_BIN=eldenring-deobf-1.17.bin`).

**A displacement is not a struct.** Every hit is "some instruction uses displacement N off some
register", and most large structs have something at any given offset -- a 0x1a0 scan finds
`ChrCtrl.lockOnTagOffset` and several dozen unrelated fields in the same breath. Narrow with
`--range` to the code that owns the struct, then confirm each survivor by name in Ghidra. This
tool bounds the search; it does not conclude it.
"""

from __future__ import annotations

import argparse
import os
import sys

DEFAULT_IMG = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "eldenring-deobf.bin"
)
BASE = 0x140000000

# Opcodes that reach memory through a ModRM byte, as (opcode bytes, mnemonic, writes-memory).
# Deliberately not exhaustive over the whole ISA: this is the set that moves a struct field --
# integer and SSE loads and stores, plus the arithmetic forms that read one in place.
OPCODES: list[tuple[bytes, str, bool]] = [
    (b"\x88", "MOV byte store", True),
    (b"\x89", "MOV store", True),
    (b"\x8a", "MOV byte load", False),
    (b"\x8b", "MOV load", False),
    (b"\xc6", "MOV byte imm store", True),
    (b"\xc7", "MOV imm store", True),
    (b"\x0f\x10", "MOVUPS load", False),
    (b"\x0f\x11", "MOVUPS store", True),
    (b"\x0f\x28", "MOVAPS load", False),
    (b"\x0f\x29", "MOVAPS store", True),
    (b"\x0f\x12", "MOVLPS load", False),
    (b"\x0f\x13", "MOVLPS store", True),
    (b"\x0f\x16", "MOVHPS load", False),
    (b"\x0f\x17", "MOVHPS store", True),
    (b"\x0f\x58", "ADDPS", False),
    (b"\x0f\x5c", "SUBPS", False),
    (b"\x0f\x59", "MULPS", False),
    (b"\x0f\x2e", "UCOMISS", False),
    (b"\x0f\x2f", "COMISS", False),
    (b"\x0f\xb6", "MOVZX byte", False),
    (b"\x0f\xb7", "MOVZX word", False),
    (b"\x0f\xbe", "MOVSX byte", False),
    (b"\x0f\xbf", "MOVSX word", False),
    (b"\x84", "TEST byte", False),
    (b"\x85", "TEST", False),
    (b"\x38", "CMP byte", False),
    (b"\x39", "CMP", False),
    (b"\x80", "grp1 byte imm", False),
    (b"\x81", "grp1 imm32", False),
    (b"\x83", "grp1 imm8", False),
    (b"\x8d", "LEA", False),
]

# Same table for the mandatory-prefix SSE forms. The prefix is part of the opcode.
PREFIXED: list[tuple[bytes, str, bool]] = [
    (b"\xf3\x0f\x10", "MOVSS load", False),
    (b"\xf3\x0f\x11", "MOVSS store", True),
    (b"\xf2\x0f\x10", "MOVSD load", False),
    (b"\xf2\x0f\x11", "MOVSD store", True),
    (b"\x66\x0f\x10", "MOVUPD load", False),
    (b"\x66\x0f\x11", "MOVUPD store", True),
    (b"\x66\x0f\x28", "MOVAPD load", False),
    (b"\x66\x0f\x29", "MOVAPD store", True),
    (b"\x66\x0f\x6f", "MOVDQA load", False),
    (b"\x66\x0f\x7f", "MOVDQA store", True),
    (b"\xf3\x0f\x58", "ADDSS", False),
    (b"\xf3\x0f\x5c", "SUBSS", False),
    (b"\xf3\x0f\x59", "MULSS", False),
    (b"\xf3\x0f\x6f", "MOVDQU load", False),
    (b"\xf3\x0f\x7f", "MOVDQU store", True),
]


def displacement_bytes(disp: int) -> tuple[bytes, int]:
    """The encoded displacement and the ModRM `mod` value that carries it.

    `mod=01` is a signed byte, so only -128..127 encode that way; anything else is `mod=10`,
    four bytes little-endian. Getting this wrong is how a scan misses every access to a field
    below 0x80 -- they are encoded in one byte, not four.
    """
    if -0x80 <= disp <= 0x7F:
        return (disp & 0xFF).to_bytes(1, "little"), 0x40
    return (disp & 0xFFFFFFFF).to_bytes(4, "little"), 0x80


def scan(data: bytes, disp: int, stores_only: bool) -> list[tuple[int, str]]:
    """Every `(file offset, mnemonic)` whose memory operand is `[reg + disp]`."""
    encoded, mod = displacement_bytes(disp)
    hits: list[tuple[int, str]] = []
    for table in (PREFIXED, OPCODES):
        for opcode, mnemonic, writes in table:
            if stores_only and not writes:
                continue
            start = 0
            while True:
                at = data.find(opcode, start)
                if at < 0:
                    break
                start = at + 1
                modrm_at = at + len(opcode)
                if modrm_at >= len(data):
                    continue
                modrm = data[modrm_at]
                if modrm & 0xC0 != mod:
                    continue
                # rm == 4 means an SIB byte follows before the displacement; rm == 5 with a
                # non-zero mod is an ordinary [rbp+disp], not rip-relative (that is mod == 0).
                after = modrm_at + 1 + (1 if modrm & 7 == 4 else 0)
                if data[after : after + len(encoded)] != encoded:
                    continue
                hits.append((at, mnemonic))
    hits.sort()
    return hits


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("displacement", help="struct offset, e.g. 0x1a0")
    parser.add_argument(
        "--range",
        dest="va_range",
        help="restrict to LO-HI virtual addresses, e.g. 0x1403f0000-0x140480000",
    )
    parser.add_argument(
        "--stores-only",
        action="store_true",
        help="only instructions that WRITE the field -- the 'who moves this value' question",
    )
    parser.add_argument("--image", default=os.environ.get("ER_DEOBF_BIN", DEFAULT_IMG))
    args = parser.parse_args()

    disp = int(args.displacement, 0)
    low, high = BASE, BASE + (1 << 32)
    if args.va_range:
        lo_text, _, hi_text = args.va_range.partition("-")
        low, high = int(lo_text, 0), int(hi_text, 0)

    if not os.path.exists(args.image):
        print(f"missing image: {args.image} (set ER_DEOBF_BIN)", file=sys.stderr)
        return 1
    with open(args.image, "rb") as image:
        data = image.read()

    hits = [(off, name) for off, name in scan(data, disp, args.stores_only) if low <= off + BASE < high]
    kind = "stores" if args.stores_only else "accesses"
    print(f"displacement {disp:#x}: {len(hits)} {kind} in {low:#x}-{high:#x} of {args.image}")
    for off, name in hits:
        print(f"  {off + BASE:#x}  {name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
