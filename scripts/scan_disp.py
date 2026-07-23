#!/usr/bin/env python3
"""Scan .text for rip-agnostic stores to [reg+disp] for a given disp32.
Reports mov reg->mem (89 /r), mov imm32->mem (C7 /0), add (01 /r) with that disp.
Read-only."""
import sys, struct
from pathlib import Path
HERE = Path(__file__).resolve().parent.parent / ".auto"
sys.path.insert(0, str(HERE))
import static_re_export as sre

disp_target = int(sys.argv[1], 16)
data = sre.DEFAULT_EXE.read_bytes()
ib, secs = sre.parse_pe(data)
text = [s for s in secs if s["name"] == ".text"][0]
raw = int(text["raw_ptr"]); rs = int(text["raw_size"]); tva = ib + int(text["virtual_address"])
td = data[raw:raw+rs]
n = len(td)
needle = struct.pack("<i", disp_target)
idx = 0
out = []
while True:
    j = td.find(needle, idx)
    if j < 0:
        break
    idx = j + 1
    # modrm with mod=10 (disp32) precedes the 4-byte disp at j. modrm at j-1.
    if j < 2:
        continue
    modrm = td[j-1]
    mod = modrm >> 6
    reg = (modrm >> 3) & 7
    rm = modrm & 7
    if mod != 2:  # need disp32 form
        continue
    # opcode is before modrm, possibly with REX
    op_pos = j - 2
    rex = 0
    if op_pos >= 0 and 0x40 <= td[op_pos] <= 0x4f:
        pass
    op = td[op_pos]
    # account for REX before op
    rex_pos = op_pos - 1
    has_rex = rex_pos >= 0 and 0x40 <= td[rex_pos] <= 0x4f
    start = rex_pos if has_rex else op_pos
    va = tva + start
    mn = {0x89: "mov m,r", 0xC7: "mov m,imm32", 0x01: "add m,r", 0x8B: "mov r,m(load)", 0x83: "grp1 m,imm8", 0x3B:"cmp"}.get(op, f"op{op:02x}")
    imm = None
    if op == 0xC7:
        imm = struct.unpack_from("<i", td, j+4)[0]
    fr = sre.find_pdata_range_for_pc(data, ib, secs, va)
    out.append((va, mn, reg, rm, imm, fr.get("begin_va")))

for va, mn, reg, rm, imm, fn in sorted(out):
    imms = f" imm={hex(imm)}" if imm is not None else ""
    print(f"{hex(va)}  {mn} [r{rm}+{hex(disp_target)}] reg={reg}{imms}  fn={fn}")
