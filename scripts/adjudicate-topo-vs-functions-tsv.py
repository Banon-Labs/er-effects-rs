#!/usr/bin/env python3
"""Where the call-graph topology and `functions.tsv` disagree, ask the bytes which one is right.

The two signals share nothing: `functions.tsv` pairs by masked byte signature across `.pdata` and
never looks at callers; the topology pairing never looks at a byte. So a disagreement is a bug in
exactly one of them, and a third, independent test can say which.

The test, and what it is careful not to claim.  Both images now have Ghidra function bodies, so a
candidate is judged over its whole declared body rather than a fixed-length prefix: the normalised
instruction sequences must have the same length and be equal at every position. Length-anchored
equality, not a prefix ratio -- because a prefix ratio is precisely how the impostor at 0xaec480
came back identical over 56 instructions while the correct pair matched over 9. A longer look-alike
must not win, so length is part of the claim rather than a tie-break on top of it.

Normalisation blanks every numeric literal: displacements and immediates are exactly what 1.17 was
expected to move, so comparing them would refuse every correct pair. That makes an accept a shape
claim, never an identity claim -- adjacent same-shape siblings both accept, and the verdict then
rests on the rejection of the loser. When both candidates accept, this reports UNDECIDABLE and
takes no side.

  python3 scripts/adjudicate-topo-vs-functions-tsv.py --pairs DIR/topo-pairs.pickle \
      --a cg-1162.pickle --b cg-1170.pickle [--selftest]
"""
import argparse
import os
import pickle
import re
import sys

try:
    import capstone
except ImportError:
    os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
NUM = re.compile(r"0x[0-9a-f]+")


def norm_body(image, va, size, md):
    """Normalised instruction list over a Ghidra-declared body. None when it does not decode."""
    rva = va - BASE
    if rva < 0 or size <= 0 or rva + size > len(image):
        return None
    out = []
    consumed = 0
    for _addr, isize, mnem, ops in md.disasm_lite(image[rva:rva + size], va):
        out.append(mnem + " " + NUM.sub("#", ops))
        consumed += isize
    if consumed < size - 15:      # a body that mostly failed to decode is not evidence
        return None
    return out


def accepts(left, right):
    return left is not None and right is not None and left == right


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pairs")
    ap.add_argument("--a")
    ap.add_argument("--b")
    ap.add_argument("--ledger", default=os.path.join(
        ROOT, "docs", "recon", "rva-map-1162-to-1170.functions.tsv"))
    ap.add_argument("--image-a", default=os.path.join(ROOT, "eldenring-deobf.bin"))
    ap.add_argument("--image-b", default=os.path.join(ROOT, "eldenring-deobf-1.17.bin"))
    ap.add_argument("--tsv")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = False

    if args.selftest:
        return selftest(args, md)

    A = pickle.load(open(args.a, "rb"))
    B = pickle.load(open(args.b, "rb"))
    ia = open(args.image_a, "rb").read()
    ib = open(args.image_b, "rb").read()
    P = pickle.load(open(args.pairs, "rb"))
    pair, origin = P["pair"], P["origin"]

    ledger = {}
    for line in open(args.ledger, encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        p = line.split("\t")
        if len(p) < 2 or not p[0].startswith("0x") or not p[1].startswith("0x"):
            continue
        ledger[int(p[0], 16) + BASE] = int(p[1], 16) + BASE

    rows = []
    tally = {}
    for a, led in ledger.items():
        topo = pair.get(a)
        if topo is None or topo == led:
            continue
        o = origin.get(a, ("-", 0, "-"))
        if o[0] == "SEED":
            continue
        la = norm_body(ia, a, A["size"].get(a, 0), md)
        rt = norm_body(ib, topo, B["size"].get(topo, 0), md)
        rl = norm_body(ib, led, B["size"].get(led, 0), md)
        at, al = accepts(la, rt), accepts(la, rl)
        if at and not al:
            v = "TOPOLOGY"
        elif al and not at:
            v = "FUNCTIONS.TSV"
        elif at and al:
            v = "UNDECIDABLE (both shape-compatible)"
        else:
            v = "NEITHER (both rejected)"
        tally[v] = tally.get(v, 0) + 1
        rows.append((a, topo, led, o[0], o[2] if len(o) > 2 else "-", v,
                     len(la) if la else -1))
    print(f"disagreements adjudicated: {len(rows)}")
    for k in sorted(tally, key=lambda x: -tally[x]):
        print(f"  {k}: {tally[k]}")
    print()
    for r in sorted(rows, key=lambda x: x[5]):
        print("0x%x  topo 0x%x  tsv 0x%x  %-6s %-6s  %-36s insns=%d" % r)
    if args.tsv:
        with open(args.tsv, "w", encoding="utf-8") as fh:
            fh.write("# 1.16.2 VA\ttopology 1.17 VA\tfunctions.tsv 1.17 VA\trule\ttier"
                     "\tbyte verdict\t1.16.2 body insns\n")
            for r in rows:
                fh.write("0x%x\t0x%x\t0x%x\t%s\t%s\t%s\t%d\n" % r)
    return 0


def selftest(args, md):
    """Negative control: how often does the shape test accept a deliberately wrong destination?

    An accept rate is not a bug -- adjacent same-length siblings really do have equal masked
    bodies -- but it is the number that says how much an accept is worth, so it is measured
    rather than assumed.
    """
    A = pickle.load(open(args.a, "rb"))
    B = pickle.load(open(args.b, "rb"))
    ia = open(args.image_a, "rb").read()
    ib = open(args.image_b, "rb").read()
    ledger = {}
    for line in open(args.ledger, encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        p = line.split("\t")
        if len(p) < 2 or not p[0].startswith("0x") or not p[1].startswith("0x"):
            continue
        ledger[int(p[0], 16) + BASE] = int(p[1], 16) + BASE
    import random
    rng = random.Random(20260830)
    keys = [k for k in ledger if A["size"].get(k, 0) > 0]
    rng.shuffle(keys)
    ents = [e for e in B["entries"] if B["size"].get(e, 0) > 0]
    right = wrong_accept = tried = 0
    for k in keys[:400]:
        la = norm_body(ia, k, A["size"][k], md)
        if la is None:
            continue
        tried += 1
        if accepts(la, norm_body(ib, ledger[k], B["size"].get(ledger[k], 0), md)):
            right += 1
        bad = ledger[k]
        while bad == ledger[k]:
            bad = ents[rng.randrange(len(ents))]
        if accepts(la, norm_body(ib, bad, B["size"][bad], md)):
            wrong_accept += 1
    print(f"tried {tried}")
    print(f"  accepts the LEDGER's own destination:  {right}")
    print(f"  accepts a RANDOM wrong destination:    {wrong_accept}")
    ok = wrong_accept <= tried * 0.05 and right >= tried * 0.5
    print("SELFTEST OK" if ok else "SELFTEST FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
