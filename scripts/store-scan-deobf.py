#!/usr/bin/env python3
"""Find RIP-relative WRITERS (mov [rip+disp32], reg) of a target global VA in the deobfuscated ER
mapped image. Mapped image: file offset == RVA, base 0x140000000.

Matches `[REX.W] 89 modrm disp32` where modrm encodes mod=00, rm=101 (RIP-relative) -- i.e.
mov [rip+disp32], r64. Also matches `[REX] 88 ...` (byte store) and `C7 ...` (mov imm).
Reports the VA of each store instruction.
Usage: store-scan-deobf.py 0x143d76060
"""
import sys, struct, os

BASE = 0x140000000
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")
RIPREL_MODRM = {0x05, 0x0D, 0x15, 0x1D, 0x2D, 0x35, 0x3D}  # mod=00, rm=101, reg=0..7
REXW = {0x48, 0x49, 0x4C, 0x4D, 0x40, 0x41, 0x44, 0x45}


def main():
    target = int(sys.argv[1], 16)
    data = open(IMG, "rb").read()
    n = len(data)
    hits = []

    def check(i, opcode, insn_len, kind):
        # store with RIP-relative modrm at byte i (the opcode byte), disp32 right after modrm.
        modrm = data[i + 1]
        if modrm not in RIPREL_MODRM:
            return
        disp = struct.unpack_from("<i", data, i + 2)[0]
        tgt = (BASE + (i - 1) + insn_len + disp) & 0xFFFFFFFFFFFFFFFF  # i-1 = REX byte start
        if tgt == target:
            hits.append((BASE + (i - 1), kind))

    for i in range(1, n - 7):
        rex = data[i - 1]
        if rex not in REXW:
            continue
        op = data[i]
        if op == 0x89:  # mov [rip+d], r64
            check(i, op, 7, "mov_r64")
        elif op == 0x88:  # mov [rip+d], r8
            check(i, op, 7, "mov_r8")
        elif op == 0xC7:  # mov [rip+d], imm32 (modrm then imm32 -> insn_len 8)
            modrm = data[i + 1]
            if modrm in RIPREL_MODRM:
                disp = struct.unpack_from("<i", data, i + 2)[0]
                tgt = (BASE + (i - 1) + 10 + disp) & 0xFFFFFFFFFFFFFFFF
                if tgt == target:
                    hits.append((BASE + (i - 1), "mov_imm"))

    print(f"# RIP-relative stores -> 0x{target:x}: {len(hits)}")
    for va, k in hits[:40]:
        print(f"  0x{va:x}  {k}")


if __name__ == "__main__":
    main()
