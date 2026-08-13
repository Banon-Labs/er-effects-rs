"""Build a cross-reference / call-graph index over the PLAINTEXT .text of ersc.dll.

ersc.dll is Themida-wrapped but its original `.text` and `.pdata` survive in the
clear on disk, so we can disassemble every RUNTIME_FUNCTION and record:
  * rip-relative data references (and the C string at the target, if any)
  * direct `call`/`jmp` edges (call graph)
  * immediate constants

Writes a JSON index to $ERSC_INDEX (default /tmp/ersc_index.json).

    uv run --with capstone python3 scripts/ersc_xref.py            # build
    uv run --with capstone python3 scripts/ersc_xref.py --str BREAKIN
    uv run --with capstone python3 scripts/ersc_xref.py --callers 0x18008f4b0
"""
from __future__ import annotations

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ersc_pe as E  # noqa: E402

INDEX = os.environ.get('ERSC_INDEX', '/tmp/ersc_index.json')


def build(pe: E.PE):
    import capstone

    m = E.md()
    funcs = pe.pdata_funcs()
    idx = {}
    for s, e in funcs:
        b = pe.read(s, min(e - s, 0x10000))
        if not b:
            continue
        refs, calls, imms = [], [], []
        for ins in m.disasm(b, s):
            if ins.mnemonic in ('call', 'jmp') and ins.operands and \
               ins.operands[0].type == capstone.x86.X86_OP_IMM:
                calls.append(ins.operands[0].imm)
            for op in ins.operands:
                if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
                    refs.append((ins.address, ins.address + ins.size + op.mem.disp))
                elif op.type == capstone.x86.X86_OP_IMM and abs(op.imm) > 0x1000:
                    imms.append((ins.address, op.imm))
        strs = {}
        for _a, t in refs:
            if t in strs:
                continue
            c = pe.cstr(t, 300)
            if c and len(c) >= 4 and all(0x20 <= ch < 0x7F for ch in c):
                strs[t] = c.decode('latin1')
        idx[hex(s)] = {
            'end': hex(e),
            'refs': [[hex(a), hex(t)] for a, t in refs],
            'calls': sorted({hex(c) for c in calls}),
            'strs': {hex(k): v for k, v in strs.items()},
            'imms': [[hex(a), hex(i & 0xFFFFFFFFFFFFFFFF)] for a, i in imms],
        }
    return idx


def load(pe=None):
    if os.path.exists(INDEX):
        return json.load(open(INDEX))
    pe = pe or E.PE()
    idx = build(pe)
    json.dump(idx, open(INDEX, 'w'))
    return idx


if __name__ == '__main__':
    import argparse

    ap = argparse.ArgumentParser()
    ap.add_argument('--rebuild', action='store_true')
    ap.add_argument('--str', dest='needle')
    ap.add_argument('--callers')
    ap.add_argument('--calls')
    ap.add_argument('--refto')
    a = ap.parse_args()
    pe = E.PE()
    if a.rebuild or not os.path.exists(INDEX):
        idx = build(pe)
        json.dump(idx, open(INDEX, 'w'))
        print(f'built {len(idx)} functions -> {INDEX}')
    else:
        idx = json.load(open(INDEX))

    if a.needle:
        n = a.needle.lower()
        for f, v in idx.items():
            hits = [s for s in v['strs'].values() if n in s.lower()]
            if hits:
                print(f'{f}: {hits}')
    if a.callers:
        t = a.callers.lower()
        for f, v in idx.items():
            if t in [c.lower() for c in v['calls']]:
                print(f)
    if a.calls:
        print(idx[a.calls.lower()]['calls'])
    if a.refto:
        t = int(a.refto, 0)
        for f, v in idx.items():
            for _a, r in v['refs']:
                if int(r, 16) == t:
                    print(f, _a)
