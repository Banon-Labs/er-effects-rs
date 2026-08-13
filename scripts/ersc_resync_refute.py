"""Resynchronizing whole-.text sweep of ersc.dll.

A from-function-start linear sweep silently drops everything after the first
undecodable byte (measured: 8.6% of .text, including the entire 0x10869-byte init
function). This sweep restarts at the next byte after every failure, so nothing is
dropped, and additionally does a raw-encoding search that cannot desync at all.

Read-only.

Usage:
  uv run --with capstone python3 scripts/ersc_resync_refute.py vcall <disp>[,<disp>...]
  uv run --with capstone python3 scripts/ersc_resync_refute.py raw <disp>
  uv run --with capstone python3 scripts/ersc_resync_refute.py xref <va>
"""
import sys, os, struct, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ersc_pe_refute import PE
from capstone import Cs, CS_ARCH_X86, CS_MODE_64
from capstone.x86 import X86_OP_MEM, X86_OP_REG, X86_REG_RIP

REGN = {0: 'rax', 1: 'rcx', 2: 'rdx', 3: 'rbx', 4: 'rsp', 5: 'rbp', 6: 'rsi', 7: 'rdi'}


def sweep(pe, md):
    """Yield every instruction decodable anywhere in .text, restarting on failure."""
    t = [s for s in pe.sections if s['name'] == '.text'][0]
    base = pe.imagebase + t['rva']
    data = pe.data[t['rawptr']:t['rawptr'] + t['rawsz']]
    pos = 0
    n = len(data)
    while pos < n:
        got = False
        for i in md.disasm(data[pos:], base + pos):
            got = True
            yield i
            pos = i.address - base + i.size
        if not got:
            pos += 1


def raw_search(pe, disp):
    """Byte-level search for `call qword ptr [reg+disp]` in every ModRM encoding.
    Cannot desync. Returns list of (va, base_reg_name, has_rex_b)."""
    t = [s for s in pe.sections if s['name'] == '.text'][0]
    base = pe.imagebase + t['rva']
    data = pe.data[t['rawptr']:t['rawptr'] + t['rawsz']]
    pats = []
    for rex in (None, 0x41, 0x49):          # none / REX.B / REX.WB
        for rm in range(8):
            if rm == 4:                      # needs SIB, skip simple form
                continue
            if -0x80 <= disp <= 0x7f:
                mod = 0x40
                enc = bytes([0xff, mod | (2 << 3) | rm, disp & 0xff])
            else:
                enc = b''
            if enc:
                pats.append(((bytes([rex]) if rex else b'') + enc, rm, rex))
            mod = 0x80
            enc32 = bytes([0xff, mod | (2 << 3) | rm]) + struct.pack('<i', disp)
            pats.append(((bytes([rex]) if rex else b'') + enc32, rm, rex))
    hits = []
    for pat, rm, rex in pats:
        i = 0
        while True:
            j = data.find(pat, i)
            if j < 0:
                break
            nm = REGN[rm]
            if rex:
                nm = 'r%d' % (8 + rm)
            hits.append((base + j, nm, len(pat)))
            i = j + 1
    hits.sort()
    return hits


def main():
    pe = PE()
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    mode = sys.argv[1]

    if mode == 'stats':
        n = 0
        cov = 0
        for i in sweep(pe, md):
            n += 1
            cov += i.size
        t = [s for s in pe.sections if s['name'] == '.text'][0]
        print('resync sweep: %d insns covering %#x of .text rawsz %#x (%.1f%%)'
              % (n, cov, t['rawsz'], 100.0 * cov / t['rawsz']))
        return

    if mode == 'raw':
        disp = int(sys.argv[2], 0)
        for va, nm, ln in raw_search(pe, disp):
            print('%#x  call qword ptr [%s + %#x]  (enc len %d)' % (va, nm, disp, ln))
        return

    if mode == 'rawcount':
        for disp in [int(x, 0) for x in sys.argv[2].split(',')]:
            print('%+#7x : %d raw encodings found' % (disp, len(raw_search(pe, disp))))
        return

    if mode == 'vcall':
        want = set(int(x, 0) for x in sys.argv[2].split(','))
        seen = set()
        for i in sweep(pe, md):
            if i.mnemonic != 'call':
                continue
            op = i.operands[0]
            if op.type == X86_OP_MEM and op.mem.base not in (0, X86_REG_RIP) and op.mem.disp in want:
                if i.address not in seen:
                    seen.add(i.address)
                    print('%#x  call %s' % (i.address, i.op_str))
        return

    if mode == 'xref':
        target = int(sys.argv[2], 0)
        seen = set()
        for i in sweep(pe, md):
            for op in i.operands:
                if op.type == X86_OP_MEM and op.mem.base == X86_REG_RIP:
                    if i.address + i.size + op.mem.disp == target and i.address not in seen:
                        seen.add(i.address)
                        print('%#x  %s %s' % (i.address, i.mnemonic, i.op_str))
        return

    raise SystemExit('unknown mode')


if __name__ == '__main__':
    main()
