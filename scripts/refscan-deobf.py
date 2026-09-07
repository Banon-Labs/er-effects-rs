#!/usr/bin/env python3
"""Scan the dearxan-DEOBFUSCATED ER mapped image for references to a target VA.
Mapped image: file offset == RVA, image base 0x140000000 (VA = offset + base).
Finds E8/E9 rel32 call/jmp sites and 8-byte le pointer occurrences to the target.
The image defaults to the 1.16.2 deobf, but `--img` is required for any 1.17 question:
the two builds are separate files (`eldenring-deobf-1.17.bin`), and a scan that silently
answers from 1.16.2 while the caller is asking about 1.17 is the exact stale-address
failure this migration exists to close.

Usage: refscan-deobf.py 0x1409b46b5 [--img eldenring-deobf-1.17.bin]
"""
import argparse, struct, os

BASE = 0x140000000
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("target", help="target VA, e.g. 0x1409b46b5")
    ap.add_argument("--img", default=IMG, help="flat deobf image (file offset == RVA)")
    ap.add_argument("--max", type=int, default=40, help="cap reported sites per category")
    args = ap.parse_args()
    target = int(args.target, 0)
    data = open(args.img, "rb").read()
    print(f"# img: {args.img}")
    n = len(data)

    # E8 (call) / E9 (jmp) rel32 sites
    e8 = []
    for i in range(0, n - 5):
        b = data[i]
        if b == 0xE8 or b == 0xE9:
            rel = struct.unpack_from("<i", data, i + 1)[0]
            tgt = (BASE + i + 5 + rel) & 0xFFFFFFFFFFFFFFFF
            if tgt == target:
                e8.append((BASE + i, "call" if b == 0xE8 else "jmp"))
    print(f"# E8/E9 rel32 sites -> 0x{target:x}: {len(e8)}")
    for va, k in e8[: args.max]:
        print(f"  0x{va:x}  {k}")

    # 8-byte le pointer occurrences (vtable slots etc.)
    tb = struct.pack("<Q", target)
    ptr = []
    start = 0
    while True:
        j = data.find(tb, start)
        if j < 0:
            break
        ptr.append(BASE + j)
        start = j + 1
    print(f"# 8-byte LE ptr refs -> 0x{target:x}: {len(ptr)}")
    for va in ptr[: args.max]:
        print(f"  0x{va:x}")


if __name__ == "__main__":
    main()
