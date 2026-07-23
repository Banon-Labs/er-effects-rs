#!/usr/bin/env python3
"""Find ANY rip-relative reference (lea reg,[rip+d] = 8D; mov reg,[rip+d] = 8B) to a target VA
in the deobfuscated ER image. base 0x140000000, file off == RVA. Reports insn VA + opcode kind."""
import sys, struct, os

BASE = 0x140000000
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")
RIPREL_MODRM = {0x05, 0x0D, 0x15, 0x1D, 0x2D, 0x35, 0x3D}  # mod=00 rm=101 reg=0..7
REX = {0x40,0x41,0x42,0x43,0x44,0x45,0x46,0x47,0x48,0x49,0x4A,0x4B,0x4C,0x4D,0x4E,0x4F}

def main():
    target = int(sys.argv[1], 16)
    data = open(IMG, "rb").read()
    n = len(data)
    hits = []
    for i in range(1, n - 7):
        rex = data[i - 1]
        if rex not in REX:
            continue
        op = data[i]
        if op in (0x8D, 0x8B):  # lea / mov reg,[rip+d]
            modrm = data[i + 1]
            if modrm not in RIPREL_MODRM:
                continue
            disp = struct.unpack_from("<i", data, i + 2)[0]
            insn_start = i - 1  # REX byte
            insn_len = 7  # rex+op+modrm+disp32
            tgt = (BASE + insn_start + insn_len + disp) & 0xFFFFFFFFFFFFFFFF
            if tgt == target:
                reg = ((rex & 1) << 3) | ((modrm >> 3) & 7)
                kind = "lea" if op == 0x8D else "mov"
                hits.append((BASE + insn_start, kind, reg))
    print(f"# rip-rel lea/mov -> 0x{target:x}: {len(hits)}")
    for va, k, reg in hits[:60]:
        print(f"  0x{va:x}  {k} reg={reg}")

if __name__ == "__main__":
    main()
