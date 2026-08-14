#!/usr/bin/env python3
"""Keep the PR #193 execution roadmap's current-file ledger honest.

The roadmap intentionally contains estimates and decision-gated destinations, but its baseline
file list and line counts are mechanical facts. Fail when a tracked Rust file is added/removed or
changes size without refreshing the roadmap baseline and disposition ledger.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPERIMENTS = ROOT / "crates/er-effects-rs/src/experiments"
ROADMAP = ROOT / "docs/plans/crate-extraction-execution-roadmap.md"
ROW = re.compile(r"^\| `([^`]+\.rs)` \| ([0-9,]+) \|", re.MULTILINE)
TOTAL = re.compile(r"^\| all `experiments/\*\*` \| ([0-9,]+) \| ([0-9,]+) \|$", re.MULTILINE)


def current_inventory() -> dict[str, int]:
    return {
        path.relative_to(EXPERIMENTS).as_posix(): sum(1 for _ in path.open(errors="replace"))
        for path in sorted(EXPERIMENTS.rglob("*.rs"))
    }


def validate(
    listed: dict[str, int],
    current: dict[str, int],
    expected_total: tuple[int, int] | None,
    ledger_count: int,
) -> list[str]:
    errors: list[str] = []
    missing = sorted(current.keys() - listed.keys())
    stale = sorted(listed.keys() - current.keys())
    if missing:
        errors.append("files absent from roadmap ledger: " + ", ".join(missing))
    if stale:
        errors.append("roadmap ledger files absent from source: " + ", ".join(stale))
    for path in sorted(current.keys() & listed.keys()):
        if current[path] != listed[path]:
            errors.append(f"line count drift: {path}: roadmap={listed[path]} current={current[path]}")

    if expected_total is None:
        errors.append("current measured-state total row is missing")
    elif expected_total != (len(current), sum(current.values())):
        errors.append(
            "measured-state total drift: "
            f"roadmap={expected_total[0]}/{expected_total[1]} "
            f"current={len(current)}/{sum(current.values())}"
        )
    if ledger_count != 1:
        errors.append("roadmap must contain exactly one current disposition ledger")
    return errors


def selftest() -> int:
    clean = {"a.rs": 2, "b.rs": 3}
    cases = [
        ("clean", clean, clean, (2, 5), 1, 0),
        ("missing file", {"a.rs": 2}, clean, (2, 5), 1, 1),
        ("stale file", {**clean, "old.rs": 1}, clean, (2, 5), 1, 1),
        ("line drift", {"a.rs": 1, "b.rs": 3}, clean, (2, 5), 1, 1),
        ("total drift", clean, clean, (2, 6), 1, 1),
        ("missing total", clean, clean, None, 1, 1),
        ("duplicate ledger", clean, clean, (2, 5), 2, 1),
    ]
    failures = []
    for label, listed, current, total, ledgers, want_errors in cases:
        got = validate(listed, current, total, ledgers)
        if (len(got) == 0) != (want_errors == 0):
            failures.append(f"{label}: expected {'pass' if want_errors == 0 else 'failure'}, got {got}")
    if failures:
        for failure in failures:
            print(f"[check-crate-extraction-roadmap] selftest FAILED: {failure}", file=sys.stderr)
        return 1
    print(f"[check-crate-extraction-roadmap] selftest ok ({len(cases)} cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    text = ROADMAP.read_text(encoding="utf-8")
    listed = {path: int(lines.replace(",", "")) for path, lines in ROW.findall(text)}
    current = current_inventory()
    total = TOTAL.search(text)
    expected_total = (
        (int(total.group(1).replace(",", "")), int(total.group(2).replace(",", "")))
        if total is not None
        else None
    )
    errors = validate(
        listed,
        current,
        expected_total,
        text.count("## Appendix A -- current 79-file disposition ledger"),
    )
    if errors:
        for error in errors:
            print(f"[check-crate-extraction-roadmap] ERROR: {error}", file=sys.stderr)
        print(
            "Refresh docs/plans/crate-extraction-execution-roadmap.md from the current source; "
            "do not copy old line offsets forward.",
            file=sys.stderr,
        )
        return 1

    print(
        f"[check-crate-extraction-roadmap] ok -- {len(current)} files / "
        f"{sum(current.values()):,} lines match the disposition ledger"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
