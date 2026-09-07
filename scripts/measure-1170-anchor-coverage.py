#!/usr/bin/env python3
"""How much of an address range can `map-rvas-1162-to-1170.py` resolve, and what do anchors buy?

Why a measurement and not an impression. "The map is sparse around 0x9axxxx" is the kind of claim
that gets repeated for months without anybody knowing whether it is still true. This runs the
mapper over every `.pdata`-declared function entry in a range and counts, so the sparseness has a
number and a fix has a before and an after.

The one-address-at-A-time rule, which is the whole point of the harness. `resolve_all` lets the
addresses in a work list anchor each OTHER: map fifty neighbours together and the ones that match
uniquely settle the ones that do not, so a batch run flatters itself and tells you nothing about
the situation an agent is actually in -- holding one address that a feature needs. So every entry
here is mapped alone. Pass 1 is computed once per address (`map_all`) and pass 2 is then run under
each anchor policy over that same cached candidate list, which is both fast and the only way to be
sure the two policies were judged on identical evidence.

    resolved-alone   the address maps with no help: its signature matched exactly once
    resolved-anchor  its signature matched several places and a nearby verified pair's delta
                     picked one of them
    unresolved       neither -- the honest answer, and a Ghidra lookup

Usage
    uv run --with capstone python3 scripts/measure-1170-anchor-coverage.py
    ... --region 0x870000:0x880000 --region 0x9a0000:0x9b0000
    ... --anchors none              # what the tool could do before any ledger was consulted
    ... --anchors docs/recon/rva-map-1162-to-1170.verified.tsv --anchors <other.tsv>
    ... --list-candidates           # the resolved-alone entries: what a new anchor can be cut from
    ... --list-unresolved

Ranges are 1.16.2 RVAs, `LO:HI`, half-open. The default is the three regions
`er-effects-rs-4uw5.13` names. Skips at exit 0 without the two gitignored de-Arxan'd images.
"""

import argparse
import importlib.util
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAPPER = os.path.join(ROOT, "scripts", "map-rvas-1162-to-1170.py")
VERIFIER = os.path.join(ROOT, "scripts", "verify-rva-map-1170.py")
# The regions er-effects-rs-4uw5.13 names, as the addresses that motivated it: the ProfileSelect
# list builder (0x875590), the System->Quit AddCancelButton clone (0x920c90), and the profile-load
# activate and list-rebuild pair (0x9a4670, 0x9a4ed0).
DEFAULT_REGIONS = ((0x870000, 0x880000), (0x920000, 0x930000), (0x9A0000, 0x9B0000))


def load(name, path):
    """A `scripts/` module whose filename is not a Python identifier."""
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def region_arg(text):
    lo, _, hi = text.partition(":")
    if not hi:
        raise argparse.ArgumentTypeError(f"expected LO:HI, got {text!r}")
    return int(lo, 0), int(hi, 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--region", type=region_arg, action="append", metavar="LO:HI")
    parser.add_argument(
        "--anchors",
        action="append",
        metavar="TSV",
        help="verdict ledger supplying pass-2 deltas; repeatable; `none` for no ledger at all",
    )
    parser.add_argument("--list-candidates", action="store_true")
    parser.add_argument("--list-unresolved", action="store_true")
    parser.add_argument(
        "--suggest",
        type=int,
        default=0,
        metavar="N",
        help="propose up to N new anchors per region, greedily: at each step the resolved-alone "
        "entry that would resolve the most still-unresolved ones. A proposal is a place to point "
        "the verifier, never a row -- nothing here has compared a single instruction",
    )
    args = parser.parse_args()

    # RE-EXEC under uv if capstone is absent, the bootstrap `check-leaf-extent-pdata-coverage.py`
    # and `verify-thunk-rva-1170.py` already carry. There is no system pip here, so the mapper's
    # decoder import would otherwise die with a bare ImportError at exit 1 -- indistinguishable
    # from a real finding.
    try:
        import capstone  # noqa: F401
    except ImportError:
        try:
            os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])
        except OSError:
            print("skipped: capstone unavailable and `uv` is not on PATH")
            return 0

    mapper = load("map_rvas_1162_to_1170", MAPPER)
    verifier = load("verify_rva_map_1170", VERIFIER)
    for image in (mapper.SRC_IMAGE, mapper.DST_IMAGE):
        if not os.path.exists(image):
            print(f"skipped: missing image {image}")
            return 0
    source = open(mapper.SRC_IMAGE, "rb").read()
    target = open(mapper.DST_IMAGE, "rb").read()
    starts = verifier.function_starts(source)

    paths = args.anchors or [mapper.VERIFIED_LEDGER]
    anchors = () if paths == ["none"] else mapper.load_anchors(paths)
    print(f"anchor pool: {len(anchors)} verified pairs from {', '.join(paths)}")

    # The mapper reads `args.want_pinned` to decide whether to walk the signature ladder. Nothing
    # here pins a length, so it walks -- the same behaviour a bare command-line run gets.
    class Args:
        want = mapper.DEFAULT_SIGNATURE_BYTES
        want_pinned = False

    totals = [0, 0, 0]
    for lo, hi in args.region or DEFAULT_REGIONS:
        entries = [mapper.BASE + rva for rva in sorted(starts) if lo <= rva < hi]
        results, pending = mapper.map_all(source, target, entries, Args())
        alone = {va for va, (mapped, _note) in results.items() if mapped is not None}

        def covered(pool):
            """Entries this pool resolves, one address at a time -- see the note below it."""
            done = set(alone)
            for candidate in entries:
                if candidate in done or candidate not in pending:
                    continue
                one = mapper.settle({}, {candidate: list(pending[candidate])}, pool)
                if one[candidate][0] is not None:
                    done.add(candidate)
            return done

        # One address per `settle` call, and this loop is the whole harness. `settle` anchors on
        # every uniquely-mapped address in the dict it is handed, so passing a whole region at once
        # lets that region's unique entries anchor the rest of it -- which measures a batch run,
        # not the question asked, and flatters the tool by exactly the amount the caller's typing
        # happened to help. Handing `settle` one address leaves the ledger as the only anchor
        # source there is, which is what somebody holding a single address a feature needs
        # actually has. The difference is not cosmetic. Measured over the three default regions
        # with no ledger: batched, 855 of 1,121 entries resolve; one at a time, 128 do, and every
        # one of those 128 matched uniquely and needed no anchor at all.
        settled = {}
        for va in entries:
            one_result = {va: results[va]} if va in results else {}
            one_pending = {va: list(pending[va])} if va in pending else {}
            settled.update(mapper.settle(one_result, one_pending, anchors))
        by_anchor = {
            va
            for va, (mapped, note) in settled.items()
            if mapped is not None and va not in alone and note.startswith("nearest-anchor")
        }
        unresolved = [va for va in entries if va not in alone and va not in by_anchor]
        totals = [
            totals[0] + len(alone),
            totals[1] + len(by_anchor),
            totals[2] + len(unresolved),
        ]
        print(
            f"{lo:#08x}..{hi:#08x}  {len(entries):>4} .pdata entries: "
            f"{len(alone):>4} resolved-alone, {len(by_anchor):>4} resolved-anchor, "
            f"{len(unresolved):>4} unresolved "
            f"({(len(alone) + len(by_anchor)) / max(1, len(entries)):.1%} covered)"
        )
        if args.list_candidates:
            for va in sorted(alone):
                print(f"    anchor candidate {va:#x} -> {results[va][0]:#x}  ({results[va][1]})")
        if args.list_unresolved:
            for va in sorted(unresolved):
                print(f"    unresolved {va:#x}  ({settled[va][1]})")
        if args.suggest:
            # Greedy, and greedy is the right shape here for a reason worth stating: an anchor's
            # value is not its own correctness but how many other addresses its delta settles, and
            # that is a set-cover payoff -- the second anchor 0x2000 from the first is worth almost
            # nothing, while one in an untouched span is worth a hundred entries. Ranking
            # candidates independently would pick a cluster.
            pool = list(anchors)
            reached = covered(pool)
            for _ in range(args.suggest):
                best = None
                for va in sorted(alone):
                    if any(va == old for old, _new in pool):
                        continue
                    got = covered(pool + [(va, results[va][0])])
                    if best is None or len(got) > len(best[1]):
                        best = ((va, results[va][0]), got)
                if best is None or len(best[1]) <= len(reached):
                    break
                gain = len(best[1]) - len(reached)
                pool.append(best[0])
                reached = best[1]
                print(
                    f"    suggest anchor {best[0][0]:#x} -> {best[0][1]:#x}  "
                    f"+{gain} resolved, region would reach "
                    f"{len(reached)}/{len(entries)} ({len(reached) / len(entries):.1%})"
                )
    print(
        f"TOTAL  {sum(totals)} entries: {totals[0]} resolved-alone, {totals[1]} resolved-anchor, "
        f"{totals[2]} unresolved ({(totals[0] + totals[1]) / max(1, sum(totals)):.1%} covered)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
