#!/usr/bin/env python3
"""Put `#[cfg(feature = "quit-rows")]` on the declarations the trimmed build reports as dead.

The rows' constant tables, statics and imports are spread through `constants/` and the shared
module headers rather than sitting inside `startup_hooks/quit_menu/`. With the rows not compiled
they are genuinely unused, and this crate denies `rust.warnings`, so every one is an error.

The compiler names them exactly, so the gate is placed from its output rather than guessed: build
with the feature off, read `never used` / `unused import` at `file:line`, insert the attribute
above that line, build again. A declaration that turns out to be reachable without the rows fails
on the next pass with `cannot find` instead, which is the signal to take that one back out -- so
the loop is checked by the compiler at every step, not by this script's judgement.

    python3 scripts/gate-quickload-quit-rows-deadcode.py            # iterate until clean
    python3 scripts/gate-quickload-quit-rows-deadcode.py --dry-run  # report the first pass only
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
GATE = '#[cfg(feature = "quit-rows")]'
BUILD = [
    "cargo", "xwin", "check", "-p", "er-quickload",
    "--target", "x86_64-pc-windows-msvc",
    "--no-default-features",
    "--features", "autoload,save-picker,loading-cover,portrait,menu-trace",
    "--message-format", "short",
]
DEAD = re.compile(r"^(crates/\S+\.rs):(\d+):\d+: error: (?:constant|static|function|type alias|struct|enum|unused import|unused imports|.*?is never used)")
DEAD_KIND = re.compile(r"error: (unused imports?|(?:constant|static|function|type alias|struct|enum|method|field) `[^`]+` is never used)")


# Every subprocess in this repo is capped at 30 seconds so a mistake fails in seconds rather than
# minutes (`scripts/check-no-timeouts.py`). A cold build of this crate does not fit in that, and
# should not: warm it first and each pass here is a few seconds.
BUILD_TIMEOUT_SECONDS = 30


def build() -> str:
    try:
        done = subprocess.run(
            BUILD, cwd=REPO, capture_output=True, text=True, timeout=BUILD_TIMEOUT_SECONDS
        )
    except subprocess.TimeoutExpired as expired:
        raise SystemExit(
            f"the check did not finish in {BUILD_TIMEOUT_SECONDS}s. Warm the build first by "
            f"running the command this script drives:\n    {' '.join(BUILD)}\n"
            "then run this again -- each pass is then a few seconds."
        ) from expired
    return done.stdout + done.stderr


def dead_sites(output: str) -> list[tuple[pathlib.Path, int]]:
    """The `file:line` of every declaration the compiler called dead, newest-first per file."""
    found: set[tuple[str, int]] = set()
    for line in output.splitlines():
        if not DEAD_KIND.search(line):
            continue
        head = line.split(": error:", 1)[0]
        parts = head.split(":")
        if len(parts) < 3 or not parts[0].startswith("crates/"):
            continue
        try:
            found.add((parts[0], int(parts[1])))
        except ValueError:
            continue
    # Descending line order so an insertion never shifts a later target in the same file.
    return [(REPO / f, n) for f, n in sorted(found, key=lambda p: (p[0], -p[1]))]


def place(sites: list[tuple[pathlib.Path, int]]) -> int:
    placed = 0
    skipped: list[tuple[pathlib.Path, int]] = []
    for path, line in sites:
        lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
        idx = line - 1
        if idx >= len(lines):
            continue
        # Walk up over the declaration's own doc block and attributes so the gate leads them.
        while idx > 0 and (
            lines[idx - 1].lstrip().startswith("///")
            or lines[idx - 1].lstrip().startswith("#[")
        ):
            idx -= 1
        if idx > 0 and lines[idx - 1].strip() == GATE:
            continue
        # Only ever gate a declaration that starts its own line at column 0. An indented target is
        # a member of a brace group -- a `use a::{b, c}` list, a struct body -- and an attribute
        # inside one is a syntax error, not a narrower build. Measured 2026-09-12: a pass without
        # this guard turned 77 dead declarations into 983 errors, most of them
        # `expected identifier, found #`, and the damage had to be unpicked file by file. An
        # indented site is left for a person to read; it is nearly always a shared import that
        # wants restructuring rather than a gate.
        if lines[idx][:1].isspace():
            skipped.append((path, idx + 1))
            continue
        lines.insert(idx, f"{GATE}\n")
        path.write_text("".join(lines), encoding="utf-8")
        placed += 1
    for path, line in skipped:
        print(f"    left for a person (indented, inside a group): {path.relative_to(REPO)}:{line}")
    return placed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--max-passes", type=int, default=12)
    args = parser.parse_args()

    for attempt in range(1, args.max_passes + 1):
        output = build()
        sites = dead_sites(output)
        hard = [
            l for l in output.splitlines()
            if ": error" in l and not DEAD_KIND.search(l) and "aborting" not in l
            and "could not compile" not in l
        ]
        print(f"pass {attempt}: {len(sites)} dead declaration(s), {len(hard)} other error(s)")
        if hard:
            for l in hard[:20]:
                print("   ", l)
        if not sites:
            return 0 if not hard else 1
        if args.dry_run:
            for path, line in sites[:40]:
                print(f"    would gate {path.relative_to(REPO)}:{line}")
            return 0
        if not place(sites):
            print("no gate could be placed; stopping so the remainder is read by hand")
            return 1
    print("ran out of passes")
    return 1


if __name__ == "__main__":
    sys.exit(main())
