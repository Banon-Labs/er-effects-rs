#!/usr/bin/env python3
"""Find E8 (call) / E9 (jmp) rel32 xrefs to a target VA in the deobf flat image.

Mapped image: file offset == RVA, image base 0x140000000 (see disas-deobf.sh).
Usage: find-xrefs.py <target_va_hex> [more_targets...]
"""
import sys

BASE = 0x140000000
IMG = __file__.rsplit("/", 1)[0] + "/../eldenring-deobf.bin"


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: find-xrefs.py <target_va_hex> [...]", file=sys.stderr)
        return 2
    targets = [int(a, 16) for a in sys.argv[1:]]
    with open(IMG, "rb") as f:
        data = f.read()
    n = len(data)
    found = {t: [] for t in targets}
    i = 0
    while i < n - 5:
        op = data[i]
        if op == 0xE8 or op == 0xE9:
            rel = int.from_bytes(data[i + 1 : i + 5], "little", signed=True)
            tgt = BASE + i + 5 + rel
            if tgt in found:
                kind = "call" if op == 0xE8 else "jmp"
                found[tgt].append((BASE + i, kind))
        i += 1
    for t in targets:
        hits = found[t]
        print(f"target 0x{t:x}: {len(hits)} rel32 xref(s)")
        for va, kind in hits:
            print(f"  {kind} from 0x{va:x}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
