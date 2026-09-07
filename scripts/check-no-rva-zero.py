#!/usr/bin/env python3
"""Forbid resolving RVA 0 through the 1.16.2 -> 1.17 address resolver.

Why
===
RVA 0 is the PE header. It is never a code or data address, so the verified 1.16.2 -> 1.17
map has nothing to translate it to and `resolve_game_address` correctly refuses it -- every
single time, forever, at whatever rate the call site runs.

That is not theoretical. One session (2026-08-29 -> 2026-08-30, a 14.9 GB
`er-quickload-autoload-debug.log`) logged 339,764 lines reading

    Address refused (game_rva): 0x140000000 -- game FileVersion 2.7.0.0 ...

because `delay_delete_pending` called `game_rva(0)` purely to obtain the module base, and it
sits on the 4 Hz telemetry write. The refusals were pure noise -- the code wanted
`game_module_base()`, which is version-independent and cannot be refused.

The rule
========
Ask for the module base with `er_game_base::mem::game_module_base()`. Do not launder it
through an address resolver by adding an RVA of 0. The fix belongs at the call site: the
resolver must not be taught to tolerate 0, because a real constant that has silently
degraded to 0 (an unset `AtomicUsize`, a sentinel like `TRACE_UNKNOWN_TABLE_RVA`, a lookup
that fell back) is a bug the refusal is right to expose -- guard the sentinel before the
call instead.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CRATES_DIR = REPO_ROOT / "crates"

ZERO = r"0(?:x0+|u32|usize)*"

# Each pattern is (description, compiled regex). They cover the resolver entry points that
# take an RVA: the one-argument and two-argument `game_rva` spellings, the named form, and
# the read-side `game_data_addr(base, rva, "NAME")`.
FORBIDDEN = (
    (
        "game_rva(0) -- use er_game_base::mem::game_module_base() instead",
        re.compile(rf"\bgame_rva\s*\(\s*{ZERO}\s*\)"),
    ),
    (
        "game_rva(base, 0) -- use the module base directly instead",
        re.compile(rf"\bgame_rva\s*\(\s*[A-Za-z0-9_.:]+\s*,\s*{ZERO}\s*\)"),
    ),
    (
        "game_rva_named(0, ...) -- use er_game_base::mem::game_module_base() instead",
        re.compile(rf"\bgame_rva_named\s*\(\s*{ZERO}\s*,"),
    ),
    (
        "game_data_addr(base, 0, ...) -- that is the module base, not a global",
        re.compile(rf"\bgame_data_addr\s*\(\s*[A-Za-z0-9_.:]+\s*,\s*{ZERO}\s*,"),
    ),
)


def scan_text(path_label: str, text: str) -> list[str]:
    """Every forbidden RVA-0 resolution in `text`, as reportable lines."""
    findings: list[str] = []
    for lineno, line in enumerate(text.split("\n"), 1):
        stripped = line.lstrip()
        # Comments and doc comments describe the rule (this file's own message included);
        # only real calls are violations.
        if stripped.startswith("//"):
            continue
        for description, pattern in FORBIDDEN:
            if pattern.search(line):
                findings.append(f"{path_label}:{lineno}: {description}\n    {stripped[:140]}")
    return findings


def scan_repo() -> list[str]:
    findings: list[str] = []
    for path in sorted(CRATES_DIR.rglob("*.rs")):
        findings.extend(
            scan_text(
                str(path.relative_to(REPO_ROOT)),
                path.read_text(encoding="utf-8", errors="replace"),
            )
        )
    return findings


def selftest() -> int:
    """Drive the scanner over synthetic sources, so the gate is never trusted on its own say-so."""
    failures: list[str] = []

    bad = "\n".join(
        (
            "    let base = game_rva(0).ok()?;",
            "    let base = game_rva(0x0).unwrap_or(0);",
            "    let f = game_rva(module_base, 0)?;",
            '    let a = game_rva_named(0, "NAME")?;',
            '    let g = game_data_addr(base, 0, "NAME");',
        )
    )
    hits = scan_text("fixture.rs", bad)
    if len(hits) != 5:
        failures.append(f"expected 5 violations in the bad fixture, got {len(hits)}: {hits}")

    good = "\n".join(
        (
            "    let base = er_game_base::mem::game_module_base().ok()?;",
            "    let f = game_rva(SOME_RVA)?;",
            '    let a = game_rva_named(SOME_RVA, "SOME_RVA")?;',
            '    let g = game_data_addr(base, SOME_RVA, "SOME_RVA");',
            "    // game_rva(0) in a comment is the rule being explained, not a call",
        )
    )
    hits = scan_text("fixture.rs", good)
    if hits:
        failures.append(f"clean fixture must not report violations, got {hits}")

    for failure in failures:
        print(f"selftest FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("check-no-rva-zero selftest: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--selftest",
        action="store_true",
        help="run the scanner against synthetic fixtures instead of the repo",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    findings = scan_repo()
    if findings:
        print("RVA 0 resolved through the address resolver:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        print(
            "\nRVA 0 is the PE header. The resolver refuses it on every non-1.16.2 build, "
            "forever, at the call site's own rate -- one such site logged 339,764 refusals in "
            "a single session. Use er_game_base::mem::game_module_base() for the base, and "
            "guard sentinel/unset RVAs before the call.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
