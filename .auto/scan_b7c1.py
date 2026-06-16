#!/usr/bin/env python3
"""Scan .text for struct-offset writes to [reg+0xb7c1] (byte) and [reg+0xb7c4] (dword).
Read-only. Reports source VA + the immediate written + nearby disasm hint."""
import struct, sys
from pathlib import Path
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre

EXE = sre.DEFAULT_EXE
data = EXE.read_bytes()
image_base, sections = sre.parse_pe(data)
text = next(s for s in sections if s["name"] == ".text")
raw_ptr = int(text["raw_ptr"]); raw_size = int(text["raw_size"])
text_va = image_base + int(text["virtual_address"])
td = data[raw_ptr:raw_ptr+raw_size]
n = len(td)

# disp32 little-endian for the two offsets
targets = {0xb7c1: "byte_b7c1", 0xb7c4: "dword_b7c4"}

def hexb(b): return " ".join(f"{x:02x}" for x in b)

for off, label in targets.items():
    disp = struct.pack("<i", off)  # 4 bytes
    needle = disp  # 00 00 b7 c1 ... actually off=0xb7c1 -> bytes c1 b7 00 00
    start = 0
    found = 0
    while True:
        idx = td.find(needle, start)
        if idx < 0: break
        start = idx + 1
        # the disp32 is preceded by a modrm byte; look back for opcode forms
        # We want writes:
        #   C6 80/81/82/83/85/86/87 <disp32> imm8   (mov byte [reg+disp32], imm8)  -> only b7c1
        #   C7 80.. <disp32> imm32                  (mov dword [reg+disp32], imm32) -> b7c4
        #   88 8x <disp32>                          (mov byte [reg+disp32], reg8)
        #   89 8x <disp32>                          (mov dword [reg+disp32], reg32)
        #   80 /x imm group  etc.
        if idx < 3: continue
        modrm = td[idx-1]
        op = td[idx-2]
        mod = modrm >> 6
        rm = modrm & 7
        if not (mod == 2):  # disp32 form mod=10
            continue
        # optional REX before op
        prefix = td[idx-3] if idx>=3 else 0
        va = text_va + (idx-2)
        if prefix and 0x40 <= prefix <= 0x4f:
            va = text_va + (idx-3)
        # classify
        kind=None; imm=None
        if op == 0xC6:
            imm = td[idx+4]; kind=f"mov byte [reg+{hex(off)}],{imm}"
        elif op == 0xC7:
            imm = struct.unpack_from("<i", td, idx+4)[0]; kind=f"mov dword [reg+{hex(off)}],{imm}"
        elif op == 0x88:
            kind=f"mov byte [reg+{hex(off)}],reg8"
        elif op == 0x89:
            kind=f"mov dword [reg+{hex(off)}],reg32"
        elif op == 0x80:
            imm=td[idx+4]; kind=f"grp1 byte [reg+{hex(off)}],{imm}"
        elif op == 0x83:
            imm=td[idx+4]; kind=f"grp1 dword [reg+{hex(off)}],{imm}"
        elif op in (0x00,0x08,0x20,0x28,0x30):
            kind=f"rmw byte [reg+{hex(off)}] op{op:02x}"
        elif op == 0x01:
            kind=f"add dword [reg+{hex(off)}],reg"
        else:
            kind=f"op{op:02x} (modrm {modrm:02x})"
        ctx = hexb(td[max(0,idx-4):idx+6])
        print(f"{label}: VA={va:#x} {kind}  [{ctx}]")
        found+=1
    if not found:
        print(f"{label}: NO struct-offset writes found")
