#!/usr/bin/env python3
"""Offline caller scan of eldenring-deobf.bin (base 0x140000000): find direct call sites to a target VA.

Ghidra xref-free fallback (bd Ghidra-Runtime-Dump / when the persistent project is locked and the MCP is
unavailable): scans the deobf image for `e8 rel32` (near CALL rel32) whose computed target equals the
requested VA, and reports each caller VA. Answers "what calls FUN and (with a follow-up disasm of the
caller) under what condition" -- e.g. why an input-array builder never fires in a given menu state.

Usage: python3 scripts/find-deobf-callers.py 0x140240dc0 [--img eldenring-deobf.bin]
"""
from __future__ import annotations

import argparse
from pathlib import Path

BASE = 0x140000000


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("target", help="target VA, e.g. 0x140240dc0")
    ap.add_argument("--img", default=str(Path(__file__).resolve().parent.parent / "eldenring-deobf.bin"))
    ap.add_argument("--max", type=int, default=200, help="cap reported callers")
    a = ap.parse_args()
    target = int(a.target, 0)
    data = Path(a.img).read_bytes()
    n = len(data)
    print(f"img: {a.img}  ({n} bytes, VA {BASE:#x}..{BASE + n:#x})")
    print(f"target: {target:#x}\n")

    callers = []
    pos = 0
    find = data.find
    while True:
        i = find(0xE8, pos)
        if i < 0 or i + 5 > n:
            break
        pos = i + 1
        rel = int.from_bytes(data[i + 1 : i + 5], "little", signed=True)
        call_va = BASE + i
        tgt = (call_va + 5 + rel) & 0xFFFFFFFFFFFFFFFF
        if tgt == target:
            callers.append(call_va)
            if len(callers) >= a.max:
                break

    if not callers:
        print("NO direct `e8` callers found (target may be reached only indirectly via vtable/ff15, or is"
              " itself a jump thunk). Try disassembling the target's callers region another way.")
        return 1
    print(f"{len(callers)} direct caller call-site(s):")
    for c in callers:
        print(f"  call at {c:#x}  -> {target:#x}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
