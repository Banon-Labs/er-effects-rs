#!/usr/bin/env python3
"""Resolve GameMan.character_name_is_empty offset: 0xe70 (upstream) vs 0xe78 (our code).

Scans the deobfuscated mapped image (file offset == RVA, base 0x140000000) for byte-field
accesses `[reg+disp32]` with disp32 in {0xe70, 0xe78}, then checks whether the base register
was loaded from the GameMan singleton global (0x143d69918) within a short window before the
access. Whichever displacement the GameMan accessors actually use is the ground-truth offset.
"""
import struct, os

BASE = 0x140000000
GAMEMAN_GLOBAL = 0x143d69918
IMG = os.path.join(os.path.dirname(__file__), "..", "eldenring-deobf.bin")
TARGET_DISPS = {0xE70: "0xe70 (upstream)", 0xE78: "0xe78 (our code)"}
WINDOW = 0x80  # bytes to look back for a GameMan-pointer load
REG_NAMES = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"]


def rip_load_target(data, modrm_pos):
    """If bytes at modrm_pos look like a RIP-rel ModRM (mod=00,rm=101), return (target, basereg)."""
    modrm = data[modrm_pos]
    if (modrm & 0xC7) != 0x05:  # mod=00, rm=101
        return None
    disp = struct.unpack_from("<i", data, modrm_pos + 1)[0]
    end = modrm_pos + 5
    tgt = (BASE + end + disp) & 0xFFFFFFFFFFFFFFFF
    reg = (modrm >> 3) & 7
    # account for REX.R extending reg (ignore; we only care rax..rdi here)
    return tgt, reg


def main():
    data = open(IMG, "rb").read()
    n = len(data)
    hits = {0xE70: [], 0xE78: []}
    for i in range(4, n - 4):
        disp = struct.unpack_from("<I", data, i)[0]
        if disp not in TARGET_DISPS:
            continue
        modrm = data[i - 1]
        if (modrm & 0xC0) != 0x80:  # need mod=10 (reg+disp32)
            continue
        rm = modrm & 7
        if rm in (4, 5):  # SIB or RIP form -> base not a plain reg
            continue
        va = BASE + (i - 1)
        # look back for a RIP-rel load of GAMEMAN_GLOBAL into ANY reg within WINDOW
        gm_load = None
        for j in range(max(0, i - 1 - WINDOW), i - 1):
            r = rip_load_target(data, j)
            if r and r[0] == GAMEMAN_GLOBAL:
                gm_load = (BASE + j, r[1])
                break
        opcode_ctx = data[max(0, i - 4):i - 1].hex()
        hits[disp].append((va, rm, opcode_ctx, gm_load))

    for disp, label in TARGET_DISPS.items():
        h = hits[disp]
        gm = [x for x in h if x[3] is not None]
        print(f"\n=== disp {label}: {len(h)} byte-accesses total, {len(gm)} with a GameMan load within {WINDOW:#x} ===")
        for va, rm, ctx, gmload in gm[:20]:
            gmva, gmreg = gmload
            print(f"  access@{va:#x} base={REG_NAMES[rm]} opbytes={ctx} <- GameMan loaded @{gmva:#x} into {REG_NAMES[gmreg]}")


if __name__ == "__main__":
    main()
