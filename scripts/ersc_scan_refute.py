"""Whole-.text scan of ersc.dll: RIP-relative xref index + indirect-call census.

Read-only. Independent of any other helper module.
Usage: uv run --with capstone python3 scripts/ersc_scan_refute.py <mode> [args]
  xref <va>            -- every instruction whose RIP-relative operand targets <va>
  icalls               -- census of indirect call displacements across all .text funcs
  icalls_at <disp>     -- list every indirect call with that displacement
  imm <hex32>          -- every instruction containing that 4-byte immediate
"""
import sys, os, struct, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ersc_pe_refute import PE, pdata
from capstone import Cs, CS_ARCH_X86, CS_MODE_64
from capstone.x86 import X86_OP_MEM, X86_OP_IMM, X86_OP_REG, X86_REG_RIP


def text_funcs(pe):
    """Function ranges limited to .text, from BOTH the exception directory and the
    .pdata section, unioned (Themida rebuilt the directory)."""
    t = [s for s in pe.sections if s['name'] == '.text'][0]
    lo, hi = t['rva'], t['rva'] + t['vsz']
    fns = set()
    for b, e, u in pdata(pe):
        if lo <= b < hi:
            fns.add((b, min(e, hi)))
    s = [x for x in pe.sections if x['name'] == '.pdata'][0]
    for i in range(s['vsz'] // 12):
        b, e, u = struct.unpack_from('<III', pe.data, s['rawptr'] + 12 * i)
        if lo <= b < hi and e > b:
            fns.add((b, min(e, hi)))
    return sorted(fns)


def iter_insns(pe, md, fns):
    for b, e in fns:
        o = pe.off(b)
        if o is None:
            continue
        for i in md.disasm(pe.data[o:o + (e - b)], pe.imagebase + b):
            yield i


def main():
    pe = PE()
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    fns = text_funcs(pe)
    mode = sys.argv[1]

    if mode == 'stats':
        cov = sum(e - b for b, e in fns)
        t = [s for s in pe.sections if s['name'] == '.text'][0]
        print('funcs %d covering %#x of .text vsz %#x (%.1f%%)' % (len(fns), cov, t['vsz'], 100.0 * cov / t['vsz']))
        return

    if mode == 'xref':
        target = int(sys.argv[2], 0)
        for i in iter_insns(pe, md, fns):
            for op in i.operands:
                if op.type == X86_OP_MEM and op.mem.base == X86_REG_RIP:
                    if i.address + i.size + op.mem.disp == target:
                        print('%#x  %s %s' % (i.address, i.mnemonic, i.op_str))
        return

    if mode == 'icalls':
        c = collections.Counter()
        for i in iter_insns(pe, md, fns):
            if i.mnemonic != 'call':
                continue
            op = i.operands[0]
            if op.type == X86_OP_MEM and op.mem.base != X86_REG_RIP and op.mem.base != 0:
                c[op.mem.disp] += 1
        for d, n in sorted(c.items()):
            print('%+#7x  %d' % (d, n))
        return

    if mode == 'icalls_at':
        want = [int(x, 0) for x in sys.argv[2].split(',')]
        for i in iter_insns(pe, md, fns):
            if i.mnemonic != 'call':
                continue
            op = i.operands[0]
            if op.type == X86_OP_MEM and op.mem.base not in (0, X86_REG_RIP) and op.mem.disp in want:
                print('%#x  call %s' % (i.address, i.op_str))
        return

    if mode == 'imm':
        want = int(sys.argv[2], 0)
        for i in iter_insns(pe, md, fns):
            for op in i.operands:
                if op.type == X86_OP_IMM and (op.imm & 0xffffffff) == want:
                    print('%#x  %s %s' % (i.address, i.mnemonic, i.op_str))
                    break
        return

    if mode == 'memdisp':
        want = [int(x, 0) for x in sys.argv[2].split(',')]
        for i in iter_insns(pe, md, fns):
            for op in i.operands:
                if op.type == X86_OP_MEM and op.mem.base not in (0, X86_REG_RIP) and op.mem.disp in want:
                    print('%#x  %s %s' % (i.address, i.mnemonic, i.op_str))
                    break
        return

    raise SystemExit('unknown mode')


if __name__ == '__main__':
    main()
