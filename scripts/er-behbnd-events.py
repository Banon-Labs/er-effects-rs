#!/usr/bin/env python3
"""Enumerate the FIREABLE hkbBehaviorGraph event names of an ELDEN RING chr.

The behavior graph payloads inside <chr>.behbnd.dcx are Havok 2018.1.0 TAG0
tagfiles. hkbBehaviorGraphStringData holds four hkArray<hkStringPtr> members at
fixed byte offsets inside its serialized blob:

    +0x18 eventNames             <- the only names hkbBehaviorGraph::fireEvent accepts
    +0x28 attributeNames
    +0x38 variableNames
    +0x48 characterPropertyNames

Event IDs are the index into eventNames, i.e. per-graph, not global.

Usage:
  er-behbnd-events.py <behbnd-dir|hkx>            # dump events
  er-behbnd-events.py <dir> --json                # machine readable
  er-behbnd-events.py --corpus <root> [--limit N] # sweep every chr
"""
import importlib.util, os, sys, json, re, glob
_h = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'hkx-tagfile.py')
_s = importlib.util.spec_from_file_location('hkx_tagfile', _h)
hkx = importlib.util.module_from_spec(_s); _s.loader.exec_module(hkx)

SLOTS = {0x18: 'eventNames', 0x28: 'attributeNames',
         0x38: 'variableNames', 0x48: 'characterPropertyNames'}


def read_graph(path):
    tf = hkx.Tagfile(open(path, 'rb').read())
    out = {k: [] for k in SLOTS.values()}
    for it in tf.find_items('hkbBehaviorGraphStringData'):
        for off, tgt, cnt in tf.arrays_in(it, 0x58):
            slot = SLOTS.get(off - it['off'])
            if slot:
                out[slot] = tf.strs_of(tgt, cnt)
    return out


def behbnd_graphs(d):
    if os.path.isfile(d):
        return [d]
    return sorted(glob.glob(os.path.join(d, 'Behaviors', '*.hkx')))


def collect(d):
    merged = {k: [] for k in SLOTS.values()}
    per = {}
    for g in behbnd_graphs(d):
        r = read_graph(g)
        per[os.path.basename(g)] = {k: len(v) for k, v in r.items()}
        for k in merged:
            merged[k] += r[k]
    return merged, per


NUMRE = re.compile(r'^([A-Za-z_][A-Za-z_]*?)(\d{3,6})$')


def bands(events):
    b = {}
    for e in events:
        mm = NUMRE.match(e)
        if not mm:
            continue
        pre, num = mm.group(1), int(mm.group(2))
        b.setdefault(pre, []).append(num)
    return {k: sorted(v) for k, v in b.items()}


if __name__ == '__main__':
    a = sys.argv[1:]
    if a and a[0] == '--corpus':
        root = a[1]
        lim = int(a[a.index('--limit') + 1]) if '--limit' in a else 10 ** 9
        rows = {}
        for d in sorted(glob.glob(os.path.join(root, '*', '*behbnd*')) +
                        glob.glob(os.path.join(root, '*behbnd*'))):
            chrid = os.path.basename(d).split('-')[0]
            try:
                merged, _ = collect(d)
            except Exception as ex:
                rows[chrid] = {'error': str(ex)[:80]}
                continue
            rows[chrid] = {'events': len(merged['eventNames']),
                           'vars': len(merged['variableNames']),
                           'bands': {k: [v[0], v[-1], len(v)]
                                     for k, v in bands(merged['eventNames']).items()}}
            if len(rows) >= lim:
                break
        print(json.dumps(rows))
    else:
        merged, per = collect(a[0])
        if '--json' in a:
            print(json.dumps({'per_file': per, **merged}))
        else:
            print('# per-file counts:', json.dumps(per))
            for k, v in merged.items():
                print(f'## {k} ({len(v)})')
                for i, s in enumerate(v):
                    print(f'{i}\t{s}')
