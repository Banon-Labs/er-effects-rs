"""Disassemble a VA range of ersc.dll. Read-only.

Usage: uv run --with capstone python3 scripts/ersc_dis_refute.py 0x18002c680 0x220
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ersc_pe_refute import PE
from capstone import Cs, CS_ARCH_X86, CS_MODE_64


def dis(va, n, pe=None):
    pe = pe or PE()
    o = pe.voff(va)
    if o is None:
        print('VA %#x has no file backing (virtual-only / outside sections)' % va)
        return
    b = pe.data[o:o + n]
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = False
    last = va
    for i in md.disasm(b, va):
        print('%#012x  %-24s %s %s' % (i.address, i.bytes.hex(), i.mnemonic, i.op_str))
        last = i.address + i.size
    if last < va + n:
        print('--- decode STOPPED at %#x (offset +%#x of %#x) ---' % (last, last - va, n))


if __name__ == '__main__':
    dis(int(sys.argv[1], 0), int(sys.argv[2], 0))
