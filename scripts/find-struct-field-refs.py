#!/usr/bin/env python3
"""Find every instruction in a flat deobf ELDEN RING image that touches `[reg + DISP]`.

Motivation: Ghidra's xref database answers "who calls this function" and "who reads this
global", but a STRUCT field accessed through a `this` pointer has no xref at all -- the
only trace it leaves is a 4-byte displacement inside a ModRM/SIB encoding. When the
question is "what READS the field I am about to force" (the difference between forcing a
predicate and forcing a value nothing consumes), that displacement is the whole evidence.

Strategy: locate the raw little-endian displacement bytes, then re-decode a short window
ending at each candidate instruction start so capstone confirms the bytes really are the
memory displacement of a real instruction rather than an immediate, a pointer, or padding.

The image is flat (file offset == RVA), so `VA = 0x140000000 + offset` -- see AGENTS.md,
"`.rdata` IS shift-0 too".

Run under uv so capstone is provisioned ephemerally:

    uv run --with capstone python3 scripts/find-struct-field-refs.py 0x88d
    uv run --with capstone python3 scripts/find-struct-field-refs.py 0x88d --image eldenring-deobf-1.17.bin
"""

from __future__ import annotations

import argparse
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
IMAGE_BASE = 0x140000000
# The longest x86-64 instruction is 15 bytes; a disp32 can start at most 11 bytes into one
# (4 prefix/REX + opcode bytes + ModRM + SIB still leaves the disp before any immediate).
MAX_BYTES_BEFORE_DISP = 12
MAX_INSN_LEN = 16


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("displacement", help="struct field offset, e.g. 0x88d")
    parser.add_argument(
        "--image",
        default="eldenring-deobf.bin",
        help="flat de-Arxan'd image, relative to the repo root (default: the 1.16.2 one)",
    )
    args = parser.parse_args()

    try:
        import capstone
    except ImportError:
        print(
            "capstone is not importable. There is no system pip; run this under uv:\n"
            f"    uv run --with capstone python3 scripts/{pathlib.Path(__file__).name} ...",
            file=sys.stderr,
        )
        return 2

    disp = int(args.displacement, 0)
    image_path = pathlib.Path(args.image)
    if not image_path.is_absolute():
        image_path = REPO_ROOT / image_path
    if not image_path.exists():
        print(f"missing image: {image_path}", file=sys.stderr)
        return 2

    data = image_path.read_bytes()
    needle = disp.to_bytes(4, "little")

    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    decoder.detail = True

    hits: dict[int, tuple[str, str, str]] = {}
    pos = 0
    while True:
        found = data.find(needle, pos)
        if found < 0:
            break
        pos = found + 1
        for back in range(1, MAX_BYTES_BEFORE_DISP + 1):
            start = found - back
            if start < 0:
                continue
            for insn in decoder.disasm(data[start : start + MAX_INSN_LEN], IMAGE_BASE + start, count=1):
                if insn.size <= back:
                    continue
                touches = any(
                    op.type == capstone.x86.X86_OP_MEM and op.mem.disp == disp for op in insn.operands
                )
                if touches:
                    hits.setdefault(
                        IMAGE_BASE + start,
                        (insn.mnemonic, insn.op_str, data[start : start + insn.size].hex()),
                    )

    print(f"image={image_path.name} displacement=0x{disp:x} instructions={len(hits)}")
    for va in sorted(hits):
        mnemonic, operands, raw = hits[va]
        print(f"  0x{va:x}  {mnemonic} {operands}   [{raw}]")
    return 0


if __name__ == "__main__":
    sys.exit(main())
