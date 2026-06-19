#!/usr/bin/env python3
"""Scan the deobfuscated ER mapped image for ALL RIP-relative references to a target VA.
Catches lea/mov/cmp r,[rip+disp32] (any opcode with a RIP-relative ModRM mod=00 rm=101).
Mapped image: file offset == RVA, base 0x140000000.
Brute-force: for every position where ModRM byte is a RIP-rel form (0x05,0x0D,..0x3D),
compute target assuming the disp32 follows ModRM and the instruction ends right after disp32.
Because instruction length varies, we test a range of plausible instruction-end offsets
(ModRM+disp32 is 5 bytes; insn ends at modrm_pos+5+trailing_imm). We report any position
whose RIP-rel target == requested VA for end at modrm_pos+5 (most lea/mov/cmp with no imm).
Usage: riprefscan-deobf.py 0x143d703e0
"""
import sys, struct, os

BASE = 0x140000000
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")
RIPREL = {0x05, 0x0D, 0x15, 0x1D, 0x25, 0x2D, 0x35, 0x3D}


def main():
    target = int(sys.argv[1], 16)
    data = open(IMG, "rb").read()
    n = len(data)
    hits = []
    # For end = modrm_pos + 5 (no trailing imm) and end = +6,+7,+8,+9 (imm8/imm16/imm32)
    for modrm_pos in range(n - 6):
        if data[modrm_pos] not in RIPREL:
            continue
        disp = struct.unpack_from("<i", data, modrm_pos + 1)[0]
        for trail in (0, 1, 2, 4):
            end = modrm_pos + 5 + trail
            tgt = (BASE + end + disp) & 0xFFFFFFFFFFFFFFFF
            if tgt == target:
                hits.append((modrm_pos, trail))
                break
    print(f"# RIP-relative refs -> 0x{target:x}: {len(hits)}")
    for modrm_pos, trail in hits:
        # show a few bytes before modrm to identify opcode
        ctx = data[max(0, modrm_pos - 4):modrm_pos]
        print(f"  modrm@0x{BASE+modrm_pos:x} trail={trail} prefix_bytes={ctx.hex()}")


if __name__ == "__main__":
    main()
