#!/usr/bin/env python3
"""Scan .text for E8 (call rel32) / E9 (jmp rel32) targeting a VA (recon only).

recon_refs.py only finds rip-relative data refs (mov/lea/cmp); it misses
direct relative calls/jumps. This finds the actual call sites of a function.
Read-only.

Usage: python3 .auto/recon_callscan.py <hex_target_va>
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def main() -> None:
    target = int(sys.argv[1], 16)
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    hits = []
    for s in sections:
        if s["name"] != ".text":
            continue
        raw_off = s["raw_ptr"]
        raw_size = s["raw_size"]
        va_base = image_base + s["virtual_address"]
        blob = data[raw_off:raw_off + raw_size]
        n = len(blob)
        for i in range(n - 5):
            op = blob[i]
            if op != 0xE8 and op != 0xE9:
                continue
            rel = int.from_bytes(blob[i + 1:i + 5], "little", signed=True)
            insn_va = va_base + i
            dest = insn_va + 5 + rel
            if dest == target:
                kind = "call" if op == 0xE8 else "jmp"
                hits.append((insn_va, kind))
    print(f"# E8/E9 rel32 sites targeting {hex(target)}: {len(hits)}")
    for va, kind in hits:
        print(f"  {hex(va)}  {kind}")


if __name__ == "__main__":
    main()
