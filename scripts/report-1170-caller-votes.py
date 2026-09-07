#!/usr/bin/env python3
"""Print the full vote tally behind a `map-data-rvas-1162-to-1170.py` carry verdict.

`CONTESTED 2 answers from 651 callers` is not a usable verdict on its own: it does not say
whether the split is 650-to-1 -- one caller whose body was edited, so the answer is obvious --
or 320-to-331, where the address genuinely forked and no answer is safe. This prints the tally
so a reader can tell those apart, and the delta each candidate implies, because a runner-up
that is not at the region's delta is the signature of a decode that slipped inside one edited
caller rather than of a second real function.

Three kinds of vote, and the two this tool was blind to until 2026-08-30
------------------------------------------------------------------------
It counted `call`/`jmp` sites and nothing else, so it could only ever answer for a function
something branches to. Every function whose address is merely taken -- stored into a `std::function`
/ functor, parked in a dispatch table, compared against a field -- was invisible, and the tool
said `None ... 0 real` about it in the same words it uses for an address nothing references at
all. That is not a rare shape in ELDEN RING's menu code; it is how most of it is written.

`MENU_ITEM_ACCEPT_IDLE_RVA` (1.16.2 `0x7add70`) is the measured case. Nothing calls it: it is a
3-byte `xor eax,eax; ret` whose address a `CS::MenuItem` row carries at `+0xf8` as its
constant-false accept predicate. The whole 1.16.2 image contains exactly one reference to it, a
`lea` -- so the old tool reported `None (1 candidate branch site, 0 real)` and no instrument in
this tree could map the address. The same `lea` is the whole answer: it sits at byte `+0xa5` of
`0x7acf80`, whose 1.17 pair is known, and reading the paired instruction there gives `0x7aebf0`.

So three carriers run, and they stay separate in the output. They are different evidence and a
reader must be able to weigh them apart:

  * `call/jmp`      -- `carry_code`: a branch at instruction N of a mapped caller.
  * `address-taken` -- `carry`: a rip-relative displacement (a `lea`, or any `mov`/`cmp` reaching
    the address) at the same byte offset of a mapped referrer. This is the carrier the data map
    already used for globals; a function pointer is carried by exactly the same arithmetic.
  * `pointer-table` -- an 8-byte absolute `BASE + rva` stored in `.rdata`/`.data`: a vtable slot
    or a function-pointer table. Carried by its neighbours in the same table, each of which is a
    code pointer the function map answers for, so the table is located in 1.17 by content rather
    than by address.

And the strength that `WEAK` hides. `carry`'s vocabulary calls a single reference weak, which is
right when an address has fifty references and one of them survived the decode -- it means the
other forty-nine were lost. It is the wrong word when the address has exactly one reference in
each image: `1-of-1` in both directions is unanimity, not a fragment. The reverse check below
distinguishes them by counting references to the answer in 1.17, and reports `UNANIMOUS 1-of-1`
only when both counts are 1.

Usage
    uv run --with capstone --with numpy python3 scripts/report-1170-caller-votes.py 0x739e20
    uv run --with capstone --with numpy python3 scripts/report-1170-caller-votes.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAPPER = os.path.join(ROOT, "scripts", "map-data-rvas-1162-to-1170.py")
FUNCTION_MAP = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.functions.tsv")
BASE = 0x140000000

# How far either side of a pointer slot to look for a neighbour the function map answers for.
# A vtable's slots are its own methods and a functor table's are its own callbacks, so a mappable
# neighbour is normally adjacent; eight slots is generous and still bounded.
POINTER_ANCHOR_SLOTS = 8
# Anchors used per slot. More than this buys nothing: two agreeing anchors already locate the
# table, and each costs a full-image scan.
POINTER_ANCHORS = 3
# Slots examined per address. A common callback can appear in hundreds of tables and they all say
# the same thing; the cap keeps one address from costing a thousand scans.
POINTER_SLOT_CAP = 32


def load_mapper():
    """`map-data-rvas-1162-to-1170.py`, imported by path because its name has hyphens in it."""
    spec = importlib.util.spec_from_file_location("mapdata", MAPPER)
    mapper = importlib.util.module_from_spec(spec)
    sys.modules["mapdata"] = mapper
    spec.loader.exec_module(mapper)
    from capstone.x86 import X86_OP_IMM

    mapper.CS_OP_IMM_TYPE = (X86_OP_IMM,)
    return mapper


def pointer_slots(image, target, cap=POINTER_SLOT_CAP):
    """8-aligned byte offsets in the image whose QWORD is `BASE + target`.

    Only the 8-aligned ones. An unaligned QWORD that happens to spell a code address is a
    coincidence of two adjacent fields, not a pointer slot, and admitting it would put a
    neighbour-anchored table search on a table that does not exist.
    """
    import numpy as np

    words = len(image.data) // 8
    array = np.frombuffer(image.data[: words * 8], dtype="<u8")
    hits = np.nonzero(array == np.uint64(BASE + target))[0]
    return [int(index) * 8 for index in hits[:cap]]


def _qword(image, offset):
    if offset < 0 or offset + 8 > len(image.data):
        return None
    return struct.unpack_from("<Q", image.data, offset)[0]


def _slots_holding(image, value):
    import numpy as np

    words = len(image.data) // 8
    array = np.frombuffer(image.data[: words * 8], dtype="<u8")
    return [int(index) * 8 for index in np.nonzero(array == np.uint64(value))[0]]


def pointer_votes(old, new, fmap, target):
    """Carry an address by the table its pointer sits in. `(votes, slots_used, note)`.

    A pointer slot has no instruction to decode and no displacement to re-read, so the anchors
    are its NEIGHBOURS: another slot in the same table holding a code pointer the function map
    answers for. Locating that neighbour's 1.17 value locates the table, and the slot at the same
    delta from it is this address's 1.17 value.

    Two independent guards, because a single anchor can land on the wrong copy of a table:

      * every anchor a slot has must agree on the same answer, or the slot abstains entirely;
      * an anchor whose 1.17 value occurs in several places contributes each of them, so the
        agreement above has to survive the ambiguity rather than pick a winner from it.
    """
    votes: dict[int, int] = {}
    used = 0
    slots = pointer_slots(old, target)
    for slot in slots:
        anchors = []
        for step in range(1, POINTER_ANCHOR_SLOTS + 1):
            for delta in (-8 * step, 8 * step):
                if len(anchors) >= POINTER_ANCHORS:
                    break
                value = _qword(old, slot + delta)
                if value is None or value < BASE:
                    continue
                rva = value - BASE
                if rva == target or rva not in fmap:
                    continue
                anchors.append((delta, BASE + fmap[rva]))
        if not anchors:
            continue
        agreed = None
        for delta, want in anchors:
            here = set()
            for at in _slots_holding(new, want):
                value = _qword(new, at - delta)
                if value is None or value < BASE or value - BASE >= len(new.data):
                    continue
                here.add(value - BASE)
            if not here:
                agreed = None
                break
            agreed = here if agreed is None else (agreed & here)
            if not agreed:
                break
        if agreed and len(agreed) == 1:
            used += 1
            answer = next(iter(agreed))
            votes[answer] = votes.get(answer, 0) + 1
    if not slots:
        return votes, 0, "no 8-byte pointer to this address"
    if not votes:
        return votes, 0, f"{len(slots)} pointer slot(s), none with a mappable neighbour"
    return votes, used, f"{used} of {len(slots)} pointer slot(s) carried by a neighbour"


def real_reference_count(md, mapper, image, target):
    """How many references of any kind this image really holds to `target`.

    Candidates decoded and discarded, not counted raw: `references` scans four displacement tails
    over the whole of `.text`, so its raw count includes bytes that merely look like the right
    displacement. The decoded count is what makes `1-of-1` mean anything.
    """
    starts = image.function_starts()
    total = 0
    for at in mapper.references(image, target):
        func = mapper.enclosing(starts, at)
        if func is not None and mapper.instruction_index(md, image, func, at, target) is not None:
            total += 1
    for at in mapper.call_references(image, target):
        func = mapper.enclosing(starts, at)
        if func is not None and mapper.call_index(md, image, func, at, target) is not None:
            total += 1
    total += len(pointer_slots(image, target))
    return total


def tally(md, mapper, old, new, fmap, target):
    """Every carrier's answer for one address. `(answer, headline, kinds)`.

    `kinds` is `{kind: (moved, note, votes, )}` so the caller can print the three tallies apart.
    The merged answer sums votes across kinds -- they are independent evidence about the same
    question -- but a disagreement between kinds is reported as contested exactly as a
    disagreement within one is, because it is the same failure.
    """
    kinds: dict[str, tuple] = {}
    code_moved, code_note, code_votes = mapper.carry_code(md, old, new, fmap, target)
    kinds["call/jmp"] = (code_moved, code_note, code_votes)
    data_moved, data_note, data_votes = mapper.carry(md, old, new, fmap, target)
    kinds["address-taken"] = (data_moved, data_note, data_votes)
    ptr_votes, _used, ptr_note = pointer_votes(old, new, fmap, target)
    kinds["pointer-table"] = (
        max(ptr_votes, key=lambda k: ptr_votes[k]) if ptr_votes else None,
        ptr_note,
        ptr_votes,
    )

    merged: dict[int, int] = {}
    for _moved, _note, votes in kinds.values():
        for candidate, count in votes.items():
            merged[candidate] = merged.get(candidate, 0) + count
    if not merged:
        return None, "no usable reference of any kind", kinds
    answer = max(merged, key=lambda k: merged[k])
    if len(merged) > 1:
        return answer, f"CONTESTED {len(merged)} answers from {sum(merged.values())} references", kinds
    if merged[answer] >= 2:
        return answer, f"agreed by {merged[answer]} references", kinds
    # One reference. Whether that is thin or unanimous depends on how many there are, which is a
    # question about the images and not about the vote.
    old_total = real_reference_count(md, mapper, old, target)
    new_total = real_reference_count(md, mapper, new, answer)
    if old_total == 1 and new_total == 1:
        return answer, "UNANIMOUS 1-of-1 (the only reference in each image)", kinds
    return answer, f"WEAK (1 vote; {old_total} reference(s) in 1.16.2, {new_total} in 1.17)", kinds


def load(mapper):
    from pathlib import Path

    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    old = mapper.Image(Path(ROOT) / "eldenring-deobf.bin")
    new = mapper.Image(Path(ROOT) / "eldenring-deobf-1.17.bin")
    fmap = {}
    for line in Path(FUNCTION_MAP).read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        a, b = line.split()[:2]
        fmap[int(a, 16)] = int(b, 16)
    return md, old, new, fmap


# Addresses whose answer is settled, one per carrier, so a carrier that stops working is caught by
# a red selftest rather than by a `None` nobody reads as a regression.
#
# The `None` is the control on the controls: a tool that answers everything answers it too, and
# three carriers voting is three times as many chances to invent an address. `0xc57666` is a
# chained continuation chunk 0x86 bytes into `0xc575e0` -- nothing in either image branches to it,
# takes its address, or stores a pointer to it, because callers reach the function and not its
# cold half. `None` is the true answer and this tool must keep giving it.
SELFTEST_CASES = (
    (0xCF9300, 0xCFA9D0, "call/jmp", "16 callers branch here; the carrier this tool started with"),
    (
        0x7ADD70, 0x7AEBF0, "address-taken",
        "MENU_ITEM_ACCEPT_IDLE: nothing calls it, one `lea` takes its address, and that `lea` is "
        "at byte +0xa5 of 0x7acf80 in both images",
    ),
    (
        0xC575E0, 0xC58CB0, "pointer-table",
        "CSFreeListMemorySystem's shutdown path: no branch and no `lea` reaches it, only a vtable "
        "slot. Its answer is corroborated independently -- check-no-chained-continuation-rows.py "
        "names the same pair from the chained-unwind record that points back at it",
    ),
    (0xC57666, None, None, "the continuation chunk 0x86 bytes inside the function above"),
)


def selftest():
    """Prove each carrier decides, rather than that the merged answer happens to be right.

    Returns a process exit code.

    The failure this is shaped against is a tool that reports a confident answer no clause of it
    actually produced. So each case names the carrier that must supply its votes, and that
    carrier is asserted to be the one holding them -- an `address-taken` case whose votes arrived
    from `call/jmp` is a pass by coincidence and is failed here. Then each carrier is disabled in
    turn and its own case must go `None`, which is the only way to tell a carrier that works from
    a carrier whose answer was already there.
    """
    mapper = load_mapper()
    md, old, new, fmap = load(mapper)
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    for target, want, carrier, why in SELFTEST_CASES:
        answer, headline, kinds = tally(md, mapper, old, new, fmap, target)
        check(f"0x{target:x} answer ({why})", answer, want)
        if carrier is None:
            check(f"0x{target:x} has no votes at all", sum(len(v) for _m, _n, v in kinds.values()), 0)
            continue
        # The named carrier holds the votes, and holds them for the answer that was reported.
        check(f"0x{target:x} is carried by {carrier}", kinds[carrier][2].get(want, 0) > 0, True)
        check(f"0x{target:x} headline is not a refusal", headline.startswith("no usable"), False)

    # ------------------------------------------------------------------ Mutation
    # Break one carrier, watch its own case fall, put it back, watch it stand again. A carrier
    # that cannot be made to fail is not the one producing the answer.
    real_carry_code, real_carry = mapper.carry_code, mapper.carry
    real_pointer = globals()["pointer_votes"]
    empty = lambda *a, **k: (None, "MUTATED", {})  # noqa: E731

    mutations = 0
    for carrier, target, break_it, restore in (
        (
            "call/jmp", 0xCF9300,
            lambda: setattr(mapper, "carry_code", empty),
            lambda: setattr(mapper, "carry_code", real_carry_code),
        ),
        (
            "address-taken", 0x7ADD70,
            lambda: setattr(mapper, "carry", empty),
            lambda: setattr(mapper, "carry", real_carry),
        ),
        (
            "pointer-table", 0xC575E0,
            lambda: globals().__setitem__("pointer_votes", lambda *a, **k: ({}, 0, "MUTATED")),
            lambda: globals().__setitem__("pointer_votes", real_pointer),
        ),
    ):
        break_it()
        try:
            answer, _headline, _kinds = tally(md, mapper, old, new, fmap, target)
        finally:
            restore()
        check(f"MUTATION {carrier}: 0x{target:x} must lose its answer while the carrier is broken",
              answer, None)
        answer, _headline, _kinds = tally(md, mapper, old, new, fmap, target)
        check(f"MUTATION {carrier}: ...and regain it once restored", answer is not None, True)
        mutations += 1

    check("every carrier was exercised", mutations, 3)
    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(SELFTEST_CASES)} case(s), {mutations} carrier(s) mutated, "
          f"{len(failures)} failure(s)")
    return 1 if failures else 0


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("vas", nargs="*", help="1.16.2 addresses (VA or RVA) to tally")
    parser.add_argument("--selftest", action="store_true", help="assert the settled answers")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    if not args.vas:
        parser.error("give at least one address, or --selftest")

    mapper = load_mapper()
    md, old, new, fmap = load(mapper)
    answered = 0
    for text in args.vas:
        value = int(text, 16)
        rva = value - BASE if value >= BASE else value
        answer, headline, kinds = tally(md, mapper, old, new, fmap, rva)
        answered += 1 if answer is not None else 0
        print(f"0x{rva:x}  ->  {answer and hex(answer)}   {headline}")
        for kind, (_moved, note, votes) in kinds.items():
            print(f"    {kind:<14} {note}")
            for candidate, count in sorted(votes.items(), key=lambda kv: -kv[1]):
                print(f"      {count:5d} votes  0x{candidate:x}  delta {candidate - rva:+#x}")
    if len(args.vas) > 1:
        print(f"\n{answered} of {len(args.vas)} address(es) answered")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
