"""Two-step vtable-call aware scan of ersc.dll .text, plus context-offset tracing.

Catches BOTH `call qword ptr [reg+disp]` and the split form
`mov r, qword ptr [reg+disp]` ... `call r`, which a single-instruction census misses.
Read-only.

Usage:
  uv run --with capstone python3 scripts/ersc_vcall_refute.py vcalls <disp>[,<disp>...]
  uv run --with capstone python3 scripts/ersc_vcall_refute.py loads <disp>[,<disp>...]
  uv run --with capstone python3 scripts/ersc_vcall_refute.py fnof <va>
"""
import sys, os, struct
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ersc_pe_refute import PE, pdata
from ersc_scan_refute import text_funcs
from capstone import Cs, CS_ARCH_X86, CS_MODE_64
from capstone.x86 import X86_OP_MEM, X86_OP_REG, X86_REG_RIP


def main():
    pe = PE()
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    fns = text_funcs(pe)
    mode = sys.argv[1]

    if mode == 'fnof':
        va = int(sys.argv[2], 0)
        rva = va - pe.imagebase
        for b, e in fns:
            if b <= rva < e:
                print('fn %#x .. %#x (size %#x)' % (pe.imagebase + b, pe.imagebase + e, e - b))
                return
        print('no pdata function covers %#x' % va)
        return

    want = set(int(x, 0) for x in sys.argv[2].split(','))

    for b, e in fns:
        o = pe.off(b)
        if o is None:
            continue
        insns = list(md.disasm(pe.data[o:o + (e - b)], pe.imagebase + b))
        # reg -> (disp, base_reg, addr) for a pending vtable-slot load
        pend = {}
        for i in insns:
            m = i.mnemonic
            ops = i.operands
            if mode == 'loads':
                for op in ops:
                    if op.type == X86_OP_MEM and op.mem.base not in (0, X86_REG_RIP) and op.mem.disp in want:
                        print('%#x  [fn %#x]  %s %s' % (i.address, pe.imagebase + b, m, i.op_str))
                        break
                continue
            # vcalls mode
            if m == 'call':
                op = ops[0]
                if op.type == X86_OP_MEM and op.mem.base not in (0, X86_REG_RIP) and op.mem.disp in want:
                    print('%#x  [fn %#x]  DIRECT  call %s' % (i.address, pe.imagebase + b, i.op_str))
                elif op.type == X86_OP_REG and op.reg in pend:
                    d, ldaddr, txt = pend[op.reg]
                    print('%#x  [fn %#x]  SPLIT   call %s   <- %#x %s'
                          % (i.address, pe.imagebase + b, i.reg_name(op.reg), ldaddr, txt))
            if m == 'mov' and len(ops) == 2 and ops[0].type == X86_OP_REG and ops[1].type == X86_OP_MEM:
                src = ops[1].mem
                if src.base not in (0, X86_REG_RIP) and src.disp in want:
                    pend[ops[0].reg] = (src.disp, i.address, i.op_str)
                else:
                    pend.pop(ops[0].reg, None)
            elif ops and ops[0].type == X86_OP_REG:
                pend.pop(ops[0].reg, None)


main()
