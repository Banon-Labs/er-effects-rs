#!/usr/bin/env python3
"""Verify the current crate-extraction R1 file and critical caller ledger."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPERIMENTS = ROOT / "crates/er-effects-rs/src/experiments"
SOURCE = ROOT / "crates/er-effects-rs/src"
ROADMAP = ROOT / "docs/plans/crate-extraction-execution-roadmap.md"
ROW = re.compile(r"^\| `([^`]+\.rs)` \| ([0-9,]+) \|", re.MULTILINE)
TOTAL = re.compile(r"^\| all `experiments/\*\*` \| ([0-9,]+) \| ([0-9,]+) \|$", re.MULTILINE)
FORBIDDEN_OWNERSHIP = re.compile(
    r"UNANALYSED|UNANALYZED|UNCLASSIFIED|APPROXIMATE(?:LY)?|ESTIMAT(?:E|ED|ES)|~",
    re.IGNORECASE,
)
# Matched by PATTERN, not by literal text, because the heading carries the file count and the
# count changes every time a file enters or leaves `experiments/**`. A literal match makes a
# truthful heading update break the gate, which pressures the next person to leave the number
# stale instead. Nothing is lost: the count is separately and exactly validated against
# measured source by the `all experiments/**` total row below.
LEDGER_HEADING = re.compile(
    r"^## Appendix A -- R1 current \d+-file partition and caller ledger$", re.MULTILINE
)

# These are source edges that define the ownership seams R1 refreshes. A later move must
# update this table and the roadmap in the same PR; it cannot silently preserve an old caller map.
REQUIRED_EDGES = {
    "save_flow_tick": {"experiments/lifecycle/task_tick.rs"},
    "tick_before_player_lookup": {"lib_parts/dll_entry_parts/task_registration.rs"},
    "install_title_visual_startup_hooks": {"lib_parts/dll_entry_parts/bootstrap.rs"},
    "install_profile_and_system_quit_hooks": {"lib_parts/dll_entry_parts/bootstrap.rs"},
    "install_boot_diagnostics_and_trace_hooks": {"lib_parts/dll_entry_parts/bootstrap.rs"},
    "own_load_pump_tick": {"experiments/lifecycle/task_tick.rs"},
    "own_load_switch_reload_fire": set(),
    "enforce_save_override_or_abort": {"lib_parts/dll_entry_parts/bootstrap.rs"},
    "install_save_redirect_hooks": {"experiments/save_redirect/path_hooks.rs"},
    "profile_editor_necromancy_tick": {"lib_parts/dll_entry_parts/task_registration.rs"},
    "profile_editor_runtime_tick": {
        "experiments/startup_hooks/loading_cover/title_resources_stats_text.rs"
    },
    "save_picker_request_path_editor": {
        "experiments/startup_hooks/quit_menu/save_picker_menu.rs"
    },
    "save_picker_menu_pump_path_editor": {
        "experiments/startup_hooks/quit_menu/profile_rows_system_quit_menu.rs"
    },
}

REQUIRED_TERMS = {
    "S10 lifecycle",
    "S11 own-load",
    "R12B1 transport",
    "R12B5 Scaleform primitives",
    "R13B1 path model",
    "R13B4 lifecycle adapter",
    "R32 re-baselines the existing `er-save-redirect` interface",
}


def current_inventory(root: Path = EXPERIMENTS) -> dict[str, int]:
    return {
        path.relative_to(root).as_posix(): sum(1 for _ in path.open(encoding="utf-8", errors="replace"))
        for path in sorted(root.rglob("*.rs"))
    }


def source_has_function(function: str) -> bool:
    pattern = re.compile(rf"\bfn\s+{re.escape(function)}\s*[<(]")
    return any(pattern.search(path.read_text(encoding="utf-8", errors="replace")) for path in SOURCE.rglob("*.rs"))


def source_has_call(caller: str, function: str) -> bool:
    path = SOURCE / caller
    if not path.is_file():
        return False
    return re.search(rf"\b{re.escape(function)}\s*\(", path.read_text(encoding="utf-8", errors="replace")) is not None


def validate(
    listed: dict[str, int],
    current: dict[str, int],
    expected_total: tuple[int, int] | None,
    ledger_count: int,
    text: str,
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
        errors.append("roadmap must contain exactly one R1 current partition and caller ledger")
    if match := FORBIDDEN_OWNERSHIP.search(text):
        errors.append(f"forbidden non-exact ownership marker: {match.group(0)!r}")
    for term in sorted(REQUIRED_TERMS):
        if term not in text:
            errors.append(f"roadmap missing required partition term: {term}")
    return errors


def validate_caller_edges() -> list[str]:
    errors: list[str] = []
    for function, callers in REQUIRED_EDGES.items():
        if not source_has_function(function):
            errors.append(f"required partition function missing from source: {function}")
        for caller in sorted(callers):
            if not source_has_call(caller, function):
                errors.append(f"required caller edge missing: {caller} -> {function}")
    return errors


def selftest() -> int:
    clean = {"a.rs": 2, "b.rs": 3}
    cases = [
        ("clean", clean, clean, (2, 5), 1, "exact", 0),
        ("missing file", {"a.rs": 2}, clean, (2, 5), 1, "exact", 1),
        ("stale file", {**clean, "old.rs": 1}, clean, (2, 5), 1, "exact", 1),
        ("line drift", {"a.rs": 1, "b.rs": 3}, clean, (2, 5), 1, "exact", 1),
        ("total drift", clean, clean, (2, 6), 1, "exact", 1),
        ("missing total", clean, clean, None, 1, "exact", 1),
        ("duplicate ledger", clean, clean, (2, 5), 2, "exact", 1),
        ("forbidden marker", clean, clean, (2, 5), 1, "approximate", 1),
    ]
    failures = []
    required_text = "\n".join(REQUIRED_TERMS)
    for label, listed, current, total, ledgers, text, want_errors in cases:
        got = validate(listed, current, total, ledgers, f"{required_text}\n{text}")
        if (len(got) == 0) != (want_errors == 0):
            failures.append(f"{label}: expected {'pass' if want_errors == 0 else 'failure'}, got {got}")
    if failures:
        for failure in failures:
            print(f"[check-crate-extraction-roadmap] selftest FAILED: {failure}", file=sys.stderr)
        return 1
    print(f"[check-crate-extraction-roadmap] selftest ok ({len(cases)} cases)")
    return 0


def refresh() -> int:
    """Rewrite the ledger's line counts and total from measured source.

    The ledger is a MEASURED MIRROR of `experiments/**`, not a plan of intent -- a row exists
    if and only if the file exists, with an exact line count, and the total row must match
    (file count, summed lines). So there is no judgement to apply and the refresh is mechanical,
    which is exactly why it belongs in the tool rather than in an ad-hoc script written from
    scratch each time. It had been hand-refreshed at least three times (PR #301 and twice during
    the 2026-08-21 lint-parity sweep, where any edit that adds or removes a line silently rots
    the gate) before this mode existed.

    Only the COUNT column is rewritten. The description and R-number columns are authored
    judgement and are preserved verbatim; a row whose file has left `experiments/**` is dropped,
    and a file with no row is reported rather than invented, because inventing one would mean
    inventing the ownership claim next to it.
    """
    text = ROADMAP.read_text(encoding="utf-8")
    inventory = current_inventory()
    seen: set[str] = set()
    retuned: list[str] = []
    dropped: list[str] = []

    full_row = re.compile(r"^\| `([^`]+\.rs)` \| ([0-9,]+) \|(.*)$", re.MULTILINE)
    out: list[str] = []
    for line in text.splitlines():
        match = full_row.match(line)
        if match is None:
            out.append(line)
            continue
        path, old, rest = match.group(1), match.group(2), match.group(3)
        if path not in inventory:
            dropped.append(path)
            continue
        seen.add(path)
        new = f"{inventory[path]:,}"
        if new != old:
            retuned.append(f"{path} {old}->{new}")
        out.append(f"| `{path}` | {new} |{rest}")
    text = "\n".join(out) + ("\n" if text.endswith("\n") else "")

    text = TOTAL.sub(
        f"| all `experiments/**` | {len(inventory):,} | {sum(inventory.values()):,} |", text
    )
    ROADMAP.write_text(text, encoding="utf-8")

    for entry in retuned:
        print(f"[check-crate-extraction-roadmap] retuned {entry}")
    for entry in dropped:
        print(f"[check-crate-extraction-roadmap] dropped (file left experiments/**): {entry}")
    missing = sorted(set(inventory) - seen)
    for entry in missing:
        print(
            f"[check-crate-extraction-roadmap] NO ROW for {entry} -- add one by hand with its "
            "ownership claim; this tool will not invent that column",
            file=sys.stderr,
        )
    print(
        f"[check-crate-extraction-roadmap] refreshed: {len(inventory)} files / "
        f"{sum(inventory.values()):,} lines"
    )
    return 1 if missing else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument(
        "--refresh",
        action="store_true",
        help="rewrite the ledger line counts + total from measured source (mechanical)",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.refresh:
        return refresh()

    text = ROADMAP.read_text(encoding="utf-8")
    listed = {path: int(lines.replace(",", "")) for path, lines in ROW.findall(text)}
    total = TOTAL.search(text)
    expected_total = (
        (int(total.group(1).replace(",", "")), int(total.group(2).replace(",", "")))
        if total is not None
        else None
    )
    errors = validate(
        listed,
        current_inventory(),
        expected_total,
        len(LEDGER_HEADING.findall(text)),
        text,
    )
    errors.extend(validate_caller_edges())
    if errors:
        for error in errors:
            print(f"[check-crate-extraction-roadmap] ERROR: {error}", file=sys.stderr)
        print(
            "Refresh the R1 roadmap from current source; do not carry forward stale line ranges or caller claims.",
            file=sys.stderr,
        )
        return 1
    print(
        "[check-crate-extraction-roadmap] ok -- "
        f"{len(listed)} files / {sum(listed.values()):,} lines and critical caller edges match"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
