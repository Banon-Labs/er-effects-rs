#!/usr/bin/env python3
"""Read PARAM rows (ids and named fields) out of the installed regulation.bin, offline.

Extends scripts/regulation-params.py (decrypt -> DCX/zstd -> BND4 -> PARAM row ids)
with a paramdef-driven field decoder. Paramdefs come from a local Smithbox/Paramdex
checkout; override with ER_PARAMDEF_DIR.

  python3 scripts/er-param-read.py NpcParam --fields behaviorVariationId --row 33200000
  python3 scripts/er-param-read.py BehaviorParam --where variationId=332 --fields behaviorJudgeId,refType,refId
"""
import argparse, importlib.util, os, re, struct, sys, xml.etree.ElementTree as ET

_HERE = os.path.dirname(os.path.abspath(__file__))
_s = importlib.util.spec_from_file_location('regulation_params',
                                            os.path.join(_HERE, 'regulation-params.py'))
RP = importlib.util.module_from_spec(_s); _s.loader.exec_module(RP)

PARAMDEF_DIR = os.environ.get(
    'ER_PARAMDEF_DIR',
    os.path.expanduser('~/.local/share/smithbox/app/Assets/PARAM/ER/Defs'))
ROWNAME_DIR = os.environ.get(
    'ER_PARAM_ROWNAME_DIR',
    os.path.expanduser('~/.local/share/smithbox/app/Assets/PARAM/ER/Param Row Names/English'))

SIZES = {'s8': 1, 'u8': 1, 's16': 2, 'u16': 2, 's32': 4, 'u32': 4,
         'f32': 4, 'f64': 8, 'b32': 4, 'dummy8': 1, 'fixstr': 1, 'fixstrW': 2}
FMT = {'s8': '<b', 'u8': '<B', 's16': '<h', 'u16': '<H', 's32': '<i',
       'u32': '<I', 'f32': '<f', 'f64': '<d', 'b32': '<i'}
DEFRE = re.compile(r'^\s*(\w+)\s+([A-Za-z_]\w*)\s*(?:\[(\d+)\])?\s*(?::\s*(\d+))?')


_DEF_INDEX = None


def _index_defs():
    global _DEF_INDEX
    if _DEF_INDEX is None:
        _DEF_INDEX = {}
        import glob as _g
        for f in _g.glob(os.path.join(PARAMDEF_DIR, '*.xml')):
            try:
                pt = ET.parse(f).getroot().findtext('ParamType')
            except Exception:
                continue
            _DEF_INDEX.setdefault(pt, f)
            _DEF_INDEX.setdefault(os.path.splitext(os.path.basename(f))[0], f)
    return _DEF_INDEX


def paramdef(param_type, modern=True):
    """Field list for a param type.

    Smithbox defs carry FirstVersion/RemovedVersion attributes: a field marked
    RemovedVersion=V is absent once the regulation reaches V, and FirstVersion=V
    is absent before it. `modern=True` takes the post-V field set. Which one is
    right is decided by matching the computed row size against the PARAM's own
    row stride (see rows()), so no version number has to be hard-coded.
    """
    path = _index_defs().get(param_type)
    if path is None:
        raise SystemExit(f'no paramdef for {param_type} in {PARAMDEF_DIR}')
    root = ET.parse(path).getroot()
    fields = []
    for f in root.find('Fields'):
        if modern and f.get('RemovedVersion'):
            continue
        if not modern and f.get('FirstVersion'):
            continue
        mm = DEFRE.match(f.get('Def'))
        ty, name, cnt, bits = mm.group(1), mm.group(2), mm.group(3), mm.group(4)
        fields.append({'type': ty, 'name': name,
                       'count': int(cnt) if cnt else 1,
                       'bits': int(bits) if bits else 0})
    return fields


def layout(fields):
    """Assign byte offsets, honouring C-style bitfield packing."""
    off = 0
    bit_ty, bit_used, bit_off = None, 0, 0
    for f in fields:
        if f['bits']:
            # `dummy8 padN:1` packs into the same storage unit as its u8 neighbours.
            ty = 'u8' if f['type'] == 'dummy8' else f['type']
            w = SIZES[ty] * 8
            if bit_ty != ty or bit_used + f['bits'] > w:
                bit_ty, bit_used, bit_off = ty, 0, off
                off += SIZES[ty]
            f['off'], f['shift'], f['bty'] = bit_off, bit_used, ty
            bit_used += f['bits']
        else:
            bit_ty, bit_used = None, 0
            # FromSoft PARAM rows are packed: no implicit alignment padding
            # (explicit `dummy8 pad[N]` fields carry every gap).
            f['off'] = off
            off += SIZES[f['type']] * f['count']
    return fields, off


def load(regulation=None):
    return RP.bnd4_entries(RP.dcx_unpack(RP.decrypt(regulation or RP.DEFAULT_REGULATION)))


def param_bytes(files, stem):
    key = next((n for n in files if n.rsplit('\\', 1)[-1].removesuffix('.param') == stem), None)
    if key is None:
        raise SystemExit(f'{stem}: not found among {len(files)} params')
    return files[key]


def param_type_of(p):
    off = struct.unpack_from('<q', p, 0x10)[0]
    end = p.index(b'\x00', off)
    return p[off:end].decode('ascii')


def row_stride(p):
    n = struct.unpack_from('<H', p, 0x0A)[0]
    if n < 2:
        return None
    offs = [struct.unpack_from('<q', p, 0x40 + i * 24 + 8)[0] for i in range(n)]
    from collections import Counter
    return Counter(b - a for a, b in zip(offs, offs[1:])).most_common(1)[0][0]


def rows(p, fields=None, strict=True):
    n = struct.unpack_from('<H', p, 0x0A)[0]
    pt = param_type_of(p)
    stride = row_stride(p)
    fl, size = layout(paramdef(pt, modern=True))
    if stride is not None and size != stride:
        fl2, size2 = layout(paramdef(pt, modern=False))
        if size2 == stride:
            fl, size = fl2, size2
        elif strict:
            raise SystemExit(
                f'{pt}: paramdef row size {size} (or {size2}) != data stride {stride}; '
                'refusing to read fields at unverified offsets')
    out = []
    for i in range(n):
        rid = struct.unpack_from('<i', p, 0x40 + i * 24)[0]
        doff = struct.unpack_from('<q', p, 0x40 + i * 24 + 8)[0]
        r = {'id': rid, '_off': doff}
        for f in fl:
            if fields and f['name'] not in fields:
                continue
            b = doff + f['off']
            if f['type'] in ('fixstr', 'fixstrW', 'dummy8'):
                continue
            v = struct.unpack_from(FMT[f.get('bty', f['type'])], p, b)[0]
            if f['bits']:
                v = (v >> f['shift']) & ((1 << f['bits']) - 1)
            r[f['name']] = v
        out.append(r)
    return out, pt, size


def row_names(stem):
    import json
    path = os.path.join(ROWNAME_DIR, stem + '.json')
    if not os.path.exists(path):
        return {}
    d = json.load(open(path, encoding='utf-8-sig'))
    if isinstance(d, dict):
        for k in ('Entries', 'entries', 'list'):
            if k in d:
                d = d[k]
                break
    if isinstance(d, dict):
        return {int(k): v for k, v in d.items()}
    out = {}
    for e in d:
        if isinstance(e, dict):
            i = e.get('ID', e.get('id'))
            # The Smithbox row-name schema nests a second 'Entries' inside each row:
            #   {"Name": "NpcParam", "Entries": [{"ID": 40700000, "Entries": ["Wolf"]}]}
            # This used to read e['Name'], which that shape does not have, so --names
            # printed `'name': None` for all 6864 named NpcParam rows and looked like
            # a missing checkout rather than a wrong key.
            nm = e.get('Name', e.get('name'))
            if nm is None:
                inner = e.get('Entries', e.get('entries'))
                if isinstance(inner, list) and inner:
                    nm = inner[0]
                elif isinstance(inner, str):
                    nm = inner
            if i is not None:
                out[int(i)] = nm
    return out


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('param')
    ap.add_argument('--regulation')
    ap.add_argument('--fields')
    ap.add_argument('--row', type=int, action='append', default=[])
    ap.add_argument('--where', action='append', default=[])
    ap.add_argument('--names', action='store_true')
    ap.add_argument('--limit', type=int, default=60)
    a = ap.parse_args()
    files = load(a.regulation)
    p = param_bytes(files, a.param)
    want = a.fields.split(',') if a.fields else None
    rs, pt, size = rows(p, want)
    print(f'# {a.param} ({pt}) rows={len(rs)} defsize={size}', file=sys.stderr)
    nm = row_names(a.param) if a.names else {}
    conds = [w.split('=') for w in a.where]
    n = 0
    for r in rs:
        if a.row and r['id'] not in a.row:
            continue
        if conds and not all(str(r.get(k)) == v for k, v in conds):
            continue
        r.pop('_off', None)
        print(r if not nm else {**r, 'name': nm.get(r['id'])})
        n += 1
        if n >= a.limit:
            break
