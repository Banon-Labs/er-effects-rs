#!/usr/bin/env python3
"""Find the game addresses in product source that 1.17.1 moved out from under.

The problem this answers
------------------------
Most game addresses in this workspace are 1.16.2 rvas, and those are safe: they
go through `er_game_base::resolve_game_address`, which looks them up in the
1.16.2-to-1.17.0 map and then carries the answer onto the running build. A few
are not. A `PrologueSpec` whose `image` is `Image::EldenRing1170` is spelled at
its 1.17.0 address on purpose -- a signature has to be assembled against the
bytes it will be compared with, so its site cannot be a 1.16.2 address -- and
those bypass the map entirely. 1.17.1 moved every function at or above
`0xafefe9` by `0x70`, so a 1.17.0-spelled address above that line would now be
wrong with nothing to say so.

Why this reads specs and not files
----------------------------------
The first version of this script decided per file: if a file mentioned
`Image::EldenRing1170` anywhere, every moving address in it was called
1.17.0-spelled. That reported 13 stale constants and all but three were false.
`crates/er-save-suppress/build.rs` holds one 1.17.0 spec and twenty-odd 1.16.2
ones, and the file-level rule called the whole file 1.17.0. The tell was that
the flagged constants' bytes do not match the 1.17.0 image at the address the
file gives: `SAVE_DISPATCH_CHAR_SIG` opens `48 89 5c 24 20`, and
`SAVE_DISPATCH_CHAR_VA` points at `89 81 ac 00 00` there, because that address
is a 1.16.2 one.

Ledger membership cannot decide it either, which is what the version before
that tried: an address can be a function start in both builds at once, so
`0x142413860` is in the 1.16.2 column of the map and is also a real 1.17.0
address. The only evidence that settles which build a constant is spelled in is
the `image` field of the spec that uses it, so that is what this reads.

    python3 scripts/audit-1171-stale-constants.py
    python3 scripts/audit-1171-stale-constants.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BUILD_SCRIPTS = sorted(REPO.glob("crates/*/build.rs"))

# The image whose addresses do not go through the 1.16.2 map, and so are the
# ones 1.17.1 can invalidate silently.
PINNED_IMAGE = "EldenRing1170"

U64_CONST = re.compile(r"const\s+([A-Z0-9_]+)\s*:\s*u64\s*=\s*(0x[0-9a-fA-F_]+)\s*;")
IMAGE_FIELD = re.compile(r"\bimage:\s*Image::(\w+)")
SPEC_START = "PrologueSpec {"


def load_mapper():
    path = REPO / "scripts/map-rvas-1170-to-1171.py"
    spec = importlib.util.spec_from_file_location("map1171", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def spec_blocks(text: str):
    """Yield the body of each `PrologueSpec { .. }`, matched by counting braces.

    A regex ending at the first `},` stops early inside any spec whose fields
    nest braces, which drops specs from the audit -- and a spec this misses is
    exactly a spec nobody is checking.
    """
    cursor = 0
    while True:
        start = text.find(SPEC_START, cursor)
        if start == -1:
            return
        open_brace = start + len(SPEC_START) - 1
        depth = 0
        for index in range(open_brace, len(text)):
            if text[index] == "{":
                depth += 1
            elif text[index] == "}":
                depth -= 1
                if depth == 0:
                    yield text[open_brace + 1 : index]
                    cursor = index
                    break
        else:
            return


def pinned_specs():
    """Every spec pinned to the 1.17.0 image, as (file, name, raw va, value)."""
    out = []
    for path in BUILD_SCRIPTS:
        text = path.read_text(encoding="utf-8", errors="replace")
        constants = {
            name: int(value.replace("_", ""), 16) for name, value in U64_CONST.findall(text)
        }
        for body in spec_blocks(text):
            image = IMAGE_FIELD.search(body)
            if not image or image.group(1) != PINNED_IMAGE:
                continue
            name = re.search(r'\bname:\s*"([^"]+)"', body)
            va = re.search(r"\bva:\s*([A-Z0-9_]+|0x[0-9a-fA-F_]+)", body)
            if not (name and va):
                continue
            raw = va.group(1)
            value = int(raw.replace("_", ""), 16) if raw.startswith("0x") else constants.get(raw)
            out.append((str(path.relative_to(REPO)), name.group(1), raw, value))
    return out


def audit(mapper):
    moved, unresolved = [], []
    specs = pinned_specs()
    for rel, name, raw, value in specs:
        if value is None:
            unresolved.append((rel, name, raw))
            continue
        carried, why = mapper.map_va(value)
        if carried != value:
            moved.append((rel, name, raw, value, carried, why))
    return specs, moved, unresolved


def selftest(mapper) -> int:
    ok = True

    def check(label, cond, detail=""):
        nonlocal ok
        print(f"  {'ok  ' if cond else 'FAIL'}  {label}{(' -- ' + detail) if detail else ''}")
        ok = ok and cond

    blocks = 0
    declared = 0
    for path in BUILD_SCRIPTS:
        text = path.read_text(encoding="utf-8", errors="replace")
        blocks += sum(1 for _ in spec_blocks(text))
        declared += len(IMAGE_FIELD.findall(text))
    check(
        "brace matching finds at least as many specs as there are image fields",
        blocks >= declared > 0,
        f"{blocks} spec blocks, {declared} image fields",
    )

    specs, _moved, unresolved = audit(mapper)
    check(f"{len(specs)} spec(s) pinned to the 1.17.0 image", len(specs) > 0)
    check("every pinned spec resolved its va", not unresolved, str(unresolved))

    carried, _ = mapper.map_va(0x14067B7D0)
    check("QUIT_PHASE_SETTLE stays put", carried == 0x14067B7D0, f"got {carried:#x}")
    carried, _ = mapper.map_va(0x142413860)
    check("an address above the boundary does carry", carried == 0x1424138D0, f"got {carried:#x}")

    text = (REPO / "crates/er-save-suppress/build.rs").read_text(encoding="utf-8", errors="replace")
    images = {
        match.group(1)
        for body in spec_blocks(text)
        for match in [IMAGE_FIELD.search(body)]
        if match
    }
    check(
        "one build.rs mixes 1.16.2 and 1.17.0 specs, so a file-level rule cannot work",
        len(images) > 1,
        f"images in er-save-suppress/build.rs: {sorted(images)}",
    )
    print()
    if not ok:
        print("selftest FAILED")
        return 1
    print("selftest passed")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()

    mapper = load_mapper()
    if a.selftest:
        return selftest(mapper)

    specs, moved, unresolved = audit(mapper)
    print(f"prologue specs pinned to the 1.17.0 image: {len(specs)}")
    for rel, name, raw, value in specs:
        if value is None:
            print(f"  {rel}  {name}  va={raw} (unresolved)")
            continue
        carried, _ = mapper.map_va(value)
        verdict = "MOVES" if carried != value else "stays"
        print(f"  {rel}  {name}  {value:#x} -> {carried:#x}  {verdict}")

    if unresolved:
        print(f"\nspecs whose va could not be resolved: {len(unresolved)}")

    print(f"\n1.17.0-spelled addresses that 1.17.1 moved: {len(moved)}")
    for rel, name, _raw, value, carried, why in moved:
        print(f"  {rel}  {name}  {value:#x} -> {carried:#x}  ({why})")
    if not moved and not unresolved:
        print(
            "  none. Every 1.17.0-spelled site is below the boundary, so the patch did not move\n"
            "  any of them. Addresses spelled at 1.16.2 are carried at runtime by\n"
            "  er_game_base::resolve_game_address and are not this script's business."
        )
    return 1 if (moved or unresolved) else 0


if __name__ == "__main__":
    sys.exit(main())
