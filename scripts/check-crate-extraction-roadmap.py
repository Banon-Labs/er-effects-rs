#!/usr/bin/env python3
"""Ratchet `crates/er-quickload/src/experiments/**` and verify the critical caller ledger.

`er-quickload` is being extracted into crates until it is a thin shim that bundles them, so
the line count under `experiments/**` is a number that may shrink but must never grow. The
ledger row in the roadmap is the high-water mark; this gate fails when measured source has
climbed past it.

It is a ratchet, not a freeze. Edits are free -- a bug fix that rewrites 300 lines in place
passes untouched, and so does a refactor that moves lines between files. Only net growth in
the total is refused, and `--refresh` accepts growth in one command. The value is not that
growth is impossible; it is that growth becomes a reviewable diff to the ledger instead of
the invisible default.

Why growth, not equality: measured across the four commits of PR #367, 62% of 1,553 added
lines already landed in extracted crates with no enforcement at all, because the host-seam
pattern pulls them there. The leak that pattern does not catch is a new module born inside
the shim -- `experiments/continue_load/picked_summary_refresh.rs` arrived as 187 fresh lines
with no reason to start there, while a same-sized chunk of the same PR correctly landed in
`er-save-redirect`. (That file is gone: this gate named it, and it was extracted into
`crates/er-profile-summary-core` with the rest of the ProfileSummary concept. It stays quoted
here because it is the shape of leak the ratchet exists for, not because it still exists.)
Most of the remaining 595 shim lines were genuinely mandatory (telemetry emission,
`product_continue` spine wiring, `path_hooks` seams), so a blanket freeze would have blocked
real work for no gain.
"""

from __future__ import annotations

import argparse
import contextlib
import os
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPERIMENTS = ROOT / "crates/er-quickload/src/experiments"
SOURCE = ROOT / "crates/er-quickload/src"
ROADMAP = ROOT / "docs/plans/crate-extraction-execution-roadmap.md"
ROW = re.compile(r"^\| `([^`]+\.rs)` \| ([0-9,]+) \|", re.MULTILINE)
TOTAL = re.compile(r"^\| all `experiments/\*\*` \| ([0-9,]+) \| ([0-9,]+) \|$", re.MULTILINE)
FORBIDDEN_OWNERSHIP = re.compile(
    r"UNANALYSED|UNANALYZED|UNCLASSIFIED|APPROXIMATE(?:LY)?|ESTIMAT(?:E|ED|ES)|~",
    re.IGNORECASE,
)
# Matched by pattern, not by literal text, because the heading carries the file count and the
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


GROWTH_REMEDY = """\
Do ONE of these:
  1. MOVE THE NEW CODE INTO A CRATE under `crates/` and leave only the seam behind. This is
     the default answer for a whole new module: it had no reason to be born in the shim.
  2. If the growth is genuinely shim-only wiring that CANNOT live outside the DLL crate
     (telemetry emission, a hook seam, spine wiring), accept it consciously in one command:
         python3 scripts/check-crate-extraction-roadmap.py --refresh
     and say in the PR why it had to land here. That rewrites the ledger, so the growth
     shows up as a reviewable diff instead of the invisible default.
A NEW FILE additionally needs a hand-written ledger row naming its owner crate / R-number
before `--refresh` will pass -- this tool reports such a file rather than inventing one,
because inventing the row would mean inventing the ownership claim next to it."""


def ratchet(
    listed: dict[str, int],
    current: dict[str, int],
    expected_total: tuple[int, int] | None,
    ledger_count: int,
    text: str,
) -> tuple[list[str], list[str]]:
    """Compare measured `experiments/**` against the ledger as a ratchet, not an equality.

    Returns `(errors, notes)`. Notes are printed and do not fail the gate.

    Hard FAILURES:
      * total measured lines exceed the ledger total (the ratchet itself);
      * a file under `experiments/**` with no ledger row (a module born in the shim carrying
        no ownership claim -- the exact leak this gate exists for, and the one case the line
        total can miss when a large extraction has left slack);
      * the roadmap's own integrity: a missing total row, a duplicated ledger heading, a
        non-exact ownership marker, a missing partition term.

    Deliberately not failures, because they are edits and extraction rather than growth:
      * per-file line drift in either direction -- a 300-line in-place bug fix must pass;
      * a file shrinking, or the total shrinking (that is the point of the whole refactor);
      * a ledger row whose file has left `experiments/**` (it was extracted; `--refresh`
        drops the row).
    """
    errors: list[str] = []
    notes: list[str] = []
    grew_or_leaked = False

    current_lines = sum(current.values())
    unlisted = sorted(current.keys() - listed.keys())
    stale = sorted(listed.keys() - current.keys())

    if expected_total is None:
        errors.append("current measured-state total row is missing")
    else:
        ledger_files, ledger_lines = expected_total
        if current_lines > ledger_lines:
            grew_or_leaked = True
            grew = sorted(
                (current[path] - listed.get(path, 0), path)
                for path in current
                if current[path] > listed.get(path, 0)
            )
            detail = "\n".join(
                f"  +{delta} {path}{' (NEW FILE -- no ledger row)' if path in set(unlisted) else ''}"
                for delta, path in reversed(grew)
            )
            errors.append(
                f"experiments/** GREW past the ledger: {ledger_lines:,} -> {current_lines:,} lines "
                f"(+{current_lines - ledger_lines:,}), {ledger_files} -> {len(current)} files.\n"
                "er-quickload is being extracted INTO crates, so this number may shrink but "
                f"never grow.\n{detail}"
            )
        elif current_lines < ledger_lines:
            notes.append(
                f"experiments/** is {ledger_lines - current_lines:,} lines BELOW the ledger "
                f"({ledger_lines:,} -> {current_lines:,}); allowed, and the point of the refactor. "
                "Run --refresh to tighten the ratchet onto the new floor -- a baseline that is "
                "never lowered after an extraction silently permits regrowth back up to it."
            )
    if unlisted:
        grew_or_leaked = True
        errors.append("born in the shim with no ledger row: " + ", ".join(unlisted))
    if grew_or_leaked:
        # Both failures above have the same two remedies. Print them once, at the end, so a
        # gate message a reader has to act on does not open with the same block twice.
        errors.append(GROWTH_REMEDY)
    if stale:
        notes.append(
            "ledger rows whose file has left experiments/**: "
            + ", ".join(stale)
            + " (extraction; --refresh drops them)"
        )
    if ledger_count != 1:
        errors.append("roadmap must contain exactly one R1 current partition and caller ledger")
    if match := FORBIDDEN_OWNERSHIP.search(text):
        errors.append(f"forbidden non-exact ownership marker: {match.group(0)!r}")
    for term in sorted(REQUIRED_TERMS):
        if term not in text:
            errors.append(f"roadmap missing required partition term: {term}")
    return errors, notes


def validate_caller_edges() -> list[str]:
    errors: list[str] = []
    for function, callers in REQUIRED_EDGES.items():
        if not source_has_function(function):
            errors.append(f"required partition function missing from source: {function}")
        for caller in sorted(callers):
            if not source_has_call(caller, function):
                errors.append(f"required caller edge missing: {caller} -> {function}")
    return errors


def _fixture_roadmap(rows: dict[str, int], total: tuple[int, int] | None = None) -> str:
    """Render a minimal roadmap that the real parsers accept, for selftest fixtures.

    The selftest must never read the live tree's numbers: a gate whose own proof moves with
    the thing it measures proves nothing, and every one of these scenarios needs a tree that
    has grown or shrunk relative to its ledger -- states the real tree is not in.
    """
    files, lines = total if total is not None else (len(rows), sum(rows.values()))
    body = "\n".join(f"| `{path}` | {count:,} | R1 owns it |" for path, count in sorted(rows.items()))
    return (
        f"## Appendix A -- R1 current {len(rows)}-file partition and caller ledger\n"
        + "\n".join(sorted(REQUIRED_TERMS))
        + "\n\n| file | lines | owner |\n|---|---|---|\n"
        + f"{body}\n"
        + f"| all `experiments/**` | {files:,} | {lines:,} |\n"
    )


def _parse_ledger(text: str) -> tuple[dict[str, int], tuple[int, int] | None, int]:
    listed = {path: int(count.replace(",", "")) for path, count in ROW.findall(text)}
    total = TOTAL.search(text)
    expected = (
        (int(total.group(1).replace(",", "")), int(total.group(2).replace(",", "")))
        if total is not None
        else None
    )
    return listed, expected, len(LEDGER_HEADING.findall(text))


def _run_fixture(text: str, current: dict[str, int]) -> list[str]:
    listed, expected, headings = _parse_ledger(text)
    errors, _ = ratchet(listed, current, expected, headings, text)
    return errors


def _refresh_fixture(rows: dict[str, int], tree: dict[str, int]) -> tuple[int, list[str]]:
    """Materialise a real tree + roadmap on disk, run `--refresh`, then re-run the ratchet."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "experiments"
        for path, count in tree.items():
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("// line\n" * count, encoding="utf-8")
        roadmap = Path(tmp) / "roadmap.md"
        roadmap.write_text(_fixture_roadmap(rows), encoding="utf-8")
        # refresh() reports on stdout/stderr for the human running it; a fixture's chatter
        # interleaved with the selftest verdict reads like the selftest itself failed.
        with open(os.devnull, "w", encoding="utf-8") as sink:
            with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
                code = refresh(roadmap, root)
        return code, _run_fixture(roadmap.read_text(encoding="utf-8"), current_inventory(root))


def selftest() -> int:
    ledger = {"a.rs": 2, "b.rs": 3}
    failures: list[str] = []

    # (label, ledger rows, measured tree, ledger total override, expect failure)
    cases: list[tuple[str, dict[str, int], dict[str, int], tuple[int, int] | None, bool]] = [
        # The ratchet itself.
        ("exact match passes", ledger, dict(ledger), None, False),
        ("growth fails", ledger, {"a.rs": 2, "b.rs": 4}, None, True),
        ("shrink passes", ledger, {"a.rs": 1, "b.rs": 3}, None, False),
        ("file removed (extracted) passes", ledger, {"a.rs": 2}, None, False),
        # An edit is not growth: lines move between files, the total is unchanged.
        ("net-neutral reshuffle passes", ledger, {"a.rs": 4, "b.rs": 1}, None, False),
        # A stale-high ledger leaves slack. Growth into that slack is allowed by design; the
        # note telling you to re-baseline is what stops the slack becoming permanent.
        ("growth within slack passes", ledger, {"a.rs": 4, "b.rs": 4}, (2, 9), False),
        ("growth past a slack ledger fails", ledger, {"a.rs": 5, "b.rs": 5}, (2, 9), True),
        # A new module born in the SHIM -- the leak this gate exists for. It fails twice: on
        # the total, and on having no ownership row. The second one still fires under slack.
        ("new file fails", ledger, {**ledger, "c.rs": 1}, None, True),
        ("new file under slack still fails", ledger, {"a.rs": 1, "b.rs": 1, "c.rs": 1}, (2, 9), True),
    ]
    for label, rows, tree, total, want_failure in cases:
        got = _run_fixture(_fixture_roadmap(rows, total), tree)
        if bool(got) != want_failure:
            failures.append(f"{label}: expected {'failure' if want_failure else 'pass'}, got {got}")

    # Roadmap integrity, unchanged from the pre-ratchet gate.
    for label, text in (
        ("missing total", TOTAL.sub("", _fixture_roadmap(ledger))),
        ("duplicate ledger", _fixture_roadmap(ledger) * 2),
        ("forbidden marker", _fixture_roadmap(ledger).replace("R1 owns it", "APPROXIMATELY R1")),
        ("missing partition term", _fixture_roadmap(ledger).replace("S10 lifecycle", "")),
    ):
        if not _run_fixture(text, dict(ledger)):
            failures.append(f"{label}: expected failure, got pass")

    # `--refresh` is the escape hatch, so prove it actually re-opens a gate it closed.
    grown = {"a.rs": 9, "b.rs": 9}
    if not _run_fixture(_fixture_roadmap(ledger), grown):
        failures.append("refresh precondition: grown tree should fail before refresh")
    code, after = _refresh_fixture(ledger, grown)
    if code != 0 or after:
        failures.append(f"refresh accepts growth: expected clean pass, got exit={code} errors={after}")
    # ...and prove it does not paper over a new file: refresh refuses to invent the ownership
    # row, so the born-in-the-shim failure survives the escape hatch.
    code, after = _refresh_fixture(ledger, {**ledger, "c.rs": 1})
    if code == 0 or not after:
        failures.append(f"refresh must not invent a row for a new file: exit={code} errors={after}")

    if failures:
        for failure in failures:
            print(f"[check-crate-extraction-roadmap] selftest FAILED: {failure}", file=sys.stderr)
        return 1
    print(f"[check-crate-extraction-roadmap] selftest ok ({len(cases) + 7} cases)")
    return 0


def refresh(roadmap: Path = ROADMAP, root: Path = EXPERIMENTS) -> int:
    """Rewrite the ledger's line counts and total from measured source.

    This is the ratchet's deliberate escape hatch and must stay trivially runnable: growth
    that genuinely has to land in the shim is accepted in one command, and lowering the
    baseline after an extraction is the same command. What it buys is that either decision
    lands as a diff a reviewer can see.

    The ledger is a measured mirror of `experiments/**`, not a plan of intent -- a row exists
    if and only if the file exists, with an exact line count, and the total row must match
    (file count, summed lines). So there is no judgement to apply and the refresh is mechanical,
    which is exactly why it belongs in the tool rather than in an ad-hoc script written from
    scratch each time. It had been hand-refreshed at least three times (PR #301 and twice during
    the 2026-08-21 lint-parity sweep, where any edit that adds or removes a line silently rots
    the gate) before this mode existed.

    Only the count column is rewritten. The description and R-number columns are authored
    judgement and are preserved verbatim; a row whose file has left `experiments/**` is dropped,
    and a file with no row is reported rather than invented, because inventing one would mean
    inventing the ownership claim next to it.
    """
    text = roadmap.read_text(encoding="utf-8")
    inventory = current_inventory(root)
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
    roadmap.write_text(text, encoding="utf-8")

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
        # `--write` is the name PR #358 gave this mode while the ratchet was being built on an
        # older copy of this file; the merge of the two kept `--refresh` as canonical. The alias
        # costs one line and means a command someone already has in their shell history does not
        # silently become an argparse error.
        "--write",
        action="store_true",
        dest="refresh",
        help="rewrite the ledger line counts + total from measured source (mechanical)",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.refresh:
        return refresh()

    text = ROADMAP.read_text(encoding="utf-8")
    listed, expected_total, headings = _parse_ledger(text)
    current = current_inventory()
    errors, notes = ratchet(listed, current, expected_total, headings, text)
    errors.extend(validate_caller_edges())
    for note in notes:
        print(f"[check-crate-extraction-roadmap] note: {note}")
    if errors:
        for error in errors:
            print(f"[check-crate-extraction-roadmap] ERROR: {error}", file=sys.stderr)
        return 1
    ceiling = expected_total[1] if expected_total is not None else sum(current.values())
    print(
        "[check-crate-extraction-roadmap] ok -- experiments/** is "
        f"{len(current)} files / {sum(current.values()):,} lines, at or under the "
        f"{ceiling:,}-line ledger ceiling; critical caller edges match"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
