#!/usr/bin/env python3
"""Raw byte-pattern scan of eldenring-deobf.bin for the 4-byte little-endian
encoding of ToS/Privacy FMG text IDs appearing as imm32 operands. For each hit,
decode a small window with capstone to confirm it is a real instruction operand
and report the VA + instruction. file offset == RVA, base 0x140000000.

Run: uv run --with capstone python3 scripts/scan-textid-bytes.py
"""
import struct
import sys

BIN = "/home/banon/projects/er-effects-rs/eldenring-deobf.bin"
IMAGE_BASE = 0x140000000

IDS = {
    607200: "PRIVACY header",
    607201: "PRIVACY date",
    607202: "PRIVACY body",
    607100: "EULA header",
    607102: "EULA intro",
    607001: "Accept (EN)",
    607002: "Decline (EN)",
    607000: "footer (EN)",
    607004: "Switch Language",
    606300: "consent title (JP)",
    607300: "consent title (EN)",
    606000: "footer (JP)",
    606001: "Accept (JP)",
    606002: "Decline (JP)",
}

def main():
    try:
        from capstone import Cs, CS_ARCH_X86, CS_MODE_64
    except ImportError:
        import os
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

    data = open(BIN, "rb").read()
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    print(f"[scan] file size 0x{len(data):x}")

    for idv, label in IDS.items():
        needle = struct.pack("<I", idv)
        print(f"\n=== id {idv} (0x{idv:x}) {label} ===")
        start = 0
        n = 0
        while True:
            i = data.find(needle, start)
            if i < 0:
                break
            start = i + 1
            # try to decode an instruction starting a few bytes before this imm
            shown = False
            for back in range(1, 11):
                off = i - back
                if off < 0:
                    continue
                for insn in md.disasm(data[off:off+15], IMAGE_BASE + off):
                    # confirm this instruction actually covers our imm bytes
                    if insn.address <= IMAGE_BASE + i < insn.address + insn.size:
                        # confirm imm/disp value matches
                        ok = False
                        for op in insn.operands:
                            if op.type == 2 and (op.imm & 0xffffffff) == idv:
                                ok = True
                            if op.type == 3 and (op.mem.disp & 0xffffffff) == idv:
                                ok = True
                        if ok:
                            print(f"  VA 0x{insn.address:x}: {insn.mnemonic} {insn.op_str}")
                            shown = True
                    break
                if shown:
                    break
            if not shown:
                print(f"  (raw match at VA 0x{IMAGE_BASE+i:x}, not a clean imm operand)")
            n += 1
            if n >= 30:
                print("  ... (capped at 30)")
                break
        if n == 0:
            print("  (no byte matches)")

if __name__ == "__main__":
    main()
