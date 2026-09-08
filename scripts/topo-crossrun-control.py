#!/usr/bin/env python3
"""The control the ledger cannot give: two pairings whose seed sets are DISJOINT, compared on the
population `functions.tsv` does not cover.

Every accuracy number in this migration so far has been measured against `functions.tsv`, and
`functions.tsv` only exists for `.pdata`-declared functions. That is the easy half. The 145k EH
funclets and the `.pdata`-less leaves -- the whole reason for going to a call graph -- have never
had a measured error rate at all, and the temptation is to quote the ledger number for them.

Run the pairing twice with complementary halves of the ledger as seeds (`--holdout 0.5` and
`--holdout 0.5 --holdout-invert`). The two runs then share no seed. On a node neither run seeded
and the ledger never mentions, their agreement is independent evidence and their disagreement is
proof that at least one of them is wrong.

Read the output honestly: a disagreement rate D means the error rate is at least D/2, and errors
the two runs make identically are invisible to this test. It is a floor, not a measurement.

  python3 scripts/topo-crossrun-control.py --x DIRA/topo-pairs.pickle --y DIRB/topo-pairs.pickle \
      --graph-a cg-1162.pickle
"""
import argparse
import collections
import os
import pickle

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000


def classify(name):
    if name.startswith("FUN_") or name.startswith("thunk_FUN_"):
        return "auto"
    if name.startswith("Unwind@") or name.startswith("Catch_All@") or name.startswith("Catch@"):
        return "funclet"
    if name.startswith("FID_conflict:"):
        return "demangled"
    return "curated"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--x", required=True)
    ap.add_argument("--y", required=True)
    ap.add_argument("--graph-a", required=True)
    ap.add_argument("--ledger", default=os.path.join(
        ROOT, "docs", "recon", "rva-map-1162-to-1170.functions.tsv"))
    ap.add_argument("--consensus-out")
    a = ap.parse_args()

    led = set()
    for line in open(a.ledger, encoding="utf-8"):
        line = line.strip()
        if line and not line.startswith("#"):
            p = line.split("\t")
            if len(p) >= 2 and p[0].startswith("0x"):
                led.add(int(p[0], 16) + BASE)
    X = pickle.load(open(a.x, "rb"))
    Y = pickle.load(open(a.y, "rb"))
    A = pickle.load(open(a.graph_a, "rb"))
    px, ox = X["pair"], X["origin"]
    py, oy = Y["pair"], Y["origin"]
    sx = {k for k, v in ox.items() if v[0] == "SEED"}
    sy = {k for k, v in oy.items() if v[0] == "SEED"}
    print(f"seeds X={len(sx)} Y={len(sy)} overlap={len(sx & sy)} (must be 0)")

    byclass = collections.Counter()
    byrule = collections.defaultdict(lambda: [0, 0])
    consensus = {}
    for va, bx in px.items():
        if va in led or va not in py:
            continue
        if ox[va][0] == "SEED" or oy[va][0] == "SEED":
            continue
        same = bx == py[va]
        k = classify(A["name"].get(va, ""))
        byclass[(k, same)] += 1
        byrule[(ox[va][0], ox[va][2], ox[va][3])][0 if same else 1] += 1
        if same:
            consensus[va] = bx
    tot_a = sum(v for (k, s), v in byclass.items() if s)
    tot_d = sum(v for (k, s), v in byclass.items() if not s)
    print(f"\ncompared {tot_a + tot_d} nodes both runs derived and the ledger never mentions")
    print(f"  agree {tot_a}  disagree {tot_d}  ({100.0*tot_d/max(1,tot_a+tot_d):.3f}%)")
    print(f"  => error floor on this population: {100.0*tot_d/2/max(1,tot_a+tot_d):.3f}%")
    print("  by 1.16.2 name class:")
    for k in ("curated", "demangled", "funclet", "auto"):
        ok, bad = byclass[(k, True)], byclass[(k, False)]
        if ok + bad:
            print(f"    {k:10s} agree {ok:7d} disagree {bad:6d} ({100.0*bad/(ok+bad):.3f}%)")
    print("  by rule/tier:")
    for k in sorted(byrule, key=lambda x: -sum(byrule[x])):
        ok, bad = byrule[k]
        print(f"    {str(k):36s} agree {ok:7d} disagree {bad:6d} "
              f"({100.0*bad/max(1,ok+bad):.3f}%)")
    if a.consensus_out:
        with open(a.consensus_out, "wb") as fh:
            pickle.dump({"consensus": consensus}, fh, protocol=4)
        print(f"\nwrote {a.consensus_out} ({len(consensus)} seed-independent agreements)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
