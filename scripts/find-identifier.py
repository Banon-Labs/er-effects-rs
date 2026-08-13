#!/usr/bin/env python3
"""Whole-repo BARE-IDENTIFIER search across every Rust source file.

Why this exists: `rtk grep` false-negatives on tokens this repo uses constantly
(`continue`, `input`, `block`, `online`, `splash`, `experiments`, `GOLD_SAVE`, ...) and bare
`grep` is intercepted by the OPA guard. AGENTS.md therefore prescribes a `python3` regex
one-liner; this is that one-liner made reusable so a dead-code sweep can be re-run and
reviewed rather than retyped.

Searches for the identifier as a WHOLE WORD, never as a qualified path -- a symbol can be
reached through a `pub(crate) use ...::*` glob re-export from a file that never names its
module, so `module::symbol` searches under-report.

Usage:
    python3 scripts/find-identifier.py IDENT [IDENT ...]
    python3 scripts/find-identifier.py --count IDENT [IDENT ...]   # counts only

Exit status is 0 always; the caller reads the counts. Repo root is discovered from this
file's location, so it works from any cwd and in any worktree.
"""

from __future__ import annotations

import glob
import os
import re
import sys

REPO_ROOT = os.environ.get(
    "REPO_ROOT", os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
)

SOURCE_GLOBS = (
    "crates/**/*.rs",
    "tools/**/*.rs",
    "src/**/*.rs",
    "build.rs",
)


def source_files(root: str) -> list[str]:
    found: set[str] = set()
    for pattern in SOURCE_GLOBS:
        found.update(glob.glob(os.path.join(root, pattern), recursive=True))
    return sorted(found)


def main(argv: list[str]) -> int:
    counts_only = "--count" in argv
    idents = [a for a in argv if not a.startswith("--")]
    if not idents:
        print(__doc__)
        return 2

    files = source_files(REPO_ROOT)
    for ident in idents:
        rx = re.compile(r"\b" + re.escape(ident) + r"\b")
        hits: list[tuple[str, int, str]] = []
        for path in files:
            try:
                with open(path, encoding="utf-8", errors="replace") as handle:
                    for lineno, line in enumerate(handle, 1):
                        if rx.search(line):
                            hits.append(
                                (os.path.relpath(path, REPO_ROOT), lineno, line.rstrip())
                            )
            except OSError:
                continue
        print(f"=== {ident}: {len(hits)} hit(s)")
        if not counts_only:
            for path, lineno, line in hits:
                print(f"  {path}:{lineno}: {line[:200]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
