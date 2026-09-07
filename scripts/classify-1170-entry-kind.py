#!/usr/bin/env python3
"""Say whether an address is a function entry, a leaf with no unwind data, or mid-function.

Why this exists, and why the verdict table cannot answer it
-----------------------------------------------------------
`verify-rva-map-1170.py`'s last column has three states, and one of them is doing two
incompatible jobs. `BOTH-ENTRIES` means both images' `.pdata` declare a function start at the
address. `NEITHER-ENTRY` means neither does -- and that covers two populations that could not be
more different:

  * a leaf function. The x64 ABI lets a function omit unwind data when it allocates no stack and
    calls nothing, so ELDEN RING's many small getters and `jmp` thunks have no `.pdata` row at
    all. Hooking one is fine.
  * an address in the middle of a function. A captured return address, a byte-patch site, a
    constant that was derived by subtracting a dump shift that did not apply. Hooking one means
    MinHook overwrites five bytes mid-body.

`er-game-base/build.rs` accepts `NEITHER-ENTRY` for detours on purpose -- refusing it would throw
away every legitimate leaf -- so the second population is licensed along with the first, and
nothing downstream re-checks. Measured 2026-08-30, during the wave-2 merge: Six mid-function
addresses reached or nearly reached the verified table carrying `IDENTICAL` over 20-94
instructions and `NEITHER-ENTRY`. Two were already merged -- `0x958b37` (+0x227 inside
`0x958910..0x958c4f`) and `0xaec480` (below). Four more (`0x7642b0`, `0x76432c`, `0x7acbf0`, `0xc57670`) were in the next batch and were refused by hand. Every
one clears `MIN_VERIFIED_INSNS`, because being mid-function does not make the surrounding code
differ -- it makes the comparison agree beautifully about the wrong thing.

Read that last sentence again before trusting a clean verdict. A mid-function address produces a
*better-looking* verdict than a real entry does: it sits in the middle of a stable neighbourhood,
so the normalised comparison runs long and agrees everywhere. `0x140aec480` verified `identical
1.000` over 56 instructions and was merged. It is +0x360 inside `0x140aec120..0x140aec567`, the
repo had already recorded that (`crates/er-title-flow/src/title_load_step_hooks.rs` names the real
entry `0x140aec570`), and `crates/er-reload-trace/src/lib.rs` carried a raw `rva: 0xaec480`
HookSpec that would have consumed the licence -- removed the same day by a different agent, which
is not a mechanism anyone should rely on twice.

The inversion in one line: that impostor row is `IDENTICAL` over 56 instructions and would carry a
detour, while the correct pair `0xaec570 -> 0xaed880` is `IDENTICAL` over 9 and is refused one by
`MIN_VERIFIED_INSNS`. Verdict quality is not hook-target validity, and no number of matching
instructions is.

The distinguishing question is not "is there a `.pdata` row AT this address" but "is this address
inside some other function's declared extent". That is what this answers.

Usage
    python3 scripts/classify-1170-entry-kind.py 0x140958b37 0x140836f30
    python3 scripts/classify-1170-entry-kind.py --map docs/recon/rva-map-1162-to-1170.verified.tsv
    python3 scripts/classify-1170-entry-kind.py --fail-on-mid      # gate mode, the tables build.rs reads
    python3 scripts/classify-1170-entry-kind.py --selftest

To see the neighbourhood of an address this flags, `scripts/pdata-enclosing-function.py` prints
the surrounding `.pdata` records in both builds.
"""

import argparse
import bisect
import os
import struct
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Env-overridable so the gate runs against a relocated corpus rather than only this machine's
# layout; same names `scripts/pdata-enclosing-function.py` already honours.
OLD_IMAGE = os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin"))
NEW_IMAGE = os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin"))
BASE = 0x140000000

ENTRY = "ENTRY"
LEAF = "LEAF"
MID = "MID-FUNCTION"

# The tables `er-game-base/build.rs` reads for call and detour licences. A mid-function row in any
# of them is a licence to transfer control -- or to write five bytes -- into the middle of a live
# function, so all three are gated together in one process (loading both images is the only slow
# part, and it is done once).
#
# `rva-map-1162-to-1170.data.tsv` is deliberately not here: its rows are `.data` globals, which by
# construction sit in no function at all, so every row would classify leaf and say nothing. The
# candidate table `rva-map-1162-to-1170.tsv` is not here either -- it is a work list, not a
# licence, and a mid-function candidate in it is caught when the row is promoted.
GATED_MAPS = (
    "docs/recon/rva-map-1162-to-1170.verified.tsv",
    "docs/recon/rva-map-1162-to-1170.needed-verified.tsv",
    "docs/recon/rva-map-1162-to-1170.needed.tsv",
)


def spans_from_image(image):
    """Sorted `(begin, end)` RVA pairs for every function the image's `.pdata` declares."""
    e_lfanew = struct.unpack_from("<I", image, 0x3C)[0]
    magic = struct.unpack_from("<H", image, e_lfanew + 24)[0]
    directories = e_lfanew + 24 + (112 if magic == 0x20B else 96)
    table_rva, table_size = struct.unpack_from("<II", image, directories + 3 * 8)
    out = []
    for offset in range(table_rva, table_rva + table_size, 12):
        begin, end, _unwind = struct.unpack_from("<III", image, offset)
        if begin or end:
            out.append((begin, end))
    out.sort()
    return out


def extents(path):
    """`spans_from_image` for an image on disk."""
    with open(path, "rb") as handle:
        return spans_from_image(handle.read())


def next_entry(spans, rva):
    """The first declared function start strictly after `rva`, or `None`."""
    index = bisect.bisect_right(spans, (rva, 1 << 62))
    return spans[index][0] if index < len(spans) else None


def classify(spans, starts, rva):
    """`(kind, detail)` for one RVA against one image's function table.

    For a mid-function address the detail carries the two addresses a reader needs to act: the
    entry of the function it landed inside, and the next entry after it. Which of the two the row
    meant is a judgement this cannot make -- `0x140aec480` landed inside `0x140aec120` but the
    address it was supposed to name is `0x140aec570`, the next one -- so both are printed and
    neither is presented as the answer.
    """
    if rva in starts:
        return ENTRY, "the image's own .pdata declares a function start here"
    index = bisect.bisect_right(spans, (rva, 1 << 62)) - 1
    if index >= 0 and spans[index][0] < rva < spans[index][1]:
        begin, end = spans[index]
        following = next_entry(spans, rva)
        detail = (
            f"inside 0x{BASE + begin:x}..0x{BASE + end:x} at +0x{rva - begin:x}"
            f"; enclosing entry 0x{BASE + begin:x}"
        )
        if following is not None:
            detail += f", next declared entry 0x{BASE + following:x}"
        return MID, detail
    return LEAF, "no .pdata entry and inside none"


def to_rva(value):
    return value - BASE if value >= BASE else value


def rows_from_map(path):
    """`(old_va, new_va)` pairs from any of the tab-separated map/verdict tables."""
    pairs = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 2 or fields[1] == "-":
                continue
            try:
                pairs.append((int(fields[0], 16), int(fields[1], 16)))
            except ValueError:
                continue
    return pairs


def audit(old, new, paths, list_rows=False, out=sys.stdout):
    """Classify every row of every table. Returns the mid-function findings.

    `old` and `new` are each `(spans, starts)` for one image, so a caller -- the selftest --
    can drive the whole gate against a synthetic function table with no game image present.
    """
    old_spans, old_starts = old
    new_spans, new_starts = new
    findings = []
    for path in paths:
        counts = {ENTRY: 0, LEAF: 0, MID: 0}
        rows = rows_from_map(path)
        for old_va, new_va in rows:
            src, src_detail = classify(old_spans, old_starts, to_rva(old_va))
            dst, dst_detail = classify(new_spans, new_starts, to_rva(new_va))
            counts[MID if MID in (src, dst) else (ENTRY if ENTRY in (src, dst) else LEAF)] += 1
            if list_rows:
                print(f"0x{old_va:x} -> 0x{new_va:x}  src={src:12s} dst={dst:12s}", file=out)
            if MID in (src, dst):
                findings.append(
                    {
                        "path": path,
                        "old_va": old_va,
                        "new_va": new_va,
                        "src": src,
                        "src_detail": src_detail,
                        "dst": dst,
                        "dst_detail": dst_detail,
                    }
                )
        print(
            f"{path}: {len(rows)} rows -- {counts[ENTRY]} entry, {counts[LEAF]} leaf, "
            f"{counts[MID]} MID-FUNCTION",
            file=out,
        )
    return findings


def report(findings, out=sys.stderr):
    """Print each mid-function row with the addresses needed to fix or drop it."""
    print(
        f"\n{len(findings)} MID-FUNCTION row(s). A row is a LICENCE: er-game-base/build.rs turns\n"
        "it into a CALL target, and a detour-grade one into an address MinHook overwrites five\n"
        "bytes of. Neither is survivable in the middle of a live function.",
        file=out,
    )
    for f in findings:
        print(f"\n  {f['path']}", file=out)
        print(f"    row  0x{f['old_va']:x} -> 0x{f['new_va']:x}", file=out)
        if f["src"] == MID:
            print(f"    1.16.2 source is MID-FUNCTION: {f['src_detail']}", file=out)
        if f["dst"] == MID:
            print(f"    1.17 destination is MID-FUNCTION: {f['dst_detail']}", file=out)
        print(
            f"    context: python3 scripts/pdata-enclosing-function.py "
            f"1162:0x{f['old_va']:x} 1170:0x{f['new_va']:x}",
            file=out,
        )
    print(
        "\nDo not read the row's verdict as a defence. A mid-function address verifies BETTER\n"
        "than a real entry -- it sits in a neighbourhood that did not change, so the comparison\n"
        "runs long and agrees everywhere. 0x140aec480 was merged on IDENTICAL 1.000 over 56\n"
        "instructions while being +0x360 inside 0x140aec120, with the real entry (0x140aec570)\n"
        "already written down in crates/er-title-flow/src/title_load_step_hooks.rs.\n"
        "Either re-derive the row at a real function entry, or drop it: an address the map does\n"
        "not carry is refused loudly by er-hook, which is the outcome you want.",
        file=out,
    )


def selftest():
    """Prove both senses of `NEITHER-ENTRY` are told apart, and that the gate fires on the bad one.

    The synthetic half runs anywhere: it builds a PE header and a `.pdata` table in memory, so the
    classification rule and the whole `--fail-on-mid` path are proven with no game image present.
    The image-backed half pins the two real precedents and skips, saying so, when the images are
    absent.
    """
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    # A minimal flat image: e_lfanew at 0x3c, PE32+ magic, data directory 3 -> our table. The deobf
    # images are flat (file offset == RVA), which is what lets the table sit at its own RVA.
    image = bytearray(0x4000)
    e_lfanew = 0x80
    struct.pack_into("<I", image, 0x3C, e_lfanew)
    struct.pack_into("<H", image, e_lfanew + 24, 0x20B)
    directories = e_lfanew + 24 + 112
    table_rva = 0x1000
    functions = [(0x2000, 0x2100), (0x2100, 0x2180), (0x3000, 0x3010)]
    struct.pack_into("<II", image, directories + 3 * 8, table_rva, 12 * len(functions))
    for index, (begin, end) in enumerate(functions):
        struct.pack_into("<III", image, table_rva + 12 * index, begin, end, 0)
    spans = spans_from_image(bytes(image))
    check("synthetic .pdata parses", spans, functions)
    starts = {b for b, _ in spans}

    check("declared start is ENTRY", classify(spans, starts, 0x2000)[0], ENTRY)
    check("shared boundary is ENTRY", classify(spans, starts, 0x2100)[0], ENTRY)
    # Sense one of neither-ENTRY: a genuine leaf. No `.pdata` row, and inside nobody's extent --
    # the x64 ABI's licence to omit unwind data. Hooking one of these is fine, and refusing them
    # is what would throw away ELDEN RING's getters and thunks.
    check("gap between functions is LEAF", classify(spans, starts, 0x2500)[0], LEAF)
    check("one past a function end is LEAF", classify(spans, starts, 0x2180)[0], LEAF)
    check("past the last function is LEAF", classify(spans, starts, 0x3010)[0], LEAF)
    # Sense TWO: inside a function. Same `NEITHER-ENTRY` word from the verdict table, opposite
    # meaning -- five bytes written here corrupt a live body.
    kind, detail = classify(spans, starts, 0x2080)
    check("inside a declared function is MID", kind, MID)
    check(
        "MID detail names the containing extent, the offset and both candidate entries",
        detail,
        "inside 0x140002000..0x140002100 at +0x80; enclosing entry 0x140002000, "
        "next declared entry 0x140002100",
    )

    # The gate end to end, on a table it has never seen: one clean row and one mid-function row.
    synthetic = (spans, starts)
    with tempfile.TemporaryDirectory() as scratch:
        table = os.path.join(scratch, "synthetic.tsv")
        with open(table, "w", encoding="utf-8") as handle:
            handle.write("# 1.16.2 VA\t1.17 VA\tverdict\tratio\tinsns\thow\tentry\n")
            handle.write("0x140002000\t0x140003000\tIDENTICAL\t1.000\t56\tsynthetic\tBOTH-ENTRIES\n")
            handle.write("0x140002080\t0x140003000\tIDENTICAL\t1.000\t56\tsynthetic\tNEITHER-ENTRY\n")
        with open(os.devnull, "w", encoding="utf-8") as quiet:
            findings = audit(synthetic, synthetic, [table], out=quiet)
    check("gate finds exactly the mid-function row", len(findings), 1)
    if findings:
        check("gate names the offending source", findings[0]["old_va"], 0x140002080)
        check("gate reports which side is mid", findings[0]["src"], MID)
        # A row that verifies identical over 56 instructions is still refused: the verdict column
        # is not evidence about where the address sits.
        check("clean-verdict row is not rescued by its verdict", findings[0]["dst"], ENTRY)

    if not (os.path.exists(OLD_IMAGE) and os.path.exists(NEW_IMAGE)):
        print(f"selftest: image-backed half SKIPPED ({OLD_IMAGE} / {NEW_IMAGE} absent)")
    else:
        old_spans = extents(OLD_IMAGE)
        old_starts = {b for b, _ in old_spans}
        # The precedent. Merged on a clean verdict, +0x360 inside another function, with the real
        # entry already recorded in the tree.
        kind, detail = classify(old_spans, old_starts, to_rva(0x140AEC480))
        check("0x140aec480 is MID-FUNCTION", kind, MID)
        check(
            "0x140aec480 names its container and the real entry",
            detail,
            "inside 0x140aec120..0x140aec567 at +0x360; enclosing entry 0x140aec120, "
            "next declared entry 0x140aec570",
        )
        # The second one that was already merged and removed: the byte after a `call`.
        check(
            "0x140958b37 is MID-FUNCTION",
            classify(old_spans, old_starts, to_rva(0x140958B37))[0],
            MID,
        )
        # A real leaf from the same table, so the check is not merely refusing everything: a
        # 0x10-byte `mov/mov/jmp` thunk (CS::ChrIns::GetPhysicsHitHeight) with no `.pdata` row.
        check(
            "0x1403efc20 is a genuine LEAF",
            classify(old_spans, old_starts, to_rva(0x1403EFC20))[0],
            LEAF,
        )
        check(
            "0x140aec570 is a declared ENTRY",
            classify(old_spans, old_starts, to_rva(0x140AEC570))[0],
            ENTRY,
        )

    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("vas", nargs="*", help="addresses (VA or RVA) to classify")
    parser.add_argument(
        "--map",
        metavar="PATH",
        action="append",
        help="classify both columns of a map table; repeatable. Defaults to the tables "
        "er-game-base/build.rs reads.",
    )
    parser.add_argument(
        "--rows", action="store_true", help="list every row, not just the summary and the failures"
    )
    parser.add_argument(
        "--fail-on-mid",
        action="store_true",
        help="exit 1 if any row is MID-FUNCTION on either side -- such a row must not carry a "
        "detour, so it does not belong in a table build.rs reads for detour licences",
    )
    parser.add_argument("--selftest", action="store_true", help="prove the classification rule")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    missing = [path for path in (OLD_IMAGE, NEW_IMAGE) if not os.path.exists(path)]
    if missing:
        # The de-Arxan'd images are untracked by policy (copyrighted game bytes), so a checkout
        # without them cannot run this. Say so rather than fail: the selftest above still proves
        # the rule, and `scripts/dearxan-deobfuscate.rs` regenerates the images.
        print(f"entry-kind classification skipped: {', '.join(missing)} absent")
        return 0

    old_spans, new_spans = extents(OLD_IMAGE), extents(NEW_IMAGE)
    old = (old_spans, {b for b, _ in old_spans})
    new = (new_spans, {b for b, _ in new_spans})

    if args.vas:
        for text in args.vas:
            rva = to_rva(int(text, 16))
            src, src_detail = classify(*old, rva)
            dst, dst_detail = classify(*new, rva)
            print(f"{text}: 1.16.2 {src} ({src_detail}) | at same RVA in 1.17 {dst} ({dst_detail})")
        return 0

    paths = args.map or [os.path.join(ROOT, name) for name in GATED_MAPS]
    findings = audit(old, new, paths, list_rows=args.rows)
    if findings:
        # The summaries went to stdout and the report goes to stderr; without this they interleave
        # and the failure block lands above the table it is about.
        sys.stdout.flush()
        report(findings)
        if args.fail_on_mid:
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
