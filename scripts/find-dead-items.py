#!/usr/bin/env python3
"""Report every item defined under a directory whose identifier appears NOWHERE else in the repo.

Companion to `scripts/find-identifier.py`. Written for the `startup_hooks/` dead-code sweep:
it enumerates `fn` / `const` / `static` / `struct` / `enum` / `type` definitions under a target
directory and counts whole-word occurrences of each identifier across every Rust source file in
the repo, EXCLUDING the definition line itself.

Zero remaining occurrences => no caller anywhere, including through a `pub(crate) use ...::*`
glob re-export (the search is on the bare identifier, never a qualified path, which is exactly
the case a `module::symbol` search misses).

A zero here is necessary but NOT sufficient to delete: a `match` arm or any other
runtime-unreachable branch inside a live function still emits machine code, and removing it
changes the shipped `.text`. Confirm the deletion with `scripts/dll-code-fingerprint.py`.

Usage:
    python3 scripts/find-dead-items.py <dir-or-file> [<dir-or-file> ...]
"""

from __future__ import annotations

import collections
import glob
import os
import re
import sys

REPO_ROOT = os.environ.get(
    "REPO_ROOT", os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)

DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"[A-Za-z-]+\"\s+)?"
    r"(fn|const|static|struct|enum|type|trait)\s+"
    r"(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)"
)

WORD_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

SOURCE_GLOBS = ("crates/**/*.rs", "tools/**/*.rs", "src/**/*.rs", "build.rs")


def all_sources(root: str) -> list[str]:
    found: set[str] = set()
    for pattern in SOURCE_GLOBS:
        found.update(glob.glob(os.path.join(root, pattern), recursive=True))
    return sorted(found)


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2

    targets: list[str] = []
    for entry in argv:
        if entry.endswith(".rs"):
            targets.append(entry)
        else:
            targets.extend(glob.glob(os.path.join(entry, "**", "*.rs"), recursive=True))
    targets = sorted({os.path.abspath(t) for t in targets})

    # One pass over the whole repo counting every identifier token. Definition lines are
    # counted too, then subtracted per definition below.
    counts: collections.Counter[str] = collections.Counter()
    for path in all_sources(REPO_ROOT):
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                counts.update(WORD_RE.findall(line))

    for path in targets:
        with open(path, encoding="utf-8", errors="replace") as handle:
            lines = handle.readlines()
        for lineno, line in enumerate(lines, 1):
            m = DEF_RE.match(line)
            if not m:
                continue
            kind, ident = m.group(1), m.group(2)
            on_def_line = len(
                [w for w in WORD_RE.findall(line) if w == ident]
            )
            if counts[ident] - on_def_line == 0:
                rel = os.path.relpath(path, REPO_ROOT)
                print(f"DEAD  {kind:7s} {ident:60s} {rel}:{lineno}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
