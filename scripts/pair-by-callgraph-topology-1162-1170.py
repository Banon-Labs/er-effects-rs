#!/usr/bin/env python3
"""Pair 1.16.2 -> 1.17 ELDEN RING functions by call-graph topology, to a fixpoint.

Every prior 1.16.2 -> 1.17 map in this repo pairs a function by what its bytes look like:
masked signatures, `.pdata` extents, prologue shapes. That has a structural ceiling -- `.pdata`
declares nothing for 5.55 MB of `.text`, and thousands of ELDEN RING getters are byte-identical
to each other, so the evidence that would separate them is exactly the evidence a byte method
does not have. The leaf pass measured the ceiling: 27,160 of 96,218 leaves paired, 28.2%.

This pairs on position in the call graph instead. Two Ghidra function lists (366k nodes each,
from the 1.16.2 and 1.17 runtime dumps served on :8765 and :8767) supply the node sets that
`.pdata` could not; `build-callgraph-from-ghidra-funcs.py` supplies the edges. A confident pair
then constrains its neighbours, and the constraint propagates.

Three rules, all of which refuse rather than guess:

  Down   the i-th direct branch of `a` pairs with the i-th of `b`, once the already-paired
         callees of both have been aligned as a monotone anchor sequence. A caller whose paired
         callees do not align is discarded whole -- it is describing a different function.
  Up     `a` and `b` are called by the same (already-paired) set of callers, that set has at
         least MIN_KEY members, and no other node on either side has that caller set.
  DSET   `a` and `b` call the same (already-paired) set of callees, same two conditions.
  Order  both images emit functions in the same order, so a run of unpaired nodes bracketed by
         two consecutive pairs has a counterpart run bracketed by their images. When the two runs
         are the same length and each candidate agrees on body shape, the run is paired
         positionally. Position alone is not enough -- ELDEN RING has thousands of interchangeable
         3-instruction getters, and the shape gate is what stops the bracket pairing all of them.
  BRKT   inside the same bracket, a node whose numeric-blanked body HASH occurs exactly once on
         each side. Run before order, because it is what catches the two ways position lies: a
         run of near-identical functions where the whole bracket slid by one, and two adjacent
         functions the two builds emitted in opposite order. Both were observed and both were
         decided against order by the bytes.

A proposal becomes a pair only when it is unanimous (no caller proposes a different target) and
INJECTIVE (no other source claims the same target). Two sources claiming one target both lose;
the more-voted one does not win. Iterating those three to a fixpoint is the whole method.

Why this is the right tool for the known hazards
  * `GAME_HEAP_ALLOC` is one of two byte-identical 19-byte functions 0xe0 apart. No byte method
    can separate them; a caller vote separated them 3766 to 0.
  * An impostor at 0xaec480 verified identical over 56 instructions while the correct pair
    verified over 9. Instruction count is not confidence. Graph position is not fooled by a
    longer look-alike.

Measured, not asserted
  --holdout F withholds a random fraction of the seed ledger, runs to fixpoint without it, and
  reports how often the topology re-derived the withheld answer. That number, per rule and per
  shape tier, is the only reason to believe any row below.

  python3 scripts/pair-by-callgraph-topology-1162-1170.py --pair --out-dir DIR --assert-known
  python3 scripts/pair-by-callgraph-topology-1162-1170.py --holdout 0.5 --out-dir DIR
  python3 scripts/pair-by-callgraph-topology-1162-1170.py --selftest
"""
import argparse
import os
import pickle
import random
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
FUNCTIONS_TSV = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.functions.tsv")
VERIFIED_TSV = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.verified.tsv")

# A caller/callee set has to be big enough that agreeing on it is not a coincidence. With
# MIN_KEY=1 every function called from exactly one paired place matches every other such
# function; the uniqueness test then throws all of them away, so the only effect of lowering it
# is noise. 2 is the smallest set that is a claim.
# The only two 1.16.2 -> 1.17 mappings in this workspace established by reading the live 1.17
# process rather than inferred from an image. `--assert-known` fails the run if the pairing cannot
# reproduce them: a method that cannot re-derive a known answer has no business proposing unknown
# ones. The second is a call site 0x28 inside `GetWwiseSettings`, so it is checked by carrying the
# offset through that function's pair -- which also exercises the offset carry the repo needs for
# its mid-function constants.
KNOWN_LIVE = {0x14025F5F0: 0x14025F5D0}
KNOWN_LIVE_INSIDE = {0x1422222D8: 0x142224238}

MIN_KEY = 2
# A bracket wide enough to hold this many unpaired nodes is not really a bracket -- it is a
# region, and positional pairing inside a region is guessing. Measured: the error rate climbs
# with gap width, so the cut is reported per width bucket rather than assumed.
MAX_GAP = 64
# BRKT's body hash is evidence at any bracket width. Order's position is not: it is a guess whose
# only support is that nothing else moved, and the wider the undecided residue the less that is
# worth. Measured on two disjoint-seed runs, order over unbounded residues disagreed with itself
# on 2.0% of the population `functions.tsv` cannot check -- eight times its rate on the population
# it can. Bounding the residue is the difference between those two numbers.
MAX_ORDER_RESIDUE = 4


def load_graph(path):
    with open(path, "rb") as fh:
        return pickle.load(fh)


def invert_callees(g):
    callers = {}
    for a, outs in g["callees"].items():
        for tgt, _kind in outs:
            callers.setdefault(tgt, set()).add(a)
    return callers


def load_functions_tsv(path=FUNCTIONS_TSV):
    pairs = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 2 or not parts[0].startswith("0x") or not parts[1].startswith("0x"):
                continue
            pairs[int(parts[0], 16) + BASE] = int(parts[1], 16) + BASE
    return pairs


def load_verified_tsv(path=VERIFIED_TSV):
    pairs = {}
    if not os.path.exists(path):
        return pairs
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) < 2 or not parts[0].startswith("0x") or not parts[1].startswith("0x"):
                continue
            pairs[int(parts[0], 16)] = int(parts[1], 16)
    return pairs


def shape_ok(sa, sb):
    """Body-shape agreement, used only to tier a pair, never to make one.

    n_direct / n_indirect are counts of branch instructions, which a relocated-but-unchanged
    function preserves exactly. Instruction count is allowed to drift a little because the two
    builds differ in scheduling around unchanged code.
    """
    if sa is None or sb is None:
        return False
    if sa[1] != sb[1] or sa[2] != sb[2]:
        return False
    ia, ib = sa[0], sb[0]
    return abs(ia - ib) <= max(2, (ia + ib) // 40)


def longest_increasing(pts):
    """Longest strictly-increasing-in-j subsequence of (i, j), i already ascending.

    Patience sorting, O(n log n). Everything it drops is a pair whose two images sit in a
    different relative order in the two builds -- real, but not usable as a bracket edge.
    """
    import bisect
    if not pts:
        return []
    tails = []      # tails[k] = smallest possible j ending an increasing run of length k+1
    tails_idx = []  # index into pts of that run's last element
    prev = [-1] * len(pts)
    for n, (_i, j) in enumerate(pts):
        k = bisect.bisect_left(tails, j)
        if k == len(tails):
            tails.append(j)
            tails_idx.append(n)
        else:
            tails[k] = j
            tails_idx[k] = n
        prev[n] = tails_idx[k - 1] if k else -1
    out = []
    n = tails_idx[-1]
    while n >= 0:
        out.append(pts[n])
        n = prev[n]
    out.reverse()
    return out


def shape_key(st, size):
    """The whole measurable body shape, used only inside an already-bracketed gap."""
    if st is None:
        return None
    return (st[0], st[1], st[2], st[3], st[4], size)


def align_callees(ca, cb, pair):
    """Monotone anchor alignment of two callee sequences.

    Returns (segments, missing) where `segments` is a list of (a_slice, b_slice) index ranges
    between consecutive matched anchors that have equal length -- the only places a positional
    proposal is defensible -- and `missing` counts paired callees of `a` whose image could not
    be found in order in `b`.
    """
    anchors = []
    j = 0
    missing = 0
    for i, (ta, ka) in enumerate(ca):
        mb = pair.get(ta)
        if mb is None:
            continue
        k = j
        found = -1
        while k < len(cb):
            if cb[k][0] == mb and cb[k][1] == ka:
                found = k
                break
            k += 1
        if found < 0:
            missing += 1
            continue
        anchors.append((i, found))
        j = found + 1
    return anchors, missing


def propose_down(ca, cb, anchors, missing, allow_open_ends):
    """Positional proposals inside anchored segments. Never across a length change."""
    out = []
    bounds = []
    prev = (-1, -1)
    for ai, bi in anchors:
        bounds.append((prev[0] + 1, ai, prev[1] + 1, bi))
        prev = (ai, bi)
    if allow_open_ends and missing == 0:
        bounds.append((prev[0] + 1, len(ca), prev[1] + 1, len(cb)))
    elif not anchors and missing == 0 and len(ca) == len(cb) and allow_open_ends:
        bounds = [(0, len(ca), 0, len(cb))]
    for a0, a1, b0, b1 in bounds:
        if (a1 - a0) != (b1 - b0) or a1 <= a0:
            continue
        for d in range(a1 - a0):
            ta, ka = ca[a0 + d]
            tb, kb = cb[b0 + d]
            if ka != kb:
                continue
            out.append((ta, tb))
    return out


def run_fixpoint(A, B, seeds, max_rounds=40, strict_callers=True, verbose=True,
                 log=print, use_order=True, max_gap=MAX_GAP,
                 max_order_residue=MAX_ORDER_RESIDUE):
    pair = dict(seeds)
    rev = {}
    for a, b in pair.items():
        rev[b] = a
    origin = {a: ("SEED", 0) for a in pair}

    cA, cB = A["callees"], B["callees"]
    stA, stB = A["stats"], B["stats"]
    bhAll, bhBll = A.get("bodyhash", {}), B.get("bodyhash", {})
    callersA = invert_callees(A)
    callersB = invert_callees(B)
    nodesA = set(A["entries"])
    nodesB = set(B["entries"])
    # nodes worth revisiting -- a caller only produces new proposals when it or one of its
    # callees changed state, so after round 1 the sweep is restricted to that frontier.
    dirty_callers = set(pair)
    for a in list(pair):
        dirty_callers |= callersA.get(a, set())

    counts = {}
    for rnd in range(max_rounds):
        t0 = time.time()
        proposals = {}      # a -> set of b
        claims = {}         # b -> set of a
        rule_of = {}

        def offer(a, b, rule):
            if a in pair or b in rev or a not in nodesA or b not in nodesB:
                return
            proposals.setdefault(a, set()).add(b)
            claims.setdefault(b, set()).add(a)
            rule_of.setdefault((a, b), rule)

        # ---- Down -------------------------------------------------------------
        sweep = [a for a in dirty_callers if a in pair]
        for a in sweep:
            b = pair[a]
            ca = cA.get(a) or ()
            cb = cB.get(b) or ()
            if not ca or not cb:
                continue
            anchors, missing = align_callees(ca, cb, pair)
            if strict_callers and missing:
                continue
            for ta, tb in propose_down(ca, cb, anchors, missing, allow_open_ends=(missing == 0)):
                offer(ta, tb, "DOWN")

        # ---- Up / DSET --------------------------------------------------------
        # Keyed indexes over the whole unpaired population; cheap enough to rebuild each round
        # and it keeps the rule honest (uniqueness is measured against everything, not a frontier).
        for keyfn, rule in ((("up"), "UP"), (("dset"), "DSET")):
            idxA = {}
            idxB = {}
            if rule == "UP":
                for a in nodesA:
                    if a in pair:
                        continue
                    k = frozenset(pair[c] for c in callersA.get(a, ()) if c in pair)
                    if len(k) >= MIN_KEY:
                        idxA.setdefault(k, []).append(a)
                for b in nodesB:
                    if b in rev:
                        continue
                    k = frozenset(c for c in callersB.get(b, ()) if c in rev)
                    if len(k) >= MIN_KEY:
                        idxB.setdefault(k, []).append(b)
            else:
                for a in nodesA:
                    if a in pair:
                        continue
                    k = frozenset(pair[t] for t, _ in cA.get(a, ()) if t in pair)
                    if len(k) >= MIN_KEY:
                        idxA.setdefault(k, []).append(a)
                for b in nodesB:
                    if b in rev:
                        continue
                    k = frozenset(t for t, _ in cB.get(b, ()) if t in rev)
                    if len(k) >= MIN_KEY:
                        idxB.setdefault(k, []).append(b)
            for k, alist in idxA.items():
                blist = idxB.get(k)
                if not blist or len(alist) != 1 or len(blist) != 1:
                    continue
                offer(alist[0], blist[0], rule)

        # ---- Order / BRKT -----------------------------------------------------
        # Both linkers emit in source order, so consecutive pairs bracket each other's gaps. A
        # gap only qualifies when every node strictly inside it is unpaired on both sides: a
        # paired node inside a gap whose image is outside it means the local order broke, and
        # pairing across that is how a bracket invents an answer.
        if use_order:
            eaL, ebL = A["entries"], B["entries"]
            bhA, bhB = A.get("bodyhash", {}), B.get("bodyhash", {})
            ia = {v: i for i, v in enumerate(eaL)}
            ib = {v: i for i, v in enumerate(ebL)}
            # The anchor spine must be the longest increasing run of (A index, B index), not a
            # greedy left-to-right one. Greedy is catastrophically wrong here: one early pair
            # whose B index is large swallows everything after it, and measured on a real run it
            # cut 124,188 pairs down to 384 anchors -- which silently turned the whole bracket
            # rule off while still reporting that it was on.
            pts = []
            for i, av in enumerate(eaL):
                bv = pair.get(av)
                if bv is None:
                    continue
                j = ib.get(bv)
                if j is not None:
                    pts.append((i, j))
            anchors = longest_increasing(pts)
            for k in range(len(anchors) - 1):
                i0, j0 = anchors[k]
                i1, j1 = anchors[k + 1]
                ga = eaL[i0 + 1:i1]
                gb = ebL[j0 + 1:j1]
                if not ga and not gb:
                    continue
                if len(ga) > max_gap or len(gb) > max_gap:
                    continue
                if any(v in pair for v in ga) or any(v in rev for v in gb):
                    continue
                # BRKT first: an exact body hash unique on both sides of the bracket beats
                # position, and disagrees with it exactly where position is wrong.
                ka = {}
                kb = {}
                for av in ga:
                    h = bhA.get(av)
                    if h is not None:
                        ka.setdefault(h, []).append(av)
                for bv in gb:
                    h = bhB.get(bv)
                    if h is not None:
                        kb.setdefault(h, []).append(bv)
                taken_a = set()
                taken_b = set()
                for key, al in ka.items():
                    bl = kb.get(key)
                    if bl and len(al) == 1 and len(bl) == 1:
                        offer(al[0], bl[0], "BRKT")
                        taken_a.add(al[0])
                        taken_b.add(bl[0])
                # Order on the residue only. Removing the hash-decided nodes from both sides
                # first is what lets a bracket survive an insertion or a swap instead of
                # sliding every remaining node by one.
                ra = [v for v in ga if v not in taken_a]
                rb = [v for v in gb if v not in taken_b]
                if len(ra) == len(rb) and len(ra) <= max_order_residue:
                    for av, bv in zip(ra, rb):
                        if shape_ok(stA.get(av), stB.get(bv)):
                            offer(av, bv, "ORDER")

        # ---- accept: unanimous and injective ----------------------------------
        accepted = 0
        new_dirty = set()
        for a, bs in proposals.items():
            if len(bs) != 1:
                continue
            b = next(iter(bs))
            if len(claims.get(b, ())) != 1:
                continue
            rule = rule_of[(a, b)]
            tier = "STRICT" if shape_ok(stA.get(a), stB.get(b)) else "LOOSE"
            ha, hb = bhAll.get(a), bhBll.get(b)
            byte = "BYTE-EQ" if (ha is not None and ha == hb) else "BYTE-DIFF"
            pair[a] = b
            rev[b] = a
            origin[a] = (rule, rnd + 1, tier, byte)
            counts[(rule, tier)] = counts.get((rule, tier), 0) + 1
            accepted += 1
            new_dirty.add(a)
            new_dirty |= callersA.get(a, set())
        dirty_callers = new_dirty
        if verbose:
            log(f"round {rnd+1}: +{accepted}  total={len(pair)}  ({time.time()-t0:.0f}s)")
        if accepted == 0:
            break
    return pair, origin, counts


def summarise(origin):
    agg = {}
    for a, o in origin.items():
        if o[0] == "SEED":
            agg["SEED"] = agg.get("SEED", 0) + 1
        else:
            agg[(o[0], o[2], o[3])] = agg.get((o[0], o[2], o[3]), 0) + 1
    return agg


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", default=None, help="1.16.2 call-graph pickle")
    ap.add_argument("--b", default=None, help="1.17 call-graph pickle")
    ap.add_argument("--out-dir", default=None)
    ap.add_argument("--pair", action="store_true")
    ap.add_argument("--holdout", type=float, default=None,
                    help="withhold this fraction of the seed ledger and score against it")
    ap.add_argument("--seed-frac", type=float, default=None,
                    help="use only this fraction of the ledger as seeds (bootstrap control)")
    ap.add_argument("--rng", type=int, default=20260830)
    ap.add_argument("--holdout-invert", action="store_true",
                    help="seed with the half --holdout would have withheld, and withhold the "
                         "half it would have seeded. Two runs of the same --holdout with and "
                         "without this share NO seed, so where they agree on a node the ledger "
                         "does not cover, that agreement is independent evidence.")
    ap.add_argument("--loose-callers", action="store_true",
                    help="do NOT discard a caller whose paired callees fail to align")
    ap.add_argument("--max-rounds", type=int, default=40)
    ap.add_argument("--max-gap", type=int, default=MAX_GAP)
    ap.add_argument("--max-order-residue", type=int, default=MAX_ORDER_RESIDUE)
    ap.add_argument("--no-order", action="store_true",
                    help="disable the ORDER/BRKT bracket rules")
    ap.add_argument("--assert-known", action="store_true",
                    help="fail unless the run reproduces the two live-verified mappings")
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()

    if a.selftest:
        return selftest()

    A = load_graph(a.a)
    B = load_graph(a.b)
    ledger = load_functions_tsv()
    nodesA, nodesB = set(A["entries"]), set(B["entries"])
    ledger = {k: v for k, v in ledger.items() if k in nodesA and v in nodesB}

    rnd = random.Random(a.rng)
    keys = sorted(ledger)
    withheld = {}
    if a.holdout:
        rnd.shuffle(keys)
        n = int(len(keys) * a.holdout)
        drop = keys[n:] if a.holdout_invert else keys[:n]
        for k in drop:
            withheld[k] = ledger.pop(k)
    elif a.seed_frac:
        rnd.shuffle(keys)
        n = int(len(keys) * a.seed_frac)
        keep = set(keys[:n])
        withheld = {k: v for k, v in ledger.items() if k not in keep}
        ledger = {k: v for k, v in ledger.items() if k in keep}

    print(f"nodes A={len(nodesA)} B={len(nodesB)}  seeds={len(ledger)}  withheld={len(withheld)}",
          flush=True)
    pair, origin, _ = run_fixpoint(A, B, ledger, max_rounds=a.max_rounds,
                                   strict_callers=not a.loose_callers,
                                   use_order=not a.no_order, max_gap=a.max_gap,
                                   max_order_residue=a.max_order_residue)

    print("\n--- pairs by rule/tier ---")
    for k, v in sorted(summarise(origin).items(), key=lambda kv: -kv[1]):
        print(f"  {k}: {v}")
    print(f"  TOTAL: {len(pair)}")

    if withheld:
        print("\n--- control: re-derived the withheld ledger? ---")
        tally = {}
        for k, truth in withheld.items():
            got = pair.get(k)
            o = origin.get(k)
            if got is None:
                key = ("(not derived)", "-")
                tally[key] = tally.get(key, [0, 0])
                tally[key][0] += 0
                continue
            key = (o[0], o[2], o[3]) if o and o[0] != "SEED" else ("SEED", "-", "-")
            t = tally.setdefault(key, [0, 0])
            if got == truth:
                t[0] += 1
            else:
                t[1] += 1
        tot_a = tot_d = 0
        for k in sorted(tally, key=lambda x: -(tally[x][0] + tally[x][1])):
            ok, bad = tally[k]
            if ok + bad == 0:
                continue
            tot_a += ok
            tot_d += bad
            print(f"  {k}: agree {ok}  disagree {bad}  ({100.0*bad/max(1,ok+bad):.3f}% wrong)")
        print(f"  OVERALL: agree {tot_a}  disagree {tot_d}  "
              f"({100.0*tot_d/max(1,tot_a+tot_d):.3f}% wrong)  "
              f"coverage {tot_a+tot_d}/{len(withheld)}")

    if a.assert_known:
        import bisect
        ents = list(A["entries"])
        bad = []
        for src, want in KNOWN_LIVE.items():
            got = pair.get(src)
            print(f"known-live 0x{src:x} -> {got and hex(got)} (want 0x{want:x})")
            if got != want:
                bad.append(src)
        for src, want in KNOWN_LIVE_INSIDE.items():
            i = bisect.bisect_right(ents, src) - 1
            f = ents[i] if i >= 0 else None
            base = pair.get(f) if f is not None else None
            got = base + (src - f) if base is not None else None
            print(f"known-live 0x{src:x} (+0x{src - f:x} inside 0x{f:x}) -> "
                  f"{got and hex(got)} (want 0x{want:x})")
            if got != want:
                bad.append(src)
        if bad:
            print("ASSERT-KNOWN FAILED: " + ", ".join(hex(x) for x in bad))
            return 3
        print("assert-known OK")

    if a.out_dir:
        os.makedirs(a.out_dir, exist_ok=True)
        with open(os.path.join(a.out_dir, "topo-pairs.pickle"), "wb") as fh:
            pickle.dump({"pair": pair, "origin": origin}, fh, protocol=4)
        with open(os.path.join(a.out_dir, "topo-pairs.tsv"), "w", encoding="utf-8") as fh:
            fh.write("# 1.16.2 VA\t1.17 VA\trule\tround\tshape tier\tbyte\t1.16.2 name\n")
            nm = A["name"]
            for k in sorted(pair):
                o = origin.get(k, ("?", 0, "-", "-"))
                fh.write("0x%x\t0x%x\t%s\t%s\t%s\t%s\t%s\n"
                         % (k, pair[k], o[0], o[1], o[2] if len(o) > 2 else "-",
                            o[3] if len(o) > 3 else "-", nm.get(k, "")))
        print(f"\nwrote {a.out_dir}/topo-pairs.tsv")
    return 0


def selftest():
    """Gates on synthetic graphs: each rule fires, and each refusal actually refuses."""
    fails = []

    def check(name, cond):
        print(("PASS " if cond else "FAIL ") + name)
        if not cond:
            fails.append(name)

    def mk(callees, sizes=None, bodies=None):
        ents = sorted(callees)
        return {"entries": tuple(ents), "size": {e: 16 for e in ents},
                "name": {e: "f%x" % e for e in ents},
                "callees": {e: tuple((t, "c") for t in callees[e]) for e in ents},
                "bodyhash": {e: (bodies or {}).get(e) for e in ents},
                "stats": {e: (sizes or {}).get(e, (5, len(callees[e]), 0, 0, 16)) for e in ents}}

    # DOWN: one paired caller, equal-length callee lists, positional propagation.
    A = mk({1: [10, 11], 10: [], 11: []})
    B = mk({101: [110, 111], 110: [], 111: []})
    p, o, _ = run_fixpoint(A, B, {1: 101}, verbose=False)
    check("DOWN pairs positionally", p.get(10) == 110 and p.get(11) == 111)

    # Down must not cross a length change with no anchors.
    A = mk({1: [10, 11], 10: [], 11: []})
    B = mk({101: [110, 111, 112], 110: [], 111: [], 112: []})
    p, _, _ = run_fixpoint(A, B, {1: 101}, verbose=False)
    check("DOWN refuses a length change", 10 not in p and 11 not in p)

    # Two callers disagreeing -> both proposals die (unanimity).
    A = mk({1: [10], 2: [10], 10: []})
    B = mk({101: [110], 102: [111], 110: [], 111: []})
    p, _, _ = run_fixpoint(A, B, {1: 101, 2: 102}, verbose=False)
    check("split vote refuses", 10 not in p)

    # Two sources claiming one target -> both lose (injectivity).
    A = mk({1: [10], 2: [11], 10: [], 11: []})
    B = mk({101: [110], 102: [110], 110: []})
    p, _, _ = run_fixpoint(A, B, {1: 101, 2: 102}, verbose=False)
    check("collision refuses both", 10 not in p and 11 not in p)

    # The GAME_HEAP_ALLOC hazard: two byte-identical siblings, separated only by who calls them.
    A = mk({1: [10], 2: [11], 10: [], 11: []})
    B = mk({101: [110], 102: [111], 110: [], 111: []})
    p, _, _ = run_fixpoint(A, B, {1: 101, 2: 102}, verbose=False)
    check("byte-identical siblings separate by caller", p.get(10) == 110 and p.get(11) == 111)

    # UP: same paired caller set of size >= MIN_KEY, unique on both sides.
    A = mk({1: [10], 2: [10], 3: [12], 10: [], 12: []})
    B = mk({101: [110], 102: [110], 103: [112], 110: [], 112: []})
    p, o, _ = run_fixpoint(A, B, {1: 101, 2: 102, 3: 103}, verbose=False)
    check("UP pairs on a caller set", p.get(10) == 110)

    # MIN_KEY: a single paired caller is not a key (that case is down's job, and down needs
    # positional agreement); with a length change down refuses and up must not rescue it.
    A = mk({1: [10, 20], 10: [], 20: []})
    B = mk({101: [110, 120, 130], 110: [], 120: [], 130: []})
    p, _, _ = run_fixpoint(A, B, {1: 101}, verbose=False)
    check("a lone caller is not an UP key", 10 not in p and 20 not in p)

    # DSET: same paired callee set.
    A = mk({1: [10, 11], 10: [], 11: []})
    B = mk({101: [110, 111], 110: [], 111: []})
    p, _, _ = run_fixpoint(A, B, {10: 110, 11: 111}, verbose=False)
    check("DSET pairs on a callee set", p.get(1) == 101)

    # A caller whose paired callee is absent from the 1.17 side is discarded whole.
    A = mk({1: [10, 11], 10: [], 11: []})
    B = mk({101: [999, 111], 110: [], 111: [], 999: []})
    p, _, _ = run_fixpoint(A, B, {1: 101, 10: 110}, verbose=False)
    check("misaligned caller is discarded", 11 not in p)

    # ORDER: one unpaired node bracketed by two pairs, same shape -> paired positionally.
    A = mk({1: [], 5: [], 9: []})
    B = mk({101: [], 105: [], 109: []})
    p, o, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("ORDER pairs inside a bracket", p.get(5) == 105 and o[5][0] == "ORDER")

    # Order must refuse when the shapes disagree -- position alone is not evidence.
    A = mk({1: [], 5: [], 9: []}, {5: (5, 0, 0, 0, 16)})
    B = mk({101: [], 105: [], 109: []}, {105: (5, 0, 3, 0, 16)})
    p, _, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("ORDER refuses a shape mismatch", 5 not in p)

    # Order must not run positionally across gaps of different length.
    A = mk({1: [], 5: [], 9: []})
    B = mk({101: [], 104: [], 105: [], 109: []})
    p, _, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("ORDER refuses unequal gap lengths", p.get(5) != 104)

    # BRKT: unequal gap, one body hash occurring exactly once on each side.
    A = mk({1: [], 5: [], 9: []}, bodies={5: b"X"})
    B = mk({101: [], 104: [], 105: [], 109: []}, bodies={104: b"Y", 105: b"X"})
    p, o, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("BRKT pairs on a unique body hash", p.get(5) == 105 and o[5][0] == "BRKT")

    # BRKT must refuse when the body hash is not unique inside the bracket.
    A = mk({1: [], 5: [], 6: [], 9: []}, bodies={5: b"X", 6: b"X"})
    B = mk({101: [], 104: [], 105: [], 106: [], 109: []},
           bodies={104: b"X", 105: b"X", 106: b"X"})
    p, _, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("BRKT refuses an ambiguous body hash", 5 not in p and 6 not in p)

    # The regression that caused this rule. Two adjacent functions the two builds emitted in the
    # opposite order. Position says 5->105, 6->106; the bytes say 5->106, 6->105, and the bytes
    # are right. Observed for real at 0x1407d9550 / 0x1407d95c0.
    A = mk({1: [], 5: [], 6: [], 9: []}, bodies={5: b"P", 6: b"Q"})
    B = mk({101: [], 105: [], 106: [], 109: []}, bodies={105: b"Q", 106: b"P"})
    p, o, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("a swapped pair follows the bytes, not the position",
          p.get(5) == 106 and p.get(6) == 105)

    # ...and order still finishes the residue the hashes could not decide.
    A = mk({1: [], 5: [], 6: [], 9: []}, bodies={5: b"P"})
    B = mk({101: [], 105: [], 106: [], 109: []}, bodies={105: b"P"})
    p, o, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("ORDER finishes the residue after BRKT",
          p.get(5) == 105 and p.get(6) == 106 and o[6][0] == "ORDER")

    # Order must refuse a residue wider than the cap -- position over a long undecided run is
    # a guess, and it measured eight times worse than the same rule over a short one.
    A = mk({0: [], 1: [], 2: [], 3: [], 4: [], 5: [], 9: []})
    B = mk({100: [], 101: [], 102: [], 103: [], 104: [], 105: [], 109: []})
    p, _, _ = run_fixpoint(A, B, {0: 100, 9: 109}, verbose=False, max_order_residue=2)
    check("ORDER refuses an over-wide residue", 3 not in p)
    p, _, _ = run_fixpoint(A, B, {0: 100, 9: 109}, verbose=False, max_order_residue=8)
    check("...and accepts it under a wider cap", p.get(3) == 103)

    # A pair carries whether the bytes agreed, independently of which rule made it.
    A = mk({1: [], 5: [], 9: []}, bodies={5: b"X"})
    B = mk({101: [], 105: [], 109: []}, bodies={105: b"X"})
    p, o, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("byte agreement is recorded on the pair", o[5][3] == "BYTE-EQ")
    A = mk({1: [], 5: [], 9: []}, bodies={5: b"X"})
    B = mk({101: [], 105: [], 109: []}, bodies={105: b"Z"})
    p, o, _ = run_fixpoint(A, B, {1: 101, 9: 109}, verbose=False)
    check("byte disagreement is recorded too", o[5][3] == "BYTE-DIFF")

    # A bracket containing a node already paired outside it is not a bracket.
    A = mk({1: [], 5: [], 6: [], 9: []})
    B = mk({101: [], 105: [], 106: [], 109: [], 200: []})
    p, _, _ = run_fixpoint(A, B, {1: 101, 9: 109, 5: 200}, verbose=False)
    check("ORDER refuses a gap with an out-of-order pair in it", 6 not in p)

    # The anchor spine is the longest increasing run, not a greedy one. One early out-of-order
    # pair must not blind the bracket rule to everything after it.
    got = [j for _i, j in longest_increasing([(0, 900), (1, 1), (2, 2), (3, 3), (4, 4)])]
    check("anchor spine is LIS, not greedy", got == [1, 2, 3, 4])
    check("LIS keeps a strictly increasing run whole",
          len(longest_increasing([(i, i) for i in range(50)])) == 50)

    # ...and the whole-pipeline consequence: a pair that sorts out of order early must not stop
    # order pairing a bracket later on.
    A = mk({1: [], 2: [], 5: [], 9: []})
    B = mk({50: [], 101: [], 105: [], 109: []})
    p, _, _ = run_fixpoint(A, B, {1: 50, 2: 101, 9: 109}, verbose=False)
    check("an out-of-order pair does not disable later brackets", p.get(5) == 105)

    # Shape tier: equal branch counts -> strict, different -> loose.
    check("shape_ok separates on n_direct", not shape_ok((5, 2, 0, 0, 16), (5, 3, 0, 0, 16)))
    check("shape_ok separates on n_indirect", not shape_ok((5, 2, 0, 0, 16), (5, 2, 1, 0, 16)))
    check("shape_ok tolerates small insn drift", shape_ok((50, 2, 0, 0, 16), (51, 2, 0, 0, 16)))
    check("shape_ok rejects big insn drift", not shape_ok((50, 2, 0, 0, 16), (90, 2, 0, 0, 16)))

    print(("\nSELFTEST FAILED: " + ", ".join(fails)) if fails else "\nSELFTEST OK")
    return 1 if fails else 0


if __name__ == "__main__":
    raise SystemExit(main())
