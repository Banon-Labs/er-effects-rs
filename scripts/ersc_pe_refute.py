"""Independent read-only PE reader for ersc.dll (Seamless Co-op).

Written to re-derive claims about the Steam matchmaking callback path from raw
bytes without reusing another agent's helper modules. READ ONLY: never writes to
the DLL.
"""
import struct

PATH = '/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll'


class PE:
    def __init__(self, path=PATH):
        self.data = open(path, 'rb').read()
        d = self.data
        e_lfanew = struct.unpack_from('<I', d, 0x3c)[0]
        assert d[e_lfanew:e_lfanew + 4] == b'PE\0\0', 'not PE'
        coff = e_lfanew + 4
        self.machine, self.nsec, _, _, _, self.optsz, self.chars = struct.unpack_from('<HHIIIHH', d, coff)
        opt = coff + 20
        self.magic = struct.unpack_from('<H', d, opt)[0]
        assert self.magic == 0x20b, hex(self.magic)
        self.imagebase = struct.unpack_from('<Q', d, opt + 24)[0]
        self.sizeofimage = struct.unpack_from('<I', d, opt + 56)[0]
        self.nrva = struct.unpack_from('<I', d, opt + 108)[0]
        self.dirs = []
        for i in range(self.nrva):
            rva, sz = struct.unpack_from('<II', d, opt + 112 + 8 * i)
            self.dirs.append((rva, sz))
        sh = opt + self.optsz
        self.sections = []
        for i in range(self.nsec):
            name = d[sh + 40 * i: sh + 40 * i + 8].rstrip(b'\0').decode('latin1')
            vsz, va, rawsz, rawptr = struct.unpack_from('<IIII', d, sh + 40 * i + 8)
            ch = struct.unpack_from('<I', d, sh + 40 * i + 36)[0]
            self.sections.append(dict(name=name, vsz=vsz, rva=va, rawsz=rawsz, rawptr=rawptr, chars=ch))

    def sec_of_rva(self, rva):
        for s in self.sections:
            if s['rva'] <= rva < s['rva'] + max(s['vsz'], s['rawsz']):
                return s
        return None

    def sec_of_va(self, va):
        return self.sec_of_rva(va - self.imagebase)

    def off(self, rva):
        s = self.sec_of_rva(rva)
        if s is None:
            return None
        delta = rva - s['rva']
        if delta >= s['rawsz']:
            return None
        return s['rawptr'] + delta

    def voff(self, va):
        return self.off(va - self.imagebase)

    def read_va(self, va, n):
        o = self.voff(va)
        return None if o is None else self.data[o:o + n]

    def u32va(self, va):
        b = self.read_va(va, 4)
        return None if b is None else struct.unpack('<I', b)[0]

    def u64va(self, va):
        b = self.read_va(va, 8)
        return None if b is None else struct.unpack('<Q', b)[0]


def pdata(pe):
    rva, sz = pe.dirs[3]
    o = pe.off(rva)
    out = []
    for i in range(sz // 12):
        b, e, u = struct.unpack_from('<III', pe.data, o + 12 * i)
        if b == 0 and e == 0:
            continue
        out.append((b, e, u))
    out.sort()
    return out


def fn_for_rva(pd, rva):
    for b, e, u in pd:
        if b <= rva < e:
            return (b, e, u)
    return None


if __name__ == '__main__':
    pe = PE()
    print('imagebase', hex(pe.imagebase), 'nsec', pe.nsec, 'sizeofimage', hex(pe.sizeofimage))
    for s in pe.sections:
        print('%-10s rva %#x vsz %#x raw %#x rawsz %#x chars %#x'
              % (s['name'], s['rva'], s['vsz'], s['rawptr'], s['rawsz'], s['chars']))
    for i, (r, z) in enumerate(pe.dirs):
        if r:
            print('dir[%d] rva %#x size %#x' % (i, r, z))
    pd = pdata(pe)
    print('pdata entries', len(pd))
