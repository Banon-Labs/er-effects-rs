#!/usr/bin/env python3
"""No `IDENTICAL-LEAF` row may sit inside a function the image already declares.

What this guards, and why it is not the check already in the VERIFIER.
`IDENTICAL-LEAF` is the one verdict that issues its own detour licence: a leaf has no `.pdata`
entry, so it reaches `DETOURABLE_ENTRY_EVIDENCE` in `crates/er-game-base/build.rs` through the
`NEITHER-ENTRY` clause, which makes no claim about the entry at all. Everything that keeps that
safe rests on one premise -- that the address really is a whole function the linker chose not to
describe, rather than a point in the middle of one it did.

`add_leaf_extents` in `scripts/verify-rva-map-1170.py` tests a weaker premise than the one the
verdict rests on. It skips a VA when `rva in extents`, i.e. when a `.pdata` entry begins there.
An address 0x10 bytes into a declared function begins nothing, so it is not in `extents`, so a
leaf extent gets decoded for it, so it can reach `IDENTICAL-LEAF` and carry a detour into the
middle of a function. That is the precise shape of the failure the 1.17 migration keeps paying
for: a confident wrong address reads as a live value and corrupts silently, where a missing one
merely turns a feature off.

Measured 2026-08-30: zero rows in any current map are affected -- every derived extent belongs to
an address no `.pdata` entry covers in either image. So this is a latent hole, not a live bug, and
this file exists to keep it that way. Coverage is computed over the raw `.pdata` table including
`UNW_FLAG_CHAININFO` continuation chunks, because a continuation is still the interior of a
function even though `function_regions` deliberately drops it as a function start.

Usage
    uv run --with capstone python3 scripts/check-leaf-extent-pdata-coverage.py
    uv run --with capstone python3 scripts/check-leaf-extent-pdata-coverage.py --verbose

Exits non-zero when a derived leaf extent, or a written `IDENTICAL-LEAF` row, lands inside a
declared function region in either image.
"""

import argparse
import bisect
import importlib.util
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VERIFIER = os.path.join(ROOT, "scripts", "verify-rva-map-1170.py")
# Every candidate map whose pairs are fed to `add_leaf_extents`, plus the verdict tables
# `er-game-base/build.rs` actually reads. A map that does not exist is skipped, not failed: the
# set of maps has changed twice during this migration.
MAPS = [
    os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.verified.tsv"),
    os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.needed-verified.tsv"),
    os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.needed.tsv"),
    os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.tsv"),
]
LEAF_VERDICT = "IDENTICAL-LEAF"


def verifier():
    """The verifier module, imported by path because its filename is not an identifier."""
    spec = importlib.util.spec_from_file_location("verify_rva_map_1170", VERIFIER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def coverage(module, image):
    """Sorted `(begin, end)` of every `.pdata` region, continuation chunks included.

    `function_regions` merges chunk runs and drops continuations from the start set, which is
    right for "does a function begin here" and wrong for "is this address inside one". A chained
    chunk that is not contiguous with its primary would vanish from a merged view entirely, and
    its interior is exactly where a mis-mapped address most wants to hide.
    """
    return sorted((begin, end) for begin, end, _unwind in module.runtime_functions(image))


def containing(regions, rva):
    """Every region whose half-open `[begin, end)` contains `rva`."""
    index = bisect.bisect_right(regions, (rva, 1 << 62))
    # Regions can nest and can be listed out of order relative to their ends, so walk back a
    # bounded window rather than trusting the single nearest predecessor.
    return [(b, e) for b, e in regions[max(0, index - 16) : index] if b <= rva < e]


def leaf_rows(path):
    """`(old_va, new_va)` for each row a verdict table already records as `IDENTICAL-LEAF`."""
    rows = []
    if not os.path.exists(path):
        return rows
    for line in open(path, encoding="utf-8"):
        if line.startswith("#") or not line.strip():
            continue
        fields = line.rstrip("\n").split("\t")
        if len(fields) < 3 or fields[2] != LEAF_VERDICT:
            continue
        try:
            rows.append((int(fields[0], 16), int(fields[1], 16)))
        except ValueError:
            continue
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verbose", action="store_true", help="print per-map counts")
    args = parser.parse_args()

    # RE-EXEC under uv if capstone is absent (2026-08-31), the same bootstrap
    # `verify-thunk-rva-1170.py` and `verify-hook-address.py` already carry. There is no system
    # pip here, so `verifier()` -- which loads `verify-rva-map-1170.py`, which imports capstone --
    # died with a bare ImportError at exit 1. That is indistinguishable from a real finding, which
    # is why this gate could not be wired into check.sh. uv provisions capstone from its cache in
    # milliseconds. Doing it here rather than making check.sh spell `uv run --with capstone` keeps
    # the step recognisable to check.sh's own `python3 ...` step-pattern accounting, which does not
    # match a `uv` command and would silently drop this gate from the summary table and the total.
    try:
        import capstone  # noqa: F401
    except ImportError:
        try:
            os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])
        except OSError:
            # No capstone and no uv: say so and skip rather than traceback. A checkout that cannot
            # decode instructions cannot answer this question either way.
            print("skipped: capstone unavailable and `uv` is not on PATH")
            return 0

    module = verifier()
    for image_path in (module.OLD_IMAGE, module.NEW_IMAGE):
        if not os.path.exists(image_path):
            print(f"skipped: missing image {image_path}")
            return 0
    old_image = open(module.OLD_IMAGE, "rb").read()
    new_image = open(module.NEW_IMAGE, "rb").read()
    old_regions, new_regions = coverage(module, old_image), coverage(module, new_image)
    old_starts, new_starts = module.function_starts(old_image), module.function_starts(new_image)
    old_extents, new_extents = module.function_extents(old_image), module.function_extents(new_image)

    failures = []

    # 1. Every extent the verifier would derive, over every map that feeds it.
    for path in MAPS:
        if not os.path.exists(path):
            continue
        pairs = module.load_map(path)
        derived_old = module.add_leaf_extents(
            old_image, dict(old_extents), old_starts, [p[0] for p in pairs]
        )
        derived_new = module.add_leaf_extents(
            new_image, dict(new_extents), new_starts, [p[1] for p in pairs]
        )
        inside_old = [(r, containing(old_regions, r)) for r in sorted(derived_old)]
        inside_new = [(r, containing(new_regions, r)) for r in sorted(derived_new)]
        inside_old = [(r, c) for r, c in inside_old if c]
        inside_new = [(r, c) for r, c in inside_new if c]
        if args.verbose:
            print(
                f"{os.path.basename(path):<44} pairs={len(pairs):<5} "
                f"derived 1.16.2={len(derived_old):<4} 1.17={len(derived_new):<4} "
                f"covered={len(inside_old)}/{len(inside_new)}"
            )
        for rva, regions in inside_old:
            failures.append(
                f"{os.path.basename(path)}: 1.16.2 {rva + module.BASE:#x} would take a DERIVED "
                f"leaf extent, but .pdata already declares "
                f"{[(hex(b + module.BASE), hex(e + module.BASE)) for b, e in regions]}"
            )
        for rva, regions in inside_new:
            failures.append(
                f"{os.path.basename(path)}: 1.17 {rva + module.BASE:#x} would take a DERIVED "
                f"leaf extent, but .pdata already declares "
                f"{[(hex(b + module.BASE), hex(e + module.BASE)) for b, e in regions]}"
            )

    # 2. Every row a verdict table has already written as identical-leaf. Independent of the
    #    maps above, because a hand-added row reaches build.rs without passing through them.
    for path in MAPS:
        for old_va, new_va in leaf_rows(path):
            for tag, va, regions, base in (
                ("1.16.2", old_va, old_regions, module.BASE),
                ("1.17", new_va, new_regions, module.BASE),
            ):
                hits = containing(regions, va - base)
                if hits:
                    failures.append(
                        f"{os.path.basename(path)}: {tag} {va:#x} carries {LEAF_VERDICT} but sits "
                        f"inside .pdata region "
                        f"{[(hex(b + base), hex(e + base)) for b, e in hits]}"
                    )

    if failures:
        print(f"FAIL: {len(failures)} leaf extent(s) inside a declared function region")
        for line in failures:
            print(f"  {line}")
        return 1
    print("OK: no derived leaf extent and no IDENTICAL-LEAF row sits inside a .pdata region")
    return 0


if __name__ == "__main__":
    sys.exit(main())
