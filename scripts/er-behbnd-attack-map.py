#!/usr/bin/env python3
"""Join hkbBehaviorGraph event name -> state -> generator -> clip (animation id).

Offline, no Havok SDK. Reads <chr>.behbnd.dcx payloads (Havok 2018.1.0 TAG0).

Empirically-validated serialized layouts (see --selftest):
  hkbNode                      +0x48  m_name (hkStringPtr)
  hkbStateMachine              +0xE0  m_states (hkArray<StateInfo*>)
                               +0xF0  m_wildcardTransitions (TransitionInfoArray*)
  hkbStateMachine::StateInfo   size 128; +0x58 m_transitions, +0x60 m_generator,
                               +0x68 m_name, +0x70 m_stateId
  ::TransitionInfo             size 72;  +0x30 m_eventId, +0x34 m_toStateId
  hkbClipGenerator             +0x98  m_animationName
"""
import importlib.util, os, sys, struct, collections, json, glob, re
_h = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'hkx-tagfile.py')
_s = importlib.util.spec_from_file_location('hkx_tagfile', _h)
hkx = importlib.util.module_from_spec(_s); _s.loader.exec_module(hkx)

NAME_OFF, SM_STATES, SM_WILD = 0x48, 0xE0, 0xF0
SI_SIZE, SI_TRANS, SI_GEN, SI_NAME, SI_ID = 128, 0x58, 0x60, 0x68, 0x70
TI_SIZE, TI_EVENT, TI_TO = 72, 0x30, 0x34
CLIP_ANIM = 0x98
CLIPRE = re.compile(r'^a\d{2,3}_(\d{6})$')


class Graph:
    def __init__(self, path):
        self.tf = tf = hkx.Tagfile(open(path, 'rb').read())
        self.rev = collections.defaultdict(list)
        for doff, tgt in tf.ptch.items():
            self.rev[tgt].append(doff)
        self.events = []
        for it in tf.find_items('hkbBehaviorGraphStringData'):
            for off, tgt, cnt in tf.arrays_in(it, 0x58):
                if off - it['off'] == 0x18:
                    self.events = tf.strs_of(tgt, cnt)

    def ptr(self, it, off):
        return self.tf.ptch.get(it['off'] + off)

    def s(self, it, off):
        p = self.ptr(it, off)
        return self.tf.cstr(p) if p is not None and self.tf.tname(self.tf.items[p]['type']) == 'char' else None

    def arr(self, it, off):
        p = self.ptr(it, off)
        if p is None:
            return []
        a = self.tf.items[p]
        return [self.tf.ptch.get(a['off'] + k * 8) for k in range(a['count'])]

    def clips_under(self, item_idx, seen=None, depth=0):
        """Every hkbClipGenerator reachable from a generator item -> animation ids."""
        if item_idx is None or depth > 6:
            return []
        seen = seen if seen is not None else set()
        if item_idx in seen:
            return []
        seen.add(item_idx)
        it = self.tf.items[item_idx]
        tn = self.tf.tname(it['type'])
        if tn == 'hkbClipGenerator':
            n = self.s(it, CLIP_ANIM)
            mm = CLIPRE.match(n or '')
            return [int(mm.group(1))] if mm else []
        out = []
        end = self.item_end(item_idx)
        for doff, tgt in self.tf.ptch.items():
            if it['off'] <= doff < end:
                out += self.clips_under(tgt, seen, depth + 1)
        return out

    def item_end(self, idx):
        if not hasattr(self, '_ends'):
            byoff = sorted(self.tf.items, key=lambda x: x['off'])
            self._ends = {}
            for k, it in enumerate(byoff):
                nxt = byoff[k + 1]['off'] if k + 1 < len(byoff) else len(self.tf.d) - self.tf.data_off
                self._ends[it['idx']] = nxt
        return self._ends[idx]

    def event_to_anim(self):
        tf = self.tf
        rows = {}
        for sm in tf.find_items('hkbStateMachine'):
            smname = self.s(sm, NAME_OFF)
            states = {}
            for sp in self.arr(sm, SM_STATES):
                if sp is None:
                    continue
                si = tf.items[sp]
                _o = tf.data_off + si['off'] + SI_ID
                if _o + 4 > len(tf.d):
                    continue
                sid = struct.unpack_from('<i', tf.d, _o)[0]
                states[sid] = (self.s(si, SI_NAME), self.ptr(si, SI_GEN))
            tia = self.ptr(sm, SM_WILD)
            arrays = [tia] if tia is not None else []
            for sp in self.arr(sm, SM_STATES):
                if sp is not None:
                    t = self.ptr(tf.items[sp], SI_TRANS)
                    if t is not None:
                        arrays.append(t)
            for a in arrays:
                body = self.ptr(tf.items[a], 0x18)
                if body is None:
                    continue
                bi = tf.items[body]
                for k in range(bi['count']):
                    base = tf.data_off + bi['off'] + k * TI_SIZE
                    if base + TI_TO + 4 > len(tf.d):
                        break
                    eid, to = struct.unpack_from('<ii', tf.d, base + TI_EVENT)
                    if not (0 <= eid < len(self.events)):
                        continue
                    st = states.get(to)
                    if not st:
                        continue
                    anims = sorted(set(self.clips_under(st[1])))
                    rows.setdefault(self.events[eid], []).append(
                        {'sm': smname, 'state': st[0], 'stateId': to, 'anims': anims})
        return rows


def graphs_of(d):
    return sorted(glob.glob(os.path.join(d, 'Behaviors', '*.hkx'))) if os.path.isdir(d) else [d]


if __name__ == '__main__':
    d = sys.argv[1]
    want = sys.argv[2] if len(sys.argv) > 2 else None
    allrows = {}
    for g in graphs_of(d):
        try:
            allrows.update(Graph(g).event_to_anim())
        except Exception as e:
            print(f'# {g}: {e}', file=sys.stderr)
    if want == '--json':
        print(json.dumps(allrows))
    else:
        for ev in sorted(allrows):
            for r in allrows[ev]:
                print(f"{ev}\t{r['sm']}\t{r['state']}\t{r['stateId']}\t{','.join(map(str,r['anims']))}")
