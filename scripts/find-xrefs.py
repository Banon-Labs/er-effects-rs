#!/usr/bin/env python3
"""Find xrefs to a target VA in the deobf flat image.

Catches: E8 (call) / E9 (jmp) rel32 code xrefs, and absolute 8-byte LE pointer
occurrences (function-pointer tables / vtables -> indirect dispatch). Mapped
image: file offset == RVA, image base 0x140000000 (see disas-deobf.sh).
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
    rel = {t: [] for t in targets}
    i = 0
    while i < n - 5:
        op = data[i]
        if op == 0xE8 or op == 0xE9:
            disp = int.from_bytes(data[i + 1 : i + 5], "little", signed=True)
            tgt = BASE + i + 5 + disp
            if tgt in rel:
                kind = "call" if op == 0xE8 else "jmp"
                rel[tgt].append((BASE + i, kind))
        i += 1
    # Absolute 8-byte LE pointer occurrences (data refs / fn-pointer tables).
    ptr = {t: [] for t in targets}
    for t in targets:
        needle = t.to_bytes(8, "little")
        start = 0
        while True:
            j = data.find(needle, start)
            if j < 0:
                break
            ptr[t].append(BASE + j)
            start = j + 1
    for t in targets:
        print(f"target 0x{t:x}: {len(rel[t])} rel32 xref(s)")
        for va, kind in rel[t]:
            print(f"  {kind} from 0x{va:x}")
        print(f"target 0x{t:x}: {len(ptr[t])} absolute-pointer ref(s)")
        for va in ptr[t]:
            print(f"  ptr @ 0x{va:x}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
