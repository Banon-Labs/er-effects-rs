#!/usr/bin/env python3
"""Complete byte-store / byte-op writer scan for a single global VA (recon only).

The existing recon_refs.py only covers C6 /0 (movb imm) and a few movq/lea forms.
This scanner is exhaustive for *byte-sized* rip-relative writes & RMW ops to a
target VA, which is what we need to settle whether 0x143d856a0 has a writer other
than the known immediate-1 latcher 0x140c8ff41:

  88 /r   MOV  byte [rip+disp32], reg8   (low regs al/cl/dl/bl/spl/bpl/sil/dil)
  8A /r   MOV  reg8, byte [rip+disp32]   (READ form, reported for completeness)
  C6 /0   MOV  byte [rip+disp32], imm8
  00 /r   ADD  byte [rip+disp32], reg8
  08 /r   OR   byte [rip+disp32], reg8
  20 /r   AND  byte [rip+disp32], reg8
  28 /r   SUB  byte [rip+disp32], reg8
  30 /r   XOR  byte [rip+disp32], reg8
  80 /0..7 imm8 group1 (ADD/OR/ADC/SBB/AND/SUB/XOR/CMP) byte [rip+disp32], imm8
  F6 /2,/3 NOT/NEG byte [rip+disp32]
  FE /0,/1 INC/DEC byte [rip+disp32]

Also handles the REX-prefixed (40-47, esp. 44/45 for r8b-r15b) variants of the
88/8A/00/08/20/28/30 reg forms.

For each: source VA, the modrm reg (writer source reg), op mnemonic, and the
containing function range. Read-only; never launches the game.

Usage: python3 .auto/recon_byte_store_scan.py <hex_target_va>
"""
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE

REG8 = ["al", "cl", "dl", "bl", "spl", "bpl", "sil", "dil"]
REG8_R = ["r8b", "r9b", "r10b", "r11b", "r12b", "r13b", "r14b", "r15b"]
GROUP1 = ["add", "or", "adc", "sbb", "and", "sub", "xor", "cmp"]


def reg_name(rex_r: int, reg_field: int) -> str:
    return (REG8_R if rex_r else REG8)[reg_field]


def decode_at(text_data: bytes, i: int):
    """Return (mnemonic, is_write, insn_len, detail) if a rip-rel byte op with
    modrm mod=00 reg/mem=101 (i.e. [rip+disp32]) starts at i, else None."""
    n = len(text_data)
    p = i
    rex = 0
    rex_r = 0
    # optional single REX prefix
    if p < n and 0x40 <= text_data[p] <= 0x4F:
        rex = text_data[p]
        rex_r = (rex >> 2) & 1
        p += 1
    if p >= n:
        return None
    op = text_data[p]
    p += 1
    if p >= n:
        return None
    modrm = text_data[p]
    mod = modrm >> 6
    reg = (modrm >> 3) & 7
    rm = modrm & 7
    # require rip-relative: mod=00, rm=101
    if not (mod == 0 and rm == 5):
        return None
    p += 1  # past modrm; disp32 follows
    disp_off = p

    def mk(mn, is_write, detail):
        insn_len = (disp_off + 4) - i
        return (mn, is_write, insn_len, detail, disp_off)

    if op == 0x88:
        return mk(f"mov byte [rip],{reg_name(rex_r,reg)}", True, "reg_store")
    if op == 0x8A:
        return mk(f"mov {reg_name(rex_r,reg)},byte [rip]", False, "reg_load")
    if op == 0xC6 and reg == 0:
        # C6 /0 has a trailing imm8 after disp32 (total len = disp_off+4+1 - i)
        insn_len = (disp_off + 4 + 1) - i
        return ("mov byte [rip],imm8", True, insn_len, "imm_store", disp_off)
    if op == 0x00:
        return mk(f"add byte [rip],{reg_name(rex_r,reg)}", True, "reg_add")
    if op == 0x08:
        return mk(f"or byte [rip],{reg_name(rex_r,reg)}", True, "reg_or")
    if op == 0x20:
        return mk(f"and byte [rip],{reg_name(rex_r,reg)}", True, "reg_and")
    if op == 0x28:
        return mk(f"sub byte [rip],{reg_name(rex_r,reg)}", True, "reg_sub")
    if op == 0x30:
        return mk(f"xor byte [rip],{reg_name(rex_r,reg)}", True, "reg_xor")
    if op == 0x80:
        mn = GROUP1[reg]
        is_write = reg != 7  # cmp is read-only
        # group1 imm8 has an extra imm byte after disp32; len accounted below
        insn_len = (disp_off + 4 + 1) - i
        return (f"{mn} byte [rip],imm8", is_write, insn_len, "imm_g1", disp_off)
    if op == 0xF6 and reg in (2, 3):
        return mk(("not" if reg == 2 else "neg") + " byte [rip]", True, "rmw")
    if op == 0xFE and reg in (0, 1):
        return mk(("inc" if reg == 0 else "dec") + " byte [rip]", True, "rmw")
    return None


def main() -> None:
    target = int(sys.argv[1], 16)
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    text = next(s for s in sections if s["name"] == ".text")
    raw_ptr = int(text["raw_ptr"])
    raw_size = int(text["raw_size"])
    text_va = image_base + int(text["virtual_address"])
    td = data[raw_ptr:raw_ptr + raw_size]
    n = len(td)

    hits = []
    for i in range(n - 6):
        res = decode_at(td, i)
        if res is None:
            continue
        mn, is_write, insn_len, detail, disp_off = res
        disp = struct.unpack_from("<i", td, disp_off)[0]
        src_va = text_va + i
        resolved = src_va + insn_len + disp
        if resolved != target:
            continue
        fr = sre.find_pdata_range_for_pc(data, image_base, sections, src_va)
        imm = None
        if detail in ("imm_store", "imm_g1"):
            imm_pos = disp_off + 4
            if imm_pos < n:
                imm = td[imm_pos]
        hits.append({
            "src_va": src_va, "mn": mn, "write": is_write, "detail": detail,
            "imm": imm, "len": insn_len, "bytes": td[i:i + insn_len].hex(),
            "fn_begin": fr.get("begin_va"), "fn_end": fr.get("end_va"),
        })

    writers = [h for h in hits if h["write"]]
    readers = [h for h in hits if not h["write"]]
    print(f"# target {hex(target)}: {len(hits)} byte-op refs ({len(writers)} write, {len(readers)} read)")
    print("## WRITERS")
    for h in sorted(writers, key=lambda x: x["src_va"]):
        imms = f" imm=0x{h['imm']:02x}" if h["imm"] is not None else ""
        print(f"  {hex(h['src_va'])}  {h['mn']}{imms}  bytes={h['bytes']}  fn={h['fn_begin']}..{h['fn_end']}")
    print("## READERS (byte form)")
    for h in sorted(readers, key=lambda x: x["src_va"]):
        print(f"  {hex(h['src_va'])}  {h['mn']}  bytes={h['bytes']}  fn={h['fn_begin']}..{h['fn_end']}")


if __name__ == "__main__":
    main()
