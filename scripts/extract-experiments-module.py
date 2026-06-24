#!/usr/bin/env python3
"""Carve a contiguous line range out of src/experiments/mod.rs into a sibling
submodule, preserving behavior (pure code motion).

    extract-experiments-module.py <module> <start_line> <end_line>

Line numbers are 1-based inclusive, referring to the CURRENT mod.rs. The moved
range is written to src/experiments/<module>.rs with a copy of mod.rs's import
preamble plus `use super::*;` (so it sees every pub(crate) sibling item), and a
`mod <module>; pub(crate) use <module>::*;` declaration is inserted into mod.rs
right after the preamble. Run one module per invocation, then rebuild.

The preamble is everything up to and including the last line of the leading
`use`/attribute block (detected as the first blank line following a line that is
neither an attribute, a `use`, nor inside a use-tree). We pin it explicitly to
the known header end to stay deterministic.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
MOD = REPO_ROOT / "src" / "experiments" / "mod.rs"


def detect_preamble_end(lines: list[str]) -> int:
    """Return the index (exclusive) of the end of the import/attribute preamble.

    Heuristic: the preamble ends at the first top-level `const`/`static`/`pub`
    item or banner-less code. We look for the first line that starts a
    `pub(crate) const`/`static`/`fn` or a non-use top-level declaration after the
    initial use block.
    """
    for i, line in enumerate(lines):
        s = line.strip()
        if s.startswith(("pub(crate) const", "pub(crate) static", "pub(crate) fn",
                         "pub(crate) unsafe fn", "static ", "const ", "fn ",
                         "unsafe fn ", "struct ", "enum ", "impl ")):
            return i
    raise SystemExit("could not locate end of preamble")


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit(__doc__)
    module = sys.argv[1]
    start = int(sys.argv[2])
    end = int(sys.argv[3])

    text = MOD.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    n = len(lines)
    if not (1 <= start <= end <= n):
        raise SystemExit(f"range {start}-{end} out of bounds (file has {n} lines)")

    preamble_end = detect_preamble_end(lines)
    if start <= preamble_end:
        raise SystemExit(
            f"range starts at {start} but preamble runs through line {preamble_end}; "
            "cannot extract into the preamble"
        )

    # Copy the import/attribute preamble into the submodule, but drop any
    # `mod <name>;` / `pub(crate) use <name>::*;` submodule declarations that
    # earlier extractions inserted -- those belong only to mod.rs.
    preamble_lines = [
        ln
        for ln in lines[:preamble_end]
        if not re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;\s*$", ln)
        and not re.match(r"\s*pub(?:\([^)]*\))?\s+use\s+\w+::\*\s*;\s*$", ln)
    ]
    preamble = "".join(preamble_lines)
    moved = "".join(lines[start - 1:end])

    out = REPO_ROOT / "src" / "experiments" / f"{module}.rs"
    if out.exists():
        raise SystemExit(f"{out} already exists")
    out.write_text(preamble + "\nuse super::*;\n\n" + moved, encoding="utf-8")

    decl = f"\nmod {module};\npub(crate) use {module}::*;\n"
    remaining = (
        "".join(lines[:preamble_end])
        + decl
        + "".join(lines[preamble_end:start - 1])
        + "".join(lines[end:])
    )
    MOD.write_text(remaining, encoding="utf-8")

    print(f"moved {end - start + 1} lines -> {out.relative_to(REPO_ROOT)}")
    print(f"mod.rs now {len(remaining.splitlines())} lines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
