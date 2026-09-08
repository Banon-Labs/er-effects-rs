#!/usr/bin/env python3
"""Minimal Havok 2018.1.0 TAG0 tagfile reader (read-only, offline).

Enough of the format to enumerate typed items and read reflected struct members,
which is all that is needed to pull hkbBehaviorGraphStringData::eventNames out of
an ELDEN RING <chr>.behbnd.dcx payload without HKLib.

Sections: TAG0{ SDKV, data, type{TPTR,TSTR,TNA1,FSTR,TBDY,THSH,TPAD}, INDX{item,PTCH} }
"""
import struct, sys, json


def _packed(b, o):
    """Havok tagfile packed int. Returns (value, new_offset)."""
    b0 = b[o]
    if b0 & 0x80 == 0:
        return b0, o + 1
    if b0 & 0x40 == 0:                      # 10xxxxxx
        return ((b0 & 0x3F) << 8) | b[o + 1], o + 2
    if b0 & 0x20 == 0:                      # 110xxxxx
        return ((b0 & 0x1F) << 16) | (b[o + 1] << 8) | b[o + 2], o + 3
    if b0 & 0x10 == 0:                      # 1110xxxx
        return ((b0 & 0x0F) << 24) | (b[o + 1] << 16) | (b[o + 2] << 8) | b[o + 3], o + 4
    return struct.unpack_from('>I', b, o + 1)[0], o + 5


class Type:
    __slots__ = ('idx', 'name', 'templates', 'parent', 'fmt', 'sub', 'version',
                 'size', 'align', 'members', 'flags')

    def __init__(self, idx):
        self.idx = idx; self.name = None; self.templates = []
        self.parent = 0; self.fmt = 0; self.sub = 0; self.version = 0
        self.size = 0; self.align = 0; self.members = []; self.flags = 0

    def all_members(self, types):
        out = []
        if self.parent:
            out += types[self.parent].all_members(types)
        out += self.members
        return out


class Tagfile:
    def __init__(self, data):
        self.d = data
        self.sec = {}
        self._walk(0, len(data), '')
        self.data_off = self.sec['TAG0/DATA'][0]
        self._types()
        self._items()
        self._patches()

    def _walk(self, off, end, prefix):
        while off <= end - 8:
            sz = struct.unpack_from('>I', self.d, off)[0] & 0x3FFFFFFF
            name = self.d[off + 4:off + 8].decode('ascii', 'replace')
            if sz < 8 or off + sz > end:
                break
            key = prefix + name
            self.sec[key] = (off + 8, off + sz)
            if name in ('TAG0', 'TYPE', 'INDX'):
                self._walk(off + 8, off + sz, key + '/')
            off += sz

    def _strs(self, key):
        s, e = self.sec[key]
        raw = self.d[s:e].split(b'\x00')
        if raw and raw[-1] == b'':
            raw.pop()
        return [x.decode('utf-8', 'replace') for x in raw]

    def _types(self):
        tstr = self._strs('TAG0/TYPE/TSTR')
        fstr = self._strs('TAG0/TYPE/FSTR')
        self.tstr, self.fstr = tstr, fstr
        s, e = self.sec['TAG0/TYPE/TNA1']
        n, o = _packed(self.d, s)
        self.types = [Type(0)]
        self.types[0].name = 'void'
        for i in range(1, n):
            t = Type(i)
            ni, o = _packed(self.d, o)
            t.name = tstr[ni] if ni < len(tstr) else f'?{ni}'
            tc, o = _packed(self.d, o)
            for _ in range(tc):
                a, o = _packed(self.d, o)
                v, o = _packed(self.d, o)
                t.templates.append((tstr[a] if a < len(tstr) else f'?{a}', v))
            self.types.append(t)
        assert 0 <= e - o <= 8, f'TNA1 under/over-read {o:#x} != {e:#x}'
        # TBDY (best-effort: member offsets are a bonus, not required)
        s, e = self.sec['TAG0/TYPE/TBDY']
        o = s
        self.tbdy_ok = True
        try:
         while o < e:
            ti, o = _packed(self.d, o)
            if ti == 0:
                continue
            t = self.types[ti]
            t.parent, o = _packed(self.d, o)
            t.flags, o = _packed(self.d, o)
            f = t.flags
            if f & 0x1:
                t.fmt, o = _packed(self.d, o)
            if f & 0x2:
                t.sub, o = _packed(self.d, o)
            if f & 0x4:
                t.version, o = _packed(self.d, o)
            if f & 0x8:
                t.size, o = _packed(self.d, o)
                t.align, o = _packed(self.d, o)
            if f & 0x10:
                _, o = _packed(self.d, o)
            if f & 0x20:
                nm, o = _packed(self.d, o)
                for _ in range(nm):
                    ni, o = _packed(self.d, o)
                    mf, o = _packed(self.d, o)
                    bo, o = _packed(self.d, o)
                    mt, o = _packed(self.d, o)
                    t.members.append((fstr[ni] if ni < len(fstr) else f'?{ni}', mf, bo, mt))
            if f & 0x40:
                ic, o = _packed(self.d, o)
                for _ in range(ic):
                    _, o = _packed(self.d, o)
                    _, o = _packed(self.d, o)
            if f & 0x80:
                _, o = _packed(self.d, o)
        except (IndexError, KeyError, struct.error):
            self.tbdy_ok = False

    def _items(self):
        s, e = self.sec['TAG0/INDX/ITEM']
        self.items = []
        for o in range(s, e - 11, 12):
            v, off, cnt = struct.unpack_from('<III', self.d, o)
            self.items.append({'type': v & 0xFFFFFF, 'flags': v >> 24,
                               'off': off, 'count': cnt, 'idx': len(self.items)})

    def _patches(self):
        s, e = self.sec['TAG0/INDX/PTCH']
        o = s
        self.ptch = {}   # Data offset -> item index
        while o + 8 <= e:
            ti, cnt = struct.unpack_from('<II', self.d, o)
            o += 8
            for _ in range(cnt):
                if o + 4 > e:
                    break
                doff = struct.unpack_from('<I', self.d, o)[0]
                o += 4
                if self.data_off + doff + 4 <= len(self.d):
                    self.ptch[doff] = struct.unpack_from('<I', self.d, self.data_off + doff)[0]

    # --- convenience -------------------------------------------------------
    def tname(self, ti):
        return self.types[ti].name if ti < len(self.types) else f'?{ti}'

    def item_bytes(self, i):
        it = self.items[i]
        return self.d[self.data_off + it['off']:]

    def cstr(self, i):
        """Item i is a `char` array -> python str."""
        it = self.items[i]
        _s = self.data_off + it['off']
        raw = self.d[_s:min(_s + it['count'], len(self.d))]
        return raw.split(b'\x00')[0].decode('utf-8', 'replace')

    def find_items(self, typename):
        return [it for it in self.items if self.tname(it['type']) == typename]

    def member_map(self, ti):
        return {m[0]: (m[2], m[3]) for m in self.types[ti].all_members(self.types)}

    def arrays_in(self, item, size_hint=None):
        """Return [(data_offset, target_item_index, count)] for every patched pointer
        inside `item`'s blob, ordered by offset. For a class whose members are all
        hkArray<T>, this is the member list in declaration order."""
        base = item['off']
        end = base + (size_hint if size_hint else 0x400)
        out = []
        for doff, tgt in self.ptch.items():
            if base <= doff < end:
                cnt = self.items[tgt]['count']
                out.append((doff, tgt, cnt))
        out.sort()
        return out

    def strs_of(self, item_index, n):
        """item_index is an hkStringPtr array body of n entries -> list[str]."""
        ai = self.items[item_index]
        out = []
        for k in range(n):
            p = self.ptch.get(ai['off'] + k * 8)
            out.append(self.cstr(p) if p is not None else '')
        return out

    def read_str_array(self, item, member):
        """Read hkArray<hkStringPtr> member of `item` -> list[str]."""
        mm = self.member_map(item['type'])
        boff, mt = mm[member]
        base = item['off'] + boff
        n = struct.unpack_from('<I', self.d, self.data_off + base + 8)[0]
        if n == 0:
            return []
        arr_item = self.ptch.get(base)
        if arr_item is None:
            return []
        ai = self.items[arr_item]
        out = []
        for k in range(n):
            p = self.ptch.get(ai['off'] + k * 8)
            out.append(self.cstr(p) if p is not None else '')
        return out


if __name__ == '__main__':
    tf = Tagfile(open(sys.argv[1], 'rb').read())
    what = sys.argv[2] if len(sys.argv) > 2 else 'summary'
    if what == 'summary':
        from collections import Counter
        c = Counter(tf.tname(i['type']) for i in tf.items)
        print(json.dumps({'items': len(tf.items), 'types': len(tf.types),
                          'top': c.most_common(25)}, indent=1))
    elif what == 'strings':
        sd = tf.find_items('hkbBehaviorGraphStringData')
        for it in sd:
            for m in ('eventNames', 'attributeNames', 'variableNames', 'characterPropertyNames'):
                v = tf.read_str_array(it, m)
                print(f'## {m} ({len(v)})')
                for i, s in enumerate(v):
                    print(f'{i}\t{s}')
    else:
        print(json.dumps([m[0] for m in tf.types[int(what)].all_members(tf.types)]))
