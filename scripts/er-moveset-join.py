#!/usr/bin/env python3
"""Join one creature's moveset across all four offline sources.

  behavior graph  (<chr>.behbnd.dcx -> Havok TAG0)   fireable event name -> state -> clip
  animation set   (<chr>.anibnd.dcx sidecar XML)     which clips actually ship
  TimeAct         (<chr>.tae, event type 1)          Behavior Judge ID per animation
  regulation.bin  NpcParam -> BehaviorParam          Behavior Judge ID -> AtkParam_Npc row

  python3 scripts/er-moveset-join.py c2120 --root <sharded/chr> [--npc-row 21200000]
"""
import argparse, glob, importlib.util, os, re, struct, sys, xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(mod, fn):
    s = importlib.util.spec_from_file_location(mod, os.path.join(HERE, fn))
    m = importlib.util.module_from_spec(s); s.loader.exec_module(m)
    return m


BMAP = _load('bmap', 'er-behbnd-attack-map.py')
TAE = _load('taescan', 'er-tae-event-scan.py')
PR = _load('paramread', 'er-param-read.py')

#: TAE event type 1 "Attack Behavior" params (Smithbox TAE.Template.ER.xml):
#: [Attack Type, Attack Index, Behavior Judge ID, Direction Type, Source, State Info]
ATTACK_BEHAVIOR_EVENT = 1
BEHAVIOR_JUDGE_PARAM = 2


def find(root, chrid, kind):
    hits = glob.glob(f'{root}/*/{chrid}-{kind}*') + glob.glob(f'{root}/{chrid}-{kind}*')
    return hits[0] if hits else None


def graph_events(behdir):
    rows = {}
    for hk in sorted(glob.glob(os.path.join(behdir, 'Behaviors', '*.hkx'))):
        g = BMAP.Graph(hk)
        for ev, rs in g.event_to_anim().items():
            rows.setdefault(ev, rs[0])
    return rows


def shipped_anims(anibnd_dirs):
    """{animId: clip filename} from the witchy sidecars (no binary work)."""
    out = {}
    for d in anibnd_dirs:
        for xml in glob.glob(os.path.join(d, '_witchy-*.xml')):
            for f in ET.parse(xml).getroot().iter('file'):
                p = (f.findtext('path') or '').replace('\\', '/')
                mm = re.search(r'/(a(\d{2,3})_(\d{6}))\.hkx$', p)
                if mm:
                    out.setdefault(int(mm.group(3)), []).append(mm.group(1))
    return out


def tae_behaviors(taepath):
    """{animId: [(attackType, attackIndex, behaviorJudgeId)]}"""
    _, anims = TAE.parse(taepath)
    out = {}
    for aid, evs in anims.items():
        for e in evs:
            if e.type != ATTACK_BEHAVIOR_EVENT:
                continue
            n = len(e.params) // 4
            vals = struct.unpack_from('<%di' % n, e.params, 0)
            out.setdefault(aid % 1000000, []).append(
                (vals[0], vals[1], vals[BEHAVIOR_JUDGE_PARAM]))
    return out


def atk_rows(variation_id):
    files = PR.load()
    beh, _, _ = PR.rows(PR.param_bytes(files, 'BehaviorParam'),
                        ['variationId', 'behaviorJudgeId', 'refType', 'refId'])
    atk = set(r['id'] for r in PR.rows(PR.param_bytes(files, 'AtkParam_Npc'), ['id'])[0])
    out = {}
    for r in beh:
        if r['variationId'] == variation_id:
            out[r['behaviorJudgeId']] = (r['id'], r['refType'], r['refId'],
                                         r['refId'] in atk)
    return out


def npc_variation(chrid, npc_row=None):
    files = PR.load()
    rs, _, _ = PR.rows(PR.param_bytes(files, 'NpcParam'), ['behaviorVariationId'])
    n = int(chrid[1:])
    lo, hi = n * 10000, (n + 1) * 10000
    cand = [r for r in rs if (r['id'] == npc_row) if npc_row] or \
           [r for r in rs if lo <= r['id'] < hi]
    if not cand:
        return None, []
    return cand[0]['behaviorVariationId'], [r['id'] for r in cand]


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('chrid')
    ap.add_argument('--root', default='/home/banon/er-extract/'
                                      'LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/chr')
    ap.add_argument('--npc-row', type=int)
    ap.add_argument('--band', default='3000-3999')
    a = ap.parse_args()
    lo, hi = (int(x) for x in a.band.split('-'))

    beh = find(a.root, a.chrid, 'behbnd')
    ani = sorted(glob.glob(f'{a.root}/*/{a.chrid}-anibnd*') +
                 glob.glob(f'{a.root}/*/{a.chrid}_div*-anibnd*') +
                 glob.glob(f'{a.root}/{a.chrid}-anibnd*'))
    taes = [p for d in ani for p in glob.glob(os.path.join(d, '**', '*.tae'), recursive=True)]
    ev = graph_events(beh)
    ships = shipped_anims(ani)
    tb = tae_behaviors(taes[0]) if taes else {}
    var, rows_used = npc_variation(a.chrid, a.npc_row)
    beh_map = atk_rows(var) if var is not None else {}
    print(f'# {a.chrid}  behbnd={os.path.basename(beh)}  tae={os.path.basename(taes[0]) if taes else "-"}'
          f'  NpcParam rows={rows_used[:4]} behaviorVariationId={var}', file=sys.stderr)
    print('anim\tclip\tevent_names\tstate\tTAE_judgeIds\tBehaviorParam\tAtkParam_Npc')
    byanim = {}
    for name, r in ev.items():
        for an in r['anims']:
            byanim.setdefault(an, []).append(name)
    for an in sorted(set(list(byanim) + list(ships)) ):
        if not (lo <= an <= hi):
            continue
        names = sorted(byanim.get(an, []))
        exact = [n for n in names if n.endswith(str(an)) and n[:-len(str(an))].isidentifier()]
        primary = [n for n in exact if n in ('W_Attack%d' % an, 'W_Event%d' % an,
                                             'W_GuardAttack%d' % an, 'W_Step%d' % an)]
        st = ev[(primary or exact or names)[0]]['state'] if names else ''
        names = (primary or exact or names)
        judges = sorted(set(j for _, _, j in tb.get(an, [])))
        bp = [beh_map.get(j, (None,))[0] for j in judges]
        atk = [beh_map[j][2] for j in judges if j in beh_map and beh_map[j][1] == 0]
        ok = [beh_map[j][3] for j in judges if j in beh_map]
        print(f'{an}\t{",".join(ships.get(an, [])) or "-"}\t{",".join(names) or "-"}\t{st}\t'
              f'{",".join(map(str,judges)) or "-"}\t{",".join(str(x) for x in bp) or "-"}\t'
              f'{",".join(str(x) for x in atk) or "-"}{"" if all(ok) else " (MISSING)"}')
