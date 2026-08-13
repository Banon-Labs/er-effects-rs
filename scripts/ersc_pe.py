"""Minimal PE reader + capstone disassembler helper for Seamless Co-op's ersc.dll.

ersc.dll is Themida-wrapped, but only the entry stub and the `.themida` section
(VA 0x18023a000, 0x546000 bytes) are packed -- the original `.text` section
(RVA 0x1000, 0x18c626 bytes) is PLAINTEXT on disk and disassembles cleanly, and
the original `.pdata` (RVA 0x21d000) still carries the RUNTIME_FUNCTION table.

Run under uv so capstone is provisioned:
    uv run --with capstone python3 scripts/ersc_pe.py --help

Env overrides (no hard-coded user paths):
    ERSC_DLL  -- path to ersc.dll (default: the me3-profile copy, then the
                 Steam game-install copy)
"""
from __future__ import annotations

import os
import struct

BASE = 0x180000000

_CANDIDATES = [
    os.environ.get('ERSC_DLL'),
    os.path.join(os.path.expanduser('~'), 'Elden', 'ersc.dll'),
    os.path.join(
        os.path.expanduser('~'),
        '.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll',
    ),
]


def default_path() -> str:
    for c in _CANDIDATES:
        if c and os.path.exists(c):
            return c
    raise FileNotFoundError('ersc.dll not found; set ERSC_DLL')


class PE:
    def __init__(self, path: str | None = None):
        self.path = path or default_path()
        self.d = open(self.path, 'rb').read()
        d = self.d
        e = struct.unpack_from('<I', d, 0x3C)[0]
        self.nsec = struct.unpack_from('<H', d, e + 6)[0]
        optsz = struct.unpack_from('<H', d, e + 20)[0]
        self.opt = e + 24
        self.base = struct.unpack_from('<Q', d, self.opt + 24)[0]
        self.secs = []
        for i in range(self.nsec):
            o = e + 24 + optsz + i * 40
            nm = d[o:o + 8].rstrip(b'\x00').decode('latin1')
            vs, va, rs, ra = struct.unpack_from('<IIII', d, o + 8)
            self.secs.append((nm, va, vs, ra, rs))

    # -- address translation -------------------------------------------------
    def r2o(self, rva: int):
        for nm, va, vs, ra, rs in self.secs:
            if va <= rva < va + max(vs, rs):
                off = ra + (rva - va)
                if off < len(self.d):
                    return off
        return None

    def va2o(self, va: int):
        return self.r2o(va - self.base)

    def read(self, va: int, n: int):
        o = self.va2o(va)
        return None if o is None else self.d[o:o + n]

    def u32(self, va: int):
        b = self.read(va, 4)
        return None if not b or len(b) < 4 else struct.unpack('<I', b)[0]

    def u64(self, va: int):
        b = self.read(va, 8)
        return None if not b or len(b) < 8 else struct.unpack('<Q', b)[0]

    def sec_of(self, va: int):
        rva = va - self.base
        for nm, sva, vs, ra, rs in self.secs:
            if sva <= rva < sva + max(vs, rs):
                return nm
        return None

    def cstr(self, va: int, maxlen: int = 260):
        b = self.read(va, maxlen)
        if not b:
            return None
        z = b.find(b'\x00')
        return b[:z if z >= 0 else maxlen]

    # -- function table ------------------------------------------------------
    def pdata_funcs(self):
        """(start_va, end_va) from the ORIGINAL .pdata section."""
        out = []
        for nm, va, vs, ra, rs in self.secs:
            if nm == '.pdata':
                for i in range(vs // 12):
                    s, e_, _u = struct.unpack_from('<III', self.d, ra + i * 12)
                    if s:
                        out.append((self.base + s, self.base + e_))
                break
        return sorted(out)

    def func_of(self, va: int):
        for s, e in self.pdata_funcs():
            if s <= va < e:
                return (s, e)
        return None


def md():
    import capstone
    m = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    m.detail = True
    return m


def disas(pe: PE, va: int, n: int = 0x100, m=None):
    m = m or md()
    b = pe.read(va, n)
    return [] if b is None else list(m.disasm(b, va))


def disas_func(pe: PE, va: int, m=None, cap: int = 0x4000):
    f = pe.func_of(va)
    n = min(f[1] - va, cap) if f else cap
    return disas(pe, va, n, m)


def fmt(insns) -> str:
    return '\n'.join(
        f'{i.address:012x}  {i.bytes.hex():<22s} {i.mnemonic} {i.op_str}' for i in insns
    )


if __name__ == '__main__':
    import argparse

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('va', nargs='?', help='virtual address, e.g. 0x18008f4b0')
    ap.add_argument('-n', type=lambda s: int(s, 0), default=0x100)
    ap.add_argument('--func', action='store_true', help='disassemble the whole .pdata function')
    ap.add_argument('--info', action='store_true', help='print section/function-table summary')
    a = ap.parse_args()
    pe = PE()
    if a.info or not a.va:
        print(f'{pe.path}  base={pe.base:#x}')
        for nm, va, vs, ra, rs in pe.secs:
            print(f'  {nm:10s} VA={pe.base + va:#012x} VS={vs:#x} RA={ra:#x} RS={rs:#x}')
        fs = pe.pdata_funcs()
        print(f'  .pdata functions: {len(fs)}  [{fs[0][0]:#x} .. {fs[-1][1]:#x}]')
    if a.va:
        va = int(a.va, 0)
        ins = disas_func(pe, va) if a.func else disas(pe, va, a.n)
        print(fmt(ins))
