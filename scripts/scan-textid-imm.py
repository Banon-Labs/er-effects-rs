#!/usr/bin/env python3
"""Scan eldenring-deobf.bin .text for instructions whose immediate/displacement
operand equals one of the ToS/Privacy FMG text IDs, and print the VA + bytes so
we can identify the dialog-builder functions. Pure-static, no game launch.

Run: uv run --with capstone python3 scripts/scan-textid-imm.py
"""
import struct
import sys

BIN = "/home/banon/projects/er-effects-rs/eldenring-deobf.bin"
IMAGE_BASE = 0x140000000

# Privacy-specific (607200/607201/607202), EULA (607100/607102), buttons/footer,
# consent dialog (606300/607300), and the JP/EN accept strings (606000..607004).
IDS = {
    607200: "PRIVACY header",
    607201: "PRIVACY date",
    607202: "PRIVACY body",
    607100: "EULA header",
    607102: "EULA intro",
    607001: "Accept (EN)",
    607002: "Decline (EN)",
    607000: "footer scroll/select/confirm/lang (EN)",
    607004: "Switch Language (EN)",
    606300: "consent title (JP)",
    607300: "consent title (EN)",
    606000: "footer (JP)",
    606001: "Accept (JP)",
}

def main():
    try:
        from capstone import Cs, CS_ARCH_X86, CS_MODE_64
    except ImportError:
        import os
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

    data = open(BIN, "rb").read()
    # The deobf bin is a flat image mapped at IMAGE_BASE. Find .text by PE headers.
    pe = data.find(b"PE\x00\x00")
    # Parse section table to get .text range
    import_off = pe + 4
    num_sec = struct.unpack_from("<H", data, import_off + 2)[0]
    opt_size = struct.unpack_from("<H", data, import_off + 16)[0]
    sec_tab = import_off + 20 + opt_size
    text_lo = text_hi = None
    for i in range(num_sec):
        off = sec_tab + i * 40
        name = data[off:off+8].rstrip(b"\x00")
        vsize = struct.unpack_from("<I", data, off+8)[0]
        vaddr = struct.unpack_from("<I", data, off+12)[0]
        rawsize = struct.unpack_from("<I", data, off+16)[0]
        if name == b".text":
            text_lo = vaddr
            text_hi = vaddr + max(vsize, rawsize)
            print(f"[scan] .text RVA 0x{vaddr:x} size 0x{max(vsize,rawsize):x}")
    if text_lo is None:
        print("no .text found; scanning whole file as flat")
        text_lo, text_hi = 0x1000, len(data)

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True

    # In a flat-mapped deobf image, file offset == RVA (section-aligned dump).
    blob = data[text_lo:text_hi]
    base_va = IMAGE_BASE + text_lo
    hits = {k: [] for k in IDS}
    count = 0
    for insn in md.disasm(blob, base_va):
        count += 1
        for op in insn.operands:
            val = None
            if op.type == 2:  # IMM
                val = op.imm & 0xffffffffffffffff
            elif op.type == 3:  # MEM disp
                val = op.mem.disp & 0xffffffffffffffff
            if val in IDS:
                hits[val].append((insn.address, insn.mnemonic, insn.op_str))
    print(f"[scan] decoded {count} instructions")
    for idv, label in IDS.items():
        hs = hits[idv]
        print(f"\n=== id {idv} ({label}) : {len(hs)} hits ===")
        for addr, mn, ops in hs[:20]:
            print(f"  0x{addr:x}: {mn} {ops}")

if __name__ == "__main__":
    main()
