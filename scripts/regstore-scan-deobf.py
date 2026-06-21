#!/usr/bin/env python3
"""Find REGISTER-relative 64-bit stores `mov [base+disp32], reg64` to a given struct offset
in the dearxan-DEOBFUSCATED ER mapped image. Complements store-scan-deobf.py (which only
finds RIP-relative writers of a *global* VA). Use this to locate where a struct field at a
known offset (e.g. CS::MenuWindowJob +0xa8 / +0xe8) is written.

Matches `[REX.W] 89 modrm [SIB] disp32` with mod=10 (disp32) and disp32 == target offset.
Covers both the no-SIB form (rm != 100) and the SIB form (rm == 100). Reports the VA of the
opcode/REX byte and the decoded base/index/source registers for triage.

Usage: regstore-scan-deobf.py 0xa8 [0xe8 ...]
"""
import sys, struct, os

BASE = 0x140000000
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")
REXW = {0x48, 0x49, 0x4C, 0x4D}  # REX.W with optional R/X/B bits
REG = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"]


def regname(idx, ext):
    n = idx + (8 if ext else 0)
    base8 = REG[idx] if not ext else None
    return base8 if base8 else f"r{n}"


def main():
    targets = [int(a, 16) for a in sys.argv[1:]] or [0xA8]
    data = open(IMG, "rb").read()
    n = len(data)
    for target in targets:
        hits = []
        for i in range(1, n - 9):
            if data[i - 1] not in REXW:
                continue
            if data[i] != 0x89:  # mov [r/m], r64
                continue
            modrm = data[i + 1]
            if (modrm & 0xC0) != 0x80:  # require mod=10 (disp32 follows)
                continue
            rm = modrm & 0x07
            reg = (modrm >> 3) & 0x07
            rex = data[i - 1]
            rex_r = (rex >> 2) & 1
            rex_b = rex & 1
            if rm == 0x04:  # SIB byte present, then disp32
                disp = struct.unpack_from("<i", data, i + 3)[0]
                if disp == target:
                    sib = data[i + 2]
                    base = sib & 0x07
                    hits.append((BASE + (i - 1), regname(reg, rex_r), f"sib base={REG[base]}+idx"))
            else:
                disp = struct.unpack_from("<i", data, i + 2)[0]
                if disp == target:
                    hits.append((BASE + (i - 1), regname(reg, rex_r), f"[{regname(rm, rex_b)}+0x{target:x}]"))
        print(f"# mov [base+0x{target:x}], reg64 : {len(hits)} hits")
        for va, src, dst in hits[:200]:
            print(f"  0x{va:x}  mov {dst}, {src}")


if __name__ == "__main__":
    main()
