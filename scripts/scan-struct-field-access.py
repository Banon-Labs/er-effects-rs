#!/usr/bin/env python3
"""Find every instruction in a VA range that touches a given struct-field displacement.

Answers "who reads/writes `ThatStruct+0x2d4`" without a Ghidra project, by decoding the
flat de-Arxan'd image with capstone and filtering on the memory operand's displacement.
Register-relative only: `[rbp+disp]` / `[rsp+disp]` stack frames are skipped, because a
struct field is reached through a heap pointer, never through the frame pointer.

  uv run --with capstone python3 scripts/scan-struct-field-access.py \
      --range 0x1403b0000-0x1403c0000 --disp 0x258,0x2d4

Defaults to `eldenring-deobf.bin` (1.16.2); point `--image` or ER_DEOBF_BIN at
`eldenring-deobf-1.17.bin` for the installed build. The image is FLAT: file offset ==
RVA, so VA = 0x140000000 + offset for every section.
"""

from __future__ import annotations

import argparse
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGE_BASE = 0x140000000


def default_image() -> str:
    """`ER_DEOBF_BIN`, else the repo copy, else the same file beside the main checkout.

    An agent worktree does not carry the (gitignored, gigabyte) image, so the sibling
    lookup keeps this runnable from one without a hard-coded home directory.
    """
    env = os.environ.get("ER_DEOBF_BIN")
    if env:
        return env
    here = os.path.join(REPO, "eldenring-deobf.bin")
    if os.path.exists(here):
        return here
    # `<main>/.claude/worktrees/<agent>` -> `<main>`
    parts = REPO.split(os.sep)
    if ".claude" in parts:
        main = os.sep.join(parts[: parts.index(".claude")])
        candidate = os.path.join(main, "eldenring-deobf.bin")
        if os.path.exists(candidate):
            return candidate
    return here


def parse_range(text: str) -> tuple[int, int]:
    lo, _, hi = text.partition("-")
    if not hi:
        raise SystemExit(f"--range wants LO-HI, got {text!r}")
    return int(lo, 0), int(hi, 0)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--image", default=None)
    ap.add_argument("--range", required=True, help="VA range, e.g. 0x1403b0000-0x1403c0000")
    ap.add_argument("--disp", required=True, help="comma-separated displacements, e.g. 0x258,0x2d4")
    ap.add_argument("--base-reg", default=None, help="only match this base register, e.g. rbx")
    args = ap.parse_args()

    try:
        from capstone import CS_ARCH_X86, CS_MODE_64, Cs
        from capstone.x86 import X86_OP_MEM
    except ImportError:
        print("capstone missing -- run under: uv run --with capstone python3 ...", file=sys.stderr)
        return 2

    image = args.image or default_image()
    lo, hi = parse_range(args.range)
    wanted = {int(d, 0) for d in args.disp.split(",")}
    with open(image, "rb") as fh:
        fh.seek(lo - IMAGE_BASE)
        code = fh.read(hi - lo)

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    # A range that starts mid-instruction desynchronises a linear sweep, and capstone stops at
    # the first undecodable byte rather than resynchronising -- which silently returns "no
    # accesses" for a field that is accessed. `skipdata` emits a filler for the bad byte and
    # carries on, so the sweep covers the whole range.
    md.skipdata = True
    for insn in md.disasm(code, lo):
        # A `skipdata` filler carries no operands and raises on access rather than answering.
        if insn.id == 0:
            continue
        for op in insn.operands:
            if op.type != X86_OP_MEM or op.mem.disp not in wanted or op.mem.base == 0:
                continue
            base = insn.reg_name(op.mem.base)
            if base in ("rbp", "rsp"):
                continue
            if args.base_reg and base != args.base_reg:
                continue
            print(f"0x{insn.address:x}  {insn.mnemonic} {insn.op_str}")
            break
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
