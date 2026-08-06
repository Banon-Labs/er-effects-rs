#!/usr/bin/env python3
"""Parse WitchyBND PARAM XML -> field table with computed byte offsets + row dump.

WitchyBND emits `<param>` XML containing the paramdef `<fields>` it applied and a
`<rows>` list.  The paramdef gives field order/type/bitsize but NOT byte offsets;
this recomputes them (including bitfield packing) so an RE agent can line the
param up against in-memory row layouts.

Usage:
    python3 scripts/witchy-param-dump.py <ParamName> [--dir DIR] [--rows] [--max=N]

DIR defaults to $WITCHY_PARAM_XML_DIR, else the current directory.  Data
artifacts (redirected stdout) belong in a scratch dir, not the repo.
"""
import os
import sys
import xml.etree.ElementTree as ET

SIZES = {'s8': 1, 'u8': 1, 'dummy8': 1, 's16': 2, 'u16': 2,
         's32': 4, 'u32': 4, 'f32': 4, 'b32': 4, 'angle32': 4,
         'f64': 8, 's64': 8, 'u64': 8}


def field_table(root):
    """-> ([(name, type, arraylen, bitsize, byte_offset, bit_offset)], row_size)."""
    out = []
    off = 0
    bit_type = None
    bit_used = 0
    bit_cap = 0
    bit_base = 0
    for f in root.find('fields'):
        name = f.get('name')
        ftype = f.get('type')
        alen = int(f.get('arraylength', '1'))
        bsize = int(f.get('bitsize', '-1'))
        if ftype == 'fixstr':
            sz = alen
        elif ftype == 'fixstrW':
            sz = alen * 2
        else:
            base = SIZES.get(ftype)
            if base is None:
                raise SystemExit('unknown paramdef type: ' + ftype)
            sz = base * alen
        if bsize > 0 and ftype in SIZES and alen == 1:
            unit = SIZES[ftype]
            # SoulsFormats packs bitfields by STORAGE SIZE, and `dummy8` shares a
            # storage unit with `u8`/`s8` neighbours -- so normalise before the
            # "same bitfield type?" test or every dummy8 reserve opens a new byte.
            key = unit
            if bit_type != key or bit_used + bsize > bit_cap:
                bit_base = off
                off += unit
                bit_type = key
                bit_cap = unit * 8
                bit_used = 0
            out.append((name, ftype, alen, bsize, bit_base, bit_used))
            bit_used += bsize
        else:
            bit_type = None
            bit_used = 0
            bit_cap = 0
            out.append((name, ftype, alen, -1, off, -1))
            off += sz
    return out, off


def report(xml_path, dump_rows=False, max_rows=None, out=sys.stdout):
    root = ET.parse(xml_path).getroot()
    fields, rowsize = field_table(root)
    rows = list(root.find('rows'))
    pdef = root.find('paramdef')
    name = os.path.basename(xml_path).replace('.param.xml', '')
    print('=' * 78, file=out)
    print('PARAM %s' % name, file=out)
    print('  paramType=%s dataVersion=%s format2D=%s paramdef_formatVersion=%s'
          % (root.findtext('type'), root.findtext('dataVersion'),
             root.findtext('format2D'),
             pdef.findtext('formatVersion') if pdef is not None else '?'),
          file=out)
    print('  ROW COUNT = %d   computed row size = %d bytes   fields = %d'
          % (len(rows), rowsize, len(fields)), file=out)
    print('  FIELDS (hexoff decoff bit name type[len])', file=out)
    for (n, t, a, b, o, bo) in fields:
        ts = '%s[%d]' % (t, a) if a != 1 else t
        bs = 'bit%d:%d' % (bo, b) if b > 0 else '-'
        print('    0x%04x %5d  %10s  %-42s %s' % (o, o, bs, n, ts), file=out)
    if dump_rows:
        names = [f[0] for f in fields]
        print('  ROWS (CSV):', file=out)
        print('    ' + ','.join(['id', 'paramdexName'] + names), file=out)
        sel = rows if max_rows is None else rows[:max_rows]
        for row in sel:
            vals = [row.get('id', ''), row.get('paramdexName', '')]
            vals += [row.get(n, '') for n in names]
            print('    ' + ','.join(v.replace(',', ';') for v in vals), file=out)
        if max_rows is not None and len(rows) > max_rows:
            print('    ... TRUNCATED, %d more rows' % (len(rows) - max_rows), file=out)
    return fields, rows


STRUCT = {'s8': 'b', 'u8': 'B', 'dummy8': 'B', 's16': 'h', 'u16': 'H',
          's32': 'i', 'u32': 'I', 'f32': 'f', 'b32': 'i', 'angle32': 'f',
          'f64': 'd', 's64': 'q', 'u64': 'Q'}


def read_param_binary(param_path, fields):
    """Decode row data straight from the .param bytes using the computed field
    offsets.  Avoids relying on WitchyBND's "omit fields equal to paramdef
    default" XML behaviour.  Returns [(row_id, {field: value})].

    Only the ER/long-data-offset row-entry layout is handled (24-byte entries at
    0x40): id s32, pad s32, dataOffset s64, nameOffset s64.
    """
    import struct
    blob = open(param_path, 'rb').read()
    row_count = struct.unpack_from('<h', blob, 0x0A)[0]
    out = []
    for i in range(row_count):
        rid, _pad, doff, _noff = struct.unpack_from('<iiqq', blob, 0x40 + i * 24)
        vals = {}
        for (n, t, a, b, o, bo) in fields:
            if b > 0:
                unit = SIZES[t]
                raw = int.from_bytes(blob[doff + o:doff + o + unit], 'little')
                vals[n] = (raw >> bo) & ((1 << b) - 1)
            elif t == 'fixstr':
                vals[n] = blob[doff + o:doff + o + a].split(b'\x00')[0].decode('shift_jis', 'replace')
            elif t == 'fixstrW':
                vals[n] = blob[doff + o:doff + o + a * 2].split(b'\x00\x00')[0].decode('utf-16-le', 'replace')
            elif a != 1:
                sz = SIZES[t]
                vals[n] = '[' + '|'.join(
                    str(struct.unpack_from('<' + STRUCT[t], blob, doff + o + k * sz)[0])
                    for k in range(a)) + ']'
            else:
                vals[n] = struct.unpack_from('<' + STRUCT[t], blob, doff + o)[0]
        out.append((rid, vals))
    return out


def report_binary(xml_path, param_path, out=sys.stdout, max_rows=None,
                  skip_dummy=False, cross_check=True):
    root = ET.parse(xml_path).getroot()
    fields, rowsize = field_table(root)
    names_xml = {int(e.get('id')): e.get('paramdexName', '')
                 for e in root.find('rows')}
    rows = read_param_binary(param_path, fields)
    sel = [f for f in fields if not (skip_dummy and f[1] == 'dummy8')]
    hdr = ['id', 'paramdexName(Paramdex annotation, NOT game data)'] + [f[0] for f in sel]
    print(','.join(hdr), file=out)
    for rid, vals in (rows if max_rows is None else rows[:max_rows]):
        line = [str(rid), names_xml.get(rid, '').replace(',', ';')]
        for f in sel:
            v = vals[f[0]]
            line.append(('%g' % v) if isinstance(v, float) else str(v))
        print(','.join(line), file=out)
    return rows


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    pname = argv[1]
    base = os.environ.get('WITCHY_PARAM_XML_DIR', '.')
    for i, a in enumerate(argv):
        if a == '--dir':
            base = argv[i + 1]
    dump = '--rows' in argv
    mx = None
    for a in argv:
        if a.startswith('--max='):
            mx = int(a.split('=', 1)[1])
    path = pname if pname.endswith('.xml') else os.path.join(base, pname + '.param.xml')
    if '--binrows' in argv:
        report_binary(path, path[:-4], max_rows=mx, skip_dummy='--nodummy' in argv)
    else:
        report(path, dump_rows=dump, max_rows=mx)


if __name__ == '__main__':
    main(sys.argv)
