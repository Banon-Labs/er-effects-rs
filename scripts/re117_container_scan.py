#!/usr/bin/env python3
"""Scan a flat ER image for the ChrIns->moduleContainer->module idiom.

Finds `mov r64, [rX + 0x190]` and, when the very next instruction dereferences the same
destination register with a displacement, records that displacement. The histogram of
displacements is the observed ChrInsModuleContainer layout for that build.
"""
import sys, os, collections
from capstone import Cs, CS_ARCH_X86, CS_MODE_64, x86_const

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGES = {"1162": os.path.join(ROOT, "eldenring-deobf.bin"),
          "1170": os.path.join(ROOT, "eldenring-deobf-1.17.bin")}
BASE = 0x140000000
CONTAINER_OFF = int(sys.argv[2], 16) if len(sys.argv) > 2 else 0x190

def main():
    which = sys.argv[1]
    data = open(IMAGES[which], "rb").read()
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    # mov r64, [r64 + disp32] where disp32 == CONTAINER_OFF
    needle = bytes([0x48, 0x8B]) + b"\x00"  # placeholder, we match modrm loosely below
    disp = CONTAINER_OFF.to_bytes(4, "little")
    hist = collections.Counter()
    sites = collections.defaultdict(list)
    total = 0
    i = 0
    while True:
        i = data.find(disp, i + 1)
        if i < 0:
            break
        # candidate instruction starts 3 or 4 bytes before the disp (REX + 8B + modrm [+ sib])
        for back in (3, 4):
            start = i - back
            if start < 0:
                continue
            insns = list(md.disasm(data[start:start + 16], BASE + start))
            if not insns:
                continue
            a = insns[0]
            if a.mnemonic != "mov" or len(a.operands) != 2:
                continue
            dst, src = a.operands
            if dst.type != x86_const.X86_OP_REG or src.type != x86_const.X86_OP_MEM:
                continue
            if src.mem.disp != CONTAINER_OFF or src.mem.index != 0:
                continue
            if a.size != back + 4:
                continue
            total += 1
            if len(insns) < 2:
                break
            b = insns[1]
            if b.mnemonic != "mov" or len(b.operands) != 2:
                break
            d2, s2 = b.operands
            if s2.type != x86_const.X86_OP_MEM or s2.mem.base != dst.reg or s2.mem.index != 0:
                break
            hist[s2.mem.disp] += 1
            if len(sites[s2.mem.disp]) < 3:
                sites[s2.mem.disp].append(hex(a.address))
            break
    print(f"# image={which} containerOff=0x{CONTAINER_OFF:x} idiom-sites={total}")
    for off in sorted(hist):
        print(f"  +0x{off:<4x}  n={hist[off]:<5d}  e.g. {', '.join(sites[off])}")

main()
