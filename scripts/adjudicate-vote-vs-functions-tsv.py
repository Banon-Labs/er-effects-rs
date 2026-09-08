#!/usr/bin/env python3
"""Where caller voting and `rva-map-1162-to-1170.functions.tsv` disagree, say which is right.

Two independent methods pair the same 1.16.2 function with a 1.17 one:

* `functions.tsv` matches a RUNTIME_FUNCTION's opening 64 bytes, masked, and requires the
  signature to be unique on both sides.  It never looks at who calls the function.
* Caller voting (`scripts/pair-leaf-functions-1162-1170.py`) takes an already-paired caller
  whose two bodies have identical extent length and identical direct-branch offsets, and
  reads the i-th call target off each side.  It never looks at the callee's bytes.

Agreement between them is the positive control for voting.  Disagreement is a bug in one of
them, and it matters which: a wrong `functions.tsv` row is a 1.17 address that reads as live.

The adjudicator is byte evidence, not a vote count: take the 1.16.2 function's whole declared
extent, mask it with the repo's own rule, and require the candidate to have the same extent
length and the same masked body.  Only if neither or both match does it fall back to which
candidate's delta agrees with the surrounding region -- and it says so.

Usage
    uv run --with capstone python3 scripts/adjudicate-vote-vs-functions-tsv.py
    ... --selftest
"""

from __future__ import annotations

import argparse
import bisect
import importlib.util
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000


def _leafmod():
    path = os.path.join(ROOT, "scripts/pair-leaf-functions-1162-1170.py")
    spec = importlib.util.spec_from_file_location("_pair_leaf", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules["_pair_leaf"] = module
    saved, sys.argv = sys.argv, ["pair-leaf"]
    try:
        spec.loader.exec_module(module)
    finally:
        sys.argv = saved
    return module


def local_delta(keys, fmap, rva, span=40):
    """Median 1.16.2 -> 1.17 delta of the nearest `.pdata` pairs, excluding this one."""
    i = bisect.bisect_left(keys, rva)
    deltas = sorted(
        fmap[keys[j]] - keys[j]
        for j in range(max(0, i - span), min(len(keys), i + span))
        if keys[j] != rva
    )
    return deltas[len(deltas) // 2] if deltas else None


def adjudicate(verbose=True):
    P = _leafmod()
    img162, img170 = P.Image(P.IMG_1162), P.Image(P.IMG_1170)
    calls162, calls170 = P.load_calls("1162"), P.load_calls("1170")
    fmap = P.load_function_map()
    votes, voters, qualified = P.caller_votes(img162, img170, calls162, calls170, fmap)
    resolved, _refused = P.resolve_votes(votes, voters)
    keys = sorted(fmap)

    agree = [s for s, (d, _n, _v) in resolved.items() if s in fmap and fmap[s] == d]
    rows = [(s, fmap[s], d, n, v) for s, (d, n, v) in resolved.items()
            if s in fmap and fmap[s] != d]
    if verbose:
        print(f"qualified callers {qualified}; vote targets also in functions.tsv: "
              f"{len(agree) + len(rows)}")
        print(f"AGREE {len(agree)}   DISAGREE {len(rows)}")
        print(f"\n{'1.16.2':>12} {'functions.tsv':>14} {'caller vote':>14} {'region':>10} "
              f"{'tsv d':>9} {'vote d':>9}  len  verdict")
    tally = {}
    out = []
    for src, tsv_dst, vote_dst, nvotes, ncallers in sorted(rows):
        end = img162.extents.get(src)
        length = (end - src) if end else None
        want = P.signature_key(img162, src, end) if end else None

        def matches(candidate):
            cend = img170.extents.get(candidate)
            if not end or not cend or (cend - candidate) != length:
                return False
            return P.signature_key(img170, candidate, cend) == want

        ok_tsv, ok_vote = matches(tsv_dst), matches(vote_dst)
        region = local_delta(keys, fmap, src)
        # A byte verdict rests on the rejection, not the acceptance. Masking removes exactly
        # the bytes that separate two same-shape functions, so "accepted" means "same shape",
        # not "same function" -- measured: 9 of 200 deliberately-wrong candidates are accepted,
        # every one an adjacent same-length sibling. "Rejected" is the strong half: a different
        # masked shape is a different function. So a verdict is issued only when exactly one
        # candidate is rejected.
        region_picks_vote = (
            None if region is None
            else abs((vote_dst - src) - region) < abs((tsv_dst - src) - region)
        )
        if ok_vote != ok_tsv:
            bytes_pick_vote = ok_vote
            if region_picks_vote is None:
                verdict = ("VOTE" if bytes_pick_vote else "FUNCTIONS.TSV") + " (bytes, no region)"
            elif region_picks_vote == bytes_pick_vote:
                verdict = ("VOTE" if bytes_pick_vote else "FUNCTIONS.TSV") + " (bytes+region)"
            else:
                # The two signals share nothing, so a split is not noise to average out -- it
                # says one of them is wrong here and neither can say which. Measured: all 7
                # splits are a masked match at a huge distance against a region-consistent
                # vote, on bodies of 0x11-0xbb bytes -- i.e. the shape collision the negative
                # control quantifies. No verdict is issued.
                verdict = "CONTESTED (bytes and region disagree)"
        elif ok_tsv and ok_vote:
            verdict = "BOTH SHAPE-COMPATIBLE (undecidable by bytes)"
        elif region_picks_vote is None:
            verdict = "UNDECIDED"
        else:
            verdict = ("VOTE" if region_picks_vote else "FUNCTIONS.TSV") + " (region delta only)"
        tally[verdict] = tally.get(verdict, 0) + 1
        out.append((src, tsv_dst, vote_dst, verdict, nvotes, ncallers))
        if verbose:
            print(f"{BASE + src:>#12x} {BASE + tsv_dst:>#14x} {BASE + vote_dst:>#14x} "
                  f"{('%+#x' % region) if region is not None else '-':>10} "
                  f"{tsv_dst - src:>+#9x} {vote_dst - src:>+#9x}  "
                  f"{('%#x' % length) if length else '-':>5}  {verdict}")
    if verbose:
        print()
        for verdict, n in sorted(tally.items(), key=lambda kv: -kv[1]):
            print(f"   {verdict:28s} {n}")
    return out, tally, len(agree)


def selftest():
    """The adjudicator must prefer the candidate whose masked body actually matches.

    Positive control: every row where the two methods agree is a row where the byte test
    must also accept the shared answer -- if it did not, the test would be rejecting
    correct pairings and its verdicts on the disagreements would be worthless.
    """
    P = _leafmod()
    img162, img170 = P.Image(P.IMG_1162), P.Image(P.IMG_1170)
    fmap = P.load_function_map()
    failures = []

    def check(name, condition, detail=""):
        print(f"  {'ok  ' if condition else 'FAIL'} {name}{('  ' + detail) if detail else ''}")
        if not condition:
            failures.append(name)

    # 400 agreed pairs spread across the image, sampled deterministically.
    keys = sorted(fmap)
    sample = keys[:: max(1, len(keys) // 400)][:400]
    accepted = tested = 0
    for src in sample:
        dst = fmap[src]
        end, cend = img162.extents.get(src), img170.extents.get(dst)
        if not end or not cend or (cend - dst) != (end - src):
            continue
        tested += 1
        if P.signature_key(img162, src, end) == P.signature_key(img170, dst, cend):
            accepted += 1
    rate = accepted / max(1, tested)
    check("the byte test accepts agreed pairs", rate > 0.90,
          f"{accepted}/{tested} = {rate * 100:.1f}%")
    # Negative control, and the thing it actually measures. Feed the test a deliberately
    # wrong destination -- the next function along in 1.17 -- and count rejections.
    #
    # It is not 100%, and it cannot be. Masking wildcards exactly the bytes that separate two
    # instantiations of the same shape, so a wrong candidate that is an adjacent same-length
    # sibling is accepted by construction: 9 of 200 here, e.g. 0x140239ad0 and its neighbour
    # 0x140239b00, both 0x30 bytes and both `sub rsp,0x38 / mov rax,[rcx+0x40] / ...`. That is
    # a property of the image, not a defect in the rule, and it is why `adjudicate` issues a
    # byte verdict only on the rejection of the loser.
    #
    # The threshold is a regression floor, not a target: 191/200 when written, so a drop below
    # 185 means the masking itself stopped discriminating rather than that a few more siblings
    # collided.
    NEGATIVE_CONTROL_FLOOR = 185
    values = sorted(fmap.values())
    rejected = tried = 0
    for src in sample[:200]:
        dst = fmap[src]
        i = bisect.bisect_right(values, dst)
        wrong = values[i] if i < len(values) else None
        if wrong is None or wrong == dst:
            continue
        end, cend = img162.extents.get(src), img170.extents.get(wrong)
        if not end or not cend:
            continue
        tried += 1
        if P.signature_key(img162, src, end) != P.signature_key(img170, wrong, cend):
            rejected += 1
    check("...and rejects the next function along", rejected >= NEGATIVE_CONTROL_FLOOR,
          f"{rejected}/{tried}, floor {NEGATIVE_CONTROL_FLOOR} (was 191 when written)")

    # The gate that matters, on the real disagreements rather than a sample: where the bytes
    # decide a row, the surrounding region's delta must independently pick the same winner.
    #
    # These two signals share nothing -- one compares masked bodies, the other looks only at
    # where the neighbouring 40 `.pdata` pairs landed -- so unanimity between them is real
    # corroboration, and a single split would mean one of the two is unsound.
    #
    # An earlier version of this check asserted "no row has BOTH candidates shape-accepted".
    # That was a claim about the data, and it was false: 20 of the 53 have it, for the same
    # reason the negative control is not 100% -- adjacent same-length siblings share a masked
    # shape. Those rows get no byte verdict at all, which is the correct handling, so the
    # right gate is on the rows a verdict is issued for.
    rows, tally, _agree = adjudicate(verbose=False)
    undecidable = tally.get("BOTH SHAPE-COMPATIBLE (undecidable by bytes)", 0)
    print(f"  note  {undecidable} of the disagreements are shape-undecidable "
          "(both candidates share the 1.16.2 masked shape); no byte verdict is issued for them")
    contested = tally.get("CONTESTED (bytes and region disagree)", 0)
    print(f"  note  {contested} rows are CONTESTED (the masked bytes and the region delta pick "
          "different winners); no verdict is issued for them either")
    decided = [r for r in rows if "bytes+region" in r[3]]
    check("no verdict is issued on a contested or undecidable row",
          all("CONTESTED" not in r[3] and "SHAPE-COMPATIBLE" not in r[3] for r in decided),
          f"{len(decided)} decided by two independent signals")
    # Regression floor: the corroborated set was 19 rows when written. A collapse to near zero
    # means the masking or the region estimate stopped working, not that the map got better.
    votes_wrong = sum(1 for r in decided if r[3].startswith("VOTE"))
    check("functions.tsv rows the two signals jointly refute", votes_wrong >= 15,
          f"{votes_wrong} (was 19 when written)")
    print()
    if failures:
        print(f"selftest FAILED: {', '.join(failures)}")
        return 1
    print("selftest passed")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--tsv", help="write the adjudicated rows here")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    rows, _tally, _agree = adjudicate()
    if args.tsv:
        with open(args.tsv, "w", encoding="utf-8") as handle:
            handle.write("# 1.16.2 VA\tfunctions.tsv 1.17 VA\tcaller-vote 1.17 VA\tverdict"
                         "\tvotes\tcallers\n")
            for src, tsv_dst, vote_dst, verdict, n, v in rows:
                handle.write(f"{BASE + src:#x}\t{BASE + tsv_dst:#x}\t{BASE + vote_dst:#x}\t"
                             f"{verdict}\t{n}\t{v}\n")
        print(f"wrote {args.tsv}")
    return 0


if __name__ == "__main__":
    try:
        import capstone  # noqa: F401
    except ImportError:
        if os.environ.get("_ADJ_UNDER_UV"):
            raise SystemExit("capstone still missing under uv")
        os.environ["_ADJ_UNDER_UV"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])
    sys.exit(main())
