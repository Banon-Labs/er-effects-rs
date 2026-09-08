#!/usr/bin/env python3
"""Second opinion on a 1.16.2 -> 1.17 pair, from Ghidra's analysis of both runtime dumps.

Why a second opinion is worth anything here. `map-rvas-1162-to-1170.py` finds where a masked
signature re-occurs and `verify-rva-map-1170.py` decodes both bodies and compares them. Both read
the same two flat images, with the same decoder, under the same normalisation -- so they share a
failure mode: if a wrong destination happens to decode into a stream shaped like the right one,
neither notices, and the second tool's agreement adds nothing the first did not already assume.

The two Ghidra dumps are genuinely independent of that. They were analysed by Ghidra's own
disassembler and function-boundary analysis, from the runtime images rather than the de-Arxan'd
files, months apart, and this asks them three questions the byte tools cannot ask at all:

    Entry   does each dump declare a function to start at its half of the pair? A destination
            0x10 bytes into a function reads as live and detours into the middle of one. A
            declared entry at the wrong address is the hard failure; No declared entry is not one,
            because that is the ordinary condition of a leaf.
    CALLEES do the two functions call the same functions, carried forward? Topology, which no byte
            signature over a 40-byte prologue can see. 1.17's dump carries no names -- everything
            is `FUN_<addr>` -- but its call graph is there, so each 1.16.2 callee can be matched to
            a 1.17 callee at a plausible forward delta. A wrong destination carries almost none of
            them; a right one carries nearly all. Where a dump has no call graph for a function at
            all, the question is vacuous rather than answered.
    Size    do the two dumps declare the same body length? Reported, never fatal. A body that grew
            is what `PATCH-SITE-IDENTICAL` exists to describe, and refusing it here would
            contradict the verifier's own vocabulary.

What the CALLEE tolerance is for, measured rather than chosen. `0x1409a4ed0 -> 0x1409a6070` --
`PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA`, verified `IDENTICAL-WHOLE` over its whole 261-byte body --
carries four of its five callees and loses one, because 1.17's dump does not split out a function
where 1.16.2's does. Requiring all five would refuse a pair the instruction comparison proves. So
a quarter of them, and never fewer than one, may fail to carry. The four that do carry are worth
seeing: two of them cross out of this region entirely, `0x875590 -> 0x876580` at +0xff0 and
`0x739e20 -> 0x73ac70` at +0xe50, each landing on the delta its own region is known to have.

None of that is a hook licence; `.pdata` entry evidence and the instruction comparison remain the
licence. This is the check that catches a plausible wrong address before it becomes an anchor and
spreads its delta over a whole region.

What it says about the curated ledger, run over all 111 rows of
`docs/recon/rva-map-1162-to-1170.verified.tsv` on 2026-09-01: 111/111, no contradictions -- 55
confirmed, 55 confirmed-thin (leaves, where one dump has no call graph to compare), 1
no-dump-opinion (a 3-byte leaf neither dump declares at all). The two rows that did come back
refused on the first pass were both correct, and both loosenings in `judge` are named after them.

Both dump VAs are shift-0: dump VA == deobf VA == runtime VA on 1.16.2 and on 1.17 alike, so the
addresses here need no translation in either direction.

Usage
    python3 scripts/confirm-1170-pair-in-dumps.py 0x1409a4670:0x1409a5810
    python3 scripts/confirm-1170-pair-in-dumps.py --tsv docs/recon/rva-map-1162-to-1170.verified.tsv
    python3 scripts/confirm-1170-pair-in-dumps.py --tsv <pairs.tsv> --quiet

Needs both MCP daemons: 1.16.2 on :8765 (`scripts/ghidra/mcp-up-1162.sh`) and 1.17 on :8767
(`scripts/ghidra/mcp-up-1170.sh`). Skips at exit 0 when either is down, because "could not look"
is not evidence of agreement. Exits 1 when a pair is contradicted.
"""

import argparse
import importlib.util
import os
import socket
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLIENT = os.path.join(ROOT, "scripts", "ghidra", "mcp_query.py")
BASE = 0x140000000
# Per-query ceiling. Well under the repo-wide 30s non-game cap (scripts/check-no-timeouts.py), and
# far above a warm daemon's answer -- `getFunctionByAddress` on either project returns in
# milliseconds once the program is loaded.
QUERY_TIMEOUT_SECONDS = 20
# How far a callee may move between the builds and still be recognised as the same callee. The
# same magnitude as the mapper's `REGION_RADIUS`, and for the same reason: the 1.16.2 -> 1.17 shift
# is locally constant, so a callee's own region moved it by its own region's delta. Every delta
# measured while pairing the three regions of er-effects-rs-4uw5.13 is far inside this -- +0xe50
# for the shared menu layer, +0xe80 around 0x814ed0, +0xff0 across 0x87xxxx, +0x11a0 across both
# 0x92xxxx and 0x9axxxx, +0x13a0 out in 0x141ebxxxx.
MAX_CALLEE_DRIFT = 0x8000
# How many callees must carry before a differing declared size is treated as a real 1.17 edit
# rather than as the wrong destination. See the clause in `judge` that uses it; three is the point
# at which the match stops being something two arbitrary entries could produce.
CORROBORATING_CALLEES = 3


def client():
    spec = importlib.util.spec_from_file_location("mcp_query", CLIENT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def port_open(port):
    try:
        with socket.create_connection(("localhost", port), timeout=2):
            return True
    except OSError:
        return False


def describe(query, va, port):
    """`(entry, size, callees)` for the function the dump declares at or around `va`."""
    reply = query("getFunctionByAddress", {"address": f"{va:x}"}, port=port,
                  timeout=QUERY_TIMEOUT_SECONDS)
    result = reply.get("result")
    if not isinstance(result, dict) or "entry" not in result:
        return None, None, ()
    entry = int(result["entry"], 16)
    callees = []
    for callee in result.get("callees") or ():
        # "Name@hexva" -- the name half is worthless on the 1.17 side, where every function is
        # `FUN_<addr>`, so only the address is read.
        _name, _, address = callee.rpartition("@")
        try:
            callees.append(int(address, 16))
        except ValueError:
            continue
    return entry, result.get("size"), tuple(sorted(callees))


def carried(old_callees, new_callees):
    """1.16.2 callees matched one-to-one onto a 1.17 callee within `MAX_CALLEE_DRIFT`.

    Greedy nearest, in address order, each 1.17 callee claimed at most once. Not an optimal
    assignment, and it does not need to be: the question is whether the destination calls
    substantially the same set of functions, and the tolerance below absorbs the odd steal.
    """
    remaining = list(new_callees)
    matched = []
    for callee in old_callees:
        if not remaining:
            break
        drift, nearest = min((abs(other - callee), other) for other in remaining)
        if drift <= MAX_CALLEE_DRIFT:
            remaining.remove(nearest)
            matched.append((callee, nearest))
    return matched


def pairs_from_tsv(path):
    rows = []
    for line in open(path, encoding="utf-8"):
        if line.startswith("#") or not line.strip():
            continue
        fields = line.rstrip("\n").split("\t")
        if len(fields) < 2 or fields[1] == "-":
            continue
        try:
            old_va, new_va = int(fields[0], 16), int(fields[1], 16)
        except ValueError:
            continue
        rows.append((old_va + BASE if old_va < BASE else old_va,
                     new_va + BASE if new_va < BASE else new_va))
    return rows


# The controls, and the gate cannot be trusted without them. Each accept is one of the eight
# anchors added for er-effects-rs-4uw5.13; each refuse is one of those same pairs with a
# destination that is wrong in a specific way, so a refusal names a clause rather than a mood.
#
# The swapped cases are here because they caught something. Before `CORROBORATING_CALLEES` existed,
# three of the four swaps came back CONFIRMED: the callee tolerance forgives one loss, so a
# function with one callee or none passed against any address that merely declared an entry, and
# only the differing size objected -- silently, since a differing size was not fatal. A gate that
# accepts a destination belonging to another function is worse than no gate.
SELFTEST_CASES = (
    ("accept", 0x140873850, 0x140874840, "0x87xxxx low anchor"),
    ("accept", 0x140874480, 0x140875470, "0x87xxxx anchor nearest the ProfileSelect list builder"),
    ("accept", 0x1408776e0, 0x1408786d0, "0x87xxxx mid anchor, 5 callees carry"),
    ("accept", 0x14087eba0, 0x14087fb90, "0x87xxxx high anchor"),
    ("accept", 0x1409202d0, 0x140921470, "0x92xxxx anchor nearest AddCancelButton"),
    ("accept", 0x140926e50, 0x140927ff0, "0x92xxxx mid anchor"),
    ("accept", 0x14092abe0, 0x14092bd80, "0x92xxxx upper-mid anchor, one callee legitimately lost"),
    ("accept", 0x14092df10, 0x14092f0b0, "0x92xxxx high anchor, 10 callees carry"),
    ("refuse", 0x140874480, 0x140875480, "destination 0x10 past the entry"),
    ("refuse", 0x14092df10, 0x14092f0c0, "destination 0x10 past the entry"),
    ("refuse", 0x140874480, 0x1408798b0, "destination is another anchor's entry"),
    ("refuse", 0x1408776e0, 0x140875470, "destination is another anchor's entry"),
    ("refuse", 0x1409202d0, 0x14092f0b0, "destination is another anchor's entry, 10x the size"),
    ("refuse", 0x14092df10, 0x140921470, "destination is another anchor's entry, none of 10 "
                                         "callees carry"),
)


def selftest(args):
    """Assert the gate accepts the eight anchors and refuses six destinations that are wrong."""
    query = client().query
    failures = []
    for want, old_va, new_va, clause in SELFTEST_CASES:
        verdict, _line = judge(query, old_va, new_va, args)
        got = "refuse" if verdict.startswith("CONTRADICTED") else "accept"
        state = "ok  " if got == want else "FAIL"
        print(f"  {state} {want:<7} {old_va:#x} -> {new_va:#x}  {verdict:<17} {clause}")
        if got != want:
            failures.append((old_va, new_va))
    if failures:
        print(f"selftest FAILED for {len(failures)} case(s)")
        return 1
    accepts = sum(1 for case in SELFTEST_CASES if case[0] == "accept")
    print(
        f"selftest passed: {accepts} anchors confirmed by both dumps, "
        f"{len(SELFTEST_CASES) - accepts} wrong destinations refused"
    )
    return 0


def judge(query, old_va, new_va, args):
    """`(verdict, printable line)` for one pair. The whole of the decision lives here."""
    old_entry, old_size, old_callees = describe(query, old_va, args.port_1162)
    new_entry, new_size, new_callees = describe(query, new_va, args.port_1170)
    notes = []
    # A declared entry at the wrong address is the refusal. "No function here at all" is not, and
    # the difference is what the ledger taught this tool on its first full pass over it: two of its
    # 111 rows came back contradicted and both were correct. `0x1407add70 -> 0x1407aebf0` is a
    # 3-byte identical-leaf-NOPATCH, and a leaf has no `.pdata` record, which is exactly why
    # neither dump declares a function at it. Refusing a row for being a leaf would refuse the
    # class of row the verifier invented identical-leaf to admit.
    if old_entry is not None and old_entry != old_va:
        notes.append(f"1.16.2 entry is {old_entry:#x}, not the pair's")
    if new_entry is not None and new_entry != new_va:
        notes.append(f"1.17 entry is {new_entry:#x}, not the pair's")
    matched = carried(old_callees, new_callees)
    # And the call graph can be the dump'S gap rather than the pair'S problem. The ledger's other
    # false refusal was `0x140249a50 -> 0x140249a50` -- byte-identical over its whole 0x3d leaf
    # extent, tail-jumping the same callee in both images -- where 1.16.2's dump lists three
    # callees and 1.17's lists none. That is Ghidra having no references for a function it barely
    # analysed, not a function that stopped calling anything, and there is nothing to compare when
    # one side is empty. Such a pair is reported confirmed-thin, never confirmed outright.
    topology_available = bool(old_callees) and bool(new_callees)
    # A quarter of them may fail to carry, and never fewer than one -- see the tolerance note in
    # the module docstring, which is a measured allowance rather than a chosen number.
    allowed = max(1, len(old_callees) // 4)
    if topology_available and len(old_callees) - len(matched) > allowed:
        lost = [c for c in old_callees if c not in [m for m, _ in matched]]
        notes.append(
            f"only {len(matched)}/{len(old_callees)} callees carry; "
            + ", ".join(f"{c:#x}" for c in lost[:4])
            + (" ..." if len(lost) > 4 else "")
        )
    resized = old_size is not None and new_size is not None and old_size != new_size
    # A different size needs the topology to speak for it, and this clause is here because the
    # negative control caught its absence -- see SELFTEST_CASES.
    if resized and len(matched) < CORROBORATING_CALLEES:
        notes.append(
            f"size {old_size} vs {new_size} with only {len(matched)} callee(s) to corroborate"
        )
    if notes:
        verdict = "CONTRADICTED"
    elif old_entry is None and new_entry is None:
        # Neither dump declares a function at either half, which is the ordinary condition of a
        # leaf. There is no opinion here to agree or disagree with, and saying confirmed would
        # manufacture one.
        verdict = "NO-DUMP-OPINION"
    elif resized:
        # A body that grew is what patch-site-identical exists to describe, so it gets its own
        # word rather than a refusal.
        verdict = "CONFIRMED-RESIZED"
    elif not topology_available:
        # Entry and size agree and there is no call graph to ask on at least one side. Said out
        # loud rather than printed as a plain confirmed, because the strongest of the three
        # questions was vacuous here and the reader cannot tell that from a carried ratio.
        verdict = "CONFIRMED-THIN"
    else:
        verdict = "CONFIRMED"
    line = (
        f"{old_va:#x} -> {new_va:#x}  {verdict:<17} "
        f"size {old_size}/{new_size}, callees {len(matched)}/{len(old_callees)} carried"
        + (f"   {'; '.join(notes)}" if notes else "")
    )
    return verdict, line


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("pairs", nargs="*", metavar="OLD:NEW", help="hex VAs, e.g. 0x140..:0x140..")
    parser.add_argument("--tsv", metavar="PATH", help="read pairs from a map or verdict table")
    parser.add_argument("--port-1162", type=int, default=8765)
    parser.add_argument("--port-1170", type=int, default=8767)
    parser.add_argument("--quiet", action="store_true", help="print only contradicted pairs")
    parser.add_argument(
        "--selftest",
        action="store_true",
        help="assert the eight anchors are confirmed and six wrong destinations are refused",
    )
    args = parser.parse_args()

    if args.selftest:
        for port, name in ((args.port_1162, "1.16.2"), (args.port_1170, "1.17")):
            if not port_open(port):
                print(f"skipped: no {name} Ghidra MCP daemon on :{port}")
                return 0
        return selftest(args)

    wanted = [tuple(int(half, 16) for half in text.split(":")) for text in args.pairs]
    if args.tsv:
        wanted += [p for p in pairs_from_tsv(args.tsv) if p not in wanted]
    if not wanted:
        sys.exit("no pairs: pass OLD:NEW or --tsv")
    for port, name in ((args.port_1162, "1.16.2"), (args.port_1170, "1.17")):
        if not port_open(port):
            print(f"skipped: no {name} Ghidra MCP daemon on :{port}")
            return 0

    query = client().query
    bad = 0
    for old_va, new_va in wanted:
        verdict, line = judge(query, old_va, new_va, args)
        bad += verdict.startswith("CONTRADICTED")
        if verdict.startswith("CONTRADICTED") or not args.quiet:
            print(line)
    print(f"\n{len(wanted) - bad}/{len(wanted)} confirmed by both dumps")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
