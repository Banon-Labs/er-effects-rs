#!/usr/bin/env python3
"""Guard against recreating giant Rust source files.

This is intentionally lighter than clippy: it keeps the refactor branch from
backsliding while semantic module extraction continues.
"""

from __future__ import annotations

import argparse
from pathlib import Path

DEFAULT_WARN_LINES = 900
DEFAULT_FAIL_LINES = 3200
SKIP_DIRS = {
    ".git",
    ".worktrees",
    "target",
    "save-files",
    "docs",
    "third_party",
}


def rust_files(root: Path) -> list[Path]:
    files: list[Path] = []
    for path in root.rglob("*.rs"):
        rel_parts = path.relative_to(root).parts
        if any(part in SKIP_DIRS for part in rel_parts):
            continue
        files.append(path)
    return sorted(files)


def line_count(path: Path) -> int:
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        return sum(1 for _ in handle)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="repository root (default: inferred from this script)",
    )
    parser.add_argument("--warn-lines", type=int, default=DEFAULT_WARN_LINES)
    parser.add_argument("--fail-lines", type=int, default=DEFAULT_FAIL_LINES)
    args = parser.parse_args()

    if args.warn_lines <= 0 or args.fail_lines <= 0:
        raise SystemExit("line thresholds must be positive")
    if args.warn_lines > args.fail_lines:
        raise SystemExit("--warn-lines must be <= --fail-lines")

    root = args.root.resolve()
    rows = sorted(
        ((line_count(path), path.relative_to(root)) for path in rust_files(root)),
        reverse=True,
    )
    failures = [(lines, path) for lines, path in rows if lines > args.fail_lines]
    warnings = [(lines, path) for lines, path in rows if lines > args.warn_lines]

    print(
        f"checked {len(rows)} Rust files; warn>{args.warn_lines} lines, "
        f"fail>{args.fail_lines} lines"
    )
    if warnings:
        print("largest Rust files:")
        for lines, path in warnings[:25]:
            marker = "FAIL" if lines > args.fail_lines else "warn"
            print(f"  {marker:4s} {lines:5d} {path}")
    if failures:
        print("\nRefactor required: Rust files above the hard size limit remain.")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
